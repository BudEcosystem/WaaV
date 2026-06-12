//! The single streaming resampler for audio rate conversion
//! (PIPECAT_FIX_PLAN C-G5).
//!
//! One implementation for every path that converts sample rates — ingress
//! decode (wire rate → the 16 kHz the VAD/smart-turn models require) and TTS
//! egress (provider rate → client playback rate) — so rate conversion can
//! never diverge per call site again.
//!
//! Properties (soxr-stream parity via rubato's `FftFixedIn`):
//! - **Filter history across chunks**: the sinc delay line persists between
//!   calls, so chunk boundaries produce no clicks (the canonical stateless-
//!   per-chunk failure).
//! - **Lazy init**: the resampler is constructed on the first call that
//!   actually needs work; identity calls never allocate it.
//! - **Stale-state clear**: a gap > [`CLEAR_AFTER`] since the last call resets
//!   the delay line, so a new utterance doesn't inherit the previous one's
//!   filter tail (another click source).
//! - **Identity fast path**: `in_rate == out_rate` returns `None` — the
//!   caller uses its input as-is, zero copies, zero state.
//!
//! NOT `Sync`: one instance per stream/direction (rubato's `process` needs
//! `&mut`); never share across concurrent streams.

use std::time::Instant;

use rubato::{FftFixedIn, Resampler};
use tracing::{debug, warn};

/// Drop stale filter state when this much wall time passed since the last
/// chunk (Pipecat `CLEAR_STREAM_AFTER_SECS` parity).
const CLEAR_AFTER: std::time::Duration = std::time::Duration::from_millis(200);

/// Fixed input chunk the resampler consumes per process call.
const CHUNK_FRAMES: usize = 1024;

/// Streaming mono f32 resampler. See the module docs for the contract.
pub struct StreamResampler {
    inner: Option<FftFixedIn<f32>>,
    in_rate: u32,
    out_rate: u32,
    last_call: Option<Instant>,
    /// Tail (< one chunk) carried between calls — continuous filter state.
    pending_in: Vec<f32>,
}

impl std::fmt::Debug for StreamResampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamResampler")
            .field("in_rate", &self.in_rate)
            .field("out_rate", &self.out_rate)
            .field("pending", &self.pending_in.len())
            .finish()
    }
}

impl Default for StreamResampler {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamResampler {
    pub fn new() -> Self {
        Self { inner: None, in_rate: 0, out_rate: 0, last_call: None, pending_in: Vec::new() }
    }

    /// Resample mono f32 samples. Returns `None` when `in_rate == out_rate` —
    /// the caller uses its input unchanged (zero-copy identity, the common
    /// case for 16 kHz clients). Otherwise returns the resampled samples;
    /// input shorter than the internal chunk is buffered and may yield an
    /// empty Vec until enough accumulates (continuous-stream semantics).
    pub fn resample(&mut self, input: &[f32], in_rate: u32, out_rate: u32) -> Option<Vec<f32>> {
        if in_rate == out_rate || in_rate == 0 || out_rate == 0 {
            return None;
        }
        self.ensure(in_rate, out_rate);
        self.maybe_clear_stale();
        self.last_call = Some(Instant::now());

        let resampler = self.inner.as_mut().expect("ensured above");
        self.pending_in.extend_from_slice(input);

        let mut out: Vec<f32> = Vec::new();
        let chunk = resampler.input_frames_next().max(1);
        while self.pending_in.len() >= chunk {
            let take: Vec<f32> = self.pending_in.drain(..chunk).collect();
            match resampler.process(&[take], None) {
                Ok(mut resampled) => {
                    if let Some(channel) = resampled.pop() {
                        out.extend_from_slice(&channel);
                    }
                }
                Err(e) => {
                    warn!(error = %e, "stream resample failed; passing chunk through unresampled");
                    // Conservative degradation: never drop audio silently.
                    return Some(input.to_vec());
                }
            }
        }
        Some(out)
    }

    /// Explicit utterance boundary (barge-in / context end): drop the filter
    /// state now instead of waiting out the stale-clear window.
    pub fn reset(&mut self) {
        if let Some(r) = self.inner.as_mut() {
            r.reset();
        }
        self.pending_in.clear();
        self.last_call = None;
    }

    fn ensure(&mut self, in_rate: u32, out_rate: u32) {
        if self.inner.is_some() && self.in_rate == in_rate && self.out_rate == out_rate {
            return;
        }
        if self.inner.is_some() {
            // Mid-stream rate change (e.g. a provider reconnect renegotiated):
            // rebuild — legitimate but worth a log line.
            debug!(
                from = format!("{}→{}", self.in_rate, self.out_rate),
                to = format!("{in_rate}→{out_rate}"),
                "stream resampler rate change; rebuilding"
            );
        }
        // FftFixedIn: FFT-based polyphase — the same engine the smart-turn
        // mel extractor uses (Send-friendly, unlike SincFixedIn's boxed
        // interpolator), with quality well above voice requirements.
        match FftFixedIn::<f32>::new(in_rate as usize, out_rate as usize, CHUNK_FRAMES, 2, 1) {
            Ok(r) => {
                self.inner = Some(r);
                self.in_rate = in_rate;
                self.out_rate = out_rate;
                self.pending_in.clear();
            }
            Err(e) => {
                warn!(error = %e, in_rate, out_rate, "failed to build sinc resampler");
                self.inner = None;
            }
        }
    }

    fn maybe_clear_stale(&mut self) {
        if let (Some(last), Some(r)) = (self.last_call, self.inner.as_mut())
            && last.elapsed() > CLEAR_AFTER
        {
            r.reset();
            self.pending_in.clear();
        }
    }
}

/// PCM16-LE convenience for the TTS egress path (`AudioData.data` is bytes).
/// Returns `None` when no conversion is needed (use the original bytes).
pub fn resample_pcm16(
    r: &mut StreamResampler,
    pcm: &[u8],
    in_rate: u32,
    out_rate: u32,
) -> Option<Vec<u8>> {
    if in_rate == out_rate || in_rate == 0 || out_rate == 0 {
        return None;
    }
    let samples: Vec<f32> = pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect();
    let out = r.resample(&samples, in_rate, out_rate)?;
    let mut bytes = Vec::with_capacity(out.len() * 2);
    for &s in &out {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, freq_norm: f32) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * freq_norm).sin() * 0.8).collect()
    }

    #[test]
    fn identity_returns_none_zero_work() {
        let mut r = StreamResampler::new();
        assert!(r.resample(&[0.1, -0.2, 0.3], 16000, 16000).is_none());
        assert!(r.inner.is_none(), "identity must not even build the resampler");
    }

    #[test]
    fn downsample_48k_to_16k_length_ratio() {
        let mut r = StreamResampler::new();
        let input = sine(48_000, 0.05); // 1s at 48k
        let out = r.resample(&input, 48000, 16000).unwrap();
        // ~16000 samples, minus the < one-chunk tail still buffered.
        let expected = 16_000.0;
        assert!(
            (out.len() as f32 - expected).abs() < 1500.0,
            "length {} far from expected ~{expected}",
            out.len()
        );
    }

    #[test]
    fn chunked_stream_has_no_boundary_click() {
        // Resample one continuous sine in two calls: the seam must be smooth
        // (filter history carries across the boundary).
        let input = sine(9600, 0.05);
        let mut r = StreamResampler::new();
        let mut joined = r.resample(&input[..4800], 48000, 16000).unwrap();
        joined.extend(r.resample(&input[4800..], 48000, 16000).unwrap());
        let max_step = joined.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(max_step < 0.3, "seam discontinuity (click): step {max_step}");
    }

    #[test]
    fn stale_state_cleared_after_gap() {
        let mut r = StreamResampler::new();
        let loud = sine(4800, 0.3);
        let _ = r.resample(&loud, 48000, 16000);
        // Simulate the inter-utterance gap.
        r.last_call = Some(Instant::now() - std::time::Duration::from_millis(400));
        let silence = vec![0.0f32; 4800];
        let out = r.resample(&silence, 48000, 16000).unwrap();
        let max_abs = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(max_abs < 1e-3, "filter tail leaked across the gap: {max_abs}");
    }

    #[test]
    fn explicit_reset_drops_pending() {
        let mut r = StreamResampler::new();
        let _ = r.resample(&sine(100, 0.05), 48000, 16000); // < chunk → all pending
        assert!(!r.pending_in.is_empty());
        r.reset();
        assert!(r.pending_in.is_empty());
    }

    #[test]
    fn rate_change_rebuilds() {
        let mut r = StreamResampler::new();
        let _ = r.resample(&sine(2048, 0.05), 48000, 16000);
        let out = r.resample(&sine(2048, 0.05), 24000, 16000);
        assert!(out.is_some(), "rate change must rebuild, not fail");
        assert_eq!(r.in_rate, 24000);
    }

    #[test]
    fn pcm16_roundtrip_even_and_scaled() {
        let mut r = StreamResampler::new();
        let pcm: Vec<u8> = sine(4800, 0.05)
            .iter()
            .flat_map(|s| ((s * 32767.0) as i16).to_le_bytes())
            .collect();
        assert!(resample_pcm16(&mut r, &pcm, 24000, 24000).is_none(), "identity → None");
        let out = resample_pcm16(&mut r, &pcm, 24000, 16000).unwrap();
        assert_eq!(out.len() % 2, 0, "whole samples only");
        assert!(!out.is_empty());
    }
}

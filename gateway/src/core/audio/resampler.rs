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

/// Input chunk the resampler consumes per process call: ~20ms at the input
/// rate (review wf_85659e16 #9 — a fixed 1024 frames meant 8 kHz telephony
/// ingress gathered 128ms before the VAD/smart-turn models saw ANY of it;
/// 20ms keeps decision latency in line with the 12ms inference budget).
fn chunk_frames_for(in_rate: u32) -> usize {
    ((in_rate as usize) / 50).clamp(64, 1024)
}

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
        Self {
            inner: None,
            in_rate: 0,
            out_rate: 0,
            last_call: None,
            pending_in: Vec::new(),
        }
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

        let Some(resampler) = self.inner.as_mut() else {
            // ensure() failed to build (absurd rate pair): degrade to
            // passthrough instead of panicking — never drop audio.
            return Some(input.to_vec());
        };
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

    /// Flush the buffered tail at an utterance boundary (review wf_85659e16
    /// #8/#11): the fixed-input-chunk resampler holds up to one chunk of
    /// input it has not emitted — without this, the END of every utterance
    /// (final stop consonants) is silently dropped. Pads the pending tail
    /// with zeros, runs it through (so the real audio clears the filter
    /// delay line), resets, and returns the emitted samples. `None` when
    /// nothing is pending.
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        let resampler = self.inner.as_mut()?;
        if self.pending_in.is_empty() {
            return None;
        }
        let chunk = resampler.input_frames_next().max(1);
        let mut tail = std::mem::take(&mut self.pending_in);
        tail.resize(chunk, 0.0);
        let out = match resampler.process(&[tail], None) {
            Ok(mut resampled) => resampled.pop().unwrap_or_default(),
            Err(e) => {
                warn!(error = %e, "stream resampler flush failed; tail dropped");
                Vec::new()
            }
        };
        // The utterance is over: a fresh start for the next one.
        resampler.reset();
        self.last_call = None;
        if out.is_empty() { None } else { Some(out) }
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
        match FftFixedIn::<f32>::new(
            in_rate as usize,
            out_rate as usize,
            chunk_frames_for(in_rate),
            2,
            1,
        ) {
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

fn f32_to_pcm16_le(sample: f32) -> [u8; 2] {
    let sample = if sample.is_finite() { sample } else { 0.0 };
    ((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes()
}

/// PCM16-LE convenience for the TTS egress path (`AudioData.data` is bytes).
/// Returns `None` when no conversion is needed (use the original bytes). Malformed
/// PCM16 returns `Some(Vec::new())` so callers do not pass invalid bytes through.
pub fn resample_pcm16(
    r: &mut StreamResampler,
    pcm: &[u8],
    in_rate: u32,
    out_rate: u32,
) -> Option<Vec<u8>> {
    if pcm.len() % 2 != 0 {
        warn!(
            bytes = pcm.len(),
            "malformed PCM16 egress chunk length; dropping chunk instead of truncating a partial sample"
        );
        r.reset();
        return Some(Vec::new());
    }
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
        bytes.extend_from_slice(&f32_to_pcm16_le(s));
    }
    Some(bytes)
}

/// C-G5 pt3: convert one egress TTS chunk to the CLIENT's playback rate.
///
/// The single standardized egress seam — every delivery path (simple
/// STT→TTS callback, DAG `audio_output` drain) calls this with the chunk's
/// own metadata. Returns `None` ⇒ deliver the ORIGINAL bytes unchanged:
/// - the chunk is not PCM-family (compressed containers can't be resampled
///   here; the client asked for a rate we can't honor on this format),
/// - the input rate is unknown (chunk undeclared AND no configured provider
///   rate — byte math would corrupt the audio),
/// - identity (input rate == target).
pub fn egress_to_client_rate(
    r: &mut StreamResampler,
    data: &[u8],
    format: &str,
    chunk_rate: u32,
    configured_provider_rate: u32,
    target_rate: u32,
) -> Option<Vec<u8>> {
    if !crate::core::tts::sniff::is_linear_pcm16(format) {
        // Compressed containers AND G.711 companded telephony formats pass
        // through untouched: mulaw/alaw bytes are 8-bit companded samples —
        // PCM16 byte math would deliver full-scale static (review
        // wf_85659e16 #6/#12). A real G.711 transcode is a separate feature.
        debug!(
            format,
            "egress resample skipped: non-linear-PCM16 passes through"
        );
        return None;
    }
    // The chunk's own declared rate wins; the session's configured provider
    // rate is the fallback for providers that don't stamp chunks.
    let in_rate = if chunk_rate != 0 {
        chunk_rate
    } else {
        configured_provider_rate
    };
    resample_pcm16(r, data, in_rate, target_rate)
}

/// Flush the egress seam's buffered tail as PCM16 bytes (utterance end).
pub fn flush_pcm16(r: &mut StreamResampler) -> Option<Vec<u8>> {
    let out = r.flush()?;
    let mut bytes = Vec::with_capacity(out.len() * 2);
    for &s in &out {
        bytes.extend_from_slice(&f32_to_pcm16_le(s));
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
        assert!(
            r.inner.is_none(),
            "identity must not even build the resampler"
        );
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
        let max_step = joined
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step < 0.3,
            "seam discontinuity (click): step {max_step}"
        );
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
        assert!(
            max_abs < 1e-3,
            "filter tail leaked across the gap: {max_abs}"
        );
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
        assert!(
            resample_pcm16(&mut r, &pcm, 24000, 24000).is_none(),
            "identity → None"
        );
        let out = resample_pcm16(&mut r, &pcm, 24000, 16000).unwrap();
        assert_eq!(out.len() % 2, 0, "whole samples only");
        assert!(!out.is_empty());
    }

    #[test]
    fn pcm16_resampler_rejects_odd_length_without_truncating_or_passthrough() {
        let mut r = StreamResampler::new();
        let _ = r.resample(&sine(10, 0.05), 24_000, 48_000);
        assert!(!r.pending_in.is_empty(), "test must seed pending state");

        let malformed = vec![0x01, 0x02, 0x03];
        let out = resample_pcm16(&mut r, &malformed, 24_000, 48_000)
            .expect("malformed PCM16 must not use None passthrough");
        assert!(out.is_empty(), "malformed chunk is dropped as a unit");
        assert!(
            r.pending_in.is_empty(),
            "malformed chunk must reset stale resampler state"
        );

        let mut identity = StreamResampler::new();
        let out = resample_pcm16(&mut identity, &malformed, 24_000, 24_000)
            .expect("identity malformed PCM16 must not pass through");
        assert!(out.is_empty());
    }

    #[test]
    fn pcm16_quantizer_silences_non_finite_samples_before_clamping() {
        let got: Vec<i16> = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.25]
            .into_iter()
            .map(|sample| i16::from_le_bytes(f32_to_pcm16_le(sample)))
            .collect();
        assert_eq!(got, vec![0, 0, 0, 8192]);
    }

    // --- C-G5 pt3: the standardized egress seam ---

    fn pcm16(samples: &[f32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|&s| ((s * 32767.0) as i16).to_le_bytes())
            .collect()
    }

    #[test]
    fn egress_passthrough_for_non_pcm() {
        let mut r = StreamResampler::new();
        let bytes = vec![0u8; 4000];
        assert!(
            egress_to_client_rate(&mut r, &bytes, "mp3", 24_000, 24_000, 48_000).is_none(),
            "compressed formats pass through untouched"
        );
    }

    #[test]
    fn egress_passthrough_for_g711_companded() {
        // mulaw/alaw bytes are 8-bit COMPANDED samples: PCM16 byte math
        // would deliver full-scale static (review wf_85659e16 #6/#12).
        let mut r = StreamResampler::new();
        let bytes = vec![0x7Fu8; 1600];
        for fmt in ["mulaw", "ulaw", "alaw"] {
            assert!(
                egress_to_client_rate(&mut r, &bytes, fmt, 8_000, 8_000, 16_000).is_none(),
                "{fmt} must pass through untouched"
            );
        }
    }

    #[test]
    fn flush_recovers_the_buffered_tail() {
        // Feed less than one chunk: nothing emitted; flush returns it.
        let mut r = StreamResampler::new();
        let input = sine(300, 0.05); // < 480-frame chunk @24k
        let out = r.resample(&input, 24_000, 48_000).unwrap();
        assert!(out.is_empty(), "sub-chunk input is buffered, not emitted");
        let tail = r.flush().expect("flush must emit the buffered tail");
        assert!(
            tail.len() >= 500,
            "tail must cover the ~600 output frames of real audio, got {}",
            tail.len()
        );
        // Flushed and reset: nothing pending.
        assert!(r.flush().is_none());
    }

    #[test]
    fn chunk_size_tracks_input_rate() {
        // ~20ms gather at every rate (8k telephony must not wait 128ms).
        assert_eq!(chunk_frames_for(8_000), 160);
        assert_eq!(chunk_frames_for(16_000), 320);
        assert_eq!(chunk_frames_for(48_000), 960);
        assert_eq!(chunk_frames_for(1_000), 64, "floor");
        assert_eq!(chunk_frames_for(96_000), 1024, "cap");
    }

    #[test]
    fn egress_identity_and_unknown_rate_pass_through() {
        let mut r = StreamResampler::new();
        let bytes = pcm16(&sine(2048, 0.05));
        // Identity: chunk already at the client rate.
        assert!(egress_to_client_rate(&mut r, &bytes, "pcm", 24_000, 0, 24_000).is_none());
        // Unknown input rate everywhere: passthrough, never corrupt.
        assert!(egress_to_client_rate(&mut r, &bytes, "pcm", 0, 0, 48_000).is_none());
    }

    #[test]
    fn egress_resamples_pcm_to_client_rate() {
        let mut r = StreamResampler::new();
        // 1s of 24k PCM16 → 48k: ~2x the samples (minus the filter tail).
        let bytes = pcm16(&sine(24_000, 0.05));
        let out = egress_to_client_rate(&mut r, &bytes, "linear16", 24_000, 0, 48_000)
            .expect("rate differs: must resample");
        let out_samples = out.len() / 2;
        assert!(
            (out_samples as f32 - 48_000.0).abs() < 4096.0,
            "expected ~48000 samples, got {out_samples}"
        );
    }

    #[test]
    fn egress_falls_back_to_configured_provider_rate() {
        let mut r = StreamResampler::new();
        // Chunk doesn't stamp its rate (0): the session's configured provider
        // rate (24k) is the input-rate fallback.
        let bytes = pcm16(&sine(24_000, 0.05));
        let out = egress_to_client_rate(&mut r, &bytes, "pcm", 0, 24_000, 16_000)
            .expect("configured fallback rate must enable the conversion");
        let out_samples = out.len() / 2;
        assert!(
            (out_samples as f32 - 16_000.0).abs() < 3000.0,
            "expected ~16000 samples, got {out_samples}"
        );
    }
}

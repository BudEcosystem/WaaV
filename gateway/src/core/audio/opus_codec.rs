//! D8 — the real opus encode/decode (feature `opus-codec`).
//!
//! Thin, allocation-light wrappers over the `opus` crate (→ vendored libopus). Only compiled under
//! `opus-codec`; the always-built negotiation layer ([`super::codec`]) decides whether these are
//! constructed at all, so a default build never references libopus.
//!
//! Framing contract (both directions): ONE opus packet per WS binary frame. Mono. The decoder
//! (uplink) turns each client packet into PCM16 @ the negotiated rate before STT; the encoder
//! (downlink) turns each normalized PCM16 egress chunk into one opus packet. Encoder/decoder are
//! continuous per-connection (opus is a streaming codec).

use super::codec::{nearest_opus_frame_ms, nearest_opus_rate};

/// Opus's maximum frame is 120 ms; at 48 kHz mono that's 5760 samples — the decode output ceiling.
const MAX_DECODE_SAMPLES: usize = 5760;
/// RFC 6716 caps an opus packet at 1275 bytes; allow headroom for the encoder's worst case.
const MAX_PACKET_BYTES: usize = 4000;
/// Default uplink/downlink target bitrate (bits/s) — 24 kbps is transparent-enough wideband voice.
pub const DEFAULT_OPUS_BITRATE: i32 = 24_000;

/// A codec error (construction, per-packet encode/decode failure, or PCM framing failure).
#[derive(Debug)]
pub enum OpusCodecError {
    Codec(opus::Error),
    Framing(String),
}

impl std::fmt::Display for OpusCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpusCodecError::Codec(e) => write!(f, "opus codec error: {e}"),
            OpusCodecError::Framing(e) => write!(f, "opus PCM framing error: {e}"),
        }
    }
}
impl std::error::Error for OpusCodecError {}
impl From<opus::Error> for OpusCodecError {
    fn from(e: opus::Error) -> Self {
        OpusCodecError::Codec(e)
    }
}

/// Streaming opus DECODER for the uplink: each client WS binary frame is one opus packet → PCM16.
pub struct OpusStreamDecoder {
    decoder: opus::Decoder,
    /// The opus output rate (one of {8,12,16,24,48}kHz); also the PCM16 rate handed to STT.
    rate: u32,
    /// Reusable decode scratch sized to the max opus frame (mono).
    out: Vec<i16>,
}

impl OpusStreamDecoder {
    /// Build a decoder whose PCM16 output is at the opus-nearest of `sample_rate` (mono). The caller
    /// should have negotiated `sample_rate` to an opus rate already; we snap defensively.
    pub fn new(sample_rate: u32) -> Result<Self, OpusCodecError> {
        let rate = nearest_opus_rate(sample_rate);
        let decoder = opus::Decoder::new(rate, opus::Channels::Mono)?;
        Ok(Self {
            decoder,
            rate,
            out: vec![0i16; MAX_DECODE_SAMPLES],
        })
    }

    /// The PCM16 sample rate this decoder emits.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Decode ONE opus packet to little-endian PCM16 bytes. Empty input → empty output (a no-op
    /// keepalive frame). A decode failure is returned so the caller can log + drop the frame
    /// (one bad packet never tears down the stream).
    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<u8>, OpusCodecError> {
        if packet.is_empty() {
            return Ok(Vec::new());
        }
        let n = self.decoder.decode(packet, &mut self.out, false)?;
        let mut bytes = Vec::with_capacity(n * 2);
        for &s in &self.out[..n] {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        Ok(bytes)
    }
}

/// Streaming opus ENCODER for the downlink: normalized PCM16 egress chunks → opus packets. Buffers
/// across calls so an arbitrary PCM byte-stream is re-framed to exact opus frames; a short tail at
/// utterance end is zero-padded by [`Self::flush`].
pub struct OpusStreamEncoder {
    encoder: opus::Encoder,
    /// Samples per opus frame = rate * frame_ms / 1000 (mono).
    samples_per_frame: usize,
    /// Accumulates PCM16 samples until a full frame is available.
    buf: Vec<i16>,
    /// Reusable encode output scratch.
    out: Vec<u8>,
}

impl OpusStreamEncoder {
    /// Build an encoder at the opus-nearest `sample_rate` with `frame_ms`-sized frames (snapped to a
    /// valid opus frame) and the given bitrate, optimized for low-latency voice (VOIP).
    pub fn new(sample_rate: u32, frame_ms: u32, bitrate: i32) -> Result<Self, OpusCodecError> {
        let rate = nearest_opus_rate(sample_rate);
        let frame_ms = nearest_opus_frame_ms(frame_ms);
        let mut encoder = opus::Encoder::new(rate, opus::Channels::Mono, opus::Application::Voip)?;
        // Best-effort bitrate cap; an unsupported value just leaves the encoder at its default.
        let _ = encoder.set_bitrate(opus::Bitrate::Bits(bitrate));
        let samples_per_frame = (rate as usize * frame_ms as usize) / 1000;
        Ok(Self {
            encoder,
            samples_per_frame,
            buf: Vec::with_capacity(samples_per_frame * 2),
            out: vec![0u8; MAX_PACKET_BYTES],
        })
    }

    /// The opus frame size in samples (mono).
    pub fn samples_per_frame(&self) -> usize {
        self.samples_per_frame
    }

    /// Feed little-endian PCM16 bytes; return zero or more opus packets — one per full frame now
    /// available. Any sub-frame tail is buffered for the next call (or [`Self::flush`]).
    pub fn push_pcm(&mut self, pcm: &[u8]) -> Result<Vec<Vec<u8>>, OpusCodecError> {
        if pcm.len() % 2 != 0 {
            self.reset_buffer();
            return Err(OpusCodecError::Framing(format!(
                "PCM16 input length must be a multiple of 2 bytes, got {}",
                pcm.len()
            )));
        }
        for ch in pcm.chunks_exact(2) {
            self.buf.push(i16::from_le_bytes([ch[0], ch[1]]));
        }
        let mut packets = Vec::new();
        while self.buf.len() >= self.samples_per_frame {
            let n = self
                .encoder
                .encode(&self.buf[..self.samples_per_frame], &mut self.out)?;
            packets.push(self.out[..n].to_vec());
            self.buf.drain(..self.samples_per_frame);
        }
        Ok(packets)
    }

    /// Drop any buffered sub-frame samples (barge-in / clear) so stale pre-interrupt audio never
    /// leaks into the next utterance. The opus encoder itself stays continuous (no reset).
    pub fn reset_buffer(&mut self) {
        self.buf.clear();
    }

    /// Emit any buffered tail as a final packet, zero-padding to a full frame (utterance end). A few
    /// ms of trailing silence is inaudible. Returns `None` when nothing is buffered.
    pub fn flush(&mut self) -> Result<Option<Vec<u8>>, OpusCodecError> {
        if self.buf.is_empty() {
            return Ok(None);
        }
        self.buf.resize(self.samples_per_frame, 0);
        let n = self
            .encoder
            .encode(&self.buf[..self.samples_per_frame], &mut self.out)?;
        self.buf.clear();
        Ok(Some(self.out[..n].to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// A `samples`-long 440 Hz sine at `rate`, as little-endian PCM16 bytes.
    fn tone_pcm(rate: u32, samples: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(samples * 2);
        for i in 0..samples {
            let s =
                (0.4 * (2.0 * PI * 440.0 * i as f32 / rate as f32).sin() * i16::MAX as f32) as i16;
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    fn rms_i16_le(bytes: &[u8]) -> f64 {
        if bytes.len() < 2 {
            return 0.0;
        }
        let n = bytes.len() / 2;
        let sum: f64 = bytes
            .chunks_exact(2)
            .map(|c| {
                let s = i16::from_le_bytes([c[0], c[1]]) as f64;
                s * s
            })
            .sum();
        (sum / n as f64).sqrt()
    }

    #[test]
    fn encode_then_decode_round_trips_length_and_energy() {
        let rate: u32 = 48000;
        let frame_ms: u32 = 20;
        let spf = (rate as usize) * (frame_ms as usize) / 1000; // 960
        let mut enc = OpusStreamEncoder::new(rate, frame_ms, DEFAULT_OPUS_BITRATE).unwrap();
        let mut dec = OpusStreamDecoder::new(rate).unwrap();
        assert_eq!(enc.samples_per_frame(), spf);
        assert_eq!(dec.rate(), rate);

        // Encode 10 frames of a tone; decode each packet back.
        let frames = 10;
        let pcm = tone_pcm(rate, spf * frames);
        let packets = enc.push_pcm(&pcm).unwrap();
        assert_eq!(packets.len(), frames, "one opus packet per 20ms frame");

        let mut decoded_samples = 0usize;
        let mut peak_rms = 0.0f64;
        for p in &packets {
            assert!(
                !p.is_empty() && p.len() < spf * 2,
                "packet should compress vs PCM"
            );
            let pcm_out = dec.decode_packet(p).unwrap();
            assert_eq!(
                pcm_out.len(),
                spf * 2,
                "each packet decodes to one PCM16 frame"
            );
            decoded_samples += pcm_out.len() / 2;
            peak_rms = peak_rms.max(rms_i16_le(&pcm_out));
        }
        assert_eq!(decoded_samples, spf * frames, "sample count preserved");
        // Opus is lossy + has warmup, but a steady tone must come back with real energy.
        let in_rms = rms_i16_le(&pcm);
        assert!(
            peak_rms > in_rms * 0.3,
            "decoded energy {peak_rms} vs input {in_rms}"
        );
    }

    #[test]
    fn empty_packet_decodes_to_nothing() {
        let mut dec = OpusStreamDecoder::new(16000).unwrap();
        assert!(dec.decode_packet(&[]).unwrap().is_empty());
    }

    #[test]
    fn push_buffers_partial_frame_then_flush_pads() {
        let rate: u32 = 24000;
        let frame_ms: u32 = 20;
        let spf = (rate as usize) * (frame_ms as usize) / 1000; // 480
        let mut enc = OpusStreamEncoder::new(rate, frame_ms, DEFAULT_OPUS_BITRATE).unwrap();

        // Feed HALF a frame — no packet yet, buffered.
        let half = tone_pcm(rate, spf / 2);
        assert!(
            enc.push_pcm(&half).unwrap().is_empty(),
            "half a frame emits nothing"
        );
        // Flush zero-pads the tail to one full frame → exactly one packet.
        let tail = enc.flush().unwrap();
        assert!(tail.is_some(), "flush emits the padded tail");
        // A second flush with an empty buffer emits nothing.
        assert!(enc.flush().unwrap().is_none());
    }

    #[test]
    fn push_rejects_odd_pcm16_without_truncating_or_flushing_stale_tail() {
        let rate: u32 = 24000;
        let frame_ms: u32 = 20;
        let mut enc = OpusStreamEncoder::new(rate, frame_ms, DEFAULT_OPUS_BITRATE).unwrap();

        let half = tone_pcm(rate, 240);
        assert!(enc.push_pcm(&half).unwrap().is_empty());
        assert!(!enc.buf.is_empty(), "test must seed a buffered tail");

        let err = enc
            .push_pcm(&[0x01, 0x02, 0x03])
            .expect_err("odd PCM16 input must fail closed");
        assert!(
            err.to_string().contains("multiple of 2"),
            "error should name PCM16 alignment, got {err}"
        );
        assert!(enc.buf.is_empty(), "malformed input clears stale PCM tail");
        assert!(
            enc.flush().unwrap().is_none(),
            "cleared stale tail must not be emitted after malformed input"
        );
    }

    #[test]
    fn rate_and_frame_are_snapped_to_opus_valid() {
        // 44100 → 48000, 15ms → 20ms.
        let enc = OpusStreamEncoder::new(44100, 15, DEFAULT_OPUS_BITRATE).unwrap();
        assert_eq!(enc.samples_per_frame(), 48000 * 20 / 1000);
        let dec = OpusStreamDecoder::new(44100).unwrap();
        assert_eq!(dec.rate(), 48000);
    }
}

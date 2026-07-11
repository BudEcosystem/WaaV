//! Output chunker for raw-WS audio egress (PIPECAT_FIX_PLAN E-G2).
//!
//! Raw-WS sessions send TTS audio VERBATIM — a 1.5s synthesis lands as one
//! WebSocket binary frame, and a barge-in clear cannot truncate a frame
//! already on the socket: the client plays out the whole residual. LiveKit
//! already re-frames internally to 10ms (its `clear_buffer` flushes the
//! queue), so this chunker is the WS-egress counterpart — slicing audio to
//! `audio_out_chunk_ms` so the barge-in residual is bounded by ONE chunk.
//!
//! Contract:
//! - **never drop or duplicate audio**: full chunks drain, the <1-chunk
//!   remainder carries to the next push (Pipecat's prefix-drop);
//! - **never split a PCM sample**: chunk size is forced even;
//! - **a clear drops the carried remainder** (it is stale bot audio).

use bytes::{Bytes, BytesMut};

/// Default chunk duration — tighter than Pipecat's 40ms default since WaaV
/// clears aggressively on barge-in.
pub const DEFAULT_AUDIO_OUT_CHUNK_MS: u32 = 20;

const MIN_PCM16_CHUNK_BYTES: usize = 2;
/// Maximum supported WS egress chunk: 1 second of 192 kHz mono PCM16.
const MAX_PCM16_CHUNK_BYTES: usize = 192_000 * 2;

/// Streaming re-framer. One per WS session egress.
#[derive(Debug)]
pub struct OutputChunker {
    buf: BytesMut,
    chunk_bytes: usize,
}

impl OutputChunker {
    /// `chunk_ms` at `sample_rate` (PCM16 mono). Chunk size is forced even
    /// (never split a sample) and at least 2 bytes.
    pub fn new(chunk_ms: u32, sample_rate: u32) -> Self {
        Self {
            buf: BytesMut::new(),
            chunk_bytes: pcm16_mono_chunk_bytes(chunk_ms, sample_rate),
        }
    }

    /// The configured chunk size in bytes.
    pub fn chunk_bytes(&self) -> usize {
        self.chunk_bytes
    }

    /// Push audio; returns ZERO OR MORE full chunks. The sub-chunk tail is
    /// carried for the next push.
    pub fn push(&mut self, data: &[u8]) -> Vec<Bytes> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::with_capacity(self.buf.len() / self.chunk_bytes);
        while self.buf.len() >= self.chunk_bytes {
            out.push(self.buf.split_to(self.chunk_bytes).freeze());
        }
        out
    }

    /// Emit the carried remainder (utterance end). `None` when empty.
    pub fn flush_remainder(&mut self) -> Option<Bytes> {
        if self.buf.is_empty() {
            None
        } else {
            Some(self.buf.split().freeze())
        }
    }

    /// Barge-in: the carried remainder is STALE bot audio — drop it.
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

fn pcm16_mono_chunk_bytes(chunk_ms: u32, sample_rate: u32) -> usize {
    let bytes = u128::from(sample_rate)
        .saturating_mul(2)
        .saturating_mul(u128::from(chunk_ms))
        / 1000;
    let bytes = usize::try_from(bytes)
        .unwrap_or(MAX_PCM16_CHUNK_BYTES)
        .clamp(MIN_PCM16_CHUNK_BYTES, MAX_PCM16_CHUNK_BYTES);
    bytes & !1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_full_chunks_and_carries_remainder() {
        // 32-byte chunks (1ms @16k): 125 bytes → three chunks + 29 carried.
        let mut c = OutputChunker::new(1, 16_000);
        assert_eq!(c.chunk_bytes(), 32);
        let chunks = c.push(&[0xAA; 125]);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|ch| ch.len() == 32));
        let rem = c.flush_remainder().expect("29 bytes carried");
        assert_eq!(rem.len(), 29);
        assert!(c.flush_remainder().is_none());
    }

    #[test]
    fn carry_concatenates_across_pushes_no_loss_no_dup() {
        let mut c = OutputChunker::new(1, 16_000); // 32B chunks
        let mut total_out = 0usize;
        // Stream 10 pushes of 13 bytes = 130 bytes total.
        for i in 0..10u8 {
            for ch in c.push(&[i; 13]) {
                total_out += ch.len();
            }
        }
        if let Some(rem) = c.flush_remainder() {
            total_out += rem.len();
        }
        assert_eq!(total_out, 130, "Σ emitted == Σ input (no drop, no dup)");
    }

    #[test]
    fn twenty_ms_at_16k_is_640_bytes() {
        let mut c = OutputChunker::new(20, 16_000);
        assert_eq!(c.chunk_bytes(), 640);
        let chunks = c.push(&[0u8; 1024]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 640);
        // 384 carried; a barge-in clear DROPS it (stale bot audio).
        c.clear();
        assert!(c.flush_remainder().is_none(), "clear drops the carry");
    }

    #[test]
    fn chunk_size_never_odd() {
        // 1ms @ 8001 Hz would be 16.002 bytes → must round to EVEN.
        let c = OutputChunker::new(1, 8_001);
        assert_eq!(c.chunk_bytes() % 2, 0, "never split a PCM16 sample");
        let c = OutputChunker::new(0, 16_000);
        assert!(c.chunk_bytes() >= 2, "floor at one sample");
    }

    #[test]
    fn chunk_size_caps_pathological_inputs_without_overflow() {
        let c = OutputChunker::new(u32::MAX, u32::MAX);
        assert_eq!(c.chunk_bytes(), MAX_PCM16_CHUNK_BYTES);
        assert_eq!(c.chunk_bytes() % 2, 0);
    }
}

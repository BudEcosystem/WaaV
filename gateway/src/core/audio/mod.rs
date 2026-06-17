//! Audio processing utilities for voice activity detection and analysis.
//!
//! This module provides:
//! - [`VADAnalyzer`] - Voice Activity Detection state machine with configurable debouncing
//! - [`VADParams`] - Configuration for VAD behavior
//! - [`VADState`] - Current state of the VAD analyzer
//! - [`AudioRingBuffer`] - Pre-allocated ring buffer for audio samples

pub mod ring_buffer;
// D8 transport-codec negotiation (linear16 | opus) — ALWAYS built. The negotiation/degrade layer
// is libopus-free; the real encode/decode lives under the `opus-codec` feature (`opus_codec`).
pub mod codec;
#[cfg(feature = "opus-codec")]
pub mod opus_codec;
// The single streaming resampler (C-G5) — ALWAYS built: TTS egress
// `client_playback_rate` is part of the session API in every build.
pub mod output_chunker;
pub mod playback_pump;
pub mod playback_queue;
pub mod resampler;
pub mod vad;

pub use codec::{AudioCodec, CodecNegotiation, negotiate as negotiate_codec};
pub use ring_buffer::AudioRingBuffer;
pub use output_chunker::{DEFAULT_AUDIO_OUT_CHUNK_MS, OutputChunker};
pub use playback_pump::{DEFAULT_PLAYBACK_LEAD, PlaybackEnqueuer, PlaybackPump, PlaybackSink};
pub use playback_queue::{PlaybackQueue, PlaybackUnit};
pub use resampler::{StreamResampler, egress_to_client_rate, resample_pcm16};
pub use vad::{VADAnalyzer, VADParams, VADState, VADTransition};

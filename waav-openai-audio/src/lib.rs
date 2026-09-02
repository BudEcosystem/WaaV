//! The OpenAI-compatible audio surface (FRD-018 §9.2, T3.7).
//!
//! Bud registers a WaaV voice endpoint like any other model, so callers reach it through the
//! same three paths OpenAI defines:
//!
//! * `POST /v1/audio/speech` — synthesis, binary audio out
//! * `POST /v1/audio/transcriptions` — multipart in, text out
//! * `POST /v1/audio/translations` — the same, translated to English
//!
//! This crate is the **translation layer only**: OpenAI request shapes in, WaaV provider
//! settings out. It performs no I/O, which is why it can be tested exhaustively in
//! milliseconds — the mapping is where the fiddly, vendor-specific mistakes live, not in the
//! HTTP plumbing around it.
//!
//! Two rules run through it:
//!
//! 1. **Never silently substitute.** An unsupported format or an out-of-range speed is an
//!    error naming the field, not a quiet fallback to a default — a caller who asked for FLAC
//!    and got MP3 has no way to find out.
//! 2. **Pass vendor voices through untouched.** OpenAI's eleven voice names are mapped where a
//!    vendor has an equivalent; anything else is forwarded verbatim, because `aura-asteria-en`
//!    is a perfectly good voice name and rejecting it would make the vendor's own catalog
//!    unreachable.

pub mod speech;
pub mod transcription;

pub use speech::{AudioFormat, SpeechRequest, SpeechSettings};
pub use transcription::{TranscriptionRequest, TranscriptionResponseFormat, TranscriptionSettings};

/// Why a request could not be translated.
///
/// Every variant names the offending field: these surface to an API caller who can only fix
/// what they can identify.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    #[error("`{field}` is required")]
    Missing { field: &'static str },

    #[error("`{field}` value {value:?} is not supported; expected one of {expected}")]
    Unsupported {
        field: &'static str,
        value: String,
        expected: String,
    },

    #[error("`{field}` must be between {min} and {max}, got {value}")]
    OutOfRange {
        field: &'static str,
        value: String,
        min: String,
        max: String,
    },

    #[error("`{field}` exceeds the {limit} limit ({actual})")]
    TooLarge {
        field: &'static str,
        limit: String,
        actual: String,
    },
}

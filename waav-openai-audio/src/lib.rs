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

pub mod pcm;
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

    /// The field is present and well-formed as a value, but its CONTENT cannot be used --
    /// an upload whose container this gateway cannot decode, say. Distinct from `Unsupported`,
    /// which enumerates a closed set of accepted values; here the reason is specific to the
    /// payload and carries its own remedy.
    #[error("`{field}` cannot be used: {reason}")]
    InvalidField { field: &'static str, reason: String },
}

/// Whether an optional identity field is worth recording onto a span.
///
/// `Some("")` must NOT be recorded. An empty string is not NULL: it makes a column look
/// populated to any check that asks "did a value ever arrive", while carrying no attribution.
/// FRD-018 has already produced that failure three times — a declaration satisfying every
/// guard while nothing was written — so the empty case is filtered in one place rather than at
/// each call site that would have to remember.
pub fn recordable(value: Option<&str>) -> Option<&str> {
    match value {
        Some(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod recordable_tests {
    use super::recordable;

    #[test]
    fn a_real_value_is_recorded() {
        assert_eq!(recordable(Some("proj-123")), Some("proj-123"));
    }

    #[test]
    fn absent_stays_absent() {
        assert_eq!(recordable(None), None);
    }

    #[test]
    fn an_empty_string_is_not_a_value() {
        // The trap: "" is not NULL. Recording it turns "this column is never written" — the
        // question verify_live.sh now asks — into a false pass, hiding missing attribution
        // instead of reporting it.
        assert_eq!(recordable(Some("")), None);
        assert_eq!(recordable(Some("   ")), None);
    }
}

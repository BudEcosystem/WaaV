//! The transcriber layer: one interface for "transcribe this utterance", one implementation per
//! request format, and one loop that sends at most two requests per utterance.
//!
//! | Concern | Owner |
//! | --- | --- |
//! | One request in one vendor's dialect | a [`SegmentTranscriber`] in [`wire`] (or a commit socket) |
//! | Whether and when a second request is sent, the breaker, the limiter, the deadline | [`attempts::SegmentAttempts`] |
//! | Request rate and concurrency per vendor host, credential and model | [`gate::Limiter`] |
//! | Failing fast while a vendor is down | [`breaker::FileBreaker`] |
//! | Dropping invented text | [`quality`] |

pub mod attempts;
pub mod breaker;
pub mod gate;
pub mod http;
pub mod quality;
pub mod testing;
pub mod wire;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::types::ErrorClass;

/// One upload unit's audio: 16 kHz mono PCM, padding included.
#[derive(Debug, Clone)]
pub struct SegmentAudio {
    pub pcm: Arc<[i16]>,
}

impl SegmentAudio {
    pub fn new(pcm: Vec<i16>) -> Self {
        Self { pcm: pcm.into() }
    }

    pub fn audio_ms(&self) -> u32 {
        (self.pcm.len() as u64 * 1000 / 16_000) as u32
    }

    /// The unit as a 16 kHz mono 16-bit WAV file.
    pub fn wav(&self) -> Vec<u8> {
        crate::audio::wav_16k_mono(&self.pcm)
    }

    /// Raw little-endian PCM bytes.
    pub fn pcm_bytes(&self) -> Vec<u8> {
        self.pcm.iter().flat_map(|s| s.to_le_bytes()).collect()
    }
}

/// What travels with one request besides the audio.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SegmentContext {
    pub turn_id: u64,
    pub seq: u32,
    /// The session's language as the client named it (BCP-47 or ISO 639-1), when known.
    pub language: Option<String>,
    /// Candidate languages for rows that take a list (`gpt-transcribe` `languages[]`).
    pub candidate_languages: Vec<String>,
    /// A vocabulary prompt.
    pub prompt: Option<String>,
    /// Key terms.
    pub keywords: Vec<String>,
    /// Optional fields a repair removed (a 400 that named them).
    pub omit_fields: Vec<String>,
    /// Send only the file and the model (a repair after a refusal that named no field).
    pub minimal: bool,
}

/// What one request returned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SegmentTranscript {
    pub text: String,
    /// The confidence the vendor reported, and only that.
    pub vendor_confidence: Option<f32>,
    /// A confidence derived from log-probabilities (Whisper `avg_logprob`), comparable within one
    /// model only.
    pub derived_confidence: Option<f32>,
    pub detected_language: Option<String>,
    /// Whisper's segment-level signals, when the row asks for them.
    pub no_speech_prob: Option<f32>,
    pub avg_logprob: Option<f32>,
    pub compression_ratio: Option<f32>,
    /// The vendor said explicitly that there was no speech.
    pub vendor_said_no_speech: bool,
    pub vendor_request_id: Option<String>,
    /// What the vendor says it billed, when it says.
    pub billed_ms: Option<u32>,
}

/// How far a request got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPhase {
    /// Nothing reached the vendor.
    BeforeSend,
    /// Sent; no response headers yet.
    Sent,
    /// Headers arrived; the body was being read.
    Headers,
}

/// A failed request, classified by the code that saw the HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentError {
    pub class: ErrorClass,
    pub status: Option<u16>,
    /// The vendor's `Retry-After`.
    pub retry_after: Option<Duration>,
    pub message: String,
    pub phase: RequestPhase,
    /// A 400 or 422 that named an optional field.
    pub refused_field: Option<String>,
    /// A vendor "busy" code worth one more try.
    pub vendor_busy: bool,
}

impl SegmentError {
    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            status: None,
            retry_after: None,
            message: message.into(),
            phase: RequestPhase::Sent,
            refused_field: None,
            vendor_busy: false,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_phase(mut self, phase: RequestPhase) -> Self {
        self.phase = phase;
        self
    }

    pub fn with_retry_after(mut self, after: Option<Duration>) -> Self {
        self.retry_after = after;
        self
    }

    /// Classify an HTTP status.
    pub fn from_status(status: u16, message: impl Into<String>) -> Self {
        let class = match status {
            401 | 403 => ErrorClass::Auth,
            404 => ErrorClass::ModelNotServed,
            408 => ErrorClass::Timeout,
            429 => ErrorClass::RateLimited,
            400..=499 => ErrorClass::BadRequest,
            _ => ErrorClass::Vendor,
        };
        Self::new(class, message)
            .with_status(status)
            .with_phase(RequestPhase::Headers)
    }

    /// A fast failure worth one more request: a connection error before any response, 500, 502,
    /// 503, 504 or 408, a rate-limit answer, or a vendor busy code.
    pub fn is_fast_retryable(&self) -> bool {
        if self.vendor_busy {
            return true;
        }
        match self.class {
            ErrorClass::Network => self.phase != RequestPhase::Headers,
            ErrorClass::RateLimited => true,
            ErrorClass::Timeout => self.status == Some(408),
            ErrorClass::Vendor => matches!(self.status, Some(500 | 502 | 503 | 504)),
            _ => false,
        }
    }

    /// Whether the outcome counts against the vendor in the breaker. A rate limit, a refused
    /// request and a cancellation never do: they say nothing about the vendor's health.
    pub fn counts_for_breaker(&self) -> bool {
        matches!(
            self.class,
            ErrorClass::Vendor | ErrorClass::Network | ErrorClass::Timeout | ErrorClass::Protocol
        )
    }
}

impl std::fmt::Display for SegmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(s) => write!(f, "{} ({s}): {}", self.class.as_str(), self.message),
            None => write!(f, "{}: {}", self.class.as_str(), self.message),
        }
    }
}

/// File upload, or a vendor socket on which the gateway commits each utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriberKind {
    File,
    Commit,
}

/// What the attempt loop and the engine need to know about a target.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriberInfo {
    /// The map's adapter id (`openai_transcriptions`, `elevenlabs_batch`, …).
    pub adapter: String,
    /// Limiter, breaker and pool key: scheme, host and port of the vendor.
    pub host_key: String,
    pub model: String,
    pub min_audio_ms: Option<u32>,
    pub max_audio_ms: Option<u32>,
    pub max_upload_bytes: Option<u64>,
    /// A server that runs one request at a time: no second request at a stall.
    pub single_process_server: bool,
    pub kind: TranscriberKind,
    /// Optional request fields the repair may drop.
    pub droppable_fields: Vec<String>,
}

impl TranscriberInfo {
    pub fn file(adapter: &str, host_key: &str, model: &str) -> Self {
        Self {
            adapter: adapter.to_string(),
            host_key: host_key.to_string(),
            model: model.to_string(),
            min_audio_ms: None,
            max_audio_ms: None,
            max_upload_bytes: None,
            single_process_server: false,
            kind: TranscriberKind::File,
            droppable_fields: Vec::new(),
        }
    }

    /// Whether a second request can be sent beside or after the first. A commit socket cannot:
    /// a second commit would commit the next utterance's audio, or an empty buffer that the vendor
    /// rejects (and the rejection would fail the first utterance). Its fallback, when the row has
    /// a file transport, is inside the commit transcriber.
    pub fn allows_second_request(&self) -> bool {
        self.kind == TranscriberKind::File && !self.single_process_server
    }
}

/// Timings of one request, filled by the adapter as the request progresses.
#[derive(Debug, Default)]
pub struct RequestProgress {
    started_ns: AtomicU64,
    headers_ns: AtomicU64,
}

impl RequestProgress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn now_ns() -> u64 {
        static START: std::sync::OnceLock<tokio::time::Instant> = std::sync::OnceLock::new();
        let start = *START.get_or_init(tokio::time::Instant::now);
        tokio::time::Instant::now()
            .saturating_duration_since(start)
            .as_nanos() as u64
            + 1
    }

    /// The first byte was written.
    pub fn mark_sent(&self) {
        let _ = self.started_ns.compare_exchange(
            0,
            Self::now_ns(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Response headers arrived.
    pub fn mark_headers(&self) {
        let _ = self.headers_ns.compare_exchange(
            0,
            Self::now_ns(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub fn headers_received(&self) -> bool {
        self.headers_ns.load(Ordering::Acquire) != 0
    }

    /// Time from the first byte to the headers, when both happened.
    pub fn time_to_headers(&self) -> Option<Duration> {
        let s = self.started_ns.load(Ordering::Acquire);
        let h = self.headers_ns.load(Ordering::Acquire);
        (s != 0 && h != 0).then(|| Duration::from_nanos(h.saturating_sub(s)))
    }
}

/// One request to transcribe one utterance. Implementations make exactly one vendor request per
/// call and never retry; the attempt loop owns every retry.
#[async_trait::async_trait]
pub trait SegmentTranscriber: Send + Sync {
    fn info(&self) -> &TranscriberInfo;

    /// `timeout` is this request's limit; the implementation must not run past it.
    async fn transcribe(
        &self,
        audio: &SegmentAudio,
        ctx: &SegmentContext,
        timeout: Duration,
        progress: &RequestProgress,
    ) -> Result<SegmentTranscript, SegmentError>;

    /// Open idle connections so the next upload does not pay DNS, TCP and TLS.
    async fn prewarm(&self, _connections: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_statuses_are_classified_once() {
        assert_eq!(SegmentError::from_status(401, "").class, ErrorClass::Auth);
        assert_eq!(SegmentError::from_status(403, "").class, ErrorClass::Auth);
        assert_eq!(
            SegmentError::from_status(404, "").class,
            ErrorClass::ModelNotServed
        );
        assert_eq!(
            SegmentError::from_status(429, "").class,
            ErrorClass::RateLimited
        );
        assert_eq!(
            SegmentError::from_status(422, "").class,
            ErrorClass::BadRequest
        );
        assert_eq!(SegmentError::from_status(503, "").class, ErrorClass::Vendor);
    }

    #[test]
    fn only_fast_failures_worth_repeating_are_retryable() {
        assert!(SegmentError::from_status(503, "").is_fast_retryable());
        assert!(SegmentError::from_status(408, "").is_fast_retryable());
        assert!(SegmentError::from_status(429, "").is_fast_retryable());
        assert!(!SegmentError::from_status(400, "").is_fast_retryable());
        assert!(!SegmentError::from_status(401, "").is_fast_retryable());
        assert!(!SegmentError::from_status(501, "").is_fast_retryable());
        let reset = SegmentError::new(ErrorClass::Network, "reset").with_phase(RequestPhase::Sent);
        assert!(reset.is_fast_retryable());
        let body = SegmentError::new(ErrorClass::Network, "body").with_phase(RequestPhase::Headers);
        assert!(!body.is_fast_retryable(), "the vendor did the work");
    }

    #[test]
    fn rate_limits_and_refusals_never_count_against_the_vendor() {
        assert!(!SegmentError::from_status(429, "").counts_for_breaker());
        assert!(!SegmentError::from_status(400, "").counts_for_breaker());
        assert!(!SegmentError::from_status(401, "").counts_for_breaker());
        assert!(SegmentError::from_status(500, "").counts_for_breaker());
        assert!(SegmentError::new(ErrorClass::Network, "").counts_for_breaker());
        assert!(!SegmentError::new(ErrorClass::Cancelled, "").counts_for_breaker());
    }

    #[test]
    fn a_commit_socket_never_gets_a_second_request() {
        let mut info = TranscriberInfo::file(
            "openai_realtime_transcription",
            "wss://api.openai.com",
            "gpt-live-transcribe",
        );
        info.kind = TranscriberKind::Commit;
        assert!(!info.allows_second_request());
        let file = TranscriberInfo::file(
            "openai_transcriptions",
            "https://api.openai.com",
            "gpt-transcribe",
        );
        assert!(file.allows_second_request());
        let single = TranscriberInfo {
            single_process_server: true,
            ..file
        };
        assert!(!single.allows_second_request());
    }

    #[test]
    fn segment_audio_reports_its_length_and_encodes_wav() {
        let a = SegmentAudio::new(vec![0; 16_000]);
        assert_eq!(a.audio_ms(), 1000);
        assert_eq!(a.wav().len(), 44 + 32_000);
        assert_eq!(a.pcm_bytes().len(), 32_000);
    }
}

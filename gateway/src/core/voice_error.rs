//! Why a voice call failed, as a closed vocabulary (FRD-021 §6.5).
//!
//! `error_type` is assigned where the vendor's numeric status is still known and carried to the
//! handler, rather than recovered from a message afterwards. Before this, the TTS path collapsed
//! every failure but a refusal into one string and the STT path mapped a 408 and a 429 to the same
//! variant, so "the vendor is rate-limiting us" and "the vendor timed out" were one number — and a
//! 32% failure rate read as a 100% success rate, because nothing on the span said "failed" in a
//! form the fact table could count.

use crate::core::stt::STTError;
use crate::core::tts::TTSError;

/// The closed set of FRD-021 §6.5. Adding a member is a contract change: budmetrics groups by it
/// and both UIs label it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoiceErrorType {
    /// Failed Bud-side validation after endpoint resolution. Caller.
    InvalidRequest,
    /// The uploaded audio could not be read. Caller.
    InputDecode,
    /// Vendor 4xx other than 401/402/403/408/429. Caller or operator.
    VendorRejected,
    /// Vendor 401/402/403. Operator.
    Auth,
    /// Vendor 429. Operator (capacity).
    RateLimited,
    /// Vendor 408, or the vendor request timed out. Operator.
    VendorTimeout,
    /// The deployment's own time limit elapsed. Operator.
    Deadline,
    /// Vendor 5xx. Vendor.
    Vendor5xx,
    /// Connect/send failure. Operator.
    Network,
    /// The deployment cannot be served as configured. Operator.
    Config,
    /// Anything else. Bud.
    Internal,
    /// Refused by an open circuit breaker: no vendor was called for this request, because the
    /// deployment's vendor failed recently (FRD-022 §6.5). Operator. Never carries a vendor status —
    /// there was no vendor response — and is not `vendor_5xx`, which would blame the vendor for a
    /// call it never received.
    CircuitOpen,
}

impl VoiceErrorType {
    pub const ALL: [Self; 12] = [
        Self::InvalidRequest,
        Self::InputDecode,
        Self::VendorRejected,
        Self::Auth,
        Self::RateLimited,
        Self::VendorTimeout,
        Self::Deadline,
        Self::Vendor5xx,
        Self::Network,
        Self::Config,
        Self::Internal,
        Self::CircuitOpen,
    ];

    /// The wire value, as budmetrics and both UIs read it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InputDecode => "input_decode",
            Self::VendorRejected => "vendor_rejected",
            Self::Auth => "auth",
            Self::RateLimited => "rate_limited",
            Self::VendorTimeout => "vendor_timeout",
            Self::Deadline => "deadline",
            Self::Vendor5xx => "vendor_5xx",
            Self::Network => "network",
            Self::Config => "config",
            Self::Internal => "internal",
            Self::CircuitOpen => "circuit_open",
        }
    }

    /// The class of a vendor's non-success HTTP status.
    ///
    /// Purely numeric on purpose. The error the caller is shown may differ — a 403 that is about
    /// the account's PLAN reaches the caller as a 400 refusal naming the plan — but who has to act
    /// on it does not: a key or plan problem is the operator's, whatever sentence the vendor used.
    pub fn from_vendor_status(status: u16) -> Self {
        match status {
            401..=403 => Self::Auth,
            408 => Self::VendorTimeout,
            429 => Self::RateLimited,
            400..=499 => Self::VendorRejected,
            500..=599 => Self::Vendor5xx,
            _ => Self::Internal,
        }
    }
}

impl std::fmt::Display for VoiceErrorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A classified failure: the class, the vendor status when a vendor response caused it, and the
/// message the caller is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceFailure {
    pub class: VoiceErrorType,
    pub vendor_status: Option<u16>,
    pub message: String,
}

impl VoiceFailure {
    /// A failure no vendor response caused.
    pub fn new(class: VoiceErrorType, message: impl Into<String>) -> Self {
        Self {
            class,
            vendor_status: None,
            message: message.into(),
        }
    }

    /// A failure a vendor's HTTP response caused, classified from its status.
    pub fn vendor(status: u16, message: impl Into<String>) -> Self {
        Self {
            class: VoiceErrorType::from_vendor_status(status),
            vendor_status: Some(status),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for VoiceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The class of a synthesis error, and the vendor status when a vendor response caused it.
pub fn classify_tts_error(e: &TTSError) -> (VoiceErrorType, Option<u16>) {
    if let Some(status) = e.vendor_status() {
        return (VoiceErrorType::from_vendor_status(status), Some(status));
    }
    let class = match e.inner() {
        TTSError::RateLimited { .. } => VoiceErrorType::RateLimited,
        TTSError::AuthenticationFailed(_) => VoiceErrorType::Auth,
        TTSError::RequestRejected(_) => VoiceErrorType::VendorRejected,
        TTSError::TimeoutError(_) => VoiceErrorType::VendorTimeout,
        TTSError::NetworkError(_) | TTSError::ConnectionFailed(_) => VoiceErrorType::Network,
        TTSError::InvalidConfiguration(_) => VoiceErrorType::Config,
        _ => VoiceErrorType::Internal,
    };
    (class, None)
}

/// The class of a transcription error, and the vendor status when a vendor response caused it.
pub fn classify_stt_error(e: &STTError) -> (VoiceErrorType, Option<u16>) {
    if let Some(status) = e.vendor_status() {
        return (VoiceErrorType::from_vendor_status(status), Some(status));
    }
    let class = match e.inner() {
        STTError::AuthenticationFailed(_) => VoiceErrorType::Auth,
        STTError::ConfigurationError(_) => VoiceErrorType::Config,
        // The request's own audio, which the caller controls.
        STTError::InvalidAudioFormat(_) => VoiceErrorType::InputDecode,
        STTError::NetworkError(_) | STTError::ConnectionFailed(_) => VoiceErrorType::Network,
        _ => VoiceErrorType::Internal,
    };
    (class, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_values_are_the_contract() {
        let wire: Vec<&str> = VoiceErrorType::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            wire,
            [
                "invalid_request",
                "input_decode",
                "vendor_rejected",
                "auth",
                "rate_limited",
                "vendor_timeout",
                "deadline",
                "vendor_5xx",
                "network",
                "config",
                "internal",
                "circuit_open",
            ]
        );
    }

    #[test]
    fn vendor_statuses_classify_per_frd_6_5() {
        for (status, class) in [
            (400, VoiceErrorType::VendorRejected),
            (401, VoiceErrorType::Auth),
            (402, VoiceErrorType::Auth),
            (403, VoiceErrorType::Auth),
            (404, VoiceErrorType::VendorRejected),
            (408, VoiceErrorType::VendorTimeout),
            (422, VoiceErrorType::VendorRejected),
            (429, VoiceErrorType::RateLimited),
            (500, VoiceErrorType::Vendor5xx),
            (503, VoiceErrorType::Vendor5xx),
            (302, VoiceErrorType::Internal),
        ] {
            assert_eq!(
                VoiceErrorType::from_vendor_status(status),
                class,
                "{status}"
            );
        }
    }

    #[test]
    fn a_wrapped_status_wins_over_the_variant() {
        // A 403 about the account's plan is shown to the caller as a refusal, but it is the
        // operator's to fix: the class follows the status, not the variant.
        let e = TTSError::VendorStatus {
            status: 403,
            error: Box::new(TTSError::RequestRejected("plan".into())),
        };
        assert_eq!(classify_tts_error(&e), (VoiceErrorType::Auth, Some(403)));

        let e = STTError::VendorStatus {
            status: 408,
            error: Box::new(STTError::ProviderError("slow".into())),
        };
        assert_eq!(
            classify_stt_error(&e),
            (VoiceErrorType::VendorTimeout, Some(408))
        );
        let e = STTError::VendorStatus {
            status: 429,
            error: Box::new(STTError::ProviderError("busy".into())),
        };
        assert_eq!(
            classify_stt_error(&e),
            (VoiceErrorType::RateLimited, Some(429))
        );
    }

    #[test]
    fn unwrapped_errors_classify_by_variant_without_a_status() {
        assert_eq!(
            classify_tts_error(&TTSError::NetworkError("refused".into())),
            (VoiceErrorType::Network, None)
        );
        assert_eq!(
            classify_tts_error(&TTSError::TimeoutError("slow".into())),
            (VoiceErrorType::VendorTimeout, None)
        );
        assert_eq!(
            classify_tts_error(&TTSError::InvalidConfiguration("no api_base".into())),
            (VoiceErrorType::Config, None)
        );
        assert_eq!(
            classify_stt_error(&STTError::InvalidAudioFormat("opus".into())),
            (VoiceErrorType::InputDecode, None)
        );
        assert_eq!(
            classify_stt_error(&STTError::ConnectionFailed("dns".into())),
            (VoiceErrorType::Network, None)
        );
    }
}

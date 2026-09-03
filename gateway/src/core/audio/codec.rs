//! D8 — audio transport-codec negotiation for the `/ws` cascade endpoint.
//!
//! A client may request `opus` for the uplink ([`STTWebSocketConfig::audio_in_codec`]) and/or the
//! downlink ([`TTSWebSocketConfig::audio_out_codec`]). Opus is OPTIONAL on the gateway: the real
//! encode/decode only compiles under the `opus-codec` cargo feature. This module owns the small,
//! ALWAYS-BUILT negotiation layer — parse the requested token, coerce it against what THIS build can
//! actually do, and report the effective codec (echoed in `ready`) plus any downgrade warning. No
//! libopus is referenced here, so it compiles in every build.
//!
//! Load-bearing invariant: an `opus` request NEVER errors. On a build without `opus-codec` (the
//! default), or on an unknown token, it degrades to `linear16` with a `config_warning`, and the
//! effective codec is echoed back so the SDK conforms.
//!
//! [`STTWebSocketConfig::audio_in_codec`]: crate::handlers::ws::config::STTWebSocketConfig
//! [`TTSWebSocketConfig::audio_out_codec`]: crate::handlers::ws::config::TTSWebSocketConfig

use std::fmt;

/// A transport codec for ONE `/ws` audio direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioCodec {
    /// Raw 16-bit little-endian PCM — the default; no encode/decode stage.
    #[default]
    Linear16,
    /// Opus, one packet per WS binary frame. The real codec is `opus-codec`-feature-gated; this
    /// variant is still PARSED in every build so the negotiation can report a downgrade.
    Opus,
}

impl AudioCodec {
    /// The canonical wire token (`linear16` | `opus`).
    pub fn as_str(self) -> &'static str {
        match self {
            AudioCodec::Linear16 => "linear16",
            AudioCodec::Opus => "opus",
        }
    }

    /// True when THIS build can actually encode/decode the codec. `linear16` is always supported;
    /// `opus` only under the `opus-codec` feature.
    pub const fn is_supported(self) -> bool {
        match self {
            AudioCodec::Linear16 => true,
            AudioCodec::Opus => cfg!(feature = "opus-codec"),
        }
    }
}

impl fmt::Display for AudioCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parse a client codec token (case-insensitive). Empty / `linear16` aliases → [`AudioCodec::Linear16`];
/// `opus` → [`AudioCodec::Opus`]; anything else → `Linear16` with a parse note (never an error).
pub fn parse_codec(raw: &str) -> (AudioCodec, Option<String>) {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "linear16" | "pcm" | "pcm16" | "l16" => (AudioCodec::Linear16, None),
        "opus" => (AudioCodec::Opus, None),
        other => (
            AudioCodec::Linear16,
            Some(format!("unknown audio codec '{other}', using linear16")),
        ),
    }
}

/// The outcome of negotiating ONE direction's transport codec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecNegotiation {
    /// What the client asked for (parsed; unknown tokens already coerced to `Linear16`).
    pub requested: AudioCodec,
    /// What the gateway will ACTUALLY use this session.
    pub effective: AudioCodec,
    /// A `config_warning` message when the request was downgraded or an unknown token coerced.
    pub warning: Option<String>,
    /// True when the client explicitly set a codec field, so `ready` should echo the effective value
    /// (lets the SDK detect a downgrade). False when the field was omitted (pure-default session).
    pub echo: bool,
}

impl CodecNegotiation {
    /// The value to echo in `ready` for this direction: `Some(effective token)` when the client set a
    /// codec, else `None` (omit the field for default linear16 sessions — backward compatible).
    pub fn echo_value(&self) -> Option<String> {
        self.echo.then(|| self.effective.as_str().to_string())
    }
}

/// Negotiate one direction. `raw` is the client's optional codec field; `direction` labels warnings
/// (e.g. `"uplink"` / `"downlink"`). An omitted field yields `Linear16` with `echo=false`; a
/// supported request passes through; an unsupported request (opus without the feature) degrades to
/// `Linear16` with a warning. Never fails.
pub fn negotiate(raw: Option<&str>, direction: &str) -> CodecNegotiation {
    let Some(raw) = raw else {
        return CodecNegotiation {
            requested: AudioCodec::Linear16,
            effective: AudioCodec::Linear16,
            warning: None,
            echo: false,
        };
    };
    let (requested, parse_warn) = parse_codec(raw);
    let (effective, warning) = if requested.is_supported() {
        (requested, parse_warn)
    } else {
        (
            AudioCodec::Linear16,
            Some(format!(
                "{direction} codec '{}' requested but this gateway build lacks opus support; \
                 using linear16 — rebuild with --features opus-codec to enable it",
                requested.as_str()
            )),
        )
    };
    CodecNegotiation {
        requested,
        effective,
        warning,
        echo: true,
    }
}

/// The five sample rates libopus accepts (Hz). Used to validate/coerce the opus egress rate.
pub const OPUS_RATES: [u32; 5] = [8000, 12000, 16000, 24000, 48000];

/// Snap an arbitrary sample rate to the nearest opus-valid rate (ties → the higher rate).
pub fn nearest_opus_rate(rate: u32) -> u32 {
    OPUS_RATES
        .into_iter()
        .min_by_key(|r| (r.abs_diff(rate), u32::MAX - r))
        .unwrap_or(48000)
}

/// True when `rate` is exactly one of the opus-valid rates.
pub fn is_opus_rate(rate: u32) -> bool {
    OPUS_RATES.contains(&rate)
}

/// The opus-valid frame durations, expressed in TENTHS of a millisecond to keep `2.5ms` integral:
/// 2.5, 5, 10, 20, 40, 60 ms → 25, 50, 100, 200, 400, 600.
pub const OPUS_FRAME_TENTHS_MS: [u32; 6] = [25, 50, 100, 200, 400, 600];

/// True when a whole-ms frame size is an opus-valid frame duration (5/10/20/40/60 ms — 2.5ms is not
/// expressible as a whole ms, so it's excluded from the whole-ms check used by `audio_out_chunk_ms`).
pub fn is_opus_frame_ms(ms: u32) -> bool {
    matches!(ms, 5 | 10 | 20 | 40 | 60)
}

/// Snap a whole-ms chunk size to the nearest opus-valid whole-ms frame (5/10/20/40/60), ties → larger.
pub fn nearest_opus_frame_ms(ms: u32) -> u32 {
    [5u32, 10, 20, 40, 60]
        .into_iter()
        .min_by_key(|f| (f.abs_diff(ms), u32::MAX - f))
        .unwrap_or(20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_linear16_aliases_and_opus() {
        for token in ["", "linear16", "LINEAR16", " pcm ", "pcm16", "l16"] {
            assert_eq!(parse_codec(token).0, AudioCodec::Linear16, "{token:?}");
            assert!(parse_codec(token).1.is_none());
        }
        assert_eq!(parse_codec("opus").0, AudioCodec::Opus);
        assert_eq!(parse_codec("OPUS").0, AudioCodec::Opus);
    }

    #[test]
    fn unknown_token_coerces_to_linear16_with_note() {
        let (codec, warn) = parse_codec("flac");
        assert_eq!(codec, AudioCodec::Linear16);
        assert!(warn.unwrap().contains("flac"));
    }

    #[test]
    fn omitted_field_is_default_no_echo() {
        let n = negotiate(None, "uplink");
        assert_eq!(n.effective, AudioCodec::Linear16);
        assert!(!n.echo);
        assert!(n.warning.is_none());
        assert_eq!(n.echo_value(), None);
    }

    #[test]
    fn linear16_request_echoes_no_warning() {
        let n = negotiate(Some("linear16"), "downlink");
        assert_eq!(n.effective, AudioCodec::Linear16);
        assert!(n.echo);
        assert!(n.warning.is_none());
        assert_eq!(n.echo_value().as_deref(), Some("linear16"));
    }

    #[test]
    fn opus_request_negotiates_per_build_capability() {
        let n = negotiate(Some("opus"), "uplink");
        assert!(n.echo);
        assert_eq!(n.requested, AudioCodec::Opus);
        if cfg!(feature = "opus-codec") {
            // Feature build: opus is honored, no downgrade.
            assert_eq!(n.effective, AudioCodec::Opus);
            assert!(n.warning.is_none());
            assert_eq!(n.echo_value().as_deref(), Some("opus"));
        } else {
            // Default build: graceful degrade to linear16 + a warning — NEVER an error.
            assert_eq!(n.effective, AudioCodec::Linear16);
            assert_eq!(n.echo_value().as_deref(), Some("linear16"));
            let w = n.warning.clone().expect("downgrade warning");
            assert!(w.contains("opus"));
            assert!(w.contains("linear16"));
        }
    }

    #[test]
    fn opus_rate_snapping() {
        assert!(is_opus_rate(48000));
        assert!(!is_opus_rate(44100));
        assert_eq!(nearest_opus_rate(44100), 48000);
        assert_eq!(nearest_opus_rate(22050), 24000);
        assert_eq!(nearest_opus_rate(16000), 16000);
        assert_eq!(nearest_opus_rate(11000), 12000);
        assert_eq!(nearest_opus_rate(0), 8000);
    }

    #[test]
    fn opus_frame_ms_snapping() {
        assert!(is_opus_frame_ms(20));
        assert!(!is_opus_frame_ms(15));
        assert_eq!(nearest_opus_frame_ms(15), 20);
        assert_eq!(nearest_opus_frame_ms(25), 20);
        assert_eq!(nearest_opus_frame_ms(100), 60);
        assert_eq!(nearest_opus_frame_ms(7), 5);
    }

    #[test]
    fn supported_matches_feature_flag() {
        assert!(AudioCodec::Linear16.is_supported());
        assert_eq!(
            AudioCodec::Opus.is_supported(),
            cfg!(feature = "opus-codec")
        );
    }
}

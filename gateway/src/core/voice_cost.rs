//! What a voice call cost, by the deployment's own price (FRD-021 §6.4).
//!
//! `cost = billable_units / per_units × cost_per_unit`, computed on the success path beside the
//! units it is computed from. It is Bud's price for the deployment — like `InferenceFact.cost` —
//! not the vendor's invoice: vendor rounding (per 15 s, minimum charges) is not modelled.
//!
//! | unit        | TTS                           | STT                  |
//! |-------------|-------------------------------|----------------------|
//! | `character` | characters synthesised        | not applicable       |
//! | `second`    | output audio seconds          | audio seconds        |
//! | `minute`    | output audio seconds / 60     | audio seconds / 60   |
//! | `request`   | 1                             | 1                    |
//!
//! No pricing, a unit that does not apply, a unit whose quantity is unknown (compressed TTS
//! output, DEG-4), or `per_units == 0` → no cost at all, never `0.0`: the UI shows such a call as
//! unpriced (M-A9), and a zero would read as "free".

use bud_auth::VoicePricing;

/// The cost of one call and the unit it was computed from, or `None` when the call is unpriced.
///
/// `characters` is the TTS input as the customer sent it (before pronunciation replacement);
/// `audio_seconds` the STT input; `output_audio_seconds` the audio a synthesis produced.
pub fn voice_cost(
    pricing: Option<&VoicePricing>,
    capability: &str,
    characters: Option<u64>,
    audio_seconds: Option<f64>,
    output_audio_seconds: Option<f64>,
) -> Option<(f64, &'static str)> {
    let pricing = pricing?;
    if pricing.per_units == 0 || !pricing.cost_per_unit.is_finite() || pricing.cost_per_unit < 0.0 {
        return None;
    }
    let tts = match capability {
        "text_to_speech" => true,
        "audio_transcription" | "audio_translation" => false,
        _ => return None,
    };
    let (units, unit) = match pricing.unit.as_str() {
        "character" if tts => (characters? as f64, "character"),
        "character" => return None,
        "second" if tts => (output_audio_seconds?, "second"),
        "second" => (audio_seconds?, "second"),
        "minute" if tts => (output_audio_seconds? / 60.0, "minute"),
        "minute" => (audio_seconds? / 60.0, "minute"),
        "request" => (1.0, "request"),
        _ => return None,
    };
    if !units.is_finite() || units < 0.0 {
        return None;
    }
    let cost = units / pricing.per_units as f64 * pricing.cost_per_unit;
    cost.is_finite().then_some((cost, unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(unit: &str, cost_per_unit: f64, per_units: u64) -> VoicePricing {
        VoicePricing {
            unit: unit.to_string(),
            cost_per_unit,
            currency: Some("USD".to_string()),
            per_units,
        }
    }

    fn close(got: Option<(f64, &'static str)>, want: f64, unit: &str) {
        let (cost, u) = got.expect("priced");
        assert!((cost - want).abs() < 1e-9, "cost {cost}, want {want}");
        assert_eq!(u, unit);
    }

    /// TC-EMIT-05.
    #[test]
    fn the_published_rates_price_the_units() {
        // TTS `character`: 120 chars at 30 per 1,000,000.
        close(
            voice_cost(
                Some(&price("character", 30.0, 1_000_000)),
                "text_to_speech",
                Some(120),
                None,
                None,
            ),
            0.0036,
            "character",
        );
        // STT `second`: 12.5 s at 0.0001 per second.
        close(
            voice_cost(
                Some(&price("second", 0.0001, 1)),
                "audio_transcription",
                None,
                Some(12.5),
                None,
            ),
            0.00125,
            "second",
        );
        // STT `minute`: 90 s at 0.0043 per minute.
        close(
            voice_cost(
                Some(&price("minute", 0.0043, 1)),
                "audio_translation",
                None,
                Some(90.0),
                None,
            ),
            0.00645,
            "minute",
        );
        // `request`: flat, whatever the units.
        close(
            voice_cost(
                Some(&price("request", 0.002, 1)),
                "text_to_speech",
                Some(5),
                None,
                None,
            ),
            0.002,
            "request",
        );
        // TTS priced by time, once the output duration is known (Phase 5).
        close(
            voice_cost(
                Some(&price("second", 0.001, 1)),
                "text_to_speech",
                Some(10),
                None,
                Some(1.5),
            ),
            0.0015,
            "second",
        );
        close(
            voice_cost(
                Some(&price("minute", 0.06, 1)),
                "text_to_speech",
                Some(10),
                None,
                Some(30.0),
            ),
            0.03,
            "minute",
        );
    }

    /// TC-EMIT-06.
    #[test]
    fn an_unpriced_call_has_no_cost_rather_than_zero() {
        // No pricing block.
        assert_eq!(
            voice_cost(None, "text_to_speech", Some(120), None, None),
            None
        );
        // TTS priced by time with no known output duration (a compressed format, DEG-4).
        assert_eq!(
            voice_cost(
                Some(&price("second", 0.001, 1)),
                "text_to_speech",
                Some(120),
                None,
                None
            ),
            None
        );
        // STT priced per character: the unit does not apply.
        assert_eq!(
            voice_cost(
                Some(&price("character", 1.0, 1)),
                "audio_transcription",
                None,
                Some(10.0),
                None
            ),
            None
        );
        // `per_units = 0` is not a division.
        assert_eq!(
            voice_cost(
                Some(&price("character", 30.0, 0)),
                "text_to_speech",
                Some(120),
                None,
                None
            ),
            None
        );
        // An STT call whose duration is unknown (self-hosted, non-WAV upload, DEG-7).
        assert_eq!(
            voice_cost(
                Some(&price("second", 0.0001, 1)),
                "audio_transcription",
                None,
                None,
                None
            ),
            None
        );
        // A unit the rule does not know, and a capability that is not a voice one.
        assert_eq!(
            voice_cost(
                Some(&price("token", 1.0, 1)),
                "text_to_speech",
                Some(1),
                None,
                None
            ),
            None
        );
        assert_eq!(
            voice_cost(Some(&price("request", 1.0, 1)), "chat", None, None, None),
            None
        );
        // A non-finite price never becomes a number.
        assert_eq!(
            voice_cost(
                Some(&price("request", f64::NAN, 1)),
                "text_to_speech",
                None,
                None,
                None
            ),
            None
        );
    }
}

//! What a realtime (speech-to-speech) record costs (FRD-023 §5.10, D-9, D-10).
//!
//! Realtime is the one place on the audio plane that bills in TOKENS, and at up to eight rates at
//! once: text, audio and image in, their cached forms, text and audio out. A single
//! `cost_per_unit` cannot express that — cached audio input is ~80× cheaper than uncached — so a
//! realtime price is a map of per-modality rates (`VoicePricing::rates`, CONTRACTS C1).
//!
//! Two rules are load-bearing:
//!
//! * **Cached tokens are a SUBSET of input tokens** (OpenAI's usage contract). Each modality's
//!   uncached count is `input − cached`, priced at its own rate; pricing both in full would bill
//!   the cached part twice.
//! * **A component with no rate is UNPRICED, never free.** It is left out of the cost and named in
//!   `unpriced`, which the turn span records (`bud.voice.unpriced_components`). Pricing it at zero
//!   would make a missing rate indistinguishable from a free one.
//!
//! Pure functions: no I/O, no spans.

use bud_auth::VoicePricing;

/// Token counts from one `response.done` (or their sum over a session).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RealtimeUsage {
    pub input_text: u64,
    pub input_audio: u64,
    pub input_image: u64,
    pub cached_text: u64,
    pub cached_audio: u64,
    pub cached_image: u64,
    pub output_text: u64,
    pub output_audio: u64,
}

impl RealtimeUsage {
    /// Read OpenAI GA `response.usage`:
    /// `{input_tokens, output_tokens, input_token_details: {text_tokens, audio_tokens,
    /// image_tokens, cached_tokens, cached_tokens_details: {text_tokens, audio_tokens,
    /// image_tokens}}, output_token_details: {text_tokens, audio_tokens}}`.
    ///
    /// `None` when there is no usage object at all (a vendor that reports none). Missing counts
    /// inside it read as zero, which is what they mean.
    pub fn from_openai(usage: &serde_json::Value) -> Option<Self> {
        let usage = usage.as_object()?;
        let n = |v: Option<&serde_json::Value>| v.and_then(serde_json::Value::as_u64).unwrap_or(0);
        let input = usage.get("input_token_details");
        let cached = input.and_then(|d| d.get("cached_tokens_details"));
        let output = usage.get("output_token_details");
        let mut u = Self {
            input_text: n(input.and_then(|d| d.get("text_tokens"))),
            input_audio: n(input.and_then(|d| d.get("audio_tokens"))),
            input_image: n(input.and_then(|d| d.get("image_tokens"))),
            cached_text: n(cached.and_then(|d| d.get("text_tokens"))),
            cached_audio: n(cached.and_then(|d| d.get("audio_tokens"))),
            cached_image: n(cached.and_then(|d| d.get("image_tokens"))),
            output_text: n(output.and_then(|d| d.get("text_tokens"))),
            output_audio: n(output.and_then(|d| d.get("audio_tokens"))),
        };
        // A vendor that reports only totals: attribute them to text rather than drop them, so
        // the record still carries volume. An all-modality breakdown always wins when present.
        if input.is_none() && output.is_none() {
            u.input_text = n(usage.get("input_tokens"));
            u.output_text = n(usage.get("output_tokens"));
        }
        // A cached count larger than its input is a vendor inconsistency; never let the
        // uncached remainder go negative (it would REDUCE the bill).
        u.cached_text = u.cached_text.min(u.input_text);
        u.cached_audio = u.cached_audio.min(u.input_audio);
        u.cached_image = u.cached_image.min(u.input_image);
        Some(u)
    }

    pub fn add(&mut self, other: &Self) {
        self.input_text += other.input_text;
        self.input_audio += other.input_audio;
        self.input_image += other.input_image;
        self.cached_text += other.cached_text;
        self.cached_audio += other.cached_audio;
        self.cached_image += other.cached_image;
        self.output_text += other.output_text;
        self.output_audio += other.output_audio;
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What an input transcription reported (`conversation.item.input_audio_transcription.completed`
/// `usage`): either seconds of audio or token counts, depending on the transcription model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TranscriptionUsage {
    Seconds(f64),
    Tokens {
        input_audio: u64,
        input_text: u64,
        output_text: u64,
    },
}

impl TranscriptionUsage {
    pub fn from_openai(usage: &serde_json::Value) -> Option<Self> {
        let n = |v: Option<&serde_json::Value>| v.and_then(serde_json::Value::as_u64).unwrap_or(0);
        match usage.get("type").and_then(|t| t.as_str()) {
            Some("duration") => usage
                .get("seconds")
                .and_then(serde_json::Value::as_f64)
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(Self::Seconds),
            Some("tokens") => {
                let details = usage.get("input_token_details");
                let (audio, text) = match details {
                    Some(d) => (n(d.get("audio_tokens")), n(d.get("text_tokens"))),
                    // No breakdown: a transcription's input is audio.
                    None => (n(usage.get("input_tokens")), 0),
                };
                Some(Self::Tokens {
                    input_audio: audio,
                    input_text: text,
                    output_text: n(usage.get("output_tokens")),
                })
            }
            _ => None,
        }
    }
}

/// A computed price and what it could not price.
#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeCost {
    /// `None` when nothing was priceable (no price, or every present component unpriced).
    pub cost: Option<f64>,
    /// What `cost` was computed from: `token` | `minute` | `second`. `None` with `cost`.
    pub unit: Option<&'static str>,
    /// Components that were present and had no rate — named, never zero-priced.
    pub unpriced: Vec<&'static str>,
}

impl RealtimeCost {
    fn none() -> Self {
        Self {
            cost: None,
            unit: None,
            unpriced: Vec::new(),
        }
    }
}

fn rate(pricing: &VoicePricing, key: &str) -> Option<f64> {
    pricing
        .rates
        .get(key)
        .copied()
        .filter(|r| r.is_finite() && *r >= 0.0)
}

/// Accumulates `count × rate / per_units` over components, naming each unpriced one.
struct Tally<'p> {
    pricing: &'p VoicePricing,
    per_units: f64,
    total: f64,
    priced_any: bool,
    unpriced: Vec<&'static str>,
}

impl<'p> Tally<'p> {
    fn new(pricing: &'p VoicePricing) -> Self {
        Self {
            pricing,
            per_units: pricing.per_units as f64,
            total: 0.0,
            priced_any: false,
            unpriced: Vec::new(),
        }
    }

    fn add(&mut self, count: u64, key: &'static str) {
        if count == 0 {
            return;
        }
        match rate(self.pricing, key) {
            Some(r) => {
                self.total += count as f64 * r / self.per_units;
                self.priced_any = true;
            }
            None => self.unpriced.push(key),
        }
    }

    fn finish(self, nothing_present: bool) -> RealtimeCost {
        let cost = if self.priced_any || nothing_present {
            Some(self.total).filter(|c| c.is_finite())
        } else {
            None
        };
        RealtimeCost {
            unit: cost.map(|_| "token"),
            cost,
            unpriced: self.unpriced,
        }
    }
}

/// One `response.done`, under a token price (FRD-023 §5.10):
///
/// ```text
/// cost = [ (in_text − cached_text)·r.input_text + cached_text·r.cached_input_text
///        + (in_audio − cached_audio)·r.input_audio + cached_audio·r.cached_input_audio
///        + (in_image − cached_image)·r.input_image + cached_image·r.cached_input_image
///        + out_text·r.output_text + out_audio·r.output_audio ] / per_units
/// ```
///
/// A minute or second price bills the session's DURATION instead ([`realtime_duration_cost`]), so
/// a response under one carries its tokens and no cost.
pub fn realtime_response_cost(
    pricing: Option<&VoicePricing>,
    usage: &RealtimeUsage,
) -> RealtimeCost {
    let Some(pricing) = pricing else {
        return RealtimeCost::none();
    };
    if pricing.unit != "token" || pricing.per_units == 0 {
        return RealtimeCost::none();
    }
    let mut t = Tally::new(pricing);
    t.add(usage.input_text - usage.cached_text, "input_text");
    t.add(usage.cached_text, "cached_input_text");
    t.add(usage.input_audio - usage.cached_audio, "input_audio");
    t.add(usage.cached_audio, "cached_input_audio");
    t.add(usage.input_image - usage.cached_image, "input_image");
    t.add(usage.cached_image, "cached_input_image");
    t.add(usage.output_text, "output_text");
    t.add(usage.output_audio, "output_audio");
    t.finish(usage.is_empty())
}

/// One input transcription. Seconds are priced at `transcription_per_minute` (per MINUTE, not per
/// `per_units`); token usage at the three `transcription_*` token rates (per `per_units`).
/// Applies under every unit: a transcription is its own billed record.
pub fn realtime_transcription_cost(
    pricing: Option<&VoicePricing>,
    usage: &TranscriptionUsage,
) -> RealtimeCost {
    let Some(pricing) = pricing else {
        return RealtimeCost::none();
    };
    match *usage {
        TranscriptionUsage::Seconds(secs) => match rate(pricing, "transcription_per_minute") {
            Some(r) => {
                let cost = secs / 60.0 * r;
                RealtimeCost {
                    cost: cost.is_finite().then_some(cost),
                    unit: cost.is_finite().then_some("minute"),
                    unpriced: Vec::new(),
                }
            }
            None if secs > 0.0 => RealtimeCost {
                cost: None,
                unit: None,
                unpriced: vec!["transcription_per_minute"],
            },
            None => RealtimeCost::none(),
        },
        TranscriptionUsage::Tokens {
            input_audio,
            input_text,
            output_text,
        } => {
            if pricing.per_units == 0 {
                return RealtimeCost::none();
            }
            let mut t = Tally::new(pricing);
            t.add(input_audio, "transcription_input_audio");
            t.add(input_text, "transcription_input_text");
            t.add(output_text, "transcription_output_text");
            t.finish(input_audio == 0 && input_text == 0 && output_text == 0)
        }
    }
}

/// A duration segment under a minute or second price (D-9: per 60 s, so a socket that drops at
/// minute 40 was still billed for 39 minutes). `None` under any other unit.
pub fn realtime_duration_cost(pricing: Option<&VoicePricing>, seconds: f64) -> RealtimeCost {
    let Some(pricing) = pricing else {
        return RealtimeCost::none();
    };
    if pricing.per_units == 0 || !seconds.is_finite() || seconds < 0.0 {
        return RealtimeCost::none();
    }
    let (units, unit) = match pricing.unit.as_str() {
        "minute" => (seconds / 60.0, "minute"),
        "second" => (seconds, "second"),
        _ => return RealtimeCost::none(),
    };
    let cost = units * pricing.cost_per_unit / pricing.per_units as f64;
    RealtimeCost {
        cost: cost.is_finite().then_some(cost),
        unit: cost.is_finite().then_some(unit),
        unpriced: Vec::new(),
    }
}

/// Whether this price bills the session's duration (so the relay emits 60 s segments).
pub fn bills_duration(pricing: Option<&VoicePricing>) -> bool {
    pricing.is_some_and(|p| matches!(p.unit.as_str(), "minute" | "second"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn token_price(rates: &[(&str, f64)]) -> VoicePricing {
        VoicePricing {
            unit: "token".into(),
            cost_per_unit: 0.0,
            currency: Some("USD".into()),
            per_units: 1_000_000,
            rates: rates
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    /// gpt-realtime-2.1 list prices (FRD §5.10).
    fn gpt_realtime_21() -> VoicePricing {
        token_price(&[
            ("input_text", 4.0),
            ("input_audio", 32.0),
            ("input_image", 5.0),
            ("cached_input_text", 0.4),
            ("cached_input_audio", 0.4),
            ("cached_input_image", 0.5),
            ("output_text", 24.0),
            ("output_audio", 64.0),
            ("transcription_per_minute", 0.003),
        ])
    }

    /// OpenAI's documented `response.done` usage (the FRD's worked example).
    fn documented_usage() -> serde_json::Value {
        serde_json::json!({
            "total_tokens": 253,
            "input_tokens": 132,
            "output_tokens": 121,
            "input_token_details": {
                "text_tokens": 119, "audio_tokens": 13, "image_tokens": 0,
                "cached_tokens": 64,
                "cached_tokens_details": {"text_tokens": 64, "audio_tokens": 0, "image_tokens": 0}
            },
            "output_token_details": {"text_tokens": 30, "audio_tokens": 91}
        })
    }

    /// TC-MET-03 🔒 — the formula, to 1e-10.
    #[test]
    fn tc_met_03_the_worked_example_costs_0_0072056() {
        let usage = RealtimeUsage::from_openai(&documented_usage()).unwrap();
        assert_eq!(usage.input_text, 119);
        assert_eq!(usage.cached_text, 64);
        let c = realtime_response_cost(Some(&gpt_realtime_21()), &usage);
        let cost = c.cost.expect("priced");
        assert!((cost - 0.0072056).abs() < 1e-10, "cost {cost}");
        assert_eq!(c.unit, Some("token"));
        assert!(c.unpriced.is_empty());
    }

    /// TC-MET-04 — cached audio at the cached rate, only the remainder at the full one.
    #[test]
    fn tc_met_04_cached_audio_is_subtracted_from_its_class() {
        let usage = RealtimeUsage {
            input_audio: 1000,
            cached_audio: 800,
            ..Default::default()
        };
        let c = realtime_response_cost(Some(&gpt_realtime_21()), &usage);
        let want = (200.0 * 32.0 + 800.0 * 0.4) / 1e6;
        assert!((c.cost.unwrap() - want).abs() < 1e-12);
    }

    /// TC-MET-05 — a present component without a rate is named, not zero-priced.
    #[test]
    fn tc_met_05_a_missing_rate_is_unpriced_not_free() {
        let pricing = token_price(&[("input_audio", 32.0), ("output_audio", 64.0)]);
        let usage = RealtimeUsage {
            input_audio: 100,
            input_image: 50,
            output_audio: 10,
            ..Default::default()
        };
        let c = realtime_response_cost(Some(&pricing), &usage);
        let want = (100.0 * 32.0 + 10.0 * 64.0) / 1e6;
        assert!((c.cost.unwrap() - want).abs() < 1e-12, "images excluded");
        assert_eq!(c.unpriced, vec!["input_image"]);
    }

    #[test]
    fn every_present_component_unpriced_means_no_cost_at_all() {
        let pricing = token_price(&[("output_audio", 64.0)]);
        let usage = RealtimeUsage {
            input_text: 10,
            ..Default::default()
        };
        let c = realtime_response_cost(Some(&pricing), &usage);
        assert_eq!(c.cost, None, "an all-unpriced record must not read as free");
        assert_eq!(c.unpriced, vec!["input_text"]);
    }

    /// TC-MET-06 — 12 s at $0.003/min.
    #[test]
    fn tc_met_06_duration_transcription_usage() {
        let usage = TranscriptionUsage::from_openai(
            &serde_json::json!({"type": "duration", "seconds": 12}),
        )
        .unwrap();
        let c = realtime_transcription_cost(Some(&gpt_realtime_21()), &usage);
        assert!((c.cost.unwrap() - 0.0006).abs() < 1e-12);
        assert_eq!(c.unit, Some("minute"));
    }

    #[test]
    fn token_transcription_usage_uses_the_transcription_token_rates() {
        let usage = TranscriptionUsage::from_openai(&serde_json::json!({
            "type": "tokens", "total_tokens": 30, "input_tokens": 20, "output_tokens": 10,
            "input_token_details": {"text_tokens": 0, "audio_tokens": 20}
        }))
        .unwrap();
        let unpriced = realtime_transcription_cost(Some(&gpt_realtime_21()), &usage);
        assert_eq!(unpriced.cost, None);
        assert_eq!(
            unpriced.unpriced,
            vec!["transcription_input_audio", "transcription_output_text"]
        );

        let priced = token_price(&[
            ("transcription_input_audio", 3.0),
            ("transcription_output_text", 5.0),
        ]);
        let c = realtime_transcription_cost(Some(&priced), &usage);
        assert!((c.cost.unwrap() - (20.0 * 3.0 + 10.0 * 5.0) / 1e6).abs() < 1e-12);
    }

    #[test]
    fn a_minute_price_bills_duration_not_tokens() {
        let pricing = VoicePricing {
            unit: "minute".into(),
            cost_per_unit: 0.06,
            currency: None,
            per_units: 1,
            rates: BTreeMap::new(),
        };
        assert!(bills_duration(Some(&pricing)));
        let usage = RealtimeUsage::from_openai(&documented_usage()).unwrap();
        assert_eq!(realtime_response_cost(Some(&pricing), &usage).cost, None);
        let seg = realtime_duration_cost(Some(&pricing), 60.0);
        assert!((seg.cost.unwrap() - 0.06).abs() < 1e-12);
        assert_eq!(seg.unit, Some("minute"));
        let partial = realtime_duration_cost(Some(&pricing), 30.0);
        assert!((partial.cost.unwrap() - 0.03).abs() < 1e-12);
    }

    #[test]
    fn a_cached_count_above_its_input_never_reduces_the_bill() {
        let usage = RealtimeUsage::from_openai(&serde_json::json!({
            "input_token_details": {"text_tokens": 10,
                "cached_tokens_details": {"text_tokens": 50}},
            "output_token_details": {}
        }))
        .unwrap();
        assert_eq!(usage.cached_text, 10);
    }

    #[test]
    fn no_price_prices_nothing() {
        let usage = RealtimeUsage::from_openai(&documented_usage()).unwrap();
        assert_eq!(realtime_response_cost(None, &usage), RealtimeCost::none());
        assert!(!bills_duration(None));
    }
}

//! Metering a relayed session (FRD-023 §5.10, D-9, D-19, FR-MET-1…3).
//!
//! * **One `voice.turn` per billed record** — each `response.done`, each input-transcription
//!   completion, and each 60 s duration segment under a minute/second price. Metering at the event
//!   rather than at close is the point: a socket that drops at minute 40 has still been billed for
//!   everything before it (and Kong shipped exactly this bug — realtime tokens that never reached
//!   metrics).
//! * **Each turn is the ROOT of its own trace**, linked to the session's trace. `VoiceTurnFact`
//!   coalesces rows on `(date, TraceId)`; turns sharing the session's trace would collapse into
//!   one row and lose every response but one.
//! * **One `voice.session` span per session**, its own root, opened at start (so turns can link to
//!   it) and ended at close with the totals. A pod crash loses it; the turns survive (DEG-4).

use bud_auth::VoicePricing;
use opentelemetry::trace::TraceContextExt;
use serde_json::Value;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::core::realtime_cost::{
    RealtimeCost, RealtimeUsage, TranscriptionUsage, realtime_duration_cost,
    realtime_response_cost, realtime_transcription_cost,
};
use crate::observability::voice_attrs::{realtime as rt, session as sess, turn};

use super::handshake::REALTIME_CAPABILITY;

/// Who a session is for, recorded on every span it emits (CONTRACTS C2).
#[derive(Debug, Clone, Default)]
pub struct Attribution {
    pub project_id: Option<String>,
    pub endpoint_id: String,
    pub model_id: Option<String>,
    pub api_key_id: Option<String>,
    pub user_id: Option<String>,
    pub api_key_project_id: Option<String>,
    /// The `?model=` the client connected with.
    pub endpoint_name: String,
    pub vendor: String,
    /// The vendor model (`voice_table.model`).
    pub model: Option<String>,
    pub session_type: String,
}

fn record_text(span: &Span, key: &'static str, value: Option<&str>) {
    // An empty string is not NULL; it would make the column look populated.
    if let Some(v) = value.filter(|v| !v.is_empty()) {
        span.record(key, v);
    }
}

fn record_attribution(span: &Span, a: &Attribution, session_id: &str) {
    record_text(span, turn::PROJECT_ID, a.project_id.as_deref());
    record_text(span, turn::ENDPOINT_ID, Some(&a.endpoint_id));
    record_text(span, turn::MODEL_ID, a.model_id.as_deref());
    record_text(span, turn::API_KEY_ID, a.api_key_id.as_deref());
    record_text(span, turn::USER_ID, a.user_id.as_deref());
    record_text(
        span,
        turn::API_KEY_PROJECT_ID,
        a.api_key_project_id.as_deref(),
    );
    record_text(span, turn::ENDPOINT_NAME, Some(&a.endpoint_name));
    record_text(span, turn::SESSION_ID, Some(session_id));
    record_text(span, rt::VENDOR, Some(&a.vendor));
    record_text(span, rt::MODEL, a.model.as_deref());
}

fn record_usage(span: &Span, u: &RealtimeUsage) {
    span.record(rt::INPUT_TEXT_TOKENS, u.input_text);
    span.record(rt::INPUT_AUDIO_TOKENS, u.input_audio);
    span.record(rt::INPUT_IMAGE_TOKENS, u.input_image);
    span.record(rt::CACHED_TEXT_TOKENS, u.cached_text);
    span.record(rt::CACHED_AUDIO_TOKENS, u.cached_audio);
    span.record(rt::CACHED_IMAGE_TOKENS, u.cached_image);
    span.record(rt::OUTPUT_TEXT_TOKENS, u.output_text);
    span.record(rt::OUTPUT_AUDIO_TOKENS, u.output_audio);
}

fn record_cost(span: &Span, c: &RealtimeCost) {
    if let (Some(cost), Some(unit)) = (c.cost, c.unit) {
        span.record(turn::COST, cost);
        span.record(turn::PRICING_UNIT, unit);
    }
    if !c.unpriced.is_empty() {
        span.record(rt::UNPRICED_COMPONENTS, c.unpriced.join(",").as_str());
    }
}

/// The transcripts a `response.done` carries (`response.output[].content[].transcript|text`).
fn response_transcript(event: &Value) -> Option<String> {
    let mut parts = Vec::new();
    for item in event
        .get("response")
        .and_then(|r| r.get("output"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for c in item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(t) = c
                .get("transcript")
                .or_else(|| c.get("text"))
                .and_then(Value::as_str)
            {
                parts.push(t.to_string());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// The meter for one session.
pub struct SessionMeter {
    session_span: Span,
    session_id: String,
    attribution: Attribution,
    pricing: Option<VoicePricing>,
    capture: bool,
    turn_index: u64,
    totals: RealtimeUsage,
    billed_seconds: f64,
    cost_total: f64,
    priced_any: bool,
    pricing_unit: Option<&'static str>,
    vendor_session_id: Option<String>,
    started: std::time::Instant,
}

impl SessionMeter {
    /// Open the session's `voice.session` span (a root: its own trace).
    pub fn start(
        session_id: String,
        attribution: Attribution,
        pricing: Option<VoicePricing>,
    ) -> Self {
        let session_span =
            crate::voice_session_span!(capability = REALTIME_CAPABILITY, transport = "websocket");
        record_attribution(&session_span, &attribution, &session_id);
        record_text(
            &session_span,
            rt::SESSION_TYPE,
            Some(&attribution.session_type),
        );
        Self {
            session_span,
            session_id,
            attribution,
            pricing,
            capture: crate::observability::trace_redact::capture_content(),
            turn_index: 0,
            totals: RealtimeUsage::default(),
            billed_seconds: 0.0,
            cost_total: 0.0,
            priced_any: false,
            pricing_unit: None,
            vendor_session_id: None,
            started: std::time::Instant::now(),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn turns(&self) -> u64 {
        self.turn_index
    }

    pub fn set_vendor_session_id(&mut self, id: Option<String>) {
        if id.is_some() {
            self.vendor_session_id = id;
        }
    }

    /// A root `voice.turn`, linked to the session trace, with attribution recorded.
    fn open_turn(&mut self, component: &'static str) -> Span {
        let span = crate::voice_turn_span!(
            parent: None,
            capability = REALTIME_CAPABILITY,
            transport = "websocket"
        );
        let link = self.session_span.context().span().span_context().clone();
        if link.is_valid() {
            span.add_link(link);
        }
        record_attribution(&span, &self.attribution, &self.session_id);
        record_text(
            &span,
            rt::VENDOR_SESSION_ID,
            self.vendor_session_id.as_deref(),
        );
        span.record(turn::TURN_INDEX, self.turn_index);
        span.record(rt::COMPONENT, component);
        self.turn_index += 1;
        span
    }

    fn account(&mut self, c: &RealtimeCost) {
        if let (Some(cost), Some(unit)) = (c.cost, c.unit) {
            self.cost_total += cost;
            self.priced_any = true;
            self.pricing_unit.get_or_insert(unit);
        }
    }

    /// One `response.done` (§5.5 tap).
    pub fn response_done(&mut self, event: &Value) {
        let response = event.get("response");
        let usage = response
            .and_then(|r| r.get("usage"))
            .and_then(RealtimeUsage::from_openai)
            .unwrap_or_default();
        let cost = realtime_response_cost(self.pricing.as_ref(), &usage);
        let span = self.open_turn("response");
        record_text(
            &span,
            rt::RESPONSE_ID,
            response.and_then(|r| r.get("id")).and_then(Value::as_str),
        );
        let status = response
            .and_then(|r| r.get("status"))
            .and_then(Value::as_str);
        record_text(&span, rt::RESPONSE_STATUS, status);
        if status == Some("failed") {
            span.record("otel.status_code", "ERROR");
            span.record(turn::ERROR_TYPE, "vendor_error");
        }
        record_usage(&span, &usage);
        record_cost(&span, &cost);
        if self.capture
            && let Some(t) = response_transcript(event)
        {
            span.record(
                turn::TRANSCRIPT,
                crate::observability::trace_redact::sanitize_body(&t).as_str(),
            );
        }
        self.totals.add(&usage);
        self.account(&cost);
    }

    /// One `conversation.item.input_audio_transcription.completed`.
    pub fn transcription_completed(&mut self, event: &Value) {
        let usage = event.get("usage").and_then(TranscriptionUsage::from_openai);
        let span = self.open_turn("input_transcription");
        let cost = match usage {
            Some(u) => {
                match u {
                    TranscriptionUsage::Seconds(s) => {
                        span.record(rt::BILLED_SECONDS, s);
                        self.billed_seconds += s;
                    }
                    TranscriptionUsage::Tokens {
                        input_audio,
                        input_text,
                        output_text,
                    } => {
                        let t = RealtimeUsage {
                            input_audio,
                            input_text,
                            output_text,
                            ..Default::default()
                        };
                        record_usage(&span, &t);
                    }
                }
                realtime_transcription_cost(self.pricing.as_ref(), &u)
            }
            None => RealtimeCost {
                cost: None,
                unit: None,
                unpriced: Vec::new(),
            },
        };
        record_cost(&span, &cost);
        if self.capture
            && let Some(t) = event.get("transcript").and_then(Value::as_str)
        {
            span.record(
                turn::TRANSCRIPT,
                crate::observability::trace_redact::sanitize_body(t).as_str(),
            );
        }
        self.account(&cost);
    }

    /// One duration segment under a minute/second price (D-9).
    pub fn duration_segment(&mut self, seconds: f64) {
        if seconds <= 0.0 {
            return;
        }
        let cost = realtime_duration_cost(self.pricing.as_ref(), seconds);
        let span = self.open_turn("duration_segment");
        span.record(rt::BILLED_SECONDS, seconds);
        record_cost(&span, &cost);
        self.billed_seconds += seconds;
        self.account(&cost);
    }

    /// Close the session span with its totals (FR-MET-3).
    pub fn finish(self, end_reason: &str, close_code: u16) {
        let span = &self.session_span;
        record_text(
            span,
            rt::VENDOR_SESSION_ID,
            self.vendor_session_id.as_deref(),
        );
        span.record(
            sess::DURATION_MS,
            self.started.elapsed().as_secs_f64() * 1000.0,
        );
        span.record(sess::TURNS, self.turn_index);
        span.record(sess::END_REASON, end_reason);
        span.record(sess::CLOSE_CODE, u64::from(close_code));
        record_usage(span, &self.totals);
        if self.billed_seconds > 0.0 {
            span.record(rt::BILLED_SECONDS, self.billed_seconds);
        }
        if self.priced_any {
            span.record(turn::COST, self.cost_total);
            if let Some(unit) = self.pricing_unit {
                span.record(turn::PRICING_UNIT, unit);
            }
        }
        if matches!(end_reason, "upstream_error" | "error") {
            span.record("otel.status_code", "ERROR");
        }
        // Dropping `self` ends the span.
    }
}

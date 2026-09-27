//! What crosses the relay, per event (FRD-023 §5.5, §5.6, D-11, D-12, S-5, S-6).
//!
//! Pure functions over the event text. The session loop calls [`client_event`] for every client
//! frame and [`vendor_event`] for every vendor frame; nothing here performs I/O.
//!
//! **Two-stage parse (R-2).** Every frame's envelope (`type`, `event_id`) is read; the body only
//! for the event types a rule inspects. Audio-bearing events — `input_audio_buffer.append` from the
//! client, `response.output_audio.delta` from the vendor — are forwarded as the original text,
//! never re-serialised.
//!
//! **Refuse loudly, strip only the routine.** A refused client event is not forwarded and the
//! client gets an `error` naming the field; the session continues. Only `model` and `tracing` are
//! removed silently, because the SDKs send them on every `session.update`.

use std::borrow::Cow;

use bud_auth::{RealtimePolicy, RealtimeSettings};
use serde::Deserialize;
use serde_json::{Map, Value};

/// The first stage: just enough to route.
#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    #[serde(default, borrow)]
    event_id: Option<Cow<'a, str>>,
}

/// Client events forwarded on the envelope alone.
const PASS_THROUGH: &[&str] = &[
    "input_audio_buffer.append",
    "input_audio_buffer.commit",
    "input_audio_buffer.clear",
    "conversation.item.retrieve",
    "conversation.item.truncate",
    "conversation.item.delete",
    "response.cancel",
    "output_audio_buffer.clear",
];

/// Client events the rules inspect.
const INSPECTED: &[&str] = &[
    "session.update",
    "response.create",
    "conversation.item.create",
];

/// A deployment's client-facing rules.
#[derive(Debug, Clone, Default)]
pub struct ClientRules {
    pub policy: RealtimePolicy,
    /// `realtime` or `transcription`: a client may not change it.
    pub session_type: String,
    /// The deployment's output cap, when it sets one.
    pub max_output_tokens: Option<u32>,
}

impl ClientRules {
    pub fn from_settings(settings: Option<&RealtimeSettings>) -> Self {
        let settings = settings.cloned().unwrap_or_default();
        Self {
            policy: settings.policy(),
            session_type: settings
                .session_type
                .clone()
                .unwrap_or_else(|| "realtime".to_string()),
            max_output_tokens: settings.defaults.as_ref().and_then(|d| d.max_output_tokens),
        }
    }
}

/// A policy refusal: the field, and what to tell the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub param: String,
    pub message: String,
    pub event_id: Option<String>,
}

/// What to do with one client frame.
#[derive(Debug, PartialEq, Eq)]
pub enum ClientOutcome<'a> {
    /// Forward this text. `response_create` asks the loop to revalidate first (D-17).
    Forward {
        text: Cow<'a, str>,
        kind: String,
        response_create: bool,
        /// The type is not one this gateway knows (forwarded; counted).
        unknown: bool,
    },
    Refuse(Refusal),
    /// Not a JSON event at all.
    Invalid(String),
}

fn refusal(
    param: &str,
    message: impl Into<String>,
    event_id: Option<&str>,
) -> ClientOutcome<'static> {
    ClientOutcome::Refuse(Refusal {
        param: param.to_string(),
        message: message.into(),
        event_id: event_id.map(str::to_string),
    })
}

fn has_mcp_tool(tools: Option<&Value>) -> bool {
    tools.and_then(Value::as_array).is_some_and(|t| {
        t.iter()
            .any(|tool| tool.get("type").and_then(Value::as_str) == Some("mcp"))
    })
}

fn has_image(content: Option<&Value>) -> bool {
    content.and_then(Value::as_array).is_some_and(|parts| {
        parts
            .iter()
            .any(|p| p.get("type").and_then(Value::as_str) == Some("input_image"))
    })
}

/// Clamp a `max_output_tokens` (a number or `"inf"`) to the deployment's cap.
fn clamp_output_tokens(obj: &mut Map<String, Value>, cap: Option<u32>) -> bool {
    let Some(cap) = cap else { return false };
    let over = match obj.get("max_output_tokens") {
        None => false,
        Some(Value::String(s)) => s == "inf",
        Some(v) => v.as_u64().is_none_or(|n| n > u64::from(cap)),
    };
    if over {
        obj.insert("max_output_tokens".into(), Value::from(cap));
    }
    over
}

/// Rules shared by a session and a per-response override (`instructions`, `tools`, `prompt`).
fn check_overridable(
    obj: &Map<String, Value>,
    prefix: &str,
    rules: &ClientRules,
    event_id: Option<&str>,
) -> Option<ClientOutcome<'static>> {
    if obj.get("instructions").is_some() && !rules.policy.allows_client_instructions() {
        return Some(refusal(
            &format!("{prefix}.instructions"),
            "This deployment fixes its instructions; `instructions` may not be set by the client.",
            event_id,
        ));
    }
    if has_mcp_tool(obj.get("tools")) && !rules.policy.allows_mcp_tools() {
        return Some(refusal(
            &format!("{prefix}.tools.mcp"),
            "MCP tools are not enabled on this deployment (they run in the vendor's organization).",
            event_id,
        ));
    }
    if obj.get("prompt").is_some_and(|p| !p.is_null()) && !rules.policy.allows_prompt_references() {
        return Some(refusal(
            &format!("{prefix}.prompt"),
            "Stored prompt references are not enabled on this deployment.",
            event_id,
        ));
    }
    None
}

fn apply_session_update(
    event: &mut Value,
    rules: &ClientRules,
    event_id: Option<&str>,
) -> Option<ClientOutcome<'static>> {
    let Some(session) = event.get_mut("session").and_then(Value::as_object_mut) else {
        return None;
    };
    // Routine: SDKs send both on every update. `model` is the deployment's (D-3); `tracing`
    // would open a trace in the vendor org every project on the credential shares (D-11).
    session.remove("model");
    session.remove("tracing");

    if let Some(kind) = session.get("type").and_then(Value::as_str)
        && kind != rules.session_type
    {
        return Some(refusal(
            "session.type",
            format!(
                "This deployment serves `{}` sessions; `session.type` cannot be changed.",
                rules.session_type
            ),
            event_id,
        ));
    }
    if let Some(refused) = check_overridable(session, "session", rules, event_id) {
        return Some(refused);
    }
    if let Some(model) = session
        .get("audio")
        .and_then(|a| a.get("input"))
        .and_then(|i| i.get("transcription"))
        .and_then(|t| t.get("model"))
        .and_then(Value::as_str)
        && !rules.policy.allows_transcription_model(model)
    {
        return Some(refusal(
            "session.audio.input.transcription.model",
            format!("Transcription model '{model}' is not enabled on this deployment."),
            event_id,
        ));
    }
    clamp_output_tokens(session, rules.max_output_tokens);
    None
}

fn apply_response_create(
    event: &mut Value,
    rules: &ClientRules,
    event_id: Option<&str>,
) -> Option<ClientOutcome<'static>> {
    let Some(response) = event.get_mut("response").and_then(Value::as_object_mut) else {
        return None;
    };
    response.remove("model");
    if let Some(refused) = check_overridable(response, "response", rules, event_id) {
        return Some(refused);
    }
    if !rules.policy.allows_image_input()
        && response
            .get("input")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|i| has_image(i.get("content"))))
    {
        return Some(refusal(
            "response.input.content.input_image",
            "Image input is not enabled on this deployment.",
            event_id,
        ));
    }
    clamp_output_tokens(response, rules.max_output_tokens);
    None
}

/// Apply the client → vendor rules to one frame (FRD §5.5).
pub fn client_event<'a>(raw: &'a str, rules: &ClientRules) -> ClientOutcome<'a> {
    let env: Envelope = match serde_json::from_str(raw) {
        Ok(e) => e,
        Err(e) => return ClientOutcome::Invalid(format!("not a JSON event with a `type`: {e}")),
    };
    let kind = env.kind.to_string();

    if PASS_THROUGH.contains(&kind.as_str()) {
        return ClientOutcome::Forward {
            text: Cow::Borrowed(raw),
            kind,
            response_create: false,
            unknown: false,
        };
    }
    if !INSPECTED.contains(&kind.as_str()) {
        // Forward-compatible with vendor additions (R-6); counted by the caller.
        return ClientOutcome::Forward {
            text: Cow::Borrowed(raw),
            kind,
            response_create: false,
            unknown: true,
        };
    }

    let event_id = env.event_id.map(|e| e.to_string());
    let mut event: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return ClientOutcome::Invalid(e.to_string()),
    };
    let refused = match kind.as_str() {
        "session.update" => apply_session_update(&mut event, rules, event_id.as_deref()),
        "response.create" => apply_response_create(&mut event, rules, event_id.as_deref()),
        "conversation.item.create" => {
            let image = has_image(event.get("item").and_then(|i| i.get("content")));
            (image && !rules.policy.allows_image_input()).then(|| {
                refusal(
                    "item.content.input_image",
                    "Image input is not enabled on this deployment.",
                    event_id.as_deref(),
                )
            })
        }
        _ => None,
    };
    if let Some(refused) = refused {
        return refused;
    }
    ClientOutcome::Forward {
        text: Cow::Owned(event.to_string()),
        response_create: kind == "response.create",
        kind,
        unknown: false,
    }
}

/// What a vendor frame means to the session, beyond forwarding it.
#[derive(Debug, PartialEq)]
pub enum Tap {
    None,
    SessionCreated { vendor_session_id: Option<String> },
    SessionUpdated { event_id: Option<String> },
    ResponseDone(Value),
    TranscriptionCompleted(Value),
    Error(Value),
}

/// What to do with one vendor frame.
#[derive(Debug, PartialEq)]
pub enum VendorOutcome<'a> {
    Forward(Cow<'a, str>, Tap),
    /// Never reaches the client (D-12).
    Drop,
    Invalid,
}

/// Rewrite `session.model` to the name the client connected with (§5.5 taps).
fn rewrite_session_model(event: &mut Value, deployment: &str) {
    if let Some(session) = event.get_mut("session").and_then(Value::as_object_mut)
        && session.contains_key("model")
    {
        session.insert("model".into(), Value::from(deployment));
    }
}

/// Apply the vendor → client taps to one frame (FRD §5.5).
pub fn vendor_event<'a>(raw: &'a str, deployment: &str) -> VendorOutcome<'a> {
    let env: Envelope = match serde_json::from_str(raw) {
        Ok(e) => e,
        Err(_) => return VendorOutcome::Invalid,
    };
    match env.kind.as_ref() {
        // It reports the VENDOR ORG's remaining budget across every tenant on the credential.
        "rate_limits.updated" => VendorOutcome::Drop,
        "session.created" | "session.updated" => {
            let created = env.kind == "session.created";
            let event_id = env.event_id.map(|e| e.to_string());
            let Ok(mut event) = serde_json::from_str::<Value>(raw) else {
                return VendorOutcome::Invalid;
            };
            let vendor_session_id = event
                .get("session")
                .and_then(|s| s.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            rewrite_session_model(&mut event, deployment);
            let tap = if created {
                Tap::SessionCreated { vendor_session_id }
            } else {
                Tap::SessionUpdated { event_id }
            };
            VendorOutcome::Forward(Cow::Owned(event.to_string()), tap)
        }
        "response.done" => match serde_json::from_str::<Value>(raw) {
            Ok(v) => VendorOutcome::Forward(Cow::Borrowed(raw), Tap::ResponseDone(v)),
            Err(_) => VendorOutcome::Invalid,
        },
        "conversation.item.input_audio_transcription.completed" => {
            match serde_json::from_str::<Value>(raw) {
                Ok(v) => VendorOutcome::Forward(Cow::Borrowed(raw), Tap::TranscriptionCompleted(v)),
                Err(_) => VendorOutcome::Invalid,
            }
        }
        "error" => match serde_json::from_str::<Value>(raw) {
            Ok(v) => VendorOutcome::Forward(Cow::Borrowed(raw), Tap::Error(v)),
            Err(_) => VendorOutcome::Invalid,
        },
        _ => VendorOutcome::Forward(Cow::Borrowed(raw), Tap::None),
    }
}

/// The `session.update` WaaV sends first, built from the deployment's defaults (FRD §5.6).
///
/// `None` when a realtime deployment has no defaults — the vendor's apply. A transcription
/// deployment always gets one: its session type and model are set here.
pub fn defaults_update(
    settings: Option<&RealtimeSettings>,
    vendor_model: Option<&str>,
    event_id: &str,
) -> Option<String> {
    let transcription = settings.is_some_and(RealtimeSettings::is_transcription);
    let defaults = settings
        .and_then(|s| s.defaults.clone())
        .unwrap_or_default();

    let mut session = Map::new();
    session.insert(
        "type".into(),
        Value::from(if transcription {
            "transcription"
        } else {
            "realtime"
        }),
    );
    let mut input = Map::new();
    let mut output = Map::new();

    let mut transcription_cfg = Map::new();
    if let Some(t) = &defaults.input_transcription {
        if let Some(m) = &t.model {
            transcription_cfg.insert("model".into(), Value::from(m.as_str()));
        }
        if let Some(l) = &t.language {
            transcription_cfg.insert("language".into(), Value::from(l.as_str()));
        }
        if let Some(p) = &t.prompt {
            transcription_cfg.insert("prompt".into(), Value::from(p.as_str()));
        }
    }
    if transcription && let Some(model) = vendor_model {
        // A transcription deployment's model IS its transcription model.
        transcription_cfg.insert("model".into(), Value::from(model));
    }
    if !transcription_cfg.is_empty() {
        input.insert("transcription".into(), Value::Object(transcription_cfg));
    }
    if let Some(td) = &defaults.turn_detection {
        input.insert("turn_detection".into(), td.clone());
    }
    if let Some(nr) = &defaults.noise_reduction {
        input.insert("noise_reduction".into(), serde_json::json!({ "type": nr }));
    }
    if !transcription {
        if let Some(v) = &defaults.voice {
            output.insert("voice".into(), Value::from(v.as_str()));
        }
        if let Some(s) = defaults.speed {
            output.insert("speed".into(), Value::from(s));
        }
        if let Some(i) = &defaults.instructions {
            session.insert("instructions".into(), Value::from(i.as_str()));
        }
        if let Some(m) = &defaults.output_modalities {
            session.insert("output_modalities".into(), Value::from(m.clone()));
        }
        if let Some(m) = defaults.max_output_tokens {
            session.insert("max_output_tokens".into(), Value::from(m));
        }
    }

    let mut audio = Map::new();
    if !input.is_empty() {
        audio.insert("input".into(), Value::Object(input));
    }
    if !output.is_empty() {
        audio.insert("output".into(), Value::Object(output));
    }
    if !audio.is_empty() {
        session.insert("audio".into(), Value::Object(audio));
    }
    if !transcription && session.len() == 1 {
        return None; // only `type`: nothing to apply
    }
    Some(
        serde_json::json!({
            "type": "session.update",
            "event_id": event_id,
            "session": Value::Object(session),
        })
        .to_string(),
    )
}

/// An OpenAI `error` event from the gateway.
pub fn error_event(
    event_id: &str,
    kind: &str,
    code: &str,
    message: &str,
    param: Option<&str>,
    client_event_id: Option<&str>,
) -> String {
    let mut error = serde_json::json!({"type": kind, "code": code, "message": message});
    if let Some(p) = param {
        error["param"] = Value::from(p);
    }
    if let Some(e) = client_event_id {
        error["event_id"] = Value::from(e);
    }
    serde_json::json!({"type": "error", "event_id": event_id, "error": error}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> ClientRules {
        ClientRules {
            policy: RealtimePolicy {
                input_transcription_models: Some(vec!["gpt-4o-mini-transcribe".into()]),
                ..Default::default()
            },
            session_type: "realtime".into(),
            max_output_tokens: Some(4096),
        }
    }

    fn forwarded(outcome: ClientOutcome<'_>) -> Value {
        match outcome {
            ClientOutcome::Forward { text, .. } => serde_json::from_str(&text).unwrap(),
            other => panic!("expected forward, got {other:?}"),
        }
    }

    fn refused(outcome: ClientOutcome<'_>) -> Refusal {
        match outcome {
            ClientOutcome::Refuse(r) => r,
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    /// TC-EVT-01
    #[test]
    fn tc_evt_01_model_is_stripped_silently() {
        let out = forwarded(client_event(
            r#"{"type":"session.update","session":{"type":"realtime","model":"gpt-4o-realtime-preview","voice":"x"}}"#,
            &rules(),
        ));
        assert!(out["session"].get("model").is_none());
        assert_eq!(out["session"]["voice"], "x");
    }

    /// TC-EVT-02 🔒 / TC-EVT-03
    #[test]
    fn tc_evt_02_mcp_tools_are_refused_unless_enabled() {
        let raw = r#"{"type":"session.update","event_id":"c1","session":{"tools":[{"type":"mcp","server_url":"https://x"}]}}"#;
        let r = refused(client_event(raw, &rules()));
        assert_eq!(r.param, "session.tools.mcp");
        assert_eq!(r.event_id.as_deref(), Some("c1"));

        let mut open = rules();
        open.policy.allow_mcp_tools = Some(true);
        forwarded(client_event(raw, &open));
    }

    /// TC-EVT-04 🔒
    #[test]
    fn tc_evt_04_stored_prompts_are_refused_by_default() {
        let r = refused(client_event(
            r#"{"type":"session.update","session":{"prompt":{"id":"pmpt_x"}}}"#,
            &rules(),
        ));
        assert_eq!(r.param, "session.prompt");
    }

    /// TC-EVT-05
    #[test]
    fn tc_evt_05_tracing_is_stripped() {
        let out = forwarded(client_event(
            r#"{"type":"session.update","session":{"tracing":"auto"}}"#,
            &rules(),
        ));
        assert!(out["session"].get("tracing").is_none());
    }

    /// TC-EVT-06 🔒
    #[test]
    fn tc_evt_06_transcription_model_allowlist() {
        let r = refused(client_event(
            r#"{"type":"session.update","session":{"audio":{"input":{"transcription":{"model":"gpt-4o-transcribe"}}}}}"#,
            &rules(),
        ));
        assert_eq!(r.param, "session.audio.input.transcription.model");
        forwarded(client_event(
            r#"{"type":"session.update","session":{"audio":{"input":{"transcription":{"model":"gpt-4o-mini-transcribe"}}}}}"#,
            &rules(),
        ));
    }

    /// TC-EVT-07
    #[test]
    fn tc_evt_07_session_type_is_locked() {
        let r = refused(client_event(
            r#"{"type":"session.update","session":{"type":"transcription"}}"#,
            &rules(),
        ));
        assert_eq!(r.param, "session.type");
    }

    /// TC-EVT-08
    #[test]
    fn tc_evt_08_instructions_lock_applies_to_session_and_response() {
        let mut locked = rules();
        locked.policy.allow_client_instructions = Some(false);
        let r = refused(client_event(
            r#"{"type":"session.update","session":{"instructions":"be rude"}}"#,
            &locked,
        ));
        assert_eq!(r.param, "session.instructions");
        let r = refused(client_event(
            r#"{"type":"response.create","response":{"instructions":"be rude"}}"#,
            &locked,
        ));
        assert_eq!(r.param, "response.instructions");
    }

    /// TC-EVT-09
    #[test]
    fn tc_evt_09_output_cap_is_clamped() {
        let out = forwarded(client_event(
            r#"{"type":"session.update","session":{"max_output_tokens":"inf"}}"#,
            &rules(),
        ));
        assert_eq!(out["session"]["max_output_tokens"], 4096);
        let out = forwarded(client_event(
            r#"{"type":"response.create","response":{"max_output_tokens":100000}}"#,
            &rules(),
        ));
        assert_eq!(out["response"]["max_output_tokens"], 4096);
        let out = forwarded(client_event(
            r#"{"type":"response.create","response":{"max_output_tokens":10}}"#,
            &rules(),
        ));
        assert_eq!(out["response"]["max_output_tokens"], 10);
    }

    /// TC-EVT-10
    #[test]
    fn tc_evt_10_image_input_refused_when_disabled() {
        let mut no_images = rules();
        no_images.policy.allow_image_input = Some(false);
        let raw = r#"{"type":"conversation.item.create","item":{"type":"message","role":"user","content":[{"type":"input_image","image_url":"data:x"}]}}"#;
        assert_eq!(
            refused(client_event(raw, &no_images)).param,
            "item.content.input_image"
        );
        forwarded(client_event(raw, &rules()));
    }

    /// TC-EVT-11
    #[test]
    fn tc_evt_11_unknown_events_are_forwarded_and_flagged() {
        match client_event(r#"{"type":"future.event","x":1}"#, &rules()) {
            ClientOutcome::Forward { unknown, text, .. } => {
                assert!(unknown);
                assert_eq!(text, r#"{"type":"future.event","x":1}"#);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn audio_frames_are_forwarded_byte_for_byte() {
        let raw = r#"{"type":"input_audio_buffer.append","audio":"AAAA//8="}"#;
        match client_event(raw, &rules()) {
            ClientOutcome::Forward { text, .. } => {
                assert!(matches!(text, Cow::Borrowed(t) if t == raw))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn response_create_asks_for_revalidation() {
        match client_event(r#"{"type":"response.create"}"#, &rules()) {
            ClientOutcome::Forward {
                response_create, ..
            } => assert!(response_create),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn junk_is_invalid_not_forwarded() {
        assert!(matches!(
            client_event("not json", &rules()),
            ClientOutcome::Invalid(_)
        ));
        assert!(matches!(
            client_event(r#"{"no":"type"}"#, &rules()),
            ClientOutcome::Invalid(_)
        ));
    }

    /// TC-EVT-12 🔒
    #[test]
    fn tc_evt_12_vendor_rate_limits_are_dropped() {
        assert_eq!(
            vendor_event(r#"{"type":"rate_limits.updated","rate_limits":[]}"#, "rt"),
            VendorOutcome::Drop
        );
    }

    /// TC-EVT-13
    #[test]
    fn tc_evt_13_session_model_is_rewritten_to_the_deployment() {
        match vendor_event(
            r#"{"type":"session.created","session":{"id":"sess_1","model":"gpt-realtime-2.1"}}"#,
            "my-rt",
        ) {
            VendorOutcome::Forward(text, Tap::SessionCreated { vendor_session_id }) => {
                let v: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(v["session"]["model"], "my-rt");
                assert_eq!(vendor_session_id.as_deref(), Some("sess_1"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn response_done_is_tapped_and_forwarded_untouched() {
        let raw = r#"{"type":"response.done","response":{"id":"r1","usage":{}}}"#;
        match vendor_event(raw, "rt") {
            VendorOutcome::Forward(text, Tap::ResponseDone(v)) => {
                assert_eq!(text, raw);
                assert_eq!(v["response"]["id"], "r1");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn audio_deltas_pass_through_untouched() {
        let raw = r#"{"type":"response.output_audio.delta","delta":"AAAA"}"#;
        assert_eq!(
            vendor_event(raw, "rt"),
            VendorOutcome::Forward(Cow::Borrowed(raw), Tap::None)
        );
    }

    /// TC-EVT-14 (builder half): the defaults update carries the deployment's voice and settings.
    #[test]
    fn tc_evt_14_defaults_update_shape() {
        let settings: RealtimeSettings = serde_json::from_value(serde_json::json!({
            "session_type": "realtime",
            "defaults": {
                "voice": "marin", "instructions": "hi", "output_modalities": ["audio"],
                "turn_detection": {"type": "semantic_vad", "eagerness": "auto"},
                "input_transcription": {"model": "gpt-4o-mini-transcribe", "language": "en"},
                "noise_reduction": "near_field", "max_output_tokens": 4096, "speed": 1.0
            }
        }))
        .unwrap();
        let update: Value = serde_json::from_str(
            &defaults_update(Some(&settings), Some("gpt-realtime-2.1"), "evt_bud_d1").unwrap(),
        )
        .unwrap();
        assert_eq!(update["type"], "session.update");
        assert_eq!(update["event_id"], "evt_bud_d1");
        let s = &update["session"];
        assert_eq!(s["type"], "realtime");
        assert_eq!(s["instructions"], "hi");
        assert_eq!(s["max_output_tokens"], 4096);
        assert_eq!(s["audio"]["output"]["voice"], "marin");
        assert_eq!(s["audio"]["output"]["speed"], 1.0);
        assert_eq!(
            s["audio"]["input"]["turn_detection"]["type"],
            "semantic_vad"
        );
        assert_eq!(
            s["audio"]["input"]["transcription"]["model"],
            "gpt-4o-mini-transcribe"
        );
        assert_eq!(s["audio"]["input"]["noise_reduction"]["type"], "near_field");
        assert!(s.get("model").is_none());
    }

    #[test]
    fn a_realtime_deployment_without_defaults_sends_no_update() {
        assert!(defaults_update(None, Some("m"), "e").is_none());
    }

    #[test]
    fn a_transcription_deployment_always_sets_its_type_and_model() {
        let settings: RealtimeSettings =
            serde_json::from_value(serde_json::json!({"session_type": "transcription"})).unwrap();
        let update: Value = serde_json::from_str(
            &defaults_update(Some(&settings), Some("gpt-4o-transcribe"), "e").unwrap(),
        )
        .unwrap();
        assert_eq!(update["session"]["type"], "transcription");
        assert_eq!(
            update["session"]["audio"]["input"]["transcription"]["model"],
            "gpt-4o-transcribe"
        );
    }

    #[test]
    fn the_error_event_is_openai_shaped() {
        let e: Value = serde_json::from_str(&error_event(
            "evt_bud_1",
            "invalid_request_error",
            "event_not_allowed",
            "no",
            Some("session.prompt"),
            Some("c9"),
        ))
        .unwrap();
        assert_eq!(e["type"], "error");
        assert_eq!(e["error"]["code"], "event_not_allowed");
        assert_eq!(e["error"]["param"], "session.prompt");
        assert_eq!(e["error"]["event_id"], "c9");
    }
}

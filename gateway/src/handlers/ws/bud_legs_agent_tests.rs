//! Spec 025 §5.4 over an in-memory control plane: a voice-agent session resolves `prompt:<name>`
//! through the caller's OWN allowlist, takes both legs from the agent's projection (D-6), admits them,
//! and is revalidated against the agent — never against the legs (S-8).

use super::*;
use crate::test_support::{TEST_CREDENTIAL_PLAIN, bud_state_with_credentials, test_credential};
use serde_json::{Value as Json, json};

const KEY: &str = "bud_agent_test_key";
const OTHER_KEY: &str = "bud_agent_other_key";
const PROJECT: &str = "7c0c7e1d-0000-4000-8000-00000000aa01";
const PROMPT_UUID: &str = "c0de0000-0000-4000-8000-000000000003";

fn stt_entry() -> Json {
    json!({"vendor": "deepgram", "credential": test_credential(), "endpoints": ["audio_transcription"],
           "model": "nova-3", "language": "en-US"})
}

fn tts_entry() -> Json {
    json!({"vendor": "elevenlabs", "credential": test_credential(), "endpoints": ["text_to_speech"],
           "model": "eleven_flash_v2_5", "voice": "deployment-voice"})
}

fn projection(version: i64, stt: &str, extra: Json) -> String {
    let mut v = json!({
        "v": 1, "prompt_id": PROMPT_UUID, "prompt_name": "support", "version": version, "project_id": PROJECT,
        "stt": {"endpoint_id": stt, "language": "en", "keyterms": ["Bud"]},
        "tts": {"endpoint_id": "ep-tts", "voice": "agent-voice", "speed": 1.1},
        "turn_detection": {"type": "semantic", "eagerness": "high"},
        "session_overrides": [],
    });
    if let (Some(b), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        b.extend(e.clone());
    }
    v.to_string()
}

/// KEY reaches the agent (`prompt:support` + `prompt:support:v2`) and NOTHING else — in particular
/// not the agent's speech deployments. OTHER_KEY reaches only a model.
async fn plane(extra_projection: Json) -> (Arc<AppState>, Arc<bud_auth::MemoryStore>) {
    let agent = |v: i64| json!({"prompt_id": PROMPT_UUID, "endpoint_id": "ep-llm", "project_id": PROJECT, "kind": "agent", "version": v});
    let keys: Vec<(String, String)> = vec![
        (
            format!("api_key:{}", bud_auth::hash_api_key(KEY)),
            json!({"prompt:support": agent(2), "prompt:support:v1": agent(1), "prompt:support:v2": agent(2),
                   "__metadata__": {"api_key_id": "ak1", "user_id": "u1", "api_key_project_id": PROJECT}})
            .to_string(),
        ),
        (
            format!("api_key:{}", bud_auth::hash_api_key(OTHER_KEY)),
            json!({"some-model": {"endpoint_id": "ep-llm", "project_id": PROJECT, "kind": "model"},
                   "__metadata__": {"api_key_id": "ak2"}})
            .to_string(),
        ),
        ("voice_table:ep-stt".into(), json!({"ep-stt": stt_entry()}).to_string()),
        ("voice_table:ep-tts".into(), json!({"ep-tts": tts_entry()}).to_string()),
        (format!("voice_agent:{PROMPT_UUID}:v2"), projection(2, "ep-stt", extra_projection)),
    ];
    let refs: Vec<(&str, &str)> = keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    bud_state_with_credentials(&refs).await
}

fn stt_cfg(v: Json) -> STTWebSocketConfig {
    let mut base =
        json!({"language": "", "sample_rate": 16000, "channels": 1, "punctuation": true});
    if let (Some(b), Some(e)) = (base.as_object_mut(), v.as_object()) {
        b.extend(e.clone());
    }
    serde_json::from_value(base).unwrap()
}

fn tts_cfg(v: Json) -> TTSWebSocketConfig {
    serde_json::from_value(v).unwrap()
}

async fn prepare(
    state: &Arc<AppState>,
    key: &str,
    model: &str,
    stt: &mut STTWebSocketConfig,
    tts: &mut TTSWebSocketConfig,
) -> Result<PreparedAgent, LegRefusal> {
    prepare_agent(
        state,
        Some(&crate::auth::SessionCredential::new(key)),
        model,
        stt,
        tts,
        "sess-1",
    )
    .await
}

/// TC-SES-01 🔒 / TC-SEC-01 🔒 — the agent's legs, not the caller's: vendor, model and credential come
/// from the projection's deployments, which the caller's allowlist never names (D-6).
#[tokio::test]
async fn the_agent_authorizes_its_own_legs() {
    let (state, _) = plane(json!({})).await;
    let mut stt = stt_cfg(json!({}));
    let mut tts = tts_cfg(json!({"sample_rate": 24000}));
    let p = prepare(&state, KEY, "prompt:support", &mut stt, &mut tts)
        .await
        .expect("served");

    assert_eq!(p.agent.prompt_name, "support");
    assert_eq!(p.agent.version, 2, "the default version, pinned (D-12)");
    assert_eq!(stt.provider, "deepgram");
    assert_eq!(stt.model, "nova-3");
    assert_eq!(stt.language, "en", "the agent's language");
    assert_eq!(
        stt.features.keyterms.as_deref(),
        Some(&["Bud".to_string()][..])
    );
    assert!(
        stt.turn_detection
            .as_ref()
            .is_some_and(|t| t.enabled && t.threshold == Some(0.3))
    );
    assert_eq!(tts.provider, "elevenlabs");
    assert_eq!(
        tts.voice_id.as_deref(),
        Some("agent-voice"),
        "the agent's voice beats the deployment's"
    );
    assert_eq!(tts.speaking_rate, Some(1.1));
    assert_eq!(
        tts.sample_rate,
        Some(24000),
        "the client's audio format is kept"
    );
    assert_eq!(p.legs.stt_key, TEST_CREDENTIAL_PLAIN);
    assert_eq!(p.legs.tts_key, TEST_CREDENTIAL_PLAIN);
    assert_eq!(p.legs.admissions.len(), 2);
    let grant = p.legs.meter.agent().expect("an agent session");
    assert_eq!(grant.alias, "prompt:support");
    assert!(
        state
            .resolve_voice_endpoint("ep-stt", STT_CAPABILITY, Some(KEY))
            .is_none(),
        "the caller cannot reach the leg directly — only through the agent"
    );
}

/// TC-SES-02 — a pinned version uses that version's projection.
#[tokio::test]
async fn a_pinned_version_without_voice_is_refused() {
    let (state, _) = plane(json!({})).await;
    let (mut stt, mut tts) = (stt_cfg(json!({})), tts_cfg(json!({})));
    let err = prepare(&state, KEY, "prompt:support:v1", &mut stt, &mut tts)
        .await
        .unwrap_err();
    assert_eq!(err.code, "agent_not_voice_enabled");
    let ok = prepare(
        &state,
        KEY,
        "prompt:support:v2",
        &mut stt_cfg(json!({})),
        &mut tts_cfg(json!({})),
    )
    .await;
    assert_eq!(ok.unwrap().agent.version, 2);
}

/// TC-SEC-02 🔒 — a caller whose allowlist does not hold the agent cannot reach it.
#[tokio::test]
async fn an_agent_outside_the_callers_allowlist_is_not_found() {
    let (state, _) = plane(json!({})).await;
    let err = prepare(
        &state,
        OTHER_KEY,
        "prompt:support",
        &mut stt_cfg(json!({})),
        &mut tts_cfg(json!({})),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "model_not_found");
    let err = prepare(
        &state,
        KEY,
        "prompt:nope",
        &mut stt_cfg(json!({})),
        &mut tts_cfg(json!({})),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "model_not_found");
}

/// S-2 🔒 — a client can never name, add or replace a leg deployment.
#[tokio::test]
async fn a_client_cannot_choose_the_legs_or_bring_a_key() {
    let (state, _) = plane(json!({})).await;
    let err = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt_cfg(json!({"model": "stt-other"})),
        &mut tts_cfg(json!({})),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "agent_owns_legs");
    let err = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt_cfg(json!({})),
        &mut tts_cfg(json!({"api_key": "sk-mine"})),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "client_key_not_accepted");
}

/// S-4 🔒 — caller overrides only where the agent allows them.
#[tokio::test]
async fn overrides_apply_only_where_the_agent_allows() {
    let (state, _) = plane(json!({})).await;
    let mut tts = tts_cfg(json!({"voice_id": "caller-voice", "speaking_rate": 2.0}));
    prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt_cfg(json!({"language": "fr"})),
        &mut tts,
    )
    .await
    .unwrap();
    assert_eq!(tts.voice_id.as_deref(), Some("agent-voice"));
    assert_eq!(tts.speaking_rate, Some(1.1));

    let (state, _) = plane(json!({"session_overrides": ["tts.voice", "stt.language"]})).await;
    let mut stt = stt_cfg(json!({"language": "fr"}));
    let mut tts = tts_cfg(json!({"voice_id": "caller-voice", "speaking_rate": 2.0}));
    prepare(&state, KEY, "prompt:support", &mut stt, &mut tts)
        .await
        .unwrap();
    assert_eq!(tts.voice_id.as_deref(), Some("caller-voice"));
    assert_eq!(stt.language, "fr");
    assert_eq!(
        tts.speaking_rate,
        Some(1.1),
        "speed is not in this agent's overrides"
    );
}

fn advised(p: &PreparedAgent, needle: &str) -> bool {
    p.legs
        .advisories
        .as_slice()
        .iter()
        .any(|a| a.contains(needle))
}

/// S-4 🔒 — a caller's `turn_detection` is only an eagerness override, and only where the agent
/// allows one; an agent never speculates, whatever the caller asks.
#[tokio::test]
async fn a_callers_turn_detection_is_an_eagerness_override_only_where_allowed() {
    let caller = json!({"turn_detection": {"enabled": true, "threshold": 0.9, "eager": true}});

    let (state, _) = plane(json!({})).await;
    let mut stt = stt_cfg(caller.clone());
    let p = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt,
        &mut tts_cfg(json!({})),
    )
    .await
    .unwrap();
    let td = stt
        .turn_detection
        .expect("the agent's semantic turn detection");
    assert!(td.enabled && !td.eager);
    assert_eq!(td.threshold, Some(0.3), "the agent's eagerness (high)");
    assert!(
        advised(&p, "turn_detection"),
        "{:?}",
        p.legs.advisories.as_slice()
    );

    let (state, _) = plane(json!({"session_overrides": ["turn_detection.eagerness"]})).await;
    let mut stt = stt_cfg(caller);
    let p = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt,
        &mut tts_cfg(json!({})),
    )
    .await
    .unwrap();
    let td = stt.turn_detection.unwrap();
    assert_eq!(td.threshold, Some(0.9), "the caller's eagerness");
    assert!(td.enabled && !td.eager, "an agent never speculates");
    assert!(!advised(&p, "turn_detection"));
}

/// A manual agent's turns are committed by the client; no caller turns its detector back on.
#[tokio::test]
async fn a_manual_agent_keeps_its_turn_taking() {
    let (state, _) = plane(json!({
        "turn_detection": {"type": "manual"},
        "session_overrides": ["turn_detection.eagerness"],
    }))
    .await;
    let mut stt = stt_cfg(json!({"turn_detection": {"enabled": true, "threshold": 0.5}}));
    let p = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt,
        &mut tts_cfg(json!({})),
    )
    .await
    .unwrap();
    assert!(stt.turn_detection.is_none());
    assert!(advised(&p, "turn_detection"));
}

/// S-4 — an override the agent does not allow is reported to the caller, never dropped silently;
/// sending the agent's own value is not an override.
#[tokio::test]
async fn a_refused_override_is_reported() {
    let (state, _) = plane(json!({})).await;
    let mut tts = tts_cfg(json!({"voice_id": "caller-voice", "speaking_rate": 2.0}));
    let p = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt_cfg(json!({"language": "fr"})),
        &mut tts,
    )
    .await
    .unwrap();
    for setting in ["tts.voice", "tts.speed", "stt.language"] {
        assert!(
            advised(&p, setting),
            "{setting}: {:?}",
            p.legs.advisories.as_slice()
        );
    }

    let mut tts = tts_cfg(json!({"voice_id": "agent-voice", "speaking_rate": 1.1}));
    let p = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt_cfg(json!({"language": "en"})),
        &mut tts,
    )
    .await
    .unwrap();
    for setting in ["tts.voice", "tts.speed", "stt.language"] {
        assert!(
            !advised(&p, setting),
            "{setting}: {:?}",
            p.legs.advisories.as_slice()
        );
    }
}

/// R-5 / DEG — a leg that is gone (or never streamed) fails at connect, by name.
#[tokio::test]
async fn an_unavailable_leg_is_refused_by_name() {
    let (state, _) = plane(json!({"stt": {"endpoint_id": "ep-missing"}})).await;
    let err = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt_cfg(json!({})),
        &mut tts_cfg(json!({})),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "voice_leg_unavailable");
    assert!(err.message.contains("support"));
}

/// TC-SEC-05 🔒 / S-8 — revalidation follows the AGENT: its alias, its projection, its legs.
#[tokio::test]
async fn revalidation_follows_the_agent() {
    let (state, store) = plane(json!({})).await;
    let p = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt_cfg(json!({})),
        &mut tts_cfg(json!({})),
    )
    .await
    .unwrap();
    let meter = p.legs.meter;
    assert!(
        session_still_allowed(&state, &meter).await,
        "an agent session is allowed by its alias"
    );

    // The projection moves the agent to another STT deployment: this session was admitted to the old one.
    let plane = state.bud_mode.as_ref().unwrap().plane();
    store.set(
        &format!("voice_agent:{PROMPT_UUID}:v2"),
        &projection(2, "ep-other", json!({})),
    );
    plane
        .on_key_event(
            &format!("voice_agent:{PROMPT_UUID}:v2"),
            bud_auth::KeyEvent::Set,
        )
        .await
        .unwrap();
    assert!(!session_still_allowed(&state, &meter).await);

    // Back on the original legs, then voice is switched off.
    store.set(
        &format!("voice_agent:{PROMPT_UUID}:v2"),
        &projection(2, "ep-stt", json!({})),
    );
    plane
        .on_key_event(
            &format!("voice_agent:{PROMPT_UUID}:v2"),
            bud_auth::KeyEvent::Set,
        )
        .await
        .unwrap();
    assert!(session_still_allowed(&state, &meter).await);
    store.remove(&format!("voice_agent:{PROMPT_UUID}:v2"));
    plane
        .on_key_event(
            &format!("voice_agent:{PROMPT_UUID}:v2"),
            bud_auth::KeyEvent::Del,
        )
        .await
        .unwrap();
    assert!(!session_still_allowed(&state, &meter).await);
}

/// S-8 🔒 — the key loses the agent: the session ends.
#[tokio::test]
async fn revalidation_ends_when_the_caller_loses_the_agent() {
    let (state, store) = plane(json!({})).await;
    let p = prepare(
        &state,
        KEY,
        "prompt:support",
        &mut stt_cfg(json!({})),
        &mut tts_cfg(json!({})),
    )
    .await
    .unwrap();
    let hashed = bud_auth::hash_api_key(KEY);
    store.set(
        &format!("api_key:{hashed}"),
        &json!({"some-model": {"endpoint_id": "ep-llm"}, "__metadata__": {"api_key_id": "ak1"}})
            .to_string(),
    );
    state
        .bud_mode
        .as_ref()
        .unwrap()
        .plane()
        .on_key_event(&format!("api_key:{hashed}"), bud_auth::KeyEvent::Set)
        .await
        .unwrap();
    assert!(!session_still_allowed(&state, &p.legs.meter).await);
}

#[test]
fn agent_models_parse() {
    use crate::state::parse_agent_model;
    assert_eq!(
        parse_agent_model("prompt:support"),
        Some(("support".into(), None))
    );
    assert_eq!(
        parse_agent_model("prompt:support:v3"),
        Some(("support".into(), Some(3)))
    );
    assert_eq!(
        parse_agent_model("prompt:my:agent"),
        Some(("my:agent".into(), None))
    );
    assert_eq!(
        parse_agent_model("prompt:x:v0"),
        Some(("x:v0".into(), None))
    );
    assert_eq!(parse_agent_model("support"), None);
    assert_eq!(parse_agent_model("prompt:"), None);
}

#[test]
fn the_ws_agent_id_becomes_a_prompt_model() {
    let cfg = |id: &str, v: Option<i64>| super::super::config::AgentWebSocketConfig {
        id: id.into(),
        version: v,
        ..Default::default()
    };
    assert_eq!(cfg("support", None).model(), "prompt:support");
    assert_eq!(cfg("prompt:support", None).model(), "prompt:support");
    assert_eq!(cfg("support", Some(4)).model(), "prompt:support:v4");
    assert_eq!(cfg("support:v2", Some(4)).model(), "prompt:support:v4");
}

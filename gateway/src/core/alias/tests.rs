//! Unit + integration tests for the alias registry (P3).

use super::*;
use crate::handlers::ws::config::{STTWebSocketConfig, TTSWebSocketConfig};

/// Parse one alias definition from a YAML fragment (the same path `config.yaml`'s
/// `aliases:` section flows through).
fn parse_def(yaml: &str) -> AliasDefinition {
    serde_yaml::from_str(yaml).expect("alias definition should parse")
}

/// The canonical `support-bot` alias used across the killer-demo tests.
fn support_bot_yaml() -> &'static str {
    r#"
kind: agent
stt:  { provider: deepgram,   model: nova-3,        language: en-US }
tts:  { provider: elevenlabs, voice: female-warm-en, emotion: empathetic }
llm:  { base_url: "http://127.0.0.1:11434/v1", model: qwen2.5, reasoning_effort: minimal }
"#
}

#[test]
fn registry_register_resolve_case_insensitive() {
    let reg = AliasRegistry::new();
    reg.register("Support-Bot", parse_def(support_bot_yaml()));

    assert!(reg.contains("support-bot"));
    assert!(reg.contains("SUPPORT-BOT"));
    assert_eq!(reg.len(), 1);

    let def = reg.resolve("support-bot").expect("resolves");
    assert_eq!(def.kind, Some(AliasKind::Agent));
    assert_eq!(def.stt.as_ref().unwrap().provider.as_deref(), Some("deepgram"));
    assert_eq!(
        def.tts.as_ref().unwrap().voice.as_deref(),
        Some("female-warm-en")
    );
    // Original casing preserved for display.
    assert_eq!(reg.names(), vec!["Support-Bot".to_string()]);
}

#[test]
fn resolve_unknown_returns_none() {
    let reg = AliasRegistry::new();
    assert!(reg.resolve("nope").is_none());
    let (code, msg) = unknown_alias_message("nope");
    assert_eq!(code, "alias_unknown");
    assert!(msg.contains("nope"));
}

/// The core deliverable: a `{alias}`-only config materializes a FULL concrete bundle.
#[test]
fn splice_materializes_full_bundle_from_alias_only() {
    let def = parse_def(support_bot_yaml());
    let (mut stt, mut tts, mut conv, mut dag) = (None, None, None, None);

    let echo = splice_alias("support-bot", &def, &mut stt, &mut tts, &mut conv, &mut dag);

    // STT materialized with provider + model + language + sane audio defaults.
    let stt = stt.expect("stt materialized");
    assert_eq!(stt.provider, "deepgram");
    assert_eq!(stt.model, "nova-3");
    assert_eq!(stt.language, "en-US");
    assert_eq!(stt.sample_rate, 16_000);
    assert_eq!(stt.channels, 1);

    // TTS materialized with provider + voice + emotion.
    let tts = tts.expect("tts materialized");
    assert_eq!(tts.provider, "elevenlabs");
    assert_eq!(tts.voice_id.as_deref(), Some("female-warm-en"));
    assert_eq!(tts.emotion, Some(crate::core::emotion::Emotion::Empathetic));

    // LLM/conversation materialized with the ollama base_url + model + effort.
    let conv = conv.expect("conversation materialized");
    assert_eq!(conv.base_url, "http://127.0.0.1:11434/v1");
    assert_eq!(conv.model, "qwen2.5");
    assert_eq!(
        conv.reasoning_effort,
        Some(crate::core::llm::ReasoningEffort::Minimal)
    );

    // Echo reflects the concrete resolved providers (the ready-ack payload).
    assert_eq!(echo.name, "support-bot");
    assert_eq!(echo.kind, Some(AliasKind::Agent));
    assert_eq!(echo.stt.as_ref().unwrap().provider.as_deref(), Some("deepgram"));
    assert_eq!(echo.tts.as_ref().unwrap().provider.as_deref(), Some("elevenlabs"));
    assert_eq!(echo.llm.as_ref().unwrap().model.as_deref(), Some("qwen2.5"));
    // The system prompt is never echoed back (may be sensitive).
    assert!(echo.llm.as_ref().unwrap().system_prompt.is_none());
}

/// Rule #3: an explicit client field ALWAYS overrides the alias default.
#[test]
fn client_field_overrides_alias_default() {
    let def = parse_def(support_bot_yaml());

    // Client pre-supplies a STT config that pins a DIFFERENT provider + model, and a
    // TTS config pinning a different voice. The alias must NOT clobber them.
    let mut stt = Some(STTWebSocketConfig {
        provider: "assemblyai".to_string(),
        language: String::new(), // left empty → alias fills en-US
        sample_rate: 16_000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "best".to_string(),
        api_key: None,
        features: Default::default(),
        extras: Default::default(),
        turn_detection: None,
    });
    let mut tts = Some(TTSWebSocketConfig {
        provider: String::new(), // empty → alias fills elevenlabs
        voice_id: Some("client-voice".to_string()),
        speaking_rate: None,
        audio_format: None,
        sample_rate: Some(24_000),
        client_playback_rate: None,
        audio_out_chunk_ms: None,
        connection_timeout: None,
        request_timeout: None,
        model: String::new(),
        pronunciations: Vec::new(),
        api_key: None,
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        voice_descriptor: None,
        features: Default::default(),
        extras: Default::default(),
    });
    let (mut conv, mut dag) = (None, None);

    splice_alias("support-bot", &def, &mut stt, &mut tts, &mut conv, &mut dag);

    let stt = stt.unwrap();
    assert_eq!(stt.provider, "assemblyai", "client provider wins");
    assert_eq!(stt.model, "best", "client model wins");
    assert_eq!(stt.language, "en-US", "empty client field filled by alias");

    let tts = tts.unwrap();
    assert_eq!(tts.provider, "elevenlabs", "empty client provider filled by alias");
    assert_eq!(
        tts.voice_id.as_deref(),
        Some("client-voice"),
        "client voice wins"
    );
    // Alias-supplied emotion fills the empty client slot.
    assert_eq!(tts.emotion, Some(crate::core::emotion::Emotion::Empathetic));
}

/// A `dag`-kind alias resolves to a named template (the same indirection layer as
/// `dag_config.template`).
#[test]
fn splice_dag_template_alias() {
    let def = parse_def(
        r#"
kind: dag
dag_template: voice-assistant
"#,
    );
    let (mut stt, mut tts, mut conv, mut dag) = (None, None, None, None);
    let echo = splice_alias("translate-bot", &def, &mut stt, &mut tts, &mut conv, &mut dag);

    let dag = dag.expect("dag config materialized");
    assert_eq!(dag.template.as_deref(), Some("voice-assistant"));
    assert_eq!(echo.dag_template.as_deref(), Some("voice-assistant"));
    // No stt/tts/llm intent → none materialized.
    assert!(stt.is_none());
    assert!(tts.is_none());
    assert!(conv.is_none());
}

/// An inline client DAG definition is an explicit override — the alias template must
/// NOT replace it.
#[test]
fn client_inline_dag_overrides_alias_template() {
    let def = parse_def("kind: dag\ndag_template: voice-assistant\n");
    let mut dag = Some(DAGWebSocketConfig {
        template: None,
        definition: Some(serde_json::json!({"id": "inline"})),
        enable_metrics: false,
        timeout_ms: None,
    });
    let (mut stt, mut tts, mut conv) = (None, None, None);
    splice_alias("x", &def, &mut stt, &mut tts, &mut conv, &mut dag);
    let dag = dag.unwrap();
    assert!(dag.template.is_none(), "inline definition is not overridden by alias template");
    assert!(dag.definition.is_some());
}

#[test]
fn initialize_aliases_populates_global() {
    let mut map = std::collections::HashMap::new();
    map.insert("news-voice".to_string(), parse_def("kind: tts\ntts: { provider: deepgram, voice: aura-asteria-en }\n"));
    let n = initialize_aliases(&AliasConfig { aliases: map });
    assert!(n >= 1);
    // The global registry now resolves it (may also contain aliases from other tests
    // sharing the process-wide singleton, so assert presence not exact count).
    assert!(global_aliases().contains("news-voice"));
    let def = global_aliases().resolve("news-voice").unwrap();
    assert_eq!(def.kind, Some(AliasKind::Tts));
}

/// `deny_unknown_fields` makes a typo in an alias definition fail loudly at config
/// load, instead of silently misconfiguring a session.
#[test]
fn unknown_field_in_alias_definition_is_rejected() {
    let res: Result<AliasDefinition, _> = serde_yaml::from_str("kind: agent\nstt: { provder: deepgram }\n");
    assert!(res.is_err(), "typo'd sub-field must be rejected");

    let res: Result<AliasDefinition, _> = serde_yaml::from_str("kind: agent\nbogus_block: 1\n");
    assert!(res.is_err(), "unknown top-level alias field must be rejected");
}

// ── SSRF / trust boundary ────────────────────────────────────────────────────────────
//
// The key safety property (plan rule #3 / SECURITY): a client may NAME an alias but can
// NEVER DEFINE one. We prove this structurally against the real wire type: the
// `/ws` config message exposes a single `alias: Option<String>` and has NO field that
// could carry an alias DEFINITION (no `aliases`, no per-message stt/tts/llm bundle map,
// no base_url on the alias name). Definitions live only in server config.

/// A client `config` message can carry an alias NAME (a plain string) — nothing more.
#[test]
fn client_config_accepts_alias_name_only() {
    let wire = r#"{"type":"config","alias":"support-bot","audio":false}"#;
    let msg: crate::handlers::ws::messages::IncomingMessage =
        serde_json::from_str(wire).expect("config with alias name parses");
    match msg {
        crate::handlers::ws::messages::IncomingMessage::Config { alias, .. } => {
            assert_eq!(alias.as_deref(), Some("support-bot"));
        }
        _ => panic!("expected Config"),
    }
}

/// A client attempting to INJECT an alias DEFINITION (an object with backend
/// url/provider) where the name goes is rejected by the typed wire — `alias` is a
/// `String`, so an object fails to deserialize. This is the SSRF guard: no
/// client-controlled backend endpoint can enter through an alias.
#[test]
fn client_cannot_inject_alias_definition_object() {
    // `alias` as an object (carrying a would-be backend url) — must fail.
    let malicious = r#"{"type":"config","alias":{"llm":{"base_url":"http://169.254.169.254/"}}}"#;
    let res: Result<crate::handlers::ws::messages::IncomingMessage, _> =
        serde_json::from_str(malicious);
    assert!(
        res.is_err(),
        "an alias DEFINITION object must not deserialize into the client `alias` field"
    );

    // A top-level `aliases` registry on the client message also does not exist — the
    // bogus key is simply not a field (and is caught by config-lint elsewhere).
    let wire = r#"{"type":"config","aliases":{"evil":{"llm":{"base_url":"http://evil/"}}}}"#;
    let msg: crate::handlers::ws::messages::IncomingMessage =
        serde_json::from_str(wire).expect("parses (unknown key ignored at this layer)");
    // Crucially: NO alias was set, and there is no way to reach a definition from here.
    match msg {
        crate::handlers::ws::messages::IncomingMessage::Config { alias, .. } => {
            assert!(alias.is_none(), "client `aliases` map is not a real field — no definition enters");
        }
        _ => panic!("expected Config"),
    }
}

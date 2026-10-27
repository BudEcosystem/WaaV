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
    assert_eq!(
        def.stt.as_ref().unwrap().provider.as_deref(),
        Some("deepgram")
    );
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
    assert_eq!(
        echo.stt.as_ref().unwrap().provider.as_deref(),
        Some("deepgram")
    );
    assert_eq!(
        echo.tts.as_ref().unwrap().provider.as_deref(),
        Some("elevenlabs")
    );
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
        transcription_mode: None,
        translation: None,
        audio_in_codec: None,
        noise_suppression: None,
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
        audio_out_codec: None,
    });
    let (mut conv, mut dag) = (None, None);

    splice_alias("support-bot", &def, &mut stt, &mut tts, &mut conv, &mut dag);

    let stt = stt.unwrap();
    assert_eq!(stt.provider, "assemblyai", "client provider wins");
    assert_eq!(stt.model, "best", "client model wins");
    assert_eq!(stt.language, "en-US", "empty client field filled by alias");

    let tts = tts.unwrap();
    assert_eq!(
        tts.provider, "elevenlabs",
        "empty client provider filled by alias"
    );
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
    let echo = splice_alias(
        "translate-bot",
        &def,
        &mut stt,
        &mut tts,
        &mut conv,
        &mut dag,
    );

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
    assert!(
        dag.template.is_none(),
        "inline definition is not overridden by alias template"
    );
    assert!(dag.definition.is_some());
}

#[test]
fn default_alias_conversation_uses_wire_defaults_without_static_expect() {
    let cfg = default_alias_conversation();

    assert_eq!(cfg.base_url, "");
    assert_eq!(cfg.model, "");
    assert!(cfg.streaming);
    assert_eq!(cfg.max_history, 20);
    assert!(cfg.allow_interruption);
    assert!(cfg.strip_markdown);
}

#[test]
fn invalid_alias_conversation_wire_defaults_fall_back_without_panic() {
    let cfg = alias_conversation_from_wire_defaults(serde_json::json!({
        "base_url": false,
        "model": false
    }));

    assert_eq!(cfg.base_url, "");
    assert_eq!(cfg.model, "");
    assert!(cfg.streaming);
    assert_eq!(cfg.max_history, 20);
    assert!(cfg.allow_interruption);
    assert!(cfg.strip_markdown);
}

#[test]
fn initialize_aliases_populates_global() {
    let mut map = std::collections::HashMap::new();
    map.insert(
        "news-voice".to_string(),
        parse_def("kind: tts\ntts: { provider: deepgram, voice: aura-asteria-en }\n"),
    );
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
    let res: Result<AliasDefinition, _> =
        serde_yaml::from_str("kind: agent\nstt: { provder: deepgram }\n");
    assert!(res.is_err(), "typo'd sub-field must be rejected");

    let res: Result<AliasDefinition, _> = serde_yaml::from_str("kind: agent\nbogus_block: 1\n");
    assert!(
        res.is_err(),
        "unknown top-level alias field must be rejected"
    );
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
            assert!(
                alias.is_none(),
                "client `aliases` map is not a real field — no definition enters"
            );
        }
        _ => panic!("expected Config"),
    }
}

// ── A leg's own server (Gaia's 25 Oct report, items 2, 3 and 4) ─────────────────────────────

fn config_of(aliases: &[(&str, &str)]) -> AliasConfig {
    AliasConfig {
        aliases: aliases
            .iter()
            .map(|(n, y)| (n.to_string(), parse_def(y)))
            .collect(),
    }
}

#[test]
fn a_leg_names_its_own_server_and_credential_server_side_only() {
    let def = parse_def(
        r#"
openai_compatible: true
stt: { provider: openai, model: local-transcribe, base_url: "https://stt.example.com", credential_ref: gw }
tts: { provider: openai, model: local-speak, voice: alloy, base_url: "https://tts.example.com/v1", request_timeout_secs: 150 }
"#,
    );
    assert!(def.openai_compatible);
    let stt = def.stt_endpoint().unwrap();
    assert_eq!(stt.base_url, "https://stt.example.com");
    assert_eq!(stt.credential_ref.as_deref(), Some("gw"));
    assert_eq!(stt.openai_api_base(), "https://stt.example.com/v1");
    let tts = def.tts_endpoint().unwrap();
    assert_eq!(
        tts.openai_api_base(),
        "https://tts.example.com/v1",
        "never /v1/v1"
    );
    assert!(tts.applies_to("OpenAI") && !tts.applies_to("deepgram"));

    // The splice fills the timeout and never writes the server into the client configs, and the
    // echo the client receives carries neither.
    let mut stt_cfg = None;
    let mut tts_cfg = None;
    let echo = splice_alias(
        "gaia",
        &def,
        &mut stt_cfg,
        &mut tts_cfg,
        &mut None,
        &mut None,
    );
    let tts_cfg = tts_cfg.unwrap();
    assert_eq!(tts_cfg.request_timeout, Some(150));
    assert!(tts_cfg.extras.0.is_empty());
    let json = serde_json::to_string(&echo).unwrap();
    assert!(
        !json.contains("example.com") && !json.contains("credential_ref"),
        "{json}"
    );
}

#[test]
fn startup_refuses_a_leg_server_it_cannot_honour() {
    let no_provider = config_of(&[("a", r#"tts: { base_url: "https://tts.example.com" }"#)]);
    assert!(
        validate_aliases(&no_provider)
            .unwrap_err()
            .contains("needs tts.provider")
    );

    let private = config_of(&[(
        "b",
        r#"stt: { provider: openai, base_url: "http://10.0.0.5:8000" }"#,
    )]);
    let e = validate_aliases(&private).unwrap_err();
    assert!(e.contains("WAAV_PRIVATE_EGRESS_ALLOWLIST"), "{e}");

    let not_openai = config_of(&[(
        "c",
        r#"
openai_compatible: true
tts: { provider: deepgram, base_url: "https://proxy.example.com" }
"#,
    )]);
    assert!(
        validate_aliases(&not_openai)
            .unwrap_err()
            .contains("must be openai")
    );

    let nothing_to_serve = config_of(&[("d", "openai_compatible: true\n")]);
    assert!(validate_aliases(&nothing_to_serve).is_err());

    let fine = config_of(&[
        (
            "e",
            r#"
openai_compatible: true
tts: { provider: openai, base_url: "https://tts.example.com" }
"#,
        ),
        ("f", support_bot_yaml()),
    ]);
    assert_eq!(validate_aliases(&fine), Ok(()));
}

fn endpoint(base: &str, credential_ref: Option<&str>) -> AliasLegEndpoint {
    AliasLegEndpoint {
        provider: "openai".into(),
        base_url: base.into(),
        credential_ref: credential_ref.map(str::to_string),
    }
}

#[test]
fn an_alias_leg_key_is_the_clients_then_the_aliases_then_an_approved_operator_key() {
    // The client's own key wins.
    let k = alias_leg_key(&endpoint("https://x.example", None), Some("sk-own"), || {
        Some("sk-op".into())
    })
    .unwrap();
    assert_eq!(k.key, "sk-own");

    // The operator's OPENAI_API_KEY may go to api.openai.com (its implicit binding).
    let k = alias_leg_key(&endpoint("https://api.openai.com", None), None, || {
        Some("sk-op".into())
    })
    .unwrap();
    assert_eq!((k.key.as_str(), k.withheld), ("sk-op", false));

    // …but not to a server nobody approved: the leg runs keyless, and the client is told.
    let k = alias_leg_key(&endpoint("https://x.example", None), None, || {
        Some("sk-op".into())
    })
    .unwrap();
    assert_eq!((k.key.as_str(), k.withheld), ("", true));

    // A keyless server with no operator key: nothing withheld.
    let k = alias_leg_key(&endpoint("https://x.example", None), None, || None).unwrap();
    assert_eq!(k, AliasLegKey::default());

    // A credential the operator never declared is an error, not a silent fallback.
    let e = alias_leg_key(&endpoint("https://x.example", Some("nobody")), None, || {
        None
    })
    .unwrap_err();
    assert!(e.contains("nobody"), "{e}");
}

// ── A vendor's own realtime API (the `realtime` block, standalone `/v1/realtime`) ────────────

#[test]
fn a_realtime_block_parses_and_refuses_unknown_keys() {
    let def = parse_def(
        r#"
openai_compatible: true
realtime:
  provider: openai
  model: gpt-realtime
  base_url: "https://api.openai.com/v1"
  credential_ref: OPENAI_API_KEY
  voice: marin
  instructions: Be brief.
"#,
    );
    let leg = def.realtime.as_ref().unwrap();
    assert_eq!(leg.provider.as_deref(), Some("openai"));
    assert_eq!(leg.model.as_deref(), Some("gpt-realtime"));
    assert_eq!(leg.base_url.as_deref(), Some("https://api.openai.com/v1"));
    assert_eq!(leg.credential_ref.as_deref(), Some("OPENAI_API_KEY"));
    assert_eq!(leg.voice.as_deref(), Some("marin"));
    assert_eq!(leg.instructions.as_deref(), Some("Be brief."));

    let typo: Result<AliasDefinition, _> =
        serde_yaml::from_str("realtime: { provider: openai, model_name: gpt-realtime }\n");
    assert!(
        typo.is_err(),
        "an unknown realtime field (model_name for model) must be rejected"
    );
}

#[test]
fn startup_accepts_a_realtime_block_it_can_relay() {
    // The vendor's own host with the gateway's key, alone or beside legs that serve /v1/audio/*.
    let fine = config_of(&[
        (
            "rt-only",
            "openai_compatible: true\nrealtime: { provider: openai, model: gpt-realtime }\n",
        ),
        (
            "rt-and-legs",
            r#"
openai_compatible: true
realtime: { provider: grok, model: grok-voice }
stt: { provider: openai, model: whisper-1 }
tts: { provider: openai, model: tts-1, voice: alloy }
"#,
        ),
        (
            // A vendor the GA facade translates.
            "rt-translated",
            "openai_compatible: true\nrealtime: { provider: gemini, model: gemini-2.5-flash-native-audio }\n",
        ),
        (
            // A credential bound to the base's host (OPENAI_API_KEY's implicit binding).
            "rt-bound",
            r#"
openai_compatible: true
realtime: { provider: openai, model: gpt-realtime, base_url: "https://api.openai.com/v1", credential_ref: OPENAI_API_KEY }
"#,
        ),
    ]);
    assert_eq!(validate_aliases(&fine), Ok(()));
}

#[test]
fn startup_refuses_a_realtime_block_it_cannot_relay() {
    let refused = |yaml: &str| validate_aliases(&config_of(&[("rt", yaml)])).unwrap_err();

    let e = refused("openai_compatible: true\nrealtime: { provider: acme, model: m }\n");
    assert!(
        e.contains("not a realtime vendor") && e.contains("openai"),
        "{e}"
    );
    let e = refused("openai_compatible: true\nrealtime: { provider: nova_sonic, model: m }\n");
    assert!(e.contains("AWS key pair"), "{e}");
    let e = refused("openai_compatible: true\nrealtime: { model: m }\n");
    assert!(e.contains("realtime.provider is required"), "{e}");
    let e = refused("openai_compatible: true\nrealtime: { provider: openai }\n");
    assert!(e.contains("realtime.model is required"), "{e}");

    // Reachable only through /v1/realtime, which needs the mark.
    let e = refused("realtime: { provider: openai, model: m }\n");
    assert!(e.contains("openai_compatible"), "{e}");

    // A base_url the egress rules refuse: private, or not a URL the relay can dial.
    let e = refused(
        "openai_compatible: true\nrealtime: { provider: openai, model: m, base_url: \"http://10.0.0.5:8000/v1\" }\n",
    );
    assert!(e.contains("WAAV_PRIVATE_EGRESS_ALLOWLIST"), "{e}");
    let e = refused(
        "openai_compatible: true\nrealtime: { provider: openai, model: m, base_url: \"ftp://api.openai.com/v1\" }\n",
    );
    assert!(e.contains("realtime.base_url"), "{e}");

    // A credential is bound to its hosts: one bound elsewhere, or one nobody declared, stops the
    // server rather than failing every session.
    let e = refused(
        r#"
openai_compatible: true
realtime: { provider: openai, model: m, base_url: "https://proxy.example.com/v1", credential_ref: OPENAI_API_KEY }
"#,
    );
    assert!(
        e.contains("may not be sent to 'proxy.example.com:443'"),
        "{e}"
    );
    let e = refused(
        r#"
openai_compatible: true
realtime: { provider: openai, model: m, base_url: "https://api.openai.com/v1", credential_ref: nobody }
"#,
    );
    assert!(e.contains("'nobody' is not defined"), "{e}");
    let e = refused(
        "openai_compatible: true\nrealtime: { provider: openai, model: m, credential_ref: OPENAI_API_KEY }\n",
    );
    assert!(e.contains("needs realtime.base_url"), "{e}");

    // A base_url without a credential_ref gets the gateway's key only where the operator
    // approved the host for it.
    let e = refused(
        "openai_compatible: true\nrealtime: { provider: openai, model: m, base_url: \"https://proxy.example.com/v1\" }\n",
    );
    assert!(
        e.contains("not approved for the gateway's openai key"),
        "{e}"
    );

    // The vendor request is built at startup, as each session builds it: Azure needs its
    // resource endpoint.
    let e = refused("openai_compatible: true\nrealtime: { provider: azure_openai, model: rt }\n");
    assert!(e.contains("api_base"), "{e}");
}

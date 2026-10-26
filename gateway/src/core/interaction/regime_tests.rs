//! The 48 resolver cases of `regime_resolve.py --self-test`, ported as they are written: 31
//! named cases and one per provider entry (`expected_block_matches_rule_for_<id>`). The six
//! cases of that file not ported check its own schema validator, its JSON files and examples,
//! its generated coverage table, its wire-example checker and its block merge; none of them
//! asks the resolver.

use super::*;

const REFERENCE_DATE: &str = "2026-10-05";
const STEP_DOWN: &str = "steps down to the plain conversation path";
const REPEAT_REQUEST: &str = "repeat-request template, continues in English";
const AS_CONTROLLING: &str = "as the /ws session that drives it";

/// The Python's `S(**kw)`: a base session, `server` merged key by key.
fn s(kw: Value) -> Value {
    let mut base = json!({
        "release": 5, "today": REFERENCE_DATE, "interaction": {}, "has_background": true,
        "server": {"enabled": true, "build_features": true, "truth_ack": [],
                   "front_model": {"licence": "lfm_open_eligible"}}});
    for (k, v) in kw.as_object().expect("an object") {
        if k == "server" {
            for (sk, sv) in v.as_object().expect("an object") {
                base["server"][sk] = sv.clone();
            }
        } else {
            base[k] = v.clone();
        }
    }
    resolve_session(&base)
}

fn ack(names: &[&str]) -> Value {
    json!({ "truth_ack": names })
}

fn has(r: &Value, warning: &str) -> bool {
    r["warnings"]
        .as_array()
        .is_some_and(|w| w.iter().any(|w| w == warning))
}

fn lookup_caps(provider: &str) -> &'static Value {
    lookup(provider, None).expect("a provider entry").0
}

#[test]
fn regime_cascade_for_ws_conversation_agent_and_dag() {
    for (sid, rel, bud) in [
        ("ws_conversation", 1, false),
        ("ws_agent", 1, true),
        ("ws_raw", 1, false),
        ("v1_realtime_cascade", 2, true),
        ("ws_dag_text", 3, false),
    ] {
        let r = s(json!({"surface": sid, "release": rel, "bud_mode": bud}));
        assert_eq!(
            (&r["interaction"], &r["regime"], &r["front"], &r["truth"]),
            (
                &json!("on"),
                &json!("Cascade"),
                &json!("local"),
                &json!("enforced")
            ),
            "{sid}"
        );
        for owner in ["filler_owner", "backchannel_owner", "idle_owner"] {
            assert_eq!(r[owner], "gateway", "{sid} {owner}");
        }
    }
}

#[test]
fn release_gating_refuses_surfaces_before_their_release() {
    let unavailable = "interaction_surface_unavailable";
    assert_eq!(
        s(json!({"surface": "ws_dag_text", "release": 2}))["refusal"],
        unavailable
    );
    assert_eq!(
        s(json!({"surface": "v1_realtime_cascade", "release": 1, "bud_mode": true}))["refusal"],
        unavailable
    );
    assert_eq!(
        s(json!({"surface": "realtime", "provider": "grok", "release": 2}))["refusal"],
        unavailable
    );
    assert_eq!(
        s(json!({"surface": "ws_realtime_voice", "provider": "grok", "release": 3}))["refusal"],
        unavailable
    );
    assert_eq!(
        s(json!({"surface": "ws_conversation", "release": 0}))["refusal"],
        unavailable
    );
}

#[test]
fn livekit_and_sip_in_release_1_only_if_echo_gates_pass() {
    assert_eq!(
        s(json!({"surface": "livekit", "release": 1}))["refusal"],
        "interaction_surface_unavailable"
    );
    assert_eq!(
        s(json!({"surface": "livekit", "release": 2}))["regime"],
        "Cascade"
    );
    assert_eq!(
        s(json!({"surface": "sip", "release": 1, "echo_gates_pass": true}))["regime"],
        "Cascade"
    );
}

#[test]
fn audio_output_dags_refused_with_interaction_dag_audio_output() {
    let r = s(json!({"surface": "ws_dag_audio", "release": 3}));
    assert_eq!(
        (&r["interaction"], &r["refusal"], &r["fatal"]),
        (
            &json!("off"),
            &json!("interaction_dag_audio_output"),
            &json!(false)
        )
    );
    assert_eq!(
        s(json!({"surface": "dag_realtime_node", "provider": "gemini"}))["refusal"],
        "interaction_dag_audio_output"
    );
}

#[test]
fn regime_delegating_for_gpt_live_gemini_38_gemini_31_and_nova2() {
    for (p, m) in [
        ("gpt_live", Value::Null),
        ("gemini", json!("gemini-3.8-live")),
        ("gemini", json!("gemini-3.8-live-extended-thinking")),
        ("gemini", json!("gemini-3.1-flash-live-preview")),
        ("nova_sonic", Value::Null),
    ] {
        let r = s(json!({"surface": "realtime", "provider": p, "model": m, "server": ack(&[p])}));
        assert_eq!(
            (&r["regime"], &r["front"], &r["truth"]),
            (
                &json!("Delegating"),
                &json!("provider"),
                &json!("monitored")
            ),
            "{p} {m}"
        );
    }
}

#[test]
fn regime_provider_speech_for_grok_inworld_deepgram_ultravox_elevenlabs_openai() {
    for p in [
        "grok",
        "inworld",
        "deepgram",
        "ultravox",
        "elevenlabs",
        "openai",
        "azure",
        "yandex",
    ] {
        let r = s(json!({"surface": "realtime", "provider": p, "server": ack(&[p])}));
        assert_eq!(
            (&r["regime"], &r["front"]),
            (&json!("ProviderSpeech"), &json!("narrator_templates")),
            "{p}"
        );
    }
}

#[test]
fn regime_openai_family_half_cascade_when_voice_owner_gateway() {
    let gateway_voice = json!({"speech": {"voice_owner": "gateway"}});
    let r = s(
        json!({"surface": "realtime", "provider": "openai", "model": "gpt-realtime-2.1",
                     "interaction": gateway_voice}),
    );
    assert_eq!(
        (&r["regime"], &r["variant"], &r["truth"], &r["voice_owner"]),
        (
            &json!("Cascade"),
            &json!("half_cascade"),
            &json!("enforced"),
            &json!("gateway")
        )
    );
    let r = s(
        json!({"surface": "realtime", "provider": "grok", "interaction": gateway_voice,
                     "server": ack(&["grok"])}),
    );
    assert_eq!(r["regime"], "ProviderSpeech");
    assert!(has(&r, "voice_owner_unavailable"));
    let r = s(
        json!({"surface": "realtime", "provider": "ultravox", "interaction": gateway_voice,
                     "server": ack(&["ultravox"])}),
    );
    assert_eq!(
        r["regime"], "ProviderSpeech",
        "ultravox has text output but no reply switch"
    );
}

#[test]
fn truth_enforced_selects_half_cascade_or_refuses() {
    let enforced = json!({"speech": {"truth": "enforced"}});
    let r = s(
        json!({"surface": "v1_realtime_relay", "provider": "azure", "bud_mode": true,
                     "interaction": enforced}),
    );
    assert_eq!(
        (&r["regime"], &r["variant"]),
        (&json!("Cascade"), &json!("half_cascade"))
    );
    let r = s(
        json!({"surface": "realtime", "provider": "gemini", "interaction": enforced,
                     "server": ack(&["gemini"])}),
    );
    assert_eq!(
        (&r["refusal"], &r["fatal"]),
        (&json!("truth_enforcement_unavailable"), &json!(true))
    );
}

#[test]
fn regime_refuses_hume_after_2026_11_13_and_warns_before() {
    let r = s(
        json!({"surface": "realtime", "provider": "hume", "today": "2026-11-12",
                     "server": ack(&["hume"])}),
    );
    assert_eq!(r["refusal"], "interaction_unsupported");
    assert!(has(&r, "provider_sunset"));
    let r = s(
        json!({"surface": "realtime", "provider": "hume", "today": "2026-11-13",
                     "interaction": null}),
    );
    assert_eq!(
        (&r["refusal"], &r["fatal"]),
        (&json!("provider_retired"), &json!(true))
    );
}

#[test]
fn regime_refuses_infer_native_duplex_without_injection_channel() {
    let r = s(
        json!({"surface": "realtime", "provider": "infer", "release": 5,
                     "server": ack(&["infer"])}),
    );
    assert_eq!(
        (&r["refusal"], &r["fatal"]),
        (&json!("interaction_unsupported"), &json!(false))
    );
    let r = s(
        json!({"surface": "realtime", "provider": "infer", "release": 5, "server": ack(&["infer"]),
                     "infer_injection_channel": true, "native_duplex_criteria": true}),
    );
    assert_eq!(
        (&r["regime"], &r["front"], &r["truth"]),
        (
            &json!("NativeDuplex"),
            &json!("provider"),
            &json!("monitored")
        )
    );
    let r = s(
        json!({"surface": "realtime", "provider": "infer", "release": 5, "server": ack(&[]),
                     "infer_injection_channel": true, "native_duplex_criteria": true}),
    );
    assert_eq!(r["refusal"], "truth_ack_required");
    assert_eq!(
        s(json!({"surface": "realtime", "provider": "infer", "release": 4}))["refusal"],
        "unknown_provider"
    );
}

#[test]
fn forced_regime_can_only_narrow_and_warns_regime_downgraded() {
    let r = s(
        json!({"surface": "realtime", "provider": "grok", "interaction": {"regime": "cascade"},
                     "server": ack(&["grok"])}),
    );
    assert_eq!(r["regime"], "ProviderSpeech");
    assert!(has(&r, "regime_downgraded"));
    let r = s(json!({"surface": "realtime", "provider": "openai",
                     "interaction": {"regime": "delegating"}, "server": ack(&["openai"])}));
    assert_eq!(
        (&r["regime"], &r["filler_owner"], &r["idle_owner"]),
        (&json!("Delegating"), &json!("provider"), &json!("provider"))
    );
    let r = s(json!({"surface": "realtime", "provider": "elevenlabs",
                     "interaction": {"regime": "delegating"}, "server": ack(&["elevenlabs"])}));
    assert_eq!(r["regime"], "ProviderSpeech");
    assert!(has(&r, "regime_downgraded"));
    let r = s(json!({"surface": "realtime", "provider": "gemini",
                     "interaction": {"regime": "provider_speech"}, "server": ack(&["gemini"])}));
    assert_eq!(r["regime"], "Delegating");
    assert!(has(&r, "regime_downgraded"));
    let r = s(
        json!({"surface": "realtime", "provider": "openai", "interaction": {"regime": "cascade"}}),
    );
    assert_eq!(
        (&r["regime"], &r["variant"]),
        (&json!("Cascade"), &json!("half_cascade"))
    );
}

#[test]
fn gpt_live_refused_on_v1_realtime_and_on_the_dag_realtime_node() {
    let r = s(json!({"surface": "v1_realtime_relay", "provider": "gpt_live", "bud_mode": true}));
    assert_eq!(
        (&r["refusal"], &r["fatal"]),
        (&json!("unsupported_vendor"), &json!(true))
    );
    let r = s(
        json!({"surface": "v1_realtime_facade", "provider": "gpt_live", "bud_mode": true,
                     "interaction": null}),
    );
    assert_eq!(
        (&r["refusal"], &r["fatal"]),
        (&json!("unsupported_vendor"), &json!(true))
    );
    let r = s(json!({"surface": "dag_realtime_node", "provider": "gpt_live"}));
    assert_eq!(
        (&r["refusal"], &r["fatal"]),
        (&json!("provider_not_turn_based"), &json!(true))
    );
    let r = s(
        json!({"surface": "ws_realtime_voice", "provider": "gpt_live", "bud_mode": true,
                     "release": 4, "server": ack(&["gpt_live"])}),
    );
    assert_eq!(r["regime"], "Delegating");
    assert_eq!(
        s(json!({"surface": "realtime", "provider": "gpt_live", "release": 2}))["refusal"],
        "unknown_provider"
    );
}

#[test]
fn truth_ack_required_unless_the_operator_lists_the_provider() {
    let r = s(json!({"surface": "realtime", "provider": "openai", "model": "gpt-realtime-2.1"}));
    assert_eq!(
        (&r["refusal"], &r["fatal"], &r["detail"]["alternative"]),
        (
            &json!("truth_ack_required"),
            &json!(false),
            &json!("speech.truth: enforced")
        )
    );
    let r = s(json!({"surface": "realtime", "provider": "grok"}));
    assert_eq!(
        (
            &r["refusal"],
            &r["detail"]["alternative"],
            &r["detail"]["path"]
        ),
        (
            &json!("truth_ack_required"),
            &Value::Null,
            &json!("interaction.speech.truth_ack")
        )
    );
    let r = s(
        json!({"surface": "realtime", "provider": "openai", "model": "gpt-realtime-2.1",
                     "server": ack(&["openai:gpt-realtime-2.1"])}),
    );
    assert_eq!(r["regime"], "ProviderSpeech");
    let r = s(
        json!({"surface": "realtime", "provider": "openai", "model": "gpt-realtime-2",
                     "server": ack(&["openai:gpt-realtime-2.1"])}),
    );
    assert_eq!(r["refusal"], "truth_ack_required");
    assert_eq!(
        s(json!({"surface": "ws_conversation", "release": 1}))["regime"],
        "Cascade"
    );
}

#[test]
fn filler_owner_auto_is_provider_on_gpt_realtime_2x_preambles() {
    let r = s(
        json!({"surface": "realtime", "provider": "openai", "model": "gpt-realtime-2.1",
                     "server": ack(&["openai"])}),
    );
    assert_eq!(r["filler_owner"], "provider");
    let r = s(
        json!({"surface": "realtime", "provider": "openai", "model": "gpt-realtime-1.5",
                     "server": ack(&["openai"])}),
    );
    assert_eq!(r["filler_owner"], "gateway");
}

#[test]
fn azure_without_model_family_resolves_fillers_to_gateway_with_model_family_unknown() {
    let azure = |extra: Value| {
        let mut kw = json!({"surface": "realtime", "provider": "azure", "model": "contoso-rt-prod",
                            "server": ack(&["azure"])});
        for (k, v) in extra.as_object().unwrap() {
            kw[k] = v.clone();
        }
        s(kw)
    };
    let r = azure(json!({}));
    assert_eq!(r["filler_owner"], "gateway");
    assert!(has(&r, "model_family_unknown"));
    let r = azure(json!({"model": "gpt-realtime-2.1"}));
    assert_eq!(r["filler_owner"], "gateway");
    assert!(
        has(&r, "model_family_unknown"),
        "a deployment name is still not read"
    );
    let r = azure(json!({"model_family": "gpt-realtime-2.1"}));
    assert_eq!(r["filler_owner"], "provider");
    assert!(!has(&r, "model_family_unknown"));
    let r = azure(json!({"model_family": "gpt-realtime"}));
    assert_eq!(
        (&r["filler_owner"], &r["warnings"]),
        (&json!("gateway"), &json!([]))
    );
    let r = azure(json!({"interaction": {"filler_owner": "gateway"}}));
    assert!(
        !has(&r, "model_family_unknown"),
        "an explicit owner needs no family"
    );
    let r = s(
        json!({"surface": "realtime", "provider": "azure", "model": "contoso-rt-prod",
                     "interaction": {"speech": {"voice_owner": "gateway"}}}),
    );
    assert_eq!(r["regime"], "Cascade");
    assert!(
        !has(&r, "model_family_unknown"),
        "half-cascade needs no family"
    );
}

#[test]
fn owner_values_the_provider_cannot_honour_are_overridden_with_a_warning() {
    let r = s(json!({"surface": "realtime", "provider": "gemini",
                     "interaction": {"filler_owner": "gateway"}, "server": ack(&["gemini"])}));
    assert_eq!(r["filler_owner"], "provider");
    assert!(has(&r, "filler_owner_overridden"));
    let r = s(json!({"surface": "realtime", "provider": "openai",
                     "interaction": {"backchannel_owner": "gateway"}, "server": ack(&["openai"])}));
    assert_eq!(r["backchannel_owner"], "none");
    assert!(has(&r, "backchannel_owner_overridden"));
    let r = s(json!({"surface": "realtime", "provider": "inworld",
                     "interaction": {"backchannel_owner": "provider"}, "server": ack(&["inworld"])}));
    assert_eq!(
        (&r["backchannel_owner"], &r["warnings"]),
        (&json!("provider"), &json!([]))
    );
    let r = s(json!({"surface": "realtime", "provider": "deepgram",
                     "interaction": {"filler_owner": "provider"}, "server": ack(&["deepgram"])}));
    assert_eq!(r["filler_owner"], "none");
    assert!(has(&r, "filler_owner_overridden"));
    let r = s(json!({"surface": "ws_conversation", "release": 1,
                     "interaction": {"filler_owner": "provider"}}));
    assert_eq!(r["filler_owner"], "gateway");
    assert!(has(&r, "filler_owner_overridden"));
}

#[test]
fn elevenlabs_gateway_fillers_only_when_the_operator_switched_the_agent_fillers_off() {
    let r = s(json!({"surface": "realtime", "provider": "elevenlabs",
                     "interaction": {"filler_owner": "gateway"}, "server": ack(&["elevenlabs"])}));
    assert_eq!(r["filler_owner"], "provider");
    assert!(has(&r, "filler_owner_overridden"));
    let r = s(json!({"surface": "realtime", "provider": "elevenlabs",
                     "interaction": {"filler_owner": "gateway"}, "server": ack(&["elevenlabs"]),
                     "operator_flags": ["elevenlabs_fillers_off"]}));
    assert_eq!(
        (&r["filler_owner"], &r["warnings"]),
        (&json!("gateway"), &json!([]))
    );
}

#[test]
fn front_model_narrator_is_masked_to_wait_progress_refuse() {
    let r = s(json!({"surface": "realtime", "provider": "grok",
                     "interaction": {"speech": {"narrator": "front_model"}}, "server": ack(&["grok"])}));
    assert_eq!(r["front"], "narrator_front_model");
    let masked = string_list(&r["masked_acts"]);
    let mut left: Vec<&str> = ACTS
        .into_iter()
        .filter(|a| !masked.iter().any(|m| m == a))
        .collect();
    left.sort_unstable();
    let mut narrowed = NARROWED_ACTS.to_vec();
    narrowed.sort_unstable();
    assert_eq!(left, narrowed);
}

#[test]
fn server_switch_build_features_and_absent_block_turn_interaction_off() {
    let r = s(json!({"surface": "ws_conversation", "release": 1, "server": {"enabled": false}}));
    assert_eq!(r["refusal"], "interaction_disabled_by_server");
    let r =
        s(json!({"surface": "ws_conversation", "release": 1, "server": {"build_features": false}}));
    assert_eq!(r["refusal"], "interaction_missing_build_features");
    let r = s(json!({"surface": "ws_conversation", "release": 1, "interaction": null}));
    assert_eq!(
        (&r["interaction"], &r["refusal"], &r["warnings"]),
        (&json!("off"), &json!("interaction_absent"), &json!([]))
    );
}

#[test]
fn raw_ws_and_provider_sessions_need_a_background() {
    let r = s(json!({"surface": "ws_raw", "release": 1, "has_background": false}));
    assert_eq!(r["refusal"], "interaction_needs_background");
    assert_eq!(
        s(json!({"surface": "ws_raw", "release": 1}))["regime"],
        "Cascade"
    );
    let r = s(
        json!({"surface": "realtime", "provider": "grok", "has_background": false,
                     "server": ack(&["grok"])}),
    );
    assert_eq!(r["refusal"], "interaction_needs_background");
}

#[test]
fn bud_mode_surfaces_and_vendors_reached() {
    let r = s(json!({"surface": "realtime", "provider": "grok", "bud_mode": true}));
    assert_eq!(r["refusal"], "deployment_required");
    let r = s(json!({"surface": "v1_realtime_relay", "provider": "openai", "bud_mode": false}));
    assert_eq!(r["refusal"], "bud_mode_required");
    let r = s(json!({"surface": "v1_realtime_relay", "provider": "inworld", "bud_mode": true}));
    assert_eq!(
        (&r["refusal"], &r["fatal"]),
        (&json!("unsupported_vendor"), &json!(true))
    );
    let r = s(
        json!({"surface": "ws_realtime_voice", "provider": "inworld", "bud_mode": true,
                     "release": 4, "server": ack(&["inworld"])}),
    );
    assert_eq!(r["regime"], "ProviderSpeech");
}

#[test]
fn non_english_steps_down_until_the_hindi_packs_in_release_4() {
    let unsupported = "interaction_locale_unsupported";
    let r = s(json!({"surface": "ws_conversation", "release": 3, "language": "hi-IN"}));
    assert_eq!(r["refusal"], unsupported);
    let r = s(json!({"surface": "ws_conversation", "release": 4, "language": "hi-IN"}));
    assert_eq!(r["regime"], "Cascade");
    let r = s(json!({"surface": "ws_conversation", "release": 4, "language": "fr"}));
    assert_eq!(r["refusal"], unsupported);
}

#[test]
fn lenient_parse_refuses_server_only_keys_and_ignores_bad_values() {
    let r = s(json!({"surface": "ws_conversation", "release": 1,
                     "interaction": {"front": {"licence": "liquid_commercial"}},
                     "server": {"front_model": {}}}));
    assert!(has(&r, "interaction_override_refused"));
    assert_eq!(r["front_profile"], "scripted");
    let r = s(json!({"surface": "ws_conversation", "release": 1,
                     "interaction": {"regime": "native", "speech": {"truth_ack": ["x"]}}}));
    assert!(has(&r, "interaction_value_ignored"));
    assert!(has(&r, "interaction_override_refused"));
    assert_eq!(r["regime"], "Cascade");
}

#[test]
fn front_profile_auto_follows_the_qualified_list_and_the_licence_attestation() {
    let conversation = |release: u64, extra: Value| {
        let mut kw = json!({"surface": "ws_conversation", "release": release});
        for (k, v) in extra.as_object().unwrap() {
            kw[k] = v.clone();
        }
        s(kw)
    };
    let r = conversation(1, json!({}));
    assert_eq!(r["front_profile"], "scripted");
    assert!(has(&r, "front_model_scripted"));
    let r = conversation(1, json!({"server": {"front_model": {}}}));
    assert_eq!(r["front_profile"], "scripted");
    let r = conversation(
        2,
        json!({"server": {"qualified_profiles": ["lfm-standard"]}}),
    );
    assert_eq!(
        (&r["front_profile"], &r["front_qualified"]),
        (&json!("lfm-standard"), &json!(true))
    );
    let r = conversation(
        2,
        json!({"server": {"qualified_profiles": ["lfm-large", "lfm-standard"]}}),
    );
    assert_eq!(r["front_profile"], "lfm-standard");
    let r = conversation(2, json!({"server": {"qualified_profiles": ["lfm-large"]}}));
    assert_eq!(r["front_profile"], "lfm-large");
    let r = conversation(
        2,
        json!({"server": {"front_model": {},
                                               "qualified_profiles": ["lfm-standard", "apache-standard"]}}),
    );
    assert_eq!(r["front_profile"], "apache-standard");
    let r = conversation(
        1,
        json!({"server": {"front_model": {"licence": "lfm_open_eligible",
                                   "profile": "lfm-standard", "allow_unqualified": true}}}),
    );
    assert_eq!(
        (&r["front_profile"], &r["front_qualified"]),
        (&json!("lfm-standard"), &json!(false))
    );
    let r = conversation(
        1,
        json!({"interaction": {"front": {"profile": "lfm-standard"}},
                                   "server": {"front_model": {"profiles_enabled": ["auto", "lfm-standard"]}}}),
    );
    assert_eq!(r["front_profile"], "scripted");
    assert!(has(&r, "front_profile_substituted"), "unlicensed request");
    let r = conversation(
        1,
        json!({"interaction": {"front": {"profile": "lfm-standard"}},
                                   "server": {"front_model": {"licence": "liquid_commercial",
                                              "profiles_enabled": ["auto", "lfm-standard"]}}}),
    );
    assert_eq!(r["front_profile"], "scripted");
    assert!(
        has(&r, "front_profile_substituted"),
        "licensed, not qualified"
    );
}

#[test]
fn half_cascade_only_on_the_openai_family_with_waav_voice_and_gateway_none_gateway_owners() {
    let r = s(
        json!({"surface": "realtime", "provider": "openai", "model": "gpt-realtime-2.1",
                     "interaction": {"speech": {"voice_owner": "gateway"}, "filler_owner": "provider",
                                     "backchannel_owner": "gateway"}}),
    );
    assert_eq!(
        (&r["regime"], &r["variant"], &r["voice_owner"], &r["front"]),
        (
            &json!("Cascade"),
            &json!("half_cascade"),
            &json!("gateway"),
            &json!("local")
        )
    );
    assert_eq!(
        (
            &r["filler_owner"],
            &r["backchannel_owner"],
            &r["idle_owner"]
        ),
        (&json!("gateway"), &json!("none"), &json!("gateway"))
    );
    assert!(has(&r, "filler_owner_overridden"));
    assert!(has(&r, "backchannel_owner_overridden"));
    for p in [
        "grok",
        "inworld",
        "deepgram",
        "elevenlabs",
        "ultravox",
        "gemini",
        "nova_sonic",
    ] {
        assert_eq!(
            lookup_caps(p)["ownership"]["half_cascade_possible"],
            false,
            "{p}"
        );
    }
}

#[test]
fn language_step_down_only_on_ws_conversation_and_agent_sessions() {
    let key = "mid_call_unsupported_language";
    assert_eq!(
        s(json!({"surface": "ws_conversation", "release": 1}))[key],
        STEP_DOWN
    );
    assert_eq!(
        s(json!({"surface": "ws_agent", "release": 1, "bud_mode": true}))[key],
        STEP_DOWN
    );
    for kw in [
        json!({"surface": "ws_raw"}),
        json!({"surface": "v1_realtime_cascade", "bud_mode": true, "release": 2}),
        json!({"surface": "realtime", "provider": "gemini", "server": ack(&["gemini"])}),
        json!({"surface": "ws_realtime_voice", "provider": "grok", "release": 4, "server": ack(&["grok"])}),
    ] {
        assert_eq!(s(kw.clone())[key], REPEAT_REQUEST, "{kw}");
    }
    assert_eq!(
        s(json!({"surface": "sip", "release": 2}))[key],
        AS_CONTROLLING
    );
}

#[test]
fn server_task_catalog_and_duplex_compute_are_refused_from_sessions() {
    for block in [
        json!({"duplex": {"compute": {"pool_threads": 4}}}),
        json!({"tasks": {"task_backends": []}}),
        json!({"tasks": {"allow_client_push": true}}),
    ] {
        let r = s(json!({"surface": "ws_conversation", "release": 1, "interaction": block}));
        assert!(has(&r, "interaction_override_refused"), "{block}");
        assert_eq!(r["regime"], "Cascade");
    }
    let (cfg, warnings) = parse_block(&json!({"memory": {"enabled": true}}), true);
    assert!(cfg.unwrap().get("memory").is_some());
    assert!(
        warnings.is_empty(),
        "memory is not a server-only interaction key"
    );
}

#[test]
fn bud_reaches_inworld_ultravox_yandex_only_from_release_4_sign_off_27() {
    for p in ["inworld", "ultravox", "yandex"] {
        for sid in ["v1_realtime_relay", "v1_realtime_facade"] {
            let r = s(json!({"surface": sid, "provider": p, "bud_mode": true}));
            assert_eq!(r["refusal"], "unsupported_vendor", "{p} {sid}");
        }
        assert_eq!(interaction_release(lookup_caps(p))["bud"], 4, "{p}");
    }
    let exceptions: Vec<&Value> = data()["coverage_exceptions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["exception"]
                .as_str()
                .unwrap_or("")
                .contains("Inworld, Ultravox or Yandex")
        })
        .collect();
    assert_eq!(exceptions.len(), 1);
    assert_eq!(exceptions[0]["sign_off"], "27");
}

#[test]
fn lifecycle_table_evaluated_against_an_injected_clock() {
    let (caps, model) = lookup("openai", Some("")).unwrap();
    assert_eq!(model, "gpt-realtime");
    assert_eq!(lifecycle_at(caps, &model, "2026-10-21")["status"], "active");
    assert_eq!(
        lifecycle_at(caps, &model, "2026-10-22")["code"],
        "model_retiring"
    );
    assert_eq!(
        lifecycle_at(caps, &model, "2027-01-20")["code"],
        "model_retired"
    );
    let r = s(
        json!({"surface": "realtime", "provider": "openai", "today": "2027-01-20",
                     "interaction": null}),
    );
    assert_eq!(
        (&r["refusal"], &r["fatal"]),
        (&json!("model_retired"), &json!(true))
    );
    let r = s(
        json!({"surface": "realtime", "provider": "openai", "model": "gpt-realtime-2.1",
                     "today": "2027-01-20", "server": ack(&["openai"])}),
    );
    assert_eq!(r["regime"], "ProviderSpeech");
    let r = s(
        json!({"surface": "realtime", "provider": "nova_sonic", "model": "amazon.nova-sonic-v1:0",
                     "interaction": null}),
    );
    assert_eq!(r["refusal"], "model_retired");
}

#[test]
fn confirmation_path_follows_w6_3_5_1() {
    let r = s(json!({"surface": "realtime", "provider": "grok", "server": ack(&["grok"])}));
    assert_eq!(r["confirm_irreversible"], "native_exact");
    let r = s(json!({"surface": "realtime", "provider": "gemini", "server": ack(&["gemini"])}));
    assert_eq!(r["confirm_irreversible"], "client_ui");
    let r = s(
        json!({"surface": "ws_realtime_voice", "provider": "gemini", "release": 4,
                     "server": ack(&["gemini"]), "client_ui": false}),
    );
    assert_eq!(r["confirm_irreversible"], "gateway_clip_different_voice");
    let r = s(
        json!({"surface": "ws_realtime_voice", "provider": "gemini", "release": 4,
                     "server": ack(&["gemini"]), "client_ui": false,
                     "interaction": {"speech": {"voice_mismatch": "never"}}}),
    );
    assert_eq!(
        r["confirm_irreversible"],
        "unanswerable_no_verbatim_channel"
    );
    let r =
        s(json!({"surface": "realtime", "provider": "elevenlabs", "server": ack(&["elevenlabs"])}));
    assert_eq!(r["confirm_irreversible"], "gateway_clip_same_voice");
    let r = s(
        json!({"surface": "ws_realtime_voice", "provider": "openai", "release": 4,
                     "server": ack(&["openai"]), "client_ui": false}),
    );
    assert_eq!(
        (&r["confirm_irreversible"], &r["confirm_reversible"]),
        (
            &json!("gateway_clip_different_voice"),
            &json!("say_guided_exact_words_checked")
        )
    );
}

#[test]
fn acknowledgement_timing_per_regime_follows_w6_3_18() {
    let ack_of = |kw: Value| s(kw)["ack"].as_str().unwrap_or("").to_owned();
    assert!(
        ack_of(json!({"surface": "realtime", "provider": "grok", "server": ack(&["grok"])}))
            .starts_with("later of E + 300")
    );
    assert!(
        ack_of(
            json!({"surface": "realtime", "provider": "openai", "model": "gpt-realtime-2.1",
                          "server": ack(&["openai"])})
        )
        .starts_with("none from the gateway: the provider's own")
    );
    assert!(
        ack_of(json!({"surface": "realtime", "provider": "gemini", "server": ack(&["gemini"])}))
            .starts_with("none from the gateway: the provider is the front")
    );
    assert!(
        ack_of(json!({"surface": "ws_conversation", "release": 1})).starts_with("E + ack_due_ms")
    );
}

/// The Python's `compute_expected`: an entry's `expected` block, at the reference date and
/// Release 5.
fn compute_expected(caps: &Value) -> Value {
    let mut model = if caps["kind"] != "cascade" {
        caps["default_model"].as_str().unwrap_or("").to_owned()
    } else {
        String::new()
    };
    match caps["id"].as_str() {
        Some("openai.gpt-realtime-2") => model = "gpt-realtime-2.1".into(),
        Some("gemini.3.8-live-extended-thinking") => {
            model = "gemini-3.8-live-extended-thinking".into()
        }
        Some("gemini.legacy") => model = "gemini-3.1-flash-live-preview".into(),
        _ => {}
    }
    let surf = surface(if caps["kind"] == "cascade" {
        "ws_conversation"
    } else {
        "ws_realtime_voice"
    })
    .unwrap();
    let ack_all = json!({"truth_ack": [caps["provider_id"]]});
    let run = |cfg: Value, server: &Value| {
        select_regime(
            surf,
            caps,
            &model,
            &cfg,
            server,
            REFERENCE_DATE,
            5,
            &json!({}),
        )
    };
    let summarise = |r: Result<Map<String, Value>, Refusal>, full: bool| match r {
        Err(r) => {
            let mut out = json!({"refusal": r.code});
            if r.code == "truth_ack_required" {
                out["alternative"] = r.detail["alternative"].clone();
            }
            if let Some(first) = r.warnings.first() {
                out["warning"] = first.clone().into();
            }
            out
        }
        Ok(d) if full => json!({
            "regime": d["regime"],
            "variant": if d["regime"] == "Cascade" { d["variant"].clone() } else { Value::Null },
            "front": d["front"], "filler_owner": d["filler_owner"],
            "backchannel_owner": d["backchannel_owner"], "idle_owner": d["idle_owner"],
            "truth": d["truth"]}),
        Ok(d) => {
            let mut out = json!({"regime": d["regime"]});
            if d["regime"] == "Cascade" {
                out["variant"] = d["variant"].clone();
            }
            if let Some(first) = d["warnings"].as_array().and_then(|w| w.first()) {
                out["warning"] = first.clone();
            }
            out
        }
    };
    let mut forced = Map::new();
    for name in ["cascade", "provider_speech", "delegating"] {
        let entry = match run(json!({"regime": name}), &ack_all) {
            Err(r) => json!({"refusal": r.code}),
            Ok(d) => json!({"regime": d["regime"],
                            "downgraded": string_list(&d["warnings"]).iter().any(|w| w == "regime_downgraded")}),
        };
        forced.insert(name.into(), entry);
    }
    json!({
        "auto_with_truth_ack": summarise(run(json!({}), &ack_all), true),
        "auto_without_truth_ack": summarise(run(json!({}), &json!({"truth_ack": []})), true),
        "voice_owner_gateway": summarise(run(json!({"speech": {"voice_owner": "gateway"}}), &ack_all), false),
        "truth_enforced": summarise(run(json!({"speech": {"truth": "enforced"}}), &ack_all), false),
        "forced": forced,
        "interaction_release": interaction_release(caps),
    })
}

fn expected_block_matches_rule_for(id: &str) {
    let caps = data()["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("no entry {id}"));
    let got = compute_expected(caps);
    for (k, v) in caps["expected"].as_object().unwrap() {
        assert_eq!(&got[k], v, "{id}.expected.{k}");
    }
}

macro_rules! expected_cases {
    ($($name:ident => $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                expected_block_matches_rule_for($id);
            }
        )*

        #[test]
        fn every_provider_entry_has_its_expected_case() {
            let mut ids: Vec<&str> = data()["providers"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|p| p["id"].as_str())
                .collect();
            ids.sort_unstable();
            let mut ported = vec![$($id),*];
            ported.sort_unstable();
            assert_eq!(ids, ported);
        }
    };
}

expected_cases! {
    expected_block_matches_rule_for_cascade => "cascade",
    expected_block_matches_rule_for_openai_gpt_realtime_2 => "openai.gpt-realtime-2",
    expected_block_matches_rule_for_openai_gpt_realtime_legacy => "openai.gpt-realtime-legacy",
    expected_block_matches_rule_for_azure => "azure",
    expected_block_matches_rule_for_yandex => "yandex",
    expected_block_matches_rule_for_grok => "grok",
    expected_block_matches_rule_for_inworld => "inworld",
    expected_block_matches_rule_for_deepgram => "deepgram",
    expected_block_matches_rule_for_elevenlabs => "elevenlabs",
    expected_block_matches_rule_for_ultravox => "ultravox",
    expected_block_matches_rule_for_gemini_3_8_live_extended_thinking => "gemini.3.8-live-extended-thinking",
    expected_block_matches_rule_for_gemini_3_8_live => "gemini.3.8-live",
    expected_block_matches_rule_for_gemini_legacy => "gemini.legacy",
    expected_block_matches_rule_for_nova_sonic => "nova_sonic",
    expected_block_matches_rule_for_gpt_live => "gpt_live",
    expected_block_matches_rule_for_hume => "hume",
    expected_block_matches_rule_for_infer => "infer",
}

// ---- this build's own rows, beside the plan's cases

#[test]
fn the_packaged_capability_data_is_the_plans_file() {
    assert_eq!(
        include_str!("assets/provider_capabilities.json"),
        include_str!("../../../../docs/duplex-agent/providers/provider_capabilities.json")
    );
}

#[test]
fn the_build_verbs_are_those_of_the_three_protocols_with_verb_wires() {
    assert_eq!(verbs("openai"), [false, true, true, true]);
    assert_eq!(verbs("deepgram"), [true, true, false, false]);
    assert_eq!(verbs("nova_sonic"), [false, true, false, false]);
    // r11-fix: xAI's `force_message`, `response.create` with `instructions`, OpenAI's others.
    assert_eq!(verbs("grok"), [true; 4]);
    assert_eq!(verbs("hume"), [false; 4]);
    assert_eq!(provider_id("Azure-OpenAI"), "azure");
    assert_eq!(provider_id("nova-sonic"), "nova_sonic");
}

#[test]
fn today_is_an_iso_date() {
    let t = today();
    assert_eq!(t.len(), 10);
    assert!(t.as_str() > "2026-01-01");
}

/// Provider speculation (chapter 6): Gemini's and Deepgram's delegate calls may be provisional
/// for their capability data's `adopt_by_ms`; an OpenAI call never is.
#[test]
fn provisional_calls_wait_for_adoption_as_long_as_the_capability_data_says() {
    assert_eq!(adopt_by("gemini", None), Some(10_000));
    assert_eq!(adopt_by("deepgram", None), Some(3_000));
    assert_eq!(adopt_by("openai", None), None);
    assert_eq!(adopt_by("unknown", None), None);
}

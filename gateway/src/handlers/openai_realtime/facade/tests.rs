//! The translator as a pure machine: GA client events in, provider calls and GA server events
//! out (FRD-023 §5.7, TC-XL-01…07). The end-to-end halves, against in-process mocks of each
//! vendor's wire protocol, are in `tests/realtime_translate.rs`.

use super::*;
use bud_auth::RealtimePolicy;

fn rules(policy: RealtimePolicy) -> ClientRules {
    ClientRules {
        policy,
        session_type: "realtime".into(),
        max_output_tokens: None,
    }
}

fn translator(vendor: &str) -> Translator {
    Translator::new(
        vendor_info(vendor).unwrap(),
        "live".into(),
        rules(RealtimePolicy::default()),
        RealtimeConfig {
            provider: vendor.into(),
            api_key: "k".into(),
            voice: Some("Puck".into()),
            ..Default::default()
        },
        "sess_bud_test",
    )
}

/// The GA events among `acts`, parsed.
fn client_events(acts: &[Act]) -> Vec<Value> {
    acts.iter()
        .filter_map(|a| match a {
            Act::Client(t) => serde_json::from_str(t).ok(),
            _ => None,
        })
        .collect()
}

fn kinds(acts: &[Act]) -> Vec<String> {
    client_events(acts)
        .iter()
        .map(|e| e["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn refusal(acts: &[Act]) -> (String, String) {
    let events = client_events(acts);
    assert_eq!(events.len(), 1, "one error, got {events:?}");
    assert_eq!(events[0]["type"], "error");
    (
        events[0]["error"]["code"].as_str().unwrap().to_string(),
        events[0]["error"]["param"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

fn ready(tr: &mut Translator) {
    let _ = tr.client(&json!({"type": "session.update", "session": {}}).to_string());
    let _ = tr.connected(Some((16_000, 24_000)));
    let _ = tr.vendor(S2sEvent::SessionReady { session_id: None });
}

fn meters(acts: &[Act]) -> Vec<(RealtimeUsage, &'static str)> {
    acts.iter()
        .filter_map(|a| match a {
            Act::Meter { usage, status, .. } => Some((*usage, *status)),
            _ => None,
        })
        .collect()
}

/// TC-XL-01 🔒 (translator half) — the setup waits for the client's first `session.update`
/// and carries it; `session.updated` follows the vendor's acceptance; afterwards a CHANGE to
/// voice or tools is refused by name, the same value again is accepted.
#[test]
fn tc_xl_01_one_setup_then_setup_time_fields_are_locked() {
    let mut tr = translator("gemini");
    let tool = json!({"type": "function", "name": "lookup", "parameters": {"type": "object"}});
    let acts = tr.client(
        &json!({"type": "session.update", "event_id": "c1", "session": {
            "type": "realtime", "instructions": "be brief",
            "audio": {"output": {"voice": "Kore"}}, "tools": [tool]}})
        .to_string(),
    );
    assert_eq!(
        acts,
        vec![Act::Connect],
        "one deferred setup, nothing else yet"
    );
    let cfg = tr.config();
    assert_eq!(cfg.voice.as_deref(), Some("Kore"), "the client's voice");
    assert_eq!(cfg.instructions.as_deref(), Some("be brief"));
    assert_eq!(
        cfg.tools
            .as_ref()
            .map(|t| t[0].function.name.clone())
            .as_deref(),
        Some("lookup")
    );

    assert!(
        tr.connected(Some((16_000, 24_000))).is_empty(),
        "Gemini: wait for setupComplete"
    );
    let acts = tr.vendor(S2sEvent::SessionReady { session_id: None });
    assert_eq!(kinds(&acts), vec!["session.updated"]);

    let acts = tr.client(
        &json!({"type": "session.update", "session": {"audio": {"output": {"voice": "Puck"}}}})
            .to_string(),
    );
    assert_eq!(
        refusal(&acts),
        (
            "event_not_allowed".into(),
            "session.audio.output.voice".into()
        )
    );
    let other = json!({"type": "function", "name": "other"});
    let acts =
        tr.client(&json!({"type": "session.update", "session": {"tools": [other]}}).to_string());
    assert_eq!(
        refusal(&acts),
        ("event_not_allowed".into(), "session.tools".into())
    );
    let acts = tr.client(
        &json!({"type": "session.update", "session": {"instructions": "be verbose"}}).to_string(),
    );
    assert_eq!(refusal(&acts).1, "session.instructions");

    // SDKs resend the whole session: unchanged values are not a change.
    let acts = tr.client(
        &json!({"type": "session.update", "session": {
            "instructions": "be brief", "audio": {"output": {"voice": "Kore"}}, "tools": [tool]}})
        .to_string(),
    );
    assert_eq!(kinds(&acts), vec!["session.updated"]);
    assert!(!acts.contains(&Act::Connect), "never a second setup");
}

/// Audio before the vendor is ready is held, then flushed after `session.updated`.
#[test]
fn vendor_bound_work_waits_for_the_vendor() {
    let mut tr = translator("gemini");
    let pcm = BASE64_STANDARD.encode([0u8; 960]);
    let acts = tr.client(&json!({"type": "input_audio_buffer.append", "audio": pcm}).to_string());
    assert_eq!(
        acts,
        vec![Act::Connect],
        "audio first: set up with the defaults, hold it"
    );
    assert!(tr.connected(Some((16_000, 24_000))).is_empty());
    let acts = tr.vendor(S2sEvent::SessionReady { session_id: None });
    assert!(
        matches!(acts.as_slice(), [Act::Audio(b)] if b.len() == 960),
        "{acts:?}"
    );
}

/// FRD §5.7 / CONTRACTS C7 — what the vendor cannot do is refused by name, whatever the
/// deployment's policy allows, and the session continues.
#[test]
fn untranslatable_events_are_refused_by_name() {
    let mut tr = Translator::new(
        vendor_info("gemini").unwrap(),
        "live".into(),
        rules(RealtimePolicy {
            allow_mcp_tools: Some(true),
            allow_prompt_references: Some(true),
            ..Default::default()
        }),
        RealtimeConfig::default(),
        "s",
    );
    ready(&mut tr);
    let cases = [
        (
            json!({"type": "conversation.item.truncate", "item_id": "i", "content_index": 0, "audio_end_ms": 10}),
            "conversation.item.truncate",
        ),
        (
            json!({"type": "conversation.item.retrieve", "item_id": "i"}),
            "conversation.item.retrieve",
        ),
        (
            json!({"type": "conversation.item.delete", "item_id": "i"}),
            "conversation.item.delete",
        ),
        (
            json!({"type": "output_audio_buffer.clear"}),
            "output_audio_buffer.clear",
        ),
        (json!({"type": "future.event"}), "future.event"),
        (
            json!({"type": "session.update", "session": {"tools": [{"type": "mcp", "server_url": "https://x"}]}}),
            "session.tools.mcp",
        ),
        (
            json!({"type": "session.update", "session": {"prompt": {"id": "pmpt_1"}}}),
            "session.prompt",
        ),
        (
            json!({"type": "session.update", "session": {"audio": {"input": {"format": {"type": "audio/pcmu"}}}}}),
            "session.audio.input.format",
        ),
        (
            json!({"type": "conversation.item.create", "item": {"type": "message", "role": "user",
            "content": [{"type": "input_image", "image_url": "data:image/png;base64,AA=="}]}}),
            "item.content.input_image",
        ),
        (
            json!({"type": "conversation.item.create", "item": {"type": "message", "role": "user",
            "content": [{"type": "input_audio", "audio": "AA=="}]}}),
            "item.content.input_audio",
        ),
        (
            json!({"type": "response.create", "response": {"conversation": "none"}}),
            "response.conversation",
        ),
        (
            json!({"type": "response.create", "response": {"prompt": {"id": "pmpt_1"}}}),
            "response.prompt",
        ),
    ];
    for (event, param) in cases {
        let acts = tr.client(&event.to_string());
        assert_eq!(
            refusal(&acts),
            ("event_not_allowed".into(), param.into()),
            "{event}"
        );
    }
}

/// TC-XL-03 🔒 (translator half) — a cumulative report replaces the running total of its
/// response, so the response is billed ONCE, with the last total, and `response.done.usage`
/// says exactly that; a report outside any response is billed on its own.
#[test]
fn tc_xl_03_usage_lands_on_its_response_and_is_metered_once() {
    let mut tr = translator("gemini");
    ready(&mut tr);
    let report = |input_audio: u64, output_audio: u64, cumulative: bool| {
        S2sEvent::Usage(UsageReport {
            tokens: RealtimeUsage {
                input_audio,
                output_audio,
                ..Default::default()
            },
            seconds: None,
            cumulative,
        })
    };
    let mut all = tr.vendor(S2sEvent::Audio {
        data: Bytes::from(vec![0u8; 480]),
        item_id: None,
        response_id: None,
    });
    all.extend(tr.vendor(report(10, 20, true)));
    all.extend(tr.vendor(report(12, 30, true)));
    all.extend(tr.vendor(S2sEvent::ResponseDone {
        response_id: "gemini-turn".into(),
    }));
    let k = kinds(&all);
    assert_eq!(k.first().map(String::as_str), Some("response.created"));
    assert!(k.contains(&"response.output_audio.delta".to_string()));
    let done = client_events(&all)
        .into_iter()
        .find(|e| e["type"] == "response.done")
        .unwrap();
    assert_eq!(done["response"]["status"], "completed");
    assert_eq!(
        done["response"]["usage"]["input_token_details"]["audio_tokens"],
        12
    );
    assert_eq!(
        done["response"]["usage"]["output_token_details"]["audio_tokens"],
        30
    );
    let billed = meters(&all);
    assert_eq!(billed.len(), 1, "one record per response");
    assert_eq!(
        (billed[0].0.input_audio, billed[0].0.output_audio),
        (12, 30)
    );

    // Nova-style deltas add within a response.
    let mut all = tr.vendor(S2sEvent::Audio {
        data: Bytes::from(vec![0u8; 480]),
        item_id: None,
        response_id: None,
    });
    all.extend(tr.vendor(report(5, 5, false)));
    all.extend(tr.vendor(report(5, 7, false)));
    all.extend(tr.vendor(S2sEvent::ResponseDone {
        response_id: String::new(),
    }));
    assert_eq!(
        meters(&all)
            .iter()
            .map(|m| (m.0.input_audio, m.0.output_audio))
            .collect::<Vec<_>>(),
        vec![(10, 12)]
    );

    // Between responses: billed alone, once.
    let acts = tr.vendor(report(3, 0, false));
    assert_eq!(meters(&acts).len(), 1);
    assert!(client_events(&acts).is_empty());
}

/// The session ends mid-response: what the vendor already reported is still billed.
#[test]
fn a_response_cut_off_by_close_keeps_its_usage() {
    let mut tr = translator("nova_sonic");
    ready(&mut tr);
    let _ = tr.vendor(S2sEvent::Audio {
        data: Bytes::from(vec![0u8; 480]),
        item_id: None,
        response_id: None,
    });
    let _ = tr.vendor(S2sEvent::Usage(UsageReport {
        tokens: RealtimeUsage {
            input_audio: 7,
            ..Default::default()
        },
        seconds: None,
        cumulative: false,
    }));
    let billed = meters(&tr.close());
    assert_eq!(billed.len(), 1);
    assert_eq!(
        billed[0],
        (
            RealtimeUsage {
                input_audio: 7,
                ..Default::default()
            },
            "incomplete"
        )
    );
}

fn transcript_deltas(acts: &[Act]) -> Vec<String> {
    client_events(acts)
        .iter()
        .filter(|e| e["type"] == "response.output_audio_transcript.delta")
        .map(|e| e["delta"].as_str().unwrap().to_string())
        .collect()
}

fn asst(text: &str, is_final: bool) -> S2sEvent {
    S2sEvent::Transcript {
        role: TranscriptRole::Assistant,
        text: text.into(),
        is_final,
        item_id: None,
    }
}

/// Gemini's last transcript chunk is marked final but is a delta; Nova's FINAL block restates
/// its SPECULATIVE one. Both reach the client as each word once.
#[test]
fn assistant_transcripts_are_sent_once_each() {
    let mut tr = translator("gemini");
    ready(&mut tr);
    let mut acts = tr.vendor(asst("Hel", false));
    acts.extend(tr.vendor(asst("lo", false)));
    acts.extend(tr.vendor(asst("!", true)));
    assert_eq!(transcript_deltas(&acts), vec!["Hel", "lo", "!"]);

    let mut tr = translator("nova_sonic");
    ready(&mut tr);
    let mut acts = tr.vendor(asst("Hi there.", false));
    acts.extend(tr.vendor(asst("Hi there.", true)));
    acts.extend(tr.vendor(asst("Bye.", false)));
    acts.extend(tr.vendor(asst("Bye.", true)));
    acts.extend(tr.vendor(S2sEvent::ResponseDone {
        response_id: String::new(),
    }));
    assert_eq!(transcript_deltas(&acts), vec!["Hi there.", "Bye."]);
    let done = client_events(&acts)
        .into_iter()
        .find(|e| e["type"] == "response.output_audio_transcript.done")
        .unwrap();
    assert_eq!(done["transcript"], "Hi there.Bye.");
}

/// A function call ends its response (the client runs the tool, then asks for the next).
#[test]
fn a_function_call_ends_its_response() {
    let mut tr = translator("gemini");
    ready(&mut tr);
    let acts = tr.vendor(S2sEvent::FunctionCall(
        crate::core::realtime::FunctionCallRequest {
            call_id: "call_1".into(),
            name: "lookup".into(),
            arguments: "{\"q\":1}".into(),
            item_id: None,
        },
    ));
    let k = kinds(&acts);
    assert_eq!(
        k,
        vec![
            "response.created",
            "response.output_item.added",
            "conversation.item.added",
            "response.function_call_arguments.done",
            "response.output_item.done",
            "conversation.item.done",
            "response.done"
        ]
    );
    let args = &client_events(&acts)[3];
    assert_eq!(args["call_id"], "call_1");
    assert_eq!(args["arguments"], "{\"q\":1}");
    // The client's answer goes to the vendor as a tool result.
    let acts = tr.client(
        &json!({"type": "conversation.item.create", "item": {"type": "function_call_output",
            "call_id": "call_1", "output": "{\"a\":2}"}})
        .to_string(),
    );
    assert!(acts.contains(&Act::ToolResult {
        call_id: "call_1".into(),
        output: "{\"a\":2}".into()
    }));
}

/// `response.cancel` ends the response for the client at once; its late output is dropped.
#[test]
fn a_client_cancel_ends_the_response_and_drops_its_output() {
    let mut tr = translator("gemini");
    ready(&mut tr);
    let _ = tr.vendor(S2sEvent::Audio {
        data: Bytes::from(vec![0u8; 480]),
        item_id: None,
        response_id: None,
    });
    let acts = tr.client(&json!({"type": "response.cancel"}).to_string());
    assert!(acts.contains(&Act::Cancel));
    let done = client_events(&acts)
        .into_iter()
        .find(|e| e["type"] == "response.done")
        .unwrap();
    assert_eq!(done["response"]["status"], "cancelled");
}

/// A barge-in the vendor reports (Gemini `interrupted`) tells a GA client to stop playing.
#[test]
fn a_vendor_interruption_is_a_speech_start_and_a_cancelled_response() {
    let mut tr = translator("gemini");
    ready(&mut tr);
    let _ = tr.vendor(S2sEvent::Audio {
        data: Bytes::from(vec![0u8; 480]),
        item_id: None,
        response_id: None,
    });
    let acts = tr.vendor(S2sEvent::InterruptedByServer);
    let k = kinds(&acts);
    assert_eq!(
        k.first().map(String::as_str),
        Some("conversation.item.added")
    );
    assert!(k.contains(&"input_audio_buffer.speech_started".to_string()));
    assert_eq!(k.last().map(String::as_str), Some("response.done"));
}

/// Hume EVI sends a WAV container per chunk: the PCM inside it, at its own rate.
#[test]
fn wav_chunks_are_unwrapped() {
    let pcm: Vec<u8> = (0..200u8).collect();
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&48_000u32.to_le_bytes());
    wav.extend_from_slice(&96_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(&pcm);
    let (rate, body) = pcm_of(&wav, 44_100);
    assert_eq!(rate, 48_000);
    assert_eq!(&*body, pcm.as_slice());
    let (rate, body) = pcm_of(&pcm, 16_000);
    assert_eq!((rate, body.len()), (16_000, 200), "raw PCM passes through");
}

/// TC-XL-02 (translator half) — 24 kHz client audio reaches the vendor at ITS rate.
#[test]
fn tc_xl_02_client_audio_is_resampled_to_the_vendor_rate() {
    let mut tr = translator("gemini");
    ready(&mut tr);
    // 1 s of a 440 Hz tone at 24 kHz, in 20 ms chunks.
    let samples: Vec<u8> = (0..24_000)
        .flat_map(|i| {
            let v = (f32::sin(i as f32 * 440.0 * std::f32::consts::TAU / 24_000.0) * 8000.0) as i16;
            v.to_le_bytes()
        })
        .collect();
    let mut out = 0usize;
    for chunk in samples.chunks(960) {
        out += tr.audio_for_vendor(chunk).len();
    }
    out += tr.audio_tail_for_vendor().map_or(0, |t| t.len());
    // 16 kHz: 32 000 bytes a second, within the resampler's one-chunk latency and padding.
    assert!((31_000..=33_000).contains(&out), "{out}");
}

// ---------------------------------------------------------------------------------------------
// The plan (pre-upgrade)
// ---------------------------------------------------------------------------------------------

fn endpoint(entry: Value) -> VoiceEndpoint {
    let blob = json!({ "ep": entry }).to_string();
    bud_auth::credentials::parse_voice_blob(&blob, &bud_auth::CredentialDecryptor::disabled())
        .unwrap()
        .remove("ep")
        .unwrap()
}

/// FRD-023 RT7.2 🔒 — a Nova 2 Sonic deployment without its AWS key pair or its region is
/// refused before the upgrade; nothing falls back to the gateway's AWS identity.
#[test]
fn rt7_2_nova_needs_the_deployment_key_pair_and_region() {
    let entry = json!({"vendor": "nova_sonic", "endpoints": ["realtime_session"],
        "model": "amazon.nova-2-sonic-v1:0", "provider_params": {"region": "us-east-1"}});
    let err = TranslatePlan::build(&endpoint(entry.clone()), None).unwrap_err();
    assert!(
        matches!(err, UpstreamError::Misconfigured(ref m) if m.contains("AWS access key pair")),
        "{err:?}"
    );

    let mut ep = endpoint(entry);
    ep.credential_parts = Some(
        [("access_key_id", "AKIDX"), ("secret_access_key", "s")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    );
    let plan = TranslatePlan::build(&ep, None).unwrap();
    assert_eq!(
        plan.aws.as_ref().map(|a| a.access_key_id.as_str()),
        Some("AKIDX")
    );
    assert_eq!(plan.base.endpoint.as_deref(), Some("us-east-1"));
    assert!(
        plan.base.api_key.is_empty(),
        "no API key rides a SigV4 session"
    );

    ep.provider_params.clear();
    let err = TranslatePlan::build(&ep, None).unwrap_err();
    assert!(
        matches!(err, UpstreamError::Misconfigured(ref m) if m.contains("region")),
        "{err:?}"
    );
}

/// The address is the deployment's (`api_base`, http(s) converted to ws(s): F-5) and must clear
/// the SSRF validator; the defaults are the deployment's.
#[test]
fn a_plan_takes_address_credential_and_defaults_from_the_deployment() {
    let mut ep = endpoint(
        json!({"vendor": "gemini", "endpoints": ["realtime_session"],
        "model": "gemini-3.8-live", "api_base": "https://gemini-proxy.example/ws",
        "config": {"realtime": {"defaults": {"voice": "Kore", "instructions": "hi",
            "turn_detection": {"type": "server_vad", "silence_duration_ms": 400}}}}}),
    );
    ep.credential = Some("gkey".into());
    let settings = ep.config.realtime.clone();
    let plan = TranslatePlan::build(&ep, settings.as_ref()).unwrap();
    assert_eq!(plan.base.api_key, "gkey");
    assert_eq!(plan.base.model, "gemini-3.8-live");
    assert_eq!(
        plan.base.realtime_endpoint_override.as_deref(),
        Some("wss://gemini-proxy.example/ws")
    );
    assert!(plan.ssrf.is_some());
    assert_eq!(plan.base.voice.as_deref(), Some("Kore"));
    assert!(matches!(
        plan.base.turn_detection,
        Some(TurnDetectionConfig::ServerVad {
            silence_duration_ms: Some(400),
            ..
        })
    ));
    // Debug never prints the key.
    assert!(!format!("{plan:?}").contains("gkey"));

    ep.credential = None;
    assert_eq!(
        TranslatePlan::build(&ep, settings.as_ref()).unwrap_err(),
        UpstreamError::MissingCredential
    );
}

/// RT7 vendors are speech-to-speech only (CONTRACTS C7).
#[test]
fn a_transcription_entry_on_a_translate_vendor_is_refused() {
    let mut ep = endpoint(
        json!({"vendor": "deepgram_voice_agent", "endpoints": ["realtime_session"],
        "config": {"realtime": {"session_type": "transcription"}}}),
    );
    ep.credential = Some("k".into());
    let settings = ep.config.realtime.clone();
    assert!(matches!(
        TranslatePlan::build(&ep, settings.as_ref()),
        Err(UpstreamError::Misconfigured(_))
    ));
}

/// An ElevenLabs agent needs its agent id (`model`): refused before the upgrade.
#[test]
fn a_plan_the_provider_cannot_build_is_refused_up_front() {
    let mut ep =
        endpoint(json!({"vendor": "elevenlabs_convai", "endpoints": ["realtime_session"]}));
    ep.credential = Some("xi".into());
    assert!(matches!(
        TranslatePlan::build(&ep, None),
        Err(UpstreamError::Misconfigured(_))
    ));
    ep.model = Some("agent_123".into());
    assert!(TranslatePlan::build(&ep, None).is_ok());
}

#[test]
fn the_translate_vendors_are_c7s() {
    for v in [
        "gemini",
        "nova_sonic",
        "deepgram_voice_agent",
        "elevenlabs_convai",
        "hume_evi",
    ] {
        assert!(is_translate_vendor(v), "{v}");
    }
    for v in ["openai", "azure_openai", "grok", "deepgram", "elevenlabs"] {
        assert!(!is_translate_vendor(v), "{v}");
    }
    assert!(vendor_info("deepgram_voice_agent").unwrap().per_minute);
    assert!(!vendor_info("gemini").unwrap().per_minute);
}

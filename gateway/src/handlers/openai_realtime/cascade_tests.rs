//! Spec 025 §5.6 — the GA face of a voice agent, as a pure translation: GA client events in, session
//! acts out; the in-process `/ws` session's messages in, GA server events out.

use std::sync::Arc;

use bud_auth::parse_voice_agent_blob;
use serde_json::{Value, json};

use super::*;

fn agent(extra: Value) -> ResolvedVoiceAgent {
    let mut e = json!({"prompt_id": "c0de", "version": 3,
        "stt": {"endpoint_id": "ep-stt", "language": "en"},
        "tts": {"endpoint_id": "ep-tts", "voice": "agent-voice", "speed": 1.0}});
    if let (Some(b), Some(x)) = (e.as_object_mut(), extra.as_object()) {
        b.extend(x.clone());
    }
    ResolvedVoiceAgent {
        alias: "prompt:support".into(),
        prompt_name: "support".into(),
        prompt_id: "c0de".into(),
        version: 3,
        meta: Default::default(),
        entry: Arc::new(parse_voice_agent_blob(&e.to_string()).unwrap()),
    }
}

fn ga(extra: Value) -> CascadeGa {
    CascadeGa::new("sess_1", "prompt:support", &agent(extra))
}

fn parse(events: &[String]) -> Vec<Value> {
    events
        .iter()
        .map(|e| serde_json::from_str(e).unwrap())
        .collect()
}

fn types(events: &[String]) -> Vec<String> {
    parse(events)
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_session_shows_the_agent_not_its_prompt() {
    let s = ga(json!({}));
    let created: Value = serde_json::from_str(&s.session_created()).unwrap();
    let sess = &created["session"];
    assert_eq!(created["type"], "session.created");
    assert_eq!(sess["model"], "prompt:support");
    assert_eq!(sess["instructions"], "");
    assert_eq!(sess["tools"], json!([]));
    assert_eq!(sess["prompt"], json!({"id": "support", "version": "3"}));
    assert_eq!(sess["audio"]["output"]["voice"], "agent-voice");
    assert_eq!(
        sess["audio"]["input"]["format"],
        json!({"type": "audio/pcm", "rate": 24000})
    );
    assert_eq!(
        sess["audio"]["input"]["turn_detection"]["type"],
        "semantic_vad"
    );
}

#[test]
fn session_update_refuses_what_belongs_to_the_agent_and_applies_the_rest() {
    let mut s = ga(json!({}));
    let (events, acts) = s.client(
        &json!({"type": "session.update", "event_id": "e1", "session": {
            "type": "realtime", "instructions": "be a pirate", "tools": [{"type": "function", "name": "f"}],
            "output_modalities": ["text"], "audio": {"input": {"turn_detection": null}},
            "prompt": {"id": "support", "variables": {"customer_id": "c1"}}
        }})
        .to_string(),
    );
    let evs = parse(&events);
    let params: Vec<_> = evs
        .iter()
        .filter(|e| e["type"] == "error")
        .map(|e| e["error"]["param"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(params, vec!["session.instructions", "session.tools"]);
    assert!(
        evs.iter()
            .all(|e| e["type"] != "error" || e["error"]["code"] == "event_not_allowed")
    );
    assert_eq!(evs.last().unwrap()["type"], "session.updated");
    assert_eq!(
        evs.last().unwrap()["session"]["output_modalities"],
        json!(["text"])
    );
    assert!(acts.contains(&ClientAct::TextOnly(true)));
    assert!(acts.contains(&ClientAct::Manual(true)));
    assert!(acts.contains(&ClientAct::Start));
    assert!(
        matches!(acts.iter().find(|a| matches!(a, ClientAct::Variables(_))), Some(ClientAct::Variables(v)) if v["customer_id"] == "c1")
    );
}

#[test]
fn another_agent_or_version_is_refused() {
    let mut s = ga(json!({}));
    let (events, _) = s.client(
        &json!({"type": "session.update", "session": {"prompt": {"id": "other"}}}).to_string(),
    );
    assert_eq!(parse(&events)[0]["error"]["param"], "session.prompt.id");
    let (events, _) = s.client(&json!({"type": "session.update", "session": {"prompt": {"id": "support", "version": "9"}}}).to_string());
    assert_eq!(
        parse(&events)[0]["error"]["param"],
        "session.prompt.version"
    );
}

#[test]
fn the_voice_changes_only_where_the_agent_allows() {
    let mut s = ga(json!({}));
    let (events, _) = s.client(
        &json!({"type": "session.update", "session": {"audio": {"output": {"voice": "alloy"}}}})
            .to_string(),
    );
    assert_eq!(
        parse(&events)[0]["error"]["param"],
        "session.audio.output.voice"
    );
    assert_eq!(
        s.core_config().2.unwrap().voice_id,
        None,
        "the agent's voice is applied by the core"
    );

    let mut s = ga(json!({"session_overrides": ["tts.voice"]}));
    let (events, _) = s.client(
        &json!({"type": "session.update", "session": {"audio": {"output": {"voice": "alloy"}}}})
            .to_string(),
    );
    assert_eq!(types(&events), vec!["session.updated"]);
    assert_eq!(
        s.core_config().2.unwrap().voice_id.as_deref(),
        Some("alloy")
    );
}

fn reported_turn_detection(s: &CascadeGa) -> Value {
    let created: Value = serde_json::from_str(&s.session_created()).unwrap();
    created["session"]["audio"]["input"]["turn_detection"].clone()
}

/// The session says how the agent takes turns: a Silence agent is `server_vad`, and an agent whose
/// caller cannot interrupt says `interrupt_response: false` — so a GA client knows not to stop its
/// playback when the caller talks. Both were reported as semantic and interruptible regardless.
#[test]
fn the_session_reports_the_agents_own_turn_taking() {
    let td = reported_turn_detection(&ga(json!({"turn_detection": {"type": "server_vad"}})));
    assert_eq!(td["type"], "server_vad");
    assert!(
        td.get("eagerness").is_none(),
        "eagerness is semantic turn-taking's"
    );
    assert_eq!(td["interrupt_response"], true);

    let td = reported_turn_detection(&ga(json!({"interruption": {"enabled": false}})));
    assert_eq!(td["type"], "semantic_vad");
    assert_eq!(td["interrupt_response"], false);

    assert!(reported_turn_detection(&ga(json!({"turn_detection": {"type": "manual"}}))).is_null());
}

#[test]
fn eagerness_is_refused_where_turns_are_not_semantic() {
    let mut s = ga(json!({"turn_detection": {"type": "server_vad"},
                          "session_overrides": ["turn_detection.eagerness"]}));
    let (events, _) = s.client(
        &json!({"type": "session.update", "session": {"audio": {"input": {
        "turn_detection": {"type": "server_vad", "eagerness": "high"}}}}})
        .to_string(),
    );
    let evs = parse(&events);
    assert_eq!(
        evs[0]["error"]["param"],
        "session.audio.input.turn_detection.eagerness"
    );
    assert!(s.core_config().1.turn_detection.is_none());
}

fn eagerness_update(eagerness: &str) -> String {
    json!({"type": "session.update", "session": {"audio": {"input": {
        "turn_detection": {"type": "semantic_vad", "eagerness": eagerness}}}}})
    .to_string()
}

/// S-4 — turn-taking eagerness is the caller's only where the agent allows it.
#[test]
fn eagerness_changes_only_where_the_agent_allows() {
    let low = json!({"turn_detection": {"type": "semantic", "eagerness": "low"}});
    let mut s = ga(low.clone());
    let (events, _) = s.client(&eagerness_update("high"));
    let evs = parse(&events);
    assert_eq!(
        evs[0]["error"]["param"],
        "session.audio.input.turn_detection.eagerness"
    );
    assert_eq!(
        evs.last().unwrap()["session"]["audio"]["input"]["turn_detection"]["eagerness"],
        "low",
        "the session keeps the agent's eagerness"
    );
    assert!(
        s.core_config().1.turn_detection.is_none(),
        "the agent's eagerness is applied by the core"
    );

    let mut allowed = low;
    allowed["session_overrides"] = json!(["turn_detection.eagerness"]);
    let mut s = ga(allowed);
    let (events, _) = s.client(&eagerness_update("high"));
    assert_eq!(types(&events), vec!["session.updated"]);
    assert_eq!(
        parse(&events)[0]["session"]["audio"]["input"]["turn_detection"]["eagerness"],
        "high"
    );
    let td = s
        .core_config()
        .1
        .turn_detection
        .expect("the caller's eagerness");
    assert!(td.enabled && !td.eager);
    assert_eq!(td.threshold, Some(0.3));
}

#[test]
fn the_agents_own_eagerness_is_not_a_change_and_an_unknown_one_is_refused() {
    let mut s = ga(json!({"turn_detection": {"type": "semantic", "eagerness": "low"}}));
    let (events, _) = s.client(&eagerness_update("low"));
    assert_eq!(types(&events), vec!["session.updated"]);

    let mut s = ga(json!({"session_overrides": ["turn_detection.eagerness"]}));
    let (events, _) = s.client(&eagerness_update("fast"));
    let evs = parse(&events);
    assert_eq!(evs[0]["type"], "error");
    assert_eq!(
        evs[0]["error"]["param"],
        "session.audio.input.turn_detection.eagerness"
    );
    assert!(s.core_config().1.turn_detection.is_none());
}

#[test]
fn audio_starts_the_session_and_is_decoded() {
    let mut s = ga(json!({}));
    let pcm = vec![1u8, 0, 2, 0];
    let (events, acts) = s.client(
        &json!({"type": "input_audio_buffer.append", "audio": BASE64.encode(&pcm)}).to_string(),
    );
    assert!(events.is_empty());
    assert_eq!(
        acts,
        vec![ClientAct::Start, ClientAct::Audio(Bytes::from(pcm))]
    );
    let (events, acts) =
        s.client(&json!({"type": "input_audio_buffer.append", "audio": "!!!"}).to_string());
    assert_eq!(parse(&events)[0]["error"]["code"], "invalid_event");
    assert!(acts.is_empty());
}

#[test]
fn a_typed_message_waits_for_response_create() {
    let mut s = ga(json!({}));
    let (events, acts) = s.client(
        &json!({"type": "conversation.item.create", "item": {"type": "message", "role": "user",
               "content": [{"type": "input_text", "text": "where is my order"}]}})
        .to_string(),
    );
    assert_eq!(
        types(&events),
        vec!["conversation.item.added", "conversation.item.done"]
    );
    assert!(!acts.iter().any(|a| matches!(a, ClientAct::Turn(_))));
    let (_, acts) = s.client(&json!({"type": "response.create"}).to_string());
    assert!(acts.contains(&ClientAct::Turn("where is my order".into())));
}

#[test]
fn client_tools_and_per_response_overrides_are_refused() {
    let mut s = ga(json!({}));
    let (events, _) = s.client(
        &json!({"type": "conversation.item.create", "item": {"type": "function_call_output", "call_id": "c", "output": "x"}}).to_string(),
    );
    assert_eq!(parse(&events)[0]["error"]["code"], "event_not_allowed");
    let (events, acts) = s.client(
        &json!({"type": "response.create", "response": {"instructions": "shout"}}).to_string(),
    );
    assert_eq!(parse(&events)[0]["error"]["param"], "response.instructions");
    assert!(acts.is_empty());
    let (events, _) =
        s.client(&json!({"type": "conversation.item.delete", "item_id": "x"}).to_string());
    assert_eq!(parse(&events)[0]["error"]["code"], "event_not_allowed");
}

#[test]
fn cancel_truncate_and_auth_become_acts() {
    let mut s = ga(json!({}));
    assert_eq!(
        s.client(&json!({"type": "response.cancel"}).to_string()).1,
        vec![ClientAct::Cancel]
    );
    let (events, acts) = s.client(
        &json!({"type": "conversation.item.truncate", "item_id": "item_1", "audio_end_ms": 1500})
            .to_string(),
    );
    assert_eq!(acts, vec![ClientAct::Truncate(1500)]);
    assert_eq!(parse(&events)[0]["type"], "conversation.item.truncated");
    assert_eq!(
        s.client(&json!({"type": "bud.session.auth", "token": "jwt2"}).to_string())
            .1,
        vec![ClientAct::Auth("jwt2".into())]
    );
}

/// The whole turn: user speech, the agent's response with a tool, audio, done — in GA order.
#[test]
fn an_agent_turn_reads_as_a_ga_response() {
    let mut s = ga(json!({}));
    let mut all = Vec::new();
    all.extend(s.core(&OutgoingMessage::STTResult {
        transcript: "where is".into(),
        is_final: false,
        is_speech_final: false,
        confidence: 0.9,
        segment_transcript: None,
        translations: vec![],
        detected_language: None,
    }));
    all.extend(s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 0,
        kind: "agent".into(),
        input: Some("where is my order".into()),
    }));
    all.extend(s.core(&OutgoingMessage::AgentResponseCreated {
        turn_index: 0,
        response_id: "resp_bp_1".into(),
    }));
    all.extend(s.core(&OutgoingMessage::AgentTool {
        turn_index: 0,
        item_id: "mcp_1".into(),
        name: "lookup_order".into(),
        status: "in_progress".into(),
    }));
    all.extend(s.core(&OutgoingMessage::AgentTool {
        turn_index: 0,
        item_id: "mcp_1".into(),
        name: "lookup_order".into(),
        status: "completed".into(),
    }));
    all.extend(s.core(&OutgoingMessage::AssistantTranscript {
        turn_index: 0,
        delta: "It shipped.".into(),
    }));
    all.extend(s.audio(&[0u8, 1, 2, 3]));
    all.extend(s.core(&OutgoingMessage::AgentResponseDone {
        turn_index: 0,
        kind: "agent".into(),
        response_id: Some("resp_bp_1".into()),
        status: "completed".into(),
        usage: Some(json!({"input_tokens": 10, "output_tokens": 4, "total_tokens": 14})),
        transcript: "It shipped.".into(),
    }));
    assert_eq!(
        types(&all),
        vec![
            "conversation.item.added",
            "input_audio_buffer.speech_started",
            "input_audio_buffer.speech_stopped",
            "input_audio_buffer.committed",
            "conversation.item.input_audio_transcription.completed",
            "conversation.item.done",
            "response.created",
            "response.output_item.added",
            "response.mcp_call.in_progress",
            "response.mcp_call.completed",
            "response.output_item.done",
            "response.output_item.added",
            "conversation.item.added",
            "response.content_part.added",
            "response.output_audio_transcript.delta",
            "response.output_audio.delta",
            "response.output_audio.done",
            "response.output_audio_transcript.done",
            "response.content_part.done",
            "response.output_item.done",
            "conversation.item.done",
            "response.done",
        ]
    );
    let evs = parse(&all);
    let done = evs.last().unwrap();
    assert_eq!(done["response"]["status"], "completed");
    assert_eq!(done["response"]["usage"]["total_tokens"], 14);
    assert_eq!(done["response"]["metadata"]["bud_response_id"], "resp_bp_1");
    assert_eq!(done["response"]["output"][0]["type"], "mcp_call");
    assert_eq!(
        done["response"]["output"][1]["content"][0]["transcript"],
        "It shipped."
    );
    let completed = evs
        .iter()
        .find(|e| e["type"] == "conversation.item.input_audio_transcription.completed")
        .unwrap();
    assert_eq!(completed["transcript"], "where is my order");
    let audio = evs
        .iter()
        .find(|e| e["type"] == "response.output_audio.delta")
        .unwrap();
    assert_eq!(
        BASE64.decode(audio["delta"].as_str().unwrap()).unwrap(),
        vec![0u8, 1, 2, 3]
    );
}

#[test]
fn a_barge_in_cancels_and_truncates_the_assistant_item() {
    let mut s = ga(json!({}));
    s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 4,
        kind: "agent".into(),
        input: Some("hi".into()),
    });
    let started = parse(&s.core(&OutgoingMessage::AssistantTranscript {
        turn_index: 4,
        delta: "Hello the".into(),
    }));
    let item_id = started
        .iter()
        .find(|e| e["type"] == "response.output_item.added")
        .unwrap()["item"]["id"]
        .clone();
    let events = parse(&s.core(&OutgoingMessage::AgentTruncated {
        turn_index: 4,
        response_id: Some("r".into()),
        spoken: "Hello".into(),
        audio_end_ms: 420,
    }));
    assert_eq!(events[0]["type"], "conversation.item.truncated");
    assert_eq!(events[0]["item_id"], item_id);
    assert_eq!(events[0]["audio_end_ms"], 420);
    let done = parse(&s.core(&OutgoingMessage::AgentResponseDone {
        turn_index: 4,
        kind: "agent".into(),
        response_id: Some("r".into()),
        status: "cancelled".into(),
        usage: None,
        transcript: "Hello".into(),
    }));
    let last = done.last().unwrap();
    assert_eq!(last["response"]["status"], "cancelled");
    assert_eq!(
        last["response"]["status_details"]["reason"],
        "turn_detected"
    );
}

#[test]
fn text_mode_streams_text_events() {
    let mut s = ga(json!({}));
    s.client(
        &json!({"type": "session.update", "session": {"output_modalities": ["text"]}}).to_string(),
    );
    s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 0,
        kind: "agent".into(),
        input: None,
    });
    let evs = types(&s.core(&OutgoingMessage::AssistantTranscript {
        turn_index: 0,
        delta: "Hi".into(),
    }));
    assert!(evs.contains(&"response.output_text.delta".to_string()));
    assert!(s.audio(&[1, 2]).is_empty(), "text mode never sends audio");
    let done = types(&s.core(&OutgoingMessage::AgentResponseDone {
        turn_index: 0,
        kind: "agent".into(),
        response_id: None,
        status: "completed".into(),
        usage: None,
        transcript: "Hi".into(),
    }));
    assert!(done.contains(&"response.output_text.done".to_string()));
    assert!(!done.contains(&"response.output_audio.done".to_string()));
}

#[test]
fn errors_keep_their_codes() {
    let mut s = ga(json!({}));
    let e = parse(&s.core(&OutgoingMessage::AgentError {
        code: "rate_limit_exceeded".into(),
        message: "slow down".into(),
    }));
    assert_eq!(e[0]["error"]["code"], "rate_limit_exceeded");
    assert_eq!(e[0]["error"]["type"], "invalid_request_error");
    let e = parse(&s.core(&OutgoingMessage::Error {
        message: "voice_leg_unavailable: the TTS deployment is gone".into(),
    }));
    assert_eq!(e[0]["error"]["code"], "voice_leg_unavailable");
    assert_eq!(e[0]["error"]["message"], "the TTS deployment is gone");
    let e = parse(&s.core(&OutgoingMessage::Error {
        message: "Something odd".into(),
    }));
    assert_eq!(e[0]["error"]["code"], "session_error");
}

#[test]
fn the_greeting_is_a_response_of_its_own() {
    let mut s = ga(json!({}));
    let started = parse(&s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 0,
        kind: "greeting".into(),
        input: None,
    }));
    assert_eq!(started[0]["type"], "response.created");
    s.core(&OutgoingMessage::AssistantTranscript {
        turn_index: 0,
        delta: "Hi there.".into(),
    });
    let done = parse(&s.core(&OutgoingMessage::AgentResponseDone {
        turn_index: 0,
        kind: "greeting".into(),
        response_id: None,
        status: "completed".into(),
        usage: None,
        transcript: "Hi there.".into(),
    }));
    assert_eq!(
        done.last().unwrap()["response"]["metadata"]["bud_greeting"],
        true
    );
}

#[test]
fn only_prompt_models_are_agents() {
    assert!(is_agent_model("prompt:support"));
    assert!(!is_agent_model("gpt-realtime"));
}

// ------------------------------------------------------------------------------------------------
// Interaction mode behind the GA wire
// ------------------------------------------------------------------------------------------------

/// A session whose core said it runs interaction mode.
fn interactive(extra: Value) -> CascadeGa {
    let mut s = ga(extra);
    let ready = OutgoingMessage::Ready {
        protocol_version: "1.1".into(),
        stream_id: "sess_1".into(),
        livekit_room_name: None,
        livekit_url: None,
        waav_participant_identity: None,
        waav_participant_name: None,
        resolved_alias: None,
        audio_in_codec: None,
        audio_out_codec: None,
        interaction: Some(json!({"enabled": true, "protocol_version": "1.1", "regime": "cascade"})),
    };
    let events = parse(&s.core(&ready));
    assert_eq!(events[0]["type"], "bud.interaction.state");
    assert_eq!(events[0]["interaction"]["regime"], "cascade");
    assert!(events[0]["event_id"].is_string());
    s
}

fn line(utterance: &str, turn: u64, words: &str, heard: &str, extra: Value) -> String {
    let mut event = json!({"type": "assistant_transcript", "turn_index": turn, "delta": words,
                           "source": "front", "kind": "ack", "verbatim": false,
                           "utterance_id": utterance, "heard": heard});
    event
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    event.to_string()
}

fn final_transcript(text: &str) -> OutgoingMessage {
    OutgoingMessage::STTResult {
        transcript: text.into(),
        is_final: true,
        is_speech_final: true,
        confidence: 0.9,
        segment_transcript: None,
        translations: vec![],
        detected_language: None,
    }
}

#[test]
fn a_plain_session_sends_nothing_for_ready_or_a_warning_of_another_kind() {
    let mut s = ga(json!({}));
    let ready = OutgoingMessage::Ready {
        protocol_version: "1.0".into(),
        stream_id: "sess_1".into(),
        livekit_room_name: None,
        livekit_url: None,
        waav_participant_identity: None,
        waav_participant_name: None,
        resolved_alias: None,
        audio_in_codec: None,
        audio_out_codec: None,
        interaction: None,
    };
    assert!(s.core(&ready).is_empty());
    let other = OutgoingMessage::ConfigWarning {
        code: "reasoning_model_on_voice_path".into(),
        message: "slow".into(),
        detail: None,
    };
    assert!(s.core(&other).is_empty());
    // Audio with no response in flight is dropped, as before.
    assert!(s.audio(&[0u8; 480]).is_empty());

    // One about the `interaction` block is told, by its code or by the path it names.
    let refused = OutgoingMessage::ConfigWarning {
        code: "interaction_override_refused".into(),
        message: "no".into(),
        detail: Some(json!({"path": "interaction"})),
    };
    let events = parse(&s.core(&refused));
    assert_eq!(events[0]["type"], "bud.warning");
    assert_eq!(events[0]["code"], "interaction_override_refused");
    assert_eq!(events[0]["detail"]["path"], "interaction");
    let unknown = OutgoingMessage::ConfigWarning {
        code: "unknown_config_keys".into(),
        message: "?".into(),
        detail: Some(json!({"path": "interaction.frnt"})),
    };
    assert_eq!(types(&s.core(&unknown)), vec!["bud.warning"]);
}

#[test]
fn each_interaction_event_keeps_its_body_under_its_bud_name() {
    let mut s = interactive(json!({}));
    for (ws, bud) in [
        ("task_created", "bud.task.created"),
        ("task_progress", "bud.task.progress"),
        ("task_question", "bud.task.question"),
        ("task_answer_received", "bud.task.answer_received"),
        ("task_result", "bud.task.result"),
        ("task_cancelled", "bud.task.cancelled"),
        ("interaction_updated", "bud.interaction.updated"),
    ] {
        let body =
            json!({"type": ws, "task_id": "task_1", "t_ms": 42, "effects": "none_committed"});
        let events = parse(&s.interaction(&body.to_string()));
        assert_eq!(events.len(), 1, "{ws}");
        assert_eq!(events[0]["type"], bud);
        assert_eq!(events[0]["task_id"], "task_1");
        assert_eq!(events[0]["t_ms"], 42);
        assert_eq!(events[0]["effects"], "none_committed");
        assert!(events[0]["event_id"].is_string());
    }
    // An error keeps its code, in the GA envelope.
    let error = json!({"type": "error", "message": "That question was already answered.", "code": "question_closed"});
    let events = parse(&s.interaction(&error.to_string()));
    assert_eq!(events[0]["type"], "error");
    assert_eq!(events[0]["error"]["code"], "question_closed");
    assert_eq!(
        events[0]["error"]["message"],
        "That question was already answered."
    );
    // An event this table does not know is not sent under a made-up name.
    assert!(
        s.interaction(&json!({"type": "audio_segment"}).to_string())
            .is_empty()
    );
}

#[test]
fn a_line_of_the_agent_is_a_response_with_what_it_is() {
    let mut s = interactive(json!({}));
    // Its first audio comes ahead of the event that says it plays: it is held, not lost.
    assert!(s.audio(&[1u8; 480]).is_empty());
    let relay = json!({"source": "background", "kind": "relay", "verbatim": true,
                       "task_id": "task_1", "sentence": {"attempt": 0, "seq": 0}});
    let opened = s.interaction(&line("u_7", 1, "The capital is Paris.", "none", relay));
    let events = parse(&opened);
    assert_eq!(
        types(&opened),
        vec![
            "response.created",
            "response.output_item.added",
            "conversation.item.added",
            "response.content_part.added",
            "response.output_audio_transcript.delta",
            "response.output_audio.delta",
        ]
    );
    let metadata = &events[0]["response"]["metadata"];
    assert_eq!(metadata["bud_agent"], "support");
    assert_eq!(metadata["bud_turn"], 1);
    assert_eq!(metadata["bud_utterance_id"], "u_7");
    assert_eq!(metadata["bud_source"], "background");
    assert_eq!(metadata["bud_kind"], "relay");
    assert_eq!(metadata["bud_verbatim"], true);
    assert_eq!(metadata["bud_task_id"], "task_1");
    assert_eq!(metadata["bud_sentence"], json!({"attempt": 0, "seq": 0}));
    assert!(metadata.get("bud_act").is_none());
    assert_eq!(events[4]["delta"], "The capital is Paris.");
    let response = events[0]["response"]["id"].clone();
    assert_eq!(events[5]["response_id"], response);

    // More audio goes straight out; all of it heard ends the response.
    assert_eq!(
        types(&s.audio(&[1u8; 480])),
        vec!["response.output_audio.delta"]
    );
    let done = parse(&s.interaction(&line("u_7", 1, "", "full", json!({}))));
    let last = done.last().unwrap();
    assert_eq!(last["type"], "response.done");
    assert_eq!(last["response"]["id"], response);
    assert_eq!(last["response"]["status"], "completed");
    assert_eq!(last["response"]["metadata"]["bud_utterance_id"], "u_7");
    assert_eq!(
        last["response"]["output"][0]["content"][0]["transcript"],
        "The capital is Paris."
    );
    // The same report again, or one for another line, ends nothing.
    assert!(
        s.interaction(&line("u_7", 1, "", "full", json!({})))
            .is_empty()
    );
}

#[test]
fn a_cut_line_is_truncated_and_its_response_cancelled() {
    let mut s = interactive(json!({}));
    let opened = parse(&s.interaction(&line("u_3", 1, "While I look that up", "none", json!({}))));
    let item = opened[1]["item"]["id"].clone();
    let stopped = json!({"type": "agent_audio_stopped", "utterance_ids": ["u_2", "u_3"],
                         "reason": "barge_in", "delivered_ms": [0, 1840], "t_ms": 9000});
    let events = parse(&s.interaction(&stopped.to_string()));
    assert_eq!(events[0]["type"], "bud.audio.stopped");
    assert_eq!(events[0]["reason"], "barge_in");
    assert_eq!(events[0]["delivered_ms"], json!([0, 1840]));
    assert_eq!(events[1]["type"], "conversation.item.truncated");
    assert_eq!(events[1]["item_id"], item);
    assert_eq!(events[1]["audio_end_ms"], 1840);
    let done = events.last().unwrap();
    assert_eq!(done["type"], "response.done");
    assert_eq!(done["response"]["status"], "cancelled");
    // What the caller heard of it comes later and ends nothing more.
    assert!(
        s.interaction(&line("u_3", 1, "", "partial", json!({})))
            .is_empty()
    );
    // A line that starts while another is still open closes that one first.
    s.interaction(&line("u_4", 1, "One.", "none", json!({})));
    let next = s.interaction(&line("u_5", 1, "Two.", "none", json!({})));
    assert!(types(&next).contains(&"response.done".to_string()));
    assert_eq!(
        types(&next)
            .iter()
            .filter(|t| *t == "response.created")
            .count(),
        1
    );
}

#[test]
fn the_callers_item_follows_the_floor_and_ends_with_the_turn() {
    let mut s = interactive(json!({}));
    let floor = |holder: &str| {
        json!({"type": "floor_state", "holder": holder, "lane": null,
                                      "reason": "user_speech", "t_ms": 1})
        .to_string()
    };
    let started = s.interaction(&floor("user"));
    assert_eq!(
        types(&started),
        vec![
            "conversation.item.added",
            "input_audio_buffer.speech_started",
            "bud.floor.state"
        ]
    );
    let item = parse(&started)[0]["item"]["id"].clone();
    // The floor is still the caller's: nothing is repeated.
    assert_eq!(
        types(&s.interaction(&floor("both"))),
        vec!["bud.floor.state"]
    );
    assert_eq!(
        types(&s.interaction(&floor("none"))),
        vec!["input_audio_buffer.speech_stopped", "bud.floor.state"]
    );
    // A final transcript is a delta of the item, and starts no speech of its own.
    let delta = parse(&s.core(&final_transcript("what is the capital of France")));
    assert_eq!(delta.len(), 1);
    assert_eq!(
        delta[0]["type"],
        "conversation.item.input_audio_transcription.delta"
    );
    assert_eq!(delta[0]["item_id"], item);

    // The agent's first line of that turn settles it: the item is complete, then the line.
    let answered = s.interaction(&line("u_1", 1, "Got it.", "none", json!({})));
    let events = parse(&answered);
    assert_eq!(
        types(&answered)[..4],
        [
            "input_audio_buffer.committed",
            "conversation.item.input_audio_transcription.completed",
            "conversation.item.done",
            "response.created",
        ]
    );
    assert_eq!(events[1]["transcript"], "what is the capital of France");
    assert_eq!(events[1]["item_id"], item);
    // A second line of the same turn completes nothing more.
    s.interaction(&line("u_1", 1, "", "full", json!({})));
    let more = types(&s.interaction(&line("u_2", 1, "Paris.", "none", json!({}))));
    assert_eq!(more[0], "response.created");
}

#[test]
fn a_turn_answered_before_its_transcript_completes_when_the_transcript_comes() {
    let mut s = interactive(json!({}));
    let floor = json!({"type": "floor_state", "holder": "user", "lane": null, "reason": "user_speech", "t_ms": 1});
    s.interaction(&floor.to_string());
    // The agent speaks at the eager end of the turn; the final transcript follows.
    let early = types(&s.interaction(&line("u_1", 1, "Okay.", "none", json!({}))));
    assert_eq!(early[0], "response.created");
    let late = s.core(&final_transcript("book a table"));
    assert_eq!(
        types(&late),
        vec![
            "conversation.item.input_audio_transcription.delta",
            "input_audio_buffer.speech_stopped",
            "input_audio_buffer.committed",
            "conversation.item.input_audio_transcription.completed",
            "conversation.item.done",
        ]
    );
    assert_eq!(parse(&late)[3]["transcript"], "book a table");
}

#[test]
fn the_three_client_messages_become_acts() {
    let mut s =
        ga(json!({"session_overrides": ["interaction.policy", "interaction.events.floor"]}));
    let answer = json!({"type": "bud.task.answer", "request_id": "q_1", "outcome": "accept", "value": "8 pm"});
    assert_eq!(
        s.client(&answer.to_string()).1,
        vec![ClientAct::TaskAnswer {
            request_id: "q_1".into(),
            outcome: HitlOutcome::Accept {
                value: Some(json!("8 pm"))
            },
        }]
    );
    let decline = json!({"type": "bud.task.answer", "request_id": "q_1", "outcome": "decline",
                         "correction": "make it Tuesday"});
    assert_eq!(
        s.client(&decline.to_string()).1,
        vec![ClientAct::TaskAnswer {
            request_id: "q_1".into(),
            outcome: HitlOutcome::Decline {
                correction: Some("make it Tuesday".into())
            },
        }]
    );
    let (events, acts) = s.client(
        &json!({"type": "bud.task.answer", "request_id": "q_1", "outcome": "maybe"}).to_string(),
    );
    assert!(acts.is_empty());
    assert_eq!(parse(&events)[0]["error"]["code"], "invalid_event");
    assert_eq!(
        s.client(&json!({"type": "bud.task.cancel", "task_id": "task_9"}).to_string())
            .1,
        vec![ClientAct::TaskCancel("task_9".into())]
    );
    assert_eq!(
        s.client(&json!({"type": "bud.task.seen", "task_id": "task_9"}).to_string())
            .1,
        vec![ClientAct::TaskSeen("task_9".into())]
    );
    let (events, acts) = s.client(&json!({"type": "bud.task.seen"}).to_string());
    assert!(acts.is_empty());
    assert_eq!(parse(&events)[0]["error"]["code"], "invalid_event");
    // None of them starts the session.
    assert!(!s.is_started());

    // A patch: what the agent lets a session set goes to the actor, the rest is refused here.
    let patch = json!({"type": "bud.interaction.update", "interaction": {
        "policy": {"ack_due_ms": 250},
        "events": {"floor": false},
        "duplex": {"barge_in": false},
        "front": {"preferred_name": null},
    }});
    let (events, acts) = s.client(&patch.to_string());
    assert_eq!(
        acts,
        vec![ClientAct::Update(
            json!({"policy": {"ack_due_ms": 250}, "events": {"floor": false}})
        )]
    );
    let refused = parse(&events);
    assert_eq!(refused[0]["type"], "bud.interaction.updated");
    assert_eq!(refused[0]["applied"], json!([]));
    assert_eq!(
        refused[0]["refused"],
        json!([{"path": "duplex.barge_in", "reason": "not_allowed"},
               {"path": "front.preferred_name", "reason": "not_allowed"}])
    );
    // An agent that lists `interaction` lets the whole block through; one that lists nothing, none.
    let mut open = ga(json!({"session_overrides": ["interaction"]}));
    let (events, acts) = open.client(&patch.to_string());
    assert!(events.is_empty());
    assert_eq!(acts, vec![ClientAct::Update(patch["interaction"].clone())]);
    let mut closed = ga(json!({}));
    let (events, acts) = closed.client(&patch.to_string());
    assert!(acts.is_empty());
    assert_eq!(parse(&events)[0]["refused"].as_array().unwrap().len(), 4);
}

// ------------------------------------------------------------------------------------------------
// A session's own tools and instructions, where the agent allows them
// ------------------------------------------------------------------------------------------------

fn lookup() -> Value {
    json!({"type": "function", "name": "lookup_order", "description": "Look up an order",
           "parameters": {"type": "object", "properties": {"order_number": {"type": "string"}}}})
}

#[test]
fn a_session_declares_tools_and_instructions_where_the_agent_allows() {
    let mut s = ga(json!({"session_overrides": ["tools", "instructions"]}));
    let update = json!({"type": "session.update", "session": {
        "type": "realtime", "instructions": "Be brief.", "tools": [lookup()]}});
    let (events, acts) = s.client(&update.to_string());
    assert_eq!(types(&events), vec!["session.updated"]);
    let session = &parse(&events)[0]["session"];
    assert_eq!(session["instructions"], "Be brief.");
    assert_eq!(session["tools"], json!([lookup()]));
    assert!(acts.contains(&ClientAct::Instructions(Some("Be brief.".into()))));
    assert!(acts.contains(&ClientAct::Tools(vec![lookup()])));
    // The same values again change nothing.
    let (_, acts) = s.client(&update.to_string());
    assert!(
        !acts
            .iter()
            .any(|a| matches!(a, ClientAct::Tools(_) | ClientAct::Instructions(_)))
    );

    // A tool that is not a function is the agent's kind of tool: refused, and nothing kept.
    let mcp = json!({"type": "session.update", "session": {"tools": [{"type": "mcp", "server_label": "x"}]}});
    let (events, acts) = s.client(&mcp.to_string());
    assert_eq!(parse(&events)[0]["error"]["param"], "session.tools");
    assert!(!acts.iter().any(|a| matches!(a, ClientAct::Tools(_))));
    assert_eq!(s.session_object()["tools"], json!([lookup()]));

    // Only `tools`: instructions stay the agent's.
    let mut tools_only = ga(json!({"session_overrides": ["tools"]}));
    let (events, acts) = tools_only.client(&update.to_string());
    assert_eq!(parse(&events)[0]["error"]["param"], "session.instructions");
    assert!(acts.contains(&ClientAct::Tools(vec![lookup()])));
    assert!(!acts.iter().any(|a| matches!(a, ClientAct::Instructions(_))));
}

#[test]
fn a_function_call_is_an_item_and_its_output_starts_the_next_run() {
    let mut s = ga(json!({"session_overrides": ["tools"]}));
    s.mark_started();
    let mut all = s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 2,
        kind: "agent".into(),
        input: Some("where is order 4417".into()),
    });
    all.extend(s.core(&OutgoingMessage::AgentFunctionCall {
        turn_index: 2,
        item_id: "fc_1".into(),
        call_id: "call_1".into(),
        name: "lookup_order".into(),
        arguments: "{\"order_number\":\"4417\"}".into(),
    }));
    all.extend(s.core(&OutgoingMessage::AgentResponseDone {
        turn_index: 2,
        kind: "agent".into(),
        response_id: None,
        status: "completed".into(),
        usage: None,
        transcript: String::new(),
    }));
    assert_eq!(
        types(&all),
        vec![
            "response.created",
            "response.output_item.added",
            "conversation.item.added",
            "response.function_call_arguments.delta",
            "response.function_call_arguments.done",
            "response.output_item.done",
            "conversation.item.done",
            "response.done",
        ]
    );
    let events = parse(&all);
    assert_eq!(events[1]["item"]["type"], "function_call");
    assert_eq!(events[1]["item"]["call_id"], "call_1");
    assert_eq!(events[1]["item"]["name"], "lookup_order");
    assert_eq!(events[4]["call_id"], "call_1");
    assert_eq!(events[4]["item_id"], "fc_1");
    assert_eq!(events[4]["arguments"], "{\"order_number\":\"4417\"}");
    assert_eq!(events[7]["response"]["output"][0]["type"], "function_call");
    assert_eq!(events[7]["response"]["output"][0]["status"], "completed");

    // The client's answer: an item now, a run at `response.create`.
    let output = json!({"type": "conversation.item.create", "item": {
        "type": "function_call_output", "call_id": "call_1", "output": "{\"status\":\"shipped\"}"}});
    let (events, acts) = s.client(&output.to_string());
    assert_eq!(
        types(&events),
        vec!["conversation.item.added", "conversation.item.done"]
    );
    assert_eq!(
        acts,
        vec![ClientAct::Item(
            json!({"type": "function_call_output", "call_id": "call_1",
                                    "output": "{\"status\":\"shipped\"}"})
        )]
    );
    let (events, acts) = s.client(&json!({"type": "response.create"}).to_string());
    assert!(events.is_empty());
    assert_eq!(
        acts,
        vec![ClientAct::Respond {
            text: String::new(),
            instructions: None
        }]
    );
    // Nothing is staged any more: a bare `response.create` asks for nothing.
    assert!(
        s.client(&json!({"type": "response.create"}).to_string())
            .1
            .is_empty()
    );
}

#[test]
fn a_response_with_instructions_is_a_run_of_its_own_with_the_clients_metadata() {
    let mut s = ga(json!({"session_overrides": ["instructions"]}));
    s.mark_started();
    let create = json!({"type": "response.create", "response": {
        "instructions": "Say exactly: your table is booked.",
        "metadata": {"bud_kind": "say", "bud_utterance_id": "u_9"}}});
    let (events, acts) = s.client(&create.to_string());
    assert!(events.is_empty());
    assert_eq!(
        acts,
        vec![ClientAct::Respond {
            text: String::new(),
            instructions: Some("Say exactly: your table is booked.".into()),
        }]
    );
    let started = parse(&s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 0,
        kind: "agent".into(),
        input: None,
    }));
    assert_eq!(started[0]["type"], "response.created");
    let metadata = &started[0]["response"]["metadata"];
    assert_eq!(metadata["bud_kind"], "say");
    assert_eq!(metadata["bud_utterance_id"], "u_9");
    assert_eq!(metadata["bud_agent"], "support");
    s.core(&OutgoingMessage::AgentResponseCreated {
        turn_index: 0,
        response_id: "resp_bp_1".into(),
    });
    let done = parse(&s.core(&OutgoingMessage::AgentResponseDone {
        turn_index: 0,
        kind: "agent".into(),
        response_id: Some("resp_bp_1".into()),
        status: "completed".into(),
        usage: None,
        transcript: String::new(),
    }));
    assert_eq!(done[0]["response"]["metadata"]["bud_kind"], "say");
    assert_eq!(
        done[0]["response"]["metadata"]["bud_response_id"],
        "resp_bp_1"
    );
    // The next response is the agent's own again.
    let next = parse(&s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 1,
        kind: "agent".into(),
        input: Some("hello".into()),
    }));
    assert!(next[0]["response"]["metadata"].get("bud_kind").is_none());

    // A system item is context: it waits for the next run, with what was typed.
    let note = json!({"type": "conversation.item.create", "item": {"type": "message", "role": "system",
                      "content": [{"type": "input_text", "text": "The shop closes at six."}]}});
    let (events, acts) = s.client(&note.to_string());
    assert_eq!(parse(&events)[0]["item"]["role"], "system");
    assert_eq!(
        acts,
        vec![ClientAct::Item(
            json!({"role": "system", "content": "The shop closes at six."})
        )]
    );
    let typed = json!({"type": "conversation.item.create", "item": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "when do you close"}]}});
    s.client(&typed.to_string());
    assert_eq!(
        s.client(&json!({"type": "response.create"}).to_string()).1,
        vec![ClientAct::Respond {
            text: "when do you close".into(),
            instructions: None
        }]
    );
}

#[test]
fn manual_turns_are_asked_for_with_null_or_with_type_none() {
    for off in [Value::Null, json!({"type": "none"})] {
        let mut s = ga(json!({}));
        let update = json!({"type": "session.update",
                            "session": {"audio": {"input": {"turn_detection": off}}}});
        let (events, acts) = s.client(&update.to_string());
        assert!(acts.contains(&ClientAct::Manual(true)), "{off}");
        assert_eq!(reported_turn_detection(&s), Value::Null);
        assert_eq!(types(&events), vec!["session.updated"]);
        // The end of the client's audio is then its turn.
        let (_, acts) = s.client(&json!({"type": "input_audio_buffer.commit"}).to_string());
        assert!(acts.contains(&ClientAct::Commit));
    }
}

/// As on OpenAI: a plain session completes the caller's item when its words are final,
/// whether or not the agent answers the turn (a speech-to-speech client of this cascade waits
/// for the transcript, not for the agent).
/// A segment of several finals: the item completes with all of them, not the last fragment.
#[test]
fn a_plain_sessions_caller_item_completes_with_every_final_of_its_segment() {
    let mut s = ga(json!({}));
    let mut first = final_transcript("Hello there.");
    if let OutgoingMessage::STTResult {
        is_speech_final, ..
    } = &mut first
    {
        *is_speech_final = false;
    }
    let mut events = s.core(&first);
    events.extend(s.core(&final_transcript("How are you")));
    let kinds = types(&events);
    let at = kinds
        .iter()
        .position(|k| k == "conversation.item.input_audio_transcription.completed")
        .unwrap_or_else(|| panic!("{kinds:?}"));
    assert_eq!(parse(&events)[at]["transcript"], "Hello there. How are you");
    // The caller goes on before the turn is answered: the next item keeps to its own words.
    let mut more = final_transcript("and you");
    if let OutgoingMessage::STTResult {
        is_speech_final, ..
    } = &mut more
    {
        *is_speech_final = false;
    }
    assert!(
        !types(&s.core(&more))
            .iter()
            .any(|k| k.ends_with("transcription.completed"))
    );
    let mut started = s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 1,
        kind: "agent".into(),
        input: Some("Hello there. How are you and you".into()),
    });
    started.retain(|e| e.contains("input_audio_transcription.completed"));
    assert_eq!(parse(&started)[0]["transcript"], "and you");
}

#[test]
fn a_plain_sessions_caller_item_completes_with_its_final_transcript() {
    let mut s = ga(json!({}));
    let events = s.core(&final_transcript("yes please"));
    let kinds = types(&events);
    let at = kinds
        .iter()
        .position(|k| k == "conversation.item.input_audio_transcription.completed")
        .unwrap_or_else(|| panic!("{kinds:?}"));
    assert_eq!(parse(&events)[at]["transcript"], "yes please");
    // The agent's response for the turn completes nothing more.
    let started = s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 1,
        kind: "agent".into(),
        input: Some("yes please".into()),
    });
    assert!(
        !types(&started)
            .iter()
            .any(|k| k == "conversation.item.input_audio_transcription.completed")
    );
}

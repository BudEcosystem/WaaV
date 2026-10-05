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

// ---------------------------------------------------------------------------------------------
// Segmented speech-to-text on the Realtime surface (customer contract §3)
// ---------------------------------------------------------------------------------------------

fn ready(stt: Option<Value>) -> OutgoingMessage {
    OutgoingMessage::Ready {
        protocol_version: crate::handlers::ws::messages::PROTOCOL_VERSION.to_string(),
        stt: stt.map(Box::new),
        stream_id: "s".into(),
        livekit_room_name: None,
        livekit_url: None,
        waav_participant_identity: None,
        waav_participant_name: None,
        resolved_alias: None,
        audio_in_codec: None,
        audio_out_codec: None,
    }
}

fn segmented_ready() -> OutgoingMessage {
    ready(Some(json!({"provider": "elevenlabs", "model": "scribe_v2",
        "transcription_mode": "segmented", "interim_results": "per_segment"})))
}

fn vad(event: &str, turn: u64) -> OutgoingMessage {
    OutgoingMessage::VadEvent {
        event: event.into(),
        turn_id: turn,
        audio_ms: Some(1_200),
        sustained_ms: None,
        discarded: None,
        had_transcript: None,
        reason: None,
    }
}

fn closed(turn: u64, reason: &str) -> OutgoingMessage {
    OutgoingMessage::VadEvent {
        event: "turn_closed".into(),
        turn_id: turn,
        audio_ms: None,
        sustained_ms: None,
        discarded: None,
        had_transcript: Some(false),
        reason: Some(reason.into()),
    }
}

fn stt(text: &str, fin: bool) -> OutgoingMessage {
    OutgoingMessage::STTResult {
        transcript: text.into(),
        is_final: fin,
        is_speech_final: fin,
        confidence: 0.9,
        segment_transcript: None,
        translations: vec![],
    }
}

#[test]
fn ready_stt_becomes_one_bud_session_stt_event_on_a_gateway_endpointed_session() {
    let mut s = ga(json!({}));
    let evs = parse(&s.core(&segmented_ready()));
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0]["type"], "bud.session.stt");
    assert_eq!(evs[0]["stt"]["transcription_mode"], "segmented");
    assert!(evs[0]["event_id"].as_str().unwrap().starts_with("evt_bud_"));
    assert!(s.core(&segmented_ready()).is_empty(), "sent once");

    for other in [
        None,
        Some(json!({"provider": "deepgram", "transcription_mode": "streaming"})),
        Some(json!({"provider": "openai", "transcription_mode": "buffered"})),
    ] {
        let mut s = ga(json!({}));
        assert!(
            s.core(&ready(other)).is_empty(),
            "a streaming or buffered session gets no new event"
        );
    }
}

#[test]
fn bud_session_events_can_be_switched_off() {
    let mut s = ga(json!({}));
    s.set_bud_session_events(false);
    assert!(s.core(&segmented_ready()).is_empty());
    assert!(
        s.core(&OutgoingMessage::SttWarning {
            code: "stt_language_unset".into(),
            message: "m".into(),
            detail: None,
        })
        .is_empty()
    );
}

#[test]
fn stt_warnings_become_bud_session_warnings_except_buffered_until_commit() {
    let mut s = ga(json!({}));
    let evs = parse(&s.core(&OutgoingMessage::ConfigWarning {
        code: "stt_segmented_mode".into(),
        message: "segmented".into(),
        detail: Some(json!({"model": "scribe_v2"})),
    }));
    assert_eq!(
        evs[0],
        json!({"type": "bud.session.warning", "event_id": evs[0]["event_id"], "code": "stt_segmented_mode",
               "message": "segmented", "detail": {"model": "scribe_v2"}})
    );
    let evs = parse(&s.core(&OutgoingMessage::SttWarning {
        code: "stt_language_unset".into(),
        message: "no language".into(),
        detail: None,
    }));
    assert_eq!(evs[0]["code"], "stt_language_unset");
    assert_eq!(evs[0]["detail"], json!({}));
    assert!(
        s.core(&OutgoingMessage::ConfigWarning {
            code: "stt_buffered_until_commit".into(),
            message: "m".into(),
            detail: None,
        })
        .is_empty(),
        "logged only"
    );
    assert!(
        s.core(&OutgoingMessage::ConfigWarning {
            code: "reasoning_model_on_voice_path".into(),
            message: "m".into(),
            detail: None,
        })
        .is_empty(),
        "not a speech-to-text warning"
    );
}

/// Finding 4: the turn's text arrives per returned segment, as deltas that add up to the transcript;
/// speech start and stop are the detector's, not the transcript's.
#[test]
fn a_segmented_turn_is_timed_by_the_detector_and_streams_its_text_as_deltas() {
    let mut s = ga(json!({}));
    s.core(&segmented_ready());
    let mut all = Vec::new();
    let opened = s.core(&vad("turn_start", 1));
    assert_eq!(
        types(&opened),
        vec![
            "conversation.item.added",
            "input_audio_buffer.speech_started"
        ]
    );
    assert_eq!(parse(&opened)[1]["audio_start_ms"], 1_200);
    all.extend(opened);
    all.extend(s.core(&vad("speech_end", 1)));
    all.extend(s.core(&stt("where is", false)));
    all.extend(s.core(&stt("where is my order", false)));
    let ended = s.core(&vad("turn_end", 1));
    assert_eq!(
        types(&ended),
        vec![
            "input_audio_buffer.speech_stopped",
            "input_audio_buffer.committed"
        ]
    );
    all.extend(ended);
    all.extend(s.core(&stt("where is my order today", true)));
    all.extend(s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 0,
        kind: "agent".into(),
        input: Some("where is my order today".into()),
    }));
    let evs = parse(&all);
    let deltas: Vec<&str> = evs
        .iter()
        .filter(|e| e["type"] == "conversation.item.input_audio_transcription.delta")
        .map(|e| e["delta"].as_str().unwrap())
        .collect();
    assert_eq!(deltas, vec!["where is", " my order", " today"]);
    assert_eq!(deltas.concat(), "where is my order today");
    for kind in [
        "input_audio_buffer.speech_started",
        "input_audio_buffer.speech_stopped",
        "input_audio_buffer.committed",
        "conversation.item.input_audio_transcription.completed",
        "conversation.item.done",
    ] {
        assert_eq!(
            evs.iter().filter(|e| e["type"] == kind).count(),
            1,
            "{kind} once"
        );
    }
    let ids: std::collections::HashSet<_> = evs
        .iter()
        .filter_map(|e| e.get("item_id").and_then(Value::as_str))
        .collect();
    assert_eq!(ids.len(), 1, "one user item");
    assert_eq!(*types(&all).last().unwrap(), "response.created");
}

/// A rewritten interim (a seam repaired) cannot be a delta; the completed transcript stays whole.
#[test]
fn a_rewritten_turn_text_sends_no_wrong_delta() {
    let mut s = ga(json!({}));
    s.core(&segmented_ready());
    s.core(&vad("turn_start", 1));
    s.core(&stt("for the the", false));
    let evs = parse(&s.core(&stt("for the order", true)));
    assert!(
        evs.iter()
            .all(|e| e["type"] != "conversation.item.input_audio_transcription.delta")
    );
    let evs = parse(&s.core(&OutgoingMessage::AgentResponseStarted {
        turn_index: 0,
        kind: "agent".into(),
        input: Some("for the order".into()),
    }));
    let done = evs
        .iter()
        .find(|e| e["type"] == "conversation.item.input_audio_transcription.completed")
        .unwrap();
    assert_eq!(done["transcript"], "for the order");
}

#[test]
fn a_turn_closed_without_text_closes_the_open_item() {
    let mut s = ga(json!({}));
    s.core(&segmented_ready());
    s.core(&vad("turn_start", 1));
    let evs = parse(&s.core(&closed(1, "transcription_failed")));
    let kinds: Vec<_> = evs.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        vec![
            "input_audio_buffer.speech_stopped",
            "input_audio_buffer.committed",
            "conversation.item.input_audio_transcription.failed",
            "conversation.item.done",
        ]
    );
    assert_eq!(evs[2]["error"]["code"], "transcription_failed");
    assert_eq!(evs[2]["error"]["type"], "transcription_error");

    s.core(&vad("turn_start", 2));
    s.core(&vad("turn_end", 2));
    let evs = parse(&s.core(&closed(2, "no_speech")));
    let kinds: Vec<_> = evs.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        vec![
            "conversation.item.input_audio_transcription.completed",
            "conversation.item.done",
        ]
    );
    assert_eq!(evs[0]["transcript"], "");
    assert!(s.core(&closed(3, "no_speech")).is_empty(), "no item open");
}

#[test]
fn a_lost_segment_fails_the_open_item_else_warns() {
    let mut s = ga(json!({}));
    s.core(&segmented_ready());
    let lost = OutgoingMessage::SttWarning {
        code: "stt_segment_failed".into(),
        message: "a segment was lost".into(),
        detail: Some(json!({"class": "timeout"})),
    };
    let evs = parse(&s.core(&lost));
    assert_eq!(evs[0]["type"], "bud.session.warning");
    s.core(&vad("turn_start", 1));
    let evs = parse(&s.core(&lost));
    assert_eq!(
        evs[0]["type"],
        "conversation.item.input_audio_transcription.failed"
    );
    assert_eq!(evs[0]["error"]["code"], "stt_segment_failed");
    assert_eq!(evs[0]["error"]["message"], "a segment was lost");
}

#[test]
fn a_streaming_session_keeps_todays_events() {
    let mut s = ga(json!({}));
    s.core(&ready(Some(
        json!({"provider": "deepgram", "transcription_mode": "streaming"}),
    )));
    assert!(s.core(&vad("turn_start", 1)).is_empty());
    let evs = parse(&s.core(&stt("hello", false)));
    assert_eq!(
        evs.iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "conversation.item.added",
            "input_audio_buffer.speech_started"
        ]
    );
    let evs = parse(&s.core(&stt("hello there", true)));
    assert_eq!(evs[0]["delta"], "hello there ");
}

#[test]
fn server_side_speech_codes_are_server_errors() {
    let mut s = ga(json!({}));
    for code in [
        "stt_segmentation_unavailable",
        "stt_overloaded",
        "stt_unavailable",
        "deployment_changed",
    ] {
        let e = parse(&s.core(&OutgoingMessage::CodedError {
            message: format!("{code}: gone"),
            code: code.into(),
            recoverable: false,
            details: None,
        }));
        assert_eq!(e[0]["error"]["code"], code);
        assert_eq!(e[0]["error"]["type"], "server_error");
        assert_eq!(e[0]["error"]["message"], "gone");
    }
    let e = parse(&s.core(&OutgoingMessage::CodedError {
        message: "stt_live_unsupported: no".into(),
        code: "stt_live_unsupported".into(),
        recoverable: true,
        details: None,
    }));
    assert_eq!(e[0]["error"]["type"], "invalid_request_error");
}

#[test]
fn turning_turn_detection_on_is_refused_on_a_buffering_model() {
    let mut s = ga(json!({"turn_detection": {"type": "manual"}}));
    s.mark_started();
    s.core(&ready(Some(
        json!({"provider": "openai", "transcription_mode": "buffered"}),
    )));
    let update = json!({"type": "session.update", "event_id": "e9", "session": {"type": "realtime",
        "audio": {"input": {"turn_detection": {"type": "server_vad"}}}}})
    .to_string();
    let (events, acts) = s.client(&update);
    let evs = parse(&events);
    let refusal = evs.iter().find(|e| e["type"] == "error").unwrap();
    assert_eq!(refusal["error"]["code"], "stt_live_unsupported");
    assert_eq!(
        refusal["error"]["param"],
        "session.audio.input.turn_detection"
    );
    assert!(
        !acts.iter().any(|a| matches!(a, ClientAct::Manual(false))),
        "the session stays manual"
    );
    assert!(reported_turn_detection(&s).is_null());

    let mut s = ga(json!({"turn_detection": {"type": "manual"}}));
    s.mark_started();
    s.core(&segmented_ready());
    let (_, acts) = s.client(&update);
    assert!(acts.iter().any(|a| matches!(a, ClientAct::Manual(false))));
}

#[test]
fn the_manual_flag_reaches_the_session_before_it_starts() {
    let mut s = ga(json!({}));
    let (_, acts) = s.client(
        &json!({"type": "session.update", "session": {"type": "realtime",
            "audio": {"input": {"turn_detection": null}}}})
        .to_string(),
    );
    assert!(acts.iter().any(|a| matches!(a, ClientAct::Manual(true))));
    assert!(s.is_manual());
}

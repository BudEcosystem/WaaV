//! A `/ws` session's segmented speech-to-text: resolution at setup, and the fan-out of the engine's
//! events to the client and to metering.
//!
//! Resolution happens once, before anything is built, so a refused session costs nothing and can
//! send a corrected `config` on the same socket.

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use waav_segmented_stt::profile::{EndpointPolicy, EndpointTuning};

use super::bud_legs::LegMeter;
use super::config::STTWebSocketConfig;
use super::messages::{MessageClass, MessageRoute, OutgoingMessage, send_with_policy};
use super::stt_contract::{self, ReadyFacts, VadEvents};
use crate::core::stt::segmented::live::{
    LegSite, LiveDecision, LiveLeg, LiveRequest, LiveResolution, LiveSessionKind, SttLiveShared,
    resolve_session,
};
use crate::core::stt::speech_activity::{SegmentResultKind, SpeechActivity};
use crate::core::voice_manager::VoiceManager;

/// One session's resolution, kept from setup to `ready`.
#[derive(Debug, Clone)]
pub struct LiveSetup {
    pub live: LiveResolution,
    pub req: LiveRequest,
    /// The client asked for speech events (`features.vad_events`), or the session is an agent.
    pub speech_events: bool,
    pub barge_in_ms: u32,
    /// The client named the model itself (not only a deployment).
    pub named_model: bool,
}

/// Which kind of session asks.
pub fn session_kind(
    agent: Option<&crate::state::ResolvedVoiceAgent>,
    agent_manual: Option<bool>,
    conversation: bool,
    dag: bool,
    stt: &STTWebSocketConfig,
) -> LiveSessionKind {
    if let Some(a) = agent {
        LiveSessionKind::Agent {
            manual: agent_manual.unwrap_or(a.entry.turn_detection.kind == "manual"),
        }
    } else if conversation {
        LiveSessionKind::Conversation {
            turn_detection: stt.turn_detection.as_ref().is_none_or(|t| t.enabled),
        }
    } else if dag {
        LiveSessionKind::Dag
    } else {
        LiveSessionKind::Plain
    }
}

/// An agent's turn-detection settings as the engine's tuning: eagerness picks the silence ceiling,
/// a chosen `max_endpointing_ms` replaces it, `server_vad` is the silence policy.
pub fn agent_tuning(entry: &bud_auth::VoiceAgentEntry) -> EndpointTuning {
    let td = &entry.turn_detection;
    let eager_ceiling = match td.eagerness.as_str() {
        "high" => 1000,
        "medium" => 1250,
        _ => 1500,
    };
    EndpointTuning {
        min_end_silence_ms: Some(td.silence_ms.min(u32::MAX as u64) as u32),
        silence_ceiling_ms: Some(
            td.chosen_max_endpointing_ms()
                .map(|v| v.min(u32::MAX as u64) as u32)
                .unwrap_or(eager_ceiling),
        ),
        policy: if td.kind == "server_vad" {
            EndpointPolicy::Silence
        } else {
            EndpointPolicy::Auto
        },
        ..EndpointTuning::default()
    }
}

/// A non-agent session's tuning from its canonical features: `endpointing_ms` is the least silence
/// that ends a turn (never the cut pause), `utterance_end_ms` the ceiling.
pub fn request_tuning(stt: &STTWebSocketConfig) -> EndpointTuning {
    EndpointTuning {
        min_end_silence_ms: stt.features.endpointing_ms,
        silence_ceiling_ms: stt.features.utterance_end_ms,
        ..EndpointTuning::default()
    }
}

/// Build the request and resolve it.
pub fn resolve(
    shared: &SttLiveShared,
    stt: &STTWebSocketConfig,
    kind: LiveSessionKind,
    leg: Option<LiveLeg>,
    agent: Option<&crate::state::ResolvedVoiceAgent>,
    client_vad_events: Option<bool>,
    client_named_model: bool,
) -> LiveSetup {
    let tuning = match agent {
        Some(a) => agent_tuning(&a.entry),
        None => request_tuning(stt),
    };
    let extras: BTreeMap<String, String> = stt
        .extras
        .0
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect();
    let prompt = stt
        .extras
        .0
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let req = LiveRequest {
        provider: stt.provider.clone(),
        model: stt.model.clone(),
        language: stt.language.clone(),
        encoding: stt.encoding.clone(),
        sample_rate: stt.sample_rate,
        channels: stt.channels,
        kind,
        requested_mode: stt.transcription_mode.clone(),
        leg,
        interim_results: stt.features.interim_results,
        keyterms: stt.features.keyterms.clone().unwrap_or_default(),
        prompt,
        tuning,
        extras,
    };
    let live = resolve_session(shared, &req);
    let barge_in_ms = agent
        .map(|a| a.entry.interruption.min_speech_ms.min(u32::MAX as u64) as u32)
        .unwrap_or(0)
        .max(500);
    LiveSetup {
        speech_events: kind.is_agent() || client_vad_events == Some(true),
        barge_in_ms,
        named_model: client_named_model,
        live,
        req,
    }
}

/// The refusal message: today's exact uncoded `error` for the two codes Bud may match on, a coded
/// one for everything else.
pub fn refusal_message(
    setup: &LiveSetup,
    shared: &SttLiveShared,
    agent_name: Option<&str>,
) -> Option<OutgoingMessage> {
    let LiveDecision::Refused(r) = &setup.live.decision else {
        return None;
    };
    let vendor = setup.req.provider.as_str();
    match (r.code.as_str(), &setup.req.leg) {
        ("unsupported_deployment", Some(leg)) if leg.site == LegSite::Named => {
            Some(OutgoingMessage::Error {
                message: format!(
                    "unsupported_deployment: {}",
                    super::bud_legs::todays_unsupported_deployment_text(&leg.name, vendor)
                ),
            })
        }
        ("stt_not_streaming", Some(leg)) if leg.site == LegSite::Agent => {
            Some(OutgoingMessage::Error {
                message: format!(
                    "stt_not_streaming: {}",
                    super::bud_legs::todays_stt_not_streaming_text(
                        agent_name.unwrap_or_default(),
                        vendor
                    )
                ),
            })
        }
        _ => Some(stt_contract::refusal_error(
            &setup.live,
            r,
            shared.map,
            shared.rollout.release,
        )),
    }
}

/// `ready.stt`: on a session the switch covers, and on every session from Release 3.
pub async fn ready_stt(
    setup: &LiveSetup,
    shared: &SttLiveShared,
    vm: Option<&Arc<VoiceManager>>,
) -> Option<serde_json::Value> {
    if !setup.live.covered && shared.rollout.release < 3 {
        return None;
    }
    let engine = match vm {
        Some(vm) => vm.live_facts().await,
        None => None,
    };
    let latency = (setup.live.decision == LiveDecision::Segmented).then(|| {
        let key = setup
            .live
            .deployment
            .clone()
            .unwrap_or_else(|| setup.live.resolution.row_id.clone());
        shared.latency.estimate(&key)
    });
    Some(stt_contract::ready_stt(
        &setup.live,
        &ReadyFacts {
            engine,
            latency,
            speech_events: setup.speech_events,
            barge_in_ms: Some(setup.barge_in_ms),
            named_model: setup.named_model,
        },
        shared.map,
        shared.rollout.release,
    ))
}

/// Fan a segmented session's engine events out to the client and to metering.
pub fn wire(
    vm: &Arc<VoiceManager>,
    setup: &LiveSetup,
    message_tx: &mpsc::Sender<MessageRoute>,
    meter: Option<Arc<LegMeter>>,
    tracker: &crate::core::observability::SessionTaskTracker,
) {
    let Some(dispatch) = vm.segmented() else {
        return;
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<OutgoingMessage>();
    if let Some(meter) = meter {
        meter.set_segmented();
        // From Release 5 a unit is priced at what the vendor bills (its minimum per request, its
        // increment); before, at the seconds uploaded.
        let vendor_minimum = setup.live.resolution.release >= 5;
        dispatch.add_outcome_listener(Arc::new(move |o| {
            let charged = matches!(
                o.kind,
                SegmentResultKind::Text | SegmentResultKind::Empty | SegmentResultKind::Filtered(_)
            );
            let seconds = if vendor_minimum {
                o.billed_seconds.max(o.uploaded_seconds)
            } else {
                o.uploaded_seconds
            };
            meter.stt_uploaded(seconds, charged, o.kind.as_str());
        }));
    }
    {
        let tx = tx.clone();
        dispatch.add_outcome_listener(Arc::new(move |o| {
            if let Some(w) = stt_contract::segment_failed(o) {
                let _ = tx.send(w);
            }
        }));
    }
    {
        let tx = tx.clone();
        dispatch.add_notice_listener(Arc::new(move |n| {
            if let Some(w) = stt_contract::notice_warning(&n) {
                let _ = tx.send(w);
            }
        }));
    }
    // `stt_degraded` on sessions without an agent: an agent ends the call by its own rule.
    if !setup.req.kind.is_agent() {
        let tx = tx.clone();
        let streak = Arc::new(parking_lot::Mutex::new(stt_contract::LossStreak::default()));
        dispatch.add_speech_listener(Arc::new(move |a: SpeechActivity| {
            if let SpeechActivity::TurnClosed { had_text, gaps, .. } = a
                && let Some(w) = streak.lock().on_turn_closed(had_text, gaps)
            {
                let _ = tx.send(w);
            }
        }));
    }
    if setup.speech_events {
        let tx = tx.clone();
        let events = Arc::new(parking_lot::Mutex::new(VadEvents::new(setup.barge_in_ms)));
        let weak = Arc::downgrade(vm);
        dispatch.add_speech_listener(Arc::new(move |a: SpeechActivity| {
            let audible = weak.upgrade().is_some_and(|vm| vm.is_bot_speaking());
            for m in events.lock().on_activity(&a, audible) {
                let _ = tx.send(m);
            }
        }));
    }
    let message_tx = message_tx.clone();
    let handle = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let class = match m {
                OutgoingMessage::VadEvent { .. } => MessageClass::Transcript,
                _ => MessageClass::Critical,
            };
            send_with_policy(&message_tx, MessageRoute::Outgoing(m), class).await;
        }
    });
    tracker.track("segmented-stt-events", handle);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(json: serde_json::Value) -> bud_auth::VoiceAgentEntry {
        let mut base = serde_json::from_str::<serde_json::Value>(include_str!(
            "../../../../bud-auth/tests/fixtures/voice_agent_entry.json"
        ))
        .unwrap();
        if let (Some(b), Some(o)) = (base.as_object_mut(), json.as_object()) {
            for (k, v) in o {
                b.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn an_agents_eagerness_picks_the_ceiling_and_3000_reads_as_unset() {
        let e = entry(
            serde_json::json!({"turn_detection": {"type": "semantic", "eagerness": "high", "silence_ms": 500, "max_endpointing_ms": 3000}}),
        );
        let t = agent_tuning(&e);
        assert_eq!(t.silence_ceiling_ms, Some(1000));
        assert_eq!(t.min_end_silence_ms, Some(500));
        assert_eq!(t.policy, EndpointPolicy::Auto);
        let e = entry(
            serde_json::json!({"turn_detection": {"type": "server_vad", "eagerness": "auto", "silence_ms": 700, "max_endpointing_ms": 2200}}),
        );
        let t = agent_tuning(&e);
        assert_eq!(t.silence_ceiling_ms, Some(2200));
        assert_eq!(t.policy, EndpointPolicy::Silence);
    }

    #[test]
    fn endpointing_ms_never_changes_the_cut_pause() {
        let mut stt = super::super::config::default_agent_stt_config();
        stt.features.endpointing_ms = Some(10);
        stt.features.utterance_end_ms = Some(1200);
        let t = request_tuning(&stt);
        assert_eq!(t.cut_pause_ms, 224);
        assert_eq!(t.min_end_silence_ms, Some(10));
        assert_eq!(t.silence_ceiling_ms, Some(1200));
    }
}

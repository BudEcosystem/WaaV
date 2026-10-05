//! `/ws` voice-agent sessions (spec 025 §5.6, FR-EVT-2).
//!
//! `{"type":"config","agent":{"id":"support"}}` resolves the agent and its two legs (see
//! [`super::bud_legs::prepare_agent`]); this module wires the session's VoiceManager to an
//! [`AgentEngine`]: STT results feed the turn controller, its events drive agent turns, and the
//! engine's signals become additive `/ws` messages (`agent_response_started`, `assistant_transcript`,
//! `agent_tool`, `agent_output`, `agent_response_done`, `agent_truncated`, `agent_error`). Old
//! clients ignore message types they do not know.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, mpsc};
use tracing::info;
#[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
use tracing::warn;

use super::config::AgentWebSocketConfig;
use super::messages::{MessageClass, MessageRoute, OutgoingMessage, send_with_policy};
use super::state::ConnectionState;
use crate::core::agent::{
    AgentBrain, AgentEngine, AgentSessionConfig, AgentSignal, SpeechOut, TurnBackend, TurnKind,
    is_ignored_phrase,
};
use crate::core::stt::STTResult;
use crate::core::voice_manager::VoiceManager;
use crate::state::{AppState, ResolvedVoiceAgent};

/// The conversation every turn of a session continues in budprompt.
///
/// Named by WaaV on the first turn — budprompt creates a conversation the moment a request names one
/// — so the id is known before any turn and a dropped first response cannot split the thread.
pub fn conversation_id_for(stream_id: &str) -> String {
    let safe: String = stream_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(200)
        .collect();
    format!("conv_voice_{safe}")
}

fn kind_str(kind: TurnKind) -> &'static str {
    match kind {
        TurnKind::Agent => "agent",
        TurnKind::Greeting => "greeting",
        TurnKind::Notice => "notice",
    }
}

/// The `/ws` message for an engine signal (`None` for the session-level `Idle`).
pub fn agent_signal_message(signal: &AgentSignal) -> Option<OutgoingMessage> {
    Some(match signal.clone() {
        AgentSignal::ResponseStarted { turn, kind, input } => {
            OutgoingMessage::AgentResponseStarted {
                turn_index: turn,
                kind: kind_str(kind).into(),
                input,
            }
        }
        AgentSignal::ResponseCreated { turn, response_id } => {
            OutgoingMessage::AgentResponseCreated {
                turn_index: turn,
                response_id,
            }
        }
        AgentSignal::Transcript { turn, delta } => OutgoingMessage::AssistantTranscript {
            turn_index: turn,
            delta,
        },
        AgentSignal::Tool {
            turn,
            item_id,
            name,
            status,
        } => OutgoingMessage::AgentTool {
            turn_index: turn,
            item_id,
            name,
            status: status.into(),
        },
        AgentSignal::Output {
            turn,
            response_id,
            output,
        } => OutgoingMessage::AgentOutput {
            turn_index: turn,
            response_id,
            output,
        },
        AgentSignal::ResponseDone {
            turn,
            kind,
            response_id,
            status,
            usage,
            transcript,
        } => OutgoingMessage::AgentResponseDone {
            turn_index: turn,
            kind: kind_str(kind).into(),
            response_id,
            status: status.as_str().into(),
            usage,
            transcript,
        },
        AgentSignal::Truncated {
            turn,
            response_id,
            spoken,
            audio_end_ms,
        } => OutgoingMessage::AgentTruncated {
            turn_index: turn,
            response_id,
            spoken,
            audio_end_ms,
        },
        AgentSignal::Error { code, message } => OutgoingMessage::AgentError {
            code: code.into(),
            message,
        },
        AgentSignal::Idle => return None,
    })
}

/// Close codes the agent ends a session with.
const CLOSE_NORMAL: u16 = 1000;

/// Wire the session's VoiceManager to a voice agent and start listening.
pub(super) async fn initialize_agent_loop(
    agent: ResolvedVoiceAgent,
    cfg: &AgentWebSocketConfig,
    stream_id: &str,
    vm: &Arc<VoiceManager>,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> Result<(), String> {
    let base_url = super::bud_legs::llm_base_url().ok_or_else(|| {
        "voice agents are not available on this gateway: WAAV_LLM_BASE_URL (the Bud gateway) is \
         not configured"
            .to_string()
    })?;
    let credential = state
        .read()
        .await
        .credential
        .clone()
        .ok_or_else(|| "a voice agent needs your Bud API key or token".to_string())?;

    let entry = Arc::clone(&agent.entry);
    let (signal_tx, mut signal_rx) = mpsc::unbounded_channel::<AgentSignal>();
    let engine = AgentEngine::new(
        AgentSessionConfig {
            session_id: stream_id.to_string(),
            prompt_name: agent.prompt_name.clone(),
            version: agent.version,
            entry: Arc::clone(&entry),
            conversation_id: conversation_id_for(stream_id),
            variables: cfg.variables.clone(),
            text_only: cfg.text_only.unwrap_or(false),
        },
        Arc::new(AgentBrain::new(base_url)) as Arc<dyn TurnBackend>,
        Arc::clone(vm) as Arc<dyn SpeechOut>,
        credential,
        signal_tx,
    );
    let missing = engine.missing_variables();
    if !missing.is_empty() {
        // The session stays open: variables can still come with the first turn's `agent_input`
        // or a GA `session.update` (D-14). Every turn until then is refused with this code.
        let _ = send_with_policy(
            message_tx,
            MessageRoute::Outgoing(OutgoingMessage::AgentError {
                code: "missing_variables".into(),
                message: format!(
                    "This agent needs the session's variables: {}.",
                    missing.join(", ")
                ),
            }),
            MessageClass::Critical,
        )
        .await;
    }

    // Engine signals -> /ws messages.
    {
        let tx = message_tx.clone();
        let tracker = state.read().await.task_tracker.clone();
        let handle = tokio::spawn(async move {
            while let Some(signal) = signal_rx.recv().await {
                if matches!(signal, AgentSignal::Idle) {
                    let _ = send_with_policy(
                        &tx,
                        MessageRoute::Outgoing(OutgoingMessage::AgentError {
                            code: "idle_timeout".into(),
                            message: "The session ended after a long silence.".into(),
                        }),
                        MessageClass::Critical,
                    )
                    .await;
                    let _ = send_with_policy(
                        &tx,
                        MessageRoute::CloseWith {
                            code: CLOSE_NORMAL,
                            reason: "idle_timeout".into(),
                        },
                        MessageClass::Critical,
                    )
                    .await;
                    return;
                }
                let class = match &signal {
                    AgentSignal::Transcript { .. } => MessageClass::Transcript,
                    _ => MessageClass::Critical,
                };
                if let Some(msg) = agent_signal_message(&signal) {
                    send_with_policy(&tx, MessageRoute::Outgoing(msg), class).await;
                }
            }
        });
        tracker.track("agent-signals", handle);
    }

    // The session's length is the agent's limit, capped by the gateway's.
    {
        let cap = app_state.realtime.timings.max_session;
        let limit = Duration::from_secs(entry.limits.max_session_seconds.max(60)).min(cap);
        let tx = message_tx.clone();
        let weak = Arc::downgrade(&engine);
        let tracker = state.read().await.task_tracker.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(limit).await;
            if let Some(engine) = weak.upgrade() {
                engine.shutdown().await;
            }
            let _ = send_with_policy(
                &tx,
                MessageRoute::Outgoing(OutgoingMessage::AgentError {
                    code: "session_limit".into(),
                    message: format!("The session reached its {} s limit.", limit.as_secs()),
                }),
                MessageClass::Critical,
            )
            .await;
            let _ = send_with_policy(
                &tx,
                MessageRoute::CloseWith {
                    code: CLOSE_NORMAL,
                    reason: "session_limit".into(),
                },
                MessageClass::Critical,
            )
            .await;
        });
        tracker.track("agent-session-limit", handle);
    }

    // Turn-taking: the same controller the conversation loop uses, with the agent's settings.
    let start_strategy = start_strategy(&entry);
    let vm_probe = Arc::clone(vm);
    let engine_probe = Arc::clone(&engine);
    let controller = Arc::new(
        crate::core::turn::TurnController::new(
            vec![start_strategy],
            vec![Box::new(
                crate::core::turn::strategies::LegacySpeechFinalStop,
            )],
            vec![],
        )
        .with_bot_speaking_probe(move || {
            vm_probe.is_bot_speaking() || engine_probe.has_active_turn()
        }),
    );

    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    {
        let eng = Arc::clone(&engine);
        let ctrl = Arc::clone(&controller);
        if let Err(e) = vm
            .on_smart_turn(move |result| {
                let eng = Arc::clone(&eng);
                let ctrl = Arc::clone(&ctrl);
                Box::pin(async move {
                    let events = ctrl.feed(&crate::core::turn::ControllerSignal::SmartTurn {
                        is_complete: result.is_turn_complete,
                    });
                    if !events.is_empty() && !eng.is_manual() {
                        eng.handle_turn_events(&events).await;
                    }
                })
            })
            .await
        {
            warn!("failed to register the agent's smart-turn callback: {e}");
        }
    }

    let tx = message_tx.clone();
    let eng = Arc::clone(&engine);
    let ctrl = Arc::clone(&controller);
    let ignore = entry.interruption.ignore_phrases.clone();
    vm.on_stt_result(move |stt: STTResult| {
        let tx = tx.clone();
        let eng = Arc::clone(&eng);
        let ctrl = Arc::clone(&ctrl);
        let ignore = ignore.clone();
        Box::pin(async move {
            // Interruptions off: while the agent answers, the caller's speech is not input — not
            // shown, not a turn — as during a greeting that cannot be talked over.
            if !eng.accepts_speech() {
                eng.poke_idle();
                return;
            }
            send_with_policy(
                &tx,
                MessageRoute::Outgoing(OutgoingMessage::STTResult {
                    transcript: stt.transcript.clone(),
                    is_final: stt.is_final,
                    is_speech_final: stt.is_speech_final,
                    confidence: stt.confidence,
                    segment_transcript: stt.segment_transcript.clone(),
                    translations: stt.translations.clone(),
                }),
                MessageClass::Transcript,
            )
            .await;
            eng.poke_idle();
            // A backchannel ("uh-huh") while the agent speaks is not an interruption.
            let busy = eng.has_active_turn();
            if busy && is_ignored_phrase(stt.turn_transcript(), &ignore) {
                return;
            }
            let signal = if stt.is_final || stt.is_speech_final {
                crate::core::turn::ControllerSignal::SttFinal {
                    text: stt.turn_transcript().to_string(),
                    is_speech_final: stt.is_speech_final,
                    is_finalized: stt.is_finalized,
                }
            } else {
                crate::core::turn::ControllerSignal::SttInterim {
                    text: stt.transcript.clone(),
                    confidence: stt.confidence,
                }
            };
            let events = ctrl.feed(&signal);
            if events.is_empty() {
                return;
            }
            if eng.is_manual() {
                // Manual turns: speech still interrupts, but only the client's commit starts a turn.
                for event in &events {
                    match event {
                        crate::core::turn::TurnEvent::Stopped { transcript, .. } => {
                            eng.append_input(transcript)
                        }
                        other => eng.handle_turn_events(std::slice::from_ref(other)).await,
                    }
                }
            } else {
                eng.handle_turn_events(&events).await;
            }
        })
    })
    .await
    .map_err(|e| format!("failed to register the agent's STT callback: {e}"))?;

    state.write().await.agent = Some(Arc::clone(&engine));
    info!(
        agent = %agent.prompt_name,
        version = agent.version,
        stt = %entry.stt.endpoint_id,
        tts = %entry.tts.endpoint_id,
        "voice agent session started"
    );
    Ok(())
}

/// What starts a caller's turn, and so interrupts the agent while it speaks. The words needed
/// count only while the agent speaks; 0 and 1 both mean any speech, because the min-words gate
/// starts at 2.
fn start_strategy(
    entry: &bud_auth::voice_agent::VoiceAgentEntry,
) -> Box<dyn crate::core::turn::UserTurnStartStrategy> {
    match entry.interruption.min_words {
        n if n >= 2 => Box::new(crate::core::turn::strategies::MinWordsStart::new(n)),
        _ => Box::new(crate::core::turn::strategies::AnySpeechStart),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::agent::TurnStatus;
    use crate::core::turn::{ControllerSignal, StartVerdict, TurnCtx};
    use bud_auth::voice_agent::VoiceAgentEntry;
    use serde_json::json;

    fn interruption(min_words: u64) -> VoiceAgentEntry {
        bud_auth::voice_agent::parse_voice_agent_blob(
            &json!({
                "prompt_id": "c0de", "version": 1,
                "stt": {"endpoint_id": "stt-1"}, "tts": {"endpoint_id": "tts-1"},
                "interruption": {"enabled": true, "min_words": min_words},
            })
            .to_string(),
        )
        .unwrap()
    }

    fn while_the_agent_speaks(entry: &VoiceAgentEntry, said: &str) -> StartVerdict {
        let ctx = TurnCtx {
            bot_speaking: true,
            turn_active: false,
            stt_ttfs_p99_ms: None,
        };
        let sig = ControllerSignal::SttInterim {
            text: said.into(),
            confidence: 0.9,
        };
        start_strategy(entry).on_signal(&sig, &ctx)
    }

    /// "Words before an interruption counts: 1" means one word. It was handed to the min-words
    /// gate, which starts at 2, so a lone "Stop." never cut the agent off (live, 2026-10-03).
    #[test]
    fn one_word_interrupts_an_agent_that_says_one_does() {
        let interrupts = StartVerdict::Start { interrupt: true };
        assert_eq!(while_the_agent_speaks(&interruption(1), "Stop"), interrupts);
        assert_eq!(while_the_agent_speaks(&interruption(0), "Stop"), interrupts);
        assert_eq!(
            while_the_agent_speaks(&interruption(2), "Stop"),
            StartVerdict::Ignore
        );
        assert_eq!(
            while_the_agent_speaks(&interruption(2), "Stop please"),
            interrupts
        );
        assert_eq!(
            while_the_agent_speaks(&interruption(3), "Stop please"),
            StartVerdict::Ignore
        );
    }

    #[test]
    fn the_conversation_id_is_stable_and_safe() {
        assert_eq!(conversation_id_for("abc-123"), "conv_voice_abc-123");
        assert_eq!(conversation_id_for("a b\u{0}c"), "conv_voice_abc");
        assert!(conversation_id_for(&"x".repeat(500)).len() <= 211);
    }

    #[test]
    fn every_signal_becomes_an_additive_message() {
        let cases = vec![
            (
                AgentSignal::ResponseStarted {
                    turn: 1,
                    kind: TurnKind::Agent,
                    input: Some("hi".into()),
                },
                json!({"type": "agent_response_started", "turn_index": 1, "kind": "agent", "input": "hi"}),
            ),
            (
                AgentSignal::ResponseCreated {
                    turn: 1,
                    response_id: "resp_1".into(),
                },
                json!({"type": "agent_response_created", "turn_index": 1, "response_id": "resp_1"}),
            ),
            (
                AgentSignal::Transcript {
                    turn: 1,
                    delta: "Hello.".into(),
                },
                json!({"type": "assistant_transcript", "turn_index": 1, "delta": "Hello."}),
            ),
            (
                AgentSignal::Tool {
                    turn: 1,
                    item_id: "mcp_1".into(),
                    name: "lookup".into(),
                    status: "in_progress",
                },
                json!({"type": "agent_tool", "turn_index": 1, "item_id": "mcp_1", "name": "lookup", "status": "in_progress"}),
            ),
            (
                AgentSignal::ResponseDone {
                    turn: 1,
                    kind: TurnKind::Agent,
                    response_id: Some("resp_1".into()),
                    status: TurnStatus::Cancelled,
                    usage: None,
                    transcript: "Hel".into(),
                },
                json!({"type": "agent_response_done", "turn_index": 1, "kind": "agent", "response_id": "resp_1", "status": "cancelled", "transcript": "Hel"}),
            ),
            (
                AgentSignal::Truncated {
                    turn: 1,
                    response_id: Some("resp_1".into()),
                    spoken: "Hel".into(),
                    audio_end_ms: 300,
                },
                json!({"type": "agent_truncated", "turn_index": 1, "response_id": "resp_1", "spoken": "Hel", "audio_end_ms": 300}),
            ),
            (
                AgentSignal::Error {
                    code: "rate_limit_exceeded",
                    message: "slow down".into(),
                },
                json!({"type": "agent_error", "code": "rate_limit_exceeded", "message": "slow down"}),
            ),
        ];
        for (signal, want) in cases {
            let msg = agent_signal_message(&signal).expect("a message");
            assert_eq!(serde_json::to_value(&msg).unwrap(), want, "{signal:?}");
        }
        assert!(agent_signal_message(&AgentSignal::Idle).is_none());
    }
}

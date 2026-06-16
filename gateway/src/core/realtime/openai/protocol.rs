//! OpenAI Realtime [`RealtimeProtocol`] — the stateless per-provider mapper that
//! the generic [`RealtimeSession`](crate::core::realtime::scaffold::RealtimeSession)
//! driver runs. Everything OpenAI *varies* (the GA session config, the wire
//! event mapping, audio base64 encoding) lives here; everything realtime
//! providers *share* (reconnect, barge-in, truncate, preroll, replay, callback
//! dispatch) lives in the driver.
//!
//! This is the verbatim extraction of the bespoke `OpenAIRealtime` logic
//! (`build_session_config`, `handle_server_event`, `create_response_with`,
//! `send_text`, `truncate`, `submit_function_result`, `replay_conversation`) —
//! the wire is byte-identical to the live-validated GA client. Pure + sync, so
//! every mapping is trivially unit-testable (serialize a config, parse a
//! captured server event).

use base64::prelude::*;
use bytes::Bytes;

use super::config::{
    OPENAI_REALTIME_URL, OpenAIRealtimeAudioFormat, OpenAIRealtimeModel, OpenAIRealtimeVoice,
};
use super::messages::{
    AudioConfig, AudioFormat, AudioInput, AudioOutput, ClientEvent, ContentPart, ConversationItem,
    InputAudioTranscription, MaxTokens, NoiseReduction, ResponseConfig, ServerEvent, SessionConfig,
    ToolDef, TurnDetection,
};
use crate::core::realtime::base::{
    FunctionCallRequest, RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult,
    ReplayConversationItem, SpeechEvent, TranscriptRole,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// The OpenAI Realtime protocol. Stateless: model/voice/audio_format are the only
/// provider-specific config the wire mappings need (everything else is read from
/// the [`RealtimeConfig`] passed back into `build_session_config`).
pub struct OpenAiProtocol {
    model: OpenAIRealtimeModel,
    voice: OpenAIRealtimeVoice,
    audio_format: OpenAIRealtimeAudioFormat,
    /// review wf_d43814c3 #7: OpenAI's `session.turn_detection` defaults to server
    /// VAD ON, and WaaV OMITS the field when `config.turn_detection` is None
    /// (`skip_serializing_if`), so the server STILL runs VAD and produces turn
    /// frames. Server VAD is therefore on UNLESS the config explicitly selects the
    /// `None` (manual) variant. Computed once at `from_config`.
    emits_user_turn_frames: bool,
}

impl OpenAiProtocol {
    /// The configured model (for the newtype's inherent `model()` accessor).
    pub fn model(&self) -> OpenAIRealtimeModel {
        self.model
    }

    /// The configured voice (for the newtype's inherent `voice()` accessor).
    pub fn voice(&self) -> OpenAIRealtimeVoice {
        self.voice
    }

    /// The configured audio format (for the newtype's inherent `audio_format()`).
    pub fn audio_format(&self) -> OpenAIRealtimeAudioFormat {
        self.audio_format
    }

    /// Build the initial session configuration (GA nested shape). EXTRACTED
    /// verbatim from `OpenAIRealtime::build_session_config` — reads model-derived
    /// voice/audio_format from `self` and everything else from `cfg`. Kept as an
    /// inherent method so the newtype can expose `build_session_config()` for the
    /// existing wire tests.
    pub fn session_config(&self, cfg: &RealtimeConfig) -> SessionConfig {
        // GA audio format object (Beta sent a bare `"pcm16"` string).
        let ga_format = || match self.audio_format {
            OpenAIRealtimeAudioFormat::Pcm16 => AudioFormat {
                format_type: "audio/pcm".to_string(),
                rate: Some(24000),
            },
            OpenAIRealtimeAudioFormat::G711Ulaw => AudioFormat {
                format_type: "audio/pcmu".to_string(),
                rate: None,
            },
            OpenAIRealtimeAudioFormat::G711Alaw => AudioFormat {
                format_type: "audio/pcma".to_string(),
                rate: None,
            },
        };

        let turn_detection = cfg.turn_detection.as_ref().map(|td| match td {
            crate::core::realtime::base::TurnDetectionConfig::ServerVad {
                threshold,
                prefix_padding_ms,
                silence_duration_ms,
                create_response,
                interrupt_response,
            } => TurnDetection::ServerVad {
                threshold: *threshold,
                prefix_padding_ms: *prefix_padding_ms,
                silence_duration_ms: *silence_duration_ms,
                create_response: *create_response,
                interrupt_response: *interrupt_response,
            },
            crate::core::realtime::base::TurnDetectionConfig::SemanticVad {
                eagerness,
                create_response,
                interrupt_response,
            } => TurnDetection::SemanticVad {
                eagerness: eagerness.clone(),
                create_response: *create_response,
                interrupt_response: *interrupt_response,
            },
            crate::core::realtime::base::TurnDetectionConfig::None => TurnDetection::None {},
        });

        // GA nests the Beta-era flat audio fields under audio.input / audio.output.
        let audio = AudioConfig {
            input: Some(AudioInput {
                format: Some(ga_format()),
                transcription: cfg.input_audio_transcription.as_ref().map(|t| {
                    InputAudioTranscription {
                        model: t.model.clone(),
                    }
                }),
                noise_reduction: NoiseReduction::from_opt(
                    cfg.input_audio_noise_reduction.as_deref(),
                ),
                turn_detection,
            }),
            output: Some(AudioOutput {
                format: Some(ga_format()),
                voice: Some(self.voice.as_str().to_string()),
                speed: None,
            }),
        };

        // NOTE: GA `gpt-realtime` exposes no session-level `temperature` or
        // `reasoning` (confirmed against the live `session.created` schema), so
        // `cfg.{temperature,reasoning_effort}` are intentionally NOT mapped here —
        // sending either 400s the session. The cascade dial still applies to the
        // LLM path; S2S reasoning awaits a reasoning-capable realtime model
        // exposing the field.
        SessionConfig {
            session_type: "realtime".to_string(),
            output_modalities: Some(vec!["audio".to_string()]),
            instructions: cfg.instructions.clone(),
            audio: Some(audio),
            tools: cfg.tools.as_ref().map(|tools| {
                tools
                    .iter()
                    .map(|t| ToolDef {
                        tool_type: t.tool_type.clone(),
                        name: t.function.name.clone(),
                        description: t.function.description.clone(),
                        parameters: t.function.parameters.clone(),
                    })
                    .collect()
            }),
            tool_choice: cfg.tool_choice.clone(),
            max_output_tokens: cfg.max_response_output_tokens.map(|t| {
                if t < 0 {
                    MaxTokens::Infinite("inf".to_string())
                } else {
                    MaxTokens::Number(t)
                }
            }),
        }
    }

    /// Map a replay-log item to the wire conversation item. EXTRACTED verbatim
    /// from `OpenAIRealtime::replay_item_to_conversation_item`: `input_text` for
    /// user, `text` for assistant; a completed message (never a response).
    fn replay_item_to_conversation_item(item: &ReplayConversationItem) -> ConversationItem {
        let (role, content_type) = match item.role {
            TranscriptRole::User => ("user", "input_text"),
            TranscriptRole::Assistant => ("assistant", "text"),
        };
        ConversationItem {
            id: None,
            item_type: "message".to_string(),
            status: Some("completed".to_string()),
            role: Some(role.to_string()),
            content: Some(vec![ContentPart {
                content_type: content_type.to_string(),
                text: Some(item.text.clone()),
                audio: None,
                transcript: None,
            }]),
            call_id: None,
            name: None,
            arguments: None,
            output: None,
        }
    }
}

impl RealtimeProtocol for OpenAiProtocol {
    type Wire = ClientEvent;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Validate API key (EXTRACTED from `OpenAIRealtime::new`).
        if cfg.api_key.is_empty() {
            return Err(RealtimeError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }

        // Parse model.
        let model = if cfg.model.is_empty() {
            OpenAIRealtimeModel::default()
        } else {
            OpenAIRealtimeModel::from_str_or_default(&cfg.model)
        };

        // Parse voice.
        let voice = if let Some(ref v) = cfg.voice {
            OpenAIRealtimeVoice::from_str_or_default(v)
        } else {
            OpenAIRealtimeVoice::default()
        };

        // Parse audio format.
        let audio_format = if let Some(ref f) = cfg.input_audio_format {
            OpenAIRealtimeAudioFormat::from_str_or_default(f)
        } else {
            OpenAIRealtimeAudioFormat::default()
        };

        // review wf_d43814c3 #7: server VAD is on UNLESS the config explicitly
        // selects the manual `None` variant.
        let emits_user_turn_frames = !matches!(
            cfg.turn_detection,
            Some(crate::core::realtime::base::TurnDetectionConfig::None)
        );

        Ok(Self {
            model,
            voice,
            audio_format,
            emits_user_turn_frames,
        })
    }

    fn provider_id(&self) -> &'static str {
        "openai"
    }

    fn caps(&self) -> ProtocolCaps {
        ProtocolCaps {
            emits_user_turn_frames: self.emits_user_turn_frames,
            output_bytes_per_ms: self.audio_format.bytes_per_ms(),
            output_sample_rate: self.audio_format.sample_rate(),
            supports_truncate: true,
            supports_input_buffer: true,
        }
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // EXACTLY the headers in `OpenAIRealtime::connect`: bearer auth + the
        // realtime subprotocol. GA Realtime API: the `OpenAI-Beta: realtime=v1`
        // header is RETIRED (sending it now 400s the session) — bearer alone.
        // The standard WS-upgrade headers are generated by `into_client_request()`
        // in the transport factory.
        Ok(ConnectSpec::WebSocket {
            url: format!("{}?model={}", OPENAI_REALTIME_URL, self.model.as_str()),
            headers: vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", cfg.api_key),
                ),
                (
                    "Sec-WebSocket-Protocol".to_string(),
                    "realtime".to_string(),
                ),
            ],
        })
    }

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        _resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        vec![ClientEvent::SessionUpdate {
            session: self.session_config(cfg),
        }]
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        let text = match raw {
            Inbound::Text(t) => t,
            // OpenAI sends JSON only; a binary frame is never expected.
            Inbound::Binary(_) => return vec![S2sEvent::Ignore],
        };
        let event = match serde_json::from_str::<ServerEvent>(text) {
            Ok(e) => e,
            // The old client warn-logged + dropped unparseable frames.
            Err(_) => return vec![S2sEvent::Ignore],
        };
        match event {
            ServerEvent::SessionCreated { session } => vec![S2sEvent::SessionReady {
                session_id: Some(session.id),
            }],
            ServerEvent::SessionUpdated { .. } => vec![S2sEvent::Ignore],
            ServerEvent::Error { error } => vec![S2sEvent::Error(RealtimeError::ProviderError(
                format!("{}: {}", error.error_type, error.message),
            ))],
            ServerEvent::SpeechStarted {
                audio_start_ms,
                item_id,
            } => {
                // Byte-identical to the old client: server `speech_started` fires
                // ONLY the speech-event callback and sends NOTHING on the wire.
                // Barge-in (cancel + truncate) is driven by the handler's explicit
                // CancelResponse → run_barge_in_sequence, NOT by this VAD edge.
                // (Emitting InterruptedByServer here sent spurious response.cancel
                // when no response was in flight — review-caught regression.)
                vec![S2sEvent::Speech(SpeechEvent::Started {
                    audio_start_ms,
                    item_id: Some(item_id),
                })]
            }
            ServerEvent::SpeechStopped {
                audio_end_ms,
                item_id,
            } => vec![S2sEvent::Speech(SpeechEvent::Stopped {
                audio_end_ms,
                item_id: Some(item_id),
            })],
            ServerEvent::TranscriptionCompleted {
                item_id,
                transcript,
                ..
            } => vec![S2sEvent::Transcript {
                role: TranscriptRole::User,
                text: transcript,
                is_final: true,
                item_id: Some(item_id),
            }],
            ServerEvent::AudioTranscriptDelta { delta, item_id, .. } => {
                vec![S2sEvent::Transcript {
                    role: TranscriptRole::Assistant,
                    text: delta,
                    is_final: false,
                    item_id: Some(item_id),
                }]
            }
            ServerEvent::AudioTranscriptDone {
                transcript,
                item_id,
                ..
            } => vec![S2sEvent::Transcript {
                role: TranscriptRole::Assistant,
                text: transcript,
                is_final: true,
                item_id: Some(item_id),
            }],
            ServerEvent::TextDelta { delta, .. } => vec![S2sEvent::Transcript {
                role: TranscriptRole::Assistant,
                text: delta,
                is_final: false,
                item_id: None,
            }],
            ServerEvent::TextDone { text, item_id, .. } => vec![S2sEvent::Transcript {
                role: TranscriptRole::Assistant,
                text,
                is_final: true,
                item_id: Some(item_id),
            }],
            ServerEvent::AudioDelta {
                delta,
                item_id,
                response_id,
                ..
            } => match BASE64_STANDARD.decode(&delta) {
                Ok(audio_bytes) => vec![S2sEvent::Audio {
                    data: Bytes::from(audio_bytes),
                    item_id: Some(item_id),
                    response_id: Some(response_id),
                }],
                Err(_) => vec![S2sEvent::Ignore],
            },
            ServerEvent::OutputItemAdded { item, .. } => {
                if item.item_type == "function_call"
                    && let (Some(call_id), Some(name)) = (&item.call_id, &item.name)
                {
                    vec![S2sEvent::TrackPendingCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                    }]
                } else {
                    vec![S2sEvent::Ignore]
                }
            }
            ServerEvent::FunctionCallArgumentsDone {
                call_id,
                arguments,
                item_id,
                ..
            } => vec![S2sEvent::FunctionCall(FunctionCallRequest {
                call_id,
                // The scaffold resolves the name from its pending_calls map.
                name: String::new(),
                arguments,
                item_id: Some(item_id),
            })],
            ServerEvent::ResponseDone { response } => vec![S2sEvent::ResponseDone {
                response_id: response.id,
            }],
            _ => vec![S2sEvent::Ignore],
        }
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        ClientEvent::audio_append(pcm)
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        // EXTRACTED verbatim from `OpenAIRealtime::send_text`.
        vec![ClientEvent::ConversationItemCreate {
            item: ConversationItem {
                id: None,
                item_type: "message".to_string(),
                status: None,
                role: Some("user".to_string()),
                content: Some(vec![ContentPart {
                    content_type: "input_text".to_string(),
                    text: Some(text.to_string()),
                    audio: None,
                    transcript: None,
                }]),
                call_id: None,
                name: None,
                arguments: None,
                output: None,
            },
            previous_item_id: None,
        }]
    }

    fn create_response(&self, overrides: Option<&RealtimeResponseOverride>) -> Vec<Self::Wire> {
        // EXTRACTED verbatim from `OpenAIRealtime::create_response_with` (and the
        // None ⇒ default path from `create_response`). Map the provider-agnostic
        // override onto the GA `response.create` `response` object: a per-response
        // voice nests under audio.output; `out_of_band` ⇒ conversation:"none".
        let response = match overrides {
            Some(overrides) => {
                let audio = overrides.voice.as_ref().map(|v| AudioConfig {
                    input: None,
                    output: Some(AudioOutput {
                        format: None,
                        voice: Some(v.clone()),
                        speed: None,
                    }),
                });
                ResponseConfig {
                    output_modalities: overrides.modalities.clone(),
                    instructions: overrides.instructions.clone(),
                    audio,
                    tools: None,
                    tool_choice: None,
                    max_output_tokens: overrides.max_output_tokens.map(|t| {
                        if t < 0 {
                            MaxTokens::Infinite("inf".to_string())
                        } else {
                            MaxTokens::Number(t)
                        }
                    }),
                    conversation: overrides.out_of_band.then(|| "none".to_string()),
                    metadata: overrides.metadata.clone(),
                    input: None,
                }
            }
            None => ResponseConfig::default(),
        };
        vec![ClientEvent::ResponseCreate {
            response: Some(response),
        }]
    }

    fn commit_turn(&self) -> Vec<Self::Wire> {
        vec![ClientEvent::InputAudioBufferCommit]
    }

    fn cancel_response(&self) -> Vec<Self::Wire> {
        vec![ClientEvent::ResponseCancel]
    }

    fn clear_input_buffer(&self) -> Vec<Self::Wire> {
        vec![ClientEvent::InputAudioBufferClear]
    }

    fn truncate(&self, item_id: &str, audio_end_ms: u64) -> Vec<Self::Wire> {
        // EXTRACTED verbatim from `OpenAIRealtime::truncate_response` field names.
        vec![ClientEvent::ConversationItemTruncate {
            item_id: item_id.to_string(),
            content_index: 0,
            audio_end_ms: audio_end_ms as u32,
        }]
    }

    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<Self::Wire> {
        // EXTRACTED verbatim from `OpenAIRealtime::submit_function_result`.
        vec![ClientEvent::ConversationItemCreate {
            item: ConversationItem {
                id: None,
                item_type: "function_call_output".to_string(),
                status: None,
                role: None,
                content: None,
                call_id: Some(call_id.to_string()),
                name: None,
                arguments: None,
                output: Some(result.to_string()),
            },
            previous_item_id: None,
        }]
    }

    fn replay_item(&self, item: &ReplayConversationItem) -> Vec<Self::Wire> {
        // EXTRACTED verbatim from `OpenAIRealtime::replay_conversation`: a
        // ConversationItemCreate WITHOUT a response (no duplicate inference).
        vec![ClientEvent::ConversationItemCreate {
            item: Self::replay_item_to_conversation_item(item),
            previous_item_id: None,
        }]
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        Ok(OutFrame::Text(serde_json::to_string(msg).map_err(|e| {
            RealtimeError::SerializationError(e.to_string())
        })?))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn proto(cfg: &RealtimeConfig) -> OpenAiProtocol {
        OpenAiProtocol::from_config(cfg).unwrap()
    }

    fn base_cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "openai".into(),
            api_key: "k".into(),
            model: "gpt-4o-realtime-preview".into(),
            voice: Some("alloy".into()),
            ..Default::default()
        }
    }

    /// THE GOLDEN WIRE ORACLE: every outbound message the protocol serializes
    /// must be byte-equivalent to the GA wire the live-validated bespoke client
    /// produced. The expected substrings are ported verbatim from the existing
    /// `messages.rs` tests (test_session_update_serialization,
    /// test_response_create_serialization, test_client_event_serialization,
    /// test_audio_append) plus the GA shapes the client asserted.
    #[test]
    fn golden_wire_equivalence() {
        let p = proto(&base_cfg());

        // ── session.update (build_session_config) ──
        // Port of test_session_update_serialization + the GA client assertions.
        let session_wires = p.build_session_config(&base_cfg(), None);
        assert_eq!(session_wires.len(), 1);
        let session_json = match p.serialize(&session_wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("session.update must be a text frame"),
        };
        assert!(session_json.contains("session.update"));
        assert!(
            session_json.contains("\"type\":\"realtime\""),
            "GA session.type required: {session_json}"
        );
        assert!(
            session_json.contains("output_modalities"),
            "GA renamed modalities"
        );
        assert!(
            session_json.contains("alloy"),
            "voice nests under audio.output"
        );
        assert!(
            session_json.contains("audio/pcm"),
            "PCM16 ⇒ {{type: audio/pcm, rate: 24000}}"
        );
        // GA gpt-realtime carries no session-level temperature/reasoning.
        assert!(!session_json.contains("reasoning"));
        assert!(!session_json.contains("temperature"));

        // ── response.create (None ⇒ default) ──
        // Port of test_response_create_serialization (ResponseCreate { None }
        // serialized "response.create"); the scaffold's None path sends
        // ResponseConfig::default() which GA accepts.
        let create_wires = p.create_response(None);
        assert_eq!(create_wires.len(), 1);
        let create_json = match p.serialize(&create_wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("response.create must be a text frame"),
        };
        assert!(create_json.contains("response.create"));

        // ── response.create (out-of-band override) ──
        // The GA `create_response_with` mapping: out_of_band ⇒ conversation:"none",
        // a per-response voice nests under audio.output, modalities ⇒
        // output_modalities.
        let ov = RealtimeResponseOverride {
            modalities: Some(vec!["text".to_string()]),
            voice: Some("verse".to_string()),
            out_of_band: true,
            ..Default::default()
        };
        let oob_wires = p.create_response(Some(&ov));
        let oob_json = match p.serialize(&oob_wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("response.create must be a text frame"),
        };
        assert!(oob_json.contains("response.create"));
        assert!(
            oob_json.contains("\"conversation\":\"none\""),
            "out_of_band ⇒ conversation:none: {oob_json}"
        );
        assert!(oob_json.contains("output_modalities"));
        assert!(oob_json.contains("verse"), "per-response voice on audio.output");

        // ── conversation.item.truncate ──
        // GA field names: item_id, content_index:0, audio_end_ms.
        let trunc_wires = p.truncate("item_42", 420);
        let trunc_json = match p.serialize(&trunc_wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("truncate must be a text frame"),
        };
        assert!(trunc_json.contains("conversation.item.truncate"));
        assert!(trunc_json.contains("\"item_id\":\"item_42\""));
        assert!(trunc_json.contains("\"content_index\":0"));
        assert!(trunc_json.contains("\"audio_end_ms\":420"));

        // ── input_audio_buffer.append (encode_user_audio) ──
        // Port of test_audio_append: the payload round-trips through base64.
        let data = vec![0u8, 1, 2, 3];
        let append = p.encode_user_audio(&data);
        match &append {
            ClientEvent::InputAudioBufferAppend { audio } => {
                let decoded = BASE64_STANDARD.decode(audio).unwrap();
                assert_eq!(decoded, data, "base64 of the PCM round-trips");
            }
            _ => panic!("encode_user_audio must be input_audio_buffer.append"),
        }
        let append_json = match p.serialize(&append).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("append must be a text frame"),
        };
        assert!(append_json.contains("input_audio_buffer.append"));
    }

    /// `from_config` validates the API key exactly like `OpenAIRealtime::new`.
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(matches!(
            OpenAiProtocol::from_config(&cfg),
            Err(RealtimeError::AuthenticationFailed(_))
        ));
    }

    /// caps: server-VAD default ON unless the explicit manual `None` variant
    /// (matches the bespoke `emits_user_turn_frames`); format-aware truncate math.
    #[test]
    fn caps_track_turn_detection_and_audio_format() {
        use crate::core::realtime::base::TurnDetectionConfig;
        let mk = |td: Option<TurnDetectionConfig>, fmt: Option<&str>| {
            proto(&RealtimeConfig {
                api_key: "k".into(),
                model: "gpt-4o-realtime-preview".into(),
                turn_detection: td,
                input_audio_format: fmt.map(|s| s.to_string()),
                ..Default::default()
            })
            .caps()
        };
        // Explicit server VAD + omitted both ⇒ server produces turn frames.
        assert!(mk(Some(TurnDetectionConfig::default()), None).emits_user_turn_frames);
        assert!(
            mk(None, None).emits_user_turn_frames,
            "omitted turn_detection ⇒ OpenAI server-VAD default is ON"
        );
        // Explicit manual None flips it off.
        assert!(!mk(Some(TurnDetectionConfig::None), None).emits_user_turn_frames);
        // PCM16 default: 48 B/ms @24k.
        let pcm = mk(None, None);
        assert_eq!(pcm.output_bytes_per_ms, 48);
        assert_eq!(pcm.output_sample_rate, 24_000);
        assert!(pcm.supports_truncate);
        assert!(pcm.supports_input_buffer);
        // Telephony g711: 8 B/ms @8k.
        let g711 = mk(None, Some("g711_ulaw"));
        assert_eq!(g711.output_bytes_per_ms, 8);
        assert_eq!(g711.output_sample_rate, 8_000);
    }

    /// connect_spec carries EXACTLY the GA headers: bearer + realtime subprotocol,
    /// NO OpenAI-Beta header (retired in the GA migration).
    #[test]
    fn connect_spec_has_ga_headers_only() {
        let p = proto(&base_cfg());
        let spec = p.connect_spec(&base_cfg()).unwrap();
        let ConnectSpec::WebSocket { url, headers } = spec;
        assert!(url.contains("wss://api.openai.com"));
        assert!(url.contains("gpt-4o-realtime-preview"));
        let names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"Authorization"));
        assert!(names.contains(&"Sec-WebSocket-Protocol"));
        assert!(
            !names.iter().any(|n| n.eq_ignore_ascii_case("OpenAI-Beta")),
            "GA: the OpenAI-Beta header is retired"
        );
        let auth = headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth, "Bearer k");
    }

    /// Server speech_started maps to ONLY a VAD Started event and sends NOTHING
    /// on the wire — byte-identical to the old client (barge-in is the handler's
    /// explicit CancelResponse, not this VAD edge).
    #[test]
    fn speech_started_emits_only_started_no_interrupt() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"input_audio_buffer.speech_started","audio_start_ms":120,"item_id":"item_1"}"#;
        let evs = p.map_server_event(Inbound::Text(raw));
        assert_eq!(evs.len(), 1);
        assert!(matches!(
            evs[0],
            S2sEvent::Speech(SpeechEvent::Started { audio_start_ms: 120, .. })
        ));
    }

    /// Function-call args-done maps with an EMPTY name (the scaffold resolves it
    /// from pending_calls populated by OutputItemAdded ⇒ TrackPendingCall).
    #[test]
    fn function_call_args_done_leaves_name_for_scaffold() {
        let p = proto(&base_cfg());
        let added = r#"{"type":"response.output_item.added","response_id":"r1","output_index":0,"item":{"type":"function_call","call_id":"c1","name":"get_weather"}}"#;
        match p.map_server_event(Inbound::Text(added)).as_slice() {
            [S2sEvent::TrackPendingCall { call_id, name }] => {
                assert_eq!(call_id, "c1");
                assert_eq!(name, "get_weather");
            }
            other => panic!("expected TrackPendingCall, got {other:?}"),
        }
        let done = r#"{"type":"response.function_call_arguments.done","response_id":"r1","item_id":"i1","output_index":0,"call_id":"c1","arguments":"{}"}"#;
        match p.map_server_event(Inbound::Text(done)).as_slice() {
            [S2sEvent::FunctionCall(req)] => {
                assert_eq!(req.call_id, "c1");
                assert!(req.name.is_empty(), "scaffold resolves the name");
                assert_eq!(req.arguments, "{}");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    /// Unparseable + binary inbound ⇒ Ignore (the old code warn-logged + dropped).
    #[test]
    fn unparseable_and_binary_inbound_ignore() {
        let p = proto(&base_cfg());
        assert!(matches!(
            p.map_server_event(Inbound::Text("not json")).as_slice(),
            [S2sEvent::Ignore]
        ));
        assert!(matches!(
            p.map_server_event(Inbound::Binary(&[1, 2, 3])).as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// replay_item renders a completed message (input_text user / text assistant)
    /// and NEVER a response — the reconnect-context contract.
    #[test]
    fn replay_item_renders_completed_message_never_response() {
        let p = proto(&base_cfg());
        let wire = p.replay_item(&ReplayConversationItem {
            role: TranscriptRole::Assistant,
            text: "hi!".into(),
        });
        let json = match p.serialize(&wire[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("replay must be a text frame"),
        };
        assert!(json.contains("conversation.item.create"));
        assert!(!json.contains("response.create"));
        assert!(json.contains("\"type\":\"text\""), "assistant side: text");
    }
}

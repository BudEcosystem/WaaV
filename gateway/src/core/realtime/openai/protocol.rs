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

use std::sync::atomic::{AtomicBool, Ordering};

use base64::prelude::*;
use bytes::Bytes;
use serde_json::{Map, Value, json};

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
    RealtimeVerb, ReplayConversationItem, SpeechEvent, TranscriptRole,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// The OpenAI Realtime protocol. Stateless: model/voice/audio_format are the only
/// provider-specific config the wire mappings need (everything else is read from
/// the [`RealtimeConfig`] passed back into `build_session_config`).
///
/// `pub(crate)` (struct + fields) so the OpenAI-PROTOCOL CLONES (Azure / Grok /
/// Inworld realtime) can EMBED it and DELEGATE every wire method to it — the GA
/// wire is then byte-identical for the shared parts (only the endpoint/auth and
/// the rare bootstrap event differ).
pub struct OpenAiProtocol {
    pub(crate) model: OpenAIRealtimeModel,
    pub(crate) voice: OpenAIRealtimeVoice,
    pub(crate) audio_format: OpenAIRealtimeAudioFormat,
    /// review wf_d43814c3 #7: OpenAI's `session.turn_detection` defaults to server
    /// VAD ON, and WaaV OMITS the field when `config.turn_detection` is None
    /// (`skip_serializing_if`), so the server STILL runs VAD and produces turn
    /// frames. Server VAD is therefore on UNLESS the config explicitly selects the
    /// `None` (manual) variant. Computed once at `from_config`.
    pub(crate) emits_user_turn_frames: bool,
    /// The provider fixes of this wire in force for the session.
    pub(crate) fixes: OpenAiFixes,
    /// The server has sent audio: OpenAI then refuses a new voice (fix `openai_update_delta`).
    audio_out: AtomicBool,
}

/// The provider fixes of the OpenAI GA wire, read once from the session's config. Azure, Grok
/// and Inworld embed this protocol, so they take them too (Grok's `grok_session` replaces its
/// session and response frames).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct OpenAiFixes {
    /// `openai_manual_turn_null`: manual turns are `turn_detection: null`, not `{"type":"none"}`.
    pub(crate) manual_turn_null: bool,
    /// `openai_replay_output_text`: a replayed assistant line is `output_text` content.
    pub(crate) replay_output_text: bool,
    /// `openai_update_delta`: a mid-session update names only what changed.
    pub(crate) update_delta: bool,
    /// `openai_modalities`: the session's `modalities` choose `output_modalities`.
    pub(crate) modalities: bool,
    /// `openai_inbound`: a function's name is read from `response.function_call_arguments.done`,
    /// and the error of a `response.cancel` with nothing to cancel is not passed on.
    pub(crate) inbound: bool,
}

impl OpenAiFixes {
    pub(crate) fn of(cfg: &RealtimeConfig) -> Self {
        Self {
            manual_turn_null: cfg.fix("openai_manual_turn_null"),
            replay_output_text: cfg.fix("openai_replay_output_text"),
            update_delta: cfg.fix("openai_update_delta"),
            modalities: cfg.fix("openai_modalities"),
            inbound: cfg.fix("openai_inbound"),
        }
    }
}

/// The members of `after` that differ from `before`. The `audio` block and its `input` and
/// `output` are compared member by member; any other member (instructions, tools, a turn
/// detection object) goes whole when it changed. A member `after` lacks is not named.
pub(crate) fn changed_fields(before: &Value, after: &Value) -> Map<String, Value> {
    let mut changed = Map::new();
    let Some(after) = after.as_object() else {
        return changed;
    };
    for (key, value) in after {
        match before.get(key) {
            Some(old) if old == value => {}
            Some(old)
                if matches!(key.as_str(), "audio" | "input" | "output")
                    && old.is_object()
                    && value.is_object() =>
            {
                let inner = changed_fields(old, value);
                if !inner.is_empty() {
                    changed.insert(key.clone(), Value::Object(inner));
                }
            }
            _ => {
                changed.insert(key.clone(), value.clone());
            }
        }
    }
    changed
}

/// A `session.update` naming the changed fields of `session` (a realtime session's `type`
/// rides along, as GA requires it); none when nothing changed.
pub(crate) fn session_patch(before: &Value, after: &Value) -> Vec<ClientEvent> {
    let mut changed = changed_fields(before, after);
    changed.remove("type");
    if changed.is_empty() {
        return Vec::new();
    }
    if let Some(kind) = after.get("type") {
        changed.insert("type".to_owned(), kind.clone());
    }
    vec![ClientEvent::Raw(
        json!({ "type": "session.update", "session": Value::Object(changed) }),
    )]
}

impl OpenAiProtocol {
    /// The configured model (for the newtype's inherent `model()` accessor).
    pub fn model(&self) -> OpenAIRealtimeModel {
        self.model.clone()
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
            } => Some(TurnDetection::ServerVad {
                threshold: *threshold,
                prefix_padding_ms: *prefix_padding_ms,
                silence_duration_ms: *silence_duration_ms,
                create_response: *create_response,
                interrupt_response: *interrupt_response,
            }),
            crate::core::realtime::base::TurnDetectionConfig::SemanticVad {
                eagerness,
                create_response,
                interrupt_response,
            } => Some(TurnDetection::SemanticVad {
                eagerness: eagerness.clone(),
                create_response: *create_response,
                interrupt_response: *interrupt_response,
            }),
            // GA types turn detection as `server_vad` or `semantic_vad` only; manual turns
            // clear it with `null` (fix `openai_manual_turn_null`).
            crate::core::realtime::base::TurnDetectionConfig::None => {
                (!self.fixes.manual_turn_null).then_some(TurnDetection::None {})
            }
        });
        // A voice a mid-session update names is the session's from then on (fix
        // `openai_update_delta`); without it the voice of the first connect stays.
        let voice = match cfg.voice.as_deref() {
            Some(voice) if self.fixes.update_delta => {
                OpenAIRealtimeVoice::from_str_or_default(voice)
            }
            _ => self.voice,
        };
        // GA answers in one modality: `["audio"]` (with its transcript) or `["text"]`.
        let modality = if cfg.fix(crate::core::realtime::HALF_CASCADE_TEXT) {
            "text"
        } else {
            match cfg.modalities.as_deref().filter(|_| self.fixes.modalities) {
                Some(asked)
                    if !asked.iter().any(|m| m.eq_ignore_ascii_case("audio"))
                        && asked.iter().any(|m| m.eq_ignore_ascii_case("text")) =>
                {
                    "text"
                }
                _ => "audio",
            }
        };

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
                voice: Some(voice.as_str().to_string()),
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
            // The half-cascade's provider writes its replies; WaaV's voice says them.
            output_modalities: Some(vec![modality.to_string()]),
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

    /// Map a replay-log item to the wire conversation item: `input_text` for user, `text`
    /// for assistant (GA's `output_text` under the fix `openai_replay_output_text`); a
    /// completed message (never a response).
    fn replay_item_to_conversation_item(&self, item: &ReplayConversationItem) -> ConversationItem {
        let assistant = if self.fixes.replay_output_text {
            "output_text"
        } else {
            "text"
        };
        let (role, content_type) = match item.role {
            TranscriptRole::User => ("user", "input_text"),
            TranscriptRole::Assistant => ("assistant", assistant),
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
            fixes: OpenAiFixes::of(cfg),
            audio_out: AtomicBool::new(false),
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
        //
        // SERVER-CONFIG `realtime_endpoint_override` (proxy / self-hosted / gov-cloud
        // / local mock) WINS over the fixed GA host when set — bearer auth rides
        // along unchanged. SSRF-safe: never client-settable (see `apply_endpoint_override`).
        let default_url = format!("{}?model={}", OPENAI_REALTIME_URL, self.model.as_str());
        let url = crate::core::realtime::scaffold::apply_endpoint_override(
            &default_url,
            cfg.realtime_endpoint_override.as_deref(),
            None,
        );
        Ok(ConnectSpec::WebSocket {
            url,
            headers: vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", cfg.api_key),
                ),
                ("Sec-WebSocket-Protocol".to_string(), "realtime".to_string()),
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

    fn session_update(
        &self,
        old: &RealtimeConfig,
        new: &RealtimeConfig,
    ) -> Option<Vec<Self::Wire>> {
        if !self.fixes.update_delta {
            return Some(self.build_session_config(new, None));
        }
        // `session.update`: "update any field except for `voice` and `model`. `voice` can be
        // updated only if there have been no other audio outputs yet." "Only the fields that
        // are present in the `session.update` are updated."
        let voice = |cfg: &RealtimeConfig| {
            cfg.voice
                .as_deref()
                .map(str::trim)
                .map(str::to_ascii_lowercase)
        };
        if old.model.trim() != new.model.trim()
            || (voice(old) != voice(new) && self.audio_out.load(Ordering::Relaxed))
        {
            return None;
        }
        let as_json = |cfg: &RealtimeConfig| {
            serde_json::to_value(self.session_config(cfg)).unwrap_or_default()
        };
        Some(session_patch(&as_json(old), &as_json(new)))
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
            // "It's safe to call `response.cancel` even if no response is in progress, an error
            // will be returned the session will remain unaffected" (fix `openai_inbound`).
            ServerEvent::Error { error }
                if self.fixes.inbound
                    && error.code.as_deref() == Some("response_cancel_not_active") =>
            {
                tracing::debug!(message = %error.message, "response.cancel with no response");
                vec![S2sEvent::Ignore]
            }
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
                Ok(audio_bytes) => {
                    self.audio_out.store(true, Ordering::Relaxed);
                    vec![S2sEvent::Audio {
                        data: Bytes::from(audio_bytes),
                        item_id: Some(item_id),
                        response_id: Some(response_id),
                    }]
                }
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
                name,
                ..
            } => vec![S2sEvent::FunctionCall(FunctionCallRequest {
                call_id,
                // The event's own name (fix `openai_inbound`); else the scaffold resolves it
                // from its pending_calls map.
                name: name.filter(|_| self.fixes.inbound).unwrap_or_default(),
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
            item: self.replay_item_to_conversation_item(item),
            previous_item_id: None,
        }]
    }

    fn verb(&self, verb: &RealtimeVerb<'_>) -> Option<Vec<Self::Wire>> {
        match *verb {
            // No wire speaks given words verbatim.
            RealtimeVerb::SayExact(_) => None,
            // A line of the gateway's, never a turn of the model's own: no function call.
            RealtimeVerb::SayGuided(guidance) => {
                let mut wires = self.create_response(Some(&RealtimeResponseOverride {
                    instructions: Some(guidance.to_string()),
                    ..Default::default()
                }));
                for wire in &mut wires {
                    if let ClientEvent::ResponseCreate {
                        response: Some(response),
                    } = wire
                    {
                        response.tool_choice = Some("none".to_string());
                    }
                }
                Some(wires)
            }
            // The `send_text` item as a system message: context, and no response is asked.
            RealtimeVerb::AddContext(text) => {
                let mut wires = self.send_text(text);
                for wire in &mut wires {
                    if let ClientEvent::ConversationItemCreate { item, .. } = wire {
                        item.role = Some("system".to_string());
                    }
                }
                Some(wires)
            }
            RealtimeVerb::StopOutput => Some(self.cancel_response()),
        }
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

    /// TC-XL-08 (F-1) — a model the gateway has never heard of reaches the vendor VERBATIM. A
    /// closed enum turned `gpt-realtime-1.5`, `-2.1` and `-2.1-mini` into `gpt-realtime` (shut
    /// down 2027-01-20) without a word.
    #[test]
    fn tc_xl_08_an_unlisted_model_reaches_upstream_verbatim() {
        for model in [
            "gpt-realtime-2.1",
            "gpt-realtime-2.1-mini",
            "gpt-realtime-1.5",
        ] {
            let cfg = RealtimeConfig {
                model: model.into(),
                ..base_cfg()
            };
            let p = proto(&cfg);
            assert_eq!(p.model().as_str(), model);
            let ConnectSpec::WebSocket { url, .. } = p.connect_spec(&cfg).unwrap() else {
                panic!("expected a WebSocket connect spec");
            };
            assert_eq!(url, format!("{OPENAI_REALTIME_URL}?model={model}"));
        }
        // Only an EMPTY model takes the default.
        let cfg = RealtimeConfig {
            model: String::new(),
            ..base_cfg()
        };
        assert_eq!(proto(&cfg).model().as_str(), "gpt-realtime");
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
        assert!(
            oob_json.contains("verse"),
            "per-response voice on audio.output"
        );

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
        let ConnectSpec::WebSocket { url, headers } = spec else {
            panic!("expected ConnectSpec::WebSocket")
        };
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

    /// SERVER-CONFIG `realtime_endpoint_override` (proxy / mock / gov-cloud) WINS
    /// over the fixed GA host VERBATIM; the bearer auth header still rides along.
    /// SECURITY: this field is set ONLY from trusted server config, never from the
    /// client `RealtimeSessionConfig` (see `build_realtime_config`).
    #[test]
    fn connect_spec_honors_server_endpoint_override() {
        let cfg = RealtimeConfig {
            realtime_endpoint_override: Some("ws://127.0.0.1:9001/x".into()),
            ..base_cfg()
        };
        let ConnectSpec::WebSocket { url, headers } = proto(&cfg).connect_spec(&cfg).unwrap()
        else {
            panic!("expected WebSocket")
        };
        assert_eq!(url, "ws://127.0.0.1:9001/x");
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer k")
        );
    }

    /// Unset override ⇒ the normal GA host (no behavior change).
    #[test]
    fn connect_spec_ignores_absent_override() {
        let ConnectSpec::WebSocket { url, .. } =
            proto(&base_cfg()).connect_spec(&base_cfg()).unwrap()
        else {
            panic!("expected WebSocket")
        };
        assert!(url.starts_with("wss://api.openai.com"));
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
            S2sEvent::Speech(SpeechEvent::Started {
                audio_start_ms: 120,
                ..
            })
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

    /// One verb's wires as JSON values.
    fn verb_json(p: &OpenAiProtocol, verb: RealtimeVerb<'_>) -> Vec<serde_json::Value> {
        let wires = p.verb(&verb).expect("the verb has a wire");
        wires
            .iter()
            .map(|wire| match p.serialize(wire).unwrap() {
                OutFrame::Text(s) => serde_json::from_str(&s).unwrap(),
                OutFrame::Binary(_) => panic!("a verb must be a text frame"),
            })
            .collect()
    }

    /// `SayGuided` = `response.create` with the instructions, and no function call.
    #[test]
    fn verb_say_guided_is_response_create_with_instructions() {
        let wires = verb_json(
            &proto(&base_cfg()),
            RealtimeVerb::SayGuided("Say you are checking."),
        );
        assert_eq!(
            wires,
            [serde_json::json!({
                "type": "response.create",
                "response": {"instructions": "Say you are checking.", "tool_choice": "none"},
            })]
        );
    }

    /// `AddContext` = the `send_text` item with role `system`, and no `response.create`;
    /// `send_text` itself stays a user message.
    #[test]
    fn verb_add_context_is_a_system_item_and_asks_for_no_response() {
        let p = proto(&base_cfg());
        let wires = verb_json(&p, RealtimeVerb::AddContext("The booking is confirmed."));
        assert_eq!(
            wires,
            [serde_json::json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "message",
                    "role": "system",
                    "content": [{"type": "input_text", "text": "The booking is confirmed."}],
                },
            })]
        );
        match p.send_text("hello").as_slice() {
            [ClientEvent::ConversationItemCreate { item, .. }] => {
                assert_eq!(item.role.as_deref(), Some("user"));
            }
            _ => panic!("send_text must be one conversation.item.create"),
        }
    }

    /// `StopOutput` = `response.cancel`; `SayExact` has no wire on this protocol.
    #[test]
    fn verb_stop_output_is_response_cancel_and_say_exact_has_no_wire() {
        let p = proto(&base_cfg());
        assert_eq!(
            verb_json(&p, RealtimeVerb::StopOutput),
            [serde_json::json!({"type": "response.cancel"})]
        );
        assert!(p.verb(&RealtimeVerb::SayExact("One moment.")).is_none());
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

    /// A config with these provider fixes on.
    fn fixed(fixes: &[&str]) -> RealtimeConfig {
        RealtimeConfig {
            provider_fixes: fixes.iter().map(|f| (*f).to_owned()).collect(),
            ..base_cfg()
        }
    }

    /// Every wire as JSON.
    fn wires(p: &OpenAiProtocol, wires: &[ClientEvent]) -> Vec<Value> {
        wires
            .iter()
            .map(|wire| match p.serialize(wire).unwrap() {
                OutFrame::Text(s) => serde_json::from_str(&s).unwrap(),
                OutFrame::Binary(_) => panic!("a text frame"),
            })
            .collect()
    }

    /// Fix `openai_manual_turn_null`: manual turns clear `turn_detection` with `null`
    /// (`session.update`: "To clear a field like `turn_detection`, pass `null`"; GA types it
    /// `server_vad` or `semantic_vad` only). Without it today's `{"type":"none"}`.
    #[test]
    fn manual_turns_are_turn_detection_null_with_the_fix() {
        use crate::core::realtime::base::TurnDetectionConfig;
        let manual = |fixes: &[&str]| {
            let cfg = RealtimeConfig {
                turn_detection: Some(TurnDetectionConfig::None),
                ..fixed(fixes)
            };
            let p = proto(&cfg);
            wires(&p, &p.build_session_config(&cfg, None)).remove(0)
        };
        let today = manual(&[]);
        assert_eq!(
            today["session"]["audio"]["input"]["turn_detection"],
            json!({"type": "none"})
        );
        let nulled = manual(&["openai_manual_turn_null"]);
        let input = nulled["session"]["audio"]["input"].as_object().unwrap();
        assert_eq!(input.get("turn_detection"), Some(&Value::Null), "{nulled}");
        // Server VAD and an unset turn detection are the same either way.
        let cfg = fixed(&["openai_manual_turn_null"]);
        let p = proto(&cfg);
        let unset = wires(&p, &p.build_session_config(&cfg, None)).remove(0);
        assert!(
            unset["session"]["audio"]["input"]
                .get("turn_detection")
                .is_none()
        );
    }

    /// Fix `openai_replay_output_text`: a replayed assistant line is `output_text` (GA:
    /// "The content type, `output_text` or `output_audio`"); without it today's `text`.
    #[test]
    fn a_replayed_assistant_line_is_output_text_with_the_fix() {
        let line = ReplayConversationItem {
            role: TranscriptRole::Assistant,
            text: "Your table is booked.".into(),
        };
        let content = |fixes: &[&str]| {
            let p = proto(&fixed(fixes));
            wires(&p, &p.replay_item(&line)).remove(0)["item"]["content"].clone()
        };
        assert_eq!(
            content(&[]),
            json!([{"type": "text", "text": "Your table is booked."}])
        );
        assert_eq!(
            content(&["openai_replay_output_text"]),
            json!([{"type": "output_text", "text": "Your table is booked."}])
        );
        let user = ReplayConversationItem {
            role: TranscriptRole::User,
            text: "Book a table.".into(),
        };
        let p = proto(&fixed(&["openai_replay_output_text"]));
        assert_eq!(
            wires(&p, &p.replay_item(&user)).remove(0)["item"]["content"][0]["type"],
            "input_text"
        );
    }

    /// Fix `openai_update_delta` (escape flag `openai_session_resend`): an update sends only
    /// what changed ("Only the fields that are present in the `session.update` are
    /// updated"); a new voice once the model has spoken, or a new model, is refused. Without
    /// it the whole session again (today).
    #[test]
    fn a_mid_session_update_names_only_what_changed_with_the_fix() {
        let old = base_cfg();
        let mut new = base_cfg();
        new.instructions = Some("Answer in French.".into());
        let today = proto(&old);
        let sent = wires(&today, &today.session_update(&old, &new).unwrap());
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["session"]["audio"]["output"]["voice"], "alloy");
        assert_eq!(sent[0]["session"]["instructions"], "Answer in French.");

        let fix = fixed(&["openai_update_delta"]);
        let (old, mut new) = (fix.clone(), fix.clone());
        new.instructions = Some("Answer in French.".into());
        let p = proto(&old);
        assert_eq!(
            wires(&p, &p.session_update(&old, &new).unwrap()),
            [json!({"type": "session.update",
                    "session": {"type": "realtime", "instructions": "Answer in French."}})]
        );
        assert!(p.session_update(&old, &old).unwrap().is_empty());
        // A new voice before the first audio goes; after it, it is refused.
        new.voice = Some("verse".into());
        assert_eq!(
            wires(&p, &p.session_update(&old, &new).unwrap()),
            [json!({"type": "session.update",
                    "session": {"type": "realtime", "instructions": "Answer in French.",
                                "audio": {"output": {"voice": "verse"}}}})]
        );
        let audio = r#"{"type":"response.output_audio.delta","response_id":"r","item_id":"i","output_index":0,"content_index":0,"delta":"AAA="}"#;
        p.map_server_event(Inbound::Text(audio));
        assert!(p.session_update(&old, &new).is_none());
        new.voice = old.voice.clone();
        assert_eq!(wires(&p, &p.session_update(&old, &new).unwrap()).len(), 1);
        new.model = "gpt-realtime-2.1".into();
        assert!(p.session_update(&old, &new).is_none());
    }

    /// Fix `openai_modalities`: the session's `modalities` choose `output_modalities` (GA:
    /// "`["text"]` can be used to make the model respond with text only. It is not possible
    /// to request both"); without it always `["audio"]`.
    #[test]
    fn the_sessions_modalities_choose_the_output_modality_with_the_fix() {
        let modality = |fixes: &[&str], asked: &[&str]| {
            let cfg = RealtimeConfig {
                modalities: Some(asked.iter().map(|m| (*m).to_owned()).collect()),
                ..fixed(fixes)
            };
            let p = proto(&cfg);
            wires(&p, &p.build_session_config(&cfg, None)).remove(0)["session"]["output_modalities"]
                .clone()
        };
        assert_eq!(modality(&[], &["text"]), json!(["audio"]));
        assert_eq!(modality(&["openai_modalities"], &["text"]), json!(["text"]));
        assert_eq!(
            modality(&["openai_modalities"], &["text", "audio"]),
            json!(["audio"])
        );
    }

    /// Fix `openai_inbound`: the error of a `response.cancel` with nothing to cancel ("the
    /// session will remain unaffected") is not passed on, and a function's name comes from
    /// `response.function_call_arguments.done` (its `name` field). Without it today's mapping.
    #[test]
    fn the_inbound_fix_reads_the_name_and_drops_the_harmless_cancel_error() {
        let cancel = r#"{"type":"error","event_id":"e1","error":{"type":"invalid_request_error","code":"response_cancel_not_active","message":"Cancellation failed: no active response found"}}"#;
        let done = r#"{"type":"response.function_call_arguments.done","event_id":"e2","response_id":"r1","item_id":"i1","output_index":0,"call_id":"c1","name":"get_weather","arguments":"{}"}"#;
        let today = proto(&base_cfg());
        assert!(matches!(
            today.map_server_event(Inbound::Text(cancel)).as_slice(),
            [S2sEvent::Error(RealtimeError::ProviderError(_))]
        ));
        match today.map_server_event(Inbound::Text(done)).as_slice() {
            [S2sEvent::FunctionCall(call)] => assert!(call.name.is_empty()),
            other => panic!("{other:?}"),
        }
        let p = proto(&fixed(&["openai_inbound"]));
        assert!(matches!(
            p.map_server_event(Inbound::Text(cancel)).as_slice(),
            [S2sEvent::Ignore]
        ));
        match p.map_server_event(Inbound::Text(done)).as_slice() {
            [S2sEvent::FunctionCall(call)] => assert_eq!(call.name, "get_weather"),
            other => panic!("{other:?}"),
        }
        // Any other error still reaches the client.
        let other = r#"{"type":"error","error":{"type":"invalid_request_error","code":"invalid_value","message":"bad"}}"#;
        assert!(matches!(
            p.map_server_event(Inbound::Text(other)).as_slice(),
            [S2sEvent::Error(_)]
        ));
    }
}

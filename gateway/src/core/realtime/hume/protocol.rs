//! Hume EVI [`RealtimeProtocol`] — the canonical DUAL-PATH provider on the S2S
//! scaffold: Hume sends BOTH JSON text events (transcripts, prosody, lifecycle,
//! errors) AND, depending on the negotiated format, raw BINARY audio frames, so
//! this is the scaffold's first real-provider validation of text+binary
//! interleaving (`map_server_event` handles `Inbound::Text` AND `Inbound::Binary`).
//!
//! This is the verbatim extraction of the bespoke `HumeEVI` logic
//! (`send_session_settings`, `handle_server_message`, `send_audio`/`send_text`/
//! `cancel_response`/`submit_function_result`) — the wire is byte-identical to the
//! existing client. Pure + sync, so every mapping is trivially unit-testable.
//!
//! Everything Hume *varies* (the EVI session-settings message, the EVI server-
//! event lowering, base64 audio input) lives here; everything realtime providers
//! *share* (reconnect — which Hume LACKED — barge-in, resilience, callback
//! dispatch) now lives ONCE in the [`RealtimeSession`](crate::core::realtime::scaffold::RealtimeSession)
//! driver.

use bytes::Bytes;

use super::config::{EVIVersion, HumeEVIConfig};
use super::messages::{
    AudioInput, AudioSettings, EVIClientMessage, EVIServerMessage, HUME_EVI_DEFAULT_SAMPLE_RATE,
    SessionSettings, StopAssistant, TextInput, ToolResponse, deserialize_server_message,
    serialize_client_message,
};
use crate::core::realtime::base::{
    FunctionCallRequest, RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult,
    SpeechEvent, TranscriptRole,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent,
};

/// linear16 @ 44.1 kHz mono is 2 bytes/sample × 44.1 samples/ms = 88.2 ⇒ 88 B/ms
/// (floored). Hume EVI's audio output is linear16 @ [`HUME_EVI_DEFAULT_SAMPLE_RATE`].
const OUTPUT_BYTES_PER_MS: u64 = (HUME_EVI_DEFAULT_SAMPLE_RATE as u64 * 2) / 1000;

/// The Hume EVI protocol. Stateless: the full [`HumeEVIConfig`] (api key, EVI
/// version, voice, encoding, sample rate, system prompt, ws url, …) is parsed once
/// in `from_config` and re-read by the wire methods — the bespoke client carried
/// exactly this and nothing more.
pub struct HumeProtocol {
    /// The parsed EVI config (validated by `from_config`). Drives the ws-url
    /// query-param auth, the conditional session-settings message, and `caps()`.
    config: HumeEVIConfig,
}

impl HumeProtocol {
    /// Build the protocol directly from a fully-formed [`HumeEVIConfig`] (the
    /// `HumeEVI::from_hume_config` path). Runs the SAME validation the bespoke
    /// client did (`HumeEVIConfig::validate`).
    pub(crate) fn from_hume_config(config: HumeEVIConfig) -> RealtimeResult<Self> {
        config
            .validate()
            .map_err(|e| RealtimeError::InvalidConfiguration(e.to_string()))?;
        Ok(Self { config })
    }

    /// Borrow the parsed EVI config (for the newtype's inherent accessors +
    /// `get_provider_info`).
    pub(crate) fn evi_config(&self) -> &HumeEVIConfig {
        &self.config
    }

    /// Build the EVI `session_settings` message from the config (EXTRACTED verbatim
    /// from `HumeEVI::send_session_settings`). Inherent so the newtype/tests can
    /// reach it; the trait method wraps it in the SAME conditional the old client
    /// applied at connect time.
    fn session_settings(&self, system_prompt: Option<String>) -> SessionSettings {
        SessionSettings {
            audio: Some(AudioSettings {
                encoding: self.config.input_encoding,
                sample_rate: Some(self.config.sample_rate),
                channels: Some(self.config.channels),
            }),
            system_prompt,
            context: None,
        }
    }
}

impl RealtimeProtocol for HumeProtocol {
    type Wire = EVIClientMessage;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // Convert RealtimeConfig → HumeEVIConfig EXACTLY as `HumeEVI::new` did
        // (same defaults: V3, Linear16, 44.1 kHz, 1 channel, production ws url,
        // 30s timeout), then run the SAME validation (`HumeEVIConfig::validate`).
        let config = HumeEVIConfig {
            api_key: cfg.api_key.clone(),
            config_id: None,
            resumed_chat_group_id: None,
            evi_version: EVIVersion::V3,
            voice_id: cfg.voice.clone(),
            verbose_transcription: false,
            input_encoding: super::messages::AudioEncoding::Linear16,
            sample_rate: HUME_EVI_DEFAULT_SAMPLE_RATE,
            channels: 1,
            system_prompt: cfg.instructions.clone(),
            websocket_url: super::messages::HUME_EVI_WEBSOCKET_URL.to_string(),
            connection_timeout_seconds: 30,
            reconnection: cfg.reconnection.clone(),
        };
        // Same validation as the bespoke client (shared with the
        // `HumeEVI::from_hume_config` path).
        Self::from_hume_config(config)
    }

    fn provider_id(&self) -> &'static str {
        "hume"
    }

    fn caps(&self) -> ProtocolCaps {
        ProtocolCaps {
            // Hume EVI runs SERVER-side VAD + turn-taking (it auto-generates
            // responses on detected end-of-turn), so it owns turns and emits
            // user-turn frames — matching the bespoke client's no-op
            // `create_response`/`commit_audio_buffer`.
            emits_user_turn_frames: true,
            output_bytes_per_ms: OUTPUT_BYTES_PER_MS,
            // Output audio is linear16 @ the EVI default rate (the bespoke client
            // stamped every audio chunk with HUME_EVI_DEFAULT_SAMPLE_RATE).
            output_sample_rate: HUME_EVI_DEFAULT_SAMPLE_RATE,
            // EVI has NO server-side item truncation — barge-in is `StopAssistant`
            // (cancel) only; the gateway just clears local playback.
            supports_truncate: false,
            // No gateway-managed input buffer (audio is base64'd straight into
            // `audio_input` JSON events; no client commit/clear concept).
            supports_input_buffer: false,
        }
    }

    fn connect_spec(&self, _cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // EVI auth + config is ALL in the query string (api_key, config_id,
        // resumed_chat_group_id, voice_id, verbose_transcription, evi_version) —
        // EXACTLY `HumeEVIConfig::build_websocket_url()`. No auth HEADER. The
        // standard WS-upgrade headers are generated by `into_client_request()` in
        // the transport factory.
        Ok(ConnectSpec::WebSocket {
            url: self.config.build_websocket_url(),
            headers: Vec::new(),
        })
    }

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        _resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        // BYTE-IDENTICAL to the bespoke client: it sent `session_settings` ON
        // CONNECT *only* when a non-default audio format OR a system prompt was
        // configured (`HumeEVI::connect_internal`'s guard). For a default config
        // (default encoding, default sample rate, no prompt) it sent NOTHING — so
        // the protocol must emit an EMPTY vec there, not an always-on settings
        // frame. The scaffold re-sends this on reconnect (EVI re-applies it).
        //
        // The system_prompt comes from the PASSED `cfg.instructions` so the
        // scaffold's `update_session` (which swaps the live RealtimeConfig) actually
        // propagates a changed prompt — the bespoke client re-sent SessionSettings
        // with the new prompt on update (review wf_3edc08cf #1). Falls back to the
        // frozen config for the `from_hume_config` path (which may not populate
        // cfg.instructions). On the initial connect cfg.instructions == the frozen
        // prompt (both from_config'd from the same input), so the wire is identical.
        // (Audio format stays config-frozen — it isn't carried on the generic cfg.)
        let system_prompt = cfg
            .instructions
            .clone()
            .or_else(|| self.config.system_prompt.clone());
        if self.config.input_encoding != super::messages::AudioEncoding::default()
            || self.config.sample_rate != HUME_EVI_DEFAULT_SAMPLE_RATE
            || system_prompt.is_some()
        {
            vec![EVIClientMessage::SessionSettings(
                self.session_settings(system_prompt),
            )]
        } else {
            Vec::new()
        }
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        // DUAL PATH: a binary frame is audio output (the bespoke client's
        // `Message::Binary` arm). No base64 — copy the bytes straight through,
        // stamped (by the driver via caps) at HUME_EVI_DEFAULT_SAMPLE_RATE; the
        // old client set item_id=None for binary audio.
        let text = match raw {
            Inbound::Binary(b) => {
                return vec![S2sEvent::Audio {
                    data: Bytes::copy_from_slice(b),
                    item_id: None,
                    response_id: None,
                }];
            }
            Inbound::Text(t) => t,
        };

        // Port of `HumeEVI::handle_server_message`: deserialize then map each arm.
        // An unparseable frame was warn-logged + dropped ⇒ Ignore.
        let msg = match deserialize_server_message(text) {
            Ok(m) => m,
            Err(_) => return vec![S2sEvent::Ignore],
        };

        match msg {
            // chat_metadata ⇒ SessionReady. The bespoke client stored BOTH chat_id
            // and chat_group_id; the scaffold carries one session identity, so the
            // chat_id (the per-chat id) becomes the session id (drives the
            // newtype's `get_chat_id()`).
            EVIServerMessage::ChatMetadata(meta) => vec![S2sEvent::SessionReady {
                session_id: Some(meta.chat_id),
            }],

            // user_message ⇒ User transcript. is_final = NOT interim (the bespoke
            // client: `interim != Some(true)`). Prosody was only trace-logged — no
            // wire/callback effect — so it is intentionally NOT mapped.
            EVIServerMessage::UserMessage(user_msg) => vec![S2sEvent::Transcript {
                role: TranscriptRole::User,
                text: user_msg.message.content,
                is_final: user_msg.interim != Some(true),
                item_id: Some(user_msg.id),
            }],

            // user_interruption ⇒ a VAD Started edge (the bespoke client fired ONLY
            // the speech-event callback with `audio_start_ms = time.unwrap_or(0)`,
            // item_id None). It did NOT cancel/truncate, so we do NOT emit
            // InterruptedByServer here — byte-identical: speech edge only.
            EVIServerMessage::UserInterruption(interruption) => {
                vec![S2sEvent::Speech(SpeechEvent::Started {
                    audio_start_ms: interruption.time.unwrap_or(0),
                    item_id: None,
                })]
            }

            // assistant_message ⇒ Assistant transcript (always final in the bespoke
            // client). The driver's conversation_log + accumulator reset replace the
            // old `current_response_id` write (now session bookkeeping).
            EVIServerMessage::AssistantMessage(asst_msg) => vec![S2sEvent::Transcript {
                role: TranscriptRole::Assistant,
                text: asst_msg.message.content,
                is_final: true,
                item_id: Some(asst_msg.id),
            }],

            // assistant_prosody was trace-logged only ⇒ no actionable event.
            EVIServerMessage::AssistantProsody(_) => vec![S2sEvent::Ignore],

            // audio_output ⇒ Audio (base64 in a JSON event). The bespoke client
            // base64-DECODED `output.data`, stamped item_id = Some(output.id); a
            // decode error was warn-logged + dropped ⇒ Ignore.
            EVIServerMessage::AudioOutput(output) => match output.decode_audio() {
                Ok(audio_bytes) => vec![S2sEvent::Audio {
                    data: Bytes::from(audio_bytes),
                    item_id: Some(output.id),
                    response_id: None,
                }],
                Err(_) => vec![S2sEvent::Ignore],
            },

            // assistant_end ⇒ ResponseDone. The bespoke client passed
            // `end.id.unwrap_or_default()` to the response-done callback.
            EVIServerMessage::AssistantEnd(end) => vec![S2sEvent::ResponseDone {
                response_id: end.id.unwrap_or_default(),
            }],

            // tool_call ⇒ FunctionCall (name carried directly — Hume gives it
            // up-front, unlike OpenAI's two-step OutputItemAdded→args.done).
            EVIServerMessage::ToolCall(call) => {
                vec![S2sEvent::FunctionCall(FunctionCallRequest {
                    call_id: call.tool_call_id,
                    name: call.name,
                    arguments: call.parameters,
                    item_id: call.id,
                })]
            }

            // tool_error ⇒ Error(ProviderError) with the bespoke client's exact
            // "Tool error: {error}" message.
            EVIServerMessage::ToolError(err) => vec![S2sEvent::Error(
                RealtimeError::ProviderError(format!("Tool error: {}", err.error)),
            )],

            // error ⇒ Error(ProviderError) with the bespoke "{code}: {message}".
            EVIServerMessage::Error(err) => vec![S2sEvent::Error(RealtimeError::ProviderError(
                format!("{}: {}", err.code, err.message),
            ))],

            // websocket_error ⇒ Error(WebSocketError) with the reason (or "Unknown
            // error"), matching the bespoke client.
            EVIServerMessage::WebSocketError(err) => vec![S2sEvent::Error(
                RealtimeError::WebSocketError(err.reason.unwrap_or_else(|| "Unknown error".to_string())),
            )],

            // Unknown (forward-compat) ⇒ trace-logged + ignored.
            EVIServerMessage::Unknown => vec![S2sEvent::Ignore],
        }
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        // EXTRACTED verbatim from `HumeEVI::send_audio`: base64 into an EVI
        // `audio_input` JSON event (NOT a binary frame — the input leg is JSON).
        EVIClientMessage::AudioInput(AudioInput::from_bytes(pcm))
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        // EXTRACTED verbatim from `HumeEVI::send_text`.
        vec![EVIClientMessage::TextInput(TextInput {
            text: text.to_string(),
        })]
    }

    fn create_response(&self, _overrides: Option<&RealtimeResponseOverride>) -> Vec<Self::Wire> {
        // EVI auto-generates responses via server turn detection — the bespoke
        // `create_response` was a no-op. Nothing on the wire.
        Vec::new()
    }

    fn commit_turn(&self) -> Vec<Self::Wire> {
        // Server VAD owns turn close — the bespoke `commit_audio_buffer` was a
        // no-op. Nothing on the wire.
        Vec::new()
    }

    fn cancel_response(&self) -> Vec<Self::Wire> {
        // EXTRACTED verbatim from `HumeEVI::cancel_response`: `StopAssistant`.
        vec![EVIClientMessage::StopAssistant(StopAssistant::default())]
    }

    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<Self::Wire> {
        // EXTRACTED verbatim from `HumeEVI::submit_function_result`: a
        // `tool_response` with tool_name=None.
        vec![EVIClientMessage::ToolResponse(ToolResponse {
            tool_call_id: call_id.to_string(),
            content: result.to_string(),
            tool_name: None,
        })]
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        // EVI is JSON-only on the OUTBOUND leg (audio input is base64'd into JSON),
        // so every wire message is a text frame — using the existing
        // `serialize_client_message` helper for byte-identical output.
        Ok(OutFrame::Text(serialize_client_message(msg).map_err(
            |e| RealtimeError::SerializationError(e.to_string()),
        )?))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::ReplayConversationItem;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    fn base_cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "hume".into(),
            api_key: "hume-key".into(),
            voice: Some("kora".into()),
            instructions: Some("Be helpful".into()),
            ..Default::default()
        }
    }

    fn proto(cfg: &RealtimeConfig) -> HumeProtocol {
        HumeProtocol::from_config(cfg).unwrap()
    }

    /// from_config requires an API key (query-param auth needs it) — same as the
    /// bespoke `HumeEVIConfig::validate`.
    #[test]
    fn from_config_requires_api_key() {
        let cfg = RealtimeConfig {
            api_key: String::new(),
            ..Default::default()
        };
        assert!(matches!(
            HumeProtocol::from_config(&cfg),
            Err(RealtimeError::InvalidConfiguration(_))
        ));
    }

    /// from_config maps RealtimeConfig → HumeEVIConfig with the SAME defaults the
    /// bespoke `HumeEVI::new` used (V3, Linear16, 44.1 kHz, mono, prod ws url).
    #[test]
    fn from_config_maps_defaults_like_bespoke_new() {
        let p = proto(&base_cfg());
        let c = p.evi_config();
        assert_eq!(c.api_key, "hume-key");
        assert_eq!(c.voice_id, Some("kora".to_string()));
        assert_eq!(c.system_prompt, Some("Be helpful".to_string()));
        assert_eq!(c.evi_version, EVIVersion::V3);
        assert_eq!(c.sample_rate, HUME_EVI_DEFAULT_SAMPLE_RATE);
        assert_eq!(c.channels, 1);
    }

    /// connect_spec: the EXACT `build_websocket_url()` (query-param api_key +
    /// evi_version), NO auth header.
    #[test]
    fn connect_spec_uses_query_param_auth_no_header() {
        let p = proto(&base_cfg());
        let ConnectSpec::WebSocket { url, headers } = p.connect_spec(&base_cfg()).unwrap();
        assert!(url.starts_with(HUME_EVI_WEBSOCKET_URL_PREFIX));
        assert!(url.contains("api_key=hume-key"));
        assert!(url.contains("evi_version=3"));
        assert!(url.contains("voice_id=kora"));
        assert!(
            headers.is_empty(),
            "Hume auth is in the query string, not a header"
        );
    }

    const HUME_EVI_WEBSOCKET_URL_PREFIX: &str = "wss://api.hume.ai/v0/evi/chat";

    /// caps: server-VAD turn frames, 88 B/ms @ 44.1 kHz, NO truncate, NO input
    /// buffer (the real linear16 @ HUME_EVI_DEFAULT_SAMPLE_RATE values).
    #[test]
    fn caps_describe_hume_dual_path_server_vad_provider() {
        let c = proto(&base_cfg()).caps();
        assert!(c.emits_user_turn_frames);
        assert_eq!(c.output_bytes_per_ms, 88);
        assert_eq!(c.output_sample_rate, 44_100);
        assert_eq!(c.output_sample_rate, HUME_EVI_DEFAULT_SAMPLE_RATE);
        assert!(!c.supports_truncate);
        assert!(!c.supports_input_buffer);
    }

    /// build_session_config: a NON-default config (system prompt present) emits the
    /// `session_settings` frame BYTE-IDENTICALLY to the bespoke client.
    #[test]
    fn session_config_emitted_for_nondefault_then_byte_identical() {
        let p = proto(&base_cfg());
        let wires = p.build_session_config(&base_cfg(), None);
        assert_eq!(wires.len(), 1, "system_prompt present ⇒ one settings frame");
        let json = match p.serialize(&wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("session_settings must be a text frame"),
        };
        assert!(json.contains("session_settings"));
        assert!(json.contains("linear16"));
        assert!(json.contains("44100"));
        assert!(json.contains("Be helpful"));
    }

    /// build_session_config reads the PASSED cfg's instructions, so the scaffold's
    /// update_session (which swaps the live RealtimeConfig) propagates a CHANGED
    /// system prompt to the wire. The bespoke client re-sent SessionSettings with
    /// the new prompt on update; the first migration froze it (review wf_3edc08cf #1).
    #[test]
    fn build_session_config_propagates_updated_instructions() {
        let p = proto(&base_cfg());
        let updated = RealtimeConfig {
            instructions: Some("You are now a pirate.".to_string()),
            ..base_cfg()
        };
        let wires = p.build_session_config(&updated, None);
        assert_eq!(wires.len(), 1);
        let json = match p.serialize(&wires[0]).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("text frame"),
        };
        assert!(
            json.contains("You are now a pirate."),
            "updated prompt must reach the wire"
        );
        assert!(
            !json.contains("Be helpful"),
            "stale frozen prompt must NOT be re-sent on update"
        );
    }

    /// build_session_config: a DEFAULT config (default encoding + rate, NO prompt)
    /// emits NOTHING — byte-identical to the bespoke connect guard which skipped
    /// `send_session_settings` entirely.
    #[test]
    fn session_config_empty_for_default_config() {
        let cfg = RealtimeConfig {
            provider: "hume".into(),
            api_key: "hume-key".into(),
            ..Default::default()
        };
        let p = proto(&cfg);
        assert!(
            p.build_session_config(&cfg, None).is_empty(),
            "default config sends no session_settings (bespoke parity)"
        );
    }

    /// encode_user_audio ⇒ an `audio_input` JSON event whose base64 data
    /// round-trips to the exact PCM (the INPUT leg is JSON, NOT binary).
    #[test]
    fn encode_user_audio_is_base64_json_event() {
        let p = proto(&base_cfg());
        let pcm: Vec<u8> = vec![0x00, 0x01, 0xFE, 0xFF, 0x7F, 0x80];
        let wire = p.encode_user_audio(&pcm);
        match &wire {
            EVIClientMessage::AudioInput(input) => {
                let decoded = BASE64.decode(&input.data).unwrap();
                assert_eq!(decoded, pcm, "base64 of the PCM round-trips");
            }
            _ => panic!("encode_user_audio must be an AudioInput event"),
        }
        let json = match p.serialize(&wire).unwrap() {
            OutFrame::Text(s) => s,
            OutFrame::Binary(_) => panic!("audio_input must serialize to a TEXT frame"),
        };
        assert!(json.contains("audio_input"));
        assert!(json.contains("\"data\""));
    }

    /// THE DUAL-PATH TEST: a raw BINARY inbound frame ⇒ Audio with byte-exact PCM
    /// (the bespoke client's `Message::Binary` arm), item_id None.
    #[test]
    fn binary_inbound_maps_to_audio() {
        let p = proto(&base_cfg());
        let pcm: &[u8] = &[1, 2, 3, 4, 250, 251];
        match p.map_server_event(Inbound::Binary(pcm)).as_slice() {
            [S2sEvent::Audio {
                data,
                item_id,
                response_id,
            }] => {
                assert_eq!(data.as_ref(), pcm);
                assert!(item_id.is_none());
                assert!(response_id.is_none());
            }
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    /// audio_output (base64 JSON) ⇒ Audio with the decoded bytes + item_id.
    #[test]
    fn audio_output_json_maps_to_decoded_audio() {
        let p = proto(&base_cfg());
        let pcm = vec![10u8, 20, 30, 40, 50];
        let encoded = BASE64.encode(&pcm);
        let raw = format!(r#"{{"type":"audio_output","id":"a1","data":"{encoded}"}}"#);
        match p.map_server_event(Inbound::Text(&raw)).as_slice() {
            [S2sEvent::Audio {
                data,
                item_id,
                response_id,
            }] => {
                assert_eq!(data.as_ref(), pcm.as_slice());
                assert_eq!(item_id.as_deref(), Some("a1"));
                assert!(response_id.is_none());
            }
            other => panic!("expected decoded Audio, got {other:?}"),
        }
    }

    /// chat_metadata ⇒ SessionReady carrying the chat_id (drives get_chat_id()).
    #[test]
    fn chat_metadata_maps_to_session_ready_with_chat_id() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"chat_metadata","chat_id":"chat_abc","chat_group_id":"grp_xyz"}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => {
                assert_eq!(session_id.as_deref(), Some("chat_abc"));
            }
            other => panic!("expected SessionReady, got {other:?}"),
        }
    }

    /// user_message ⇒ User transcript; interim:true ⇒ NOT final; absent ⇒ final
    /// (the bespoke `interim != Some(true)`).
    #[test]
    fn user_message_maps_to_user_transcript_with_finality() {
        let p = proto(&base_cfg());
        let interim = r#"{"type":"user_message","id":"u1","message":{"role":"user","content":"hi"},"interim":true}"#;
        match p.map_server_event(Inbound::Text(interim)).as_slice() {
            [S2sEvent::Transcript {
                role,
                text,
                is_final,
                item_id,
            }] => {
                assert_eq!(*role, TranscriptRole::User);
                assert_eq!(text, "hi");
                assert!(!*is_final, "interim:true ⇒ not final");
                assert_eq!(item_id.as_deref(), Some("u1"));
            }
            other => panic!("expected user Transcript, got {other:?}"),
        }
        let final_msg = r#"{"type":"user_message","id":"u2","message":{"role":"user","content":"done"}}"#;
        match p.map_server_event(Inbound::Text(final_msg)).as_slice() {
            [S2sEvent::Transcript { is_final, .. }] => {
                assert!(*is_final, "absent interim ⇒ final");
            }
            other => panic!("expected user Transcript, got {other:?}"),
        }
    }

    /// assistant_message ⇒ FINAL Assistant transcript.
    #[test]
    fn assistant_message_maps_to_final_assistant_transcript() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"assistant_message","id":"m2","message":{"role":"assistant","content":"hello!"}}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::Transcript {
                role,
                text,
                is_final,
                item_id,
            }] => {
                assert_eq!(*role, TranscriptRole::Assistant);
                assert_eq!(text, "hello!");
                assert!(*is_final);
                assert_eq!(item_id.as_deref(), Some("m2"));
            }
            other => panic!("expected assistant Transcript, got {other:?}"),
        }
    }

    /// user_interruption ⇒ Speech(Started) ONLY (no InterruptedByServer) —
    /// byte-identical to the bespoke client (it fired only the speech callback).
    #[test]
    fn user_interruption_maps_to_speech_started_only() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"user_interruption","time":123}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::Speech(SpeechEvent::Started {
                audio_start_ms,
                item_id,
            })] => {
                assert_eq!(*audio_start_ms, 123);
                assert!(item_id.is_none());
            }
            other => panic!("expected ONLY Speech(Started), got {other:?}"),
        }
        // Absent time ⇒ 0 (the bespoke `time.unwrap_or(0)`).
        let no_time = r#"{"type":"user_interruption"}"#;
        match p.map_server_event(Inbound::Text(no_time)).as_slice() {
            [S2sEvent::Speech(SpeechEvent::Started { audio_start_ms, .. })] => {
                assert_eq!(*audio_start_ms, 0);
            }
            other => panic!("expected Speech(Started) with 0, got {other:?}"),
        }
    }

    /// assistant_end ⇒ ResponseDone (id, or "" when absent).
    #[test]
    fn assistant_end_maps_to_response_done() {
        let p = proto(&base_cfg());
        let with_id = r#"{"type":"assistant_end","id":"m3"}"#;
        match p.map_server_event(Inbound::Text(with_id)).as_slice() {
            [S2sEvent::ResponseDone { response_id }] => assert_eq!(response_id, "m3"),
            other => panic!("expected ResponseDone, got {other:?}"),
        }
        let no_id = r#"{"type":"assistant_end"}"#;
        match p.map_server_event(Inbound::Text(no_id)).as_slice() {
            [S2sEvent::ResponseDone { response_id }] => assert!(response_id.is_empty()),
            other => panic!("expected ResponseDone with empty id, got {other:?}"),
        }
    }

    /// tool_call ⇒ FunctionCall carrying call_id/name/arguments directly (Hume
    /// gives the name up-front — no pending-call two-step).
    #[test]
    fn tool_call_maps_to_function_call() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"tool_call","tool_call_id":"c9","name":"get_weather","parameters":"{\"city\":\"NYC\"}","id":"i9"}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::FunctionCall(req)] => {
                assert_eq!(req.call_id, "c9");
                assert_eq!(req.name, "get_weather");
                assert_eq!(req.arguments, r#"{"city":"NYC"}"#);
                assert_eq!(req.item_id.as_deref(), Some("i9"));
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    /// error ⇒ Error(ProviderError) with the bespoke "{code}: {message}"; tool_error
    /// ⇒ "Tool error: {error}".
    #[test]
    fn error_and_tool_error_map_to_provider_error() {
        let p = proto(&base_cfg());
        let err = r#"{"type":"error","code":"rate_limit_exceeded","message":"Too many requests"}"#;
        match p.map_server_event(Inbound::Text(err)).as_slice() {
            [S2sEvent::Error(RealtimeError::ProviderError(d))] => {
                assert_eq!(d, "rate_limit_exceeded: Too many requests");
            }
            other => panic!("expected Error(ProviderError), got {other:?}"),
        }
        let tool_err = r#"{"type":"tool_error","tool_call_id":"c1","error":"boom"}"#;
        match p.map_server_event(Inbound::Text(tool_err)).as_slice() {
            [S2sEvent::Error(RealtimeError::ProviderError(d))] => {
                assert_eq!(d, "Tool error: boom");
            }
            other => panic!("expected Tool error ProviderError, got {other:?}"),
        }
    }

    /// websocket_error ⇒ Error(WebSocketError) with the reason (or "Unknown error").
    #[test]
    fn websocket_error_maps_to_ws_error() {
        let p = proto(&base_cfg());
        let raw = r#"{"type":"web_socket_error","code":42,"reason":"closed by policy"}"#;
        match p.map_server_event(Inbound::Text(raw)).as_slice() {
            [S2sEvent::Error(RealtimeError::WebSocketError(d))] => {
                assert_eq!(d, "closed by policy");
            }
            other => panic!("expected WebSocketError, got {other:?}"),
        }
    }

    /// Unknown event + unparseable text ⇒ Ignore (forward-compat / warn-drop).
    #[test]
    fn unknown_and_unparseable_map_to_ignore() {
        let p = proto(&base_cfg());
        assert!(matches!(
            p.map_server_event(Inbound::Text(r#"{"type":"future_thing"}"#))
                .as_slice(),
            [S2sEvent::Ignore]
        ));
        assert!(matches!(
            p.map_server_event(Inbound::Text("not json")).as_slice(),
            [S2sEvent::Ignore]
        ));
    }

    /// Outbound builders: send_text ⇒ text_input; cancel_response ⇒ stop_assistant;
    /// submit ⇒ tool_response; turn controls empty (server VAD owns them); no
    /// truncate / input-buffer / replay (default-empty).
    #[test]
    fn outbound_builders_byte_identical_and_turn_controls_empty() {
        let p = proto(&base_cfg());
        // text_input
        match p.send_text("hello").as_slice() {
            [w] => {
                let json = match p.serialize(w).unwrap() {
                    OutFrame::Text(s) => s,
                    OutFrame::Binary(_) => panic!("text frame"),
                };
                assert!(json.contains("text_input"));
                assert!(json.contains("hello"));
            }
            other => panic!("expected one text_input, got {other:?}"),
        }
        // stop_assistant (barge-in cancel)
        match p.cancel_response().as_slice() {
            [w] => {
                let json = match p.serialize(w).unwrap() {
                    OutFrame::Text(s) => s,
                    OutFrame::Binary(_) => panic!("text frame"),
                };
                assert!(json.contains("stop_assistant"));
            }
            other => panic!("expected stop_assistant, got {other:?}"),
        }
        // tool_response
        match p.format_tool_result("c1", r#"{"ok":true}"#).as_slice() {
            [w] => {
                let json = match p.serialize(w).unwrap() {
                    OutFrame::Text(s) => s,
                    OutFrame::Binary(_) => panic!("text frame"),
                };
                assert!(json.contains("tool_response"));
                assert!(json.contains("c1"));
            }
            other => panic!("expected tool_response, got {other:?}"),
        }
        // Server-VAD: response/turn are no-ops; no truncate/input-buffer/replay.
        assert!(p.create_response(None).is_empty());
        assert!(p.commit_turn().is_empty());
        assert!(p.truncate("x", 1).is_empty());
        assert!(p.clear_input_buffer().is_empty());
        assert!(
            p.replay_item(&ReplayConversationItem {
                role: TranscriptRole::User,
                text: "x".into(),
            })
            .is_empty()
        );
    }
}

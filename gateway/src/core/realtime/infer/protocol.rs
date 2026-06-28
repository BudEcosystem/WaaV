//! WaaV Infer native-S2S [`RealtimeProtocol`] — Regime B (INFER_GATEWAY_INTEGRATION §6, §13 **GW-9**):
//! the **native full-duplex S2S** adapter. The Infer engine hosts a Moshi-class *interaction model*
//! (its LLM is intrinsic, §1/§6.4); the gateway is a thin passthrough that streams audio
//! BIDIRECTIONALLY and forwards only `cancel`/`truncate` (which degrade to `clear` / no-op for a
//! continuous-duplex model — §6.4). Turn-taking, interruption, backchannel, and overlap are all INSIDE
//! the model, so [`ProtocolCaps::emits_user_turn_frames`] is `true` (the cascade Smart-Turn /
//! `TurnController` is bypassed — §6.1: "an integration test asserts the cascade Smart-Turn is bypassed
//! iff true").
//!
//! This is the gateway twin of the Infer-side `waav-infer-provider::s2s` wire mapping: it speaks the
//! engine's native WS v1 wire (§5.2) — `session.config{task:s2s}` on connect, raw-binary user audio
//! OUT, raw-binary assistant audio IN (the §6.2 "ws-only today / loopback-ws first" transport, D5).
//! Pure + sync (every method maps config/audio/events to/from typed wire frames); the generic
//! [`RealtimeSession`](crate::core::realtime::scaffold::RealtimeSession) driver owns reconnect /
//! barge-in / resilience / dispatch — so registering `InferProtocol` makes `/realtime` + the DAG
//! `realtime_provider` node inherit them for free (§6.5).
//!
//! ## Full-duplex (the §6.4 experience bar)
//! Audio is FULL-DUPLEX: [`encode_user_audio`](RealtimeProtocol::encode_user_audio) puts the user's
//! mic PCM on the wire as a raw binary frame CONTINUOUSLY (no commit / no turn boundary), while
//! [`map_server_event`](RealtimeProtocol::map_server_event) lowers each inbound assistant-audio binary
//! frame to an [`S2sEvent::Audio`] — independently and concurrently. The two halves have no ordering
//! dependency (the duplex keystone the integration test exercises).

use bytes::Bytes;
use serde_json::{Value, json};

use crate::core::realtime::base::{
    RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult, ReplayConversationItem,
    SpeechEvent, TranscriptRole,
};
use crate::core::realtime::scaffold::{
    ConnectSpec, Inbound, OutFrame, ProtocolCaps, RealtimeProtocol, S2sEvent, apply_endpoint_override,
};
use crate::middleware::request_id::is_w3c_traceparent;

/// The default loopback native-WS endpoint for a co-located Infer S2S tier (§6.2 D5: loopback-ws
/// first; same-box TCP-HOL is negligible). A SERVER-CONFIG `realtime_endpoint_override` points it at a
/// remote Infer router / sidecar / mock (SSRF-safe — never client-settable).
const INFER_S2S_URL: &str = "ws://127.0.0.1:8088/v1/realtime";

/// The Infer S2S native audio rate (PCM16). The interaction model runs at a fixed native rate; the
/// gateway resamples to it at the edge (§5.3). 24 kHz is the engine TTS-egress canon (§4).
const INFER_S2S_SAMPLE_RATE: u32 = 24_000;

/// PCM16 @ 24 kHz mono ⇒ 48 output bytes per ms (the barge-in truncate math, though a continuous-duplex
/// model rarely truncates — §6.4).
const INFER_S2S_OUTPUT_BYTES_PER_MS: u64 = 48;

/// The `unix://` scheme that selects the in-box UDS sidecar transport (GW-13 UDS half, §6.2/§13).
const INFER_UDS_SCHEME: &str = "unix://";

/// PURE, SSRF-safe parse of a SERVER-CONFIG `realtime_endpoint_override` into an in-box UDS socket path.
/// `Some(path)` iff the (trimmed) override is a `unix://<non-empty-path>`; otherwise `None` (the caller
/// falls back to the ws path). The override is server-config only (the `*_REALTIME_URL` env), never
/// client-settable — so a `unix://` value can only come from a trusted deployment, not a request.
fn unix_socket_override(override_url: Option<&str>) -> Option<String> {
    let trimmed = override_url.map(str::trim).filter(|s| !s.is_empty())?;
    let path = trimmed.strip_prefix(INFER_UDS_SCHEME)?;
    // A blank path (`unix://`) is malformed ⇒ ignore (fall back to ws, no scheme downgrade surprise).
    (!path.is_empty()).then(|| path.to_string())
}

/// The WaaV Infer native-S2S protocol. Stateless: the only provider-specific config the wire mappings
/// need is the model id + voice (everything else rides in `cfg` back into `build_session_config`).
pub struct InferProtocol {
    model: String,
    voice: Option<String>,
}

impl InferProtocol {
    /// The native `session.config{task:s2s}` handshake frame body (the §6.1 `SessionConfig{task:S2S}`).
    /// The voice the model speaks in is threaded through as `conditioning.voice` (§5.4 features survive
    /// the hop). Same JSON shape as the engine's native WS v1 `session.config` (§5.2).
    fn session_config(&self, cfg: &RealtimeConfig) -> Value {
        let mut sc = json!({
            "type": "session.config",
            "task": "s2s",
            "model": self.model,
            "audio": { "encoding": "pcm16", "sample_rate": INFER_S2S_SAMPLE_RATE, "channels": 1 },
        });
        // Voice from the config (or the protocol default) → conditioning.voice. Skip-if-none discipline
        // (the engine rejects unknown/explicit-null keys on the handshake, §6.1 note on the scaffold).
        // NB: a `system`/instructions prompt is deliberately NEVER sent — an interaction model's persona
        // is intrinsic (§6.4 / §10 D2: the LLM is inside the model), so there is no system-prompt key.
        if let Some(voice) = cfg.voice.as_deref().filter(|v| !v.is_empty()).or(self.voice.as_deref()) {
            sc["conditioning"] = json!({ "voice": voice });
        }
        // GW-17 (INFER_GATEWAY_INTEGRATION §13): forward the propagated W3C `traceparent` on the handshake
        // so the engine parses it into `SessionConfig::trace` and parents its per-turn / per-stage spans
        // under the gateway trace — one distributed trace spans the gateway turn AND the intra-Infer
        // STT/LLM/TTS stages. The Infer `trace` field is a typed traceparent, so we inject ONLY a
        // well-formed value (a malformed one would fail the engine's `session.config` deserialization);
        // absent/invalid ⇒ the key is omitted (an untraced handshake is byte-unchanged).
        if let Some(tp) = cfg.trace.as_deref().filter(|t| is_w3c_traceparent(t)) {
            sc["trace"] = json!(tp);
        }
        sc
    }
}

impl RealtimeProtocol for InferProtocol {
    /// The Infer native wire is JSON control frames + raw-binary audio. We model the outbound wire
    /// message as a [`Value`] for control OR a sentinel for a raw binary frame; `serialize` lowers it
    /// to the matching [`OutFrame`]. (Audio out is the raw-binary path — no base64, byte-exact.)
    type Wire = InferWire;

    fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        // A native-S2S session needs a model id (the interaction model to host). No API key is required
        // for a co-located loopback tier (§6.2/§7: same-box; auth, if any, rides the endpoint override).
        let model = if cfg.model.trim().is_empty() {
            return Err(RealtimeError::InvalidConfiguration(
                "Infer S2S requires a model id".to_string(),
            ));
        } else {
            cfg.model.clone()
        };
        Ok(Self {
            model,
            voice: cfg.voice.clone().filter(|v| !v.is_empty()),
        })
    }

    fn provider_id(&self) -> &'static str {
        "waav-infer"
    }

    fn caps(&self) -> ProtocolCaps {
        ProtocolCaps {
            // THE §6.1/§6.4 KEYSTONE: the interaction model owns turn-taking/interruption intrinsically,
            // so the gateway gets out of the way — `emits_user_turn_frames=true` bypasses the cascade
            // Smart-Turn / `TurnController`. (An integration test asserts this — §6.1.)
            emits_user_turn_frames: true,
            output_bytes_per_ms: INFER_S2S_OUTPUT_BYTES_PER_MS,
            output_sample_rate: INFER_S2S_SAMPLE_RATE,
            // A continuous-duplex model has no server-side item truncation — barge-in is simply the
            // model hearing the user (§6.4); the gateway only clears local playback + forwards `clear`.
            supports_truncate: false,
            // No gateway-managed input buffer: raw user audio streams continuously (full-duplex).
            supports_input_buffer: false,
        }
    }

    fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        // GW-13 (UDS half, §6.2/§13): a SERVER-CONFIG `unix://<path>` override selects the in-box
        // sidecar topology (§7 single-box) — the gateway reaches a co-located Infer engine over a UNIX
        // DOMAIN SOCKET instead of loopback WS, removing the loopback-TCP hop. SSRF-safe: the override is
        // server-config only (never client-settable), exactly like the ws override below.
        if let Some(path) = unix_socket_override(cfg.realtime_endpoint_override.as_deref()) {
            return Ok(ConnectSpec::Unix { path });
        }
        // Otherwise loopback native-WS by default (§6.2 D5); a SERVER-CONFIG `ws://` override points at a
        // remote Infer router / sidecar / mock. SSRF-safe (see `apply_endpoint_override`).
        let url = apply_endpoint_override(
            INFER_S2S_URL,
            cfg.realtime_endpoint_override.as_deref(),
            None,
        );
        // A co-located loopback tier needs no auth header; if the override carries an authenticated
        // remote, the gateway's per-deployment header config attaches it (not modeled in the open seam).
        // GW-17: also forward the propagated W3C `traceparent` as a connect header (the standard
        // out-of-band carrier) so a trace-aware proxy / collector on the hop sees it too — the engine's
        // primary read is the `session.config` `trace` field above, but the header keeps the hop W3C-clean.
        let headers = cfg
            .trace
            .as_deref()
            .filter(|t| is_w3c_traceparent(t))
            .map(|tp| vec![("traceparent".to_string(), tp.to_string())])
            .unwrap_or_default();
        Ok(ConnectSpec::WebSocket { url, headers })
    }

    fn build_session_config(
        &self,
        cfg: &RealtimeConfig,
        _resumption: Option<&str>,
    ) -> Vec<Self::Wire> {
        // The single `session.config{task:s2s}` frame, sent on connect (and re-sent on reconnect — the
        // engine re-applies it on a fresh socket).
        vec![InferWire::Control(self.session_config(cfg))]
    }

    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        // FULL-DUPLEX IN: a raw binary frame is the assistant's audio (PCM16, native rate) — carried
        // straight through byte-exact (no base64). This is the assistant-audio half of the duplex.
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

        let value: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            // The engine warn-logs + drops unparseable frames; ignore (nothing actionable).
            Err(_) => return vec![S2sEvent::Ignore],
        };
        let msg_type = value.get("type").and_then(Value::as_str).unwrap_or("");

        match msg_type {
            // Handshake ack → the driver flips `is_ready`.
            "ready" => vec![S2sEvent::SessionReady {
                session_id: value.get("session_id").and_then(Value::as_str).map(str::to_string),
            }],
            // The interaction model transcribes BOTH the user it heard and the assistant it spoke;
            // `role` disambiguates (the gateway routes to `on_transcript` with the matching role). A
            // `chunk_meta` for an audio binary that follows is a control frame the driver also gets —
            // but the AUDIO bytes are the bare binary frame above (the gateway scaffold's bare-binary
            // audio contract), so a `chunk_meta` carries only timing and is not actionable here.
            "transcript" => {
                let role = match value.get("role").and_then(Value::as_str) {
                    Some("user") => TranscriptRole::User,
                    _ => TranscriptRole::Assistant,
                };
                let text = value.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                let is_final = value.get("is_final").and_then(Value::as_bool).unwrap_or(true);
                vec![S2sEvent::Transcript { role, text, is_final, item_id: None }]
            }
            // A VAD edge the model surfaces (the user started/stopped) — drives `on_speech_event`. For a
            // continuous-duplex model this is informational; the model owns the turn either way.
            "speech_started" => vec![S2sEvent::Speech(SpeechEvent::Started {
                audio_start_ms: value.get("audio_start_ms").and_then(Value::as_u64).unwrap_or(0),
                item_id: None,
            })],
            "speech_stopped" => vec![S2sEvent::Speech(SpeechEvent::Stopped {
                audio_end_ms: value.get("audio_end_ms").and_then(Value::as_u64).unwrap_or(0),
                item_id: None,
            })],
            // The model finished THIS response's audio (generation done — NOT a playout/clear signal,
            // per the truncate invariant). Drives `on_response_done`.
            "response_done" => vec![S2sEvent::ResponseDone {
                response_id: value.get("response_id").and_then(Value::as_str).unwrap_or("").to_string(),
            }],
            // `chunk_meta` (audio timing) is informational on the bare-binary gateway path — ignore.
            "chunk_meta" => vec![S2sEvent::Ignore],
            // A typed engine error → `on_error` (GW-3 breaker classification).
            "error" => {
                let msg = value.get("message").and_then(Value::as_str).unwrap_or("Infer S2S error");
                vec![S2sEvent::Error(RealtimeError::ProviderError(msg.to_string()))]
            }
            // Keepalive / lifecycle / unknown ⇒ nothing actionable.
            _ => vec![S2sEvent::Ignore],
        }
    }

    fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
        // FULL-DUPLEX OUT: the user's mic PCM as a raw binary frame, byte-exact (no base64/JSON). This
        // streams continuously — there is no commit / turn boundary (§6.4).
        InferWire::Binary(Bytes::copy_from_slice(pcm))
    }

    fn send_text(&self, text: &str) -> Vec<Self::Wire> {
        // A native-S2S model is audio-first; a text user turn is a `speak`-style inject (best-effort).
        vec![InferWire::Control(json!({ "type": "user_text", "text": text }))]
    }

    fn create_response(&self, _overrides: Option<&RealtimeResponseOverride>) -> Vec<Self::Wire> {
        // The interaction model owns turn-taking — it responds on its own when the user stops. Nothing
        // to send (forcing a response would fight the model's intrinsic turn-taking — §6.4).
        Vec::new()
    }

    fn commit_turn(&self) -> Vec<Self::Wire> {
        // No client-driven turn close — the model owns the turn boundary (§6.4).
        Vec::new()
    }

    fn cancel_response(&self) -> Vec<Self::Wire> {
        // Barge-in / interrupt → an explicit `clear` (drop queued assistant audio). For a continuous
        // model this degrades toward a no-op, but the gateway still forwards it (§6.4).
        vec![InferWire::Control(json!({ "type": "clear" }))]
    }

    fn clear_input_buffer(&self) -> Vec<Self::Wire> {
        // No gateway-managed input buffer (full-duplex streams raw audio); nothing to clear client-side.
        Vec::new()
    }

    fn format_tool_result(&self, _call_id: &str, _result: &str) -> Vec<Self::Wire> {
        // The intrinsic-LLM interaction model has no gateway-mediated tool loop (§6.4 / §10 D2: the LLM
        // is inside the model). Nothing to send.
        Vec::new()
    }

    fn replay_item(&self, _item: &ReplayConversationItem) -> Vec<Self::Wire> {
        // No post-reconnect context replay for a stateless-per-connection interaction model. Empty.
        Vec::new()
    }

    fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
        match msg {
            InferWire::Control(v) => Ok(OutFrame::Text(
                serde_json::to_string(v).map_err(|e| RealtimeError::SerializationError(e.to_string()))?,
            )),
            // The raw-binary audio path: byte-exact, no JSON / no base64.
            InferWire::Binary(b) => Ok(OutFrame::Binary(b.clone())),
        }
    }
}

/// The Infer native outbound wire message: a JSON control frame, OR a raw binary audio frame. (Audio
/// rides byte-exact as a binary frame — the accuracy-at-the-seam invariant; control is JSON.)
#[derive(Debug, Clone, PartialEq)]
pub enum InferWire {
    /// A JSON control frame (`session.config` / `clear` / `user_text`).
    Control(Value),
    /// A raw binary audio frame (user audio OUT), carried byte-exact.
    Binary(Bytes),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "waav-infer".into(),
            model: "moshi".into(),
            voice: Some("af_sky".into()),
            ..Default::default()
        }
    }

    /// Pull the JSON out of a Control wire (panics in-test if binary — a test assertion, not the SUT).
    fn control(w: &InferWire) -> &Value {
        match w {
            InferWire::Control(v) => v,
            InferWire::Binary(_) => panic!("expected a Control frame, got Binary"),
        }
    }

    /// **RED→GREEN `infer_protocol_full_duplex_frames`** (§6.1, §13 GW-9 accept): the native S2S
    /// InferProtocol maps the full-duplex frame contract in BOTH directions.
    /// - OUTBOUND: `connect`→`session.config{task:s2s}` (voice + native rate threaded); the user-audio
    ///   `encode_user_audio` serializes to a byte-exact raw BINARY frame (the OUT half — no base64).
    /// - INBOUND: a raw binary frame maps to an assistant `S2sEvent::Audio` (the IN half, byte-exact);
    ///   `transcript` frames for BOTH roles map distinctly; a `ready` frame is the handshake ack.
    /// - CAPS: `emits_user_turn_frames=true` (the model owns turns — the cascade Smart-Turn is bypassed,
    ///   §6.1), `supports_truncate=false` (continuous-duplex barge-in, §6.4).
    #[test]
    fn infer_protocol_full_duplex_frames() {
        let p = InferProtocol::from_config(&cfg()).expect("a model id ⇒ a valid S2S protocol");
        assert_eq!(p.provider_id(), "waav-infer");

        // ── CAPS: the §6.1/§6.4 keystone — the model owns turns; truncate degrades to clear ──
        let caps = p.caps();
        assert!(caps.emits_user_turn_frames, "S2S model owns turn-taking ⇒ cascade Smart-Turn bypassed (§6.1)");
        assert!(!caps.supports_truncate, "a continuous-duplex model has no server-side truncate (§6.4)");
        assert!(!caps.supports_input_buffer, "full-duplex streams raw audio ⇒ no gateway input buffer");
        assert_eq!(caps.output_sample_rate, INFER_S2S_SAMPLE_RATE);

        // ── OUTBOUND connect → session.config{task:s2s} (the §6.1 handshake) ──
        let frames = p.build_session_config(&cfg(), None);
        assert_eq!(frames.len(), 1, "one session.config on connect");
        let sc = control(&frames[0]);
        assert_eq!(sc["type"], "session.config");
        assert_eq!(sc["task"], "s2s", "the S2S regime (duplex), not stt/tts");
        assert_eq!(sc["model"], "moshi");
        assert_eq!(sc["audio"]["sample_rate"], INFER_S2S_SAMPLE_RATE);
        assert_eq!(sc["conditioning"]["voice"], "af_sky", "the assistant voice survives the hop (§5.4)");
        // it serializes to a Text frame on the wire.
        match p.serialize(&frames[0]).unwrap() {
            OutFrame::Text(t) => assert!(t.contains("\"task\":\"s2s\""), "session.config serializes to JSON text"),
            OutFrame::Binary(_) => panic!("session.config must be a Text frame"),
        }

        // ── OUTBOUND user audio → a byte-exact raw BINARY frame (the full-duplex OUT half) ──
        let user_pcm = [0xde_u8, 0xad, 0xbe, 0xef];
        let wire = p.encode_user_audio(&user_pcm);
        match p.serialize(&wire).unwrap() {
            OutFrame::Binary(b) => assert_eq!(b.as_ref(), &user_pcm, "user audio rides the wire byte-exact, no base64"),
            OutFrame::Text(_) => panic!("user audio must be a raw binary frame, not JSON"),
        }

        // ── INBOUND assistant audio: a raw binary frame → an assistant Audio event (the IN half) ──
        let asst_pcm = [0x00_u8, 0x40, 0x01, 0x80];
        match p.map_server_event(Inbound::Binary(&asst_pcm)).as_slice() {
            [S2sEvent::Audio { data, .. }] => assert_eq!(data.as_ref(), &asst_pcm, "assistant audio in is byte-exact"),
            other => panic!("expected one assistant Audio event, got {other:?}"),
        }

        // ── INBOUND transcripts for BOTH roles map distinctly (the model transcribes user AND assistant) ──
        let user_evt = p.map_server_event(Inbound::Text(
            r#"{"type":"transcript","role":"user","text":"what time is it","is_final":true}"#,
        ));
        match user_evt.as_slice() {
            [S2sEvent::Transcript { role, text, is_final, .. }] => {
                assert_eq!(*role, TranscriptRole::User);
                assert_eq!(text, "what time is it");
                assert!(*is_final);
            }
            other => panic!("expected a user transcript, got {other:?}"),
        }
        let asst_evt = p.map_server_event(Inbound::Text(
            r#"{"type":"transcript","role":"assistant","text":"it is noon","is_final":true}"#,
        ));
        match asst_evt.as_slice() {
            [S2sEvent::Transcript { role, text, .. }] => {
                assert_eq!(*role, TranscriptRole::Assistant, "the assistant transcript is role-tagged distinctly");
                assert_eq!(text, "it is noon");
            }
            other => panic!("expected an assistant transcript, got {other:?}"),
        }

        // ── INBOUND handshake ack → SessionReady (flips is_ready) ──
        match p.map_server_event(Inbound::Text(r#"{"type":"ready","session_id":"s_42"}"#)).as_slice() {
            [S2sEvent::SessionReady { session_id }] => assert_eq!(session_id.as_deref(), Some("s_42")),
            other => panic!("expected SessionReady, got {other:?}"),
        }

        // ── barge-in / cancel → an explicit `clear` control frame (forwarded even for a continuous model) ──
        let cancel = p.cancel_response();
        assert_eq!(cancel.len(), 1);
        assert_eq!(control(&cancel[0])["type"], "clear");
        // the model owns turns ⇒ create_response/commit_turn send NOTHING (don't fight intrinsic turns).
        assert!(p.create_response(None).is_empty(), "the model owns response creation (§6.4)");
        assert!(p.commit_turn().is_empty(), "the model owns the turn boundary (§6.4)");
    }

    /// **`infer_protocol_injects_propagated_traceparent`** (GW-17, the gateway INJECTING half): when the
    /// connection carries a propagated W3C `traceparent`, the Infer-S2S adapter forwards it on BOTH the
    /// `session.config` `trace` field (the engine's primary read → `SessionConfig::trace`) AND a
    /// `traceparent` connect header — so one distributed trace (`trace_id` X) spans the gateway turn and the
    /// intra-Infer stages. An untraced or malformed config injects NEITHER (an untraced handshake is
    /// byte-unchanged; a malformed value can never fail the engine's typed `trace` deserialization).
    #[test]
    fn infer_protocol_injects_propagated_traceparent() {
        const TP: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

        let traced = RealtimeConfig { trace: Some(TP.to_string()), ..cfg() };
        let p = InferProtocol::from_config(&traced).unwrap();

        // (a) session.config carries the traceparent on the `trace` field, with the gateway's trace id X.
        let sc = control(&p.build_session_config(&traced, None)[0]).clone();
        assert_eq!(sc["trace"], serde_json::json!(TP), "session.config.trace is the propagated traceparent");
        assert!(
            sc["trace"].as_str().unwrap().contains(TRACE_ID),
            "the gateway's trace id rides the handshake (one trace spans both halves)"
        );

        // (b) connect_spec carries the standard `traceparent` connect header too (W3C-clean hop).
        match p.connect_spec(&traced).unwrap() {
            ConnectSpec::WebSocket { headers, .. } => assert!(
                headers.iter().any(|(k, v)| k == "traceparent" && v == TP),
                "the traceparent rides a connect header, got {headers:?}"
            ),
            other => panic!("expected a WebSocket spec, got {other:?}"),
        }

        // (c) an untraced config injects NOTHING (byte-unchanged untraced handshake).
        let untraced = InferProtocol::from_config(&cfg()).unwrap();
        assert!(control(&untraced.build_session_config(&cfg(), None)[0]).get("trace").is_none());
        match untraced.connect_spec(&cfg()).unwrap() {
            ConnectSpec::WebSocket { headers, .. } => assert!(headers.is_empty(), "no trace ⇒ no header"),
            other => panic!("expected a WebSocket spec, got {other:?}"),
        }

        // (d) a MALFORMED traceparent is injected NOWHERE (it would fail the engine's typed deserialize).
        let bad = RealtimeConfig { trace: Some("not-a-traceparent".into()), ..cfg() };
        let pbad = InferProtocol::from_config(&bad).unwrap();
        assert!(control(&pbad.build_session_config(&bad, None)[0]).get("trace").is_none());
        match pbad.connect_spec(&bad).unwrap() {
            ConnectSpec::WebSocket { headers, .. } => assert!(headers.is_empty(), "malformed ⇒ no header"),
            other => panic!("expected a WebSocket spec, got {other:?}"),
        }
    }

    /// **`infer_protocol_from_config_requires_model`**: a native-S2S session must name the interaction
    /// model to host — an empty model id is a typed `InvalidConfiguration`, not a panic.
    #[test]
    fn infer_protocol_from_config_requires_model() {
        let bad = RealtimeConfig { model: String::new(), ..cfg() };
        assert!(matches!(
            InferProtocol::from_config(&bad),
            Err(RealtimeError::InvalidConfiguration(_))
        ));
    }

    /// **`infer_protocol_connect_spec_honors_override`**: the loopback default (§6.2 D5) is dialed when
    /// no override is set; a SERVER-CONFIG `ws://` override (proxy / remote router / mock) WINS verbatim;
    /// a non-ws override is ignored (no scheme downgrade — SSRF-safe).
    #[test]
    fn infer_protocol_connect_spec_honors_override() {
        let p = InferProtocol::from_config(&cfg()).unwrap();
        // default → loopback.
        match p.connect_spec(&cfg()).unwrap() {
            ConnectSpec::WebSocket { url, .. } => assert_eq!(url, INFER_S2S_URL),
            other => panic!("expected a WebSocket spec, got {other:?}"),
        }
        // a ws override is used verbatim.
        let mut over = cfg();
        over.realtime_endpoint_override = Some("ws://10.0.0.5:9000/realtime".into());
        match p.connect_spec(&over).unwrap() {
            ConnectSpec::WebSocket { url, .. } => assert_eq!(url, "ws://10.0.0.5:9000/realtime"),
            other => panic!("expected a WebSocket spec, got {other:?}"),
        }
        // a non-ws override is ignored (no downgrade).
        let mut bad = cfg();
        bad.realtime_endpoint_override = Some("http://evil".into());
        match p.connect_spec(&bad).unwrap() {
            ConnectSpec::WebSocket { url, .. } => assert_eq!(url, INFER_S2S_URL, "non-ws override ignored"),
            other => panic!("expected a WebSocket spec, got {other:?}"),
        }
    }

    /// **`infer_protocol_connect_spec_unix_override`** (GW-13 UDS half, §6.2/§13): a SERVER-CONFIG
    /// `unix://<path>` override selects the in-box sidecar topology — `connect_spec` yields a
    /// `ConnectSpec::Unix { path }` carrying the EXACT socket path (so S2S connects over the UDS), while
    /// a blank/non-unix override stays on the ws path (SSRF-safe, no scheme surprise).
    #[test]
    fn infer_protocol_connect_spec_unix_override() {
        let p = InferProtocol::from_config(&cfg()).unwrap();

        // a `unix://` override → ConnectSpec::Unix with the verbatim path.
        let mut uds = cfg();
        uds.realtime_endpoint_override = Some("unix:///run/waav-infer/s2s.sock".into());
        match p.connect_spec(&uds).unwrap() {
            ConnectSpec::Unix { path } => assert_eq!(path, "/run/waav-infer/s2s.sock"),
            other => panic!("expected a Unix spec, got {other:?}"),
        }

        // a blank `unix://` (no path) is malformed ⇒ fall back to the ws loopback default (no downgrade).
        let mut blank = cfg();
        blank.realtime_endpoint_override = Some("unix://".into());
        match p.connect_spec(&blank).unwrap() {
            ConnectSpec::WebSocket { url, .. } => assert_eq!(url, INFER_S2S_URL, "blank unix:// ignored"),
            other => panic!("expected a WebSocket spec, got {other:?}"),
        }

        // the pure SSRF-safe parser: only a `unix://<non-empty>` yields a path.
        assert_eq!(
            unix_socket_override(Some("  unix:///tmp/a.sock  ")).as_deref(),
            Some("/tmp/a.sock"),
            "trims whitespace, strips the scheme"
        );
        assert_eq!(unix_socket_override(Some("ws://x")), None, "ws is not unix");
        assert_eq!(unix_socket_override(Some("unix://")), None, "blank path rejected");
        assert_eq!(unix_socket_override(None), None);
        assert_eq!(unix_socket_override(Some("   ")), None);
    }
}

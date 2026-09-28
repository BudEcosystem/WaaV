//! The translate engine: OpenAI Realtime GA over WaaV's native realtime providers (FRD-023 §5.7,
//! RT7).
//!
//! A deployment whose vendor has no GA surface — Gemini Live, Nova 2 Sonic, and the per-minute
//! voice agents (Deepgram Voice Agent, ElevenLabs Agents, Hume EVI) — is served by the vendor's
//! [`BaseRealtime`] provider (the S2S scaffold) behind this facade. The client still speaks GA;
//! everything around the session is the relay's: authentication, `ek_bud_` secrets, resolution,
//! the admission held for the session, revalidation, idle and maximum-length limits, pings, the
//! drain close, the session span and the policy on client events.
//!
//! **What is translated** (§5.7): `session.update` (voice, instructions, turn detection, tools,
//! input transcription), `input_audio_buffer.{append,commit,clear}` (24 kHz PCM resampled to the
//! vendor's rate), `conversation.item.create` (user text, `function_call_output`),
//! `response.{create,cancel}`. The vendor's events come back as the GA server events a client
//! plays: `response.created` … `response.output_audio.delta` … `response.done`, with a `usage`
//! the gateway computed from the vendor's own report.
//!
//! **What is refused** with `event_not_allowed`, naming the event or field: what the vendor cannot
//! do (`conversation.item.truncate`/`retrieve`/`delete`, `output_audio_buffer.clear`, audio or
//! image content, MCP tools, stored prompts, out-of-band responses) and, once the vendor's session
//! is set up, any CHANGE to a setup-time field (voice, instructions, tools, turn detection): these
//! vendors take them only in their opening message (Gemini's `setup`), so a later change would
//! otherwise be accepted and silently ignored. Re-sending the same value is fine — SDKs resend the
//! whole session on every update.
//!
//! **Setup is deferred** to the client's first `session.update` (or first audio, text or
//! response), so the one opening message the vendor accepts carries the client's configuration as
//! well as the deployment's. The client gets `session.created` at once; `session.updated` when
//! the vendor has accepted the setup.
//!
//! **Metering** (§5.10): a token vendor's usage report (Gemini `usageMetadata`, Nova `usageEvent`)
//! becomes the `usage` of the `response.done` it belongs to and exactly one `voice.turn`; a report
//! outside any response is metered on its own. A per-minute vendor bills 60 s duration segments
//! from the moment its connection opens.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use bud_auth::{RealtimeSettings, VoiceEndpoint};

use crate::core::audio::resampler::{StreamResampler, flush_pcm16, resample_pcm16};
use crate::core::realtime::scaffold::{BedrockBidiTransportFactory, S2sEvent, UsageReport};
use crate::core::realtime::{
    AwsStaticCredentials, BaseRealtime, FunctionDefinition, InputTranscriptionConfig,
    RealtimeConfig, ReconnectionConfig, SpeechEvent, ToolDefinition, TranscriptRole,
    TurnDetectionConfig,
};
use crate::core::realtime_cost::RealtimeUsage;
use crate::middleware::connection_limit::ConnectionSlot;
use crate::state::AppState;

use super::metering::{SegmentClock, SessionMeter, ga_usage};
use super::policy::{self, ClientOutcome, ClientRules};
use super::session::{
    self, CLIENT_QUEUE, End, Engine, Outbound, Prepared, SessionLimits, Timings, gateway_error,
    next_event_id,
};
use super::upstream::{self, UpstreamError};

/// The client's audio format (GA `audio/pcm` is 24 kHz PCM16 mono only).
const GA_RATE: u32 = 24_000;
/// Vendor-bound work queued while the vendor connection is being replaced (a `goAway`, the Nova
/// cap). Past this the vendor is not coming back in useful time.
const MAX_PENDING: usize = 2_048;

/// A vendor served by the translate engine (CONTRACTS C7).
#[derive(Debug)]
pub struct TranslateVendor {
    /// `voice_table.vendor`.
    pub vendor: &'static str,
    /// The provider's name in WaaV's realtime registry.
    provider: &'static str,
    /// Billed by duration, not tokens (its usage is not reported per response).
    pub per_minute: bool,
    /// Announces its session (`S2sEvent::SessionReady`); until it does, nothing else is sent.
    awaits_ready: bool,
}

const VENDORS: &[TranslateVendor] = &[
    TranslateVendor {
        vendor: "gemini",
        provider: "gemini",
        per_minute: false,
        awaits_ready: true,
    },
    TranslateVendor {
        vendor: "nova_sonic",
        provider: "nova_sonic",
        per_minute: false,
        awaits_ready: false,
    },
    TranslateVendor {
        vendor: "deepgram_voice_agent",
        provider: "deepgram",
        per_minute: true,
        awaits_ready: true,
    },
    TranslateVendor {
        vendor: "elevenlabs_convai",
        provider: "elevenlabs",
        per_minute: true,
        awaits_ready: true,
    },
    TranslateVendor {
        vendor: "hume_evi",
        provider: "hume",
        per_minute: true,
        awaits_ready: true,
    },
];

fn vendor_info(vendor: &str) -> Option<&'static TranslateVendor> {
    let v = vendor.trim().to_ascii_lowercase();
    VENDORS.iter().find(|t| t.vendor == v)
}

/// Is this `voice_table.vendor` served by the translate engine?
pub fn is_translate_vendor(vendor: &str) -> bool {
    vendor_info(vendor).is_some()
}

// =============================================================================================
// The plan: everything decided before the upgrade
// =============================================================================================

/// How to reach a translated vendor, decided before the upgrade from the deployment alone.
pub struct TranslatePlan {
    pub info: &'static TranslateVendor,
    /// The provider config: the deployment's credential, model, address and defaults.
    base: RealtimeConfig,
    /// Nova Sonic: the deployment's key pair (never the gateway's AWS identity).
    aws: Option<AwsStaticCredentials>,
    /// Nova Sonic: a Bedrock endpoint other than the region's (`api_base`).
    bedrock_endpoint: Option<String>,
    /// An address from `voice_table` that must clear the SSRF validator.
    ssrf: Option<(String, &'static [&'static str])>,
}

impl std::fmt::Debug for TranslatePlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the config: it carries the vendor key.
        f.debug_struct("TranslatePlan")
            .field("vendor", &self.info.vendor)
            .field("model", &self.base.model)
            .finish()
    }
}

fn nonempty(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// GA `turn_detection` (an object, or `null` for none) in the provider vocabulary.
fn turn_detection_from_ga(v: &Value) -> Option<TurnDetectionConfig> {
    if v.is_null() {
        return Some(TurnDetectionConfig::None);
    }
    serde_json::from_value(v.clone()).ok()
}

/// GA function tools (`{type, name, description, parameters}`) in the provider vocabulary.
fn tools_from_ga(tools: &[Value]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .filter(|t| t.get("type").and_then(Value::as_str) == Some("function"))
        .map(|t| ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: t
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                description: nonempty(t.get("description").and_then(Value::as_str)),
                parameters: t.get("parameters").cloned(),
            },
        })
        .collect()
}

fn tools_to_ga(tools: Option<&Vec<ToolDefinition>>) -> Value {
    Value::Array(
        tools
            .into_iter()
            .flatten()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": t.function.parameters,
                })
            })
            .collect(),
    )
}

impl TranslatePlan {
    /// Build the vendor leg from the deployment (FRD-023 §5.3, CONTRACTS C7). The credential,
    /// model and address come from `voice_table` and nowhere else (D-5).
    pub fn build(
        endpoint: &VoiceEndpoint,
        settings: Option<&RealtimeSettings>,
    ) -> Result<Self, UpstreamError> {
        let info = vendor_info(&endpoint.vendor)
            .ok_or_else(|| UpstreamError::UnsupportedVendor(endpoint.vendor.clone()))?;
        if settings.is_some_and(RealtimeSettings::is_transcription) {
            return Err(UpstreamError::Misconfigured(format!(
                "vendor '{}' serves speech-to-speech sessions only, not transcription",
                info.vendor
            )));
        }

        let mut base = RealtimeConfig {
            provider: info.provider.to_string(),
            model: endpoint
                .model
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string(),
            // Transparent reconnects (a `goAway`, the Nova cap) need the driver's supervisor;
            // a vendor that stays away past these attempts ends the session with 1011.
            reconnection: Some(ReconnectionConfig::default()),
            ..Default::default()
        };
        let mut aws = None;
        let mut bedrock_endpoint = None;
        let mut ssrf = None;

        if info.vendor == "nova_sonic" {
            // SigV4 with the deployment's own key pair (budapp packs the AWS names without the
            // `aws_` prefix) in the deployment's region — refused without either, never falling
            // back to the gateway's AWS identity (RT0, D-5).
            let part = |name: &str| {
                endpoint
                    .credential_parts
                    .as_ref()
                    .and_then(|p| p.get(name))
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            };
            let (Some(access_key_id), Some(secret_access_key)) =
                (part("access_key_id"), part("secret_access_key"))
            else {
                return Err(UpstreamError::Misconfigured(
                    "the credential must be an AWS access key pair; without it the session would \
                     authenticate as the gateway's own AWS identity"
                        .into(),
                ));
            };
            aws = Some(AwsStaticCredentials {
                access_key_id,
                secret_access_key,
                session_token: part("session_token"),
            });
            let region = endpoint.provider_param("region").ok_or_else(|| {
                UpstreamError::Misconfigured(
                    "no AWS region is configured for the deployment".into(),
                )
            })?;
            // The Nova protocol reads its region from the generic `endpoint` slot.
            base.endpoint = Some(region.to_string());
            if let Some(api_base) = nonempty(endpoint.api_base.as_deref()) {
                ssrf = Some((api_base.clone(), &["https", "http"][..]));
                bedrock_endpoint = Some(api_base);
            }
        } else {
            base.api_key =
                nonempty(endpoint.credential.as_deref()).ok_or(UpstreamError::MissingCredential)?;
            if let Some(api_base) = nonempty(endpoint.api_base.as_deref()) {
                let ws = upstream::to_ws_base(&api_base)?;
                ssrf = Some((ws.clone(), &["ws", "wss"][..]));
                // Server-config-only override, now from the deployment (F-5: an `https://` base
                // is converted rather than ignored).
                base.realtime_endpoint_override = Some(ws);
            }
        }

        // The deployment's defaults (§5.6). The client may override them in its first
        // `session.update`, which is folded into the same setup.
        if let Some(d) = settings.and_then(|s| s.defaults.as_ref()) {
            base.voice = nonempty(d.voice.as_deref());
            base.instructions = nonempty(d.instructions.as_deref());
            base.modalities = d.output_modalities.clone();
            base.turn_detection = d.turn_detection.as_ref().and_then(turn_detection_from_ga);
            base.input_audio_transcription = d
                .input_transcription
                .as_ref()
                .and_then(|t| t.model.clone())
                .map(|model| InputTranscriptionConfig { model });
            base.input_audio_noise_reduction = d.noise_reduction.clone();
            base.max_response_output_tokens = d.max_output_tokens.map(|m| m as i32);
        }

        let plan = Self {
            info,
            base,
            aws,
            bedrock_endpoint,
            ssrf,
        };
        // Construct (never connect) once: a deployment the provider cannot be built for — an
        // ElevenLabs entry with no agent id — is refused before it takes a slot.
        drop(plan.provider(plan.base.clone(), None).map_err(|e| {
            UpstreamError::Misconfigured(format!("the vendor provider refused the entry: {e}"))
        })?);
        Ok(plan)
    }

    /// SSRF-validate the one address that is not a vendor constant (blocking DNS, so off the
    /// async workers).
    pub async fn validate(&self) -> Result<(), UpstreamError> {
        let Some((url, schemes)) = self.ssrf.clone() else {
            return Ok(());
        };
        tokio::task::spawn_blocking(move || crate::core::net::validate_url_for_ssrf(&url, schemes))
            .await
            .map_err(|e| UpstreamError::InvalidApiBase(format!("validation task failed: {e}")))?
            .map_err(UpstreamError::InvalidApiBase)
    }

    /// Build (not connect) the provider for `config`.
    fn provider(
        &self,
        config: RealtimeConfig,
        bedrock_http: Option<aws_smithy_runtime_api::client::http::SharedHttpClient>,
    ) -> Result<Box<dyn BaseRealtime>, crate::core::realtime::RealtimeError> {
        if self.info.vendor == "nova_sonic" {
            let Some(keys) = self.aws.clone() else {
                return Err(crate::core::realtime::RealtimeError::AuthenticationFailed(
                    "no deployment key pair".into(),
                ));
            };
            let factory = BedrockBidiTransportFactory::with_credentials(keys)
                .endpoint_url(self.bedrock_endpoint.clone())
                .http_client(bedrock_http);
            return Ok(Box::new(
                crate::core::realtime::NovaSonicRealtime::with_transport(config, factory)?,
            ));
        }
        crate::core::realtime::create_realtime_provider(self.info.provider, config)
    }
}

// =============================================================================================
// The translator: GA events ↔ provider calls and events (pure, no I/O)
// =============================================================================================

/// One thing the session shell must do, in order.
#[derive(Debug, PartialEq)]
pub(super) enum Act {
    /// A GA server event for the client.
    Client(String),
    /// Build the provider from [`Translator::config`] and connect it (the deferred setup).
    Connect,
    /// Revalidate the caller before a `response.create` (D-17).
    Revalidate,
    /// Client audio for the vendor, at the vendor's rate.
    Audio(Bytes),
    Text(String),
    CreateResponse,
    Cancel,
    Commit,
    Clear,
    ToolResult {
        call_id: String,
        output: String,
    },
    /// One billed response.
    Meter {
        response_id: String,
        status: &'static str,
        usage: RealtimeUsage,
        transcript: Option<String>,
    },
}

/// The response in flight.
#[derive(Debug)]
struct Response {
    id: String,
    /// The assistant message item (audio + transcript), once output began.
    item_id: Option<String>,
    output_index: u64,
    /// Output items completed so far (function calls, then the message at the end).
    output: Vec<Value>,
    transcript: String,
    /// Assistant interim text since its last final (see `Translator::assistant_text`).
    interim_since_final: String,
    usage: RealtimeUsage,
    usage_seen: bool,
    /// The client cancelled it: its remaining output is dropped.
    cancelled: bool,
}

/// The user's turn in flight (their speech and its transcription).
#[derive(Debug, Default)]
struct UserTurn {
    item_id: Option<String>,
    transcript: String,
    speaking: bool,
}

pub(super) struct Translator {
    vendor: &'static TranslateVendor,
    /// The deployment name the client connected with (reported as the session's model).
    deployment: String,
    rules: ClientRules,
    /// What the vendor is (or will be) set up with.
    config: RealtimeConfig,
    session_id: String,
    vendor_session_id: Option<String>,
    /// The provider was asked to connect.
    setup: bool,
    /// The vendor accepted the setup.
    ready: bool,
    /// `session.updated` owed once the vendor is ready (the client's event ids).
    owed_updates: Vec<Option<String>>,
    /// Vendor-bound acts held until the vendor is ready.
    held: VecDeque<Act>,
    response: Option<Response>,
    last_response_id: Option<String>,
    last_item_id: Option<String>,
    user: UserTurn,
    input_rate: u32,
    output_rate: u32,
    to_vendor: StreamResampler,
    to_client: StreamResampler,
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_bud_{}", uuid::Uuid::new_v4().simple())
}

/// A GA server event: `type`, a fresh `event_id`, and the body's fields.
fn event(kind: &str, body: Value) -> String {
    let mut m = match body {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.insert("type".into(), Value::from(kind));
    m.insert("event_id".into(), Value::from(next_event_id()));
    Value::Object(m).to_string()
}

macro_rules! ev {
    ($kind:expr, $($body:tt)+) => {
        event($kind, json!($($body)+))
    };
}

/// Decode a vendor audio chunk to PCM16 mono and its rate: raw PCM at the declared rate, or a
/// WAV container (Hume EVI sends one per chunk).
fn pcm_of(data: &[u8], declared_rate: u32) -> (u32, std::borrow::Cow<'_, [u8]>) {
    if data.len() > 44 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        let (mut rate, mut channels, mut bits) = (declared_rate, 1u16, 16u16);
        let mut i = 12;
        while i + 8 <= data.len() {
            let id = &data[i..i + 4];
            let len =
                u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
            let body = i + 8;
            if id == b"fmt " && body + 16 <= data.len() {
                channels = u16::from_le_bytes([data[body + 2], data[body + 3]]);
                rate = u32::from_le_bytes([
                    data[body + 4],
                    data[body + 5],
                    data[body + 6],
                    data[body + 7],
                ]);
                bits = u16::from_le_bytes([data[body + 14], data[body + 15]]);
            } else if id == b"data" {
                let end = (body + len).min(data.len());
                let pcm = &data[body..end];
                if bits != 16 || channels == 0 {
                    return (rate, std::borrow::Cow::Owned(Vec::new()));
                }
                if channels == 1 {
                    return (rate, std::borrow::Cow::Borrowed(pcm));
                }
                // Down-mix to mono: the first channel of every frame.
                let frame = 2 * channels as usize;
                let mono: Vec<u8> = pcm.chunks_exact(frame).flat_map(|f| [f[0], f[1]]).collect();
                return (rate, std::borrow::Cow::Owned(mono));
            }
            i = body + len + (len & 1);
        }
    }
    (declared_rate, std::borrow::Cow::Borrowed(data))
}

impl Translator {
    pub(super) fn new(
        vendor: &'static TranslateVendor,
        deployment: String,
        rules: ClientRules,
        config: RealtimeConfig,
        session_id: &str,
    ) -> Self {
        Self {
            vendor,
            deployment,
            rules,
            config,
            session_id: session_id.to_string(),
            vendor_session_id: None,
            setup: false,
            ready: false,
            owed_updates: Vec::new(),
            held: VecDeque::new(),
            response: None,
            last_response_id: None,
            last_item_id: None,
            user: UserTurn::default(),
            input_rate: GA_RATE,
            output_rate: GA_RATE,
            to_vendor: StreamResampler::new(),
            to_client: StreamResampler::new(),
        }
    }

    /// The provider config the deferred setup uses.
    pub(super) fn config(&self) -> RealtimeConfig {
        self.config.clone()
    }

    pub(super) fn vendor_session_id(&self) -> Option<String> {
        self.vendor_session_id.clone()
    }

    /// The GA session object the client is told about.
    fn session_object(&self) -> Value {
        let c = &self.config;
        let format = json!({"type": "audio/pcm", "rate": GA_RATE});
        json!({
            "object": "realtime.session",
            "type": "realtime",
            "id": self.session_id,
            "model": self.deployment,
            "output_modalities": c.modalities.clone().unwrap_or_else(|| vec!["audio".into()]),
            "instructions": c.instructions,
            "tools": tools_to_ga(c.tools.as_ref()),
            "audio": {
                "input": {
                    "format": format,
                    "turn_detection": c.turn_detection.as_ref().map(|t| match t {
                        TurnDetectionConfig::None => Value::Null,
                        other => serde_json::to_value(other).unwrap_or(Value::Null),
                    }),
                    "transcription": c.input_audio_transcription.as_ref().map(|t| json!({"model": t.model})),
                },
                "output": {"format": format, "voice": c.voice},
            },
        })
    }

    pub(super) fn session_created(&self) -> String {
        ev!("session.created", {"session": self.session_object()})
    }

    fn session_updated(&self) -> String {
        ev!("session.updated", {"session": self.session_object()})
    }

    fn refuse(param: &str, message: impl Into<String>, event_id: Option<&str>) -> Act {
        Act::Client(gateway_error(
            "event_not_allowed",
            &message.into(),
            Some(param),
            event_id,
        ))
    }

    fn untranslatable(&self, what: &str, event_id: Option<&str>) -> Act {
        Self::refuse(
            what,
            format!(
                "`{what}` cannot be served on this deployment: its vendor ({}) has no equivalent.",
                self.vendor.vendor
            ),
            event_id,
        )
    }

    /// Queue vendor-bound work: held until the vendor is ready, and preceded by the deferred
    /// setup when the session has none yet.
    fn queue_for_vendor(&mut self, out: &mut Vec<Act>, act: Act) {
        if !self.setup {
            self.setup = true;
            out.push(Act::Connect);
        }
        if self.ready {
            out.push(act);
        } else if self.held.len() < MAX_PENDING {
            self.held.push_back(act);
        }
    }

    // ----------------------------------------------------------------------------------------
    // Client → vendor
    // ----------------------------------------------------------------------------------------

    pub(super) fn client(&mut self, raw: &str) -> Vec<Act> {
        let mut out = Vec::new();
        let (text, kind) = match policy::client_event(raw, &self.rules) {
            ClientOutcome::Invalid(why) => {
                out.push(Act::Client(gateway_error(
                    "invalid_event",
                    &format!("The event could not be read: {why}"),
                    None,
                    None,
                )));
                return out;
            }
            ClientOutcome::Refuse(r) => {
                out.push(Self::refuse(&r.param, r.message, r.event_id.as_deref()));
                return out;
            }
            ClientOutcome::Forward { text, kind, .. } => (text.into_owned(), kind),
        };
        let Ok(event) = serde_json::from_str::<Value>(&text) else {
            return out;
        };
        let event_id = event.get("event_id").and_then(Value::as_str);
        match kind.as_str() {
            "session.update" => self.session_update(&event, event_id, &mut out),
            "input_audio_buffer.append" => {
                match event
                    .get("audio")
                    .and_then(Value::as_str)
                    .map(|a| BASE64_STANDARD.decode(a))
                {
                    Some(Ok(pcm)) if pcm.len() % 2 == 0 => {
                        self.queue_for_vendor(&mut out, Act::Audio(Bytes::from(pcm)))
                    }
                    _ => out.push(Act::Client(gateway_error(
                        "invalid_event",
                        "`audio` must be base64 PCM16 (audio/pcm, 24 kHz).",
                        Some("audio"),
                        event_id,
                    ))),
                }
            }
            "input_audio_buffer.commit" => {
                self.queue_for_vendor(&mut out, Act::Commit);
                let item = self.user_item(&mut out);
                out.push(Act::Client(ev!("input_audio_buffer.committed", {
                    "previous_item_id": Value::Null, "item_id": item
                })));
            }
            "input_audio_buffer.clear" => {
                self.queue_for_vendor(&mut out, Act::Clear);
                out.push(Act::Client(ev!("input_audio_buffer.cleared", {})));
            }
            "conversation.item.create" => self.item_create(&event, event_id, &mut out),
            "response.create" => self.response_create(&event, event_id, &mut out),
            "response.cancel" => {
                if self.response.is_some() {
                    self.queue_for_vendor(&mut out, Act::Cancel);
                    self.finish_response("cancelled", &mut out);
                }
            }
            other => out.push(self.untranslatable(other, event_id)),
        }
        out
    }

    fn session_update(&mut self, event: &Value, event_id: Option<&str>, out: &mut Vec<Act>) {
        let empty = Map::new();
        let s = event
            .get("session")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        // Untranslatable whatever the deployment allows (CONTRACTS C7).
        if s.get("prompt").is_some_and(|p| !p.is_null()) {
            out.push(self.untranslatable("session.prompt", event_id));
            return;
        }
        let tools = s.get("tools").and_then(Value::as_array);
        if tools.is_some_and(|t| {
            t.iter()
                .any(|t| t.get("type").and_then(Value::as_str) != Some("function"))
        }) {
            out.push(self.untranslatable("session.tools.mcp", event_id));
            return;
        }
        let input = s.get("audio").and_then(|a| a.get("input"));
        let output = s.get("audio").and_then(|a| a.get("output"));
        for (fmt, param) in [
            (
                input.and_then(|i| i.get("format")),
                "session.audio.input.format",
            ),
            (
                output.and_then(|o| o.get("format")),
                "session.audio.output.format",
            ),
        ] {
            if let Some(f) = fmt {
                let pcm = f.get("type").and_then(Value::as_str) == Some("audio/pcm")
                    && f.get("rate")
                        .is_none_or(|r| r.as_u64() == Some(GA_RATE as u64));
                if !pcm {
                    out.push(Self::refuse(
                        param,
                        "This deployment's vendor is served in audio/pcm at 24 kHz only.",
                        event_id,
                    ));
                    return;
                }
            }
        }

        let voice = output
            .and_then(|o| o.get("voice"))
            .or_else(|| s.get("voice"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let instructions = s
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::to_string);
        let tools = tools.map(|t| tools_from_ga(t));
        let turn_detection = match input.and_then(|i| i.get("turn_detection")) {
            None => None,
            Some(v) => match turn_detection_from_ga(v) {
                Some(td) => Some(td),
                None => {
                    out.push(Self::refuse(
                        "session.audio.input.turn_detection",
                        "`turn_detection` is not a turn-detection object.",
                        event_id,
                    ));
                    return;
                }
            },
        };
        let modalities: Option<Vec<String>> = s
            .get("output_modalities")
            .and_then(|m| serde_json::from_value(m.clone()).ok());

        if self.setup {
            // Setup-time only: a CHANGE after the vendor's opening message is refused rather
            // than accepted and ignored. The same value again is fine.
            let changed = |a: Option<Value>, b: Value| a.is_some_and(|a| a != b);
            let as_value = |v: &Option<String>| serde_json::to_value(v).unwrap_or(Value::Null);
            let checks = [
                (
                    "session.audio.output.voice",
                    changed(
                        voice.as_ref().map(|v| json!(v)),
                        as_value(&self.config.voice),
                    ),
                ),
                (
                    "session.instructions",
                    changed(
                        instructions.as_ref().map(|v| json!(v)),
                        as_value(&self.config.instructions),
                    ),
                ),
                (
                    "session.tools",
                    changed(
                        tools.as_ref().map(|t| tools_to_ga(Some(t))),
                        tools_to_ga(self.config.tools.as_ref()),
                    ),
                ),
                (
                    "session.audio.input.turn_detection",
                    changed(
                        turn_detection
                            .as_ref()
                            .map(|t| serde_json::to_value(t).unwrap_or(Value::Null)),
                        serde_json::to_value(&self.config.turn_detection).unwrap_or(Value::Null),
                    ),
                ),
            ];
            if let Some((param, _)) = checks.iter().find(|(_, changed)| *changed) {
                out.push(Self::refuse(
                    param,
                    format!(
                        "`{param}` is fixed when the session starts on this deployment's vendor \
                         ({}); it cannot be changed mid-session.",
                        self.vendor.vendor
                    ),
                    event_id,
                ));
                return;
            }
            out.push(Act::Client(self.session_updated()));
            return;
        }

        // Before setup: fold the client's choices into the one opening message.
        if voice.is_some() {
            self.config.voice = voice;
        }
        if instructions.is_some() {
            self.config.instructions = instructions;
        }
        if let Some(t) = tools {
            self.config.tools = (!t.is_empty()).then_some(t);
        }
        if turn_detection.is_some() {
            self.config.turn_detection = turn_detection;
        }
        if let Some(t) = input.and_then(|i| i.get("transcription")) {
            self.config.input_audio_transcription = t
                .get("model")
                .and_then(Value::as_str)
                .map(|model| InputTranscriptionConfig {
                    model: model.to_string(),
                })
                .or_else(|| {
                    (!t.is_null()).then(|| InputTranscriptionConfig {
                        model: String::new(),
                    })
                });
        }
        if modalities.is_some() {
            self.config.modalities = modalities;
        }
        if let Some(m) = s.get("max_output_tokens").and_then(Value::as_u64) {
            self.config.max_response_output_tokens = Some(m.min(i32::MAX as u64) as i32);
        }
        self.setup = true;
        out.push(Act::Connect);
        self.owed_updates.push(event_id.map(str::to_string));
    }

    fn item_create(&mut self, event: &Value, event_id: Option<&str>, out: &mut Vec<Act>) {
        let Some(item) = event.get("item") else {
            out.push(Self::refuse("item", "`item` is required.", event_id));
            return;
        };
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| new_id("item"));
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if item.get("role").and_then(Value::as_str) != Some("user") {
                    out.push(self.untranslatable("item.role", event_id));
                    return;
                }
                let mut text = String::new();
                for part in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match part.get("type").and_then(Value::as_str) {
                        Some("input_text") | Some("text") => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(part.get("text").and_then(Value::as_str).unwrap_or(""));
                        }
                        Some(other) => {
                            out.push(
                                self.untranslatable(&format!("item.content.{other}"), event_id),
                            );
                            return;
                        }
                        None => {}
                    }
                }
                self.queue_for_vendor(out, Act::Text(text));
            }
            Some("function_call_output") => {
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let output = item
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.queue_for_vendor(out, Act::ToolResult { call_id, output });
            }
            other => {
                out.push(self.untranslatable(
                    &format!("item.type.{}", other.unwrap_or("missing")),
                    event_id,
                ));
                return;
            }
        }
        let mut item = item.clone();
        item["id"] = Value::from(item_id.clone());
        item["object"] = Value::from("realtime.item");
        item["status"] = Value::from("completed");
        out.push(Act::Client(ev!("conversation.item.added", {
            "previous_item_id": self.last_item_id, "item": item
        })));
        out.push(Act::Client(ev!("conversation.item.done", {
            "previous_item_id": self.last_item_id, "item": item
        })));
        self.last_item_id = Some(item_id);
    }

    fn response_create(&mut self, event: &Value, event_id: Option<&str>, out: &mut Vec<Act>) {
        if let Some(r) = event.get("response").and_then(Value::as_object) {
            if r.get("prompt").is_some_and(|p| !p.is_null()) {
                out.push(self.untranslatable("response.prompt", event_id));
                return;
            }
            if r.get("conversation").and_then(Value::as_str) == Some("none") {
                out.push(self.untranslatable("response.conversation", event_id));
                return;
            }
            if r.get("input").is_some() {
                out.push(self.untranslatable("response.input", event_id));
                return;
            }
            // Per-response overrides the vendor would ignore.
            let differs = |key: &str, current: Value| {
                r.get(key).is_some_and(|v| !v.is_null() && *v != current)
            };
            if differs(
                "instructions",
                serde_json::to_value(&self.config.instructions).unwrap_or(Value::Null),
            ) {
                out.push(self.untranslatable("response.instructions", event_id));
                return;
            }
            if r.get("tools")
                .is_some_and(|t| *t != tools_to_ga(self.config.tools.as_ref()))
            {
                out.push(self.untranslatable("response.tools", event_id));
                return;
            }
            let voice = r
                .get("audio")
                .and_then(|a| a.get("output"))
                .and_then(|o| o.get("voice"))
                .or_else(|| r.get("voice"));
            if voice.is_some_and(|v| {
                *v != serde_json::to_value(&self.config.voice).unwrap_or(Value::Null)
            }) {
                out.push(self.untranslatable("response.audio.output.voice", event_id));
                return;
            }
        }
        out.push(Act::Revalidate);
        self.queue_for_vendor(out, Act::CreateResponse);
    }

    // ----------------------------------------------------------------------------------------
    // The vendor's side
    // ----------------------------------------------------------------------------------------

    /// The provider is connected (the deferred setup went out); `rates` are its PCM rates.
    pub(super) fn connected(&mut self, rates: Option<(u32, u32)>) -> Vec<Act> {
        if let Some((input, output)) = rates {
            self.input_rate = input;
            self.output_rate = output;
        }
        if self.vendor.awaits_ready {
            Vec::new()
        } else {
            self.became_ready()
        }
    }

    pub(super) fn awaits_ready(&self) -> bool {
        self.vendor.awaits_ready && !self.ready
    }

    fn became_ready(&mut self) -> Vec<Act> {
        let mut out = Vec::new();
        if self.ready {
            return out;
        }
        self.ready = true;
        for _ in std::mem::take(&mut self.owed_updates) {
            out.push(Act::Client(self.session_updated()));
        }
        out.extend(self.held.drain(..));
        out
    }

    /// 24 kHz client PCM → the vendor's rate (streaming: the filter state carries across chunks).
    pub(super) fn audio_for_vendor(&mut self, pcm: &[u8]) -> Bytes {
        match resample_pcm16(&mut self.to_vendor, pcm, GA_RATE, self.input_rate) {
            Some(v) => Bytes::from(v),
            None => Bytes::copy_from_slice(pcm),
        }
    }

    /// The resampler's buffered tail at a turn boundary (a commit).
    pub(super) fn audio_tail_for_vendor(&mut self) -> Option<Bytes> {
        flush_pcm16(&mut self.to_vendor)
            .filter(|t| !t.is_empty())
            .map(Bytes::from)
    }

    fn user_item(&mut self, out: &mut Vec<Act>) -> String {
        if let Some(id) = &self.user.item_id {
            return id.clone();
        }
        let id = new_id("item");
        out.push(Act::Client(ev!("conversation.item.added", {
            "previous_item_id": self.last_item_id,
            "item": {"id": id, "object": "realtime.item", "type": "message", "role": "user",
                     "status": "in_progress", "content": [{"type": "input_audio", "transcript": Value::Null}]}
        })));
        self.last_item_id = Some(id.clone());
        self.user.item_id = Some(id.clone());
        id
    }

    /// Close the user's transcription (a vendor that never marks it final: at the response).
    fn complete_user(&mut self, out: &mut Vec<Act>, final_text: Option<String>) {
        let Some(id) = self.user.item_id.take() else {
            return;
        };
        let transcript = final_text
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| std::mem::take(&mut self.user.transcript));
        self.user.transcript.clear();
        out.push(Act::Client(
            ev!("conversation.item.input_audio_transcription.completed", {
                "item_id": id, "content_index": 0, "transcript": transcript
            }),
        ));
    }

    fn start_response(&mut self, out: &mut Vec<Act>) {
        if self.response.is_some() {
            return;
        }
        // The user spoke before this answer: their transcription is done.
        let pending_user = self.user.item_id.is_some();
        if pending_user && !self.user.transcript.is_empty() {
            self.complete_user(out, None);
        }
        let id = new_id("resp");
        out.push(Act::Client(ev!("response.created", {
            "response": {"id": id, "object": "realtime.response", "status": "in_progress",
                         "output": [], "usage": Value::Null}
        })));
        self.response = Some(Response {
            id,
            item_id: None,
            output_index: 0,
            output: Vec::new(),
            transcript: String::new(),
            interim_since_final: String::new(),
            usage: RealtimeUsage::default(),
            usage_seen: false,
            cancelled: false,
        });
    }

    /// The assistant message item of the response in flight, announced on first output.
    fn message_item(
        &mut self,
        out: &mut Vec<Act>,
        vendor_id: Option<&str>,
    ) -> Option<(String, String)> {
        self.start_response(out);
        let last_item = self.last_item_id.clone();
        let r = self.response.as_mut()?;
        if r.cancelled {
            return None;
        }
        if let Some(id) = &r.item_id {
            return Some((r.id.clone(), id.clone()));
        }
        let item_id = vendor_id
            .map(str::to_string)
            .unwrap_or_else(|| new_id("item"));
        r.item_id = Some(item_id.clone());
        let item = json!({"id": item_id, "object": "realtime.item", "type": "message",
            "role": "assistant", "status": "in_progress", "content": []});
        out.push(Act::Client(ev!("response.output_item.added", {
            "response_id": r.id, "output_index": r.output_index, "item": item
        })));
        out.push(Act::Client(ev!("conversation.item.added", {
            "previous_item_id": last_item, "item": item
        })));
        out.push(Act::Client(ev!("response.content_part.added", {
            "response_id": r.id, "item_id": item_id, "output_index": r.output_index,
            "content_index": 0, "part": {"type": "audio", "transcript": ""}
        })));
        let rid = r.id.clone();
        self.last_item_id = Some(item_id.clone());
        Some((rid, item_id))
    }

    /// Finish the response in flight: its item events, `response.done` with the gateway's
    /// `usage`, and — when the vendor reported usage — its one billed record.
    fn finish_response(&mut self, status: &'static str, out: &mut Vec<Act>) {
        let Some(mut r) = self.response.take() else {
            return;
        };
        let status = if r.cancelled { "cancelled" } else { status };
        if let Some(item_id) = r.item_id.clone() {
            if let Some(tail) = flush_pcm16(&mut self.to_client).filter(|t| !t.is_empty()) {
                if !r.cancelled {
                    out.push(Act::Client(ev!("response.output_audio.delta", {
                        "response_id": r.id, "item_id": item_id, "output_index": r.output_index,
                        "content_index": 0, "delta": BASE64_STANDARD.encode(tail)
                    })));
                }
            }
            let item_status = if status == "completed" {
                "completed"
            } else {
                "incomplete"
            };
            let item = json!({"id": item_id, "object": "realtime.item", "type": "message",
                "role": "assistant", "status": item_status,
                "content": [{"type": "output_audio", "transcript": r.transcript}]});
            for (kind, extra) in [
                ("response.output_audio.done", json!({})),
                (
                    "response.output_audio_transcript.done",
                    json!({"transcript": r.transcript}),
                ),
                (
                    "response.content_part.done",
                    json!({"part": {"type": "audio", "transcript": r.transcript}}),
                ),
            ] {
                let mut m = Map::new();
                m.insert("response_id".into(), json!(r.id));
                m.insert("item_id".into(), json!(item_id));
                m.insert("output_index".into(), json!(r.output_index));
                m.insert("content_index".into(), json!(0));
                if let Value::Object(x) = extra {
                    m.extend(x);
                }
                out.push(Act::Client(event(kind, Value::Object(m))));
            }
            out.push(Act::Client(ev!("response.output_item.done", {
                "response_id": r.id, "output_index": r.output_index, "item": item
            })));
            out.push(Act::Client(ev!("conversation.item.done", {"item": item})));
            r.output.push(item);
        }
        let usage = r.usage_seen.then(|| ga_usage(&r.usage));
        out.push(Act::Client(ev!("response.done", {
            "response": {"id": r.id, "object": "realtime.response", "status": status,
                         "output": r.output, "usage": usage}
        })));
        if r.usage_seen {
            out.push(Act::Meter {
                response_id: r.id.clone(),
                status,
                usage: r.usage,
                transcript: (!r.transcript.is_empty()).then_some(r.transcript.clone()),
            });
        }
        self.last_response_id = Some(r.id);
    }

    /// Assistant text. An interim chunk is a delta. A FINAL one is either the next delta
    /// (Gemini marks its last chunk final) or a restatement of the interim text already sent
    /// (Nova Sonic's FINAL block after its SPECULATIVE one) — only its unsent part is new.
    fn assistant_text(&mut self, text: &str, is_final: bool, out: &mut Vec<Act>) {
        let Some((rid, item_id)) = self.message_item(out, None) else {
            return;
        };
        let Some(r) = self.response.as_mut() else {
            return;
        };
        let delta = if !is_final {
            r.interim_since_final.push_str(text);
            text.to_string()
        } else {
            let seen = std::mem::take(&mut r.interim_since_final);
            if seen.is_empty() {
                text.to_string()
            } else if let Some(rest) = text.strip_prefix(seen.as_str()) {
                rest.to_string()
            } else if seen.starts_with(text) {
                String::new()
            } else {
                text.to_string()
            }
        };
        if delta.is_empty() {
            return;
        }
        r.transcript.push_str(&delta);
        out.push(Act::Client(ev!("response.output_audio_transcript.delta", {
            "response_id": rid, "item_id": item_id, "output_index": r.output_index,
            "content_index": 0, "delta": delta
        })));
    }

    fn user_text(&mut self, text: &str, is_final: bool, out: &mut Vec<Act>) {
        let item_id = self.user_item(out);
        if is_final {
            self.complete_user(out, Some(text.to_string()));
            return;
        }
        self.user.transcript.push_str(text);
        out.push(Act::Client(
            ev!("conversation.item.input_audio_transcription.delta", {
                "item_id": item_id, "content_index": 0, "delta": text
            }),
        ));
    }

    fn usage(&mut self, report: UsageReport, out: &mut Vec<Act>) {
        match self.response.as_mut() {
            Some(r) => {
                if report.cumulative && r.usage_seen {
                    r.usage = report.tokens;
                } else {
                    r.usage.add(&report.tokens);
                }
                r.usage_seen = true;
            }
            // Outside any response (a report after its `response.done`, or between turns):
            // billed on its own, once.
            None => out.push(Act::Meter {
                response_id: self
                    .last_response_id
                    .clone()
                    .unwrap_or_else(|| new_id("resp")),
                status: "completed",
                usage: report.tokens,
                transcript: None,
            }),
        }
    }

    pub(super) fn vendor(&mut self, ev: S2sEvent) -> Vec<Act> {
        let mut out = Vec::new();
        match ev {
            S2sEvent::SessionReady { session_id } => {
                if session_id.is_some() && self.vendor_session_id.is_none() {
                    self.vendor_session_id = session_id;
                }
                out.extend(self.became_ready());
            }
            S2sEvent::Speech(SpeechEvent::Started { audio_start_ms, .. }) => {
                self.user.speaking = true;
                let item = self.user_item(&mut out);
                out.push(Act::Client(ev!("input_audio_buffer.speech_started", {
                    "audio_start_ms": audio_start_ms, "item_id": item
                })));
            }
            S2sEvent::Speech(SpeechEvent::Stopped { audio_end_ms, .. }) => {
                self.user.speaking = false;
                let item = self.user_item(&mut out);
                out.push(Act::Client(ev!("input_audio_buffer.speech_stopped", {
                    "audio_end_ms": audio_end_ms, "item_id": item
                })));
                out.push(Act::Client(ev!("input_audio_buffer.committed", {
                    "previous_item_id": Value::Null, "item_id": item
                })));
            }
            S2sEvent::Transcript {
                role: TranscriptRole::User,
                text,
                is_final,
                ..
            } => self.user_text(&text, is_final, &mut out),
            S2sEvent::Transcript {
                role: TranscriptRole::Assistant,
                text,
                is_final,
                ..
            } => self.assistant_text(&text, is_final, &mut out),
            S2sEvent::Audio { data, item_id, .. } => {
                let (rate, pcm) = pcm_of(&data, self.output_rate);
                let pcm = match resample_pcm16(&mut self.to_client, &pcm, rate, GA_RATE) {
                    Some(v) => Bytes::from(v),
                    None => Bytes::copy_from_slice(&pcm),
                };
                if let Some((rid, iid)) = self.message_item(&mut out, item_id.as_deref())
                    && !pcm.is_empty()
                    && let Some(r) = self.response.as_ref()
                {
                    out.push(Act::Client(ev!("response.output_audio.delta", {
                        "response_id": rid, "item_id": iid, "output_index": r.output_index,
                        "content_index": 0, "delta": BASE64_STANDARD.encode(&pcm)
                    })));
                }
            }
            S2sEvent::ItemAdded {
                item_id,
                role: TranscriptRole::Assistant,
            } => {
                let _ = self.message_item(&mut out, Some(&item_id));
            }
            S2sEvent::FunctionCall(call) => {
                self.start_response(&mut out);
                let last_item = self.last_item_id.clone();
                if let Some(r) = self.response.as_mut()
                    && !r.cancelled
                {
                    if r.item_id.is_some() {
                        r.output_index += 1;
                    }
                    let item_id = call.item_id.clone().unwrap_or_else(|| new_id("item"));
                    let item = json!({"id": item_id, "object": "realtime.item", "type": "function_call",
                        "status": "completed", "call_id": call.call_id, "name": call.name,
                        "arguments": call.arguments});
                    out.push(Act::Client(ev!("response.output_item.added", {
                        "response_id": r.id, "output_index": r.output_index, "item": item
                    })));
                    out.push(Act::Client(ev!("conversation.item.added", {
                        "previous_item_id": last_item, "item": item
                    })));
                    out.push(Act::Client(ev!("response.function_call_arguments.done", {
                        "response_id": r.id, "item_id": item_id, "output_index": r.output_index,
                        "call_id": call.call_id, "name": call.name, "arguments": call.arguments
                    })));
                    out.push(Act::Client(ev!("response.output_item.done", {
                        "response_id": r.id, "output_index": r.output_index, "item": item
                    })));
                    out.push(Act::Client(ev!("conversation.item.done", {"item": item})));
                    r.output.push(item);
                    self.last_item_id = Some(item_id);
                }
                // A GA response ends at its function call: the client runs the tool and asks
                // for the next response.
                self.finish_response("completed", &mut out);
            }
            S2sEvent::ResponseDone { .. } => {
                if self.response.is_some() {
                    self.finish_response("completed", &mut out);
                }
                if self.user.item_id.is_some() && !self.user.transcript.is_empty() {
                    self.complete_user(&mut out, None);
                }
            }
            S2sEvent::InterruptedByServer => {
                // Barge-in: the vendor stopped its answer. A GA client stops playback on
                // `speech_started`, so say so if the vendor did not.
                if !self.user.speaking {
                    let item = self.user_item(&mut out);
                    out.push(Act::Client(ev!("input_audio_buffer.speech_started", {
                        "audio_start_ms": 0, "item_id": item
                    })));
                }
                self.finish_response("cancelled", &mut out);
            }
            S2sEvent::Usage(report) => self.usage(report, &mut out),
            S2sEvent::Error(e) => {
                out.push(Act::Client(gateway_error(
                    "vendor_error",
                    &format!("The vendor reported an error: {e}"),
                    None,
                    None,
                )));
            }
            // Driver-internal, or nothing a GA client is told about.
            S2sEvent::TrackPendingCall { .. }
            | S2sEvent::ItemAdded { .. }
            | S2sEvent::ItemDone { .. }
            | S2sEvent::ResumptionHandle(_)
            | S2sEvent::GoAway { .. }
            | S2sEvent::SendFrame(_)
            | S2sEvent::Ignore => {}
        }
        out
    }

    /// The session is ending: bill what the response in flight already reported.
    pub(super) fn close(&mut self) -> Vec<Act> {
        let mut out = Vec::new();
        if let Some(r) = self.response.take()
            && r.usage_seen
        {
            out.push(Act::Meter {
                response_id: r.id,
                status: "incomplete",
                usage: r.usage,
                transcript: (!r.transcript.is_empty()).then_some(r.transcript),
            });
        }
        out
    }
}

// =============================================================================================
// The session shell: the relay's lifecycle around the translator
// =============================================================================================

/// What the provider's callbacks send the session.
enum VendorMsg {
    Event(S2sEvent),
    Reconnected(bool),
}

struct Shell<'a> {
    state: &'a AppState,
    p: &'a Prepared,
    plan: &'a TranslatePlan,
    tr: Translator,
    meter: SessionMeter,
    client_tx: mpsc::Sender<Outbound>,
    timings: Timings,
    provider: Option<Box<dyn BaseRealtime>>,
    vendor_tx: mpsc::UnboundedSender<VendorMsg>,
    /// Vendor-bound work waiting out a reconnect.
    pending: VecDeque<Act>,
    /// Set when the vendor connected and must still say it is ready.
    ready_deadline: Option<Instant>,
    segments: Option<SegmentClock>,
    last_activity: Instant,
    client_missed: u32,
}

impl Shell<'_> {
    async fn to_client(&self, text: String) -> Result<(), End> {
        match tokio::time::timeout(
            self.timings.slow_client,
            self.client_tx
                .send(Outbound::Frame(Message::Text(text.into()))),
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(End::new("client_close", 1006)),
            Err(_) => Err(End::new("client_too_slow", 1011).with_error(
                "client_too_slow",
                "The client did not read the session's output fast enough; audio is never dropped \
                 silently, so the session is closed.",
            )),
        }
    }

    async fn revalidate(&self) -> Result<(), End> {
        if session::still_allowed(self.state, &self.p.caller.check, &self.p.endpoint_id)
            .await
            .is_some()
        {
            return Ok(());
        }
        info!(endpoint_id = %self.p.endpoint_id, "realtime session revoked");
        Err(End::new("revoked", 1008).with_error(
            "session_revoked",
            "The credential or the deployment was revoked; the session is closed.",
        ))
    }

    fn upstream_error(message: impl Into<String>) -> End {
        End::new("upstream_error", 1011).with_error("upstream_error", message)
    }

    /// Build the provider from the translator's config, wire its event tap, and connect.
    async fn connect(&mut self) -> Result<(), End> {
        if self.provider.is_some() {
            return Ok(());
        }
        let bedrock_http = self.state.realtime.bedrock_http_client.clone();
        let mut config = self.tr.config();
        config.max_connection = self.timings.connection_cap;
        let mut provider = self.plan.provider(config, bedrock_http).map_err(|e| {
            Self::upstream_error(format!("The vendor session could not be built: {e}"))
        })?;
        let tx = self.vendor_tx.clone();
        provider
            .on_event(Arc::new(move |ev| {
                let _ = tx.send(VendorMsg::Event(ev));
                Box::pin(async {})
            }))
            .map_err(|e| Self::upstream_error(e.to_string()))?;
        let tx = self.vendor_tx.clone();
        provider
            .on_reconnection(Arc::new(move |ev| {
                let _ = tx.send(VendorMsg::Reconnected(ev.success));
                Box::pin(async {})
            }))
            .map_err(|e| Self::upstream_error(e.to_string()))?;
        let connected = tokio::time::timeout(self.timings.connect, provider.connect()).await;
        let vkey = &self.p.vkey;
        match connected {
            Ok(Ok(())) => {
                if let Some(pol) = &self.state.policies {
                    pol.breakers().record_success(&self.p.endpoint_id, vkey);
                }
            }
            other => {
                let why = match other {
                    Ok(Err(e)) => e.to_string(),
                    _ => "the vendor did not answer within the connect deadline".to_string(),
                };
                warn!(endpoint_id = %self.p.endpoint_id, vendor = self.plan.info.vendor, "translated vendor connect failed");
                if let Some(pol) = &self.state.policies {
                    let verdict = crate::core::deployment_policy::classify_message(&why, None);
                    pol.breakers()
                        .record_failure(&self.p.endpoint_id, vkey, &verdict);
                }
                let _ = provider.disconnect().await;
                return Err(Self::upstream_error(format!(
                    "Could not connect to the vendor: {why}"
                )));
            }
        }
        let rates = provider.audio_rates();
        self.provider = Some(provider);
        let now = Instant::now();
        if self.plan.info.per_minute
            && crate::core::realtime_cost::bills_duration(self.p.endpoint.pricing.as_ref())
        {
            self.segments = Some(SegmentClock::start(now, self.timings.segment));
        }
        let acts = self.tr.connected(rates);
        if self.tr.awaits_ready() {
            self.ready_deadline = Some(now + self.timings.hold);
        }
        Box::pin(self.run_acts(acts)).await
    }

    /// Execute one vendor-bound act, or queue it while the vendor reconnects.
    async fn vendor_call(&mut self, act: Act) -> Result<(), End> {
        let ready = self.provider.as_ref().is_some_and(|p| p.is_ready());
        if !ready || !self.pending.is_empty() {
            if self.pending.len() >= MAX_PENDING {
                return Err(Self::upstream_error(
                    "The vendor did not come back in time; the session is closed.",
                ));
            }
            self.pending.push_back(act);
            return Ok(());
        }
        self.send_now(act).await
    }

    async fn send_now(&mut self, act: Act) -> Result<(), End> {
        let Some(provider) = self.provider.as_mut() else {
            return Ok(());
        };
        let result = match act {
            Act::Audio(pcm) => {
                let pcm = self.tr.audio_for_vendor(&pcm);
                if pcm.is_empty() {
                    Ok(())
                } else {
                    provider.send_audio(pcm).await
                }
            }
            Act::Commit => {
                if let Some(tail) = self.tr.audio_tail_for_vendor() {
                    let _ = provider.send_audio(tail).await;
                }
                provider.commit_audio_buffer().await
            }
            Act::Clear => provider.clear_audio_buffer().await,
            Act::Text(t) => provider.send_text(&t).await,
            Act::CreateResponse => provider.create_response().await,
            Act::Cancel => provider.cancel_response().await,
            Act::ToolResult { call_id, output } => {
                provider.submit_function_result(&call_id, &output).await
            }
            _ => Ok(()),
        };
        match result {
            Ok(()) => Ok(()),
            Err(crate::core::realtime::RealtimeError::NotConnected) => Ok(()),
            Err(e) => {
                debug!(error = %e, "translated vendor call failed");
                self.to_client(gateway_error(
                    "vendor_error",
                    &format!("The vendor refused the request: {e}"),
                    None,
                    None,
                ))
                .await
            }
        }
    }

    async fn flush_pending(&mut self) -> Result<(), End> {
        while self.provider.as_ref().is_some_and(|p| p.is_ready()) {
            let Some(act) = self.pending.pop_front() else {
                break;
            };
            self.send_now(act).await?;
        }
        Ok(())
    }

    async fn run_acts(&mut self, acts: Vec<Act>) -> Result<(), End> {
        for act in acts {
            match act {
                Act::Client(text) => self.to_client(text).await?,
                Act::Connect => self.connect().await?,
                Act::Revalidate => {
                    self.revalidate().await?;
                    if !session::admit_response(&self.p.caller, &self.p.endpoint_id) {
                        self.to_client(gateway_error(
                            "quota_exceeded",
                            "The project's spend quota is exhausted.",
                            None,
                            None,
                        ))
                        .await?;
                    }
                }
                Act::Meter {
                    response_id,
                    status,
                    usage,
                    transcript,
                } => self.meter.usage_turn(
                    Some(&response_id),
                    Some(status),
                    &usage,
                    transcript.as_deref(),
                ),
                vendor_bound => self.vendor_call(vendor_bound).await?,
            }
        }
        Ok(())
    }

    async fn on_client_text(&mut self, raw: &str) -> Result<(), End> {
        self.last_activity = Instant::now();
        let acts = self.tr.client(raw);
        self.run_acts(acts).await
    }

    async fn on_vendor(&mut self, msg: VendorMsg) -> Result<(), End> {
        match msg {
            VendorMsg::Event(ev) => {
                self.last_activity = Instant::now();
                if matches!(ev, S2sEvent::SessionReady { .. }) {
                    self.ready_deadline = None;
                    if let Some(id) = match &ev {
                        S2sEvent::SessionReady { session_id } => session_id.clone(),
                        _ => None,
                    } {
                        self.meter.set_vendor_session_id(Some(id));
                    }
                }
                let acts = self.tr.vendor(ev);
                self.run_acts(acts).await?;
                self.flush_pending().await
            }
            VendorMsg::Reconnected(true) => {
                debug!("translated vendor reconnected");
                self.flush_pending().await
            }
            VendorMsg::Reconnected(false) => Err(Self::upstream_error(
                "The connection to the vendor was lost and could not be restored.",
            )),
        }
    }
}

/// Run a translated session (the counterpart of the relay's `run`).
pub(super) async fn run(
    state: Arc<AppState>,
    p: Prepared,
    socket: WebSocket,
    slot: Option<ConnectionSlot>,
) {
    let _slot = slot;
    let Engine::Translate(plan) = &p.engine else {
        unreachable!("the translate engine runs translated deployments only");
    };
    let timings = state.realtime.timings.clone();
    let session_id = format!("sess_bud_{}", uuid::Uuid::new_v4().simple());
    let vendor = p.endpoint.vendor.clone();
    let meter = SessionMeter::start(
        session_id.clone(),
        session::attribution(&p),
        p.endpoint.pricing.clone(),
    );
    metrics::gauge!("waav_realtime_sessions_active", "vendor" => vendor.clone()).increment(1.0);
    info!(session_id = %session_id, endpoint_id = %p.endpoint_id, vendor = %vendor, "realtime (translated) session opened");

    let (client_sink, mut client_rx) = socket.split();
    let (client_tx, writer) = session::spawn_writer(client_sink, CLIENT_QUEUE);
    let (vendor_tx, mut vendor_rx) = mpsc::unbounded_channel();
    let now = Instant::now();
    let SessionLimits {
        max_len,
        idle,
        max_at,
        warn_at,
    } = SessionLimits::new(&p, &timings, now);

    let mut shell = Shell {
        state: &state,
        p: &p,
        plan,
        tr: Translator::new(
            plan.info,
            p.endpoint_name.clone(),
            p.rules.clone(),
            plan.base.clone(),
            &session_id,
        ),
        meter,
        client_tx: client_tx.clone(),
        timings: timings.clone(),
        provider: None,
        vendor_tx,
        pending: VecDeque::new(),
        ready_deadline: None,
        segments: None,
        last_activity: now,
        client_missed: 0,
    };
    let mut ping = tokio::time::interval_at(now + timings.ping, timings.ping);
    let mut revalidate = tokio::time::interval_at(now + timings.revalidate, timings.revalidate);
    let mut warned = warn_at.is_none();

    let first = shell.tr.session_created();
    let end: End = match shell.to_client(first).await {
        Err(end) => end,
        Ok(()) => loop {
            let idle_at = shell.last_activity + idle;
            let ready_at = shell.ready_deadline;
            let segment_at = shell.segments.as_ref().map(SegmentClock::next_due);
            let step: Result<(), End> = tokio::select! {
                _ = state.shutdown.cancelled() => Err(End::new("drain", 1012)
                    .with_error("server_shutdown", "The server is restarting; reconnect.")),
                msg = client_rx.next() => match msg {
                    None | Some(Err(_)) => Err(End::new("client_close", 1006)),
                    Some(Ok(Message::Close(frame))) => Err(End::new("client_close", frame.map_or(1005, |f| f.code))),
                    Some(Ok(Message::Pong(_))) => { shell.client_missed = 0; Ok(()) }
                    Some(Ok(Message::Ping(_))) => Ok(()),
                    Some(Ok(Message::Binary(_))) => shell.to_client(gateway_error(
                        "invalid_event", "Binary frames are not part of the Realtime protocol; send JSON events.", None, None,
                    )).await,
                    Some(Ok(Message::Text(t))) => shell.on_client_text(t.as_str()).await,
                },
                Some(msg) = vendor_rx.recv() => shell.on_vendor(msg).await,
                _ = ping.tick() => {
                    if shell.client_missed >= timings.max_missed_pongs {
                        Err(End::new("client_timeout", 1011)
                            .with_error("client_timeout", "The client stopped answering pings."))
                    } else {
                        shell.client_missed += 1;
                        let _ = shell.client_tx.try_send(Outbound::Frame(Message::Ping(Vec::new().into())));
                        Ok(())
                    }
                }
                _ = revalidate.tick() => shell.revalidate().await,
                _ = tokio::time::sleep_until(idle_at) => Err(End::new("idle", 1000)
                    .with_error("session_expired", format!("The session was idle for {} s.", idle.as_secs()))),
                _ = tokio::time::sleep_until(warn_at.unwrap_or(max_at)), if !warned => {
                    warned = true;
                    shell.to_client(gateway_error("session_expiring",
                        &format!("The session reaches its maximum length in {} s.", timings.warn_before.as_secs()),
                        None, None)).await
                }
                _ = tokio::time::sleep_until(max_at) => Err(End::new("max_duration", 1000)
                    .with_error("session_expired", format!("The session reached its maximum length of {} s.", max_len.as_secs()))),
                _ = tokio::time::sleep_until(ready_at.unwrap_or(max_at)), if ready_at.is_some() => Err(Shell::upstream_error(
                    "The vendor did not start the session in time.")),
                _ = tokio::time::sleep_until(segment_at.unwrap_or(max_at)), if segment_at.is_some() => {
                    if let Some(clock) = shell.segments.as_mut() {
                        for secs in clock.due(Instant::now()) {
                            shell.meter.duration_segment(secs);
                        }
                    }
                    Ok(())
                }
            };
            if let Err(end) = step {
                break end;
            }
        },
    };

    for act in shell.tr.close() {
        if let Act::Meter {
            response_id,
            status,
            usage,
            transcript,
        } = act
        {
            shell.meter.usage_turn(
                Some(&response_id),
                Some(status),
                &usage,
                transcript.as_deref(),
            );
        }
    }
    if let Some(clock) = shell.segments.take() {
        // The final partial segment: a 150 s session bills 60 + 60 + 30 (TC-XL-07).
        for secs in clock.close(Instant::now()) {
            shell.meter.duration_segment(secs);
        }
    }
    if let Some(mut provider) = shell.provider.take() {
        let _ = tokio::time::timeout(Duration::from_secs(2), provider.disconnect()).await;
    }
    let Shell { meter, tr, .. } = shell;
    let mut meter = meter;
    meter.set_vendor_session_id(tr.vendor_session_id());
    session::finish(meter, end, &client_tx, writer, Some(&session_id), &vendor).await;
    drop(p);
}

#[cfg(test)]
mod tests;

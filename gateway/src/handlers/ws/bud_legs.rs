//! `/ws` on Bud deployments (FRD-023 RT6, §5.9, FR-WS-1…5).
//!
//! Under the Bud control plane a `/ws` session's legs address DEPLOYMENTS, exactly as the REST
//! routes do: `stt_config.model` names a transcription deployment and `tts_config.model` a
//! text-to-speech deployment, each resolved through the caller's own allowlist. The vendor, its
//! model, its credential, its `api_base` and its non-secret parameters come from the deployment's
//! `voice_table` entry — never from the client and never from WaaV's environment (D-5, X-4).
//!
//! Each leg takes the deployment's admission once, at session start, and holds it for the session
//! (FRD-022 §6.2). Each leg is metered as its own `voice.turn` records: a final transcription
//! bills the audio seconds streamed since the previous one; a `speak` bills its characters. And the
//! session is revalidated every 30 s, so a revoked key, a user removed from the project, or an
//! unpublished deployment ends it (D-17).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bud_auth::{AliasMetadata, VoiceEndpoint, VoicePricing};
use tracing::Span;

use crate::core::deployment_policy::{Admission, Rejection};
use crate::core::voice_cost::voice_cost;
use crate::handlers::advisories::Advisories;
use crate::handlers::endpoint_settings;
use crate::handlers::openai_realtime::session::{Caller, CallerCheck};
use crate::observability::voice_attrs::{leg as leg_attr, turn};
use crate::state::AppState;

use super::config::{STTWebSocketConfig, TTSWebSocketConfig};

pub const STT_CAPABILITY: &str = "audio_transcription";
pub const TTS_CAPABILITY: &str = "text_to_speech";

/// A leg resolved to a Bud deployment, with its admission held.
pub struct BudLeg {
    pub endpoint_id: String,
    /// The name the client used.
    pub endpoint_name: String,
    pub endpoint: VoiceEndpoint,
    pub alias: Option<AliasMetadata>,
    pub admission: Admission,
}

impl std::fmt::Debug for BudLeg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BudLeg")
            .field("endpoint_id", &self.endpoint_id)
            .field("endpoint_name", &self.endpoint_name)
            .field("vendor", &self.endpoint.vendor)
            .finish()
    }
}

/// Why a leg could not be served. `message` goes to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegRefusal {
    pub code: &'static str,
    pub message: String,
    /// Close the socket with this code. `None` leaves it open for a corrected `config`.
    pub close: Option<u16>,
}

impl LegRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            close: None,
        }
    }

    fn closing(mut self, code: u16) -> Self {
        self.close = Some(code);
        self
    }
}

/// The close code for a session its caller may no longer hold (revoked, removed, unpublished).
pub const CLOSE_REVOKED: u16 = 1008;
/// The close code for a deployment at its rate or concurrency limit: retry later.
pub const CLOSE_TRY_LATER: u16 = 1013;

fn kind(capability: &str) -> &'static str {
    if capability == STT_CAPABILITY {
        "transcription"
    } else {
        "text-to-speech"
    }
}

/// Resolve `model` (named at `field`, e.g. `stt_config.model`) as a deployment serving `capability`
/// and admit the session to it.
pub async fn resolve_leg(
    state: &AppState,
    credential: &str,
    field: &str,
    model: &str,
    capability: &'static str,
) -> Result<BudLeg, LegRefusal> {
    let model = model.trim();
    if model.is_empty() {
        return Err(LegRefusal::new(
            "deployment_required",
            format!(
                "{field} must name your Bud {} deployment: this gateway serves Bud \
                 deployments only, and uses the deployment's own vendor credential; `provider` \
                 is taken from the deployment (FRD-023 RT6).",
                kind(capability)
            ),
        ));
    }
    let Some(resolved) = state.resolve_voice_endpoint(model, capability, Some(credential)) else {
        return Err(LegRefusal::new(
            "model_not_found",
            format!(
                "{field} '{model}' is not a {} deployment this credential can reach.",
                kind(capability)
            ),
        ));
    };
    let vendor = resolved.endpoint.vendor.as_str();
    if capability == STT_CAPABILITY
        && (crate::core::tts::self_hosted::is_self_hosted(vendor)
            || crate::core::tts::self_hosted::is_azure_openai(vendor))
    {
        // These answer one uploaded file over HTTP; there is no stream to hold open.
        return Err(LegRefusal::new(
            "unsupported_deployment",
            format!(
                "Deployment '{model}' ({vendor}) transcribes uploaded files and cannot stream; use \
                 it through /v1/audio/transcriptions, or name a streaming transcription deployment."
            ),
        ));
    }
    let mut ignored = Advisories::new();
    if let Some(why) = crate::handlers::openai_audio::endpoint_misconfiguration_reason(
        &resolved.endpoint,
        &mut ignored,
    ) {
        return Err(LegRefusal::new(
            "deployment_misconfigured",
            format!("Deployment '{model}' is misconfigured for vendor '{vendor}': {why}"),
        ));
    }
    let admission = state
        .admit_deployment(&resolved.endpoint_id)
        .await
        .map_err(|rej| {
            let (code, what) = match rej {
                Rejection::Rate(_) => ("rate_limit_exceeded", "rate limit"),
                Rejection::Concurrency(_) => {
                    ("concurrency_limit_exceeded", "concurrent-session limit")
                }
            };
            LegRefusal::new(
                code,
                format!(
                    "Deployment '{model}' reached its {what}; retry in {} s.",
                    rej.retry_after().as_secs().max(1)
                ),
            )
            .closing(CLOSE_TRY_LATER)
        })?;
    Ok(BudLeg {
        endpoint_id: resolved.endpoint_id,
        endpoint_name: model.to_string(),
        endpoint: resolved.endpoint,
        alias: resolved.alias,
        admission,
    })
}

/// The deployment's own vendor parameters replace whatever the client sent: some providers read a
/// DESTINATION from `extras` (Groq's `url`, Azure Speech's host), and on a Bud leg the credential
/// that would travel there is the deployment's.
fn replace_extras(
    target: &mut serde_json::Map<String, serde_json::Value>,
    endpoint: &VoiceEndpoint,
) {
    target.clear();
    target.extend(crate::handlers::openai_audio::deployment_extras(endpoint));
}

/// `base`, with every field the client set explicitly laid over it: request > deployment.
fn overlay<T>(base: T, client: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let (Ok(serde_json::Value::Object(mut merged)), Ok(serde_json::Value::Object(ours))) =
        (serde_json::to_value(&base), serde_json::to_value(client))
    else {
        return base;
    };
    for (k, v) in ours {
        if !v.is_null() {
            merged.insert(k, v);
        }
    }
    match serde_json::from_value(serde_json::Value::Object(merged)) {
        Ok(v) => v,
        Err(_) => base,
    }
}

fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Point the STT leg at its deployment (FR-WS-1, FR-WS-4). Returns the vendor credential.
///
/// The deployment's published transcription settings are the defaults and the client's explicit
/// choices win, as on REST; `stt.streaming` applies here and only here.
pub fn apply_stt(
    cfg: &mut STTWebSocketConfig,
    leg: &BudLeg,
    advisories: &mut Advisories,
) -> String {
    let ep = &leg.endpoint;
    let settings = ep.config.stt();
    cfg.provider = ep.vendor.clone();
    cfg.model = non_empty(&settings.model)
        .or_else(|| ep.model.clone())
        .unwrap_or_default();
    if cfg.language.trim().is_empty()
        && let Some(lang) = non_empty(&settings.language).or_else(|| ep.language.clone())
    {
        cfg.language = lang;
    }

    let mut base = endpoint_settings::stt_features_for(&settings, &ep.vendor, advisories);
    // `stt_features_for` pins Deepgram's `vad_events` to the REST plane's historical default;
    // on this plane unset has always meant unset.
    base.vad_events = None;
    if let Some(streaming) = &settings.streaming {
        base.interim_results = streaming.interim_results;
        base.vad_events = streaming.vad_events;
        base.endpointing_ms = streaming.endpointing_ms;
        base.utterance_end_ms = streaming.utterance_end_ms;
        base.speech_begin_event = streaming.speech_begin_event;
    }
    cfg.features = overlay(base, &cfg.features);
    if cfg.translation.is_none() {
        cfg.translation =
            endpoint_settings::translation_for(ep.config.translation.as_ref(), false, advisories);
    }
    replace_extras(&mut cfg.extras.0, ep);
    cfg.api_key = None;
    ep.credential.clone().unwrap_or_default()
}

/// Point the TTS leg at its deployment (FR-WS-1). Returns the vendor credential; the deployment's
/// `api_base` is the leg's [`BudLeg::api_base`].
pub fn apply_tts(
    cfg: &mut TTSWebSocketConfig,
    leg: &BudLeg,
    advisories: &mut Advisories,
) -> String {
    let ep = &leg.endpoint;
    let settings = ep.config.tts();
    cfg.provider = ep.vendor.clone();
    cfg.model = ep.model.clone().unwrap_or_default();
    if cfg.sample_rate.is_none() {
        cfg.sample_rate = settings.sample_rate;
    }
    if cfg.connection_timeout.is_none() {
        cfg.connection_timeout = settings.connection_timeout;
    }
    if cfg.request_timeout.is_none() {
        cfg.request_timeout = settings.request_timeout;
    }
    if cfg.pronunciations.is_empty()
        && let Some(list) = &settings.pronunciations
    {
        cfg.pronunciations = list
            .iter()
            .map(|p| crate::core::tts::Pronunciation {
                word: p.word.clone(),
                pronunciation: p.pronunciation.clone(),
            })
            .collect();
    }
    let language = ep.language.clone().or_else(|| settings.language.clone());
    let base =
        endpoint_settings::tts_features_for(&settings, &ep.vendor, language.as_deref(), advisories);
    cfg.features = overlay(base, &cfg.features);
    replace_extras(&mut cfg.extras.0, ep);
    cfg.api_key = None;
    ep.credential.clone().unwrap_or_default()
}

/// Choose the TTS leg's voice as REST does (request > deployment), with every catalogue read made
/// with the DEPLOYMENT's credential: the client's voice id; the client's descriptor matched in the
/// deployment vendor's catalogue; the deployment's voice; the deployment's descriptor.
pub async fn resolve_tts_voice(
    state: &Arc<AppState>,
    cfg: &mut TTSWebSocketConfig,
    leg: &BudLeg,
    advisories: &mut Advisories,
) {
    if cfg
        .voice_id
        .as_deref()
        .is_some_and(|v| !v.trim().is_empty())
    {
        return;
    }
    let ep = &leg.endpoint;
    if let Some(described) = cfg.voice_descriptor.clone().filter(|d| d.is_set()) {
        let catalog = crate::handlers::voices::fetch_provider_catalog_with_key(
            state,
            &ep.vendor,
            ep.credential.as_deref(),
        )
        .await;
        let resolved = crate::core::voice::resolve_voice(
            &described,
            &catalog,
            crate::handlers::voices::provider_default_voice(&ep.vendor),
        );
        if let Some(warning) = resolved.warning {
            advisories.warn(warning);
        }
        if !resolved.voice_id.trim().is_empty() {
            cfg.voice_id = Some(resolved.voice_id);
        }
        return;
    }
    if let Some(voice) = non_empty(&ep.voice) {
        cfg.voice_id = Some(voice);
        return;
    }
    cfg.voice_id =
        crate::handlers::openai_audio::resolve_described_voice(state, ep, advisories).await;
    // Nothing named or described a voice: where the vendor REQUIRES one, its default (as REST
    // answers, rather than a session that cannot speak); elsewhere the vendor picks.
    if cfg.voice_id.is_none() && crate::handlers::voices::voice_required(&ep.vendor) {
        let default = crate::handlers::voices::provider_default_voice(&ep.vendor);
        if !default.is_empty() {
            advisories.warn(format!(
                "deployment '{}' has no voice configured, so {}'s default voice was used; set one \
                 on the deployment or send tts_config.voice_id to choose it",
                leg.endpoint_name, ep.vendor
            ));
            cfg.voice_id = Some(default.to_string());
        }
    }
}

/// Who a leg's records are attributed to and what they cost.
#[derive(Debug, Clone)]
struct LegBilling {
    capability: &'static str,
    endpoint_id: String,
    endpoint_name: String,
    model_id: Option<String>,
    project_id: Option<String>,
    vendor: String,
    vendor_model: Option<String>,
    pricing: Option<VoicePricing>,
}

impl LegBilling {
    fn from_leg(capability: &'static str, leg: &BudLeg, caller: &Caller) -> Self {
        Self {
            capability,
            endpoint_id: leg.endpoint_id.clone(),
            endpoint_name: leg.endpoint_name.clone(),
            model_id: leg.alias.as_ref().and_then(|a| a.model_id.clone()),
            project_id: leg
                .alias
                .as_ref()
                .and_then(|a| a.project_id.clone())
                .or_else(|| caller.principal.project_id.clone()),
            vendor: leg.endpoint.vendor.clone(),
            vendor_model: leg.endpoint.model.clone(),
            pricing: leg.endpoint.pricing.clone(),
        }
    }
}

fn record_text(span: &Span, key: &'static str, value: Option<&str>) {
    if let Some(v) = value.filter(|v| !v.is_empty()) {
        span.record(key, v);
    }
}

/// Meters a Bud-mode `/ws` session's STT and TTS legs (FR-WS-5, TC-WS-12).
pub struct LegMeter {
    session_id: String,
    caller: Caller,
    stt: Option<LegBilling>,
    tts: Option<LegBilling>,
    /// PCM16 bytes streamed to the STT leg since the last final transcript.
    stt_bytes: AtomicU64,
    /// Settable: the codec negotiation may move the rate after the legs are resolved.
    stt_bytes_per_second: AtomicU64,
    turn_index: AtomicU64,
    /// Further deployments the session holds (a DAG template's bound nodes), revalidated with it.
    held: parking_lot::Mutex<Vec<(String, &'static str)>>,
}

impl std::fmt::Debug for LegMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegMeter")
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl LegMeter {
    pub fn new(
        session_id: String,
        caller: Caller,
        stt: Option<(&BudLeg, u32, u16)>,
        tts: Option<&BudLeg>,
    ) -> Self {
        let meter = Self {
            session_id,
            stt: stt.map(|(leg, _, _)| LegBilling::from_leg(STT_CAPABILITY, leg, &caller)),
            tts: tts.map(|leg| LegBilling::from_leg(TTS_CAPABILITY, leg, &caller)),
            caller,
            stt_bytes: AtomicU64::new(0),
            stt_bytes_per_second: AtomicU64::new(0),
            turn_index: AtomicU64::new(0),
            held: parking_lot::Mutex::new(Vec::new()),
        };
        if let Some((_, rate, channels)) = stt {
            meter.set_stt_format(rate, channels);
        }
        meter
    }

    /// The PCM16 format the STT leg receives, as finally negotiated.
    pub fn set_stt_format(&self, sample_rate: u32, channels: u16) {
        let bps = u64::from(sample_rate) * 2 * u64::from(channels.max(1));
        self.stt_bytes_per_second.store(bps, Ordering::Relaxed);
    }

    /// Count audio streamed to the STT leg (after any codec decode). One atomic add: hot path.
    pub fn add_stt_audio(&self, bytes: usize) {
        self.stt_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    fn open(&self, billing: &LegBilling) -> Span {
        let span = crate::voice_turn_span!(
            parent: None,
            capability = billing.capability,
            transport = "websocket"
        );
        let p = &self.caller.principal;
        record_text(&span, turn::PROJECT_ID, billing.project_id.as_deref());
        record_text(&span, turn::ENDPOINT_ID, Some(&billing.endpoint_id));
        record_text(&span, turn::ENDPOINT_NAME, Some(&billing.endpoint_name));
        record_text(&span, turn::MODEL_ID, billing.model_id.as_deref());
        record_text(&span, turn::API_KEY_ID, p.api_key_id.as_deref());
        record_text(&span, turn::USER_ID, p.user_id.as_deref());
        record_text(&span, turn::API_KEY_PROJECT_ID, p.project_id.as_deref());
        record_text(&span, turn::SESSION_ID, Some(&self.session_id));
        span.record(
            turn::TURN_INDEX,
            self.turn_index.fetch_add(1, Ordering::Relaxed),
        );
        span
    }

    /// A final transcript: bill the audio streamed since the previous one.
    pub fn stt_final(&self, transcript: &str) {
        let Some(billing) = &self.stt else { return };
        let bps = self.stt_bytes_per_second.load(Ordering::Relaxed);
        if bps == 0 {
            return;
        }
        let bytes = self.stt_bytes.swap(0, Ordering::Relaxed);
        if bytes == 0 {
            return;
        }
        let seconds = bytes as f64 / bps as f64;
        let span = self.open(billing);
        record_text(&span, leg_attr::STT_VENDOR, Some(&billing.vendor));
        record_text(&span, leg_attr::STT_MODEL, billing.vendor_model.as_deref());
        span.record(turn::AUDIO_SECONDS, seconds);
        if let Some((cost, unit)) = voice_cost(
            billing.pricing.as_ref(),
            STT_CAPABILITY,
            None,
            Some(seconds),
            None,
        ) {
            span.record(turn::COST, cost);
            span.record(turn::PRICING_UNIT, unit);
        }
        if crate::observability::trace_redact::capture_content() {
            span.record(
                turn::TRANSCRIPT,
                crate::observability::trace_redact::sanitize_body(transcript).as_str(),
            );
        }
    }

    /// A `speak`: bill its characters.
    pub fn tts_spoken(&self, text: &str) {
        let Some(billing) = &self.tts else { return };
        let characters = text.chars().count() as u64;
        if characters == 0 {
            return;
        }
        let span = self.open(billing);
        record_text(&span, leg_attr::TTS_VENDOR, Some(&billing.vendor));
        record_text(&span, leg_attr::TTS_MODEL, billing.vendor_model.as_deref());
        span.record(turn::CHARACTERS, characters);
        if let Some((cost, unit)) = voice_cost(
            billing.pricing.as_ref(),
            TTS_CAPABILITY,
            Some(characters),
            None,
            None,
        ) {
            span.record(turn::COST, cost);
            span.record(turn::PRICING_UNIT, unit);
        }
    }

    /// At close: bill audio streamed after the last final transcript, so nothing streamed goes
    /// unbilled.
    pub fn finish(&self) {
        self.stt_final("");
    }

    /// How to re-check this session's caller.
    pub fn check(&self) -> &CallerCheck {
        &self.caller.check
    }

    /// The deployments this session holds and what each serves it, for revalidation.
    pub fn endpoints(&self) -> Vec<(String, &'static str)> {
        let mut all: Vec<(String, &'static str)> = self
            .stt
            .iter()
            .chain(self.tts.iter())
            .map(|b| (b.endpoint_id.clone(), b.capability))
            .collect();
        all.extend(self.held.lock().iter().cloned());
        all
    }

    /// Revalidate `endpoint_id` with the session from now on.
    pub fn hold(&self, endpoint_id: String, capability: &'static str) {
        self.held.lock().push((endpoint_id, capability));
    }

    /// The caller the session acts as.
    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    /// Whether `other` is the same caller: an `auth` refresh may renew a credential, never swap
    /// the principal a live session bills and authorizes against.
    pub fn same_caller(&self, other: &CallerCheck) -> bool {
        self.caller.check == *other
    }
}

/// Every deployment a Bud-mode `/ws` session holds is still reachable by its caller and still
/// serves what the session uses it for (D-17).
pub async fn session_still_allowed(state: &AppState, meter: &LegMeter) -> bool {
    let Some(bud) = state.bud_mode.as_ref() else {
        return false;
    };
    let plane = bud.plane();
    for (ep, capability) in meter.endpoints() {
        let serving = plane
            .voice_endpoint(&ep)
            .is_some_and(|e| e.serves(capability));
        let reachable = match meter.check() {
            CallerCheck::ApiKey { hashed, client_key } => {
                plane.hash_reaches(hashed, &ep, *client_key).is_some()
            }
            CallerCheck::Jwt { sub } => plane.subject_reaches(sub, &ep).await.is_some(),
        };
        if !serving || !reachable {
            return false;
        }
    }
    true
}

/// A Bud-mode `/ws` session's legs, resolved and admitted.
pub struct PreparedLegs {
    pub stt_key: String,
    pub tts_key: String,
    /// The TTS deployment's own address (self-hosted, Azure OpenAI); never the client's (§5.9).
    pub tts_api_base: Option<String>,
    pub meter: Arc<LegMeter>,
    /// One per leg deployment, held for the session (FRD-022 §6.2).
    pub admissions: Vec<Admission>,
    /// What the client should hear about settings that did not apply.
    pub advisories: Advisories,
}

impl std::fmt::Debug for PreparedLegs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedLegs")
            .field("meter", &self.meter)
            .field("admissions", &self.admissions.len())
            .finish()
    }
}

/// Authenticate the session's caller, resolve both legs as deployments through the caller's
/// allowlist, admit each, and point the configs at them (FR-WS-1, TC-WS-01…05).
///
/// A leg refused after the other was admitted releases that admission as it returns: nothing is
/// held for a session that never starts (TC-WS-05).
pub async fn prepare(
    state: &Arc<AppState>,
    credential: Option<&crate::auth::SessionCredential>,
    stt: &mut STTWebSocketConfig,
    tts: &mut TTSWebSocketConfig,
    session_id: &str,
) -> Result<PreparedLegs, LegRefusal> {
    let Some(credential) = credential.map(|c| c.current()) else {
        return Err(LegRefusal::new(
            "authentication_required",
            "This gateway serves Bud deployments, which need your Bud API key or token.",
        )
        .closing(CLOSE_REVOKED));
    };
    let bearer = crate::handlers::openai_realtime::handshake::Credential::new(
        credential.clone(),
        crate::handlers::openai_realtime::handshake::CredentialSource::Bearer,
    );
    let caller = crate::handlers::openai_realtime::session::authenticate(state, &bearer)
        .await
        .map_err(|e| {
            let close = if e.status == axum::http::StatusCode::SERVICE_UNAVAILABLE
                || e.status == axum::http::StatusCode::TOO_MANY_REQUESTS
            {
                CLOSE_TRY_LATER
            } else {
                CLOSE_REVOKED
            };
            LegRefusal::new(e.code, e.message).closing(close)
        })?;

    // A leg's credential is its deployment's. A key of the client's own would bypass attribution,
    // quota and billing (FRD-018 §5.3.7), and is refused rather than silently replaced.
    for (field, key) in [
        ("stt_config.api_key", &stt.api_key),
        ("tts_config.api_key", &tts.api_key),
    ] {
        if key.as_deref().is_some_and(|k| !k.trim().is_empty()) {
            return Err(LegRefusal::new(
                "client_key_not_accepted",
                format!(
                    "{field} is not accepted by this gateway: each leg uses its deployment's own \
                     credential. Remove the field and name the deployment in `model`."
                ),
            ));
        }
    }

    let stt_leg = resolve_leg(
        state,
        &credential,
        "stt_config.model",
        &stt.model,
        STT_CAPABILITY,
    )
    .await?;
    let tts_leg = resolve_leg(
        state,
        &credential,
        "tts_config.model",
        &tts.model,
        TTS_CAPABILITY,
    )
    .await?;

    let mut advisories = Advisories::new();
    let stt_key = apply_stt(stt, &stt_leg, &mut advisories);
    let tts_key = apply_tts(tts, &tts_leg, &mut advisories);
    resolve_tts_voice(state, tts, &tts_leg, &mut advisories).await;

    let meter = Arc::new(LegMeter::new(
        session_id.to_string(),
        caller,
        Some((&stt_leg, stt.sample_rate, stt.channels)),
        Some(&tts_leg),
    ));
    let tts_api_base = tts_leg.endpoint.api_base.clone();
    Ok(PreparedLegs {
        stt_key,
        tts_key,
        tts_api_base,
        meter,
        admissions: vec![stt_leg.admission, tts_leg.admission],
        advisories,
    })
}

/// Headers a template may not send to the Bud gateway: the caller's own credential is the one.
const LLM_CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    "api-key",
    "x-api-key",
    "proxy-authorization",
];

/// Make a server DAG template's nodes address Bud deployments as the caller (FRD-023 WP-RT6.3,
/// TC-WS-10).
///
/// * A TTS provider node's `model` names a text-to-speech deployment: resolved through the
///   caller's allowlist, admitted for the session, metered per synthesis and revalidated.
/// * An LLM or translate node's `model` names a chat deployment, reached through the Bud gateway
///   with the caller's credential; the template's `base_url`, `api_key` and credential headers are
///   dropped.
/// * An STT provider node marks where the session's STT leg injects its transcript; it never runs.
/// * A realtime provider node is refused: GA realtime on Bud deployments is the `/v1/realtime`
///   relay, and the native engines address deployments from RT7.
///
/// Returns the admissions to hold for the session.
pub async fn bind_dag(
    state: &Arc<AppState>,
    definition: &mut crate::dag::definition::DAGDefinition,
    credential: &crate::auth::SessionCredential,
    session_meter: &Arc<LegMeter>,
    session_id: &str,
) -> Result<Vec<Admission>, LegRefusal> {
    use crate::dag::definition::{NodeDefinition, NodeType};
    let raw = credential.current();
    let mut admissions = Vec::new();
    for node in definition.nodes.iter_mut() {
        let NodeDefinition {
            id,
            node_type,
            config,
            bud,
            ..
        } = node;
        match node_type {
            NodeType::RealtimeProvider { .. } => {
                return Err(LegRefusal::new(
                    "unsupported_node",
                    format!(
                        "DAG node '{id}' is a realtime provider, which this gateway does not run on \
                         Bud deployments yet; use /v1/realtime with a realtime deployment."
                    ),
                ));
            }
            NodeType::TtsProvider {
                provider,
                voice_id,
                model,
            } => {
                let name = model.clone().unwrap_or_default();
                let field = format!("DAG node '{id}' model");
                let leg = resolve_leg(state, &raw, &field, &name, TTS_CAPABILITY).await?;
                let ep = &leg.endpoint;
                *provider = ep.vendor.clone();
                *model = ep.model.clone();
                if voice_id.as_deref().is_none_or(|v| v.trim().is_empty()) {
                    *voice_id = match non_empty(&ep.voice) {
                        Some(voice) => Some(voice),
                        None => {
                            crate::handlers::openai_audio::resolve_described_voice(
                                state,
                                ep,
                                &mut Advisories::new(),
                            )
                            .await
                        }
                    };
                }
                if let Some(obj) = config.as_object_mut() {
                    obj.remove("api_key");
                }
                let meter = Arc::new(LegMeter::new(
                    session_id.to_string(),
                    session_meter.caller().clone(),
                    None,
                    Some(&leg),
                ));
                session_meter.hold(leg.endpoint_id.clone(), TTS_CAPABILITY);
                *bud = Some(Arc::new(crate::dag::nodes::BudNodeBinding {
                    endpoint_id: leg.endpoint_id.clone(),
                    vendor_credential: ep.credential.clone(),
                    api_base: ep.api_base.clone(),
                    extras: crate::handlers::openai_audio::deployment_extras(ep),
                    session_credential: None,
                    on_synthesis: Some(Arc::new(move |text: &str| meter.tts_spoken(text))),
                }));
                admissions.push(leg.admission);
            }
            NodeType::LlmEndpoint {
                base_url,
                model,
                api_key,
                headers,
                ..
            }
            | NodeType::Translate {
                base_url,
                model,
                api_key,
                headers,
                ..
            } => {
                let Some(url) = llm_base_url() else {
                    return Err(LegRefusal::new(
                        "llm_unavailable",
                        format!(
                            "DAG node '{id}' needs the Bud gateway (WAAV_LLM_BASE_URL), which this \
                             gateway is not configured with."
                        ),
                    ));
                };
                *base_url = url;
                *api_key = None;
                headers.retain(|k, _| {
                    !LLM_CREDENTIAL_HEADERS.contains(&k.to_ascii_lowercase().as_str())
                });
                *bud = Some(Arc::new(crate::dag::nodes::BudNodeBinding {
                    endpoint_id: model.clone(),
                    vendor_credential: None,
                    api_base: None,
                    extras: serde_json::Map::new(),
                    session_credential: Some(credential.clone()),
                    on_synthesis: None,
                }));
            }
            _ => {}
        }
    }
    Ok(admissions)
}

/// The budgateway base URL for the voice agent's LLM leg (`WAAV_LLM_BASE_URL`, F-8).
pub fn llm_base_url() -> Option<String> {
    std::env::var("WAAV_LLM_BASE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}

#[allow(dead_code)]
fn _assert_send_sync() {
    fn is<T: Send + Sync>() {}
    is::<LegMeter>();
    is::<Arc<LegMeter>>();
}

#[cfg(test)]
mod tests {
    //! FRD-023 RT6 over an in-memory control plane: TC-WS-01…05, 10…13 and the leg security
    //! rules. The vendors are never dialled — a leg's job ends at a config pointed at the
    //! deployment, which is what these assert; the live streams are the pde-ditto E2E.

    use super::*;
    use crate::test_support::{TEST_CREDENTIAL, TEST_CREDENTIAL_PLAIN, bud_state_with_credentials};
    use serde_json::{Value as Json, json};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::registry::LookupSpan;

    const KEY: &str = "bud_ws_legs_test_key";
    const OTHER_KEY: &str = "bud_ws_legs_other_key";
    const PROJECT: &str = "7c0c7e1d-0000-4000-8000-00000000aa01";
    const OTHER_PROJECT: &str = "7c0c7e1d-0000-4000-8000-00000000aa02";
    const USER: &str = "7c0c7e1d-0000-4000-8000-00000000bb01";
    const API_KEY_ID: &str = "7c0c7e1d-0000-4000-8000-00000000cc01";
    const MODEL_ID: &str = "7c0c7e1d-0000-4000-8000-00000000dd01";

    fn stt_entry(extra: Json) -> Json {
        merge(
            json!({
                "vendor": "deepgram",
                "credential": TEST_CREDENTIAL.trim(),
                "endpoints": ["audio_transcription"],
                "model": "nova-3",
                "language": "en-US",
                "pricing": {"unit": "minute", "per_units": 1, "cost_per_unit": 0.0043, "currency": "USD"},
                "config": {"stt": {"diarization": true,
                    "streaming": {"interim_results": true, "endpointing_ms": 300, "utterance_end_ms": 1000}}}
            }),
            extra,
        )
    }

    fn tts_entry(extra: Json) -> Json {
        merge(
            json!({
                "vendor": "elevenlabs",
                "credential": TEST_CREDENTIAL.trim(),
                "endpoints": ["text_to_speech"],
                "model": "eleven_flash_v2_5",
                "voice": "JBFqnCBsd6RMkjVDRZzb",
                "pricing": {"unit": "character", "per_units": 1000, "cost_per_unit": 0.3, "currency": "USD"},
                "config": {"tts": {"pronunciations": [{"word": "Bud", "pronunciation": "bʌd"}]}}
            }),
            extra,
        )
    }

    fn merge(mut base: Json, extra: Json) -> Json {
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                base[k] = v.clone();
            }
        }
        base
    }

    /// `(alias, endpoint id, entry)`; KEY reaches `mine`, OTHER_KEY reaches `theirs`.
    async fn plane(
        mine: &[(&str, &str, Json)],
        theirs: &[(&str, &str, Json)],
    ) -> (Arc<AppState>, Arc<bud_auth::MemoryStore>) {
        let blob = |eps: &[(&str, &str, Json)], project: &str| {
            let mut m = serde_json::Map::new();
            for (alias, id, _) in eps {
                m.insert(
                    alias.to_string(),
                    json!({"endpoint_id": id, "model_id": MODEL_ID, "project_id": project, "kind": "model"}),
                );
            }
            m.insert(
                "__metadata__".into(),
                json!({"api_key_id": API_KEY_ID, "user_id": USER, "api_key_project_id": project}),
            );
            Json::Object(m).to_string()
        };
        let mut keys: Vec<(String, String)> = vec![
            (
                format!("api_key:{}", bud_auth::hash_api_key(KEY)),
                blob(mine, PROJECT),
            ),
            (
                format!("api_key:{}", bud_auth::hash_api_key(OTHER_KEY)),
                blob(theirs, OTHER_PROJECT),
            ),
        ];
        for (_, id, entry) in mine.iter().chain(theirs.iter()) {
            keys.push((
                format!("voice_table:{id}"),
                json!({ *id: entry }).to_string(),
            ));
        }
        let refs: Vec<(&str, &str)> = keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        bud_state_with_credentials(&refs).await
    }

    fn stt_cfg(v: Json) -> STTWebSocketConfig {
        serde_json::from_value(merge(
            json!({"language": "en-US", "sample_rate": 16000, "channels": 1, "punctuation": true}),
            v,
        ))
        .expect("a Bud client may omit provider")
    }

    fn tts_cfg(v: Json) -> TTSWebSocketConfig {
        serde_json::from_value(merge(
            json!({"voice_id": null, "speaking_rate": null, "audio_format": "linear16",
                   "sample_rate": 24000, "connection_timeout": null, "request_timeout": null}),
            v,
        ))
        .expect("a Bud client may omit provider")
    }

    fn cred(raw: &str) -> crate::auth::SessionCredential {
        crate::auth::SessionCredential::new(raw)
    }

    async fn default_plane() -> (Arc<AppState>, Arc<bud_auth::MemoryStore>) {
        plane(
            &[
                ("stt-dg", "ep-stt", stt_entry(json!({}))),
                ("tts-el", "ep-tts", tts_entry(json!({}))),
            ],
            &[("their-tts", "ep-their-tts", tts_entry(json!({})))],
        )
        .await
    }

    /// TC-WS-01 🔒 / TC-WS-02 🔒 — each leg is its deployment: vendor, model, credential and voice
    /// from voice_table; the client's `provider` is informational.
    #[tokio::test]
    async fn tc_ws_01_02_legs_take_the_deployment_vendor_model_and_credential() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"provider": "assemblyai", "model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));

        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .expect("both legs resolve");

        assert_eq!(stt.provider, "deepgram");
        assert_eq!(stt.model, "nova-3");
        assert_eq!(legs.stt_key, TEST_CREDENTIAL_PLAIN);
        assert_eq!(tts.provider, "elevenlabs");
        assert_eq!(tts.model, "eleven_flash_v2_5");
        assert_eq!(tts.voice_id.as_deref(), Some("JBFqnCBsd6RMkjVDRZzb"));
        assert_eq!(legs.tts_key, TEST_CREDENTIAL_PLAIN);
        assert_eq!(
            tts.pronunciations.len(),
            1,
            "the deployment's pronunciations apply"
        );
        assert!(stt.api_key.is_none() && tts.api_key.is_none());
        assert_eq!(legs.admissions.len(), 2, "one admission per leg");
    }

    /// A deployment with no voice, on a vendor that requires one, speaks with the vendor's default
    /// and says so (as REST does).
    #[tokio::test]
    async fn a_voiceless_deployment_uses_the_vendor_default_and_says_so() {
        let (state, _) = plane(
            &[
                ("stt-dg", "ep-stt", stt_entry(json!({}))),
                ("tts-el", "ep-tts", tts_entry(json!({"voice": null}))),
            ],
            &[],
        )
        .await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap();
        assert_eq!(
            tts.voice_id.as_deref(),
            Some(crate::handlers::voices::provider_default_voice(
                "elevenlabs"
            ))
        );
        assert!(
            legs.advisories
                .as_slice()
                .iter()
                .any(|a| a.contains("no voice configured")),
            "{:?}",
            legs.advisories.as_slice()
        );
    }

    /// TC-WS-02 — request > deployment: the client's own voice wins.
    #[tokio::test]
    async fn tc_ws_02_the_clients_voice_wins() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el", "voice_id": "EXAVITQu4vr4xnSDxMaL"}));
        prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap();
        assert_eq!(tts.voice_id.as_deref(), Some("EXAVITQu4vr4xnSDxMaL"));
    }

    /// TC-WS-03 🔒 — a self-hosted TTS deployment's `api_base` is used (it was forced `None` on
    /// the socket path).
    #[tokio::test]
    async fn tc_ws_03_the_deployment_api_base_is_used() {
        let (state, _) = plane(
            &[
                ("stt-dg", "ep-stt", stt_entry(json!({}))),
                (
                    "tts-own",
                    "ep-own",
                    tts_entry(
                        json!({"vendor": "self_hosted", "api_base": "http://tts.internal:8000",
                        "model": "kokoro", "voice": "af_heart"}),
                    ),
                ),
            ],
            &[],
        )
        .await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-own"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap();
        assert_eq!(
            legs.tts_api_base.as_deref(),
            Some("http://tts.internal:8000")
        );
        assert_eq!(tts.provider, "self_hosted");
    }

    /// 🔒 A client's `extras` never ride a Bud leg: Groq reads a destination from `extras.url`,
    /// Azure Speech its host, and the credential that would travel there is the deployment's.
    #[tokio::test]
    async fn client_extras_are_replaced_by_the_deployments() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg",
            "extras": {"url": "https://attacker.example/listen", "endpoint_override": "wss://attacker.example"}}));
        let mut tts =
            tts_cfg(json!({"model": "tts-el", "extras": {"url": "https://attacker.example"}}));
        prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap();
        assert!(
            !serde_json::to_string(&stt.extras.0)
                .unwrap()
                .contains("attacker")
                && !serde_json::to_string(&tts.extras.0)
                    .unwrap()
                    .contains("attacker"),
            "stt extras {:?}, tts extras {:?}",
            stt.extras.0,
            tts.extras.0
        );
    }

    /// TC-WS-04 🔒 — `provider` without a deployment `model` is refused with the addressing hint;
    /// a name the caller cannot reach is not found.
    #[tokio::test]
    async fn tc_ws_04_provider_only_and_unreachable_names_are_refused() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"provider": "deepgram"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let refusal = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap_err();
        assert_eq!(refusal.code, "deployment_required");
        assert!(
            refusal.message.contains("stt_config.model"),
            "{}",
            refusal.message
        );
        assert_eq!(
            refusal.close, None,
            "the client may send a corrected config"
        );

        // Another project's deployment, by alias or by endpoint id.
        for name in ["their-tts", "ep-their-tts"] {
            let mut stt = stt_cfg(json!({"model": "stt-dg"}));
            let mut tts = tts_cfg(json!({"model": name}));
            let refusal = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
                .await
                .unwrap_err();
            assert_eq!(refusal.code, "model_not_found", "{name}");
        }
    }

    /// 🔒 A client's own vendor key is refused on a Bud leg, not silently replaced.
    #[tokio::test]
    async fn a_client_vendor_key_is_refused_by_name() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el", "api_key": "sk-byok"}));
        let refusal = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap_err();
        assert_eq!(refusal.code, "client_key_not_accepted");
        assert!(
            refusal.message.contains("tts_config.api_key"),
            "{}",
            refusal.message
        );
    }

    /// A leg's capability is checked: a TTS deployment is not an STT leg.
    #[tokio::test]
    async fn a_deployment_of_the_wrong_capability_is_not_found() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "tts-el"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let refusal = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap_err();
        assert_eq!(refusal.code, "model_not_found");
    }

    /// A self-hosted or Azure OpenAI transcription deployment answers uploads, not streams.
    #[tokio::test]
    async fn an_upload_only_stt_deployment_is_refused_by_name() {
        let (state, _) = plane(
            &[
                (
                    "whisper",
                    "ep-whisper",
                    stt_entry(json!({"vendor": "self_hosted", "api_base": "http://whisper:8000"})),
                ),
                ("tts-el", "ep-tts", tts_entry(json!({}))),
            ],
            &[],
        )
        .await;
        let mut stt = stt_cfg(json!({"model": "whisper"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let refusal = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap_err();
        assert_eq!(refusal.code, "unsupported_deployment");
        assert!(refusal.message.contains("/v1/audio/transcriptions"));
    }

    /// 🔒 An AWS deployment without its key pair would authenticate as the GATEWAY's identity; it
    /// is refused as REST refuses it.
    #[tokio::test]
    async fn an_aws_deployment_without_its_key_pair_is_refused() {
        let (state, _) = plane(
            &[
                ("stt-dg", "ep-stt", stt_entry(json!({}))),
                (
                    "polly",
                    "ep-polly",
                    tts_entry(json!({"vendor": "aws_polly", "credential": null,
                        "provider_params": {"region": "us-east-1"}})),
                ),
            ],
            &[],
        )
        .await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "polly"}));
        let refusal = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap_err();
        assert_eq!(refusal.code, "deployment_misconfigured", "{refusal:?}");
    }

    /// TC-WS-05 🔒 — a leg at its concurrency cap refuses the session with 1013, and the other
    /// leg's admission is released rather than leaked.
    #[tokio::test]
    async fn tc_ws_05_per_leg_admission_refuses_1013_and_releases_the_other_leg() {
        let (state, _) = plane(
            &[
                ("stt-dg", "ep-stt", stt_entry(json!({"max_concurrent": 1}))),
                ("tts-el", "ep-tts", tts_entry(json!({"max_concurrent": 1}))),
            ],
            &[],
        )
        .await;
        let held_tts = state.admit_deployment("ep-tts").await.expect("free");

        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let refusal = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap_err();
        assert_eq!(refusal.code, "concurrency_limit_exceeded");
        assert_eq!(refusal.close, Some(CLOSE_TRY_LATER));
        let stt_again = state.admit_deployment("ep-stt").await;
        assert!(stt_again.is_ok(), "the STT leg's slot was released");
        drop(stt_again);
        drop(held_tts);

        // And a session holds both slots for its whole life.
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-2")
            .await
            .unwrap();
        assert!(state.admit_deployment("ep-stt").await.is_err());
        drop(legs);
        assert!(state.admit_deployment("ep-stt").await.is_ok());
    }

    /// No credential, or one the plane rejects: refused and closed.
    #[tokio::test]
    async fn an_unauthenticated_session_is_refused_and_closed() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let refusal = prepare(&state, None, &mut stt, &mut tts, "s-1")
            .await
            .unwrap_err();
        assert_eq!(refusal.close, Some(CLOSE_REVOKED));
        let refusal = prepare(
            &state,
            Some(&cred("bud_not_a_key")),
            &mut stt,
            &mut tts,
            "s-1",
        )
        .await
        .unwrap_err();
        assert_eq!(refusal.code, "invalid_api_key");
        assert_eq!(refusal.close, Some(CLOSE_REVOKED));
    }

    /// TC-WS-11 🔒 — `stt.streaming` applies on `/ws`; the client's explicit choice wins; the
    /// batch features apply too.
    #[tokio::test]
    async fn tc_ws_11_streaming_features_apply_on_ws() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg", "features": {"endpointing_ms": 800}}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s-1")
            .await
            .unwrap();
        assert_eq!(stt.features.interim_results, Some(true));
        assert_eq!(stt.features.utterance_end_ms, Some(1000));
        assert_eq!(
            stt.features.endpointing_ms,
            Some(800),
            "request > deployment"
        );
        assert_eq!(stt.features.diarization, Some(true));
        assert_eq!(stt.features.vad_events, None, "unset stays unset on /ws");
    }

    /// TC-WS-11 (REST half) — the upload path ignores `stt.streaming`.
    #[test]
    fn tc_ws_11_rest_ignores_streaming_features() {
        let settings: bud_auth::endpoint_config::SttSettings = serde_json::from_value(
            json!({"streaming": {"interim_results": true, "endpointing_ms": 300}}),
        )
        .unwrap();
        let features =
            endpoint_settings::stt_features_for(&settings, "deepgram", &mut Advisories::new());
        assert_eq!(features.interim_results, None);
        assert_eq!(features.endpointing_ms, None);
    }

    // ---------------------------------------------------------------------------------------
    // TC-WS-12 — metering
    // ---------------------------------------------------------------------------------------

    #[derive(Clone, Default)]
    struct Spans(Arc<Mutex<Vec<(u64, String, HashMap<String, String>)>>>);

    struct V<'a>(&'a mut HashMap<String, String>);
    impl Visit for V<'_> {
        fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
            self.0.insert(f.name().to_string(), format!("{v:?}"));
        }
        fn record_str(&mut self, f: &Field, v: &str) {
            self.0.insert(f.name().to_string(), v.to_string());
        }
    }

    struct Capture(Spans);
    impl<S> Layer<S> for Capture
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
            let name = ctx
                .span(id)
                .map(|s| s.name().to_string())
                .unwrap_or_default();
            let mut fields = HashMap::new();
            attrs.record(&mut V(&mut fields));
            self.0.0.lock().unwrap().push((id.into_u64(), name, fields));
        }
        fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
            let mut all = self.0.0.lock().unwrap();
            if let Some((_, _, fields)) = all.iter_mut().rev().find(|(i, _, _)| *i == id.into_u64())
            {
                values.record(&mut V(fields));
            }
        }
    }

    fn turns(spans: &Spans) -> Vec<HashMap<String, String>> {
        spans
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, name, _)| name == "voice.turn")
            .map(|(_, _, f)| f.clone())
            .collect()
    }

    /// TC-WS-12 🔒 — one utterance and one `speak`: an STT record with seconds and cost, a TTS
    /// record with characters and cost, each attributed to the caller and the deployment.
    #[tokio::test]
    async fn tc_ws_12_each_leg_is_metered_and_attributed() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "sess-12")
            .await
            .unwrap();
        let meter = legs.meter.clone();

        let spans = Spans::default();
        let subscriber = tracing_subscriber::registry().with(Capture(spans.clone()));
        tracing::subscriber::with_default(subscriber, || {
            // 1.5 s of 16 kHz mono PCM16, then its final transcript.
            meter.add_stt_audio(16_000 * 2 * 3 / 2);
            meter.stt_final("hello there");
            meter.stt_final("");
            meter.tts_spoken("Hello from Bud!");
            meter.tts_spoken("");
        });

        let records = turns(&spans);
        assert_eq!(
            records.len(),
            2,
            "an empty final or speak bills nothing: {records:?}"
        );
        let stt_rec = &records[0];
        assert_eq!(stt_rec["bud.voice.capability"], "audio_transcription");
        assert_eq!(stt_rec["bud.voice.audio_seconds"], "1.5");
        let cost: f64 = stt_rec["bud.voice.cost"].parse().unwrap();
        assert!((cost - 1.5 / 60.0 * 0.0043).abs() < 1e-12, "{cost}");
        assert_eq!(stt_rec["bud.voice.pricing_unit"], "minute");
        assert_eq!(stt_rec["bud.endpoint_id"], "ep-stt");
        assert_eq!(stt_rec["bud.voice.endpoint_name"], "stt-dg");
        assert_eq!(stt_rec["bud.project_id"], PROJECT);
        assert_eq!(stt_rec["bud.api_key_id"], API_KEY_ID);
        assert_eq!(stt_rec["bud.voice.session_id"], "sess-12");

        let tts_rec = &records[1];
        assert_eq!(tts_rec["bud.voice.capability"], "text_to_speech");
        assert_eq!(tts_rec["bud.voice.characters"], "15");
        let cost: f64 = tts_rec["bud.voice.cost"].parse().unwrap();
        assert!((cost - 15.0 / 1000.0 * 0.3).abs() < 1e-12, "{cost}");
        assert_eq!(tts_rec["bud.endpoint_id"], "ep-tts");
        assert_ne!(
            stt_rec["bud.voice.turn_index"],
            tts_rec["bud.voice.turn_index"]
        );
    }

    /// TC-WS-12 — audio streamed after the last final is billed at close; nothing is billed twice.
    #[tokio::test]
    async fn tc_ws_12_audio_after_the_last_final_is_billed_at_close() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s")
            .await
            .unwrap();
        let meter = legs.meter.clone();
        // The codec negotiation moved the session to 48 kHz stereo.
        meter.set_stt_format(48_000, 2);
        let spans = Spans::default();
        let subscriber = tracing_subscriber::registry().with(Capture(spans.clone()));
        tracing::subscriber::with_default(subscriber, || {
            meter.add_stt_audio(48_000 * 2 * 2);
            meter.finish();
            meter.finish();
        });
        let records = turns(&spans);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["bud.voice.audio_seconds"], "1.0");
    }

    // ---------------------------------------------------------------------------------------
    // TC-WS-13 — revalidation
    // ---------------------------------------------------------------------------------------

    /// TC-WS-13 🔒 — a revoked key, and an unpublished deployment, fail revalidation.
    #[tokio::test]
    async fn tc_ws_13_revocation_and_unpublishing_fail_revalidation() {
        let (state, store) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s")
            .await
            .unwrap();
        assert!(session_still_allowed(&state, &legs.meter).await);

        let plane = state.bud_mode.as_ref().unwrap().plane().clone();
        store.remove("voice_table:ep-tts");
        plane
            .on_key_event("voice_table:ep-tts", bud_auth::KeyEvent::Del)
            .await
            .unwrap();
        assert!(
            !session_still_allowed(&state, &legs.meter).await,
            "an unpublished leg deployment ends the session"
        );

        let (state, store) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s")
            .await
            .unwrap();
        let key = format!("api_key:{}", bud_auth::hash_api_key(KEY));
        store.remove(&key);
        state
            .bud_mode
            .as_ref()
            .unwrap()
            .plane()
            .on_key_event(&key, bud_auth::KeyEvent::Del)
            .await
            .unwrap();
        assert!(
            !session_still_allowed(&state, &legs.meter).await,
            "a revoked key"
        );
    }

    /// TC-WS-13 🔒 — a leg deployment republished without the capability the session uses it for
    /// (still in the table, no longer a text-to-speech deployment) fails revalidation.
    #[tokio::test]
    async fn tc_ws_13_a_leg_that_no_longer_serves_its_capability_fails_revalidation() {
        let (state, store) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s")
            .await
            .unwrap();
        assert!(session_still_allowed(&state, &legs.meter).await);

        let republished = tts_entry(json!({"endpoints": ["audio_transcription"]}));
        store.set(
            "voice_table:ep-tts",
            &json!({ "ep-tts": republished }).to_string(),
        );
        state
            .bud_mode
            .as_ref()
            .unwrap()
            .plane()
            .on_key_event("voice_table:ep-tts", bud_auth::KeyEvent::Set)
            .await
            .unwrap();
        assert!(
            state
                .bud_mode
                .as_ref()
                .unwrap()
                .plane()
                .voice_endpoint("ep-tts")
                .is_some(),
            "the deployment is still published"
        );
        assert!(
            !session_still_allowed(&state, &legs.meter).await,
            "but no longer serves text_to_speech"
        );
    }

    // ---------------------------------------------------------------------------------------
    // TC-WS-10 — DAG templates
    // ---------------------------------------------------------------------------------------

    fn template(nodes: Json) -> crate::dag::definition::DAGDefinition {
        serde_json::from_value(json!({
            "id": "t", "name": "t", "nodes": nodes, "edges": [],
            "entry_node": "in", "exit_nodes": []
        }))
        .expect("template parses")
    }

    /// TC-WS-10 🔒 — a template's TTS node resolves a deployment (vendor, model, voice,
    /// credential, admission, revalidation); its LLM node goes to the Bud gateway with the
    /// caller's credential and none of the template's own.
    #[tokio::test]
    #[serial_test::serial]
    async fn tc_ws_10_template_nodes_address_deployments() {
        unsafe { std::env::set_var("WAAV_LLM_BASE_URL", "http://bud-gateway:3000/v1/") };
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s")
            .await
            .unwrap();
        let mut def = template(json!([
            {"id": "in", "type": "audio_input"},
            {"id": "speak", "type": "tts_provider", "provider": "cartesia", "model": "tts-el",
             "config": {"api_key": "${CARTESIA_API_KEY}"}},
            {"id": "think", "type": "llm_endpoint", "base_url": "https://api.openai.com/v1",
             "model": "chat-deployment", "api_key": "${OPENAI_API_KEY}",
             "headers": {"Authorization": "Bearer sk-template", "X-Trace": "keep"}}
        ]));
        let session_credential = cred(KEY);
        let admissions = bind_dag(&state, &mut def, &session_credential, &legs.meter, "s")
            .await
            .expect("binds");
        unsafe { std::env::remove_var("WAAV_LLM_BASE_URL") };

        assert_eq!(admissions.len(), 1, "the TTS node's deployment is admitted");
        let speak = &def.nodes[1];
        match &speak.node_type {
            crate::dag::definition::NodeType::TtsProvider {
                provider,
                model,
                voice_id,
            } => {
                assert_eq!(provider, "elevenlabs");
                assert_eq!(model.as_deref(), Some("eleven_flash_v2_5"));
                assert_eq!(voice_id.as_deref(), Some("JBFqnCBsd6RMkjVDRZzb"));
            }
            other => panic!("{other:?}"),
        }
        let binding = speak.bud.as_ref().expect("bound");
        assert_eq!(
            binding.vendor_credential.as_deref(),
            Some(TEST_CREDENTIAL_PLAIN)
        );
        assert!(speak.config.get("api_key").is_none());
        assert!(
            legs.meter
                .endpoints()
                .iter()
                .filter(|(id, _)| id == "ep-tts")
                .count()
                == 2,
            "the node's deployment is revalidated with the session"
        );

        let think = &def.nodes[2];
        match &think.node_type {
            crate::dag::definition::NodeType::LlmEndpoint {
                base_url,
                api_key,
                headers,
                model,
                ..
            } => {
                assert_eq!(base_url, "http://bud-gateway:3000/v1");
                assert!(api_key.is_none());
                assert_eq!(model, "chat-deployment");
                assert!(
                    !headers
                        .keys()
                        .any(|k| k.eq_ignore_ascii_case("authorization"))
                );
                assert_eq!(headers.get("X-Trace").map(String::as_str), Some("keep"));
            }
            other => panic!("{other:?}"),
        }
        let binding = think.bud.as_ref().expect("bound");
        session_credential.replace("bud_refreshed");
        assert_eq!(
            binding
                .session_credential
                .as_ref()
                .map(|c| c.current())
                .as_deref(),
            Some("bud_refreshed"),
            "the node reads the session's credential per call"
        );
    }

    /// TC-WS-10 — a template node naming a deployment the caller cannot reach, or a realtime
    /// provider node, is refused.
    #[tokio::test]
    #[serial_test::serial]
    async fn tc_ws_10_unreachable_and_realtime_nodes_are_refused() {
        unsafe { std::env::set_var("WAAV_LLM_BASE_URL", "http://bud-gateway:3000/v1") };
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s")
            .await
            .unwrap();
        let mut def = template(json!([
            {"id": "speak", "type": "tts_provider", "provider": "elevenlabs", "model": "their-tts"}
        ]));
        let refusal = bind_dag(&state, &mut def, &cred(KEY), &legs.meter, "s")
            .await
            .unwrap_err();
        assert_eq!(refusal.code, "model_not_found");

        let mut def = template(json!([
            {"id": "rt", "type": "realtime_provider", "provider": "openai", "model": "gpt-realtime"}
        ]));
        let refusal = bind_dag(&state, &mut def, &cred(KEY), &legs.meter, "s")
            .await
            .unwrap_err();
        unsafe { std::env::remove_var("WAAV_LLM_BASE_URL") };
        assert_eq!(refusal.code, "unsupported_node");
    }

    /// The same-caller rule an `auth` refresh is held to (TC-WS-08).
    #[tokio::test]
    async fn the_meter_knows_its_caller() {
        let (state, _) = default_plane().await;
        let mut stt = stt_cfg(json!({"model": "stt-dg"}));
        let mut tts = tts_cfg(json!({"model": "tts-el"}));
        let legs = prepare(&state, Some(&cred(KEY)), &mut stt, &mut tts, "s")
            .await
            .unwrap();
        let same = CallerCheck::ApiKey {
            hashed: bud_auth::hash_api_key(KEY),
            client_key: false,
        };
        let other = CallerCheck::ApiKey {
            hashed: bud_auth::hash_api_key(OTHER_KEY),
            client_key: false,
        };
        assert!(legs.meter.same_caller(&same));
        assert!(!legs.meter.same_caller(&other));
    }
}

//! One relayed session: upgrade → authenticate → resolve → admit → connect → relay → close
//! (FRD-023 §5.4, §5.5, §5.6; D-8, D-13, D-14, D-17).
//!
//! Everything that can be refused is refused BEFORE the upgrade, as HTTP with OpenAI's error
//! envelope. After the upgrade a failure is an `error` event followed by a close with the D-14
//! code, and every exit path runs the same teardown: the session span, the metrics, and — by
//! dropping — the deployment's concurrency slot and the connection slot.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as UpMessage;
use tracing::{debug, info, warn};

use bud_auth::{AliasMetadata, Principal, PrincipalKind, RealtimeSettings, VoiceEndpoint};

use crate::auth::ephemeral::{self, OpenError, Parent};
use crate::core::deployment_policy::{Admission, Rejection, vendor_key};
use crate::middleware::connection_limit::ConnectionSlot;
use crate::state::AppState;

use super::handshake::{
    self, Credential, HandshakeError, MAX_MESSAGE_BYTES, REALTIME_CAPABILITY, SUBPROTOCOL,
};
use super::metering::{Attribution, SessionMeter};
use super::policy::{self, ClientOutcome, ClientRules, Tap, VendorOutcome};
use super::upstream::{self, UpstreamRequest};

/// Session timings (D-13). From the environment, with the FRD's defaults; tests shorten them.
#[derive(Debug, Clone)]
pub struct Timings {
    pub ping: Duration,
    pub max_missed_pongs: u32,
    pub revalidate: Duration,
    pub connect: Duration,
    /// How long client frames are held for the vendor to apply the deployment defaults.
    pub hold: Duration,
    /// How long a full client-bound queue is tolerated before `client_too_slow`.
    pub slow_client: Duration,
    /// The ceiling on any deployment's maximum session length.
    pub max_session: Duration,
    pub default_idle: Duration,
    pub warn_before: Duration,
    pub segment: Duration,
    pub upstream_send: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            ping: Duration::from_secs(20),
            max_missed_pongs: 3,
            revalidate: Duration::from_secs(30),
            connect: Duration::from_secs(10),
            hold: Duration::from_secs(5),
            slow_client: Duration::from_secs(5),
            max_session: Duration::from_secs(3600),
            default_idle: Duration::from_secs(300),
            warn_before: Duration::from_secs(60),
            segment: Duration::from_secs(60),
            upstream_send: Duration::from_secs(10),
        }
    }
}

impl Timings {
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let secs = |k: &str, dflt: Duration| {
            get(k)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|v| *v > 0)
                .map(Duration::from_secs)
                .unwrap_or(dflt)
        };
        Self {
            ping: secs("WAAV_REALTIME_PING_SECS", d.ping),
            revalidate: secs("WAAV_REALTIME_REVALIDATE_SECS", d.revalidate),
            max_session: secs("WAAV_REALTIME_MAX_SESSION_SECS", d.max_session),
            default_idle: secs("WAAV_REALTIME_IDLE_SECS", d.default_idle),
            ..d
        }
    }
}

/// Process-wide realtime state, built once at startup.
#[derive(Debug, Default)]
pub struct RealtimeRuntime {
    pub timings: Timings,
    /// `None`: client secrets are not configured (the mint route answers 501, `ek_bud_` refused).
    pub client_secret_keys: Option<ephemeral::ClientSecretKeys>,
}

impl RealtimeRuntime {
    /// Fails on a bad `WAAV_CLIENT_SECRET_KEYS` (TC-EK-16): a pod must not start with keys it
    /// cannot use.
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            timings: Timings::from_env(),
            client_secret_keys: ephemeral::ClientSecretKeys::from_env()?,
        })
    }
}

/// How a caller is re-checked during the session (D-17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerCheck {
    ApiKey { hashed: String, client_key: bool },
    Jwt { sub: String },
}

/// An authenticated caller.
#[derive(Debug, Clone)]
pub struct Caller {
    pub principal: Principal,
    pub check: CallerCheck,
    /// Minted by a client secret (the principal is the parent's).
    pub via_client_secret: bool,
}

/// Does the caller still reach the endpoint, and is it still a realtime deployment?
pub async fn still_allowed(
    state: &AppState,
    check: &CallerCheck,
    endpoint_id: &str,
) -> Option<AliasMetadata> {
    let plane = state.bud_mode.as_ref()?.plane();
    let serving = plane
        .voice_endpoint(endpoint_id)
        .is_some_and(|e| e.serves(REALTIME_CAPABILITY));
    if !serving {
        return None;
    }
    match check {
        CallerCheck::ApiKey { hashed, client_key } => {
            plane.hash_reaches(hashed, endpoint_id, *client_key)
        }
        CallerCheck::Jwt { sub } => plane.subject_reaches(sub, endpoint_id).await,
    }
}

/// The per-response quota hook (NG-8, §5.4): always `Allow` until FRD-021 Q-1 decides it.
pub fn admit_response(_caller: &Caller, _endpoint_id: &str) -> bool {
    true
}

fn not_found(model: &str) -> HandshakeError {
    HandshakeError::new(
        StatusCode::NOT_FOUND,
        "model_not_found",
        format!("Model '{model}' not found or does not support {REALTIME_CAPABILITY}."),
    )
    .param("model")
}

/// Authenticate a Bud key or JWT.
pub async fn authenticate(
    state: &AppState,
    credential: &Credential,
) -> Result<Caller, HandshakeError> {
    let Some(bud) = state.bud_mode.as_ref() else {
        return Err(bud_mode_required());
    };
    let raw = credential.expose();
    match bud.plane().authenticate(raw).await {
        Ok(principal) => {
            let check = match principal.via {
                PrincipalKind::ApiKey => CallerCheck::ApiKey {
                    hashed: bud_auth::hash_api_key(raw),
                    client_key: raw.starts_with("bud_client"),
                },
                PrincipalKind::Jwt => CallerCheck::Jwt {
                    sub: principal.user_id.clone().unwrap_or_default(),
                },
            };
            Ok(Caller {
                principal,
                check,
                via_client_secret: false,
            })
        }
        Err(bud_auth::AuthFailure::NotReady) => Err(HandshakeError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "not_ready",
            "The gateway is still loading its control plane; retry shortly.",
        )
        .retry_after(1)),
        Err(bud_auth::AuthFailure::Throttled) => Err(HandshakeError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limit_exceeded",
            "Too many authentication failures; retry later.",
        )
        .retry_after(1)),
        Err(bud_auth::AuthFailure::JwtRejected) => Err(HandshakeError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "The bearer token (a JWT) was rejected: check that it has not expired and that its \
             client is allowed on this gateway.",
        )),
        Err(_) => Err(HandshakeError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "Invalid API key.",
        )),
    }
}

fn bud_mode_required() -> HandshakeError {
    HandshakeError::new(
        StatusCode::NOT_FOUND,
        "bud_mode_required",
        "/v1/realtime serves Bud deployments and this gateway has no Bud control plane; \
         WaaV's native realtime protocol is at /realtime.",
    )
}

/// Open and check an `ek_bud_` secret against `?model` and its parent (§5.8 "Validation").
async fn authenticate_client_secret(
    state: &AppState,
    credential: &Credential,
    model: &str,
) -> Result<(Caller, String, Option<AliasMetadata>), HandshakeError> {
    if state.bud_mode.is_none() {
        return Err(bud_mode_required());
    }
    let Some(keys) = state.realtime.client_secret_keys.as_ref() else {
        return Err(HandshakeError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_client_secret",
            "Client secrets are not enabled on this gateway.",
        ));
    };
    let claims = keys
        .open(credential.expose(), ephemeral::now_epoch())
        .map_err(|e| match e {
            OpenError::Expired => HandshakeError::new(
                StatusCode::UNAUTHORIZED,
                "client_secret_expired",
                "The client secret has expired; mint a new one.",
            ),
            _ => HandshakeError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_client_secret",
                "The client secret is not valid.",
            ),
        })?;
    if model != claims.alias && model != claims.ep {
        return Err(HandshakeError::new(
            StatusCode::FORBIDDEN,
            "model_mismatch",
            "This client secret was minted for a different deployment.",
        )
        .param("model"));
    }
    let (check, via) = match &claims.parent {
        Parent::ApiKey { h, ck } => (
            CallerCheck::ApiKey {
                hashed: h.clone(),
                client_key: *ck,
            },
            PrincipalKind::ApiKey,
        ),
        Parent::Jwt { sub } => (CallerCheck::Jwt { sub: sub.clone() }, PrincipalKind::Jwt),
    };
    let alias = still_allowed(state, &check, &claims.ep).await;
    if alias.is_none() {
        return Err(HandshakeError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_client_secret",
            "The credential that minted this client secret no longer reaches the deployment.",
        ));
    }
    let caller = Caller {
        principal: Principal {
            project_id: claims.pid.clone(),
            api_key_id: claims.akid.clone(),
            user_id: claims.uid.clone(),
            via,
            expires_at: None,
        },
        check,
        via_client_secret: true,
    };
    Ok((caller, claims.ep, alias))
}

/// Everything a session needs, decided before the upgrade.
pub struct Prepared {
    pub caller: Caller,
    pub endpoint_id: String,
    pub endpoint_name: String,
    pub endpoint: VoiceEndpoint,
    pub alias: Option<AliasMetadata>,
    pub settings: Option<RealtimeSettings>,
    pub rules: ClientRules,
    pub upstream: UpstreamRequest,
    pub vkey: String,
    /// Held for the session: the deployment's concurrency slot (D-8).
    pub admission: Admission,
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prepared")
            .field("endpoint_id", &self.endpoint_id)
            .field("endpoint_name", &self.endpoint_name)
            .field("vendor", &self.endpoint.vendor)
            .finish()
    }
}

fn rejection_error(rej: Rejection) -> HandshakeError {
    let retry = rej.retry_after().as_secs().max(1);
    match rej {
        Rejection::Rate(_) => HandshakeError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limit_exceeded",
            "This deployment's rate limit was reached; retry after the indicated delay.",
        ),
        Rejection::Concurrency(_) => HandshakeError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "concurrency_limit_exceeded",
            "This deployment's concurrent-session limit was reached; retry after the indicated delay.",
        ),
    }
    .retry_after(retry)
}

/// Authenticate, resolve, admit and build the vendor request (FRD §5.3 steps 1-5).
pub async fn prepare(
    state: &AppState,
    query: Option<&str>,
    headers: &HeaderMap,
) -> Result<Prepared, HandshakeError> {
    let hs = handshake::parse(query, headers)?;
    debug!(model = %hs.model, source = hs.credential.source.as_str(), "realtime handshake parsed");
    if state.bud_mode.is_none() {
        return Err(bud_mode_required());
    }

    let (caller, endpoint_id, alias) = if hs.credential.is_client_secret() {
        authenticate_client_secret(state, &hs.credential, &hs.model).await?
    } else {
        let caller = authenticate(state, &hs.credential).await?;
        let resolved = state
            .resolve_voice_endpoint(&hs.model, REALTIME_CAPABILITY, Some(hs.credential.expose()))
            .ok_or_else(|| not_found(&hs.model))?;
        (caller, resolved.endpoint_id, resolved.alias)
    };

    debug!(endpoint_id = %endpoint_id, "realtime caller authenticated and endpoint resolved");
    let plane = state
        .bud_mode
        .as_ref()
        .map(|b| b.plane())
        .ok_or_else(bud_mode_required)?;
    let endpoint = plane
        .voice_endpoint(&endpoint_id)
        .filter(|e| e.serves(REALTIME_CAPABILITY))
        .ok_or_else(|| not_found(&hs.model))?;
    let settings = endpoint.config.realtime.clone();
    let transcription = settings
        .as_ref()
        .is_some_and(RealtimeSettings::is_transcription);

    // Build before admitting: a deployment the relay cannot serve must not take a slot.
    let upstream_req = upstream::build(&endpoint, transcription).map_err(|e| match e {
        upstream::UpstreamError::UnsupportedVendor(_) => HandshakeError::new(
            StatusCode::NOT_IMPLEMENTED,
            "unsupported_vendor",
            e.to_string(),
        ),
        other => HandshakeError::new(StatusCode::BAD_GATEWAY, "upstream_error", other.to_string()),
    })?;
    upstream::validate(&upstream_req).await.map_err(|e| {
        HandshakeError::new(StatusCode::BAD_GATEWAY, "upstream_error", e.to_string())
    })?;

    debug!(endpoint_id = %endpoint_id, "realtime upstream request validated");
    let vkey = vendor_key(&endpoint.vendor, endpoint.api_base.as_deref());
    if let Some(p) = &state.policies
        && let Err(open) = p.breakers().check(&endpoint_id, &vkey)
    {
        return Err(HandshakeError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "circuit_open",
            "This deployment's vendor is failing; its circuit breaker is open.",
        )
        .retry_after(open.retry_in.as_secs()));
    }

    let admission = state
        .admit_deployment(&endpoint_id)
        .await
        .map_err(rejection_error)?;
    debug!(endpoint_id = %endpoint_id, "realtime session admitted");

    Ok(Prepared {
        rules: ClientRules::from_settings(settings.as_ref()),
        caller,
        endpoint_name: hs.model,
        endpoint_id,
        endpoint,
        alias,
        settings,
        upstream: upstream_req,
        vkey,
        admission,
    })
}

/// `GET /v1/realtime` (FR-RT-1).
pub async fn realtime_ws_handler(
    State(state): State<Arc<AppState>>,
    uri: Uri,
    headers: HeaderMap,
    slot: Option<Extension<ConnectionSlot>>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let prepared = match prepare(&state, uri.query(), &headers).await {
        Ok(p) => p,
        Err(e) => {
            metrics::counter!("waav_realtime_refusals_total", "code" => e.code).increment(1);
            return e.into_response();
        }
    };
    let Ok(ws) = ws else {
        return HandshakeError::new(
            StatusCode::BAD_REQUEST,
            "websocket_required",
            "/v1/realtime is a WebSocket endpoint; send an Upgrade request.",
        )
        .into_response();
    };
    let slot = slot.map(|Extension(s)| s);
    ws.protocols([SUBPROTOCOL])
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(state, prepared, socket, slot))
}

/// How a session ended.
#[derive(Debug, Clone)]
pub struct End {
    pub reason: &'static str,
    pub close_code: u16,
    /// The `error` event sent before the close: (code, message).
    pub error: Option<(&'static str, String)>,
}

impl End {
    fn new(reason: &'static str, close_code: u16) -> Self {
        Self {
            reason,
            close_code,
            error: None,
        }
    }

    fn with_error(mut self, code: &'static str, message: impl Into<String>) -> Self {
        self.error = Some((code, message.into()));
        self
    }
}

enum Outbound {
    Frame(Message),
    Close {
        error: Option<String>,
        code: u16,
        reason: String,
    },
}

fn next_event_id() -> String {
    format!("evt_bud_{}", uuid::Uuid::new_v4().simple())
}

fn gateway_error(
    code: &str,
    message: &str,
    param: Option<&str>,
    client_event_id: Option<&str>,
) -> String {
    let kind = match code {
        "upstream_error" | "server_shutdown" | "client_too_slow" => "server_error",
        _ => "invalid_request_error",
    };
    policy::error_event(
        &next_event_id(),
        kind,
        code,
        message,
        param,
        client_event_id,
    )
}

/// The client-bound writer: a bounded queue drained by its own task, so a slow client applies
/// backpressure without the relay dropping a frame (FR-EVT-4).
fn spawn_writer(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    capacity: usize,
) -> (mpsc::Sender<Outbound>, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<Outbound>(capacity);
    let task = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Outbound::Frame(m) => {
                    if sink.send(m).await.is_err() {
                        return;
                    }
                }
                Outbound::Close {
                    error,
                    code,
                    reason,
                } => {
                    if let Some(e) = error {
                        let _ = sink.send(Message::Text(e.into())).await;
                    }
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code,
                            reason: reason.into(),
                        })))
                        .await;
                    let _ = sink.close().await;
                    return;
                }
            }
        }
    });
    (tx, task)
}

/// The client-bound queue: 2 s of 24 kHz audio at 20 ms frames, plus headroom for events.
const CLIENT_QUEUE: usize = 256;
/// Client frames held while the vendor applies the defaults.
const MAX_HELD: usize = 4096;

struct Relay<'a> {
    state: &'a AppState,
    p: &'a Prepared,
    meter: SessionMeter,
    client_tx: mpsc::Sender<Outbound>,
    up_tx: futures_util::stream::SplitSink<upstream::UpstreamSocket, UpMessage>,
    timings: Timings,
    /// Client frames wait here until the vendor has applied the deployment defaults (§5.6).
    held: VecDeque<(String, bool)>,
    ready: bool,
    /// The `event_id` of the defaults update in flight, and its deadline.
    awaiting_defaults: Option<String>,
    hold_deadline: Option<Instant>,
    last_activity: Instant,
    client_missed: u32,
    vendor_missed: u32,
}

impl Relay<'_> {
    async fn to_client(&self, text: String) -> Result<(), End> {
        let started = std::time::Instant::now();
        match tokio::time::timeout(
            self.timings.slow_client,
            self.client_tx
                .send(Outbound::Frame(Message::Text(text.into()))),
        )
        .await
        {
            Ok(Ok(())) => {
                metrics::histogram!("waav_realtime_relay_latency_seconds", "direction" => "vendor_to_client")
                    .record(started.elapsed().as_secs_f64());
                Ok(())
            }
            Ok(Err(_)) => Err(End::new("client_close", 1006)),
            Err(_) => Err(End::new("client_too_slow", 1011).with_error(
                "client_too_slow",
                "The client did not read the session's output fast enough; audio is never dropped \
                 silently, so the session is closed.",
            )),
        }
    }

    async fn send_to_vendor(&mut self, text: String) -> Result<(), End> {
        let started = std::time::Instant::now();
        match tokio::time::timeout(
            self.timings.upstream_send,
            self.up_tx.send(UpMessage::Text(text.into())),
        )
        .await
        {
            Ok(Ok(())) => {
                metrics::histogram!("waav_realtime_relay_latency_seconds", "direction" => "client_to_vendor")
                    .record(started.elapsed().as_secs_f64());
                Ok(())
            }
            _ => Err(End::new("upstream_error", 1011)
                .with_error("upstream_error", "The connection to the vendor failed.")),
        }
    }

    async fn refuse(
        &self,
        code: &str,
        message: &str,
        param: Option<&str>,
        event_id: Option<&str>,
    ) -> Result<(), End> {
        self.to_client(gateway_error(code, message, param, event_id))
            .await
    }

    async fn revalidate(&self) -> Result<(), End> {
        if still_allowed(self.state, &self.p.caller.check, &self.p.endpoint_id)
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

    /// Forward one policy-approved client frame, revalidating a `response.create` first.
    async fn forward_client(&mut self, text: String, response_create: bool) -> Result<(), End> {
        if response_create {
            self.revalidate().await?;
            if !admit_response(&self.p.caller, &self.p.endpoint_id) {
                return self
                    .refuse(
                        "quota_exceeded",
                        "The project's spend quota is exhausted.",
                        None,
                        None,
                    )
                    .await;
            }
        }
        self.send_to_vendor(text).await
    }

    async fn release_held(&mut self) -> Result<(), End> {
        self.ready = true;
        self.awaiting_defaults = None;
        self.hold_deadline = None;
        while let Some((text, response_create)) = self.held.pop_front() {
            self.forward_client(text, response_create).await?;
        }
        Ok(())
    }

    async fn on_client_text(&mut self, raw: &str) -> Result<(), End> {
        self.last_activity = Instant::now();
        match policy::client_event(raw, &self.p.rules) {
            ClientOutcome::Invalid(why) => {
                self.refuse(
                    "invalid_event",
                    &format!("The event could not be read: {why}"),
                    None,
                    None,
                )
                .await
            }
            ClientOutcome::Refuse(r) => {
                metrics::counter!("waav_realtime_policy_refusals_total", "field" => r.param.clone())
                    .increment(1);
                self.refuse(
                    "event_not_allowed",
                    &r.message,
                    Some(&r.param),
                    r.event_id.as_deref(),
                )
                .await
            }
            ClientOutcome::Forward {
                text,
                response_create,
                unknown,
                ..
            } => {
                if unknown {
                    metrics::counter!("waav_realtime_unknown_client_events_total").increment(1);
                }
                if !self.ready {
                    if self.held.len() >= MAX_HELD {
                        return Err(End::new("upstream_error", 1011).with_error(
                            "upstream_error",
                            "The vendor session did not become ready.",
                        ));
                    }
                    self.held.push_back((text.into_owned(), response_create));
                    return Ok(());
                }
                self.forward_client(text.into_owned(), response_create)
                    .await
            }
        }
    }

    async fn on_vendor_text(&mut self, raw: &str) -> Result<(), End> {
        self.last_activity = Instant::now();
        match policy::vendor_event(raw, &self.p.endpoint_name) {
            VendorOutcome::Drop => Ok(()),
            VendorOutcome::Invalid => self.to_client(raw.to_string()).await,
            VendorOutcome::Forward(text, tap) => {
                let text = text.into_owned();
                match tap {
                    Tap::SessionCreated { vendor_session_id } => {
                        self.meter.set_vendor_session_id(vendor_session_id);
                        self.to_client(text).await?;
                        let event_id =
                            format!("evt_bud_defaults_{}", uuid::Uuid::new_v4().simple());
                        match policy::defaults_update(
                            self.p.settings.as_ref(),
                            self.p.endpoint.model.as_deref(),
                            &event_id,
                        ) {
                            Some(update) => {
                                self.send_to_vendor(update).await?;
                                self.awaiting_defaults = Some(event_id);
                                self.hold_deadline = Some(Instant::now() + self.timings.hold);
                                Ok(())
                            }
                            None => self.release_held().await,
                        }
                    }
                    Tap::SessionUpdated { .. } => {
                        self.to_client(text).await?;
                        if self.awaiting_defaults.is_some() {
                            self.release_held().await?;
                        }
                        Ok(())
                    }
                    Tap::ResponseDone(event) => {
                        self.to_client(text).await?;
                        self.meter.response_done(&event);
                        Ok(())
                    }
                    Tap::TranscriptionCompleted(event) => {
                        self.to_client(text).await?;
                        self.meter.transcription_completed(&event);
                        Ok(())
                    }
                    Tap::Error(event) => {
                        let about_defaults = self.awaiting_defaults.as_deref().is_some_and(|id| {
                            event
                                .get("error")
                                .and_then(|e| e.get("event_id"))
                                .and_then(|v| v.as_str())
                                == Some(id)
                        });
                        self.to_client(text).await?;
                        if about_defaults {
                            // A deployment whose defaults the vendor refuses still serves the
                            // session on the vendor's defaults; the client has seen the error.
                            warn!(endpoint_id = %self.p.endpoint_id, "vendor refused the deployment defaults");
                            self.release_held().await?;
                        }
                        Ok(())
                    }
                    Tap::None => self.to_client(text).await,
                }
            }
        }
    }
}

async fn run(state: Arc<AppState>, p: Prepared, socket: WebSocket, slot: Option<ConnectionSlot>) {
    let _slot = slot;
    let timings = state.realtime.timings.clone();
    let session_id = format!("sess_bud_{}", uuid::Uuid::new_v4().simple());
    let vendor = p.endpoint.vendor.clone();
    let attribution = Attribution {
        project_id: p
            .alias
            .as_ref()
            .and_then(|a| a.project_id.clone())
            .or_else(|| p.caller.principal.project_id.clone()),
        endpoint_id: p.endpoint_id.clone(),
        model_id: p.alias.as_ref().and_then(|a| a.model_id.clone()),
        api_key_id: p.caller.principal.api_key_id.clone(),
        user_id: p.caller.principal.user_id.clone(),
        api_key_project_id: p.caller.principal.project_id.clone(),
        endpoint_name: p.endpoint_name.clone(),
        vendor: vendor.clone(),
        model: p.endpoint.model.clone(),
        session_type: p
            .settings
            .as_ref()
            .and_then(|s| s.session_type.clone())
            .unwrap_or_else(|| "realtime".into()),
    };
    let meter = SessionMeter::start(session_id.clone(), attribution, p.endpoint.pricing.clone());
    metrics::gauge!("waav_realtime_sessions_active", "vendor" => vendor.clone()).increment(1.0);
    info!(session_id = %session_id, endpoint_id = %p.endpoint_id, vendor = %vendor, "realtime session opened");

    debug!(session_id = %session_id, "realtime upgrade complete; connecting upstream");
    let (client_sink, mut client_rx) = socket.split();
    let (client_tx, writer) = spawn_writer(client_sink, CLIENT_QUEUE);

    let upstream_socket = match upstream::connect(&p.upstream, timings.connect).await {
        Ok(s) => {
            if let Some(pol) = &state.policies {
                pol.breakers().record_success(&p.endpoint_id, &p.vkey);
            }
            s
        }
        Err(e) => {
            warn!(session_id = %session_id, error = %e, "realtime vendor connect failed");
            if let Some(pol) = &state.policies {
                let verdict =
                    crate::core::deployment_policy::classify_message(&e.to_string(), None);
                pol.breakers()
                    .record_failure(&p.endpoint_id, &p.vkey, &verdict);
            }
            let end = End::new("upstream_error", 1011).with_error("upstream_error", e.to_string());
            finish(meter, end, &client_tx, writer, None, &vendor).await;
            return;
        }
    };
    debug!(session_id = %session_id, "realtime upstream connected");
    let (up_tx, mut up_rx) = upstream_socket.split();

    let now = Instant::now();
    let limits = p
        .settings
        .as_ref()
        .and_then(|s| s.limits.clone())
        .unwrap_or_default();
    let max_len = limits
        .max_session_seconds
        .map(Duration::from_secs)
        .map_or(timings.max_session, |d| d.min(timings.max_session));
    let idle = limits
        .idle_timeout_seconds
        .map(Duration::from_secs)
        .unwrap_or(timings.default_idle);
    let max_at = now + max_len;
    let warn_at = max_len.checked_sub(timings.warn_before).map(|d| now + d);
    let bills_duration = crate::core::realtime_cost::bills_duration(p.endpoint.pricing.as_ref());

    let mut relay = Relay {
        state: &state,
        p: &p,
        meter,
        client_tx: client_tx.clone(),
        up_tx,
        timings: timings.clone(),
        held: VecDeque::new(),
        ready: false,
        awaiting_defaults: None,
        // The vendor must say `session.created` within the hold window as well.
        hold_deadline: Some(now + timings.connect),
        last_activity: now,
        client_missed: 0,
        vendor_missed: 0,
    };
    let mut ping = tokio::time::interval_at(now + timings.ping, timings.ping);
    let mut revalidate = tokio::time::interval_at(now + timings.revalidate, timings.revalidate);
    let mut segment = tokio::time::interval_at(now + timings.segment, timings.segment);
    let mut last_segment = now;
    let mut warned = warn_at.is_none();

    let end: End = loop {
        let idle_at = relay.last_activity + idle;
        let hold_at = relay.hold_deadline;
        let step: Result<(), End> = tokio::select! {
            _ = state.shutdown.cancelled() => Err(End::new("drain", 1012)
                .with_error("server_shutdown", "The server is restarting; reconnect.")),
            msg = client_rx.next() => match msg {
                None | Some(Err(_)) => Err(End::new("client_close", 1006)),
                Some(Ok(Message::Close(frame))) => Err(End::new("client_close", frame.map_or(1005, |f| f.code))),
                Some(Ok(Message::Pong(_))) => { relay.client_missed = 0; Ok(()) }
                Some(Ok(Message::Ping(_))) => Ok(()),
                Some(Ok(Message::Binary(_))) => relay.refuse(
                    "invalid_event", "Binary frames are not part of the Realtime protocol; send JSON events.", None, None,
                ).await,
                Some(Ok(Message::Text(t))) => relay.on_client_text(t.as_str()).await,
            },
            msg = up_rx.next() => match msg {
                None | Some(Err(_)) => Err(End::new("upstream_error", 1011)
                    .with_error("upstream_error", "The connection to the vendor was lost.")),
                Some(Ok(UpMessage::Close(frame))) => {
                    let detail = frame.map(|f| format!(" (code {}: {})", u16::from(f.code), f.reason)).unwrap_or_default();
                    Err(End::new("upstream_error", 1011)
                        .with_error("upstream_error", format!("The vendor closed the session{detail}.")))
                }
                Some(Ok(UpMessage::Pong(_))) => { relay.vendor_missed = 0; Ok(()) }
                Some(Ok(UpMessage::Ping(_))) => { let _ = relay.up_tx.flush().await; Ok(()) }
                Some(Ok(UpMessage::Text(t))) => relay.on_vendor_text(t.as_str()).await,
                Some(Ok(_)) => Ok(()),
            },
            _ = ping.tick() => {
                if relay.client_missed >= timings.max_missed_pongs {
                    Err(End::new("client_timeout", 1011)
                        .with_error("client_timeout", "The client stopped answering pings."))
                } else if relay.vendor_missed >= timings.max_missed_pongs {
                    Err(End::new("upstream_error", 1011)
                        .with_error("upstream_error", "The vendor stopped answering pings."))
                } else {
                    relay.client_missed += 1;
                    relay.vendor_missed += 1;
                    let _ = relay.client_tx.try_send(Outbound::Frame(Message::Ping(Vec::new().into())));
                    match tokio::time::timeout(timings.upstream_send, relay.up_tx.send(UpMessage::Ping(Vec::new().into()))).await {
                        Ok(Ok(())) => Ok(()),
                        _ => Err(End::new("upstream_error", 1011)
                            .with_error("upstream_error", "The connection to the vendor failed.")),
                    }
                }
            }
            _ = revalidate.tick() => relay.revalidate().await,
            _ = tokio::time::sleep_until(idle_at) => Err(End::new("idle", 1000)
                .with_error("session_expired", format!("The session was idle for {} s.", idle.as_secs()))),
            _ = tokio::time::sleep_until(warn_at.unwrap_or(max_at)), if !warned => {
                warned = true;
                relay.refuse("session_expiring",
                    &format!("The session reaches its maximum length in {} s.", timings.warn_before.as_secs()),
                    None, None).await
            }
            _ = tokio::time::sleep_until(max_at) => Err(End::new("max_duration", 1000)
                .with_error("session_expired", format!("The session reached its maximum length of {} s.", max_len.as_secs()))),
            _ = tokio::time::sleep_until(hold_at.unwrap_or(max_at)), if hold_at.is_some() => Err(End::new("upstream_error", 1011)
                .with_error("upstream_error", "The vendor did not start or configure the session in time.")),
            _ = segment.tick(), if bills_duration => {
                relay.meter.duration_segment(timings.segment.as_secs_f64());
                last_segment = Instant::now();
                Ok(())
            }
        };
        if let Err(end) = step {
            break end;
        }
    };

    if bills_duration {
        // The final partial segment: a 150 s session bills 60 + 60 + 30 (TC-XL-07).
        relay
            .meter
            .duration_segment(last_segment.elapsed().as_secs_f64());
    }
    let Relay {
        meter, mut up_tx, ..
    } = relay;
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        up_tx.send(UpMessage::Close(Some(
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                reason: "".into(),
            },
        ))),
    )
    .await;
    finish(meter, end, &client_tx, writer, Some(&session_id), &vendor).await;
    drop(p);
}

/// The common teardown: the error event and close, the session record, the metrics.
async fn finish(
    meter: SessionMeter,
    end: End,
    client_tx: &mpsc::Sender<Outbound>,
    writer: tokio::task::JoinHandle<()>,
    session_id: Option<&str>,
    vendor: &str,
) {
    let error = end
        .error
        .as_ref()
        .map(|(code, message)| gateway_error(code, message, None, None));
    // Behind any backlog, so every frame queued before the close still arrives in order.
    let _ = tokio::time::timeout(
        Duration::from_secs(30),
        client_tx.send(Outbound::Close {
            error,
            code: end.close_code.clamp(1000, 4999),
            reason: end.reason.to_string(),
        }),
    )
    .await;
    let abort = writer.abort_handle();
    if tokio::time::timeout(Duration::from_secs(30), writer)
        .await
        .is_err()
    {
        abort.abort();
    }
    info!(
        session_id = session_id.unwrap_or(meter.session_id()),
        end_reason = end.reason,
        close_code = end.close_code,
        turns = meter.turns(),
        "realtime session closed"
    );
    metrics::counter!("waav_realtime_sessions_total", "vendor" => vendor.to_string(), "end_reason" => end.reason)
        .increment(1);
    metrics::gauge!("waav_realtime_sessions_active", "vendor" => vendor.to_string()).decrement(1.0);
    meter.finish(end.reason, end.close_code);
    debug!("realtime session torn down");
}

//! WebSocket message processing orchestrator
//!
//! This module serves as the main entry point for processing incoming WebSocket
//! messages, delegating to specialized handlers based on message type.

use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};

use crate::auth::{Auth, match_api_secret_id};
use crate::plugin::capabilities::{WSContext, WSResponse};
use crate::plugin::global_registry;
use crate::state::AppState;

use super::{
    audio_handler::{handle_audio_end, handle_clear_message, handle_speak_message},
    command_handler::{handle_send_message, handle_sip_transfer},
    config_handler::handle_config_message,
    messages::{IncomingMessage, MessageClass, MessageRoute, OutgoingMessage, send_with_policy},
    state::ConnectionState,
};

async fn send_critical(message_tx: &mpsc::Sender<MessageRoute>, route: MessageRoute) {
    send_with_policy(message_tx, route, MessageClass::Critical).await;
}

async fn send_error(message_tx: &mpsc::Sender<MessageRoute>, message: impl Into<String>) {
    send_critical(
        message_tx,
        MessageRoute::Outgoing(OutgoingMessage::Error {
            message: message.into(),
        }),
    )
    .await;
}

/// Process incoming WebSocket message based on its type
///
/// This is the main message router that delegates to specialized handlers
/// based on the message type. It maintains the separation of concerns by
/// routing audio, configuration, and command messages to their respective handlers.
///
/// # Arguments
/// * `msg` - The parsed incoming message from the WebSocket client
/// * `state` - Connection state shared across handlers
/// * `message_tx` - Channel for sending response messages back to the client
/// * `app_state` - Application state containing global configuration
///
/// # Returns
/// * `bool` - true to continue processing, false to terminate the connection
///
/// # Performance Notes
/// - Marked inline to reduce function call overhead in the hot path
/// - Delegates to specialized handlers for better code organization
#[inline]
pub async fn handle_incoming_message(
    msg: IncomingMessage,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    // Check if auth is pending - only Auth messages are allowed
    {
        let conn_state = state.read().await;
        if conn_state.auth.is_pending() {
            // Only allow Auth messages when auth is pending
            if !matches!(msg, IncomingMessage::Auth { .. }) {
                warn!("Received non-auth message while auth is pending, rejecting");
                send_error(
                    message_tx,
                    "Authentication required. Send auth message first.",
                )
                .await;
                // Close connection for security
                send_critical(message_tx, MessageRoute::Close).await;
                return false;
            }
        }
    }

    match msg {
        // Handle first-message authentication for browser clients
        IncomingMessage::Auth { token } => {
            handle_auth_message(token, state, message_tx, app_state).await
        }
        IncomingMessage::Config {
            stream_id,
            audio,
            audio_disabled,
            stt_config,
            tts_config,
            livekit,
            dag_config,
            conversation_config,
            agent,
            alias,
        } => {
            // Handle backward compatibility for audio_disabled field
            // Priority: audio field takes precedence if explicitly set
            // If only audio_disabled is set, invert it to get audio value
            let resolved_audio = if audio.is_some() {
                // Explicit audio field set - use it directly
                if audio_disabled.is_some() {
                    warn!(
                        "Both 'audio' and 'audio_disabled' fields present in config. \
                         Using 'audio' value. 'audio_disabled' is deprecated."
                    );
                }
                audio
            } else if let Some(disabled) = audio_disabled {
                // Legacy audio_disabled field - invert and warn
                warn!(
                    "'audio_disabled' is deprecated. Use 'audio: {}' instead.",
                    !disabled
                );
                Some(!disabled)
            } else {
                // Neither set - use default
                None
            };

            handle_config_message(
                stream_id,
                resolved_audio,
                stt_config,
                tts_config,
                livekit,
                dag_config,
                conversation_config,
                agent,
                alias,
                state,
                message_tx,
                app_state,
            )
            .await
        }
        IncomingMessage::Speak {
            text,
            flush,
            allow_interruption,
        } => handle_speak_message(text, flush, allow_interruption, state, message_tx).await,
        IncomingMessage::Clear => {
            // Spec 025: on a voice-agent session `clear` stops the agent too — the live turn ends
            // and its history keeps what was heard (a barge-in without speech).
            if let Some(engine) = state.read().await.agent.clone() {
                engine.cancel_response().await;
                return true;
            }
            handle_clear_message(state, message_tx).await
        }
        IncomingMessage::Truncate { audio_end_ms } => {
            match state.read().await.agent.clone() {
                Some(engine) => engine.truncate_at(audio_end_ms),
                None => {
                    send_error(message_tx, "truncate applies to voice-agent sessions only").await
                }
            }
            true
        }
        IncomingMessage::AgentInput { text } => {
            match state.read().await.agent.clone() {
                Some(engine) => engine.start_turn(text).await,
                None => {
                    send_error(
                        message_tx,
                        "agent_input applies to voice-agent sessions only",
                    )
                    .await
                }
            }
            true
        }
        IncomingMessage::SendMessage {
            message,
            role,
            topic,
            debug,
        } => handle_send_message(message, role, topic, debug, state, message_tx).await,
        IncomingMessage::SIPTransfer { transfer_to } => {
            handle_sip_transfer(transfer_to, state, message_tx, app_state).await
        }
        IncomingMessage::Custom {
            message_type,
            payload,
        } => handle_custom_message(message_type, payload, state, message_tx, app_state).await,
        IncomingMessage::AudioEnd => {
            let keep = handle_audio_end(state, message_tx).await;
            // Spec 025 manual turn detection: the end of the client's audio commits its turn. The
            // provider's last final lands within the finalize; give it a moment before committing.
            if let Some(engine) = state.read().await.agent.clone()
                && engine.is_manual()
            {
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    engine.commit_input().await;
                });
            }
            keep
        }
    }
}

/// Handle first-message authentication for browser clients
///
/// Validates the provided token against configured API secrets and updates
/// the connection's auth state. Only supports API secret mode for WebSocket
/// first-message auth (JWT would require additional network calls).
///
/// # Arguments
/// * `token` - The bearer token to validate
/// * `state` - Connection state to update on success
/// * `message_tx` - Channel for sending response messages
/// * `app_state` - Application state containing API secrets
///
/// # Returns
/// * `bool` - true on successful auth, false to close connection
/// Which authenticator the first-message path should consult.
///
/// Extracted so the ORDERING is testable, because getting it wrong is invisible: the previous
/// version consulted only `auth_api_secrets`, which a Bud deployment never configures, so
/// every deferred WebSocket auth was refused with "API secret authentication not configured"
/// no matter how good the caller's Bud key was. Browser clients cannot set an Authorization
/// header on a WebSocket, so that deferred path is their ONLY route in — the effect was that
/// no browser could open a voice session in Bud mode at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsAuthPath {
    /// Resolve against the Bud control plane, exactly as `auth_middleware` does.
    Bud,
    /// Standalone WaaV: match against the configured API secret list.
    ApiSecret,
    /// JWT-only deployment: validate against the external auth service, as `auth_middleware`
    /// does for HTTP. Without this a JWT-only gateway advertises first-message auth and then
    /// closes every socket, leaving `?token=` — which leaks the token into access logs — as the
    /// only way in.
    Jwt,
    /// Nothing is configured; there is nothing to authenticate against.
    Unconfigured,
}

/// Bud mode wins whenever it is present, then API secrets, then JWT.
///
/// Same precedence as `auth_middleware`, and for the same reason: when a control plane is
/// configured it IS the deployment's identity source, and falling through to a secret list
/// that a Bud chart never populates rejects valid credentials.
pub fn ws_auth_path(
    bud_mode_present: bool,
    has_api_secrets: bool,
    has_jwt_auth: bool,
) -> WsAuthPath {
    if bud_mode_present {
        WsAuthPath::Bud
    } else if has_api_secrets {
        WsAuthPath::ApiSecret
    } else if has_jwt_auth {
        WsAuthPath::Jwt
    } else {
        WsAuthPath::Unconfigured
    }
}

async fn handle_auth_message(
    token: String,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    let path = ws_auth_path(
        app_state.bud_mode.is_some(),
        app_state.config.has_api_secret_auth(),
        app_state.config.has_jwt_auth(),
    );

    if path == WsAuthPath::Bud && !state.read().await.auth.is_pending() {
        return refresh_bud_credential(token, state, message_tx, app_state).await;
    }

    let resolved: Option<String> = match path {
        WsAuthPath::Bud => {
            // Unwrap is safe: `ws_auth_path` returns Bud only when bud_mode is Some.
            let bud = app_state.bud_mode.as_ref().expect("bud mode present");
            match bud.authenticate(&token).await {
                Ok(auth) => {
                    // FRD-023 RT6: the session acts as this caller from here on.
                    let bearer = crate::handlers::openai_realtime::handshake::Credential::new(
                        token.clone(),
                        crate::handlers::openai_realtime::handshake::CredentialSource::Bearer,
                    );
                    let check =
                        crate::handlers::openai_realtime::session::authenticate(app_state, &bearer)
                            .await
                            .ok()
                            .map(|caller| caller.check);
                    let mut guard = state.write().await;
                    guard.credential = Some(crate::auth::SessionCredential::new(token.clone()));
                    guard.caller_check = check;
                    auth.id
                }
                Err(e) => {
                    warn!(error = ?e, "first-message bud authentication failed");
                    None
                }
            }
        }
        WsAuthPath::ApiSecret => {
            match_api_secret_id(&token, &app_state.config.auth_api_secrets).map(str::to_string)
        }
        WsAuthPath::Jwt => {
            // Route a first-message token through the SAME external auth service the HTTP
            // middleware uses (empty body/headers; the /ws upgrade context).
            let Some(auth_client) = app_state.auth_client.as_ref() else {
                warn!("First-message JWT auth attempted but auth client not initialized");
                send_error(message_tx, "Authentication service unavailable").await;
                send_critical(message_tx, MessageRoute::Close).await;
                return false;
            };
            match auth_client
                .validate_token(
                    &token,
                    &serde_json::Value::Object(serde_json::Map::new()),
                    std::collections::HashMap::new(),
                    "/ws",
                    "GET",
                )
                .await
            {
                Ok(auth) => auth.id,
                Err(e) => {
                    warn!(error = %e, "First-message JWT authentication failed");
                    None
                }
            }
        }
        WsAuthPath::Unconfigured => {
            warn!("First-message auth attempted but no authenticator is configured");
            send_error(
                message_tx,
                "Authentication is not configured on this gateway",
            )
            .await;
            send_critical(message_tx, MessageRoute::Close).await;
            return false;
        }
    };

    if let Some(id) = resolved {
        info!(auth_id = %id, ?path, "First-message authentication successful");
        {
            let mut conn_state = state.write().await;
            conn_state.auth = Auth::new(id.clone());
        }
        // Send authenticated response
        send_critical(
            message_tx,
            MessageRoute::Outgoing(OutgoingMessage::Authenticated { id: Some(id) }),
        )
        .await;

        true
    } else {
        warn!("First-message authentication failed: invalid token");
        send_error(message_tx, "Invalid authentication token").await;
        // Close connection on auth failure
        send_critical(message_tx, MessageRoute::Close).await;
        false
    }
}

/// An `auth` message on an authenticated Bud-mode session: a credential REFRESH (FRD-023 WP-RT6.2,
/// TC-WS-08). A Keycloak token lives minutes and a call longer, so a JWT caller keeps its voice
/// agent's LLM leg alive by sending a fresh token; the leg reads it on its next call.
///
/// The new credential must identify the SAME principal — the session's legs are admitted, billed
/// and authorized against it. A refresh that fails leaves the session and its current credential
/// as they were: the credential failing is what `auth_expired` reports, and revalidation is what
/// ends a session whose caller lost access.
async fn refresh_bud_credential(
    token: String,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    let bearer = crate::handlers::openai_realtime::handshake::Credential::new(
        token.clone(),
        crate::handlers::openai_realtime::handshake::CredentialSource::Bearer,
    );
    let caller =
        match crate::handlers::openai_realtime::session::authenticate(app_state, &bearer).await {
            Ok(caller) => caller,
            Err(e) => {
                warn!(code = e.code, "Bud-mode /ws credential refresh refused");
                send_error(
                    message_tx,
                    format!(
                        "auth_refresh_failed: {}. The session keeps its current credential.",
                        e.message
                    ),
                )
                .await;
                return true;
            }
        };
    let guard = state.read().await;
    let same = guard.caller_check.as_ref() == Some(&caller.check);
    let Some(credential) = guard.credential.clone().filter(|_| same) else {
        drop(guard);
        warn!("Bud-mode /ws credential refresh carried a different identity; refused");
        send_error(
            message_tx,
            "auth_refresh_refused: a refresh must renew this session's own credential (the same \
             API key or the same user); open a new connection to act as someone else.",
        )
        .await;
        return true;
    };
    credential.replace(token);
    let id = guard.auth.id.clone();
    drop(guard);
    info!("Bud-mode /ws credential refreshed");
    send_critical(
        message_tx,
        MessageRoute::Outgoing(OutgoingMessage::Authenticated { id }),
    )
    .await;
    true
}

/// Handle custom plugin message
///
/// Dispatches the message to all registered handlers for this message type.
/// Handlers are called in registration order, and all handlers are invoked
/// even if one fails (errors are logged but don't stop other handlers).
///
/// # Arguments
/// * `message_type` - The plugin-defined message type identifier
/// * `payload` - The message payload as JSON
/// * `state` - Connection state for this WebSocket session
/// * `message_tx` - Channel for sending response messages
/// * `app_state` - Application state containing global configuration
///
/// # Returns
/// * `bool` - Always true to continue processing (plugins can't close connections)
async fn handle_custom_message(
    message_type: String,
    payload: serde_json::Value,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    _app_state: &Arc<AppState>,
) -> bool {
    let registry = global_registry();
    let handlers = registry.get_ws_handlers(&message_type);

    if handlers.is_empty() {
        debug!(
            message_type = %message_type,
            "No handlers registered for custom message type"
        );
        send_error(
            message_tx,
            format!("Unknown custom message type: {}", message_type),
        )
        .await;
        return true;
    }

    // Build context for handlers
    let ctx = {
        let conn_state = state.read().await;
        WSContext {
            stream_id: conn_state.stream_id.clone().unwrap_or_default(),
            authenticated: !conn_state.auth.is_pending(),
            tenant_id: conn_state.auth.id.clone(),
        }
    };

    // Call all registered handlers
    for handler in handlers {
        match handler(payload.clone(), ctx.clone()).await {
            Ok(Some(response)) => {
                // Convert response to outgoing message
                match response {
                    WSResponse::Json(json) => {
                        send_critical(
                            message_tx,
                            MessageRoute::Outgoing(OutgoingMessage::PluginResponse {
                                message_type: message_type.clone(),
                                payload: json,
                            }),
                        )
                        .await;
                    }
                    WSResponse::Binary(data) => {
                        send_critical(message_tx, MessageRoute::Binary(data)).await;
                    }
                    WSResponse::Multiple(responses) => {
                        for resp in responses {
                            match resp {
                                WSResponse::Json(json) => {
                                    send_critical(
                                        message_tx,
                                        MessageRoute::Outgoing(OutgoingMessage::PluginResponse {
                                            message_type: message_type.clone(),
                                            payload: json,
                                        }),
                                    )
                                    .await;
                                }
                                WSResponse::Binary(data) => {
                                    send_critical(message_tx, MessageRoute::Binary(data)).await;
                                }
                                WSResponse::Multiple(_) => {
                                    // Don't recurse into nested multiples
                                    warn!("Nested Multiple responses not supported");
                                }
                                WSResponse::None => {}
                            }
                        }
                    }
                    WSResponse::None => {}
                }
            }
            Ok(None) => {
                // Handler processed but no response needed
                debug!(message_type = %message_type, "Plugin handler returned no response");
            }
            Err(e) => {
                warn!(
                    message_type = %message_type,
                    error = %e,
                    "Plugin handler failed"
                );
                send_error(message_tx, format!("Plugin handler error: {}", e)).await;
            }
        }
    }

    true
}

#[cfg(test)]
mod ws_auth_path_tests {
    use super::*;

    #[test]
    fn bud_mode_is_consulted_whenever_it_is_configured() {
        // THE regression. A Bud chart configures no auth_api_secrets, so consulting the
        // secret list first refused every valid Bud credential on the deferred path.
        assert_eq!(ws_auth_path(true, false, false), WsAuthPath::Bud);
    }

    #[test]
    fn bud_mode_outranks_a_configured_secret_list() {
        // Same precedence as auth_middleware: when a control plane exists it is the identity
        // source, and a leftover secret must not shadow it.
        assert_eq!(ws_auth_path(true, true, false), WsAuthPath::Bud);
    }

    #[test]
    fn bud_mode_outranks_the_external_auth_service() {
        // The merge hazard, pinned. Upstream fixed this same handler by routing first-message
        // tokens to the external auth service; a Bud deployment configures BOTH (it has a
        // control plane, and `has_jwt_auth()` is true), so resolving to Jwt here would send
        // every Bud credential to a service that has never heard of it — the exact 401-on-a-
        // valid-key outage the Bud path was added to fix.
        assert_eq!(ws_auth_path(true, false, true), WsAuthPath::Bud);
        assert_eq!(ws_auth_path(true, true, true), WsAuthPath::Bud);
    }

    #[test]
    fn standalone_waav_still_uses_its_api_secrets() {
        assert_eq!(ws_auth_path(false, true, false), WsAuthPath::ApiSecret);
        // Secrets outrank JWT, matching the HTTP middleware's order.
        assert_eq!(ws_auth_path(false, true, true), WsAuthPath::ApiSecret);
    }

    #[test]
    fn a_jwt_only_gateway_authenticates_rather_than_closing_the_socket() {
        // Upstream's G3 fix, kept. A browser cannot set an Authorization header on a WebSocket,
        // so first-message auth is its only route in; refusing here left `?token=` as the sole
        // alternative, which leaks the token into access logs.
        assert_eq!(ws_auth_path(false, false, true), WsAuthPath::Jwt);
    }

    #[test]
    fn with_nothing_configured_the_socket_is_refused_rather_than_admitted() {
        // Fail CLOSED. An unconfigured gateway must not treat "nothing to check against" as
        // "everything passes".
        assert_eq!(ws_auth_path(false, false, false), WsAuthPath::Unconfigured);
    }

    // -----------------------------------------------------------------------------------------
    // FRD-023 WP-RT6.2: `auth` on an authenticated Bud-mode session is a credential refresh.
    // -----------------------------------------------------------------------------------------

    const KEY: &str = "bud_ws_refresh_key";
    const OTHER_KEY: &str = "bud_ws_refresh_other_key";

    async fn refresh_plane() -> Arc<AppState> {
        let blob = |project: &str| {
            serde_json::json!({"__metadata__": {"api_key_id": "k", "user_id": "u",
                "api_key_project_id": project}})
            .to_string()
        };
        let a = (
            format!("api_key:{}", bud_auth::hash_api_key(KEY)),
            blob("p1"),
        );
        let b = (
            format!("api_key:{}", bud_auth::hash_api_key(OTHER_KEY)),
            blob("p2"),
        );
        crate::test_support::bud_state_with_credentials(&[
            (a.0.as_str(), a.1.as_str()),
            (b.0.as_str(), b.1.as_str()),
        ])
        .await
        .0
    }

    /// A session authenticated as KEY, as the upgrade leaves it.
    async fn authenticated(app_state: &Arc<AppState>) -> Arc<RwLock<ConnectionState>> {
        let state = Arc::new(RwLock::new(ConnectionState::with_auth(Auth::new("p1"))));
        let bearer = crate::handlers::openai_realtime::handshake::Credential::new(
            KEY,
            crate::handlers::openai_realtime::handshake::CredentialSource::Bearer,
        );
        let check = crate::handlers::openai_realtime::session::authenticate(app_state, &bearer)
            .await
            .unwrap()
            .check;
        let mut guard = state.write().await;
        guard.credential = Some(crate::auth::SessionCredential::new(KEY));
        guard.caller_check = Some(check);
        drop(guard);
        state
    }

    fn first_error(rx: &mut mpsc::Receiver<MessageRoute>) -> Option<String> {
        while let Ok(route) = rx.try_recv() {
            if let MessageRoute::Outgoing(OutgoingMessage::Error { message }) = route {
                return Some(message);
            }
        }
        None
    }

    /// TC-WS-08 🔒 — a refresh with the same identity replaces the session's credential.
    #[tokio::test]
    async fn tc_ws_08_a_refresh_renews_the_credential() {
        let app_state = refresh_plane().await;
        let state = authenticated(&app_state).await;
        let credential = state.read().await.credential.clone().unwrap();
        let (tx, mut rx) = mpsc::channel(8);

        // The same key, re-sent (a JWT caller sends a fresh token for the same subject).
        assert!(handle_auth_message(KEY.to_string(), &state, &tx, &app_state).await);
        assert!(matches!(
            rx.try_recv(),
            Ok(MessageRoute::Outgoing(
                OutgoingMessage::Authenticated { .. }
            ))
        ));
        assert_eq!(credential.current(), KEY);
    }

    /// TC-WS-08 🔒 — a refresh can never change who the session is.
    #[tokio::test]
    async fn tc_ws_08_a_refresh_to_another_identity_is_refused() {
        let app_state = refresh_plane().await;
        let state = authenticated(&app_state).await;
        let credential = state.read().await.credential.clone().unwrap();
        let (tx, mut rx) = mpsc::channel(8);

        let keep = handle_auth_message(OTHER_KEY.to_string(), &state, &tx, &app_state).await;
        assert!(keep, "the session continues on its own credential");
        let error = first_error(&mut rx).expect("an error frame");
        assert!(error.starts_with("auth_refresh_refused"), "{error}");
        assert_eq!(credential.current(), KEY, "unchanged");
    }

    /// A refresh that fails leaves the session and its credential as they were.
    #[tokio::test]
    async fn a_failed_refresh_keeps_the_session() {
        let app_state = refresh_plane().await;
        let state = authenticated(&app_state).await;
        let credential = state.read().await.credential.clone().unwrap();
        let (tx, mut rx) = mpsc::channel(8);

        let keep = handle_auth_message("bud_nope".to_string(), &state, &tx, &app_state).await;
        assert!(keep);
        let error = first_error(&mut rx).expect("an error frame");
        assert!(error.starts_with("auth_refresh_failed"), "{error}");
        assert_eq!(credential.current(), KEY);
    }

    /// First-message auth in Bud mode keeps the credential and fixes the identity.
    #[tokio::test]
    async fn first_message_auth_keeps_the_credential_for_the_session() {
        let app_state = refresh_plane().await;
        let state = Arc::new(RwLock::new(ConnectionState::with_auth(Auth::pending())));
        let (tx, _rx) = mpsc::channel(8);
        assert!(handle_auth_message(KEY.to_string(), &state, &tx, &app_state).await);
        let guard = state.read().await;
        assert_eq!(
            guard.credential.as_ref().map(|c| c.current()).as_deref(),
            Some(KEY)
        );
        assert!(guard.caller_check.is_some());
    }
}

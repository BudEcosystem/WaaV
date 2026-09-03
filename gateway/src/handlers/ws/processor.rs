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
        IncomingMessage::Clear => handle_clear_message(state, message_tx).await,
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
        IncomingMessage::AudioEnd => handle_audio_end(state, message_tx).await,
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

    let resolved: Option<String> = match path {
        WsAuthPath::Bud => {
            // Unwrap is safe: `ws_auth_path` returns Bud only when bud_mode is Some.
            let bud = app_state.bud_mode.as_ref().expect("bud mode present");
            match bud.authenticate(&token).await {
                Ok(auth) => auth.id,
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
    use super::{WsAuthPath, ws_auth_path};

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
}

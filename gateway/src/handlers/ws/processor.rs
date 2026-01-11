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
    audio_handler::{handle_clear_message, handle_speak_message},
    command_handler::{handle_send_message, handle_sip_transfer},
    config_handler::handle_config_message,
    messages::{IncomingMessage, MessageRoute, OutgoingMessage},
    state::ConnectionState,
};

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
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: "Authentication required. Send auth message first.".to_string(),
                    }))
                    .await;
                // Close connection for security
                let _ = message_tx.send(MessageRoute::Close).await;
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
async fn handle_auth_message(
    token: String,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    // Validate token against configured API secrets
    if !app_state.config.has_api_secret_auth() {
        warn!("First-message auth attempted but API secret auth not configured");
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: "API secret authentication not configured".to_string(),
            }))
            .await;
        let _ = message_tx.send(MessageRoute::Close).await;
        return false;
    }

    // Match token against configured API secrets
    if let Some(secret_id) = match_api_secret_id(&token, &app_state.config.auth_api_secrets) {
        let secret_id_owned = secret_id.to_string();
        info!(auth_id = %secret_id_owned, "First-message authentication successful");

        // Update connection state with authenticated auth
        {
            let mut conn_state = state.write().await;
            conn_state.auth = Auth::new(secret_id_owned.clone());
        }

        // Send authenticated response
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Authenticated {
                id: Some(secret_id_owned),
            }))
            .await;

        true
    } else {
        warn!("First-message authentication failed: invalid token");
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: "Invalid authentication token".to_string(),
            }))
            .await;
        // Close connection on auth failure
        let _ = message_tx.send(MessageRoute::Close).await;
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
        let _ = message_tx
            .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                message: format!("Unknown custom message type: {}", message_type),
            }))
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
                        let _ = message_tx
                            .send(MessageRoute::Outgoing(OutgoingMessage::PluginResponse {
                                message_type: message_type.clone(),
                                payload: json,
                            }))
                            .await;
                    }
                    WSResponse::Binary(data) => {
                        let _ = message_tx.send(MessageRoute::Binary(data)).await;
                    }
                    WSResponse::Multiple(responses) => {
                        for resp in responses {
                            match resp {
                                WSResponse::Json(json) => {
                                    let _ = message_tx
                                        .send(MessageRoute::Outgoing(
                                            OutgoingMessage::PluginResponse {
                                                message_type: message_type.clone(),
                                                payload: json,
                                            },
                                        ))
                                        .await;
                                }
                                WSResponse::Binary(data) => {
                                    let _ = message_tx.send(MessageRoute::Binary(data)).await;
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
                let _ = message_tx
                    .send(MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: format!("Plugin handler error: {}", e),
                    }))
                    .await;
            }
        }
    }

    true
}

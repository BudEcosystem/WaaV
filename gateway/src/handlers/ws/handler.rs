//! Axum WebSocket handler
//!
//! This module contains the main WebSocket upgrade handler for Axum
//! and the core WebSocket connection handling logic.

use axum::{
    Extension,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio::{select, time::Duration};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::auth::Auth;
use crate::middleware::ClientIp;
use crate::state::AppState;

use super::{
    audio_handler::handle_audio_message,
    messages::{IncomingMessage, MessageClass, MessageRoute, OutgoingMessage, send_with_policy},
    processor::handle_incoming_message,
    state::ConnectionState,
};

/// Optimized channel buffer size for audio workloads
/// Larger buffer (1024 vs default 256) reduces contention in high-throughput scenarios
/// Trade-off: Uses more memory but provides better latency characteristics
const CHANNEL_BUFFER_SIZE: usize = 1024;

/// How long voice WebSocket teardown waits for the sender task to drain queued
/// critical messages and emit a close frame before aborting it.
const SENDER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

/// Maximum WebSocket frame size (10 MB)
/// This limits individual frame sizes to prevent memory exhaustion attacks
const MAX_WS_FRAME_SIZE: usize = 10 * 1024 * 1024;

/// Maximum WebSocket message size (10 MB)
/// This limits the total message size (can be multiple frames)
const MAX_WS_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Maximum text message size before deserialization (1 MB)
/// This prevents JSON parsing attacks with extremely large payloads
/// Binary audio messages use MAX_WS_MESSAGE_SIZE instead
const MAX_TEXT_MESSAGE_SIZE: usize = 1024 * 1024;

/// WebSocket voice processing handler
///
/// Upgrades the HTTP connection to WebSocket for real-time voice processing.
/// This is the main entry point for WebSocket connections to the voice service.
///
/// # Arguments
/// * `ws` - The WebSocket upgrade request from Axum
/// * `state` - Application state containing configuration and shared resources
/// * `auth` - Auth context from middleware for tenant isolation
/// * `client_ip` - Optional client IP from connection limit middleware for releasing connection
///
/// # Returns
/// * `Response` - HTTP response that upgrades the connection to WebSocket
pub async fn ws_voice_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<Auth>,
    client_ip: Option<Extension<ClientIp>>,
) -> Response {
    info!(
        auth_id = ?auth.id,
        client_ip = ?client_ip.as_ref().map(|c| c.0.0),
        "WebSocket voice connection upgrade requested"
    );
    debug!("AppState extracted successfully, preparing upgrade");

    // Extract the IP address if present (used for connection limit tracking)
    let ip = client_ip.map(|Extension(ClientIp(ip))| ip);

    // Apply message size limits to prevent memory exhaustion attacks
    let response = ws
        .max_frame_size(MAX_WS_FRAME_SIZE)
        .max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            debug!("WebSocket upgrade callback triggered");
            handle_voice_socket(socket, state, auth, ip)
        });

    debug!("WebSocket upgrade response created");
    response
}

/// Handle WebSocket voice connection with optimized performance
///
/// This function manages the entire WebSocket session for voice processing,
/// including message routing, resource management, and graceful cleanup.
///
/// # Arguments
/// * `socket` - The established WebSocket connection
/// * `app_state` - Application state containing shared resources
/// * `auth` - Auth context for tenant isolation (room name prefixing)
/// * `client_ip` - Optional client IP for connection limit tracking (releases on disconnect)
///
/// # Lifecycle
/// 1. Split socket into sender/receiver for bidirectional communication
/// 2. Set up message routing channels with optimized buffer sizes
/// 3. Spawn sender task for outgoing messages
/// 4. Process incoming messages in a loop
/// 5. Clean up resources on connection close (including connection limit release)
///
/// # Performance Optimizations
/// - Large channel buffer (1024) for reduced contention
/// - RwLock for connection state (frequent reads, rare writes)
/// - Timeout handling for stale connection detection
async fn handle_voice_socket(
    socket: WebSocket,
    app_state: Arc<AppState>,
    auth: Auth,
    client_ip: Option<IpAddr>,
) {
    // Multi-tenant panic isolation (W-E1 / E6).
    //
    // The release profile uses `panic = "unwind"` so that a panic inside one
    // session's task body does NOT abort the whole process (which would drop
    // every other tenant's connection). Here we contain a panic to THIS session
    // by running the session body inside `catch_unwind`.
    //
    // The connection-slot guard is created OUTSIDE the caught region so that the
    // per-IP connection counter is released via RAII even if the session panics.
    // `AssertUnwindSafe` is sound here: on panic we abandon this session entirely
    // (no shared state is observed after the unwind), so there is no risk of
    // exposing a logically-torn invariant to another session.
    let _connection_guard = client_ip.map(|ip| ConnectionGuard {
        app_state: app_state.clone(),
        ip,
    });

    let session =
        std::panic::AssertUnwindSafe(run_voice_socket_session(socket, app_state, auth, client_ip));
    if futures::FutureExt::catch_unwind(session).await.is_err() {
        // A panic was caught and contained to this session. The process and all
        // other sessions remain alive. The connection guard above still releases
        // the slot on scope exit.
        error!("WebSocket session panicked; connection terminated (process unaffected)");
    }
}

/// Inner body of a single WebSocket voice session.
///
/// Separated from [`handle_voice_socket`] so the latter can wrap it in
/// `catch_unwind` for per-session panic isolation. A panic here unwinds back to
/// the wrapper and kills only this connection.
async fn run_voice_socket_session(
    socket: WebSocket,
    app_state: Arc<AppState>,
    auth: Auth,
    client_ip: Option<IpAddr>,
) {
    debug!("handle_voice_socket started");
    info!(
        auth_id = ?auth.id,
        pending = %auth.pending,
        client_ip = ?client_ip,
        "WebSocket voice connection established"
    );

    debug!("Splitting socket into sender and receiver");
    // Split the socket into sender and receiver
    let (mut sender, mut receiver) = socket.split();
    debug!("Socket split completed");

    // Connection state with RwLock for rare writes, frequent reads
    // Initialize with auth context for room name normalization
    let state = Arc::new(RwLock::new(ConnectionState::with_auth(auth.clone())));

    let (message_tx, mut message_rx) = mpsc::channel::<MessageRoute>(CHANNEL_BUFFER_SIZE);

    // If authentication is pending, send AuthRequired notification immediately
    // This informs browser clients they need to send an auth message first
    if auth.is_pending() {
        info!("Auth pending - sending auth_required notification");
        let auth_required_msg =
            serde_json::to_string(&OutgoingMessage::AuthRequired).unwrap_or_default();
        if let Err(e) = sender.send(Message::Text(auth_required_msg.into())).await {
            error!("Failed to send auth_required message: {}", e);
            return;
        }
    }

    // Create shutdown channel for graceful sender task termination
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    // Spawn task to handle outgoing messages - simple and direct for low latency
    let mut sender_task = tokio::spawn(async move {
        loop {
            select! {
                route_opt = message_rx.recv() => {
                    let Some(route) = route_opt else {
                        // Channel closed, exit gracefully
                        break;
                    };
                    let should_close = matches!(route, MessageRoute::Close);

                    let result = match route {
                        MessageRoute::Outgoing(message) => {
                            // Direct serialization and send - no batching for low latency
                            match serde_json::to_string(&message) {
                                Ok(json_str) => sender.send(Message::Text(json_str.into())).await,
                                Err(e) => {
                                    error!("Failed to serialize outgoing message: {}", e);
                                    continue;
                                }
                            }
                        }
                        MessageRoute::Binary(data) => sender.send(Message::Binary(data)).await,
                        MessageRoute::Close => {
                            info!("Closing WebSocket connection");
                            sender.send(Message::Close(None)).await
                        }
                    };

                    if let Err(e) = result {
                        error!("Failed to send WebSocket message: {}", e);
                        break;
                    }

                    // If we sent a Close message, break the loop
                    if should_close {
                        break;
                    }
                }
                _ = &mut shutdown_rx => {
                    // Graceful shutdown requested - drain remaining messages
                    while let Ok(route) = message_rx.try_recv() {
                        let result = match route {
                            MessageRoute::Outgoing(message) => {
                                match serde_json::to_string(&message) {
                                    Ok(json_str) => sender.send(Message::Text(json_str.into())).await,
                                    Err(_) => continue,
                                }
                            }
                            MessageRoute::Binary(data) => sender.send(Message::Binary(data)).await,
                            MessageRoute::Close => sender.send(Message::Close(None)).await,
                        };
                        if result.is_err() {
                            break;
                        }
                    }
                    // Send close frame for clean WebSocket termination
                    let _ = sender.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    });

    // Timeout for checking idle connections (configurable via WS_PROCESSING_TIMEOUT_SECS)
    // This determines how often we check if the connection is stale
    let processing_timeout = Duration::from_secs(app_state.config.ws_processing_timeout_secs);

    // Maximum idle time before closing the connection (5 minutes with ±10% jitter)
    // Audio connections without activity for this long are likely stale
    // Jitter prevents thundering herd when many connections timeout simultaneously
    let base_idle_secs: u64 = 300;
    let jitter_range: u64 = 30; // ±10% = 30 seconds
    let jitter_offset = (std::time::Instant::now().elapsed().as_nanos() as u64 % (jitter_range * 2))
        as i64
        - jitter_range as i64;
    let idle_secs = (base_idle_secs as i64 + jitter_offset).max(1) as u64;
    let idle_timeout = Duration::from_secs(idle_secs);

    // Drive the receive loop until the client disconnects, the connection goes
    // idle, or app-wide shutdown is signalled (RC6 SIGTERM drain). Extracted
    // into `run_session_loop` so the lifecycle is unit-testable.
    //
    // The processor closure clones its Arc/Sender handles per call (cheap:
    // refcount bumps) so the returned future owns its captures — a lending
    // (borrowing) closure here trips rustc's "`Send` is not general enough"
    // HRTB limitation once this future flows into `on_upgrade`.
    let exit = run_session_loop(
        &mut receiver,
        &message_tx,
        &app_state.shutdown,
        processing_timeout,
        idle_timeout,
        {
            let state = state.clone();
            let message_tx = message_tx.clone();
            let app_state = app_state.clone();
            move |msg| {
                let state = state.clone();
                let message_tx = message_tx.clone();
                let app_state = app_state.clone();
                async move { process_message(msg, &state, &message_tx, &app_state).await }
            }
        },
    )
    .await;
    debug!(exit = ?exit, "WebSocket session receive loop exited");

    // Clean up resources - graceful shutdown with timeout fallback
    // Signal shutdown to sender task
    shutdown_voice_sender_task(shutdown_tx, &mut sender_task).await;

    // Snapshot state before cleanup so we can drop the read lock before awaiting
    let (voice_manager, livekit_client, recording_egress_id, room_name) = {
        let state_guard = state.read().await;
        (
            state_guard.voice_manager.clone(),
            state_guard.livekit_client.clone(),
            state_guard.recording_egress_id.clone(),
            state_guard.livekit_room_name.clone(),
        )
    };

    // Disconnect LiveKit first to stop inbound audio before tearing down STT/TTS
    if let Some(livekit_client) = livekit_client {
        // Try to get write lock with timeout for cleanup
        match tokio::time::timeout(Duration::from_millis(100), livekit_client.write()).await {
            Ok(mut client) => {
                if let Err(e) = client.disconnect().await {
                    error!("Failed to disconnect LiveKit client: {:?}", e);
                }
            }
            Err(_) => {
                warn!("Timeout acquiring LiveKit lock for cleanup - client may be busy");
            }
        }
    }

    // Now stop the voice manager after audio sources are quiet.
    // BOUNDED (D-G4, Pipecat CANCEL_TIMEOUT parity): a provider disconnect
    // wedged in native/network code must not hang session teardown — warn
    // loudly and move on; the session's spawned tasks are detached and the
    // process-level drain (RC6) remains the backstop.
    if let Some(voice_manager) = voice_manager {
        const SESSION_TEARDOWN_BUDGET: Duration = Duration::from_secs(10);
        match tokio::time::timeout(SESSION_TEARDOWN_BUDGET, voice_manager.stop()).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => error!("Failed to stop voice manager: {}", e),
            Err(_) => {
                warn!(
                    budget_secs = SESSION_TEARDOWN_BUDGET.as_secs(),
                    "voice manager stop exceeded the teardown budget — being                      blocked somewhere? continuing teardown"
                );
                crate::core::metrics::bridge::record_session_teardown_timeout();
            }
        }
    }

    // B-G2: disconnect every PERSISTENT realtime (S2S) session BEFORE the D-G4
    // audit. Each session's upstream WebSocket is owned by a `RealtimeSession`
    // supervisor spawned OFF the task_tracker, so the audit below cannot reach it
    // — this is its explicit, bounded teardown owner (aborts the supervisor +
    // closes the socket gracefully). The map lives in the session's DAGContext;
    // extract the `Arc` under a short read lock, then drop the lock before
    // awaiting the (bounded) disconnects. No-op when there is no DAG / no
    // persistent realtime node. Gated on `dag-routing`: the `dag` module, the
    // `dag_context` field, and the `dag::nodes` symbols below only exist under
    // that feature (mirrors the gated `initialize_dag_routing` that inserts the
    // map). The block is self-contained, so no `cfg(not(...))` no-op is needed.
    #[cfg(feature = "dag-routing")]
    {
        let sessions = {
            let guard = state.read().await;
            guard.dag_context.as_ref().and_then(|c| {
                c.get_resource_as::<crate::dag::nodes::RealtimeSessionMap>(
                    &crate::dag::nodes::realtime_sessions_key(),
                )
            })
        };
        if let Some(sessions) = sessions {
            let closed = crate::dag::nodes::disconnect_realtime_sessions(
                &sessions,
                crate::core::observability::DEFAULT_TEARDOWN_GRACE,
            )
            .await;
            if closed > 0 {
                info!(
                    count = closed,
                    "B-G2: disconnected persistent realtime session(s) at teardown"
                );
            }
        }
    }

    // D-G4 (Pipecat `_print_dangling_tasks` parity): cancel the session's
    // tracked tasks (LiveKit audio forwarder, DAG output drain) and warn about
    // any that RESIST cancellation. These loops block on a recv whose sender
    // ConnectionState still holds, so they only stop via this abort; a clean
    // cancellation finishes at its await point within the grace and is not
    // counted, while a wedged task surfaces as `waav_session_dangling_tasks_total`.
    // Clone the Arc so we never hold the connection read lock across the grace.
    {
        let tracker = state.read().await.task_tracker.clone();
        let audit = tracker
            .abort_and_audit_details(crate::core::observability::DEFAULT_TEARDOWN_GRACE)
            .await;
        if audit.dangling > 0 {
            warn!(
                count = audit.dangling,
                "session task(s) resisted cancellation at teardown (D-G4)"
            );
        }
        if audit.panicked > 0 {
            error!(
                count = audit.panicked,
                "session task panic(s) observed at teardown (W-E1/D-G4)"
            );
        }
    }

    // Stop recording if it was started
    if let (Some(egress_id), Some(room_handler)) =
        (&recording_egress_id, &app_state.livekit_room_handler)
    {
        info!("Stopping recording with egress ID: {}", egress_id);
        if let Err(e) = room_handler.stop_room_recording(egress_id).await {
            error!("Failed to stop room recording: {:?}", e);
        } else {
            info!("Recording stopped successfully");
        }
    }

    // Delete room if it exists
    if let (Some(room), Some(room_handler)) = (&room_name, &app_state.livekit_room_handler) {
        info!("Deleting LiveKit room: {}", room);
        if let Err(e) = room_handler.delete_room(room).await {
            error!("Failed to delete room: {:?}", e);
        } else {
            info!("Room deleted successfully");
        }
    }

    info!("WebSocket voice connection terminated");
}

/// Why [`run_session_loop`] returned. Drives only logging today, but gives the
/// drain path (RC6) an explicit, testable outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionLoopExit {
    /// Client closed the socket, the socket errored, or a handler requested close.
    Closed,
    /// The connection exceeded the idle timeout.
    IdleTimeout,
    /// App-wide shutdown was signalled (RC6 SIGTERM session drain).
    Shutdown,
}

/// Drive a session's receive loop until the client disconnects, the connection
/// goes idle, or server shutdown is signalled.
///
/// Extracted from [`run_voice_socket_session`] — and made generic over the
/// inbound stream and the message processor — so the session lifecycle (in
/// particular the RC6 SIGTERM-drain branch) is unit-testable without a real
/// WebSocket. Production passes the split socket stream and a closure invoking
/// [`process_message`].
///
/// On shutdown the loop sends a final `{"type":"error"}` protocol notice
/// (Critical class — must not be shed) and returns cleanly, so the caller's
/// normal teardown (sender drain + close frame, LiveKit disconnect, voice
/// manager stop) runs within the drain window.
async fn run_session_loop<R, F, Fut>(
    receiver: &mut R,
    message_tx: &mpsc::Sender<MessageRoute>,
    shutdown: &CancellationToken,
    processing_timeout: Duration,
    idle_timeout: Duration,
    mut on_message: F,
) -> SessionLoopExit
where
    R: futures::Stream<Item = Result<Message, axum::Error>> + Unpin,
    F: FnMut(Message) -> Fut,
    Fut: Future<Output = bool>,
{
    // Track last activity time for idle connection detection
    let mut last_activity = std::time::Instant::now();

    loop {
        select! {
            msg_result = receiver.next() => {
                // Update activity time on any message
                last_activity = std::time::Instant::now();

                match msg_result {
                    Some(Ok(msg)) => {
                        if !on_message(msg).await {
                            return SessionLoopExit::Closed;
                        }
                    }
                    Some(Err(e)) => {
                        warn!("WebSocket error: {}", e);
                        send_with_policy(
                            message_tx,
                            MessageRoute::Outgoing(OutgoingMessage::Error {
                                message: format!("WebSocket error: {e}"),
                            }),
                            MessageClass::Critical,
                        )
                        .await;
                        return SessionLoopExit::Closed;
                    }
                    None => {
                        info!("WebSocket connection closed by client");
                        return SessionLoopExit::Closed;
                    }
                }
            }
            _ = tokio::time::sleep(processing_timeout) => {
                // Check if connection has been idle too long
                if last_activity.elapsed() > idle_timeout {
                    warn!(
                        "WebSocket connection idle for {}s, closing stale connection",
                        last_activity.elapsed().as_secs()
                    );
                    send_with_policy(
                        message_tx,
                        MessageRoute::Outgoing(OutgoingMessage::Error {
                            message: "Connection closed due to inactivity".to_string(),
                        }),
                        MessageClass::Critical,
                    )
                    .await;
                    return SessionLoopExit::IdleTimeout;
                }
                debug!(
                    "WebSocket connection alive, idle for {}s",
                    last_activity.elapsed().as_secs()
                );
            }
            _ = shutdown.cancelled() => {
                // RC6 SIGTERM drain: main() cancelled the app-wide token before
                // axum's graceful drain started. Tell the client this close is
                // server-initiated, then exit cleanly so provider teardown runs.
                info!("Server shutdown signalled; draining WebSocket session");
                send_with_policy(
                    message_tx,
                    MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: "server shutting down".to_string(),
                    }),
                    MessageClass::Critical,
                )
                .await;
                return SessionLoopExit::Shutdown;
            }
        }
    }
}

async fn shutdown_voice_sender_task(
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    sender_task: &mut tokio::task::JoinHandle<()>,
) -> bool {
    let _ = shutdown_tx.send(());
    // Wait for graceful completion with timeout, then abort if needed.
    // D-G4: on timeout, ABORT the handle rather than letting `timeout` drop it
    // (a dropped JoinHandle detaches — the sender would leak past the session);
    // record only true timeout leaks as dangling tasks. A completed JoinHandle
    // can still be a panic, so inspect the join result instead of treating
    // every completed handle as graceful.
    match tokio::time::timeout(SENDER_SHUTDOWN_TIMEOUT, &mut *sender_task).await {
        Ok(Ok(())) => {
            debug!("Sender task completed gracefully");
            true
        }
        Ok(Err(e)) => {
            if e.is_panic() {
                error!("Sender task panicked during shutdown: {}", e);
                crate::core::metrics::bridge::record_session_task_panic();
            } else {
                debug!("Sender task cancelled during shutdown: {}", e);
            }
            true
        }
        Err(_) => {
            warn!("Sender task did not complete within timeout; aborting (D-G4)");
            sender_task.abort();
            crate::core::metrics::bridge::record_session_dangling_task();
            let _ = sender_task.await;
            false
        }
    }
}

/// Process incoming WebSocket message with optimizations
///
/// Routes different message types to appropriate handlers and manages
/// the connection lifecycle based on message processing results.
///
/// # Arguments
/// * `msg` - The WebSocket message to process
/// * `state` - Connection state for this WebSocket session
/// * `message_tx` - Channel for sending response messages
/// * `app_state` - Application state with global configuration
///
/// # Returns
/// * `bool` - true to continue processing, false to close connection
///
/// # Performance Notes
/// - Marked inline(always) for hot path optimization
/// - Fast JSON parsing with pre-validation
/// - Zero-copy audio data handling where possible
#[inline(always)]
async fn process_message(
    msg: Message,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
    app_state: &Arc<AppState>,
) -> bool {
    match msg {
        Message::Text(text) => {
            debug!("Received text message: {} bytes", text.len());

            // Panic-isolation test seam (W-E1). This is debug-only so release
            // deployments cannot be env-triggered into a session panic.
            #[cfg(debug_assertions)]
            if let Ok(token) = std::env::var("WAAV_TEST_PANIC_ON_TEXT")
                && !token.is_empty()
                && text.starts_with(token.as_str())
            {
                panic!("WAAV_TEST_PANIC_ON_TEXT injected panic (debug-only test seam)");
            }

            // Pre-deserialization size check to prevent JSON parsing attacks
            if text.len() > MAX_TEXT_MESSAGE_SIZE {
                warn!(
                    size = text.len(),
                    max = MAX_TEXT_MESSAGE_SIZE,
                    "Text message exceeds maximum size before deserialization"
                );
                send_with_policy(
                    message_tx,
                    MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: format!(
                            "Message too large: {} bytes (max {} bytes)",
                            text.len(),
                            MAX_TEXT_MESSAGE_SIZE
                        ),
                    }),
                    MessageClass::Critical,
                )
                .await;
                return true;
            }

            // Fast path JSON parsing with pre-validation
            let incoming_msg: IncomingMessage = match serde_json::from_str(&text) {
                Ok(msg) => msg,
                Err(e) => {
                    error!("Failed to parse incoming message: {}", e);
                    send_with_policy(
                        message_tx,
                        MessageRoute::Outgoing(OutgoingMessage::Error {
                            message: format!("Invalid message format: {e}"),
                        }),
                        MessageClass::Critical,
                    )
                    .await;
                    return true;
                }
            };

            // Validate message field sizes to prevent resource exhaustion
            if let Err(e) = incoming_msg.validate_size() {
                warn!("Message validation failed: {}", e);
                send_with_policy(
                    message_tx,
                    MessageRoute::Outgoing(OutgoingMessage::Error {
                        message: e.to_string(),
                    }),
                    MessageClass::Critical,
                )
                .await;
                return true;
            }

            // P0 gw-enforce: surface unknown / wrong-nested `config` keys that
            // serde silently dropped as a non-fatal `config_warning` advisory,
            // so the SDK-typo bug class (e.g. a misnested `turn_detection`) fails
            // LOUDLY instead of vanishing. Cheap and config-only: we re-parse the
            // (already size-capped) text to a `Value` ONLY for a config message;
            // every other message kind skips this entirely.
            if matches!(incoming_msg, IncomingMessage::Config { .. }) {
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(raw_value) => {
                        super::config_lint::warn_unknown_config_keys(
                            &raw_value,
                            &incoming_msg,
                            message_tx,
                        )
                        .await;
                    }
                    // The typed parse already succeeded above, so a Value parse
                    // failing here is not expected; skip the lint rather than
                    // disturb a session that is otherwise valid.
                    Err(e) => debug!("config lint: raw Value re-parse failed: {e}"),
                }
            }

            handle_incoming_message(incoming_msg, state, message_tx, app_state).await
        }
        Message::Binary(data) => {
            debug!("Received binary message: {} bytes", data.len());

            // Handle binary audio data with zero-copy optimization
            handle_audio_message(data, state, message_tx).await
        }
        Message::Ping(_data) => {
            debug!("Received ping message");
            // Ping/Pong is handled automatically by axum
            true
        }
        Message::Pong(_) => {
            debug!("Received pong message");
            true
        }
        Message::Close(_) => {
            info!("WebSocket connection closed by client");
            false
        }
    }
}

/// Guard struct that releases a connection slot when dropped
///
/// This implements RAII pattern to ensure connection slots are always released,
/// even if the WebSocket handler panics or encounters errors.
struct ConnectionGuard {
    app_state: Arc<AppState>,
    ip: IpAddr,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        debug!(ip = %self.ip, "Releasing connection slot");
        self.app_state.release_connection(self.ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    /// Generous defaults that keep the idle branch out of the way of the
    /// scenario under test.
    const TEST_PROCESSING_TIMEOUT: Duration = Duration::from_secs(10);
    const TEST_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

    /// RC6: a cancelled shutdown token makes the session loop exit with
    /// `Shutdown` and emit a final protocol notice (even with a socket that
    /// never yields a message — the drain must not wait on client traffic).
    #[tokio::test]
    async fn shutdown_cancellation_exits_session_loop_with_goodbye() {
        let mut receiver = stream::pending::<Result<Message, axum::Error>>();
        let (tx, mut rx) = mpsc::channel::<MessageRoute>(8);
        let shutdown = CancellationToken::new();
        shutdown.cancel();

        let exit = tokio::time::timeout(
            Duration::from_secs(5),
            run_session_loop(
                &mut receiver,
                &tx,
                &shutdown,
                TEST_PROCESSING_TIMEOUT,
                TEST_IDLE_TIMEOUT,
                |_msg| async move { true },
            ),
        )
        .await
        .expect("session loop must exit promptly once the shutdown token is cancelled");

        assert_eq!(exit, SessionLoopExit::Shutdown);

        match rx
            .try_recv()
            .expect("a final protocol message must be queued")
        {
            MessageRoute::Outgoing(OutgoingMessage::Error { message }) => {
                assert_eq!(message, "server shutting down");
            }
            _ => panic!("expected the shutdown notice as an error protocol message"),
        }
    }

    /// RC6: cancellation arriving mid-session (the realistic SIGTERM case —
    /// the loop is already parked in select!) still drains the session.
    #[tokio::test]
    async fn shutdown_mid_session_exits_running_loop() {
        let (tx, mut rx) = mpsc::channel::<MessageRoute>(8);
        let shutdown = CancellationToken::new();
        let task_token = shutdown.clone();

        let session = tokio::spawn(async move {
            let mut receiver = stream::pending::<Result<Message, axum::Error>>();
            run_session_loop(
                &mut receiver,
                &tx,
                &task_token,
                TEST_PROCESSING_TIMEOUT,
                TEST_IDLE_TIMEOUT,
                |_msg| async move { true },
            )
            .await
        });

        // Let the loop start and park in select! before signalling shutdown.
        tokio::task::yield_now().await;
        shutdown.cancel();

        let exit = tokio::time::timeout(Duration::from_secs(5), session)
            .await
            .expect("session loop must observe mid-session cancellation")
            .expect("session loop task must not panic");
        assert_eq!(exit, SessionLoopExit::Shutdown);

        assert!(
            matches!(
                rx.recv().await,
                Some(MessageRoute::Outgoing(OutgoingMessage::Error { .. }))
            ),
            "shutdown notice must be sent before the loop exits"
        );
    }

    /// Sanity: without shutdown, a client-side close (stream end) still exits
    /// the loop via the normal path — the new select branch must not capture
    /// ordinary disconnects.
    #[tokio::test]
    async fn client_disconnect_exits_loop_as_closed() {
        let mut receiver = stream::iter(Vec::<Result<Message, axum::Error>>::new());
        let (tx, _rx) = mpsc::channel::<MessageRoute>(8);
        let shutdown = CancellationToken::new();

        let exit = tokio::time::timeout(
            Duration::from_secs(5),
            run_session_loop(
                &mut receiver,
                &tx,
                &shutdown,
                TEST_PROCESSING_TIMEOUT,
                TEST_IDLE_TIMEOUT,
                |_msg| async move { true },
            ),
        )
        .await
        .expect("loop must exit when the inbound stream ends");

        assert_eq!(exit, SessionLoopExit::Closed);
    }

    /// Sanity: a handler that requests close (process_message returning false,
    /// e.g. on a Close frame) exits the loop as Closed.
    #[tokio::test]
    async fn handler_requested_close_exits_loop_as_closed() {
        let mut receiver = stream::iter(vec![Ok::<Message, axum::Error>(Message::Close(None))]);
        let (tx, _rx) = mpsc::channel::<MessageRoute>(8);
        let shutdown = CancellationToken::new();

        let exit = tokio::time::timeout(
            Duration::from_secs(5),
            run_session_loop(
                &mut receiver,
                &tx,
                &shutdown,
                TEST_PROCESSING_TIMEOUT,
                TEST_IDLE_TIMEOUT,
                |msg| async move { !matches!(msg, Message::Close(_)) },
            ),
        )
        .await
        .expect("loop must exit when the handler requests close");

        assert_eq!(exit, SessionLoopExit::Closed);
    }

    #[tokio::test]
    async fn voice_sender_shutdown_observes_panicked_task() {
        let (shutdown_tx, _shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let mut sender_task = tokio::spawn(async move {
            panic!("voice sender panic regression");
        });

        assert!(
            shutdown_voice_sender_task(shutdown_tx, &mut sender_task).await,
            "a panicked-but-finished sender task must be observed, not classified as dangling"
        );
        assert!(
            sender_task.is_finished(),
            "panicked sender handle must be joined by shutdown"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn voice_sender_shutdown_aborts_stuck_task() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let observed_shutdown = Arc::new(AtomicBool::new(false));
        let observed = observed_shutdown.clone();
        let mut sender_task = tokio::spawn(async move {
            let _ = shutdown_rx.await;
            observed.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        });

        assert!(
            !shutdown_voice_sender_task(shutdown_tx, &mut sender_task).await,
            "stuck sender task must time out and be aborted"
        );
        assert!(
            observed_shutdown.load(Ordering::SeqCst),
            "sender task should still receive shutdown before timing out"
        );
        assert!(
            sender_task.is_finished(),
            "aborted sender handle must be joined by shutdown"
        );
    }
}

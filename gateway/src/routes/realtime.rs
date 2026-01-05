//! Realtime WebSocket route configuration
//!
//! This module configures the WebSocket endpoint for real-time audio-to-audio
//! streaming using providers like OpenAI's Realtime API.

use axum::{Router, routing::get};
use tower_http::trace::TraceLayer;

use crate::handlers::realtime::realtime_handler;
use crate::state::AppState;
use std::sync::Arc;

/// Create the Realtime WebSocket router
///
/// # Endpoint
///
/// `GET /realtime` - WebSocket upgrade for real-time audio processing
///
/// # Protocol
///
/// After WebSocket upgrade, clients send:
/// 1. `config` message to configure provider, model, voice
/// 2. Binary audio frames (PCM 16-bit, 24kHz, mono)
///
/// Server responds with:
/// - `session_created` when session is established
/// - `transcript` for speech transcription
/// - Binary audio frames for TTS output
/// - `error` on failures
///
/// # Authentication
///
/// Uses the same auth middleware as REST endpoints for tenant isolation.
///
/// # Example
///
/// ```json
/// // Client sends config
/// {"type": "config", "provider": "openai", "model": "gpt-4o-realtime-preview", "voice": "alloy"}
///
/// // Server responds
/// {"type": "session_created", "session_id": "...", "provider": "openai", "model": "gpt-4o-realtime-preview"}
///
/// // Client sends audio as binary frames
/// // Server sends back transcripts and audio
/// ```
pub fn create_realtime_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/realtime", get(realtime_handler))
        .layer(TraceLayer::new_for_http())
}

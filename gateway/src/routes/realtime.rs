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
/// Every path the NATIVE realtime handler is served at.
///
/// A CONSTANT the router iterates, rather than a list of `.route()` calls, so the test below
/// asserts on the same data the router uses.
///
/// Only `/realtime`: FRD-023 D-1 made `/v1/realtime` the OpenAI Realtime GA endpoint for Bud
/// deployments (`routes::openai_realtime`). WaaV's own SDKs connect to `/realtime`, and the
/// native protocol could never have served a Bud deployment on `/v1/realtime` (it had no way to
/// name one, nor a credential to use).
pub const REALTIME_PATHS: &[&str] = &["/realtime"];

pub fn create_realtime_router() -> Router<Arc<AppState>> {
    let mut router = Router::new();
    for path in REALTIME_PATHS {
        router = router.route(path, get(realtime_handler));
    }
    router.layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod route_tests {
    use super::REALTIME_PATHS;

    #[test]
    fn the_openai_compatible_path_is_not_the_native_handler() {
        // FRD-023 D-1: `/v1/realtime` speaks OpenAI GA (routes::openai_realtime). Serving it with
        // the native handler too would register the path twice — axum panics at construction.
        assert!(!REALTIME_PATHS.contains(&"/v1/realtime"));
        assert_eq!(
            crate::handlers::openai_realtime::OPENAI_REALTIME_PATH,
            "/v1/realtime"
        );
    }

    #[test]
    fn the_native_path_is_kept() {
        assert!(
            REALTIME_PATHS.contains(&"/realtime"),
            "standalone deployments address this path directly"
        );
    }

    #[test]
    fn no_path_is_registered_twice() {
        // A duplicate would panic axum at router construction, taking the process down at
        // startup rather than failing a request.
        let mut seen = REALTIME_PATHS.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(
            before,
            seen.len(),
            "duplicate path: axum panics on construction"
        );
    }
}

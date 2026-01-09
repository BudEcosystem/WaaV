use axum::{
    Router,
    routing::{delete, get, post},
};
use tower_http::trace::TraceLayer;

use crate::handlers::{livekit, plugins, recording, sip, speak, voices};
use crate::state::AppState;
use std::sync::Arc;

/// Create the API router with protected routes
///
/// Note: Authentication middleware should be applied in main.rs after state is available
pub fn create_api_router() -> Router<Arc<AppState>> {
    Router::new()
        // Protected routes (auth required when AUTH_REQUIRED=true)
        .route("/voices", get(voices::list_voices))
        .route("/voices/clone", post(voices::clone_voice))
        .route("/speak", post(speak::speak_handler))
        .route("/livekit/token", post(livekit::generate_token))
        .route("/livekit/rooms", get(livekit::list_rooms))
        .route("/livekit/rooms/{room_name}", get(livekit::get_room_details))
        .route("/livekit/participant", delete(livekit::remove_participant))
        .route("/livekit/participant/mute", post(livekit::mute_participant))
        .route("/recording/{stream_id}", get(recording::download_recording))
        // SIP hooks management
        .route(
            "/sip/hooks",
            get(sip::list_sip_hooks)
                .post(sip::update_sip_hooks)
                .delete(sip::delete_sip_hooks),
        )
        // SIP call transfer
        .route("/sip/transfer", post(sip::sip_transfer))
        // Plugin discovery endpoints
        .route("/plugins", get(plugins::list_plugins))
        .route("/plugins/stt", get(plugins::list_stt_providers))
        .route("/plugins/tts", get(plugins::list_tts_providers))
        .route("/plugins/realtime", get(plugins::list_realtime_providers))
        .route("/plugins/processors", get(plugins::list_processors))
        .route("/plugins/{provider_id}", get(plugins::get_provider_info))
        .route("/plugins/{provider_id}/health", get(plugins::get_provider_health))
        .layer(TraceLayer::new_for_http())
}

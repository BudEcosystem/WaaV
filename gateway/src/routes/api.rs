use axum::{
    Router,
    routing::{delete, get, post},
};
use tower_http::trace::TraceLayer;

use crate::handlers::{dag, livekit, recording, sip, speak, voices};
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
        // OpenAI-compatible audio surface (FRD-018). These are what the Bud ingress routes
        // /v1/audio/* to, so a WaaV voice endpoint is reachable as an ordinary Bud model.
        .route(
            "/v1/audio/speech",
            post(crate::handlers::openai_audio::speech_handler),
        )
        .route(
            "/v1/audio/transcriptions",
            post(crate::handlers::openai_audio::transcription_handler),
        )
        .route(
            // translation_handler, NOT transcription_handler: the two differ only by the
            // `translate` flag, and wiring both routes to the same one would make
            // /v1/audio/translations silently return source-language text that looks correct.
            "/v1/audio/translations",
            post(crate::handlers::openai_audio::translation_handler),
        )
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
        // DAG routing endpoints
        .route("/dag/templates", get(dag::list_templates))
        .route("/dag/templates/{template_name}", get(dag::get_template))
        .route("/dag/validate", post(dag::validate_dag))
        .layer(TraceLayer::new_for_http())
}

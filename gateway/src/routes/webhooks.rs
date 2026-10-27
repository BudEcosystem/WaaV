use axum::{Router, routing::post};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

use crate::handlers::{livekit, tasks};
use crate::state::AppState;

/// Create the webhook router for unauthenticated webhook endpoints
///
/// These routes are called by external services (like LiveKit) and use
/// their own authentication mechanisms (e.g., signed payloads).
/// This router should be merged without the auth middleware.
pub fn create_webhook_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/livekit/webhook", post(livekit::handle_livekit_webhook))
        // Interaction mode: a webhook backend reports on its task, with its own signature.
        .route("/v1/tasks/{task_id}/callback", post(tasks::task_callback))
        .layer(TraceLayer::new_for_http())
}

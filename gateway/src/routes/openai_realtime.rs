//! `/v1/realtime` and `/v1/realtime/client_secrets` (FRD-023).
//!
//! Mounted WITHOUT `auth_middleware`: these handlers authenticate themselves, because they need
//! credential sources the middleware does not read (the `openai-insecure-api-key.` subprotocol and
//! the `api-key` header), refuse one it does accept (`?token=`), and validate `ek_bud_` client
//! secrets. `main.rs` still layers `connection_limit_middleware` (FRD-022) outside them, so every
//! session holds a connection slot.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

use crate::handlers::openai_realtime::{
    CLIENT_SECRETS_PATH, OPENAI_REALTIME_PATH, client_secrets_handler, realtime_ws_handler,
};
use crate::state::AppState;

pub fn create_openai_realtime_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(OPENAI_REALTIME_PATH, get(realtime_ws_handler))
        .route(CLIENT_SECRETS_PATH, post(client_secrets_handler))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_paths_are_the_openai_ones() {
        assert_eq!(super::OPENAI_REALTIME_PATH, "/v1/realtime");
        assert_eq!(super::CLIENT_SECRETS_PATH, "/v1/realtime/client_secrets");
    }
}

pub mod auth;
pub mod config;
pub mod core;
#[cfg(feature = "dag-routing")]
pub mod dag;
pub mod docs;
pub mod errors;
pub mod handlers;
pub mod init;
pub mod livekit;
pub mod middleware;
pub mod plugin;
pub mod routes;
pub mod state;
pub mod utils;

// Re-export commonly used items for convenience
pub use config::ServerConfig;
pub use core::*;
pub use errors::app_error::{AppError, AppResult};
pub use errors::auth_error::{AuthError, AuthResult};
pub use plugin::global_registry;
pub use state::AppState;

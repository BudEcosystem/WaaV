//! Shared WebSocket utilities for STT/TTS/Realtime providers.
//!
//! This module provides:
//! - [`ReconnectionConfig`] - Configuration for automatic reconnection with exponential backoff
//! - [`ReconnectionState`] - State machine for tracking reconnection process
//! - [`ReconnectionManager`] - Thread-safe manager for coordinating reconnection attempts
//!
//! # Usage
//!
//! These utilities are designed to be used by WebSocket-based providers (STT, TTS, Realtime)
//! to implement consistent reconnection behavior with exponential backoff and jitter.
//!
//! ```rust,ignore
//! use waav_gateway::core::websocket::{ReconnectionConfig, ReconnectionManager};
//!
//! // Create a manager with default config
//! let config = ReconnectionConfig::default();
//! let manager = ReconnectionManager::new(config);
//!
//! // On connection loss
//! manager.on_connection_lost();
//!
//! // Reconnection loop
//! while manager.should_retry() {
//!     let delay = manager.next_delay_duration();
//!     tokio::time::sleep(delay).await;
//!
//!     manager.start_attempt();
//!     match try_connect().await {
//!         Ok(_) => manager.record_attempt(true),
//!         Err(_) => manager.record_attempt(false),
//!     }
//! }
//! ```

pub mod reconnection;

pub use reconnection::{
    ReconnectionConfig, ReconnectionConfigBuilder, ReconnectionManager, ReconnectionSnapshot,
    ReconnectionState,
};

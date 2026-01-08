//! Plugin System for WaaV Gateway
//!
//! This module provides a comprehensive plugin architecture that enables:
//! - Dynamic provider registration (STT, TTS, Realtime)
//! - Audio processor plugins (VAD, noise filter, resample)
//! - Middleware plugins (auth, rate limiting)
//! - WebSocket message handler plugins
//! - Full backward compatibility with existing APIs
//!
//! # Architecture
//!
//! The plugin system uses a capability-based design where plugins declare their
//! capabilities through traits. The registry indexes these capabilities and
//! factory functions delegate to the registry for provider creation.
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Plugin Registration                          │
//! │  inventory crate ──▶ PHF Static Map ──▶ DashMap Runtime Registry │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ## Registering a Built-in Provider
//!
//! ```ignore
//! use waav_gateway::plugin::prelude::*;
//!
//! pub struct MySTTPlugin;
//!
//! impl PluginCapability for MySTTPlugin {}
//!
//! impl STTCapability for MySTTPlugin {
//!     fn provider_id(&self) -> &'static str { "my-stt" }
//!
//!     fn create_stt(&self, config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
//!         Ok(Box::new(MySTT::new(config)?))
//!     }
//!
//!     fn metadata(&self) -> ProviderMetadata {
//!         ProviderMetadata::new("my-stt", "My STT Provider")
//!     }
//! }
//!
//! inventory::submit!(PluginConstructor::new::<MySTTPlugin>());
//! ```
//!
//! ## Using the Registry
//!
//! ```ignore
//! use waav_gateway::plugin::global_registry;
//!
//! let provider = global_registry().create_stt("deepgram", config)?;
//! ```

pub mod builtin;
pub mod capabilities;
pub mod dispatch;
pub mod isolation;
pub mod lifecycle;
#[macro_use]
pub mod macros;
pub mod metadata;
pub mod registry;

// Re-exports for convenience
pub use capabilities::{
    AudioProcessorCapability, AuthCapability, MiddlewareCapability, PluginCapability,
    RealtimeCapability, STTCapability, TTSCapability, WSHandlerCapability,
};
pub use isolation::{PluginError, call_plugin_safely};
pub use lifecycle::{PluginHealth, PluginLifecycle, PluginState};
pub use metadata::{PluginManifest, ProviderMetadata};
pub use registry::{PluginRegistry, global_registry};

/// Prelude module for convenient imports
///
/// Use this for plugin development:
/// ```ignore
/// use waav_gateway::plugin::prelude::*;
/// ```
pub mod prelude {
    pub use super::capabilities::{
        AudioProcessorCapability, AuthCapability, MiddlewareCapability, PluginCapability,
        RealtimeCapability, STTCapability, TTSCapability, WSHandlerCapability,
    };
    pub use super::isolation::{PluginError, call_plugin_safely};
    pub use super::lifecycle::{PluginHealth, PluginLifecycle, PluginState};
    pub use super::metadata::{PluginManifest, ProviderMetadata};
    pub use super::registry::{
        PluginConstructor, PluginRegistry, RealtimeFactoryFn, STTFactoryFn, TTSFactoryFn,
        global_registry,
    };

    // Re-export commonly needed external crates
    pub use async_trait::async_trait;
    pub use inventory;
    pub use serde_json::Value;
    pub use std::sync::Arc;

    // Re-export core traits for plugin implementations
    pub use crate::core::realtime::{BaseRealtime, RealtimeConfig, RealtimeError, RealtimeResult};
    pub use crate::core::stt::{BaseSTT, STTConfig, STTError};
    pub use crate::core::tts::{BaseTTS, TTSConfig, TTSError, TTSResult};
}

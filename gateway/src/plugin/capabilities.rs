//! Plugin Capability Traits
//!
//! This module defines the capability traits that plugins can implement
//! to provide different types of functionality. Each capability trait
//! represents a specific type of provider or handler.
//!
//! # Capability-Based Design
//!
//! Plugins declare their capabilities by implementing one or more of these traits.
//! The registry indexes plugins by their capabilities, allowing efficient lookup
//! when creating providers or handling requests.

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use super::metadata::ProviderMetadata;
use crate::core::realtime::{BaseRealtime, RealtimeConfig, RealtimeResult};
use crate::core::stt::{BaseSTT, STTConfig, STTError};
use crate::core::tts::{BaseTTS, TTSConfig, TTSResult};

/// Marker trait for all plugin capabilities
///
/// All capability traits must implement this marker trait to be recognized
/// by the plugin system. This enables type-safe capability indexing.
pub trait PluginCapability: Send + Sync + 'static {}

/// STT (Speech-to-Text) provider capability
///
/// Implement this trait to register an STT provider with the plugin system.
///
/// # Example
///
/// ```ignore
/// pub struct MySTTPlugin;
///
/// impl PluginCapability for MySTTPlugin {}
///
/// impl STTCapability for MySTTPlugin {
///     fn provider_id(&self) -> &'static str { "my-stt" }
///
///     fn create_stt(&self, config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
///         Ok(Box::new(MySTT::new(config)?))
///     }
///
///     fn metadata(&self) -> ProviderMetadata {
///         ProviderMetadata::stt("my-stt", "My STT Provider")
///     }
/// }
/// ```
pub trait STTCapability: PluginCapability {
    /// Provider identifier used in configuration (e.g., "deepgram", "google")
    fn provider_id(&self) -> &'static str;

    /// Create an STT provider instance with the given configuration
    fn create_stt(&self, config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError>;

    /// Provider metadata for discovery and documentation
    fn metadata(&self) -> ProviderMetadata;
}

/// TTS (Text-to-Speech) provider capability
///
/// Implement this trait to register a TTS provider with the plugin system.
pub trait TTSCapability: PluginCapability {
    /// Provider identifier used in configuration
    fn provider_id(&self) -> &'static str;

    /// Create a TTS provider instance with the given configuration
    fn create_tts(&self, config: TTSConfig) -> TTSResult<Box<dyn BaseTTS>>;

    /// Provider metadata for discovery and documentation
    fn metadata(&self) -> ProviderMetadata;
}

/// Realtime (Audio-to-Audio) provider capability
///
/// Implement this trait for real-time audio processing providers like
/// OpenAI Realtime API or Hume.
pub trait RealtimeCapability: PluginCapability {
    /// Provider identifier used in configuration
    fn provider_id(&self) -> &'static str;

    /// Create a Realtime provider instance with the given configuration
    fn create_realtime(&self, config: RealtimeConfig) -> RealtimeResult<Box<dyn BaseRealtime>>;

    /// Provider metadata for discovery and documentation
    fn metadata(&self) -> ProviderMetadata;
}

/// Audio processor capability for pipeline processing
///
/// Implement this trait for audio processing plugins like VAD, noise filter,
/// resampling, etc.
#[async_trait]
pub trait AudioProcessorCapability: PluginCapability {
    /// Processor identifier
    fn processor_id(&self) -> &'static str;

    /// Create a processor instance with the given configuration
    fn create_processor(
        &self,
        config: Value,
    ) -> Result<Box<dyn AudioProcessor>, AudioProcessorError>;

    /// Processor metadata
    fn metadata(&self) -> ProcessorMetadata;
}

/// Audio processor trait for processing audio data
#[async_trait]
pub trait AudioProcessor: Send + Sync {
    /// Process audio data
    ///
    /// Takes audio bytes and format, returns processed audio bytes.
    /// Should be zero-copy where possible.
    async fn process(
        &self,
        audio: Bytes,
        format: &AudioFormat,
    ) -> Result<Bytes, AudioProcessorError>;

    /// Whether this processor modifies audio duration
    fn changes_duration(&self) -> bool {
        false
    }

    /// Estimated latency added by this processor in milliseconds
    fn latency_ms(&self) -> u32 {
        0
    }
}

/// Audio format specification
#[derive(Debug, Clone)]
pub struct AudioFormat {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of channels
    pub channels: u16,
    /// Bits per sample
    pub bits_per_sample: u16,
    /// Audio encoding
    pub encoding: AudioEncoding,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
            encoding: AudioEncoding::Pcm,
        }
    }
}

/// Audio encoding type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEncoding {
    Pcm,
    MuLaw,
    ALaw,
    Opus,
    Mp3,
}

/// Audio processor metadata
#[derive(Debug, Clone, Default)]
pub struct ProcessorMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub supported_formats: Vec<AudioEncoding>,
}

/// Audio processor error type
#[derive(Debug, thiserror::Error)]
pub enum AudioProcessorError {
    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Processing error: {0}")]
    ProcessingError(String),

    #[error("Unsupported format: {0:?}")]
    UnsupportedFormat(AudioEncoding),

    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Middleware capability for Axum middleware layers
///
/// Implement this trait to add custom middleware to the gateway.
pub trait MiddlewareCapability: PluginCapability {
    /// Middleware identifier
    fn middleware_id(&self) -> &'static str;

    /// Priority for ordering (lower = earlier in chain)
    fn priority(&self) -> i32;

    /// Create the middleware layer
    fn create_middleware(&self, config: Value) -> Result<MiddlewareLayer, MiddlewareError>;

    /// Middleware metadata
    fn metadata(&self) -> MiddlewareMetadata;
}

/// Middleware layer type (boxed tower layer)
pub type MiddlewareLayer =
    Box<dyn tower::Layer<axum::Router, Service = axum::Router> + Send + Sync>;

/// Middleware metadata
#[derive(Debug, Clone, Default)]
pub struct MiddlewareMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// Middleware error type
#[derive(Debug, thiserror::Error)]
pub enum MiddlewareError {
    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Initialization error: {0}")]
    InitializationError(String),
}

/// WebSocket message handler capability
///
/// Implement this trait to handle custom WebSocket message types.
pub trait WSHandlerCapability: PluginCapability {
    /// Message type this handler handles (e.g., "custom_command")
    fn message_type(&self) -> &'static str;

    /// Handle the message
    ///
    /// Returns a future that resolves to an optional response.
    fn handle<'a>(
        &'a self,
        msg: Value,
        ctx: &'a WSContext,
    ) -> Pin<Box<dyn Future<Output = Result<Option<WSResponse>, WSError>> + Send + 'a>>;
}

/// WebSocket handler context
#[derive(Clone)]
pub struct WSContext {
    /// Stream ID for this connection
    pub stream_id: String,
    /// Whether the connection is authenticated
    pub authenticated: bool,
    /// Tenant ID if authenticated
    pub tenant_id: Option<String>,
}

/// WebSocket response type
#[derive(Debug)]
pub enum WSResponse {
    /// JSON response
    Json(Value),
    /// Binary response
    Binary(Bytes),
    /// Multiple responses
    Multiple(Vec<WSResponse>),
    /// No response (handled internally)
    None,
}

/// WebSocket handler error
#[derive(Debug, thiserror::Error)]
pub enum WSError {
    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Handler error: {0}")]
    HandlerError(String),

    #[error("Not authenticated")]
    NotAuthenticated,

    #[error("Permission denied: {0}")]
    PermissionDenied(String),
}

/// Auth strategy capability
///
/// Implement this trait to add custom authentication strategies.
pub trait AuthCapability: PluginCapability {
    /// Strategy identifier (e.g., "jwt", "api-key", "oauth2")
    fn strategy_id(&self) -> &'static str;

    /// Create an auth strategy instance
    fn create_strategy(&self, config: Value) -> Result<Box<dyn AuthStrategy>, AuthStrategyError>;

    /// Strategy metadata
    fn metadata(&self) -> AuthMetadata;
}

/// Auth strategy trait
#[async_trait]
pub trait AuthStrategy: Send + Sync {
    /// Validate authentication credentials
    ///
    /// Returns the authenticated identity on success.
    async fn authenticate(
        &self,
        credentials: &AuthCredentials,
    ) -> Result<AuthIdentity, AuthStrategyError>;

    /// Extract credentials from HTTP headers
    fn extract_credentials(&self, headers: &http::HeaderMap) -> Option<AuthCredentials>;
}

/// Authentication credentials
#[derive(Debug, Clone)]
pub struct AuthCredentials {
    /// Credential type (e.g., "bearer", "api-key")
    pub credential_type: String,
    /// The credential value
    pub value: String,
}

/// Authenticated identity
#[derive(Debug, Clone)]
pub struct AuthIdentity {
    /// Unique identifier
    pub id: String,
    /// Tenant ID (for multi-tenant setups)
    pub tenant_id: Option<String>,
    /// Roles/permissions
    pub roles: Vec<String>,
}

/// Auth strategy metadata
#[derive(Debug, Clone, Default)]
pub struct AuthMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// Auth strategy error
#[derive(Debug, thiserror::Error)]
pub enum AuthStrategyError {
    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Expired credentials")]
    ExpiredCredentials,

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_format_default() {
        let format = AudioFormat::default();
        assert_eq!(format.sample_rate, 16000);
        assert_eq!(format.channels, 1);
        assert_eq!(format.bits_per_sample, 16);
    }
}

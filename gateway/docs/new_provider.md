# Adding New Providers to Bud WaaV

This comprehensive guide explains how to add new providers to Bud WaaV, covering Speech-to-Text (STT), Text-to-Speech (TTS), Audio-to-Audio, Voice Cloning, and Dubbing/Translation providers.

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [STT Provider Guide](#stt-provider-guide)
3. [TTS Provider Guide](#tts-provider-guide)
4. [Audio-to-Audio Provider Guide](#audio-to-audio-provider-guide)
5. [Voice Cloning Provider Guide](#voice-cloning-provider-guide)
6. [Dubbing/Translation Provider Guide](#dubbingtranslation-provider-guide)
7. [Configuration & Environment Variables](#configuration--environment-variables)
8. [Testing Best Practices](#testing-best-practices)
9. [Performance Guidelines](#performance-guidelines)
10. [Security Checklist](#security-checklist)

---

## Architecture Overview

### Provider System Design

Bud WaaV uses a **trait-based abstraction** with **factory pattern** for provider management:

```
Client Request
      │
      ▼
┌─────────────────────────────────────┐
│     Rust Gateway (Axum)             │
│  - WebSocket: /ws (real-time)       │
│  - REST: /speak, /voices            │
└─────────────────────────────────────┘
      │
      │ Factory Pattern
      ▼
┌─────────────────────────────────────┐
│     Provider Factory                │
│  - create_stt_provider()            │
│  - create_tts_provider()            │
└─────────────────────────────────────┘
      │
      ▼
┌─────────────────────────────────────┐
│     VoiceManager                    │
│  - Orchestrates STT/TTS lifecycle   │
│  - Routes audio and callbacks       │
│  - Thread-safe with Arc<RwLock<>>   │
└─────────────────────────────────────┘
      │
      ▼
┌─────────────────────────────────────┐
│     Provider Implementation         │
│  - BaseSTT / BaseTTS trait impl     │
│  - WebSocket, HTTP, or gRPC client  │
└─────────────────────────────────────┘
```

### Key Design Principles

1. **Trait-Based Abstraction**: All providers implement common traits (`BaseSTT`, `BaseTTS`)
2. **Factory Pattern**: Dynamic provider instantiation via factory functions
3. **Zero-Copy Audio**: Use `Bytes` type for efficient audio handling
4. **Async/Await**: All I/O operations are async using Tokio
5. **Callback Pattern**: Event-driven results via registered callbacks
6. **Lock-Free Hot Paths**: Use `Arc<AtomicBool>` for connection state

### Existing Providers

| Provider | STT | TTS | Protocol | Key Features |
|----------|-----|-----|----------|--------------|
| Deepgram | Yes | Yes | WebSocket / HTTP | Nova-3 model, interim results |
| Google | Yes | Yes | gRPC / HTTP | WaveNet voices, 60+ languages |
| ElevenLabs | Yes | Yes | WebSocket / HTTP | Voice cloning, 29 languages |
| Azure | Yes | Yes | WebSocket / HTTP | 400+ voices, 140+ languages |
| Cartesia | Yes | Yes | WebSocket | Sonic-3 model, voice cloning |

---

## STT Provider Guide

### BaseSTT Trait Definition

Location: `src/core/stt/base.rs`

```rust
/// Base trait for Speech-to-Text providers
#[async_trait::async_trait]
pub trait BaseSTT: Send + Sync {
    /// Create a new instance of the STT provider with the given configuration
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized;

    /// Connect to the STT provider
    async fn connect(&mut self) -> Result<(), STTError>;

    /// Disconnect from the STT provider
    async fn disconnect(&mut self) -> Result<(), STTError>;

    /// Check if the connection is ready to be used
    fn is_ready(&self) -> bool;

    /// Send audio data to the STT provider for transcription
    /// IMPORTANT: Use Bytes for zero-copy audio handling
    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError>;

    /// Register a callback for transcription results
    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError>;

    /// Register a callback for streaming errors
    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError>;

    /// Get the current configuration
    fn get_config(&self) -> Option<&STTConfig>;

    /// Update configuration while maintaining connection
    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError>;

    /// Get provider-specific information
    fn get_provider_info(&self) -> &'static str;
}
```

### STTConfig Structure

```rust
/// Configuration for STT providers
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct STTConfig {
    pub provider: String,
    /// API key for the STT provider
    pub api_key: String,
    /// Language code for transcription (e.g., "en-US", "es-ES")
    pub language: String,
    /// Sample rate of the audio in Hz
    pub sample_rate: u32,
    /// Number of audio channels (1 for mono, 2 for stereo)
    pub channels: u16,
    /// Enable punctuation in results
    pub punctuation: bool,
    /// Encoding of the audio (e.g., "linear16", "pcm")
    pub encoding: String,
    /// Model to use for transcription
    pub model: String,
}

impl Default for STTConfig {
    fn default() -> Self {
        Self {
            model: "nova-3".to_string(),
            provider: String::new(),
            api_key: String::new(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
        }
    }
}
```

### STTResult Structure

```rust
/// Result structure containing transcription data
#[derive(Debug, Clone, PartialEq)]
pub struct STTResult {
    /// The transcribed text from the audio
    pub transcript: String,
    /// Whether this is a final transcription result (not interim)
    pub is_final: bool,
    /// Whether this marks the end of a speech segment
    pub is_speech_final: bool,
    /// Confidence score of the transcription (0.0 to 1.0)
    pub confidence: f32,
}
```

### STTError Types

```rust
#[derive(Debug, Clone, thiserror::Error)]
pub enum STTError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),
    #[error("Audio processing error: {0}")]
    AudioProcessingError(String),
    #[error("Provider error: {0}")]
    ProviderError(String),
    #[error("Configuration error: {0}")]
    ConfigurationError(String),
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Invalid audio format: {0}")]
    InvalidAudioFormat(String),
}
```

### Callback Types

```rust
/// Type alias for STT result callback
pub type STTResultCallback =
    Arc<dyn Fn(STTResult) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Type alias for STT error callback
pub type STTErrorCallback =
    Arc<dyn Fn(STTError) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
```

### Step-by-Step: Implementing a New STT Provider

#### Step 1: Create Provider Directory Structure

```
src/core/stt/
└── new_provider/
    ├── mod.rs           # Module exports
    ├── config.rs        # Provider-specific configuration
    ├── client.rs        # WebSocket/HTTP/gRPC client implementation
    ├── messages.rs      # API message types (request/response)
    └── tests.rs         # Unit tests
```

#### Step 2: Define Provider-Specific Configuration

Create `src/core/stt/new_provider/config.rs`:

```rust
use serde::{Deserialize, Serialize};
use crate::core::stt::base::STTConfig;

/// Provider-specific configuration extending base STTConfig
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProviderSTTConfig {
    /// Base configuration (api_key, language, sample_rate, etc.)
    #[serde(flatten)]
    pub base: STTConfig,

    // Provider-specific options
    /// Custom model identifier
    pub custom_model: Option<String>,
    /// Enable special feature
    pub feature_flag: bool,
    /// Request timeout in milliseconds
    pub timeout_ms: Option<u64>,
    /// Regional endpoint selection
    pub region: Option<String>,
}

impl Default for NewProviderSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            custom_model: Some("default-model".to_string()),
            feature_flag: false,
            timeout_ms: Some(30000),
            region: None,
        }
    }
}

impl NewProviderSTTConfig {
    /// Create from base config with defaults
    pub fn from_base(base: STTConfig) -> Self {
        Self {
            base,
            ..Default::default()
        }
    }

    /// Build WebSocket URL for connection
    pub fn build_websocket_url(&self) -> String {
        let region = self.region.as_deref().unwrap_or("us-east-1");
        let mut url = format!(
            "wss://api.newprovider.com/{}/v1/stream?language={}",
            region, self.base.language
        );

        if let Some(model) = &self.custom_model {
            url.push_str(&format!("&model={}", model));
        }

        url
    }
}
```

#### Step 3: Define API Message Types

Create `src/core/stt/new_provider/messages.rs`:

```rust
use serde::{Deserialize, Serialize};

/// WebSocket message to configure the stream
#[derive(Debug, Serialize)]
pub struct ConfigureMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub config: StreamConfig,
}

#[derive(Debug, Serialize)]
pub struct StreamConfig {
    pub language: String,
    pub sample_rate: u32,
    pub encoding: String,
    pub interim_results: bool,
    pub punctuation: bool,
}

/// API response message
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum NewProviderMessage {
    #[serde(rename = "transcript")]
    Transcript(TranscriptMessage),
    #[serde(rename = "error")]
    Error(ErrorMessage),
    #[serde(rename = "metadata")]
    Metadata(MetadataMessage),
}

#[derive(Debug, Deserialize)]
pub struct TranscriptMessage {
    pub text: String,
    pub is_final: bool,
    pub confidence: Option<f32>,
    pub words: Option<Vec<WordInfo>>,
}

#[derive(Debug, Deserialize)]
pub struct WordInfo {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub confidence: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ErrorMessage {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct MetadataMessage {
    pub request_id: String,
    pub model_info: Option<String>,
}
```

#### Step 4: Implement the STT Client

Create `src/core/stt/new_provider/client.rs`:

```rust
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTResult, STTResultCallback, STTErrorCallback
};
use super::config::NewProviderSTTConfig;
use super::messages::{NewProviderMessage, ConfigureMessage, StreamConfig};

/// NewProvider STT client implementation
pub struct NewProviderSTT {
    /// Provider-specific configuration
    config: Option<NewProviderSTTConfig>,
    /// WebSocket connection (wrapped for Send + Sync)
    ws_sender: Option<Arc<RwLock<futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>
        >,
        Message,
    >>>>,
    /// Connection state (lock-free for performance)
    is_connected: Arc<AtomicBool>,
    /// Result callback
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,
    /// Error callback
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,
    /// Handle to message processing task
    message_task: Option<tokio::task::JoinHandle<()>>,
}

impl NewProviderSTT {
    /// Create a new instance
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate required configuration
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required for NewProvider STT".to_string()
            ));
        }

        let provider_config = NewProviderSTTConfig::from_base(config);

        Ok(Self {
            config: Some(provider_config),
            ws_sender: None,
            is_connected: Arc::new(AtomicBool::new(false)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            message_task: None,
        })
    }

    /// Process incoming WebSocket messages
    async fn process_message(
        &self,
        message: NewProviderMessage,
    ) -> Result<(), STTError> {
        match message {
            NewProviderMessage::Transcript(transcript) => {
                let result = STTResult::new(
                    transcript.text,
                    transcript.is_final,
                    transcript.is_final, // Use is_final for speech_final too
                    transcript.confidence.unwrap_or(0.0),
                );

                // Invoke callback if registered
                if let Some(callback) = self.result_callback.read().await.as_ref() {
                    callback(result).await;
                }
            }
            NewProviderMessage::Error(error) => {
                let stt_error = STTError::ProviderError(
                    format!("{}: {}", error.code, error.message)
                );

                // Invoke error callback if registered
                if let Some(callback) = self.error_callback.read().await.as_ref() {
                    callback(stt_error).await;
                }
            }
            NewProviderMessage::Metadata(metadata) => {
                debug!("Received metadata: request_id={}", metadata.request_id);
            }
        }
        Ok(())
    }
}

impl Default for NewProviderSTT {
    fn default() -> Self {
        Self {
            config: None,
            ws_sender: None,
            is_connected: Arc::new(AtomicBool::new(false)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            message_task: None,
        }
    }
}

#[async_trait]
impl BaseSTT for NewProviderSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        NewProviderSTT::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let config = self.config.as_ref()
            .ok_or_else(|| STTError::ConfigurationError("No configuration set".to_string()))?;

        // Build WebSocket URL
        let url = config.build_websocket_url();
        info!("Connecting to NewProvider STT: {}", url);

        // Create request with authentication header
        let request = http::Request::builder()
            .uri(&url)
            .header("Authorization", format!("Bearer {}", config.base.api_key))
            .header("Sec-WebSocket-Protocol", "v1.stt.newprovider.com")
            .body(())
            .map_err(|e| STTError::ConnectionFailed(e.to_string()))?;

        // Connect to WebSocket
        let (ws_stream, _response) = connect_async(request)
            .await
            .map_err(|e| STTError::ConnectionFailed(e.to_string()))?;

        let (sender, mut receiver) = ws_stream.split();
        self.ws_sender = Some(Arc::new(RwLock::new(sender)));

        // Send configuration message
        let configure_msg = ConfigureMessage {
            msg_type: "configure".to_string(),
            config: StreamConfig {
                language: config.base.language.clone(),
                sample_rate: config.base.sample_rate,
                encoding: config.base.encoding.clone(),
                interim_results: true,
                punctuation: config.base.punctuation,
            },
        };

        let msg_json = serde_json::to_string(&configure_msg)
            .map_err(|e| STTError::ConfigurationError(e.to_string()))?;

        if let Some(sender) = &self.ws_sender {
            sender.write().await
                .send(Message::Text(msg_json.into()))
                .await
                .map_err(|e| STTError::ConnectionFailed(e.to_string()))?;
        }

        // Spawn message processing task
        let result_callback = self.result_callback.clone();
        let error_callback = self.error_callback.clone();
        let is_connected = self.is_connected.clone();

        self.message_task = Some(tokio::spawn(async move {
            while let Some(msg_result) = receiver.next().await {
                match msg_result {
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<NewProviderMessage>(&text) {
                            Ok(message) => {
                                // Process message and invoke callbacks
                                match message {
                                    NewProviderMessage::Transcript(t) => {
                                        let result = STTResult::new(
                                            t.text,
                                            t.is_final,
                                            t.is_final,
                                            t.confidence.unwrap_or(0.0),
                                        );
                                        if let Some(cb) = result_callback.read().await.as_ref() {
                                            cb(result).await;
                                        }
                                    }
                                    NewProviderMessage::Error(e) => {
                                        let err = STTError::ProviderError(
                                            format!("{}: {}", e.code, e.message)
                                        );
                                        if let Some(cb) = error_callback.read().await.as_ref() {
                                            cb(err).await;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Err(e) => {
                                warn!("Failed to parse message: {}", e);
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        info!("WebSocket closed by server");
                        is_connected.store(false, Ordering::Release);
                        break;
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        is_connected.store(false, Ordering::Release);
                        break;
                    }
                    _ => {}
                }
            }
        }));

        self.is_connected.store(true, Ordering::Release);
        info!("Connected to NewProvider STT");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Send close frame
        if let Some(sender) = &self.ws_sender {
            let _ = sender.write().await.send(Message::Close(None)).await;
        }

        // Abort message task
        if let Some(task) = self.message_task.take() {
            task.abort();
        }

        self.ws_sender = None;
        self.is_connected.store(false, Ordering::Release);

        info!("Disconnected from NewProvider STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_connected.load(Ordering::Acquire)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        // CRITICAL: No heap allocations here - audio_data is already Bytes (zero-copy)
        if let Some(sender) = &self.ws_sender {
            sender.write().await
                .send(Message::Binary(audio_data.to_vec().into()))
                .await
                .map_err(|e| STTError::AudioProcessingError(e.to_string()))?;
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.write().await = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.write().await = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.config.as_ref().map(|c| &c.base)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        self.config = Some(NewProviderSTTConfig::from_base(config));
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "NewProvider STT WebSocket API"
    }
}
```

#### Step 5: Create Module Exports

Create `src/core/stt/new_provider/mod.rs`:

```rust
mod client;
mod config;
mod messages;

pub use client::NewProviderSTT;
pub use config::NewProviderSTTConfig;
pub use messages::{NewProviderMessage, TranscriptMessage};

#[cfg(test)]
mod tests;
```

#### Step 6: Register in Factory

Update `src/core/stt/mod.rs`:

```rust
// Add module declaration
pub mod new_provider;

// Add re-exports
pub use new_provider::{NewProviderSTT, NewProviderSTTConfig};

// Add to STTProvider enum
pub enum STTProvider {
    Deepgram,
    Google,
    ElevenLabs,
    Azure,
    Cartesia,
    NewProvider,  // Add new variant
}

// Update FromStr implementation
impl std::str::FromStr for STTProvider {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "deepgram" => Ok(STTProvider::Deepgram),
            "google" => Ok(STTProvider::Google),
            "elevenlabs" => Ok(STTProvider::ElevenLabs),
            "microsoft-azure" | "azure" => Ok(STTProvider::Azure),
            "cartesia" => Ok(STTProvider::Cartesia),
            "new-provider" | "newprovider" => Ok(STTProvider::NewProvider),  // Add
            _ => Err(STTError::ConfigurationError(format!(
                "Unsupported STT provider: {s}. Supported: deepgram, google, elevenlabs, \
                microsoft-azure, cartesia, new-provider"
            ))),
        }
    }
}

// Update factory function
pub fn create_stt_provider(
    provider: &str,
    config: STTConfig,
) -> Result<Box<dyn BaseSTT>, STTError> {
    let provider_enum: STTProvider = provider.parse()?;

    match provider_enum {
        STTProvider::Deepgram => Ok(Box::new(DeepgramSTT::new(config)?)),
        STTProvider::Google => Ok(Box::new(GoogleSTT::new(config)?)),
        STTProvider::ElevenLabs => Ok(Box::new(ElevenLabsSTT::new(config)?)),
        STTProvider::Azure => Ok(Box::new(AzureSTT::new(config)?)),
        STTProvider::Cartesia => Ok(Box::new(CartesiaSTT::new(config)?)),
        STTProvider::NewProvider => Ok(Box::new(NewProviderSTT::new(config)?)),  // Add
    }
}

// Update supported providers list
pub fn get_supported_stt_providers() -> Vec<&'static str> {
    vec![
        "deepgram",
        "google",
        "elevenlabs",
        "microsoft-azure",
        "cartesia",
        "new-provider",  // Add
    ]
}
```

---

## TTS Provider Guide

### BaseTTS Trait Definition

Location: `src/core/tts/base.rs`

```rust
/// Base trait for Text-to-Speech providers
#[async_trait]
pub trait BaseTTS: Send + Sync {
    /// Create a new instance of the TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized;

    /// Get the underlying TTSProvider for HTTP-based providers
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        None
    }

    /// Connect to the TTS provider
    async fn connect(&mut self) -> TTSResult<()>;

    /// Disconnect from the TTS provider
    async fn disconnect(&mut self) -> TTSResult<()>;

    /// Check if the TTS provider is ready to process requests
    fn is_ready(&self) -> bool;

    /// Get the current connection state
    fn get_connection_state(&self) -> ConnectionState;

    /// Send text to the TTS provider for synthesis
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()>;

    /// Clear any queued text
    async fn clear(&mut self) -> TTSResult<()>;

    /// Flush and start processing immediately
    async fn flush(&self) -> TTSResult<()>;

    /// Register an audio callback
    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()>;

    /// Remove the registered audio callback
    fn remove_audio_callback(&mut self) -> TTSResult<()>;

    /// Get provider-specific information
    fn get_provider_info(&self) -> serde_json::Value;

    /// Set the request manager for pooled HTTP clients
    async fn set_req_manager(&mut self, _req_manager: Arc<ReqManager>) {
        // Default no-op
    }
}
```

### TTSConfig Structure

```rust
/// Configuration for TTS providers
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TTSConfig {
    pub provider: String,
    /// API key for the TTS provider
    pub api_key: String,
    /// Voice ID or name to use for synthesis
    pub voice_id: Option<String>,
    /// Model to use for TTS
    pub model: String,
    /// Speaking rate (0.25 to 4.0, 1.0 is normal)
    pub speaking_rate: Option<f32>,
    /// Audio format preference (e.g., "linear16", "mp3", "pcm")
    pub audio_format: Option<String>,
    /// Sample rate preference
    pub sample_rate: Option<u32>,
    /// Connection timeout in seconds
    pub connection_timeout: Option<u64>,
    /// Request timeout in seconds
    pub request_timeout: Option<u64>,
    /// Pronunciation replacements to apply before TTS
    pub pronunciations: Vec<Pronunciation>,
    /// Request pool size for concurrent HTTP requests
    pub request_pool_size: Option<usize>,
}
```

### AudioCallback Trait

```rust
/// Audio callback trait for handling audio data from TTS providers
pub trait AudioCallback: Send + Sync {
    /// Called when audio data is received
    fn on_audio(&self, audio_data: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    /// Called when an error occurs
    fn on_error(&self, error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    /// Called when processing is complete
    fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Audio data structure for TTS output
#[derive(Debug, Clone)]
pub struct AudioData {
    /// Audio bytes in the format specified by the provider
    pub data: Vec<u8>,
    /// Sample rate of the audio
    pub sample_rate: u32,
    /// Audio format (e.g., "wav", "mp3", "pcm")
    pub format: String,
    /// Duration of the audio chunk in milliseconds
    pub duration_ms: Option<u32>,
}
```

### Two Implementation Approaches

#### Option A: HTTP-based TTS (Recommended for REST APIs)

For providers with REST APIs, extend the existing `TTSProvider` HTTP infrastructure:

```rust
use crate::core::tts::provider::TTSRequestBuilder;
use crate::core::tts::base::TTSConfig;

/// Request builder for HTTP-based TTS provider
pub struct NewProviderTTSBuilder {
    config: TTSConfig,
}

impl TTSRequestBuilder for NewProviderTTSBuilder {
    fn build_http_request(
        &self,
        client: &reqwest::Client,
        text: &str,
    ) -> reqwest::RequestBuilder {
        self.build_http_request_with_context(client, text, None)
    }

    fn build_http_request_with_context(
        &self,
        client: &reqwest::Client,
        text: &str,
        previous_text: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let voice_id = self.config.voice_id.as_deref().unwrap_or("default-voice");
        let url = format!("https://api.newprovider.com/v1/synthesize/{}", voice_id);

        // Build query parameters
        let sample_rate = self.config.sample_rate.unwrap_or(24000);
        let format = self.config.audio_format.as_deref().unwrap_or("pcm");

        let mut body = serde_json::json!({
            "text": text,
            "model": &self.config.model,
            "output_format": format!("{}_{}", format, sample_rate),
        });

        // Add context for voice continuity
        if let Some(prev) = previous_text {
            body["previous_text"] = serde_json::json!(prev);
        }

        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "audio/pcm")
            .json(&body)
    }

    fn get_config(&self) -> &TTSConfig {
        &self.config
    }
}

/// HTTP-based TTS provider wrapper
pub struct NewProviderTTS {
    provider: TTSProvider,
    request_builder: NewProviderTTSBuilder,
}

#[async_trait]
impl BaseTTS for NewProviderTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        if config.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "API key is required".to_string()
            ));
        }

        Ok(Self {
            provider: TTSProvider::new()?,
            request_builder: NewProviderTTSBuilder { config },
        })
    }

    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        self.provider
            .generic_connect_with_config(
                "https://api.newprovider.com",
                &self.request_builder.config
            )
            .await
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.provider.generic_disconnect().await
    }

    fn is_ready(&self) -> bool {
        self.provider.is_ready()
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.provider.get_connection_state()
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if !self.is_ready() {
            self.connect().await?;
        }
        self.provider
            .generic_speak(self.request_builder.clone(), text, flush)
            .await
    }

    async fn clear(&mut self) -> TTSResult<()> {
        self.provider.generic_clear().await
    }

    async fn flush(&self) -> TTSResult<()> {
        self.provider.generic_flush().await
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.provider.generic_on_audio(callback)
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        self.provider.generic_remove_audio_callback()
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "new-provider",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "endpoint": "https://api.newprovider.com/v1/synthesize"
        })
    }
}
```

#### Option B: WebSocket-based TTS

For providers with WebSocket streaming APIs (like Cartesia):

```rust
/// WebSocket-based TTS provider
pub struct NewProviderWebSocketTTS {
    config: Option<TTSConfig>,
    ws_sender: Option<WsSender>,
    is_connected: Arc<AtomicBool>,
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
}

#[async_trait]
impl BaseTTS for NewProviderWebSocketTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        // Validate and store config
        Ok(Self {
            config: Some(config),
            ws_sender: None,
            is_connected: Arc::new(AtomicBool::new(false)),
            audio_callback: Arc::new(RwLock::new(None)),
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        // 1. Build WebSocket URL
        // 2. Connect with authentication
        // 3. Spawn message handler
        // 4. Set is_connected = true
        todo!()
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        // 1. Build synthesis request message
        // 2. Send via WebSocket
        // 3. Audio arrives via message handler -> callback
        todo!()
    }

    // ... implement remaining methods
}
```

### Register TTS Provider in Factory

Update `src/core/tts/mod.rs`:

```rust
pub fn create_tts_provider(
    provider_type: &str,
    config: TTSConfig
) -> TTSResult<Box<dyn BaseTTS>> {
    match provider_type.to_lowercase().as_str() {
        "deepgram" => Ok(Box::new(DeepgramTTS::new(config)?)),
        "elevenlabs" => Ok(Box::new(ElevenLabsTTS::new(config)?)),
        "google" => Ok(Box::new(GoogleTTS::new(config)?)),
        "azure" | "microsoft-azure" => Ok(Box::new(AzureTTS::new(config)?)),
        "cartesia" => Ok(Box::new(CartesiaTTS::new(config)?)),
        "new-provider" => Ok(Box::new(NewProviderTTS::new(config)?)),  // Add
        _ => Err(TTSError::InvalidConfiguration(format!(
            "Unsupported TTS provider: {provider_type}"
        ))),
    }
}
```

---

## Audio-to-Audio Provider Guide

Audio-to-Audio (A2A) providers transform audio input into different audio output (voice conversion, style transfer, accent modification, etc.).

### Proposed BaseA2A Trait

This is a **new provider category** to be implemented:

```rust
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;

/// Configuration for A2A providers
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct A2AConfig {
    pub provider: String,
    pub api_key: String,
    /// Target voice/style for transformation
    pub target_voice_id: Option<String>,
    /// Transformation model
    pub model: String,
    /// Preserve original prosody/timing
    pub preserve_prosody: bool,
    /// Sample rate for input/output
    pub sample_rate: u32,
}

/// Result callback for A2A processing
pub type A2AResultCallback = Arc<dyn Fn(A2AResult) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A2A processing result
#[derive(Debug, Clone)]
pub struct A2AResult {
    /// Transformed audio data
    pub audio: Vec<u8>,
    /// Sample rate
    pub sample_rate: u32,
    /// Processing latency in ms
    pub latency_ms: u32,
}

/// Base trait for Audio-to-Audio providers
#[async_trait]
pub trait BaseA2A: Send + Sync {
    /// Create new instance
    fn new(config: A2AConfig) -> Result<Self, A2AError> where Self: Sized;

    /// Connect to provider
    async fn connect(&mut self) -> Result<(), A2AError>;

    /// Disconnect from provider
    async fn disconnect(&mut self) -> Result<(), A2AError>;

    /// Check if ready
    fn is_ready(&self) -> bool;

    /// Send audio for transformation (streaming)
    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), A2AError>;

    /// Process audio batch (non-streaming)
    async fn process_batch(&mut self, audio_data: Bytes) -> Result<A2AResult, A2AError>;

    /// Register result callback
    async fn on_result(&mut self, callback: A2AResultCallback) -> Result<(), A2AError>;

    /// Get provider info
    fn get_provider_info(&self) -> &'static str;
}
```

### Integration Points

A2A providers integrate into the existing audio pipeline:

```
Audio Input → [Optional: DeepFilterNet] → A2A Provider → STT/Output
                                               │
                                               ▼
                                    Voice Conversion / Style Transfer
```

**Key Files to Modify:**
- `src/core/` - Add new `a2a/` module
- `src/handlers/ws/audio_handler.rs` - Route audio to A2A providers
- `src/core/voice_manager/manager.rs` - Add A2A provider management

### Reference Implementation: DeepFilterNet

For DSP-style audio processing, reference `src/utils/noise_filter.rs`:

```rust
// Key patterns from noise_filter.rs:

// 1. Lazy initialization for models
static DEEP_FILTER: LazyLock<Option<Arc<DeepFilterWrapper>>> = LazyLock::new(|| {
    // Load model once, share across requests
});

// 2. Thread pool for CPU-intensive work
static WORKER_POOL: LazyLock<Option<Arc<WorkerPool>>> = LazyLock::new(|| {
    // Dedicated threads for audio processing
});

// 3. Adaptive processing based on audio characteristics
fn should_process(audio: &[f32]) -> ProcessingDecision {
    // Check RMS, SNR, etc. to decide processing level
}

// 4. Pre-allocated buffers
struct AudioProcessor {
    input_buffer: Vec<f32>,   // Pre-allocated
    output_buffer: Vec<f32>,  // Pre-allocated
}
```

---

## Voice Cloning Provider Guide

Voice cloning creates custom voices from audio samples. Existing implementations:

### ElevenLabs Voice Cloning

Location: `src/core/tts/elevenlabs.rs`

```rust
/// Voice settings for ElevenLabs TTS with cloning parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceSettings {
    /// Voice stability (0.0 to 1.0) - lower = more variable/expressive
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stability: Option<f32>,

    /// Similarity boost (0.0 to 1.0) - higher = closer to original voice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_boost: Option<f32>,

    /// Style strength (0.0 to 1.0) - emotion/style transfer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<f32>,

    /// Use speaker boost for clarity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_speaker_boost: Option<bool>,

    /// Speaking rate (0.25 to 4.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            stability: Some(0.5),       // Balanced stability
            similarity_boost: Some(0.8), // High similarity to cloned voice
            style: Some(0.0),           // No style transfer by default
            use_speaker_boost: Some(false),
            speed: Some(1.0),
        }
    }
}
```

### Using Cloned Voices

1. **Create cloned voice via provider dashboard** (ElevenLabs, Cartesia)
2. **Get voice_id** from dashboard
3. **Configure TTS with voice_id**:

```rust
let config = TTSConfig {
    provider: "elevenlabs".to_string(),
    api_key: "your-api-key".to_string(),
    voice_id: Some("cloned-voice-uuid".to_string()),  // Your cloned voice
    model: "eleven_v3".to_string(),
    speaking_rate: Some(1.0),
    ..Default::default()
};

let tts = create_tts_provider("elevenlabs", config)?;
```

### Voice Cloning Parameters

| Parameter | ElevenLabs | Cartesia | Effect |
|-----------|------------|----------|--------|
| stability | 0.0-1.0 | N/A | Lower = more expressive |
| similarity_boost | 0.0-1.0 | N/A | Higher = closer to original |
| style | 0.0-1.0 | N/A | Emotion/style transfer |
| speed | 0.25-4.0 | 0.5-2.0 | Speaking rate |

### Implementing Custom Voice Cloning

For local voice cloning (RVC, Coqui, etc.):

```rust
/// Local voice cloning provider using RVC or similar
pub struct LocalVoiceCloningTTS {
    model_path: PathBuf,
    voice_embedding: Vec<f32>,
    config: TTSConfig,
}

impl LocalVoiceCloningTTS {
    /// Load voice embedding from audio sample
    pub async fn clone_voice(&mut self, audio_sample: &[u8]) -> Result<String, TTSError> {
        // 1. Extract speaker embedding from audio
        // 2. Store embedding with unique ID
        // 3. Return voice_id for future use
        todo!()
    }

    /// Synthesize using cloned voice
    pub async fn synthesize(&self, text: &str, voice_id: &str) -> Result<Vec<u8>, TTSError> {
        // 1. Load voice embedding
        // 2. Run TTS model with embedding
        // 3. Return audio
        todo!()
    }
}
```

---

## Dubbing/Translation Provider Guide

Dubbing involves translating speech from one language to another while preserving voice characteristics.

### Pipeline Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Dubbing Pipeline                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Audio Input (Source Language)                                   │
│       │                                                          │
│       ▼                                                          │
│  ┌─────────────────┐                                            │
│  │   STT Provider  │  ← Transcribe source language              │
│  │   (Deepgram,    │                                            │
│  │    Azure, etc.) │                                            │
│  └────────┬────────┘                                            │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────┐                                            │
│  │   Translation   │  ← External translation service            │
│  │   Service       │    (Google Translate, DeepL, etc.)         │
│  │                 │                                            │
│  └────────┬────────┘                                            │
│           │                                                      │
│           ▼                                                      │
│  ┌─────────────────┐                                            │
│  │   TTS Provider  │  ← Synthesize target language              │
│  │   (ElevenLabs,  │    with voice cloning (optional)           │
│  │    Cartesia)    │                                            │
│  └────────┬────────┘                                            │
│           │                                                      │
│           ▼                                                      │
│  Audio Output (Target Language)                                  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Proposed DubbingProvider Trait

```rust
/// Configuration for dubbing/translation
#[derive(Debug, Clone)]
pub struct DubbingConfig {
    /// Source language code (e.g., "en-US")
    pub source_language: String,
    /// Target language code (e.g., "es-ES")
    pub target_language: String,
    /// STT provider for source transcription
    pub stt_provider: String,
    /// TTS provider for target synthesis
    pub tts_provider: String,
    /// Translation service
    pub translation_service: String,
    /// Voice ID for target language (optional, for voice cloning)
    pub target_voice_id: Option<String>,
    /// Preserve original timing/pacing
    pub preserve_timing: bool,
}

/// Dubbing provider combining STT, translation, and TTS
pub struct DubbingProvider {
    config: DubbingConfig,
    stt: Box<dyn BaseSTT>,
    tts: Box<dyn BaseTTS>,
    translator: Box<dyn TranslationService>,
}

impl DubbingProvider {
    pub async fn dub_audio(&mut self, audio: Bytes) -> Result<DubbingResult, DubbingError> {
        // 1. Transcribe source audio
        let transcription = self.transcribe(audio).await?;

        // 2. Translate text
        let translated = self.translator.translate(
            &transcription.text,
            &self.config.source_language,
            &self.config.target_language,
        ).await?;

        // 3. Synthesize in target language
        let audio_output = self.synthesize(&translated).await?;

        Ok(DubbingResult {
            original_text: transcription.text,
            translated_text: translated,
            audio: audio_output,
            timing_info: if self.config.preserve_timing {
                Some(transcription.word_timings)
            } else {
                None
            },
        })
    }
}
```

### Multi-Language Support

Current provider language support:

| Provider | STT Languages | TTS Languages |
|----------|---------------|---------------|
| Deepgram | 36+ | 7 |
| Google | 125+ | 60+ |
| Azure | 100+ | 140+ |
| ElevenLabs | 29 | 29 |
| Cartesia | 30+ | 30+ |

---

## Configuration & Environment Variables

### Environment Variable Naming Convention

```bash
# Pattern: {PROVIDER_NAME}_{SETTING_NAME}

# API Keys
DEEPGRAM_API_KEY="your-deepgram-key"
ELEVENLABS_API_KEY="your-elevenlabs-key"
CARTESIA_API_KEY="your-cartesia-key"
GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
AZURE_SPEECH_SUBSCRIPTION_KEY="your-azure-key"
AZURE_SPEECH_REGION="eastus"

# New Provider
NEW_PROVIDER_API_KEY="your-new-provider-key"
NEW_PROVIDER_REGION="us-east-1"
```

### YAML Configuration

Add to `config.example.yaml`:

```yaml
providers:
  # Existing providers
  deepgram_api_key: "${DEEPGRAM_API_KEY}"
  elevenlabs_api_key: "${ELEVENLABS_API_KEY}"
  google_credentials: "${GOOGLE_APPLICATION_CREDENTIALS}"
  azure_speech_subscription_key: "${AZURE_SPEECH_SUBSCRIPTION_KEY}"
  azure_speech_region: "${AZURE_SPEECH_REGION:-eastus}"
  cartesia_api_key: "${CARTESIA_API_KEY}"

  # New provider
  new_provider_api_key: "${NEW_PROVIDER_API_KEY}"
  new_provider_region: "${NEW_PROVIDER_REGION:-us-east-1}"

stt:
  # Default STT provider
  provider: "deepgram"

  # Provider-specific settings
  new_provider:
    model: "default-model"
    feature_flag: false
    timeout_ms: 30000

tts:
  # Default TTS provider
  provider: "elevenlabs"

  # Provider-specific settings
  new_provider:
    model: "default-voice-model"
    voice_id: "default-voice-id"
```

### Configuration Loading

Update `src/config/mod.rs`:

```rust
/// Get API key for a specific provider
pub fn get_provider_api_key(&self, provider: &str) -> Option<String> {
    match provider.to_lowercase().as_str() {
        "deepgram" => self.providers.deepgram_api_key.clone(),
        "elevenlabs" => self.providers.elevenlabs_api_key.clone(),
        "google" => self.providers.google_credentials.clone(),
        "azure" | "microsoft-azure" => self.providers.azure_speech_subscription_key.clone(),
        "cartesia" => self.providers.cartesia_api_key.clone(),
        "new-provider" => self.providers.new_provider_api_key.clone(),  // Add
        _ => None,
    }
}
```

---

## Testing Best Practices

### Unit Test Template

Create `src/core/stt/new_provider/tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::{BaseSTT, STTConfig, STTError};

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "new-provider".to_string(),
            api_key: "test-api-key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "test-model".to_string(),
        }
    }

    // =================================================================
    // Provider Creation Tests
    // =================================================================

    #[test]
    fn test_new_provider_creation() {
        let config = create_test_config();
        let result = NewProviderSTT::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_provider_requires_api_key() {
        let mut config = create_test_config();
        config.api_key = String::new();

        let result = NewProviderSTT::new(config);
        assert!(result.is_err());

        match result {
            Err(STTError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("API key"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    // =================================================================
    // Connection State Tests
    // =================================================================

    #[test]
    fn test_new_provider_not_connected_initially() {
        let config = create_test_config();
        let provider = NewProviderSTT::new(config).unwrap();
        assert!(!provider.is_ready());
    }

    #[tokio::test]
    async fn test_send_audio_requires_connection() {
        let config = create_test_config();
        let mut provider = NewProviderSTT::new(config).unwrap();

        let result = provider.send_audio(bytes::Bytes::from_static(b"test")).await;
        assert!(result.is_err());

        match result {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("Not connected"));
            }
            _ => panic!("Expected ConnectionFailed error"),
        }
    }

    // =================================================================
    // Callback Tests
    // =================================================================

    #[tokio::test]
    async fn test_callback_registration() {
        let config = create_test_config();
        let mut provider = NewProviderSTT::new(config).unwrap();

        let callback = std::sync::Arc::new(|_result: crate::core::stt::STTResult| {
            Box::pin(async move {})
                as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let result = provider.on_result(callback).await;
        assert!(result.is_ok());
    }

    // =================================================================
    // Configuration Tests
    // =================================================================

    #[test]
    fn test_get_config() {
        let config = create_test_config();
        let provider = NewProviderSTT::new(config.clone()).unwrap();

        let retrieved = provider.get_config().unwrap();
        assert_eq!(retrieved.api_key, config.api_key);
        assert_eq!(retrieved.language, config.language);
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let provider = NewProviderSTT::new(config).unwrap();
        assert!(!provider.get_provider_info().is_empty());
    }
}
```

### Integration Test Template

Create `tests/new_provider_stt_integration.rs`:

```rust
//! Integration tests for NewProvider STT
//!
//! Tests requiring actual API calls are marked with #[ignore]
//! and require NEW_PROVIDER_API_KEY environment variable.

use waav_gateway::core::stt::{
    create_stt_provider, create_stt_provider_from_enum, get_supported_stt_providers,
    BaseSTT, STTConfig, STTError, STTProvider, STTResult,
};
use std::sync::Arc;

// =============================================================================
// Factory Integration Tests
// =============================================================================

#[test]
fn test_new_provider_in_supported_providers() {
    let providers = get_supported_stt_providers();
    assert!(providers.contains(&"new-provider"));
}

#[test]
fn test_create_new_provider_by_name() {
    let config = STTConfig {
        provider: "new-provider".to_string(),
        api_key: "test-key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let result = create_stt_provider("new-provider", config);
    assert!(result.is_ok());
}

#[test]
fn test_create_new_provider_case_insensitive() {
    let config = STTConfig {
        api_key: "test-key".to_string(),
        ..Default::default()
    };

    assert!(create_stt_provider("new-provider", config.clone()).is_ok());
    assert!(create_stt_provider("New-Provider", config.clone()).is_ok());
    assert!(create_stt_provider("NEW-PROVIDER", config).is_ok());
}

// =============================================================================
// Real Connection Tests (require credentials)
// =============================================================================

fn get_new_provider_credentials() -> Option<String> {
    std::env::var("NEW_PROVIDER_API_KEY").ok()
}

#[tokio::test]
#[ignore = "Requires NEW_PROVIDER_API_KEY environment variable"]
async fn test_new_provider_real_connection() {
    let api_key = get_new_provider_credentials()
        .expect("NEW_PROVIDER_API_KEY must be set");

    let config = STTConfig {
        api_key,
        language: "en-US".to_string(),
        sample_rate: 16000,
        ..Default::default()
    };

    let mut provider = create_stt_provider("new-provider", config).unwrap();

    // Register callback
    let received = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let received_clone = received.clone();

    let callback = Arc::new(move |result: STTResult| {
        let received = received_clone.clone();
        Box::pin(async move {
            println!("Received: {} (final: {})", result.transcript, result.is_final);
            received.store(true, std::sync::atomic::Ordering::SeqCst);
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });

    provider.on_result(callback).await.unwrap();

    // Connect
    let connect_result = provider.connect().await;
    assert!(connect_result.is_ok(), "Connection failed: {:?}", connect_result);
    assert!(provider.is_ready());

    // Disconnect
    provider.disconnect().await.unwrap();
    assert!(!provider.is_ready());
}
```

### Running Tests

```bash
# Run all tests
cargo test

# Run specific provider tests
cargo test new_provider

# Run with output
cargo test -- --nocapture

# Run credential tests (requires env vars)
cargo test -- --ignored --nocapture

# Run specific test
cargo test test_new_provider_creation

# Run with sanitizers (CI)
RUSTFLAGS="-Zsanitizer=address" cargo +nightly test
```

---

## Performance Guidelines

### Critical Performance Rules

1. **No Heap Allocations in `send_audio()` Hot Path**

   ```rust
   // BAD: Creates new Vec on each call
   async fn send_audio(&mut self, audio_data: Vec<u8>) -> Result<(), Error>

   // GOOD: Uses Bytes for zero-copy
   async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), Error>
   ```

2. **Use Lock-Free Atomics for Connection State**

   ```rust
   // BAD: Mutex lock on every is_ready() check
   fn is_ready(&self) -> bool {
       self.state.lock().unwrap().connected
   }

   // GOOD: Atomic load with proper ordering
   fn is_ready(&self) -> bool {
       self.is_connected.load(Ordering::Acquire)
   }
   ```

3. **Pre-Allocate Buffers During Connect**

   ```rust
   async fn connect(&mut self) -> Result<(), Error> {
       // Pre-allocate buffers for audio processing
       self.audio_buffer = Vec::with_capacity(4096);
       self.text_buffer = String::with_capacity(1024);
       // ... rest of connection
   }
   ```

4. **HTTP/2 Connection Pooling for HTTP Providers**

   ```rust
   // Use ReqManager for connection pooling
   pub async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
       self.provider.set_req_manager(req_manager).await;
   }
   ```

### Latency Budgets

| Component | Target Latency (p99) |
|-----------|---------------------|
| Audio capture to STT start | < 10ms |
| STT inference | < 50ms |
| TTS synthesis | < 100ms |
| End-to-end pipeline | < 200ms |

### Benchmarking Template

Create `benches/new_provider_benchmarks.rs`:

```rust
use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use std::time::Duration;

fn bench_message_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("new_provider_parsing");
    group.measurement_time(Duration::from_secs(5));

    let small_msg = r#"{"type":"transcript","text":"hello","is_final":true}"#;
    let large_msg = r#"{"type":"transcript","text":"This is a longer message with more content to parse and process","is_final":true,"confidence":0.95,"words":[{"word":"This","start":0.0,"end":0.1},{"word":"is","start":0.1,"end":0.2}]}"#;

    group.throughput(Throughput::Bytes(small_msg.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("small_message", small_msg.len()),
        &small_msg,
        |b, msg| {
            b.iter(|| {
                let _: serde_json::Value = serde_json::from_str(msg).unwrap();
            });
        },
    );

    group.throughput(Throughput::Bytes(large_msg.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("large_message", large_msg.len()),
        &large_msg,
        |b, msg| {
            b.iter(|| {
                let _: serde_json::Value = serde_json::from_str(msg).unwrap();
            });
        },
    );

    group.finish();
}

criterion_group!(benches, bench_message_parsing);
criterion_main!(benches);
```

Run benchmarks:
```bash
cargo bench new_provider
```

---

## Security Checklist

### API Key Security

- [ ] Never log API keys (use `#[serde(skip_serializing)]`)
- [ ] Store keys in environment variables, not code
- [ ] Use secure credential storage in production
- [ ] Rotate keys periodically

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProviderConfig {
    #[serde(skip_serializing)]  // Don't serialize API key
    pub api_key: String,
    // ...
}
```

### Input Validation

- [ ] Validate all external inputs (audio size, format, language codes)
- [ ] Sanitize text before sending to TTS
- [ ] Limit audio chunk sizes to prevent DoS
- [ ] Validate WebSocket messages before processing

```rust
async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), Error> {
    // Validate audio size
    const MAX_CHUNK_SIZE: usize = 1024 * 1024; // 1MB
    if audio_data.len() > MAX_CHUNK_SIZE {
        return Err(Error::InvalidAudioFormat(
            format!("Audio chunk too large: {} bytes", audio_data.len())
        ));
    }

    // ... process audio
}
```

### Network Security

- [ ] Use TLS for all connections (wss://, https://)
- [ ] Verify SSL certificates (don't disable verification)
- [ ] Implement request timeouts
- [ ] Handle connection errors gracefully

```rust
// Always use secure WebSocket (wss://)
let url = format!("wss://api.provider.com/v1/stream");

// Set timeouts
let client = reqwest::Client::builder()
    .connect_timeout(Duration::from_secs(10))
    .timeout(Duration::from_secs(30))
    .build()?;
```

### Rate Limiting

- [ ] Implement client-side rate limiting
- [ ] Handle 429 (Too Many Requests) responses
- [ ] Use exponential backoff for retries

---

## Quick Reference

### Files to Create for New STT Provider

```
src/core/stt/new_provider/
├── mod.rs           # Module exports
├── config.rs        # NewProviderSTTConfig
├── client.rs        # NewProviderSTT implements BaseSTT
├── messages.rs      # API message types
└── tests.rs         # Unit tests

tests/
└── new_provider_stt_integration.rs  # Integration tests
```

### Files to Modify

| File | Change |
|------|--------|
| `src/core/stt/mod.rs` | Add module, re-exports, enum variant, factory case |
| `src/core/tts/mod.rs` | Same for TTS providers |
| `src/config/mod.rs` | Add config loading |
| `src/config/env.rs` | Add environment variable |
| `config.example.yaml` | Document configuration |
| `Cargo.toml` | Add dependencies (if any) |

### Test Commands

```bash
# All tests
cargo test

# Specific provider
cargo test new_provider

# With credentials
cargo test -- --ignored

# With output
cargo test -- --nocapture

# Benchmarks
cargo bench new_provider

# Linting
cargo clippy -- -D warnings

# Formatting
cargo fmt --check
```

---

## Additional Resources

- [API Reference](./api-reference.md) - REST and WebSocket API documentation
- [WebSocket Protocol](./websocket.md) - Real-time communication protocol
- [Authentication](./authentication.md) - Auth configuration guide
- [Deployment](./deployment.md) - Production deployment guide

### Existing Provider Documentation

- [Azure STT](./azure-stt.md) - Azure Speech-to-Text integration
- [Azure TTS](./azure-tts.md) - Azure Text-to-Speech integration
- [Cartesia STT](./cartesia-stt.md) - Cartesia STT integration
- [Cartesia TTS](./cartesia-tts.md) - Cartesia TTS integration
- [Google STT](./google-stt.md) - Google Cloud STT integration
- [Google TTS](./google-tts.md) - Google Cloud TTS integration

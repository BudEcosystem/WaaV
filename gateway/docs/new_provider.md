# Adding New Providers to WaaV Gateway

This guide covers how to implement and register new STT, TTS, and Realtime providers using WaaV Gateway's plugin architecture.

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Plugin System Components](#plugin-system-components)
3. [STT Provider Implementation](#stt-provider-implementation)
4. [TTS Provider Implementation](#tts-provider-implementation)
5. [Realtime Provider Implementation](#realtime-provider-implementation)
6. [Provider Metadata](#provider-metadata)
7. [PHF Dispatch System](#phf-dispatch-system)
8. [Plugin Lifecycle](#plugin-lifecycle)
9. [Plugin Isolation](#plugin-isolation)
10. [WebSocket Handler Plugins](#websocket-handler-plugins)
11. [Testing](#testing)
12. [Quick Reference](#quick-reference)
13. [Configuration](#configuration)
14. [Security Checklist](#security-checklist)
15. [Performance Guidelines](#performance-guidelines)

---

## Architecture Overview

WaaV Gateway uses a compile-time plugin registration system based on the `inventory` crate. Providers are registered at compile time and indexed in O(1) PHF (Perfect Hash Function) maps for fast lookup.

### Plugin Registration Flow

```
                         Compile Time                      Runtime
                         ────────────                      ───────
┌─────────────────────────────────────────────────────────────────────────────┐
│                                                                             │
│  inventory::submit!(PluginConstructor)                                      │
│           │                                                                 │
│           ▼                                                                 │
│  ┌─────────────────┐                                                        │
│  │ inventory crate │ ──► Collects all PluginConstructor submissions         │
│  └────────┬────────┘                                                        │
│           │                                                                 │
│           ▼                                                                 │
│  ┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐       │
│  │ PHF Static Maps │ ◄── │ global_registry │ ◄── │ DashMap Runtime │       │
│  │ (O(1) builtin)  │     │   (OnceLock)    │     │   (O(1) amort)  │       │
│  └────────┬────────┘     └────────┬────────┘     └────────┬────────┘       │
│           │                       │                       │                 │
│           └───────────────────────┼───────────────────────┘                 │
│                                   ▼                                         │
│                    registry.create_stt("provider", config)                  │
│                    registry.create_tts("provider", config)                  │
│                    registry.create_realtime("provider", config)             │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Key Concepts

| Concept | Description |
|---------|-------------|
| `PluginConstructor` | Const-compatible struct holding factory function pointers |
| `inventory::submit!` | Compile-time registration macro |
| `PHF Maps` | Perfect Hash Function maps for O(1) guaranteed lookup |
| `DashMap` | Concurrent hashmap for runtime-registered providers |
| `ProviderMetadata` | Rich metadata for discovery and documentation |
| `Plugin Isolation` | Panic catching to prevent provider crashes from crashing gateway |

### Existing Providers

**STT Providers (11):**
- Deepgram, Google, ElevenLabs, Azure, Cartesia, OpenAI, AssemblyAI, AWS Transcribe, IBM Watson, Groq, Gnani

**TTS Providers (12):**
- Deepgram, ElevenLabs, Google, Azure, Cartesia, OpenAI, AWS Polly, IBM Watson, Hume, LMNT, PlayHT, Gnani

**Realtime Providers (2):**
- OpenAI Realtime, Hume EVI

---

## Plugin System Components

### Core Traits

#### 1. Base Provider Traits

Providers implement these async traits from `src/core/{stt,tts,realtime}/base.rs`:

```rust
// STT Provider Trait (src/core/stt/base.rs)
#[async_trait]
pub trait BaseSTT: Send + Sync {
    fn new(config: STTConfig) -> Result<Self, STTError> where Self: Sized;
    async fn connect(&mut self) -> Result<(), STTError>;
    async fn disconnect(&mut self) -> Result<(), STTError>;
    async fn send_audio(&mut self, audio: &[u8]) -> Result<(), STTError>;
    fn on_result(&mut self, callback: Arc<dyn STTCallback>) -> Result<(), STTError>;
    fn is_connected(&self) -> bool;
    fn get_provider_info(&self) -> serde_json::Value;
}

// TTS Provider Trait (src/core/tts/base.rs)
#[async_trait]
pub trait BaseTTS: Send + Sync {
    fn new(config: TTSConfig) -> TTSResult<Self> where Self: Sized;
    async fn connect(&mut self) -> TTSResult<()>;
    async fn disconnect(&mut self) -> TTSResult<()>;
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()>;
    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()>;
    fn is_ready(&self) -> bool;
    fn get_provider_info(&self) -> serde_json::Value;
}

// Realtime Provider Trait (src/core/realtime/base.rs)
#[async_trait]
pub trait BaseRealtime: Send + Sync {
    fn new(config: RealtimeConfig) -> Result<Self, RealtimeError> where Self: Sized;
    async fn connect(&mut self) -> Result<(), RealtimeError>;
    async fn disconnect(&mut self) -> Result<(), RealtimeError>;
    async fn send_audio(&mut self, audio: &[u8]) -> Result<(), RealtimeError>;
    async fn send_text(&mut self, text: &str) -> Result<(), RealtimeError>;
    fn on_event(&mut self, callback: Arc<dyn RealtimeCallback>) -> Result<(), RealtimeError>;
}
```

#### 2. PluginConstructor

The `PluginConstructor` struct enables compile-time registration with const-compatible function pointers:

```rust
// From src/plugin/registry.rs
pub struct PluginConstructor {
    /// Factory function to create the provider
    pub create_stt: Option<STTFactoryPtr>,
    pub create_tts: Option<TTSFactoryPtr>,
    pub create_realtime: Option<RealtimeFactoryPtr>,

    /// Metadata function (deferred creation for const compatibility)
    pub metadata_fn: MetadataFn,

    /// Provider ID for lookup
    pub provider_id: &'static str,

    /// Aliases for this provider
    pub aliases: &'static [&'static str],
}

impl PluginConstructor {
    /// Create an STT plugin constructor
    pub const fn stt(
        provider_id: &'static str,
        metadata_fn: MetadataFn,
        factory: fn(STTConfig) -> Result<Box<dyn BaseSTT>, STTError>,
    ) -> Self;

    /// Create a TTS plugin constructor
    pub const fn tts(
        provider_id: &'static str,
        metadata_fn: MetadataFn,
        factory: fn(TTSConfig) -> TTSResult<Box<dyn BaseTTS>>,
    ) -> Self;

    /// Create a Realtime plugin constructor
    pub const fn realtime(
        provider_id: &'static str,
        metadata_fn: MetadataFn,
        factory: fn(RealtimeConfig) -> RealtimeResult<Box<dyn BaseRealtime>>,
    ) -> Self;

    /// Add aliases for this provider
    pub const fn with_aliases(mut self, aliases: &'static [&'static str]) -> Self;
}
```

---

## STT Provider Implementation

### Step 1: Create Provider Directory Structure

```
src/core/stt/
└── my_provider/
    ├── mod.rs       # Module exports and documentation
    ├── config.rs    # Provider-specific configuration
    └── provider.rs  # Provider implementation
```

### Step 2: Implement STTConfig Extension (Optional)

If your provider needs configuration beyond `STTConfig`, create a config struct:

```rust
// src/core/stt/my_provider/config.rs
use crate::core::stt::STTConfig;

pub const MY_PROVIDER_URL: &str = "wss://api.myprovider.com/v1/stt";
pub const DEFAULT_MODEL: &str = "default-model";

#[derive(Clone, Debug)]
pub struct MyProviderSTTConfig {
    pub model: String,
    pub custom_option: Option<String>,
}

impl MyProviderSTTConfig {
    pub fn from_base(config: &STTConfig) -> Self {
        Self {
            model: if config.model.is_empty() {
                DEFAULT_MODEL.to_string()
            } else {
                config.model.clone()
            },
            custom_option: config.provider_options.get("custom_option").cloned(),
        }
    }
}
```

### Step 3: Implement BaseSTT Trait

```rust
// src/core/stt/my_provider/provider.rs
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, error};

use super::config::{MyProviderSTTConfig, MY_PROVIDER_URL};
use crate::core::stt::{BaseSTT, STTCallback, STTConfig, STTError, STTResult};

pub struct MyProviderSTT {
    config: STTConfig,
    provider_config: MyProviderSTTConfig,
    callback: Option<Arc<dyn STTCallback>>,
    connected: bool,
}

impl MyProviderSTT {
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate configuration
        if config.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "API key is required".to_string()
            ));
        }

        let provider_config = MyProviderSTTConfig::from_base(&config);

        info!(
            provider = "my-provider",
            model = %provider_config.model,
            "Created MyProvider STT instance"
        );

        Ok(Self {
            config,
            provider_config,
            callback: None,
            connected: false,
        })
    }
}

#[async_trait]
impl BaseSTT for MyProviderSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        MyProviderSTT::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        debug!("Connecting to MyProvider STT...");

        // Implement WebSocket/gRPC connection logic here
        // Example: self.ws = connect_websocket(MY_PROVIDER_URL, &self.config).await?;

        self.connected = true;
        info!("MyProvider STT connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        debug!("Disconnecting from MyProvider STT...");
        self.connected = false;
        Ok(())
    }

    async fn send_audio(&mut self, audio: &[u8]) -> Result<(), STTError> {
        if !self.connected {
            return Err(STTError::NotConnected);
        }

        // Send audio to provider
        // When results arrive, invoke the callback:
        // if let Some(callback) = &self.callback {
        //     callback.on_result(stt_result);
        // }

        Ok(())
    }

    fn on_result(&mut self, callback: Arc<dyn STTCallback>) -> Result<(), STTError> {
        self.callback = Some(callback);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "my-provider",
            "version": "1.0.0",
            "endpoint": MY_PROVIDER_URL,
            "model": self.provider_config.model,
            "features": ["streaming", "word-timestamps"]
        })
    }
}
```

### Step 4: Register Provider with Plugin System

Add registration to `src/plugin/builtin/mod.rs`:

```rust
// Add imports
use crate::core::stt::MyProviderSTT;

// Metadata function (must return ProviderMetadata)
fn my_provider_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("my-provider", "My Provider STT")
        .with_description("Custom STT provider with advanced features")
        .with_alias("myprov")  // Optional alias
        .with_features(["streaming", "word-timestamps", "custom-feature"])
        .with_languages(["en", "es", "fr"])
        .with_models(["default-model", "enhanced-model"])
}

// Factory function (must match STTFactoryPtr signature)
fn create_my_provider_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(MyProviderSTT::new(config)?))
}

// Register with inventory
inventory::submit! {
    PluginConstructor::stt("my-provider", my_provider_stt_metadata, create_my_provider_stt)
        .with_aliases(&["myprov", "my_provider"])
}
```

### Step 5: Add to PHF Dispatch Map

Add your provider to `src/plugin/dispatch.rs`:

```rust
// Add to BuiltinSTTProvider enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BuiltinSTTProvider {
    // ... existing providers ...
    MyProvider = 11,  // Next available index
}

impl BuiltinSTTProvider {
    pub const fn canonical_name(&self) -> &'static str {
        match self {
            // ... existing matches ...
            Self::MyProvider => "my-provider",
        }
    }
}

// Add to PHF map
pub static STT_PROVIDER_MAP: phf::Map<&'static str, BuiltinSTTProvider> = phf_map! {
    // ... existing entries ...
    "my-provider" => BuiltinSTTProvider::MyProvider,
    "myprov" => BuiltinSTTProvider::MyProvider,      // alias
    "my_provider" => BuiltinSTTProvider::MyProvider, // alias
};

// Update count
pub const BUILTIN_STT_COUNT: usize = 12;  // Increment

// Add to names array
pub const BUILTIN_STT_NAMES: [&str; BUILTIN_STT_COUNT] = [
    // ... existing names ...
    "my-provider",
];
```

### Step 6: Export from Module

Update `src/core/stt/mod.rs`:

```rust
pub mod my_provider;
pub use my_provider::{MyProviderSTT, MY_PROVIDER_URL};
```

---

## TTS Provider Implementation

### TTS-Specific Patterns

TTS providers typically use one of two patterns:

1. **HTTP REST API** - Use `TTSProvider` generic infrastructure with `TTSRequestBuilder`
2. **WebSocket Streaming** - Direct WebSocket implementation

### HTTP-Based TTS (Recommended Pattern)

WaaV provides `TTSProvider` infrastructure for HTTP-based TTS with:
- Connection pooling via `ReqManager`
- Ordered audio delivery via dispatcher
- Audio caching with config+text hash keys
- Queue worker for sequential processing

#### Implement TTSRequestBuilder

```rust
// src/core/tts/my_provider/provider.rs
use crate::core::tts::provider::{TTSProvider, TTSRequestBuilder, PronunciationReplacer};
use crate::core::tts::{BaseTTS, TTSConfig, TTSResult, AudioCallback, ConnectionState};

pub const MY_PROVIDER_TTS_URL: &str = "https://api.myprovider.com/v1/tts";

#[derive(Clone)]
pub struct MyProviderRequestBuilder {
    config: TTSConfig,
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl MyProviderRequestBuilder {
    pub fn new(config: TTSConfig) -> Self {
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };

        Self {
            config,
            pronunciation_replacer,
        }
    }
}

impl TTSRequestBuilder for MyProviderRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        let body = serde_json::json!({
            "text": text,
            "voice": self.config.voice_id,
            "format": "linear16",
            "sample_rate": self.config.sample_rate.unwrap_or(16000)
        });

        client
            .post(MY_PROVIDER_TTS_URL)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
    }

    fn get_config(&self) -> &TTSConfig {
        &self.config
    }

    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}
```

#### Implement BaseTTS Using TTSProvider

```rust
pub struct MyProviderTTS {
    provider: TTSProvider,
    request_builder: MyProviderRequestBuilder,
    config_hash: String,
}

impl MyProviderTTS {
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        let request_builder = MyProviderRequestBuilder::new(config.clone());
        let config_hash = compute_config_hash(&config);

        Ok(Self {
            provider: TTSProvider::new()?,
            request_builder,
            config_hash,
        })
    }
}

#[async_trait]
impl BaseTTS for MyProviderTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> where Self: Sized {
        MyProviderTTS::new(config)
    }

    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        self.provider
            .generic_connect_with_config(MY_PROVIDER_TTS_URL, &self.request_builder.config)
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

        self.provider.set_tts_config_hash(self.config_hash.clone()).await;
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
            "provider": "my-provider",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "endpoint": MY_PROVIDER_TTS_URL
        })
    }
}
```

### Register TTS Provider

```rust
// In src/plugin/builtin/mod.rs

fn my_provider_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("my-provider", "My Provider TTS")
        .with_description("Custom TTS with natural voice synthesis")
        .with_features(["streaming", "voice-cloning"])
        .with_languages(["en", "es", "fr"])
}

fn create_my_provider_tts(config: TTSConfig) -> TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(MyProviderTTS::new(config)?))
}

inventory::submit! {
    PluginConstructor::tts("my-provider", my_provider_tts_metadata, create_my_provider_tts)
        .with_aliases(&["myprov"])
}
```

---

## Realtime Provider Implementation

Realtime providers handle bidirectional audio streaming (audio-to-audio) like OpenAI Realtime API or Hume EVI.

### BaseRealtime Trait

```rust
// src/core/realtime/base.rs
#[async_trait]
pub trait BaseRealtime: Send + Sync {
    fn new(config: RealtimeConfig) -> Result<Self, RealtimeError> where Self: Sized;
    async fn connect(&mut self) -> Result<(), RealtimeError>;
    async fn disconnect(&mut self) -> Result<(), RealtimeError>;
    async fn send_audio(&mut self, audio: &[u8]) -> Result<(), RealtimeError>;
    async fn send_text(&mut self, text: &str) -> Result<(), RealtimeError>;
    fn on_event(&mut self, callback: Arc<dyn RealtimeCallback>) -> Result<(), RealtimeError>;
    fn is_connected(&self) -> bool;
    fn supports_interruption(&self) -> bool;
    fn get_provider_info(&self) -> serde_json::Value;
}

// Callback trait for realtime events
pub trait RealtimeCallback: Send + Sync {
    fn on_audio(&self, audio: AudioData);
    fn on_transcript(&self, transcript: &str, is_final: bool);
    fn on_function_call(&self, name: &str, arguments: serde_json::Value);
    fn on_error(&self, error: RealtimeError);
}
```

### Example Realtime Implementation

```rust
pub struct MyRealtimeProvider {
    config: RealtimeConfig,
    callback: Option<Arc<dyn RealtimeCallback>>,
    ws: Option<WebSocketStream>,
}

impl MyRealtimeProvider {
    pub fn new(config: RealtimeConfig) -> Result<Self, RealtimeError> {
        Ok(Self {
            config,
            callback: None,
            ws: None,
        })
    }
}

#[async_trait]
impl BaseRealtime for MyRealtimeProvider {
    fn new(config: RealtimeConfig) -> Result<Self, RealtimeError> where Self: Sized {
        MyRealtimeProvider::new(config)
    }

    async fn connect(&mut self) -> Result<(), RealtimeError> {
        // Establish WebSocket connection for bidirectional audio
        Ok(())
    }

    async fn send_audio(&mut self, audio: &[u8]) -> Result<(), RealtimeError> {
        // Send audio chunk to provider
        // Provider will respond with audio via callback
        Ok(())
    }

    async fn send_text(&mut self, text: &str) -> Result<(), RealtimeError> {
        // Send text message (e.g., for interruption or user input)
        Ok(())
    }

    fn on_event(&mut self, callback: Arc<dyn RealtimeCallback>) -> Result<(), RealtimeError> {
        self.callback = Some(callback);
        Ok(())
    }

    // ... other trait methods
}
```

### Register Realtime Provider

```rust
fn my_realtime_metadata() -> ProviderMetadata {
    ProviderMetadata::realtime("my-realtime", "My Realtime Provider")
        .with_description("Full-duplex audio-to-audio streaming")
        .with_features(["full-duplex", "function-calling", "turn-detection"])
}

fn create_my_realtime(config: RealtimeConfig) -> Result<Box<dyn BaseRealtime>, RealtimeError> {
    Ok(Box::new(MyRealtimeProvider::new(config)?))
}

inventory::submit! {
    PluginConstructor::realtime("my-realtime", my_realtime_metadata, create_my_realtime)
}
```

---

## Provider Metadata

The `ProviderMetadata` struct provides rich metadata for discovery, documentation, and validation.

### ProviderMetadata Structure

```rust
// From src/plugin/metadata.rs
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderMetadata {
    /// Provider identifier (e.g., "deepgram", "elevenlabs")
    pub name: String,

    /// Display name (e.g., "Deepgram Nova-3")
    pub display_name: String,

    /// Brief description
    pub description: String,

    /// Version string
    pub version: String,

    /// Required configuration keys (for validation)
    pub required_config_keys: Vec<String>,

    /// Optional configuration keys
    pub optional_config_keys: Vec<String>,

    /// Provider aliases (e.g., ["azure", "microsoft-azure"])
    pub aliases: Vec<String>,

    /// Supported languages (ISO 639-1 codes)
    pub supported_languages: Vec<String>,

    /// Supported models
    pub supported_models: Vec<String>,

    /// Provider features (e.g., "streaming", "word-timestamps")
    pub features: HashSet<String>,

    /// Provider type (stt, tts, realtime)
    pub provider_type: ProviderType,
}
```

### Builder Pattern Usage

```rust
// STT provider metadata
fn my_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("my-provider", "My Provider STT")
        .with_description("Real-time STT with advanced features")
        .with_alias("myprov")
        .with_aliases(["my_provider", "my-prov"])
        .with_features(["streaming", "word-timestamps", "speaker-diarization"])
        .with_languages(["en", "es", "fr", "de", "it", "pt"])
        .with_models(["base", "enhanced", "multilingual"])
        .with_required_config(["api_key"])
}

// TTS provider metadata
fn my_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("my-provider", "My Provider TTS")
        .with_description("Natural voice synthesis")
        .with_features(["streaming", "voice-cloning", "ssml"])
        .with_languages(["en", "es"])
}

// Realtime provider metadata
fn my_realtime_metadata() -> ProviderMetadata {
    ProviderMetadata::realtime("my-realtime", "My Realtime Provider")
        .with_description("Full-duplex audio streaming")
        .with_features(["full-duplex", "function-calling"])
}
```

---

## PHF Dispatch System

WaaV uses Perfect Hash Function (PHF) maps for O(1) guaranteed provider lookup with zero hash collisions.

### PHF Map Structure

```rust
// From src/plugin/dispatch.rs
use phf::phf_map;

/// PHF map for STT provider name resolution (including aliases)
pub static STT_PROVIDER_MAP: phf::Map<&'static str, BuiltinSTTProvider> = phf_map! {
    // Primary names
    "deepgram" => BuiltinSTTProvider::Deepgram,
    "google" => BuiltinSTTProvider::Google,
    "microsoft-azure" => BuiltinSTTProvider::Azure,
    // Aliases
    "azure" => BuiltinSTTProvider::Azure,
    "watson" => BuiltinSTTProvider::IbmWatson,
    "transcribe" => BuiltinSTTProvider::AwsTranscribe,
};
```

### SmallString for Case-Insensitive Lookup

PHF maps are case-sensitive, so we use `SmallString` for stack-allocated lowercase conversion:

```rust
// Stack-allocated small string (32 bytes inline, heap fallback)
pub struct SmallString {
    inline: [u8; 32],
    len: u8,
    heap: Option<String>,
}

impl SmallString {
    pub fn from_lowercase(s: &str) -> Self;
    pub fn as_str(&self) -> &str;
}

// Usage in lookup
pub fn resolve_stt_provider(name: &str) -> Option<BuiltinSTTProvider> {
    let lowercase = SmallString::from_lowercase(name);
    STT_PROVIDER_MAP.get(lowercase.as_str()).copied()
}
```

### Adding New Provider to PHF

1. Add to the enum:
```rust
#[repr(u8)]
pub enum BuiltinSTTProvider {
    // ... existing ...
    MyProvider = 11,
}
```

2. Add canonical name:
```rust
impl BuiltinSTTProvider {
    pub const fn canonical_name(&self) -> &'static str {
        match self {
            // ...
            Self::MyProvider => "my-provider",
        }
    }
}
```

3. Add to PHF map:
```rust
pub static STT_PROVIDER_MAP: phf::Map<&'static str, BuiltinSTTProvider> = phf_map! {
    // ... existing ...
    "my-provider" => BuiltinSTTProvider::MyProvider,
    "myprov" => BuiltinSTTProvider::MyProvider,  // alias
};
```

4. Update constants:
```rust
pub const BUILTIN_STT_COUNT: usize = 12;  // Increment

pub const BUILTIN_STT_NAMES: [&str; BUILTIN_STT_COUNT] = [
    // ... existing ...
    "my-provider",
];
```

---

## Plugin Lifecycle

Advanced plugins can participate in the gateway's lifecycle using the `PluginLifecycle` trait.

### Plugin State Machine

```
    ┌─────────────┐
    │ Discovered  │  (inventory::collect!)
    └──────┬──────┘
           │
           ▼
    ┌──────┴──────┐
    │ Registered  │  (dependencies checked)
    └──────┬──────┘
           │
           ▼
    ┌──────┴──────┐
    │ Initializing│  (init() called)
    └──────┬──────┘
           │
    ┌──────┴──────┐
    │             │
    ▼             ▼
┌───┴───┐    ┌────┴────┐
│ Ready │    │  Failed │
└───┬───┘    └─────────┘
    │
    ▼
┌───┴───┐
│ Running │  (start() called)
└───┬───┘
    │
    ▼
┌───┴───┐
│ Stopping │  (shutdown() called)
└───┬───┘
    │
    ▼
┌───┴───┐
│ Stopped │
└─────────┘
```

### PluginLifecycle Trait

```rust
// From src/plugin/lifecycle.rs
#[async_trait]
pub trait PluginLifecycle: Send + Sync {
    /// Returns the plugin manifest
    fn manifest(&self) -> &PluginManifest;

    /// Plugin initialization (parse config, validate settings)
    async fn init(&mut self, ctx: &PluginContext) -> Result<(), PluginError> {
        let _ = ctx;
        Ok(())
    }

    /// Plugin startup (connect to services, start background tasks)
    async fn start(&mut self, ctx: &PluginContext) -> Result<(), PluginError> {
        let _ = ctx;
        Ok(())
    }

    /// Plugin shutdown (close connections, flush buffers)
    async fn shutdown(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    /// Health check (called periodically)
    async fn health(&self) -> PluginHealth {
        PluginHealth::Healthy
    }

    /// Configuration schema (JSON Schema for validation)
    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
```

### PluginEntry Metrics

```rust
pub struct PluginEntry {
    pub state: PluginState,
    pub loaded_at: Instant,
    pub last_active: Instant,
    pub call_count: u64,
    pub error_count: u64,
    pub last_error: Option<String>,
}

impl PluginEntry {
    pub fn record_success(&mut self);
    pub fn record_error(&mut self, error: impl Into<String>);
    pub fn uptime(&self) -> Duration;
    pub fn idle_time(&self) -> Duration;
}
```

---

## Plugin Isolation

WaaV catches panics in plugin code to prevent provider failures from crashing the gateway.

### Panic Isolation Functions

```rust
// From src/plugin/isolation.rs

/// Safely call a plugin function with panic catching
pub fn call_plugin_safely<F, T, E>(plugin_fn: F) -> Result<T, PluginError>
where
    F: FnOnce() -> Result<T, E> + UnwindSafe,
    E: std::fmt::Display,
{
    match catch_unwind(plugin_fn) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err(PluginError::InternalError(e.to_string())),
        Err(panic_info) => {
            let msg = extract_panic_message(&panic_info);
            tracing::error!(message = %msg, "Plugin panicked");
            Err(PluginError::Panic(msg))
        }
    }
}

/// Preserve original error type (used by registry)
pub fn call_plugin_preserving_error<F, T, E, PC>(
    plugin_fn: F,
    panic_to_error: PC
) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E> + UnwindSafe,
    PC: FnOnce(String) -> E,
{
    match catch_unwind(plugin_fn) {
        Ok(result) => result,
        Err(panic_info) => {
            let msg = extract_panic_message(&panic_info);
            tracing::error!(message = %msg, "Plugin panicked");
            Err(panic_to_error(msg))
        }
    }
}
```

### Usage in Registry

The registry automatically wraps factory calls with panic isolation:

```rust
// In PluginRegistry::create_stt
let result = call_plugin_preserving_error(
    std::panic::AssertUnwindSafe(|| factory(config)),
    |panic_msg| STTError::ProviderError(format!("Plugin panicked: {}", panic_msg)),
);
```

---

## WebSocket Handler Plugins

Custom WebSocket message handlers can be registered for extending the gateway's WebSocket API.

### WSHandlerCapability Trait

```rust
// From src/plugin/capabilities.rs
pub trait WSHandlerCapability: PluginCapability {
    /// Message type this handler handles (e.g., "custom_command")
    fn message_type(&self) -> &'static str;

    /// Handle the message
    fn handle<'a>(
        &'a self,
        msg: Value,
        ctx: &'a WSContext,
    ) -> Pin<Box<dyn Future<Output = Result<Option<WSResponse>, WSError>> + Send + 'a>>;
}

pub struct WSContext {
    pub stream_id: String,
    pub authenticated: bool,
    pub tenant_id: Option<String>,
}

pub enum WSResponse {
    Json(Value),
    Binary(Bytes),
    Multiple(Vec<WSResponse>),
    None,
}
```

### Register WS Handler at Runtime

```rust
use crate::plugin::global_registry;

async fn handle_custom_message(
    payload: serde_json::Value,
    ctx: WSContext,
) -> Result<Option<WSResponse>, WSError> {
    let action = payload["action"].as_str().unwrap_or("");

    match action {
        "ping" => Ok(Some(WSResponse::Json(serde_json::json!({"pong": true})))),
        _ => Ok(None),
    }
}

// Register during initialization
global_registry().register_ws_handler(
    "custom_message",
    Arc::new(|payload, ctx| Box::pin(handle_custom_message(payload, ctx))),
);
```

### Using the Macro

```rust
// Use the convenience macro
register_ws_handler!("my_message", handle_my_message);
```

---

## Testing

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let config = STTConfig::default();
        let provider = MyProviderSTT::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_provider_requires_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };
        let result = MyProviderSTT::new(config);
        assert!(matches!(result, Err(STTError::ConfigurationError(_))));
    }

    #[tokio::test]
    async fn test_provider_connection() {
        let config = STTConfig {
            api_key: "test-key".to_string(),
            ..Default::default()
        };
        let mut provider = MyProviderSTT::new(config).unwrap();

        // Note: This will fail without a real API key
        // Use mock servers for integration testing
        assert!(!provider.is_connected());
    }
}
```

### Registry Integration Tests

```rust
#[test]
fn test_provider_registered() {
    use crate::plugin::global_registry;

    let registry = global_registry();

    // Verify provider is registered
    assert!(registry.has_stt_provider("my-provider"));

    // Verify aliases work
    assert!(registry.has_stt_provider("myprov"));
    assert!(registry.has_stt_provider("MYPROV"));  // case-insensitive

    // Verify metadata
    let metadata = registry.get_stt_metadata("my-provider").unwrap();
    assert_eq!(metadata.name, "my-provider");
    assert!(metadata.features.contains("streaming"));
}

#[test]
fn test_phf_alias_resolution() {
    use crate::plugin::dispatch::resolve_stt_provider;

    // Primary name
    assert!(resolve_stt_provider("my-provider").is_some());

    // Aliases
    assert!(resolve_stt_provider("myprov").is_some());
    assert!(resolve_stt_provider("my_provider").is_some());

    // Case insensitivity
    assert!(resolve_stt_provider("MY-PROVIDER").is_some());
    assert!(resolve_stt_provider("MyProvider").is_some());
}
```

### Integration Tests with Mock Servers

See `tests/mock_providers/` for comprehensive mock server implementations:

```rust
// tests/mock_providers/mod.rs provides:
// - HTTP mock servers (ElevenLabs, OpenAI, PlayHT)
// - WebSocket mock servers (Deepgram, Cartesia, LMNT)
// - gRPC mock servers (Google)
// - Chaos testing (failures, timeouts, rate limits)
```

---

## Quick Reference

### Files to Create for New Provider

```
src/core/{stt,tts,realtime}/my_provider/
├── mod.rs       # Module exports, documentation
├── config.rs    # Provider-specific configuration
└── provider.rs  # Provider implementation
```

### Files to Modify

| File | Changes |
|------|---------|
| `src/core/{stt,tts}/mod.rs` | Add module and re-exports |
| `src/plugin/builtin/mod.rs` | Add metadata, factory, inventory::submit! |
| `src/plugin/dispatch.rs` | Add to PHF map and enum |

### Registration Checklist

- [ ] Implement base trait (`BaseSTT`, `BaseTTS`, or `BaseRealtime`)
- [ ] Create metadata function returning `ProviderMetadata`
- [ ] Create factory function returning `Box<dyn Base*>`
- [ ] Add `inventory::submit!` with `PluginConstructor`
- [ ] Add to PHF map in `dispatch.rs`
- [ ] Update constants (`BUILTIN_*_COUNT`, `BUILTIN_*_NAMES`)
- [ ] Export from module's `mod.rs`
- [ ] Add unit tests
- [ ] Add integration tests

### Convenience Macros

```rust
// Instead of manual inventory::submit!
register_stt_plugin!("my-stt", my_stt_metadata, create_my_stt);
register_stt_plugin!("my-stt", my_stt_metadata, create_my_stt, aliases: ["mystt"]);

register_tts_plugin!("my-tts", my_tts_metadata, create_my_tts);
register_tts_plugin!("my-tts", my_tts_metadata, create_my_tts, aliases: ["mytts"]);

register_realtime_plugin!("my-rt", my_rt_metadata, create_my_rt);
```

---

## Configuration

### STTConfig Structure

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct STTConfig {
    /// API key for authentication
    pub api_key: String,

    /// Model identifier
    pub model: String,

    /// Language code (e.g., "en-US")
    pub language: Option<String>,

    /// Sample rate in Hz (default: 16000)
    pub sample_rate: Option<u32>,

    /// Audio encoding (e.g., "linear16", "opus")
    pub encoding: Option<String>,

    /// Enable interim results
    pub interim_results: Option<bool>,

    /// Enable word timestamps
    pub word_timestamps: Option<bool>,

    /// Provider-specific options
    pub provider_options: HashMap<String, String>,
}
```

### TTSConfig Structure

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TTSConfig {
    /// Provider identifier
    pub provider: String,

    /// API key for authentication
    pub api_key: String,

    /// Voice identifier
    pub voice_id: Option<String>,

    /// Model identifier
    pub model: String,

    /// Speaking rate multiplier
    pub speaking_rate: Option<f32>,

    /// Audio format (e.g., "linear16", "mp3")
    pub audio_format: Option<String>,

    /// Sample rate in Hz
    pub sample_rate: Option<u32>,

    /// Connection timeout in seconds
    pub connection_timeout: Option<u64>,

    /// Request timeout in seconds
    pub request_timeout: Option<u64>,

    /// Pronunciation replacements
    pub pronunciations: Vec<Pronunciation>,

    /// Connection pool size
    pub request_pool_size: Option<usize>,

    /// Emotion configuration
    pub emotion_config: Option<EmotionConfig>,
}
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `MYPROVIDER_API_KEY` | API key for MyProvider |
| `MYPROVIDER_MODEL` | Default model to use |
| `MYPROVIDER_REGION` | API region (if applicable) |

---

## Security Checklist

- [ ] **Never log API keys** - Use `tracing::field::debug` for redaction
- [ ] **Validate all inputs** - Check audio format, sample rate, text length
- [ ] **Use HTTPS/WSS only** - Never connect to unencrypted endpoints
- [ ] **Implement timeouts** - Connection, read, and write timeouts
- [ ] **Rate limit awareness** - Handle 429 responses gracefully
- [ ] **Credential rotation** - Support runtime credential updates
- [ ] **Audit logging** - Log provider calls (without sensitive data)

```rust
// Example: Secure logging
tracing::info!(
    provider = "my-provider",
    api_key = tracing::field::Empty,  // Never log
    model = %config.model,
    "Connecting to provider"
);
```

---

## Performance Guidelines

### Connection Pooling

- Use `ReqManager` for HTTP providers (shared connection pool)
- Reuse WebSocket connections when possible
- Implement connection health checks

### Memory Management

- Avoid allocations in audio hot paths
- Use pre-allocated buffers for audio processing
- Stream large audio files instead of loading into memory

### Latency Optimization

- Measure and log latency metrics
- Use connection keep-alive
- Implement request pipelining where supported

### Benchmarks

```rust
#[cfg(test)]
mod benchmarks {
    use super::*;
    use criterion::{black_box, criterion_group, criterion_main, Criterion};

    fn benchmark_provider_creation(c: &mut Criterion) {
        c.bench_function("MyProviderSTT::new", |b| {
            b.iter(|| {
                let config = STTConfig::default();
                black_box(MyProviderSTT::new(config))
            });
        });
    }

    criterion_group!(benches, benchmark_provider_creation);
    criterion_main!(benches);
}
```

---

## See Also

- [CLAUDE.md](../CLAUDE.md) - Project overview and architecture
- [API Documentation](./api.md) - REST and WebSocket API reference
- [Configuration Guide](./configuration.md) - YAML and environment configuration
- [Testing Guide](./testing.md) - Comprehensive testing strategies
- Source files:
  - `src/plugin/registry.rs` - Plugin registry implementation
  - `src/plugin/capabilities.rs` - Capability traits
  - `src/plugin/dispatch.rs` - PHF lookup maps
  - `src/plugin/builtin/mod.rs` - Built-in provider registrations

# WaaV Gateway Plugin Architecture

This document provides a comprehensive guide to the WaaV Gateway plugin system, including architecture details, plugin types, and tutorials for creating custom plugins.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Plugin Types](#plugin-types)
- [Built-in Providers](#built-in-providers)
- [Creating Custom Plugins](#creating-custom-plugins)
- [Plugin Registration](#plugin-registration)
- [Configuration](#configuration)
- [Best Practices](#best-practices)
- [API Reference](#api-reference)

---

## Overview

The WaaV Gateway plugin system provides a **capability-based architecture** that enables:

- **Dynamic Provider Registration**: Add STT, TTS, and Realtime providers at compile-time
- **Audio Processor Plugins**: VAD, noise filtering, resampling pipelines
- **Middleware Plugins**: Custom authentication, rate limiting, logging
- **WebSocket Handler Plugins**: Extend WebSocket message handling
- **Full Backward Compatibility**: Existing APIs remain unchanged

### Key Features

| Feature | Description |
|---------|-------------|
| **O(1) Lookup** | PHF perfect hash maps for built-in provider resolution |
| **Panic Isolation** | `catch_unwind` prevents plugin panics from crashing the gateway |
| **Lifecycle Management** | Plugins go through defined states (Discovered → Running → Stopped) |
| **Concurrent Access** | DashMap for thread-safe runtime registration |
| **Compile-time Registration** | `inventory` crate for zero-overhead plugin discovery |

---

## Architecture

### High-Level Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        PLUGIN REGISTRATION FLOW                              │
│                                                                              │
│   ┌──────────────────┐    ┌──────────────────┐    ┌────────────────────┐   │
│   │  inventory crate │───▶│  PHF Static Map  │───▶│  DashMap Runtime   │   │
│   │  (compile-time)  │    │  (O(1) lookup)   │    │  Registry          │   │
│   └──────────────────┘    └──────────────────┘    └────────────────────┘   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                 ┌────────────────────┼────────────────────┐
                 ▼                    ▼                    ▼
       ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
       │ Provider Plugins│  │ Audio Processor │  │   Middleware    │
       │ (STT/TTS/RT)    │  │ Plugins         │  │   Plugins       │
       └────────┬────────┘  └────────┬────────┘  └────────┬────────┘
                │                    │                    │
                ▼                    ▼                    ▼
       ┌─────────────────────────────────────────────────────────────┐
       │                    Capability Index                          │
       │   TypeId::of::<dyn STTCapability>() → [provider_ids...]     │
       │   TypeId::of::<dyn TTSCapability>() → [provider_ids...]     │
       │   TypeId::of::<dyn RealtimeCapability>() → [provider_ids...]│
       └─────────────────────────────────────────────────────────────┘
```

### Module Structure

```
src/plugin/
├── mod.rs              # Module exports and prelude
├── registry.rs         # PluginRegistry with inventory integration
├── capabilities.rs     # Capability traits (STT, TTS, Realtime, etc.)
├── metadata.rs         # PluginManifest, ProviderMetadata
├── lifecycle.rs        # Plugin lifecycle state machine
├── isolation.rs        # catch_unwind wrappers for panic safety
├── dispatch.rs         # PHF static maps for O(1) provider lookup
├── macros.rs           # Helper macros for plugin registration
└── builtin/
    └── mod.rs          # Built-in provider registrations (25 providers)
```

### Plugin Lifecycle State Machine

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
│Running│  (start() called)
└───┬───┘
    │
    ▼
┌───┴────┐
│Stopping│  (shutdown() called)
└───┬────┘
    │
    ▼
┌───┴───┐
│Stopped│
└───────┘
```

---

## Plugin Types

The gateway supports seven capability types, each with specific use cases:

### 1. STT Capability (Speech-to-Text)

Implement `STTCapability` to add speech recognition providers.

```rust
pub trait STTCapability: PluginCapability {
    /// Provider identifier (e.g., "deepgram", "google")
    fn provider_id(&self) -> &'static str;

    /// Create an STT provider instance
    fn create_stt(&self, config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError>;

    /// Provider metadata for discovery
    fn metadata(&self) -> ProviderMetadata;
}
```

**Use Cases**: Cloud STT services, on-device speech recognition, specialized language models.

### 2. TTS Capability (Text-to-Speech)

Implement `TTSCapability` to add speech synthesis providers.

```rust
pub trait TTSCapability: PluginCapability {
    fn provider_id(&self) -> &'static str;
    fn create_tts(&self, config: TTSConfig) -> TTSResult<Box<dyn BaseTTS>>;
    fn metadata(&self) -> ProviderMetadata;
}
```

**Use Cases**: Voice cloning services, neural TTS, SSML-based synthesis.

### 3. Realtime Capability (Audio-to-Audio)

Implement `RealtimeCapability` for bidirectional audio streaming.

```rust
pub trait RealtimeCapability: PluginCapability {
    fn provider_id(&self) -> &'static str;
    fn create_realtime(&self, config: RealtimeConfig) -> RealtimeResult<Box<dyn BaseRealtime>>;
    fn metadata(&self) -> ProviderMetadata;
}
```

**Use Cases**: Voice assistants, real-time translation, empathic voice interfaces.

### 4. Audio Processor Capability

Implement `AudioProcessorCapability` for audio pipeline processing.

```rust
#[async_trait]
pub trait AudioProcessorCapability: PluginCapability {
    fn processor_id(&self) -> &'static str;
    fn create_processor(&self, config: Value) -> Result<Box<dyn AudioProcessor>, AudioProcessorError>;
    fn metadata(&self) -> ProcessorMetadata;
}

#[async_trait]
pub trait AudioProcessor: Send + Sync {
    async fn process(&self, audio: Bytes, format: &AudioFormat) -> Result<Bytes, AudioProcessorError>;
    fn changes_duration(&self) -> bool { false }
    fn latency_ms(&self) -> u32 { 0 }
}
```

**Use Cases**: VAD (Voice Activity Detection), noise reduction, echo cancellation, resampling.

### 5. Middleware Capability

Implement `MiddlewareCapability` to add Axum middleware layers.

```rust
pub trait MiddlewareCapability: PluginCapability {
    fn middleware_id(&self) -> &'static str;
    fn priority(&self) -> i32;  // Lower = earlier in chain
    fn create_middleware(&self, config: Value) -> Result<MiddlewareLayer, MiddlewareError>;
    fn metadata(&self) -> MiddlewareMetadata;
}
```

**Use Cases**: Custom authentication, request logging, metrics collection, rate limiting.

### 6. WebSocket Handler Capability

Implement `WSHandlerCapability` to handle custom WebSocket message types.

```rust
pub trait WSHandlerCapability: PluginCapability {
    fn message_type(&self) -> &'static str;
    fn handle<'a>(
        &'a self,
        msg: Value,
        ctx: &'a WSContext,
    ) -> Pin<Box<dyn Future<Output = Result<Option<WSResponse>, WSError>> + Send + 'a>>;
}
```

**Use Cases**: Custom commands, plugin-specific protocols, real-time data streaming.

### 7. Auth Capability

Implement `AuthCapability` to add authentication strategies.

```rust
pub trait AuthCapability: PluginCapability {
    fn strategy_id(&self) -> &'static str;
    fn create_strategy(&self, config: Value) -> Result<Box<dyn AuthStrategy>, AuthStrategyError>;
    fn metadata(&self) -> AuthMetadata;
}

#[async_trait]
pub trait AuthStrategy: Send + Sync {
    async fn authenticate(&self, credentials: &AuthCredentials) -> Result<AuthIdentity, AuthStrategyError>;
    fn extract_credentials(&self, headers: &http::HeaderMap) -> Option<AuthCredentials>;
}
```

**Use Cases**: OAuth2, SAML, API keys, custom token validation.

---

## Built-in Providers

WaaV Gateway ships with **25 built-in providers** registered via the plugin system:

### STT Providers (11)

| Provider | ID | Aliases | Features |
|----------|-----|---------|----------|
| Deepgram | `deepgram` | - | streaming, word-timestamps, speaker-diarization |
| Google Cloud | `google` | - | streaming, word-timestamps, speaker-diarization |
| ElevenLabs | `elevenlabs` | - | streaming, word-timestamps |
| Microsoft Azure | `microsoft-azure` | `azure` | streaming, word-timestamps, punctuation |
| Cartesia | `cartesia` | - | streaming, low-latency |
| OpenAI Whisper | `openai` | - | word-timestamps, translation |
| AssemblyAI | `assemblyai` | - | streaming, speaker-diarization, sentiment-analysis |
| AWS Transcribe | `aws-transcribe` | `transcribe`, `amazon-transcribe` | streaming, word-timestamps |
| IBM Watson | `ibm-watson` | `watson`, `ibm` | streaming, speaker-diarization |
| Groq | `groq` | - | fast-inference (216x real-time), translation |
| Gnani | `gnani` | `gnani-ai`, `gnani.ai`, `vachana` | indic-languages (14), interim-results |

### TTS Providers (12)

| Provider | ID | Aliases | Features |
|----------|-----|---------|----------|
| Deepgram Aura | `deepgram` | - | streaming, websocket |
| ElevenLabs | `elevenlabs` | - | streaming, voice-cloning, emotion-control |
| Google Cloud | `google` | - | ssml, neural-voices |
| Microsoft Azure | `microsoft-azure` | `azure` | streaming, ssml, neural-voices |
| Cartesia Sonic | `cartesia` | - | streaming, low-latency, voice-cloning |
| OpenAI | `openai` | - | streaming |
| AWS Polly | `aws-polly` | `polly`, `amazon-polly` | ssml, neural-voices |
| IBM Watson | `ibm-watson` | `watson`, `ibm` | streaming, ssml |
| Hume AI Octave | `hume` | `hume-ai` | streaming, emotion-control |
| LMNT | `lmnt` | `lmnt-ai` | streaming, low-latency (~150ms) |
| Play.ht | `playht` | `play.ht`, `play-ht` | streaming, voice-cloning |
| Gnani | `gnani` | `gnani-ai`, `gnani.ai` | multi-speaker, ssml-gender, indic-languages (12) |

### Realtime Providers (2)

| Provider | ID | Aliases | Features |
|----------|-----|---------|----------|
| OpenAI Realtime | `openai` | - | full-duplex, function-calling, turn-detection |
| Hume EVI | `hume` | `evi`, `hume-evi` | full-duplex, emotion-analysis, prosody-scores |

---

## Creating Custom Plugins

### Tutorial 1: Simple STT Provider

This example shows how to create a basic STT provider plugin.

```rust
// src/my_stt_plugin.rs
use waav_gateway::plugin::prelude::*;

/// My custom STT provider
pub struct MySTT {
    config: STTConfig,
    is_connected: bool,
}

impl MySTT {
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        Ok(Self {
            config,
            is_connected: false,
        })
    }
}

#[async_trait]
impl BaseSTT for MySTT {
    async fn connect(&mut self) -> Result<(), STTError> {
        // Connect to your STT service
        self.is_connected = true;
        Ok(())
    }

    async fn send_audio(&mut self, audio_data: bytes::Bytes) -> Result<(), STTError> {
        if !self.is_connected {
            return Err(STTError::NotConnected);
        }
        // Process audio and emit transcripts via callbacks
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        self.is_connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.is_connected
    }

    fn provider_info(&self) -> &'static str {
        "my-stt"
    }
}

// Metadata function (called at registration time)
fn my_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("my-stt", "My Custom STT Provider")
        .with_description("A custom speech-to-text provider")
        .with_features(["streaming", "word-timestamps"])
        .with_languages(["en", "es", "fr"])
}

// Factory function
fn create_my_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(MySTT::new(config)?))
}

// Register the plugin at compile time
inventory::submit! {
    PluginConstructor::stt("my-stt", my_stt_metadata, create_my_stt)
        .with_aliases(&["my-stt-alias", "custom-stt"])
}
```

### Tutorial 2: TTS Provider with Voice Cloning

```rust
use waav_gateway::plugin::prelude::*;
use waav_gateway::core::tts::{AudioCallback, AudioData};

pub struct MyCloneTTS {
    config: TTSConfig,
    audio_callback: Option<Arc<dyn AudioCallback>>,
}

impl MyCloneTTS {
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        Ok(Self {
            config,
            audio_callback: None,
        })
    }
}

#[async_trait]
impl BaseTTS for MyCloneTTS {
    async fn connect(&mut self) -> TTSResult<()> {
        // Initialize connection to TTS service
        Ok(())
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        // Generate speech from text
        let audio_data = self.synthesize(text).await?;

        // Emit audio via callback
        if let Some(callback) = &self.audio_callback {
            callback.on_audio(audio_data).await;
            if flush {
                callback.on_complete().await;
            }
        }
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.audio_callback = Some(callback);
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        Ok(())
    }

    fn provider_info(&self) -> &'static str {
        "my-clone-tts"
    }
}

fn my_clone_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("my-clone-tts", "My Voice Cloning TTS")
        .with_description("Custom TTS with voice cloning support")
        .with_features(["streaming", "voice-cloning", "emotion-control"])
        .with_alias("clone-tts")
}

fn create_my_clone_tts(config: TTSConfig) -> TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(MyCloneTTS::new(config)?))
}

inventory::submit! {
    PluginConstructor::tts("my-clone-tts", my_clone_tts_metadata, create_my_clone_tts)
}
```

### Tutorial 3: Audio Processor Plugin

```rust
use waav_gateway::plugin::prelude::*;
use waav_gateway::plugin::capabilities::{
    AudioProcessor, AudioProcessorCapability, AudioProcessorError, AudioFormat, ProcessorMetadata
};

/// Noise gate processor that silences audio below a threshold
pub struct NoiseGateProcessor {
    threshold: f32,
}

#[async_trait]
impl AudioProcessor for NoiseGateProcessor {
    async fn process(&self, audio: Bytes, format: &AudioFormat) -> Result<Bytes, AudioProcessorError> {
        // Convert to samples, apply noise gate, return processed audio
        let samples: Vec<i16> = audio.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();

        let processed: Vec<i16> = samples.iter()
            .map(|&s| {
                let amplitude = (s as f32).abs() / 32768.0;
                if amplitude < self.threshold { 0 } else { s }
            })
            .collect();

        let bytes: Vec<u8> = processed.iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();

        Ok(Bytes::from(bytes))
    }

    fn latency_ms(&self) -> u32 {
        0  // Zero additional latency
    }
}

pub struct NoiseGatePlugin;

impl PluginCapability for NoiseGatePlugin {}

impl AudioProcessorCapability for NoiseGatePlugin {
    fn processor_id(&self) -> &'static str {
        "noise-gate"
    }

    fn create_processor(&self, config: Value) -> Result<Box<dyn AudioProcessor>, AudioProcessorError> {
        let threshold = config.get("threshold")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.01) as f32;

        Ok(Box::new(NoiseGateProcessor { threshold }))
    }

    fn metadata(&self) -> ProcessorMetadata {
        ProcessorMetadata {
            id: "noise-gate".to_string(),
            name: "Noise Gate".to_string(),
            description: "Silences audio below a threshold".to_string(),
            ..Default::default()
        }
    }
}
```

### Tutorial 4: WebSocket Handler Plugin

```rust
use waav_gateway::plugin::prelude::*;
use waav_gateway::plugin::capabilities::{
    WSHandlerCapability, WSContext, WSResponse, WSError
};
use std::future::Future;
use std::pin::Pin;

/// Custom ping/pong handler
pub struct PingPongHandler;

impl PluginCapability for PingPongHandler {}

impl WSHandlerCapability for PingPongHandler {
    fn message_type(&self) -> &'static str {
        "ping"
    }

    fn handle<'a>(
        &'a self,
        msg: Value,
        ctx: &'a WSContext,
    ) -> Pin<Box<dyn Future<Output = Result<Option<WSResponse>, WSError>> + Send + 'a>> {
        Box::pin(async move {
            // Extract payload from ping message
            let payload = msg.get("payload")
                .and_then(|v| v.as_str())
                .unwrap_or("pong");

            // Return pong response
            Ok(Some(WSResponse::Json(serde_json::json!({
                "type": "pong",
                "payload": payload,
                "stream_id": ctx.stream_id,
            }))))
        })
    }
}
```

---

## Plugin Registration

### Compile-time Registration (Recommended)

Use the `inventory` crate for automatic plugin discovery at compile time:

```rust
// Register an STT provider
inventory::submit! {
    PluginConstructor::stt("provider-id", metadata_fn, factory_fn)
        .with_aliases(&["alias1", "alias2"])
}

// Register a TTS provider
inventory::submit! {
    PluginConstructor::tts("provider-id", metadata_fn, factory_fn)
}

// Register a Realtime provider
inventory::submit! {
    PluginConstructor::realtime("provider-id", metadata_fn, factory_fn)
}
```

### Runtime Registration

For dynamic plugin loading or testing, use the registry directly:

```rust
use waav_gateway::plugin::global_registry;

let registry = global_registry();

// Register an STT factory at runtime
registry.register_stt(
    "runtime-stt",
    Arc::new(|config| Ok(Box::new(MySTT::new(config)?))),
    ProviderMetadata::stt("runtime-stt", "Runtime STT Provider"),
);
```

### Using Registered Providers

```rust
use waav_gateway::plugin::global_registry;

let registry = global_registry();

// Create an STT provider
let stt = registry.create_stt("deepgram", config)?;

// Create a TTS provider (with alias)
let tts = registry.create_tts("polly", config)?;  // Resolves to aws-polly

// Check if a provider exists
if registry.has_stt_provider("my-custom-stt") {
    // Provider is available
}

// List all registered providers
let stt_providers = registry.get_stt_provider_names();
let tts_providers = registry.get_tts_provider_names();
```

---

## Configuration

### Plugin Configuration in ServerConfig

```rust
pub struct ServerConfig {
    // ... other fields ...

    /// Plugin configuration (optional)
    #[serde(default)]
    pub plugins: PluginConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PluginConfig {
    /// Enable plugin system
    #[serde(default)]
    pub enabled: bool,

    /// Directory for external plugins (future)
    #[serde(default)]
    pub plugin_dir: Option<PathBuf>,

    /// Provider-specific configuration
    #[serde(default)]
    pub provider_config: HashMap<String, Value>,
}
```

### YAML Configuration Example

```yaml
plugins:
  enabled: true
  provider_config:
    my-custom-stt:
      endpoint: "https://api.example.com/stt"
      timeout_ms: 5000
    noise-gate:
      threshold: 0.02
```

### Environment Variables

Provider-specific credentials are loaded from environment variables:

```bash
# STT Providers
DEEPGRAM_API_KEY=your-key
GOOGLE_APPLICATION_CREDENTIALS=/path/to/credentials.json
AZURE_SPEECH_SUBSCRIPTION_KEY=your-key
AZURE_SPEECH_REGION=eastus
OPENAI_API_KEY=your-key
ASSEMBLYAI_API_KEY=your-key
CARTESIA_API_KEY=your-key
AWS_ACCESS_KEY_ID=your-key
AWS_SECRET_ACCESS_KEY=your-secret
IBM_WATSON_API_KEY=your-key
IBM_WATSON_INSTANCE_ID=your-instance
GROQ_API_KEY=your-key
GNANI_TOKEN=your-token
GNANI_ACCESS_KEY=your-access-key
GNANI_CERTIFICATE_PATH=/path/to/cert.pem

# TTS Providers (additional)
ELEVENLABS_API_KEY=your-key
HUME_API_KEY=your-key
LMNT_API_KEY=your-key
PLAYHT_API_KEY=your-key
PLAYHT_USER_ID=your-user-id
```

---

## Best Practices

### 1. Error Handling

Always return descriptive errors from factory functions:

```rust
fn create_my_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    if config.api_key.is_empty() {
        return Err(STTError::ConfigurationError(
            "API key is required for MySTT provider".to_string()
        ));
    }
    Ok(Box::new(MySTT::new(config)?))
}
```

### 2. Panic Safety

The plugin system wraps factory calls in `catch_unwind`, but avoid panicking:

```rust
// BAD: Panics on error
fn create_provider(config: Config) -> Result<Box<dyn Provider>, Error> {
    let api_key = config.api_key.unwrap();  // Panics if None!
    // ...
}

// GOOD: Returns error
fn create_provider(config: Config) -> Result<Box<dyn Provider>, Error> {
    let api_key = config.api_key
        .ok_or_else(|| Error::ConfigurationError("API key required".into()))?;
    // ...
}
```

### 3. Metadata Completeness

Provide complete metadata for discoverability:

```rust
fn my_provider_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("my-provider", "My Provider Display Name")
        .with_description("Detailed description of what this provider does")
        .with_features(["streaming", "word-timestamps", "speaker-diarization"])
        .with_languages(["en-US", "es-ES", "fr-FR"])
        .with_models(["model-v1", "model-v2-turbo"])
        .with_aliases(&["my-alias", "another-alias"])
}
```

### 4. Async Best Practices

Use async/await properly in provider implementations:

```rust
#[async_trait]
impl BaseSTT for MySTT {
    async fn connect(&mut self) -> Result<(), STTError> {
        // Use tokio's async HTTP client
        let response = self.client.get(&self.endpoint).await?;

        // Don't block the async runtime
        // BAD: std::thread::sleep(Duration::from_secs(1));
        // GOOD:
        tokio::time::sleep(Duration::from_secs(1)).await;

        Ok(())
    }
}
```

### 5. Resource Cleanup

Implement proper cleanup in `disconnect`:

```rust
async fn disconnect(&mut self) -> Result<(), STTError> {
    // Close WebSocket connections
    if let Some(ws) = self.websocket.take() {
        let _ = ws.close(None).await;
    }

    // Cancel background tasks
    if let Some(handle) = self.task_handle.take() {
        handle.abort();
    }

    self.is_connected = false;
    Ok(())
}
```

---

## API Reference

### PluginRegistry

```rust
impl PluginRegistry {
    /// Create a new empty registry
    pub fn new() -> Self;

    /// Register an STT provider factory
    pub fn register_stt(&self, provider_id: &str, factory: STTFactoryFn, metadata: ProviderMetadata);

    /// Register a TTS provider factory
    pub fn register_tts(&self, provider_id: &str, factory: TTSFactoryFn, metadata: ProviderMetadata);

    /// Register a Realtime provider factory
    pub fn register_realtime(&self, provider_id: &str, factory: RealtimeFactoryFn, metadata: ProviderMetadata);

    /// Create an STT provider by name (O(1) lookup)
    pub fn create_stt(&self, provider: &str, config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError>;

    /// Create a TTS provider by name (O(1) lookup)
    pub fn create_tts(&self, provider: &str, config: TTSConfig) -> TTSResult<Box<dyn BaseTTS>>;

    /// Create a Realtime provider by name (O(1) lookup)
    pub fn create_realtime(&self, provider: &str, config: RealtimeConfig) -> RealtimeResult<Box<dyn BaseRealtime>>;

    /// Check if an STT provider is registered
    pub fn has_stt_provider(&self, provider: &str) -> bool;

    /// Get all registered STT provider names
    pub fn get_stt_provider_names(&self) -> Vec<String>;

    /// Get provider metadata
    pub fn get_stt_metadata(&self, provider: &str) -> Option<ProviderMetadata>;
}
```

### ProviderMetadata Builder

```rust
impl ProviderMetadata {
    pub fn stt(name: &str, display_name: &str) -> Self;
    pub fn tts(name: &str, display_name: &str) -> Self;
    pub fn realtime(name: &str, display_name: &str) -> Self;

    pub fn with_description(self, desc: &str) -> Self;
    pub fn with_alias(self, alias: &str) -> Self;
    pub fn with_aliases(self, aliases: impl IntoIterator<Item = impl Into<String>>) -> Self;
    pub fn with_features(self, features: impl IntoIterator<Item = impl Into<String>>) -> Self;
    pub fn with_languages(self, languages: impl IntoIterator<Item = impl Into<String>>) -> Self;
    pub fn with_models(self, models: impl IntoIterator<Item = impl Into<String>>) -> Self;
}
```

### Global Registry Access

```rust
use waav_gateway::plugin::global_registry;

// Get the global registry (lazily initialized)
let registry = global_registry();

// The registry is automatically populated with all
// plugins registered via inventory::submit!
```

---

## Performance Characteristics

| Operation | Time Complexity | Notes |
|-----------|-----------------|-------|
| Built-in provider lookup | O(1) | PHF perfect hash |
| Alias resolution | O(1) | PHF includes all aliases |
| Case-insensitive lookup | O(n) where n = name length | Stack-allocated lowercase |
| Runtime provider lookup | O(1) amortized | DashMap |
| Provider registration | O(1) amortized | DashMap insert |
| Provider creation | O(1) + factory time | Factory dominates |

---

## Troubleshooting

### Provider Not Found

```
Error: Unknown STT provider: 'my-provider'. Available providers: [...]
```

**Solutions**:
1. Ensure the plugin is linked into the binary
2. Check that `inventory::submit!` is called
3. Verify the provider ID matches exactly (case-insensitive)

### Plugin Panic

```
Error: Plugin panicked: index out of bounds
```

**Solutions**:
1. Check for None/empty values before accessing
2. Validate configuration before using
3. Use proper error handling instead of unwrap/expect

### Configuration Error

```
Error: API key is required for provider
```

**Solutions**:
1. Set the appropriate environment variable
2. Check YAML configuration for typos
3. Ensure credentials are valid

---

## Future Roadmap

- **Dynamic Loading**: Load plugins from shared libraries at runtime
- **WASM Sandboxing**: Run untrusted plugins in WebAssembly sandbox
- **Hot Reload**: Update plugins without gateway restart
- **Plugin Marketplace**: Discover and install community plugins

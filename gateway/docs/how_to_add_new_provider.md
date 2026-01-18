# How to Add a New Provider to WaaV Gateway

> **Status**: VERIFIED - Successfully tested with Sarvam.ai STT implementation (12 providers total)

This guide provides a complete, verified methodology for integrating new STT, TTS, and Realtime providers into the WaaV gateway. It includes real code examples from the Sarvam.ai integration as a running case study.

## Table of Contents

1. [Quick Start](#quick-start)
2. [Architecture Overview](#architecture-overview)
3. [Provider Types](#provider-types)
4. [Step-by-Step Integration](#step-by-step-integration)
5. [Running Example: Sarvam.ai](#running-example-sarvam-ai)
6. [Testing Your Provider](#testing-your-provider)
7. [Best Practices](#best-practices)
8. [Common Mistakes](#common-mistakes)
9. [Troubleshooting](#troubleshooting)

---

## Quick Start

Adding a new provider requires these steps:

1. **Create provider files** in `src/core/stt/<provider>/` or `src/core/tts/<provider>/`
2. **Implement the base trait** (`BaseSTT` or `BaseTTS`)
3. **Register with plugin system** in `src/plugin/builtin/mod.rs`
4. **Add to PHF dispatch** in `src/plugin/dispatch.rs`
5. **Add environment variable** support in `src/config/`
6. **Write tests** and verify integration

**Time estimate**: 2-4 hours for a WebSocket-based provider

---

## Architecture Overview

### Plugin System Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                   Plugin Registry (Global)                       │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │          PHF Static Maps (O(1) - Built-in)              │    │
│  │   Provider name/alias → BuiltinProvider enum            │    │
│  └─────────────────────────────────────────────────────────┘    │
│                              │                                   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │       DashMap Runtime Factories (O(1) amortized)        │    │
│  │   Provider ID → (Factory Function, Metadata)            │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
                               │
           ┌───────────────────┼───────────────────┐
           ↓                   ↓                   ↓
    BaseSTT Trait        BaseTTS Trait     BaseRealtime Trait
```

### Key Files

| Component | Path | Purpose |
|-----------|------|---------|
| STT Base Trait | `src/core/stt/base.rs` | STT provider interface |
| TTS Base Trait | `src/core/tts/base.rs` | TTS provider interface |
| Plugin Registry | `src/plugin/registry.rs` | Factory registration |
| Built-in Registration | `src/plugin/builtin/mod.rs` | Provider registration |
| PHF Dispatch | `src/plugin/dispatch.rs` | O(1) provider lookup |
| STT Module | `src/core/stt/mod.rs` | STT provider exports |
| TTS Module | `src/core/tts/mod.rs` | TTS provider exports |

---

## Provider Types

### STT Provider Requirements

The `BaseSTT` trait defines the interface for Speech-to-Text providers:

```rust
#[async_trait]
pub trait BaseSTT: Send + Sync {
    // Create new instance with configuration
    fn new(config: STTConfig) -> Result<Self, STTError> where Self: Sized;

    // Connection lifecycle
    async fn connect(&mut self) -> Result<(), STTError>;
    async fn disconnect(&mut self) -> Result<(), STTError>;
    fn is_ready(&self) -> bool;

    // Audio processing
    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError>;

    // Callbacks
    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError>;
    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError>;

    // Configuration
    fn get_config(&self) -> Option<&STTConfig>;
    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError>;

    // Provider info
    fn get_provider_info(&self) -> &'static str;
}
```

**STTConfig Structure**:
```rust
pub struct STTConfig {
    pub provider: String,
    pub api_key: String,
    pub language: String,        // ISO format: "en-US", "hi-IN"
    pub sample_rate: u32,        // Hz (typically 16000)
    pub channels: u16,           // 1 for mono, 2 for stereo
    pub punctuation: bool,
    pub encoding: String,        // "linear16", "mulaw", etc.
    pub model: String,           // Provider-specific model name
}
```

### TTS Provider Requirements

The `BaseTTS` trait defines the interface for Text-to-Speech providers:

```rust
#[async_trait]
pub trait BaseTTS: Send + Sync {
    fn new(config: TTSConfig) -> TTSResult<Self> where Self: Sized;

    async fn connect(&mut self) -> TTSResult<()>;
    async fn disconnect(&mut self) -> TTSResult<()>;
    fn is_ready(&self) -> bool;

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()>;
    async fn clear(&mut self) -> TTSResult<()>;
    async fn flush(&self) -> TTSResult<()>;

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()>;
    fn get_provider_info(&self) -> serde_json::Value;
}
```

---

## Step-by-Step Integration

### Step 1: Create Provider Directory Structure

For an STT provider:
```
src/core/stt/<provider>/
├── mod.rs          # Module exports
├── config.rs       # Provider-specific configuration
└── provider.rs     # BaseSTT implementation
```

For a TTS provider:
```
src/core/tts/<provider>/
├── mod.rs          # Module exports
├── config.rs       # Provider-specific configuration
└── provider.rs     # BaseTTS implementation
```

### Step 2: Implement Provider Configuration

Create a provider-specific config that extends the base config:

```rust
// src/core/stt/<provider>/config.rs

/// WebSocket endpoint for the provider
pub const PROVIDER_WS_URL: &str = "wss://api.provider.com/stt";

/// Default model to use
pub const DEFAULT_MODEL: &str = "default-model";

/// Default sample rate
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

#[derive(Debug, Clone)]
pub struct ProviderSTTConfig {
    pub model: String,
    pub language_code: Option<String>,
    pub sample_rate: u32,
    // Provider-specific options
    pub custom_option: bool,
}

impl ProviderSTTConfig {
    pub fn from_base(config: &STTConfig) -> Self {
        Self {
            model: if config.model.is_empty() {
                DEFAULT_MODEL.to_string()
            } else {
                config.model.clone()
            },
            language_code: if config.language.is_empty() {
                None
            } else {
                Some(config.language.clone())
            },
            sample_rate: if config.sample_rate == 0 {
                DEFAULT_SAMPLE_RATE
            } else {
                config.sample_rate
            },
            custom_option: false, // Parse from provider_options if needed
        }
    }
}
```

### Step 3: Implement the Provider

```rust
// src/core/stt/<provider>/provider.rs

use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::core::stt::{BaseSTT, STTConfig, STTError, STTResult, STTResultCallback, STTErrorCallback};
use super::config::{ProviderSTTConfig, PROVIDER_WS_URL, DEFAULT_MODEL};

pub struct ProviderSTT {
    config: STTConfig,
    provider_config: ProviderSTTConfig,
    result_callback: Option<STTResultCallback>,
    error_callback: Option<STTErrorCallback>,
    connected: AtomicBool,
    // WebSocket components
    ws_sender: Option<Arc<Mutex<futures::stream::SplitSink<...>>>>,
}

impl ProviderSTT {
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate required configuration
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required for Provider STT".to_string()
            ));
        }

        let provider_config = ProviderSTTConfig::from_base(&config);

        info!(
            provider = "provider-name",
            model = %provider_config.model,
            "Created Provider STT instance"
        );

        Ok(Self {
            config,
            provider_config,
            result_callback: None,
            error_callback: None,
            connected: AtomicBool::new(false),
            ws_sender: None,
        })
    }
}

#[async_trait::async_trait]
impl BaseSTT for ProviderSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        ProviderSTT::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        debug!("Connecting to Provider STT...");

        // Build WebSocket URL with query parameters
        let url = format!(
            "{}?model={}&sample_rate={}",
            PROVIDER_WS_URL,
            self.provider_config.model,
            self.provider_config.sample_rate
        );

        // Build request with authentication header
        // IMPORTANT: Check provider docs for correct auth header
        let request = http::Request::builder()
            .uri(&url)
            .header("api-key", &self.config.api_key)  // Varies by provider!
            .body(())
            .map_err(|e| STTError::ConnectionFailed(e.to_string()))?;

        // Connect to WebSocket
        let (ws_stream, _response) = connect_async(request)
            .await
            .map_err(|e| STTError::ConnectionFailed(e.to_string()))?;

        let (write, read) = ws_stream.split();
        self.ws_sender = Some(Arc::new(Mutex::new(write)));
        self.connected.store(true, Ordering::SeqCst);

        // Spawn message receiver task
        self.spawn_receiver_task(read);

        info!("Provider STT connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        debug!("Disconnecting from Provider STT...");
        self.connected.store(false, Ordering::SeqCst);

        if let Some(sender) = &self.ws_sender {
            let mut guard = sender.lock().await;
            let _ = guard.close().await;
        }
        self.ws_sender = None;

        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        if let Some(sender) = &self.ws_sender {
            let mut guard = sender.lock().await;
            guard.send(Message::Binary(audio_data.to_vec()))
                .await
                .map_err(|e| STTError::NetworkError(e.to_string()))?;
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        self.result_callback = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        self.error_callback = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        self.config = config.clone();
        self.provider_config = ProviderSTTConfig::from_base(&config);
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Provider STT WebSocket"
    }
}
```

### Step 4: Create Module File

```rust
// src/core/stt/<provider>/mod.rs

//! Provider STT Implementation
//!
//! Streaming speech-to-text using Provider's WebSocket API.

mod config;
mod provider;

pub use config::{ProviderSTTConfig, PROVIDER_WS_URL, DEFAULT_MODEL};
pub use provider::ProviderSTT;
```

### Step 5: Add to STT Module

```rust
// In src/core/stt/mod.rs

// Add module declaration
pub mod provider_name;

// Add re-export
pub use provider_name::{ProviderSTT, ProviderSTTConfig, PROVIDER_WS_URL};

// Update STTProvider enum (if used)
pub enum STTProvider {
    // ... existing
    ProviderName,
}

impl std::str::FromStr for STTProvider {
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            // ... existing
            "provider-name" | "provider_name" | "providername" => Ok(STTProvider::ProviderName),
            _ => Err(...)
        }
    }
}
```

### Step 6: Register with Plugin System

```rust
// In src/plugin/builtin/mod.rs

use crate::core::stt::ProviderSTT;

// Metadata function
fn provider_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("provider-name", "Provider Display Name")
        .with_description("Description of the provider")
        .with_features(["streaming", "word-timestamps", "custom-feature"])
        .with_languages(["en-US", "es-ES", "fr-FR"])  // Supported languages
        .with_models(["model-v1", "model-v2"])
        .with_aliases(&["provider", "prov"])
}

// Factory function
fn create_provider_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(ProviderSTT::new(config)?))
}

// Register with inventory
inventory::submit! {
    PluginConstructor::stt("provider-name", provider_stt_metadata, create_provider_stt)
        .with_aliases(&["provider", "prov"])
}
```

### Step 7: Add to PHF Dispatch

```rust
// In src/plugin/dispatch.rs

// Add to enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BuiltinSTTProvider {
    // ... existing providers
    ProviderName = 12,  // Next available index
}

impl BuiltinSTTProvider {
    pub const fn canonical_name(&self) -> &'static str {
        match self {
            // ... existing
            Self::ProviderName => "provider-name",
        }
    }
}

// Add to PHF map
pub static STT_PROVIDER_MAP: phf::Map<&'static str, BuiltinSTTProvider> = phf_map! {
    // ... existing entries
    "provider-name" => BuiltinSTTProvider::ProviderName,
    "provider" => BuiltinSTTProvider::ProviderName,      // alias
    "prov" => BuiltinSTTProvider::ProviderName,          // alias
};

// Update count
pub const BUILTIN_STT_COUNT: usize = 12;  // Increment

// Update names array
pub const BUILTIN_STT_NAMES: [&str; BUILTIN_STT_COUNT] = [
    // ... existing
    "provider-name",
];
```

### Step 8: Add Environment Variable Support

```rust
// In src/config/mod.rs - Add to ServerConfig
pub struct ServerConfig {
    // ... existing fields
    pub provider_api_key: Option<String>,
}

// In src/config/env.rs - Add mapping
fn load_from_env(config: &mut ServerConfig) {
    // ... existing mappings
    if let Ok(val) = std::env::var("PROVIDER_API_KEY") {
        config.provider_api_key = Some(val);
    }
}
```

---

## Running Example: Sarvam.ai

This section documents the real implementation of Sarvam.ai as verification.

### Sarvam.ai Overview

| Feature | Detail |
|---------|--------|
| **Company** | Sarvam AI (India-focused speech technology) |
| **STT Model** | Saarika v2.5 (11 Indian languages + English) |
| **TTS Model** | Bulbul v2 (natural voice synthesis) |
| **Auth Header** | `api-subscription-key` (NOT Bearer token!) |
| **Streaming** | WebSocket (WAV/PCM only, 16kHz) |
| **Languages** | hi-IN, bn-IN, ta-IN, te-IN, gu-IN, kn-IN, ml-IN, mr-IN, od-IN, pa-IN, en-IN |

### Key Implementation Details

**Authentication** (CRITICAL):
```rust
// WRONG - Standard Bearer token
.header("Authorization", format!("Bearer {}", api_key))

// CORRECT - Sarvam uses custom header
.header("api-subscription-key", &api_key)
```

**Audio Format Requirements**:
- Streaming only accepts WAV or raw PCM
- Must be 16kHz sample rate
- MP3, AAC, OGG not supported for streaming

**Connection Keep-alive**:
```rust
// Sarvam connections timeout after 60 seconds of inactivity
// Must send ping periodically
async fn start_ping_task(&self) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            // Send ping message
        }
    });
}
```

### Files Created

```
src/core/stt/sarvam/
├── mod.rs
├── config.rs
└── provider.rs

src/core/tts/sarvam/
├── mod.rs
├── config.rs
└── provider.rs
```

### Lessons Learned

**Verified from Sarvam.ai implementation (2024)**:

- [x] **WebSocket URL Format**: `wss://api.sarvam.ai/speech-to-text-translate?model=saarika:v2.5&language_code=hi-IN&sample_rate=16000&input_audio_codec=pcm_s16le&vad_signals=true`
- [x] **Audio Protocol**: Audio must be base64-encoded and sent as JSON: `{"audio": "<base64>"}`
- [x] **Response Types**: `transcript`, `speech_start`, `speech_end`, `error`
- [x] **Keep-alive Required**: Connections timeout after 60 seconds of inactivity - must send `{"type":"ping"}` periodically
- [x] **Error Messages**: Errors returned with `{"type":"error","message":"...","code":"..."}`

**Key Implementation Patterns**:

1. **Channel-based architecture**: Use `mpsc` channels to decouple WebSocket management from audio sending
2. **Bounded channels**: Use bounded channels (e.g., 32 for audio, 256 for results) for backpressure handling
3. **Separate forwarding tasks**: Spawn separate tasks for result and error forwarding to avoid blocking
4. **Atomic state management**: Use `AtomicBool` for connection state, avoid locks in hot paths

**Files Created**:
- `src/core/stt/sarvam/config.rs` (150 lines) - Configuration with validation
- `src/core/stt/sarvam/provider.rs` (660 lines) - Full BaseSTT implementation
- `src/core/stt/sarvam/mod.rs` (50 lines) - Module exports and documentation

**Test Results**: 18 unit tests pass, all compile-time checks pass

---

## Testing Your Provider

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_config_defaults() {
        let config = STTConfig::default();
        let provider_config = ProviderSTTConfig::from_base(&config);

        assert_eq!(provider_config.model, DEFAULT_MODEL);
        assert_eq!(provider_config.sample_rate, DEFAULT_SAMPLE_RATE);
    }

    #[test]
    fn test_requires_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = ProviderSTT::new(config);
        assert!(result.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key"));
        }
    }

    #[tokio::test]
    async fn test_connection_lifecycle() {
        let config = STTConfig {
            api_key: "test-key".to_string(),
            ..Default::default()
        };

        let mut stt = ProviderSTT::new(config).unwrap();
        assert!(!stt.is_ready());

        // Would need mock server for actual connection test
    }
}
```

### Integration Tests

```bash
# Set API key
export PROVIDER_API_KEY="your-api-key"

# Run specific provider tests
cargo test stt::provider_name -- --nocapture

# Run with integration test feature
cargo test --features integration-tests stt::provider_name -- --ignored
```

### Manual Testing

```bash
# Start gateway
cargo run

# Test via WebSocket client
wscat -c "ws://localhost:3000/ws"

# Send config
{"type":"config","stt_config":{"provider":"provider-name","language":"en-US"}}

# Send audio (base64 encoded)
{"type":"audio","data":"<base64-encoded-audio>"}
```

---

## Best Practices

### 1. Authentication Handling

```rust
// Always validate API key early
fn new(config: STTConfig) -> Result<Self, STTError> {
    if config.api_key.is_empty() {
        return Err(STTError::AuthenticationFailed(
            "API key is required".to_string()
        ));
    }
    // ...
}
```

### 2. Connection State Management

```rust
// Use atomic for thread-safe state
connected: AtomicBool::new(false),

// Always update state consistently
async fn connect(&mut self) -> Result<(), STTError> {
    self.connected.store(true, Ordering::SeqCst);
    // ...
}

async fn disconnect(&mut self) -> Result<(), STTError> {
    self.connected.store(false, Ordering::SeqCst);
    // ...
}
```

### 3. Error Propagation

```rust
// Use error callbacks for streaming errors
if let Some(ref callback) = self.error_callback {
    callback(STTError::NetworkError("Connection lost".to_string())).await;
}
```

### 4. Logging

```rust
use tracing::{debug, info, warn, error};

// Use structured logging
info!(
    provider = "provider-name",
    model = %config.model,
    language = %config.language,
    "Provider connected"
);
```

### 5. Zero-Copy Audio

```rust
// Use Bytes for zero-copy audio transfer
async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
    // Don't clone unless necessary
    sender.send(Message::Binary(audio_data.to_vec())).await?;
    Ok(())
}
```

---

## Common Mistakes

### 1. Wrong Authentication Header

Each provider uses different auth headers:

| Provider | Header |
|----------|--------|
| Deepgram | `Authorization: Token <key>` |
| ElevenLabs | `xi-api-key: <key>` |
| OpenAI | `Authorization: Bearer <key>` |
| Sarvam | `api-subscription-key: <key>` |
| Azure | `Ocp-Apim-Subscription-Key: <key>` |

### 2. Audio Format Mismatch

Always verify:
- Sample rate matches configuration
- Encoding format is supported
- Streaming APIs often have format restrictions

### 3. Missing PHF Entry

If provider lookup fails, check:
1. Entry added to `STT_PROVIDER_MAP`
2. Aliases added for common variations
3. `BUILTIN_STT_COUNT` incremented
4. Entry in `BUILTIN_STT_NAMES`

### 4. Connection Timeout

Many WebSocket APIs require keep-alive:
```rust
// Bad: No keep-alive
async fn connect(&mut self) { /* just connect */ }

// Good: Start ping task
async fn connect(&mut self) {
    // ... connect
    self.start_ping_task();
}
```

### 5. Callback Deadlock

```rust
// Bad: Holding lock during callback
let guard = self.callback.lock().await;
guard.on_result(result).await;  // May deadlock!

// Good: Clone and release lock first
let callback = {
    let guard = self.callback.lock().await;
    guard.clone()
};
// Lock released
callback.on_result(result).await;
```

---

## Troubleshooting

### Provider Not Found

```
Error: Unknown STT provider: 'provider-name'
```

**Solutions**:
1. Verify `inventory::submit!` macro is called
2. Check PHF map entry in `dispatch.rs`
3. Ensure module is declared in `mod.rs`
4. Run `cargo build` to trigger inventory collection

### Connection Failed

```
Error: Connection failed: <details>
```

**Check**:
1. API key is valid
2. Endpoint URL is correct
3. Authentication header matches provider spec
4. Network connectivity

### Audio Not Processed

**Check**:
1. Audio format matches provider requirements
2. Sample rate is correct
3. Connection is in ready state
4. Callbacks are registered

---

## Verification Checklist

Before marking provider complete:

- [ ] Provider compiles without errors
- [ ] Unit tests pass
- [ ] API key validation works
- [ ] Connection lifecycle works (connect/disconnect)
- [ ] Audio processing works
- [ ] Callbacks invoke correctly
- [ ] Error handling works
- [ ] PHF lookup works (including aliases, case-insensitive)
- [ ] Provider appears in registry list
- [ ] Environment variable support works
- [ ] Documentation updated
- [ ] Integration tests pass (with real API key)

---

## See Also

- [Plugin Architecture](./plugins.md) - Full plugin system documentation
- [Provider Documentation](./new_provider.md) - Detailed provider guide
- [API Reference](./api-reference.md) - REST and WebSocket API
- Provider-specific docs in `docs/<provider>-stt.md` and `docs/<provider>-tts.md`

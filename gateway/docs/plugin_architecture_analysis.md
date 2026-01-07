# WaaV Gateway Plugin Architecture Analysis

## Executive Summary

This document provides a comprehensive analysis of the WaaV Gateway source code and recommends a plugin architecture that enables:

1. **Provider Isolation** - A bug in one provider cannot crash others
2. **Dynamic Extension** - Add new providers without modifying core code
3. **Processor Pipelines** - Add VAD, post-processors, custom transformations
4. **Feature Extensibility** - New endpoints, rate limiting strategies, batching
5. **Core Stability** - New features available to all providers automatically

---

## Part 1: Current Architecture Analysis

### 1.1 Module Organization (211 Rust Source Files)

```
gateway/src/
├── core/                          # Provider abstractions & implementations
│   ├── stt/                       # 10 STT providers
│   ├── tts/                       # 11 TTS providers
│   ├── realtime/                  # Audio-to-audio (OpenAI, Hume)
│   ├── voice_manager/             # STT/TTS orchestration
│   ├── turn_detect/               # ONNX-based turn detection
│   ├── emotion/                   # Emotion mapping
│   └── cache/                     # Result caching
├── handlers/                      # HTTP/WebSocket handlers
│   ├── ws/                        # Main WebSocket handler
│   ├── realtime/                  # Realtime audio handler
│   └── api.rs, speak.rs           # REST handlers
├── routes/                        # API, WS, Realtime, Webhooks
├── middleware/                    # Auth & connection limiting
├── config/                        # YAML/ENV configuration
├── state/                         # Application state (AppState)
├── auth/                          # JWT/API secret auth
└── errors/                        # Error types
```

### 1.2 Provider Pattern Analysis

**Current Factory Pattern:**

```rust
// src/core/stt/mod.rs - Hardcoded match statement
pub fn create_stt_provider(provider: &str, config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    let provider_enum: STTProvider = provider.parse()?;
    match provider_enum {
        STTProvider::Deepgram => Ok(Box::new(DeepgramSTT::new(config)?)),
        STTProvider::Google => Ok(Box::new(GoogleSTT::new(config)?)),
        STTProvider::ElevenLabs => Ok(Box::new(ElevenLabsSTT::new(config)?)),
        // ... 10 providers hardcoded
    }
}
```

**Key Traits (Well-Designed):**

```rust
// BaseSTT trait - src/core/stt/base.rs
#[async_trait]
pub trait BaseSTT: Send + Sync {
    fn new(config: STTConfig) -> Result<Self, STTError> where Self: Sized;
    async fn connect(&mut self) -> Result<(), STTError>;
    async fn disconnect(&mut self) -> Result<(), STTError>;
    fn is_ready(&self) -> bool;
    async fn send_audio(&mut self, data: Bytes) -> Result<(), STTError>;
    fn on_result(&mut self, callback: Arc<dyn STTResultCallback>) -> Result<(), STTError>;
    fn on_error(&mut self, callback: Arc<dyn STTErrorCallback>) -> Result<(), STTError>;
}

// BaseTTS trait - src/core/tts/base.rs
#[async_trait]
pub trait BaseTTS: Send + Sync {
    fn new(config: TTSConfig) -> TTSResult<Self> where Self: Sized;
    async fn connect(&mut self) -> TTSResult<()>;
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()>;
    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()>;
}
```

### 1.3 Extension Points That Exist

| Component | Current Extension Method | Difficulty |
|-----------|-------------------------|------------|
| STT Provider | Implement `BaseSTT` + modify factory | Medium |
| TTS Provider | Implement `BaseTTS` + modify factory | Medium |
| Realtime Provider | Implement `BaseRealtime` + modify factory | Medium |
| REST Endpoint | Add handler + route | Easy |
| WebSocket Messages | Modify `IncomingMessage` enum | Hard |
| Middleware | Layer composition in `main.rs` | Easy |
| Configuration | Add to `ServerConfig` struct | Medium |

### 1.4 Coupling Issues for Plugin Architecture

**Problem 1: Hardcoded Factory Functions**
```rust
// Must modify source to add provider
match provider_enum {
    STTProvider::Deepgram => ...,
    STTProvider::Google => ...,
    // Can't add external providers dynamically
}
```

**Problem 2: Static Configuration Schema**
```rust
// ServerConfig has explicit fields for each provider
pub struct ServerConfig {
    pub deepgram_api_key: Option<String>,
    pub elevenlabs_api_key: Option<String>,
    // Can't add plugin-specific config at runtime
}
```

**Problem 3: Enum-Based Provider Selection**
```rust
// STTProvider enum is exhaustive
pub enum STTProvider {
    Deepgram, Google, ElevenLabs, Azure, ...
    // Can't extend from external code
}
```

**Problem 4: WebSocket Message Type Coupling**
```rust
// Fixed message variants
pub enum IncomingMessage {
    Config(ConfigMessage),
    Audio(AudioMessage),
    Speak(SpeakCommand),
    // Can't add custom message types
}
```

---

## Part 2: Plugin Architecture Options

Based on extensive research, there are four main approaches for Rust plugin systems:

### Option A: Trait-Based Registry (Recommended for WaaV)

**Pattern:** Compile-time plugins with runtime registration

```rust
// Plugin interface (in core crate)
pub trait ProviderPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn version(&self) -> &'static str;

    // Factory methods
    fn create_stt(&self, config: &PluginConfig) -> Option<Box<dyn BaseSTT>>;
    fn create_tts(&self, config: &PluginConfig) -> Option<Box<dyn BaseTTS>>;
    fn create_processor(&self, config: &PluginConfig) -> Option<Box<dyn AudioProcessor>>;

    // Capabilities declaration
    fn capabilities(&self) -> PluginCapabilities;
}

// Registry for all plugins
pub struct PluginRegistry {
    providers: DashMap<String, Arc<dyn ProviderPlugin>>,
    processors: DashMap<String, Arc<dyn ProcessorPlugin>>,
    middleware: Vec<Arc<dyn MiddlewarePlugin>>,
}

impl PluginRegistry {
    pub fn register(&self, plugin: Arc<dyn ProviderPlugin>) {
        self.providers.insert(plugin.name().to_string(), plugin);
    }

    pub fn create_stt(&self, name: &str, config: &PluginConfig) -> Result<Box<dyn BaseSTT>> {
        self.providers.get(name)
            .and_then(|p| p.create_stt(config))
            .ok_or_else(|| Error::UnknownProvider(name.to_string()))
    }
}
```

**Pros:**
- No runtime overhead for core providers (static dispatch)
- Type-safe at compile time
- No FFI/ABI stability concerns
- Works with existing `BaseSTT`/`BaseTTS` traits

**Cons:**
- External plugins require recompilation
- Not truly dynamic (can't add at runtime)

**Best For:** WaaV Gateway's primary use case of curated providers

---

### Option B: Dynamic Library Loading (dlopen)

**Pattern:** Load `.so`/`.dll` plugins at runtime via `libloading` + `abi_stable`

```rust
// Stable ABI interface
#[repr(C)]
#[derive(StableAbi)]
pub struct PluginDeclaration {
    pub rustc_version: RustcVersion,
    pub plugin_version: RStr<'static>,
    pub register: extern "C" fn(&mut dyn PluginRegistrar),
}

// Plugin implementation (in separate crate)
#[export_root_module]
pub fn get_plugin() -> PluginDeclaration {
    PluginDeclaration {
        rustc_version: RustcVersion::new(),
        plugin_version: RStr::from("1.0.0"),
        register: |registrar| {
            registrar.register_stt("my-provider", Box::new(MySTT));
        }
    }
}
```

**Pros:**
- True runtime loading
- Can add providers without recompilation
- Hot reload possible

**Cons:**
- Requires `abi_stable` crate and careful FFI design
- Complex error handling across library boundaries
- `unsafe` code required
- Must rebuild all plugins when ABI changes

**Crates:** [abi_stable](https://lib.rs/crates/abi_stable), [libloading](https://lib.rs/crates/libloading), [dynamic-plugin](https://lib.rs/crates/dynamic-plugin)

---

### Option C: WebAssembly Plugin System (WASM)

**Pattern:** Sandboxed plugins using Extism/Wasmtime

```rust
// Host-side plugin loading
use extism::{Plugin, Manifest, Wasm};

let manifest = Manifest::new([Wasm::file("my_provider.wasm")]);
let mut plugin = Plugin::new(&manifest, [], true)?;

// Call plugin function
let result = plugin.call::<&str, &str>("create_stt", config_json)?;
```

**Pros:**
- Complete isolation (sandboxed execution)
- Cross-platform compatibility
- Can limit CPU/memory usage
- Safe execution of untrusted code

**Cons:**
- ~3-10x slower than native code
- Limited async support
- Data must be serialized across boundary
- Complex host function implementation

**Crates:** [extism](https://github.com/extism/extism), [wasmtime](https://docs.wasmtime.dev/)

---

### Option D: Process-Based Isolation

**Pattern:** Each plugin runs in separate process, communicates via IPC

```rust
// Plugin subprocess
pub struct PluginProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl PluginProcess {
    pub async fn call(&mut self, request: &Request) -> Result<Response> {
        // Serialize request to JSON
        writeln!(self.stdin, "{}", serde_json::to_string(request)?)?;
        // Read response
        let mut line = String::new();
        self.stdout.read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }
}
```

**Pros:**
- Complete crash isolation
- Easy to implement
- Can use any language for plugins
- Resource limits via cgroups

**Cons:**
- IPC overhead (microseconds per call)
- Complex state management
- Process management complexity

---

## Part 3: Recommended Architecture for WaaV

### 3.1 Hybrid Approach: Registry + Extension Points

Given WaaV's requirements for:
- Real-time audio processing (<10ms latency)
- Production stability
- Multiple provider types
- Processor pipelines

**Recommended: Trait-Based Registry with Extension Points**

```
┌─────────────────────────────────────────────────────────────────┐
│                       WaaV Gateway Core                         │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    Plugin Registry                        │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌──────────────────┐   │  │
│  │  │  Providers  │ │ Processors  │ │ Middleware/Routes│   │  │
│  │  │  Registry   │ │  Registry   │ │    Registry      │   │  │
│  │  └─────────────┘ └─────────────┘ └──────────────────┘   │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│  ┌───────────────────────────┼───────────────────────────────┐ │
│  │         Plugin Configuration Manager                       │ │
│  │  (Dynamic config loading from YAML/JSON for plugins)       │ │
│  └───────────────────────────────────────────────────────────┘ │
│                              │                                  │
├──────────────────────────────┼──────────────────────────────────┤
│                    Extension Points                             │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐  │
│  │ STT Plugin │ │ TTS Plugin │ │ Processor  │ │ Middleware │  │
│  │ Interface  │ │ Interface  │ │ Interface  │ │ Interface  │  │
│  └────────────┘ └────────────┘ └────────────┘ └────────────┘  │
└─────────────────────────────────────────────────────────────────┘
         │                │               │               │
┌────────┴────────┐ ┌─────┴─────┐ ┌──────┴──────┐ ┌──────┴──────┐
│ Built-in        │ │ Built-in  │ │ Built-in    │ │ Built-in    │
│ STT Providers   │ │ TTS       │ │ Processors  │ │ Middleware  │
│ - Deepgram      │ │ Providers │ │ - VAD       │ │ - RateLimit │
│ - Google        │ │ - ElevenL │ │ - Denoise   │ │ - Auth      │
│ - OpenAI        │ │ - Azure   │ │ - Resample  │ │ - Logging   │
│ - ...           │ │ - ...     │ │             │ │             │
└─────────────────┘ └───────────┘ └─────────────┘ └─────────────┘
         │                │               │               │
┌────────┴────────┐ ┌─────┴─────┐ ┌──────┴──────┐ ┌──────┴──────┐
│ Feature Crate   │ │ Feature   │ │ Feature     │ │ Feature     │
│ Plugins         │ │ Crate     │ │ Crate       │ │ Crate       │
│ (compile-time)  │ │ Plugins   │ │ Plugins     │ │ Plugins     │
└─────────────────┘ └───────────┘ └─────────────┘ └─────────────┘
```

### 3.2 Core Traits Design

```rust
// ─────────────────────────────────────────────────────────────────
// Plugin Core Traits (in waav-plugin-api crate)
// ─────────────────────────────────────────────────────────────────

/// Base trait for all plugins
pub trait Plugin: Send + Sync + 'static {
    /// Unique identifier for the plugin
    fn id(&self) -> &'static str;

    /// Human-readable name
    fn name(&self) -> &'static str;

    /// Semantic version
    fn version(&self) -> &'static str;

    /// Plugin initialization
    fn init(&mut self, context: &PluginContext) -> Result<(), PluginError>;

    /// Plugin shutdown
    fn shutdown(&mut self) -> Result<(), PluginError>;

    /// Health check
    fn health_check(&self) -> HealthStatus;
}

/// STT Provider Plugin
pub trait STTProviderPlugin: Plugin {
    /// Create a new STT provider instance
    fn create(&self, config: &PluginConfig) -> Result<Box<dyn BaseSTT>, PluginError>;

    /// Supported models
    fn supported_models(&self) -> Vec<ModelInfo>;

    /// Supported audio formats
    fn supported_formats(&self) -> Vec<AudioFormat>;
}

/// TTS Provider Plugin
pub trait TTSProviderPlugin: Plugin {
    /// Create a new TTS provider instance
    fn create(&self, config: &PluginConfig) -> Result<Box<dyn BaseTTS>, PluginError>;

    /// Available voices
    fn available_voices(&self) -> Vec<VoiceInfo>;

    /// Supported output formats
    fn supported_formats(&self) -> Vec<AudioFormat>;
}

/// Audio Processor Plugin (VAD, denoise, resample, etc.)
pub trait ProcessorPlugin: Plugin {
    /// Create a processor instance
    fn create(&self, config: &PluginConfig) -> Result<Box<dyn AudioProcessor>, PluginError>;

    /// Processor type (pre-stt, post-stt, pre-tts, post-tts)
    fn processor_type(&self) -> ProcessorType;

    /// Processing latency estimate
    fn latency_estimate(&self) -> Duration;
}

/// Audio Processor trait
#[async_trait]
pub trait AudioProcessor: Send + Sync {
    /// Process audio data (must be real-time safe)
    async fn process(&mut self, input: AudioChunk) -> Result<AudioChunk, ProcessorError>;

    /// Reset processor state
    fn reset(&mut self);

    /// Get processing statistics
    fn stats(&self) -> ProcessorStats;
}

/// Middleware Plugin (rate limiting, custom auth, logging)
pub trait MiddlewarePlugin: Plugin {
    /// Create middleware layer
    fn create_layer(&self, config: &PluginConfig) -> Result<BoxLayer, PluginError>;

    /// Middleware order (lower = earlier in chain)
    fn order(&self) -> i32;
}

/// Route Plugin (add custom REST/WebSocket endpoints)
pub trait RoutePlugin: Plugin {
    /// Get routes to register
    fn routes(&self) -> Vec<RouteDefinition>;

    /// WebSocket message handlers (optional)
    fn ws_handlers(&self) -> Option<Box<dyn WsMessageHandler>>;
}
```

### 3.3 Plugin Registry Design

```rust
// ─────────────────────────────────────────────────────────────────
// Plugin Registry (in waav-gateway core)
// ─────────────────────────────────────────────────────────────────

use dashmap::DashMap;
use std::sync::Arc;

pub struct PluginRegistry {
    /// STT provider plugins
    stt_providers: DashMap<String, Arc<dyn STTProviderPlugin>>,

    /// TTS provider plugins
    tts_providers: DashMap<String, Arc<dyn TTSProviderPlugin>>,

    /// Audio processors (ordered by pipeline position)
    processors: DashMap<String, Arc<dyn ProcessorPlugin>>,

    /// Middleware plugins (sorted by order)
    middleware: RwLock<Vec<Arc<dyn MiddlewarePlugin>>>,

    /// Route plugins
    routes: DashMap<String, Arc<dyn RoutePlugin>>,

    /// Plugin initialization context
    context: Arc<PluginContext>,
}

impl PluginRegistry {
    pub fn new(config: &ServerConfig) -> Self {
        let context = Arc::new(PluginContext::from_config(config));
        Self {
            stt_providers: DashMap::new(),
            tts_providers: DashMap::new(),
            processors: DashMap::new(),
            middleware: RwLock::new(Vec::new()),
            routes: DashMap::new(),
            context,
        }
    }

    /// Register a plugin (type-erased, dispatches to correct registry)
    pub fn register<P: Plugin + 'static>(&self, mut plugin: P) -> Result<(), PluginError> {
        // Initialize plugin
        plugin.init(&self.context)?;

        let plugin = Arc::new(plugin);

        // Dispatch to appropriate registry based on type
        if let Some(stt) = (plugin.clone() as Arc<dyn Any>).downcast_ref::<dyn STTProviderPlugin>() {
            self.stt_providers.insert(plugin.id().to_string(), stt.clone());
        }
        // ... similar for other plugin types

        tracing::info!(
            plugin_id = plugin.id(),
            plugin_version = plugin.version(),
            "Plugin registered"
        );

        Ok(())
    }

    /// Get all registered STT provider names
    pub fn stt_provider_names(&self) -> Vec<String> {
        self.stt_providers.iter().map(|e| e.key().clone()).collect()
    }

    /// Create an STT provider instance
    pub fn create_stt(&self, name: &str, config: &PluginConfig) -> Result<Box<dyn BaseSTT>, PluginError> {
        self.stt_providers
            .get(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?
            .create(config)
    }

    /// Create processor pipeline
    pub fn create_processor_pipeline(
        &self,
        pipeline: &[String],
        configs: &HashMap<String, PluginConfig>,
    ) -> Result<ProcessorPipeline, PluginError> {
        let mut processors = Vec::new();
        for name in pipeline {
            let config = configs.get(name).cloned().unwrap_or_default();
            let processor = self.processors
                .get(name)
                .ok_or_else(|| PluginError::NotFound(name.clone()))?
                .create(&config)?;
            processors.push(processor);
        }
        Ok(ProcessorPipeline::new(processors))
    }

    /// Build middleware stack
    pub fn middleware_layers(&self) -> Vec<BoxLayer> {
        let middleware = self.middleware.read().unwrap();
        middleware.iter()
            .map(|m| m.create_layer(&PluginConfig::default()).unwrap())
            .collect()
    }
}
```

### 3.4 Plugin Configuration

```yaml
# config.yaml - Plugin configuration section
plugins:
  # STT Providers
  stt:
    deepgram:
      enabled: true
      api_key: "${DEEPGRAM_API_KEY}"
      default_model: "nova-3"

    custom-whisper:
      enabled: true
      path: "./plugins/custom_whisper"
      config:
        model_path: "/models/whisper-large-v3"
        device: "cuda"

  # TTS Providers
  tts:
    elevenlabs:
      enabled: true
      api_key: "${ELEVENLABS_API_KEY}"
      default_voice: "rachel"

  # Audio Processors
  processors:
    vad:
      enabled: true
      type: "silero-vad"
      config:
        threshold: 0.5
        min_speech_ms: 250

    denoise:
      enabled: true
      type: "deepfilter"
      config:
        attenuation_db: 30

    resample:
      enabled: false
      type: "libsamplerate"

  # Processor Pipelines
  pipelines:
    default_stt_pre:
      - denoise
      - vad
    default_stt_post: []
    default_tts_pre: []
    default_tts_post:
      - resample

  # Middleware
  middleware:
    rate_limiting:
      enabled: true
      type: "sliding_window"
      config:
        requests_per_second: 100
        burst: 20

    custom_auth:
      enabled: false
      path: "./plugins/custom_auth"
```

### 3.5 Built-in Plugin Registration

```rust
// src/plugins/mod.rs - Built-in plugin registration

mod builtin;

use crate::core::stt::*;
use crate::core::tts::*;

/// Register all built-in plugins
pub fn register_builtin_plugins(registry: &PluginRegistry) -> Result<(), PluginError> {
    // STT Providers
    registry.register(builtin::DeepgramSTTPlugin::new())?;
    registry.register(builtin::GoogleSTTPlugin::new())?;
    registry.register(builtin::OpenAISTTPlugin::new())?;
    registry.register(builtin::ElevenLabsSTTPlugin::new())?;
    registry.register(builtin::AzureSTTPlugin::new())?;
    registry.register(builtin::CartesiaSTTPlugin::new())?;
    registry.register(builtin::AssemblyAISTTPlugin::new())?;
    registry.register(builtin::AwsTranscribeSTTPlugin::new())?;
    registry.register(builtin::IbmWatsonSTTPlugin::new())?;
    registry.register(builtin::GroqSTTPlugin::new())?;

    // TTS Providers
    registry.register(builtin::DeepgramTTSPlugin::new())?;
    registry.register(builtin::ElevenLabsTTSPlugin::new())?;
    registry.register(builtin::GoogleTTSPlugin::new())?;
    registry.register(builtin::AzureTTSPlugin::new())?;
    registry.register(builtin::CartesiaTTSPlugin::new())?;
    registry.register(builtin::OpenAITTSPlugin::new())?;
    registry.register(builtin::AwsPollyTTSPlugin::new())?;
    registry.register(builtin::IbmWatsonTTSPlugin::new())?;
    registry.register(builtin::HumeTTSPlugin::new())?;
    registry.register(builtin::LmntTTSPlugin::new())?;
    registry.register(builtin::PlayHtTTSPlugin::new())?;

    // Processors
    #[cfg(feature = "turn-detect")]
    registry.register(builtin::TurnDetectPlugin::new())?;

    #[cfg(feature = "noise-filter")]
    registry.register(builtin::NoiseFilterPlugin::new())?;

    tracing::info!(
        stt_count = registry.stt_provider_names().len(),
        tts_count = registry.tts_provider_names().len(),
        "Built-in plugins registered"
    );

    Ok(())
}
```

### 3.6 Example Built-in Plugin Implementation

```rust
// src/plugins/builtin/deepgram_stt.rs

use waav_plugin_api::*;
use crate::core::stt::{BaseSTT, DeepgramSTT, STTConfig, STTError};

pub struct DeepgramSTTPlugin {
    initialized: bool,
}

impl DeepgramSTTPlugin {
    pub fn new() -> Self {
        Self { initialized: false }
    }
}

impl Plugin for DeepgramSTTPlugin {
    fn id(&self) -> &'static str { "deepgram-stt" }
    fn name(&self) -> &'static str { "Deepgram Speech-to-Text" }
    fn version(&self) -> &'static str { env!("CARGO_PKG_VERSION") }

    fn init(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
        self.initialized = true;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        self.initialized = false;
        Ok(())
    }

    fn health_check(&self) -> HealthStatus {
        if self.initialized {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy("Not initialized".into())
        }
    }
}

impl STTProviderPlugin for DeepgramSTTPlugin {
    fn create(&self, config: &PluginConfig) -> Result<Box<dyn BaseSTT>, PluginError> {
        let stt_config = STTConfig {
            provider: "deepgram".to_string(),
            api_key: config.get_string("api_key")
                .ok_or_else(|| PluginError::ConfigMissing("api_key".into()))?,
            language: config.get_string("language").unwrap_or("en-US".into()),
            sample_rate: config.get_u32("sample_rate").unwrap_or(16000),
            channels: config.get_u32("channels").unwrap_or(1) as u8,
            punctuation: config.get_bool("punctuation").unwrap_or(true),
            encoding: config.get_string("encoding").unwrap_or("linear16".into()),
            model: config.get_string("model").unwrap_or("nova-3".into()),
        };

        let stt = DeepgramSTT::new(stt_config)
            .map_err(|e| PluginError::Creation(e.to_string()))?;

        Ok(Box::new(stt))
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo { id: "nova-3", name: "Nova 3", languages: vec!["en", "es", "fr", ...] },
            ModelInfo { id: "nova-2", name: "Nova 2", languages: vec!["en", "es", "fr", ...] },
            ModelInfo { id: "whisper", name: "Whisper", languages: vec!["en", "es", "fr", ...] },
        ]
    }

    fn supported_formats(&self) -> Vec<AudioFormat> {
        vec![
            AudioFormat::PCM16,
            AudioFormat::FLAC,
            AudioFormat::MP3,
            AudioFormat::OGG,
        ]
    }
}
```

### 3.7 Processor Pipeline Design

```rust
// src/core/pipeline.rs

/// Audio processing pipeline
pub struct ProcessorPipeline {
    processors: Vec<Box<dyn AudioProcessor>>,
    stats: PipelineStats,
}

impl ProcessorPipeline {
    pub fn new(processors: Vec<Box<dyn AudioProcessor>>) -> Self {
        Self {
            processors,
            stats: PipelineStats::default(),
        }
    }

    /// Process audio through the pipeline
    pub async fn process(&mut self, mut chunk: AudioChunk) -> Result<AudioChunk, PipelineError> {
        let start = Instant::now();

        for (i, processor) in self.processors.iter_mut().enumerate() {
            let proc_start = Instant::now();

            chunk = processor.process(chunk).await
                .map_err(|e| PipelineError::ProcessorFailed {
                    index: i,
                    error: e
                })?;

            self.stats.processor_times[i] = proc_start.elapsed();
        }

        self.stats.total_time = start.elapsed();
        self.stats.chunks_processed += 1;

        Ok(chunk)
    }

    /// Get pipeline statistics
    pub fn stats(&self) -> &PipelineStats {
        &self.stats
    }

    /// Reset all processors
    pub fn reset(&mut self) {
        for processor in &mut self.processors {
            processor.reset();
        }
        self.stats = PipelineStats::default();
    }
}

/// Pipeline execution modes
pub enum PipelineMode {
    /// Process each chunk through all processors sequentially
    Sequential,
    /// Process chunks in parallel where processors are independent
    Parallel,
    /// Stream processing with buffering
    Streaming { buffer_size: usize },
}
```

---

## Part 4: Implementation Roadmap

### Phase 1: Plugin API Foundation (2-3 weeks)

**Files to Create:**
```
waav-plugin-api/           # New crate
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── traits.rs          # Plugin, STTProviderPlugin, TTSProviderPlugin, etc.
│   ├── config.rs          # PluginConfig, ConfigValue
│   ├── context.rs         # PluginContext
│   ├── error.rs           # PluginError
│   ├── types.rs           # AudioChunk, ModelInfo, VoiceInfo
│   └── processor.rs       # AudioProcessor trait
```

**Tasks:**
1. Create `waav-plugin-api` crate with core traits
2. Define stable API for plugins
3. Implement `PluginConfig` with type-safe accessors
4. Add derive macros for common patterns
5. Write comprehensive documentation

### Phase 2: Plugin Registry (1-2 weeks)

**Files to Modify:**
```
gateway/src/
├── plugins/
│   ├── mod.rs             # New module
│   ├── registry.rs        # PluginRegistry implementation
│   ├── loader.rs          # Plugin discovery and loading
│   └── builtin/           # Built-in plugin wrappers
│       ├── mod.rs
│       ├── deepgram_stt.rs
│       ├── elevenlabs_tts.rs
│       └── ...
├── state/mod.rs           # Add registry to AppState
├── main.rs                # Initialize registry
```

**Tasks:**
1. Implement `PluginRegistry` with DashMap storage
2. Create wrapper plugins for all existing providers
3. Modify `AppState` to hold registry
4. Update `create_stt_provider`/`create_tts_provider` to use registry
5. Add plugin health check endpoints

### Phase 3: Processor Pipeline (2 weeks)

**Files to Create/Modify:**
```
gateway/src/
├── core/
│   ├── pipeline/
│   │   ├── mod.rs
│   │   ├── processor.rs    # ProcessorPipeline
│   │   ├── chain.rs        # ProcessorChain builder
│   │   └── stats.rs        # Pipeline statistics
│   └── voice_manager/
│       └── manager.rs      # Integrate pipelines
├── plugins/builtin/
│   ├── vad_processor.rs
│   ├── denoise_processor.rs
│   └── resample_processor.rs
```

**Tasks:**
1. Implement `ProcessorPipeline` with async processing
2. Create `ProcessorChain` builder for configuration
3. Wrap existing turn detection as processor plugin
4. Wrap DeepFilterNet as processor plugin
5. Integrate pipelines into VoiceManager

### Phase 4: Configuration System (1 week)

**Files to Modify:**
```
gateway/src/
├── config/
│   ├── mod.rs
│   ├── yaml.rs            # Add plugin section parsing
│   └── plugins.rs         # New: Plugin-specific config
```

**Tasks:**
1. Add `plugins` section to YAML schema
2. Implement plugin config parsing
3. Support environment variable substitution
4. Add plugin enable/disable flags
5. Implement pipeline configuration

### Phase 5: Error Isolation (1 week)

**Tasks:**
1. Add `catch_unwind` wrappers for plugin calls
2. Implement circuit breaker for failing plugins
3. Add plugin-specific error types
4. Implement graceful degradation
5. Add plugin failure metrics

### Phase 6: Advanced Features (Optional, 2+ weeks)

**Dynamic Loading (if needed):**
- Use `abi_stable` for stable ABI
- Implement plugin discovery
- Add hot reload support

**WASM Plugins (for untrusted code):**
- Integrate Extism runtime
- Define WASM plugin interface
- Implement host functions

---

## Part 5: Migration Strategy

### Step 1: Non-Breaking Addition
- Add plugin system alongside existing factory functions
- Existing code continues to work unchanged

### Step 2: Gradual Migration
- Migrate one provider at a time to plugin wrapper
- Keep factory functions as fallback

### Step 3: Deprecation
- Mark old factory functions as deprecated
- Log warnings when used

### Step 4: Removal
- Remove old factory code
- Plugin registry becomes sole provider source

### Backward Compatibility
```rust
// Compatibility layer during migration
pub fn create_stt_provider(provider: &str, config: STTConfig) -> Result<Box<dyn BaseSTT>> {
    // Try plugin registry first
    if let Ok(stt) = REGISTRY.create_stt(provider, &config.into()) {
        return Ok(stt);
    }

    // Fall back to legacy factory (deprecated)
    tracing::warn!("Using deprecated factory for provider: {}", provider);
    legacy_create_stt_provider(provider, config)
}
```

---

## Part 6: Testing Strategy

### Unit Tests
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use waav_plugin_api::*;

    #[test]
    fn test_plugin_registration() {
        let registry = PluginRegistry::new(&ServerConfig::default());
        registry.register(MockSTTPlugin::new()).unwrap();

        assert!(registry.stt_provider_names().contains(&"mock-stt".to_string()));
    }

    #[tokio::test]
    async fn test_processor_pipeline() {
        let mut pipeline = ProcessorPipeline::new(vec![
            Box::new(MockProcessor::new()),
            Box::new(MockProcessor::new()),
        ]);

        let input = AudioChunk::new(vec![0u8; 1024], 16000);
        let output = pipeline.process(input).await.unwrap();

        assert_eq!(pipeline.stats().chunks_processed, 1);
    }

    #[test]
    fn test_plugin_isolation() {
        let registry = PluginRegistry::new(&ServerConfig::default());
        registry.register(PanickingPlugin::new()).unwrap();

        // Should not crash, returns error
        let result = registry.create_stt("panicking", &PluginConfig::default());
        assert!(result.is_err());
    }
}
```

### Integration Tests
```rust
#[tokio::test]
async fn test_full_pipeline_with_plugins() {
    let config = load_test_config();
    let state = AppState::new(config).await;

    // Create voice manager with plugin-based providers
    let voice_manager = VoiceManager::new(VoiceManagerConfig {
        stt_provider: "deepgram-stt",
        tts_provider: "elevenlabs-tts",
        pre_stt_pipeline: vec!["denoise", "vad"],
        ..Default::default()
    }, &state.plugin_registry).await?;

    // Test full flow
    voice_manager.send_audio(test_audio()).await?;
}
```

---

## Part 7: Performance Considerations

### Real-Time Constraints

```rust
// Processor must complete within budget
pub trait AudioProcessor: Send + Sync {
    /// Maximum processing time for real-time safety
    const MAX_LATENCY_MS: u64 = 10;

    async fn process(&mut self, input: AudioChunk) -> Result<AudioChunk, ProcessorError> {
        let start = Instant::now();

        let result = self.process_impl(input).await;

        let elapsed = start.elapsed();
        if elapsed.as_millis() > Self::MAX_LATENCY_MS as u128 {
            tracing::warn!(
                processor = std::any::type_name::<Self>(),
                elapsed_ms = elapsed.as_millis(),
                "Processor exceeded latency budget"
            );
        }

        result
    }
}
```

### Lock-Free Plugin Access

```rust
// Use DashMap for concurrent access without global locks
pub struct PluginRegistry {
    stt_providers: DashMap<String, Arc<dyn STTProviderPlugin>>,
    // ...
}

// Access is O(1) and lock-free for reads
impl PluginRegistry {
    pub fn get_stt(&self, name: &str) -> Option<Ref<'_, String, Arc<dyn STTProviderPlugin>>> {
        self.stt_providers.get(name)
    }
}
```

### Pre-Allocated Buffers

```rust
// Pre-allocate buffers in processor pipeline
impl ProcessorPipeline {
    pub fn with_buffer_pool(processors: Vec<Box<dyn AudioProcessor>>, pool_size: usize) -> Self {
        Self {
            processors,
            buffer_pool: BufferPool::new(pool_size),
            stats: PipelineStats::default(),
        }
    }
}
```

---

## Appendix A: Key Files Reference

| File | Purpose |
|------|---------|
| `src/core/stt/base.rs` | STT trait definition |
| `src/core/tts/base.rs` | TTS trait definition |
| `src/core/stt/mod.rs` | STT factory (to be replaced) |
| `src/core/tts/mod.rs` | TTS factory (to be replaced) |
| `src/core/voice_manager/manager.rs` | Voice orchestration |
| `src/handlers/ws/handler.rs` | WebSocket handling |
| `src/config/mod.rs` | Configuration loading |
| `src/state/mod.rs` | Application state |
| `src/main.rs` | Server startup |

## Appendix B: Research Sources

- [Implementing Plugin Architecture for Dynamic Loading](https://peerdh.com/blogs/programming-insights/implementing-a-rust-based-plugin-architecture-for-dynamic-feature-loading)
- [Plugins in Rust - Michael-F-Bryan](https://adventures.michaelfbryan.com/posts/plugins-in-rust/)
- [Plugin Technologies Comparison](https://nullderef.com/blog/plugin-tech/)
- [Dynamic Library Loading in Rust](https://nullderef.com/blog/plugin-dynload/)
- [Extism - WASM Plugin Framework](https://github.com/extism/extism)
- [abi_stable - Stable ABI for Rust](https://lib.rs/crates/abi_stable)
- [Wasmtime Documentation](https://docs.wasmtime.dev/)

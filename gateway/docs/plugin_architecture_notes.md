# WaaV Gateway Plugin Architecture - Comprehensive Source Analysis Notes

## Executive Summary

This document contains exhaustive notes from reading every source file in the WaaV Gateway codebase, documenting all components relevant to implementing a plugin architecture.

---

## Part 1: Source Code Structure Overview

### Total File Count: 122+ Rust source files

```
src/
├── main.rs                    (298 lines) - Server startup, middleware stack
├── lib.rs                     (20 lines)  - Library exports
├── init.rs                    (69 lines)  - Model initialization
├── core/                      - Provider abstractions & implementations
│   ├── mod.rs                 - Re-exports
│   ├── state.rs               - CoreState (connection pools, cache, turn detector)
│   ├── stt/                   - 10 STT providers
│   ├── tts/                   - 11 TTS providers
│   ├── realtime/              - 2 Realtime providers
│   ├── voice_manager/         - STT/TTS orchestration
│   ├── emotion/               - Emotion mapping system
│   ├── turn_detect/           - ML turn detection
│   ├── cache/                 - Caching system
│   └── providers/             - Cloud infrastructure (Google, Azure)
├── handlers/                  - Request handlers
│   ├── ws/                    - WebSocket voice processing
│   ├── realtime/              - Realtime audio handler
│   ├── livekit/               - LiveKit integration
│   ├── sip/                   - SIP integration
│   ├── api.rs                 - Health check
│   ├── voices.rs              - Voice listing & cloning
│   ├── speak.rs               - TTS REST endpoint
│   └── recording.rs           - Recording download
├── middleware/                - Auth & connection limiting
├── routes/                    - Route definitions
├── config/                    - Configuration system
├── state/                     - Application state
├── auth/                      - Authentication
├── errors/                    - Error types
├── utils/                     - Utilities
├── livekit/                   - LiveKit client
└── docs/                      - OpenAPI generation
```

---

## Part 2: Core Traits Analysis (Key Extension Points)

### 2.1 BaseSTT Trait (`src/core/stt/base.rs`)

```rust
#[async_trait]
pub trait BaseSTT: Send + Sync {
    fn new(config: STTConfig) -> Result<Self, STTError> where Self: Sized;
    async fn connect(&mut self) -> Result<(), STTError>;
    async fn disconnect(&mut self) -> Result<(), STTError>;
    fn is_ready(&self) -> bool;
    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError>;
    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError>;
    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError>;
    fn get_config(&self) -> Option<&STTConfig>;
    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError>;
    fn get_provider_info(&self) -> &'static str;
}
```

**Callback Types:**
- `STTResultCallback`: `Arc<dyn Fn(STTResult) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>`
- `STTErrorCallback`: Similar async callback pattern

**Observations:**
- ✓ Async-first design with `#[async_trait]`
- ✓ Send + Sync bounds for thread safety
- ✓ Result-based error handling
- ✗ `new()` requires `Self: Sized` - prevents trait object construction
- ✗ No version information
- ✗ No capability declaration

### 2.2 BaseTTS Trait (`src/core/tts/base.rs`)

```rust
#[async_trait]
pub trait BaseTTS: Send + Sync {
    fn new(config: TTSConfig) -> TTSResult<Self> where Self: Sized;
    fn get_provider(&mut self) -> Option<&mut TTSProvider>;
    async fn connect(&mut self) -> TTSResult<()>;
    async fn disconnect(&mut self) -> TTSResult<()>;
    fn is_ready(&self) -> bool;
    fn get_connection_state(&self) -> ConnectionState;
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()>;
    async fn clear(&mut self) -> TTSResult<()>;
    async fn flush(&self) -> TTSResult<()>;
    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()>;
    fn remove_audio_callback(&mut self) -> TTSResult<()>;
    fn get_provider_info(&self) -> serde_json::Value;
    async fn set_req_manager(&mut self, _req_manager: Arc<ReqManager>);
}
```

**AudioCallback Trait:**
```rust
#[async_trait]
pub trait AudioCallback: Send + Sync {
    async fn on_audio(&self, audio_data: AudioData);
    async fn on_error(&self, error: TTSError);
    async fn on_complete(&self);
}
```

### 2.3 BaseRealtime Trait (`src/core/realtime/base.rs`)

```rust
#[async_trait]
pub trait BaseRealtime: Send + Sync {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> where Self: Sized;
    async fn connect(&mut self) -> RealtimeResult<()>;
    async fn disconnect(&mut self) -> RealtimeResult<()>;
    fn is_ready(&self) -> bool;
    async fn send_audio(&mut self, audio: Bytes) -> RealtimeResult<()>;
    async fn on_transcript(&mut self, callback: TranscriptCallback) -> RealtimeResult<()>;
    async fn on_audio(&mut self, callback: AudioOutputCallback) -> RealtimeResult<()>;
    async fn on_error(&mut self, callback: RealtimeErrorCallback) -> RealtimeResult<()>;
    async fn on_response_done(&mut self, callback: ResponseDoneCallback) -> RealtimeResult<()>;
    fn get_provider_info(&self) -> &'static str;
}
```

### 2.4 EmotionMapper Trait (`src/core/emotion/mapper.rs`)

```rust
pub trait EmotionMapper: Send + Sync {
    fn map_emotion(&self, config: &EmotionConfig) -> MappedEmotion;
    fn supports_intensity(&self) -> bool;
    fn supports_style(&self) -> bool;
    fn supports_description(&self) -> bool;
    fn provider_name(&self) -> &'static str;
}
```

---

## Part 3: Factory Functions (Hardcoded Match Statements)

### 3.1 STT Factory (`src/core/stt/mod.rs:176-224`)

```rust
pub fn create_stt_provider(provider: &str, config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    let provider_enum: STTProvider = provider.parse()?;
    match provider_enum {
        STTProvider::Deepgram => Ok(Box::new(DeepgramSTT::new(config)?)),
        STTProvider::Google => Ok(Box::new(GoogleSTT::new(config)?)),
        STTProvider::ElevenLabs => Ok(Box::new(ElevenLabsSTT::new(config)?)),
        STTProvider::Azure => Ok(Box::new(AzureSTT::new(config)?)),
        STTProvider::Cartesia => Ok(Box::new(CartesiaSTT::new(config)?)),
        STTProvider::OpenAI => Ok(Box::new(OpenAISTT::new(config)?)),
        STTProvider::AssemblyAI => Ok(Box::new(AssemblyAISTT::new(config)?)),
        STTProvider::AwsTranscribe => Ok(Box::new(AwsTranscribeSTT::new(config)?)),
        STTProvider::IbmWatson => Ok(Box::new(IbmWatsonSTT::new(config)?)),
        STTProvider::Groq => Ok(Box::new(GroqSTT::new(config)?)),
    }
}
```

**Problem:** Adding a new provider requires:
1. Adding enum variant to `STTProvider`
2. Adding match arm here
3. Recompilation of entire gateway

### 3.2 TTS Factory (`src/core/tts/mod.rs:69-88`)

```rust
pub fn create_tts_provider(provider_type: &str, config: TTSConfig) -> TTSResult<Box<dyn BaseTTS>> {
    match provider_type.to_lowercase().as_str() {
        "deepgram" => Ok(Box::new(DeepgramTTS::new(config)?)),
        "elevenlabs" => Ok(Box::new(ElevenLabsTTS::new(config)?)),
        "google" => Ok(Box::new(GoogleTTS::new(config)?)),
        "azure" | "microsoft-azure" => Ok(Box::new(AzureTTS::new(config)?)),
        "cartesia" => Ok(Box::new(CartesiaTTS::new(config)?)),
        "openai" => Ok(Box::new(OpenAITTS::new(config)?)),
        "aws-polly" | "aws_polly" | "amazon-polly" | "polly" => Ok(Box::new(AwsPollyTTS::new(config)?)),
        "ibm-watson" | "ibm_watson" | "watson" | "ibm" => Ok(Box::new(IbmWatsonTTS::new(config)?)),
        "hume" | "hume-ai" | "hume_ai" => Ok(Box::new(HumeTTS::new(config)?)),
        "lmnt" | "lmnt-ai" | "lmnt_ai" => Ok(Box::new(LmntTts::new(config)?)),
        "playht" | "play-ht" | "play_ht" | "play.ht" => Ok(Box::new(PlayHtTts::new(config)?)),
        _ => Err(TTSError::InvalidConfiguration(...)),
    }
}
```

### 3.3 Realtime Factory (`src/core/realtime/mod.rs:120-134`)

```rust
pub fn create_realtime_provider(provider_type: &str, config: RealtimeConfig) -> RealtimeResult<Box<dyn BaseRealtime>> {
    match provider_type.to_lowercase().as_str() {
        "openai" => Ok(Box::new(OpenAIRealtime::new(config)?)),
        "hume" | "hume-ai" | "hume_ai" => Ok(Box::new(HumeRealtime::new(config)?)),
        _ => Err(RealtimeError::InvalidConfiguration(...)),
    }
}
```

---

## Part 4: Configuration System Analysis

### 4.1 ServerConfig (`src/config/mod.rs:76-177`)

**Structure:** Monolithic struct with 44+ fields

**Provider API Keys (all Optional, zeroized):**
- `deepgram_api_key`
- `elevenlabs_api_key`
- `google_credentials`
- `azure_speech_subscription_key` + `azure_speech_region`
- `cartesia_api_key`
- `openai_api_key`
- `assemblyai_api_key`
- `hume_api_key`
- `lmnt_api_key`
- `groq_api_key`
- `playht_api_key` + `playht_user_id`
- `ibm_watson_api_key` + `ibm_watson_instance_id` + `ibm_watson_region`
- `aws_access_key_id` + `aws_secret_access_key` + `aws_region`

**Problem:** Adding a new provider requires adding fields to this struct.

### 4.2 Configuration Loading Priority
1. YAML file
2. Environment variables
3. .env file
4. Defaults

---

## Part 5: Handler Architecture

### 5.1 WebSocket Handler Flow (`src/handlers/ws/`)

**Message Types (`src/handlers/ws/messages.rs`):**

```rust
pub enum IncomingMessage {
    Config { stream_id?, audio?, audio_disabled?, stt_config?, tts_config?, livekit? },
    Speak { text, flush?, allow_interruption? },
    Clear,
    SendMessage { message, role, topic?, debug? },
    SIPTransfer { transfer_to },
    Auth { token },
}

pub enum OutgoingMessage {
    Ready { stream_id, livekit_room_name?, livekit_url?, ... },
    STTResult { transcript, is_final, is_speech_final, confidence },
    Message { message: UnifiedMessage },
    ParticipantDisconnected { participant: ParticipantDisconnectedInfo },
    TTSPlaybackComplete { timestamp },
    Error { message },
    SIPTransferError { message },
    Authenticated { id? },
    AuthRequired,
}
```

**Problem:** Fixed enum variants - can't add custom message types without modifying core code.

### 5.2 Connection State (`src/handlers/ws/state.rs`)

```rust
pub struct ConnectionState {
    pub voice_manager: Option<Arc<VoiceManager>>,
    pub livekit_client: Option<Arc<RwLock<LiveKitClient>>>,
    pub livekit_operation_queue: Option<OperationQueue>,
    pub audio_enabled: AtomicBool,
    pub stream_id: Option<String>,
    pub livekit_room_name: Option<String>,
    pub livekit_local_identity: Option<String>,
    pub recording_egress_id: Option<String>,
    pub auth: Auth,
}
```

---

## Part 6: Middleware Analysis

### 6.1 Auth Middleware (`src/middleware/auth.rs`)

**Two hardcoded auth strategies:**
1. **API Secret:** Constant-time comparison with `auth_api_secrets`
2. **JWT:** External validation via `AuthClient`

**Token Extraction Priority:**
1. Authorization header: `Bearer <token>`
2. Query parameter: `?token=<token>`

**Problem:** Can't add new auth strategies without modifying middleware.

### 6.2 Connection Limit Middleware (`src/middleware/connection_limit.rs`)

**Fixed limit enforcement:**
- Global WebSocket limit
- Per-IP limit (default: 100)

**Problem:** Fixed limit strategy - can't add priority queues, custom key extractors.

### 6.3 Rate Limiting (`src/main.rs:176-188`)

**Uses tower_governor:**
- `SmartIpKeyExtractor` hardcoded
- Disabled at 100,000 RPS (performance testing)

**Problem:** Can't change rate limit algorithm or key extraction.

---

## Part 7: Voice Manager Analysis

### 7.1 VoiceManager (`src/core/voice_manager/manager.rs`)

**Orchestrates:**
- STT provider lifecycle
- TTS provider lifecycle
- Speech final detection (multi-tier fallback)
- Callback distribution

**Speech Final Detection Tiers:**
| Tier | Method | Timeout | Priority |
|------|--------|---------|----------|
| Primary | STT provider speech_final | 1.8s | Highest |
| Secondary | ML turn detection | 500ms | Mid |
| Tertiary | Hard timeout | 4.0s | Lowest |

### 7.2 VoiceManagerConfig (`src/core/voice_manager/config.rs`)

```rust
pub struct VoiceManagerConfig {
    pub stt_config: STTConfig,
    pub tts_config: TTSConfig,
    pub speech_final_config: SpeechFinalConfig,
}
```

---

## Part 8: Audio Processing

### 8.1 DeepFilterNet (`src/utils/noise_filter.rs`)

- Feature-gated: `#[cfg(feature = "noise-filter")]`
- Uses lazy static with thread pool
- Not pluggable - hardcoded implementation

### 8.2 Turn Detection (`src/core/turn_detect/`)

- Feature-gated: `#[cfg(feature = "turn-detect")]`
- ONNX-based ML model
- Not pluggable - specific model implementation

---

## Part 9: Caching System

### 9.1 CacheStore (`src/core/cache/`)

**Backends:**
- `MemoryCacheBackend` - In-memory with size limits
- `FilesystemCacheBackend` - Disk-based

**Configuration:**
- Max entries: 5,000,000
- Max size: 500 MB
- TTL: configurable

**Already has trait abstraction** - more pluggable than other systems.

---

## Part 10: Integration Points Summary

### Points That Need Plugin Support:

| Component | Current State | Plugin Priority |
|-----------|--------------|-----------------|
| STT Providers | Hardcoded factory | HIGH |
| TTS Providers | Hardcoded factory | HIGH |
| Realtime Providers | Hardcoded factory | HIGH |
| Auth Strategies | 2 hardcoded | HIGH |
| Rate Limiters | tower_governor only | MEDIUM |
| WS Message Types | Fixed enum | MEDIUM |
| Audio Processors | DeepFilterNet only | MEDIUM |
| Middleware Stack | Fixed in main.rs | MEDIUM |
| Webhooks | LiveKit only | LOW |
| Cache Backends | Trait exists | LOW |
| Config Sources | YAML/ENV only | LOW |

---

## Part 11: Backward Compatibility Constraints

### Must Preserve:

1. **Trait APIs:** `BaseSTT`, `BaseTTS`, `BaseRealtime` must remain unchanged
2. **Message Formats:** WebSocket message JSON schemas
3. **Config Formats:** YAML and ENV var names
4. **API Endpoints:** REST and WebSocket paths
5. **Error Codes:** HTTP status codes and error structures
6. **Provider Names:** "deepgram", "elevenlabs", etc.

### Can Change (Internal):

1. Factory function implementations
2. Internal state management
3. Middleware ordering mechanism
4. Provider instantiation logic

---

## Part 12: Hardcoded Constants Reference

### Timeouts:
- Provider ready: 30s
- STT speech final wait: 1800ms
- Turn detection inference: 500ms
- Hard timeout: 4000ms
- LiveKit audio wait: 10s
- Idle connection: 300 ± 30s
- Sender shutdown: 500ms

### Size Limits:
- WS frame: 10MB
- Text message: 1MB
- Audio frame: 5MB
- TTS text: 100KB
- LiveKit message: 50KB
- Phone number: 256 bytes
- Stream ID: 256 bytes
- Auth token: 4KB

### Defaults:
- Port: 3001
- Rate limit: 60 req/s per IP
- Burst: 10
- Max connections per IP: 100
- Cache TTL: 30 days
- STT sample rate: 16000 Hz
- TTS sample rate: 24000 Hz

---

## Part 13: Key Files for Plugin Implementation

### Must Modify:
1. `src/core/stt/mod.rs` - Factory function
2. `src/core/tts/mod.rs` - Factory function
3. `src/core/realtime/mod.rs` - Factory function
4. `src/config/mod.rs` - Plugin config section
5. `src/state/mod.rs` - Registry in AppState
6. `src/main.rs` - Plugin initialization

### Should Not Modify:
1. `src/core/stt/base.rs` - BaseSTT trait
2. `src/core/tts/base.rs` - BaseTTS trait
3. `src/core/realtime/base.rs` - BaseRealtime trait
4. `src/handlers/ws/messages.rs` - Message types (extend, not replace)
5. Individual provider implementations

---

## Part 14: Dependency Graph

```
main.rs
  └── ServerConfig (from config/)
  └── AppState (from state/)
        └── CoreState (from core/state.rs)
              └── TTS ReqManagers (HashMap)
              └── CacheStore
              └── TurnDetector (optional)
        └── LiveKitRoomHandler
        └── LiveKitSipHandler
        └── AuthClient
        └── Connection tracking (DashMap)

handlers/ws/handler.rs
  └── ConnectionState
        └── VoiceManager
              └── Box<dyn BaseSTT> (from factory)
              └── Box<dyn BaseTTS> (from factory)
              └── Callbacks
        └── LiveKitClient
        └── Auth context

Factory Call Chain:
  Config message → config_handler → VoiceManager::new() → create_stt_provider() / create_tts_provider()
```

---

## Part 15: Testing Infrastructure

### Test Locations:
- `src/core/stt/mod.rs` - STT factory tests
- `src/core/tts/mod.rs` - TTS factory tests
- `src/handlers/ws/tests.rs` - WS handler tests
- `src/middleware/connection_limit.rs` - Connection limit tests
- `tests/` directory - Integration tests

### Test Patterns:
- Mock providers for unit tests
- Config-based provider selection
- Feature-gated test cases

---

## Conclusion

The gateway has excellent trait abstractions (`BaseSTT`, `BaseTTS`, `BaseRealtime`) but suffers from:

1. **Hardcoded factories** - Match statements prevent external provider registration
2. **Static configuration** - ServerConfig has explicit fields per provider
3. **Fixed middleware** - No abstraction for auth strategies or rate limiters
4. **Closed message types** - Enums can't be extended externally

A plugin architecture must:
1. Replace match-based factories with registry pattern
2. Add dynamic configuration for plugins
3. Abstract middleware as pluggable components
4. Support message type extensions
5. Maintain full backward compatibility with existing code

# Tencent Cloud Speech Integration Plan

> **Provider #40** | Batch 6: China & East Asia
> **Created:** 2026-01-13

## Overview

This document outlines the implementation plan for integrating Tencent Cloud Speech services (ASR + TTS) into the WaaV Gateway plugin system.

## Key Differences from Other Providers

| Aspect | Baidu | Tencent Cloud |
|--------|-------|---------------|
| **Auth (STT)** | OAuth 2.0 access token | HMAC-SHA1 signature in URL params |
| **Auth (TTS)** | OAuth 2.0 access token | TC3-HMAC-SHA256 header signature |
| **STT URL Format** | `wss://host?sn=SESSION` | `wss://host/<appid>?{signature_params}` |
| **Audio Chunk** | 160ms (5120 bytes @ 16kHz) | 40ms (1280 bytes @ 16kHz) |
| **Response Format** | `MID_TEXT`/`FIN_TEXT` | `slice_type`: 0=interim, 1=final, 2=complete |

## Module Structure

```
src/core/stt/tencent/
├── mod.rs           # Module exports and tests
├── config.rs        # Configuration types, engine models, audio formats
├── messages.rs      # WebSocket request/response message types
├── signature.rs     # HMAC-SHA1 signature generation for ASR
└── client.rs        # BaseSTT implementation

src/core/tts/tencent/
├── mod.rs           # Module exports and tests
├── config.rs        # Configuration types, voice types
├── signature.rs     # TC3-HMAC-SHA256 signature for API v3
└── provider.rs      # BaseTTS implementation
```

## STT Implementation

### 1. Configuration (`config.rs`)

**Constants:**
```rust
pub const TENCENT_ASR_WS_URL: &str = "wss://asr.cloud.tencent.com/asr/v2";
pub const DEFAULT_ENGINE_MODEL: &str = "16k_zh";
pub const DEFAULT_VOICE_FORMAT: u32 = 4;  // Speex
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;
pub const RECOMMENDED_CHUNK_DURATION_MS: u32 = 40;
```

**Engine Models Enum:**
```rust
pub enum TencentEngineModel {
    Mandarin8k,        // 8k_zh
    MandarinShort8k,   // 8k_zh_s
    Mandarin16k,       // 16k_zh (default)
    MandarinVideo16k,  // 16k_zh_video
    English16k,        // 16k_en
    Cantonese16k,      // 16k_ca
    Japanese16k,       // 16k_ja
    Korean16k,         // 16k_ko
    Thai16k,           // 16k_th
    Vietnamese16k,     // 16k_vi
    Indonesian16k,     // 16k_id
}
```

**Audio Format Enum:**
```rust
pub enum TencentAudioFormat {
    Pcm = 1,    // Raw PCM
    Speex = 4,  // Default, recommended
    Silk = 6,   // WeChat format
    Mp3 = 8,
    Opus = 10,  // Low latency
    Wav = 12,
    M4a = 14,
    Aac = 16,
}
```

**Config Struct:**
```rust
pub struct TencentSttConfig {
    pub secret_id: String,
    pub secret_key: String,
    pub app_id: String,
    pub engine_model_type: TencentEngineModel,
    pub voice_format: TencentAudioFormat,
    pub needvad: bool,                // Voice Activity Detection
    pub filter_dirty: bool,           // Filter profanity
    pub filter_modal: bool,           // Filter modal particles
    pub filter_punc: bool,            // Filter punctuation
    pub word_info: u8,                // 0-2: word timestamp level
    pub vad_silence_time: Option<u32>, // 240-2000ms
    pub hotword_id: Option<String>,   // Custom vocabulary ID
}
```

### 2. Signature Generation (`signature.rs`)

**HMAC-SHA1 Signature Algorithm:**
```rust
pub fn generate_signature(
    secret_key: &str,
    secret_id: &str,
    app_id: &str,
    engine_model_type: &str,
    voice_id: &str,
    timestamp: u64,
    expired: u64,
    nonce: u64,
    voice_format: u32,
    // ... other optional params
) -> Result<(String, String), SignatureError> {
    // 1. Build params map
    // 2. Sort params alphabetically
    // 3. Generate param_string: key1=value1&key2=value2
    // 4. Calculate signature: Base64(HMAC-SHA1(secret_key, param_string))
    // 5. URL-encode signature
    // 6. Return (query_string, signature)
}
```

### 3. Messages (`messages.rs`)

**Response Structure:**
```rust
#[derive(Debug, Deserialize)]
pub struct TencentAsrResponse {
    pub code: i32,
    pub message: String,
    pub voice_id: String,
    pub message_id: Option<String>,
    pub result: Option<TencentAsrResult>,
    #[serde(rename = "final")]
    pub is_final: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct TencentAsrResult {
    pub slice_type: i32,      // 0=interim, 1=one-sentence end, 2=final
    pub index: i32,
    pub start_time: u64,
    pub end_time: u64,
    pub voice_text_str: String,
    pub word_size: Option<i32>,
    pub word_list: Option<Vec<TencentWord>>,
}

#[derive(Debug, Deserialize)]
pub struct TencentWord {
    pub word: String,
    pub start_time: u64,
    pub end_time: u64,
    pub stable_flag: i32,
}
```

**Error Codes:**
```rust
pub enum TencentAsrError {
    InvalidParameters = 4001,
    AuthenticationFailure = 4002,
    ServiceNotActivated = 4003,
    InsufficientQuota = 4004,
    ServiceNotSupported = 4005,
    AudioTooLong = 4006,
    AudioDecodingFailed = 4007,
    ClientUploadTimeout = 4008,
    ServerError = 5000,
    ServerBusy = 5001,
    ServerTimeout = 5002,
}
```

### 4. Client Implementation (`client.rs`)

**Connection Flow:**
1. Generate unique `voice_id` (16 characters)
2. Calculate timestamp, expired (timestamp + 86400), nonce (random)
3. Generate HMAC-SHA1 signature with all params
4. Build WebSocket URL: `wss://asr.cloud.tencent.com/asr/v2/{app_id}?{query_params}`
5. Connect to WebSocket (no auth headers needed - signature in URL)
6. Stream audio in 40ms chunks (1280 bytes @ 16kHz)
7. Process responses with slice_type parsing

**Audio Chunking:**
```rust
fn get_chunk_size(&self) -> usize {
    // 40ms at 16kHz, 16-bit: 16000 * 2 * 40 / 1000 = 1280 bytes
    (self.config.sample_rate as usize * 2 * 40) / 1000
}
```

## TTS Implementation

### 1. Configuration (`config.rs`)

**Constants:**
```rust
pub const TENCENT_TTS_URL: &str = "https://tts.intl.tencentcloudapi.com";
pub const TENCENT_TTS_ACTION: &str = "TextToVoice";
pub const TENCENT_TTS_VERSION: &str = "2019-08-23";
pub const TENCENT_TTS_REGION: &str = "ap-singapore";  // International endpoint
pub const MAX_TEXT_LENGTH_CHINESE: usize = 150;       // Chinese characters
pub const MAX_TEXT_LENGTH_LETTERS: usize = 500;       // Letters for English
```

**Voice Types Enum:**
```rust
pub enum TencentVoiceType {
    // Standard voices (1000-series)
    IntelligentWoman = 1001,
    IntelligentMan = 1002,
    MatureMan = 1003,
    WechatXiaowei = 1050,
    WechatXiaoweiFemale = 1051,

    // Premium voices (101000-series)
    IntelligentWomanPremium = 101001,
    IntelligentManPremium = 101002,
    CustomerServiceFemale = 101003,
    CustomerServiceMale = 101004,
    NewsFemale = 101005,
    NewsMale = 101006,
    CantoneseFemalePremium = 101015,
    CantoneseMalePremium = 101016,
    SichuanDialect = 101017,
    EnglishFemale = 101050,
    EnglishMale = 101051,
}
```

**Config Struct:**
```rust
pub struct TencentTtsConfig {
    pub secret_id: String,
    pub secret_key: String,
    pub region: String,
    pub voice_type: TencentVoiceType,
    pub volume: f32,           // 0.0-10.0, default 0
    pub speed: f32,            // -2.0 to 6.0, default 0
    pub primary_language: u32, // 1=Chinese, 2=English
    pub sample_rate: u32,      // 16000 or 8000
    pub codec: String,         // "wav", "mp3", "pcm"
    pub enable_subtitle: bool, // Word-level timestamps
}
```

**Speed Mapping:**
```rust
impl TencentTtsConfig {
    pub fn speed_factor(&self) -> f32 {
        // -2 → 0.6x, -1 → 0.8x, 0 → 1.0x, 1 → 1.2x, 2 → 1.5x, 6 → 2.5x
        match self.speed as i32 {
            -2 => 0.6,
            -1 => 0.8,
            0 => 1.0,
            1 => 1.2,
            2 => 1.5,
            n if n >= 6 => 2.5,
            _ => 1.0,
        }
    }
}
```

### 2. TC3 Signature Generation (`signature.rs`)

**TC3-HMAC-SHA256 Algorithm:**
```rust
pub fn generate_tc3_signature(
    secret_id: &str,
    secret_key: &str,
    action: &str,
    version: &str,
    region: &str,
    payload: &str,
    timestamp: u64,
) -> Result<TencentAuthHeaders, SignatureError> {
    // Step 1: Build canonical request
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        "POST",                              // HTTP Method
        "/",                                 // URI
        "",                                  // Query string (empty)
        canonical_headers,                   // content-type;host
        signed_headers,                      // content-type;host
        sha256_hex(payload)                  // Payload hash
    );

    // Step 2: Build string to sign
    let date = format_date(timestamp);       // YYYY-MM-DD
    let credential_scope = format!("{}/tts/tc3_request", date);
    let string_to_sign = format!(
        "{}\n{}\n{}\n{}",
        "TC3-HMAC-SHA256",
        timestamp,
        credential_scope,
        sha256_hex(&canonical_request)
    );

    // Step 3: Calculate signature
    let secret_date = hmac_sha256(format!("TC3{}", secret_key).as_bytes(), date.as_bytes());
    let secret_service = hmac_sha256(&secret_date, b"tts");
    let secret_signing = hmac_sha256(&secret_service, b"tc3_request");
    let signature = hex_encode(hmac_sha256(&secret_signing, string_to_sign.as_bytes()));

    // Step 4: Build Authorization header
    let authorization = format!(
        "TC3-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        secret_id, credential_scope, signed_headers, signature
    );

    Ok(TencentAuthHeaders {
        authorization,
        timestamp: timestamp.to_string(),
        action,
        version,
        region,
    })
}
```

### 3. Provider Implementation (`provider.rs`)

**Request Flow:**
1. Build JSON payload with text, voice, speed, volume, etc.
2. Generate TC3 signature with all headers
3. Send POST request to TTS endpoint
4. Parse response and extract base64-encoded audio
5. Decode audio and pass to callback

**Response Structure:**
```rust
#[derive(Debug, Deserialize)]
pub struct TencentTtsResponse {
    #[serde(rename = "Response")]
    pub response: TencentTtsResponseBody,
}

#[derive(Debug, Deserialize)]
pub struct TencentTtsResponseBody {
    #[serde(rename = "Audio")]
    pub audio: Option<String>,  // Base64 encoded
    #[serde(rename = "SessionId")]
    pub session_id: String,
    #[serde(rename = "RequestId")]
    pub request_id: String,
    #[serde(rename = "Subtitles")]
    pub subtitles: Option<Vec<TencentSubtitle>>,
    #[serde(rename = "Error")]
    pub error: Option<TencentTtsError>,
}
```

## Plugin System Registration

### 1. Add to `src/core/stt/mod.rs`
```rust
pub mod tencent;
pub use tencent::{
    TencentStt, TencentSttConfig, TencentEngineModel, TencentAudioFormat,
    TENCENT_ASR_WS_URL,
};
```

### 2. Add to `src/core/tts/mod.rs`
```rust
pub mod tencent;
pub use tencent::{
    TencentTts, TencentTtsConfig, TencentVoiceType,
    TENCENT_TTS_URL, TENCENT_TTS_ACTION, TENCENT_TTS_VERSION,
};
```

### 3. Register in `src/plugin/builtin/mod.rs`
```rust
// STT metadata and factory
fn tencent_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("tencent", "Tencent Cloud Speech")
        .with_description("Tencent Cloud ASR with 97% accuracy")
        .with_features(["streaming", "vad", "word-timestamps", "hot-words"])
        .with_languages(["zh", "en", "ja", "ko", "th", "vi", "id", "yue"])
        .with_models(["16k_zh", "16k_en", "16k_ja", "16k_ko", "8k_zh"])
        .with_aliases(&["tencent-cloud", "tencent_cloud", "腾讯云"])
}

fn create_tencent_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(TencentStt::new(config)?))
}

inventory::submit! {
    PluginConstructor::stt("tencent", tencent_stt_metadata, create_tencent_stt)
        .with_aliases(&["tencent-cloud", "tencent_cloud", "腾讯云"])
}

// TTS metadata and factory
fn tencent_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("tencent", "Tencent Cloud TTS")
        .with_description("Tencent Cloud Text-to-Speech with WeChat voices")
        .with_features(["streaming", "word-timestamps", "cantonese", "dialects"])
        .with_voices(["1001", "1002", "101001", "101050"])
        .with_aliases(&["tencent-cloud", "tencent_cloud", "腾讯云"])
}

fn create_tencent_tts(config: TTSConfig) -> Result<Box<dyn BaseTTS>, TTSError> {
    Ok(Box::new(TencentTts::new(config)?))
}

inventory::submit! {
    PluginConstructor::tts("tencent", tencent_tts_metadata, create_tencent_tts)
        .with_aliases(&["tencent-cloud", "tencent_cloud", "腾讯云"])
}
```

### 4. Add to PHF Dispatch (`src/plugin/dispatch.rs`)
```rust
// Add to BuiltinSTTProvider enum
TencentCloud = XX,  // Next available index

// Add to STT_PROVIDER_MAP
"tencent" => BuiltinSTTProvider::TencentCloud,
"tencent-cloud" => BuiltinSTTProvider::TencentCloud,
"tencent_cloud" => BuiltinSTTProvider::TencentCloud,

// Add to BuiltinTTSProvider enum
TencentCloud = XX,  // Next available index

// Add to TTS_PROVIDER_MAP
"tencent" => BuiltinTTSProvider::TencentCloud,
"tencent-cloud" => BuiltinTTSProvider::TencentCloud,
"tencent_cloud" => BuiltinTTSProvider::TencentCloud,
```

## Testing Plan

### Unit Tests (~100 tests target)

**Config Tests:**
- [ ] Engine model parsing (all 11 models)
- [ ] Audio format parsing (all 8 formats)
- [ ] Voice type parsing (all standard and premium)
- [ ] Configuration validation
- [ ] Sample rate validation (8k/16k)
- [ ] Speed/volume bounds checking

**Signature Tests:**
- [ ] HMAC-SHA1 signature generation
- [ ] Parameter sorting
- [ ] URL encoding special characters
- [ ] Nonce generation
- [ ] Timestamp/expired calculation
- [ ] TC3-HMAC-SHA256 signature generation
- [ ] Canonical request building
- [ ] Authorization header format

**Message Tests:**
- [ ] Response parsing (success, interim, final)
- [ ] Error code parsing
- [ ] Word list extraction
- [ ] Slice type interpretation
- [ ] TTS response parsing
- [ ] Subtitle extraction

**Client Tests:**
- [ ] Client creation with valid config
- [ ] Client creation with invalid config
- [ ] Connection state management
- [ ] Audio chunking (40ms)
- [ ] Not connected send error
- [ ] Callback registration

### Integration Tests

- [ ] Real WebSocket connection (with API key)
- [ ] Audio streaming flow
- [ ] TTS synthesis
- [ ] Error handling (invalid credentials)

## Implementation Order

1. **STT Config Module** (config.rs) - 200 lines, 20 tests
2. **STT Signature Module** (signature.rs) - 100 lines, 15 tests
3. **STT Messages Module** (messages.rs) - 150 lines, 15 tests
4. **STT Client Module** (client.rs) - 400 lines, 25 tests
5. **STT Module Exports** (mod.rs) - 50 lines
6. **TTS Config Module** (config.rs) - 150 lines, 15 tests
7. **TTS Signature Module** (signature.rs) - 100 lines, 10 tests
8. **TTS Provider Module** (provider.rs) - 300 lines, 20 tests
9. **TTS Module Exports** (mod.rs) - 50 lines
10. **Plugin Registration** - Update builtin/mod.rs, dispatch.rs
11. **Documentation Updates** - provider_integration_status.md

## Dependencies

**Existing (no new dependencies needed):**
- `hmac` - HMAC algorithms
- `sha1` - SHA1 hashing (for ASR signature)
- `sha2` - SHA256 hashing (for TTS signature)
- `base64` - Base64 encoding
- `tokio-tungstenite` - WebSocket client
- `reqwest` - HTTP client
- `serde_json` - JSON serialization

## API Key Format

For STT:
```
secret_id|secret_key|app_id
```

For TTS:
```
secret_id|secret_key
```

Example parsing:
```rust
impl TencentSttConfig {
    pub fn from_base(config: STTConfig) -> Result<Self, STTError> {
        let parts: Vec<&str> = config.api_key.splitn(3, '|').collect();
        if parts.len() != 3 {
            return Err(STTError::ConfigurationError(
                "API key must be in format: secret_id|secret_key|app_id".to_string(),
            ));
        }

        Ok(Self {
            secret_id: parts[0].to_string(),
            secret_key: parts[1].to_string(),
            app_id: parts[2].to_string(),
            ..Default::default()
        })
    }
}
```

## Estimated Lines of Code

| File | Lines | Tests |
|------|-------|-------|
| stt/config.rs | 250 | 25 |
| stt/signature.rs | 120 | 15 |
| stt/messages.rs | 180 | 15 |
| stt/client.rs | 450 | 25 |
| stt/mod.rs | 60 | 0 |
| tts/config.rs | 200 | 20 |
| tts/signature.rs | 120 | 10 |
| tts/provider.rs | 350 | 25 |
| tts/mod.rs | 60 | 0 |
| builtin/mod.rs updates | 100 | 5 |
| dispatch.rs updates | 20 | 0 |
| **Total** | **~1900** | **~140** |

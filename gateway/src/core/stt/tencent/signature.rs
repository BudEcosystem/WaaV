//! Tencent Cloud ASR HMAC-SHA1 Signature Generation
//!
//! This module implements the signature algorithm required for Tencent Cloud
//! Speech Recognition (ASR) WebSocket authentication.
//!
//! # Algorithm
//!
//! 1. Collect all request parameters
//! 2. Sort parameters alphabetically by key
//! 3. Generate parameter string: `key1=value1&key2=value2&...`
//! 4. Calculate HMAC-SHA1: `signature = HMAC-SHA1(secret_key, param_string)`
//! 5. Base64 encode the signature
//! 6. URL-encode the base64 signature
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::tencent::signature::TencentSignatureBuilder;
//!
//! let builder = TencentSignatureBuilder::new("secret_id", "secret_key", "app_id")
//!     .with_engine_model("16k_zh")
//!     .with_voice_format(4)
//!     .with_voice_id("unique_voice_id");
//!
//! let (query_string, signature) = builder.build()?;
//! ```

use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use super::config::{MAX_NONCE, SIGNATURE_VALIDITY_SECS, VOICE_ID_LENGTH};

// =============================================================================
// Type Aliases
// =============================================================================

type HmacSha1 = Hmac<Sha1>;

// =============================================================================
// Error Types
// =============================================================================

/// Signature generation errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SignatureError {
    /// Invalid parameter value.
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    /// HMAC computation failed.
    #[error("HMAC computation failed: {0}")]
    HmacError(String),

    /// Missing required parameter.
    #[error("Missing required parameter: {0}")]
    MissingParameter(String),
}

// =============================================================================
// Signature Builder
// =============================================================================

/// Builder for Tencent Cloud ASR signatures.
///
/// Collects all required and optional parameters and generates the
/// HMAC-SHA1 signature for WebSocket authentication.
#[derive(Debug, Clone)]
pub struct TencentSignatureBuilder {
    /// Tencent Cloud Secret ID.
    secret_id: String,

    /// Tencent Cloud Secret Key (used for HMAC).
    secret_key: String,

    /// Request timestamp (Unix seconds).
    timestamp: u64,

    /// Signature expiration timestamp.
    expired: u64,

    /// Random nonce (positive integer, max 10 digits).
    nonce: u64,

    /// ASR engine model type (e.g., "16k_zh").
    engine_model_type: String,

    /// Unique voice/audio session ID (16 characters).
    voice_id: String,

    /// Audio format (1=PCM, 4=Speex, etc.).
    voice_format: u32,

    /// Enable VAD (0=off, 1=on).
    needvad: Option<u32>,

    /// Custom vocabulary ID.
    hotword_id: Option<String>,

    /// Temporary hotword list (base64-encoded JSON).
    hotword_list: Option<String>,

    /// Enable homophonic replacement (0=off, 1=on).
    reinforce_hotword: Option<u32>,

    /// Filter profanity (0=off, 1=filter, 2=replace).
    filter_dirty: Option<u32>,

    /// Filter modal particles (0=off, 1=partial, 2=strict).
    filter_modal: Option<u32>,

    /// Filter punctuation (0=off, 1=on).
    filter_punc: Option<u32>,

    /// Word timestamp level (0-2).
    word_info: Option<u32>,

    /// VAD silence threshold in milliseconds.
    vad_silence_time: Option<u32>,

    /// Maximum speak time before forced segmentation.
    max_speak_time: Option<u32>,

    /// Numeral conversion mode (0=Chinese, 1=Arabic, 3=Math).
    convert_num_mode: Option<u32>,

    /// Self-learning model customization ID.
    customization_id: Option<String>,

    /// Filter empty result callbacks (0=off, 1=on).
    filter_empty_result: Option<u32>,
}

impl TencentSignatureBuilder {
    /// Create a new signature builder with required credentials.
    pub fn new(secret_id: impl Into<String>, secret_key: impl Into<String>) -> Self {
        let now = Self::current_timestamp();

        Self {
            secret_id: secret_id.into(),
            secret_key: secret_key.into(),
            timestamp: now,
            expired: now + SIGNATURE_VALIDITY_SECS,
            nonce: Self::generate_nonce(),
            engine_model_type: "16k_zh".to_string(),
            voice_id: Self::generate_voice_id(),
            voice_format: 4, // Speex default
            needvad: None,
            hotword_id: None,
            hotword_list: None,
            reinforce_hotword: None,
            filter_dirty: None,
            filter_modal: None,
            filter_punc: None,
            word_info: None,
            vad_silence_time: None,
            max_speak_time: None,
            convert_num_mode: None,
            customization_id: None,
            filter_empty_result: None,
        }
    }

    /// Get current Unix timestamp in seconds.
    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Generate a random nonce (positive integer, max 10 digits).
    fn generate_nonce() -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        SystemTime::now().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        hasher.finish() % MAX_NONCE
    }

    /// Generate a unique voice ID (16 characters).
    pub fn generate_voice_id() -> String {
        let uuid = uuid::Uuid::new_v4();
        uuid.to_string().replace("-", "")[..VOICE_ID_LENGTH].to_string()
    }

    /// Set the engine model type.
    pub fn with_engine_model(mut self, model: impl Into<String>) -> Self {
        self.engine_model_type = model.into();
        self
    }

    /// Set the voice/audio format.
    pub fn with_voice_format(mut self, format: u32) -> Self {
        self.voice_format = format;
        self
    }

    /// Set a custom voice ID.
    pub fn with_voice_id(mut self, voice_id: impl Into<String>) -> Self {
        self.voice_id = voice_id.into();
        self
    }

    /// Set custom timestamp (for testing).
    pub fn with_timestamp(mut self, timestamp: u64) -> Self {
        self.timestamp = timestamp;
        self.expired = timestamp + SIGNATURE_VALIDITY_SECS;
        self
    }

    /// Set custom nonce (for testing).
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self
    }

    /// Set custom expiration time.
    pub fn with_expired(mut self, expired: u64) -> Self {
        self.expired = expired;
        self
    }

    /// Enable/disable VAD.
    pub fn with_needvad(mut self, enabled: bool) -> Self {
        self.needvad = Some(if enabled { 1 } else { 0 });
        self
    }

    /// Set custom vocabulary ID.
    pub fn with_hotword_id(mut self, id: impl Into<String>) -> Self {
        self.hotword_id = Some(id.into());
        self
    }

    /// Set profanity filter mode (0=off, 1=filter, 2=replace).
    pub fn with_filter_dirty(mut self, mode: u32) -> Self {
        self.filter_dirty = Some(mode.min(2));
        self
    }

    /// Set modal particle filter mode (0=off, 1=partial, 2=strict).
    pub fn with_filter_modal(mut self, mode: u32) -> Self {
        self.filter_modal = Some(mode.min(2));
        self
    }

    /// Enable/disable punctuation filter.
    pub fn with_filter_punc(mut self, enabled: bool) -> Self {
        self.filter_punc = Some(if enabled { 1 } else { 0 });
        self
    }

    /// Set word info level.
    pub fn with_word_info(mut self, level: u32) -> Self {
        self.word_info = Some(level.min(2));
        self
    }

    /// Set VAD silence time threshold.
    pub fn with_vad_silence_time(mut self, ms: u32) -> Self {
        self.vad_silence_time = Some(ms);
        self
    }

    /// Set temporary hotword list (base64-encoded JSON).
    pub fn with_hotword_list(mut self, list: &[String]) -> Self {
        if !list.is_empty() {
            // Format as JSON array and base64 encode
            let json = serde_json::json!(list);
            let encoded = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                json.to_string().as_bytes(),
            );
            self.hotword_list = Some(encoded);
        }
        self
    }

    /// Enable homophonic replacement (for 8k_zh and 16k_zh models).
    pub fn with_reinforce_hotword(mut self, enabled: bool) -> Self {
        self.reinforce_hotword = Some(if enabled { 1 } else { 0 });
        self
    }

    /// Set maximum speak time before forced segmentation.
    pub fn with_max_speak_time(mut self, ms: u32) -> Self {
        self.max_speak_time = Some(ms);
        self
    }

    /// Set numeral conversion mode (0=Chinese, 1=Arabic, 3=Math).
    pub fn with_convert_num_mode(mut self, mode: u32) -> Self {
        self.convert_num_mode = Some(mode);
        self
    }

    /// Set self-learning model customization ID.
    pub fn with_customization_id(mut self, id: impl Into<String>) -> Self {
        self.customization_id = Some(id.into());
        self
    }

    /// Enable filtering of empty result callbacks.
    pub fn with_filter_empty_result(mut self, enabled: bool) -> Self {
        self.filter_empty_result = Some(if enabled { 1 } else { 0 });
        self
    }

    /// Build the sorted parameter map.
    fn build_params(&self) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();

        // Required parameters
        params.insert("secretid".to_string(), self.secret_id.clone());
        params.insert("timestamp".to_string(), self.timestamp.to_string());
        params.insert("expired".to_string(), self.expired.to_string());
        params.insert("nonce".to_string(), self.nonce.to_string());
        params.insert(
            "engine_model_type".to_string(),
            self.engine_model_type.clone(),
        );
        params.insert("voice_id".to_string(), self.voice_id.clone());
        params.insert("voice_format".to_string(), self.voice_format.to_string());

        // Optional parameters
        if let Some(needvad) = self.needvad {
            params.insert("needvad".to_string(), needvad.to_string());
        }

        if let Some(ref hotword_id) = self.hotword_id {
            params.insert("hotword_id".to_string(), hotword_id.clone());
        }

        if let Some(filter_dirty) = self.filter_dirty {
            params.insert("filter_dirty".to_string(), filter_dirty.to_string());
        }

        if let Some(filter_modal) = self.filter_modal {
            params.insert("filter_modal".to_string(), filter_modal.to_string());
        }

        if let Some(filter_punc) = self.filter_punc {
            params.insert("filter_punc".to_string(), filter_punc.to_string());
        }

        if let Some(word_info) = self.word_info {
            params.insert("word_info".to_string(), word_info.to_string());
        }

        if let Some(vad_silence_time) = self.vad_silence_time {
            params.insert("vad_silence_time".to_string(), vad_silence_time.to_string());
        }

        if let Some(ref hotword_list) = self.hotword_list {
            params.insert("hotword_list".to_string(), hotword_list.clone());
        }

        if let Some(reinforce_hotword) = self.reinforce_hotword {
            params.insert(
                "reinforce_hotword".to_string(),
                reinforce_hotword.to_string(),
            );
        }

        if let Some(max_speak_time) = self.max_speak_time {
            params.insert("max_speak_time".to_string(), max_speak_time.to_string());
        }

        if let Some(convert_num_mode) = self.convert_num_mode {
            params.insert("convert_num_mode".to_string(), convert_num_mode.to_string());
        }

        if let Some(ref customization_id) = self.customization_id {
            params.insert("customization_id".to_string(), customization_id.clone());
        }

        if let Some(filter_empty_result) = self.filter_empty_result {
            params.insert(
                "filter_empty_result".to_string(),
                filter_empty_result.to_string(),
            );
        }

        params
    }

    /// Generate the parameter string for signature calculation.
    ///
    /// Parameters are sorted alphabetically and joined with `&`.
    fn build_param_string(params: &BTreeMap<String, String>) -> String {
        params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Calculate HMAC-SHA1 signature.
    fn calculate_signature(&self, param_string: &str) -> Result<String, SignatureError> {
        let mut mac = HmacSha1::new_from_slice(self.secret_key.as_bytes())
            .map_err(|e| SignatureError::HmacError(e.to_string()))?;

        mac.update(param_string.as_bytes());

        let result = mac.finalize();
        let signature_bytes = result.into_bytes();

        // Base64 encode
        let signature_base64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &signature_bytes);

        Ok(signature_base64)
    }

    /// URL-encode a string for query parameters.
    fn url_encode(s: &str) -> String {
        let mut encoded = String::with_capacity(s.len() * 3);
        for byte in s.bytes() {
            match byte {
                // Unreserved characters (RFC 3986)
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(byte as char);
                }
                // Everything else needs to be percent-encoded
                _ => {
                    encoded.push_str(&format!("%{:02X}", byte));
                }
            }
        }
        encoded
    }

    /// Build the complete query string with signature.
    ///
    /// Returns a tuple of (query_string, raw_signature).
    pub fn build(&self) -> Result<(String, String), SignatureError> {
        // Validate required fields
        if self.secret_id.is_empty() {
            return Err(SignatureError::MissingParameter("secret_id".to_string()));
        }
        if self.secret_key.is_empty() {
            return Err(SignatureError::MissingParameter("secret_key".to_string()));
        }
        if self.voice_id.is_empty() {
            return Err(SignatureError::MissingParameter("voice_id".to_string()));
        }

        // Build parameters
        let params = self.build_params();
        let param_string = Self::build_param_string(&params);

        // Calculate signature
        let signature = self.calculate_signature(&param_string)?;
        let signature_encoded = Self::url_encode(&signature);

        // Build query string with signature
        let query_string = format!("{}&signature={}", param_string, signature_encoded);

        Ok((query_string, signature))
    }

    /// Build the canonical Tencent ASR string-to-sign.
    ///
    /// Per the Tencent real-time ASR spec the signature is computed over the request URL
    /// WITHOUT the scheme: `asr.cloud.tencent.com/asr/v2/{app_id}?{sorted_params}` (the
    /// `signature` param itself is NOT included). Signing only the param string (the previous
    /// behaviour) is rejected server-side with an authentication failure, which made every
    /// Tencent STT connection fail.
    fn build_string_to_sign(base_url: &str, app_id: &str, param_string: &str) -> String {
        let host_path = base_url
            .trim_start_matches("wss://")
            .trim_start_matches("ws://")
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        format!("{}/{}?{}", host_path, app_id, param_string)
    }

    /// Build the complete WebSocket URL with app_id and a correctly-prefixed signature.
    pub fn build_url(&self, base_url: &str, app_id: &str) -> Result<String, SignatureError> {
        // Validate required fields.
        if self.secret_id.is_empty() {
            return Err(SignatureError::MissingParameter("secret_id".to_string()));
        }
        if self.secret_key.is_empty() {
            return Err(SignatureError::MissingParameter("secret_key".to_string()));
        }
        if self.voice_id.is_empty() {
            return Err(SignatureError::MissingParameter("voice_id".to_string()));
        }

        let params = self.build_params();
        let param_string = Self::build_param_string(&params);

        // Sign over host+path+app_id+params (NOT just the params — that is the bug fix).
        let string_to_sign = Self::build_string_to_sign(base_url, app_id, &param_string);
        let signature = self.calculate_signature(&string_to_sign)?;
        let signature_encoded = Self::url_encode(&signature);

        let query_string = format!("{}&signature={}", param_string, signature_encoded);
        Ok(format!("{}/{}?{}", base_url, app_id, query_string))
    }

    /// Get the voice ID being used.
    pub fn voice_id(&self) -> &str {
        &self.voice_id
    }

    /// Get the timestamp being used.
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Get the nonce being used.
    pub fn nonce(&self) -> u64 {
        self.nonce
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Builder Creation Tests
    // =========================================================================

    #[test]
    fn test_builder_creation() {
        let builder = TencentSignatureBuilder::new("secret_id", "secret_key");

        assert_eq!(builder.secret_id, "secret_id");
        assert_eq!(builder.secret_key, "secret_key");
        assert!(!builder.voice_id.is_empty());
        assert_eq!(builder.voice_id.len(), VOICE_ID_LENGTH);
        assert!(builder.timestamp > 0);
        assert!(builder.expired > builder.timestamp);
        assert!(builder.nonce > 0);
    }

    #[test]
    fn test_builder_with_methods() {
        let builder = TencentSignatureBuilder::new("id", "key")
            .with_engine_model("16k_en")
            .with_voice_format(10)
            .with_voice_id("custom_voice_id_")
            .with_timestamp(1234567890)
            .with_nonce(9876543210)
            .with_needvad(true)
            .with_filter_dirty(1) // Mode: 1=filter
            .with_filter_modal(0) // Mode: 0=off
            .with_filter_punc(true)
            .with_word_info(2)
            .with_vad_silence_time(500)
            .with_hotword_id("vocab123");

        assert_eq!(builder.engine_model_type, "16k_en");
        assert_eq!(builder.voice_format, 10);
        assert_eq!(builder.voice_id, "custom_voice_id_");
        assert_eq!(builder.timestamp, 1234567890);
        assert_eq!(builder.nonce, 9876543210);
        assert_eq!(builder.needvad, Some(1));
        assert_eq!(builder.filter_dirty, Some(1));
        assert_eq!(builder.filter_modal, Some(0));
        assert_eq!(builder.filter_punc, Some(1));
        assert_eq!(builder.word_info, Some(2));
        assert_eq!(builder.vad_silence_time, Some(500));
        assert_eq!(builder.hotword_id, Some("vocab123".to_string()));
    }

    #[test]
    fn test_builder_word_info_clamping() {
        let builder = TencentSignatureBuilder::new("id", "key").with_word_info(10); // Above max

        assert_eq!(builder.word_info, Some(2));
    }

    // =========================================================================
    // Voice ID Generation Tests
    // =========================================================================

    #[test]
    fn test_voice_id_generation() {
        let id1 = TencentSignatureBuilder::generate_voice_id();
        let id2 = TencentSignatureBuilder::generate_voice_id();

        assert_eq!(id1.len(), VOICE_ID_LENGTH);
        assert_eq!(id2.len(), VOICE_ID_LENGTH);
        // Should be unique (extremely unlikely to be equal)
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_voice_id_alphanumeric() {
        let id = TencentSignatureBuilder::generate_voice_id();
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // =========================================================================
    // Parameter Building Tests
    // =========================================================================

    #[test]
    fn test_build_params_sorted() {
        let builder = TencentSignatureBuilder::new("my_secret_id", "my_secret_key")
            .with_engine_model("16k_zh")
            .with_voice_format(4)
            .with_voice_id("test_voice_id_ab")
            .with_timestamp(1609459200)
            .with_nonce(123456789);

        let params = builder.build_params();

        // BTreeMap should be sorted
        let keys: Vec<_> = params.keys().collect();
        let mut sorted_keys = keys.clone();
        sorted_keys.sort();
        assert_eq!(keys, sorted_keys);

        // Check values
        assert_eq!(params.get("secretid").unwrap(), "my_secret_id");
        assert_eq!(params.get("timestamp").unwrap(), "1609459200");
        assert_eq!(params.get("nonce").unwrap(), "123456789");
        assert_eq!(params.get("engine_model_type").unwrap(), "16k_zh");
        assert_eq!(params.get("voice_format").unwrap(), "4");
    }

    #[test]
    fn test_build_param_string() {
        let mut params = BTreeMap::new();
        params.insert("a_param".to_string(), "value_a".to_string());
        params.insert("b_param".to_string(), "value_b".to_string());
        params.insert("c_param".to_string(), "value_c".to_string());

        let param_string = TencentSignatureBuilder::build_param_string(&params);

        assert_eq!(
            param_string,
            "a_param=value_a&b_param=value_b&c_param=value_c"
        );
    }

    #[test]
    fn test_build_params_with_optional() {
        let builder = TencentSignatureBuilder::new("id", "key")
            .with_voice_id("test_voice_id_ab")
            .with_needvad(true)
            .with_filter_dirty(1) // Mode: 1=filter
            .with_hotword_id("vocab123");

        let params = builder.build_params();

        assert!(params.contains_key("needvad"));
        assert!(params.contains_key("filter_dirty"));
        assert!(params.contains_key("hotword_id"));
        assert_eq!(params.get("needvad").unwrap(), "1");
        assert_eq!(params.get("filter_dirty").unwrap(), "1");
        assert_eq!(params.get("hotword_id").unwrap(), "vocab123");
    }

    // =========================================================================
    // URL Encoding Tests
    // =========================================================================

    #[test]
    fn test_url_encode_alphanumeric() {
        assert_eq!(TencentSignatureBuilder::url_encode("abc123"), "abc123");
        assert_eq!(TencentSignatureBuilder::url_encode("ABC"), "ABC");
    }

    #[test]
    fn test_url_encode_special_chars() {
        assert_eq!(TencentSignatureBuilder::url_encode("a+b"), "a%2Bb");
        assert_eq!(TencentSignatureBuilder::url_encode("a/b"), "a%2Fb");
        assert_eq!(TencentSignatureBuilder::url_encode("a=b"), "a%3Db");
        assert_eq!(TencentSignatureBuilder::url_encode("a&b"), "a%26b");
    }

    #[test]
    fn test_url_encode_unreserved() {
        // RFC 3986 unreserved characters should not be encoded
        assert_eq!(TencentSignatureBuilder::url_encode("-_.~"), "-_.~");
    }

    #[test]
    fn test_url_encode_base64_chars() {
        // Base64 alphabet includes + and / which must be URL-encoded
        // Using a raw string with these chars to verify encoding
        let sample_with_special = "test+value/path=end";
        let encoded = TencentSignatureBuilder::url_encode(sample_with_special);
        assert!(encoded.contains("%2B")); // + -> %2B
        assert!(encoded.contains("%2F")); // / -> %2F
        assert!(encoded.contains("%3D")); // = -> %3D
        assert_eq!(encoded, "test%2Bvalue%2Fpath%3Dend");
    }

    // =========================================================================
    // Signature Calculation Tests
    // =========================================================================

    #[test]
    fn test_signature_calculation_deterministic() {
        let builder = TencentSignatureBuilder::new("test_secret_id", "test_secret_key")
            .with_voice_id("fixed_voice_id_a")
            .with_timestamp(1609459200)
            .with_nonce(123456789)
            .with_engine_model("16k_zh")
            .with_voice_format(4);

        let params = builder.build_params();
        let param_string = TencentSignatureBuilder::build_param_string(&params);

        let sig1 = builder.calculate_signature(&param_string).unwrap();
        let sig2 = builder.calculate_signature(&param_string).unwrap();

        // Same input should produce same signature
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_signature_is_base64() {
        let builder = TencentSignatureBuilder::new("id", "key").with_voice_id("test_voice_id_ab");

        let params = builder.build_params();
        let param_string = TencentSignatureBuilder::build_param_string(&params);
        let signature = builder.calculate_signature(&param_string).unwrap();

        // Should be valid base64
        let decoded =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &signature);
        assert!(decoded.is_ok());

        // HMAC-SHA1 produces 20 bytes
        assert_eq!(decoded.unwrap().len(), 20);
    }

    #[test]
    fn test_signature_changes_with_different_key() {
        let builder1 = TencentSignatureBuilder::new("id", "key1")
            .with_voice_id("test_voice_id_ab")
            .with_timestamp(1609459200)
            .with_nonce(123456789);

        let builder2 = TencentSignatureBuilder::new("id", "key2")
            .with_voice_id("test_voice_id_ab")
            .with_timestamp(1609459200)
            .with_nonce(123456789);

        let params1 = builder1.build_params();
        let params2 = builder2.build_params();
        let ps1 = TencentSignatureBuilder::build_param_string(&params1);
        let ps2 = TencentSignatureBuilder::build_param_string(&params2);

        let sig1 = builder1.calculate_signature(&ps1).unwrap();
        let sig2 = builder2.calculate_signature(&ps2).unwrap();

        assert_ne!(sig1, sig2);
    }

    // =========================================================================
    // Build Tests
    // =========================================================================

    #[test]
    fn test_build_success() {
        let builder = TencentSignatureBuilder::new("test_id", "test_key")
            .with_voice_id("test_voice_id_ab")
            .with_timestamp(1609459200)
            .with_nonce(123456789);

        let result = builder.build();
        assert!(result.is_ok());

        let (query_string, signature) = result.unwrap();

        // Query string should contain all params
        assert!(query_string.contains("secretid=test_id"));
        assert!(query_string.contains("timestamp=1609459200"));
        assert!(query_string.contains("nonce=123456789"));
        assert!(query_string.contains("signature="));

        // Signature should be base64
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_build_missing_secret_id() {
        let builder = TencentSignatureBuilder::new("", "key");
        let result = builder.build();
        assert!(matches!(result, Err(SignatureError::MissingParameter(_))));
    }

    #[test]
    fn test_build_missing_secret_key() {
        let builder = TencentSignatureBuilder::new("id", "");
        let result = builder.build();
        assert!(matches!(result, Err(SignatureError::MissingParameter(_))));
    }

    #[test]
    fn test_build_missing_voice_id() {
        let mut builder = TencentSignatureBuilder::new("id", "key");
        builder.voice_id = String::new();
        let result = builder.build();
        assert!(matches!(result, Err(SignatureError::MissingParameter(_))));
    }

    // =========================================================================
    // Build URL Tests
    // =========================================================================

    #[test]
    fn test_build_url_success() {
        let builder = TencentSignatureBuilder::new("test_id", "test_key")
            .with_voice_id("test_voice_id_ab")
            .with_timestamp(1609459200)
            .with_nonce(123456789);

        let url = builder
            .build_url("wss://asr.cloud.tencent.com/asr/v2", "12345678")
            .unwrap();

        assert!(url.starts_with("wss://asr.cloud.tencent.com/asr/v2/12345678?"));
        assert!(url.contains("secretid=test_id"));
        assert!(url.contains("signature="));
    }

    // Regression: the Tencent ASR signature MUST be computed over the scheme-less
    // host+path+app_id+params, not the bare param string. Signing params-only was rejected
    // server-side, making every Tencent STT connection fail.
    #[test]
    fn test_signature_includes_host_path_prefix() {
        let builder = TencentSignatureBuilder::new("test_id", "test_key")
            .with_voice_id("test_voice_id_ab")
            .with_timestamp(1609459200)
            .with_nonce(123456789);

        let params = builder.build_params();
        let param_string = TencentSignatureBuilder::build_param_string(&params);

        // Canonical string-to-sign has no scheme and is prefixed with host/path/app_id.
        let sts = TencentSignatureBuilder::build_string_to_sign(
            "wss://asr.cloud.tencent.com/asr/v2",
            "12345678",
            &param_string,
        );
        assert_eq!(
            sts,
            format!("asr.cloud.tencent.com/asr/v2/12345678?{param_string}")
        );

        // The signature in the URL must be over the prefixed string, not params-only.
        let sig_correct = builder.calculate_signature(&sts).unwrap();
        let sig_params_only = builder.calculate_signature(&param_string).unwrap();
        assert_ne!(
            sig_correct, sig_params_only,
            "signature must incorporate the host+path prefix"
        );

        let url = builder
            .build_url("wss://asr.cloud.tencent.com/asr/v2", "12345678")
            .unwrap();
        assert!(url.contains(&TencentSignatureBuilder::url_encode(&sig_correct)));
    }

    // =========================================================================
    // Accessor Tests
    // =========================================================================

    #[test]
    fn test_accessors() {
        let builder = TencentSignatureBuilder::new("id", "key")
            .with_voice_id("custom_voice_id_")
            .with_timestamp(1234567890)
            .with_nonce(9876543210);

        assert_eq!(builder.voice_id(), "custom_voice_id_");
        assert_eq!(builder.timestamp(), 1234567890);
        assert_eq!(builder.nonce(), 9876543210);
    }

    // =========================================================================
    // Error Type Tests
    // =========================================================================

    #[test]
    fn test_error_display() {
        let err = SignatureError::InvalidParameter("test param".to_string());
        assert!(err.to_string().contains("test param"));

        let err = SignatureError::HmacError("hmac failed".to_string());
        assert!(err.to_string().contains("hmac failed"));

        let err = SignatureError::MissingParameter("secret_id".to_string());
        assert!(err.to_string().contains("secret_id"));
    }
}

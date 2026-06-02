use bytes::Bytes;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// =============================================================================
// Rich Metadata Types for STT Results
// =============================================================================

/// Word-level timing information with optional speaker assignment.
///
/// Provides precise timing for each word in the transcription, enabling
/// features like word-level highlighting, karaoke-style display, and
/// speaker-attributed transcripts.
#[derive(Debug, Clone, PartialEq)]
pub struct WordTiming {
    /// The transcribed word text.
    pub word: String,
    /// Start time in seconds from the beginning of the audio.
    pub start: f64,
    /// End time in seconds from the beginning of the audio.
    pub end: f64,
    /// Confidence score for this specific word (0.0 to 1.0).
    pub confidence: Option<f32>,
    /// Speaker identifier if diarization is enabled.
    /// Format varies by provider (e.g., "speaker_0", "SPEAKER_1", numeric).
    pub speaker_id: Option<String>,
    /// Log probability of the word (for providers that support it).
    /// More negative values indicate lower confidence.
    pub logprob: Option<f64>,
}

impl WordTiming {
    /// Create a new WordTiming with minimal required fields.
    pub fn new(word: String, start: f64, end: f64) -> Self {
        Self {
            word,
            start,
            end,
            confidence: None,
            speaker_id: None,
            logprob: None,
        }
    }

    /// Create WordTiming with all fields populated.
    pub fn with_all(
        word: String,
        start: f64,
        end: f64,
        confidence: Option<f32>,
        speaker_id: Option<String>,
        logprob: Option<f64>,
    ) -> Self {
        Self {
            word,
            start,
            end,
            confidence,
            speaker_id,
            logprob,
        }
    }

    /// Duration of this word in seconds.
    #[inline]
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

/// Speaker information from diarization.
///
/// Contains metadata about identified speakers in the audio.
/// Populated when diarization is enabled in the STT provider.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerInfo {
    /// Unique identifier for this speaker within the session.
    pub speaker_id: String,
    /// Optional human-readable name (if known speaker recognition is used).
    pub name: Option<String>,
    /// Confidence score for speaker identification (0.0 to 1.0).
    pub confidence: Option<f32>,
    /// Total speaking time for this speaker in seconds.
    pub total_speaking_time: Option<f64>,
}

impl SpeakerInfo {
    /// Create a new SpeakerInfo with just the ID.
    pub fn new(speaker_id: String) -> Self {
        Self {
            speaker_id,
            name: None,
            confidence: None,
            total_speaking_time: None,
        }
    }

    /// Create SpeakerInfo with known speaker name.
    pub fn with_name(speaker_id: String, name: String, confidence: Option<f32>) -> Self {
        Self {
            speaker_id,
            name: Some(name),
            confidence,
            total_speaking_time: None,
        }
    }
}

/// Detected entity from entity detection/recognition.
///
/// Represents named entities, PII, PHI, or other structured information
/// detected in the transcription.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedEntity {
    /// The entity text as it appears in the transcript.
    pub text: String,
    /// Entity category/type (e.g., "person_name", "phone_number", "medical_condition").
    /// Category names vary by provider.
    pub category: String,
    /// Start character offset in the transcript (0-indexed).
    pub start_offset: Option<usize>,
    /// End character offset in the transcript (exclusive, 0-indexed).
    pub end_offset: Option<usize>,
    /// Confidence score for entity detection (0.0 to 1.0).
    pub confidence: Option<f32>,
    /// Additional metadata (provider-specific).
    /// Examples: subcategory, normalized value, entity ID.
    pub metadata: Option<std::collections::HashMap<String, String>>,
}

impl DetectedEntity {
    /// Create a new DetectedEntity with minimal required fields.
    pub fn new(text: String, category: String) -> Self {
        Self {
            text,
            category,
            start_offset: None,
            end_offset: None,
            confidence: None,
            metadata: None,
        }
    }

    /// Create DetectedEntity with character offsets.
    pub fn with_offsets(
        text: String,
        category: String,
        start_offset: usize,
        end_offset: usize,
        confidence: Option<f32>,
    ) -> Self {
        Self {
            text,
            category,
            start_offset: Some(start_offset),
            end_offset: Some(end_offset),
            confidence,
            metadata: None,
        }
    }
}

/// Sensitive data detection result (PII/PHI).
///
/// Contains information about detected sensitive data that may have been
/// redacted from the transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct SensitiveDataItem {
    /// Type of sensitive data (e.g., "pii", "phi", "ssn", "credit_card").
    pub data_type: String,
    /// The detected sensitive text (may be redacted in actual value).
    pub original_text: Option<String>,
    /// The redaction placeholder used (e.g., "[SSN]", "***").
    pub redacted_text: Option<String>,
    /// Start character offset in the original transcript.
    pub start_offset: Option<usize>,
    /// End character offset in the original transcript.
    pub end_offset: Option<usize>,
    /// Confidence score for detection (0.0 to 1.0).
    pub confidence: Option<f32>,
}

impl SensitiveDataItem {
    /// Create a new SensitiveDataItem.
    pub fn new(data_type: String) -> Self {
        Self {
            data_type,
            original_text: None,
            redacted_text: None,
            start_offset: None,
            end_offset: None,
            confidence: None,
        }
    }
}

// =============================================================================
// Main STTResult Structure
// =============================================================================

/// Result structure containing transcription data from STT providers.
///
/// This structure supports both basic transcription results and rich metadata
/// including word-level timestamps, speaker diarization, entity detection,
/// and PII/PHI redaction.
///
/// # Backward Compatibility
///
/// All metadata fields are optional (`Option<T>`), ensuring backward
/// compatibility with existing code. Providers that don't support certain
/// features will leave those fields as `None`.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::STTResult;
///
/// // Basic result (backward compatible)
/// let basic = STTResult::new("Hello world".to_string(), true, true, 0.95);
///
/// // Rich result with word timing
/// let mut rich = STTResult::new("Hello world".to_string(), true, true, 0.95);
/// rich.words = Some(vec![
///     WordTiming::new("Hello".to_string(), 0.0, 0.5),
///     WordTiming::new("world".to_string(), 0.6, 1.0),
/// ]);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct STTResult {
    /// The transcribed text from the audio.
    pub transcript: String,
    /// Whether this is a final transcription result (not an interim result).
    pub is_final: bool,
    /// Whether this marks the end of a speech segment.
    pub is_speech_final: bool,
    /// Confidence score of the transcription (0.0 to 1.0).
    pub confidence: f32,

    // =========================================================================
    // Rich Metadata Fields (Optional for backward compatibility)
    // =========================================================================
    /// Word-level timing and speaker information.
    ///
    /// Populated when word timestamps are requested and supported by the provider.
    /// Each word includes start/end times and optionally speaker assignment.
    pub words: Option<Vec<WordTiming>>,

    /// Speaker information from diarization.
    ///
    /// Contains metadata about each identified speaker when diarization is enabled.
    /// Speakers are typically identified as "speaker_0", "speaker_1", etc.
    pub speakers: Option<Vec<SpeakerInfo>>,

    /// Detected entities from entity recognition.
    ///
    /// Includes named entities, PII, PHI, and other structured information
    /// detected in the transcript. Categories vary by provider.
    pub entities: Option<Vec<DetectedEntity>>,

    /// Detected sensitive data items (PII/PHI).
    ///
    /// Separate from entities for providers that distinguish between
    /// general entity detection and sensitive data detection.
    pub sensitive_data: Option<Vec<SensitiveDataItem>>,

    /// Redacted version of the transcript.
    ///
    /// Contains the transcript with sensitive information replaced with
    /// redaction placeholders. Only populated when redaction is enabled.
    pub redacted_transcript: Option<String>,

    /// Average log probability of the transcription.
    ///
    /// Used by some providers as a confidence metric. More negative values
    /// indicate lower confidence. Typically in range [-5, 0] for good quality.
    pub logprobs: Option<f64>,

    /// Detected language code (ISO 639-1, e.g., "en", "es").
    ///
    /// Populated when automatic language detection is enabled.
    pub detected_language: Option<String>,

    /// Duration of the audio segment that produced this result (in seconds).
    pub audio_duration: Option<f64>,
}

impl STTResult {
    /// Creates a new STTResult with basic fields.
    ///
    /// All optional metadata fields are initialized to `None` for backward compatibility.
    ///
    /// # Arguments
    ///
    /// * `transcript` - The transcribed text
    /// * `is_final` - Whether this is a final result (not interim)
    /// * `is_speech_final` - Whether this marks end of speech segment
    /// * `confidence` - Confidence score (will be clamped to 0.0-1.0)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let result = STTResult::new("Hello world".to_string(), true, true, 0.95);
    /// assert!(result.words.is_none()); // Metadata fields default to None
    /// ```
    pub fn new(transcript: String, is_final: bool, is_speech_final: bool, confidence: f32) -> Self {
        Self {
            transcript,
            is_final,
            is_speech_final,
            confidence: confidence.clamp(0.0, 1.0),
            // Initialize all optional metadata fields to None for backward compatibility
            words: None,
            speakers: None,
            entities: None,
            sensitive_data: None,
            redacted_transcript: None,
            logprobs: None,
            detected_language: None,
            audio_duration: None,
        }
    }

    /// Creates a new STTResult with all fields specified.
    ///
    /// Use this constructor when you have rich metadata to include.
    #[allow(clippy::too_many_arguments)]
    pub fn with_metadata(
        transcript: String,
        is_final: bool,
        is_speech_final: bool,
        confidence: f32,
        words: Option<Vec<WordTiming>>,
        speakers: Option<Vec<SpeakerInfo>>,
        entities: Option<Vec<DetectedEntity>>,
        sensitive_data: Option<Vec<SensitiveDataItem>>,
        redacted_transcript: Option<String>,
        logprobs: Option<f64>,
        detected_language: Option<String>,
        audio_duration: Option<f64>,
    ) -> Self {
        Self {
            transcript,
            is_final,
            is_speech_final,
            confidence: confidence.clamp(0.0, 1.0),
            words,
            speakers,
            entities,
            sensitive_data,
            redacted_transcript,
            logprobs,
            detected_language,
            audio_duration,
        }
    }

    /// Returns the total duration of words if word timing is available.
    pub fn word_duration(&self) -> Option<f64> {
        self.words.as_ref().and_then(|words| {
            if words.is_empty() {
                return None;
            }
            let first = words.first()?;
            let last = words.last()?;
            Some(last.end - first.start)
        })
    }

    /// Returns the number of unique speakers if diarization data is available.
    pub fn speaker_count(&self) -> Option<usize> {
        self.speakers.as_ref().map(|s| s.len())
    }

    /// Returns words spoken by a specific speaker.
    pub fn words_by_speaker(&self, speaker_id: &str) -> Vec<&WordTiming> {
        self.words
            .as_ref()
            .map(|words| {
                words
                    .iter()
                    .filter(|w| w.speaker_id.as_deref() == Some(speaker_id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns entities of a specific category.
    pub fn entities_by_category(&self, category: &str) -> Vec<&DetectedEntity> {
        self.entities
            .as_ref()
            .map(|entities| entities.iter().filter(|e| e.category == category).collect())
            .unwrap_or_default()
    }

    /// Check if this result contains any sensitive data.
    pub fn has_sensitive_data(&self) -> bool {
        self.sensitive_data
            .as_ref()
            .map(|sd| !sd.is_empty())
            .unwrap_or(false)
    }

    /// Check if this result has word-level timing.
    pub fn has_word_timing(&self) -> bool {
        self.words.as_ref().map(|w| !w.is_empty()).unwrap_or(false)
    }

    /// Check if this result has speaker diarization.
    pub fn has_diarization(&self) -> bool {
        self.speakers
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }
}

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
    /// Encoding of the audio
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

/// Error types for STT operations
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

/// Type alias for STT result callback
pub type STTResultCallback =
    Arc<dyn Fn(STTResult) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Type alias for STT error callback
pub type STTErrorCallback =
    Arc<dyn Fn(STTError) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Base trait for Speech-to-Text providers
#[async_trait::async_trait]
pub trait BaseSTT: Send + Sync {
    /// Create a new instance of the STT provider with the given configuration
    ///
    /// # Arguments
    /// * `config` - Configuration for the STT provider
    ///
    /// # Returns
    /// * `Result<Self, STTError>` - New instance or error
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized;

    /// Connect to the STT provider
    ///
    /// # Returns
    /// * `Result<(), STTError>` - Success or error
    async fn connect(&mut self) -> Result<(), STTError>;

    /// Disconnect from the STT provider
    ///
    /// # Returns
    /// * `Result<(), STTError>` - Success or error
    async fn disconnect(&mut self) -> Result<(), STTError>;

    /// Check if the connection is ready to be used
    ///
    /// # Returns
    /// * `bool` - True if ready, false otherwise
    fn is_ready(&self) -> bool;

    /// Send audio data to the STT provider for transcription
    ///
    /// # Arguments
    /// * `audio_data` - Audio bytes to process (zero-copy via Bytes)
    ///
    /// # Returns
    /// * `Result<(), STTError>` - Success or error
    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError>;

    /// Register a callback function that gets triggered when transcription results are available
    ///
    /// # Arguments
    /// * `callback` - Callback function to handle STT results
    ///
    /// # Returns
    /// * `Result<(), STTError>` - Success or error
    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError>;

    /// Register a callback function that gets triggered when errors occur during streaming
    ///
    /// This is critical for propagating streaming errors (e.g., permission denied, rate limits)
    /// that occur after the initial connection is established.
    ///
    /// # Arguments
    /// * `callback` - Callback function to handle STT errors
    ///
    /// # Returns
    /// * `Result<(), STTError>` - Success or error
    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError>;

    /// Get the current configuration
    fn get_config(&self) -> Option<&STTConfig>;

    /// Update configuration while maintaining connection
    ///
    /// # Arguments
    /// * `config` - New configuration
    ///
    /// # Returns
    /// * `Result<(), STTError>` - Success or error
    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError>;

    /// Get provider-specific information
    fn get_provider_info(&self) -> &'static str;

    /// Inject the shared, process-global resilience handles (W-D2): the single reconnect
    /// governor (storm control across all sessions) and this provider's shared circuit breaker
    /// (a trip in one session is visible to every other session of the provider).
    ///
    /// Streaming providers that run a supervised reconnect loop (Deepgram, AssemblyAI, …)
    /// override this to store the handles and use them in their connect path instead of
    /// creating per-session ones. Non-streaming / one-shot providers use the default no-op.
    fn set_resilience(&mut self, _resilience: crate::core::resilience::ResilienceHandles) {}
}

/// Factory trait for creating STT providers
pub trait STTFactory {
    /// Create a new STT provider instance
    fn create_stt() -> Box<dyn BaseSTT>;
}

/// Helper trait for common STT operations
pub trait STTHelper {
    /// Validate audio format
    fn validate_audio_format(&self, sample_rate: u32, channels: u16) -> Result<(), STTError>;

    /// Convert audio to required format
    fn convert_audio_format(
        &self,
        audio_data: Vec<u8>,
        target_format: &str,
    ) -> Result<Vec<u8>, STTError>;
}

/// Connection state for STT providers
#[derive(Debug, Clone, PartialEq)]
pub enum STTConnectionState {
    /// Not connected
    Disconnected,
    /// In the process of connecting
    Connecting,
    /// Connected and ready to receive audio
    Connected,
    /// Error state
    Error(String),
}

/// Statistics for STT operations
#[derive(Debug, Default, Clone)]
pub struct STTStats {
    /// Total audio bytes processed
    pub total_audio_bytes: u64,
    /// Number of transcription results received
    pub results_count: u32,
    /// Number of final results received
    pub final_results_count: u32,
    /// Average confidence score
    pub average_confidence: f32,
    /// Connection uptime in seconds
    pub uptime_seconds: u64,
}

impl STTStats {
    /// Update statistics with a new result
    pub fn update_with_result(&mut self, result: &STTResult) {
        self.results_count += 1;
        if result.is_final {
            self.final_results_count += 1;
        }

        // Update average confidence
        let total_confidence =
            self.average_confidence * (self.results_count - 1) as f32 + result.confidence;
        self.average_confidence = total_confidence / self.results_count as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Mock implementation for testing
    struct MockSTT {
        config: Option<STTConfig>,
        connected: AtomicBool,
        callback: Option<STTResultCallback>,
    }

    #[async_trait::async_trait]
    impl BaseSTT for MockSTT {
        fn new(config: STTConfig) -> Result<Self, STTError> {
            Ok(Self {
                config: Some(config),
                connected: AtomicBool::new(false),
                callback: None,
            })
        }

        async fn connect(&mut self) -> Result<(), STTError> {
            self.connected.store(true, Ordering::Relaxed);
            Ok(())
        }

        async fn disconnect(&mut self) -> Result<(), STTError> {
            self.connected.store(false, Ordering::Relaxed);
            Ok(())
        }

        fn is_ready(&self) -> bool {
            self.connected.load(Ordering::Relaxed)
        }

        async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
            if !self.is_ready() {
                return Err(STTError::ConnectionFailed("Not connected".to_string()));
            }

            // Mock processing - simulate transcription result
            if let Some(ref callback) = self.callback {
                let result = STTResult::new(
                    format!("Transcribed {} bytes of audio", audio_data.len()),
                    true,
                    true,
                    0.95,
                );
                callback(result).await;
            }

            Ok(())
        }

        async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
            self.callback = Some(callback);
            Ok(())
        }

        async fn on_error(&mut self, _callback: STTErrorCallback) -> Result<(), STTError> {
            // Mock implementation - errors not simulated
            Ok(())
        }

        fn get_config(&self) -> Option<&STTConfig> {
            self.config.as_ref()
        }

        async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
            if self.is_ready() {
                self.config = Some(config);
                Ok(())
            } else {
                Err(STTError::ConnectionFailed("Not connected".to_string()))
            }
        }

        fn get_provider_info(&self) -> &'static str {
            "MockSTT v1.0"
        }
    }

    #[tokio::test]
    async fn test_stt_new_function() {
        let config = STTConfig {
            model: "nova-3".to_string(),
            provider: "mock".to_string(),
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
        };

        let stt = MockSTT::new(config.clone()).unwrap();

        // Should have config set but not be connected
        assert!(stt.get_config().is_some());
        assert!(!stt.is_ready());

        // Config should match what we passed
        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, "test_key");
        assert_eq!(stored_config.language, "en-US");
        assert_eq!(stored_config.sample_rate, 16000);
        assert_eq!(stored_config.channels, 1);
        assert!(stored_config.punctuation);
    }

    #[tokio::test]
    async fn test_mock_stt_implementation() {
        // Test creation with config
        let config = STTConfig::default();
        let mut stt = MockSTT::new(config.clone()).unwrap();

        // Test initial state - should not be connected yet
        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());

        // Test connection
        stt.connect().await.unwrap();
        assert!(stt.is_ready());
        assert!(stt.get_config().is_some());

        // Test callback registration - simplified for testing
        let callback = Arc::new(|result: STTResult| {
            Box::pin(async move {
                println!("Received result: {result:?}");
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        stt.on_result(callback).await.unwrap();

        // Test audio processing
        let audio_data: Bytes = vec![0u8; 1024].into();
        stt.send_audio(audio_data).await.unwrap();

        // Give some time for async callback to execute
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Test disconnect
        stt.disconnect().await.unwrap();
        assert!(!stt.is_ready());

        // Test provider info
        assert_eq!(stt.get_provider_info(), "MockSTT v1.0");
    }

    #[test]
    fn test_stt_result_creation() {
        let result = STTResult::new("Hello world".to_string(), true, true, 0.95);
        assert_eq!(result.transcript, "Hello world");
        assert!(result.is_final);
        assert!(result.is_speech_final);
        assert_eq!(result.confidence, 0.95);
    }

    #[test]
    fn test_stt_result_confidence_clamping() {
        let result = STTResult::new("Test".to_string(), true, false, 1.5);
        assert_eq!(result.confidence, 1.0);

        let result = STTResult::new("Test".to_string(), true, false, -0.5);
        assert_eq!(result.confidence, 0.0);
    }

    #[test]
    fn test_stt_config_default() {
        let config = STTConfig::default();
        assert_eq!(config.language, "en-US");
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
        assert!(config.punctuation);
    }

    #[test]
    fn test_stt_stats_update() {
        let mut stats = STTStats::default();
        let result = STTResult::new("Test".to_string(), true, false, 0.8);

        stats.update_with_result(&result);

        assert_eq!(stats.results_count, 1);
        assert_eq!(stats.final_results_count, 1);
        assert_eq!(stats.average_confidence, 0.8);
    }

    #[test]
    fn test_stt_connection_states() {
        let disconnected = STTConnectionState::Disconnected;
        let connecting = STTConnectionState::Connecting;
        let connected = STTConnectionState::Connected;
        let error = STTConnectionState::Error("Test error".to_string());

        assert_eq!(disconnected, STTConnectionState::Disconnected);
        assert_eq!(connecting, STTConnectionState::Connecting);
        assert_eq!(connected, STTConnectionState::Connected);
        assert_eq!(error, STTConnectionState::Error("Test error".to_string()));
    }

    // =========================================================================
    // Tests for Rich Metadata Types
    // =========================================================================

    #[test]
    fn test_word_timing_new() {
        let word = WordTiming::new("hello".to_string(), 0.0, 0.5);
        assert_eq!(word.word, "hello");
        assert_eq!(word.start, 0.0);
        assert_eq!(word.end, 0.5);
        assert!(word.confidence.is_none());
        assert!(word.speaker_id.is_none());
        assert!(word.logprob.is_none());
    }

    #[test]
    fn test_word_timing_with_all() {
        let word = WordTiming::with_all(
            "world".to_string(),
            0.6,
            1.1,
            Some(0.95),
            Some("speaker_0".to_string()),
            Some(-0.5),
        );
        assert_eq!(word.word, "world");
        assert_eq!(word.start, 0.6);
        assert_eq!(word.end, 1.1);
        assert_eq!(word.confidence, Some(0.95));
        assert_eq!(word.speaker_id, Some("speaker_0".to_string()));
        assert_eq!(word.logprob, Some(-0.5));
    }

    #[test]
    fn test_word_timing_duration() {
        let word = WordTiming::new("test".to_string(), 1.5, 2.3);
        let duration = word.duration();
        assert!((duration - 0.8).abs() < 1e-10);
    }

    #[test]
    fn test_speaker_info_new() {
        let speaker = SpeakerInfo::new("speaker_0".to_string());
        assert_eq!(speaker.speaker_id, "speaker_0");
        assert!(speaker.name.is_none());
        assert!(speaker.confidence.is_none());
        assert!(speaker.total_speaking_time.is_none());
    }

    #[test]
    fn test_speaker_info_with_name() {
        let speaker =
            SpeakerInfo::with_name("speaker_1".to_string(), "John Doe".to_string(), Some(0.87));
        assert_eq!(speaker.speaker_id, "speaker_1");
        assert_eq!(speaker.name, Some("John Doe".to_string()));
        assert_eq!(speaker.confidence, Some(0.87));
        assert!(speaker.total_speaking_time.is_none());
    }

    #[test]
    fn test_detected_entity_new() {
        let entity = DetectedEntity::new("John".to_string(), "person_name".to_string());
        assert_eq!(entity.text, "John");
        assert_eq!(entity.category, "person_name");
        assert!(entity.start_offset.is_none());
        assert!(entity.end_offset.is_none());
        assert!(entity.confidence.is_none());
        assert!(entity.metadata.is_none());
    }

    #[test]
    fn test_detected_entity_with_offsets() {
        let entity = DetectedEntity::with_offsets(
            "555-1234".to_string(),
            "phone_number".to_string(),
            10,
            18,
            Some(0.99),
        );
        assert_eq!(entity.text, "555-1234");
        assert_eq!(entity.category, "phone_number");
        assert_eq!(entity.start_offset, Some(10));
        assert_eq!(entity.end_offset, Some(18));
        assert_eq!(entity.confidence, Some(0.99));
        assert!(entity.metadata.is_none());
    }

    #[test]
    fn test_sensitive_data_item_new() {
        let item = SensitiveDataItem::new("ssn".to_string());
        assert_eq!(item.data_type, "ssn");
        assert!(item.original_text.is_none());
        assert!(item.redacted_text.is_none());
        assert!(item.start_offset.is_none());
        assert!(item.end_offset.is_none());
        assert!(item.confidence.is_none());
    }

    #[test]
    fn test_stt_result_new_initializes_optional_fields() {
        let result = STTResult::new("Test".to_string(), true, true, 0.9);

        // All optional fields should be None
        assert!(result.words.is_none());
        assert!(result.speakers.is_none());
        assert!(result.entities.is_none());
        assert!(result.sensitive_data.is_none());
        assert!(result.redacted_transcript.is_none());
        assert!(result.logprobs.is_none());
        assert!(result.detected_language.is_none());
        assert!(result.audio_duration.is_none());
    }

    #[test]
    fn test_stt_result_with_metadata() {
        let words = vec![
            WordTiming::new("Hello".to_string(), 0.0, 0.5),
            WordTiming::new("world".to_string(), 0.6, 1.0),
        ];
        let speakers = vec![SpeakerInfo::new("speaker_0".to_string())];
        let entities = vec![DetectedEntity::new(
            "world".to_string(),
            "common_noun".to_string(),
        )];
        let sensitive_data = vec![SensitiveDataItem::new("pii".to_string())];

        let result = STTResult::with_metadata(
            "Hello world".to_string(),
            true,
            true,
            0.95,
            Some(words.clone()),
            Some(speakers.clone()),
            Some(entities.clone()),
            Some(sensitive_data.clone()),
            Some("[REDACTED] world".to_string()),
            Some(-0.5),
            Some("en".to_string()),
            Some(1.0),
        );

        assert_eq!(result.transcript, "Hello world");
        assert!(result.is_final);
        assert!(result.is_speech_final);
        assert_eq!(result.confidence, 0.95);
        assert_eq!(result.words.as_ref().unwrap().len(), 2);
        assert_eq!(result.speakers.as_ref().unwrap().len(), 1);
        assert_eq!(result.entities.as_ref().unwrap().len(), 1);
        assert_eq!(result.sensitive_data.as_ref().unwrap().len(), 1);
        assert_eq!(
            result.redacted_transcript,
            Some("[REDACTED] world".to_string())
        );
        assert_eq!(result.logprobs, Some(-0.5));
        assert_eq!(result.detected_language, Some("en".to_string()));
        assert_eq!(result.audio_duration, Some(1.0));
    }

    #[test]
    fn test_stt_result_word_duration() {
        let mut result = STTResult::new("Test".to_string(), true, true, 0.9);

        // No words - should return None
        assert!(result.word_duration().is_none());

        // Empty words - should return None
        result.words = Some(vec![]);
        assert!(result.word_duration().is_none());

        // With words - should calculate duration
        result.words = Some(vec![
            WordTiming::new("Hello".to_string(), 0.0, 0.5),
            WordTiming::new("world".to_string(), 0.6, 1.2),
        ]);
        let duration = result.word_duration().unwrap();
        assert!((duration - 1.2).abs() < 1e-10);
    }

    #[test]
    fn test_stt_result_speaker_count() {
        let mut result = STTResult::new("Test".to_string(), true, true, 0.9);

        // No speakers - should return None
        assert!(result.speaker_count().is_none());

        // With speakers
        result.speakers = Some(vec![
            SpeakerInfo::new("speaker_0".to_string()),
            SpeakerInfo::new("speaker_1".to_string()),
        ]);
        assert_eq!(result.speaker_count(), Some(2));
    }

    #[test]
    fn test_stt_result_words_by_speaker() {
        let mut result = STTResult::new("Test".to_string(), true, true, 0.9);

        // No words - should return empty
        assert!(result.words_by_speaker("speaker_0").is_empty());

        // With words from multiple speakers
        result.words = Some(vec![
            WordTiming::with_all(
                "Hello".to_string(),
                0.0,
                0.5,
                None,
                Some("speaker_0".to_string()),
                None,
            ),
            WordTiming::with_all(
                "there".to_string(),
                0.6,
                0.9,
                None,
                Some("speaker_1".to_string()),
                None,
            ),
            WordTiming::with_all(
                "friend".to_string(),
                1.0,
                1.4,
                None,
                Some("speaker_0".to_string()),
                None,
            ),
        ]);

        let speaker_0_words = result.words_by_speaker("speaker_0");
        assert_eq!(speaker_0_words.len(), 2);
        assert_eq!(speaker_0_words[0].word, "Hello");
        assert_eq!(speaker_0_words[1].word, "friend");

        let speaker_1_words = result.words_by_speaker("speaker_1");
        assert_eq!(speaker_1_words.len(), 1);
        assert_eq!(speaker_1_words[0].word, "there");

        let unknown_speaker = result.words_by_speaker("speaker_99");
        assert!(unknown_speaker.is_empty());
    }

    #[test]
    fn test_stt_result_entities_by_category() {
        let mut result = STTResult::new("Test".to_string(), true, true, 0.9);

        // No entities - should return empty
        assert!(result.entities_by_category("person_name").is_empty());

        // With entities of multiple categories
        result.entities = Some(vec![
            DetectedEntity::new("John".to_string(), "person_name".to_string()),
            DetectedEntity::new("555-1234".to_string(), "phone_number".to_string()),
            DetectedEntity::new("Jane".to_string(), "person_name".to_string()),
        ]);

        let person_entities = result.entities_by_category("person_name");
        assert_eq!(person_entities.len(), 2);
        assert_eq!(person_entities[0].text, "John");
        assert_eq!(person_entities[1].text, "Jane");

        let phone_entities = result.entities_by_category("phone_number");
        assert_eq!(phone_entities.len(), 1);
        assert_eq!(phone_entities[0].text, "555-1234");

        let unknown_category = result.entities_by_category("email");
        assert!(unknown_category.is_empty());
    }

    #[test]
    fn test_stt_result_has_sensitive_data() {
        let mut result = STTResult::new("Test".to_string(), true, true, 0.9);

        // No sensitive data
        assert!(!result.has_sensitive_data());

        // Empty sensitive data
        result.sensitive_data = Some(vec![]);
        assert!(!result.has_sensitive_data());

        // With sensitive data
        result.sensitive_data = Some(vec![SensitiveDataItem::new("ssn".to_string())]);
        assert!(result.has_sensitive_data());
    }

    #[test]
    fn test_stt_result_has_word_timing() {
        let mut result = STTResult::new("Test".to_string(), true, true, 0.9);

        // No words
        assert!(!result.has_word_timing());

        // Empty words
        result.words = Some(vec![]);
        assert!(!result.has_word_timing());

        // With words
        result.words = Some(vec![WordTiming::new("Test".to_string(), 0.0, 0.5)]);
        assert!(result.has_word_timing());
    }

    #[test]
    fn test_stt_result_has_diarization() {
        let mut result = STTResult::new("Test".to_string(), true, true, 0.9);

        // No speakers
        assert!(!result.has_diarization());

        // Empty speakers
        result.speakers = Some(vec![]);
        assert!(!result.has_diarization());

        // With speakers
        result.speakers = Some(vec![SpeakerInfo::new("speaker_0".to_string())]);
        assert!(result.has_diarization());
    }

    #[test]
    fn test_word_timing_equality() {
        let w1 = WordTiming::new("hello".to_string(), 0.0, 0.5);
        let w2 = WordTiming::new("hello".to_string(), 0.0, 0.5);
        let w3 = WordTiming::new("world".to_string(), 0.0, 0.5);

        assert_eq!(w1, w2);
        assert_ne!(w1, w3);
    }

    #[test]
    fn test_speaker_info_equality() {
        let s1 = SpeakerInfo::new("speaker_0".to_string());
        let s2 = SpeakerInfo::new("speaker_0".to_string());
        let s3 = SpeakerInfo::new("speaker_1".to_string());

        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_detected_entity_equality() {
        let e1 = DetectedEntity::new("John".to_string(), "person".to_string());
        let e2 = DetectedEntity::new("John".to_string(), "person".to_string());
        let e3 = DetectedEntity::new("Jane".to_string(), "person".to_string());

        assert_eq!(e1, e2);
        assert_ne!(e1, e3);
    }

    #[test]
    fn test_sensitive_data_item_equality() {
        let sd1 = SensitiveDataItem::new("ssn".to_string());
        let sd2 = SensitiveDataItem::new("ssn".to_string());
        let sd3 = SensitiveDataItem::new("credit_card".to_string());

        assert_eq!(sd1, sd2);
        assert_ne!(sd1, sd3);
    }
}

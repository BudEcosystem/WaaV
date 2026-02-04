//! Bhashini TTS provider implementation.
//!
//! This module provides the `BhashiniTts` provider that implements the `BaseTTS` trait
//! for Bhashini's Pipeline Compute API for text-to-speech synthesis.

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::config::{BHASHINI_CONFIG_URL, BhashiniTtsConfig};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};

// Re-use message types from STT module since Bhashini uses same pipeline architecture
use crate::core::stt::bhashini::{PipelineConfigRequest, PipelineConfigResponse};

/// Provider information.
const PROVIDER_INFO: &str = "Bhashini ULCA TTS v1.0 (MeitY Government of India)";

/// Default request timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Default connect timeout in seconds.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;

/// Maximum retries for transient errors.
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (milliseconds).
const BASE_RETRY_DELAY_MS: u64 = 500;

/// User-Agent header value for API requests.
const USER_AGENT: &str = concat!("WaaV-Gateway/", env!("CARGO_PKG_VERSION"));

/// Cached pipeline configuration for TTS.
#[derive(Debug, Clone)]
struct CachedTtsPipelineConfig {
    /// Callback URL for compute calls.
    callback_url: String,
    /// Inference API key header name.
    auth_header_name: String,
    /// Inference API key value.
    auth_header_value: String,
    /// Service ID for TTS.
    service_id: String,
}

/// TTS Pipeline Compute request payload.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsPipelineComputeRequest {
    /// Pipeline tasks.
    pipeline_tasks: Vec<TtsTask>,
    /// Input data.
    input_data: TtsInputData,
}

/// TTS task configuration.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsTask {
    /// Task type ("tts").
    task_type: String,
    /// Task configuration.
    config: TtsTaskConfig,
}

/// TTS task configuration.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsTaskConfig {
    /// Language code.
    language: TtsLanguage,
    /// Service ID.
    service_id: String,
    /// Gender for voice selection.
    gender: String,
    /// Sample rate for output.
    sampling_rate: u32,
}

/// Language configuration for TTS.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsLanguage {
    /// Source language code.
    source_language: String,
}

/// TTS input data.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsInputData {
    /// Input array with text.
    input: Vec<TtsInput>,
}

/// Individual TTS input.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsInput {
    /// Text to synthesize.
    source: String,
}

/// TTS Pipeline Compute response.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TtsPipelineComputeResponse {
    /// Pipeline response.
    #[serde(default)]
    pipeline_response: Vec<TtsPipelineResponseItem>,
}

/// TTS pipeline response item.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TtsPipelineResponseItem {
    /// Task type.
    task_type: Option<String>,
    /// Audio array.
    #[serde(default)]
    audio: Vec<TtsAudioItem>,
}

/// TTS audio item.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TtsAudioItem {
    /// Base64-encoded audio.
    audio_content: Option<String>,
    /// Audio URI (alternative to content).
    #[allow(dead_code)]
    audio_uri: Option<String>,
}

impl TtsPipelineComputeResponse {
    /// Get the audio content from the response.
    fn audio_content(&self) -> Option<&str> {
        self.pipeline_response
            .iter()
            .find(|r| r.task_type.as_deref() == Some("tts"))
            .and_then(|r| r.audio.first())
            .and_then(|a| a.audio_content.as_deref())
    }
}

/// Error response from Bhashini API.
#[derive(Debug, Clone, serde::Deserialize)]
struct BhashiniTtsErrorResponse {
    /// Error message.
    message: Option<String>,
    /// Error details.
    error: Option<String>,
}

impl BhashiniTtsErrorResponse {
    fn error_message(&self) -> String {
        self.message
            .as_ref()
            .or(self.error.as_ref())
            .cloned()
            .unwrap_or_else(|| "Unknown error".to_string())
    }
}

/// Bhashini TTS provider.
pub struct BhashiniTts {
    /// Base TTS configuration.
    #[allow(dead_code)]
    base_config: TTSConfig,
    /// Bhashini-specific configuration.
    config: BhashiniTtsConfig,
    /// HTTP client.
    client: Client,
    /// Cached pipeline configuration.
    pipeline_config: Arc<Mutex<Option<CachedTtsPipelineConfig>>>,
    /// Connection state.
    connected: AtomicBool,
    /// Audio callback.
    audio_callback: Arc<Mutex<Option<Arc<dyn AudioCallback>>>>,
}

impl BhashiniTts {
    /// Create a new Bhashini TTS provider (internal).
    fn create_internal(config: TTSConfig) -> TTSResult<Self> {
        let bhashini_config = BhashiniTtsConfig::from_base(config.clone())?;

        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| {
                TTSError::ConnectionFailed(format!("Failed to create HTTP client: {}", e))
            })?;

        Ok(Self {
            base_config: config,
            config: bhashini_config,
            client,
            pipeline_config: Arc::new(Mutex::new(None)),
            connected: AtomicBool::new(false),
            audio_callback: Arc::new(Mutex::new(None)),
        })
    }

    /// Fetch pipeline configuration for TTS.
    async fn fetch_pipeline_config(&self) -> TTSResult<CachedTtsPipelineConfig> {
        debug!(
            "Fetching Bhashini TTS pipeline config for language: {}",
            self.config.language.as_code()
        );

        let request = PipelineConfigRequest::new_tts(
            self.config.language.as_code(),
            self.config.pipeline_id(),
        );

        let response = self
            .client
            .post(BHASHINI_CONFIG_URL)
            .header("userID", &self.config.user_id)
            .header("ulcaApiKey", &self.config.ulca_api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                TTSError::NetworkError(format!("Pipeline config request failed: {}", e))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if let Ok(error) = serde_json::from_str::<BhashiniTtsErrorResponse>(&body) {
                return Err(TTSError::ProviderError(format!(
                    "Bhashini error ({}): {}",
                    status.as_u16(),
                    error.error_message()
                )));
            }
            return Err(TTSError::ProviderError(format!(
                "Pipeline config failed ({}): {}",
                status.as_u16(),
                body
            )));
        }

        let config_response: PipelineConfigResponse = response.json().await.map_err(|e| {
            TTSError::ProviderError(format!("Failed to parse pipeline config response: {}", e))
        })?;

        // Extract callback URL
        let callback_url = config_response
            .callback_url()
            .map(|s| s.to_string())
            .or_else(|| self.config.custom_callback_url.clone())
            .ok_or_else(|| {
                TTSError::ProviderError("No callback URL in pipeline config response".to_string())
            })?;

        // Extract inference API key
        let (auth_header_name, auth_header_value) = config_response
            .inference_api_key()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .or_else(|| {
                self.config
                    .inference_api_key
                    .as_ref()
                    .map(|k| ("Authorization".to_string(), k.clone()))
            })
            .ok_or_else(|| {
                TTSError::AuthenticationFailed(
                    "No inference API key in pipeline config response".to_string(),
                )
            })?;

        // Extract service ID for TTS
        let service_id = config_response
            .tts_service_id()
            .map(|s| s.to_string())
            .or_else(|| self.config.custom_service_id.clone())
            .unwrap_or_else(|| self.config.tts_service_id().to_string());

        info!(
            "Bhashini TTS pipeline configured: callback_url={}, service_id={}",
            callback_url, service_id
        );

        Ok(CachedTtsPipelineConfig {
            callback_url,
            auth_header_name,
            auth_header_value,
            service_id,
        })
    }

    /// Execute TTS synthesis request.
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        let pipeline_config = {
            let guard = self.pipeline_config.lock().await;
            guard
                .clone()
                .ok_or_else(|| TTSError::ProviderNotReady("Pipeline not configured".to_string()))?
        };

        debug!("Synthesizing text with Bhashini TTS: {} chars", text.len());

        // Create compute request
        let request = TtsPipelineComputeRequest {
            pipeline_tasks: vec![TtsTask {
                task_type: "tts".to_string(),
                config: TtsTaskConfig {
                    language: TtsLanguage {
                        source_language: self.config.language.as_code().to_string(),
                    },
                    service_id: pipeline_config.service_id.clone(),
                    gender: self.config.gender.as_str().to_string(),
                    sampling_rate: self.config.sample_rate,
                },
            }],
            input_data: TtsInputData {
                input: vec![TtsInput {
                    source: text.to_string(),
                }],
            },
        };

        // Send request with retries
        let mut last_error = None;
        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_millis(BASE_RETRY_DELAY_MS * 2u64.pow(attempt - 1));
                tokio::time::sleep(delay).await;
                warn!("Retrying Bhashini TTS request (attempt {})", attempt + 1);
            }

            match self
                .execute_compute_request(&pipeline_config, &request)
                .await
            {
                Ok(audio_data) => {
                    info!(
                        "Bhashini TTS synthesis complete: {} bytes, language: {}",
                        audio_data.len(),
                        self.config.language.as_code()
                    );
                    return Ok(audio_data);
                }
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| TTSError::ProviderError("Max retries exceeded".to_string())))
    }

    /// Execute a single compute request.
    async fn execute_compute_request(
        &self,
        pipeline_config: &CachedTtsPipelineConfig,
        request: &TtsPipelineComputeRequest,
    ) -> TTSResult<Vec<u8>> {
        let response = self
            .client
            .post(&pipeline_config.callback_url)
            .header(
                &pipeline_config.auth_header_name,
                &pipeline_config.auth_header_value,
            )
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Compute request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if let Ok(error) = serde_json::from_str::<BhashiniTtsErrorResponse>(&body) {
                return Err(TTSError::ProviderError(format!(
                    "Bhashini TTS error ({}): {}",
                    status.as_u16(),
                    error.error_message()
                )));
            }
            return Err(TTSError::ProviderError(format!(
                "Compute request failed ({}): {}",
                status.as_u16(),
                body
            )));
        }

        let compute_response: TtsPipelineComputeResponse = response.json().await.map_err(|e| {
            TTSError::ProviderError(format!("Failed to parse compute response: {}", e))
        })?;

        // Extract and decode audio
        let audio_base64 = compute_response.audio_content().ok_or_else(|| {
            TTSError::AudioGenerationFailed("No audio content in response".to_string())
        })?;

        let audio_data = BASE64.decode(audio_base64).map_err(|e| {
            TTSError::AudioGenerationFailed(format!("Failed to decode audio: {}", e))
        })?;

        Ok(audio_data)
    }

    /// Invoke audio callback with data.
    async fn invoke_audio_callback(&self, audio_data: Vec<u8>) {
        if let Some(callback) = &*self.audio_callback.lock().await {
            let data = AudioData {
                data: audio_data,
                sample_rate: self.config.sample_rate,
                format: self.config.audio_format.as_str().to_string(),
                duration_ms: None,
            };
            callback.on_audio(data).await;
            callback.on_complete().await;
        }
    }

    /// Invoke error callback.
    async fn invoke_error_callback(&self, error: TTSError) {
        if let Some(callback) = &*self.audio_callback.lock().await {
            callback.on_error(error).await;
        }
    }
}

#[async_trait]
impl BaseTTS for BhashiniTts {
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        Self::create_internal(config)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!(
            "Connecting to Bhashini TTS (language: {}, gender: {})",
            self.config.language.display_name(),
            self.config.gender.as_str()
        );

        // Fetch and cache pipeline configuration
        let pipeline_config = self.fetch_pipeline_config().await?;
        *self.pipeline_config.lock().await = Some(pipeline_config);

        self.connected.store(true, Ordering::SeqCst);
        info!("Connected to Bhashini TTS");

        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from Bhashini TTS");

        *self.pipeline_config.lock().await = None;
        self.connected.store(false, Ordering::SeqCst);

        info!("Disconnected from Bhashini TTS");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn get_connection_state(&self) -> ConnectionState {
        if self.connected.load(Ordering::SeqCst) {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        if !self.is_ready() {
            return Err(TTSError::ProviderNotReady(
                "Bhashini TTS not connected".to_string(),
            ));
        }

        if text.trim().is_empty() {
            return Ok(());
        }

        match self.synthesize(text).await {
            Ok(audio_data) => {
                self.invoke_audio_callback(audio_data).await;
                Ok(())
            }
            Err(e) => {
                error!("Bhashini TTS synthesis failed: {}", e);
                self.invoke_error_callback(e.clone()).await;
                Err(e)
            }
        }
    }

    async fn flush(&self) -> TTSResult<()> {
        // Bhashini TTS is synchronous per request, no buffering
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        let mut guard = self
            .audio_callback
            .try_lock()
            .map_err(|_| TTSError::InternalError("Failed to acquire callback lock".to_string()))?;
        *guard = Some(callback);
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        let mut guard = self
            .audio_callback
            .try_lock()
            .map_err(|_| TTSError::InternalError("Failed to acquire callback lock".to_string()))?;
        *guard = None;
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": PROVIDER_INFO,
            "language": self.config.language.display_name(),
            "language_code": self.config.language.as_code(),
            "gender": self.config.gender.as_str(),
            "sample_rate": self.config.sample_rate,
            "audio_format": self.config.audio_format.as_str(),
            "supported_languages": BhashiniTtsConfig::default().language.display_name(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test_user|test_key".to_string(),
            voice_id: Some("hi".to_string()),
            sample_rate: Some(22050),
            ..Default::default()
        }
    }

    #[test]
    fn test_bhashini_tts_creation() {
        let config = create_test_config();
        let result = BhashiniTts::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_bhashini_tts_invalid_config() {
        let config = TTSConfig {
            api_key: "invalid".to_string(),
            ..Default::default()
        };
        let result = BhashiniTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_bhashini_tts_initial_state() {
        let config = create_test_config();
        let tts = BhashiniTts::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_bhashini_tts_provider_info() {
        let config = create_test_config();
        let tts = BhashiniTts::new(config).unwrap();
        let info = tts.get_provider_info();
        assert!(info["provider"].as_str().unwrap().contains("Bhashini"));
    }

    #[tokio::test]
    async fn test_speak_not_connected() {
        let config = create_test_config();
        let mut tts = BhashiniTts::new(config).unwrap();

        let result = tts.speak("Hello", false).await;
        assert!(result.is_err());
        if let Err(TTSError::ProviderNotReady(_)) = result {
            // Expected
        } else {
            panic!("Expected ProviderNotReady error");
        }
    }

    #[test]
    fn test_tts_compute_request_serialization() {
        let request = TtsPipelineComputeRequest {
            pipeline_tasks: vec![TtsTask {
                task_type: "tts".to_string(),
                config: TtsTaskConfig {
                    language: TtsLanguage {
                        source_language: "hi".to_string(),
                    },
                    service_id: "test-service".to_string(),
                    gender: "female".to_string(),
                    sampling_rate: 22050,
                },
            }],
            input_data: TtsInputData {
                input: vec![TtsInput {
                    source: "Hello world".to_string(),
                }],
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("pipelineTasks"));
        assert!(json.contains("inputData"));
        assert!(json.contains("Hello world"));
    }

    #[test]
    fn test_tts_compute_response_parsing() {
        let json = r#"{
            "pipelineResponse": [
                {
                    "taskType": "tts",
                    "audio": [
                        {"audioContent": "SGVsbG8gV29ybGQ="}
                    ]
                }
            ]
        }"#;

        let response: TtsPipelineComputeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.audio_content(), Some("SGVsbG8gV29ybGQ="));
    }
}

//! Bhashini (ULCA) STT client implementation.
//!
//! This module provides the main `BhashiniStt` client that implements the `BaseSTT` trait
//! for Bhashini's Pipeline Compute API.
//!
//! # Architecture
//!
//! Bhashini uses a pipeline-based REST API. This implementation:
//!
//! 1. On connect: Fetches pipeline configuration to get callback URL and inference key
//! 2. Buffers incoming audio data in memory
//! 3. On disconnect: Sends accumulated audio to compute endpoint
//! 4. Parses the response and invokes callbacks
//!
//! # Pipeline Flow
//!
//! ```text
//! ┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
//! │ Pipeline Config │ --> │ Get Service IDs │ --> │ Pipeline Compute │
//! │   (on connect)  │     │  & Callback URL │     │  (on disconnect) │
//! └─────────────────┘     └─────────────────┘     └─────────────────┘
//! ```

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bytes::Bytes;
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::super::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use super::config::{BHASHINI_CONFIG_URL, BhashiniSttConfig};
use super::messages::{
    BhashiniErrorResponse, PipelineComputeRequest, PipelineComputeResponse, PipelineConfigRequest,
    PipelineConfigResponse, wav,
};

/// Bhashini provider identifier.
const PROVIDER_INFO: &str = "Bhashini ULCA v1.0 (MeitY Government of India)";

// =============================================================================
// Constants
// =============================================================================

/// Maximum allowed buffer size to prevent unbounded growth (10MB).
const MAX_BUFFER_SIZE_BYTES: usize = 10 * 1024 * 1024;

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

// =============================================================================
// Cached Pipeline Config
// =============================================================================

/// Cached pipeline configuration.
#[derive(Debug, Clone)]
struct CachedPipelineConfig {
    /// Callback URL for compute calls.
    callback_url: String,
    /// Inference API key header name.
    auth_header_name: String,
    /// Inference API key value.
    auth_header_value: String,
    /// Service ID for ASR.
    service_id: String,
}

// =============================================================================
// Bhashini STT Client
// =============================================================================

/// Bhashini STT client implementing the BaseSTT trait.
///
/// This client uses the Bhashini Pipeline API to convert speech to text.
/// Since it's a batch API (not streaming), audio is buffered and sent
/// when the connection is closed.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::bhashini::BhashiniStt;
///
/// let config = STTConfig {
///     api_key: "userId|ulcaApiKey".to_string(),
///     language: "hi".to_string(),
///     sample_rate: 16000,
///     ..Default::default()
/// };
///
/// let mut stt = BhashiniStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct BhashiniStt {
    /// Base STT configuration (for BaseSTT trait).
    base_config: STTConfig,

    /// Provider-specific configuration.
    config: BhashiniSttConfig,

    /// HTTP client for API requests.
    client: Client,

    /// Cached pipeline configuration.
    pipeline_config: Arc<Mutex<Option<CachedPipelineConfig>>>,

    /// Audio buffer.
    audio_buffer: Arc<Mutex<Vec<u8>>>,

    /// Connection state.
    connected: AtomicBool,

    /// Result callback.
    result_callback: Arc<Mutex<Option<STTResultCallback>>>,

    /// Error callback.
    error_callback: Arc<Mutex<Option<STTErrorCallback>>>,
}

impl BhashiniStt {
    /// Create a new Bhashini STT client (internal, for use by BaseSTT::new).
    fn create_internal(config: STTConfig) -> Result<Self, STTError> {
        let bhashini_config = BhashiniSttConfig::from_base(config.clone()).map_err(|e| {
            STTError::ConfigurationError(format!("Invalid Bhashini configuration: {}", e))
        })?;
        Self::from_bhashini_config(config, bhashini_config)
    }

    /// Internal: assemble the client from an already-mapped Bhashini config (shared by
    /// `create_internal`/`new` and `new_standard`).
    fn from_bhashini_config(
        base_config: STTConfig,
        bhashini_config: BhashiniSttConfig,
    ) -> Result<Self, STTError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| {
                STTError::ConnectionFailed(format!("Failed to create HTTP client: {}", e))
            })?;

        Ok(Self {
            base_config,
            config: bhashini_config,
            client,
            pipeline_config: Arc::new(Mutex::new(None)),
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            connected: AtomicBool::new(false),
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
        })
    }

    /// Public constructor for external use.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        Self::create_internal(config)
    }

    /// W1 keystone — construct directly from the standardized config. The typed `SttFeatures`
    /// are all capability gaps for this batch ULCA provider and stay at default, but two
    /// Bhashini-specific ULCA pipeline knobs (`sourceScriptCode`, `postProcessors`) carried via
    /// the standardized extras passthrough DO reach `pipelineTasks[].config` on the wire — see
    /// `BhashiniSttConfig::from_standard` and `fetch_pipeline_config`.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let bhashini_config = BhashiniSttConfig::from_standard(std).map_err(|e| {
            STTError::ConfigurationError(format!("Invalid Bhashini configuration: {}", e))
        })?;
        Self::from_bhashini_config(std.base.clone(), bhashini_config)
    }

    /// Fetch pipeline configuration from Bhashini.
    async fn fetch_pipeline_config(&self) -> Result<CachedPipelineConfig, STTError> {
        debug!(
            "Fetching Bhashini pipeline config for language: {}",
            self.config.language.as_code()
        );

        // Carry the advanced ULCA ASR knobs (sourceScriptCode + postProcessors) into the
        // pipeline-config request body when configured via the standardized extras passthrough.
        let request = PipelineConfigRequest::new_asr_with(
            self.config.language.as_code(),
            self.config.pipeline_id(),
            self.config.source_script_code.clone(),
            self.config.post_processors.clone(),
        );

        let response = self
            .client
            .post(crate::core::tts::standard::override_rest_endpoint(
                BHASHINI_CONFIG_URL,
                self.config.endpoint_override.as_deref(),
            ))
            .header("userID", &self.config.user_id)
            .header("ulcaApiKey", &self.config.ulca_api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                STTError::NetworkError(format!("Pipeline config request failed: {}", e))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if let Ok(error) = serde_json::from_str::<BhashiniErrorResponse>(&body) {
                return Err(STTError::ProviderError(format!(
                    "Bhashini error ({}): {}",
                    status.as_u16(),
                    error.error_message()
                )));
            }
            return Err(STTError::ProviderError(format!(
                "Pipeline config failed ({}): {}",
                status.as_u16(),
                body
            )));
        }

        let config_response: PipelineConfigResponse = response.json().await.map_err(|e| {
            STTError::ProviderError(format!("Failed to parse pipeline config response: {}", e))
        })?;

        // Extract callback URL
        let callback_url = config_response
            .callback_url()
            .map(|s| s.to_string())
            .or_else(|| self.config.custom_callback_url.clone())
            .ok_or_else(|| {
                STTError::ProviderError("No callback URL in pipeline config response".to_string())
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
                STTError::AuthenticationFailed(
                    "No inference API key in pipeline config response".to_string(),
                )
            })?;

        // Extract service ID
        let service_id = config_response
            .asr_service_id()
            .map(|s| s.to_string())
            .or_else(|| self.config.custom_service_id.clone())
            .unwrap_or_else(|| self.config.language.asr_service_id().to_string());

        info!(
            "Bhashini pipeline configured: callback_url={}, service_id={}",
            callback_url, service_id
        );

        Ok(CachedPipelineConfig {
            callback_url,
            auth_header_name,
            auth_header_value,
            service_id,
        })
    }

    /// Send buffered audio to the compute endpoint.
    async fn send_audio_to_compute(&self) -> Result<(), STTError> {
        let pipeline_config = {
            let guard = self.pipeline_config.lock().await;
            guard
                .clone()
                .ok_or_else(|| STTError::ConnectionFailed("Pipeline not configured".to_string()))?
        };

        // Get and clear buffer
        let audio_data = {
            let mut buffer = self.audio_buffer.lock().await;
            if buffer.is_empty() {
                debug!("No audio data to process");
                return Ok(());
            }
            std::mem::take(&mut *buffer)
        };

        debug!(
            "Sending {} bytes of audio to Bhashini compute endpoint",
            audio_data.len()
        );

        // Encode audio as WAV and then base64
        let wav_data = wav::encode_wav(&audio_data, self.config.sample_rate, 16, 1);
        let audio_base64 = BASE64.encode(&wav_data);

        // Create compute request
        let request = PipelineComputeRequest::new_asr(
            self.config.language.as_code(),
            &pipeline_config.service_id,
            self.config.audio_format.as_str(),
            self.config.sample_rate,
            audio_base64,
        );

        // Send request with retries
        let mut last_error = None;
        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_millis(BASE_RETRY_DELAY_MS * 2u64.pow(attempt - 1));
                tokio::time::sleep(delay).await;
                warn!(
                    "Retrying Bhashini compute request (attempt {})",
                    attempt + 1
                );
            }

            match self
                .execute_compute_request(&pipeline_config, &request)
                .await
            {
                Ok(response) => {
                    // Extract transcription result
                    if let Some(text) = response.asr_result() {
                        let result = STTResult::new(
                            text.to_string(),
                            true, // is_final
                            true, // is_speech_final
                            0.95, // confidence (Bhashini doesn't return confidence)
                        );

                        // Invoke callback
                        if let Some(ref callback) = *self.result_callback.lock().await {
                            callback(result).await;
                        }

                        info!(
                            "Bhashini ASR result: {} chars, language: {}",
                            text.len(),
                            self.config.language.as_code()
                        );
                    } else {
                        debug!("No transcription result in response");
                    }

                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| STTError::ProviderError("Max retries exceeded".to_string())))
    }

    /// Execute a single compute request.
    async fn execute_compute_request(
        &self,
        pipeline_config: &CachedPipelineConfig,
        request: &PipelineComputeRequest,
    ) -> Result<PipelineComputeResponse, STTError> {
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
            .map_err(|e| STTError::NetworkError(format!("Compute request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if let Ok(error) = serde_json::from_str::<BhashiniErrorResponse>(&body) {
                return Err(STTError::ProviderError(format!(
                    "Bhashini error ({}): {}",
                    status.as_u16(),
                    error.error_message()
                )));
            }
            return Err(STTError::ProviderError(format!(
                "Compute request failed ({}): {}",
                status.as_u16(),
                body
            )));
        }

        response.json().await.map_err(|e| {
            STTError::ProviderError(format!("Failed to parse compute response: {}", e))
        })
    }

    /// Invoke error callback.
    async fn invoke_error_callback(&self, error: STTError) {
        if let Some(ref callback) = *self.error_callback.lock().await {
            callback(error).await;
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for BhashiniStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        Self::create_internal(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!(
            "Connecting to Bhashini STT (language: {}, provider: {})",
            self.config.language.display_name(),
            self.config.pipeline_provider.display_name()
        );

        // Fetch and cache pipeline configuration
        let pipeline_config = self.fetch_pipeline_config().await?;
        *self.pipeline_config.lock().await = Some(pipeline_config);

        // Clear any existing audio buffer
        self.audio_buffer.lock().await.clear();

        self.connected.store(true, Ordering::SeqCst);
        info!("Connected to Bhashini STT");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from Bhashini STT");

        // Process any remaining audio
        if let Err(e) = self.send_audio_to_compute().await {
            error!("Error processing final audio: {}", e);
            self.invoke_error_callback(e.clone()).await;
        }

        // Clear state
        self.audio_buffer.lock().await.clear();
        *self.pipeline_config.lock().await = None;
        self.connected.store(false, Ordering::SeqCst);

        info!("Disconnected from Bhashini STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(STTError::ConnectionFailed(
                "Not connected to Bhashini STT".to_string(),
            ));
        }

        let mut buffer = self.audio_buffer.lock().await;

        // Check buffer size limit
        if buffer.len() + audio_data.len() > MAX_BUFFER_SIZE_BYTES {
            return Err(STTError::AudioProcessingError(format!(
                "Buffer full: {} bytes (max: {} bytes)",
                buffer.len() + audio_data.len(),
                MAX_BUFFER_SIZE_BYTES
            )));
        }

        buffer.extend_from_slice(&audio_data);
        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        let mut guard = self.result_callback.lock().await;
        *guard = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        let mut guard = self.error_callback.lock().await;
        *guard = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.base_config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Cannot update config while not connected".to_string(),
            ));
        }

        // Parse new Bhashini config
        let bhashini_config = BhashiniSttConfig::from_base(config.clone()).map_err(|e| {
            STTError::ConfigurationError(format!("Invalid Bhashini configuration: {}", e))
        })?;

        // Update configs
        self.base_config = config;
        self.config = bhashini_config;

        info!("Bhashini STT configuration updated");
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        PROVIDER_INFO
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: "test_user|test_ulca_key".to_string(),
            language: "hi".to_string(),
            sample_rate: 16000,
            ..Default::default()
        }
    }

    #[test]
    fn test_bhashini_stt_creation() {
        let config = create_test_config();
        let result = BhashiniStt::new(config);
        assert!(result.is_ok());
    }

    // W1 keystone: Bhashini maps zero advanced features (batch ULCA provider), so new_standard is a
    // pure passthrough — the base credentials parsed from api_key carry through to the stored config.
    #[test]
    fn new_standard_passthrough_carries_base() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "bhashini".into(),
                api_key: "user123|apikey456".into(),
                language: "hi".into(),
                sample_rate: 16000,
                ..Default::default()
            },
            features: SttFeatures {
                // None of these map to a real Bhashini field; they must be ignored, not error.
                diarization: Some(true),
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
            translation: None,
        };
        let stt = BhashiniStt::new_standard(&std).unwrap();
        assert_eq!(stt.config.user_id, "user123");
        assert_eq!(stt.config.ulca_api_key, "apikey456");
    }

    #[test]
    fn test_bhashini_stt_invalid_config() {
        let config = STTConfig {
            api_key: "invalid_no_separator".to_string(),
            ..Default::default()
        };
        let result = BhashiniStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_bhashini_stt_provider_info() {
        let config = create_test_config();
        let stt = BhashiniStt::new(config).unwrap();
        assert!(stt.get_provider_info().contains("Bhashini"));
    }

    #[test]
    fn test_bhashini_stt_initial_state() {
        let config = create_test_config();
        let stt = BhashiniStt::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = BhashiniStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());
        if let Err(STTError::ConnectionFailed(_)) = result {
            // Expected
        } else {
            panic!("Expected ConnectionFailed error");
        }
    }

    #[test]
    fn test_bhashini_stt_get_config() {
        let config = create_test_config();
        let stt = BhashiniStt::new(config).unwrap();
        let stored_config = stt.get_config();
        assert!(stored_config.is_some());
        assert_eq!(stored_config.unwrap().language, "hi");
    }

    #[test]
    fn test_bhashini_stt_with_inference_key() {
        let config = STTConfig {
            api_key: "test_user|test_ulca_key|test_inference_key".to_string(),
            language: "ta".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };
        let stt = BhashiniStt::new(config).unwrap();
        assert!(stt.config.inference_api_key.is_some());
    }

    #[test]
    fn test_bhashini_stt_language_selection() {
        let config = STTConfig {
            api_key: "test_user|test_ulca_key".to_string(),
            language: "ta".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };
        let stt = BhashiniStt::new(config).unwrap();
        assert_eq!(stt.config.language.as_code(), "ta");
    }

    #[test]
    fn test_wav_encoding() {
        let pcm_data = vec![0u8; 1000];
        let wav = wav::encode_wav(&pcm_data, 16000, 16, 1);

        // Check WAV header
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 1000);
    }

    #[test]
    fn test_constants() {
        assert_eq!(MAX_BUFFER_SIZE_BYTES, 10 * 1024 * 1024);
        assert_eq!(DEFAULT_TIMEOUT_SECS, 60);
        assert_eq!(MAX_RETRIES, 3);
    }
}

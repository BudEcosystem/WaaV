//! Bhashini (ULCA) API message types for STT.
//!
//! This module contains all request and response structures for the
//! Bhashini Pipeline Config and Compute API calls.

use serde::{Deserialize, Serialize};

// =============================================================================
// Pipeline Config Request
// =============================================================================

/// Request payload for Pipeline Config API call.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineConfigRequest {
    /// List of tasks to configure.
    pub pipeline_tasks: Vec<PipelineTask>,
    /// Pipeline request configuration.
    pub pipeline_request_config: PipelineRequestConfig,
}

/// Individual pipeline task.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineTask {
    /// Task type: "asr", "translation", "tts"
    pub task_type: String,
    /// Task configuration.
    pub config: TaskConfig,
}

/// Task configuration.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskConfig {
    /// Language configuration.
    pub language: LanguageConfig,
}

/// Language configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LanguageConfig {
    /// Source language code (ISO-639).
    #[serde(default)]
    pub source_language: String,
    /// Target language code (for translation only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target_language: Option<String>,
}

/// Pipeline request configuration.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRequestConfig {
    /// Pipeline ID.
    pub pipeline_id: String,
}

impl PipelineConfigRequest {
    /// Create a new ASR pipeline config request.
    pub fn new_asr(source_language: &str, pipeline_id: &str) -> Self {
        Self {
            pipeline_tasks: vec![PipelineTask {
                task_type: "asr".to_string(),
                config: TaskConfig {
                    language: LanguageConfig {
                        source_language: source_language.to_string(),
                        target_language: None,
                    },
                },
            }],
            pipeline_request_config: PipelineRequestConfig {
                pipeline_id: pipeline_id.to_string(),
            },
        }
    }

    /// Create a new TTS pipeline config request.
    pub fn new_tts(source_language: &str, pipeline_id: &str) -> Self {
        Self {
            pipeline_tasks: vec![PipelineTask {
                task_type: "tts".to_string(),
                config: TaskConfig {
                    language: LanguageConfig {
                        source_language: source_language.to_string(),
                        target_language: None,
                    },
                },
            }],
            pipeline_request_config: PipelineRequestConfig {
                pipeline_id: pipeline_id.to_string(),
            },
        }
    }
}

// =============================================================================
// Pipeline Config Response
// =============================================================================

/// Response payload from Pipeline Config API call.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineConfigResponse {
    /// List of languages supported.
    #[serde(default)]
    pub languages: Vec<LanguageInfo>,
    /// Pipeline inference API endpoint configuration.
    /// Note: API uses "pipelineInferenceAPIEndPoint" (uppercase API)
    #[serde(rename = "pipelineInferenceAPIEndPoint")]
    pub pipeline_inference_api_end_point: Option<PipelineInferenceEndpoint>,
    /// Pipeline response configuration.
    #[serde(default)]
    pub pipeline_response_config: Vec<PipelineResponseConfig>,
}

/// Language information.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageInfo {
    /// Source language code.
    pub source_language: String,
    /// Target language codes (for translation).
    #[serde(default)]
    pub target_language_list: Vec<String>,
}

/// Pipeline inference endpoint configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineInferenceEndpoint {
    /// Callback URL for compute calls.
    pub callback_url: String,
    /// Inference API key.
    pub inference_api_key: InferenceApiKey,
}

/// Inference API key structure.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InferenceApiKey {
    /// Header name for authorization.
    pub name: String,
    /// Header value (the actual API key).
    pub value: String,
}

/// Pipeline response configuration for each task.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineResponseConfig {
    /// Task type.
    pub task_type: String,
    /// Configuration for this task.
    #[serde(default)]
    pub config: Vec<TaskResponseConfig>,
}

/// Task response configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResponseConfig {
    /// Service ID for this task.
    pub service_id: Option<String>,
    /// Language configuration.
    pub language: Option<LanguageConfig>,
}

impl PipelineConfigResponse {
    /// Get the callback URL for compute calls.
    pub fn callback_url(&self) -> Option<&str> {
        self.pipeline_inference_api_end_point
            .as_ref()
            .map(|e| e.callback_url.as_str())
    }

    /// Get the inference API key.
    pub fn inference_api_key(&self) -> Option<(&str, &str)> {
        self.pipeline_inference_api_end_point.as_ref().map(|e| {
            (
                e.inference_api_key.name.as_str(),
                e.inference_api_key.value.as_str(),
            )
        })
    }

    /// Get the service ID for ASR task.
    pub fn asr_service_id(&self) -> Option<&str> {
        self.pipeline_response_config
            .iter()
            .find(|c| c.task_type == "asr")
            .and_then(|c| c.config.first())
            .and_then(|c| c.service_id.as_deref())
    }

    /// Get the service ID for TTS task.
    pub fn tts_service_id(&self) -> Option<&str> {
        self.pipeline_response_config
            .iter()
            .find(|c| c.task_type == "tts")
            .and_then(|c| c.config.first())
            .and_then(|c| c.service_id.as_deref())
    }
}

// =============================================================================
// Pipeline Compute Request
// =============================================================================

/// Request payload for Pipeline Compute API call.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineComputeRequest {
    /// List of pipeline tasks with service IDs.
    pub pipeline_tasks: Vec<ComputeTask>,
    /// Input data.
    pub input_data: InputData,
}

/// Compute task with service ID.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputeTask {
    /// Task type: "asr", "translation", "tts"
    pub task_type: String,
    /// Task configuration with service ID.
    pub config: ComputeTaskConfig,
}

/// Compute task configuration.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputeTaskConfig {
    /// Language configuration.
    pub language: LanguageConfig,
    /// Service ID for this task.
    pub service_id: String,
    /// Audio format (for ASR).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_format: Option<String>,
    /// Sampling rate (for ASR).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling_rate: Option<u32>,
}

/// Input data for compute request.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InputData {
    /// Text input (for translation/TTS).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<TextInput>>,
    /// Audio input (for ASR).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<Vec<AudioInput>>,
}

/// Text input.
#[derive(Debug, Clone, Serialize)]
pub struct TextInput {
    /// Source text.
    pub source: String,
}

/// Audio input.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInput {
    /// Base64-encoded audio content.
    pub audio_content: String,
}

impl PipelineComputeRequest {
    /// Create a new ASR compute request.
    pub fn new_asr(
        source_language: &str,
        service_id: &str,
        audio_format: &str,
        sampling_rate: u32,
        audio_base64: String,
    ) -> Self {
        Self {
            pipeline_tasks: vec![ComputeTask {
                task_type: "asr".to_string(),
                config: ComputeTaskConfig {
                    language: LanguageConfig {
                        source_language: source_language.to_string(),
                        target_language: None,
                    },
                    service_id: service_id.to_string(),
                    audio_format: Some(audio_format.to_string()),
                    sampling_rate: Some(sampling_rate),
                },
            }],
            input_data: InputData {
                input: None,
                audio: Some(vec![AudioInput {
                    audio_content: audio_base64,
                }]),
            },
        }
    }
}

// =============================================================================
// Pipeline Compute Response
// =============================================================================

/// Response payload from Pipeline Compute API call.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineComputeResponse {
    /// Pipeline response for each task.
    pub pipeline_response: Vec<TaskResponse>,
}

/// Task response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResponse {
    /// Task type.
    pub task_type: String,
    /// Output (for ASR/translation).
    #[serde(default)]
    pub output: Vec<OutputItem>,
    /// Audio output (for TTS).
    #[serde(default)]
    pub audio: Vec<AudioOutput>,
}

/// Output item (text result).
#[derive(Debug, Clone, Deserialize)]
pub struct OutputItem {
    /// Transcribed/translated text.
    pub source: String,
}

/// Audio output.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioOutput {
    /// Base64-encoded audio content.
    pub audio_content: String,
}

impl PipelineComputeResponse {
    /// Get the ASR transcription result.
    pub fn asr_result(&self) -> Option<&str> {
        self.pipeline_response
            .iter()
            .find(|r| r.task_type == "asr")
            .and_then(|r| r.output.first())
            .map(|o| o.source.as_str())
    }

    /// Get TTS audio output (base64).
    pub fn tts_audio(&self) -> Option<&str> {
        self.pipeline_response
            .iter()
            .find(|r| r.task_type == "tts")
            .and_then(|r| r.audio.first())
            .map(|a| a.audio_content.as_str())
    }
}

// =============================================================================
// Error Response
// =============================================================================

/// Error response from Bhashini API.
#[derive(Debug, Clone, Deserialize)]
pub struct BhashiniErrorResponse {
    /// Error details.
    pub error: Option<ErrorDetails>,
    /// Alternative error message field.
    pub message: Option<String>,
    /// Status code.
    #[allow(dead_code)]
    pub status: Option<String>,
}

/// Error details.
#[derive(Debug, Clone, Deserialize)]
pub struct ErrorDetails {
    /// Error code.
    #[allow(dead_code)]
    pub code: Option<String>,
    /// Error message.
    pub message: Option<String>,
}

impl BhashiniErrorResponse {
    /// Get the error message.
    pub fn error_message(&self) -> String {
        if let Some(ref err) = self.error {
            if let Some(ref msg) = err.message {
                return msg.clone();
            }
        }
        if let Some(ref msg) = self.message {
            return msg.clone();
        }
        "Unknown error".to_string()
    }
}

// =============================================================================
// WAV Utilities
// =============================================================================

pub mod wav {
    //! WAV file format utilities for encoding audio data.

    /// Create a WAV header for the given audio parameters.
    pub fn create_wav_header(
        sample_rate: u32,
        bits_per_sample: u16,
        num_channels: u16,
        data_size: u32,
    ) -> Vec<u8> {
        let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
        let block_align = num_channels * bits_per_sample / 8;
        let chunk_size = 36 + data_size;

        let mut header = Vec::with_capacity(44);

        // RIFF header
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&chunk_size.to_le_bytes());
        header.extend_from_slice(b"WAVE");

        // fmt subchunk
        header.extend_from_slice(b"fmt ");
        header.extend_from_slice(&16u32.to_le_bytes()); // Subchunk1Size (16 for PCM)
        header.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat (1 = PCM)
        header.extend_from_slice(&num_channels.to_le_bytes());
        header.extend_from_slice(&sample_rate.to_le_bytes());
        header.extend_from_slice(&byte_rate.to_le_bytes());
        header.extend_from_slice(&block_align.to_le_bytes());
        header.extend_from_slice(&bits_per_sample.to_le_bytes());

        // data subchunk
        header.extend_from_slice(b"data");
        header.extend_from_slice(&data_size.to_le_bytes());

        header
    }

    /// Encode raw PCM audio data as WAV file.
    pub fn encode_wav(
        pcm_data: &[u8],
        sample_rate: u32,
        bits_per_sample: u16,
        num_channels: u16,
    ) -> Vec<u8> {
        let header = create_wav_header(
            sample_rate,
            bits_per_sample,
            num_channels,
            pcm_data.len() as u32,
        );
        let mut wav = header;
        wav.extend_from_slice(pcm_data);
        wav
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_config_request() {
        let request = PipelineConfigRequest::new_asr("hi", "test-pipeline-id");
        assert_eq!(request.pipeline_tasks.len(), 1);
        assert_eq!(request.pipeline_tasks[0].task_type, "asr");
        assert_eq!(
            request.pipeline_tasks[0].config.language.source_language,
            "hi"
        );
        assert_eq!(
            request.pipeline_request_config.pipeline_id,
            "test-pipeline-id"
        );
    }

    #[test]
    fn test_pipeline_config_request_serialization() {
        let request = PipelineConfigRequest::new_asr("ta", "pipeline123");
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"taskType\":\"asr\""));
        assert!(json.contains("\"sourceLanguage\":\"ta\""));
        assert!(json.contains("\"pipelineId\":\"pipeline123\""));
    }

    #[test]
    fn test_pipeline_compute_request() {
        let request = PipelineComputeRequest::new_asr(
            "hi",
            "ai4bharat/conformer-hi-gpu--t4",
            "wav",
            16000,
            "dGVzdAo=".to_string(),
        );
        assert_eq!(request.pipeline_tasks.len(), 1);
        assert_eq!(
            request.pipeline_tasks[0].config.service_id,
            "ai4bharat/conformer-hi-gpu--t4"
        );
        assert!(request.input_data.audio.is_some());
    }

    #[test]
    fn test_pipeline_compute_response_parsing() {
        let json = r#"{
            "pipelineResponse": [
                {
                    "taskType": "asr",
                    "output": [
                        {"source": "नमस्ते"}
                    ],
                    "audio": []
                }
            ]
        }"#;

        let response: PipelineComputeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.asr_result(), Some("नमस्ते"));
    }

    #[test]
    fn test_pipeline_config_response_parsing() {
        let json = r#"{
            "languages": [{"sourceLanguage": "hi", "targetLanguageList": []}],
            "pipelineInferenceAPIEndPoint": {
                "callbackUrl": "https://example.com/compute",
                "inferenceApiKey": {
                    "name": "Authorization",
                    "value": "test-key-123"
                }
            },
            "pipelineResponseConfig": [
                {
                    "taskType": "asr",
                    "config": [
                        {"serviceId": "ai4bharat/conformer-hi-gpu--t4"}
                    ]
                }
            ]
        }"#;

        let response: PipelineConfigResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.callback_url(), Some("https://example.com/compute"));
        assert_eq!(
            response.inference_api_key(),
            Some(("Authorization", "test-key-123"))
        );
        assert_eq!(
            response.asr_service_id(),
            Some("ai4bharat/conformer-hi-gpu--t4")
        );
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{
            "error": {
                "code": "INVALID_LANGUAGE",
                "message": "Language 'xyz' is not supported"
            }
        }"#;

        let response: BhashiniErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.error_message(), "Language 'xyz' is not supported");
    }

    #[test]
    fn test_wav_header_creation() {
        let header = wav::create_wav_header(16000, 16, 1, 1000);
        assert_eq!(header.len(), 44);
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
        assert_eq!(&header[12..16], b"fmt ");
        assert_eq!(&header[36..40], b"data");
    }

    #[test]
    fn test_wav_encoding() {
        let pcm_data = vec![0u8; 100];
        let wav = wav::encode_wav(&pcm_data, 16000, 16, 1);
        assert_eq!(wav.len(), 44 + 100); // Header + data
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[test]
    fn test_language_config_serialization() {
        let config = LanguageConfig {
            source_language: "hi".to_string(),
            target_language: Some("en".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"sourceLanguage\":\"hi\""));
        assert!(json.contains("\"targetLanguage\":\"en\""));
    }

    #[test]
    fn test_language_config_without_target() {
        let config = LanguageConfig {
            source_language: "ta".to_string(),
            target_language: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"sourceLanguage\":\"ta\""));
        assert!(!json.contains("targetLanguage"));
    }
}

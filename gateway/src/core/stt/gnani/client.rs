//! Gnani.ai STT Client Implementation
//!
//! Implements the BaseSTT trait for Gnani's Speech-to-Text service using
//! gRPC bidirectional streaming for real-time transcription.
//!
//! ## Architecture
//!
//! The client uses Gnani's `Listener.DoSpeechToText` gRPC service which accepts
//! a stream of `SpeechChunk` messages and returns a stream of `TranscriptChunk`
//! responses.
//!
//! ```text
//! Audio Input → SpeechChunk stream → gRPC → TranscriptChunk stream → Callbacks
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tonic::transport::Channel;
use tracing::{debug, info, warn};

use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

use super::config::GnaniSTTConfig;
use super::grpc::{create_gnani_channel, GnaniGrpcClient};

/// Gnani Speech-to-Text client
///
/// Implements streaming STT using Gnani's gRPC bidirectional streaming API.
/// The client maintains a persistent connection and streams audio chunks
/// to the server while receiving transcription results asynchronously.
pub struct GnaniSTT {
    /// Provider-specific configuration
    config: Option<GnaniSTTConfig>,

    /// gRPC channel
    grpc_channel: Option<Channel>,

    /// Connection state (lock-free for performance)
    is_connected: Arc<AtomicBool>,

    /// Result callback
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,

    /// Error callback
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,

    /// Audio sender for the streaming session
    audio_sender: Option<mpsc::Sender<Bytes>>,

    /// Handle to the result processing task
    result_task: Option<tokio::task::JoinHandle<()>>,

    /// Last transcript to detect changes (for interim results)
    last_transcript: Arc<RwLock<String>>,
}

impl Default for GnaniSTT {
    fn default() -> Self {
        Self {
            config: None,
            grpc_channel: None,
            is_connected: Arc::new(AtomicBool::new(false)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            audio_sender: None,
            result_task: None,
            last_transcript: Arc::new(RwLock::new(String::new())),
        }
    }
}

impl GnaniSTT {
    /// Create a new Gnani STT instance
    pub fn create(config: STTConfig) -> Result<Self, STTError> {
        // Convert base config to Gnani-specific config
        let gnani_config =
            GnaniSTTConfig::from_base(config).map_err(STTError::ConfigurationError)?;

        Ok(Self {
            config: Some(gnani_config),
            ..Default::default()
        })
    }

    /// Start the gRPC streaming session
    async fn start_streaming_session(&mut self) -> Result<(), STTError> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| STTError::ConfigurationError("No configuration set".to_string()))?
            .clone();

        let channel = self
            .grpc_channel
            .as_ref()
            .ok_or_else(|| STTError::ConnectionFailed("Not connected".to_string()))?
            .clone();

        // Create gRPC client
        let client = GnaniGrpcClient::new(channel, config);

        // Start streaming
        let (audio_tx, mut result_rx) = client.start_streaming().await?;

        self.audio_sender = Some(audio_tx);

        // Clone refs for the result processing task
        let result_callback = self.result_callback.clone();
        let error_callback = self.error_callback.clone();
        let last_transcript = self.last_transcript.clone();
        let is_connected = self.is_connected.clone();

        // Spawn task to process results
        let handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                match result {
                    Ok(chunk) => {
                        // Create STT result from transcript chunk
                        let stt_result = STTResult::new(
                            chunk.best_transcript().to_string(),
                            chunk.is_final,
                            chunk.is_final,
                            chunk.confidence,
                        );

                        // Only send if transcript changed (avoid duplicate interim results)
                        let should_send = {
                            let last = last_transcript.read().await;
                            *last != stt_result.transcript
                        };

                        if should_send && !stt_result.transcript.is_empty() {
                            {
                                let mut last = last_transcript.write().await;
                                *last = stt_result.transcript.clone();
                            }

                            if let Some(callback) = result_callback.read().await.as_ref() {
                                callback(stt_result).await;
                            }
                        }

                        // Clear last transcript on final result
                        if chunk.is_final {
                            let mut last = last_transcript.write().await;
                            last.clear();
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Gnani STT streaming error");
                        if let Some(callback) = error_callback.read().await.as_ref() {
                            callback(e).await;
                        }
                    }
                }
            }

            debug!("Gnani STT result processing task ended");
            is_connected.store(false, Ordering::Release);
        });

        self.result_task = Some(handle);

        Ok(())
    }
}

#[async_trait]
impl BaseSTT for GnaniSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        GnaniSTT::create(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| STTError::ConfigurationError("No configuration set".to_string()))?;

        // Validate configuration
        config.validate().map_err(STTError::ConfigurationError)?;

        info!(
            language = %config.base.language,
            "Connecting to Gnani STT via gRPC"
        );

        // Create gRPC channel with mTLS
        let channel = create_gnani_channel(config).await?;
        self.grpc_channel = Some(channel);

        // Start streaming session
        self.start_streaming_session().await?;

        self.is_connected.store(true, Ordering::Release);
        info!("Connected to Gnani STT");

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        info!("Disconnecting from Gnani STT");

        // Drop audio sender to signal end of stream
        self.audio_sender = None;

        // Wait for result task to complete
        if let Some(task) = self.result_task.take() {
            // Give it a moment to finish processing
            tokio::time::timeout(std::time::Duration::from_secs(2), task)
                .await
                .ok();
        }

        self.grpc_channel = None;
        self.is_connected.store(false, Ordering::Release);

        // Clear state
        {
            let mut last = self.last_transcript.write().await;
            last.clear();
        }

        info!("Disconnected from Gnani STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_connected.load(Ordering::Acquire) && self.audio_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        // Send audio to the streaming channel
        if let Some(ref sender) = self.audio_sender {
            sender.send(audio_data).await.map_err(|_| {
                self.is_connected.store(false, Ordering::Release);
                STTError::AudioProcessingError("Audio channel closed".to_string())
            })?;
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.write().await = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.write().await = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.config.as_ref().map(|c| &c.base)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        let gnani_config =
            GnaniSTTConfig::from_base(config).map_err(STTError::ConfigurationError)?;
        self.config = Some(gnani_config);
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Gnani.ai Vachana STT - Indic Speech-to-Text with 14 language support via gRPC streaming"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "gnani".to_string(),
            api_key: String::new(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        }
    }

    #[test]
    fn test_gnani_stt_creation() {
        let config = create_test_config();
        let result = GnaniSTT::create(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_gnani_stt_not_connected_initially() {
        let config = create_test_config();
        let stt = GnaniSTT::create(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_gnani_stt_send_audio_requires_connection() {
        let config = create_test_config();
        let mut stt = GnaniSTT::create(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(b"test")).await;
        assert!(result.is_err());
        match result {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("Not connected"));
            }
            _ => panic!("Expected ConnectionFailed error"),
        }
    }

    #[test]
    fn test_gnani_stt_provider_info() {
        let config = create_test_config();
        let stt = GnaniSTT::create(config).unwrap();
        assert!(stt.get_provider_info().contains("Gnani"));
        assert!(stt.get_provider_info().contains("gRPC"));
    }

    #[tokio::test]
    async fn test_gnani_stt_callback_registration() {
        let config = create_test_config();
        let mut stt = GnaniSTT::create(config).unwrap();

        let callback = Arc::new(|_result: STTResult| {
            Box::pin(async move {})
                as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_gnani_stt_connect_requires_credentials() {
        let config = create_test_config();
        let mut stt = GnaniSTT::create(config).unwrap();

        // Should fail because no credentials are set
        let result = stt.connect().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_gnani_stt_default() {
        let stt = GnaniSTT::default();
        assert!(!stt.is_ready());
        assert!(stt.config.is_none());
    }

    #[tokio::test]
    async fn test_gnani_stt_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = GnaniSTT::create(config).unwrap();

        // Should not error when disconnecting without being connected
        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }
}

//! Tinkoff VoiceKit STT Provider Implementation
//!
//! Implements the BaseSTT trait for Tinkoff's gRPC-based Speech-to-Text service.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{Notify, RwLock, mpsc, oneshot};
use tracing::{debug, error, info};

use super::config::TinkoffSttConfig;
use super::grpc::{TinkoffGrpcClient, create_tinkoff_channel};
use super::messages::{RecognitionConfig, StreamingRecognitionConfig};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

#[derive(Debug, Clone)]
pub(super) enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    #[allow(dead_code)]
    Error(String),
}

/// Audio channel buffer size
const AUDIO_CHANNEL_BUFFER_SIZE: usize = 32;

/// Tinkoff VoiceKit STT provider
pub struct TinkoffStt {
    pub(super) config: Option<TinkoffSttConfig>,
    pub(super) state: ConnectionState,
    pub(super) state_notify: Arc<Notify>,
    pub(super) audio_sender: Option<mpsc::Sender<Bytes>>,
    pub(super) shutdown_tx: Option<oneshot::Sender<()>>,
    pub(super) connection_handle: Option<tokio::task::JoinHandle<()>>,
    pub(super) result_forward_handle: Option<tokio::task::JoinHandle<()>>,
    pub(super) error_forward_handle: Option<tokio::task::JoinHandle<()>>,
    pub(super) result_callback: Arc<RwLock<Option<AsyncSTTCallback>>>,
    pub(super) error_callback: Arc<RwLock<Option<AsyncErrorCallback>>>,
}

impl Default for TinkoffStt {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            audio_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        }
    }
}

impl TinkoffStt {
    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// Tinkoff can express (interim/partial results) are honored END-TO-END. The flat
    /// `BaseSTT::new` path resets those to provider defaults; this is the reachable
    /// standardized path. Mirrors `DeepgramSTT::new_standard`.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        let tinkoff_config =
            TinkoffSttConfig::from_standard(std).map_err(STTError::ConfigurationError)?;
        Ok(Self {
            config: Some(tinkoff_config),
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            audio_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        })
    }

    /// Create streaming recognition config from provider config
    fn create_streaming_config(config: &TinkoffSttConfig) -> StreamingRecognitionConfig {
        // Map provider-side speech contexts to wire messages (phrase boosting, field 6).
        let speech_contexts = config
            .speech_contexts
            .iter()
            .map(|ctx| super::messages::SpeechContext {
                phrases: ctx
                    .phrases
                    .iter()
                    .map(|p| super::messages::SpeechContextPhrase {
                        text: p.text.clone(),
                        score: p.score,
                    })
                    .collect(),
                speech_context_dictionary_id: ctx.speech_context_dictionary_id.clone(),
            })
            .collect();

        StreamingRecognitionConfig {
            config: RecognitionConfig {
                encoding: config.encoding,
                sample_rate_hertz: config.base.sample_rate,
                language_code: config.base.language.clone(),
                max_alternatives: config.max_alternatives,
                profanity_filter: config.profanity_filter,
                speech_contexts,
                enable_automatic_punctuation: config.enable_punctuation,
                num_channels: config.base.channels as u32,
                // oneof vad: `do_not_perform_vad` (field 13) takes precedence over `vad_config`
                // (field 14); never emit both.
                do_not_perform_vad: config.do_not_perform_vad,
                vad: if config.do_not_perform_vad {
                    None
                } else {
                    config.vad_config.clone()
                },
                enable_denormalization: config.enable_denormalization,
                enable_gender_identification: config.enable_gender_identification,
            },
            interim_results: config.interim_results,
            single_utterance: config.single_utterance,
            interim_results_interval: config.interim_results_interval,
        }
    }

    async fn start_connection(&mut self, config: TinkoffSttConfig) -> Result<(), STTError> {
        // Validate configuration
        config.validate().map_err(STTError::ConfigurationError)?;

        // Create channels
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER_SIZE);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(64);
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.audio_sender = Some(audio_tx);
        self.shutdown_tx = Some(shutdown_tx);

        let streaming_config = Self::create_streaming_config(&config);
        let callback_ref = self.result_callback.clone();
        let error_callback_ref = self.error_callback.clone();
        let config_clone = config.clone();

        let connection_handle = tokio::spawn(async move {
            // Create gRPC channel
            let channel = match create_tinkoff_channel(&config_clone).await {
                Ok(ch) => ch,
                Err(e) => {
                    error!("Failed to create Tinkoff gRPC channel: {}", e);
                    let _ = error_tx.try_send(e);
                    return;
                }
            };

            // Create gRPC client
            let client = TinkoffGrpcClient::new(channel, config_clone);

            // Start streaming
            let (grpc_audio_tx, mut result_rx) =
                match client.start_streaming(streaming_config).await {
                    Ok(streams) => streams,
                    Err(e) => {
                        error!("Failed to start Tinkoff streaming: {}", e);
                        let _ = error_tx.try_send(e);
                        return;
                    }
                };

            info!("Connected to Tinkoff VoiceKit STT");

            // Signal successful connection
            let _ = connected_tx.send(());

            // Forward audio from our channel to gRPC channel
            let audio_forward_handle = tokio::spawn(async move {
                let mut audio_rx = audio_rx;
                while let Some(audio) = audio_rx.recv().await {
                    if grpc_audio_tx.send(audio).await.is_err() {
                        debug!("gRPC audio channel closed");
                        break;
                    }
                }
            });

            // Process results
            loop {
                tokio::select! {
                    result_opt = result_rx.recv() => {
                        match result_opt {
                            Some(Ok(response)) => {
                                // Convert to STTResult
                                for result in response.results {
                                    if let Some(alt) = result.alternatives.first() {
                                        let stt_result = STTResult::new(
                                            alt.transcript.clone(),
                                            result.is_final,
                                            result.is_final,
                                            alt.confidence,
                                        );

                                        // Call result callback
                                        let callback_opt = {
                                            let guard = callback_ref.read().await;
                                            guard.as_ref().map(|cb| cb(stt_result.clone()))
                                        };

                                        if let Some(future) = callback_opt {
                                            future.await;
                                        } else {
                                            debug!(
                                                "Received STT result but no callback: {} (confidence: {})",
                                                stt_result.transcript, stt_result.confidence
                                            );
                                        }
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                error!("Tinkoff streaming error: {}", e);
                                // Call error callback
                                let callback_opt = {
                                    let guard = error_callback_ref.read().await;
                                    guard.as_ref().map(|cb| cb(e.clone()))
                                };

                                if let Some(future) = callback_opt {
                                    future.await;
                                }
                                break;
                            }
                            None => {
                                info!("Tinkoff result stream ended");
                                break;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        info!("Shutdown signal received");
                        break;
                    }
                }
            }

            audio_forward_handle.abort();
            info!("Tinkoff VoiceKit STT connection closed");
        });

        self.connection_handle = Some(connection_handle);

        // Error forwarding task
        let error_callback_ref = self.error_callback.clone();
        let error_forward_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                let callback_opt = {
                    let guard = error_callback_ref.read().await;
                    guard.as_ref().map(|cb| cb(error.clone()))
                };

                if let Some(future) = callback_opt {
                    future.await;
                } else {
                    error!("STT streaming error but no error callback: {}", error);
                }
            }
        });

        self.error_forward_handle = Some(error_forward_handle);
        self.state = ConnectionState::Connecting;

        // Wait for connection to be established
        match tokio::time::timeout(Duration::from_secs(30), connected_rx).await {
            Ok(Ok(())) => {
                self.state = ConnectionState::Connected;
                self.state_notify.notify_waiters();
                info!("Successfully connected to Tinkoff VoiceKit STT");
                Ok(())
            }
            Ok(Err(_)) => {
                let error_msg = "Connection channel closed unexpectedly".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
            Err(_) => {
                let error_msg = "Connection timeout (30s)".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for TinkoffStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        let tinkoff_config =
            TinkoffSttConfig::from_base(config).map_err(STTError::ConfigurationError)?;

        Ok(Self {
            config: Some(tinkoff_config),
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            audio_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let config = self.config.clone().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;

        self.start_connection(config).await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        if let Some(handle) = self.connection_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        if let Some(handle) = self.result_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        if let Some(handle) = self.error_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        self.audio_sender = None;
        *self.result_callback.write().await = None;
        *self.error_callback.write().await = None;

        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from Tinkoff VoiceKit STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, ConnectionState::Connected) && self.audio_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Tinkoff VoiceKit STT".to_string(),
            ));
        }

        if let Some(audio_sender) = &self.audio_sender {
            let data_len = audio_data.len();

            audio_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio data: {}", e)))?;

            debug!("Sent {} bytes of audio data to Tinkoff STT", data_len);
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.write().await = Some(Box::new(move |result| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(result).await;
            })
        }));
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.write().await = Some(Box::new(move |error| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(error).await;
            })
        }));
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.config.as_ref().map(|c| &c.base)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        let tinkoff_config =
            TinkoffSttConfig::from_base(config).map_err(STTError::ConfigurationError)?;

        self.config = Some(tinkoff_config);
        self.connect().await?;
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Tinkoff VoiceKit Speech-to-Text"
    }
}

impl Drop for TinkoffStt {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "tinkoff".to_string(),
            api_key: "test-api-key".to_string(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "default".to_string(),
        }
    }

    // W1 keystone: an advanced feature Tinkoff supports (interim/partial results) must survive
    // through `new_standard` into the provider-specific config, instead of being reset to the
    // provider default by the flat path. RED until `new_standard` maps it.
    #[test]
    fn test_tinkoff_new_standard_unlocks_advanced_features() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tinkoff".into(),
                api_key: "test-api-key".into(),
                language: "ru-RU".into(),
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(false),
                ..Default::default()
            },
            ..StandardSTTConfig::from_base(STTConfig::default())
        };
        let stt = TinkoffStt::new_standard(&std).unwrap();
        let cfg = stt.config.as_ref().expect("config present");
        // interim_results survived from the standardized feature (provider default is true).
        assert!(!cfg.interim_results);
        assert_eq!(cfg.api_key, "test-api-key");
    }

    #[test]
    fn test_tinkoff_new_standard_requires_api_key() {
        use crate::core::stt::standard::StandardSTTConfig;
        let std = StandardSTTConfig::from_base(STTConfig {
            provider: "tinkoff".into(),
            api_key: String::new(),
            ..Default::default()
        });
        assert!(TinkoffStt::new_standard(&std).is_err());
    }

    #[test]
    fn test_tinkoff_stt_new() {
        let config = create_test_config();
        let stt = TinkoffStt::new(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert!(stt.config.is_some());
    }

    #[test]
    fn test_tinkoff_stt_default() {
        let stt = TinkoffStt::default();
        assert!(!stt.is_ready());
        assert!(stt.config.is_none());
    }

    #[test]
    fn test_tinkoff_stt_get_provider_info() {
        let config = create_test_config();
        let stt = TinkoffStt::new(config).unwrap();
        assert_eq!(stt.get_provider_info(), "Tinkoff VoiceKit Speech-to-Text");
    }

    #[test]
    fn test_tinkoff_stt_get_config() {
        let config = create_test_config();
        let stt = TinkoffStt::new(config).unwrap();

        let retrieved_config = stt.get_config();
        assert!(retrieved_config.is_some());

        let config = retrieved_config.unwrap();
        assert_eq!(config.language, "ru-RU");
        assert_eq!(config.sample_rate, 16000);
    }

    #[tokio::test]
    async fn test_tinkoff_stt_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = TinkoffStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(&[0x01, 0x02])).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), STTError::ConnectionFailed(_)));
    }

    #[tokio::test]
    async fn test_tinkoff_stt_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = TinkoffStt::new(config).unwrap();

        // Disconnecting when not connected should succeed
        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_streaming_config() {
        let mut tinkoff_config = TinkoffSttConfig::default();
        tinkoff_config.base.sample_rate = 16000;
        tinkoff_config.base.language = "ru-RU".to_string();
        tinkoff_config.interim_results = true;
        tinkoff_config.single_utterance = false;

        let streaming_config = TinkoffStt::create_streaming_config(&tinkoff_config);

        assert_eq!(streaming_config.config.sample_rate_hertz, 16000);
        assert_eq!(streaming_config.config.language_code, "ru-RU");
        assert!(streaming_config.interim_results);
        assert!(!streaming_config.single_utterance);
    }

    // WIRE-LEVEL end-to-end: the standardized features must reach the ENCODED protobuf bytes of the
    // first streaming request — `StandardSTTConfig` → `from_standard` → `create_streaming_config`
    // → `StreamingRecognizeRequest::config(..).encode()` (the exact bytes the gRPC stream sends).
    // This guards the recurring "set on config struct, never serialized to the wire" bug class.
    #[test]
    fn test_streaming_features_reach_encoded_request_e2e() {
        use super::super::messages::StreamingRecognizeRequest;
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};

        let mut extras = serde_json::Map::new();
        extras.insert("interim_results_config.interval".into(), serde_json::json!(0.25));
        extras.insert("enable_gender_identification".into(), serde_json::json!(true));
        extras.insert("vad_config.silence_max".into(), serde_json::json!(1.5));
        extras.insert("vad_config.silence_min".into(), serde_json::json!(0.3));

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tinkoff".into(),
                api_key: "test-api-key".into(),
                language: "ru-RU".into(),
                sample_rate: 16000,
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(true),
                profanity_filter: Some(true),
                numerals: Some(true),
                keyterms: Some(vec!["Тинькофф".into()]),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };

        let cfg = TinkoffSttConfig::from_standard(&std).unwrap();
        let streaming_config = TinkoffStt::create_streaming_config(&cfg);
        // Encode the actual first-message bytes the AudioChunkStream emits onto the gRPC stream.
        let bytes = StreamingRecognizeRequest::config(streaming_config).encode();

        let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);

        // profanity_filter (field 5) bytes 0x28 0x01 must be present inside the embedded config.
        assert!(contains(&[0x28, 0x01]), "profanity_filter missing from encoded request");
        // speech_contexts phrase text bytes must be present.
        assert!(contains("Тинькофф".as_bytes()), "speech context phrase missing from encoded request");
        // enable_denormalization (field 16) bytes 0x80 0x01 0x01.
        assert!(contains(&[0x80, 0x01, 0x01]), "enable_denormalization missing from encoded request");
        // enable_gender_identification (field 18) bytes 0x90 0x01 0x01.
        assert!(contains(&[0x90, 0x01, 0x01]), "enable_gender_identification missing from encoded request");
        // VAD silence_max (1.5) and silence_min (0.3) float bytes.
        assert!(contains(&1.5f32.to_le_bytes()), "vad silence_max missing from encoded request");
        assert!(contains(&0.3f32.to_le_bytes()), "vad silence_min missing from encoded request");
        // interim_results_config.interval (0.25) float bytes.
        assert!(contains(&0.25f32.to_le_bytes()), "interim interval missing from encoded request");
    }
}

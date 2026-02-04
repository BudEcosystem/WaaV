//! Tinkoff VoiceKit TTS Provider Implementation
//!
//! Implements the BaseTTS trait for Tinkoff's gRPC-based Text-to-Speech service.

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};
use tracing::{debug, info, warn};

use super::config::TinkoffTtsConfig;
use super::grpc::{TinkoffTtsGrpcClient, create_tinkoff_tts_channel};
use super::messages::{StreamingSynthesizeSpeechResponse, SynthesizeSpeechRequest};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};

/// Tinkoff VoiceKit TTS provider
pub struct TinkoffTts {
    config: Option<TinkoffTtsConfig>,
    state: ConnectionState,
    state_notify: Arc<Notify>,
    client: Option<TinkoffTtsGrpcClient>,
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
}

impl Default for TinkoffTts {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            client: None,
            audio_callback: Arc::new(RwLock::new(None)),
        }
    }
}

impl TinkoffTts {
    /// Create synthesis request from text
    fn create_synthesis_request(&self, text: &str) -> Result<SynthesizeSpeechRequest, TTSError> {
        let config = self.config.as_ref().ok_or_else(|| {
            TTSError::InvalidConfiguration("No configuration available".to_string())
        })?;

        Ok(SynthesizeSpeechRequest::new(
            text,
            config.voice.as_str(),
            config.encoding,
            config.sample_rate,
        ))
    }

    /// Synthesize text using non-streaming API
    async fn synthesize_non_streaming(&self, text: &str) -> TTSResult<Bytes> {
        let client = self.client.as_ref().ok_or_else(|| {
            TTSError::ConnectionFailed("Not connected to Tinkoff TTS".to_string())
        })?;

        let request = self.create_synthesis_request(text)?;
        let response = client.synthesize(request).await?;

        Ok(response.audio_content)
    }

    /// Synthesize text using streaming API for lower latency
    async fn synthesize_streaming(&self, text: &str) -> TTSResult<()> {
        let client = self.client.as_ref().ok_or_else(|| {
            TTSError::ConnectionFailed("Not connected to Tinkoff TTS".to_string())
        })?;

        let config = self.config.as_ref().ok_or_else(|| {
            TTSError::InvalidConfiguration("No configuration available".to_string())
        })?;

        let request = self.create_synthesis_request(text)?;
        let mut stream = client.streaming_synthesize(request).await?;

        use futures::StreamExt;

        let callback_ref = self.audio_callback.clone();
        let sample_rate = config.sample_rate;
        let format = config.encoding.mime_type().to_string();

        while let Some(result) = stream.next().await {
            match result {
                Ok(data) => match StreamingSynthesizeSpeechResponse::decode(&data) {
                    Ok(response) => {
                        if !response.audio_chunk.is_empty() {
                            let audio_data = AudioData {
                                data: response.audio_chunk.to_vec(),
                                sample_rate,
                                format: format.clone(),
                                duration_ms: None,
                            };

                            let callback_guard = callback_ref.read().await;
                            if let Some(callback) = callback_guard.as_ref() {
                                callback.on_audio(audio_data).await;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to decode streaming response");
                    }
                },
                Err(status) => {
                    let error = super::grpc::grpc_status_to_tts_error(status);
                    let callback_guard = callback_ref.read().await;
                    if let Some(callback) = callback_guard.as_ref() {
                        callback.on_error(error.clone()).await;
                    }
                    return Err(error);
                }
            }
        }

        // Signal completion
        let callback_guard = callback_ref.read().await;
        if let Some(callback) = callback_guard.as_ref() {
            callback.on_complete().await;
        }

        Ok(())
    }
}

#[async_trait]
impl BaseTTS for TinkoffTts {
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        let tinkoff_config =
            TinkoffTtsConfig::from_base(config).map_err(TTSError::InvalidConfiguration)?;

        Ok(Self {
            config: Some(tinkoff_config),
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            client: None,
            audio_callback: Arc::new(RwLock::new(None)),
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        let config = self.config.clone().ok_or_else(|| {
            TTSError::InvalidConfiguration("No configuration available".to_string())
        })?;

        // Validate configuration
        config.validate().map_err(TTSError::InvalidConfiguration)?;

        self.state = ConnectionState::Connecting;
        self.state_notify.notify_waiters();

        // Create gRPC channel
        let channel = create_tinkoff_tts_channel(&config).await?;

        // Create client
        self.client = Some(TinkoffTtsGrpcClient::new(channel, config));

        self.state = ConnectionState::Connected;
        self.state_notify.notify_waiters();

        info!("Connected to Tinkoff VoiceKit TTS");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.client = None;
        *self.audio_callback.write().await = None;

        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from Tinkoff VoiceKit TTS");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, ConnectionState::Connected) && self.client.is_some()
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.state.clone()
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        if !self.is_ready() {
            // Try to reconnect
            self.connect().await?;
        }

        if text.is_empty() {
            return Ok(());
        }

        let config = self.config.as_ref().ok_or_else(|| {
            TTSError::InvalidConfiguration("No configuration available".to_string())
        })?;

        debug!(
            text_len = text.len(),
            voice = config.voice.as_str(),
            encoding = config.encoding.as_str(),
            "Synthesizing text with Tinkoff TTS"
        );

        // Use streaming for better latency when callback is registered
        let has_callback = self.audio_callback.read().await.is_some();

        if has_callback && config.encoding.supports_streaming() {
            self.synthesize_streaming(text).await
        } else {
            // Non-streaming synthesis
            let audio_content = self.synthesize_non_streaming(text).await?;

            let audio_data = AudioData {
                data: audio_content.to_vec(),
                sample_rate: config.sample_rate,
                format: config.encoding.mime_type().to_string(),
                duration_ms: None,
            };

            let callback_guard = self.audio_callback.read().await;
            if let Some(callback) = callback_guard.as_ref() {
                callback.on_audio(audio_data).await;
                callback.on_complete().await;
            }

            Ok(())
        }
    }

    async fn clear(&mut self) -> TTSResult<()> {
        // Tinkoff TTS is request-based, no queue to clear
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // Tinkoff TTS processes requests immediately, no buffering
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        let callback_ref = self.audio_callback.clone();
        tokio::spawn(async move {
            *callback_ref.write().await = Some(callback);
        });
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        let callback_ref = self.audio_callback.clone();
        tokio::spawn(async move {
            *callback_ref.write().await = None;
        });
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        let config = self.config.as_ref();

        serde_json::json!({
            "provider": "tinkoff",
            "name": "Tinkoff VoiceKit TTS",
            "version": "1.0.0",
            "endpoint": super::TINKOFF_GRPC_ENDPOINT,
            "language": "ru-RU",
            "voices": [
                {
                    "id": "alyona",
                    "name": "Alyona",
                    "gender": "female",
                    "language": "ru-RU"
                },
                {
                    "id": "dorofeev",
                    "name": "Dorofeev",
                    "gender": "male",
                    "language": "ru-RU"
                }
            ],
            "encodings": ["linear16", "opus", "alaw"],
            "features": {
                "streaming": true,
                "ssml": true,
                "speaking_rate": true,
                "pitch": true,
                "volume_gain": true
            },
            "current_config": config.map(|c| serde_json::json!({
                "voice": c.voice.as_str(),
                "encoding": c.encoding.as_str(),
                "sample_rate": c.sample_rate,
                "speaking_rate": c.speaking_rate,
                "pitch": c.pitch
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "tinkoff".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("alyona".to_string()),
            model: "default".to_string(),
            audio_format: Some("linear16".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        }
    }

    #[test]
    fn test_tinkoff_tts_new() {
        let config = create_test_config();
        let tts = TinkoffTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert!(!tts.is_ready());
        assert!(tts.config.is_some());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_tinkoff_tts_default() {
        let tts = TinkoffTts::default();
        assert!(!tts.is_ready());
        assert!(tts.config.is_none());
    }

    #[test]
    fn test_tinkoff_tts_get_provider_info() {
        let config = create_test_config();
        let tts = TinkoffTts::new(config).unwrap();

        let info = tts.get_provider_info();
        assert_eq!(info["provider"], "tinkoff");
        assert_eq!(info["name"], "Tinkoff VoiceKit TTS");
        assert_eq!(info["language"], "ru-RU");

        let voices = info["voices"].as_array().unwrap();
        assert_eq!(voices.len(), 2);
        assert_eq!(voices[0]["id"], "alyona");
        assert_eq!(voices[1]["id"], "dorofeev");
    }

    #[test]
    fn test_create_synthesis_request() {
        let config = create_test_config();
        let tts = TinkoffTts::new(config).unwrap();

        let request = tts.create_synthesis_request("Привет, мир!");
        assert!(request.is_ok());

        let request = request.unwrap();
        let encoded = request.encode();
        assert!(!encoded.is_empty());
    }

    #[tokio::test]
    async fn test_tinkoff_tts_speak_not_connected() {
        let config = create_test_config();
        let mut tts = TinkoffTts::new(config).unwrap();

        // Should fail because connect() will fail without real credentials
        let result = tts.speak("Привет", false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tinkoff_tts_speak_empty_text() {
        let config = create_test_config();
        let mut tts = TinkoffTts::new(config).unwrap();

        // Empty text should return Ok without doing anything
        // Note: This will still fail at connect, but empty text check is first
        tts.state = ConnectionState::Connected;
        tts.client = None; // Simulate connected state without real client

        // With no client, it should return error trying to connect
        let result = tts.speak("", false).await;
        // Empty text returns Ok before trying to synthesize
        // But is_ready() returns false, so it tries to connect
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tinkoff_tts_disconnect() {
        let config = create_test_config();
        let mut tts = TinkoffTts::new(config).unwrap();

        // Disconnect should succeed even when not connected
        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_tinkoff_tts_clear() {
        let config = create_test_config();
        let mut tts = TinkoffTts::new(config).unwrap();

        // Clear should always succeed (no-op for request-based TTS)
        let result = tts.clear().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tinkoff_tts_flush() {
        let config = create_test_config();
        let tts = TinkoffTts::new(config).unwrap();

        // Flush should always succeed (no buffering)
        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    struct MockAudioCallback {
        audio_count: AtomicUsize,
        error_count: AtomicUsize,
        complete_count: AtomicUsize,
    }

    impl MockAudioCallback {
        fn new() -> Self {
            Self {
                audio_count: AtomicUsize::new(0),
                error_count: AtomicUsize::new(0),
                complete_count: AtomicUsize::new(0),
            }
        }
    }

    impl AudioCallback for MockAudioCallback {
        fn on_audio(
            &self,
            _audio_data: AudioData,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.audio_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }

        fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.error_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }

        fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.complete_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn test_tinkoff_tts_callback_registration() {
        let config = create_test_config();
        let mut tts = TinkoffTts::new(config).unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        let result = tts.on_audio(callback);
        assert!(result.is_ok());

        // Give the async task time to complete
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Verify callback was registered
        let callback_guard = tts.audio_callback.read().await;
        assert!(callback_guard.is_some());
    }

    #[tokio::test]
    async fn test_tinkoff_tts_callback_removal() {
        let config = create_test_config();
        let mut tts = TinkoffTts::new(config).unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        tts.on_audio(callback).unwrap();

        // Give the async task time to complete
        tokio::time::sleep(Duration::from_millis(10)).await;

        let result = tts.remove_audio_callback();
        assert!(result.is_ok());

        // Give the async task time to complete
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Verify callback was removed
        let callback_guard = tts.audio_callback.read().await;
        assert!(callback_guard.is_none());
    }

    #[test]
    fn test_tinkoff_tts_config_with_different_voices() {
        let mut config = create_test_config();
        config.voice_id = Some("dorofeev".to_string());

        let tts = TinkoffTts::new(config).unwrap();
        let tinkoff_config = tts.config.as_ref().unwrap();

        assert_eq!(tinkoff_config.voice.as_str(), "dorofeev");
        assert_eq!(tinkoff_config.voice.gender(), "male");
    }

    #[test]
    fn test_tinkoff_tts_config_with_different_encodings() {
        let mut config = create_test_config();
        config.audio_format = Some("opus".to_string());

        let tts = TinkoffTts::new(config).unwrap();
        let tinkoff_config = tts.config.as_ref().unwrap();

        assert_eq!(tinkoff_config.encoding.as_str(), "RAW_OPUS");
        assert!(tinkoff_config.encoding.supports_streaming());
    }

    #[test]
    fn test_tinkoff_tts_config_invalid_voice() {
        let mut config = create_test_config();
        config.voice_id = Some("invalid_voice".to_string());

        let result = TinkoffTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_tinkoff_tts_config_invalid_encoding() {
        let mut config = create_test_config();
        config.audio_format = Some("invalid_format".to_string());

        let result = TinkoffTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_tinkoff_tts_config_invalid_sample_rate() {
        let mut config = create_test_config();
        config.sample_rate = Some(500); // Below minimum

        let result = TinkoffTts::new(config);
        assert!(result.is_err());
    }
}

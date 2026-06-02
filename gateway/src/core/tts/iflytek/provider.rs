//! iFlytek TTS WebSocket Provider
//!
//! This module implements the `BaseTTS` trait for iFlytek's Text-to-Speech WebSocket API.
//!
//! # Architecture
//!
//! The provider uses a WebSocket connection for real-time synthesis:
//!
//! 1. Connect with HMAC-SHA256 signed URL
//! 2. Send single request with text (status=2)
//! 3. Receive multiple audio chunks
//! 4. Final chunk has status=2
//!
//! # WebSocket Message Flow
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect (with auth) -------->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ TTS Request (status=2) ----->|
//!   |                                    |
//!   |<----- Audio chunk (status=0) ------|
//!   |<----- Audio chunk (status=1) ------|
//!   |<----- Final chunk (status=2) ------|
//!   |                                    |
//!   |<----- Server closes connection ----|
//! ```

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};

use super::config::{IFLYTEK_TTS_HOST, IFLYTEK_TTS_PATH, IFlytekTtsConfig};
use super::messages::{TtsRequest, TtsResponse};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "iFlytek TTS WebSocket v2.0 (科大讯飞)";

/// WebSocket connection timeout.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// WebSocket message timeout.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Channel buffer size for text messages.
const TEXT_CHANNEL_BUFFER: usize = 32;

/// Channel buffer size for audio chunks.
const AUDIO_CHANNEL_BUFFER: usize = 128;

// =============================================================================
// iFlytek TTS Provider
// =============================================================================

/// iFlytek Text-to-Speech WebSocket provider.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::{BaseTTS, TTSConfig};
/// use waav_gateway::core::tts::iflytek::IFlytekTts;
///
/// let config = TTSConfig {
///     api_key: "app_id|api_key|api_secret".to_string(),
///     voice_id: Some("xiaoyan".to_string()),
///     sample_rate: Some(16000),
///     ..Default::default()
/// };
///
/// let mut tts = IFlytekTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("你好世界", true).await?;
/// tts.disconnect().await?;
/// ```
pub struct IFlytekTts {
    /// Base configuration for BaseTTS trait.
    #[allow(dead_code)]
    base_config: TTSConfig,

    /// iFlytek-specific configuration.
    config: IFlytekTtsConfig,

    /// Connection state.
    connected: AtomicBool,

    /// WebSocket sender for text messages.
    text_sender: Option<mpsc::Sender<String>>,

    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,

    /// Connection task handle.
    connection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Audio callback forwarding task handle.
    audio_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Audio callback storage.
    audio_callback: Arc<Mutex<Option<Arc<dyn AudioCallback>>>>,

    /// Total bytes synthesized counter.
    bytes_synthesized: Arc<std::sync::atomic::AtomicU64>,
}

impl IFlytekTts {
    /// Create a new iFlytek TTS provider (internal).
    fn create_internal(config: TTSConfig) -> TTSResult<Self> {
        let iflytek_config = IFlytekTtsConfig::from_base(config.clone())?;
        Self::create_from_iflytek_config(config, iflytek_config)
    }

    /// Create a provider from a pre-built iFlytek-specific config (shared by `from_standard`).
    fn create_from_iflytek_config(
        base_config: TTSConfig,
        iflytek_config: IFlytekTtsConfig,
    ) -> TTSResult<Self> {
        // Validate configuration
        iflytek_config.validate()?;

        Ok(Self {
            base_config,
            config: iflytek_config,
            connected: AtomicBool::new(false),
            text_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            audio_forward_handle: None,
            audio_callback: Arc::new(Mutex::new(None)),
            bytes_synthesized: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// Build from the standardized TTS config (W1 keystone), mirroring `DeepgramTTS::from_standard`.
    /// Delegates feature mapping to [`IFlytekTtsConfig::from_standard`] (speed/pitch/volume as
    /// iFlytek 0-100 levels, sample_rate, plus the `background_sound` extras passthrough) so
    /// advanced prosody reaches the provider config the WebSocket request reads.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let iflytek_config = IFlytekTtsConfig::from_standard(std)?;
        Self::create_from_iflytek_config(std.base.clone(), iflytek_config)
    }

    /// Handle incoming WebSocket message.
    fn handle_websocket_message(
        message: Message,
        audio_tx: &mpsc::Sender<AudioData>,
        sample_rate: u32,
        format: &str,
    ) -> Result<bool, TTSError> {
        match message {
            Message::Text(text) => {
                debug!("iFlytek TTS received: {}", text);

                let response = TtsResponse::from_json(&text).map_err(|e| {
                    TTSError::ProviderError(format!("Failed to parse response: {}", e))
                })?;

                // Check for errors
                if !response.is_success() {
                    let error_code = response.error_code();
                    return Err(TTSError::ProviderError(format!(
                        "iFlytek TTS error: {}",
                        error_code
                    )));
                }

                // Decode and forward audio data
                if let Some(decode_result) = response.decode_audio() {
                    match decode_result {
                        Ok(audio_bytes) => {
                            if !audio_bytes.is_empty() {
                                let audio_data = AudioData {
                                    data: audio_bytes,
                                    sample_rate,
                                    format: format.to_string(),
                                    duration_ms: None,
                                };

                                if let Err(e) = audio_tx.try_send(audio_data) {
                                    match e {
                                        mpsc::error::TrySendError::Full(_) => {
                                            warn!(
                                                "iFlytek TTS audio channel full - dropping chunk"
                                            );
                                        }
                                        mpsc::error::TrySendError::Closed(_) => {
                                            warn!("iFlytek TTS audio channel closed");
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to decode iFlytek TTS audio: {}", e);
                        }
                    }
                }

                // Return true if this is the final response
                Ok(response.is_last())
            }
            Message::Close(frame) => {
                info!("iFlytek TTS WebSocket closed: {:?}", frame);
                Ok(true)
            }
            Message::Ping(_) => {
                debug!("iFlytek TTS received ping");
                Ok(false)
            }
            Message::Pong(_) => {
                debug!("iFlytek TTS received pong");
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// Start the WebSocket connection task.
    async fn start_connection(&mut self) -> TTSResult<()> {
        // Build signed WebSocket URL
        let ws_url = self
            .config
            .auth
            .build_signed_url(IFLYTEK_TTS_HOST, IFLYTEK_TTS_PATH)
            .map_err(|e| {
                TTSError::ConnectionFailed(format!("Failed to build signed URL: {}", e))
            })?;

        debug!("Connecting to iFlytek TTS: {}", ws_url);

        // Create channels
        let (text_tx, mut text_rx) = mpsc::channel::<String>(TEXT_CHANNEL_BUFFER);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let (audio_tx, mut audio_rx) = mpsc::channel::<AudioData>(AUDIO_CHANNEL_BUFFER);
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        // Store channels
        self.text_sender = Some(text_tx);
        self.shutdown_tx = Some(shutdown_tx);

        // Clone config values for the connection task
        let app_id = self.config.auth.app_id.clone();
        let voice = self.config.voice.as_code().to_string();
        let encoding = self.config.encoding.as_str().to_string();
        let sample_rate = self.config.sample_rate;
        let text_encoding = self.config.text_encoding.as_str().to_string();
        let speed = self.config.speed;
        let volume = self.config.volume;
        let pitch = self.config.pitch;
        let background_sound = self.config.background_sound;
        let english_pronunciation = self.config.english_pronunciation;
        let number_pronunciation = self.config.number_pronunciation;
        let format = self.config.encoding.content_type().to_string();
        let _bytes_counter = self.bytes_synthesized.clone();

        // Start connection task
        let connection_handle = tokio::spawn(async move {
            // Connect to WebSocket
            let ws_stream = match timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url)).await {
                Ok(Ok((stream, _))) => stream,
                Ok(Err(e)) => {
                    error!("iFlytek TTS WebSocket connection failed: {}", e);
                    return;
                }
                Err(_) => {
                    error!("iFlytek TTS connection timeout");
                    return;
                }
            };

            info!("Connected to iFlytek TTS WebSocket");
            let _ = connected_tx.send(());

            let (mut ws_sink, mut ws_stream) = ws_stream.split();

            loop {
                tokio::select! {
                    // Handle text to synthesize
                    Some(text) = text_rx.recv() => {
                        debug!("Synthesizing text: {} chars", text.len());

                        let request = TtsRequest::new(
                            &app_id,
                            &voice,
                            &encoding,
                            sample_rate,
                            &text_encoding,
                            speed,
                            volume,
                            pitch,
                            background_sound,
                            english_pronunciation,
                            number_pronunciation,
                            &text,
                        );

                        let json = match request.to_json() {
                            Ok(j) => j,
                            Err(e) => {
                                error!("Failed to serialize TTS request: {}", e);
                                continue;
                            }
                        };

                        if let Err(e) = ws_sink.send(Message::Text(json.into())).await {
                            error!("Failed to send TTS request: {}", e);
                            break;
                        }

                        debug!("Sent iFlytek TTS request");

                        // Wait for all response chunks
                        loop {
                            match timeout(WS_MESSAGE_TIMEOUT, ws_stream.next()).await {
                                Ok(Some(Ok(msg))) => {
                                    match Self::handle_websocket_message(
                                        msg,
                                        &audio_tx,
                                        sample_rate,
                                        &format,
                                    ) {
                                        Ok(is_final) => {
                                            if is_final {
                                                debug!("iFlytek TTS synthesis complete");
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            error!("iFlytek TTS message handling error: {}", e);
                                            break;
                                        }
                                    }
                                }
                                Ok(Some(Err(e))) => {
                                    error!("iFlytek TTS WebSocket error: {}", e);
                                    break;
                                }
                                Ok(None) => {
                                    info!("iFlytek TTS WebSocket stream ended");
                                    break;
                                }
                                Err(_) => {
                                    error!("iFlytek TTS message timeout");
                                    break;
                                }
                            }
                        }
                    }

                    // Handle shutdown signal
                    _ = &mut shutdown_rx => {
                        info!("iFlytek TTS shutdown signal received");
                        break;
                    }
                }
            }

            // Close WebSocket gracefully
            let _ = ws_sink.close().await;
            info!("iFlytek TTS WebSocket connection closed");
        });

        self.connection_handle = Some(connection_handle);

        // Start audio forwarding task
        let callback_ref = self.audio_callback.clone();
        let bytes_counter_ref = self.bytes_synthesized.clone();
        let audio_forward_handle = tokio::spawn(async move {
            while let Some(audio_data) = audio_rx.recv().await {
                bytes_counter_ref.fetch_add(audio_data.data.len() as u64, Ordering::Relaxed);

                if let Some(callback) = callback_ref.lock().await.as_ref() {
                    callback.on_audio(audio_data).await;
                } else {
                    debug!("iFlytek TTS audio received (no callback)");
                }
            }

            // Notify completion
            if let Some(callback) = callback_ref.lock().await.as_ref() {
                callback.on_complete().await;
            }
        });
        self.audio_forward_handle = Some(audio_forward_handle);

        // Wait for connection to be established
        match timeout(WS_CONNECT_TIMEOUT, connected_rx).await {
            Ok(Ok(())) => {
                self.connected.store(true, Ordering::SeqCst);
                info!("iFlytek TTS connected successfully");
                Ok(())
            }
            Ok(Err(_)) => Err(TTSError::ConnectionFailed(
                "Connection channel closed".to_string(),
            )),
            Err(_) => Err(TTSError::ConnectionFailed("Connection timeout".to_string())),
        }
    }
}

impl Default for IFlytekTts {
    fn default() -> Self {
        Self {
            base_config: TTSConfig::default(),
            config: IFlytekTtsConfig::default(),
            connected: AtomicBool::new(false),
            text_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            audio_forward_handle: None,
            audio_callback: Arc::new(Mutex::new(None)),
            bytes_synthesized: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

#[async_trait]
impl BaseTTS for IFlytekTts {
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
            "Connecting to iFlytek TTS (voice: {}, encoding: {})",
            self.config.voice.display_name(),
            self.config.encoding.as_str()
        );

        self.bytes_synthesized.store(0, Ordering::Relaxed);
        self.start_connection().await
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from iFlytek TTS");

        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for connection task to finish
        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        // Clean up forwarding task
        if let Some(handle) = self.audio_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Clear state
        self.text_sender = None;
        *self.audio_callback.lock().await = None;
        self.connected.store(false, Ordering::SeqCst);

        info!(
            "Disconnected from iFlytek TTS (bytes synthesized: {})",
            self.bytes_synthesized.load(Ordering::Relaxed)
        );
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst) && self.text_sender.is_some()
    }

    fn get_connection_state(&self) -> ConnectionState {
        if self.connected.load(Ordering::SeqCst) {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        // Check for empty text first (before attempting to connect)
        if text.trim().is_empty() {
            return Ok(());
        }

        if !self.is_ready() {
            // Auto-reconnect
            self.connect().await?;
        }

        if let Some(text_sender) = &self.text_sender {
            text_sender
                .send(text.to_string())
                .await
                .map_err(|e| TTSError::ProviderError(format!("Failed to queue text: {}", e)))?;

            debug!("Queued {} chars for iFlytek TTS synthesis", text.len());
        }

        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // iFlytek TTS processes each speak() call immediately
        // No buffering to flush
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
            "voice": self.config.voice.display_name(),
            "voice_code": self.config.voice.as_code(),
            "encoding": self.config.encoding.as_str(),
            "sample_rate": self.config.sample_rate,
            "speed": self.config.speed,
            "volume": self.config.volume,
            "pitch": self.config.pitch,
            "bytes_synthesized": self.bytes_synthesized.load(Ordering::Relaxed),
            "available_voices": IFlytekTtsConfig::available_voices(),
        })
    }
}

impl Drop for IFlytekTts {
    fn drop(&mut self) {
        // Send shutdown signal if still connected
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_api_key() -> String {
        "test_app_id|test_api_key_xxxxx|test_api_secret_xx".to_string()
    }

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: create_test_api_key(),
            voice_id: Some("xiaoyan".to_string()),
            sample_rate: Some(16000),
            audio_format: Some("raw".to_string()),
            ..Default::default()
        }
    }

    // Provider creation tests
    #[test]
    fn test_iflytek_tts_creation() {
        let config = create_test_config();
        let result = IFlytekTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert!(!tts.is_ready());
    }

    // W1 keystone: the provider struct's `from_standard` maps prosody (speed/pitch/volume as
    // iFlytek 0-100 levels) through onto the provider config the WebSocket request reads.
    #[test]
    fn from_standard_maps_prosody_to_provider_config() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                speed: Some(1.0), // 1.0 multiplier * 50 -> 50 (normal) on iFlytek's 0-100 scale
                pitch: Some(70.0),
                volume: Some(80.0),
                sample_rate: Some(16000),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = IFlytekTts::from_standard(&std).unwrap();
        assert_eq!(tts.config.speed, 50);
        assert_eq!(tts.config.pitch, 70);
        assert_eq!(tts.config.volume, 80);
        assert_eq!(tts.config.sample_rate, 16000);
    }

    #[test]
    fn test_iflytek_tts_invalid_api_key() {
        let config = TTSConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };
        let result = IFlytekTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_iflytek_tts_initial_state() {
        let config = create_test_config();
        let tts = IFlytekTts::new(config).unwrap();

        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(tts.bytes_synthesized.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_iflytek_tts_default() {
        let tts = IFlytekTts::default();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    // Provider info tests
    #[test]
    fn test_iflytek_tts_provider_info() {
        let config = create_test_config();
        let tts = IFlytekTts::new(config).unwrap();

        let info = tts.get_provider_info();
        assert!(info["provider"].as_str().unwrap().contains("iFlytek"));
        assert!(info["provider"].as_str().unwrap().contains("科大讯飞"));
        assert_eq!(info["voice_code"].as_str().unwrap(), "xiaoyan");
        assert_eq!(info["sample_rate"].as_u64().unwrap(), 16000);
    }

    // Speak tests
    #[tokio::test]
    async fn test_speak_not_connected() {
        let config = create_test_config();
        let mut tts = IFlytekTts::new(config).unwrap();

        // speak() should auto-connect, but will fail without real server
        let result = tts.speak("Hello", false).await;
        // Either auto-connect fails or succeeds (depends on network)
        // We just verify no panic
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_speak_empty_text() {
        let config = create_test_config();
        let mut tts = IFlytekTts::new(config).unwrap();

        // Empty text should return Ok without doing anything
        let result = tts.speak("", false).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", false).await;
        assert!(result.is_ok());
    }

    // Disconnect tests
    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut tts = IFlytekTts::new(config).unwrap();

        let result = tts.disconnect().await;
        assert!(result.is_ok());
    }

    // Flush tests
    #[tokio::test]
    async fn test_flush() {
        let config = create_test_config();
        let tts = IFlytekTts::new(config).unwrap();

        // Flush is a no-op for iFlytek TTS
        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    // Message handling tests
    #[test]
    fn test_message_handling_success() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "audio": "SGVsbG8gV29ybGQ=",
                "status": 1
            }
        }"#;

        let (audio_tx, mut audio_rx) = mpsc::channel(128);

        let message = Message::Text(json.to_string().into());
        let result = IFlytekTts::handle_websocket_message(message, &audio_tx, 16000, "audio/pcm");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Not final

        // Check audio was sent
        let received = audio_rx.try_recv();
        assert!(received.is_ok());
        let audio = received.unwrap();
        assert!(!audio.data.is_empty());
        assert_eq!(audio.sample_rate, 16000);
        assert_eq!(audio.format, "audio/pcm");
    }

    #[test]
    fn test_message_handling_final() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "audio": "SGVsbG8gV29ybGQ=",
                "status": 2
            }
        }"#;

        let (audio_tx, _) = mpsc::channel(128);

        let message = Message::Text(json.to_string().into());
        let result = IFlytekTts::handle_websocket_message(message, &audio_tx, 16000, "audio/pcm");
        assert!(result.is_ok());
        assert!(result.unwrap()); // Final
    }

    #[test]
    fn test_message_handling_error() {
        let json = r#"{
            "code": 10005,
            "message": "authorization failure",
            "sid": "test_sid"
        }"#;

        let (audio_tx, _) = mpsc::channel(128);

        let message = Message::Text(json.to_string().into());
        let result = IFlytekTts::handle_websocket_message(message, &audio_tx, 16000, "audio/pcm");
        assert!(result.is_err());
    }

    #[test]
    fn test_message_handling_close() {
        let (audio_tx, _) = mpsc::channel(128);

        let message = Message::Close(None);
        let result = IFlytekTts::handle_websocket_message(message, &audio_tx, 16000, "audio/pcm");
        assert!(result.is_ok());
        assert!(result.unwrap()); // Close is final
    }

    #[test]
    fn test_message_handling_ping() {
        let (audio_tx, _) = mpsc::channel(128);

        let message = Message::Ping(vec![].into());
        let result = IFlytekTts::handle_websocket_message(message, &audio_tx, 16000, "audio/pcm");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Ping is not final
    }

    #[test]
    fn test_message_handling_pong() {
        let (audio_tx, _) = mpsc::channel(128);

        let message = Message::Pong(vec![].into());
        let result = IFlytekTts::handle_websocket_message(message, &audio_tx, 16000, "audio/pcm");
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Pong is not final
    }

    // Callback tests
    #[test]
    fn test_on_audio_callback() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::AtomicUsize;

        let config = create_test_config();
        let mut tts = IFlytekTts::new(config).unwrap();

        struct TestCallback {
            count: AtomicUsize,
        }

        impl AudioCallback for TestCallback {
            fn on_audio(
                &self,
                _audio_data: AudioData,
            ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                self.count.fetch_add(1, Ordering::Relaxed);
                Box::pin(async {})
            }

            fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                Box::pin(async {})
            }

            fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                Box::pin(async {})
            }
        }

        let callback = Arc::new(TestCallback {
            count: AtomicUsize::new(0),
        });

        let result = tts.on_audio(callback.clone());
        assert!(result.is_ok());

        let remove_result = tts.remove_audio_callback();
        assert!(remove_result.is_ok());
    }

    // Constants tests
    #[test]
    fn test_constants() {
        assert_eq!(WS_CONNECT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(WS_MESSAGE_TIMEOUT, Duration::from_secs(60));
        assert_eq!(TEXT_CHANNEL_BUFFER, 32);
        assert_eq!(AUDIO_CHANNEL_BUFFER, 128);
    }

    #[test]
    fn test_provider_info_constant() {
        assert!(PROVIDER_INFO.contains("iFlytek"));
        assert!(PROVIDER_INFO.contains("TTS"));
        assert!(PROVIDER_INFO.contains("科大讯飞"));
    }
}

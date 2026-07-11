//! Alibaba Cloud DashScope TTS WebSocket Provider
//!
//! This module implements the `BaseTTS` trait for Alibaba Cloud's DashScope
//! Text-to-Speech WebSocket API.
//!
//! # Architecture
//!
//! The provider supports two message formats:
//!
//! 1. **Qwen3-TTS**: OpenAI-like realtime protocol for Qwen models
//! 2. **CosyVoice**: DashScope inference protocol for CosyVoice models
//!
//! # WebSocket Message Flow (CosyVoice)
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect with Bearer -------->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ run-task ------------------->|
//!   |<----- task-started ----------------|
//!   |                                    |
//!   |------ continue-task (text) ------->|
//!   |<----- result-generated (audio) ----|
//!   |<----- result-generated (audio) ----|
//!   |                                    |
//!   |------ finish-task ---------------->|
//!   |<----- task-finished ---------------|
//! ```

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{HeaderValue, Request},
        protocol::Message,
    },
};
use tracing::{debug, error, info, warn};

use super::config::DashScopeTtsConfig;
use super::messages::{
    CosyVoiceContinueTask, CosyVoiceFinishTask, CosyVoiceParameters, CosyVoiceResponse,
    CosyVoiceRunTask, QwenTtsServerMessage, QwenTtsSessionUpdate, QwenTtsTextAppend,
    QwenTtsTextCommit,
};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
#[allow(dead_code)]
const PROVIDER_INFO: &str = "Alibaba Cloud DashScope TTS (阿里云)";

/// WebSocket connection timeout.

/// Channel buffer size for text messages.
const TEXT_CHANNEL_BUFFER: usize = 32;

/// Channel buffer size for audio chunks.
const AUDIO_CHANNEL_BUFFER: usize = 128;

// =============================================================================
// DashScope TTS Provider
// =============================================================================

/// Alibaba Cloud DashScope Text-to-Speech WebSocket provider.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::{BaseTTS, TTSConfig};
/// use waav_gateway::core::tts::alibaba_cloud::DashScopeTts;
///
/// let config = TTSConfig {
///     api_key: "sk-xxxxxxxx".to_string(),
///     voice_id: Some("longxiaochun".to_string()),
///     sample_rate: Some(22050),
///     ..Default::default()
/// };
///
/// let mut tts = DashScopeTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("你好世界", true).await?;
/// tts.disconnect().await?;
/// ```
pub struct DashScopeTts {
    /// Base configuration for BaseTTS trait.
    #[allow(dead_code)]
    base_config: TTSConfig,

    /// DashScope-specific configuration.
    config: DashScopeTtsConfig,

    /// Connection state.
    connected: Arc<AtomicBool>,

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
    bytes_synthesized: Arc<AtomicU64>,

    /// Task ID for CosyVoice (stored during connection).
    task_id: Arc<Mutex<Option<String>>>,
}

impl DashScopeTts {
    /// Create a new DashScope TTS provider (internal).
    fn create_internal(config: TTSConfig) -> TTSResult<Self> {
        let dashscope_config = DashScopeTtsConfig::from_base(config.clone())?;
        Self::create_from_parts(config, dashscope_config)
    }

    /// Assemble the provider from a base config and a fully-resolved DashScope config.
    fn create_from_parts(
        base_config: TTSConfig,
        dashscope_config: DashScopeTtsConfig,
    ) -> TTSResult<Self> {
        dashscope_config.validate()?;

        Ok(Self {
            base_config,
            config: dashscope_config,
            connected: Arc::new(AtomicBool::new(false)),
            text_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            audio_forward_handle: None,
            audio_callback: Arc::new(Mutex::new(None)),
            bytes_synthesized: Arc::new(AtomicU64::new(0)),
            task_id: Arc::new(Mutex::new(None)),
        })
    }

    /// Build the provider from the standardized config (W1 keystone), mirroring
    /// `DeepgramTTS::from_standard`. Delegates feature mapping to
    /// [`DashScopeTtsConfig::from_standard`] (speed→rate, pitch, volume, sample_rate + the
    /// `region` extra) so advanced prosody reaches the WebSocket synthesis params through the
    /// standardized dispatch instead of being dropped at the flat boundary.
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let dashscope_config = DashScopeTtsConfig::from_standard(std)?;
        Self::create_from_parts(std.base.clone(), dashscope_config)
    }

    /// Build the DashScope WebSocket upgrade request with authentication headers.
    ///
    /// CRITICAL: built via `into_client_request` so the 5 mandatory WS handshake headers (`Host`,
    /// `Connection`, `Upgrade`, `Sec-WebSocket-Version`, `Sec-WebSocket-Key`) are present. A bare
    /// `Request::builder().uri(url).header("Authorization", ...)` omits them all, so tungstenite's
    /// `generate_request` rejects EVERY connect with `Protocol(InvalidHeader)` — surfacing only as a
    /// connect timeout under the reconnect path. (This provider could not connect at all before this.)
    fn build_request(&self) -> Result<Request<()>, TTSError> {
        let url = self.config.get_websocket_url();
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| TTSError::InternalError(format!("Failed to build request: {e}")))?;
        let headers = request.headers_mut();
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.config.api_key)
                .parse()
                .map_err(|e| {
                    TTSError::InternalError(format!("Invalid Authorization header: {e}"))
                })?,
        );
        headers.insert("User-Agent", HeaderValue::from_static("WaaV-Gateway/1.0"));
        // Add OpenAI-Beta header for Qwen models
        if self.config.model.is_qwen_model() {
            headers.insert("OpenAI-Beta", HeaderValue::from_static("realtime=v1"));
        }
        Ok(request)
    }

    /// Create session update message for Qwen TTS.
    fn create_qwen_session_update(&self) -> String {
        let msg = QwenTtsSessionUpdate::new(
            &self.config.voice,
            self.config.audio_format.as_format_str(),
            self.config.sample_rate,
            Some(self.config.rate),
        );
        msg.to_json().unwrap_or_default()
    }

    /// Create run-task message for CosyVoice.
    ///
    /// Carries both the base prosody (`voice`/`format`/`sample_rate`/`volume`/`rate`/`pitch`) and
    /// the advanced CosyVoice inference-protocol knobs (`enable_ssml`, `instruction`,
    /// `word_timestamp_enabled`, `seed`, `language_hints`, `bit_rate`, `hot_fix`,
    /// `enable_markdown_filter`, `enable_aigc_tag`) from the resolved config into the run-task
    /// `payload.parameters`. Unset advanced knobs are omitted from the wire.
    fn create_cosyvoice_run_task(&self) -> (String, String) {
        let msg = CosyVoiceRunTask::with_parameters(
            self.config.model.as_model_id(),
            CosyVoiceParameters {
                voice: self.config.voice.clone(),
                format: self.config.audio_format.as_format_str().to_string(),
                sample_rate: self.config.sample_rate,
                volume: self.config.volume,
                rate: self.config.rate,
                pitch: self.config.pitch,
                enable_ssml: self.config.enable_ssml,
                instruction: self.config.instruction.clone(),
                word_timestamp_enabled: self.config.word_timestamp_enabled,
                seed: self.config.seed,
                language_hints: self.config.language_hints.clone(),
                bit_rate: self.config.bit_rate,
                hot_fix: self.config.hot_fix.clone(),
                enable_markdown_filter: self.config.enable_markdown_filter,
                enable_aigc_tag: self.config.enable_aigc_tag,
            },
        );
        let task_id = msg.task_id().to_string();
        let json = msg.to_json().unwrap_or_default();
        (json, task_id)
    }

    /// Handle Qwen TTS response.
    fn handle_qwen_response(
        text: &str,
        audio_tx: &mpsc::Sender<AudioData>,
        sample_rate: u32,
        format: &str,
        bytes_counter: &Arc<AtomicU64>,
    ) -> bool {
        match QwenTtsServerMessage::from_json(text) {
            Ok(msg) => {
                if msg.is_error() {
                    if let Some(err) = &msg.error {
                        error!("Qwen TTS error: {} - {}", err.code, err.message);
                    }
                    return true;
                } else if msg.is_audio_delta() {
                    if let Some(audio_data) = msg.get_audio_data() {
                        let len = audio_data.len();
                        bytes_counter.fetch_add(len as u64, Ordering::Relaxed);
                        let _ = audio_tx.try_send(AudioData {
                            data: audio_data,
                            sample_rate,
                            format: format.to_string(),
                            duration_ms: None,
                        });
                    }
                } else if msg.is_response_done() {
                    debug!("Qwen TTS response done");
                    return true;
                } else if msg.is_session_created() {
                    debug!("Qwen TTS session created");
                }
                false
            }
            Err(e) => {
                warn!("Failed to parse Qwen TTS response: {}", e);
                false
            }
        }
    }

    /// Handle CosyVoice response.
    fn handle_cosyvoice_response(
        text: &str,
        audio_tx: &mpsc::Sender<AudioData>,
        sample_rate: u32,
        format: &str,
        bytes_counter: &Arc<AtomicU64>,
    ) -> bool {
        match CosyVoiceResponse::from_json(text) {
            Ok(msg) => {
                if msg.is_task_failed() {
                    if let Some((code, message)) = msg.get_error() {
                        error!("CosyVoice error: {} - {}", code, message);
                    }
                    return true;
                } else if msg.is_result_generated() {
                    if let Some(audio_data) = msg.get_audio_data() {
                        let len = audio_data.len();
                        bytes_counter.fetch_add(len as u64, Ordering::Relaxed);
                        let _ = audio_tx.try_send(AudioData {
                            data: audio_data,
                            sample_rate,
                            format: format.to_string(),
                            duration_ms: None,
                        });
                    }
                } else if msg.is_task_finished() {
                    debug!("CosyVoice task finished");
                    return true;
                } else if msg.is_task_started() {
                    debug!("CosyVoice task started");
                }
                false
            }
            Err(e) => {
                warn!("Failed to parse CosyVoice response: {}", e);
                false
            }
        }
    }
}

#[async_trait]
impl BaseTTS for DashScopeTts {
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

        info!("Connecting to Alibaba Cloud DashScope TTS...");

        // Build request
        let request = self.build_request()?;
        let url = self.config.get_websocket_url();

        // Connect with timeout
        // 10s dial bound (provider-historical; tighter than the canonical 15s in
        // resilience::connect — kept to preserve behavior), via the shared helper.
        let (ws_stream, _) = match crate::core::resilience::connect::with_timeout(
            Duration::from_secs(10),
            connect_async(request),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                return Err(TTSError::ConnectionFailed(format!(
                    "WebSocket connection failed: {}",
                    e
                )));
            }
            Err(_) => {
                return Err(TTSError::ConnectionFailed("Connection timeout".to_string()));
            }
        };

        info!("Connected to DashScope TTS: {}", url);

        // Split stream
        let (mut write, mut read) = ws_stream.split();

        // Create channels
        let (text_tx, mut text_rx) = mpsc::channel::<String>(TEXT_CHANNEL_BUFFER);
        let (audio_tx, mut audio_rx) = mpsc::channel::<AudioData>(AUDIO_CHANNEL_BUFFER);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        self.text_sender = Some(text_tx);
        self.shutdown_tx = Some(shutdown_tx);

        // Send initial message based on model type
        let is_qwen = self.config.model.is_qwen_model();
        let task_id = if is_qwen {
            let session_update = self.create_qwen_session_update();
            write
                .send(Message::Text(session_update.into()))
                .await
                .map_err(|e| {
                    TTSError::ConnectionFailed(format!("Failed to send session update: {}", e))
                })?;
            None
        } else {
            let (run_task_json, task_id) = self.create_cosyvoice_run_task();
            write
                .send(Message::Text(run_task_json.into()))
                .await
                .map_err(|e| {
                    TTSError::ConnectionFailed(format!("Failed to send run-task: {}", e))
                })?;
            Some(task_id)
        };

        // Store task ID for CosyVoice
        if let Some(tid) = &task_id {
            *self.task_id.lock().await = Some(tid.clone());
        }

        // Set connected state
        self.connected.store(true, Ordering::SeqCst);

        // Clone for tasks
        let connected = self.connected.clone();
        let sample_rate = self.config.sample_rate;
        let format = self.config.audio_format.as_format_str().to_string();
        let bytes_counter = self.bytes_synthesized.clone();
        let task_id_clone = task_id.clone();

        // Spawn connection handler task
        let connection_handle = tokio::spawn(async move {
            let send_task = tokio::spawn(async move {
                while let Some(text) = text_rx.recv().await {
                    let msg = if is_qwen {
                        // For Qwen: append text and commit
                        let append = QwenTtsTextAppend::new(&text);
                        let commit = QwenTtsTextCommit::new();
                        format!(
                            "{}\n{}",
                            append.to_json().unwrap_or_default(),
                            commit.to_json().unwrap_or_default()
                        )
                    } else if let Some(tid) = &task_id_clone {
                        // For CosyVoice: continue-task with text
                        let continue_task = CosyVoiceContinueTask::new(tid, &text);
                        continue_task.to_json().unwrap_or_default()
                    } else {
                        continue;
                    };

                    // Send text message (may be multiple JSON for Qwen)
                    for line in msg.lines() {
                        if !line.is_empty()
                            && write
                                .send(Message::Text(line.to_string().into()))
                                .await
                                .is_err()
                        {
                            break;
                        }
                    }
                }

                // Send finish message for CosyVoice
                if !is_qwen && let Some(tid) = &task_id_clone {
                    let finish = CosyVoiceFinishTask::new(tid);
                    let _ = write
                        .send(Message::Text(finish.to_json().unwrap_or_default().into()))
                        .await;
                }

                write
            });

            let recv_task = tokio::spawn(async move {
                while let Some(msg_result) = read.next().await {
                    match msg_result {
                        Ok(Message::Text(text)) => {
                            let done = if is_qwen {
                                Self::handle_qwen_response(
                                    &text,
                                    &audio_tx,
                                    sample_rate,
                                    &format,
                                    &bytes_counter,
                                )
                            } else {
                                Self::handle_cosyvoice_response(
                                    &text,
                                    &audio_tx,
                                    sample_rate,
                                    &format,
                                    &bytes_counter,
                                )
                            };
                            if done {
                                break;
                            }
                        }
                        Ok(Message::Binary(data)) => {
                            // CosyVoice may send binary audio directly
                            let len = data.len();
                            bytes_counter.fetch_add(len as u64, Ordering::Relaxed);
                            let _ = audio_tx.try_send(AudioData {
                                data: data.to_vec(),
                                sample_rate,
                                format: format.clone(),
                                duration_ms: None,
                            });
                        }
                        Ok(Message::Close(_)) => {
                            debug!("DashScope TTS WebSocket closed");
                            break;
                        }
                        Err(e) => {
                            error!("WebSocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            });

            // Wait for shutdown or completion
            tokio::select! {
                _ = &mut shutdown_rx => {
                    debug!("TTS shutdown signal received");
                }
                _ = recv_task => {
                    debug!("TTS receive task completed");
                }
            }

            send_task.abort();
            connected.store(false, Ordering::SeqCst);
        });

        self.connection_handle = Some(connection_handle);

        // Spawn audio forwarding task
        let audio_callback = self.audio_callback.clone();
        let audio_forward_handle = tokio::spawn(async move {
            while let Some(audio) = audio_rx.recv().await {
                let callback = audio_callback.lock().await;
                if let Some(cb) = callback.as_ref() {
                    cb.on_audio(audio).await;
                }
            }
        });

        self.audio_forward_handle = Some(audio_forward_handle);

        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from DashScope TTS...");

        // Drop text sender to signal end
        self.text_sender.take();

        // Send shutdown signal
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Wait for connection task to complete
        if let Some(handle) = self.connection_handle.take() {
            crate::core::observability::await_task_shutdown(
                "alibaba-cloud-tts-connection",
                handle,
                Duration::from_secs(5),
            )
            .await;
        }

        // Abort audio forwarding task
        if let Some(handle) = self.audio_forward_handle.take() {
            crate::core::observability::abort_and_await_task(
                "alibaba-cloud-tts-audio-forwarder",
                handle,
            )
            .await;
        }

        // Clear task ID
        *self.task_id.lock().await = None;

        self.connected.store(false, Ordering::SeqCst);

        info!("Disconnected from DashScope TTS");
        Ok(())
    }

    fn get_connection_state(&self) -> ConnectionState {
        if self.connected.load(Ordering::SeqCst) {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        if !self.connected.load(Ordering::SeqCst) {
            // Auto-connect if not connected
            self.connect().await?;
        }

        if let Some(sender) = &self.text_sender {
            sender.send(text.to_string()).await.map_err(|_| {
                TTSError::InternalError("Failed to send text to channel".to_string())
            })?;
        }

        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // No-op for streaming TTS
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        let audio_callback = self.audio_callback.clone();
        tokio::spawn(async move {
            *audio_callback.lock().await = Some(callback);
        });
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        let audio_callback = self.audio_callback.clone();
        tokio::spawn(async move {
            *audio_callback.lock().await = None;
        });
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "alibaba-cloud",
            "name": "Alibaba Cloud DashScope TTS",
            "version": "1.0.0",
            "description": "Alibaba Cloud DashScope Model Studio TTS with CosyVoice and Qwen3-TTS",
            "models": DashScopeTtsConfig::supported_models(),
            "voices": DashScopeTtsConfig::supported_voices(),
            "audio_formats": ["mp3", "pcm", "wav", "opus"],
            "sample_rates": [8000, 16000, 22050, 24000, 44100, 48000],
            "features": [
                "streaming",
                "websocket",
                "speed-control",
                "volume-control",
                "pitch-control",
                "chinese-dialects"
            ],
            "languages": [
                "zh", "en", "ja", "ko", "ru", "fr", "de", "es", "pt", "it",
                "ar", "hi", "th", "vi", "id", "ms", "tr", "uk", "pl", "nl",
                "sv", "da", "fi", "no", "cs", "yue", "wuu"
            ],
            "region": format!("{:?}", self.config.region),
            "model": self.config.model.as_model_id()
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            api_key: "test_api_key".to_string(),
            voice_id: Some("longxiaochun".to_string()),
            sample_rate: Some(22050),
            model: "cosyvoice-v3-flash".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_provider() {
        let config = create_test_config();
        let result = DashScopeTts::new(config);
        assert!(result.is_ok());
    }

    // W1 keystone: a StandardTTSConfig advanced feature DashScope supports (prosody speed/pitch/
    // volume + sample_rate) reaches the provider's resolved `config` through the provider struct's
    // `from_standard`, mirroring `DeepgramTTS::from_standard`.
    #[test]
    fn from_standard_reaches_provider_config() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "alibaba-cloud".into(),
                api_key: "k".into(),
                voice_id: Some("longxiaochun".into()),
                model: "cosyvoice-v3-flash".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.25),
                pitch: Some(0.9),
                volume: Some(80.0),
                sample_rate: Some(24000),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = DashScopeTts::from_standard(&std).unwrap();
        assert_eq!(tts.config.rate, 1.25);
        assert_eq!(tts.config.pitch, 0.9);
        assert_eq!(tts.config.volume, 80);
        assert_eq!(tts.config.sample_rate, 24000);
        assert_eq!(tts.base_config.api_key, "k");
    }

    #[test]
    fn from_standard_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "alibaba-cloud".into(),
            api_key: "k".into(),
            voice_id: Some("longxiaochun".into()),
            ..Default::default()
        })
        .with_endpoint_override("ws://127.0.0.1:9000");

        match DashScopeTts::from_standard(&std) {
            Ok(_) => {
                panic!("DashScope provider construction must reject unsafe endpoint_override")
            }
            Err(err) => assert!(err.to_string().contains("SSRF protection")),
        }
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let config = TTSConfig {
            api_key: "".to_string(),
            ..Default::default()
        };
        let result = DashScopeTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let tts = DashScopeTts::new(config).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let tts = DashScopeTts::new(config).unwrap();
        let info = tts.get_provider_info();
        let info_str = info.to_string();
        assert!(info_str.contains("alibaba-cloud") || info_str.contains("DashScope"));
        assert!(info["provider"] == "alibaba-cloud");
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut tts = DashScopeTts::new(config).unwrap();
        let result = tts.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_flush_ok() {
        let config = create_test_config();
        let tts = DashScopeTts::new(config).unwrap();
        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_qwen_session_update_creation() {
        let mut config = create_test_config();
        config.model = "qwen3-tts-flash-realtime".to_string();
        let tts = DashScopeTts::new(config).unwrap();

        let session_update = tts.create_qwen_session_update();
        assert!(session_update.contains("session.update"));
    }

    #[test]
    fn test_cosyvoice_run_task_creation() {
        let config = create_test_config();
        let tts = DashScopeTts::new(config).unwrap();

        let (json, task_id) = tts.create_cosyvoice_run_task();
        assert!(json.contains("run-task"));
        assert!(json.contains("cosyvoice-v3-flash"));
        assert!(!task_id.is_empty());
    }

    // WIRE-LEVEL: the advanced CosyVoice features set through the standardized config must reach
    // the actual run-task JSON sent over the WebSocket (under `payload.parameters`), not merely
    // sit on the config struct. This guards the recurring "config set but never emitted" bug.
    #[test]
    fn cosyvoice_features_reach_run_task_payload_parameters() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("bit_rate".into(), serde_json::json!(48000));
        extras.insert(
            "hot_fix".into(),
            serde_json::json!({"WaaV": "wave", "TTS": "tee tee ess"}),
        );
        extras.insert("enable_markdown_filter".into(), serde_json::json!(true));
        extras.insert("enable_aigc_tag".into(), serde_json::json!(true));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "alibaba-cloud".into(),
                api_key: "k".into(),
                voice_id: Some("longxiaochun".into()),
                model: "cosyvoice-v3-flash".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                ssml: Some(true),
                instructions: Some("speak with a cheerful Sichuan dialect".into()),
                word_timestamps: Some(true),
                seed: Some(2024),
                language: Some("zh".into()),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let tts = DashScopeTts::from_standard(&std).unwrap();
        let (json, _task_id) = tts.create_cosyvoice_run_task();

        // Parse the emitted bytes and assert each knob landed under payload.parameters.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let params = &v["payload"]["parameters"];
        assert_eq!(params["enable_ssml"], serde_json::json!(true));
        assert_eq!(
            params["instruction"],
            serde_json::json!("speak with a cheerful Sichuan dialect")
        );
        assert_eq!(params["word_timestamp_enabled"], serde_json::json!(true));
        assert_eq!(params["seed"], serde_json::json!(2024));
        assert_eq!(params["language_hints"], serde_json::json!(["zh"]));
        assert_eq!(params["bit_rate"], serde_json::json!(48000));
        assert_eq!(
            params["hot_fix"],
            serde_json::json!({"WaaV": "wave", "TTS": "tee tee ess"})
        );
        assert_eq!(params["enable_markdown_filter"], serde_json::json!(true));
        assert_eq!(params["enable_aigc_tag"], serde_json::json!(true));
    }

    // The default (no advanced features) run-task must stay byte-compatible: the optional knobs
    // are omitted from the wire entirely (skip_serializing_if), not emitted as null.
    #[test]
    fn cosyvoice_run_task_omits_unset_advanced_knobs() {
        let config = create_test_config();
        let tts = DashScopeTts::new(config).unwrap();
        let (json, _) = tts.create_cosyvoice_run_task();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let params = &v["payload"]["parameters"];
        for key in [
            "enable_ssml",
            "instruction",
            "word_timestamp_enabled",
            "seed",
            "language_hints",
            "bit_rate",
            "hot_fix",
            "enable_markdown_filter",
            "enable_aigc_tag",
        ] {
            assert!(
                params.get(key).is_none(),
                "{key} must be omitted when unset"
            );
        }
        // Base prosody still present.
        assert!(params.get("voice").is_some());
        assert!(params.get("rate").is_some());
    }

    #[test]
    fn test_websocket_url_cosyvoice() {
        let config = create_test_config();
        let tts = DashScopeTts::new(config).unwrap();

        let url = tts.config.get_websocket_url();
        assert!(url.contains("inference"));
    }

    #[test]
    fn test_websocket_url_qwen() {
        let mut config = create_test_config();
        config.model = "qwen3-tts-flash-realtime".to_string();
        let tts = DashScopeTts::new(config).unwrap();

        let url = tts.config.get_websocket_url();
        assert!(url.contains("realtime"));
        assert!(url.contains("qwen3-tts-flash-realtime"));
    }

    #[test]
    fn test_build_request() {
        let config = create_test_config();
        let tts = DashScopeTts::new(config).unwrap();

        let request = tts.build_request().expect("build_request");
        let h = request.headers();
        // The 5 mandatory WS upgrade headers MUST be present (else tungstenite rejects the connect
        // with InvalidHeader — the connect-timeout bug), plus DashScope's Bearer auth.
        for required in [
            "host",
            "connection",
            "upgrade",
            "sec-websocket-version",
            "sec-websocket-key",
        ] {
            assert!(h.contains_key(required), "missing WS header: {required}");
        }
        assert!(
            h.get("authorization")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("Bearer ")),
            "missing Bearer Authorization header"
        );
    }

    #[test]
    fn qwen_build_request_sets_static_beta_header_without_parsing() {
        let mut config = create_test_config();
        config.model = "qwen3-tts-flash-realtime".to_string();
        let tts = DashScopeTts::new(config).unwrap();

        let request = tts.build_request().expect("build qwen request");
        let h = request.headers();
        assert_eq!(
            h.get("user-agent").and_then(|v| v.to_str().ok()),
            Some("WaaV-Gateway/1.0")
        );
        assert_eq!(
            h.get("openai-beta").and_then(|v| v.to_str().ok()),
            Some("realtime=v1")
        );
    }
}

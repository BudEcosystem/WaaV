//! STT, TTS, and Realtime provider nodes
//!
//! These nodes wrap speech recognition and synthesis providers for DAG pipelines.
//! They use channel-based bridging to convert callback-based providers to async/await.

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::Mutex;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, trace, warn};

use super::{DAGData, DAGNode, NodeCapability, STTResultData, TTSAudioData};
use crate::core::realtime::{
    AudioOutputCallback, RealtimeAudioData, RealtimeConfig, RealtimeError, RealtimeErrorCallback,
    TranscriptCallback, TranscriptResult,
};
use crate::core::stt::{STTError, STTErrorCallback, STTResult, STTResultCallback};
use crate::core::tts::{AudioCallback, AudioData, TTSError};
use crate::dag::context::DAGContext;
use crate::dag::error::{DAGError, DAGResult};

/// Resolve a provider credential from a DAG node's `config` blob.
///
/// Supports either a literal value or `${ENV_VAR}` indirection. To keep a DAG definition from
/// exfiltrating arbitrary process environment variables, `${ENV_VAR}` is honored ONLY when the
/// variable name looks like a credential (upper-snake-case ending in a credential suffix such as
/// `_API_KEY`/`_API_TOKEN`/`_SECRET_KEY`/`_ACCESS_KEY`/`_SUBSCRIPTION_KEY`/`_TOKEN`). Returns
/// `None` when the field is absent or a non-credential env var was requested (logged + blocked).
///
/// Without this, STT/TTS provider nodes built `STTConfig`/`TTSConfig` with an EMPTY `api_key`, so a
/// DAG could never authenticate to a real vendor — the node failed with "API key is required".
pub(crate) fn resolve_node_credential(config: &serde_json::Value, field: &str) -> Option<String> {
    let raw = config.get(field)?.as_str()?;
    if let Some(var) = raw.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        let looks_like_credential = !var.is_empty()
            && var
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && [
                "_API_KEY",
                "_API_TOKEN",
                "_SECRET_KEY",
                "_ACCESS_KEY",
                "_SUBSCRIPTION_KEY",
                "_TOKEN",
            ]
            .iter()
            .any(|suffix| var.ends_with(suffix));
        if !looks_like_credential {
            warn!(var = %var, "DAG node config: blocked ${{}} reference to a non-credential env var");
            return None;
        }
        return std::env::var(var).ok();
    }
    Some(raw.to_string())
}

/// Callback bridge for TTS provider to DAG node
///
/// This struct implements the `AudioCallback` trait and bridges
/// the callback-based TTS provider to the channel-based DAG node.
struct DAGTTSCallback {
    /// Channel for sending audio chunks
    audio_tx: mpsc::Sender<AudioData>,
    /// Channel for sending errors
    error_tx: mpsc::Sender<TTSError>,
    /// One-shot channel for completion signal (wrapped in Mutex for interior mutability)
    complete_tx: Mutex<Option<oneshot::Sender<()>>>,
}

impl AudioCallback for DAGTTSCallback {
    fn on_audio(&self, audio_data: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let tx = self.audio_tx.clone();
        Box::pin(async move {
            if tx.send(audio_data).await.is_err() {
                trace!("TTS audio callback: channel closed, likely node execution completed");
            }
        })
    }

    fn on_error(&self, error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let tx = self.error_tx.clone();
        Box::pin(async move {
            if tx.send(error).await.is_err() {
                trace!("TTS error callback: channel closed, likely node execution completed");
            }
        })
    }

    fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        // Take the sender out (can only complete once)
        let sender = self.complete_tx.lock().take();
        Box::pin(async move {
            if let Some(tx) = sender {
                let _ = tx.send(());
            }
        })
    }
}

/// Default STT timeout in seconds (configurable)
const DEFAULT_STT_TIMEOUT_SECS: u64 = 30;

/// Maximum STT timeout in seconds (cap for safety)
const MAX_STT_TIMEOUT_SECS: u64 = 300; // 5 minutes max

/// STT (Speech-to-Text) provider node
///
/// Wraps an STT provider for converting audio to text in a DAG pipeline.
#[derive(Clone)]
pub struct STTProviderNode {
    id: String,
    provider: String,
    model: Option<String>,
    language: Option<String>,
    config: serde_json::Value,
    /// Configurable timeout in seconds (default: 30, max: 300)
    timeout_secs: u64,
}

impl STTProviderNode {
    /// Create a new STT provider node
    pub fn new(id: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            model: None,
            language: None,
            config: serde_json::Value::Null,
            timeout_secs: DEFAULT_STT_TIMEOUT_SECS,
        }
    }

    /// Set the model to use
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the language
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Set additional configuration
    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = config;
        self
    }

    /// Set timeout in seconds (default: 30, max: 300)
    ///
    /// Values exceeding MAX_STT_TIMEOUT_SECS will be capped.
    pub fn with_timeout_secs(mut self, timeout: u64) -> Self {
        self.timeout_secs = timeout.min(MAX_STT_TIMEOUT_SECS);
        self
    }

    /// Get the provider name
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Get the configured timeout in seconds
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }
}

impl std::fmt::Debug for STTProviderNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("STTProviderNode")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("language", &self.language)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

#[async_trait]
impl DAGNode for STTProviderNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "stt_provider"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::AudioInput,
            NodeCapability::TextOutput,
            NodeCapability::Streaming,
            NodeCapability::Cancellable,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Extract audio from input
        let audio_bytes = match &input {
            DAGData::Audio(bytes) => bytes.clone(),
            DAGData::TTSAudio(tts) => tts.data.clone(),
            DAGData::Empty => return Ok(DAGData::Empty),
            other => {
                return Err(DAGError::UnsupportedDataType {
                    expected: "audio".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        // Skip empty audio
        if audio_bytes.is_empty() {
            debug!(node_id = %self.id, "Empty audio input, skipping STT");
            return Ok(DAGData::Empty);
        }

        debug!(
            node_id = %self.id,
            provider = %self.provider,
            audio_size = %audio_bytes.len(),
            "Processing audio through STT"
        );

        // Get STT provider from registry
        let registry = crate::plugin::global_registry();

        // Build STT configuration. The credential comes from the node's `config` blob
        // (`"api_key"`, literal or `${ENV_VAR}`); without it the provider would reject the
        // connection with "API key is required".
        let stt_config = crate::core::stt::STTConfig {
            provider: self.provider.clone(),
            model: self.model.clone().unwrap_or_default(),
            language: self.language.clone().unwrap_or_else(|| "en-US".to_string()),
            api_key: resolve_node_credential(&self.config, "api_key").unwrap_or_default(),
            ..Default::default()
        };

        // Create STT provider
        let mut stt = match registry.create_stt(&self.provider, stt_config) {
            Ok(stt) => stt,
            Err(e) => {
                return Err(DAGError::STTProviderError {
                    provider: self.provider.clone(),
                    error: e.to_string(),
                });
            }
        };

        // Create channel for receiving STT results
        // We use mpsc to collect potentially multiple interim results
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(16);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(4);

        // Create callback for STT results
        let result_sender = result_tx.clone();
        let result_callback: STTResultCallback = Arc::new(move |result: STTResult| {
            let tx = result_sender.clone();
            Box::pin(async move {
                if tx.send(result).await.is_err() {
                    trace!("STT result callback: channel closed, likely node execution completed");
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        // Create callback for STT errors
        let error_sender = error_tx.clone();
        let error_callback: STTErrorCallback = Arc::new(move |error: STTError| {
            let tx = error_sender.clone();
            Box::pin(async move {
                if tx.send(error).await.is_err() {
                    trace!("STT error callback: channel closed, likely node execution completed");
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        // Register callbacks
        if let Err(e) = stt.on_result(result_callback).await {
            return Err(DAGError::STTProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to register result callback: {}", e),
            });
        }

        if let Err(e) = stt.on_error(error_callback).await {
            return Err(DAGError::STTProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to register error callback: {}", e),
            });
        }

        // Connect to the STT provider
        if let Err(e) = stt.connect().await {
            return Err(DAGError::STTProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to connect: {}", e),
            });
        }

        info!(
            node_id = %self.id,
            provider = %self.provider,
            "Connected to STT provider"
        );

        // Send audio data
        if let Err(e) = stt.send_audio(audio_bytes).await {
            let _ = stt.disconnect().await;
            return Err(DAGError::STTProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to send audio: {}", e),
            });
        }

        // Wait for results with configurable timeout
        // Use the configured timeout, but respect context remaining time if smaller
        let configured_timeout = Duration::from_secs(self.timeout_secs);
        let timeout_duration = match ctx.remaining_time() {
            Some(remaining) => remaining.min(configured_timeout),
            None => configured_timeout,
        };

        // Collect results until we get a final one or timeout
        let mut final_result: Option<STTResult> = None;
        let mut last_interim: Option<STTResult> = None;

        let deadline = tokio::time::Instant::now() + timeout_duration;

        loop {
            tokio::select! {
                // Check for cancellation
                _ = ctx.cancel_token.cancelled() => {
                    let _ = stt.disconnect().await;
                    return Err(DAGError::Cancelled);
                }

                // Receive results
                result = result_rx.recv() => {
                    match result {
                        Some(r) => {
                            debug!(
                                node_id = %self.id,
                                transcript = %r.transcript,
                                is_final = %r.is_final,
                                is_speech_final = %r.is_speech_final,
                                confidence = %r.confidence,
                                "Received STT result"
                            );

                            if r.is_speech_final || r.is_final {
                                final_result = Some(r);
                                break;
                            } else {
                                last_interim = Some(r);
                            }
                        }
                        None => {
                            // Channel closed - no more results
                            break;
                        }
                    }
                }

                // Check for errors
                error = error_rx.recv() => {
                    if let Some(e) = error {
                        let _ = stt.disconnect().await;
                        return Err(DAGError::STTProviderError {
                            provider: self.provider.clone(),
                            error: e.to_string(),
                        });
                    }
                }

                // Timeout
                _ = tokio::time::sleep_until(deadline) => {
                    debug!(
                        node_id = %self.id,
                        "STT timeout reached, using best available result"
                    );
                    break;
                }
            }
        }

        // Disconnect from provider
        if let Err(e) = stt.disconnect().await {
            warn!(
                node_id = %self.id,
                error = %e,
                "Failed to disconnect from STT provider"
            );
        }

        // Use final result if available, otherwise use last interim
        let result = final_result.or(last_interim);

        match result {
            Some(r) => {
                info!(
                    node_id = %self.id,
                    provider = %self.provider,
                    transcript_len = %r.transcript.len(),
                    confidence = %r.confidence,
                    "STT completed successfully"
                );

                // Determine if actual speech was detected based on transcript content
                let speech_detected = !r.transcript.trim().is_empty();

                Ok(DAGData::STTResult(STTResultData {
                    transcript: r.transcript,
                    is_final: r.is_final,
                    is_speech_final: r.is_speech_final,
                    confidence: r.confidence as f64,
                    language: self.language.clone(),
                    words: None,
                    metadata: serde_json::json!({
                        "provider": self.provider,
                        "model": self.model,
                    }),
                    speech_detected,
                }))
            }
            None => {
                // No result received - this could happen for short audio or silence
                debug!(
                    node_id = %self.id,
                    "No STT result received (possibly silence or too short)"
                );

                Ok(DAGData::STTResult(STTResultData {
                    transcript: String::new(),
                    is_final: true,
                    is_speech_final: true,
                    confidence: 0.0,
                    language: self.language.clone(),
                    words: None,
                    metadata: serde_json::json!({
                        "provider": self.provider,
                        "model": self.model,
                        "note": "No speech detected"
                    }),
                    speech_detected: false,
                }))
            }
        }
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// Default maximum TTS audio size (100 MB)
const DEFAULT_MAX_TTS_AUDIO_BYTES: usize = 100 * 1024 * 1024;

/// Maximum audio bytes a realtime provider response may accumulate before being
/// treated as a runaway and terminated (W-O3 bug #5). 100 MB of PCM16 @ 24 kHz
/// is ~35 minutes — far beyond any single realtime turn.
const MAX_REALTIME_COLLECTED_AUDIO_BYTES: usize = 100 * 1024 * 1024;

/// TTS (Text-to-Speech) provider node
///
/// Wraps a TTS provider for converting text to audio in a DAG pipeline.
///
/// # Memory Safety
/// Audio chunks are collected with a configurable size limit (default 100MB)
/// to prevent memory exhaustion from malicious or abnormally long audio.
#[derive(Clone)]
pub struct TTSProviderNode {
    id: String,
    provider: String,
    voice_id: Option<String>,
    model: Option<String>,
    config: serde_json::Value,
    /// Maximum total audio bytes to collect (prevents memory exhaustion)
    max_audio_bytes: usize,
}

impl TTSProviderNode {
    /// Create a new TTS provider node
    pub fn new(id: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            voice_id: None,
            model: None,
            config: serde_json::Value::Null,
            max_audio_bytes: DEFAULT_MAX_TTS_AUDIO_BYTES,
        }
    }

    /// Set the voice ID
    pub fn with_voice(mut self, voice_id: impl Into<String>) -> Self {
        self.voice_id = Some(voice_id.into());
        self
    }

    /// Set the model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set additional configuration
    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = config;
        self
    }

    /// Set maximum audio bytes limit (default: 100MB)
    ///
    /// This prevents memory exhaustion from abnormally long TTS audio.
    pub fn with_max_audio_bytes(mut self, max_bytes: usize) -> Self {
        self.max_audio_bytes = max_bytes;
        self
    }

    /// Get the provider name
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Get the maximum audio bytes limit
    pub fn max_audio_bytes(&self) -> usize {
        self.max_audio_bytes
    }
}

impl std::fmt::Debug for TTSProviderNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TTSProviderNode")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("voice_id", &self.voice_id)
            .field("model", &self.model)
            .finish()
    }
}

#[async_trait]
impl DAGNode for TTSProviderNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "tts_provider"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::TextInput,
            NodeCapability::AudioOutput,
            NodeCapability::Streaming,
            NodeCapability::Cancellable,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Extract text from input
        let text = match &input {
            DAGData::Text(t) => t.clone(),
            DAGData::STTResult(r) => r.transcript.clone(),
            DAGData::Json(j) => j
                .get("text")
                .or_else(|| j.get("content"))
                .or_else(|| j.get("message"))
                .and_then(|v| v.as_str())
                .map(String::from)
                .ok_or_else(|| DAGError::UnsupportedDataType {
                    expected: "text".to_string(),
                    actual: "json without text field".to_string(),
                })?,
            DAGData::Empty => return Ok(DAGData::Empty),
            other => {
                return Err(DAGError::UnsupportedDataType {
                    expected: "text".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        if text.is_empty() {
            return Ok(DAGData::Empty);
        }

        debug!(
            node_id = %self.id,
            provider = %self.provider,
            text_length = %text.len(),
            "Synthesizing speech"
        );

        // Get TTS provider from registry
        let registry = crate::plugin::global_registry();

        // Build TTS configuration. The credential comes from the node's `config` blob
        // (`"api_key"`, literal or `${ENV_VAR}`); without it the provider could not authenticate.
        let tts_config = crate::core::tts::TTSConfig {
            provider: self.provider.clone(),
            voice_id: self.voice_id.clone(),
            model: self.model.clone().unwrap_or_default(),
            api_key: resolve_node_credential(&self.config, "api_key").unwrap_or_default(),
            ..Default::default()
        };

        // Create TTS provider
        let mut tts = match registry.create_tts(&self.provider, tts_config) {
            Ok(tts) => tts,
            Err(e) => {
                return Err(DAGError::TTSProviderError {
                    provider: self.provider.clone(),
                    error: e.to_string(),
                });
            }
        };

        // Create channels for collecting audio chunks and completion signal
        let (audio_tx, mut audio_rx) = mpsc::channel::<AudioData>(64);
        let (error_tx, mut error_rx) = mpsc::channel::<TTSError>(4);
        let (complete_tx, complete_rx) = oneshot::channel::<()>();

        // Create callback struct that implements AudioCallback
        let callback = Arc::new(DAGTTSCallback {
            audio_tx,
            error_tx,
            complete_tx: Mutex::new(Some(complete_tx)),
        });

        // Register callback
        if let Err(e) = tts.on_audio(callback) {
            return Err(DAGError::TTSProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to register audio callback: {}", e),
            });
        }

        // Connect to the TTS provider
        if let Err(e) = tts.connect().await {
            return Err(DAGError::TTSProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to connect: {}", e),
            });
        }

        info!(
            node_id = %self.id,
            provider = %self.provider,
            "Connected to TTS provider"
        );

        // Send text for synthesis
        if let Err(e) = tts.speak(&text, true).await {
            let _ = tts.disconnect().await;
            return Err(DAGError::TTSProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to synthesize speech: {}", e),
            });
        }

        // Wait for audio chunks with timeout
        let timeout = ctx.remaining_time().unwrap_or(Duration::from_secs(60));
        let timeout_duration = timeout.min(Duration::from_secs(60)); // Cap at 60s for TTS
        let deadline = tokio::time::Instant::now() + timeout_duration;

        // Collect all audio chunks with size tracking
        let mut audio_chunks: Vec<AudioData> = Vec::new();
        let mut total_audio_bytes: usize = 0;
        let mut sample_rate = 16000u32;
        let mut format = "pcm16".to_string();

        // Wrap oneshot in Option so we can consume it only once
        let mut complete_rx = Some(complete_rx);
        let mut complete_received = false;

        loop {
            // Check if we should exit (completion received or channels closed)
            if complete_received {
                // Give a small grace period to receive remaining chunks
                tokio::time::sleep(Duration::from_millis(50)).await;
                // Drain any remaining chunks (with size limit check)
                while let Ok(audio_data) = audio_rx.try_recv() {
                    let chunk_size = audio_data.data.len();
                    // Stop collecting if we'd exceed the limit
                    if total_audio_bytes.saturating_add(chunk_size) > self.max_audio_bytes {
                        warn!(
                            node_id = %self.id,
                            total_bytes = %total_audio_bytes,
                            max_bytes = %self.max_audio_bytes,
                            "Dropping remaining TTS chunks due to size limit"
                        );
                        break;
                    }
                    total_audio_bytes += chunk_size;
                    sample_rate = audio_data.sample_rate;
                    format = audio_data.format.clone();
                    audio_chunks.push(audio_data);
                }
                break;
            }

            tokio::select! {
                biased;

                // Check for cancellation (highest priority)
                _ = ctx.cancel_token.cancelled() => {
                    let _ = tts.disconnect().await;
                    return Err(DAGError::Cancelled);
                }

                // Check for errors
                error = error_rx.recv() => {
                    if let Some(e) = error {
                        let _ = tts.disconnect().await;
                        return Err(DAGError::TTSProviderError {
                            provider: self.provider.clone(),
                            error: e.to_string(),
                        });
                    }
                }

                // Receive audio chunks
                chunk = audio_rx.recv() => {
                    match chunk {
                        Some(audio_data) => {
                            let chunk_size = audio_data.data.len();

                            // Check size limit to prevent memory exhaustion
                            if total_audio_bytes.saturating_add(chunk_size) > self.max_audio_bytes {
                                let _ = tts.disconnect().await;
                                return Err(DAGError::TTSProviderError {
                                    provider: self.provider.clone(),
                                    error: format!(
                                        "Audio size limit exceeded: {} bytes received, max {} bytes allowed",
                                        total_audio_bytes + chunk_size,
                                        self.max_audio_bytes
                                    ),
                                });
                            }

                            debug!(
                                node_id = %self.id,
                                chunk_size = %chunk_size,
                                total_bytes = %total_audio_bytes,
                                sample_rate = %audio_data.sample_rate,
                                "Received TTS audio chunk"
                            );

                            total_audio_bytes += chunk_size;
                            sample_rate = audio_data.sample_rate;
                            format = audio_data.format.clone();
                            audio_chunks.push(audio_data);
                        }
                        None => {
                            // Audio channel closed - synthesis complete
                            break;
                        }
                    }
                }

                // Wait for completion signal (only if we haven't received it yet)
                result = async {
                    if let Some(rx) = complete_rx.take() {
                        rx.await
                    } else {
                        // Already consumed, just pend forever
                        std::future::pending().await
                    }
                } => {
                    match result {
                        Ok(()) => debug!(node_id = %self.id, "TTS synthesis complete"),
                        Err(_) => debug!(node_id = %self.id, "TTS completion channel closed"),
                    }
                    complete_received = true;
                    // Continue loop to drain remaining chunks
                }

                // Timeout
                _ = tokio::time::sleep_until(deadline) => {
                    debug!(
                        node_id = %self.id,
                        chunks_received = %audio_chunks.len(),
                        "TTS timeout reached"
                    );
                    break;
                }
            }
        }

        // Disconnect from provider
        if let Err(e) = tts.disconnect().await {
            warn!(
                node_id = %self.id,
                error = %e,
                "Failed to disconnect from TTS provider"
            );
        }

        // Combine all audio chunks into a single buffer
        let total_duration: u32 = audio_chunks.iter().filter_map(|c| c.duration_ms).sum();

        let combined_data: Vec<u8> = audio_chunks.into_iter().flat_map(|c| c.data).collect();

        if combined_data.is_empty() {
            warn!(
                node_id = %self.id,
                "No audio data received from TTS provider"
            );
            return Ok(DAGData::Empty);
        }

        info!(
            node_id = %self.id,
            provider = %self.provider,
            audio_size = %combined_data.len(),
            duration_ms = %total_duration,
            "TTS synthesis completed"
        );

        Ok(DAGData::TTSAudio(TTSAudioData {
            data: Bytes::from(combined_data),
            sample_rate,
            format,
            duration_ms: if total_duration > 0 {
                Some(total_duration as u64)
            } else {
                None
            },
            is_final: true,
        }))
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// B-G2: one PERSISTENT realtime (S2S) session per node id — the socket,
/// its callbacks, and per-response signaling live across turns instead of
/// being rebuilt per `execute()` (the old request-scoped behavior cost a
/// full WS handshake + session.update on every utterance and discarded all
/// server-side conversation state).
pub struct SessionRealtime {
    /// The connected provider (locked per turn — one in-flight response).
    pub provider: tokio::sync::Mutex<crate::core::realtime::BoxedRealtime>,
    /// Monotonic `response.done` counter (watch, not Notify: a done that
    /// lands BEFORE the node starts waiting must not be lost — the waiter
    /// compares against the count it captured before `create_response`).
    response_done_tx: tokio::sync::watch::Sender<u64>,
    /// Latest finalized assistant transcript (taken per turn).
    last_transcript: parking_lot::Mutex<String>,
}

/// Shared across every per-turn `DAGContext` clone (the map itself is the
/// stable resource; entries are created on first use per node id). Inserted
/// at DAG-init under [`resource_keys::REALTIME_PROVIDER_PREFIX`]`sessions`.
pub type RealtimeSessionMap = parking_lot::Mutex<
    std::collections::HashMap<String, Arc<SessionRealtime>>,
>;

/// Resource key for the [`RealtimeSessionMap`].
pub fn realtime_sessions_key() -> String {
    format!(
        "{}sessions",
        crate::dag::context::resource_keys::REALTIME_PROVIDER_PREFIX
    )
}

/// Realtime provider node
///
/// Wraps a realtime provider (e.g., OpenAI Realtime) for bidirectional voice processing.
#[derive(Clone)]
pub struct RealtimeProviderNode {
    id: String,
    provider: String,
    model: Option<String>,
    config: serde_json::Value,
}

impl RealtimeProviderNode {
    /// Create a new realtime provider node
    pub fn new(id: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            model: None,
            config: serde_json::Value::Null,
        }
    }

    /// Set the model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set additional configuration
    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = config;
        self
    }

    /// Get the provider name
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// B-G2: get-or-create the persistent session for this node, run one
    /// turn through it, stream audio to the cascade sink, return the
    /// assistant transcript as the node's data output.
    async fn execute_session_scoped(
        &self,
        input: DAGData,
        ctx: &mut DAGContext,
        sessions: Arc<RealtimeSessionMap>,
    ) -> DAGResult<DAGData> {
        let (audio_data, text_data) = match &input {
            DAGData::Audio(bytes) => (Some(bytes.clone()), None),
            DAGData::TTSAudio(tts) => (Some(tts.data.clone()), None),
            DAGData::Text(text) => (None, Some(text.clone())),
            DAGData::STTResult(stt) => (None, Some(stt.transcript.clone())),
            DAGData::Empty => return Ok(DAGData::Empty),
            other => {
                return Err(DAGError::UnsupportedDataType {
                    expected: "audio or text".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        // Get-or-create OUTSIDE the entry lock (creation awaits connect).
        let existing = sessions.lock().get(self.id.to_string().as_str()).cloned();
        let session = match existing {
            Some(s) => s,
            None => {
                let s = self.create_session(ctx).await?;
                sessions.lock().insert(self.id.clone(), Arc::clone(&s));
                s
            }
        };

        let mut provider = session.provider.lock().await;
        // Subscribe BEFORE triggering the response: a done landing between
        // create_response and the wait below still advances the counter we
        // compare against (no lost wakeup).
        let mut done_rx = session.response_done_tx.subscribe();
        let baseline = *done_rx.borrow();
        if let Some(audio) = audio_data {
            provider.send_audio(audio).await.map_err(|e| {
                DAGError::RealtimeProviderError {
                    provider: self.provider.clone(),
                    error: format!("Failed to send audio: {e}"),
                }
            })?;
            // Manual mode needs an explicit commit; server-VAD providers
            // commit on their own.
            if !provider.emits_user_turn_frames()
                && let Err(e) = provider.commit_audio_buffer().await
            {
                warn!(node_id = %self.id, error = %e, "audio buffer commit failed");
            }
        }
        if let Some(text) = text_data {
            provider.send_text(&text).await.map_err(|e| {
                DAGError::RealtimeProviderError {
                    provider: self.provider.clone(),
                    error: format!("Failed to send text: {e}"),
                }
            })?;
        }
        provider.create_response().await.map_err(|e| {
            DAGError::RealtimeProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to create response: {e}"),
            }
        })?;
        drop(provider);

        // Audio streams to the sink as it arrives; here we only wait for the
        // turn boundary, then surface the transcript downstream.
        let timeout = ctx
            .remaining_time()
            .unwrap_or(Duration::from_secs(30))
            .min(Duration::from_secs(30));
        let wait = async {
            while *done_rx.borrow() <= baseline {
                if done_rx.changed().await.is_err() {
                    break;
                }
            }
        };
        if tokio::time::timeout(timeout, wait).await.is_err() {
            warn!(node_id = %self.id, "realtime response timed out; returning partial transcript");
        }
        let transcript = std::mem::take(&mut *session.last_transcript.lock());
        Ok(DAGData::Text(transcript))
    }

    /// Connect a NEW persistent provider with session-scoped callbacks:
    /// audio → the cascade sink (`DagOutput::Audio`), finalized assistant
    /// transcripts → the per-turn slot, `response.done` → the turn notify.
    async fn create_session(&self, ctx: &DAGContext) -> DAGResult<Arc<SessionRealtime>> {
        let registry = crate::plugin::global_registry();
        let realtime_config = RealtimeConfig {
            model: self.model.clone().unwrap_or_default(),
            provider: self.provider.clone(),
            ..Default::default()
        };
        let realtime = registry
            .create_realtime(&self.provider, realtime_config)
            .map_err(|e| DAGError::RealtimeProviderError {
                provider: self.provider.clone(),
                error: e.to_string(),
            })?;

        let (response_done_tx, _) = tokio::sync::watch::channel(0u64);
        let session = Arc::new(SessionRealtime {
            provider: tokio::sync::Mutex::new(realtime),
            response_done_tx,
            last_transcript: parking_lot::Mutex::new(String::new()),
        });

        {
            let mut provider = session.provider.lock().await;

            // Audio → the cascade sink: downstream cannot tell S2S from TTS.
            let output_tx = ctx.output_tx.clone().expect("checked by caller");
            let audio_cb: AudioOutputCallback = Arc::new(move |audio: RealtimeAudioData| {
                let tx = output_tx.clone();
                Box::pin(async move {
                    let _ = tx
                        .send(crate::dag::context::DagOutput::Audio(
                            crate::dag::nodes::TTSAudioData {
                                data: audio.data,
                                sample_rate: audio.sample_rate,
                                format: "pcm16".to_string(),
                                duration_ms: None,
                                is_final: false,
                            },
                        ))
                        .await;
                }) as Pin<Box<dyn Future<Output = ()> + Send>>
            });
            provider.on_audio(audio_cb).map_err(|e| DAGError::RealtimeProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to register audio callback: {e}"),
            })?;

            // Finalized ASSISTANT transcripts → the per-turn slot.
            let transcript_slot = Arc::clone(&session);
            let transcript_cb: TranscriptCallback = Arc::new(move |t: TranscriptResult| {
                let session = Arc::clone(&transcript_slot);
                Box::pin(async move {
                    if t.is_final
                        && matches!(t.role, crate::core::realtime::TranscriptRole::Assistant)
                    {
                        *session.last_transcript.lock() = t.text;
                    }
                }) as Pin<Box<dyn Future<Output = ()> + Send>>
            });
            provider.on_transcript(transcript_cb).map_err(|e| {
                DAGError::RealtimeProviderError {
                    provider: self.provider.clone(),
                    error: format!("Failed to register transcript callback: {e}"),
                }
            })?;

            // response.done → the turn boundary.
            let done = Arc::clone(&session);
            let done_cb: crate::core::realtime::ResponseDoneCallback =
                Arc::new(move |_response_id: String| {
                    let session = Arc::clone(&done);
                    Box::pin(async move {
                        session.response_done_tx.send_modify(|c| *c += 1);
                    }) as Pin<Box<dyn Future<Output = ()> + Send>>
                });
            provider.on_response_done(done_cb).map_err(|e| {
                DAGError::RealtimeProviderError {
                    provider: self.provider.clone(),
                    error: format!("Failed to register response-done callback: {e}"),
                }
            })?;

            provider.connect().await.map_err(|e| DAGError::RealtimeProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to connect: {e}"),
            })?;
            info!(
                node_id = %self.id,
                provider = %self.provider,
                "Persistent realtime session connected (B-G2)"
            );
        }

        Ok(session)
    }
}

impl std::fmt::Debug for RealtimeProviderNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealtimeProviderNode")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("model", &self.model)
            .finish()
    }
}

#[async_trait]
impl DAGNode for RealtimeProviderNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "realtime_provider"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::AudioInput,
            NodeCapability::TextInput,
            NodeCapability::AudioOutput,
            NodeCapability::TextOutput,
            NodeCapability::Streaming,
            NodeCapability::Cancellable,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(
            node_id = %self.id,
            provider = %self.provider,
            input_type = %input.type_name(),
            "Processing through realtime provider"
        );

        // B-G2: session-scoped path. When the session map resource is
        // present (every gateway DAG session — inserted at init) AND the
        // cascade output sink is attached, the provider persists across
        // turns and its audio rides DagOutput::Audio — byte-for-byte the
        // same downstream contract as cascade TTS (S2S is a drop-in).
        if let Some(sessions) = ctx.get_resource_as::<RealtimeSessionMap>(&realtime_sessions_key())
            && ctx.output_tx.is_some()
        {
            return self.execute_session_scoped(input, ctx, sessions).await;
        }

        // Legacy request-scoped path (direct executor use, unit tests, no
        // sink): unchanged behavior.

        // Extract input data (audio or text)
        let (audio_data, text_data, has_audio_input) = match &input {
            DAGData::Audio(bytes) => (Some(bytes.clone()), None, true),
            DAGData::TTSAudio(tts) => (Some(tts.data.clone()), None, true),
            DAGData::Text(text) => (None, Some(text.clone()), false),
            DAGData::STTResult(stt) => (None, Some(stt.transcript.clone()), false),
            DAGData::Empty => return Ok(DAGData::Empty),
            other => {
                return Err(DAGError::UnsupportedDataType {
                    expected: "audio or text".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        // Get realtime provider from registry
        let registry = crate::plugin::global_registry();

        // Build realtime configuration
        let realtime_config = RealtimeConfig {
            model: self.model.clone().unwrap_or_default(),
            provider: self.provider.clone(),
            ..Default::default()
        };

        // Create realtime provider
        let mut realtime = match registry.create_realtime(&self.provider, realtime_config) {
            Ok(rt) => rt,
            Err(e) => {
                return Err(DAGError::RealtimeProviderError {
                    provider: self.provider.clone(),
                    error: e.to_string(),
                });
            }
        };

        // Create channels for receiving results
        let (transcript_tx, mut transcript_rx) = mpsc::channel::<TranscriptResult>(16);
        let (audio_tx, mut audio_rx) = mpsc::channel::<RealtimeAudioData>(32);
        let (error_tx, mut error_rx) = mpsc::channel::<RealtimeError>(4);

        // Create transcript callback
        let transcript_sender = transcript_tx.clone();
        let transcript_callback: TranscriptCallback = Arc::new(move |result: TranscriptResult| {
            let tx = transcript_sender.clone();
            Box::pin(async move {
                if tx.send(result).await.is_err() {
                    trace!(
                        "Realtime transcript callback: channel closed, likely node execution completed"
                    );
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        // Create audio output callback
        let audio_sender = audio_tx.clone();
        let audio_callback: AudioOutputCallback = Arc::new(move |audio: RealtimeAudioData| {
            let tx = audio_sender.clone();
            Box::pin(async move {
                if tx.send(audio).await.is_err() {
                    trace!(
                        "Realtime audio callback: channel closed, likely node execution completed"
                    );
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        // Create error callback
        let error_sender = error_tx.clone();
        let error_callback: RealtimeErrorCallback = Arc::new(move |error: RealtimeError| {
            let tx = error_sender.clone();
            Box::pin(async move {
                if tx.send(error).await.is_err() {
                    trace!(
                        "Realtime error callback: channel closed, likely node execution completed"
                    );
                }
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        // Register callbacks
        if let Err(e) = realtime.on_transcript(transcript_callback) {
            return Err(DAGError::RealtimeProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to register transcript callback: {}", e),
            });
        }

        if let Err(e) = realtime.on_audio(audio_callback) {
            return Err(DAGError::RealtimeProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to register audio callback: {}", e),
            });
        }

        if let Err(e) = realtime.on_error(error_callback) {
            return Err(DAGError::RealtimeProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to register error callback: {}", e),
            });
        }

        // Connect to the realtime provider
        if let Err(e) = realtime.connect().await {
            return Err(DAGError::RealtimeProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to connect: {}", e),
            });
        }

        info!(
            node_id = %self.id,
            provider = %self.provider,
            "Connected to realtime provider"
        );

        // Send input data
        if let Some(audio) = audio_data {
            if let Err(e) = realtime.send_audio(audio).await {
                let _ = realtime.disconnect().await;
                return Err(DAGError::RealtimeProviderError {
                    provider: self.provider.clone(),
                    error: format!("Failed to send audio: {}", e),
                });
            }
            // Commit the audio buffer to trigger processing
            if let Err(e) = realtime.commit_audio_buffer().await {
                warn!(
                    node_id = %self.id,
                    provider = %self.provider,
                    error = %e,
                    "Failed to commit audio buffer"
                );
            }
        }

        if let Some(text) = text_data
            && let Err(e) = realtime.send_text(&text).await {
                let _ = realtime.disconnect().await;
                return Err(DAGError::RealtimeProviderError {
                    provider: self.provider.clone(),
                    error: format!("Failed to send text: {}", e),
                });
            }

        // Request a response from the model
        if let Err(e) = realtime.create_response().await {
            let _ = realtime.disconnect().await;
            return Err(DAGError::RealtimeProviderError {
                provider: self.provider.clone(),
                error: format!("Failed to create response: {}", e),
            });
        }

        // Wait for results with timeout
        let timeout = ctx.remaining_time().unwrap_or(Duration::from_secs(30));
        let timeout_duration = timeout.min(Duration::from_secs(30));

        // Collect results
        let mut collected_audio: Vec<u8> = Vec::new();
        let mut collected_transcript = String::new();
        let mut response_complete = false;

        let deadline = tokio::time::Instant::now() + timeout_duration;

        loop {
            tokio::select! {
                // Check for cancellation
                _ = ctx.cancel_token.cancelled() => {
                    let _ = realtime.disconnect().await;
                    return Err(DAGError::Cancelled);
                }

                // Receive transcript
                result = transcript_rx.recv() => {
                    if let Some(transcript) = result {
                        debug!(
                            node_id = %self.id,
                            text = %transcript.text,
                            is_final = %transcript.is_final,
                            "Received transcript"
                        );
                        if transcript.is_final {
                            collected_transcript = transcript.text;
                            // If we have transcript and either have audio or don't expect it, we're done
                            if !collected_audio.is_empty() || !has_audio_input {
                                response_complete = true;
                            }
                        }
                    }
                }

                // Receive audio
                result = audio_rx.recv() => {
                    if let Some(audio) = result {
                        debug!(
                            node_id = %self.id,
                            audio_size = %audio.data.len(),
                            "Received audio chunk"
                        );
                        collected_audio.extend_from_slice(&audio.data);
                        // Bound accumulated audio to prevent unbounded memory
                        // growth from a runaway realtime stream (W-O3 bug #5).
                        if collected_audio.len() > MAX_REALTIME_COLLECTED_AUDIO_BYTES {
                            warn!(
                                node_id = %self.id,
                                provider = %self.provider,
                                limit = %MAX_REALTIME_COLLECTED_AUDIO_BYTES,
                                "Realtime collected audio exceeded max size, terminating"
                            );
                            let _ = realtime.disconnect().await;
                            return Err(DAGError::RealtimeProviderError {
                                provider: self.provider.clone(),
                                error: format!(
                                    "Collected audio exceeded {} bytes",
                                    MAX_REALTIME_COLLECTED_AUDIO_BYTES
                                ),
                            });
                        }
                        // Check if we have enough context to consider response complete
                        // Audio is complete when we have both audio and a final transcript
                        if !collected_transcript.is_empty() {
                            response_complete = true;
                        }
                    }
                }

                // Receive errors
                result = error_rx.recv() => {
                    if let Some(error) = result {
                        let _ = realtime.disconnect().await;
                        return Err(DAGError::RealtimeProviderError {
                            provider: self.provider.clone(),
                            error: error.to_string(),
                        });
                    }
                }

                // Timeout
                _ = tokio::time::sleep_until(deadline) => {
                    warn!(
                        node_id = %self.id,
                        provider = %self.provider,
                        "Realtime response timeout"
                    );
                    break;
                }
            }

            if response_complete {
                break;
            }
        }

        // Disconnect from provider
        let _ = realtime.disconnect().await;

        // Return appropriate result based on what we collected
        if !collected_audio.is_empty() {
            info!(
                node_id = %self.id,
                provider = %self.provider,
                audio_size = %collected_audio.len(),
                transcript_len = %collected_transcript.len(),
                "Realtime processing complete"
            );

            // Return audio if we have it
            Ok(DAGData::TTSAudio(TTSAudioData {
                data: Bytes::from(collected_audio),
                sample_rate: 24000, // Realtime providers typically use 24kHz
                format: "pcm16".to_string(),
                duration_ms: None,
                is_final: true,
            }))
        } else if !collected_transcript.is_empty() {
            info!(
                node_id = %self.id,
                provider = %self.provider,
                transcript_len = %collected_transcript.len(),
                "Realtime processing complete (text only)"
            );

            // Return transcript if no audio
            Ok(DAGData::Text(collected_transcript))
        } else {
            warn!(
                node_id = %self.id,
                provider = %self.provider,
                "Realtime processing completed with no output"
            );
            Ok(DAGData::Empty)
        }
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stt_provider_builder() {
        let node = STTProviderNode::new("stt", "deepgram")
            .with_model("nova-2")
            .with_language("en-US");

        assert_eq!(node.id(), "stt");
        assert_eq!(node.provider(), "deepgram");
        assert_eq!(node.model, Some("nova-2".to_string()));
        assert_eq!(node.language, Some("en-US".to_string()));
    }

    #[test]
    fn test_tts_provider_builder() {
        let node = TTSProviderNode::new("tts", "elevenlabs")
            .with_voice("voice-123")
            .with_model("eleven_turbo_v2");

        assert_eq!(node.id(), "tts");
        assert_eq!(node.provider(), "elevenlabs");
        assert_eq!(node.voice_id, Some("voice-123".to_string()));
    }

    #[test]
    fn test_stt_capabilities() {
        let node = STTProviderNode::new("stt", "deepgram");
        let caps = node.capabilities();

        assert!(caps.contains(&NodeCapability::AudioInput));
        assert!(caps.contains(&NodeCapability::TextOutput));
        assert!(caps.contains(&NodeCapability::Streaming));
    }

    #[test]
    fn test_tts_capabilities() {
        let node = TTSProviderNode::new("tts", "elevenlabs");
        let caps = node.capabilities();

        assert!(caps.contains(&NodeCapability::TextInput));
        assert!(caps.contains(&NodeCapability::AudioOutput));
        assert!(caps.contains(&NodeCapability::Streaming));
    }

    #[test]
    fn test_realtime_capabilities() {
        let node = RealtimeProviderNode::new("rt", "openai");
        let caps = node.capabilities();

        // Realtime should support both audio and text I/O
        assert!(caps.contains(&NodeCapability::AudioInput));
        assert!(caps.contains(&NodeCapability::TextInput));
        assert!(caps.contains(&NodeCapability::AudioOutput));
        assert!(caps.contains(&NodeCapability::TextOutput));
    }
}

#[cfg(test)]
mod session_realtime_tests {
    use super::*;
    use crate::core::realtime::{
        AudioOutputCallback as RtAudioCb, BaseRealtime, ConnectionState as RtConn,
        RealtimeResult, TranscriptCallback as RtTranscriptCb, TranscriptRole,
    };
    use crate::dag::context::DagOutput;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CONNECTS: AtomicUsize = AtomicUsize::new(0);

    /// Mock S2S provider: `create_response` immediately emits one audio
    /// chunk, a finalized assistant transcript, and response-done.
    struct MockS2S {
        audio_cb: Option<RtAudioCb>,
        transcript_cb: Option<RtTranscriptCb>,
        done_cb: Option<crate::core::realtime::ResponseDoneCallback>,
        turn: usize,
    }

    #[async_trait::async_trait]
    impl BaseRealtime for MockS2S {
        fn new(_c: RealtimeConfig) -> RealtimeResult<Self> {
            Ok(Self { audio_cb: None, transcript_cb: None, done_cb: None, turn: 0 })
        }
        async fn connect(&mut self) -> RealtimeResult<()> {
            CONNECTS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn disconnect(&mut self) -> RealtimeResult<()> {
            Ok(())
        }
        fn is_ready(&self) -> bool {
            true
        }
        fn get_connection_state(&self) -> RtConn {
            RtConn::Connected
        }
        async fn send_audio(&mut self, _a: bytes::Bytes) -> RealtimeResult<()> {
            Ok(())
        }
        async fn send_text(&mut self, _t: &str) -> RealtimeResult<()> {
            Ok(())
        }
        async fn create_response(&mut self) -> RealtimeResult<()> {
            self.turn += 1;
            let turn = self.turn;
            if let Some(cb) = &self.audio_cb {
                cb(crate::core::realtime::RealtimeAudioData {
                    data: bytes::Bytes::from(vec![0xBB; 480]),
                    sample_rate: 24_000,
                    item_id: Some(format!("item_{turn}")),
                    response_id: Some(format!("resp_{turn}")),
                })
                .await;
            }
            if let Some(cb) = &self.transcript_cb {
                cb(crate::core::realtime::TranscriptResult {
                    text: format!("answer {turn}"),
                    role: TranscriptRole::Assistant,
                    is_final: true,
                    item_id: Some(format!("item_{turn}")),
                })
                .await;
            }
            if let Some(cb) = &self.done_cb {
                cb(format!("resp_{turn}")).await;
            }
            Ok(())
        }
        async fn cancel_response(&mut self) -> RealtimeResult<()> {
            Ok(())
        }
        async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
            Ok(())
        }
        async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
            Ok(())
        }
        fn on_transcript(&mut self, c: RtTranscriptCb) -> RealtimeResult<()> {
            self.transcript_cb = Some(c);
            Ok(())
        }
        fn on_audio(&mut self, c: RtAudioCb) -> RealtimeResult<()> {
            self.audio_cb = Some(c);
            Ok(())
        }
        fn on_error(
            &mut self,
            _c: crate::core::realtime::RealtimeErrorCallback,
        ) -> RealtimeResult<()> {
            Ok(())
        }
        fn on_function_call(
            &mut self,
            _c: crate::core::realtime::FunctionCallCallback,
        ) -> RealtimeResult<()> {
            Ok(())
        }
        fn on_speech_event(
            &mut self,
            _c: crate::core::realtime::SpeechEventCallback,
        ) -> RealtimeResult<()> {
            Ok(())
        }
        fn on_response_done(
            &mut self,
            c: crate::core::realtime::ResponseDoneCallback,
        ) -> RealtimeResult<()> {
            self.done_cb = Some(c);
            Ok(())
        }
        fn on_reconnection(
            &mut self,
            _c: crate::core::realtime::ReconnectionCallback,
        ) -> RealtimeResult<()> {
            Ok(())
        }
        async fn update_session(&mut self, _c: RealtimeConfig) -> RealtimeResult<()> {
            Ok(())
        }
        async fn submit_function_result(&mut self, _i: &str, _r: &str) -> RealtimeResult<()> {
            Ok(())
        }
        fn get_provider_info(&self) -> serde_json::Value {
            serde_json::json!({})
        }
    }

    /// B-G2: session-scoped realtime — ONE connect across turns, audio
    /// routed through the cascade `DagOutput::Audio` sink, transcript
    /// surfaced as the node's data output.
    #[tokio::test]
    async fn realtime_session_persists_and_routes_audio_through_cascade_sink() {
        crate::plugin::global_registry().register_realtime(
            "mock-s2s-bg2",
            Arc::new(|c| Ok(Box::new(MockS2S::new(c)?) as Box<dyn BaseRealtime>)),
            crate::plugin::ProviderMetadata {
                name: "mock-s2s-bg2".into(),
                display_name: "Mock S2S".into(),
                description: "test".into(),
                ..Default::default()
            },
        );
        CONNECTS.store(0, Ordering::SeqCst);

        let (output_tx, mut output_rx) = tokio::sync::mpsc::channel::<DagOutput>(16);
        let mut ctx = DAGContext::new("session-rt".to_string());
        ctx.set_output_tx(output_tx);
        let sessions: Arc<RealtimeSessionMap> =
            Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
        ctx.set_resource(realtime_sessions_key(), Arc::clone(&sessions));

        let node = RealtimeProviderNode::new("rt1", "mock-s2s-bg2");
        let started = std::time::Instant::now();

        // Turn 1
        let out = node
            .execute(DAGData::Text("hello".into()), &mut ctx)
            .await
            .expect("turn 1");
        assert!(matches!(&out, DAGData::Text(t) if t == "answer 1"), "got {out:?}");
        // Turn 2 — the SAME session (no reconnect).
        let out = node
            .execute(DAGData::Text("again".into()), &mut ctx)
            .await
            .expect("turn 2");
        assert!(matches!(&out, DAGData::Text(t) if t == "answer 2"), "got {out:?}");

        assert_eq!(
            CONNECTS.load(Ordering::SeqCst),
            1,
            "the provider must persist across turns (request-scoped was 1 connect PER turn)"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "a done landing before the wait must not be LOST (lost wakeup = 30s timeout/turn)"
        );
        assert_eq!(sessions.lock().len(), 1);

        // Audio rode the cascade sink — indistinguishable from cascade TTS.
        let mut sink_audio = 0usize;
        while let Ok(out) = output_rx.try_recv() {
            if let DagOutput::Audio(a) = out {
                assert_eq!(a.data.len(), 480);
                assert_eq!(a.sample_rate, 24_000);
                assert_eq!(a.format, "pcm16");
                sink_audio += 1;
            }
        }
        assert_eq!(sink_audio, 2, "one audio chunk per turn through DagOutput::Audio");
    }
}

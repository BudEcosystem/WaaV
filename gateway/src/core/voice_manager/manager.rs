//! Main VoiceManager implementation

use bytes::Bytes;
use parking_lot::RwLock as SyncRwLock;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use tokio::sync::{Notify, RwLock};
use tokio::time::Duration;
use tracing::debug;

use crate::core::cache::store::CacheStore;
use crate::core::{
    create_stt_provider, create_tts_provider,
    stt::{
        BaseSTT, STTError, STTErrorCallback as ProviderSTTErrorCallback, STTResult,
        STTResultCallback,
    },
    tts::{AudioData, BaseTTS, TTSError},
    turn_detect::TurnDetector,
};

#[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
use crate::core::smart_turn::{SmartTurnProcessResult, SmartTurnProcessor};

use super::{
    callbacks::{
        AudioClearCallback, STTCallback, STTErrorCallback, TTSAudioCallback, TTSCompleteCallback,
        TTSErrorCallback, VoiceManagerTTSCallback,
    },
    config::VoiceManagerConfig,
    errors::{VoiceManagerError, VoiceManagerResult},
    state::{InterruptionState, SpeechFinalState},
    stt_result::{STTProcessingConfig, STTResultProcessor},
};

/// Callback type for smart turn detection results.
#[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
pub type SmartTurnCallback = Arc<
    dyn Fn(SmartTurnProcessResult) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync
        + 'static,
>;

/// VoiceManager provides a unified interface for managing STT and TTS providers
/// Optimized for extreme low-latency with lock-free atomics and pre-allocated buffers
pub struct VoiceManager {
    tts: Arc<RwLock<Box<dyn BaseTTS>>>,
    stt: Arc<RwLock<Box<dyn BaseSTT>>>,

    // Callbacks - using parking_lot RwLock for faster synchronization
    stt_callback: Arc<SyncRwLock<Option<STTCallback>>>,
    stt_error_callback: Arc<SyncRwLock<Option<STTErrorCallback>>>,
    tts_audio_callback: Arc<SyncRwLock<Option<TTSAudioCallback>>>,
    tts_error_callback: Arc<SyncRwLock<Option<TTSErrorCallback>>>,
    audio_clear_callback: Arc<SyncRwLock<Option<AudioClearCallback>>>,
    tts_complete_callback: Arc<SyncRwLock<Option<TTSCompleteCallback>>>,

    // Speech final timing control - using parking_lot for faster access
    speech_final_state: Arc<SyncRwLock<SpeechFinalState>>,

    // Turn detection for better end-of-speech detection
    turn_detector: Option<Arc<RwLock<TurnDetector>>>,

    // Audio-based smart turn processor (VAD + ML turn detection)
    // Uses interior mutability so it can be initialized in start()
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    smart_turn_processor: Arc<RwLock<Option<SmartTurnProcessor>>>,

    // Callback for smart turn detection results
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    smart_turn_callback: Arc<SyncRwLock<Option<SmartTurnCallback>>>,

    // Interruption control - mostly lock-free with atomics
    interruption_state: Arc<InterruptionState>,

    // Configuration
    config: VoiceManagerConfig,

    // Notification for audio clear completion instead of sleep
    clear_notify: Arc<Notify>,
}

impl VoiceManager {
    /// Create a new VoiceManager with the given configuration
    ///
    /// # Arguments
    /// * `config` - Configuration for both STT and TTS providers
    ///
    /// # Returns
    /// * `VoiceManagerResult<Self>` - A new VoiceManager instance or error
    ///
    /// # Example
    /// ```rust,no_run
    /// use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// use waav_gateway::core::stt::STTConfig;
    /// use waav_gateway::core::tts::TTSConfig;
    ///
    /// fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let stt_config = STTConfig {
    ///         provider: "deepgram".to_string(),
    ///         api_key: "your-api-key".to_string(),
    ///         ..Default::default()
    ///     };
    ///     let tts_config = TTSConfig {
    ///         provider: "deepgram".to_string(),
    ///         api_key: "your-api-key".to_string(),
    ///         ..Default::default()
    ///     };
    ///
    ///     let config = VoiceManagerConfig::new(stt_config, tts_config);
    ///     let voice_manager = VoiceManager::new(config, None)?;
    ///     Ok(())
    /// }
    /// ```
    pub fn new(
        config: VoiceManagerConfig,
        turn_detector: Option<Arc<RwLock<TurnDetector>>>,
    ) -> VoiceManagerResult<Self> {
        let tts = create_tts_provider(&config.tts_config.provider, config.tts_config.clone())
            .map_err(VoiceManagerError::TTSError)?;
        let stt = create_stt_provider(&config.stt_config.provider, config.stt_config.clone())
            .map_err(VoiceManagerError::STTError)?;

        // Pre-allocate string buffers with reasonable capacity
        const TEXT_BUFFER_CAPACITY: usize = 1024;
        let text_buffer = String::with_capacity(TEXT_BUFFER_CAPACITY);

        Ok(Self {
            tts: Arc::new(RwLock::new(tts)),
            stt: Arc::new(RwLock::new(stt)),
            stt_callback: Arc::new(SyncRwLock::new(None)),
            stt_error_callback: Arc::new(SyncRwLock::new(None)),
            tts_audio_callback: Arc::new(SyncRwLock::new(None)),
            tts_error_callback: Arc::new(SyncRwLock::new(None)),
            audio_clear_callback: Arc::new(SyncRwLock::new(None)),
            tts_complete_callback: Arc::new(SyncRwLock::new(None)),
            speech_final_state: Arc::new(SyncRwLock::new(SpeechFinalState {
                text_buffer,
                turn_detection_handle: None,
                hard_timeout_handle: None,
                waiting_for_speech_final: AtomicBool::new(false),
                user_callback: None,
                turn_detection_last_fired_ms: AtomicUsize::new(0),
                last_forced_text: String::with_capacity(1024),
                segment_start_ms: AtomicUsize::new(0),
                hard_timeout_deadline_ms: AtomicUsize::new(0),
            })),
            turn_detector,
            #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
            smart_turn_processor: Arc::new(RwLock::new(None)), // Initialized in start() if configured
            #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
            smart_turn_callback: Arc::new(SyncRwLock::new(None)),
            interruption_state: Arc::new(InterruptionState {
                allow_interruption: AtomicBool::new(true),
                non_interruptible_until_ms: AtomicUsize::new(0),
                current_sample_rate: AtomicU32::new(24000),
                is_completed: AtomicBool::new(true), // Start as completed
            }),
            config,
            clear_notify: Arc::new(Notify::new()),
        })
    }

    /// Set the TTS cache store and optionally the precomputed TTS config hash
    pub async fn set_tts_cache(
        &self,
        cache: Arc<CacheStore>,
        config_hash: Option<String>,
    ) -> VoiceManagerResult<()> {
        let mut tts = self.tts.write().await;
        if let Some(provider) = tts.get_provider() {
            provider.set_cache(cache).await;
            if let Some(hash) = config_hash {
                provider.set_tts_config_hash(hash).await;
            }
            Ok(())
        } else {
            Err(VoiceManagerError::InitializationError(
                "TTS provider does not support cache".to_string(),
            ))
        }
    }

    /// Start the VoiceManager by connecting to both STT and TTS providers
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default());
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// voice_manager.start().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn start(&self) -> VoiceManagerResult<()> {
        // Connect STT provider
        {
            let mut stt = self.stt.write().await;
            stt.connect().await.map_err(VoiceManagerError::STTError)?;
        }

        // Connect TTS provider
        {
            let mut tts = self.tts.write().await;
            tts.connect().await.map_err(VoiceManagerError::TTSError)?;
        }

        // Initialize Smart Turn Processor if configured
        #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
        {
            if let Some(ref smart_turn_config) = self.config.smart_turn_config {
                debug!("Initializing SmartTurnProcessor...");
                let processor = SmartTurnProcessor::new(smart_turn_config.clone())
                    .await
                    .map_err(|e| {
                        VoiceManagerError::InitializationError(format!(
                            "Failed to initialize SmartTurnProcessor: {}",
                            e
                        ))
                    })?;

                let mut smart_turn = self.smart_turn_processor.write().await;
                *smart_turn = Some(processor);
                debug!("SmartTurnProcessor initialized successfully");
            }
        }

        // Set up internal TTS callback - using parking_lot for faster access
        {
            let mut tts = self.tts.write().await;
            let tts_callback = Arc::new(VoiceManagerTTSCallback {
                audio_callback: self.tts_audio_callback.read().clone(),
                error_callback: self.tts_error_callback.read().clone(),
                interruption_state: Some(self.interruption_state.clone()),
                complete_callback: self.tts_complete_callback.read().clone(),
            });

            tts.on_audio(tts_callback)
                .map_err(VoiceManagerError::TTSError)?;
        }

        Ok(())
    }

    /// Stop the VoiceManager by disconnecting from both providers
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default());
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// voice_manager.stop().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn stop(&self) -> VoiceManagerResult<()> {
        // Cancel any pending speech final timer
        {
            let mut state = self.speech_final_state.write();
            if let Some(handle) = state.turn_detection_handle.take() {
                handle.abort();
            }
            // Cancel hard timeout handle
            if let Some(handle) = state.hard_timeout_handle.take() {
                handle.abort();
            }
            // Reset speech final state - reuse allocated capacity
            state.text_buffer.clear();
            state
                .waiting_for_speech_final
                .store(false, Ordering::Release);
            state.user_callback = None;
            state
                .turn_detection_last_fired_ms
                .store(0, Ordering::Release);
            state.last_forced_text.clear();
            state.segment_start_ms.store(0, Ordering::Release);
            state.hard_timeout_deadline_ms.store(0, Ordering::Release);
        }

        // Reset smart turn processor
        #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
        {
            let mut smart_turn = self.smart_turn_processor.write().await;
            if let Some(ref mut processor) = *smart_turn {
                processor.reset();
            }
        }

        // Disconnect STT provider
        {
            let mut stt = self.stt.write().await;
            stt.disconnect()
                .await
                .map_err(VoiceManagerError::STTError)?;
        }

        // Disconnect TTS provider
        {
            let mut tts = self.tts.write().await;
            tts.disconnect()
                .await
                .map_err(VoiceManagerError::TTSError)?;
        }

        Ok(())
    }

    /// Check if both STT and TTS providers are ready
    ///
    /// # Returns
    /// * `bool` - True if both providers are ready, false otherwise
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default());
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// if voice_manager.is_ready().await {
    ///     println!("VoiceManager is ready!");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn is_ready(&self) -> bool {
        let stt_ready = {
            let stt = self.stt.read().await;
            stt.is_ready()
        };

        let tts_ready = {
            let tts = self.tts.read().await;
            tts.is_ready()
        };

        stt_ready && tts_ready
    }

    /// Send audio data to the STT provider for transcription
    ///
    /// # Arguments
    /// * `audio` - Audio bytes to process
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default());
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// let audio_data = bytes::Bytes::from(vec![0u8; 1024]); // Your audio data
    /// voice_manager.receive_audio(audio_data).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn receive_audio(&self, audio: Bytes) -> VoiceManagerResult<()> {
        // CRITICAL: Audio MUST always reach STT provider for real-time guarantees.
        // Smart turn processing is optional - skip if lock is busy to avoid blocking.

        // Process audio through Smart Turn Processor if enabled AND lock is available
        #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
        {
            // Use try_write() to avoid blocking the audio hot path
            // If another frame is being processed, skip smart turn for this frame
            // This ensures audio forwarding is never delayed by ML inference (~20ms)
            match self.smart_turn_processor.try_write() {
                Ok(mut smart_turn_guard) => {
                    if let Some(ref mut processor) = *smart_turn_guard {
                        // Convert bytes to f32 samples (assuming 16-bit PCM)
                        let samples = self.bytes_to_f32_samples(&audio);

                        // Process through smart turn detector
                        match processor.process_audio(&samples).await {
                            Ok(result) => {
                                // If turn was detected, call the callback
                                if result.is_turn_complete {
                                    debug!(
                                        "Smart turn detected: prob={:.3}, silence={}ms",
                                        result.probability, result.silence_duration_ms
                                    );

                                    let callback_opt = self.smart_turn_callback.read().clone();
                                    if let Some(callback) = callback_opt {
                                        // Drop the lock before calling callback to avoid deadlock
                                        drop(smart_turn_guard);
                                        callback(result).await;
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("Smart turn processing error: {}", e);
                            }
                        }
                    }
                }
                Err(_) => {
                    // Smart turn lock is contended - ML inference is in progress on another frame.
                    // This is expected under high load. Log at trace level to enable monitoring
                    // without flooding logs. Audio will still be sent to STT below.
                    // Turn detection may be slightly delayed but audio latency is preserved.
                    tracing::trace!(
                        "Smart turn lock contended, skipping frame. Audio forwarding continues."
                    );
                }
            }
        }

        // Send audio to STT provider (zero-copy pass-through)
        // This ALWAYS happens regardless of smart turn processing result
        let mut stt = self.stt.write().await;
        stt.send_audio(audio)
            .await
            .map_err(VoiceManagerError::STTError)?;
        Ok(())
    }

    /// Convert raw audio bytes (16-bit PCM) to f32 samples normalized to [-1.0, 1.0]
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    #[inline]
    fn bytes_to_f32_samples(&self, bytes: &[u8]) -> Vec<f32> {
        // 16-bit PCM: 2 bytes per sample, little-endian
        let sample_count = bytes.len() / 2;
        let mut samples = Vec::with_capacity(sample_count);

        for i in 0..sample_count {
            let idx = i * 2;
            if idx + 1 < bytes.len() {
                // Little-endian 16-bit signed integer
                let sample_i16 = i16::from_le_bytes([bytes[idx], bytes[idx + 1]]);
                // Normalize to [-1.0, 1.0]
                samples.push(sample_i16 as f32 / 32768.0);
            }
        }

        samples
    }

    /// Send text to the TTS provider for synthesis
    ///
    /// # Arguments
    /// * `text` - Text to synthesize
    /// * `flush` - Whether to immediately flush and start processing the text
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default());
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// // Queue text without immediate processing
    /// voice_manager.speak("Hello, world!", false).await?;
    /// voice_manager.speak("How are you?", false).await?;
    ///
    /// // Send and immediately process
    /// voice_manager.speak("Final message", true).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn speak(&self, text: &str, flush: bool) -> VoiceManagerResult<()> {
        // Send text to TTS provider
        {
            let mut tts = self.tts.write().await;
            tts.speak(text, flush)
                .await
                .map_err(VoiceManagerError::TTSError)?;
        }

        Ok(())
    }

    /// Send text to the TTS provider with interruption control
    ///
    /// # Arguments
    /// * `text` - Text to synthesize
    /// * `flush` - Whether to immediately flush and start processing the text
    /// * `allow_interruption` - Whether this audio can be interrupted by STT or clear commands
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    pub async fn speak_with_interruption(
        &self,
        text: &str,
        flush: bool,
        allow_interruption: bool,
    ) -> VoiceManagerResult<()> {
        // Update interruption state
        self.interruption_state
            .allow_interruption
            .store(allow_interruption, Ordering::Release);

        if !allow_interruption {
            // Update sample rate from TTS config
            if let Some(sample_rate) = self.config.tts_config.sample_rate {
                self.interruption_state
                    .current_sample_rate
                    .store(sample_rate, Ordering::Release);
            }

            // Get current timestamp in milliseconds
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as usize;

            // Initialize non_interruptible_until_ms to current time
            // The actual duration will be calculated as TTS chunks arrive
            self.interruption_state
                .non_interruptible_until_ms
                .store(now, Ordering::Release);

            // Mark as not completed since new audio is starting
            self.interruption_state
                .is_completed
                .store(false, Ordering::SeqCst);
        } else {
            // For interruptible audio, just reset to defaults
            self.interruption_state.reset();
        }

        // Send text to TTS provider
        {
            let mut tts = self.tts.write().await;
            tts.speak(text, flush)
                .await
                .map_err(VoiceManagerError::TTSError)?;
        }

        Ok(())
    }

    /// Check if interruption is currently blocked
    ///
    /// # Returns
    /// * `bool` - True if interruption is currently blocked
    pub async fn is_interruption_blocked(&self) -> bool {
        !self.interruption_state.can_interrupt()
    }

    /// Clear any queued text from the TTS provider and audio buffers
    ///
    /// This method clears both the TTS text queue and any audio buffers
    /// (e.g., LiveKit audio source) if an audio clear callback is registered.
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    pub async fn clear_tts(&self) -> VoiceManagerResult<()> {
        // Check if we're allowed to clear
        if !self.interruption_state.can_interrupt() {
            // Not allowed to interrupt yet
            return Ok(());
        }

        debug!("Starting audio clearing process");

        // Clear TTS text queue
        let mut tts = self.tts.write().await;
        tts.clear().await.map_err(VoiceManagerError::TTSError)?;
        drop(tts); // Release the lock

        // Call audio clear callback to clear any audio buffers (e.g., LiveKit)
        {
            let callback_opt = self.audio_clear_callback.read().clone();
            if let Some(callback) = callback_opt {
                callback().await;
            }
        }

        // Notify any waiters that the clear operation is complete
        // This wakes up coroutines waiting on clear_notify.notified()
        self.clear_notify.notify_one();

        // Use notification instead of sleep for better latency
        // Wait briefly for any pending audio to be flushed through the pipeline
        let _ = tokio::time::timeout(
            Duration::from_millis(50), // Short timeout for pipeline flush
            tokio::time::sleep(Duration::from_millis(10)),
        )
        .await;

        // Reset interruption state since we interrupted
        self.interruption_state.reset();

        debug!("Completed audio clearing process");

        Ok(())
    }

    /// Flush the TTS provider to process queued text immediately
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    pub async fn flush_tts(&self) -> VoiceManagerResult<()> {
        let tts = self.tts.read().await;
        tts.flush().await.map_err(VoiceManagerError::TTSError)?;
        Ok(())
    }

    /// Register a callback for STT results
    ///
    /// # Arguments
    /// * `callback` - Callback function to handle STT results
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default());
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// voice_manager.on_stt_result(|result| {
    ///     Box::pin(async move {
    ///         println!("Transcription: {}", result.transcript);
    ///     })
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn on_stt_result<F>(&self, callback: F) -> VoiceManagerResult<()>
    where
        F: Fn(STTResult) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);

        // Store the callback for later use - using parking_lot for faster access
        {
            let mut stt_callback = self.stt_callback.write();
            *stt_callback = Some(callback.clone());
        }

        // Also store in speech final state for timer access
        {
            let mut state = self.speech_final_state.write();
            state.user_callback = Some(callback.clone());
        }

        // Pre-clone Arc references outside the callback to reduce per-invocation overhead
        let speech_final_state_clone = self.speech_final_state.clone();
        let interruption_state_clone = self.interruption_state.clone();
        let turn_detector_clone = self.turn_detector.clone();

        // Create STT processor with configured timeouts from VoiceManagerConfig
        let processing_config = STTProcessingConfig::new(
            self.config.speech_final_config.stt_speech_final_wait_ms,
            self.config
                .speech_final_config
                .turn_detection_inference_timeout_ms,
            self.config.speech_final_config.speech_final_hard_timeout_ms,
            self.config.speech_final_config.duplicate_window_ms,
        );
        let stt_processor = STTResultProcessor::new(processing_config);

        let wrapper_callback: STTResultCallback = Arc::new(move |result| {
            // Clone Arc references per invocation (lightweight operation)
            let callback = callback.clone();
            let speech_final_state = speech_final_state_clone.clone();
            let interruption_state = interruption_state_clone.clone();
            let turn_detector = turn_detector_clone.clone();
            let stt_processor = stt_processor.clone();

            Box::pin(async move {
                // Fast synchronous check for interruption - execute before any async ops
                if !interruption_state.can_interrupt() {
                    // Still within non-interruptible period, ignore STT result
                    return;
                }

                // Process result with timing control - now non-blocking for result delivery
                let processed_result = stt_processor
                    .process_result(result, speech_final_state, turn_detector)
                    .await;

                if let Some(processed_result) = processed_result {
                    // Call user callback with processed result
                    callback(processed_result).await;
                }
                // If None returned, result was suppressed (empty interim result)
            })
        });

        // Register callback with STT provider
        {
            let mut stt = self.stt.write().await;
            stt.on_result(wrapper_callback)
                .await
                .map_err(VoiceManagerError::STTError)?;
        }

        Ok(())
    }

    /// Register a callback for STT streaming errors
    ///
    /// This callback is triggered when errors occur during STT streaming,
    /// such as permission errors, network failures, or API errors that happen
    /// after the initial connection is established.
    ///
    /// # Arguments
    /// * `callback` - Callback function to handle STT errors
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let voice_manager = VoiceManager::new(
    /// #     VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default()),
    /// #     None
    /// # )?;
    /// voice_manager.on_stt_error(|error| {
    ///     Box::pin(async move {
    ///         eprintln!("STT streaming error: {}", error);
    ///         // Handle error: notify user, reconnect, etc.
    ///     })
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn on_stt_error<F>(&self, callback: F) -> VoiceManagerResult<()>
    where
        F: Fn(STTError) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);

        // Store the callback for later use
        {
            let mut stt_error_callback = self.stt_error_callback.write();
            *stt_error_callback = Some(callback.clone());
        }

        // Create wrapper callback for the provider
        let wrapper_callback: ProviderSTTErrorCallback = Arc::new(move |error| {
            let callback = callback.clone();
            Box::pin(async move {
                callback(error).await;
            })
        });

        // Register callback with STT provider
        {
            let mut stt = self.stt.write().await;
            stt.on_error(wrapper_callback)
                .await
                .map_err(VoiceManagerError::STTError)?;
        }

        Ok(())
    }

    /// Register a callback for TTS audio data
    ///
    /// # Arguments
    /// * `callback` - Callback function to handle TTS audio data
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default());
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// voice_manager.on_tts_audio(|audio_data| {
    ///     Box::pin(async move {
    ///         println!("Received {} bytes of audio", audio_data.data.len());
    ///     })
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn on_tts_audio<F>(&self, callback: F) -> VoiceManagerResult<()>
    where
        F: Fn(AudioData) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    {
        let user_callback = Arc::new(callback);
        let interruption_state_clone = self.interruption_state.clone();

        // Create wrapper that checks clearing state and updates interruption timing
        let wrapper_callback = Arc::new(move |audio_data: AudioData| {
            let user_cb = user_callback.clone();
            let int_state = interruption_state_clone.clone();

            Box::pin(async move {
                // Check if this is new audio after completion
                if int_state.is_completed.load(Ordering::Acquire) {
                    // New audio starting after completion
                    int_state.is_completed.store(false, Ordering::SeqCst);

                    // Reset the non_interruptible_until_ms to current time if we're in non-interruptible mode
                    if !int_state.allow_interruption.load(Ordering::Acquire) {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as usize;
                        int_state
                            .non_interruptible_until_ms
                            .store(now_ms, Ordering::Release);
                    }
                }

                // Calculate audio duration and update non_interruptible_until_ms
                if !int_state.allow_interruption.load(Ordering::Acquire) {
                    // Calculate actual audio duration from audio data
                    // For PCM/linear16: bytes / (sample_rate * bytes_per_sample * channels)
                    // Assuming 16-bit audio (2 bytes per sample) and mono (1 channel)
                    let bytes_per_sample = 2;
                    let channels = 1;
                    let sample_rate = int_state.current_sample_rate.load(Ordering::Acquire);

                    // Guard against division by zero - use default sample rate if zero
                    let safe_sample_rate = if sample_rate == 0 { 24000 } else { sample_rate };

                    let chunk_duration_seconds = audio_data.data.len() as f32
                        / (safe_sample_rate as f32 * bytes_per_sample as f32 * channels as f32);

                    let chunk_duration_ms = (chunk_duration_seconds * 1000.0) as usize;

                    // Add duration to non_interruptible_until_ms
                    let current_until =
                        int_state.non_interruptible_until_ms.load(Ordering::Acquire);
                    int_state
                        .non_interruptible_until_ms
                        .store(current_until + chunk_duration_ms, Ordering::Release);
                }

                // Call the user's callback
                user_cb(audio_data).await;
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        // Store callback and release lock before await
        let audio_callback = {
            let mut tts_audio_callback = self.tts_audio_callback.write();
            *tts_audio_callback = Some(wrapper_callback.clone());
            tts_audio_callback.clone()
        };

        // Update the internal TTS callback
        {
            let mut tts = self.tts.write().await;
            let tts_callback = Arc::new(VoiceManagerTTSCallback {
                audio_callback,
                error_callback: self.tts_error_callback.read().clone(),
                interruption_state: Some(self.interruption_state.clone()),
                complete_callback: self.tts_complete_callback.read().clone(),
            });

            tts.on_audio(tts_callback)
                .map_err(VoiceManagerError::TTSError)?;
        }

        Ok(())
    }

    /// Register a callback for TTS errors
    ///
    /// # Arguments
    /// * `callback` - Callback function to handle TTS errors
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    pub async fn on_tts_error<F>(&self, callback: F) -> VoiceManagerResult<()>
    where
        F: Fn(TTSError) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    {
        // Store callback and then release lock before await
        let error_callback = {
            let mut tts_error_callback = self.tts_error_callback.write();
            *tts_error_callback = Some(Arc::new(callback));
            tts_error_callback.clone()
        };

        // Update the internal TTS callback
        {
            let mut tts = self.tts.write().await;
            let tts_callback = Arc::new(VoiceManagerTTSCallback {
                audio_callback: self.tts_audio_callback.read().clone(),
                error_callback,
                interruption_state: Some(self.interruption_state.clone()),
                complete_callback: self.tts_complete_callback.read().clone(),
            });

            tts.on_audio(tts_callback)
                .map_err(VoiceManagerError::TTSError)?;
        }

        Ok(())
    }

    /// Register a callback for audio clear operations
    ///
    /// This callback is called when the TTS queue is cleared and any audio
    /// buffers (e.g., LiveKit audio source) need to be cleared as well.
    ///
    /// # Arguments
    /// * `callback` - Closure that returns a Future for clearing audio
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default());
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// voice_manager.on_audio_clear(|| {
    ///     Box::pin(async move {
    ///         // Clear LiveKit audio buffer or other audio sources
    ///         println!("Clearing audio buffers");
    ///     })
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn on_audio_clear<F>(&self, callback: F) -> VoiceManagerResult<()>
    where
        F: Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    {
        let mut audio_clear_callback = self.audio_clear_callback.write();
        *audio_clear_callback = Some(Arc::new(callback));
        Ok(())
    }

    /// Register a callback for smart turn detection results.
    ///
    /// This callback is called when the audio-based turn detector determines
    /// that the user has finished speaking. This provides earlier turn detection
    /// than waiting for STT speech_final, enabling faster response times.
    ///
    /// # Arguments
    /// * `callback` - Callback function to handle turn detection results
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,ignore
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # use waav_gateway::core::smart_turn::SmartTurnProcessorConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let smart_turn_config = SmartTurnProcessorConfig::default();
    /// # let config = VoiceManagerConfig::with_smart_turn(
    /// #     STTConfig::default(),
    /// #     TTSConfig::default(),
    /// #     smart_turn_config,
    /// # );
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// voice_manager.on_smart_turn(|result| {
    ///     Box::pin(async move {
    ///         if result.is_turn_complete {
    ///             println!("Turn complete: prob={:.2}, silence={}ms",
    ///                      result.probability, result.silence_duration_ms);
    ///         }
    ///     })
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    pub async fn on_smart_turn<F>(&self, callback: F) -> VoiceManagerResult<()>
    where
        F: Fn(SmartTurnProcessResult) -> Pin<Box<dyn Future<Output = ()> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let mut smart_turn_callback = self.smart_turn_callback.write();
        *smart_turn_callback = Some(Arc::new(callback));
        Ok(())
    }

    /// Returns whether smart turn detection is enabled and initialized.
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    pub async fn is_smart_turn_enabled(&self) -> bool {
        let guard = self.smart_turn_processor.read().await;
        guard.is_some()
    }

    /// Returns the current speech state from smart turn processor.
    ///
    /// Returns `None` if smart turn is not enabled.
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    pub async fn smart_turn_is_speech(&self) -> Option<bool> {
        let guard = self.smart_turn_processor.read().await;
        guard.as_ref().map(|p| p.is_speech())
    }

    /// Returns the current silence duration from smart turn processor.
    ///
    /// Returns `None` if smart turn is not enabled.
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    pub async fn smart_turn_silence_duration_ms(&self) -> Option<f32> {
        let guard = self.smart_turn_processor.read().await;
        guard.as_ref().map(|p| p.silence_duration_ms())
    }

    /// Reset the smart turn processor state for a new conversation.
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    pub async fn reset_smart_turn(&self) -> VoiceManagerResult<()> {
        let mut guard = self.smart_turn_processor.write().await;
        if let Some(ref mut processor) = *guard {
            processor.reset();
            debug!("SmartTurnProcessor reset");
        }
        Ok(())
    }

    /// Register a callback to be invoked when TTS playback completes
    ///
    /// The completion callback is triggered after the TTS provider finishes generating
    /// all audio chunks for a given `speak()` command. This is useful for:
    /// - Updating UI state (hiding loading indicators)
    /// - Coordinating sequential actions
    /// - Analytics and monitoring
    /// - Knowing when it's safe to perform operations
    ///
    /// # Important Notes
    /// - Callback fires once per `speak()` call
    /// - Callback fires after all audio chunks are generated
    /// - Callback timing indicates server-side generation completion, not client playback
    /// - Multiple `speak()` calls will trigger multiple callbacks in FIFO order
    ///
    /// # Arguments
    /// * `callback` - Async function to call when TTS playback completes
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,ignore
    /// use waav_gateway::core::voice_manager::VoiceManager;
    ///
    /// let voice_manager = VoiceManager::new(config, None)?;
    /// voice_manager.start().await?;
    ///
    /// // Register completion callback
    /// voice_manager.on_tts_complete(|| {
    ///     Box::pin(async move {
    ///         println!("TTS playback completed!");
    ///         // Update UI state, trigger next action, etc.
    ///     })
    /// }).await?;
    ///
    /// voice_manager.speak("Hello world", true, true).await?;
    /// // Callback will fire after "Hello world" is fully generated
    /// ```
    pub async fn on_tts_complete<F>(&self, callback: F) -> VoiceManagerResult<()>
    where
        F: Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    {
        // Store the callback
        *self.tts_complete_callback.write() = Some(Arc::new(callback));

        // Update the TTS provider's callback to include completion callback
        let mut tts = self.tts.write().await;
        let audio_callback = self.tts_audio_callback.read().clone();
        let error_callback = self.tts_error_callback.read().clone();
        let complete_callback = self.tts_complete_callback.read().clone();

        let callback = Arc::new(VoiceManagerTTSCallback {
            audio_callback,
            error_callback,
            interruption_state: Some(self.interruption_state.clone()),
            complete_callback,
        });

        tts.on_audio(callback)
            .map_err(VoiceManagerError::TTSError)?;

        Ok(())
    }

    /// Get the current configuration
    ///
    /// # Returns
    /// * `&VoiceManagerConfig` - Current configuration
    pub fn get_config(&self) -> &VoiceManagerConfig {
        &self.config
    }

    /// Check if STT provider is ready
    ///
    /// # Returns
    /// * `bool` - True if STT provider is ready
    pub async fn is_stt_ready(&self) -> bool {
        let stt = self.stt.read().await;
        stt.is_ready()
    }

    /// Check if TTS provider is ready
    ///
    /// # Returns
    /// * `bool` - True if TTS provider is ready
    pub async fn is_tts_ready(&self) -> bool {
        let tts = self.tts.read().await;
        tts.is_ready()
    }

    /// Get STT provider information
    ///
    /// # Returns
    /// * `&'static str` - STT provider information
    pub async fn get_stt_provider_info(&self) -> &'static str {
        let stt = self.stt.read().await;
        stt.get_provider_info()
    }

    /// Get TTS provider information
    ///
    /// # Returns
    /// * `serde_json::Value` - TTS provider information
    pub async fn get_tts_provider_info(&self) -> serde_json::Value {
        let tts = self.tts.read().await;
        tts.get_provider_info()
    }

    /// Finalize the STT stream to signal end of audio input.
    ///
    /// This method disconnects and immediately reconnects the STT provider,
    /// which triggers the CloseStream message to be sent. For providers like
    /// Deepgram, this causes them to finalize any pending transcripts and
    /// send `speech_final=true`.
    ///
    /// # Returns
    /// * `VoiceManagerResult<()>` - Success or error
    ///
    /// # Example
    /// ```rust,no_run
    /// # use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
    /// # use waav_gateway::core::stt::STTConfig;
    /// # use waav_gateway::core::tts::TTSConfig;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = VoiceManagerConfig::new(STTConfig::default(), TTSConfig::default());
    /// # let voice_manager = VoiceManager::new(config, None)?;
    /// // After sending all audio...
    /// voice_manager.finalize_stt().await?;
    /// // Wait for final transcripts to arrive
    /// # Ok(())
    /// # }
    /// ```
    pub async fn finalize_stt(&self) -> VoiceManagerResult<()> {
        tracing::info!("Finalizing STT stream - sending CloseStream signal");

        // Disconnect STT to trigger CloseStream message
        // NOTE: The Deepgram implementation now waits for speech_final during disconnect,
        // so the final transcripts should arrive before this returns
        {
            let mut stt = self.stt.write().await;
            stt.disconnect()
                .await
                .map_err(VoiceManagerError::STTError)?;
        }

        // Small delay to ensure callbacks have processed the final results
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Reconnect STT for continued use
        {
            let mut stt = self.stt.write().await;
            stt.connect()
                .await
                .map_err(VoiceManagerError::STTError)?;
        }

        tracing::info!("STT stream finalized and reconnected");
        Ok(())
    }
}

// Ensure VoiceManager is thread-safe
unsafe impl Send for VoiceManager {}
unsafe impl Sync for VoiceManager {}

//! Observer trait and registry for voice processing events.
//!
//! This module implements the Observer pattern for non-intrusive monitoring
//! of STT/TTS events without modifying the core pipeline.

use crate::core::stt::STTResult;
use crate::core::tts::{AudioData, ConnectionState, TTSError};
use parking_lot::RwLock;
use std::sync::Arc;
use uuid::Uuid;

// =============================================================================
// VoiceObserver Trait
// =============================================================================

/// Trait for observing voice processing events.
///
/// All methods have default no-op implementations, so observers only need to
/// implement the events they care about.
///
/// # Thread Safety
///
/// Observers must be thread-safe (`Send + Sync`) as they may be called from
/// multiple threads concurrently.
///
/// # Performance
///
/// Observer methods are called synchronously in the hot path. Keep implementations
/// fast and non-blocking. For expensive operations, use channels to offload work.
pub trait VoiceObserver: Send + Sync {
    /// Called when an STT result is received.
    ///
    /// # Arguments
    /// * `result` - The transcription result
    /// * `latency_ns` - Time since audio segment started (nanoseconds)
    fn on_stt_result(&self, _result: &STTResult, _latency_ns: u64) {}

    /// Called when a TTS audio chunk is delivered.
    ///
    /// # Arguments
    /// * `chunk` - The audio data chunk
    /// * `ttfb_ns` - Time-to-first-byte if this is the first chunk (nanoseconds)
    fn on_tts_chunk(&self, _chunk: &AudioData, _ttfb_ns: Option<u64>) {}

    /// Called when TTS synthesis completes.
    ///
    /// # Arguments
    /// * `total_duration_ms` - Total duration of synthesized audio (milliseconds)
    fn on_tts_complete(&self, _total_duration_ms: u64) {}

    /// Called when a provider connection state changes.
    ///
    /// # Arguments
    /// * `provider` - Provider name (e.g., "deepgram", "elevenlabs")
    /// * `old_state` - Previous connection state
    /// * `new_state` - New connection state
    fn on_connection_state_change(
        &self,
        _provider: &str,
        _old_state: ConnectionState,
        _new_state: ConnectionState,
    ) {
    }

    /// Called when an error occurs.
    ///
    /// # Arguments
    /// * `provider` - Provider that experienced the error
    /// * `error` - The error that occurred
    fn on_error(&self, _provider: &str, _error: &TTSError) {}

    /// Called when bot starts speaking (first TTS chunk).
    ///
    /// # Arguments
    /// * `timestamp_ns` - Monotonic timestamp when speaking started
    fn on_bot_speaking_started(&self, _timestamp_ns: u64) {}

    /// Called when bot stops speaking (silence detected).
    ///
    /// # Arguments
    /// * `duration_ms` - How long the bot was speaking (milliseconds)
    fn on_bot_speaking_stopped(&self, _duration_ms: u64) {}

    /// Called when local VAD detects speech start.
    fn on_local_vad_speech_started(&self) {}

    /// Called when local VAD detects speech end.
    fn on_local_vad_speech_ended(&self) {}

    // -------------------------------------------------------------------------
    // Per-turn latency checkpoints (live profiling).
    //
    // These thread monotonic-ns timestamps (from `now_monotonic_ns()`) through
    // the realtime audio-to-audio path so a profiler can assemble a per-turn
    // timeline: audio_in -> smart_turn -> stt_partial -> stt_final (reuses
    // `on_stt_result`) -> llm_request -> llm_first_token -> llm_first_sentence ->
    // tts_request -> tts_first_audio (reuses `on_tts_chunk`) -> audio_out.
    // All default no-op so existing observers stay source-compatible.
    // -------------------------------------------------------------------------

    /// Last user audio frame ingested before end-of-speech (pre-turn anchor).
    fn on_audio_in(&self, _ts_ns: u64) {}

    /// A smart-turn / VAD inference completed on an audio frame.
    ///
    /// * `inference_us` - ONNX inference wall-clock for this frame (microseconds)
    /// * `is_complete` - whether the model declared the turn complete (end-of-turn)
    /// * `ts_ns` - monotonic timestamp when inference finished
    fn on_smart_turn(&self, _inference_us: u64, _is_complete: bool, _ts_ns: u64) {}

    /// A smart-turn audio frame was skipped because the inference lock was
    /// contended (ML still running on a prior frame). Backpressure signal.
    fn on_frame_skipped(&self) {}

    /// First interim (non-final) STT result of the current turn.
    fn on_stt_partial(&self, _ts_ns: u64) {}

    /// The LLM request was dispatched.
    fn on_llm_request(&self, _ts_ns: u64) {}

    /// The first LLM token (SSE delta) arrived (TTFT anchor).
    fn on_llm_first_token(&self, _ts_ns: u64) {}

    /// The first complete sentence was flushed toward TTS.
    fn on_llm_first_sentence(&self, _ts_ns: u64) {}

    /// A TTS synthesis request was issued for this turn.
    fn on_tts_request(&self, _ts_ns: u64) {}

    /// The first synthesized audio actually left the gateway (headline end).
    fn on_audio_out(&self, _ts_ns: u64) {}

    /// A per-audio-frame processing stage completed (the realtime "frame budget").
    ///
    /// * `stage` - a fixed `&'static str` label (`"receive"`, `"decode"`, `"vad"`,
    ///   `"smart_turn"`, `"stt_send"`)
    /// * `dur_ns` - wall-clock spent in that stage for this frame
    fn on_frame_stage(&self, _stage: &'static str, _dur_ns: u64) {}
}

// =============================================================================
// Observer Registry
// =============================================================================

/// Entry in the observer registry
struct ObserverEntry {
    id: Uuid,
    observer: Arc<dyn VoiceObserver>,
}

/// Registry for managing multiple observers.
///
/// The registry is thread-safe and uses `parking_lot::RwLock` for fast
/// read-heavy access patterns (notifications are reads, registrations are writes).
///
/// # Capacity
///
/// The registry pre-allocates space for 4 observers to avoid allocations
/// in common use cases. More observers can be added but may cause allocation.
///
/// # Example
///
/// ```ignore
/// let registry = ObserverRegistry::new();
///
/// // Register an observer
/// let id = registry.register(Arc::new(MyObserver));
///
/// // Notify all observers
/// registry.notify_stt_result(&result, latency_ns);
///
/// // Unregister when done
/// registry.unregister(id);
/// ```
pub struct ObserverRegistry {
    observers: RwLock<Vec<ObserverEntry>>,
}

impl ObserverRegistry {
    /// Create a new empty registry with pre-allocated capacity.
    pub fn new() -> Self {
        Self {
            observers: RwLock::new(Vec::with_capacity(4)),
        }
    }

    /// Register an observer and return its unique ID.
    ///
    /// The ID can be used later to unregister the observer.
    pub fn register(&self, observer: Arc<dyn VoiceObserver>) -> Uuid {
        let id = Uuid::new_v4();
        self.observers.write().push(ObserverEntry { id, observer });
        id
    }

    /// Unregister an observer by ID.
    ///
    /// Returns `true` if the observer was found and removed, `false` otherwise.
    pub fn unregister(&self, id: Uuid) -> bool {
        let mut observers = self.observers.write();
        if let Some(pos) = observers.iter().position(|e| e.id == id) {
            observers.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get the current number of registered observers.
    pub fn len(&self) -> usize {
        self.observers.read().len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.observers.read().is_empty()
    }

    /// Clear all registered observers.
    pub fn clear(&self) {
        self.observers.write().clear();
    }

    // =========================================================================
    // Notification Methods
    // =========================================================================

    /// Notify all observers of an STT result.
    #[inline]
    pub fn notify_stt_result(&self, result: &STTResult, latency_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_stt_result(result, latency_ns);
        }
    }

    /// Notify all observers of a TTS chunk.
    #[inline]
    pub fn notify_tts_chunk(&self, chunk: &AudioData, ttfb_ns: Option<u64>) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_tts_chunk(chunk, ttfb_ns);
        }
    }

    /// Notify all observers of TTS completion.
    #[inline]
    pub fn notify_tts_complete(&self, total_duration_ms: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_tts_complete(total_duration_ms);
        }
    }

    /// Notify all observers of a connection state change.
    #[inline]
    pub fn notify_connection_state_change(
        &self,
        provider: &str,
        old_state: ConnectionState,
        new_state: ConnectionState,
    ) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_connection_state_change(
                provider,
                old_state.clone(),
                new_state.clone(),
            );
        }
    }

    /// Notify all observers of an error.
    #[inline]
    pub fn notify_error(&self, provider: &str, error: &TTSError) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_error(provider, error);
        }
    }

    /// Notify all observers that bot started speaking.
    #[inline]
    pub fn notify_bot_speaking_started(&self, timestamp_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_bot_speaking_started(timestamp_ns);
        }
    }

    /// Notify all observers that bot stopped speaking.
    #[inline]
    pub fn notify_bot_speaking_stopped(&self, duration_ms: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_bot_speaking_stopped(duration_ms);
        }
    }

    /// Notify all observers of local VAD speech start.
    #[inline]
    pub fn notify_local_vad_speech_started(&self) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_local_vad_speech_started();
        }
    }

    /// Notify all observers of local VAD speech end.
    #[inline]
    pub fn notify_local_vad_speech_ended(&self) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_local_vad_speech_ended();
        }
    }

    // -------------------------------------------------------------------------
    // Per-turn / per-frame latency checkpoint notifications.
    //
    // Each is a read-lock acquire over a (usually empty) Vec + an empty-iterator
    // loop — ~1 atomic load when no observer is registered. The audio-hot-path
    // sites (`notify_audio_in`/`notify_frame_stage`/`notify_smart_turn`/
    // `notify_frame_skipped`) must be called BEFORE acquiring the STT write lock.
    // -------------------------------------------------------------------------

    /// Notify: last user audio frame before end-of-speech.
    #[inline]
    pub fn notify_audio_in(&self, ts_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_audio_in(ts_ns);
        }
    }

    /// Notify: a smart-turn inference completed on a frame.
    #[inline]
    pub fn notify_smart_turn(&self, inference_us: u64, is_complete: bool, ts_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_smart_turn(inference_us, is_complete, ts_ns);
        }
    }

    /// Notify: a smart-turn frame was skipped due to inference-lock contention.
    #[inline]
    pub fn notify_frame_skipped(&self) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_frame_skipped();
        }
    }

    /// Notify: first interim STT result of the turn.
    #[inline]
    pub fn notify_stt_partial(&self, ts_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_stt_partial(ts_ns);
        }
    }

    /// Notify: the LLM request was dispatched.
    #[inline]
    pub fn notify_llm_request(&self, ts_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_llm_request(ts_ns);
        }
    }

    /// Notify: the first LLM token arrived (TTFT).
    #[inline]
    pub fn notify_llm_first_token(&self, ts_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_llm_first_token(ts_ns);
        }
    }

    /// Notify: the first sentence was flushed toward TTS.
    #[inline]
    pub fn notify_llm_first_sentence(&self, ts_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_llm_first_sentence(ts_ns);
        }
    }

    /// Notify: a TTS synthesis request was issued.
    #[inline]
    pub fn notify_tts_request(&self, ts_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_tts_request(ts_ns);
        }
    }

    /// Notify: the first synthesized audio left the gateway (headline end).
    #[inline]
    pub fn notify_audio_out(&self, ts_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_audio_out(ts_ns);
        }
    }

    /// Notify: a per-frame processing stage completed (the realtime frame budget).
    #[inline]
    pub fn notify_frame_stage(&self, stage: &'static str, dur_ns: u64) {
        let observers = self.observers.read();
        for entry in observers.iter() {
            entry.observer.on_frame_stage(stage, dur_ns);
        }
    }
}

impl Default for ObserverRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // -------------------------------------------------------------------------
    // Test Fixtures
    // -------------------------------------------------------------------------

    struct TestObserver {
        stt_result_count: AtomicU64,
        tts_chunk_count: AtomicU64,
        tts_complete_count: AtomicU64,
        connection_change_count: AtomicU64,
        error_count: AtomicU64,
        bot_started_count: AtomicU64,
        bot_stopped_count: AtomicU64,
        vad_started_count: AtomicU64,
        vad_ended_count: AtomicU64,
        last_latency_ns: AtomicU64,
        last_ttfb_ns: AtomicU64,
    }

    impl TestObserver {
        fn new() -> Self {
            Self {
                stt_result_count: AtomicU64::new(0),
                tts_chunk_count: AtomicU64::new(0),
                tts_complete_count: AtomicU64::new(0),
                connection_change_count: AtomicU64::new(0),
                error_count: AtomicU64::new(0),
                bot_started_count: AtomicU64::new(0),
                bot_stopped_count: AtomicU64::new(0),
                vad_started_count: AtomicU64::new(0),
                vad_ended_count: AtomicU64::new(0),
                last_latency_ns: AtomicU64::new(0),
                last_ttfb_ns: AtomicU64::new(0),
            }
        }
    }

    impl VoiceObserver for TestObserver {
        fn on_stt_result(&self, _result: &STTResult, latency_ns: u64) {
            self.stt_result_count.fetch_add(1, Ordering::Relaxed);
            self.last_latency_ns.store(latency_ns, Ordering::Relaxed);
        }

        fn on_tts_chunk(&self, _chunk: &AudioData, ttfb_ns: Option<u64>) {
            self.tts_chunk_count.fetch_add(1, Ordering::Relaxed);
            if let Some(ttfb) = ttfb_ns {
                self.last_ttfb_ns.store(ttfb, Ordering::Relaxed);
            }
        }

        fn on_tts_complete(&self, _total_duration_ms: u64) {
            self.tts_complete_count.fetch_add(1, Ordering::Relaxed);
        }

        fn on_connection_state_change(
            &self,
            _provider: &str,
            _old_state: ConnectionState,
            _new_state: ConnectionState,
        ) {
            self.connection_change_count.fetch_add(1, Ordering::Relaxed);
        }

        fn on_error(&self, _provider: &str, _error: &TTSError) {
            self.error_count.fetch_add(1, Ordering::Relaxed);
        }

        fn on_bot_speaking_started(&self, _timestamp_ns: u64) {
            self.bot_started_count.fetch_add(1, Ordering::Relaxed);
        }

        fn on_bot_speaking_stopped(&self, _duration_ms: u64) {
            self.bot_stopped_count.fetch_add(1, Ordering::Relaxed);
        }

        fn on_local_vad_speech_started(&self) {
            self.vad_started_count.fetch_add(1, Ordering::Relaxed);
        }

        fn on_local_vad_speech_ended(&self) {
            self.vad_ended_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn create_test_stt_result() -> STTResult {
        STTResult::new("test transcript".to_string(), true, false, 0.95)
    }

    fn create_test_audio_data() -> AudioData {
        AudioData {
            data: vec![0u8; 480], // 10ms at 24kHz 16-bit
            sample_rate: 24000,
            format: "linear16".to_string(),
            duration_ms: Some(10),
        }
    }

    // -------------------------------------------------------------------------
    // Registry Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_registry_creation() {
        let registry = ObserverRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_registry_default() {
        let registry = ObserverRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_register_observer() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());

        let id = registry.register(observer);
        assert!(!id.is_nil());
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_register_multiple_observers() {
        let registry = ObserverRegistry::new();

        let ids: Vec<Uuid> = (0..5)
            .map(|_| registry.register(Arc::new(TestObserver::new())))
            .collect();

        assert_eq!(registry.len(), 5);

        // All IDs should be unique
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j]);
            }
        }
    }

    #[test]
    fn test_unregister_observer() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());

        let id = registry.register(observer);
        assert_eq!(registry.len(), 1);

        // Unregister should succeed
        assert!(registry.unregister(id));
        assert_eq!(registry.len(), 0);

        // Unregistering again should fail
        assert!(!registry.unregister(id));
    }

    #[test]
    fn test_unregister_nonexistent() {
        let registry = ObserverRegistry::new();
        let fake_id = Uuid::new_v4();
        assert!(!registry.unregister(fake_id));
    }

    #[test]
    fn test_clear_registry() {
        let registry = ObserverRegistry::new();

        for _ in 0..10 {
            registry.register(Arc::new(TestObserver::new()));
        }
        assert_eq!(registry.len(), 10);

        registry.clear();
        assert!(registry.is_empty());
    }

    // -------------------------------------------------------------------------
    // Notification Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_notify_stt_result() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        let result = create_test_stt_result();
        let latency_ns = 1_000_000; // 1ms

        registry.notify_stt_result(&result, latency_ns);

        assert_eq!(observer.stt_result_count.load(Ordering::Relaxed), 1);
        assert_eq!(observer.last_latency_ns.load(Ordering::Relaxed), latency_ns);
    }

    #[test]
    fn test_notify_tts_chunk_with_ttfb() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        let chunk = create_test_audio_data();
        let ttfb_ns = 500_000; // 0.5ms

        registry.notify_tts_chunk(&chunk, Some(ttfb_ns));

        assert_eq!(observer.tts_chunk_count.load(Ordering::Relaxed), 1);
        assert_eq!(observer.last_ttfb_ns.load(Ordering::Relaxed), ttfb_ns);
    }

    #[test]
    fn test_notify_tts_chunk_without_ttfb() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        let chunk = create_test_audio_data();

        registry.notify_tts_chunk(&chunk, None);

        assert_eq!(observer.tts_chunk_count.load(Ordering::Relaxed), 1);
        assert_eq!(observer.last_ttfb_ns.load(Ordering::Relaxed), 0); // Not updated
    }

    #[test]
    fn test_notify_tts_complete() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        registry.notify_tts_complete(5000);

        assert_eq!(observer.tts_complete_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_notify_connection_state_change() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        registry.notify_connection_state_change(
            "deepgram",
            ConnectionState::Disconnected,
            ConnectionState::Connected,
        );

        assert_eq!(observer.connection_change_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_notify_error() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        let error = TTSError::NetworkError("connection lost".to_string());
        registry.notify_error("elevenlabs", &error);

        assert_eq!(observer.error_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_notify_bot_speaking_started() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        registry.notify_bot_speaking_started(1_000_000_000);

        assert_eq!(observer.bot_started_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_notify_bot_speaking_stopped() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        registry.notify_bot_speaking_stopped(2500);

        assert_eq!(observer.bot_stopped_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_notify_vad_events() {
        let registry = ObserverRegistry::new();
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        registry.notify_local_vad_speech_started();
        registry.notify_local_vad_speech_ended();

        assert_eq!(observer.vad_started_count.load(Ordering::Relaxed), 1);
        assert_eq!(observer.vad_ended_count.load(Ordering::Relaxed), 1);
    }

    // -------------------------------------------------------------------------
    // Multiple Observer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_multiple_observers_receive_notifications() {
        let registry = ObserverRegistry::new();

        let observers: Vec<Arc<TestObserver>> =
            (0..5).map(|_| Arc::new(TestObserver::new())).collect();

        for obs in &observers {
            registry.register(obs.clone());
        }

        let result = create_test_stt_result();
        registry.notify_stt_result(&result, 1000);

        // All observers should have received the notification
        for obs in &observers {
            assert_eq!(obs.stt_result_count.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn test_unregistered_observer_stops_receiving() {
        let registry = ObserverRegistry::new();
        let observer1 = Arc::new(TestObserver::new());
        let observer2 = Arc::new(TestObserver::new());

        let id1 = registry.register(observer1.clone());
        registry.register(observer2.clone());

        let result = create_test_stt_result();

        // Both receive first notification
        registry.notify_stt_result(&result, 1000);
        assert_eq!(observer1.stt_result_count.load(Ordering::Relaxed), 1);
        assert_eq!(observer2.stt_result_count.load(Ordering::Relaxed), 1);

        // Unregister observer1
        registry.unregister(id1);

        // Only observer2 receives second notification
        registry.notify_stt_result(&result, 2000);
        assert_eq!(observer1.stt_result_count.load(Ordering::Relaxed), 1);
        assert_eq!(observer2.stt_result_count.load(Ordering::Relaxed), 2);
    }

    // -------------------------------------------------------------------------
    // Thread Safety Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_concurrent_registration() {
        use std::thread;

        let registry = Arc::new(ObserverRegistry::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let reg = registry.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let observer = Arc::new(TestObserver::new());
                    let id = reg.register(observer);
                    reg.unregister(id);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Registry should be empty after all registrations/unregistrations
        assert!(registry.is_empty());
    }

    #[test]
    fn test_concurrent_notifications() {
        use std::thread;

        let registry = Arc::new(ObserverRegistry::new());
        let observer = Arc::new(TestObserver::new());
        registry.register(observer.clone());

        let mut handles = vec![];

        for _ in 0..10 {
            let reg = registry.clone();
            handles.push(thread::spawn(move || {
                let result = create_test_stt_result();
                for _ in 0..100 {
                    reg.notify_stt_result(&result, 1000);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should have received all 1000 notifications
        assert_eq!(observer.stt_result_count.load(Ordering::Relaxed), 1000);
    }

    // -------------------------------------------------------------------------
    // Default Trait Implementation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_trait_methods_are_noop() {
        struct EmptyObserver;
        impl VoiceObserver for EmptyObserver {}

        let observer = EmptyObserver;

        // These should all be no-ops and not panic
        observer.on_stt_result(&create_test_stt_result(), 1000);
        observer.on_tts_chunk(&create_test_audio_data(), Some(500));
        observer.on_tts_complete(1000);
        observer.on_connection_state_change(
            "test",
            ConnectionState::Disconnected,
            ConnectionState::Connected,
        );
        observer.on_error("test", &TTSError::NetworkError("test".to_string()));
        observer.on_bot_speaking_started(1000);
        observer.on_bot_speaking_stopped(1000);
        observer.on_local_vad_speech_started();
        observer.on_local_vad_speech_ended();
    }

    // -------------------------------------------------------------------------
    // Edge Case Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_notify_empty_registry() {
        let registry = ObserverRegistry::new();

        // Should not panic when notifying empty registry
        registry.notify_stt_result(&create_test_stt_result(), 1000);
        registry.notify_tts_chunk(&create_test_audio_data(), Some(500));
        registry.notify_tts_complete(1000);
        registry.notify_bot_speaking_started(1000);
        registry.notify_bot_speaking_stopped(1000);
    }

    #[test]
    fn test_observer_drop_while_registered() {
        let registry = ObserverRegistry::new();

        {
            let observer = Arc::new(TestObserver::new());
            registry.register(observer.clone());
            // observer goes out of scope but Arc keeps it alive in registry
        }

        // Should still work since registry holds Arc
        let result = create_test_stt_result();
        registry.notify_stt_result(&result, 1000);

        assert_eq!(registry.len(), 1);
    }

    // -------------------------------------------------------------------------
    // Performance Characteristic Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_notification_with_many_observers() {
        let registry = ObserverRegistry::new();
        let observer_count = 100;

        let observers: Vec<Arc<TestObserver>> = (0..observer_count)
            .map(|_| Arc::new(TestObserver::new()))
            .collect();

        for obs in &observers {
            registry.register(obs.clone());
        }

        assert_eq!(registry.len(), observer_count);

        // Notify all observers
        let result = create_test_stt_result();
        for _ in 0..100 {
            registry.notify_stt_result(&result, 1000);
        }

        // Verify all received 100 notifications
        for obs in &observers {
            assert_eq!(obs.stt_result_count.load(Ordering::Relaxed), 100);
        }
    }

    // -------------------------------------------------------------------------
    // Per-turn / per-frame checkpoint dispatch (live profiling).
    // -------------------------------------------------------------------------

    /// Observer that counts every per-turn / per-frame checkpoint hook.
    #[derive(Default)]
    struct CheckpointObserver {
        audio_in: AtomicU64,
        smart_turn: AtomicU64,
        smart_turn_complete: AtomicU64,
        frame_skipped: AtomicU64,
        stt_partial: AtomicU64,
        llm_request: AtomicU64,
        llm_first_token: AtomicU64,
        llm_first_sentence: AtomicU64,
        tts_request: AtomicU64,
        audio_out: AtomicU64,
        frame_stage: AtomicU64,
        last_inference_us: AtomicU64,
        last_frame_stage: parking_lot::Mutex<&'static str>,
    }

    impl VoiceObserver for CheckpointObserver {
        fn on_audio_in(&self, _ts_ns: u64) {
            self.audio_in.fetch_add(1, Ordering::Relaxed);
        }
        fn on_smart_turn(&self, inference_us: u64, is_complete: bool, _ts_ns: u64) {
            self.smart_turn.fetch_add(1, Ordering::Relaxed);
            self.last_inference_us.store(inference_us, Ordering::Relaxed);
            if is_complete {
                self.smart_turn_complete.fetch_add(1, Ordering::Relaxed);
            }
        }
        fn on_frame_skipped(&self) {
            self.frame_skipped.fetch_add(1, Ordering::Relaxed);
        }
        fn on_stt_partial(&self, _ts_ns: u64) {
            self.stt_partial.fetch_add(1, Ordering::Relaxed);
        }
        fn on_llm_request(&self, _ts_ns: u64) {
            self.llm_request.fetch_add(1, Ordering::Relaxed);
        }
        fn on_llm_first_token(&self, _ts_ns: u64) {
            self.llm_first_token.fetch_add(1, Ordering::Relaxed);
        }
        fn on_llm_first_sentence(&self, _ts_ns: u64) {
            self.llm_first_sentence.fetch_add(1, Ordering::Relaxed);
        }
        fn on_tts_request(&self, _ts_ns: u64) {
            self.tts_request.fetch_add(1, Ordering::Relaxed);
        }
        fn on_audio_out(&self, _ts_ns: u64) {
            self.audio_out.fetch_add(1, Ordering::Relaxed);
        }
        fn on_frame_stage(&self, stage: &'static str, _dur_ns: u64) {
            self.frame_stage.fetch_add(1, Ordering::Relaxed);
            *self.last_frame_stage.lock() = stage;
        }
    }

    #[test]
    fn test_checkpoint_hooks_each_fire_once() {
        let registry = ObserverRegistry::new();
        let obs = Arc::new(CheckpointObserver::default());
        registry.register(obs.clone());

        registry.notify_audio_in(1);
        registry.notify_smart_turn(18_000, true, 2);
        registry.notify_frame_skipped();
        registry.notify_stt_partial(3);
        registry.notify_llm_request(4);
        registry.notify_llm_first_token(5);
        registry.notify_llm_first_sentence(6);
        registry.notify_tts_request(7);
        registry.notify_audio_out(8);
        registry.notify_frame_stage("smart_turn", 9);

        assert_eq!(obs.audio_in.load(Ordering::Relaxed), 1);
        assert_eq!(obs.smart_turn.load(Ordering::Relaxed), 1);
        assert_eq!(obs.smart_turn_complete.load(Ordering::Relaxed), 1);
        assert_eq!(obs.last_inference_us.load(Ordering::Relaxed), 18_000);
        assert_eq!(obs.frame_skipped.load(Ordering::Relaxed), 1);
        assert_eq!(obs.stt_partial.load(Ordering::Relaxed), 1);
        assert_eq!(obs.llm_request.load(Ordering::Relaxed), 1);
        assert_eq!(obs.llm_first_token.load(Ordering::Relaxed), 1);
        assert_eq!(obs.llm_first_sentence.load(Ordering::Relaxed), 1);
        assert_eq!(obs.tts_request.load(Ordering::Relaxed), 1);
        assert_eq!(obs.audio_out.load(Ordering::Relaxed), 1);
        assert_eq!(obs.frame_stage.load(Ordering::Relaxed), 1);
        assert_eq!(*obs.last_frame_stage.lock(), "smart_turn");
    }

    #[test]
    fn test_checkpoint_notify_empty_registry_is_noop() {
        // No observers registered: the new notify_* methods must be safe no-ops
        // (this is the zero-cost hot-path guarantee).
        let registry = ObserverRegistry::new();
        registry.notify_audio_in(1);
        registry.notify_smart_turn(1, false, 1);
        registry.notify_frame_skipped();
        registry.notify_stt_partial(1);
        registry.notify_llm_request(1);
        registry.notify_llm_first_token(1);
        registry.notify_llm_first_sentence(1);
        registry.notify_tts_request(1);
        registry.notify_audio_out(1);
        registry.notify_frame_stage("receive", 1);
        assert!(registry.is_empty());
    }
}

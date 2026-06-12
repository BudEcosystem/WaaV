//! Observer fan-out isolation (PIPECAT_FIX_PLAN D-G5).
//!
//! `ObserverRegistry::notify_*` dispatches SYNCHRONOUSLY on the hot path
//! (audio frames, STT results, TTS chunks). The built-in profilers are
//! cheap by design and STAY inline — but any heavier registered observer
//! (a logging sink, an SSE fan-out, a metrics exporter doing I/O) would add
//! its latency directly to the voice path.
//!
//! [`AsyncObserver`] is the second tier (Pipecat `WorkerObserver`): it
//! wraps an observer behind a bounded channel + drain task. The hot path
//! only constructs an owned event and `try_send`s it — when the queue is
//! full the event is DROPPED and counted, never blocked on. Async
//! observers are therefore eventually-consistent and may miss events under
//! sustained backpressure; observers needing exact ordering or zero loss
//! (the profilers) stay on the sync tier.
//!
//! Cost note: owned events mean the heavy payloads (STT result text, TTS
//! chunk bytes) are CLONED on the hot path for async-tier observers. That
//! is a bounded memcpy (µs), not unbounded consumer latency — the invariant
//! is "never blocks", deliberately not "never copies".

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;
use tracing::warn;

use super::observer::VoiceObserver;
use crate::core::stt::STTResult;
use crate::core::tts::{AudioData, ConnectionState, TTSError};

/// Default queue depth for an async observer.
pub const DEFAULT_ASYNC_OBSERVER_QUEUE: usize = 256;

/// One observable event, owned (queueable).
enum ObserverEvent {
    SttResult(STTResult, u64),
    TtsChunk(AudioData, Option<u64>),
    TtsComplete(u64),
    ConnectionStateChange { provider: String, from: ConnectionState, to: ConnectionState },
    Error { provider: String, error: TTSError },
    BotSpeakingStarted(u64),
    BotSpeakingStopped(u64),
    LocalVadSpeechStarted,
    LocalVadSpeechEnded,
    AudioIn(u64),
    SmartTurn { inference_us: u64, is_complete: bool, ts_ns: u64 },
    FrameSkipped,
    SttPartial(u64),
    LlmRequest(u64),
    LlmFirstToken(u64),
    LlmFirstSentence(u64),
    TtsRequest(u64),
    AudioOut(u64),
    FrameStage(&'static str, u64),
}

/// The async-tier wrapper. Implements [`VoiceObserver`] itself, so it
/// registers like any observer; every hook is a non-blocking `try_send`.
pub struct AsyncObserver {
    tx: mpsc::Sender<ObserverEvent>,
    dropped: Arc<AtomicU64>,
    label: &'static str,
}

impl AsyncObserver {
    /// Wrap `inner` behind a bounded queue + drain task. Must be called in
    /// a tokio runtime context (session setup is async).
    pub fn new(inner: Arc<dyn VoiceObserver>, queue: usize) -> Self {
        Self::with_label(inner, queue, "async-observer")
    }

    pub fn with_label(
        inner: Arc<dyn VoiceObserver>,
        queue: usize,
        label: &'static str,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel::<ObserverEvent>(queue.max(1));
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                dispatch(&*inner, event);
            }
        });
        Self { tx, dropped: Arc::new(AtomicU64::new(0)), label }
    }

    /// Events dropped because the queue was full (observability/tests).
    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    #[inline]
    fn enqueue(&self, event: ObserverEvent) {
        if self.tx.try_send(event).is_err() {
            // Full or closed: DROP, count, never block — backpressure must
            // not reach the voice hot path. Warn once per 1000 to avoid
            // flooding at frame rate.
            let n = self.dropped.fetch_add(1, Ordering::Relaxed);
            if n.is_multiple_of(1000) {
                warn!(
                    observer = self.label,
                    total_dropped = n + 1,
                    "async observer queue full; dropping events"
                );
            }
        }
    }
}

fn dispatch(observer: &dyn VoiceObserver, event: ObserverEvent) {
    match event {
        ObserverEvent::SttResult(r, latency_ns) => observer.on_stt_result(&r, latency_ns),
        ObserverEvent::TtsChunk(c, ttfb_ns) => observer.on_tts_chunk(&c, ttfb_ns),
        ObserverEvent::TtsComplete(d) => observer.on_tts_complete(d),
        ObserverEvent::ConnectionStateChange { provider, from, to } => {
            observer.on_connection_state_change(&provider, from, to)
        }
        ObserverEvent::Error { provider, error } => observer.on_error(&provider, &error),
        ObserverEvent::BotSpeakingStarted(ts) => observer.on_bot_speaking_started(ts),
        ObserverEvent::BotSpeakingStopped(d) => observer.on_bot_speaking_stopped(d),
        ObserverEvent::LocalVadSpeechStarted => observer.on_local_vad_speech_started(),
        ObserverEvent::LocalVadSpeechEnded => observer.on_local_vad_speech_ended(),
        ObserverEvent::AudioIn(ts) => observer.on_audio_in(ts),
        ObserverEvent::SmartTurn { inference_us, is_complete, ts_ns } => {
            observer.on_smart_turn(inference_us, is_complete, ts_ns)
        }
        ObserverEvent::FrameSkipped => observer.on_frame_skipped(),
        ObserverEvent::SttPartial(ts) => observer.on_stt_partial(ts),
        ObserverEvent::LlmRequest(ts) => observer.on_llm_request(ts),
        ObserverEvent::LlmFirstToken(ts) => observer.on_llm_first_token(ts),
        ObserverEvent::LlmFirstSentence(ts) => observer.on_llm_first_sentence(ts),
        ObserverEvent::TtsRequest(ts) => observer.on_tts_request(ts),
        ObserverEvent::AudioOut(ts) => observer.on_audio_out(ts),
        ObserverEvent::FrameStage(stage, dur) => observer.on_frame_stage(stage, dur),
    }
}

impl VoiceObserver for AsyncObserver {
    fn on_stt_result(&self, result: &STTResult, latency_ns: u64) {
        self.enqueue(ObserverEvent::SttResult(result.clone(), latency_ns));
    }
    fn on_tts_chunk(&self, chunk: &AudioData, ttfb_ns: Option<u64>) {
        self.enqueue(ObserverEvent::TtsChunk(chunk.clone(), ttfb_ns));
    }
    fn on_tts_complete(&self, total_duration_ms: u64) {
        self.enqueue(ObserverEvent::TtsComplete(total_duration_ms));
    }
    fn on_connection_state_change(
        &self,
        provider: &str,
        old_state: ConnectionState,
        new_state: ConnectionState,
    ) {
        self.enqueue(ObserverEvent::ConnectionStateChange {
            provider: provider.to_string(),
            from: old_state,
            to: new_state,
        });
    }
    fn on_error(&self, provider: &str, error: &TTSError) {
        self.enqueue(ObserverEvent::Error {
            provider: provider.to_string(),
            error: error.clone(),
        });
    }
    fn on_bot_speaking_started(&self, timestamp_ns: u64) {
        self.enqueue(ObserverEvent::BotSpeakingStarted(timestamp_ns));
    }
    fn on_bot_speaking_stopped(&self, duration_ms: u64) {
        self.enqueue(ObserverEvent::BotSpeakingStopped(duration_ms));
    }
    fn on_local_vad_speech_started(&self) {
        self.enqueue(ObserverEvent::LocalVadSpeechStarted);
    }
    fn on_local_vad_speech_ended(&self) {
        self.enqueue(ObserverEvent::LocalVadSpeechEnded);
    }
    fn on_audio_in(&self, ts_ns: u64) {
        self.enqueue(ObserverEvent::AudioIn(ts_ns));
    }
    fn on_smart_turn(&self, inference_us: u64, is_complete: bool, ts_ns: u64) {
        self.enqueue(ObserverEvent::SmartTurn { inference_us, is_complete, ts_ns });
    }
    fn on_frame_skipped(&self) {
        self.enqueue(ObserverEvent::FrameSkipped);
    }
    fn on_stt_partial(&self, ts_ns: u64) {
        self.enqueue(ObserverEvent::SttPartial(ts_ns));
    }
    fn on_llm_request(&self, ts_ns: u64) {
        self.enqueue(ObserverEvent::LlmRequest(ts_ns));
    }
    fn on_llm_first_token(&self, ts_ns: u64) {
        self.enqueue(ObserverEvent::LlmFirstToken(ts_ns));
    }
    fn on_llm_first_sentence(&self, ts_ns: u64) {
        self.enqueue(ObserverEvent::LlmFirstSentence(ts_ns));
    }
    fn on_tts_request(&self, ts_ns: u64) {
        self.enqueue(ObserverEvent::TtsRequest(ts_ns));
    }
    fn on_audio_out(&self, ts_ns: u64) {
        self.enqueue(ObserverEvent::AudioOut(ts_ns));
    }
    fn on_frame_stage(&self, stage: &'static str, dur_ns: u64) {
        self.enqueue(ObserverEvent::FrameStage(stage, dur_ns));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    /// Deliberately slow consumer (5ms per event).
    struct SlowObserver {
        seen: Arc<AtomicUsize>,
    }
    impl VoiceObserver for SlowObserver {
        fn on_frame_stage(&self, _stage: &'static str, _dur_ns: u64) {
            std::thread::sleep(Duration::from_millis(5));
            self.seen.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_observer_does_not_block_hot_path() {
        let seen = Arc::new(AtomicUsize::new(0));
        let slow = Arc::new(SlowObserver { seen: Arc::clone(&seen) });
        let wrapped = Arc::new(AsyncObserver::new(slow, 64));

        let registry = super::super::ObserverRegistry::new();
        registry.register(wrapped.clone());

        // 100k notifications against a consumer that takes 5ms each
        // (= 8+ minutes of consumer work). The hot path must finish in
        // well under a second — enqueue-or-drop only, never blocked.
        let start = Instant::now();
        for i in 0..100_000u64 {
            registry.notify_frame_stage("receive", i);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "hot path must not block on a slow async observer: {elapsed:?} for 100k events"
        );
        assert!(
            wrapped.dropped_events() > 90_000,
            "most events drop against a 5ms consumer: {}",
            wrapped.dropped_events()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_queue_drops_and_counts_never_blocks() {
        struct Blocked;
        impl VoiceObserver for Blocked {
            fn on_frame_stage(&self, _s: &'static str, _d: u64) {
                std::thread::sleep(Duration::from_secs(2));
            }
        }
        let wrapped = AsyncObserver::new(Arc::new(Blocked), 4);
        let start = Instant::now();
        for i in 0..100u64 {
            wrapped.on_frame_stage("receive", i);
        }
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "100 sends against a wedged consumer must return immediately"
        );
        // 4 queued (+1 possibly in-flight at the consumer): ≥95 dropped.
        assert!(
            wrapped.dropped_events() >= 95,
            "drops counted: {}",
            wrapped.dropped_events()
        );
    }

    #[tokio::test]
    async fn sync_observer_still_inline() {
        // Regression: a SYNC-registered observer sees the event before
        // notify_* returns (the profilers' exact-ordering contract).
        struct Counting(Arc<AtomicUsize>);
        impl VoiceObserver for Counting {
            fn on_frame_stage(&self, _s: &'static str, _d: u64) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        let registry = super::super::ObserverRegistry::new();
        registry.register(Arc::new(Counting(Arc::clone(&count))));
        registry.notify_frame_stage("receive", 1);
        assert_eq!(count.load(Ordering::SeqCst), 1, "sync tier is synchronous");
    }
}

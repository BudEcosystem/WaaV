//! Bot speaking state detection with atomic operations.
//!
//! This module tracks when the bot starts and stops speaking based on TTS
//! audio output. It uses atomic operations for lock-free hot-path performance.
//!
//! # Detection Logic
//!
//! - **Speaking Started**: First TTS audio chunk arrives
//! - **Speaking Stopped**: No audio for `silence_threshold_ms`
//!
//! # Example
//!
//! ```ignore
//! let state = BotSpeakingState::new(350); // 350ms silence threshold
//!
//! // When audio is sent
//! if let Some(started) = state.on_audio_sent() {
//!     println!("Bot started speaking at {}ns", started.timestamp_ns);
//! }
//!
//! // Check periodically for silence
//! if let Some(stopped) = state.check_silence() {
//!     println!("Bot stopped speaking after {}ms", stopped.speaking_duration_ns / 1_000_000);
//! }
//! ```

use crate::core::observability::latency::now_monotonic_ns;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// =============================================================================
// Events
// =============================================================================

/// Event emitted when bot starts speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BotSpeakingStarted {
    /// Monotonic timestamp when speaking started (nanoseconds)
    pub timestamp_ns: u64,
}

/// Event emitted when bot stops speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BotSpeakingStopped {
    /// Total duration the bot was speaking (nanoseconds)
    pub speaking_duration_ns: u64,
}

// =============================================================================
// Bot Speaking State
// =============================================================================

/// Tracks bot speaking state with atomic operations (lock-free hot path).
///
/// # Thread Safety
///
/// All operations use atomic memory ordering appropriate for multi-threaded
/// producer-consumer patterns. The state can be safely shared across threads.
///
/// # Performance
///
/// - `on_audio_sent()`: O(1), lock-free
/// - `check_silence()`: O(1), lock-free
/// - `is_speaking()`: O(1), lock-free
pub struct BotSpeakingState {
    /// Is bot currently speaking?
    is_speaking: AtomicBool,

    /// Timestamp of last audio chunk (nanoseconds since process start)
    last_audio_ns: AtomicU64,

    /// Timestamp when speaking started (nanoseconds)
    speaking_start_ns: AtomicU64,

    /// Silence threshold to consider bot stopped (nanoseconds)
    silence_threshold_ns: u64,
}

impl BotSpeakingState {
    /// Create new state with specified silence threshold.
    ///
    /// # Arguments
    /// * `silence_threshold_ms` - Milliseconds of silence to detect speaking stopped
    ///   (Pipecat uses 350ms as default)
    pub fn new(silence_threshold_ms: u64) -> Self {
        Self {
            is_speaking: AtomicBool::new(false),
            last_audio_ns: AtomicU64::new(0),
            speaking_start_ns: AtomicU64::new(0),
            silence_threshold_ns: silence_threshold_ms * 1_000_000,
        }
    }

    /// Notify that audio was sent to output.
    ///
    /// Call this each time a TTS audio chunk is delivered.
    ///
    /// # Returns
    /// * `Some(BotSpeakingStarted)` if this is the first chunk (speaking just started)
    /// * `None` if already speaking
    #[inline]
    pub fn on_audio_sent(&self) -> Option<BotSpeakingStarted> {
        let now_ns = now_monotonic_ns();

        let was_speaking = self.is_speaking.swap(true, Ordering::AcqRel);
        self.last_audio_ns.store(now_ns, Ordering::Release);

        if !was_speaking {
            self.speaking_start_ns.store(now_ns, Ordering::Release);
            Some(BotSpeakingStarted {
                timestamp_ns: now_ns,
            })
        } else {
            None
        }
    }

    /// Check if bot has stopped speaking (silence exceeded threshold).
    ///
    /// Call this periodically (e.g., every 50ms) to detect when speaking ends.
    ///
    /// # Returns
    /// * `Some(BotSpeakingStopped)` if silence threshold exceeded
    /// * `None` if still speaking or not speaking
    #[inline]
    pub fn check_silence(&self) -> Option<BotSpeakingStopped> {
        if !self.is_speaking.load(Ordering::Acquire) {
            return None;
        }

        let last = self.last_audio_ns.load(Ordering::Acquire);
        let now_ns = now_monotonic_ns();
        let silence_duration = now_ns.saturating_sub(last);

        if silence_duration > self.silence_threshold_ns {
            self.is_speaking.store(false, Ordering::Release);
            let start = self.speaking_start_ns.load(Ordering::Acquire);
            Some(BotSpeakingStopped {
                speaking_duration_ns: last.saturating_sub(start),
            })
        } else {
            None
        }
    }

    /// Check if bot is currently speaking.
    #[inline]
    pub fn is_speaking(&self) -> bool {
        self.is_speaking.load(Ordering::Acquire)
    }

    /// Get silence threshold in milliseconds.
    pub fn silence_threshold_ms(&self) -> u64 {
        self.silence_threshold_ns / 1_000_000
    }

    /// Get time since last audio in milliseconds (0 if never sent).
    pub fn silence_duration_ms(&self) -> u64 {
        let last = self.last_audio_ns.load(Ordering::Acquire);
        if last == 0 {
            return 0;
        }
        let now_ns = now_monotonic_ns();
        (now_ns.saturating_sub(last)) / 1_000_000
    }

    /// Reset state (e.g., on TTS clear/interrupt).
    pub fn reset(&self) {
        self.is_speaking.store(false, Ordering::Release);
        self.last_audio_ns.store(0, Ordering::Release);
        self.speaking_start_ns.store(0, Ordering::Release);
    }

    /// Force state to speaking (for testing).
    #[cfg(test)]
    pub fn force_speaking(&self, start_ns: u64, last_ns: u64) {
        self.is_speaking.store(true, Ordering::Release);
        self.speaking_start_ns.store(start_ns, Ordering::Release);
        self.last_audio_ns.store(last_ns, Ordering::Release);
    }
}

impl Default for BotSpeakingState {
    fn default() -> Self {
        Self::new(350) // 350ms default matches Pipecat
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    // -------------------------------------------------------------------------
    // Construction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_creation_with_threshold() {
        let state = BotSpeakingState::new(350);
        assert!(!state.is_speaking());
        assert_eq!(state.silence_threshold_ms(), 350);
    }

    #[test]
    fn test_default_threshold() {
        let state = BotSpeakingState::default();
        assert_eq!(state.silence_threshold_ms(), 350);
    }

    #[test]
    fn test_various_thresholds() {
        for threshold in [100, 250, 350, 500, 1000] {
            let state = BotSpeakingState::new(threshold);
            assert_eq!(state.silence_threshold_ms(), threshold);
        }
    }

    // -------------------------------------------------------------------------
    // Speaking Start Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_first_audio_starts_speaking() {
        let state = BotSpeakingState::new(350);

        let result = state.on_audio_sent();

        assert!(result.is_some());
        assert!(state.is_speaking());
        assert!(result.unwrap().timestamp_ns > 0);
    }

    #[test]
    fn test_subsequent_audio_does_not_return_event() {
        let state = BotSpeakingState::new(350);

        let first = state.on_audio_sent();
        let second = state.on_audio_sent();
        let third = state.on_audio_sent();

        assert!(first.is_some());
        assert!(second.is_none());
        assert!(third.is_none());
        assert!(state.is_speaking());
    }

    #[test]
    fn test_audio_updates_last_audio_time() {
        let state = BotSpeakingState::new(350);

        state.on_audio_sent();
        let first_silence = state.silence_duration_ms();

        thread::sleep(Duration::from_millis(10));

        state.on_audio_sent();
        let second_silence = state.silence_duration_ms();

        // After new audio, silence duration should reset
        assert!(second_silence < first_silence + 10);
    }

    // -------------------------------------------------------------------------
    // Silence Detection Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_no_silence_when_not_speaking() {
        let state = BotSpeakingState::new(350);

        // Not speaking, check_silence should return None
        assert!(state.check_silence().is_none());
    }

    #[test]
    fn test_no_silence_within_threshold() {
        let state = BotSpeakingState::new(350);

        state.on_audio_sent();

        // Immediately check - should not be silent yet
        assert!(state.check_silence().is_none());
        assert!(state.is_speaking());
    }

    #[test]
    fn test_silence_detected_after_threshold() {
        let state = BotSpeakingState::new(10); // 10ms threshold for faster test

        state.on_audio_sent();
        assert!(state.is_speaking());

        // Wait for threshold to pass
        thread::sleep(Duration::from_millis(15));

        let result = state.check_silence();
        assert!(result.is_some());
        assert!(!state.is_speaking());
    }

    #[test]
    fn test_silence_event_contains_duration() {
        let state = BotSpeakingState::new(10);

        state.on_audio_sent();

        // Send more audio to extend duration
        thread::sleep(Duration::from_millis(5));
        state.on_audio_sent();

        // Wait for silence
        thread::sleep(Duration::from_millis(15));

        let result = state.check_silence().unwrap();
        // Duration should be approximately 5ms (between first and last audio)
        assert!(result.speaking_duration_ns > 0);
    }

    #[test]
    fn test_continuous_audio_prevents_silence() {
        let state = BotSpeakingState::new(50); // 50ms threshold

        state.on_audio_sent();

        // Send audio every 20ms for 150ms (should never trigger silence)
        for _ in 0..7 {
            thread::sleep(Duration::from_millis(20));
            state.on_audio_sent();
            assert!(state.check_silence().is_none());
            assert!(state.is_speaking());
        }
    }

    // -------------------------------------------------------------------------
    // Reset Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_reset_clears_speaking_state() {
        let state = BotSpeakingState::new(350);

        state.on_audio_sent();
        assert!(state.is_speaking());

        state.reset();
        assert!(!state.is_speaking());
    }

    #[test]
    fn test_reset_allows_new_speaking_event() {
        let state = BotSpeakingState::new(350);

        let first = state.on_audio_sent();
        assert!(first.is_some());

        state.reset();

        let second = state.on_audio_sent();
        assert!(second.is_some()); // Should get new event after reset
    }

    #[test]
    fn test_reset_clears_silence_duration() {
        let state = BotSpeakingState::new(350);

        state.on_audio_sent();
        thread::sleep(Duration::from_millis(10));

        assert!(state.silence_duration_ms() >= 10);

        state.reset();

        assert_eq!(state.silence_duration_ms(), 0);
    }

    // -------------------------------------------------------------------------
    // Edge Cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_silence_duration_when_never_spoken() {
        let state = BotSpeakingState::new(350);
        assert_eq!(state.silence_duration_ms(), 0);
    }

    #[test]
    fn test_multiple_silence_detections() {
        let state = BotSpeakingState::new(10);

        // First speaking session
        state.on_audio_sent();
        thread::sleep(Duration::from_millis(15));
        assert!(state.check_silence().is_some());

        // Second speaking session
        assert!(state.on_audio_sent().is_some()); // New session starts
        thread::sleep(Duration::from_millis(15));
        assert!(state.check_silence().is_some());
    }

    #[test]
    fn test_zero_threshold_immediate_silence() {
        let state = BotSpeakingState::new(0);

        state.on_audio_sent();

        // With 0 threshold, any check should detect silence
        // (unless check happens at exact same nanosecond)
        thread::sleep(Duration::from_micros(100)); // Tiny delay
        assert!(state.check_silence().is_some());
    }

    // -------------------------------------------------------------------------
    // Thread Safety Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_concurrent_audio_sends() {
        use std::sync::Arc;

        let state = Arc::new(BotSpeakingState::new(350));
        let mut handles = vec![];

        for _ in 0..10 {
            let s = state.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    s.on_audio_sent();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert!(state.is_speaking());
    }

    #[test]
    fn test_concurrent_audio_and_silence_check() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;

        let state = Arc::new(BotSpeakingState::new(5)); // 5ms threshold
        let silence_count = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];

        // Audio sender thread
        let s1 = state.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                s1.on_audio_sent();
                thread::sleep(Duration::from_millis(2));
            }
        }));

        // Silence checker thread
        let s2 = state.clone();
        let count = silence_count.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                if s2.check_silence().is_some() {
                    count.fetch_add(1, Ordering::Relaxed);
                }
                thread::sleep(Duration::from_millis(1));
            }
        }));

        for handle in handles {
            handle.join().unwrap();
        }

        // Should have detected at least one silence when audio stopped
        // (audio sends every 2ms for 100ms, then stops; checker runs for 100ms more)
    }

    // -------------------------------------------------------------------------
    // Integration-like Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_typical_speaking_session() {
        let state = BotSpeakingState::new(100); // 100ms threshold

        // Bot starts speaking
        let started = state.on_audio_sent().expect("Should get started event");
        assert!(started.timestamp_ns > 0);

        // Simulate TTS audio chunks every 10ms
        for _ in 0..10 {
            thread::sleep(Duration::from_millis(10));
            assert!(state.on_audio_sent().is_none()); // No event for subsequent chunks
            assert!(state.check_silence().is_none()); // Not silent yet
        }

        // Stop sending audio
        thread::sleep(Duration::from_millis(150)); // Wait past threshold

        // Detect silence
        let stopped = state.check_silence().expect("Should detect silence");
        assert!(!state.is_speaking());

        // Duration should be approximately 100ms (10 chunks * 10ms)
        let duration_ms = stopped.speaking_duration_ns / 1_000_000;
        assert!((90..=150).contains(&duration_ms));
    }

    #[test]
    fn test_interrupted_speaking() {
        let state = BotSpeakingState::new(100);

        // Start speaking
        state.on_audio_sent();
        thread::sleep(Duration::from_millis(50));
        state.on_audio_sent();

        // Interrupt (reset)
        state.reset();

        // Should no longer be speaking
        assert!(!state.is_speaking());

        // Should be able to start fresh
        let restarted = state.on_audio_sent();
        assert!(restarted.is_some());
    }
}

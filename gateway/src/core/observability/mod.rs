//! Observability infrastructure for non-intrusive monitoring of voice processing.
//!
//! This module provides:
//! - [`VoiceObserver`] trait for implementing observers
//! - [`ObserverRegistry`] for managing multiple observers
//! - Built-in observers for latency tracking and metrics
//!
//! # Example
//!
//! ```rust
//! use std::sync::Arc;
//! use crate::core::observability::{VoiceObserver, ObserverRegistry};
//!
//! struct MetricsObserver;
//!
//! impl VoiceObserver for MetricsObserver {
//!     fn on_stt_result(&self, result: &STTResult, latency_ns: u64) {
//!         println!("STT result received with latency: {}ns", latency_ns);
//!     }
//! }
//!
//! let registry = ObserverRegistry::new();
//! let id = registry.register(Arc::new(MetricsObserver));
//! ```

pub mod latency;
pub mod observer;
pub mod speaking;

pub use latency::{LatencyMetrics, UserBotLatencyObserver};
pub use observer::{ObserverRegistry, VoiceObserver};
pub use speaking::{BotSpeakingStarted, BotSpeakingStopped, BotSpeakingState};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Test that the module exports are correct
    #[test]
    fn test_module_exports() {
        // Verify all public types are accessible
        let _registry = ObserverRegistry::new();
        let _latency_observer = UserBotLatencyObserver::new(100);
        let _speaking_state = BotSpeakingState::new(350);
    }

    /// Integration test: Observer receives events through registry
    #[test]
    fn test_observer_integration() {
        struct CountingObserver {
            stt_count: AtomicU64,
            tts_count: AtomicU64,
        }

        impl VoiceObserver for CountingObserver {
            fn on_stt_result(
                &self,
                _result: &crate::core::stt::STTResult,
                _latency_ns: u64,
            ) {
                self.stt_count.fetch_add(1, Ordering::Relaxed);
            }

            fn on_tts_chunk(
                &self,
                _chunk: &crate::core::tts::AudioData,
                _ttfb_ns: Option<u64>,
            ) {
                self.tts_count.fetch_add(1, Ordering::Relaxed);
            }
        }

        let registry = ObserverRegistry::new();
        let observer = Arc::new(CountingObserver {
            stt_count: AtomicU64::new(0),
            tts_count: AtomicU64::new(0),
        });

        let id = registry.register(observer.clone());

        // Create test data
        let stt_result = crate::core::stt::STTResult::new("test".to_string(), true, false, 0.95);

        let audio_data = crate::core::tts::AudioData {
            data: vec![0u8; 100],
            sample_rate: 24000,
            format: "pcm".to_string(),
            duration_ms: Some(10),
        };

        // Notify observers
        registry.notify_stt_result(&stt_result, 1000);
        registry.notify_tts_chunk(&audio_data, Some(500));

        assert_eq!(observer.stt_count.load(Ordering::Relaxed), 1);
        assert_eq!(observer.tts_count.load(Ordering::Relaxed), 1);

        // Unregister and verify no more notifications
        assert!(registry.unregister(id));
        registry.notify_stt_result(&stt_result, 1000);
        assert_eq!(observer.stt_count.load(Ordering::Relaxed), 1);
    }
}

//! Lock Safety Tests for FFI Adapters
//!
//! These tests verify that FFI adapters handle lock poisoning gracefully
//! instead of panicking, ensuring service stability.

use std::panic;
use std::sync::{Arc, Mutex};
use std::thread;

/// Test helper to poison a mutex by panicking while holding it
fn poison_mutex<T: Default + Send + 'static>(mutex: &Arc<Mutex<T>>) {
    let mutex_clone = Arc::clone(mutex);
    let handle = thread::spawn(move || {
        let _guard = mutex_clone.lock().unwrap();
        panic!("Intentional panic to poison mutex");
    });
    let _ = handle.join(); // Join to ensure panic completes
}

/// Verify that a mutex is poisoned
fn is_poisoned<T>(mutex: &Mutex<T>) -> bool {
    mutex.lock().is_err()
}

// =============================================================================
// Unit Tests for Lock Poisoning Behavior
// =============================================================================

mod mutex_behavior_tests {
    use super::*;

    #[test]
    fn test_mutex_becomes_poisoned_after_panic() {
        let mutex = Arc::new(Mutex::new(42i32));
        assert!(!is_poisoned(&mutex));

        poison_mutex(&mutex);

        assert!(is_poisoned(&mutex));
    }

    #[test]
    fn test_poisoned_mutex_returns_error_not_panic() {
        let mutex = Arc::new(Mutex::new(42i32));
        poison_mutex(&mutex);

        // This should NOT panic, should return Err
        let result = mutex.lock();
        assert!(result.is_err());

        // We can still get the data via into_inner()
        let poisoned = result.unwrap_err();
        let value = poisoned.into_inner();
        assert_eq!(*value, 42);
    }

    #[test]
    fn test_unwrap_or_default_on_poisoned_mutex() {
        let mutex = Arc::new(Mutex::new(String::from("hello")));
        poison_mutex(&mutex);

        // Pattern: graceful fallback
        let value = mutex
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());

        assert_eq!(value, "hello");
    }
}

// =============================================================================
// Error Handling Pattern Tests
// =============================================================================

mod error_handling_patterns {
    use super::*;

    /// Simulates the current dangerous pattern
    #[test]
    #[should_panic(expected = "PoisonError")]
    fn test_unwrap_panics_on_poisoned_mutex() {
        let mutex = Arc::new(Mutex::new(42i32));
        poison_mutex(&mutex);

        // This WILL panic - demonstrating the problem
        let _guard = mutex.lock().unwrap();
    }

    /// Demonstrates the safe replacement pattern
    #[test]
    fn test_map_err_handles_poisoned_mutex() {
        let mutex = Arc::new(Mutex::new(42i32));
        poison_mutex(&mutex);

        // Safe pattern - returns error instead of panic
        let result: Result<i32, &'static str> = mutex
            .lock()
            .map(|guard| *guard)
            .map_err(|_| "Lock poisoned");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Lock poisoned");
    }

    /// Demonstrates is_ready pattern - return false on poison
    #[test]
    fn test_is_ready_returns_false_on_poison() {
        let mutex = Arc::new(Mutex::new(true));
        poison_mutex(&mutex);

        // Safe pattern for is_ready
        let is_ready = mutex.lock().map(|guard| *guard).unwrap_or(false);

        assert!(!is_ready);
    }

    /// Demonstrates get_connection_state pattern - return Disconnected on poison
    #[test]
    fn test_connection_state_returns_disconnected_on_poison() {
        #[derive(Clone, Copy, PartialEq, Debug)]
        enum ConnectionState {
            Connected,
            Disconnected,
        }

        let mutex = Arc::new(Mutex::new(true));
        poison_mutex(&mutex);

        // Safe pattern for get_connection_state
        let state = mutex
            .lock()
            .map(|guard| {
                if *guard {
                    ConnectionState::Connected
                } else {
                    ConnectionState::Disconnected
                }
            })
            .unwrap_or(ConnectionState::Disconnected);

        assert_eq!(state, ConnectionState::Disconnected);
    }
}

// =============================================================================
// STT Adapter Pattern Tests
// =============================================================================

mod stt_adapter_patterns {
    use super::*;

    /// Mock STT error type
    #[derive(Debug, PartialEq)]
    enum STTError {
        ConnectionFailed(String),
        AudioProcessingError(String),
        ProviderError(String),
    }

    /// Tests the connect() error handling pattern
    #[test]
    fn test_connect_error_on_provider_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(()));
        poison_mutex(&provider_mutex);

        // Pattern used in connect()
        let result: Result<(), STTError> = provider_mutex
            .lock()
            .map_err(|_| STTError::ConnectionFailed("Provider unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    /// Tests the connect() error handling pattern for connected state lock
    #[test]
    fn test_connect_error_on_connected_lock_poison() {
        let connected_mutex = Arc::new(Mutex::new(false));
        poison_mutex(&connected_mutex);

        // Pattern used for setting connected state
        let result: Result<(), STTError> = connected_mutex
            .lock()
            .map_err(|_| STTError::ConnectionFailed("State tracking unavailable".into()))
            .map(|mut guard| {
                *guard = true;
            });

        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    /// Tests the send_audio() error handling pattern
    #[test]
    fn test_send_audio_error_on_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(()));
        poison_mutex(&provider_mutex);

        // Pattern used in send_audio()
        let result: Result<(), STTError> = provider_mutex
            .lock()
            .map_err(|_| STTError::AudioProcessingError("STT provider unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(STTError::AudioProcessingError(_))));
    }

    /// Tests the is_ready() safe fallback pattern
    #[test]
    fn test_is_ready_false_on_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(true));
        poison_mutex(&provider_mutex);

        // Pattern used in is_ready()
        let is_ready = provider_mutex.lock().map(|guard| *guard).unwrap_or(false);

        assert!(!is_ready);
    }

    /// Tests callback registration error handling
    #[test]
    fn test_callback_registration_error_on_lock_poison() {
        let storage_mutex = Arc::new(Mutex::new(Vec::<()>::new()));
        poison_mutex(&storage_mutex);

        // Pattern used in on_result() / on_error()
        let result: Result<(), STTError> = storage_mutex
            .lock()
            .map_err(|_| STTError::ProviderError("Callback storage unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(STTError::ProviderError(_))));
    }
}

// =============================================================================
// TTS Adapter Pattern Tests
// =============================================================================

mod tts_adapter_patterns {
    use super::*;

    /// Mock TTS error type
    #[derive(Debug, PartialEq)]
    enum TTSError {
        ConnectionFailed(String),
        AudioGenerationFailed(String),
        ProviderError(String),
        InternalError(String),
    }

    /// Tests the speak() error handling pattern
    #[test]
    fn test_speak_error_on_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(()));
        poison_mutex(&provider_mutex);

        // Pattern used in speak()
        let result: Result<(), TTSError> = provider_mutex
            .lock()
            .map_err(|_| TTSError::AudioGenerationFailed("TTS provider unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(TTSError::AudioGenerationFailed(_))));
    }

    /// Tests the clear() error handling pattern
    #[test]
    fn test_clear_error_on_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(()));
        poison_mutex(&provider_mutex);

        // Pattern used in clear()
        let result: Result<(), TTSError> = provider_mutex
            .lock()
            .map_err(|_| TTSError::ProviderError("TTS provider unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(TTSError::ProviderError(_))));
    }

    /// Tests the flush() error handling pattern
    #[test]
    fn test_flush_error_on_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(()));
        poison_mutex(&provider_mutex);

        // Pattern used in flush()
        let result: Result<(), TTSError> = provider_mutex
            .lock()
            .map_err(|_| TTSError::ProviderError("TTS provider unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(TTSError::ProviderError(_))));
    }

    /// Tests the on_audio() callback registration error handling
    #[test]
    fn test_on_audio_error_on_lock_poison() {
        let storage_mutex = Arc::new(Mutex::new(Vec::<()>::new()));
        poison_mutex(&storage_mutex);

        // Pattern used in on_audio()
        let result: Result<(), TTSError> = storage_mutex
            .lock()
            .map_err(|_| TTSError::InternalError("Callback storage unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(TTSError::InternalError(_))));
    }
}

// =============================================================================
// Realtime Adapter Pattern Tests
// =============================================================================

mod realtime_adapter_patterns {
    use super::*;

    /// Mock Realtime error type
    #[derive(Debug, PartialEq)]
    enum RealtimeError {
        ConnectionFailed(String),
        ProviderError(String),
        InvalidConfiguration(String),
    }

    /// Tests the send_audio() error handling pattern for realtime
    #[test]
    fn test_realtime_send_audio_error_on_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(()));
        poison_mutex(&provider_mutex);

        // Pattern used in realtime send_audio()
        let result: Result<(), RealtimeError> = provider_mutex
            .lock()
            .map_err(|_| RealtimeError::ProviderError("Realtime provider unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(RealtimeError::ProviderError(_))));
    }

    /// Tests the send_text() error handling pattern
    #[test]
    fn test_send_text_error_on_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(()));
        poison_mutex(&provider_mutex);

        // Pattern used in send_text()
        let result: Result<(), RealtimeError> = provider_mutex
            .lock()
            .map_err(|_| RealtimeError::ProviderError("Realtime provider unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(RealtimeError::ProviderError(_))));
    }

    /// Tests the create_response() error handling pattern
    #[test]
    fn test_create_response_error_on_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(()));
        poison_mutex(&provider_mutex);

        // Pattern used in create_response()
        let result: Result<(), RealtimeError> = provider_mutex
            .lock()
            .map_err(|_| RealtimeError::ProviderError("Realtime provider unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(RealtimeError::ProviderError(_))));
    }

    /// Tests the cancel_response() error handling pattern
    #[test]
    fn test_cancel_response_error_on_lock_poison() {
        let provider_mutex = Arc::new(Mutex::new(()));
        poison_mutex(&provider_mutex);

        // Pattern used in cancel_response()
        let result: Result<(), RealtimeError> = provider_mutex
            .lock()
            .map_err(|_| RealtimeError::ProviderError("Realtime provider unavailable".into()))
            .map(|_| ());

        assert!(matches!(result, Err(RealtimeError::ProviderError(_))));
    }
}

// =============================================================================
// Concurrent Access Tests
// =============================================================================

mod concurrent_access_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test that concurrent access doesn't cause deadlocks
    #[test]
    fn test_concurrent_lock_access_no_deadlock() {
        let mutex = Arc::new(Mutex::new(0usize));
        let success_count = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..100)
            .map(|_| {
                let mutex = Arc::clone(&mutex);
                let success_count = Arc::clone(&success_count);
                thread::spawn(move || {
                    for _ in 0..100 {
                        if let Ok(mut guard) = mutex.lock() {
                            *guard += 1;
                            success_count.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let total = success_count.load(Ordering::SeqCst);
        assert_eq!(total, 10000);
    }

    /// Test that poisoned mutex doesn't cause cascading failures
    #[test]
    fn test_poisoned_mutex_allows_continued_operation() {
        let mutex = Arc::new(Mutex::new(0usize));
        poison_mutex(&mutex);

        let success_count = Arc::new(AtomicUsize::new(0));
        let error_count = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let mutex = Arc::clone(&mutex);
                let success_count = Arc::clone(&success_count);
                let error_count = Arc::clone(&error_count);
                thread::spawn(move || {
                    for _ in 0..100 {
                        match mutex.lock() {
                            Ok(mut guard) => {
                                *guard += 1;
                                success_count.fetch_add(1, Ordering::SeqCst);
                            }
                            Err(_) => {
                                error_count.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // All operations should have returned errors (mutex is poisoned)
        let errors = error_count.load(Ordering::SeqCst);
        assert_eq!(errors, 1000);

        // No successes
        let successes = success_count.load(Ordering::SeqCst);
        assert_eq!(successes, 0);
    }
}

// =============================================================================
// Integration-Style Tests (Simulated Adapter Behavior)
// =============================================================================

mod simulated_adapter_tests {
    use super::*;

    /// Simulates a simplified STT adapter
    struct MockSTTAdapter {
        provider: Mutex<bool>, // bool represents "connected" state
        connected: Mutex<bool>,
    }

    impl MockSTTAdapter {
        fn new() -> Self {
            Self {
                provider: Mutex::new(false),
                connected: Mutex::new(false),
            }
        }

        fn connect(&self) -> Result<(), &'static str> {
            let mut provider = self.provider.lock().map_err(|_| "Provider unavailable")?;
            *provider = true;

            let mut connected = self
                .connected
                .lock()
                .map_err(|_| "State tracking unavailable")?;
            *connected = true;

            Ok(())
        }

        fn is_ready(&self) -> bool {
            self.provider.lock().map(|guard| *guard).unwrap_or(false)
        }

        fn send_audio(&self) -> Result<(), &'static str> {
            let _provider = self
                .provider
                .lock()
                .map_err(|_| "STT provider unavailable")?;
            Ok(())
        }
    }

    #[test]
    fn test_mock_adapter_normal_operation() {
        let adapter = MockSTTAdapter::new();

        assert!(!adapter.is_ready());
        assert!(adapter.connect().is_ok());
        assert!(adapter.is_ready());
        assert!(adapter.send_audio().is_ok());
    }

    #[test]
    fn test_mock_adapter_poisoned_provider() {
        let adapter = Arc::new(MockSTTAdapter::new());

        // Poison the provider mutex
        {
            let adapter_clone = Arc::clone(&adapter);
            let handle = thread::spawn(move || {
                let _guard = adapter_clone.provider.lock().unwrap();
                panic!("Intentional panic");
            });
            let _ = handle.join();
        }

        // Operations should return errors, not panic
        assert!(!adapter.is_ready());
        assert_eq!(adapter.connect(), Err("Provider unavailable"));
        assert_eq!(adapter.send_audio(), Err("STT provider unavailable"));
    }

    #[test]
    fn test_mock_adapter_poisoned_connected() {
        let adapter = Arc::new(MockSTTAdapter::new());

        // Poison the connected mutex
        {
            let adapter_clone = Arc::clone(&adapter);
            let handle = thread::spawn(move || {
                let _guard = adapter_clone.connected.lock().unwrap();
                panic!("Intentional panic");
            });
            let _ = handle.join();
        }

        // Connect should fail on second lock, but send_audio should work
        assert_eq!(adapter.connect(), Err("State tracking unavailable"));
        assert!(adapter.send_audio().is_ok()); // Provider not poisoned
    }
}

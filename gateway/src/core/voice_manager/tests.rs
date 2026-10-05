//! Tests for VoiceManager

use crate::core::stt::{STTConfig, STTResult};
use crate::core::tts::TTSConfig;
use crate::core::voice_manager::manager::WAAV_UNINTERRUPTIBLE_PLAYBACK_ENV;
use crate::core::voice_manager::state::SpeechFinalState;
use crate::core::voice_manager::stt_result::STTResultProcessor;
use crate::core::voice_manager::{
    VoiceManager, VoiceManagerConfig, VoiceManagerError, VoiceManagerResult,
};
use parking_lot::RwLock as SyncRwLock;
use serial_test::serial;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::mpsc;

fn ag6_voice_manager_result() -> VoiceManagerResult<VoiceManager> {
    let stt_config = STTConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    let tts_config = TTSConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    VoiceManager::new(VoiceManagerConfig::new(stt_config, tts_config), None)
}

fn ag6_voice_manager() -> VoiceManager {
    ag6_voice_manager_result().unwrap()
}

struct UninterruptiblePlaybackEnvGuard(Option<String>);

impl Drop for UninterruptiblePlaybackEnvGuard {
    fn drop(&mut self) {
        match self.0.as_deref() {
            Some(value) => unsafe { std::env::set_var(WAAV_UNINTERRUPTIBLE_PLAYBACK_ENV, value) },
            None => unsafe { std::env::remove_var(WAAV_UNINTERRUPTIBLE_PLAYBACK_ENV) },
        }
    }
}

fn set_uninterruptible_playback_env(value: Option<&str>) -> UninterruptiblePlaybackEnvGuard {
    let previous = std::env::var(WAAV_UNINTERRUPTIBLE_PLAYBACK_ENV).ok();
    match value {
        Some(value) => unsafe { std::env::set_var(WAAV_UNINTERRUPTIBLE_PLAYBACK_ENV, value) },
        None => unsafe { std::env::remove_var(WAAV_UNINTERRUPTIBLE_PLAYBACK_ENV) },
    }
    UninterruptiblePlaybackEnvGuard(previous)
}

fn ag6_audio(tag: u8) -> crate::core::tts::AudioData {
    crate::core::tts::AudioData {
        data: vec![tag; 8],
        sample_rate: 24_000,
        format: "pcm".to_string(),
        duration_ms: Some(40),
    }
}

/// A gated egress sink: signals (on `started_tx`) when the pump pops a chunk and
/// begins delivering it (so the test knows it is in flight), then blocks on a
/// semaphore until the test releases it, recording delivered chunk ids.
fn ag6_gated_sink() -> (
    impl Fn(
        crate::core::tts::AudioData,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    + Send
    + Sync
    + 'static,
    Arc<std::sync::Mutex<Vec<u8>>>,
    Arc<tokio::sync::Semaphore>,
    mpsc::UnboundedReceiver<u8>,
) {
    let delivered = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (started_tx, started_rx) = mpsc::unbounded_channel();
    let d = delivered.clone();
    let g = gate.clone();
    let sink = move |a: crate::core::tts::AudioData| {
        let d = d.clone();
        let g = g.clone();
        let started_tx = started_tx.clone();
        Box::pin(async move {
            let id = a.data[0];
            let _ = started_tx.send(id);
            let _permit = g.acquire().await.unwrap();
            d.lock().unwrap().push(id);
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    };
    (sink, delivered, gate, started_rx)
}

#[tokio::test]
#[serial]
async fn ag6_pump_engaged_only_when_enabled() {
    let _guard = set_uninterruptible_playback_env(None);

    // Disabled (default): no pump — the validated immediate-delivery path.
    let vm = ag6_voice_manager();
    vm.on_tts_audio(|_a| Box::pin(async {})).await.unwrap();
    assert!(!vm.test_has_playback_pump(), "default A-G6 off ⇒ no pump");

    // Enabled before on_tts_audio ⇒ the pump is created.
    let vm = ag6_voice_manager();
    vm.set_uninterruptible_playback(true);
    vm.on_tts_audio(|_a| Box::pin(async {})).await.unwrap();
    assert!(vm.test_has_playback_pump(), "A-G6 on ⇒ pump engaged");
}

#[tokio::test]
#[serial]
async fn ag6_env_true_engages_pump() {
    let _guard = set_uninterruptible_playback_env(Some("yes"));

    let vm = ag6_voice_manager();
    vm.on_tts_audio(|_a| Box::pin(async {})).await.unwrap();

    assert!(vm.test_has_playback_pump(), "env true ⇒ pump engaged");
}

#[tokio::test]
#[serial]
async fn ag6_env_false_keeps_pump_off() {
    let _guard = set_uninterruptible_playback_env(Some("0"));

    let vm = ag6_voice_manager();
    vm.on_tts_audio(|_a| Box::pin(async {})).await.unwrap();

    assert!(!vm.test_has_playback_pump(), "env false ⇒ no pump");
}

#[test]
#[serial]
fn ag6_env_malformed_rejects_voice_manager_initialization() {
    let _guard = set_uninterruptible_playback_env(Some("tru"));

    let err = match ag6_voice_manager_result() {
        Ok(_) => panic!("malformed env should reject VoiceManager initialization"),
        Err(err) => err,
    };

    match err {
        VoiceManagerError::InitializationError(message) => {
            assert!(
                message.contains(WAAV_UNINTERRUPTIBLE_PLAYBACK_ENV),
                "error should name env var: {message}"
            );
        }
        other => panic!("expected InitializationError, got {other:?}"),
    }
}

#[tokio::test]
#[serial]
async fn ag6_queued_uninterruptible_survives_barge_in() {
    // Mixed queue: int1, uninterruptible(2), int2. A barge-in keeps 2 and drops
    // the interruptible audio queued behind it. No synthesis-deadline override
    // is needed (the gate is has_uninterruptible_active, not a timer).
    let vm = ag6_voice_manager();
    vm.set_uninterruptible_playback(true);
    let (sink, delivered, gate, _started) = ag6_gated_sink();
    vm.on_tts_audio(sink).await.unwrap();

    vm.test_set_allow_interruption(true);
    vm.test_emit_tts_chunk(ag6_audio(1)).await;
    vm.test_set_allow_interruption(false);
    vm.test_emit_tts_chunk(ag6_audio(2)).await;
    vm.test_set_allow_interruption(true);
    vm.test_emit_tts_chunk(ag6_audio(3)).await;

    vm.clear_tts().await.unwrap();
    gate.add_permits(8);
    for _ in 0..50 {
        if delivered.lock().unwrap().contains(&2) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let got = delivered.lock().unwrap().clone();
    assert!(
        got.contains(&2),
        "the uninterruptible chunk survived: {got:?}"
    );
    assert!(
        !got.contains(&3),
        "the trailing interruptible chunk was dropped: {got:?}"
    );
}

#[tokio::test]
#[serial]
async fn ag6_in_flight_uninterruptible_survives_barge_in() {
    // The review-#2 case: the disclaimer's only chunk is already POPPED (in
    // flight, playing in the transport) when the barge-in arrives. The protection
    // signal (has_uninterruptible_active) must still hold, so clear_tts takes the
    // selective path and does NOT flush the transport mid-disclaimer.
    let vm = ag6_voice_manager();
    vm.set_uninterruptible_playback(true);
    let (sink, delivered, gate, mut started) = ag6_gated_sink();
    vm.on_tts_audio(sink).await.unwrap();

    vm.test_set_allow_interruption(false);
    vm.test_emit_tts_chunk(ag6_audio(2)).await;
    // Wait until the pump has popped it and begun delivery (now in flight).
    assert_eq!(started.recv().await, Some(2));

    // Barge-in while it is in flight: must be the selective (non-flushing) path.
    vm.clear_tts().await.unwrap();

    gate.add_permits(8);
    for _ in 0..50 {
        if delivered.lock().unwrap().contains(&2) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        delivered.lock().unwrap().contains(&2),
        "the in-flight uninterruptible chunk was protected from the barge-in"
    );
}

#[tokio::test]
async fn test_voice_manager_creation() {
    let stt_config = STTConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    let tts_config = TTSConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    let config = VoiceManagerConfig::new(stt_config, tts_config);

    let result = VoiceManager::new(config, None);
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_voice_manager_config_access() {
    let stt_config = STTConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    let tts_config = TTSConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    let config = VoiceManagerConfig::new(stt_config, tts_config);

    let voice_manager = VoiceManager::new(config, None).unwrap();
    let retrieved_config = voice_manager.get_config();

    assert_eq!(retrieved_config.stt_config.provider, "deepgram");
    assert_eq!(retrieved_config.tts_config.provider, "deepgram");
}

#[tokio::test]
async fn test_voice_manager_callback_registration() {
    let stt_config = STTConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    let tts_config = TTSConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    let config = VoiceManagerConfig::new(stt_config, tts_config);

    let voice_manager = VoiceManager::new(config, None).unwrap();

    // Test STT callback registration
    let stt_result = voice_manager
        .on_stt_result(|result| {
            Box::pin(async move {
                println!("STT Result: {}", result.transcript);
            })
        })
        .await;

    assert!(stt_result.is_ok());

    // Test TTS callback registration
    let tts_result = voice_manager
        .on_tts_audio(|audio_data| {
            Box::pin(async move {
                println!("TTS Audio: {} bytes", audio_data.data.len());
            })
        })
        .await;

    assert!(tts_result.is_ok());
}

#[tokio::test]
async fn test_speech_final_timing_control() {
    let stt_config = STTConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    let tts_config = TTSConfig {
        provider: "deepgram".to_string(),
        api_key: "test_key".to_string(),
        ..Default::default()
    };
    let config = VoiceManagerConfig::new(stt_config, tts_config);

    let voice_manager = VoiceManager::new(config, None).unwrap();

    // Channel to collect results
    let (tx, _rx) = mpsc::unbounded_channel();
    let result_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = result_counter.clone();

    // Register callback to collect results
    voice_manager
        .on_stt_result(move |result| {
            let tx = tx.clone();
            let counter = counter_clone.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::Relaxed);
                let _ = tx.send(result);
            })
        })
        .await
        .unwrap();

    // Test Case 1: is_final result should return immediately and start timer
    {
        let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::with_capacity(1024),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: None,
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::with_capacity(1024),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // Reset state first
        {
            let mut state = speech_final_state.write();
            *state = SpeechFinalState {
                text_buffer: String::with_capacity(1024),
                turn_detection_handle: None,
                hard_timeout_handle: None,
                waiting_for_speech_final: AtomicBool::new(false),
                user_callback: None,
                turn_detection_last_fired_ms: AtomicUsize::new(0),
                last_forced_text: String::with_capacity(1024),
                segment_start_ms: AtomicUsize::new(0),
                hard_timeout_deadline_ms: AtomicUsize::new(0),
                fire_generation: AtomicUsize::new(0),
            };
        }

        // Send is_final=true without is_speech_final=true
        let result1 = STTResult::new("Hello".to_string(), true, false, 0.9);
        let processor = STTResultProcessor::default();
        let processed = processor
            .process_result(result1, speech_final_state.clone(), None)
            .await;

        // Should return original result immediately
        assert!(processed.is_some());
        let processed_result = processed.unwrap();
        assert_eq!(processed_result.transcript, "Hello");
        assert!(processed_result.is_final);
        assert!(!processed_result.is_speech_final);

        // Timer should be started and state updated
        let state = speech_final_state.read();
        assert!(state.waiting_for_speech_final.load(Ordering::Acquire));
        assert_eq!(state.text_buffer, "Hello");
        assert!(state.turn_detection_handle.is_some());
    }

    // Test Case 2: Timer should be started for final results and state should be set correctly
    {
        let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::with_capacity(1024),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: None,
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::with_capacity(1024),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // Reset state first
        {
            let mut state = speech_final_state.write();
            *state = SpeechFinalState {
                text_buffer: String::with_capacity(1024),
                turn_detection_handle: None,
                hard_timeout_handle: None,
                waiting_for_speech_final: AtomicBool::new(false),
                user_callback: None,
                turn_detection_last_fired_ms: AtomicUsize::new(0),
                last_forced_text: String::with_capacity(1024),
                segment_start_ms: AtomicUsize::new(0),
                hard_timeout_deadline_ms: AtomicUsize::new(0),
                fire_generation: AtomicUsize::new(0),
            };
        }

        // Send is_final=true without is_speech_final=true
        let result1 = STTResult::new("Test message".to_string(), true, false, 0.8);
        let processor = STTResultProcessor::default();
        let processed = processor
            .process_result(result1, speech_final_state.clone(), None)
            .await;

        // Should return the original result immediately
        assert!(processed.is_some());
        let processed_result = processed.unwrap();
        assert_eq!(processed_result.transcript, "Test message");
        assert!(processed_result.is_final);
        assert!(!processed_result.is_speech_final);

        // Check that timer was started and state is correct
        let state = speech_final_state.read();
        assert!(state.waiting_for_speech_final.load(Ordering::Acquire));
        assert_eq!(state.text_buffer, "Test message");
        assert!(state.turn_detection_handle.is_some());
    }

    // Test Case 3: Real speech_final should cancel timer and reset state
    {
        let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::with_capacity(1024),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: None,
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::with_capacity(1024),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // Reset state first
        {
            let mut state = speech_final_state.write();
            *state = SpeechFinalState {
                text_buffer: String::with_capacity(1024),
                turn_detection_handle: None,
                hard_timeout_handle: None,
                waiting_for_speech_final: AtomicBool::new(false),
                user_callback: None,
                turn_detection_last_fired_ms: AtomicUsize::new(0),
                last_forced_text: String::with_capacity(1024),
                segment_start_ms: AtomicUsize::new(0),
                hard_timeout_deadline_ms: AtomicUsize::new(0),
                fire_generation: AtomicUsize::new(0),
            };
        }

        // Send is_final=true result to start timer
        let result1 = STTResult::new("Hello world".to_string(), true, false, 0.9);
        let processor = STTResultProcessor::default();
        let _processed1 = processor
            .process_result(result1, speech_final_state.clone(), None)
            .await;

        // Verify timer was started
        {
            let state = speech_final_state.read();
            assert!(state.waiting_for_speech_final.load(Ordering::Acquire));
            assert!(state.turn_detection_handle.is_some());
            assert_eq!(state.text_buffer, "Hello world");
        }

        // Send is_speech_final=true (should cancel timer and reset state)
        let result2 = STTResult::new("final result".to_string(), true, true, 0.95);
        let processor2 = STTResultProcessor::default();
        let processed2 = processor2
            .process_result(result2, speech_final_state.clone(), None)
            .await;

        // Turn policy must see the FULL segment (buffered fragments + this
        // one) via turn_transcript() — the old pass-through dropped "Hello
        // world" and ran turns with truncated input. The RAW transcript stays
        // the provider's last fragment so client egress (which already saw
        // "Hello world") gets no duplicate (review wf_5772cd64 #6).
        assert!(processed2.is_some());
        let final_result = processed2.unwrap();
        assert!(final_result.is_speech_final);
        assert!(final_result.is_final);
        assert_eq!(final_result.transcript, "final result");
        assert_eq!(final_result.turn_transcript(), "Hello world final result");
        assert_eq!(final_result.confidence, 0.95);

        // State should be reset
        let state = speech_final_state.read();
        assert!(!state.waiting_for_speech_final.load(Ordering::Acquire));
        assert!(state.text_buffer.is_empty());
        assert!(state.turn_detection_handle.is_none());
    }

    // Test Case 4: Direct speech_final with no prior timer should return original result
    {
        let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::with_capacity(1024),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: None,
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::with_capacity(1024),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // Reset state first
        {
            let mut state = speech_final_state.write();
            *state = SpeechFinalState {
                text_buffer: String::with_capacity(1024),
                turn_detection_handle: None,
                hard_timeout_handle: None,
                waiting_for_speech_final: AtomicBool::new(false),
                user_callback: None,
                turn_detection_last_fired_ms: AtomicUsize::new(0),
                last_forced_text: String::with_capacity(1024),
                segment_start_ms: AtomicUsize::new(0),
                hard_timeout_deadline_ms: AtomicUsize::new(0),
                fire_generation: AtomicUsize::new(0),
            };
        }

        // Send is_speech_final=true with no prior timer (direct speech final)
        let result = STTResult::new("Direct speech final".to_string(), true, true, 0.85);
        let processor = STTResultProcessor::default();
        let processed = processor
            .process_result(result, speech_final_state.clone(), None)
            .await;

        assert!(processed.is_some());
        let final_result = processed.unwrap();
        assert!(final_result.is_speech_final);
        assert!(final_result.is_final);
        // Should return original text as-is
        assert_eq!(final_result.transcript, "Direct speech final");
        assert_eq!(final_result.confidence, 0.85);

        // State should be reset
        let state = speech_final_state.read();
        assert!(!state.waiting_for_speech_final.load(Ordering::Acquire));
        assert!(state.text_buffer.is_empty());
    }
}

#[tokio::test]
async fn test_duplicate_speech_final_prevention() {
    // Test Case 1: Timer fires, then real speech_final arrives - should prevent duplicate
    {
        let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::with_capacity(1024),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: None,
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::with_capacity(1024),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // Simulate the scenario:
        // 1. is_final=true arrives
        let result1 = STTResult::new("Hello world".to_string(), true, false, 0.9);
        let processor1 = STTResultProcessor::default();
        let processed1 = processor1
            .process_result(result1.clone(), speech_final_state.clone(), None)
            .await;

        assert!(processed1.is_some());
        assert_eq!(processed1.unwrap().transcript, "Hello world");

        // Verify timer was started
        {
            let state = speech_final_state.read();
            assert!(state.waiting_for_speech_final.load(Ordering::Acquire));
            assert!(state.turn_detection_handle.is_some());
        }

        // 2. Simulate timer firing (mark as fired)
        {
            let mut state = speech_final_state.write();
            let fire_time_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as usize;
            state
                .turn_detection_last_fired_ms
                .store(fire_time_ms, Ordering::Release);
            state.last_forced_text = "Hello world".to_string();
            state
                .waiting_for_speech_final
                .store(false, Ordering::Release);
        }

        // 3. Real speech_final arrives after timer fired
        let result2 = STTResult::new("Hello world".to_string(), true, true, 0.95);
        let processor2 = STTResultProcessor::default();
        let processed2 = processor2
            .process_result(result2, speech_final_state.clone(), None)
            .await;

        // Should be None (ignored) because timer already fired
        assert!(processed2.is_none());
    }

    // Test Case 2: Multiple is_final results after timer fired should not restart timer
    {
        let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::with_capacity(1024),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: None,
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::with_capacity(1024),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // 1. First is_final=true
        let result1 = STTResult::new("First".to_string(), true, false, 0.9);
        let processor1 = STTResultProcessor::default();
        let processed1 = processor1
            .process_result(result1, speech_final_state.clone(), None)
            .await;

        assert!(processed1.is_some());

        // Mark timer as fired (simulate timer expiry)
        {
            let mut state = speech_final_state.write();
            let old_time_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as usize
                - 1000; // 1 second ago
            state
                .turn_detection_last_fired_ms
                .store(old_time_ms, Ordering::Release);
            state.last_forced_text = "First".to_string();
            state
                .waiting_for_speech_final
                .store(false, Ordering::Release);
        }

        // 2. Another is_final=true arrives after timer fired
        let result2 = STTResult::new("Second".to_string(), true, false, 0.9);
        let processor2 = STTResultProcessor::default();
        let processed2 = processor2
            .process_result(result2, speech_final_state.clone(), None)
            .await;

        // Should still return the result but NOT start a new timer
        assert!(processed2.is_some());
        assert_eq!(processed2.unwrap().transcript, "Second");

        // Verify new timer WAS started (continuous speech should work)
        {
            let state = speech_final_state.read();
            assert!(state.waiting_for_speech_final.load(Ordering::Acquire));
            assert!(state.turn_detection_handle.is_some());
        }
    }

    // Test Case 3: New speech segment after proper reset should work normally
    {
        let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
            text_buffer: String::with_capacity(1024),
            turn_detection_handle: None,
            hard_timeout_handle: None,
            waiting_for_speech_final: AtomicBool::new(false),
            user_callback: None,
            turn_detection_last_fired_ms: AtomicUsize::new(0),
            last_forced_text: String::with_capacity(1024),
            segment_start_ms: AtomicUsize::new(0),
            hard_timeout_deadline_ms: AtomicUsize::new(0),
            fire_generation: AtomicUsize::new(0),
        }));

        // First sequence: is_final=true starts timer
        let result1 = STTResult::new("First segment".to_string(), true, false, 0.9);
        let processor1 = STTResultProcessor::default();
        let processed1 = processor1
            .process_result(result1, speech_final_state.clone(), None)
            .await;
        assert!(processed1.is_some());

        // Mark timer as fired (with recent timestamp)
        {
            let mut state = speech_final_state.write();
            let fire_time_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as usize;
            state
                .turn_detection_last_fired_ms
                .store(fire_time_ms, Ordering::Release);
            state.last_forced_text = "First segment".to_string();
            state
                .waiting_for_speech_final
                .store(false, Ordering::Release);
        }

        // Real speech_final arrives but is ignored (timer already fired)
        let result2 = STTResult::new("First segment".to_string(), true, true, 0.9);
        let processor2 = STTResultProcessor::default();
        let processed2 = processor2
            .process_result(result2, speech_final_state.clone(), None)
            .await;
        assert!(processed2.is_none()); // Ignored due to timer fired recently with same text

        // Clear the state to simulate a clean new segment
        {
            let mut state = speech_final_state.write();
            state.text_buffer.clear();
            state
                .waiting_for_speech_final
                .store(false, Ordering::Release);
            // Clear timer fired state to allow new timers
        }

        // Now a completely new segment starts (new is_final without speech_final)
        let new_result = STTResult::new("New segment".to_string(), true, false, 0.9);
        let processor_new = STTResultProcessor::default();
        let processed_new = processor_new
            .process_result(new_result, speech_final_state.clone(), None)
            .await;

        assert!(processed_new.is_some());
        assert_eq!(processed_new.unwrap().transcript, "New segment");

        // Verify new timer started for continuous speech
        {
            let state = speech_final_state.read();
            // New timer should be started for continuous speech
            assert!(state.waiting_for_speech_final.load(Ordering::Acquire));
            assert!(state.turn_detection_handle.is_some());
        }
    }
}

#[tokio::test]
async fn test_hard_timeout_fallback_without_turn_detector() {
    // Test that hard timeout fires when no turn detector is available
    // This is the regression test for the bug where utterances could hang indefinitely

    use crate::core::voice_manager::stt_result::STTProcessingConfig;

    let config = STTProcessingConfig {
        stt_speech_final_wait_ms: 50,
        turn_detection_inference_timeout_ms: 50,
        speech_final_hard_timeout_ms: 200, // 200ms hard timeout for faster testing
        duplicate_window_ms: 100,
        stt_ttfs_p99_ms: None,
    };

    let processor = crate::core::voice_manager::stt_result::STTResultProcessor::new(config);

    // Track callback invocations
    let callback_fired = Arc::new(AtomicBool::new(false));
    let callback_fired_clone = callback_fired.clone();

    use crate::core::voice_manager::callbacks::STTCallback;
    use std::future::Future;
    use std::pin::Pin;

    let callback: STTCallback = Arc::new(move |result: STTResult| {
        let fired = callback_fired_clone.clone();
        Box::pin(async move {
            if result.is_speech_final {
                fired.store(true, Ordering::SeqCst);
            }
        }) as Pin<Box<dyn Future<Output = ()> + Send>>
    });

    let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
        text_buffer: String::new(),
        turn_detection_handle: None,
        hard_timeout_handle: None,
        waiting_for_speech_final: AtomicBool::new(false),
        user_callback: Some(callback),
        turn_detection_last_fired_ms: AtomicUsize::new(0),
        last_forced_text: String::new(),
        segment_start_ms: AtomicUsize::new(0),
        hard_timeout_deadline_ms: AtomicUsize::new(0),
        fire_generation: AtomicUsize::new(0),
    }));

    // Send is_final result without speech_final (simulating Deepgram behavior)
    let result = STTResult::new("Hello world".to_string(), true, false, 0.95);

    // Process with NO turn detector
    let processed = processor
        .process_result(result, speech_final_state.clone(), None)
        .await;

    // Should return the result immediately
    assert!(processed.is_some());
    assert_eq!(processed.unwrap().transcript, "Hello world");

    // State should indicate we're waiting for speech_final
    {
        let state = speech_final_state.read();
        assert!(state.waiting_for_speech_final.load(Ordering::Acquire));
        assert!(state.hard_timeout_handle.is_some());
    }

    // Wait for hard timeout to fire
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // Hard timeout should have fired the callback
    assert!(
        callback_fired.load(Ordering::SeqCst),
        "Hard timeout should force speech_final after 200ms"
    );

    // State should be reset
    {
        let state = speech_final_state.read();
        assert!(!state.waiting_for_speech_final.load(Ordering::Acquire));
        assert!(state.text_buffer.is_empty());
        assert!(state.hard_timeout_handle.is_none());
    }
}

#[tokio::test]
#[ignore = "Requires turn detector model files to be downloaded"]
async fn test_hard_timeout_with_turn_detector_failure() {
    // Test that hard timeout fires even if turn detector fails

    use crate::core::turn_detect::TurnDetector;
    use crate::core::voice_manager::stt_result::STTProcessingConfig;
    use tokio::sync::RwLock;

    let config = STTProcessingConfig {
        stt_speech_final_wait_ms: 50,
        turn_detection_inference_timeout_ms: 50,
        speech_final_hard_timeout_ms: 200,
        duplicate_window_ms: 100,
        stt_ttfs_p99_ms: None,
    };

    let processor = crate::core::voice_manager::stt_result::STTResultProcessor::new(config);

    // Create a turn detector with temporary cache directory
    use crate::core::turn_detect::TurnDetectorConfig;
    let temp_dir = std::env::temp_dir().join("waav_test_turn_detect");
    let turn_config = TurnDetectorConfig {
        cache_path: Some(temp_dir),
        ..Default::default()
    };

    let turn_detector = Arc::new(RwLock::new(
        TurnDetector::with_config(turn_config)
            .await
            .expect("Failed to create turn detector"),
    ));

    let callback_fired = Arc::new(AtomicBool::new(false));
    let callback_fired_clone = callback_fired.clone();

    use crate::core::voice_manager::callbacks::STTCallback;
    use std::future::Future;
    use std::pin::Pin;

    let callback: STTCallback = Arc::new(move |result: STTResult| {
        let fired = callback_fired_clone.clone();
        Box::pin(async move {
            if result.is_speech_final {
                fired.store(true, Ordering::SeqCst);
            }
        }) as Pin<Box<dyn Future<Output = ()> + Send>>
    });

    let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
        text_buffer: String::new(),
        turn_detection_handle: None,
        hard_timeout_handle: None,
        waiting_for_speech_final: AtomicBool::new(false),
        user_callback: Some(callback),
        turn_detection_last_fired_ms: AtomicUsize::new(0),
        last_forced_text: String::new(),
        segment_start_ms: AtomicUsize::new(0),
        hard_timeout_deadline_ms: AtomicUsize::new(0),
        fire_generation: AtomicUsize::new(0),
    }));

    // Send is_final result
    let result = STTResult::new("Test utterance".to_string(), true, false, 0.95);

    // Process with turn detector that may fail or return false
    let processed = processor
        .process_result(result, speech_final_state.clone(), Some(turn_detector))
        .await;

    assert!(processed.is_some());

    // Wait for hard timeout to fire (even if turn detector fails)
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // Hard timeout should still fire regardless of turn detector behavior
    assert!(
        callback_fired.load(Ordering::SeqCst),
        "Hard timeout should fire even if turn detector fails"
    );
}

#[tokio::test]
async fn test_cancellation_cleanup_on_real_speech_final() {
    // Test that both turn_detection_handle and hard_timeout_handle are properly
    // aborted and cleared when real speech_final arrives

    use crate::core::voice_manager::stt_result::STTProcessingConfig;

    let config = STTProcessingConfig {
        stt_speech_final_wait_ms: 1000, // Long enough to not interfere
        turn_detection_inference_timeout_ms: 100,
        speech_final_hard_timeout_ms: 5000, // Long timeout
        duplicate_window_ms: 500,
        stt_ttfs_p99_ms: None,
    };

    let processor = crate::core::voice_manager::stt_result::STTResultProcessor::new(config);

    use crate::core::voice_manager::callbacks::STTCallback;
    use std::future::Future;
    use std::pin::Pin;

    let callback: STTCallback = Arc::new(move |_result: STTResult| {
        Box::pin(async move {}) as Pin<Box<dyn Future<Output = ()> + Send>>
    });

    let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
        text_buffer: String::new(),
        turn_detection_handle: None,
        hard_timeout_handle: None,
        waiting_for_speech_final: AtomicBool::new(false),
        user_callback: Some(callback),
        turn_detection_last_fired_ms: AtomicUsize::new(0),
        last_forced_text: String::new(),
        segment_start_ms: AtomicUsize::new(0),
        hard_timeout_deadline_ms: AtomicUsize::new(0),
        fire_generation: AtomicUsize::new(0),
    }));

    // Send is_final to start both timers
    let result1 = STTResult::new("Hello world".to_string(), true, false, 0.9);
    let processed1 = processor
        .process_result(result1, speech_final_state.clone(), None)
        .await;

    assert!(processed1.is_some());

    // Verify both handles were created
    {
        let state = speech_final_state.read();
        assert!(
            state.turn_detection_handle.is_some(),
            "Turn detection handle should be created"
        );
        assert!(
            state.hard_timeout_handle.is_some(),
            "Hard timeout handle should be created"
        );
        assert!(state.waiting_for_speech_final.load(Ordering::Acquire));
    }

    // Wait a bit
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Send real speech_final before any timeout fires
    let result2 = STTResult::new("Hello world complete".to_string(), true, true, 0.95);
    let processed2 = processor
        .process_result(result2, speech_final_state.clone(), None)
        .await;

    assert!(processed2.is_some());

    // Verify both handles were cleaned up
    {
        let state = speech_final_state.read();
        assert!(
            state.turn_detection_handle.is_none(),
            "Turn detection handle should be cancelled"
        );
        assert!(
            state.hard_timeout_handle.is_none(),
            "Hard timeout handle should be cancelled"
        );
        assert!(!state.waiting_for_speech_final.load(Ordering::Acquire));
        assert_eq!(state.segment_start_ms.load(Ordering::Acquire), 0);
        assert_eq!(state.hard_timeout_deadline_ms.load(Ordering::Acquire), 0);
    }
}

#[tokio::test]
async fn test_continuous_speech_hard_timeout_not_restarted() {
    // Test that hard timeout is NOT restarted when new is_final results arrive
    // during the same speech segment (person still talking)

    use crate::core::voice_manager::stt_result::STTProcessingConfig;

    let config = STTProcessingConfig {
        stt_speech_final_wait_ms: 300, // Longer than hard timeout
        turn_detection_inference_timeout_ms: 50,
        speech_final_hard_timeout_ms: 200, // Hard timeout fires first
        duplicate_window_ms: 100,
        stt_ttfs_p99_ms: None,
    };

    let processor = crate::core::voice_manager::stt_result::STTResultProcessor::new(config);

    let callback_count = Arc::new(AtomicUsize::new(0));
    let callback_count_clone = callback_count.clone();

    use crate::core::voice_manager::callbacks::STTCallback;
    use std::future::Future;
    use std::pin::Pin;

    let callback: STTCallback = Arc::new(move |result: STTResult| {
        let count = callback_count_clone.clone();
        Box::pin(async move {
            if result.is_speech_final {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }) as Pin<Box<dyn Future<Output = ()> + Send>>
    });

    let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
        text_buffer: String::new(),
        turn_detection_handle: None,
        hard_timeout_handle: None,
        waiting_for_speech_final: AtomicBool::new(false),
        user_callback: Some(callback),
        turn_detection_last_fired_ms: AtomicUsize::new(0),
        last_forced_text: String::new(),
        segment_start_ms: AtomicUsize::new(0),
        hard_timeout_deadline_ms: AtomicUsize::new(0),
        fire_generation: AtomicUsize::new(0),
    }));

    // Send first is_final at t=0
    let result1 = STTResult::new("Hello".to_string(), true, false, 0.95);
    processor
        .process_result(result1, speech_final_state.clone(), None)
        .await;

    // Record the initial deadline
    let initial_deadline = {
        let state = speech_final_state.read();
        state.hard_timeout_deadline_ms.load(Ordering::Acquire)
    };

    assert_ne!(initial_deadline, 0, "Hard timeout deadline should be set");

    // Wait 100ms and send another is_final (person still talking)
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    let result2 = STTResult::new(" world".to_string(), true, false, 0.95);
    processor
        .process_result(result2, speech_final_state.clone(), None)
        .await;

    // Verify deadline hasn't changed (hard timeout not restarted)
    let second_deadline = {
        let state = speech_final_state.read();
        state.hard_timeout_deadline_ms.load(Ordering::Acquire)
    };

    assert_eq!(
        initial_deadline, second_deadline,
        "Hard timeout deadline should NOT be restarted by new is_final"
    );

    // Wait for hard timeout to fire (should fire at t=200ms from first is_final)
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    // Hard timeout should fire exactly once
    assert_eq!(
        callback_count.load(Ordering::SeqCst),
        1,
        "Hard timeout should fire once based on first is_final timestamp"
    );
}

#[tokio::test]
async fn test_hard_timeout_observability() {
    // Test that hard timeout fires and enables observability
    // This is crucial for SREs to track fallback occurrences through logs
    //
    // Expected behavior:
    // - When hard timeout fires, it emits: tracing::warn!("Hard timeout fired after {}ms...")
    // - This log is at WARN level so it's easily alertable in production
    // - The log includes timing information for debugging
    //
    // See implementation at src/core/voice_manager/stt_result.rs:258-261

    use crate::core::voice_manager::stt_result::STTProcessingConfig;

    let config = STTProcessingConfig {
        stt_speech_final_wait_ms: 50,
        turn_detection_inference_timeout_ms: 50,
        speech_final_hard_timeout_ms: 200,
        duplicate_window_ms: 100,
        stt_ttfs_p99_ms: None,
    };

    let processor = crate::core::voice_manager::stt_result::STTResultProcessor::new(config);

    let callback_fired = Arc::new(AtomicBool::new(false));
    let callback_fired_clone = callback_fired.clone();

    use crate::core::voice_manager::callbacks::STTCallback;
    use std::future::Future;
    use std::pin::Pin;

    let callback: STTCallback = Arc::new(move |result: STTResult| {
        let fired = callback_fired_clone.clone();
        Box::pin(async move {
            if result.is_speech_final {
                fired.store(true, Ordering::SeqCst);
            }
        }) as Pin<Box<dyn Future<Output = ()> + Send>>
    });

    let speech_final_state = Arc::new(SyncRwLock::new(SpeechFinalState {
        text_buffer: String::new(),
        turn_detection_handle: None,
        hard_timeout_handle: None,
        waiting_for_speech_final: AtomicBool::new(false),
        user_callback: Some(callback),
        turn_detection_last_fired_ms: AtomicUsize::new(0),
        last_forced_text: String::new(),
        segment_start_ms: AtomicUsize::new(0),
        hard_timeout_deadline_ms: AtomicUsize::new(0),
        fire_generation: AtomicUsize::new(0),
    }));

    // Send is_final result
    let result = STTResult::new("Test message".to_string(), true, false, 0.95);
    processor
        .process_result(result, speech_final_state.clone(), None)
        .await;

    // Wait for hard timeout to fire (this emits a warning log for observability)
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // Verify the callback was fired
    assert!(
        callback_fired.load(Ordering::SeqCst),
        "Hard timeout should fire and emit warning log for SRE monitoring"
    );

    // Note: The actual log verification happens in production monitoring.
    // The implementation emits: tracing::warn!("Hard timeout fired after {}ms - forcing speech_final...")
    // This allows SREs to create alerts on fallback frequency.
}

// ---------------------------------------------------------------------------------------------
// Segmented sessions: the engine behind a real VoiceManager
// ---------------------------------------------------------------------------------------------

mod segmented_chain {
    use super::*;
    use crate::core::stt::segmented::SegmentedPlan;
    use crate::core::stt::speech_activity::SpeechActivity;
    use waav_segmented_stt::detector::{DetectorError, SpeechDetector};
    use waav_segmented_stt::engine::{EngineConfig, TokioClock};
    use waav_segmented_stt::profile::SegmentProfile;
    use waav_segmented_stt::sequencer::{SegmentUpload, UnitUpload};
    use waav_segmented_stt::transcriber::attempts::{Ledger, UploadResolution};
    use waav_segmented_stt::transcriber::{SegmentAudio, SegmentTranscript};
    use waav_segmented_stt::types::DetectorKind;

    struct Level;
    impl SpeechDetector for Level {
        fn probability(&mut self, f: &[f32]) -> Result<f32, DetectorError> {
            let rms = (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt();
            Ok(if rms > 0.02 { 0.9 } else { 0.02 })
        }
        fn reset(&mut self) {}
        fn kind(&self) -> DetectorKind {
            DetectorKind::Scripted
        }
    }

    struct Vendor {
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl SegmentUpload for Vendor {
        async fn run(&self, audio: SegmentAudio, _req: UnitUpload) -> UploadResolution {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            UploadResolution {
                result: Ok(SegmentTranscript {
                    text: format!("segment {}", n + 1),
                    ..Default::default()
                }),
                ledger: Ledger {
                    requests: 1,
                    uploaded_ms: audio.audio_ms(),
                },
                queue_wait: std::time::Duration::ZERO,
                round_trip: None,
                fatal: None,
                warnings: Vec::new(),
            }
        }
        fn deadline_ms(&self) -> u32 {
            6000
        }
        fn try_speculative(&self) -> bool {
            true
        }
        async fn prewarm(&self, _n: usize) {}
        fn min_audio_ms(&self) -> u32 {
            0
        }
    }

    fn manager(vendor: Arc<Vendor>) -> VoiceManager {
        let stt_config = STTConfig {
            provider: "elevenlabs".to_string(),
            model: "scribe_v2".into(),
            api_key: "k".to_string(),
            ..Default::default()
        };
        let tts_config = TTSConfig {
            provider: "deepgram".to_string(),
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let plan = SegmentedPlan {
            engine: EngineConfig::new(SegmentProfile::for_tests()),
            detector: Arc::new(|| {
                Box::pin(async { Ok(Box::new(Level) as Box<dyn SpeechDetector>) })
            }),
            upload: vendor,
            audio_model: None,
            text_model: None,
            clock: Arc::new(TokioClock::default()),
            provider_info: "segmented:elevenlabs_batch",
        };
        VoiceManager::new(
            VoiceManagerConfig::new(stt_config, tts_config).with_segmented(plan),
            None,
        )
        .unwrap()
    }

    async fn feed(vm: &VoiceManager, ms: u64, loud: bool) {
        for _ in 0..ms / 20 {
            let level: i16 = if loud { 4000 } else { 0 };
            let bytes: Vec<u8> = (0..320)
                .flat_map(|i| (if i % 2 == 0 { level } else { -level }).to_le_bytes())
                .collect();
            vm.receive_audio(bytes::Bytes::from(bytes)).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn transcripts_reach_the_session_callback_with_their_turn_and_speech_events_come_first() {
        let vendor = Arc::new(Vendor {
            calls: AtomicUsize::new(0),
        });
        let vm = manager(Arc::clone(&vendor));
        assert!(vm.is_gateway_endpointed());
        let results = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let r = Arc::clone(&results);
        vm.on_stt_result(move |res| {
            r.lock().push(res);
            Box::pin(async {})
        })
        .await
        .unwrap();
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let e = Arc::clone(&events);
        vm.segmented()
            .unwrap()
            .add_speech_listener(Arc::new(move |a| e.lock().push(a)));
        vm.test_connect_stt().await.unwrap();
        feed(&vm, 200, false).await;
        feed(&vm, 1000, true).await;
        assert!(results.lock().is_empty());
        assert!(
            events
                .lock()
                .iter()
                .any(|a| matches!(a, SpeechActivity::Started { .. }))
        );
        feed(&vm, 1500, false).await;
        // The upload can return just before the silence rule closes the turn, so an interim with
        // the same text may come first; exactly one final, always end of turn.
        let got = results.lock().clone();
        let finals: Vec<_> = got.iter().filter(|r| r.is_final).collect();
        assert_eq!(finals.len(), 1, "{got:?}");
        assert!(got.iter().all(|r| r.is_final == r.is_speech_final));
        assert!(finals[0].is_finalized);
        assert_eq!(finals[0].transcript, "segment 1");
        assert!(got.iter().all(|r| r.speech_turn_id == Some(1)));
        let facts = vm.live_facts().await.expect("facts");
        assert_eq!(facts.final_deadline_ms, 6000);
    }

    #[tokio::test(start_paused = true)]
    async fn speech_over_a_protected_utterance_is_not_input_and_is_never_uploaded() {
        let vendor = Arc::new(Vendor {
            calls: AtomicUsize::new(0),
        });
        let vm = manager(Arc::clone(&vendor));
        let results = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let r = Arc::clone(&results);
        vm.on_stt_result(move |res| {
            r.lock().push(res);
            Box::pin(async {})
        })
        .await
        .unwrap();
        vm.segmented().unwrap().set_gate(Arc::new(|| false));
        vm.test_connect_stt().await.unwrap();
        feed(&vm, 200, false).await;
        feed(&vm, 1000, true).await;
        feed(&vm, 1500, false).await;
        assert!(results.lock().is_empty());
        assert_eq!(vendor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn finalize_stt_flushes_without_a_reconnect_and_the_commit_is_answered() {
        let vendor = Arc::new(Vendor {
            calls: AtomicUsize::new(0),
        });
        let vm = manager(Arc::clone(&vendor));
        let results = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let r = Arc::clone(&results);
        vm.on_stt_result(move |res| {
            r.lock().push(res);
            Box::pin(async {})
        })
        .await
        .unwrap();
        vm.test_connect_stt().await.unwrap();
        feed(&vm, 200, false).await;
        feed(&vm, 400, true).await;
        let out = vm.flush_stt().await.expect("the engine flushes");
        assert!(out.result_follows && out.will_upload);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let got = results.lock().clone();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].transcript, "segment 1");
        // finalize_stt on the same session: a flush, never a disconnect.
        vm.finalize_stt().await.unwrap();
    }
}

/// Addendum B5: a buffering client (OpenAI, Groq) clears its callbacks in `disconnect`, which
/// `finalize_stt` uses to flush at every `audio_end`; and Groq's failed flush used to stop the
/// finalize before its reconnect.
mod buffering_audio_end {
    use super::*;
    use crate::core::stt::{BaseSTT, STTError, STTErrorCallback, STTResultCallback};

    /// A client that behaves as OpenAI's and Groq's do: text only at `disconnect`, which clears
    /// both callbacks; optionally a flush that fails.
    struct Buffering {
        config: STTConfig,
        connected: bool,
        buffered: usize,
        result: Option<STTResultCallback>,
        error: Option<STTErrorCallback>,
        fail_next_flush: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl BaseSTT for Buffering {
        fn new(config: STTConfig) -> Result<Self, STTError> {
            Ok(Self {
                config,
                connected: false,
                buffered: 0,
                result: None,
                error: None,
                fail_next_flush: Arc::new(AtomicBool::new(false)),
            })
        }
        async fn connect(&mut self) -> Result<(), STTError> {
            self.connected = true;
            Ok(())
        }
        async fn disconnect(&mut self) -> Result<(), STTError> {
            let failed = self.fail_next_flush.swap(false, Ordering::SeqCst);
            let outcome = if failed {
                Err(STTError::NetworkError("flush upload failed".into()))
            } else {
                if self.buffered > 0
                    && let Some(cb) = &self.result
                {
                    cb(STTResult::new(
                        format!("{} bytes", self.buffered),
                        true,
                        true,
                        0.9,
                    ))
                    .await;
                }
                Ok(())
            };
            self.buffered = 0;
            self.connected = false;
            self.result = None;
            self.error = None;
            outcome
        }
        fn is_ready(&self) -> bool {
            self.connected
        }
        async fn send_audio(&mut self, audio: bytes::Bytes) -> Result<(), STTError> {
            if !self.connected {
                return Err(STTError::ConnectionFailed("not connected".into()));
            }
            self.buffered += audio.len();
            Ok(())
        }
        async fn on_result(&mut self, cb: STTResultCallback) -> Result<(), STTError> {
            self.result = Some(cb);
            Ok(())
        }
        async fn on_error(&mut self, cb: STTErrorCallback) -> Result<(), STTError> {
            self.error = Some(cb);
            Ok(())
        }
        fn get_config(&self) -> Option<&STTConfig> {
            Some(&self.config)
        }
        async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
            self.config = config;
            Ok(())
        }
        fn get_provider_info(&self) -> &'static str {
            "buffering-test"
        }
    }

    async fn session(
        fail: Arc<AtomicBool>,
    ) -> (VoiceManager, Arc<parking_lot::Mutex<Vec<String>>>) {
        let vm = ag6_voice_manager();
        let mut stt = Buffering::new(STTConfig::default()).unwrap();
        stt.fail_next_flush = fail;
        vm.test_replace_stt(Box::new(stt)).await;
        vm.test_connect_stt().await.unwrap();
        let got = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let g = Arc::clone(&got);
        vm.on_stt_result(move |r| {
            let g = Arc::clone(&g);
            Box::pin(async move { g.lock().push(r.transcript) })
        })
        .await
        .unwrap();
        vm.on_stt_error(|_| Box::pin(async {})).await.unwrap();
        (vm, got)
    }

    async fn turn(vm: &VoiceManager, bytes: usize) {
        vm.test_send_stt(bytes::Bytes::from(vec![1u8; bytes]))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_session_on_a_buffering_model_gets_a_transcript_at_every_audio_end() {
        let (vm, got) = session(Arc::new(AtomicBool::new(false))).await;
        for (i, n) in [320usize, 640, 960].into_iter().enumerate() {
            turn(&vm, n).await;
            vm.finalize_stt().await.unwrap();
            let seen = got.lock().clone();
            assert_eq!(
                seen.len(),
                i + 1,
                "audio_end {} delivered nothing: {seen:?}",
                i + 1
            );
        }
        let seen = got.lock().clone();
        assert_eq!(seen, vec!["320 bytes", "640 bytes", "960 bytes"]);
    }

    #[tokio::test]
    async fn a_failed_flush_does_not_leave_the_session_disconnected() {
        let fail = Arc::new(AtomicBool::new(true));
        let (vm, got) = session(Arc::clone(&fail)).await;
        turn(&vm, 320).await;
        assert!(
            vm.finalize_stt().await.is_err(),
            "the failure is still reported"
        );
        turn(&vm, 640).await;
        vm.finalize_stt().await.unwrap();
        let seen = got.lock().clone();
        assert_eq!(
            seen,
            vec!["640 bytes"],
            "the next turn still gets its transcript"
        );
    }
}

/// The greeting fix (plan chapter 4, addendum A6): each utterance carries its own "may be cut".
mod greeting_fix {
    use crate::core::voice_manager::manager::per_utterance_interruptibility;
    use crate::core::voice_manager::state::{InterruptionState, now_monotonic_ms};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use std::time::Duration;

    fn state() -> InterruptionState {
        InterruptionState {
            allow_interruption: AtomicBool::new(true),
            non_interruptible_until_ms: AtomicUsize::new(0),
            current_sample_rate: AtomicU32::new(24_000),
            is_completed: AtomicBool::new(true),
            playout_end_ms: AtomicUsize::new(0),
            audio_out_ms_total: AtomicU64::new(0),
            protected_tail_until_ms: AtomicUsize::new(0),
        }
    }

    /// As `speak_with_interruption(.., false)` and the egress leave it: a protected greeting of
    /// `ms` whose audio has all arrived.
    fn protected_greeting(s: &InterruptionState, ms: usize) {
        s.allow_interruption.store(false, Ordering::Release);
        s.non_interruptible_until_ms
            .store(now_monotonic_ms(), Ordering::Release);
        s.note_protected_chunk(ms);
        s.is_completed.store(true, Ordering::Release);
    }

    #[test]
    fn today_a_reply_after_a_non_interruptible_greeting_cannot_be_cleared() {
        let s = state();
        protected_greeting(&s, 50);
        s.begin_interruptible_utterance(false);
        s.note_protected_chunk(5_000); // the reply's audio
        std::thread::sleep(Duration::from_millis(120));
        assert!(
            !s.can_interrupt(),
            "the reply's audio extended the greeting's window: barge-in cannot cut it"
        );
    }

    #[test]
    fn the_reply_can_be_cut_once_the_protected_greeting_has_played() {
        let s = state();
        protected_greeting(&s, 100);
        s.begin_interruptible_utterance(true);
        s.note_protected_chunk(5_000);
        assert!(!s.can_interrupt(), "the greeting is still playing");
        std::thread::sleep(Duration::from_millis(160));
        assert!(s.can_interrupt(), "the reply may be cut");
        // Nothing protected is left once a clear has happened.
        s.reset();
        assert!(s.can_interrupt());
        assert_eq!(s.protected_tail_until_ms.load(Ordering::Acquire), 0);
    }

    #[test]
    fn an_interruptible_utterance_after_another_changes_nothing() {
        let s = state();
        s.begin_interruptible_utterance(true);
        assert!(s.can_interrupt());
        assert_eq!(s.protected_tail_until_ms.load(Ordering::Acquire), 0);
    }

    #[test]
    fn the_setting_covers_segmented_sessions_by_default() {
        assert!(per_utterance_interruptibility(None, true));
        assert!(!per_utterance_interruptibility(None, false));
        assert!(per_utterance_interruptibility(Some("all"), false));
        assert!(!per_utterance_interruptibility(Some("off"), true));
        assert!(per_utterance_interruptibility(Some("segmented"), true));
        assert!(!per_utterance_interruptibility(Some("segmented"), false));
    }
}

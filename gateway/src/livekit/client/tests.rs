//! Unit tests for `LiveKitClient` public APIs and helpers.

use super::*;
use crate::AppError;
use tokio::time::{Duration, timeout};

fn create_test_config() -> LiveKitConfig {
    LiveKitConfig {
        url: "wss://test-server.com".to_string(),
        token: "mock-jwt-token".to_string(),
        room_name: "test-room".to_string(),
        sample_rate: 24000,
        ingress_sample_rate: 16000,
        channels: 1,
        enable_noise_filter: cfg!(feature = "noise-filter"),
        listen_participants: vec![],
    }
}

#[tokio::test]
async fn test_livekit_client_creation() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);

    assert!(!client.is_connected());
    assert!(!client.has_audio_source());
    assert!(!client.has_local_audio_track());
    assert!(!client.has_local_track_publication().await);
}

#[test]
fn test_livekit_audio_buffer_capacity_is_checked_and_capped() {
    assert_eq!(
        LiveKitClient::audio_buffer_capacity(24_000, 1),
        MIN_AUDIO_BUFFER_POOL_CAPACITY
    );
    assert_eq!(
        LiveKitClient::audio_buffer_capacity(u32::MAX, u16::MAX),
        MAX_AUDIO_BUFFER_POOL_CAPACITY
    );
}

#[test]
fn test_livekit_audio_config_validation_rejects_pathological_sample_rate() {
    let mut config = create_test_config();
    config.sample_rate = u32::MAX;

    let err = LiveKitClient::validate_audio_config(&config).unwrap_err();
    match err {
        AppError::InternalServerError(msg) => {
            assert!(msg.contains("sample_rate"));
            assert!(msg.contains("192000"));
        }
        other => panic!("expected InternalServerError, got {other:?}"),
    }
}

#[test]
fn test_livekit_audio_config_validation_rejects_zero_channels() {
    let mut config = create_test_config();
    config.channels = 0;

    let err = LiveKitClient::validate_audio_config(&config).unwrap_err();
    match err {
        AppError::InternalServerError(msg) => {
            assert!(msg.contains("channels"));
            assert!(msg.contains("1..=8"));
        }
        other => panic!("expected InternalServerError, got {other:?}"),
    }
}

#[test]
fn test_livekit_samples_per_10ms_validates_rate() {
    assert_eq!(LiveKitClient::samples_per_10ms(24_000).unwrap(), 240);
    assert!(LiveKitClient::samples_per_10ms(0).is_err());
}

#[tokio::test]
async fn test_livekit_client_clear_audio_not_connected() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);

    let result = client.clear_audio().await;
    assert!(
        result.is_err(),
        "clear_audio should fail when not connected"
    );

    if let Err(AppError::InternalServerError(msg)) = result {
        assert!(msg.contains("Not connected"));
    } else {
        panic!("Expected InternalServerError about not being connected");
    }
}

#[tokio::test]
async fn test_livekit_client_clear_audio_no_audio_source() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);

    client.set_connected(true).await;

    let result = client.clear_audio().await;
    assert!(
        result.is_ok(),
        "clear_audio should succeed when no audio source"
    );
}

#[tokio::test]
async fn test_livekit_client_callback_registration() {
    let config = create_test_config();
    let mut client = LiveKitClient::new(config);

    client.set_audio_callback(|_data| {});
    assert!(client.audio_callback.is_some());

    client.set_data_callback(|_data| {});
    assert!(client.data_callback.is_some());

    client.set_participant_disconnect_callback(|_event| {});
    assert!(client.participant_disconnect_callback.is_some());
}

#[tokio::test]
async fn test_livekit_client_audio_frame_conversion() {
    let config = create_test_config();
    let audio_data = vec![0u8, 1u8, 2u8, 3u8];
    let result =
        LiveKitClient::convert_audio_to_frame_ref(&audio_data, config.sample_rate, config.channels);

    assert!(result.is_ok());
    let audio_frame = result.unwrap();
    assert_eq!(audio_frame.sample_rate, 24000);
    assert_eq!(audio_frame.num_channels, 1);
    assert_eq!(audio_frame.samples_per_channel, 2);
}

#[tokio::test]
async fn test_livekit_client_audio_frame_conversion_invalid_data() {
    let config = create_test_config();
    let audio_data = vec![0u8, 1u8, 2u8];
    let result =
        LiveKitClient::convert_audio_to_frame_ref(&audio_data, config.sample_rate, config.channels);

    assert!(result.is_err());
    if let Err(AppError::InternalServerError(msg)) = result {
        assert!(msg.contains("even"));
    }
}

#[tokio::test]
async fn test_livekit_client_clear_audio_timing() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);
    client.set_connected(true).await;

    let result = timeout(Duration::from_millis(100), client.clear_audio()).await;

    assert!(result.is_ok(), "clear_audio should complete within 100ms");
    assert!(result.unwrap().is_ok());
}

#[tokio::test]
async fn test_livekit_client_send_tts_audio_not_connected() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);

    let audio_data = vec![0u8; 1024];
    let result = client.send_tts_audio(audio_data).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_livekit_client_send_tts_audio_no_source() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);
    client.set_connected(true).await;

    let audio_data = vec![0u8; 1024];
    let result = client.send_tts_audio(audio_data.clone()).await;

    assert!(result.is_ok());
    let queue_len = client.get_audio_queue_len().await;
    assert_eq!(queue_len, 1);
}

#[tokio::test]
async fn test_livekit_client_multiple_clear_audio_calls() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);
    client.set_connected(true).await;

    for _ in 0..3 {
        assert!(client.clear_audio().await.is_ok());
    }
}

#[tokio::test]
async fn test_livekit_client_clear_audio_state_consistency() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);
    client.set_connected(true).await;

    assert!(client.is_connected());
    assert!(!client.has_audio_source());

    let result = client.clear_audio().await;
    assert!(result.is_ok());
    assert!(client.is_connected());
}

#[tokio::test]
async fn test_livekit_client_data_message_serialization() {
    use serde_json::json;

    let config = create_test_config();
    let client = LiveKitClient::new(config);
    client.set_connected(true).await;

    let test_data = json!({
        "type": "test",
        "message": "hello world"
    });

    // With the refactored code, send_data_message returns error when no room
    let result = client.send_data_message("test-topic", test_data).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_livekit_client_disconnect_cleanup() {
    let config = create_test_config();
    let mut client = LiveKitClient::new(config);
    client.set_connected(true).await;

    assert!(client.disconnect().await.is_ok());

    assert!(!client.is_connected());
    assert!(!client.has_audio_source());
    assert!(!client.has_local_audio_track());
    assert!(!client.has_local_track_publication().await);
    assert!(!client.has_room().await);
    assert!(!client.has_room_events());
}

#[tokio::test]
async fn reconnect_event_handler_is_owned_and_disconnected() {
    struct DropFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    let config = create_test_config();
    let mut client = LiveKitClient::new(config);
    let old_dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let old_dropped_clone = old_dropped.clone();
    let (old_started_tx, old_started_rx) = tokio::sync::oneshot::channel();
    let old = tokio::spawn(async move {
        let _guard = DropFlag(old_dropped_clone);
        let _ = old_started_tx.send(());
        std::future::pending::<()>().await;
    });
    old_started_rx
        .await
        .expect("old event handler should start");
    client
        .event_handler_handle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .replace(old);

    let (_room_events_tx, room_events_rx) = tokio::sync::mpsc::unbounded_channel();
    LiveKitClient::restart_event_handler(
        room_events_rx,
        &client.audio_callback,
        &client.data_callback,
        &client.participant_disconnect_callback,
        &client.active_streams,
        &client.event_handler_handle,
        &client.is_connected,
        &client.config,
    )
    .await;

    assert!(
        old_dropped.load(std::sync::atomic::Ordering::Acquire),
        "reconnect must abort and observe the replaced room-event handler"
    );
    assert!(
        client
            .event_handler_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some(),
        "reconnect must store the replacement room-event handler for teardown"
    );

    client
        .disconnect()
        .await
        .expect("disconnect should clean up");
    assert!(
        client
            .event_handler_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none(),
        "disconnect must take and abort the current room-event handler"
    );
}

#[tokio::test]
async fn test_livekit_client_clear_audio_integration_pattern() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);
    client.set_connected(true).await;

    let result = client.clear_audio().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_livekit_client_audio_queue_drain_success() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);
    client.set_connected(true).await;

    // Queue multiple audio frames
    let audio_data = vec![0u8, 1u8, 2u8, 3u8];
    for _ in 0..3 {
        let _ = client.send_tts_audio(audio_data.clone()).await;
    }

    // Verify frames were queued
    let queue_len = client.get_audio_queue_len().await;
    assert_eq!(queue_len, 3);

    // Clear the queue
    let result = client.clear_audio().await;
    assert!(result.is_ok());
    assert_eq!(client.get_audio_queue_len().await, 0);
}

#[tokio::test]
async fn test_livekit_client_message_send_with_serializable_struct() {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct TestMessage {
        id: u32,
        text: String,
    }

    let config = create_test_config();
    let client = LiveKitClient::new(config);
    client.set_connected(true).await;

    let msg = TestMessage {
        id: 42,
        text: "test".to_string(),
    };

    // Should succeed with serialization but fail due to no room
    let result = client.send_data_message("test-topic", msg).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_livekit_client_reconnect_audio_queue_preservation() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);
    client.set_connected(true).await;

    // Queue audio data
    let audio_data = vec![0u8; 48];
    let _ = client.send_tts_audio(audio_data.clone()).await;
    assert_eq!(client.get_audio_queue_len().await, 1);

    // Audio should remain queued after status changes
    client.set_connected(false).await;
    assert_eq!(client.get_audio_queue_len().await, 1);

    client.set_connected(true).await;
    assert_eq!(client.get_audio_queue_len().await, 1);
}

#[tokio::test]
async fn test_livekit_client_operation_priority_ordering() {
    let config = create_test_config();
    let client = LiveKitClient::new(config);

    // Get operation queue and verify it's available
    let queue = client.get_operation_queue();
    assert!(queue.is_none()); // Queue only exists after connect
}

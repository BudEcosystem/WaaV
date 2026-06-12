use base64::{Engine as _, engine::general_purpose};
use bytes::Bytes;

use super::config::{LiveKitWebSocketConfig, STTWebSocketConfig, TTSWebSocketConfig};
use super::messages::{
    IncomingMessage, MessageRoute, OutgoingMessage, ParticipantDisconnectedInfo, UnifiedMessage,
};

#[test]
fn test_ws_config_serialization() {
    // Test STT WebSocket config
    let stt_ws_config = STTWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "nova-3".to_string(),
        features: Default::default(),
        extras: Default::default(),
            turn_detection: None,
    };

    let json = serde_json::to_string(&stt_ws_config).unwrap();
    assert!(json.contains("\"provider\":\"deepgram\""));
    assert!(json.contains("\"language\":\"en-US\""));
    assert!(!json.contains("api_key")); // Should not contain API key

    // Test TTS WebSocket config
    let tts_ws_config = TTSWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        voice_id: Some("aura-luna-en".to_string()),
        speaking_rate: Some(1.0),
        audio_format: Some("pcm".to_string()),
        sample_rate: Some(22050),
        client_playback_rate: None,
        connection_timeout: Some(30),
        request_timeout: Some(60),
        model: "".to_string(), // Model is in Voice ID for Deepgram
        pronunciations: Vec::new(),
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        features: Default::default(),
        extras: Default::default(),
    };

    let json = serde_json::to_string(&tts_ws_config).unwrap();
    assert!(json.contains("\"provider\":\"deepgram\""));
    assert!(json.contains("\"voice_id\":\"aura-luna-en\""));
    assert!(!json.contains("api_key")); // Should not contain API key
}

#[test]
fn test_incoming_message_serialization() {
    // Test config message
    let config_msg = IncomingMessage::Config {
        stream_id: None,
        audio: Some(true),
        audio_disabled: None,
        stt_config: Some(STTWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "nova-3".to_string(),
            features: Default::default(),
            extras: Default::default(),
            turn_detection: None,
        }),
        tts_config: Some(TTSWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            voice_id: Some("aura-luna-en".to_string()),
            speaking_rate: Some(1.0),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(22050),
            client_playback_rate: None,
            connection_timeout: Some(30),
            request_timeout: Some(60),
            model: "".to_string(), // Model is in Voice ID for Deepgram
            pronunciations: Vec::new(),
            emotion: None,
            emotion_intensity: None,
            delivery_style: None,
            emotion_description: None,
            features: Default::default(),
            extras: Default::default(),
        }),
        livekit: None,
        dag_config: None,
        conversation_config: None,
    };

    let json = serde_json::to_string(&config_msg).unwrap();
    assert!(json.contains("\"type\":\"config\""));
    assert!(!json.contains("api_key")); // Should not contain API key

    // Test speak message
    let speak_msg = IncomingMessage::Speak {
        text: "Hello world".to_string(),
        flush: Some(true),
        allow_interruption: Some(true),
    };

    let json = serde_json::to_string(&speak_msg).unwrap();
    assert!(json.contains("\"type\":\"speak\""));
    assert!(json.contains("Hello world"));
    assert!(json.contains("\"flush\":true"));

    // Test speak message without flush (backward compatibility)
    let speak_msg_no_flush = IncomingMessage::Speak {
        text: "Hello world".to_string(),
        flush: None,
        allow_interruption: None,
    };

    let json = serde_json::to_string(&speak_msg_no_flush).unwrap();
    assert!(json.contains("\"type\":\"speak\""));
    assert!(json.contains("Hello world"));
    // Should not contain flush when None
    assert!(!json.contains("flush"));

    // Test parsing message without flush field (backward compatibility)
    let json_without_flush = r#"{"type":"speak","text":"Hello world"}"#;
    let parsed: IncomingMessage = serde_json::from_str(json_without_flush).unwrap();
    if let IncomingMessage::Speak {
        text,
        flush,
        allow_interruption,
    } = parsed
    {
        assert_eq!(text, "Hello world");
        assert_eq!(flush, None);
        assert_eq!(allow_interruption, Some(true)); // Defaults to true
    } else {
        panic!("Expected Speak message");
    }

    // Test speak message with allow_interruption=false
    let speak_msg_no_interruption = IncomingMessage::Speak {
        text: "Do not interrupt me".to_string(),
        flush: Some(true),
        allow_interruption: Some(false),
    };

    let json = serde_json::to_string(&speak_msg_no_interruption).unwrap();
    assert!(json.contains("\"type\":\"speak\""));
    assert!(json.contains("Do not interrupt me"));
    assert!(json.contains("\"allow_interruption\":false"));

    // Test parsing message with allow_interruption field
    let json_with_interruption = r#"{"type":"speak","text":"Hello","allow_interruption":false}"#;
    let parsed: IncomingMessage = serde_json::from_str(json_with_interruption).unwrap();
    if let IncomingMessage::Speak {
        text,
        flush,
        allow_interruption,
    } = parsed
    {
        assert_eq!(text, "Hello");
        assert_eq!(flush, None);
        assert_eq!(allow_interruption, Some(false));
    } else {
        panic!("Expected Speak message");
    }

    // Test flush message
    let clear_msg = IncomingMessage::Clear;
    let json = serde_json::to_string(&clear_msg).unwrap();
    assert!(json.contains("\"type\":\"clear\""));

    // Test send_message message with topic
    let send_msg = IncomingMessage::SendMessage {
        message: "Hello from client!".to_string(),
        role: "user".to_string(),
        topic: Some("chat".to_string()),
        debug: None,
    };
    let json = serde_json::to_string(&send_msg).unwrap();
    assert!(json.contains("\"type\":\"send_message\""));
    assert!(json.contains("\"message\":\"Hello from client!\""));
    assert!(json.contains("\"role\":\"user\""));
    assert!(json.contains("\"topic\":\"chat\""));

    // Test send_message message without topic
    let send_msg_no_topic = IncomingMessage::SendMessage {
        message: "Hello without topic!".to_string(),
        role: "user".to_string(),
        topic: None,
        debug: None,
    };
    let json = serde_json::to_string(&send_msg_no_topic).unwrap();
    assert!(json.contains("\"type\":\"send_message\""));
    assert!(json.contains("\"message\":\"Hello without topic!\""));
    assert!(json.contains("\"role\":\"user\""));
    // Should not contain topic field when None (but may contain the word "topic" elsewhere)
    assert!(!json.contains("\"topic\""));

    // Test parsing send_message JSON with topic
    let json_with_topic =
        r#"{"type":"send_message","message":"Hello from JSON!","role":"user","topic":"general"}"#;
    let parsed: IncomingMessage = serde_json::from_str(json_with_topic).unwrap();
    if let IncomingMessage::SendMessage {
        message,
        role,
        topic,
        ..
    } = parsed
    {
        assert_eq!(message, "Hello from JSON!");
        assert_eq!(role, "user");
        assert_eq!(topic, Some("general".to_string()));
    } else {
        panic!("Expected SendMessage message");
    }

    // Test parsing send_message JSON without topic
    let json_without_topic =
        r#"{"type":"send_message","message":"Hello without topic!","role":"user"}"#;
    let parsed: IncomingMessage = serde_json::from_str(json_without_topic).unwrap();
    if let IncomingMessage::SendMessage {
        message,
        role,
        topic,
        ..
    } = parsed
    {
        assert_eq!(message, "Hello without topic!");
        assert_eq!(role, "user");
        assert_eq!(topic, None);
    } else {
        panic!("Expected SendMessage message");
    }

    // Test send_message with different role
    let send_msg_system = IncomingMessage::SendMessage {
        message: "System notification".to_string(),
        role: "system".to_string(),
        topic: Some("notifications".to_string()),
        debug: None,
    };
    let json = serde_json::to_string(&send_msg_system).unwrap();
    assert!(json.contains("\"type\":\"send_message\""));
    assert!(json.contains("\"message\":\"System notification\""));
    assert!(json.contains("\"role\":\"system\""));
    assert!(json.contains("\"topic\":\"notifications\""));

    // Test send_message with debug field
    let send_msg_with_debug = IncomingMessage::SendMessage {
        message: "Debug message".to_string(),
        role: "user".to_string(),
        topic: None,
        debug: Some(serde_json::json!({
            "request_id": "123",
            "metadata": {
                "source": "test",
                "timestamp": 1234567890
            }
        })),
    };
    let json = serde_json::to_string(&send_msg_with_debug).unwrap();
    assert!(json.contains("\"type\":\"send_message\""));
    assert!(json.contains("\"message\":\"Debug message\""));
    assert!(json.contains("\"debug\""));
    assert!(json.contains("\"request_id\":\"123\""));
    assert!(json.contains("\"source\":\"test\""));

    // Test parsing send_message with debug field
    let json_with_debug = r#"{"type":"send_message","message":"Test","role":"user","debug":{"foo":"bar","nested":{"value":42}}}"#;
    let parsed: IncomingMessage = serde_json::from_str(json_with_debug).unwrap();
    if let IncomingMessage::SendMessage {
        message,
        role,
        debug,
        ..
    } = parsed
    {
        assert_eq!(message, "Test");
        assert_eq!(role, "user");
        assert!(debug.is_some());
        let debug_val = debug.unwrap();
        assert_eq!(debug_val["foo"], "bar");
        assert_eq!(debug_val["nested"]["value"], 42);
    } else {
        panic!("Expected SendMessage message");
    }
}

#[test]
fn test_outgoing_message_serialization() {
    // Test ready message without LiveKit
    let ready_msg = OutgoingMessage::Ready {
        protocol_version: crate::handlers::ws::messages::PROTOCOL_VERSION.to_string(),
        stream_id: "test-stream".to_string(),
        livekit_room_name: None,
        livekit_url: None,
        waav_participant_identity: None,
        waav_participant_name: None,
    };
    let json = serde_json::to_string(&ready_msg).unwrap();
    assert!(json.contains("\"type\":\"ready\""));

    // Test ready message with LiveKit room info
    let ready_msg_with_livekit = OutgoingMessage::Ready {
        protocol_version: crate::handlers::ws::messages::PROTOCOL_VERSION.to_string(),
        stream_id: "test-stream".to_string(),
        livekit_room_name: Some("test-room".to_string()),
        livekit_url: Some("ws://localhost:7880".to_string()),
        waav_participant_identity: Some("waav-ai".to_string()),
        waav_participant_name: Some("WaaV AI".to_string()),
    };
    let json_with_livekit = serde_json::to_string(&ready_msg_with_livekit).unwrap();
    assert!(json_with_livekit.contains("\"type\":\"ready\""));
    assert!(json_with_livekit.contains("\"livekit_room_name\":\"test-room\""));
    assert!(json_with_livekit.contains("\"livekit_url\":\"ws://localhost:7880\""));
    assert!(json_with_livekit.contains("\"waav_participant_identity\":\"waav-ai\""));
    assert!(json_with_livekit.contains("\"waav_participant_name\":\"WaaV AI\""));

    // Test STT result message
    let stt_msg = OutgoingMessage::STTResult {
        transcript: "Hello world".to_string(),
        is_final: true,
        is_speech_final: true,
        confidence: 0.95,
            segment_transcript: None,
    };

    let json = serde_json::to_string(&stt_msg).unwrap();
    assert!(json.contains("\"type\":\"stt_result\""));
    assert!(json.contains("Hello world"));
    assert!(json.contains("0.95"));

    // Test error message
    let error_msg = OutgoingMessage::Error {
        message: "Test error".to_string(),
    };

    let json = serde_json::to_string(&error_msg).unwrap();
    assert!(json.contains("\"type\":\"error\""));
    assert!(json.contains("Test error"));
}

#[test]
fn test_binary_audio_handling() {
    // Test that binary audio data is handled directly as bytes
    // without JSON serialization/deserialization
    let audio_data = vec![1, 2, 3, 4, 5];
    let bytes_data = Bytes::from(audio_data.clone());

    // Binary audio messages are now handled directly
    // No JSON serialization involved for better performance
    assert_eq!(bytes_data.to_vec(), audio_data);
    assert_eq!(bytes_data.len(), 5);
}

#[test]
fn test_stt_ws_config_conversion() {
    let stt_ws_config = STTWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: "nova-3".to_string(),
        features: Default::default(),
        extras: Default::default(),
            turn_detection: None,
    };

    let api_key = "test_api_key".to_string();
    let stt_config = stt_ws_config.to_stt_config(api_key.clone());

    assert_eq!(stt_config.provider, "deepgram");
    assert_eq!(stt_config.api_key, api_key);
    assert_eq!(stt_config.language, "en-US");
    assert_eq!(stt_config.sample_rate, 16000);
    assert_eq!(stt_config.channels, 1);
    assert!(stt_config.punctuation);
}

#[test]
fn test_tts_ws_config_conversion_with_all_values() {
    let tts_ws_config = TTSWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        voice_id: Some("custom-voice".to_string()),
        speaking_rate: Some(1.5),
        audio_format: Some("wav".to_string()),
        sample_rate: Some(22050),
        client_playback_rate: None,
        connection_timeout: Some(60),
        request_timeout: Some(120),
        model: "".to_string(), // Model is in Voice ID for Deepgram
        pronunciations: Vec::new(),
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        features: Default::default(),
        extras: Default::default(),
    };

    let api_key = "test_api_key".to_string();
    let tts_config = tts_ws_config.to_tts_config(api_key.clone());

    assert_eq!(tts_config.provider, "deepgram");
    assert_eq!(tts_config.api_key, api_key);
    assert_eq!(tts_config.voice_id, Some("custom-voice".to_string()));
    assert_eq!(tts_config.speaking_rate, Some(1.5));
    assert_eq!(tts_config.audio_format, Some("wav".to_string()));
    assert_eq!(tts_config.sample_rate, Some(22050));
    assert_eq!(tts_config.connection_timeout, Some(60));
    assert_eq!(tts_config.request_timeout, Some(120));
}

#[test]
fn test_tts_ws_config_conversion_with_defaults() {
    let tts_ws_config = TTSWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        voice_id: None,
        speaking_rate: None,
        audio_format: None,
        sample_rate: None,
        client_playback_rate: None,
        connection_timeout: None,
        request_timeout: None,
        model: "".to_string(), // Model is in Voice ID for Deepgram
        pronunciations: Vec::new(),
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        features: Default::default(),
        extras: Default::default(),
    };

    let api_key = "test_api_key".to_string();
    let tts_config = tts_ws_config.to_tts_config(api_key.clone());

    assert_eq!(tts_config.provider, "deepgram");
    assert_eq!(tts_config.api_key, api_key);

    // Should use default values
    assert_eq!(tts_config.voice_id, Some("aura-asteria-en".to_string()));
    assert_eq!(tts_config.speaking_rate, Some(1.0));
    assert_eq!(tts_config.audio_format, Some("linear16".to_string()));
    assert_eq!(tts_config.sample_rate, Some(24000));
    assert_eq!(tts_config.connection_timeout, Some(30));
    assert_eq!(tts_config.request_timeout, Some(60));
}

#[test]
fn test_livekit_ws_config_serialization() {
    let livekit_config = LiveKitWebSocketConfig {
        room_name: "test-room".to_string(),
        enable_recording: true,
        waav_participant_identity: Some("waav-ai".to_string()),
        waav_participant_name: Some("WaaV AI".to_string()),
        listen_participants: vec![],
    };

    let json = serde_json::to_string(&livekit_config).unwrap();
    assert!(json.contains("\"room_name\":\"test-room\""));
    assert!(json.contains("\"enable_recording\":true"));
    assert!(
        !json.contains("recording_file_key"),
        "recording_file_key should not be serialized"
    );
}

#[test]
fn test_livekit_ws_config_conversion() {
    let livekit_ws_config = LiveKitWebSocketConfig {
        room_name: "test-room".to_string(),
        enable_recording: false,
        waav_participant_identity: None,
        waav_participant_name: None,
        listen_participants: vec![],
    };

    let tts_ws_config = TTSWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        voice_id: Some("aura-luna-en".to_string()),
        speaking_rate: Some(1.0),
        audio_format: Some("pcm".to_string()),
        sample_rate: Some(22050),
        client_playback_rate: None,
        connection_timeout: Some(30),
        request_timeout: Some(60),
        model: "".to_string(),
        pronunciations: Vec::new(),
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        features: Default::default(),
        extras: Default::default(),
    };

    let livekit_url = "wss://test-livekit.com".to_string();
    let test_token = "test-jwt-token".to_string();
    let livekit_config =
        livekit_ws_config.to_livekit_config(test_token.clone(), &tts_ws_config, 16_000, &livekit_url);
    assert_eq!(livekit_config.url, "wss://test-livekit.com");
    assert_eq!(livekit_config.token, test_token);
    assert_eq!(livekit_config.room_name, "test-room");
    assert_eq!(livekit_config.sample_rate, 22050);
    assert_eq!(livekit_config.channels, 1);
    assert_eq!(
        livekit_config.enable_noise_filter,
        cfg!(feature = "noise-filter")
    );
}

#[test]
fn test_livekit_config_with_empty_listen_participants() {
    let livekit_ws_config = LiveKitWebSocketConfig {
        room_name: "test-room".to_string(),
        enable_recording: false,
        waav_participant_identity: None,
        waav_participant_name: None,
        listen_participants: vec![],
    };

    let tts_ws_config = TTSWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        voice_id: Some("aura-luna-en".to_string()),
        speaking_rate: Some(1.0),
        audio_format: Some("pcm".to_string()),
        sample_rate: Some(22050),
        client_playback_rate: None,
        connection_timeout: Some(30),
        request_timeout: Some(60),
        model: "".to_string(),
        pronunciations: Vec::new(),
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        features: Default::default(),
        extras: Default::default(),
    };

    let livekit_config = livekit_ws_config.to_livekit_config(
        "test-token".to_string(),
        &tts_ws_config,
        16_000,
        "wss://test.com",
    );

    assert!(
        livekit_config.listen_participants.is_empty(),
        "Empty listen_participants should be preserved"
    );
}

#[test]
fn test_livekit_config_with_listen_participants() {
    let livekit_ws_config = LiveKitWebSocketConfig {
        room_name: "test-room".to_string(),
        enable_recording: false,
        waav_participant_identity: None,
        waav_participant_name: None,
        listen_participants: vec!["user-123".to_string(), "user-456".to_string()],
    };

    let tts_ws_config = TTSWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        voice_id: Some("aura-luna-en".to_string()),
        speaking_rate: Some(1.0),
        audio_format: Some("pcm".to_string()),
        sample_rate: Some(22050),
        client_playback_rate: None,
        connection_timeout: Some(30),
        request_timeout: Some(60),
        model: "".to_string(),
        pronunciations: Vec::new(),
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        features: Default::default(),
        extras: Default::default(),
    };

    let livekit_config = livekit_ws_config.to_livekit_config(
        "test-token".to_string(),
        &tts_ws_config,
        16_000,
        "wss://test.com",
    );

    assert_eq!(
        livekit_config.listen_participants.len(),
        2,
        "listen_participants should be preserved"
    );
    assert!(
        livekit_config
            .listen_participants
            .contains(&"user-123".to_string())
    );
    assert!(
        livekit_config
            .listen_participants
            .contains(&"user-456".to_string())
    );
}

#[test]
fn test_livekit_ws_config_serialization_with_listen_participants() {
    let config = LiveKitWebSocketConfig {
        room_name: "test-room".to_string(),
        enable_recording: false,
        waav_participant_identity: None,
        waav_participant_name: None,
        listen_participants: vec!["user-1".to_string(), "user-2".to_string()],
    };

    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("\"listen_participants\":[\"user-1\",\"user-2\"]"));
}

#[test]
fn test_livekit_ws_config_serialization_omits_empty_listen_participants() {
    let config = LiveKitWebSocketConfig {
        room_name: "test-room".to_string(),
        enable_recording: false,
        waav_participant_identity: None,
        waav_participant_name: None,
        listen_participants: vec![],
    };

    let json = serde_json::to_string(&config).unwrap();
    // Should not include listen_participants when empty (skip_serializing_if)
    assert!(!json.contains("listen_participants"));
}

#[test]
fn test_livekit_ws_config_deserialization_with_listen_participants() {
    let json = r#"{
        "room_name": "test-room",
        "enable_recording": false,
        "listen_participants": ["user-1", "user-2"]
    }"#;

    let config: LiveKitWebSocketConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.listen_participants.len(), 2);
    assert!(config.listen_participants.contains(&"user-1".to_string()));
    assert!(config.listen_participants.contains(&"user-2".to_string()));
}

#[test]
fn test_livekit_ws_config_deserialization_without_listen_participants() {
    let json = r#"{
        "room_name": "test-room",
        "enable_recording": false
    }"#;

    let config: LiveKitWebSocketConfig = serde_json::from_str(json).unwrap();
    // Should default to empty vec
    assert!(config.listen_participants.is_empty());
}

#[test]
fn test_livekit_config_ignores_unknown_fields() {
    let json = r#"{
        "room_name": "test-room",
        "enable_recording": true,
        "recording_file_key": "legacy-key"
    }"#;

    let config: LiveKitWebSocketConfig =
        serde_json::from_str(json).expect("Should parse even with legacy recording_file_key");

    assert_eq!(config.room_name, "test-room");
    assert!(config.enable_recording);
}

#[test]
fn test_incoming_message_config_with_livekit() {
    let config_msg = IncomingMessage::Config {
        stream_id: None,
        audio: Some(true),
        audio_disabled: None,
        stt_config: Some(STTWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "nova-3".to_string(),
            features: Default::default(),
            extras: Default::default(),
            turn_detection: None,
        }),
        tts_config: Some(TTSWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            voice_id: Some("aura-luna-en".to_string()),
            speaking_rate: Some(1.0),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(22050),
            client_playback_rate: None,
            connection_timeout: Some(30),
            request_timeout: Some(60),
            model: "".to_string(),
            pronunciations: Vec::new(),
            emotion: None,
            emotion_intensity: None,
            delivery_style: None,
            emotion_description: None,
            features: Default::default(),
            extras: Default::default(),
        }),
        livekit: Some(LiveKitWebSocketConfig {
            room_name: "test-room".to_string(),
            enable_recording: true,
            waav_participant_identity: None,
            waav_participant_name: None,
            listen_participants: vec![],
        }),
        dag_config: None,
        conversation_config: None,
    };

    let json = serde_json::to_string(&config_msg).unwrap();
    assert!(json.contains("\"type\":\"config\""));
    assert!(json.contains("\"room_name\":\"test-room\""));
    assert!(json.contains("\"livekit\"")); // Verify LiveKit section is present
}

#[test]
fn test_incoming_message_config_without_livekit() {
    let config_msg = IncomingMessage::Config {
        stream_id: None,
        audio: Some(true),
        audio_disabled: None,
        stt_config: Some(STTWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "nova-3".to_string(),
            features: Default::default(),
            extras: Default::default(),
            turn_detection: None,
        }),
        tts_config: Some(TTSWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            voice_id: Some("aura-luna-en".to_string()),
            speaking_rate: Some(1.0),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(22050),
            client_playback_rate: None,
            connection_timeout: Some(30),
            request_timeout: Some(60),
            model: "".to_string(),
            pronunciations: Vec::new(),
            emotion: None,
            emotion_intensity: None,
            delivery_style: None,
            emotion_description: None,
            features: Default::default(),
            extras: Default::default(),
        }),
        livekit: None,
        dag_config: None,
        conversation_config: None,
    };

    let json = serde_json::to_string(&config_msg).unwrap();
    assert!(json.contains("\"type\":\"config\""));
    // Should not contain livekit field when None
    assert!(!json.contains("livekit"));
}

#[test]
fn test_parse_config_message_with_livekit() {
    let json = r#"{
            "type": "config",
            "stt_config": {
                "provider": "deepgram",
                "language": "en-US",
                "sample_rate": 16000,
                "channels": 1,
                "punctuation": true,
                "encoding": "linear16",
                "model": "nova-3"
            },
            "tts_config": {
                "provider": "deepgram",
                "voice_id": "aura-luna-en",
                "speaking_rate": 1.0,
                "audio_format": "pcm",
                "sample_rate": 22050,
                "connection_timeout": 30,
                "request_timeout": 60,
                "model": ""
            },
            "livekit": {
                "room_name": "test-room",
                "enable_recording": true
            }
        }"#;

    let parsed: IncomingMessage = serde_json::from_str(json).unwrap();
    if let IncomingMessage::Config {
        stream_id: None,
        audio,
        stt_config,
        tts_config,
        livekit,
        ..
    } = parsed
    {
        assert_eq!(audio, Some(true));
        assert_eq!(stt_config.as_ref().unwrap().provider, "deepgram");
        assert_eq!(tts_config.as_ref().unwrap().provider, "deepgram");

        let livekit_config = livekit.unwrap();
        assert_eq!(livekit_config.room_name, "test-room");
        assert!(livekit_config.enable_recording);
    } else {
        panic!("Expected Config message");
    }
}

#[test]
fn test_unified_message_text_serialization() {
    let unified_msg = UnifiedMessage {
        message: Some("Hello from LiveKit!".to_string()),
        data: None,
        identity: "participant123".to_string(),
        topic: "chat".to_string(),
        room: "test-room".to_string(),
        timestamp: 1234567890,
    };

    let outgoing_msg = OutgoingMessage::Message {
        message: unified_msg,
    };

    let json = serde_json::to_string(&outgoing_msg).unwrap();
    assert!(json.contains("\"type\":\"message\""));
    assert!(json.contains("\"identity\":\"participant123\""));
    assert!(json.contains("\"message\":\"Hello from LiveKit!\""));
    assert!(json.contains("\"topic\":\"chat\""));
    assert!(json.contains("\"room\":\"test-room\""));
    assert!(json.contains("\"timestamp\":1234567890"));
}

#[test]
fn test_unified_message_data_serialization() {
    let test_data = vec![1, 2, 3, 4, 5];
    let unified_msg = UnifiedMessage {
        message: None,
        data: Some(general_purpose::STANDARD.encode(&test_data)),
        identity: "participant123".to_string(),
        topic: "files".to_string(),
        room: "test-room".to_string(),
        timestamp: 1234567890,
    };

    let outgoing_msg = OutgoingMessage::Message {
        message: unified_msg,
    };

    let json = serde_json::to_string(&outgoing_msg).unwrap();
    assert!(json.contains("\"type\":\"message\""));
    assert!(json.contains("\"identity\":\"participant123\""));
    assert!(json.contains("\"topic\":\"files\""));
    assert!(json.contains("\"room\":\"test-room\""));
    // Data should be present, message field should not be present (due to serde skip_serializing_if)
    assert!(json.contains("\"data\":"));
    assert!(json.contains("\"timestamp\":1234567890"));
    // message field should not be present (None value with skip_serializing_if)
    assert!(!json.contains("\"message\":null"));
}

#[test]
fn test_parse_config_message_without_livekit() {
    let json = r#"{
            "type": "config",
            "stt_config": {
                "provider": "deepgram",
                "language": "en-US",
                "sample_rate": 16000,
                "channels": 1,
                "punctuation": true,
                "encoding": "linear16",
                "model": "nova-3"
            },
            "tts_config": {
                "provider": "deepgram",
                "voice_id": "aura-luna-en",
                "speaking_rate": 1.0,
                "audio_format": "pcm",
                "sample_rate": 22050,
                "connection_timeout": 30,
                "request_timeout": 60,
                "model": ""
            }
        }"#;

    let parsed: IncomingMessage = serde_json::from_str(json).unwrap();
    if let IncomingMessage::Config {
        stream_id: None,
        audio,
        stt_config,
        tts_config,
        livekit,
        ..
    } = parsed
    {
        assert_eq!(audio, Some(true));
        assert_eq!(stt_config.as_ref().unwrap().provider, "deepgram");
        assert_eq!(tts_config.as_ref().unwrap().provider, "deepgram");
        assert!(livekit.is_none());
    } else {
        panic!("Expected Config message");
    }
}

#[test]
fn test_parse_config_with_stream_id() {
    let json = r#"{
        "type": "config",
        "stream_id": "test-123",
        "audio": true,
        "stt_config": {
            "provider": "deepgram",
            "language": "en-US",
            "sample_rate": 16000,
            "channels": 1,
            "punctuation": true,
            "encoding": "linear16",
            "model": "nova-2"
        }
    }"#;

    let msg: IncomingMessage = serde_json::from_str(json).expect("Should parse");

    match msg {
        IncomingMessage::Config {
            stream_id, audio, ..
        } => {
            assert_eq!(stream_id, Some("test-123".to_string()));
            assert_eq!(audio, Some(true));
        }
        _ => panic!("Expected Config message"),
    }
}

#[test]
fn test_parse_config_without_stream_id() {
    let json = r#"{
        "type": "config",
        "audio": true
    }"#;

    let msg: IncomingMessage = serde_json::from_str(json).expect("Should parse");

    match msg {
        IncomingMessage::Config { stream_id, .. } => {
            assert!(
                stream_id.is_none(),
                "stream_id should be None when not provided"
            );
        }
        _ => panic!("Expected Config message"),
    }
}

#[test]
fn test_parse_config_with_null_stream_id() {
    let json = r#"{
        "type": "config",
        "stream_id": null,
        "audio": true
    }"#;

    let msg: IncomingMessage = serde_json::from_str(json).expect("Should parse");

    match msg {
        IncomingMessage::Config { stream_id, .. } => {
            assert!(stream_id.is_none(), "stream_id should be None when null");
        }
        _ => panic!("Expected Config message"),
    }
}

#[test]
fn test_parse_config_with_uuid_stream_id() {
    let json = r#"{
        "type": "config",
        "stream_id": "550e8400-e29b-41d4-a716-446655440000",
        "audio": false
    }"#;

    let msg: IncomingMessage = serde_json::from_str(json).expect("Should parse");

    match msg {
        IncomingMessage::Config { stream_id, .. } => {
            assert_eq!(
                stream_id,
                Some("550e8400-e29b-41d4-a716-446655440000".to_string())
            );
        }
        _ => panic!("Expected Config message"),
    }
}

#[test]
fn test_tts_ws_config_conversion_mixed_values() {
    let tts_ws_config = TTSWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        voice_id: Some("custom-voice".to_string()),
        speaking_rate: None, // Should use default
        audio_format: Some("pcm".to_string()),
        sample_rate: None, // Should use default
        client_playback_rate: None,
        connection_timeout: Some(45),
        request_timeout: None, // Should use default
        model: "".to_string(), // Model is in Voice ID for Deepgram
        pronunciations: Vec::new(),
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        features: Default::default(),
        extras: Default::default(),
    };

    let api_key = "test_api_key".to_string();
    let tts_config = tts_ws_config.to_tts_config(api_key.clone());

    assert_eq!(tts_config.provider, "deepgram");
    assert_eq!(tts_config.api_key, api_key);
    assert_eq!(tts_config.voice_id, Some("custom-voice".to_string()));
    assert_eq!(tts_config.speaking_rate, Some(1.0)); // Default
    assert_eq!(tts_config.audio_format, Some("pcm".to_string()));
    assert_eq!(tts_config.sample_rate, Some(24000)); // Default
    assert_eq!(tts_config.connection_timeout, Some(45));
    assert_eq!(tts_config.request_timeout, Some(60)); // Default
}

#[test]
fn test_config_message_without_livekit_routing() {
    // Test that configuration without LiveKit creates proper routing logic
    let config_msg = IncomingMessage::Config {
        stream_id: None,
        audio: Some(true),
        audio_disabled: None,
        stt_config: Some(STTWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "nova-3".to_string(),
            features: Default::default(),
            extras: Default::default(),
            turn_detection: None,
        }),
        tts_config: Some(TTSWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            voice_id: Some("aura-luna-en".to_string()),
            speaking_rate: Some(1.0),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(22050),
            client_playback_rate: None,
            connection_timeout: Some(30),
            request_timeout: Some(60),
            model: "".to_string(),
            pronunciations: Vec::new(),
            emotion: None,
            emotion_intensity: None,
            delivery_style: None,
            emotion_description: None,
            features: Default::default(),
            extras: Default::default(),
        }),
        livekit: None, // No LiveKit configuration
        dag_config: None,
        conversation_config: None,
    };

    let json = serde_json::to_string(&config_msg).unwrap();
    assert!(json.contains("\"type\":\"config\""));
    assert!(!json.contains("livekit")); // Should not contain LiveKit field

    // Parse back to ensure structure is correct
    let parsed: IncomingMessage = serde_json::from_str(&json).unwrap();
    if let IncomingMessage::Config { livekit, .. } = parsed {
        assert!(
            livekit.is_none(),
            "LiveKit should be None when not configured"
        );
    } else {
        panic!("Expected Config message");
    }
}

#[test]
fn test_config_message_with_livekit_routing() {
    // Test that configuration with LiveKit creates proper routing logic
    let config_msg = IncomingMessage::Config {
        stream_id: None,
        audio: Some(true),
        audio_disabled: None,
        stt_config: Some(STTWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "nova-3".to_string(),
            features: Default::default(),
            extras: Default::default(),
            turn_detection: None,
        }),
        tts_config: Some(TTSWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            voice_id: Some("aura-luna-en".to_string()),
            speaking_rate: Some(1.0),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(22050),
            client_playback_rate: None,
            connection_timeout: Some(30),
            request_timeout: Some(60),
            model: "".to_string(),
            pronunciations: Vec::new(),
            emotion: None,
            emotion_intensity: None,
            delivery_style: None,
            emotion_description: None,
            features: Default::default(),
            extras: Default::default(),
        }),
        livekit: Some(LiveKitWebSocketConfig {
            room_name: "test-room".to_string(),
            enable_recording: false,
            waav_participant_identity: None,
            waav_participant_name: None,
            listen_participants: vec![],
        }),
        dag_config: None,
        conversation_config: None,
    };

    let json = serde_json::to_string(&config_msg).unwrap();
    assert!(json.contains("\"type\":\"config\""));
    assert!(json.contains("\"room_name\":\"test-room\""));
    assert!(json.contains("\"livekit\"")); // Verify LiveKit section is present

    // Parse back to ensure structure is correct
    let parsed: IncomingMessage = serde_json::from_str(&json).unwrap();
    if let IncomingMessage::Config { livekit, .. } = parsed {
        let livekit_config = livekit.unwrap();
        assert_eq!(livekit_config.room_name, "test-room");
        assert!(!livekit_config.enable_recording);
    } else {
        panic!("Expected Config message");
    }
}

#[test]
fn test_participant_disconnected_info_serialization() {
    let participant_info = ParticipantDisconnectedInfo {
        identity: "user123".to_string(),
        name: Some("John Doe".to_string()),
        room: "test-room".to_string(),
        timestamp: 1234567890,
    };

    let json = serde_json::to_string(&participant_info).unwrap();
    assert!(json.contains("\"identity\":\"user123\""));
    assert!(json.contains("\"name\":\"John Doe\""));
    assert!(json.contains("\"room\":\"test-room\""));
    assert!(json.contains("\"timestamp\":1234567890"));
}

#[test]
fn test_participant_disconnected_info_serialization_without_name() {
    let participant_info = ParticipantDisconnectedInfo {
        identity: "user456".to_string(),
        name: None,
        room: "test-room".to_string(),
        timestamp: 1234567890,
    };

    let json = serde_json::to_string(&participant_info).unwrap();
    assert!(json.contains("\"identity\":\"user456\""));
    assert!(json.contains("\"room\":\"test-room\""));
    assert!(json.contains("\"timestamp\":1234567890"));
    // Should not contain name field when None due to skip_serializing_if
    assert!(!json.contains("name"));
}

#[test]
fn test_participant_disconnected_outgoing_message() {
    let participant_info = ParticipantDisconnectedInfo {
        identity: "participant789".to_string(),
        name: Some("Alice Smith".to_string()),
        room: "conference-room".to_string(),
        timestamp: 9876543210,
    };

    let outgoing_msg = OutgoingMessage::ParticipantDisconnected {
        participant: participant_info,
    };

    let json = serde_json::to_string(&outgoing_msg).unwrap();
    assert!(json.contains("\"type\":\"participant_disconnected\""));
    assert!(json.contains("\"identity\":\"participant789\""));
    assert!(json.contains("\"name\":\"Alice Smith\""));
    assert!(json.contains("\"room\":\"conference-room\""));
    assert!(json.contains("\"timestamp\":9876543210"));
}

#[test]
fn test_participant_disconnected_json_format() {
    // Test that the serialized JSON has the expected format
    let participant_info = ParticipantDisconnectedInfo {
        identity: "user999".to_string(),
        name: Some("Bob Wilson".to_string()),
        room: "meeting-room".to_string(),
        timestamp: 1111111111,
    };

    let outgoing_msg = OutgoingMessage::ParticipantDisconnected {
        participant: participant_info,
    };

    let json = serde_json::to_string(&outgoing_msg).unwrap();

    // Verify that the JSON contains all expected fields
    assert!(json.contains("\"type\":\"participant_disconnected\""));
    assert!(json.contains("\"participant\":{"));
    assert!(json.contains("\"identity\":\"user999\""));
    assert!(json.contains("\"name\":\"Bob Wilson\""));
    assert!(json.contains("\"room\":\"meeting-room\""));
    assert!(json.contains("\"timestamp\":1111111111"));
}

#[test]
fn test_participant_disconnected_message_structure() {
    // Test the structure matches the documented API format
    let participant_info = ParticipantDisconnectedInfo {
        identity: "test-participant".to_string(),
        name: Some("Test User".to_string()),
        room: "test-room".to_string(),
        timestamp: 1000000000,
    };

    let outgoing_msg = OutgoingMessage::ParticipantDisconnected {
        participant: participant_info,
    };

    let json = serde_json::to_string(&outgoing_msg).unwrap();

    // Verify the JSON structure matches the API documentation
    let parsed_value: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed_value["type"], "participant_disconnected");
    assert_eq!(parsed_value["participant"]["identity"], "test-participant");
    assert_eq!(parsed_value["participant"]["name"], "Test User");
    assert_eq!(parsed_value["participant"]["room"], "test-room");
    assert_eq!(parsed_value["participant"]["timestamp"], 1000000000);
}

#[test]
fn test_tts_audio_routing_logic_without_livekit() {
    // Test that TTS audio data is properly handled without LiveKit
    let audio_data = vec![1, 2, 3, 4, 5];
    let bytes_data = Bytes::from(audio_data.clone());

    // When LiveKit is not configured, audio should go directly to WebSocket as binary
    assert_eq!(bytes_data.to_vec(), audio_data);
    assert_eq!(bytes_data.len(), 5);

    // Test that MessageRoute::Binary correctly wraps the audio data
    let route = MessageRoute::Binary(bytes_data);
    match route {
        MessageRoute::Binary(data) => {
            assert_eq!(data.to_vec(), audio_data);
        }
        _ => panic!("Expected Binary route"),
    }
}

#[test]
fn test_tts_audio_routing_logic_with_livekit() {
    // Test that TTS audio routing logic properly handles LiveKit scenarios

    // Test case 1: LiveKit configured and available
    // In this case, audio should be routed to LiveKit, not WebSocket
    // This is tested through the unified callback logic in the actual handler

    // Test case 2: LiveKit configured but disconnected
    // Audio should fall back to WebSocket

    // Test case 3: LiveKit send failure
    // Audio should fall back to WebSocket

    // These scenarios are integration-tested through the actual WebSocket handler
    // The unit test here validates the data structures and routing logic

    let test_audio = vec![0x01, 0x02, 0x03, 0x04];
    let audio_bytes = Bytes::from(test_audio.clone());

    // Verify the audio data can be properly converted for both routing paths
    assert_eq!(audio_bytes.to_vec(), test_audio);

    // Test that cloning works for dual routing scenarios
    let cloned_audio = audio_bytes.clone();
    assert_eq!(cloned_audio.to_vec(), test_audio);
    assert_eq!(audio_bytes.to_vec(), test_audio);
}

#[test]
fn test_config_message_audio_disabled() {
    // Test configuration with audio=false (LiveKit-only mode)
    let config_msg = IncomingMessage::Config {
        stream_id: None,
        audio: Some(false),
        audio_disabled: None,
        stt_config: None, // Not required when audio=false
        tts_config: None, // Not required when audio=false
        livekit: Some(LiveKitWebSocketConfig {
            room_name: "test-room".to_string(),
            enable_recording: false,
            waav_participant_identity: None,
            waav_participant_name: None,
            listen_participants: vec![],
        }),
        dag_config: None,
        conversation_config: None,
    };

    let json = serde_json::to_string(&config_msg).unwrap();
    assert!(json.contains("\"type\":\"config\""));
    assert!(json.contains("\"audio\":false"));
    assert!(json.contains("\"room_name\":\"test-room\""));
    assert!(json.contains("\"livekit\""));
    // Should not contain stt_config or tts_config fields when None
    assert!(!json.contains("stt_config"));
    assert!(!json.contains("tts_config"));

    // Parse back to ensure structure is correct
    let parsed: IncomingMessage = serde_json::from_str(&json).unwrap();
    if let IncomingMessage::Config {
        stream_id: None,
        audio,
        stt_config,
        tts_config,
        livekit,
        ..
    } = parsed
    {
        assert_eq!(audio, Some(false));
        assert!(stt_config.is_none());
        assert!(tts_config.is_none());
        assert!(livekit.is_some());
    } else {
        panic!("Expected Config message");
    }
}

#[test]
fn test_config_message_audio_default() {
    // Test configuration with no audio field (should default to true)
    let config_msg = IncomingMessage::Config {
        stream_id: None,
        audio: None, // Should default to true
        audio_disabled: None,
        stt_config: Some(STTWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "nova-3".to_string(),
            features: Default::default(),
            extras: Default::default(),
            turn_detection: None,
        }),
        tts_config: Some(TTSWebSocketConfig {
            api_key: None,
            provider: "deepgram".to_string(),
            voice_id: Some("aura-luna-en".to_string()),
            speaking_rate: Some(1.0),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(22050),
            client_playback_rate: None,
            connection_timeout: Some(30),
            request_timeout: Some(60),
            model: "".to_string(),
            pronunciations: Vec::new(),
            emotion: None,
            emotion_intensity: None,
            delivery_style: None,
            emotion_description: None,
            features: Default::default(),
            extras: Default::default(),
        }),
        livekit: None,
        dag_config: None,
        conversation_config: None,
    };

    let json = serde_json::to_string(&config_msg).unwrap();
    assert!(json.contains("\"type\":\"config\""));
    // Should not contain audio field when None (due to skip_serializing_if)
    assert!(!json.contains("\"audio\""));

    // Parse back to ensure structure is correct
    let parsed: IncomingMessage = serde_json::from_str(&json).unwrap();
    if let IncomingMessage::Config {
        stream_id: None,
        audio,
        stt_config,
        tts_config,
        livekit,
        ..
    } = parsed
    {
        assert_eq!(audio, Some(true)); // Should default to true via serde default
        assert!(stt_config.is_some());
        assert!(tts_config.is_some());
        assert!(livekit.is_none());
    } else {
        panic!("Expected Config message");
    }
}

#[test]
fn test_unified_message_for_livekit_integration() {
    // Test unified message structure used for LiveKit data forwarding
    let test_data = b"Hello from LiveKit participant";

    // Test text message from LiveKit
    let text_message = UnifiedMessage {
        message: Some(String::from_utf8(test_data.to_vec()).unwrap()),
        data: None,
        identity: "participant123".to_string(),
        topic: "chat".to_string(),
        room: "test-room".to_string(),
        timestamp: 1234567890,
    };

    let outgoing_msg = OutgoingMessage::Message {
        message: text_message,
    };

    let json = serde_json::to_string(&outgoing_msg).unwrap();
    assert!(json.contains("\"type\":\"message\""));
    assert!(json.contains("\"identity\":\"participant123\""));
    assert!(json.contains("Hello from LiveKit participant"));

    // Test binary data message from LiveKit
    let binary_message = UnifiedMessage {
        message: None,
        data: Some(general_purpose::STANDARD.encode(test_data)),
        identity: "participant456".to_string(),
        topic: "files".to_string(),
        room: "test-room".to_string(),
        timestamp: 1234567891,
    };

    let outgoing_msg_binary = OutgoingMessage::Message {
        message: binary_message,
    };

    let json_binary = serde_json::to_string(&outgoing_msg_binary).unwrap();
    assert!(json_binary.contains("\"type\":\"message\""));
    assert!(json_binary.contains("\"identity\":\"participant456\""));
    assert!(json_binary.contains("\"topic\":\"files\""));
    assert!(json_binary.contains("\"data\":"));
    // Should not contain message field for binary data
    assert!(!json_binary.contains("\"message\":null"));
}

// --- C-G5 pt3: client playback rate (egress resampling) ---

fn tts_cfg(provider_rate: Option<u32>, client_rate: Option<u32>) -> TTSWebSocketConfig {
    TTSWebSocketConfig {
        api_key: None,
        provider: "deepgram".to_string(),
        voice_id: None,
        speaking_rate: None,
        audio_format: Some("linear16".to_string()),
        sample_rate: provider_rate,
        client_playback_rate: client_rate,
        connection_timeout: None,
        request_timeout: None,
        model: "aura-asteria-en".to_string(),
        pronunciations: Vec::new(),
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        features: Default::default(),
        extras: Default::default(),
    }
}

#[test]
fn client_playback_rate_is_wire_compatible() {
    // Absent on the wire → None (existing clients unaffected).
    let json = r#"{"provider":"deepgram","model":"aura-asteria-en"}"#;
    let cfg: TTSWebSocketConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.client_playback_rate, None);
    // Present → honored.
    let json = r#"{"provider":"deepgram","model":"aura-asteria-en","client_playback_rate":48000}"#;
    let cfg: TTSWebSocketConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.client_playback_rate, Some(48000));
    // Unset never serializes (no schema noise for old readers).
    let out = serde_json::to_string(&tts_cfg(Some(24000), None)).unwrap();
    assert!(!out.contains("client_playback_rate"));
}

#[test]
fn egress_audio_context_only_when_requested() {
    use super::config_handler::EgressAudio;
    assert!(EgressAudio::from_tts_config(None).is_none(), "no TTS config");
    assert!(
        EgressAudio::from_tts_config(Some(&tts_cfg(Some(24000), None))).is_none(),
        "no client rate requested → zero-cost None"
    );
    assert!(EgressAudio::from_tts_config(Some(&tts_cfg(Some(24000), Some(48000)))).is_some());
}

#[test]
fn egress_audio_converts_pcm_and_passes_compressed_through() {
    use super::config_handler::EgressAudio;
    let egress = EgressAudio::from_tts_config(Some(&tts_cfg(Some(24000), Some(48000)))).unwrap();

    // 0.5s of 24k PCM16 → ~2x bytes at 48k (minus the filter tail).
    let pcm: Vec<u8> = (0..12_000i32)
        .flat_map(|i| (((i as f32 * 0.05).sin() * 20000.0) as i16).to_le_bytes())
        .collect();
    let out = egress.convert(pcm.clone(), "linear16", 24_000);
    assert!(
        out.len() > pcm.len() * 3 / 2,
        "expected ~2x upsampled bytes, got {} from {}",
        out.len(),
        pcm.len()
    );

    // Compressed: untouched.
    let mp3 = vec![0xFFu8; 1200];
    assert_eq!(egress.convert(mp3.clone(), "mp3", 24_000), mp3);

    // Chunk without a stamped rate: the configured provider rate (24k) is
    // the fallback, so conversion still happens.
    let out = egress.convert(pcm.clone(), "pcm", 0);
    assert!(out.len() > pcm.len() * 3 / 2, "configured-rate fallback must convert");
}

#[test]
fn livekit_track_rate_matches_delivered_bytes() {
    let lk = LiveKitWebSocketConfig {
        room_name: "room".to_string(),
        enable_recording: false,
        waav_participant_identity: None,
        waav_participant_name: None,
        listen_participants: vec![],
    };
    // No client rate → provider rate.
    let cfg = lk.to_livekit_config("t".into(), &tts_cfg(Some(22050), None), 16_000, "ws://x");
    assert_eq!(cfg.sample_rate, 22050);
    // Client rate set → the track must run at the rate of the RESAMPLED bytes.
    let cfg = lk.to_livekit_config("t".into(), &tts_cfg(Some(24000), Some(48000)), 16_000, "ws://x");
    assert_eq!(cfg.sample_rate, 48000);
    // Nothing configured → 24k default.
    let cfg = lk.to_livekit_config("t".into(), &tts_cfg(None, None), 16_000, "ws://x");
    assert_eq!(cfg.sample_rate, 24000);
}

#[test]
fn invalid_client_playback_rate_disables_egress_resampling() {
    use super::config_handler::EgressAudio;
    // 0 Hz would configure a 0 Hz LiveKit track; absurd rates build
    // pathological resamplers — both rejected loudly (review wf_85659e16).
    assert!(EgressAudio::from_tts_config(Some(&tts_cfg(Some(24000), Some(0)))).is_none());
    assert!(EgressAudio::from_tts_config(Some(&tts_cfg(Some(24000), Some(4)))).is_none());
    assert!(
        EgressAudio::from_tts_config(Some(&tts_cfg(Some(24000), Some(700_000)))).is_none()
    );
    assert!(EgressAudio::from_tts_config(Some(&tts_cfg(Some(24000), Some(8_000)))).is_some());
}

#[test]
fn egress_flush_recovers_the_utterance_tail() {
    use super::config_handler::EgressAudio;
    let egress = EgressAudio::from_tts_config(Some(&tts_cfg(Some(24000), Some(48000)))).unwrap();
    // A sub-chunk final piece converts to NOTHING (buffered)...
    let tail_in: Vec<u8> = vec![0x10; 600]; // 300 samples < 480-frame chunk
    let out = egress.convert(tail_in, "linear16", 24_000);
    assert!(out.is_empty(), "sub-chunk piece is buffered, not emitted");
    // ...until the utterance-end flush delivers it (review wf_85659e16 #8/#11).
    let tail = egress.flush();
    assert!(
        tail.len() >= 1000,
        "flush must emit the ~600 buffered output frames, got {} bytes",
        tail.len()
    );
    assert!(egress.flush().is_empty(), "second flush has nothing pending");
}

#[test]
fn livekit_ingress_rate_is_stt_derived_not_client_rate() {
    // Review wf_85659e16 CRITICAL #1: the egress-only client_playback_rate
    // must NEVER re-rate the user's microphone (ingress). Ingress follows
    // the STT pipeline's declared rate; egress follows the client rate.
    let lk = LiveKitWebSocketConfig {
        room_name: "room".to_string(),
        enable_recording: false,
        waav_participant_identity: None,
        waav_participant_name: None,
        listen_participants: vec![],
    };
    let cfg =
        lk.to_livekit_config("t".into(), &tts_cfg(Some(24000), Some(48000)), 16_000, "ws://x");
    assert_eq!(cfg.sample_rate, 48000, "egress track follows the client rate");
    assert_eq!(
        cfg.ingress_sample_rate, 16_000,
        "ingress (user mic -> STT/VAD) follows the STT rate, untouched by the knob"
    );
    // An INVALID client rate must not poison the egress track either.
    let cfg = lk.to_livekit_config("t".into(), &tts_cfg(Some(24000), Some(0)), 16_000, "ws://x");
    assert_eq!(cfg.sample_rate, 24000, "invalid client rate falls back to provider rate");
}

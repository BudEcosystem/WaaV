//! # LiveKit Integration Module
//!
//! This module provides LiveKit WebRTC integration for the WaaV project, enabling
//! real-time audio streaming and room connectivity. It offers both low-level client
//! access and high-level management interfaces for seamless integration with STT/TTS
//! services.
//!
//! ## Features
//!
//! - **Real-time Audio Streaming**: Receive audio tracks from LiveKit room participants
//! - **WebRTC Connectivity**: Full WebRTC support through LiveKit's Rust SDK
//! - **Audio Format Conversion**: Convert LiveKit audio (i16) to processing format (u8)
//! - **Event-Driven Architecture**: Handle room events, participant changes, track subscriptions
//! - **Connection Management**: Robust connection lifecycle with proper cleanup
//! - **Callback System**: Flexible audio processing callbacks for integration
//! - **High-Level Management**: Simplified interface for common use cases
//!
//! ## Usage
//!
//! The live audio path drives [`LiveKitClient`] directly (see
//! `handlers/ws/config_handler.rs` for the production wiring); room/token
//! lifecycle goes through [`LiveKitRoomHandler`]. (A former `LiveKitManager`
//! convenience wrapper was removed: it was dead code with an unbounded audio
//! channel — a backpressure trap for any future caller.)
//!
//! ```rust,ignore
//! let handler = LiveKitRoomHandler::new(url, api_key, api_secret, None)?;
//! handler.create_room("my-room").await?;
//! ```
//!
//! ## Audio Processing
//!
//! The module handles audio data conversion from LiveKit's native i16 format to u8 byte
//! arrays using little-endian encoding, making it compatible with STT services and other
//! audio processing systems in the WaaV ecosystem.
//!
//! ## Integration with WaaV Gateway
//!
//! This module integrates with the unified STT/TTS server architecture by:
//! - Receiving real-time audio from LiveKit participants
//! - Converting audio format for STT processing compatibility
//! - Providing callbacks for forwarding audio to STT services
//! - Supporting the WebSocket API abstraction layer

mod client;
pub mod ice;
pub mod operations;
pub mod room_handler;
pub mod sip_handler;
mod types;

// Re-export public types and traits
pub use client::{AudioCallback, DataCallback, DataMessage, LiveKitClient};
pub use operations::{LiveKitOperation, OperationQueue};
pub use room_handler::LiveKitRoomHandler;
pub use sip_handler::{DispatchConfig, LiveKitSipHandler, TrunkConfig};
pub use types::{
    AudioFrameInfo, CallerTap, ConnectionStatus, LiveKitConfig, LiveKitError, ParticipantInfo,
    RoomInfo,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_livekit_config() {
        let config = LiveKitConfig {
            url: "wss://test.example.com".to_string(),
            token: "test-token".to_string(),
            room_name: "test-room".to_string(),
            sample_rate: 24000,
            ingress_sample_rate: 16000,
            channels: 1,
            enable_noise_filter: cfg!(feature = "noise-filter"),
            listen_participants: vec![],
            caller_tap: None,
        };

        assert_eq!(config.url, "wss://test.example.com");
        assert_eq!(config.token, "test-token");
        assert_eq!(config.enable_noise_filter, cfg!(feature = "noise-filter"));
    }

    /// Interaction mode in a room with several participants: the first whose audio arrives
    /// is the caller, and stays it.
    #[test]
    fn the_first_participant_heard_is_the_sessions_caller() {
        use std::sync::{Arc, Mutex};

        let found = Arc::new(Mutex::new(Vec::new()));
        let frames = Arc::new(Mutex::new(0usize));
        let (found_by, heard) = (found.clone(), frames.clone());
        let tap = CallerTap::new(
            move |frame| *heard.lock().unwrap() += frame.len(),
            move |phone| found_by.lock().unwrap().push(phone),
        );
        // Nobody is the caller yet: whoever leaves may be the one the session waits for.
        assert!(!tap.is_other("sip_4155550100"));

        assert!(tap.admits("sip_4155550100", true));
        assert!(!tap.admits("supervisor", false), "a second participant");
        // A mute and unmute subscribes the caller's track again.
        assert!(tap.admits("sip_4155550100", true));
        assert_eq!(*found.lock().unwrap(), [true], "told once, a phone line");
        assert!(tap.is_other("supervisor") && !tap.is_other("sip_4155550100"));

        // The room's reconnect clones the configuration: the caller is kept.
        let again = tap.clone();
        assert!(!again.admits("supervisor", false));
        again.audio(&[0; 320]);
        assert_eq!(*frames.lock().unwrap(), 320);
        assert_eq!(
            format!("{tap:?}"),
            r#"CallerTap { caller: Some("sip_4155550100") }"#
        );
    }

    #[test]
    fn only_the_callers_keys_are_passed_on() {
        use std::sync::{Arc, Mutex};

        let keys = Arc::new(Mutex::new(String::new()));
        let pressed = keys.clone();
        let tap =
            CallerTap::new(|_| {}, |_| {}).on_keys(move |key| pressed.lock().unwrap().push(key));
        // Before the caller is known, and from anybody else, nothing.
        tap.key(Some("sip_4155550100"), 1, Some("1"));
        assert!(tap.admits("sip_4155550100", true));
        tap.key(Some("supervisor"), 2, Some("2"));
        tap.key(None, 3, Some("3"));
        // The digit, else the code; nothing that is not a key.
        tap.key(Some("sip_4155550100"), 4, Some("4"));
        tap.key(Some("sip_4155550100"), 11, None);
        tap.key(Some("sip_4155550100"), 10, Some(""));
        tap.key(Some("sip_4155550100"), 12, None);
        tap.key(Some("sip_4155550100"), 0, Some("A"));
        assert_eq!(*keys.lock().unwrap(), "4#*");
    }

    #[test]
    fn test_connection_status_enum() {
        use ConnectionStatus::*;

        assert_eq!(Disconnected, Disconnected);
        assert_ne!(Connecting, Connected);
        assert_ne!(Connected, Failed);
    }
}

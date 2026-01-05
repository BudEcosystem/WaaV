//! HTTP and WebSocket request handlers
//!
//! This module organizes all API handlers into logical groups:
//! - `api` - Health check endpoint
//! - `livekit` - LiveKit token generation and webhook handling
//! - `realtime` - Realtime audio-to-audio WebSocket (OpenAI Realtime API)
//! - `recording` - Recording download endpoint
//! - `sip` - SIP hooks management and call transfer
//! - `speak` - Text-to-speech REST API
//! - `voices` - Voice listing endpoint
//! - `ws` - WebSocket real-time voice processing

pub mod api;
pub mod livekit;
pub mod realtime;
pub mod recording;
pub mod sip;
pub mod speak;
pub mod voices;
pub mod ws;

// Re-export commonly used handlers for convenient access
pub use realtime::realtime_handler;
pub use ws::ws_voice_handler;

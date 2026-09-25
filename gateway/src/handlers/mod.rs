//! HTTP and WebSocket request handlers
//!
//! This module organizes all API handlers into logical groups:
//! - `api` - Health check endpoint
//! - `advisories` - The `/v1/audio/*` warning channel (FRD-018 Part III W1)
//! - `capabilities` - Language and provider-feature support matrix discovery (P2, FRD-018 M1)
//! - `dag` - DAG template management and validation
//! - `endpoint_settings` - Applying a deployment's stored config to a request (FRD-018 Part III)
//! - `livekit` - LiveKit token generation and webhook handling
//! - `realtime` - Realtime audio-to-audio WebSocket (OpenAI Realtime API)
//! - `recording` - Recording download endpoint
//! - `sip` - SIP hooks management and call transfer
//! - `speak` - Text-to-speech REST API
//! - `voices` - Voice listing endpoint
//! - `ws` - WebSocket real-time voice processing

pub mod advisories;
pub mod api;
pub mod capabilities;
pub mod dag;
pub mod debug_profile;
pub mod endpoint_settings;
pub mod livekit;
pub mod openai_audio;
pub mod realtime;
pub mod recording;
pub mod sip;
pub mod speak;
pub mod transcribe;
pub mod voice_catalog;
pub mod voices;
pub mod ws;

// Re-export commonly used handlers for convenient access
pub use realtime::realtime_handler;
pub use ws::ws_voice_handler;

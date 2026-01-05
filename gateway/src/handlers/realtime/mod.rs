//! Realtime audio-to-audio WebSocket handlers
//!
//! This module provides WebSocket handlers for real-time bidirectional
//! audio streaming with transcription and TTS.
//!
//! # Supported Providers
//!
//! - **OpenAI Realtime API** - Full duplex audio with GPT-4o
//!
//! # Protocol
//!
//! The WebSocket protocol is provider-agnostic:
//!
//! ## Client → Server
//!
//! - **config**: Configure the session (provider, model, voice, etc.)
//! - **text**: Send text message to conversation
//! - **create_response**: Request model to generate response
//! - **cancel_response**: Cancel current response
//! - **commit_audio**: Commit audio buffer (manual VAD)
//! - **clear_audio**: Clear audio buffer
//! - **function_result**: Submit function call result
//! - **update_session**: Update session configuration
//! - **Binary frames**: Audio data (PCM 16-bit, 24kHz, mono)
//!
//! ## Server → Client
//!
//! - **session_created**: Session established
//! - **session_updated**: Session configuration updated
//! - **transcript**: Speech transcription (user or assistant)
//! - **speech_event**: VAD events (started/stopped)
//! - **function_call**: Function call request from model
//! - **response_started**: Response generation started
//! - **response_done**: Response generation completed
//! - **error**: Error message
//! - **closing**: Connection closing
//! - **Binary frames**: Audio data from TTS

mod handler;
pub mod messages;

pub use handler::realtime_handler;

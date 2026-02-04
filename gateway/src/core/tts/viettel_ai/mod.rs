//! Viettel AI TTS provider module.
//!
//! This module provides text-to-speech functionality using Viettel Group's
//! AI platform, optimized for Vietnamese language with regional accent support.
//!
//! # Overview
//!
//! Viettel AI TTS is developed by Viettel Group, Vietnam's largest
//! telecommunications corporation. The service provides:
//!
//! - 11 Vietnamese voices (5 Northern, 4 Southern, 2 Central)
//! - 95% human-like speech quality
//! - Regional accent preservation
//! - Automatic pauses and intonation
//!
//! # Protocol
//!
//! Uses REST API:
//! 1. POST text to synthesis endpoint
//! 2. Receive WAV audio binary response
//!
//! # Authentication
//!
//! - Token-based authentication via `token` header
//! - Tokens valid for 180 days
//! - Create tokens at https://viettelgroup.ai/dashboard/token

mod client;
pub mod config;

#[cfg(test)]
mod tests;

pub use client::ViettelTts;
pub use config::{
    AUDIO_SAMPLE_RATE, DEFAULT_SPEED, DEFAULT_TTS_RETURN_OPTION, MAX_SPEED, MAX_TEXT_LENGTH,
    MIN_SPEED, MIN_TEXT_LENGTH, VIETTEL_TTS_ENDPOINT, VIETTEL_VOICES_ENDPOINT, ViettelTtsConfig,
    ViettelTtsResponse, ViettelVoice,
};

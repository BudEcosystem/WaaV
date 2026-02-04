//! Prosa.ai TTS provider module.
//!
//! This module provides text-to-speech functionality using Prosa.ai's
//! Indonesian NLP platform, optimized for Bahasa Indonesia.
//!
//! # Overview
//!
//! Prosa.ai is an Indonesian AI company founded in 2018 that specializes in
//! Natural Language Processing (NLP) technology for Bahasa Indonesia.
//! The TTS service provides:
//!
//! - 40+ Indonesian voices (various styles and accents)
//! - English voice support (Roger, Jennifer)
//! - Adjustable pitch and tempo
//! - Multiple audio formats (opus, mp3, wav)
//! - Synchronous and asynchronous synthesis
//!
//! # Protocol
//!
//! ## REST API
//! - POST `https://api.prosa.ai/v2/speech/tts` for synthesis
//! - GET `https://api.prosa.ai/v2/speech/tts/{job_id}` for async results
//!
//! # Authentication
//!
//! - API key authentication via `x-api-key` header
//! - Obtain API keys from https://console.prosa.ai

mod client;
pub mod config;

#[cfg(test)]
mod tests;

pub use client::ProsaTts;
pub use config::{
    DEFAULT_PITCH, DEFAULT_REQUEST_TIMEOUT, DEFAULT_SAMPLE_RATE, DEFAULT_TEMPO,
    MAX_ASYNC_TEXT_LENGTH, MAX_PITCH, MAX_SYNC_TEXT_LENGTH, MAX_TEMPO, MIN_PITCH, MIN_TEMPO,
    MIN_TEXT_LENGTH, PROSA_TTS_BASE_URL, ProsaTtsAudioFormat, ProsaTtsConfig, ProsaTtsError,
    ProsaTtsRequest, ProsaTtsRequestConfig, ProsaTtsRequestData, ProsaTtsResponse, ProsaTtsResult,
    ProsaTtsVoice,
};

//! Prosa.ai STT provider module.
//!
//! This module provides speech-to-text functionality using Prosa.ai's
//! Indonesian NLP platform, optimized for Bahasa Indonesia and regional languages.
//!
//! # Overview
//!
//! Prosa.ai is an Indonesian AI company founded in 2018 that specializes in
//! Natural Language Processing (NLP) technology for Bahasa Indonesia.
//! The service provides:
//!
//! - Indonesian, Javanese, Sundanese, and English support
//! - Synchronous and asynchronous recognition
//! - WebSocket streaming
//! - Speaker diarization
//! - Multi-channel audio support
//!
//! # Protocol
//!
//! ## REST API
//! - POST `https://api.prosa.ai/v2/speech/stt` for transcription
//! - GET `https://api.prosa.ai/v2/speech/stt/{job_id}` for async results
//!
//! ## WebSocket Streaming
//! - Connect to `wss://s-api.prosa.ai/v2/speech/stt`
//! - Send configuration, then audio chunks
//! - Receive partial and final transcripts
//!
//! # Authentication
//!
//! - API key authentication via `x-api-key` header
//! - Obtain API keys from https://console.prosa.ai

mod client;
pub mod config;

#[cfg(test)]
mod tests;

pub use client::ProsaStt;
pub use config::{
    ProsaAudioFormat, ProsaSttConfig, ProsaSttError, ProsaSttModel, ProsaSttResponse, ProsaSttResult,
    ProsaSttSegment, ProsaSttWsMessage, DEFAULT_CHANNELS, DEFAULT_CHUNK_SIZE,
    DEFAULT_REQUEST_TIMEOUT, DEFAULT_SAMPLE_RATE, MAX_ASYNC_DURATION_SECS, MAX_SYNC_DURATION_SECS,
    MAX_SYNC_SIZE_BYTES, MIN_AUDIO_BUFFER_SIZE, PROSA_STT_BASE_URL, PROSA_STT_WS_ENDPOINT,
};

//! Viettel AI STT provider module.
//!
//! This module provides speech-to-text functionality using Viettel Group's
//! AI platform, optimized for Vietnamese language with 96% accuracy.
//!
//! # Overview
//!
//! Viettel AI STT is developed by Viettel Group, Vietnam's largest
//! telecommunications corporation. The service provides:
//!
//! - 96% accuracy for Vietnamese speech
//! - Regional accent detection
//! - Multiple input formats (WAV, PCM, MP3)
//! - Enterprise-grade security
//!
//! # Protocol
//!
//! Uses REST API with multipart file upload:
//! 1. POST audio file to decode endpoint
//! 2. Receive JSON response with transcription
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

pub use client::ViettelStt;
pub use config::{
    DEFAULT_CHANNELS, DEFAULT_REQUEST_TIMEOUT, DEFAULT_SAMPLE_RATE, PCM_FORMAT_S16LE,
    VIETTEL_STT_ENDPOINT, ViettelSttConfig, ViettelSttResponse,
};

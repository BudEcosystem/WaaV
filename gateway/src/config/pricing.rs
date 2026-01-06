//! Centralized pricing configuration for all STT/TTS providers.
//!
//! This module provides a single source of truth for model pricing across all providers.
//! Pricing can be updated without modifying provider-specific code.
//!
//! # Pricing Sources
//!
//! Prices are based on official provider pricing pages as of the last update.
//! All prices are in USD per hour of audio processed unless otherwise noted.
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::config::pricing::{ModelPricing, get_stt_price, get_tts_price};
//!
//! // Get STT price per hour
//! let price = get_stt_price("groq", "whisper-large-v3-turbo");
//!
//! // Get TTS price per character
//! let price = get_tts_price("elevenlabs", "eleven_multilingual_v2");
//! ```
//!
//! # Updates
//!
//! When provider pricing changes, update the constants in this file.
//! Last updated: 2024-01-06

use std::collections::HashMap;
use std::sync::LazyLock;

// =============================================================================
// Pricing Types
// =============================================================================

/// Pricing unit for a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PricingUnit {
    /// Price per hour of audio (STT/TTS audio duration)
    PerHour,
    /// Price per 1000 characters (TTS text input)
    Per1KChars,
    /// Price per 1 million characters (TTS text input)
    Per1MChars,
    /// Price per minute of audio
    PerMinute,
    /// Price per second of audio
    PerSecond,
}

/// Pricing information for a model.
#[derive(Debug, Clone)]
pub struct ModelPricing {
    /// Price amount in USD
    pub price: f64,
    /// Unit for the price
    pub unit: PricingUnit,
    /// Optional notes about pricing (e.g., "minimum 10s billing")
    pub notes: Option<&'static str>,
}

impl ModelPricing {
    /// Create new pricing entry.
    pub const fn new(price: f64, unit: PricingUnit) -> Self {
        Self {
            price,
            unit,
            notes: None,
        }
    }

    /// Create pricing entry with notes.
    pub const fn with_notes(price: f64, unit: PricingUnit, notes: &'static str) -> Self {
        Self {
            price,
            unit,
            notes: Some(notes),
        }
    }

    /// Convert price to per-hour rate for comparison.
    pub fn to_per_hour(&self) -> f64 {
        match self.unit {
            PricingUnit::PerHour => self.price,
            PricingUnit::PerMinute => self.price * 60.0,
            PricingUnit::PerSecond => self.price * 3600.0,
            // Character-based pricing can't be converted to time-based
            PricingUnit::Per1KChars | PricingUnit::Per1MChars => 0.0,
        }
    }
}

// =============================================================================
// STT Provider Pricing
// =============================================================================

/// STT pricing database.
/// Key format: "provider:model" (lowercase)
static STT_PRICING: LazyLock<HashMap<&'static str, ModelPricing>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // -------------------------------------------------------------------------
    // Groq Whisper
    // https://console.groq.com/docs/speech-to-text
    // -------------------------------------------------------------------------
    m.insert(
        "groq:whisper-large-v3",
        ModelPricing::with_notes(0.111, PricingUnit::PerHour, "10.3% WER, 189x real-time"),
    );
    m.insert(
        "groq:whisper-large-v3-turbo",
        ModelPricing::with_notes(0.04, PricingUnit::PerHour, "12% WER, 216x real-time, min 10s billing"),
    );

    // -------------------------------------------------------------------------
    // Deepgram
    // https://deepgram.com/pricing
    // -------------------------------------------------------------------------
    m.insert(
        "deepgram:nova-2",
        ModelPricing::with_notes(0.0043 * 60.0, PricingUnit::PerHour, "Pay-as-you-go rate per minute"),
    );
    m.insert(
        "deepgram:nova-2-general",
        ModelPricing::new(0.0043 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "deepgram:nova-2-meeting",
        ModelPricing::new(0.0043 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "deepgram:nova-2-phonecall",
        ModelPricing::new(0.0043 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "deepgram:nova-2-medical",
        ModelPricing::new(0.0043 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "deepgram:enhanced",
        ModelPricing::new(0.0145 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "deepgram:base",
        ModelPricing::new(0.0125 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "deepgram:whisper",
        ModelPricing::new(0.0048 * 60.0, PricingUnit::PerHour),
    );

    // -------------------------------------------------------------------------
    // OpenAI Whisper
    // https://openai.com/api/pricing/
    // -------------------------------------------------------------------------
    m.insert(
        "openai:whisper-1",
        ModelPricing::new(0.006 * 60.0, PricingUnit::PerHour), // $0.006/minute
    );

    // -------------------------------------------------------------------------
    // Google Cloud Speech-to-Text
    // https://cloud.google.com/speech-to-text/pricing
    // -------------------------------------------------------------------------
    m.insert(
        "google:default",
        ModelPricing::new(0.024 * 60.0, PricingUnit::PerHour), // $0.024 per minute
    );
    m.insert(
        "google:latest_long",
        ModelPricing::new(0.024 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "google:latest_short",
        ModelPricing::new(0.024 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "google:chirp",
        ModelPricing::with_notes(0.016 * 60.0, PricingUnit::PerHour, "Chirp universal model"),
    );
    m.insert(
        "google:chirp_2",
        ModelPricing::with_notes(0.016 * 60.0, PricingUnit::PerHour, "Chirp 2 universal model"),
    );

    // -------------------------------------------------------------------------
    // Microsoft Azure Speech Services
    // https://azure.microsoft.com/en-us/pricing/details/cognitive-services/speech-services/
    // -------------------------------------------------------------------------
    m.insert(
        "azure:default",
        ModelPricing::new(1.0, PricingUnit::PerHour), // $1/hour standard
    );
    m.insert(
        "azure:batch",
        ModelPricing::with_notes(0.36, PricingUnit::PerHour, "Batch transcription"),
    );
    m.insert(
        "azure:conversation",
        ModelPricing::new(2.1, PricingUnit::PerHour), // Conversation transcription
    );

    // -------------------------------------------------------------------------
    // AssemblyAI
    // https://www.assemblyai.com/pricing
    // -------------------------------------------------------------------------
    m.insert(
        "assemblyai:best",
        ModelPricing::new(0.65 * 60.0 / 100.0, PricingUnit::PerHour), // $0.65 per 100 minutes
    );
    m.insert(
        "assemblyai:nano",
        ModelPricing::new(0.12 * 60.0 / 100.0, PricingUnit::PerHour), // $0.12 per 100 minutes
    );

    // -------------------------------------------------------------------------
    // Cartesia (ink-whisper)
    // https://www.cartesia.ai/pricing
    // -------------------------------------------------------------------------
    m.insert(
        "cartesia:ink-whisper",
        ModelPricing::new(0.40 * 60.0, PricingUnit::PerHour), // $0.40 per minute
    );

    // -------------------------------------------------------------------------
    // ElevenLabs STT
    // https://elevenlabs.io/pricing
    // -------------------------------------------------------------------------
    m.insert(
        "elevenlabs:scribe_v1",
        ModelPricing::with_notes(0.40 * 60.0, PricingUnit::PerHour, "99 languages, min 15s billing"),
    );

    // -------------------------------------------------------------------------
    // IBM Watson Speech to Text
    // https://cloud.ibm.com/catalog/services/speech-to-text
    // -------------------------------------------------------------------------
    m.insert(
        "ibm_watson:en-us_narrowbandmodel",
        ModelPricing::new(0.02 * 60.0, PricingUnit::PerHour), // $0.02 per minute
    );
    m.insert(
        "ibm_watson:en-us_broadbandmodel",
        ModelPricing::new(0.02 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "ibm_watson:en-us_multimedia",
        ModelPricing::new(0.02 * 60.0, PricingUnit::PerHour),
    );
    m.insert(
        "ibm_watson:en-us_telephony",
        ModelPricing::new(0.02 * 60.0, PricingUnit::PerHour),
    );

    // -------------------------------------------------------------------------
    // AWS Transcribe
    // https://aws.amazon.com/transcribe/pricing/
    // -------------------------------------------------------------------------
    m.insert(
        "aws_transcribe:standard",
        ModelPricing::new(0.024 * 60.0, PricingUnit::PerHour), // $0.024 per minute
    );
    m.insert(
        "aws_transcribe:medical",
        ModelPricing::new(0.0625 * 60.0, PricingUnit::PerHour), // $0.0625 per minute
    );

    m
});

// =============================================================================
// TTS Provider Pricing
// =============================================================================

/// TTS pricing database.
/// Key format: "provider:model" (lowercase)
static TTS_PRICING: LazyLock<HashMap<&'static str, ModelPricing>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // -------------------------------------------------------------------------
    // ElevenLabs TTS
    // https://elevenlabs.io/pricing
    // -------------------------------------------------------------------------
    m.insert(
        "elevenlabs:eleven_multilingual_v2",
        ModelPricing::new(0.24, PricingUnit::Per1KChars), // $0.24 per 1K chars (Creator plan)
    );
    m.insert(
        "elevenlabs:eleven_turbo_v2_5",
        ModelPricing::new(0.24, PricingUnit::Per1KChars),
    );
    m.insert(
        "elevenlabs:eleven_flash_v2_5",
        ModelPricing::with_notes(0.08, PricingUnit::Per1KChars, "Lower latency, lower quality"),
    );
    m.insert(
        "elevenlabs:eleven_monolingual_v1",
        ModelPricing::new(0.24, PricingUnit::Per1KChars),
    );

    // -------------------------------------------------------------------------
    // OpenAI TTS
    // https://openai.com/api/pricing/
    // -------------------------------------------------------------------------
    m.insert(
        "openai:tts-1",
        ModelPricing::new(15.0, PricingUnit::Per1MChars), // $15 per 1M chars
    );
    m.insert(
        "openai:tts-1-hd",
        ModelPricing::new(30.0, PricingUnit::Per1MChars), // $30 per 1M chars
    );

    // -------------------------------------------------------------------------
    // Google Cloud Text-to-Speech
    // https://cloud.google.com/text-to-speech/pricing
    // -------------------------------------------------------------------------
    m.insert(
        "google:standard",
        ModelPricing::new(4.0, PricingUnit::Per1MChars), // $4 per 1M chars
    );
    m.insert(
        "google:wavenet",
        ModelPricing::new(16.0, PricingUnit::Per1MChars), // $16 per 1M chars
    );
    m.insert(
        "google:neural2",
        ModelPricing::new(16.0, PricingUnit::Per1MChars),
    );
    m.insert(
        "google:studio",
        ModelPricing::with_notes(160.0, PricingUnit::Per1MChars, "Custom voice"),
    );
    m.insert(
        "google:journey",
        ModelPricing::new(30.0, PricingUnit::Per1MChars),
    );

    // -------------------------------------------------------------------------
    // Microsoft Azure TTS
    // https://azure.microsoft.com/en-us/pricing/details/cognitive-services/speech-services/
    // -------------------------------------------------------------------------
    m.insert(
        "azure:neural",
        ModelPricing::new(16.0, PricingUnit::Per1MChars), // $16 per 1M chars
    );
    m.insert(
        "azure:neural-hd",
        ModelPricing::new(24.0, PricingUnit::Per1MChars),
    );
    m.insert(
        "azure:custom-neural",
        ModelPricing::new(24.0, PricingUnit::Per1MChars),
    );

    // -------------------------------------------------------------------------
    // Cartesia TTS (sonic-3)
    // https://www.cartesia.ai/pricing
    // -------------------------------------------------------------------------
    m.insert(
        "cartesia:sonic-3",
        ModelPricing::new(0.10, PricingUnit::Per1KChars), // $0.10 per 1K chars
    );
    m.insert(
        "cartesia:sonic-2",
        ModelPricing::new(0.10, PricingUnit::Per1KChars),
    );
    m.insert(
        "cartesia:sonic-english",
        ModelPricing::new(0.10, PricingUnit::Per1KChars),
    );
    m.insert(
        "cartesia:sonic-multilingual",
        ModelPricing::new(0.10, PricingUnit::Per1KChars),
    );

    // -------------------------------------------------------------------------
    // Deepgram TTS (Aura)
    // https://deepgram.com/pricing
    // -------------------------------------------------------------------------
    m.insert(
        "deepgram:aura",
        ModelPricing::new(0.015, PricingUnit::Per1KChars), // $0.015 per 1K chars
    );
    m.insert(
        "deepgram:aura-asteria-en",
        ModelPricing::new(0.015, PricingUnit::Per1KChars),
    );
    m.insert(
        "deepgram:aura-luna-en",
        ModelPricing::new(0.015, PricingUnit::Per1KChars),
    );
    m.insert(
        "deepgram:aura-stella-en",
        ModelPricing::new(0.015, PricingUnit::Per1KChars),
    );

    // -------------------------------------------------------------------------
    // IBM Watson Text to Speech
    // https://cloud.ibm.com/catalog/services/text-to-speech
    // -------------------------------------------------------------------------
    m.insert(
        "ibm_watson:neural",
        ModelPricing::new(20.0, PricingUnit::Per1MChars), // $0.02 per 1K = $20 per 1M
    );
    m.insert(
        "ibm_watson:enhanced",
        ModelPricing::new(20.0, PricingUnit::Per1MChars),
    );

    // -------------------------------------------------------------------------
    // AWS Polly
    // https://aws.amazon.com/polly/pricing/
    // -------------------------------------------------------------------------
    m.insert(
        "aws_polly:standard",
        ModelPricing::new(4.0, PricingUnit::Per1MChars), // $4 per 1M chars
    );
    m.insert(
        "aws_polly:neural",
        ModelPricing::new(16.0, PricingUnit::Per1MChars), // $16 per 1M chars
    );
    m.insert(
        "aws_polly:long-form",
        ModelPricing::new(100.0, PricingUnit::Per1MChars), // $100 per 1M chars
    );
    m.insert(
        "aws_polly:generative",
        ModelPricing::new(30.0, PricingUnit::Per1MChars), // $30 per 1M chars
    );

    m
});

// =============================================================================
// Public API
// =============================================================================

/// Get STT pricing for a provider and model.
///
/// # Arguments
/// * `provider` - Provider name (e.g., "groq", "deepgram")
/// * `model` - Model name (e.g., "whisper-large-v3-turbo", "nova-2")
///
/// # Returns
/// * `Option<&ModelPricing>` - Pricing info if found
pub fn get_stt_pricing(provider: &str, model: &str) -> Option<&'static ModelPricing> {
    let key = format!("{}:{}", provider.to_lowercase(), model.to_lowercase());
    STT_PRICING.get(key.as_str())
}

/// Get STT price per hour for a provider and model.
///
/// # Arguments
/// * `provider` - Provider name
/// * `model` - Model name
///
/// # Returns
/// * `Option<f64>` - Price per hour in USD
pub fn get_stt_price_per_hour(provider: &str, model: &str) -> Option<f64> {
    get_stt_pricing(provider, model).map(|p| p.to_per_hour())
}

/// Get TTS pricing for a provider and model.
///
/// # Arguments
/// * `provider` - Provider name (e.g., "elevenlabs", "openai")
/// * `model` - Model name (e.g., "eleven_multilingual_v2", "tts-1")
///
/// # Returns
/// * `Option<&ModelPricing>` - Pricing info if found
pub fn get_tts_pricing(provider: &str, model: &str) -> Option<&'static ModelPricing> {
    let key = format!("{}:{}", provider.to_lowercase(), model.to_lowercase());
    TTS_PRICING.get(key.as_str())
}

/// Calculate estimated cost for audio transcription.
///
/// # Arguments
/// * `provider` - Provider name
/// * `model` - Model name
/// * `duration_seconds` - Audio duration in seconds
///
/// # Returns
/// * `Option<f64>` - Estimated cost in USD
pub fn estimate_stt_cost(provider: &str, model: &str, duration_seconds: f64) -> Option<f64> {
    get_stt_pricing(provider, model).map(|pricing| {
        let duration_hours = duration_seconds / 3600.0;
        match pricing.unit {
            PricingUnit::PerHour => pricing.price * duration_hours,
            PricingUnit::PerMinute => pricing.price * (duration_seconds / 60.0),
            PricingUnit::PerSecond => pricing.price * duration_seconds,
            // Can't estimate character-based pricing from duration
            PricingUnit::Per1KChars | PricingUnit::Per1MChars => 0.0,
        }
    })
}

/// Calculate estimated cost for text-to-speech synthesis.
///
/// # Arguments
/// * `provider` - Provider name
/// * `model` - Model name
/// * `char_count` - Number of characters to synthesize
///
/// # Returns
/// * `Option<f64>` - Estimated cost in USD
pub fn estimate_tts_cost(provider: &str, model: &str, char_count: usize) -> Option<f64> {
    get_tts_pricing(provider, model).map(|pricing| match pricing.unit {
        PricingUnit::Per1KChars => pricing.price * (char_count as f64 / 1000.0),
        PricingUnit::Per1MChars => pricing.price * (char_count as f64 / 1_000_000.0),
        // Can't estimate time-based pricing from character count
        PricingUnit::PerHour | PricingUnit::PerMinute | PricingUnit::PerSecond => 0.0,
    })
}

/// List all available STT models for a provider.
pub fn list_stt_models(provider: &str) -> Vec<&'static str> {
    let prefix = format!("{}:", provider.to_lowercase());
    STT_PRICING
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .map(|k| k.strip_prefix(&prefix).unwrap_or(k))
        .collect()
}

/// List all available TTS models for a provider.
pub fn list_tts_models(provider: &str) -> Vec<&'static str> {
    let prefix = format!("{}:", provider.to_lowercase());
    TTS_PRICING
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .map(|k| k.strip_prefix(&prefix).unwrap_or(k))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_groq_stt_pricing() {
        let pricing = get_stt_pricing("groq", "whisper-large-v3-turbo");
        assert!(pricing.is_some());
        let p = pricing.unwrap();
        assert!((p.price - 0.04).abs() < f64::EPSILON);
        assert_eq!(p.unit, PricingUnit::PerHour);
    }

    #[test]
    fn test_groq_stt_pricing_case_insensitive() {
        let pricing1 = get_stt_pricing("GROQ", "WHISPER-LARGE-V3-TURBO");
        let pricing2 = get_stt_pricing("groq", "whisper-large-v3-turbo");
        assert!(pricing1.is_some());
        assert!(pricing2.is_some());
        assert!((pricing1.unwrap().price - pricing2.unwrap().price).abs() < f64::EPSILON);
    }

    #[test]
    fn test_get_stt_price_per_hour() {
        let price = get_stt_price_per_hour("groq", "whisper-large-v3");
        assert!(price.is_some());
        assert!((price.unwrap() - 0.111).abs() < f64::EPSILON);
    }

    #[test]
    fn test_deepgram_stt_pricing() {
        let pricing = get_stt_pricing("deepgram", "nova-2");
        assert!(pricing.is_some());
        // $0.0043/min = $0.258/hour
        assert!((pricing.unwrap().price - (0.0043 * 60.0)).abs() < 0.001);
    }

    #[test]
    fn test_elevenlabs_tts_pricing() {
        let pricing = get_tts_pricing("elevenlabs", "eleven_multilingual_v2");
        assert!(pricing.is_some());
        let p = pricing.unwrap();
        assert!((p.price - 0.24).abs() < f64::EPSILON);
        assert_eq!(p.unit, PricingUnit::Per1KChars);
    }

    #[test]
    fn test_estimate_stt_cost() {
        // 1 hour of Groq whisper-large-v3-turbo at $0.04/hour = $0.04
        let cost = estimate_stt_cost("groq", "whisper-large-v3-turbo", 3600.0);
        assert!(cost.is_some());
        assert!((cost.unwrap() - 0.04).abs() < f64::EPSILON);

        // 30 minutes (1800 seconds)
        let cost = estimate_stt_cost("groq", "whisper-large-v3-turbo", 1800.0);
        assert!(cost.is_some());
        assert!((cost.unwrap() - 0.02).abs() < f64::EPSILON);
    }

    #[test]
    fn test_estimate_tts_cost() {
        // 1000 chars of ElevenLabs at $0.24/1K chars = $0.24
        let cost = estimate_tts_cost("elevenlabs", "eleven_multilingual_v2", 1000);
        assert!(cost.is_some());
        assert!((cost.unwrap() - 0.24).abs() < f64::EPSILON);

        // 500 chars
        let cost = estimate_tts_cost("elevenlabs", "eleven_multilingual_v2", 500);
        assert!(cost.is_some());
        assert!((cost.unwrap() - 0.12).abs() < f64::EPSILON);
    }

    #[test]
    fn test_list_stt_models() {
        let models = list_stt_models("groq");
        assert!(models.contains(&"whisper-large-v3"));
        assert!(models.contains(&"whisper-large-v3-turbo"));
    }

    #[test]
    fn test_list_tts_models() {
        let models = list_tts_models("elevenlabs");
        assert!(!models.is_empty());
        assert!(models.contains(&"eleven_multilingual_v2"));
    }

    #[test]
    fn test_unknown_model_returns_none() {
        assert!(get_stt_pricing("unknown", "unknown").is_none());
        assert!(get_tts_pricing("unknown", "unknown").is_none());
        assert!(estimate_stt_cost("unknown", "unknown", 3600.0).is_none());
        assert!(estimate_tts_cost("unknown", "unknown", 1000).is_none());
    }

    #[test]
    fn test_to_per_hour_conversion() {
        let per_minute = ModelPricing::new(1.0, PricingUnit::PerMinute);
        assert!((per_minute.to_per_hour() - 60.0).abs() < f64::EPSILON);

        let per_second = ModelPricing::new(1.0, PricingUnit::PerSecond);
        assert!((per_second.to_per_hour() - 3600.0).abs() < f64::EPSILON);

        let per_hour = ModelPricing::new(10.0, PricingUnit::PerHour);
        assert!((per_hour.to_per_hour() - 10.0).abs() < f64::EPSILON);

        // Character-based returns 0
        let per_1k_chars = ModelPricing::new(0.24, PricingUnit::Per1KChars);
        assert!((per_1k_chars.to_per_hour()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_openai_pricing() {
        // STT
        let stt = get_stt_pricing("openai", "whisper-1");
        assert!(stt.is_some());
        assert!((stt.unwrap().price - (0.006 * 60.0)).abs() < 0.001);

        // TTS
        let tts = get_tts_pricing("openai", "tts-1");
        assert!(tts.is_some());
        assert!((tts.unwrap().price - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_aws_pricing() {
        // STT
        let stt = get_stt_pricing("aws_transcribe", "standard");
        assert!(stt.is_some());

        // TTS
        let tts = get_tts_pricing("aws_polly", "neural");
        assert!(tts.is_some());
        assert!((tts.unwrap().price - 16.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_ibm_watson_pricing() {
        // STT
        let stt = get_stt_pricing("ibm_watson", "en-us_telephony");
        assert!(stt.is_some());

        // TTS
        let tts = get_tts_pricing("ibm_watson", "neural");
        assert!(tts.is_some());
    }
}

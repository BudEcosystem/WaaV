//! Unified Emotion System for TTS Providers.
//!
//! This module provides a standardized abstraction layer for controlling
//! emotional expression in text-to-speech synthesis across different providers.
//! Each provider has its own mechanism for emotion control, and this system
//! translates a unified configuration into provider-specific formats.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                    WaaV Unified Emotion System                          │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                          │
//! │  ┌─────────────────────┐      ┌─────────────────────┐                   │
//! │  │  EmotionConfig      │      │  EmotionMapper      │                   │
//! │  │  (User-facing)      │─────▶│  (Provider-specific)│                   │
//! │  │                     │      │                     │                   │
//! │  │  emotion: "happy"   │      │  to_hume()          │                   │
//! │  │  intensity: 0.8     │      │  to_elevenlabs()    │                   │
//! │  │  style: "whisper"   │      │  to_azure_ssml()    │                   │
//! │  └─────────────────────┘      └─────────────────────┘                   │
//! │                                        │                                 │
//! │                    ┌───────────────────┼───────────────────┐            │
//! │                    ▼                   ▼                   ▼            │
//! │            ┌───────────────┐   ┌───────────────┐   ┌───────────────┐   │
//! │            │ Hume          │   │ ElevenLabs    │   │ Azure         │   │
//! │            │               │   │               │   │               │   │
//! │            │ description:  │   │ stability:0.3 │   │ <mstts:       │   │
//! │            │ "happy,       │   │ style:0.5     │   │  express-as   │   │
//! │            │  energetic"   │   │               │   │  style=...>   │   │
//! │            └───────────────┘   └───────────────┘   └───────────────┘   │
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Provider Support Matrix
//!
//! | Provider | Emotions | Intensity | Styles | Free Description | Method |
//! |----------|----------|-----------|--------|------------------|--------|
//! | **Hume AI** | All | Yes | Yes | Yes | Natural language |
//! | **ElevenLabs** | Core set | Yes | Yes | No | Voice settings |
//! | **Azure** | Core set | Yes | Yes | No | SSML express-as |
//! | **Deepgram** | None | No | No | No | N/A |
//! | **Cartesia** | None | No | No | No | N/A |
//! | **Google** | None | No | No | No | N/A |
//! | **OpenAI** | None* | No | No | No | N/A |
//! | **AWS Polly** | None | No | No | No | N/A |
//! | **IBM Watson** | None | No | No | No | N/A |
//!
//! *OpenAI's gpt-4o-mini-tts supports instructions but not through this system yet.
//!
//! # Quick Start
//!
//! ## Basic Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::emotion::{EmotionConfig, Emotion, DeliveryStyle};
//! use waav_gateway::core::emotion::mappers::get_mapper_for_provider;
//!
//! // Create emotion configuration
//! let config = EmotionConfig::new()
//!     .emotion(Emotion::Happy)
//!     .intensity(0.8)
//!     .style(DeliveryStyle::Expressive);
//!
//! // Get mapper for your TTS provider
//! let mapper = get_mapper_for_provider("hume");
//!
//! // Map to provider-specific format
//! let mapped = mapper.map_emotion(&config);
//!
//! // Use the mapped values
//! if let Some(description) = mapped.description {
//!     println!("Hume description: {}", description);
//! }
//! ```
//!
//! ## Free-Form Descriptions (Hume AI)
//!
//! ```rust,ignore
//! use waav_gateway::core::emotion::EmotionConfig;
//!
//! let config = EmotionConfig::with_description("warm, friendly, inviting");
//! // This works with Hume and will generate warnings for other providers
//! ```
//!
//! ## Checking Provider Support
//!
//! ```rust,ignore
//! use waav_gateway::core::emotion::mappers::provider_supports_emotions;
//!
//! if provider_supports_emotions("elevenlabs") {
//!     // Apply emotion settings
//! } else {
//!     // Use default voice settings
//! }
//! ```
//!
//! ## Handling Warnings
//!
//! The emotion system uses graceful degradation: when emotions aren't
//! supported, audio is still synthesized but with warnings.
//!
//! ```rust,ignore
//! use waav_gateway::core::emotion::{EmotionConfig, Emotion};
//! use waav_gateway::core::emotion::mappers::get_mapper_for_provider;
//!
//! let config = EmotionConfig::with_emotion(Emotion::Sarcastic);
//! let mapper = get_mapper_for_provider("deepgram");
//! let mapped = mapper.map_emotion(&config);
//!
//! if mapped.has_warnings() {
//!     for warning in &mapped.warnings {
//!         tracing::warn!("Emotion warning: {}", warning);
//!     }
//! }
//! ```
//!
//! # See Also
//!
//! - [`types`] - Core types: `Emotion`, `EmotionConfig`, `DeliveryStyle`
//! - [`mapper`] - Mapper trait and `MappedEmotion`
//! - [`mappers`] - Provider-specific mapper implementations

pub mod mapper;
pub mod mappers;
pub mod types;

// =============================================================================
// Public Re-exports
// =============================================================================

// Core types
pub use types::{DeliveryStyle, Emotion, EmotionConfig, EmotionIntensity, IntensityLevel};

// Mapper trait and result
pub use mapper::{EmotionMapper, EmotionMethod, MappedEmotion, ProviderEmotionSupport};

// Provider mappers
pub use mappers::{
    AzureEmotionMapper, ElevenLabsEmotionMapper, FallbackEmotionMapper, HumeEmotionMapper,
    get_mapper_for_provider, provider_supports_emotions, providers_with_emotion_support,
    providers_without_emotion_support,
};

// =============================================================================
// Convenience Functions
// =============================================================================

/// Maps an emotion configuration to provider format and returns with warnings.
///
/// This is a convenience function that combines mapper lookup and mapping.
///
/// # Arguments
///
/// * `provider` - The TTS provider name
/// * `config` - The emotion configuration
///
/// # Returns
///
/// A tuple of (`MappedEmotion`, `Option<String>`) where the second element
/// is a formatted warning string if any warnings were generated.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::emotion::{map_emotion_for_provider, EmotionConfig, Emotion};
///
/// let config = EmotionConfig::with_emotion(Emotion::Happy);
/// let (mapped, warnings) = map_emotion_for_provider("hume", &config);
///
/// if let Some(warn) = warnings {
///     tracing::warn!("Emotion: {}", warn);
/// }
/// ```
pub fn map_emotion_for_provider(
    provider: &str,
    config: &EmotionConfig,
) -> (MappedEmotion, Option<String>) {
    let mapper = get_mapper_for_provider(provider);
    let mapped = mapper.map_emotion(config);
    let warnings = mapped.format_warnings();
    (mapped, warnings)
}

/// Validates an emotion configuration and returns any issues.
///
/// This performs validation without actually mapping:
/// - Checks description length (for Hume)
/// - Validates intensity range
/// - Returns list of issues found
///
/// # Arguments
///
/// * `config` - The emotion configuration to validate
///
/// # Returns
///
/// A vector of validation issues. Empty if configuration is valid.
pub fn validate_emotion_config(config: &EmotionConfig) -> Vec<String> {
    let mut issues = Vec::new();

    // Check description length
    if let Some(desc) = &config.description {
        if desc.len() > mappers::MAX_DESCRIPTION_LENGTH {
            issues.push(format!(
                "Description exceeds maximum length of {} characters (actual: {})",
                mappers::MAX_DESCRIPTION_LENGTH,
                desc.len()
            ));
        }
    }

    // Check intensity
    if let Some(intensity) = &config.intensity {
        let value = intensity.as_f32();
        if !(0.0..=1.0).contains(&value) {
            issues.push(format!(
                "Intensity {} is outside valid range [0.0, 1.0]",
                value
            ));
        }
    }

    issues
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_emotion_for_provider_hume() {
        let config = EmotionConfig::with_emotion(Emotion::Happy);
        let (mapped, warnings) = map_emotion_for_provider("hume", &config);

        assert!(mapped.description.is_some());
        assert!(warnings.is_none());
    }

    #[test]
    fn test_map_emotion_for_provider_deepgram() {
        let config = EmotionConfig::with_emotion(Emotion::Happy);
        let (mapped, warnings) = map_emotion_for_provider("deepgram", &config);

        assert!(!mapped.has_modifications());
        assert!(warnings.is_some());
    }

    #[test]
    fn test_validate_emotion_config_valid() {
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .intensity(0.8f32);

        let issues = validate_emotion_config(&config);
        assert!(issues.is_empty());
    }

    #[test]
    fn test_validate_emotion_config_long_description() {
        let long_desc = "a".repeat(150);
        let config = EmotionConfig::with_description(long_desc);

        let issues = validate_emotion_config(&config);
        assert!(!issues.is_empty());
        assert!(issues[0].contains("exceeds maximum length"));
    }

    #[test]
    fn test_public_exports() {
        // Verify all expected types are accessible
        let _emotion = Emotion::Happy;
        let _style = DeliveryStyle::Whispered;
        let _intensity = EmotionIntensity::from_f32(0.5);
        let _level = IntensityLevel::High;
        let _config = EmotionConfig::new();
        let _method = EmotionMethod::NaturalLanguage;
        let _mapped = MappedEmotion::empty();

        // Mappers accessible
        let _hume = HumeEmotionMapper::new();
        let _elevenlabs = ElevenLabsEmotionMapper::new();
        let _azure = AzureEmotionMapper::new();
        let _fallback = FallbackEmotionMapper::deepgram();

        // Functions accessible
        let _ = get_mapper_for_provider("test");
        let _ = provider_supports_emotions("test");
        let _ = providers_with_emotion_support();
        let _ = providers_without_emotion_support();
    }

    #[test]
    fn test_emotion_config_builder_chain() {
        let config = EmotionConfig::new()
            .emotion(Emotion::Excited)
            .intensity(0.9f32)
            .style(DeliveryStyle::Expressive)
            .context("customer greeting");

        assert_eq!(config.emotion, Some(Emotion::Excited));
        assert!(config.has_emotion());
        assert!(!config.is_neutral());
    }
}

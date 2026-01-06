//! Provider-specific emotion mappers.
//!
//! This module contains emotion mapper implementations for each TTS provider,
//! translating the unified `EmotionConfig` to provider-specific formats.
//!
//! # Available Mappers
//!
//! | Mapper | Provider | Method | Description |
//! |--------|----------|--------|-------------|
//! | `HumeEmotionMapper` | Hume AI | Natural language | Free-form descriptions |
//! | `ElevenLabsEmotionMapper` | ElevenLabs | Voice settings | stability, style |
//! | `AzureEmotionMapper` | Azure | SSML | express-as styles |
//! | `FallbackEmotionMapper` | Others | None | Warning generation |
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::emotion::{EmotionConfig, Emotion};
//! use waav_gateway::core::emotion::mappers::{
//!     HumeEmotionMapper, ElevenLabsEmotionMapper, AzureEmotionMapper,
//!     get_mapper_for_provider,
//! };
//! use waav_gateway::core::emotion::EmotionMapper;
//!
//! let config = EmotionConfig::with_emotion(Emotion::Happy);
//!
//! // Get mapper for a specific provider
//! let mapper = get_mapper_for_provider("hume");
//! let mapped = mapper.map_emotion(&config);
//!
//! println!("Hume description: {:?}", mapped.description);
//! ```

mod azure;
mod elevenlabs;
mod fallback;
mod hume;

pub use azure::AzureEmotionMapper;
pub use elevenlabs::ElevenLabsEmotionMapper;
pub use fallback::FallbackEmotionMapper;
pub use hume::{HumeEmotionMapper, MAX_DESCRIPTION_LENGTH};

use super::mapper::EmotionMapper;

/// Returns the appropriate emotion mapper for a given provider.
///
/// # Arguments
///
/// * `provider` - The provider name (case-insensitive)
///
/// # Returns
///
/// A boxed emotion mapper trait object.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::emotion::mappers::get_mapper_for_provider;
/// use waav_gateway::core::emotion::EmotionMapper;
///
/// let hume_mapper = get_mapper_for_provider("hume");
/// assert!(hume_mapper.supports_free_description());
///
/// let deepgram_mapper = get_mapper_for_provider("deepgram");
/// assert!(!deepgram_mapper.get_support().supports_emotions);
/// ```
pub fn get_mapper_for_provider(provider: &str) -> Box<dyn EmotionMapper> {
    match provider.to_lowercase().as_str() {
        "hume" | "hume-ai" | "hume_ai" => Box::new(HumeEmotionMapper::new()),
        "elevenlabs" | "eleven_labs" => Box::new(ElevenLabsEmotionMapper::new()),
        "azure" | "microsoft-azure" | "microsoft_azure" => Box::new(AzureEmotionMapper::new()),
        "deepgram" => Box::new(FallbackEmotionMapper::deepgram()),
        "cartesia" => Box::new(FallbackEmotionMapper::cartesia()),
        "google" => Box::new(FallbackEmotionMapper::google()),
        "ibm-watson" | "ibm_watson" | "watson" | "ibm" => {
            Box::new(FallbackEmotionMapper::ibm_watson())
        }
        "aws-polly" | "aws_polly" | "amazon-polly" | "polly" => {
            Box::new(FallbackEmotionMapper::aws_polly())
        }
        "openai" => Box::new(FallbackEmotionMapper::openai()),
        _ => Box::new(FallbackEmotionMapper::new("unknown")),
    }
}

/// Returns whether a provider has any emotion support.
///
/// # Arguments
///
/// * `provider` - The provider name (case-insensitive)
///
/// # Returns
///
/// `true` if the provider supports at least some emotion control.
#[inline]
pub fn provider_supports_emotions(provider: &str) -> bool {
    get_mapper_for_provider(provider)
        .get_support()
        .has_any_support()
}

/// Returns the list of providers with full emotion support.
///
/// These providers support custom emotions, intensity, and/or free descriptions.
#[inline]
pub fn providers_with_emotion_support() -> &'static [&'static str] {
    &["hume", "elevenlabs", "azure"]
}

/// Returns the list of providers without emotion support.
///
/// These providers only support voice selection, not emotional expression.
#[inline]
pub fn providers_without_emotion_support() -> &'static [&'static str] {
    &["deepgram", "cartesia", "google", "ibm-watson", "aws-polly", "openai"]
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::emotion::mapper::EmotionMethod;

    #[test]
    fn test_get_mapper_hume() {
        let mapper = get_mapper_for_provider("hume");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "hume");
        assert!(support.supports_free_description);
        assert_eq!(support.method, EmotionMethod::NaturalLanguage);
    }

    #[test]
    fn test_get_mapper_hume_variants() {
        for provider in ["hume", "hume-ai", "hume_ai", "HUME"] {
            let mapper = get_mapper_for_provider(provider);
            assert_eq!(mapper.get_support().provider_id, "hume");
        }
    }

    #[test]
    fn test_get_mapper_elevenlabs() {
        let mapper = get_mapper_for_provider("elevenlabs");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "elevenlabs");
        assert_eq!(support.method, EmotionMethod::VoiceSettings);
    }

    #[test]
    fn test_get_mapper_azure() {
        let mapper = get_mapper_for_provider("azure");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "azure");
        assert_eq!(support.method, EmotionMethod::Ssml);
    }

    #[test]
    fn test_get_mapper_azure_variants() {
        for provider in ["azure", "microsoft-azure", "microsoft_azure", "AZURE"] {
            let mapper = get_mapper_for_provider(provider);
            assert_eq!(mapper.get_support().provider_id, "azure");
        }
    }

    #[test]
    fn test_get_mapper_deepgram_fallback() {
        let mapper = get_mapper_for_provider("deepgram");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "deepgram");
        assert!(!support.supports_emotions);
        assert_eq!(support.method, EmotionMethod::None);
    }

    #[test]
    fn test_get_mapper_cartesia_fallback() {
        let mapper = get_mapper_for_provider("cartesia");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "cartesia");
        assert!(!support.supports_emotions);
    }

    #[test]
    fn test_get_mapper_google_fallback() {
        let mapper = get_mapper_for_provider("google");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "google");
        assert!(!support.supports_emotions);
    }

    #[test]
    fn test_get_mapper_ibm_fallback() {
        for provider in ["ibm-watson", "ibm_watson", "watson", "ibm"] {
            let mapper = get_mapper_for_provider(provider);
            assert_eq!(mapper.get_support().provider_id, "ibm-watson");
        }
    }

    #[test]
    fn test_get_mapper_aws_fallback() {
        for provider in ["aws-polly", "aws_polly", "amazon-polly", "polly"] {
            let mapper = get_mapper_for_provider(provider);
            assert_eq!(mapper.get_support().provider_id, "aws-polly");
        }
    }

    #[test]
    fn test_get_mapper_openai_fallback() {
        let mapper = get_mapper_for_provider("openai");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "openai");
        assert!(!support.supports_emotions);
    }

    #[test]
    fn test_get_mapper_unknown() {
        let mapper = get_mapper_for_provider("unknown-provider");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "unknown");
        assert!(!support.supports_emotions);
    }

    #[test]
    fn test_provider_supports_emotions() {
        assert!(provider_supports_emotions("hume"));
        assert!(provider_supports_emotions("elevenlabs"));
        assert!(provider_supports_emotions("azure"));

        assert!(!provider_supports_emotions("deepgram"));
        assert!(!provider_supports_emotions("cartesia"));
        assert!(!provider_supports_emotions("google"));
    }

    #[test]
    fn test_providers_with_emotion_support() {
        let providers = providers_with_emotion_support();
        assert!(providers.contains(&"hume"));
        assert!(providers.contains(&"elevenlabs"));
        assert!(providers.contains(&"azure"));
        assert!(!providers.contains(&"deepgram"));
    }

    #[test]
    fn test_providers_without_emotion_support() {
        let providers = providers_without_emotion_support();
        assert!(providers.contains(&"deepgram"));
        assert!(providers.contains(&"cartesia"));
        assert!(providers.contains(&"google"));
        assert!(!providers.contains(&"hume"));
    }
}

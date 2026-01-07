//! Fallback emotion mapper for providers without emotion support.
//!
//! This mapper is used for TTS providers that don't support any form
//! of emotion control (e.g., Deepgram, Cartesia basic voices, Google TTS).
//!
//! The fallback mapper:
//! 1. Returns empty mappings (no modifications)
//! 2. Generates warnings when emotion settings are requested
//! 3. Allows audio to be synthesized without emotion
//!
//! This enables graceful degradation: users can still get audio output
//! even when emotions aren't supported, with clear warnings about
//! what features were ignored.

use crate::core::emotion::mapper::{
    EmotionMapper, EmotionMethod, MappedEmotion, ProviderEmotionSupport,
};
use crate::core::emotion::types::{Emotion, EmotionConfig};

/// Fallback emotion mapper for providers without emotion support.
///
/// This mapper is used when a provider doesn't support any form
/// of emotion control. It generates appropriate warnings while
/// allowing the TTS synthesis to proceed.
#[derive(Debug, Clone)]
pub struct FallbackEmotionMapper {
    /// The provider identifier for warning messages
    provider_id: &'static str,
}

impl FallbackEmotionMapper {
    /// Creates a new fallback mapper for a specific provider.
    ///
    /// # Arguments
    ///
    /// * `provider_id` - The provider name for warning messages
    #[inline]
    pub const fn new(provider_id: &'static str) -> Self {
        Self { provider_id }
    }

    /// Creates a fallback mapper for Deepgram.
    #[inline]
    pub const fn deepgram() -> Self {
        Self::new("deepgram")
    }

    /// Creates a fallback mapper for Cartesia.
    #[inline]
    pub const fn cartesia() -> Self {
        Self::new("cartesia")
    }

    /// Creates a fallback mapper for Google TTS.
    #[inline]
    pub const fn google() -> Self {
        Self::new("google")
    }

    /// Creates a fallback mapper for IBM Watson.
    #[inline]
    pub const fn ibm_watson() -> Self {
        Self::new("ibm-watson")
    }

    /// Creates a fallback mapper for AWS Polly.
    #[inline]
    pub const fn aws_polly() -> Self {
        Self::new("aws-polly")
    }

    /// Creates a fallback mapper for OpenAI (basic models).
    ///
    /// Note: gpt-4o-mini-tts supports instructions for emotion,
    /// but the basic tts-1 and tts-1-hd models don't.
    #[inline]
    pub const fn openai() -> Self {
        Self::new("openai")
    }

    /// Creates a fallback mapper for LMNT.
    ///
    /// Note: LMNT doesn't support emotion tags directly, but users can
    /// control expressiveness through `top_p` (0-1, speech stability) and
    /// `temperature` (≥0, expressiveness range) parameters instead.
    #[inline]
    pub const fn lmnt() -> Self {
        Self::new("lmnt")
    }
}

impl Default for FallbackEmotionMapper {
    fn default() -> Self {
        Self::new("unknown")
    }
}

impl EmotionMapper for FallbackEmotionMapper {
    fn get_support(&self) -> ProviderEmotionSupport {
        ProviderEmotionSupport {
            provider_id: self.provider_id,
            supports_emotions: false,
            supported_emotions: &[],
            supports_intensity: false,
            supports_style: false,
            supports_free_description: false,
            method: EmotionMethod::None,
        }
    }

    fn map_emotion(&self, config: &EmotionConfig) -> MappedEmotion {
        let mut mapped = MappedEmotion::empty();

        // Generate warning if emotion was requested
        if let Some(emotion) = &config.emotion {
            if *emotion != Emotion::Neutral {
                mapped.add_warning(format!(
                    "Emotion '{}' is not supported by provider '{}'; using neutral voice",
                    emotion, self.provider_id
                ));
            }
        }

        // Generate warning for free-form description
        if let Some(description) = &config.description {
            if !description.is_empty() {
                mapped.add_warning(format!(
                    "Emotion descriptions are not supported by provider '{}'; ignoring '{}'",
                    self.provider_id,
                    if description.len() > 30 {
                        format!("{}...", &description[..27])
                    } else {
                        description.clone()
                    }
                ));
            }
        }

        // Generate warning for delivery style
        if let Some(style) = &config.style {
            if *style != crate::core::emotion::types::DeliveryStyle::Normal {
                mapped.add_warning(format!(
                    "Delivery style '{}' is not supported by provider '{}'; using normal delivery",
                    style, self.provider_id
                ));
            }
        }

        // Generate warning for intensity if significantly non-default
        if let Some(intensity) = &config.intensity {
            let value = intensity.as_f32();
            if (value - 0.6).abs() > 0.2 {
                mapped.add_warning(format!(
                    "Emotion intensity is not supported by provider '{}'; ignoring intensity {}",
                    self.provider_id, value
                ));
            }
        }

        mapped
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::emotion::types::{DeliveryStyle, EmotionIntensity};

    #[test]
    fn test_fallback_mapper_support() {
        let mapper = FallbackEmotionMapper::new("test-provider");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "test-provider");
        assert!(!support.supports_emotions);
        assert!(!support.supports_intensity);
        assert!(!support.supports_style);
        assert!(!support.supports_free_description);
        assert_eq!(support.method, EmotionMethod::None);
        assert!(support.supported_emotions.is_empty());
    }

    #[test]
    fn test_fallback_mapper_empty_config() {
        let mapper = FallbackEmotionMapper::new("test");
        let config = EmotionConfig::new();

        let mapped = mapper.map_emotion(&config);
        assert!(!mapped.has_modifications());
        assert!(!mapped.has_warnings());
    }

    #[test]
    fn test_fallback_mapper_neutral_no_warning() {
        let mapper = FallbackEmotionMapper::new("test");
        let config = EmotionConfig::with_emotion(Emotion::Neutral);

        let mapped = mapper.map_emotion(&config);
        assert!(!mapped.has_warnings());
    }

    #[test]
    fn test_fallback_mapper_emotion_warning() {
        let mapper = FallbackEmotionMapper::deepgram();
        let config = EmotionConfig::with_emotion(Emotion::Happy);

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.has_warnings());
        assert!(mapped.warnings[0].contains("happy"));
        assert!(mapped.warnings[0].contains("deepgram"));
    }

    #[test]
    fn test_fallback_mapper_description_warning() {
        let mapper = FallbackEmotionMapper::cartesia();
        let config = EmotionConfig::with_description("happy, energetic");

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.has_warnings());
        assert!(mapped.warnings[0].contains("cartesia"));
        assert!(mapped.warnings[0].contains("descriptions"));
    }

    #[test]
    fn test_fallback_mapper_long_description_truncated() {
        let mapper = FallbackEmotionMapper::new("test");
        let long_desc = "a".repeat(50);
        let config = EmotionConfig::with_description(long_desc);

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.has_warnings());
        assert!(mapped.warnings[0].contains("..."));
    }

    #[test]
    fn test_fallback_mapper_style_warning() {
        let mapper = FallbackEmotionMapper::google();
        let config = EmotionConfig::new().style(DeliveryStyle::Whispered);

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.has_warnings());
        assert!(mapped.warnings[0].contains("whispered"));
        assert!(mapped.warnings[0].contains("google"));
    }

    #[test]
    fn test_fallback_mapper_normal_style_no_warning() {
        let mapper = FallbackEmotionMapper::new("test");
        let config = EmotionConfig::new().style(DeliveryStyle::Normal);

        let mapped = mapper.map_emotion(&config);
        assert!(!mapped.has_warnings());
    }

    #[test]
    fn test_fallback_mapper_intensity_warning() {
        let mapper = FallbackEmotionMapper::ibm_watson();
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .intensity(EmotionIntensity::from_f32(0.9));

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.has_warnings());
        // Should have both emotion and intensity warnings
        assert!(mapped.warnings.len() >= 2);
    }

    #[test]
    fn test_fallback_mapper_default_intensity_no_warning() {
        let mapper = FallbackEmotionMapper::new("test");
        let config = EmotionConfig::new().intensity(EmotionIntensity::from_f32(0.6));

        let mapped = mapper.map_emotion(&config);
        // Default intensity shouldn't produce warning
        assert!(!mapped.has_warnings());
    }

    #[test]
    fn test_fallback_mapper_multiple_warnings() {
        let mapper = FallbackEmotionMapper::aws_polly();
        let config = EmotionConfig::new()
            .emotion(Emotion::Angry)
            .style(DeliveryStyle::Shouted)
            .description("very angry");

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.warnings.len() >= 3);
    }

    #[test]
    fn test_fallback_mapper_no_modifications() {
        let mapper = FallbackEmotionMapper::new("test");
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .style(DeliveryStyle::Expressive);

        let mapped = mapper.map_emotion(&config);
        // Even with warnings, no modifications should be made
        assert!(!mapped.has_modifications());
    }

    #[test]
    fn test_fallback_mapper_factory_methods() {
        let deepgram = FallbackEmotionMapper::deepgram();
        assert_eq!(deepgram.provider_id, "deepgram");

        let cartesia = FallbackEmotionMapper::cartesia();
        assert_eq!(cartesia.provider_id, "cartesia");

        let google = FallbackEmotionMapper::google();
        assert_eq!(google.provider_id, "google");

        let ibm = FallbackEmotionMapper::ibm_watson();
        assert_eq!(ibm.provider_id, "ibm-watson");

        let aws = FallbackEmotionMapper::aws_polly();
        assert_eq!(aws.provider_id, "aws-polly");

        let openai = FallbackEmotionMapper::openai();
        assert_eq!(openai.provider_id, "openai");

        let lmnt = FallbackEmotionMapper::lmnt();
        assert_eq!(lmnt.provider_id, "lmnt");
    }

    #[test]
    fn test_fallback_mapper_no_supported_emotions() {
        let mapper = FallbackEmotionMapper::new("test");

        for emotion in Emotion::all() {
            assert!(
                !mapper.supports_emotion(emotion),
                "Fallback mapper should not support any emotion"
            );
        }
    }
}

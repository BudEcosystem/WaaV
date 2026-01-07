//! Azure TTS emotion mapper.
//!
//! Maps standardized emotions to Azure SSML express-as styles.
//! Azure uses the `mstts:express-as` SSML element with a `style` attribute
//! and optional `styledegree` for intensity control.
//!
//! # SSML Express-As
//!
//! ```xml
//! <mstts:express-as style="cheerful" styledegree="1.5">
//!     Hello, how can I help you today?
//! </mstts:express-as>
//! ```
//!
//! # Supported Styles (varies by voice)
//!
//! Common styles across Azure neural voices:
//! - cheerful, sad, angry, fearful, excited, calm
//! - newscast, customerservice, assistant
//! - chat, friendly, hopeful, empathetic
//!
//! # Style Degree
//!
//! - 0.01 to 2.0 (default: 1.0)
//! - Values < 1.0 = subtler expression
//! - Values > 1.0 = more intense expression

use crate::core::emotion::mapper::{
    EmotionMapper, EmotionMethod, MappedEmotion, ProviderEmotionSupport,
};
use crate::core::emotion::types::{DeliveryStyle, Emotion, EmotionConfig};

/// Emotions that Azure explicitly supports via express-as styles
static AZURE_SUPPORTED_EMOTIONS: &[Emotion] = &[
    Emotion::Neutral,
    Emotion::Happy,
    Emotion::Sad,
    Emotion::Angry,
    Emotion::Fearful,
    Emotion::Excited,
    Emotion::Calm,
    Emotion::Empathetic,
    Emotion::Hopeful,
    Emotion::Disappointed,
];

/// Emotion mapper for Microsoft Azure TTS.
///
/// Maps emotions to SSML express-as styles with styledegree.
#[derive(Debug, Clone, Copy, Default)]
pub struct AzureEmotionMapper;

impl AzureEmotionMapper {
    /// Creates a new Azure emotion mapper.
    #[inline]
    pub const fn new() -> Self {
        Self
    }

    /// Maps an emotion to an Azure express-as style name.
    ///
    /// Returns `None` for emotions that have no direct mapping.
    fn emotion_to_style(emotion: &Emotion) -> Option<&'static str> {
        match emotion {
            Emotion::Neutral => None, // No style needed for neutral
            Emotion::Happy => Some("cheerful"),
            Emotion::Sad => Some("sad"),
            Emotion::Angry => Some("angry"),
            Emotion::Fearful => Some("terrified"),
            Emotion::Surprised => Some("excited"), // Closest match
            Emotion::Disgusted => Some("disgruntled"),
            Emotion::Excited => Some("excited"),
            Emotion::Calm => Some("calm"),
            Emotion::Anxious => Some("fearful"),
            Emotion::Confident => Some("newscast-casual"), // Authoritative
            Emotion::Confused => Some("gentle"),           // Softer approach
            Emotion::Empathetic => Some("empathetic"),
            Emotion::Sarcastic => Some("cheerful"), // No direct match
            Emotion::Hopeful => Some("hopeful"),
            Emotion::Disappointed => Some("depressed"),
            Emotion::Curious => Some("chat"), // Conversational
            Emotion::Grateful => Some("friendly"),
            Emotion::Proud => Some("excited"), // Positive energy
            Emotion::Embarrassed => Some("shy"),
            Emotion::Content => Some("calm"),
            Emotion::Bored => Some("disgruntled"), // Closest match
        }
    }

    /// Converts intensity (0.0-1.0) to Azure styledegree (0.01-2.0).
    ///
    /// - Low intensity (0.0-0.3) → 0.5-0.8
    /// - Medium intensity (0.3-0.7) → 0.8-1.2
    /// - High intensity (0.7-1.0) → 1.2-2.0
    fn intensity_to_styledegree(intensity: f32) -> f32 {
        // Linear mapping: 0.0 -> 0.5, 0.5 -> 1.0, 1.0 -> 2.0
        0.5 + (intensity * 1.5)
    }

    /// Maps delivery style to a speed modifier.
    fn style_to_speed(style: &DeliveryStyle) -> Option<f32> {
        match style {
            DeliveryStyle::Normal => None,
            DeliveryStyle::Whispered => None, // Azure has a separate whisper effect
            DeliveryStyle::Shouted => Some(1.1),
            DeliveryStyle::Rushed => Some(1.3),
            DeliveryStyle::Measured => Some(0.8),
            DeliveryStyle::Monotone => Some(0.9),
            DeliveryStyle::Expressive => Some(1.05),
            DeliveryStyle::Professional => Some(0.95),
            DeliveryStyle::Casual => Some(1.1),
            DeliveryStyle::Storytelling => Some(0.9),
            DeliveryStyle::Soft => Some(0.95),
            DeliveryStyle::Loud => Some(1.05),
            DeliveryStyle::Cheerful => Some(1.1),
            DeliveryStyle::Serious => Some(0.9),
            DeliveryStyle::Formal => Some(0.95),
        }
    }
}

impl EmotionMapper for AzureEmotionMapper {
    fn get_support(&self) -> ProviderEmotionSupport {
        ProviderEmotionSupport {
            provider_id: "azure",
            supports_emotions: true,
            supported_emotions: AZURE_SUPPORTED_EMOTIONS,
            supports_intensity: true,
            supports_style: true,
            supports_free_description: false,
            method: EmotionMethod::Ssml,
        }
    }

    fn map_emotion(&self, config: &EmotionConfig) -> MappedEmotion {
        let mut mapped = MappedEmotion::empty();

        // Warn if free-form description was provided
        if config.description.is_some() {
            mapped.add_warning(
                "Azure does not support free-form emotion descriptions; using SSML express-as styles instead"
            );
        }

        // Map emotion to SSML style
        if let Some(emotion) = &config.emotion {
            if let Some(style) = Self::emotion_to_style(emotion) {
                mapped.ssml_style = Some(style.to_string());

                // Apply intensity as styledegree
                let intensity = config.effective_intensity();
                let styledegree = Self::intensity_to_styledegree(intensity);
                mapped.ssml_style_degree = Some(styledegree);
            } else if *emotion != Emotion::Neutral {
                mapped.add_warning(format!(
                    "Emotion '{}' has no direct Azure express-as style",
                    emotion
                ));
            }

            // Check if it's in the officially supported list
            if !AZURE_SUPPORTED_EMOTIONS.contains(emotion) && *emotion != Emotion::Neutral {
                mapped.add_warning(format!(
                    "Emotion '{}' may not be supported by all Azure voices",
                    emotion
                ));
            }
        }

        // Apply delivery style speed modifier
        if let Some(style) = &config.style {
            if let Some(speed) = Self::style_to_speed(style) {
                mapped.speed = Some(speed);
            }

            // Special case for whispered
            if *style == DeliveryStyle::Whispered {
                // Azure uses a separate SSML element for whisper
                // We'll note this but can't fully express it through MappedEmotion
                mapped.add_warning(
                    "Whispered style requires Azure-specific SSML; consider using custom SSML",
                );
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
    use crate::core::emotion::types::EmotionIntensity;

    #[test]
    fn test_azure_mapper_support() {
        let mapper = AzureEmotionMapper::new();
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "azure");
        assert!(support.supports_emotions);
        assert!(support.supports_intensity);
        assert!(support.supports_style);
        assert!(!support.supports_free_description);
        assert_eq!(support.method, EmotionMethod::Ssml);
    }

    #[test]
    fn test_azure_mapper_supported_emotions() {
        let mapper = AzureEmotionMapper::new();

        assert!(mapper.supports_emotion(&Emotion::Happy));
        assert!(mapper.supports_emotion(&Emotion::Sad));
        assert!(mapper.supports_emotion(&Emotion::Angry));
        assert!(mapper.supports_emotion(&Emotion::Calm));
        assert!(mapper.supports_emotion(&Emotion::Empathetic));
    }

    #[test]
    fn test_azure_mapper_empty_config() {
        let mapper = AzureEmotionMapper::new();
        let config = EmotionConfig::new();

        let mapped = mapper.map_emotion(&config);
        assert!(!mapped.has_modifications());
    }

    #[test]
    fn test_azure_mapper_happy_emotion() {
        let mapper = AzureEmotionMapper::new();
        let config = EmotionConfig::with_emotion(Emotion::Happy);

        let mapped = mapper.map_emotion(&config);
        assert_eq!(mapped.ssml_style, Some("cheerful".to_string()));
        assert!(mapped.ssml_style_degree.is_some());
    }

    #[test]
    fn test_azure_mapper_sad_emotion() {
        let mapper = AzureEmotionMapper::new();
        let config = EmotionConfig::with_emotion(Emotion::Sad);

        let mapped = mapper.map_emotion(&config);
        assert_eq!(mapped.ssml_style, Some("sad".to_string()));
    }

    #[test]
    fn test_azure_mapper_neutral_no_style() {
        let mapper = AzureEmotionMapper::new();
        let config = EmotionConfig::with_emotion(Emotion::Neutral);

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.ssml_style.is_none());
        assert!(!mapped.has_warnings());
    }

    #[test]
    fn test_azure_mapper_intensity_affects_styledegree() {
        let mapper = AzureEmotionMapper::new();

        // High intensity
        let config_high = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .intensity(EmotionIntensity::from_f32(1.0));
        let mapped_high = mapper.map_emotion(&config_high);

        // Low intensity
        let config_low = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .intensity(EmotionIntensity::from_f32(0.0));
        let mapped_low = mapper.map_emotion(&config_low);

        // High intensity should have higher styledegree
        assert!(mapped_high.ssml_style_degree.unwrap() > mapped_low.ssml_style_degree.unwrap());
    }

    #[test]
    fn test_azure_mapper_styledegree_range() {
        // Test minimum intensity
        let min_degree = AzureEmotionMapper::intensity_to_styledegree(0.0);
        assert!((min_degree - 0.5).abs() < 0.01);

        // Test maximum intensity
        let max_degree = AzureEmotionMapper::intensity_to_styledegree(1.0);
        assert!((max_degree - 2.0).abs() < 0.01);

        // Test middle intensity
        let mid_degree = AzureEmotionMapper::intensity_to_styledegree(0.5);
        assert!((mid_degree - 1.25).abs() < 0.01);
    }

    #[test]
    fn test_azure_mapper_delivery_style_speed() {
        let mapper = AzureEmotionMapper::new();

        // Rushed should increase speed
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .style(DeliveryStyle::Rushed);
        let mapped = mapper.map_emotion(&config);
        assert!(mapped.speed.unwrap() > 1.0);

        // Measured should decrease speed
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .style(DeliveryStyle::Measured);
        let mapped = mapper.map_emotion(&config);
        assert!(mapped.speed.unwrap() < 1.0);
    }

    #[test]
    fn test_azure_mapper_whispered_warning() {
        let mapper = AzureEmotionMapper::new();
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .style(DeliveryStyle::Whispered);

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.has_warnings());
        assert!(mapped.warnings.iter().any(|w| w.contains("Whispered")));
    }

    #[test]
    fn test_azure_mapper_description_warning() {
        let mapper = AzureEmotionMapper::new();
        let config = EmotionConfig::with_description("happy, energetic");

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.has_warnings());
        assert!(mapped.warnings[0].contains("does not support free-form"));
    }

    #[test]
    fn test_azure_mapper_unsupported_emotion_warning() {
        let mapper = AzureEmotionMapper::new();
        let config = EmotionConfig::with_emotion(Emotion::Sarcastic);

        let mapped = mapper.map_emotion(&config);
        // Should still produce a style (best match)
        assert!(mapped.ssml_style.is_some());
        // But should warn about it
        assert!(mapped.has_warnings());
    }

    #[test]
    fn test_azure_mapper_all_emotions_produce_styles() {
        let mapper = AzureEmotionMapper::new();

        for emotion in Emotion::all() {
            if *emotion == Emotion::Neutral {
                continue; // Neutral is special
            }

            let config = EmotionConfig::with_emotion(*emotion);
            let mapped = mapper.map_emotion(&config);

            // All non-neutral emotions should produce some style
            assert!(
                mapped.ssml_style.is_some(),
                "Emotion {:?} should produce SSML style",
                emotion
            );
        }
    }

    #[test]
    fn test_azure_emotion_style_mappings() {
        let test_cases = [
            (Emotion::Happy, "cheerful"),
            (Emotion::Sad, "sad"),
            (Emotion::Angry, "angry"),
            (Emotion::Fearful, "terrified"),
            (Emotion::Excited, "excited"),
            (Emotion::Calm, "calm"),
            (Emotion::Empathetic, "empathetic"),
            (Emotion::Hopeful, "hopeful"),
            (Emotion::Disappointed, "depressed"),
        ];

        for (emotion, expected_style) in test_cases {
            let style = AzureEmotionMapper::emotion_to_style(&emotion);
            assert_eq!(
                style,
                Some(expected_style),
                "Emotion {:?} should map to style '{}'",
                emotion,
                expected_style
            );
        }
    }
}

//! Hume AI emotion mapper.
//!
//! Maps standardized emotions to Hume's natural language descriptions.
//! Hume AI's Octave TTS uses free-form text descriptions for emotion
//! control, making it the most flexible provider for emotional expression.
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::emotion::{EmotionConfig, Emotion, DeliveryStyle};
//! use waav_gateway::core::emotion::mappers::HumeEmotionMapper;
//! use waav_gateway::core::emotion::EmotionMapper;
//!
//! let mapper = HumeEmotionMapper;
//! let config = EmotionConfig::new()
//!     .emotion(Emotion::Happy)
//!     .style(DeliveryStyle::Expressive);
//!
//! let mapped = mapper.map_emotion(&config);
//! assert!(mapped.description.unwrap().contains("happy"));
//! ```

use crate::core::emotion::mapper::{
    EmotionMapper, EmotionMethod, MappedEmotion, ProviderEmotionSupport,
};
use crate::core::emotion::types::{DeliveryStyle, Emotion, EmotionConfig};

/// Maximum length for Hume description field.
pub const MAX_DESCRIPTION_LENGTH: usize = 100;

/// All emotions supported by Hume (natural language = all)
static HUME_SUPPORTED_EMOTIONS: &[Emotion] = Emotion::all();

/// Emotion mapper for Hume AI Octave TTS.
///
/// Hume uses natural language descriptions for emotion control,
/// supporting any emotion that can be described in text.
#[derive(Debug, Clone, Copy, Default)]
pub struct HumeEmotionMapper;

impl HumeEmotionMapper {
    /// Creates a new Hume emotion mapper.
    #[inline]
    pub const fn new() -> Self {
        Self
    }

    /// Maps an emotion to its natural language equivalent.
    fn emotion_to_description(emotion: &Emotion, intensity: f32) -> String {
        let base = match emotion {
            Emotion::Neutral => return String::new(), // No description for neutral
            Emotion::Happy => "happy, joyful",
            Emotion::Sad => "sad, melancholic",
            Emotion::Angry => "angry, frustrated",
            Emotion::Fearful => "frightened, scared",
            Emotion::Surprised => "surprised, astonished",
            Emotion::Disgusted => "disgusted, repulsed",
            Emotion::Excited => "excited, enthusiastic, energetic",
            Emotion::Calm => "calm, peaceful, relaxed",
            Emotion::Anxious => "anxious, nervous, worried",
            Emotion::Confident => "confident, assured, authoritative",
            Emotion::Confused => "confused, uncertain, puzzled",
            Emotion::Empathetic => "empathetic, understanding, compassionate",
            Emotion::Sarcastic => "sarcastic, ironic, dry",
            Emotion::Hopeful => "hopeful, optimistic, encouraging",
            Emotion::Disappointed => "disappointed, let down, dejected",
            Emotion::Curious => "curious, interested, inquisitive",
            Emotion::Grateful => "grateful, thankful, appreciative",
            Emotion::Proud => "proud, accomplished, satisfied",
            Emotion::Embarrassed => "embarrassed, sheepish, awkward",
            Emotion::Content => "content, satisfied, at peace",
            Emotion::Bored => "bored, uninterested, disengaged",
        };

        // Add intensity modifier
        if intensity >= 0.8 {
            format!("very {base}")
        } else if intensity <= 0.3 {
            format!("slightly {base}")
        } else {
            base.to_string()
        }
    }

    /// Maps a delivery style to its description modifier.
    fn style_to_description(style: &DeliveryStyle) -> Option<&'static str> {
        match style {
            DeliveryStyle::Normal => None,
            DeliveryStyle::Whispered => Some("whispered"),
            DeliveryStyle::Shouted => Some("shouted, loud"),
            DeliveryStyle::Rushed => Some("rushed, urgent, fast"),
            DeliveryStyle::Measured => Some("measured, deliberate, slow"),
            DeliveryStyle::Monotone => Some("monotone, flat"),
            DeliveryStyle::Expressive => Some("expressive, animated"),
            DeliveryStyle::Professional => Some("professional, business-like"),
            DeliveryStyle::Casual => Some("casual, conversational"),
            DeliveryStyle::Storytelling => Some("storytelling, narrative, engaging"),
            DeliveryStyle::Soft => Some("soft, gentle, tender"),
            DeliveryStyle::Loud => Some("loud, strong, emphatic"),
            DeliveryStyle::Cheerful => Some("cheerful, upbeat, bright"),
            DeliveryStyle::Serious => Some("serious, grave, solemn"),
            DeliveryStyle::Formal => Some("formal, proper, polished"),
        }
    }

    /// Truncates description to max length while preserving word boundaries.
    /// Safely handles UTF-8 multi-byte characters by finding char boundaries.
    fn truncate_description(description: &str) -> String {
        if description.len() <= MAX_DESCRIPTION_LENGTH {
            return description.to_string();
        }

        // Find the last valid UTF-8 char boundary at or before MAX_DESCRIPTION_LENGTH
        // This prevents panics when MAX_DESCRIPTION_LENGTH falls in the middle of a multi-byte char
        let safe_end = description
            .char_indices()
            .take_while(|(i, _)| *i < MAX_DESCRIPTION_LENGTH)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);

        if safe_end == 0 {
            return String::new();
        }

        let truncated = &description[..safe_end];

        // Find last comma or space before the limit for word boundary
        if let Some(pos) = truncated.rfind(|c| c == ',' || c == ' ') {
            // Trim both whitespace and trailing commas
            description[..pos]
                .trim()
                .trim_end_matches(',')
                .trim()
                .to_string()
        } else {
            truncated.trim().to_string()
        }
    }
}

impl EmotionMapper for HumeEmotionMapper {
    fn get_support(&self) -> ProviderEmotionSupport {
        ProviderEmotionSupport {
            provider_id: "hume",
            supports_emotions: true,
            supported_emotions: HUME_SUPPORTED_EMOTIONS,
            supports_intensity: true,
            supports_style: true,
            supports_free_description: true,
            method: EmotionMethod::NaturalLanguage,
        }
    }

    fn map_emotion(&self, config: &EmotionConfig) -> MappedEmotion {
        // If a free-form description is provided, use it directly
        if let Some(description) = &config.description {
            let truncated = Self::truncate_description(description);
            let mut mapped = MappedEmotion::with_description(&truncated);

            if truncated.len() < description.len() {
                mapped.add_warning(format!(
                    "Description truncated to {} characters",
                    MAX_DESCRIPTION_LENGTH
                ));
            }

            return mapped;
        }

        // Build description from emotion and style
        let mut parts = Vec::new();

        // Add emotion description
        if let Some(emotion) = &config.emotion {
            let intensity = config.effective_intensity();
            let emotion_desc = Self::emotion_to_description(emotion, intensity);
            if !emotion_desc.is_empty() {
                parts.push(emotion_desc);
            }
        }

        // Add style description
        if let Some(style) = &config.style {
            if let Some(style_desc) = Self::style_to_description(style) {
                parts.push(style_desc.to_string());
            }
        }

        // Add context if it provides useful hints
        if let Some(context) = &config.context {
            // Only include short context hints
            if context.len() <= 30 {
                parts.push(context.clone());
            }
        }

        // Combine into final description
        if parts.is_empty() {
            MappedEmotion::empty()
        } else {
            let description = parts.join(", ");
            let truncated = Self::truncate_description(&description);
            MappedEmotion::with_description(truncated)
        }
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
    fn test_hume_mapper_support() {
        let mapper = HumeEmotionMapper::new();
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "hume");
        assert!(support.supports_emotions);
        assert!(support.supports_intensity);
        assert!(support.supports_style);
        assert!(support.supports_free_description);
        assert_eq!(support.method, EmotionMethod::NaturalLanguage);
    }

    #[test]
    fn test_hume_mapper_all_emotions_supported() {
        let mapper = HumeEmotionMapper::new();

        for emotion in Emotion::all() {
            assert!(
                mapper.supports_emotion(emotion),
                "Emotion {:?} should be supported",
                emotion
            );
        }
    }

    #[test]
    fn test_hume_mapper_free_description() {
        let mapper = HumeEmotionMapper::new();
        let config = EmotionConfig::with_description("warm, friendly, inviting");

        let mapped = mapper.map_emotion(&config);
        assert_eq!(
            mapped.description,
            Some("warm, friendly, inviting".to_string())
        );
        assert!(!mapped.has_warnings());
    }

    #[test]
    fn test_hume_mapper_description_truncation() {
        let mapper = HumeEmotionMapper::new();
        let long_description = "a".repeat(150);
        let config = EmotionConfig::with_description(long_description);

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.description.as_ref().unwrap().len() <= MAX_DESCRIPTION_LENGTH);
        assert!(mapped.has_warnings());
        assert!(mapped.warnings[0].contains("truncated"));
    }

    #[test]
    fn test_hume_mapper_emotion_only() {
        let mapper = HumeEmotionMapper::new();
        let config = EmotionConfig::with_emotion(Emotion::Happy);

        let mapped = mapper.map_emotion(&config);
        let desc = mapped.description.unwrap();
        assert!(desc.contains("happy") || desc.contains("joyful"));
    }

    #[test]
    fn test_hume_mapper_emotion_with_high_intensity() {
        let mapper = HumeEmotionMapper::new();
        let config = EmotionConfig::new()
            .emotion(Emotion::Angry)
            .intensity(EmotionIntensity::from_f32(0.9));

        let mapped = mapper.map_emotion(&config);
        let desc = mapped.description.unwrap();
        assert!(desc.contains("very"));
        assert!(desc.contains("angry") || desc.contains("frustrated"));
    }

    #[test]
    fn test_hume_mapper_emotion_with_low_intensity() {
        let mapper = HumeEmotionMapper::new();
        let config = EmotionConfig::new()
            .emotion(Emotion::Sad)
            .intensity(EmotionIntensity::from_f32(0.2));

        let mapped = mapper.map_emotion(&config);
        let desc = mapped.description.unwrap();
        assert!(desc.contains("slightly"));
    }

    #[test]
    fn test_hume_mapper_with_style() {
        let mapper = HumeEmotionMapper::new();
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .style(DeliveryStyle::Whispered);

        let mapped = mapper.map_emotion(&config);
        let desc = mapped.description.unwrap();
        assert!(desc.contains("happy") || desc.contains("joyful"));
        assert!(desc.contains("whispered"));
    }

    #[test]
    fn test_hume_mapper_neutral_emotion() {
        let mapper = HumeEmotionMapper::new();
        let config = EmotionConfig::with_emotion(Emotion::Neutral);

        let mapped = mapper.map_emotion(&config);
        // Neutral should produce empty or no description
        assert!(
            mapped.description.is_none()
                || mapped
                    .description
                    .as_ref()
                    .map(|s| s.is_empty())
                    .unwrap_or(true)
        );
    }

    #[test]
    fn test_hume_mapper_empty_config() {
        let mapper = HumeEmotionMapper::new();
        let config = EmotionConfig::new();

        let mapped = mapper.map_emotion(&config);
        assert!(!mapped.has_modifications());
    }

    #[test]
    fn test_hume_mapper_all_delivery_styles() {
        let mapper = HumeEmotionMapper::new();

        let styles = [
            DeliveryStyle::Normal,
            DeliveryStyle::Whispered,
            DeliveryStyle::Shouted,
            DeliveryStyle::Rushed,
            DeliveryStyle::Measured,
            DeliveryStyle::Monotone,
            DeliveryStyle::Expressive,
            DeliveryStyle::Professional,
            DeliveryStyle::Casual,
            DeliveryStyle::Storytelling,
            DeliveryStyle::Soft,
            DeliveryStyle::Loud,
            DeliveryStyle::Cheerful,
            DeliveryStyle::Serious,
            DeliveryStyle::Formal,
        ];

        for style in styles {
            let config = EmotionConfig::new().emotion(Emotion::Happy).style(style);

            let mapped = mapper.map_emotion(&config);

            if style != DeliveryStyle::Normal {
                let desc = mapped.description.unwrap();
                assert!(
                    !desc.is_empty(),
                    "Style {:?} should produce description",
                    style
                );
            }
        }
    }

    #[test]
    fn test_hume_mapper_with_context() {
        let mapper = HumeEmotionMapper::new();
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .context("greeting");

        let mapped = mapper.map_emotion(&config);
        let desc = mapped.description.unwrap();
        assert!(desc.contains("greeting"));
    }

    #[test]
    fn test_hume_mapper_long_context_ignored() {
        let mapper = HumeEmotionMapper::new();
        let config = EmotionConfig::new().emotion(Emotion::Happy).context(
            "this is a very long context that should be ignored because it exceeds the limit",
        );

        let mapped = mapper.map_emotion(&config);
        let desc = mapped.description.unwrap();
        assert!(!desc.contains("very long context"));
    }

    #[test]
    fn test_truncate_preserves_word_boundaries() {
        // This string is 118 characters - exceeds the 100 char limit
        let long_desc = "happy, joyful, excited, enthusiastic, energetic, positive, cheerful, delighted, pleased, content, wonderful";
        let truncated = HumeEmotionMapper::truncate_description(long_desc);

        assert!(truncated.len() <= MAX_DESCRIPTION_LENGTH);
        // Should not end mid-word or with comma
        assert!(!truncated.ends_with(','));
        // After truncation at word boundary, should end with "content" (the last complete word before limit)
        assert!(truncated.ends_with("content") || truncated.ends_with("pleased"));
    }

    #[test]
    fn test_hume_mapper_description_priority() {
        let mapper = HumeEmotionMapper::new();

        // Description should take priority over emotion
        let config = EmotionConfig::new()
            .emotion(Emotion::Angry) // This should be ignored
            .description("calm, peaceful"); // This should be used

        let mapped = mapper.map_emotion(&config);
        let desc = mapped.description.unwrap();

        assert!(desc.contains("calm"));
        assert!(!desc.contains("angry"));
    }
}

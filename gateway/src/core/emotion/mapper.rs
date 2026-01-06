//! Emotion mapper trait and provider capability detection.
//!
//! This module defines the trait for mapping standardized emotions to
//! provider-specific formats, as well as capability detection for
//! determining what features each provider supports.

use super::types::{DeliveryStyle, Emotion, EmotionConfig};
use serde::{Deserialize, Serialize};

// =============================================================================
// Emotion Method Enum
// =============================================================================

/// Method used by a provider to express emotions.
///
/// Different TTS providers use different mechanisms to control
/// emotional expression in synthesized speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmotionMethod {
    /// Natural language descriptions (Hume AI, OpenAI gpt-4o-mini-tts)
    ///
    /// Example: `description: "happy, energetic, welcoming"`
    NaturalLanguage,

    /// Audio tags embedded in text (ElevenLabs v3)
    ///
    /// Example: `[happy] Hello world!`
    AudioTags,

    /// SSML express-as styles (Azure, Google with limited support)
    ///
    /// Example: `<mstts:express-as style="cheerful">Hello!</mstts:express-as>`
    Ssml,

    /// Voice settings parameters (ElevenLabs stability/style)
    ///
    /// Example: `stability: 0.3, similarity_boost: 0.8`
    VoiceSettings,

    /// No emotion support (Deepgram, Cartesia basic)
    None,
}

impl EmotionMethod {
    /// Returns whether this method supports fine-grained emotion control.
    #[inline]
    pub fn supports_custom_emotions(&self) -> bool {
        matches!(self, EmotionMethod::NaturalLanguage | EmotionMethod::Ssml)
    }

    /// Returns whether this method allows free-form descriptions.
    #[inline]
    pub fn supports_free_description(&self) -> bool {
        matches!(self, EmotionMethod::NaturalLanguage)
    }
}

// =============================================================================
// Provider Emotion Support
// =============================================================================

/// Describes the emotion capabilities of a TTS provider.
///
/// This structure allows the system to understand what emotions
/// each provider supports and how to gracefully degrade when
/// requested emotions aren't available.
#[derive(Debug, Clone)]
pub struct ProviderEmotionSupport {
    /// Provider identifier (e.g., "hume", "elevenlabs", "azure")
    pub provider_id: &'static str,

    /// Whether the provider supports any emotion control
    pub supports_emotions: bool,

    /// List of emotions this provider explicitly supports
    pub supported_emotions: &'static [Emotion],

    /// Whether intensity control is supported
    pub supports_intensity: bool,

    /// Whether delivery style is supported
    pub supports_style: bool,

    /// Whether free-form descriptions are supported
    pub supports_free_description: bool,

    /// The method used to express emotions
    pub method: EmotionMethod,
}

impl ProviderEmotionSupport {
    /// Returns whether a specific emotion is supported.
    #[inline]
    pub fn supports_emotion(&self, emotion: &Emotion) -> bool {
        if !self.supports_emotions {
            return false;
        }
        self.supported_emotions.contains(emotion)
    }

    /// Returns whether any emotion features are available.
    #[inline]
    pub fn has_any_support(&self) -> bool {
        self.supports_emotions || self.supports_style || self.supports_free_description
    }
}

// =============================================================================
// Mapped Emotion Output
// =============================================================================

/// The result of mapping an emotion configuration to a provider format.
///
/// This structure contains all the provider-specific data needed
/// to apply the emotion, along with any warnings about unsupported features.
#[derive(Debug, Clone, Default)]
pub struct MappedEmotion {
    /// Natural language description (for Hume, OpenAI)
    pub description: Option<String>,

    /// SSML content modifications (for Azure)
    pub ssml_style: Option<String>,
    pub ssml_style_degree: Option<f32>,

    /// Voice settings adjustments (for ElevenLabs)
    pub stability: Option<f32>,
    pub similarity_boost: Option<f32>,
    pub style: Option<f32>,

    /// Audio tags to prepend to text (for ElevenLabs v3)
    pub audio_tags: Option<String>,

    /// Speed/rate adjustment (0.5 to 2.0)
    pub speed: Option<f32>,

    /// Warnings about unsupported features
    pub warnings: Vec<String>,
}

impl MappedEmotion {
    /// Creates an empty mapped emotion with no modifications.
    #[inline]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Creates a mapped emotion with just a description.
    #[inline]
    pub fn with_description(description: impl Into<String>) -> Self {
        Self {
            description: Some(description.into()),
            ..Default::default()
        }
    }

    /// Creates a mapped emotion with just SSML style.
    #[inline]
    pub fn with_ssml_style(style: impl Into<String>, degree: Option<f32>) -> Self {
        Self {
            ssml_style: Some(style.into()),
            ssml_style_degree: degree,
            ..Default::default()
        }
    }

    /// Creates a mapped emotion with voice settings.
    #[inline]
    pub fn with_voice_settings(stability: f32, similarity_boost: f32, style: f32) -> Self {
        Self {
            stability: Some(stability),
            similarity_boost: Some(similarity_boost),
            style: Some(style),
            ..Default::default()
        }
    }

    /// Adds a warning message.
    #[inline]
    pub fn add_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    /// Returns whether there are any warnings.
    #[inline]
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Returns whether this mapping made any changes.
    #[inline]
    pub fn has_modifications(&self) -> bool {
        self.description.is_some()
            || self.ssml_style.is_some()
            || self.stability.is_some()
            || self.audio_tags.is_some()
            || self.speed.is_some()
    }

    /// Formats all warnings as a single string.
    pub fn format_warnings(&self) -> Option<String> {
        if self.warnings.is_empty() {
            None
        } else {
            Some(self.warnings.join("; "))
        }
    }
}

// =============================================================================
// Emotion Mapper Trait
// =============================================================================

/// Trait for mapping standardized emotions to provider-specific formats.
///
/// Each TTS provider has a different way of handling emotions:
/// - Hume uses natural language descriptions
/// - Azure uses SSML express-as styles
/// - ElevenLabs uses voice settings (stability, style)
/// - Some providers don't support emotions at all
///
/// The mapper converts our unified `EmotionConfig` to the appropriate
/// provider format, generating warnings when features aren't supported.
///
/// # Implementation Notes
///
/// Implementors should:
/// 1. Map emotions to the closest available provider equivalent
/// 2. Add warnings when exact matches aren't available
/// 3. Handle intensity appropriately for the provider
/// 4. Support free-form descriptions where applicable
pub trait EmotionMapper: Send + Sync {
    /// Returns the provider's emotion support capabilities.
    fn get_support(&self) -> ProviderEmotionSupport;

    /// Maps an emotion configuration to provider-specific format.
    ///
    /// # Arguments
    ///
    /// * `config` - The emotion configuration to map
    ///
    /// # Returns
    ///
    /// A `MappedEmotion` containing provider-specific settings and any warnings.
    fn map_emotion(&self, config: &EmotionConfig) -> MappedEmotion;

    /// Returns the provider identifier.
    fn provider_id(&self) -> &'static str {
        self.get_support().provider_id
    }

    /// Returns whether a specific emotion is supported.
    fn supports_emotion(&self, emotion: &Emotion) -> bool {
        self.get_support().supports_emotion(emotion)
    }

    /// Returns whether delivery styles are supported.
    fn supports_style(&self, _style: &DeliveryStyle) -> bool {
        self.get_support().supports_style
    }

    /// Returns whether free-form descriptions are supported.
    fn supports_free_description(&self) -> bool {
        self.get_support().supports_free_description
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emotion_method_supports_custom() {
        assert!(EmotionMethod::NaturalLanguage.supports_custom_emotions());
        assert!(EmotionMethod::Ssml.supports_custom_emotions());
        assert!(!EmotionMethod::VoiceSettings.supports_custom_emotions());
        assert!(!EmotionMethod::None.supports_custom_emotions());
    }

    #[test]
    fn test_emotion_method_supports_free_description() {
        assert!(EmotionMethod::NaturalLanguage.supports_free_description());
        assert!(!EmotionMethod::Ssml.supports_free_description());
        assert!(!EmotionMethod::AudioTags.supports_free_description());
        assert!(!EmotionMethod::None.supports_free_description());
    }

    #[test]
    fn test_provider_support() {
        let support = ProviderEmotionSupport {
            provider_id: "test",
            supports_emotions: true,
            supported_emotions: &[Emotion::Happy, Emotion::Sad],
            supports_intensity: true,
            supports_style: false,
            supports_free_description: false,
            method: EmotionMethod::Ssml,
        };

        assert!(support.supports_emotion(&Emotion::Happy));
        assert!(support.supports_emotion(&Emotion::Sad));
        assert!(!support.supports_emotion(&Emotion::Angry));
        assert!(support.has_any_support());
    }

    #[test]
    fn test_provider_support_no_emotions() {
        let support = ProviderEmotionSupport {
            provider_id: "test",
            supports_emotions: false,
            supported_emotions: &[],
            supports_intensity: false,
            supports_style: false,
            supports_free_description: false,
            method: EmotionMethod::None,
        };

        assert!(!support.supports_emotion(&Emotion::Happy));
        assert!(!support.has_any_support());
    }

    #[test]
    fn test_mapped_emotion_empty() {
        let mapped = MappedEmotion::empty();
        assert!(!mapped.has_modifications());
        assert!(!mapped.has_warnings());
    }

    #[test]
    fn test_mapped_emotion_with_description() {
        let mapped = MappedEmotion::with_description("happy, energetic");
        assert_eq!(mapped.description, Some("happy, energetic".to_string()));
        assert!(mapped.has_modifications());
    }

    #[test]
    fn test_mapped_emotion_with_ssml() {
        let mapped = MappedEmotion::with_ssml_style("cheerful", Some(1.5));
        assert_eq!(mapped.ssml_style, Some("cheerful".to_string()));
        assert_eq!(mapped.ssml_style_degree, Some(1.5));
        assert!(mapped.has_modifications());
    }

    #[test]
    fn test_mapped_emotion_with_voice_settings() {
        let mapped = MappedEmotion::with_voice_settings(0.3, 0.8, 0.5);
        assert_eq!(mapped.stability, Some(0.3));
        assert_eq!(mapped.similarity_boost, Some(0.8));
        assert_eq!(mapped.style, Some(0.5));
        assert!(mapped.has_modifications());
    }

    #[test]
    fn test_mapped_emotion_warnings() {
        let mut mapped = MappedEmotion::empty();
        assert!(!mapped.has_warnings());

        mapped.add_warning("Emotion 'sarcastic' not supported");
        assert!(mapped.has_warnings());
        assert_eq!(mapped.warnings.len(), 1);

        mapped.add_warning("Intensity ignored");
        assert_eq!(mapped.warnings.len(), 2);

        let formatted = mapped.format_warnings();
        assert!(formatted.is_some());
        assert!(formatted.unwrap().contains("sarcastic"));
    }

    #[test]
    fn test_emotion_method_serialization() {
        let method = EmotionMethod::NaturalLanguage;
        let json = serde_json::to_string(&method).unwrap();
        assert_eq!(json, "\"natural_language\"");

        let parsed: EmotionMethod = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, EmotionMethod::NaturalLanguage);
    }
}

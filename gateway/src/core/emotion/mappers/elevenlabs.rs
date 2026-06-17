//! ElevenLabs emotion mapper.
//!
//! Maps standardized emotions to ElevenLabs voice settings.
//! ElevenLabs uses stability, similarity_boost, and style parameters
//! to control emotional expression, rather than explicit emotion tags.
//!
//! # Voice Settings
//!
//! - **Stability** (0.0-1.0): Lower values = more expressive/emotional
//! - **Similarity Boost** (0.0-1.0): Higher values = closer to original voice
//! - **Style** (0.0-1.0): Style exaggeration, only for v2 models
//!
//! # Mapping Strategy
//!
//! Emotions are mapped by adjusting stability:
//! - High energy emotions (happy, excited, angry) → Low stability (0.25-0.4)
//! - Calm/neutral emotions → Medium-high stability (0.5-0.7)
//! - Sad/subdued emotions → High stability (0.6-0.8)

use crate::core::emotion::mapper::{
    EmotionMapper, EmotionMethod, MappedEmotion, ProviderEmotionSupport,
};
use crate::core::emotion::types::{DeliveryStyle, Emotion, EmotionConfig};

/// Emotions that ElevenLabs can express through voice settings
static ELEVENLABS_SUPPORTED_EMOTIONS: &[Emotion] = &[
    Emotion::Neutral,
    Emotion::Happy,
    Emotion::Sad,
    Emotion::Angry,
    Emotion::Excited,
    Emotion::Calm,
    Emotion::Anxious,
    Emotion::Confident,
];

/// Default voice settings
const DEFAULT_STABILITY: f32 = 0.5;
const DEFAULT_SIMILARITY_BOOST: f32 = 0.75;
const DEFAULT_STYLE: f32 = 0.0;

/// Emotion mapper for ElevenLabs TTS.
///
/// Maps emotions to voice settings (stability, similarity_boost, style).
#[derive(Debug, Clone, Copy, Default)]
pub struct ElevenLabsEmotionMapper;

impl ElevenLabsEmotionMapper {
    /// Creates a new ElevenLabs emotion mapper.
    #[inline]
    pub const fn new() -> Self {
        Self
    }

    /// Maps an emotion to voice settings.
    ///
    /// Returns (stability, similarity_boost, style) tuple.
    fn emotion_to_settings(emotion: &Emotion, intensity: f32) -> (f32, f32, f32) {
        // Base settings for each emotion
        let (base_stability, base_similarity, base_style) = match emotion {
            Emotion::Neutral => (0.5, 0.75, 0.0),
            Emotion::Happy => (0.35, 0.75, 0.3),
            Emotion::Sad => (0.65, 0.8, 0.1),
            Emotion::Angry => (0.25, 0.7, 0.4),
            Emotion::Fearful => (0.3, 0.75, 0.2),
            Emotion::Surprised => (0.3, 0.7, 0.35),
            Emotion::Disgusted => (0.4, 0.75, 0.25),
            Emotion::Excited => (0.25, 0.7, 0.45),
            Emotion::Calm => (0.7, 0.8, 0.0),
            Emotion::Anxious => (0.35, 0.75, 0.25),
            Emotion::Confident => (0.45, 0.75, 0.35),
            Emotion::Confused => (0.4, 0.75, 0.2),
            Emotion::Empathetic => (0.55, 0.8, 0.15),
            Emotion::Sarcastic => (0.4, 0.7, 0.4),
            Emotion::Hopeful => (0.45, 0.75, 0.25),
            Emotion::Disappointed => (0.55, 0.8, 0.15),
            Emotion::Curious => (0.4, 0.75, 0.3),
            Emotion::Grateful => (0.5, 0.8, 0.2),
            Emotion::Proud => (0.45, 0.75, 0.35),
            Emotion::Embarrassed => (0.5, 0.8, 0.15),
            Emotion::Content => (0.6, 0.8, 0.1),
            Emotion::Bored => (0.7, 0.75, 0.05),
            // P4 widened affective states (voice-settings approximation; the v3 tag
            // path in `map_emotion` carries the open-vocabulary token verbatim).
            Emotion::Amazed => (0.3, 0.7, 0.4),
            Emotion::Amused => (0.35, 0.72, 0.35),
            Emotion::Affectionate => (0.55, 0.82, 0.2),
            Emotion::Intrigued => (0.42, 0.75, 0.3),
            Emotion::Flirtatious => (0.4, 0.75, 0.4),
            Emotion::Frustrated => (0.28, 0.7, 0.4),
            Emotion::Annoyed => (0.35, 0.72, 0.3),
            Emotion::Determined => (0.45, 0.75, 0.3),
            Emotion::Reassuring => (0.6, 0.82, 0.1),
            Emotion::Sympathetic => (0.55, 0.82, 0.15),
            Emotion::Nostalgic => (0.6, 0.8, 0.15),
            Emotion::Serene => (0.72, 0.82, 0.0),
            Emotion::Terrified => (0.25, 0.72, 0.35),
            Emotion::Ecstatic => (0.2, 0.68, 0.5),
            Emotion::Skeptical => (0.5, 0.75, 0.25),
            Emotion::Relieved => (0.55, 0.8, 0.15),
            Emotion::Panicked => (0.2, 0.7, 0.4),
            Emotion::Concerned => (0.45, 0.78, 0.2),
            Emotion::Apologetic => (0.55, 0.8, 0.15),
            Emotion::Resigned => (0.65, 0.78, 0.1),
            Emotion::Wistful => (0.6, 0.8, 0.15),
            Emotion::Contemplative => (0.6, 0.8, 0.1),
        };

        // Apply intensity modifier
        // Higher intensity = more extreme settings
        let intensity_factor = (intensity - 0.5) * 0.3; // -0.15 to +0.15

        let stability = (base_stability - intensity_factor).clamp(0.0, 1.0);
        let style = (base_style + intensity_factor).clamp(0.0, 1.0);

        (stability, base_similarity, style)
    }

    /// Applies delivery style adjustments to voice settings.
    fn apply_style_adjustment(
        style: &DeliveryStyle,
        stability: f32,
        similarity_boost: f32,
        style_value: f32,
    ) -> (f32, f32, f32) {
        match style {
            DeliveryStyle::Normal => (stability, similarity_boost, style_value),
            DeliveryStyle::Whispered => (stability + 0.1, similarity_boost, style_value - 0.1),
            DeliveryStyle::Shouted => (
                stability - 0.15,
                similarity_boost - 0.05,
                style_value + 0.15,
            ),
            DeliveryStyle::Rushed => (stability - 0.1, similarity_boost, style_value + 0.1),
            DeliveryStyle::Measured => {
                (stability + 0.15, similarity_boost + 0.05, style_value - 0.1)
            }
            DeliveryStyle::Monotone => (0.8, similarity_boost, 0.0),
            DeliveryStyle::Expressive => {
                (stability - 0.2, similarity_boost - 0.05, style_value + 0.2)
            }
            DeliveryStyle::Professional => (0.6, similarity_boost + 0.05, style_value),
            DeliveryStyle::Casual => (stability - 0.1, similarity_boost, style_value + 0.1),
            DeliveryStyle::Storytelling => (stability - 0.15, similarity_boost, style_value + 0.15),
            DeliveryStyle::Loud => (stability - 0.1, similarity_boost - 0.05, style_value + 0.1),
            DeliveryStyle::Formal => (0.6, similarity_boost + 0.05, style_value),
            // Gentle / relaxed narration → softer, more stable.
            DeliveryStyle::Gentle | DeliveryStyle::NarrationRelaxed | DeliveryStyle::PoetryReading => {
                (stability + 0.1, similarity_boost + 0.05, style_value - 0.1)
            }
            // Animated scenario reads → more expressive.
            DeliveryStyle::SportsCommentaryExcited | DeliveryStyle::AdvertisementUpbeat => {
                (stability - 0.2, similarity_boost - 0.05, style_value + 0.2)
            }
            // Composed scenario reads (newscast/customer-service/assistant/documentary/…)
            // → steady, professional register.
            _ => (0.55, similarity_boost + 0.03, style_value),
        }
    }

    /// Maps a canonical [`Emotion`] to an ElevenLabs **v3 inline audio tag** (the
    /// open-vocabulary bracket tag that gets PREPENDED to the input text on v3,
    /// e.g. `[excited]`). v2/turbo ignore tags and fall back to voice settings, so
    /// the handler only injects this when the model is v3-capable.
    fn emotion_to_v3_tag(emotion: &Emotion) -> Option<&'static str> {
        match emotion {
            Emotion::Neutral => None,
            Emotion::Happy => Some("[happy]"),
            Emotion::Sad => Some("[sad]"),
            Emotion::Angry => Some("[angry]"),
            Emotion::Fearful => Some("[nervous]"),
            Emotion::Surprised => Some("[gasps]"),
            Emotion::Disgusted => Some("[disgusted]"),
            Emotion::Excited => Some("[excited]"),
            Emotion::Calm => Some("[calm]"),
            Emotion::Anxious => Some("[nervous]"),
            Emotion::Confident => Some("[confident]"),
            Emotion::Confused => Some("[confused]"),
            Emotion::Empathetic => Some("[empathetic]"),
            Emotion::Sarcastic => Some("[sarcastic]"),
            Emotion::Hopeful => Some("[hopeful]"),
            Emotion::Disappointed => Some("[disappointed]"),
            Emotion::Curious => Some("[curious]"),
            Emotion::Grateful => Some("[grateful]"),
            Emotion::Proud => Some("[proud]"),
            Emotion::Embarrassed => Some("[embarrassed]"),
            Emotion::Content => Some("[content]"),
            Emotion::Bored => Some("[tired]"),
            Emotion::Amazed => Some("[amazed]"),
            Emotion::Amused => Some("[laughs]"),
            Emotion::Affectionate => Some("[affectionate]"),
            Emotion::Intrigued => Some("[curious]"),
            Emotion::Flirtatious => Some("[mischievously]"),
            Emotion::Frustrated => Some("[frustrated]"),
            Emotion::Annoyed => Some("[annoyed]"),
            Emotion::Determined => Some("[determined]"),
            Emotion::Reassuring => Some("[reassuring]"),
            Emotion::Sympathetic => Some("[sympathetic]"),
            Emotion::Nostalgic => Some("[wistful]"),
            Emotion::Serene => Some("[calm]"),
            Emotion::Terrified => Some("[terrified]"),
            Emotion::Ecstatic => Some("[ecstatic]"),
            Emotion::Skeptical => Some("[skeptical]"),
            Emotion::Relieved => Some("[relieved]"),
            Emotion::Panicked => Some("[panicked]"),
            Emotion::Concerned => Some("[concerned]"),
            Emotion::Apologetic => Some("[apologetic]"),
            Emotion::Resigned => Some("[sighs]"),
            Emotion::Wistful => Some("[wistful]"),
            Emotion::Contemplative => Some("[thoughtful]"),
        }
    }

    /// Maps a delivery [`DeliveryStyle`] to an ElevenLabs v3 inline tag where one
    /// exists for the *mechanism* (whisper/shout/rushed/…). Scenario framings have
    /// no native v3 tag and return `None` (the voice-settings path still applies).
    fn delivery_style_to_v3_tag(style: &DeliveryStyle) -> Option<&'static str> {
        match style {
            DeliveryStyle::Whispered => Some("[whispers]"),
            DeliveryStyle::Shouted | DeliveryStyle::Loud => Some("[shouting]"),
            DeliveryStyle::Rushed => Some("[rushed]"),
            DeliveryStyle::Measured => Some("[drawn out]"),
            DeliveryStyle::Gentle => Some("[gently]"),
            _ => None,
        }
    }
}

impl EmotionMapper for ElevenLabsEmotionMapper {
    fn get_support(&self) -> ProviderEmotionSupport {
        ProviderEmotionSupport {
            provider_id: "elevenlabs",
            supports_emotions: true,
            supported_emotions: ELEVENLABS_SUPPORTED_EMOTIONS,
            supports_intensity: true,
            supports_style: true,
            supports_free_description: false,
            method: EmotionMethod::VoiceSettings,
        }
    }

    fn map_emotion(&self, config: &EmotionConfig) -> MappedEmotion {
        let mut mapped = MappedEmotion::empty();

        // Warn if free-form description was provided
        if config.description.is_some() {
            mapped.add_warning(
                "ElevenLabs does not support free-form emotion descriptions; using voice settings instead"
            );
        }

        // Start with defaults
        let mut stability = DEFAULT_STABILITY;
        let mut similarity_boost = DEFAULT_SIMILARITY_BOOST;
        let mut style = DEFAULT_STYLE;

        // Apply emotion settings
        if let Some(emotion) = &config.emotion {
            let intensity = config.effective_intensity();
            let (s, sim, st) = Self::emotion_to_settings(emotion, intensity);
            stability = s;
            similarity_boost = sim;
            style = st;

            // Warn about unsupported emotions
            if !ELEVENLABS_SUPPORTED_EMOTIONS.contains(emotion) {
                mapped.add_warning(format!(
                    "Emotion '{}' is not fully supported by ElevenLabs; approximating with voice settings",
                    emotion
                ));
            }
        }

        // Apply delivery style adjustments
        if let Some(delivery_style) = &config.style {
            let (s, sim, st) =
                Self::apply_style_adjustment(delivery_style, stability, similarity_boost, style);
            stability = s.clamp(0.0, 1.0);
            similarity_boost = sim.clamp(0.0, 1.0);
            style = st.clamp(0.0, 1.0);
        }

        // Only set values if they differ from defaults (or if explicitly configured)
        if config.emotion.is_some() || config.style.is_some() {
            mapped.stability = Some(stability);
            mapped.similarity_boost = Some(similarity_boost);
            mapped.style = Some(style);
        }

        // ElevenLabs v3 open-vocabulary inline tags: emit the bracket tag(s) so a
        // v3-capable handler can PREPEND them to the input text. v2/turbo ignore
        // tags and use the voice-settings above, so this is purely additive — the
        // handler decides whether to inject based on the model id.
        let mut tags = String::new();
        if let Some(emotion) = &config.emotion
            && let Some(tag) = Self::emotion_to_v3_tag(emotion)
        {
            tags.push_str(tag);
        }
        if let Some(delivery_style) = &config.style
            && let Some(tag) = Self::delivery_style_to_v3_tag(delivery_style)
        {
            if !tags.is_empty() {
                tags.push(' ');
            }
            tags.push_str(tag);
        }
        if !tags.is_empty() {
            tags.push(' ');
            mapped.inline_tags = Some(tags);
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
    fn test_elevenlabs_mapper_support() {
        let mapper = ElevenLabsEmotionMapper::new();
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "elevenlabs");
        assert!(support.supports_emotions);
        assert!(support.supports_intensity);
        assert!(support.supports_style);
        assert!(!support.supports_free_description);
        assert_eq!(support.method, EmotionMethod::VoiceSettings);
    }

    #[test]
    fn test_elevenlabs_mapper_supported_emotions() {
        let mapper = ElevenLabsEmotionMapper::new();

        assert!(mapper.supports_emotion(&Emotion::Happy));
        assert!(mapper.supports_emotion(&Emotion::Sad));
        assert!(mapper.supports_emotion(&Emotion::Angry));
        assert!(mapper.supports_emotion(&Emotion::Excited));
        assert!(mapper.supports_emotion(&Emotion::Calm));
    }

    #[test]
    fn test_elevenlabs_mapper_empty_config() {
        let mapper = ElevenLabsEmotionMapper::new();
        let config = EmotionConfig::new();

        let mapped = mapper.map_emotion(&config);
        assert!(!mapped.has_modifications());
    }

    #[test]
    fn test_elevenlabs_mapper_happy_emotion() {
        let mapper = ElevenLabsEmotionMapper::new();
        let config = EmotionConfig::with_emotion(Emotion::Happy);

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.stability.is_some());
        assert!(mapped.similarity_boost.is_some());
        assert!(mapped.style.is_some());

        // Happy should have lower stability (more expressive)
        assert!(mapped.stability.unwrap() < 0.5);
    }

    #[test]
    fn test_elevenlabs_mapper_calm_emotion() {
        let mapper = ElevenLabsEmotionMapper::new();
        let config = EmotionConfig::with_emotion(Emotion::Calm);

        let mapped = mapper.map_emotion(&config);

        // Calm should have higher stability
        assert!(mapped.stability.unwrap() > 0.5);
    }

    #[test]
    fn test_elevenlabs_mapper_intensity_affects_stability() {
        let mapper = ElevenLabsEmotionMapper::new();

        // High intensity
        let config_high = EmotionConfig::new()
            .emotion(Emotion::Angry)
            .intensity(EmotionIntensity::from_f32(0.9));
        let mapped_high = mapper.map_emotion(&config_high);

        // Low intensity
        let config_low = EmotionConfig::new()
            .emotion(Emotion::Angry)
            .intensity(EmotionIntensity::from_f32(0.2));
        let mapped_low = mapper.map_emotion(&config_low);

        // High intensity should have lower stability
        assert!(mapped_high.stability.unwrap() < mapped_low.stability.unwrap());
    }

    #[test]
    fn test_elevenlabs_mapper_delivery_style() {
        let mapper = ElevenLabsEmotionMapper::new();

        // Expressive style
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .style(DeliveryStyle::Expressive);

        let mapped = mapper.map_emotion(&config);

        // Expressive should lower stability further
        let base_stability = ElevenLabsEmotionMapper::emotion_to_settings(&Emotion::Happy, 0.6).0;
        assert!(mapped.stability.unwrap() < base_stability);
    }

    #[test]
    fn test_elevenlabs_mapper_monotone_style() {
        let mapper = ElevenLabsEmotionMapper::new();
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .style(DeliveryStyle::Monotone);

        let mapped = mapper.map_emotion(&config);

        // Monotone should have high stability and zero style
        assert!(mapped.stability.unwrap() > 0.7);
        assert!(mapped.style.unwrap() < 0.1);
    }

    #[test]
    fn test_elevenlabs_mapper_description_warning() {
        let mapper = ElevenLabsEmotionMapper::new();
        let config = EmotionConfig::with_description("happy, energetic");

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.has_warnings());
        assert!(mapped.warnings[0].contains("does not support free-form"));
    }

    #[test]
    fn test_elevenlabs_mapper_unsupported_emotion_warning() {
        let mapper = ElevenLabsEmotionMapper::new();
        let config = EmotionConfig::with_emotion(Emotion::Sarcastic);

        let mapped = mapper.map_emotion(&config);
        assert!(mapped.has_warnings());
        assert!(mapped.warnings[0].contains("not fully supported"));
    }

    #[test]
    fn test_elevenlabs_mapper_values_clamped() {
        let mapper = ElevenLabsEmotionMapper::new();

        // Very high intensity with expressive style should still be clamped
        let config = EmotionConfig::new()
            .emotion(Emotion::Excited)
            .intensity(EmotionIntensity::from_f32(1.0))
            .style(DeliveryStyle::Expressive);

        let mapped = mapper.map_emotion(&config);

        assert!(mapped.stability.unwrap() >= 0.0);
        assert!(mapped.stability.unwrap() <= 1.0);
        assert!(mapped.style.unwrap() >= 0.0);
        assert!(mapped.style.unwrap() <= 1.0);
    }

    #[test]
    fn test_elevenlabs_mapper_all_emotions() {
        let mapper = ElevenLabsEmotionMapper::new();

        for emotion in Emotion::all() {
            let config = EmotionConfig::with_emotion(*emotion);
            let mapped = mapper.map_emotion(&config);

            // All emotions should produce valid settings
            if let Some(stability) = mapped.stability {
                assert!((0.0..=1.0).contains(&stability));
            }
            if let Some(style) = mapped.style {
                assert!((0.0..=1.0).contains(&style));
            }
        }
    }
}

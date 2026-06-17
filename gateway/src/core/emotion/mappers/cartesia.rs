//! Cartesia TTS emotion mapper (real, replaces the former fallback stub).
//!
//! Cartesia expresses emotion through a **native emotion string** in its API
//! params, not SSML or voice settings. Two forms exist by model:
//!
//! - **Sonic-3** (current): `generation_config.emotion` = a single token chosen
//!   from Cartesia's ~48-value native vocabulary (best results:
//!   `neutral`/`angry`/`excited`/`content`/`sad`/`scared`). Also supports SSML in
//!   the transcript (`<emotion value="angry"/>`), `<speed ratio=…/>`,
//!   `<volume ratio=…/>`, and the `[laughter]` nonverbal tag.
//! - **Sonic-2** (legacy/deprecated): `voice.__experimental_controls.emotion` =
//!   an array of `"name:level"` strings (names: anger/positivity/surprise/sadness/
//!   curiosity; levels: lowest/low/(omit)/high/highest).
//!
//! This mapper maps the canonical [`Emotion`] to the NEAREST of Cartesia's 48
//! Sonic-3 tokens via [`MappedEmotion::native_emotion`], emits the legacy
//! Sonic-2 array via [`MappedEmotion::emotion_array`], and (for `Amused`) emits
//! the `[laughter]` nonverbal via [`MappedEmotion::inline_tags`]. Speed from the
//! delivery style rides [`MappedEmotion::speed`].

use crate::core::emotion::mapper::{
    EmotionMapper, EmotionMethod, MappedEmotion, ProviderEmotionSupport,
};
use crate::core::emotion::types::{DeliveryStyle, Emotion, EmotionConfig};

/// The canonical emotions Cartesia Sonic-3 expresses well. Sonic-3's native
/// vocabulary is broad (≈48 tokens), so nearly the whole affective superset maps
/// to a real token; this list is the subset that lands on its OWN distinct token
/// (drives `supported_emotions` / the "approximated" warning).
static CARTESIA_SUPPORTED_EMOTIONS: &[Emotion] = &[
    Emotion::Neutral,
    Emotion::Happy,
    Emotion::Sad,
    Emotion::Angry,
    Emotion::Fearful,
    Emotion::Surprised,
    Emotion::Disgusted,
    Emotion::Excited,
    Emotion::Calm,
    Emotion::Anxious,
    Emotion::Confident,
    Emotion::Confused,
    Emotion::Sarcastic,
    Emotion::Grateful,
    Emotion::Proud,
    Emotion::Content,
    Emotion::Bored,
    Emotion::Amazed,
    Emotion::Affectionate,
    Emotion::Flirtatious,
    Emotion::Frustrated,
    Emotion::Determined,
    Emotion::Sympathetic,
    Emotion::Nostalgic,
    Emotion::Serene,
    Emotion::Skeptical,
    Emotion::Panicked,
    Emotion::Resigned,
    Emotion::Wistful,
    Emotion::Contemplative,
    Emotion::Ecstatic,
    Emotion::Terrified,
    Emotion::Disappointed,
];

/// Emotion mapper for Cartesia (Sonic-3 native emotion string + Sonic-2 array).
#[derive(Debug, Clone, Copy, Default)]
pub struct CartesiaEmotionMapper;

impl CartesiaEmotionMapper {
    /// Creates a new Cartesia emotion mapper.
    #[inline]
    pub const fn new() -> Self {
        Self
    }

    /// Maps a canonical [`Emotion`] to the NEAREST Cartesia Sonic-3 native token
    /// (one of its ≈48-value vocabulary). `None` for [`Emotion::Neutral`] (Cartesia
    /// treats absent emotion as neutral). This is the single source of truth for
    /// `generation_config.emotion`.
    pub fn emotion_to_native(emotion: &Emotion) -> Option<&'static str> {
        match emotion {
            Emotion::Neutral => None,
            // Direct token matches in Cartesia's vocabulary.
            Emotion::Happy => Some("happy"),
            Emotion::Sad => Some("sad"),
            Emotion::Angry => Some("angry"),
            Emotion::Excited => Some("excited"),
            Emotion::Calm => Some("calm"),
            Emotion::Anxious => Some("anxious"),
            Emotion::Confident => Some("confident"),
            Emotion::Confused => Some("confused"),
            Emotion::Sarcastic => Some("sarcastic"),
            Emotion::Grateful => Some("grateful"),
            Emotion::Proud => Some("proud"),
            Emotion::Content => Some("content"),
            Emotion::Amazed => Some("amazed"),
            Emotion::Surprised => Some("surprised"),
            Emotion::Affectionate => Some("affectionate"),
            Emotion::Flirtatious => Some("flirtatious"),
            Emotion::Frustrated => Some("frustrated"),
            Emotion::Determined => Some("determined"),
            Emotion::Sympathetic => Some("sympathetic"),
            Emotion::Nostalgic => Some("nostalgic"),
            Emotion::Serene => Some("serene"),
            Emotion::Skeptical => Some("skeptical"),
            Emotion::Panicked => Some("panicked"),
            Emotion::Resigned => Some("resigned"),
            Emotion::Wistful => Some("wistful"),
            Emotion::Contemplative => Some("contemplative"),
            Emotion::Ecstatic => Some("euphoric"),
            Emotion::Terrified => Some("scared"),
            Emotion::Disappointed => Some("disappointed"),
            // Nearest-token approximations (no exact 1:1 token).
            Emotion::Fearful => Some("scared"),
            Emotion::Disgusted => Some("disgusted"),
            Emotion::Empathetic => Some("sympathetic"),
            Emotion::Hopeful => Some("anticipation"),
            Emotion::Curious => Some("curious"),
            Emotion::Embarrassed => Some("insecure"),
            Emotion::Bored => Some("tired"),
            Emotion::Amused => Some("happy"),
            Emotion::Intrigued => Some("curious"),
            Emotion::Annoyed => Some("agitated"),
            Emotion::Reassuring => Some("calm"),
            Emotion::Relieved => Some("grateful"),
            Emotion::Concerned => Some("anxious"),
            Emotion::Apologetic => Some("apologetic"),
        }
    }

    /// Maps the canonical [`Emotion`] to a Sonic-2 legacy `"name:level"` array
    /// entry (names limited to anger/positivity/surprise/sadness/curiosity).
    /// `None` when the emotion has no Sonic-2 axis. `intensity` selects the level.
    fn emotion_to_sonic2_array(emotion: &Emotion, intensity: f32) -> Option<String> {
        let level = if intensity >= 0.85 {
            ":highest"
        } else if intensity >= 0.7 {
            ":high"
        } else if intensity <= 0.15 {
            ":lowest"
        } else if intensity <= 0.3 {
            ":low"
        } else {
            "" // omit → default level
        };
        let name = match emotion {
            Emotion::Happy
            | Emotion::Excited
            | Emotion::Ecstatic
            | Emotion::Content
            | Emotion::Proud
            | Emotion::Grateful
            | Emotion::Affectionate
            | Emotion::Amused => "positivity",
            Emotion::Angry | Emotion::Frustrated | Emotion::Annoyed => "anger",
            Emotion::Surprised | Emotion::Amazed => "surprise",
            Emotion::Sad | Emotion::Disappointed | Emotion::Wistful | Emotion::Nostalgic => {
                "sadness"
            }
            Emotion::Curious | Emotion::Intrigued => "curiosity",
            _ => return None,
        };
        Some(format!("{name}{level}"))
    }

    /// Delivery style → Cartesia `<speed ratio>` multiplier (range 0.6–1.5).
    fn style_to_speed(style: &DeliveryStyle) -> Option<f32> {
        match style {
            DeliveryStyle::Rushed | DeliveryStyle::SportsCommentaryExcited => Some(1.3),
            DeliveryStyle::Measured | DeliveryStyle::DocumentaryNarration => Some(0.8),
            DeliveryStyle::NarrationRelaxed | DeliveryStyle::Gentle | DeliveryStyle::PoetryReading => {
                Some(0.9)
            }
            _ => None,
        }
    }
}

impl EmotionMapper for CartesiaEmotionMapper {
    fn get_support(&self) -> ProviderEmotionSupport {
        ProviderEmotionSupport {
            provider_id: "cartesia",
            supports_emotions: true,
            supported_emotions: CARTESIA_SUPPORTED_EMOTIONS,
            supports_intensity: true,
            supports_style: true,
            supports_free_description: false,
            method: EmotionMethod::NativeEmotionString,
        }
    }

    fn map_emotion(&self, config: &EmotionConfig) -> MappedEmotion {
        let mut mapped = MappedEmotion::empty();

        // Free-form descriptions are not a Cartesia mechanism.
        if config.description.is_some() {
            mapped.add_warning(
                "Cartesia does not support free-form emotion descriptions; mapping to the nearest native emotion token instead",
            );
        }

        if let Some(emotion) = &config.emotion {
            if let Some(native) = Self::emotion_to_native(emotion) {
                mapped.native_emotion = Some(native.to_string());

                // Legacy Sonic-2 array (handler uses this only for Sonic-2 voices).
                if let Some(entry) =
                    Self::emotion_to_sonic2_array(emotion, config.effective_intensity())
                {
                    mapped.emotion_array = Some(vec![entry]);
                }

                // `Amused` → emit the [laughter] nonverbal as an inline tag.
                if *emotion == Emotion::Amused {
                    mapped.inline_tags = Some("[laughter] ".to_string());
                }
            } else if *emotion != Emotion::Neutral {
                mapped.add_warning(format!(
                    "Emotion '{emotion}' has no Cartesia native token; using neutral voice"
                ));
            }

            if !CARTESIA_SUPPORTED_EMOTIONS.contains(emotion) && *emotion != Emotion::Neutral {
                mapped.add_warning(format!(
                    "Emotion '{emotion}' is approximated to the nearest Cartesia token '{}'",
                    mapped.native_emotion.as_deref().unwrap_or("neutral")
                ));
            }
        }

        // Delivery style → speed ratio (Cartesia has no scenario styles → warn).
        if let Some(style) = &config.style
            && *style != DeliveryStyle::Normal
        {
            if let Some(speed) = Self::style_to_speed(style) {
                mapped.speed = Some(speed);
            }
            if matches!(
                style,
                DeliveryStyle::Newscast
                    | DeliveryStyle::NewscastCasual
                    | DeliveryStyle::NewscastFormal
                    | DeliveryStyle::CustomerService
                    | DeliveryStyle::Assistant
                    | DeliveryStyle::Chat
                    | DeliveryStyle::AdvertisementUpbeat
                    | DeliveryStyle::SportsCommentary
                    | DeliveryStyle::NarrationProfessional
            ) {
                mapped.add_warning(format!(
                    "Cartesia has no '{style}' scenario style; only speed/emotion are applied"
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
    use crate::core::emotion::types::EmotionIntensity;

    #[test]
    fn test_cartesia_mapper_support() {
        let mapper = CartesiaEmotionMapper::new();
        let support = mapper.get_support();
        assert_eq!(support.provider_id, "cartesia");
        assert!(support.supports_emotions);
        assert!(support.supports_intensity);
        assert!(support.supports_style);
        assert!(!support.supports_free_description);
        assert_eq!(support.method, EmotionMethod::NativeEmotionString);
    }

    #[test]
    fn test_cartesia_native_emotion_direct_tokens() {
        let mapper = CartesiaEmotionMapper::new();
        for (emotion, token) in [
            (Emotion::Happy, "happy"),
            (Emotion::Angry, "angry"),
            (Emotion::Excited, "excited"),
            (Emotion::Sad, "sad"),
            (Emotion::Content, "content"),
        ] {
            let mapped = mapper.map_emotion(&EmotionConfig::with_emotion(emotion));
            assert_eq!(mapped.native_emotion.as_deref(), Some(token));
        }
    }

    #[test]
    fn test_cartesia_native_emotion_approximations() {
        // Fearful/Terrified → "scared", Bored → "tired", Ecstatic → "euphoric".
        let mapper = CartesiaEmotionMapper::new();
        assert_eq!(
            mapper
                .map_emotion(&EmotionConfig::with_emotion(Emotion::Fearful))
                .native_emotion
                .as_deref(),
            Some("scared")
        );
        assert_eq!(
            mapper
                .map_emotion(&EmotionConfig::with_emotion(Emotion::Bored))
                .native_emotion
                .as_deref(),
            Some("tired")
        );
        assert_eq!(
            mapper
                .map_emotion(&EmotionConfig::with_emotion(Emotion::Ecstatic))
                .native_emotion
                .as_deref(),
            Some("euphoric")
        );
    }

    #[test]
    fn test_cartesia_neutral_no_token() {
        let mapper = CartesiaEmotionMapper::new();
        let mapped = mapper.map_emotion(&EmotionConfig::with_emotion(Emotion::Neutral));
        assert!(mapped.native_emotion.is_none());
        assert!(!mapped.has_warnings());
    }

    #[test]
    fn test_cartesia_sonic2_array_with_level() {
        let mapper = CartesiaEmotionMapper::new();
        // High-intensity happy → positivity:high (or :highest).
        let mapped = mapper.map_emotion(
            &EmotionConfig::new()
                .emotion(Emotion::Happy)
                .intensity(EmotionIntensity::from_f32(0.9)),
        );
        let arr = mapped.emotion_array.unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0].starts_with("positivity"), "got {}", arr[0]);
        assert!(arr[0].contains("high"));
    }

    #[test]
    fn test_cartesia_amused_emits_laughter_tag() {
        let mapper = CartesiaEmotionMapper::new();
        let mapped = mapper.map_emotion(&EmotionConfig::with_emotion(Emotion::Amused));
        assert_eq!(mapped.inline_tags.as_deref(), Some("[laughter] "));
    }

    #[test]
    fn test_cartesia_scenario_style_warns() {
        let mapper = CartesiaEmotionMapper::new();
        let mapped = mapper.map_emotion(&EmotionConfig::new().style(DeliveryStyle::Newscast));
        assert!(mapped.has_warnings());
        assert!(mapped.warnings.iter().any(|w| w.contains("scenario")));
    }

    #[test]
    fn test_cartesia_rushed_sets_speed() {
        let mapper = CartesiaEmotionMapper::new();
        let mapped = mapper.map_emotion(
            &EmotionConfig::new()
                .emotion(Emotion::Excited)
                .style(DeliveryStyle::Rushed),
        );
        assert!(mapped.speed.unwrap() > 1.0);
    }

    #[test]
    fn test_cartesia_all_emotions_map_to_a_token() {
        let mapper = CartesiaEmotionMapper::new();
        for emotion in Emotion::all() {
            if *emotion == Emotion::Neutral {
                continue;
            }
            let mapped = mapper.map_emotion(&EmotionConfig::with_emotion(*emotion));
            assert!(
                mapped.native_emotion.is_some(),
                "Emotion {emotion:?} must map to a Cartesia native token"
            );
        }
    }
}

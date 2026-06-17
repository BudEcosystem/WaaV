//! OpenAI TTS emotion mapper (real, replaces the former fallback stub).
//!
//! `gpt-4o-mini-tts` and `gpt-realtime` accept a natural-language **`instructions`**
//! string that steers affect, tone, pacing and delivery. This mapper SYNTHESIZES
//! that string from the canonical [`EmotionConfig`] (emotion + intensity + style +
//! description) and puts it on [`MappedEmotion::instruction_text`].
//!
//! Precedence (per the standardization plan): `description` (free-form acting
//! instruction) WINS verbatim > the synthesized `emotion+style+intensity` block >
//! `context`-only. The older `tts-1` / `tts-1-hd` models do NOT accept
//! `instructions`; those route to the fallback mapper (see `mappers::mod`), not here.
//!
//! Canonical template (OpenAI's structured headings):
//! ```text
//! Voice Affect: {affect adjective}, {intensity adverb}.
//! Tone: {style/context}.
//! Emotion: {emotion} ({intensity tier}).
//! Pacing: {from delivery style}.
//! Delivery: {whispered/shouted/monotone/expressive nuance}.
//! ```

use crate::core::emotion::mapper::{
    EmotionMapper, EmotionMethod, MappedEmotion, ProviderEmotionSupport,
};
use crate::core::emotion::types::{DeliveryStyle, Emotion, EmotionConfig};

/// OpenAI's instruction models cover the whole superset via natural language.
static OPENAI_SUPPORTED_EMOTIONS: &[Emotion] = Emotion::all();

/// Emotion mapper for OpenAI `gpt-4o-mini-tts` / `gpt-realtime` (instructions).
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiEmotionMapper;

impl OpenAiEmotionMapper {
    /// Creates a new OpenAI emotion mapper.
    #[inline]
    pub const fn new() -> Self {
        Self
    }

    /// The intensity adverb tier used in the synthesized instructions.
    fn intensity_adverb(intensity: f32) -> &'static str {
        if intensity < 0.35 {
            "subtly"
        } else if intensity <= 0.7 {
            "moderately"
        } else {
            "intensely"
        }
    }

    /// The named intensity tier shown in the `Emotion:` line.
    fn intensity_tier(intensity: f32) -> &'static str {
        if intensity < 0.35 {
            "low"
        } else if intensity <= 0.7 {
            "medium"
        } else {
            "high"
        }
    }

    /// Maps an [`Emotion`] to a short affect adjective phrase for `Voice Affect:`.
    fn emotion_affect(emotion: &Emotion) -> &'static str {
        match emotion {
            Emotion::Neutral => "even and natural",
            Emotion::Happy => "warm and upbeat",
            Emotion::Sad => "downcast and subdued",
            Emotion::Angry => "sharp and forceful",
            Emotion::Fearful => "tense and uneasy",
            Emotion::Surprised => "startled and bright",
            Emotion::Disgusted => "repulsed and recoiling",
            Emotion::Excited => "enthusiastic and energetic",
            Emotion::Calm => "relaxed and steady",
            Emotion::Anxious => "nervous and on-edge",
            Emotion::Confident => "assured and self-possessed",
            Emotion::Confused => "uncertain and questioning",
            Emotion::Empathetic => "warm and understanding",
            Emotion::Sarcastic => "dry and ironic",
            Emotion::Hopeful => "optimistic and encouraging",
            Emotion::Disappointed => "let-down and flat",
            Emotion::Curious => "inquisitive and engaged",
            Emotion::Grateful => "thankful and warm",
            Emotion::Proud => "accomplished and self-assured",
            Emotion::Embarrassed => "sheepish and hesitant",
            Emotion::Content => "satisfied and at-ease",
            Emotion::Bored => "flat and disengaged",
            Emotion::Amazed => "awestruck and wide-eyed",
            Emotion::Amused => "lightly amused and playful",
            Emotion::Affectionate => "tender and loving",
            Emotion::Intrigued => "drawn-in and attentive",
            Emotion::Flirtatious => "playful and teasing",
            Emotion::Frustrated => "exasperated and strained",
            Emotion::Annoyed => "irritated and curt",
            Emotion::Determined => "resolute and driven",
            Emotion::Reassuring => "soothing and comforting",
            Emotion::Sympathetic => "gentle and commiserating",
            Emotion::Nostalgic => "fond and reminiscing",
            Emotion::Serene => "deeply tranquil and still",
            Emotion::Terrified => "panicked and trembling",
            Emotion::Ecstatic => "elated and overjoyed",
            Emotion::Skeptical => "doubtful and guarded",
            Emotion::Relieved => "unburdened and easing",
            Emotion::Panicked => "frantic and overwhelmed",
            Emotion::Concerned => "worried and caring",
            Emotion::Apologetic => "contrite and regretful",
            Emotion::Resigned => "flat and accepting",
            Emotion::Wistful => "yearning and bittersweet",
            Emotion::Contemplative => "reflective and measured",
        }
    }

    /// Maps a [`DeliveryStyle`] to a `Pacing:` clause, or `None` if pacing-neutral.
    fn style_pacing(style: &DeliveryStyle) -> Option<&'static str> {
        match style {
            DeliveryStyle::Rushed => Some("quick and urgent"),
            DeliveryStyle::Measured | DeliveryStyle::DocumentaryNarration => {
                Some("slow and deliberate")
            }
            DeliveryStyle::SportsCommentaryExcited => Some("rapid and animated"),
            DeliveryStyle::NarrationRelaxed | DeliveryStyle::Gentle | DeliveryStyle::PoetryReading => {
                Some("unhurried with natural pauses")
            }
            _ => None,
        }
    }

    /// Maps a [`DeliveryStyle`] to a `Delivery:` clause for the production manner.
    fn style_delivery(style: &DeliveryStyle) -> Option<&'static str> {
        match style {
            DeliveryStyle::Whispered => Some("soft and hushed, almost whispered"),
            DeliveryStyle::Shouted | DeliveryStyle::Loud => Some("loud and forceful"),
            DeliveryStyle::Monotone => Some("flat and even, with little variation"),
            DeliveryStyle::Expressive => Some("animated and dynamic"),
            DeliveryStyle::Professional | DeliveryStyle::NarrationProfessional => {
                Some("polished and professional")
            }
            DeliveryStyle::Newscast | DeliveryStyle::NewscastFormal => {
                Some("clear, authoritative news-anchor delivery")
            }
            DeliveryStyle::CustomerService => Some("helpful and patient"),
            _ => None,
        }
    }

    /// Maps a [`DeliveryStyle`] to a `Tone:` descriptor used when no context is set.
    fn style_tone(style: &DeliveryStyle) -> Option<&'static str> {
        match style {
            DeliveryStyle::Casual | DeliveryStyle::Chat => Some("casual and conversational"),
            DeliveryStyle::Professional | DeliveryStyle::Formal => Some("formal and composed"),
            DeliveryStyle::Storytelling => Some("engaging and narrative"),
            DeliveryStyle::AdvertisementUpbeat => Some("upbeat and promotional"),
            DeliveryStyle::Lyrical => Some("lyrical and song-like"),
            _ => None,
        }
    }
}

impl EmotionMapper for OpenAiEmotionMapper {
    fn get_support(&self) -> ProviderEmotionSupport {
        ProviderEmotionSupport {
            provider_id: "openai",
            supports_emotions: true,
            supported_emotions: OPENAI_SUPPORTED_EMOTIONS,
            supports_intensity: true,
            supports_style: true,
            supports_free_description: true,
            method: EmotionMethod::InstructionText,
        }
    }

    fn map_emotion(&self, config: &EmotionConfig) -> MappedEmotion {
        // Precedence 1: a free-form description wins verbatim (acting instruction).
        if let Some(description) = config.description.as_ref().filter(|d| !d.is_empty()) {
            return MappedEmotion::with_instruction_text(description.clone());
        }

        let intensity = config.effective_intensity();
        let mut lines: Vec<String> = Vec::new();

        // Precedence 2: synthesize from emotion + style + intensity.
        if let Some(emotion) = &config.emotion
            && *emotion != Emotion::Neutral
        {
            lines.push(format!(
                "Voice Affect: {}, {} so.",
                Self::emotion_affect(emotion),
                Self::intensity_adverb(intensity)
            ));
            lines.push(format!(
                "Emotion: {} ({}).",
                emotion,
                Self::intensity_tier(intensity)
            ));
        }

        // Tone line: prefer an explicit context, else derive from the delivery style.
        let tone = config
            .context
            .clone()
            .filter(|c| !c.is_empty())
            .or_else(|| {
                config
                    .style
                    .as_ref()
                    .and_then(Self::style_tone)
                    .map(str::to_string)
            });
        if let Some(tone) = tone {
            lines.push(format!("Tone: {tone}."));
        }

        if let Some(style) = &config.style {
            if let Some(pacing) = Self::style_pacing(style) {
                lines.push(format!("Pacing: {pacing}."));
            }
            if let Some(delivery) = Self::style_delivery(style) {
                lines.push(format!("Delivery: {delivery}."));
            }
        }

        if lines.is_empty() {
            // Precedence 3: nothing actionable → no instructions (provider default).
            MappedEmotion::empty()
        } else {
            MappedEmotion::with_instruction_text(lines.join("\n"))
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
    fn test_openai_mapper_support() {
        let mapper = OpenAiEmotionMapper::new();
        let support = mapper.get_support();
        assert_eq!(support.provider_id, "openai");
        assert!(support.supports_emotions);
        assert!(support.supports_free_description);
        assert_eq!(support.method, EmotionMethod::InstructionText);
    }

    #[test]
    fn test_openai_synthesizes_structured_instructions() {
        // emotion=Excited, intensity=0.9, style=Casual → multi-line instructions.
        let mapper = OpenAiEmotionMapper::new();
        let mapped = mapper.map_emotion(
            &EmotionConfig::new()
                .emotion(Emotion::Excited)
                .intensity(EmotionIntensity::from_f32(0.9))
                .style(DeliveryStyle::Casual),
        );
        let instr = mapped.instruction_text.unwrap();
        assert!(instr.contains("Voice Affect:"));
        assert!(instr.contains("enthusiastic"));
        assert!(instr.contains("intensely"));
        assert!(instr.contains("Emotion: excited (high)"));
        assert!(instr.contains("Tone: casual"));
        // No SSML / voice-settings on the OpenAI path.
        assert!(mapped.ssml_style.is_none());
        assert!(mapped.stability.is_none());
    }

    #[test]
    fn test_openai_description_wins_verbatim() {
        let mapper = OpenAiEmotionMapper::new();
        let mapped = mapper.map_emotion(
            &EmotionConfig::new()
                .emotion(Emotion::Angry) // ignored
                .description("Speak like a calm late-night radio host."),
        );
        assert_eq!(
            mapped.instruction_text.as_deref(),
            Some("Speak like a calm late-night radio host.")
        );
    }

    #[test]
    fn test_openai_intensity_adverb_tiers() {
        let mapper = OpenAiEmotionMapper::new();
        let low = mapper
            .map_emotion(
                &EmotionConfig::new()
                    .emotion(Emotion::Sad)
                    .intensity(EmotionIntensity::from_f32(0.1)),
            )
            .instruction_text
            .unwrap();
        assert!(low.contains("subtly"));
        assert!(low.contains("(low)"));

        let high = mapper
            .map_emotion(
                &EmotionConfig::new()
                    .emotion(Emotion::Sad)
                    .intensity(EmotionIntensity::from_f32(0.95)),
            )
            .instruction_text
            .unwrap();
        assert!(high.contains("intensely"));
        assert!(high.contains("(high)"));
    }

    #[test]
    fn test_openai_delivery_clauses() {
        let mapper = OpenAiEmotionMapper::new();
        let whispered = mapper
            .map_emotion(&EmotionConfig::new().style(DeliveryStyle::Whispered))
            .instruction_text
            .unwrap();
        assert!(whispered.contains("Delivery:"));
        assert!(whispered.contains("hushed"));

        let rushed = mapper
            .map_emotion(&EmotionConfig::new().style(DeliveryStyle::Rushed))
            .instruction_text
            .unwrap();
        assert!(rushed.contains("Pacing:"));
        assert!(rushed.contains("quick"));
    }

    #[test]
    fn test_openai_neutral_empty() {
        let mapper = OpenAiEmotionMapper::new();
        let mapped = mapper.map_emotion(&EmotionConfig::with_emotion(Emotion::Neutral));
        assert!(mapped.instruction_text.is_none());
        assert!(!mapped.has_modifications());
    }
}

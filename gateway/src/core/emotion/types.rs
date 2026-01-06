//! Core emotion types for the unified emotion system.
//!
//! This module defines the standardized emotion types that can be used
//! across different TTS providers. The emotion system provides a common
//! abstraction layer that gets mapped to provider-specific formats:
//!
//! - **Hume AI**: Natural language descriptions (e.g., "happy, energetic")
//! - **ElevenLabs**: Voice settings (stability, style, similarity_boost)
//! - **Azure**: SSML express-as styles (e.g., "cheerful", "sad")
//! - **OpenAI**: Instructions field for TTS models
//! - **Others**: Graceful fallback with warnings

use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Primary Emotion Enum
// =============================================================================

/// Standardized emotions supported across TTS providers.
///
/// These emotions represent common emotional states that can be expressed
/// through speech synthesis. Different providers support different subsets
/// of these emotions, and the emotion mapper handles the translation.
///
/// # Provider Support Matrix
///
/// | Emotion | Hume | ElevenLabs | Azure | OpenAI |
/// |---------|------|------------|-------|--------|
/// | Neutral | Yes  | Yes (default) | Yes | Yes |
/// | Happy   | Yes  | Yes (low stability) | Yes (cheerful) | Yes |
/// | Sad     | Yes  | Yes (high stability) | Yes | Yes |
/// | Angry   | Yes  | Yes (low stability) | Yes | Yes |
/// | Fearful | Yes  | Partial | Yes (terrified) | Partial |
/// | Surprised | Yes | Partial | Yes | Partial |
/// | Excited | Yes | Yes | Yes (excited) | Yes |
/// | Calm    | Yes | Yes (high stability) | Yes (calm) | Yes |
/// | Sarcastic | Yes | Partial | Partial | Yes |
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::emotion::Emotion;
///
/// let emotion = Emotion::Happy;
/// assert_eq!(emotion.to_string(), "happy");
/// assert!(emotion.is_widely_supported());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Emotion {
    // =========================================================================
    // Primary Emotions (widely supported)
    // =========================================================================
    /// Neutral, default emotional state
    #[default]
    Neutral,
    /// Happy, joyful, positive
    Happy,
    /// Sad, melancholic, sorrowful
    Sad,
    /// Angry, frustrated, annoyed
    Angry,
    /// Fearful, scared, anxious
    Fearful,
    /// Surprised, shocked, astonished
    Surprised,
    /// Disgusted, repulsed
    Disgusted,

    // =========================================================================
    // Secondary Emotions (varying support)
    // =========================================================================
    /// Excited, enthusiastic, energetic
    Excited,
    /// Calm, peaceful, relaxed
    Calm,
    /// Anxious, nervous, worried
    Anxious,
    /// Confident, assured, assertive
    Confident,
    /// Confused, uncertain, puzzled
    Confused,
    /// Empathetic, understanding, compassionate
    Empathetic,
    /// Sarcastic, ironic, dry humor
    Sarcastic,
    /// Hopeful, optimistic
    Hopeful,
    /// Disappointed, let down
    Disappointed,
    /// Curious, inquisitive, interested
    Curious,
    /// Grateful, thankful, appreciative
    Grateful,
    /// Proud, accomplished
    Proud,
    /// Embarrassed, sheepish
    Embarrassed,
    /// Content, satisfied, at peace
    Content,
    /// Bored, uninterested, disengaged
    Bored,
}

impl Emotion {
    /// Returns all available emotions as a slice.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::emotion::Emotion;
    ///
    /// let emotions = Emotion::all();
    /// assert!(emotions.contains(&Emotion::Happy));
    /// ```
    #[inline]
    pub const fn all() -> &'static [Emotion] {
        &[
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
            Emotion::Empathetic,
            Emotion::Sarcastic,
            Emotion::Hopeful,
            Emotion::Disappointed,
            Emotion::Curious,
            Emotion::Grateful,
            Emotion::Proud,
            Emotion::Embarrassed,
            Emotion::Content,
            Emotion::Bored,
        ]
    }

    /// Returns primary emotions that are widely supported across providers.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::emotion::Emotion;
    ///
    /// let primary = Emotion::primary();
    /// assert!(primary.contains(&Emotion::Happy));
    /// assert!(!primary.contains(&Emotion::Sarcastic));
    /// ```
    #[inline]
    pub const fn primary() -> &'static [Emotion] {
        &[
            Emotion::Neutral,
            Emotion::Happy,
            Emotion::Sad,
            Emotion::Angry,
            Emotion::Fearful,
            Emotion::Surprised,
            Emotion::Disgusted,
        ]
    }

    /// Returns whether this emotion is a primary emotion (widely supported).
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::emotion::Emotion;
    ///
    /// assert!(Emotion::Happy.is_primary());
    /// assert!(!Emotion::Sarcastic.is_primary());
    /// ```
    #[inline]
    pub fn is_primary(&self) -> bool {
        matches!(
            self,
            Emotion::Neutral
                | Emotion::Happy
                | Emotion::Sad
                | Emotion::Angry
                | Emotion::Fearful
                | Emotion::Surprised
                | Emotion::Disgusted
        )
    }

    /// Returns whether this emotion is widely supported across most providers.
    ///
    /// Primary emotions plus some commonly supported secondary emotions.
    #[inline]
    pub fn is_widely_supported(&self) -> bool {
        matches!(
            self,
            Emotion::Neutral
                | Emotion::Happy
                | Emotion::Sad
                | Emotion::Angry
                | Emotion::Excited
                | Emotion::Calm
        )
    }

    /// Returns the emotion as a lowercase string suitable for APIs.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::emotion::Emotion;
    ///
    /// assert_eq!(Emotion::Happy.as_str(), "happy");
    /// assert_eq!(Emotion::Sarcastic.as_str(), "sarcastic");
    /// ```
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Emotion::Neutral => "neutral",
            Emotion::Happy => "happy",
            Emotion::Sad => "sad",
            Emotion::Angry => "angry",
            Emotion::Fearful => "fearful",
            Emotion::Surprised => "surprised",
            Emotion::Disgusted => "disgusted",
            Emotion::Excited => "excited",
            Emotion::Calm => "calm",
            Emotion::Anxious => "anxious",
            Emotion::Confident => "confident",
            Emotion::Confused => "confused",
            Emotion::Empathetic => "empathetic",
            Emotion::Sarcastic => "sarcastic",
            Emotion::Hopeful => "hopeful",
            Emotion::Disappointed => "disappointed",
            Emotion::Curious => "curious",
            Emotion::Grateful => "grateful",
            Emotion::Proud => "proud",
            Emotion::Embarrassed => "embarrassed",
            Emotion::Content => "content",
            Emotion::Bored => "bored",
        }
    }

    /// Parses an emotion from a string (case-insensitive).
    ///
    /// # Arguments
    ///
    /// * `s` - The string to parse
    ///
    /// # Returns
    ///
    /// The parsed emotion, or `None` if the string doesn't match any emotion.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::emotion::Emotion;
    ///
    /// assert_eq!(Emotion::from_str("happy"), Some(Emotion::Happy));
    /// assert_eq!(Emotion::from_str("ANGRY"), Some(Emotion::Angry));
    /// assert_eq!(Emotion::from_str("unknown"), None);
    /// ```
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "neutral" => Some(Emotion::Neutral),
            "happy" | "joyful" | "cheerful" => Some(Emotion::Happy),
            "sad" | "melancholic" | "sorrowful" => Some(Emotion::Sad),
            "angry" | "frustrated" | "mad" => Some(Emotion::Angry),
            "fearful" | "scared" | "frightened" | "afraid" => Some(Emotion::Fearful),
            "surprised" | "shocked" | "astonished" => Some(Emotion::Surprised),
            "disgusted" | "repulsed" => Some(Emotion::Disgusted),
            "excited" | "enthusiastic" | "energetic" => Some(Emotion::Excited),
            "calm" | "peaceful" | "relaxed" | "serene" => Some(Emotion::Calm),
            "anxious" | "nervous" | "worried" => Some(Emotion::Anxious),
            "confident" | "assured" | "assertive" => Some(Emotion::Confident),
            "confused" | "uncertain" | "puzzled" => Some(Emotion::Confused),
            "empathetic" | "compassionate" | "understanding" => Some(Emotion::Empathetic),
            "sarcastic" | "ironic" | "dry" => Some(Emotion::Sarcastic),
            "hopeful" | "optimistic" => Some(Emotion::Hopeful),
            "disappointed" | "let_down" => Some(Emotion::Disappointed),
            "curious" | "inquisitive" | "interested" => Some(Emotion::Curious),
            "grateful" | "thankful" | "appreciative" => Some(Emotion::Grateful),
            "proud" | "accomplished" => Some(Emotion::Proud),
            "embarrassed" | "sheepish" => Some(Emotion::Embarrassed),
            "content" | "satisfied" | "at_peace" => Some(Emotion::Content),
            "bored" | "uninterested" | "disengaged" => Some(Emotion::Bored),
            _ => None,
        }
    }
}

impl fmt::Display for Emotion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Delivery Style Enum
// =============================================================================

/// Delivery style modifiers for speech synthesis.
///
/// These styles can be combined with emotions to further customize
/// how the speech is delivered. They affect pacing, volume, and
/// prosodic patterns.
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::emotion::DeliveryStyle;
///
/// let style = DeliveryStyle::Whispered;
/// assert_eq!(style.to_string(), "whispered");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStyle {
    /// Normal, default delivery
    #[default]
    Normal,
    /// Whispered, quiet, intimate
    Whispered,
    /// Shouted, loud, emphatic
    Shouted,
    /// Rushed, fast, urgent
    Rushed,
    /// Measured, deliberate, careful
    Measured,
    /// Monotone, flat, robotic
    Monotone,
    /// Expressive, animated, dynamic
    Expressive,
    /// Professional, formal, business-like
    Professional,
    /// Casual, informal, conversational
    Casual,
    /// Storytelling, narrative, engaging
    Storytelling,
    /// Soft, gentle, tender
    Soft,
    /// Loud, strong volume
    Loud,
    /// Cheerful, upbeat, bright
    Cheerful,
    /// Serious, grave, solemn
    Serious,
    /// Formal, proper, polished
    Formal,
}

impl DeliveryStyle {
    /// Returns the style as a lowercase string.
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            DeliveryStyle::Normal => "normal",
            DeliveryStyle::Whispered => "whispered",
            DeliveryStyle::Shouted => "shouted",
            DeliveryStyle::Rushed => "rushed",
            DeliveryStyle::Measured => "measured",
            DeliveryStyle::Monotone => "monotone",
            DeliveryStyle::Expressive => "expressive",
            DeliveryStyle::Professional => "professional",
            DeliveryStyle::Casual => "casual",
            DeliveryStyle::Storytelling => "storytelling",
            DeliveryStyle::Soft => "soft",
            DeliveryStyle::Loud => "loud",
            DeliveryStyle::Cheerful => "cheerful",
            DeliveryStyle::Serious => "serious",
            DeliveryStyle::Formal => "formal",
        }
    }

    /// Parses a delivery style from a string (case-insensitive).
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "normal" | "default" => Some(DeliveryStyle::Normal),
            "whispered" | "whisper" | "quiet" => Some(DeliveryStyle::Whispered),
            "shouted" | "shout" => Some(DeliveryStyle::Shouted),
            "rushed" | "rush" | "fast" | "urgent" => Some(DeliveryStyle::Rushed),
            "measured" | "slow" | "deliberate" | "careful" => Some(DeliveryStyle::Measured),
            "monotone" | "flat" | "robotic" => Some(DeliveryStyle::Monotone),
            "expressive" | "animated" | "dynamic" => Some(DeliveryStyle::Expressive),
            "professional" | "business" => Some(DeliveryStyle::Professional),
            "casual" | "informal" | "conversational" => Some(DeliveryStyle::Casual),
            "storytelling" | "narrative" | "story" => Some(DeliveryStyle::Storytelling),
            "soft" | "gentle" | "tender" => Some(DeliveryStyle::Soft),
            "loud" | "strong" => Some(DeliveryStyle::Loud),
            "cheerful" | "upbeat" | "bright" => Some(DeliveryStyle::Cheerful),
            "serious" | "grave" | "solemn" => Some(DeliveryStyle::Serious),
            "formal" | "proper" | "polished" => Some(DeliveryStyle::Formal),
            _ => None,
        }
    }
}

impl fmt::Display for DeliveryStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Intensity Level
// =============================================================================

/// Named intensity levels for emotion expression.
///
/// These map to numeric values for providers that support intensity:
/// - Low: 0.3
/// - Medium: 0.6
/// - High: 1.0
///
/// Providers can also accept raw numeric intensity (0.0 to 1.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntensityLevel {
    /// Subtle expression (0.3)
    Low,
    /// Moderate expression (0.6)
    #[default]
    Medium,
    /// Maximum expression (1.0)
    High,
}

impl IntensityLevel {
    /// Converts the intensity level to a numeric value (0.0 to 1.0).
    #[inline]
    pub const fn as_f32(&self) -> f32 {
        match self {
            IntensityLevel::Low => 0.3,
            IntensityLevel::Medium => 0.6,
            IntensityLevel::High => 1.0,
        }
    }

    /// Creates an intensity level from a numeric value.
    ///
    /// # Arguments
    ///
    /// * `value` - A value between 0.0 and 1.0
    ///
    /// # Returns
    ///
    /// The closest matching intensity level.
    pub fn from_f32(value: f32) -> Self {
        if value < 0.45 {
            IntensityLevel::Low
        } else if value < 0.8 {
            IntensityLevel::Medium
        } else {
            IntensityLevel::High
        }
    }

    /// Parses an intensity level from a string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "low" | "subtle" | "light" => Some(IntensityLevel::Low),
            "medium" | "moderate" | "normal" => Some(IntensityLevel::Medium),
            "high" | "strong" | "maximum" | "max" => Some(IntensityLevel::High),
            _ => None,
        }
    }
}

// =============================================================================
// Emotion Intensity (Unified)
// =============================================================================

/// Unified emotion intensity that supports both numeric and named levels.
///
/// This allows users to specify intensity either as a numeric value (0.0-1.0)
/// or as a named level (low, medium, high) for convenience.
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::emotion::{EmotionIntensity, IntensityLevel};
///
/// // Numeric intensity
/// let intensity = EmotionIntensity::Numeric(0.8);
/// assert_eq!(intensity.as_f32(), 0.8);
///
/// // Named intensity
/// let intensity = EmotionIntensity::Named(IntensityLevel::High);
/// assert_eq!(intensity.as_f32(), 1.0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EmotionIntensity {
    /// Numeric intensity (0.0 to 1.0)
    Numeric(f32),
    /// Named intensity level
    Named(IntensityLevel),
}

impl Default for EmotionIntensity {
    fn default() -> Self {
        EmotionIntensity::Named(IntensityLevel::Medium)
    }
}

impl EmotionIntensity {
    /// Converts the intensity to a numeric value (0.0 to 1.0).
    ///
    /// Numeric values are clamped to the valid range.
    #[inline]
    pub fn as_f32(&self) -> f32 {
        match self {
            EmotionIntensity::Numeric(v) => v.clamp(0.0, 1.0),
            EmotionIntensity::Named(level) => level.as_f32(),
        }
    }

    /// Creates an intensity from a numeric value.
    #[inline]
    pub fn from_f32(value: f32) -> Self {
        EmotionIntensity::Numeric(value.clamp(0.0, 1.0))
    }

    /// Creates an intensity from a named level.
    #[inline]
    pub const fn from_level(level: IntensityLevel) -> Self {
        EmotionIntensity::Named(level)
    }

    /// Returns whether this is a high intensity (>= 0.8).
    #[inline]
    pub fn is_high(&self) -> bool {
        self.as_f32() >= 0.8
    }

    /// Returns whether this is a low intensity (<= 0.35).
    #[inline]
    pub fn is_low(&self) -> bool {
        self.as_f32() <= 0.35
    }
}

// =============================================================================
// Emotion Configuration
// =============================================================================

/// User-facing emotion configuration for TTS requests.
///
/// This configuration specifies the desired emotional expression
/// and delivery style for speech synthesis. It gets mapped to
/// provider-specific formats by the emotion mapper.
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::emotion::{EmotionConfig, Emotion, DeliveryStyle, EmotionIntensity};
///
/// let config = EmotionConfig {
///     emotion: Some(Emotion::Happy),
///     intensity: Some(EmotionIntensity::from_f32(0.8)),
///     style: Some(DeliveryStyle::Expressive),
///     description: None,
///     context: Some("Customer support greeting".to_string()),
/// };
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EmotionConfig {
    /// Primary emotion to express.
    ///
    /// If `None`, the provider's default emotional expression is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "happy"))]
    pub emotion: Option<Emotion>,

    /// Intensity of the emotion (0.0 to 1.0, or named level).
    ///
    /// Higher values mean more expressive delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = 0.8))]
    pub intensity: Option<EmotionIntensity>,

    /// Delivery style modifier.
    ///
    /// Combines with emotion to affect pacing and prosody.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "expressive"))]
    pub style: Option<DeliveryStyle>,

    /// Free-form description for providers that support it (e.g., Hume).
    ///
    /// This takes precedence over `emotion` for Hume AI, allowing
    /// natural language instructions like "warm, friendly, inviting".
    /// Maximum 100 characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "warm, friendly, inviting"))]
    pub description: Option<String>,

    /// Speaking context hint for better inference.
    ///
    /// Examples: "customer support", "news broadcast", "bedtime story"
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "customer support greeting"))]
    pub context: Option<String>,
}

impl EmotionConfig {
    /// Creates an empty emotion configuration (neutral).
    #[inline]
    pub const fn new() -> Self {
        Self {
            emotion: None,
            intensity: None,
            style: None,
            description: None,
            context: None,
        }
    }

    /// Creates an emotion configuration with just an emotion.
    #[inline]
    pub fn with_emotion(emotion: Emotion) -> Self {
        Self {
            emotion: Some(emotion),
            ..Default::default()
        }
    }

    /// Creates an emotion configuration with emotion and intensity.
    #[inline]
    pub fn with_emotion_and_intensity(emotion: Emotion, intensity: f32) -> Self {
        Self {
            emotion: Some(emotion),
            intensity: Some(EmotionIntensity::from_f32(intensity)),
            ..Default::default()
        }
    }

    /// Creates an emotion configuration from a free-form description.
    ///
    /// This is ideal for Hume AI which supports natural language instructions.
    #[inline]
    pub fn with_description(description: impl Into<String>) -> Self {
        Self {
            description: Some(description.into()),
            ..Default::default()
        }
    }

    /// Sets the emotion.
    #[inline]
    pub fn emotion(mut self, emotion: Emotion) -> Self {
        self.emotion = Some(emotion);
        self
    }

    /// Sets the intensity.
    #[inline]
    pub fn intensity(mut self, intensity: impl Into<EmotionIntensity>) -> Self {
        self.intensity = Some(match intensity.into() {
            EmotionIntensity::Numeric(v) => EmotionIntensity::Numeric(v),
            EmotionIntensity::Named(l) => EmotionIntensity::Named(l),
        });
        self
    }

    /// Sets the delivery style.
    #[inline]
    pub fn style(mut self, style: DeliveryStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Sets the free-form description.
    #[inline]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sets the context hint.
    #[inline]
    pub fn context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Returns whether this configuration specifies any emotion settings.
    #[inline]
    pub fn has_emotion(&self) -> bool {
        self.emotion.is_some() || self.description.is_some()
    }

    /// Returns whether this is a default/neutral configuration.
    #[inline]
    pub fn is_neutral(&self) -> bool {
        self.emotion.is_none()
            && self.intensity.is_none()
            && self.style.is_none()
            && self.description.is_none()
    }

    /// Gets the effective intensity value (defaults to 0.6 if not set).
    #[inline]
    pub fn effective_intensity(&self) -> f32 {
        self.intensity
            .as_ref()
            .map(|i| i.as_f32())
            .unwrap_or(0.6)
    }
}

impl From<f32> for EmotionIntensity {
    fn from(value: f32) -> Self {
        EmotionIntensity::Numeric(value)
    }
}

impl From<IntensityLevel> for EmotionIntensity {
    fn from(level: IntensityLevel) -> Self {
        EmotionIntensity::Named(level)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Emotion Tests
    // =========================================================================

    #[test]
    fn test_emotion_as_str() {
        assert_eq!(Emotion::Neutral.as_str(), "neutral");
        assert_eq!(Emotion::Happy.as_str(), "happy");
        assert_eq!(Emotion::Sad.as_str(), "sad");
        assert_eq!(Emotion::Angry.as_str(), "angry");
        assert_eq!(Emotion::Sarcastic.as_str(), "sarcastic");
    }

    #[test]
    fn test_emotion_from_str() {
        assert_eq!(Emotion::from_str("happy"), Some(Emotion::Happy));
        assert_eq!(Emotion::from_str("ANGRY"), Some(Emotion::Angry));
        assert_eq!(Emotion::from_str("Cheerful"), Some(Emotion::Happy));
        assert_eq!(Emotion::from_str("frightened"), Some(Emotion::Fearful));
        assert_eq!(Emotion::from_str("unknown"), None);
        assert_eq!(Emotion::from_str(""), None);
    }

    #[test]
    fn test_emotion_display() {
        assert_eq!(format!("{}", Emotion::Happy), "happy");
        assert_eq!(format!("{}", Emotion::Sarcastic), "sarcastic");
    }

    #[test]
    fn test_emotion_is_primary() {
        assert!(Emotion::Happy.is_primary());
        assert!(Emotion::Sad.is_primary());
        assert!(Emotion::Neutral.is_primary());
        assert!(!Emotion::Sarcastic.is_primary());
        assert!(!Emotion::Excited.is_primary());
    }

    #[test]
    fn test_emotion_is_widely_supported() {
        assert!(Emotion::Happy.is_widely_supported());
        assert!(Emotion::Calm.is_widely_supported());
        assert!(!Emotion::Sarcastic.is_widely_supported());
    }

    #[test]
    fn test_emotion_all() {
        let all = Emotion::all();
        assert!(all.len() >= 15);
        assert!(all.contains(&Emotion::Neutral));
        assert!(all.contains(&Emotion::Happy));
    }

    #[test]
    fn test_emotion_primary() {
        let primary = Emotion::primary();
        assert_eq!(primary.len(), 7);
        for emotion in primary {
            assert!(emotion.is_primary());
        }
    }

    #[test]
    fn test_emotion_default() {
        assert_eq!(Emotion::default(), Emotion::Neutral);
    }

    #[test]
    fn test_emotion_serialization() {
        let emotion = Emotion::Happy;
        let json = serde_json::to_string(&emotion).unwrap();
        assert_eq!(json, "\"happy\"");

        let parsed: Emotion = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Emotion::Happy);
    }

    // =========================================================================
    // DeliveryStyle Tests
    // =========================================================================

    #[test]
    fn test_delivery_style_as_str() {
        assert_eq!(DeliveryStyle::Normal.as_str(), "normal");
        assert_eq!(DeliveryStyle::Whispered.as_str(), "whispered");
        assert_eq!(DeliveryStyle::Shouted.as_str(), "shouted");
    }

    #[test]
    fn test_delivery_style_from_str() {
        assert_eq!(
            DeliveryStyle::from_str("whispered"),
            Some(DeliveryStyle::Whispered)
        );
        assert_eq!(
            DeliveryStyle::from_str("SHOUTED"),
            Some(DeliveryStyle::Shouted)
        );
        assert_eq!(
            DeliveryStyle::from_str("fast"),
            Some(DeliveryStyle::Rushed)
        );
        assert_eq!(DeliveryStyle::from_str("unknown"), None);
    }

    #[test]
    fn test_delivery_style_default() {
        assert_eq!(DeliveryStyle::default(), DeliveryStyle::Normal);
    }

    #[test]
    fn test_delivery_style_serialization() {
        let style = DeliveryStyle::Whispered;
        let json = serde_json::to_string(&style).unwrap();
        assert_eq!(json, "\"whispered\"");

        let parsed: DeliveryStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, DeliveryStyle::Whispered);
    }

    // =========================================================================
    // IntensityLevel Tests
    // =========================================================================

    #[test]
    fn test_intensity_level_as_f32() {
        assert_eq!(IntensityLevel::Low.as_f32(), 0.3);
        assert_eq!(IntensityLevel::Medium.as_f32(), 0.6);
        assert_eq!(IntensityLevel::High.as_f32(), 1.0);
    }

    #[test]
    fn test_intensity_level_from_f32() {
        assert_eq!(IntensityLevel::from_f32(0.1), IntensityLevel::Low);
        assert_eq!(IntensityLevel::from_f32(0.5), IntensityLevel::Medium);
        assert_eq!(IntensityLevel::from_f32(0.9), IntensityLevel::High);
    }

    #[test]
    fn test_intensity_level_from_str() {
        assert_eq!(IntensityLevel::from_str("low"), Some(IntensityLevel::Low));
        assert_eq!(
            IntensityLevel::from_str("HIGH"),
            Some(IntensityLevel::High)
        );
        assert_eq!(
            IntensityLevel::from_str("moderate"),
            Some(IntensityLevel::Medium)
        );
        assert_eq!(IntensityLevel::from_str("unknown"), None);
    }

    // =========================================================================
    // EmotionIntensity Tests
    // =========================================================================

    #[test]
    fn test_emotion_intensity_numeric() {
        let intensity = EmotionIntensity::Numeric(0.8);
        assert_eq!(intensity.as_f32(), 0.8);
        assert!(intensity.is_high());
        assert!(!intensity.is_low());
    }

    #[test]
    fn test_emotion_intensity_named() {
        let intensity = EmotionIntensity::Named(IntensityLevel::High);
        assert_eq!(intensity.as_f32(), 1.0);
        assert!(intensity.is_high());
    }

    #[test]
    fn test_emotion_intensity_clamping() {
        let too_high = EmotionIntensity::Numeric(1.5);
        assert_eq!(too_high.as_f32(), 1.0);

        let too_low = EmotionIntensity::Numeric(-0.5);
        assert_eq!(too_low.as_f32(), 0.0);
    }

    #[test]
    fn test_emotion_intensity_default() {
        let default = EmotionIntensity::default();
        assert_eq!(default.as_f32(), 0.6);
    }

    #[test]
    fn test_emotion_intensity_from() {
        let from_f32: EmotionIntensity = 0.7f32.into();
        assert_eq!(from_f32.as_f32(), 0.7);

        let from_level: EmotionIntensity = IntensityLevel::Low.into();
        assert_eq!(from_level.as_f32(), 0.3);
    }

    #[test]
    fn test_emotion_intensity_serialization() {
        // Numeric serialization
        let numeric = EmotionIntensity::Numeric(0.8);
        let json = serde_json::to_string(&numeric).unwrap();
        assert_eq!(json, "0.8");

        // Named serialization
        let named = EmotionIntensity::Named(IntensityLevel::High);
        let json = serde_json::to_string(&named).unwrap();
        assert_eq!(json, "\"high\"");

        // Deserialization from number
        let parsed: EmotionIntensity = serde_json::from_str("0.5").unwrap();
        assert_eq!(parsed.as_f32(), 0.5);

        // Deserialization from string
        let parsed: EmotionIntensity = serde_json::from_str("\"low\"").unwrap();
        assert_eq!(parsed.as_f32(), 0.3);
    }

    // =========================================================================
    // EmotionConfig Tests
    // =========================================================================

    #[test]
    fn test_emotion_config_new() {
        let config = EmotionConfig::new();
        assert!(config.emotion.is_none());
        assert!(config.intensity.is_none());
        assert!(config.style.is_none());
        assert!(config.description.is_none());
        assert!(config.is_neutral());
    }

    #[test]
    fn test_emotion_config_with_emotion() {
        let config = EmotionConfig::with_emotion(Emotion::Happy);
        assert_eq!(config.emotion, Some(Emotion::Happy));
        assert!(config.has_emotion());
        assert!(!config.is_neutral());
    }

    #[test]
    fn test_emotion_config_with_emotion_and_intensity() {
        let config = EmotionConfig::with_emotion_and_intensity(Emotion::Excited, 0.9);
        assert_eq!(config.emotion, Some(Emotion::Excited));
        assert_eq!(config.effective_intensity(), 0.9);
    }

    #[test]
    fn test_emotion_config_with_description() {
        let config = EmotionConfig::with_description("warm, friendly, inviting");
        assert_eq!(
            config.description,
            Some("warm, friendly, inviting".to_string())
        );
        assert!(config.has_emotion());
    }

    #[test]
    fn test_emotion_config_builder_pattern() {
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .intensity(0.8f32)
            .style(DeliveryStyle::Expressive)
            .context("customer support");

        assert_eq!(config.emotion, Some(Emotion::Happy));
        assert_eq!(config.effective_intensity(), 0.8);
        assert_eq!(config.style, Some(DeliveryStyle::Expressive));
        assert_eq!(config.context, Some("customer support".to_string()));
    }

    #[test]
    fn test_emotion_config_effective_intensity() {
        let config = EmotionConfig::new();
        assert_eq!(config.effective_intensity(), 0.6); // Default

        let config = EmotionConfig::new().intensity(0.9f32);
        assert_eq!(config.effective_intensity(), 0.9);
    }

    #[test]
    fn test_emotion_config_serialization() {
        let config = EmotionConfig::new()
            .emotion(Emotion::Happy)
            .intensity(0.8f32);

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"emotion\":\"happy\""));
        assert!(json.contains("\"intensity\":0.8"));

        let parsed: EmotionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.emotion, Some(Emotion::Happy));
        assert_eq!(parsed.effective_intensity(), 0.8);
    }

    #[test]
    fn test_emotion_config_default() {
        let config = EmotionConfig::default();
        assert!(config.is_neutral());
        assert!(!config.has_emotion());
    }
}

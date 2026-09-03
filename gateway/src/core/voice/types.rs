//! Canonical voice-descriptor types (P4 of the SDK standardization plan).
//!
//! A developer can ask for a voice by its CHARACTERISTICS — gender, accent/locale,
//! style/timbre, age, optional name hint — instead of memorizing each provider's
//! opaque `voice_id`. The [`crate::core::voice::resolver`] scores the `/voices`
//! catalog for the target provider and returns a concrete `voice_id`, so
//! `{female, en-US, warm}` resolves to `aura-asteria-en` / `21m00Tcm4TlvDq8ikWAM`
//! / `en-US-JennyNeural` with NO client edit. The raw `voice_id` always wins as an
//! escape hatch (resolution only fires when it is absent).
//!
//! Mirrors the [`crate::core::emotion`] / [`crate::core::lang`] chassis: canonical
//! enums + a serializable descriptor + a per-provider resolver + a 1:1 Python mirror.

use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Gender
// =============================================================================

/// Canonical voice gender. Providers spell this `Female`/`Male`/`Neutral` (Google
/// `ssml_gender`), `"female"`/`"male"` (ElevenLabs labels), etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum Gender {
    /// Male-presenting voice.
    Male,
    /// Female-presenting voice.
    Female,
    /// Gender-neutral / unspecified voice.
    Neutral,
}

impl Gender {
    /// Canonical lowercase token.
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Gender::Male => "male",
            Gender::Female => "female",
            Gender::Neutral => "neutral",
        }
    }

    /// Parse a gender from a provider/client string (case-insensitive). Accepts the
    /// many provider spellings ("Female", "feminine", "woman", "F", …).
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "male" | "masculine" | "man" | "m" => Some(Gender::Male),
            "female" | "feminine" | "woman" | "f" => Some(Gender::Female),
            "neutral" | "nonbinary" | "non-binary" | "n" => Some(Gender::Neutral),
            _ => None,
        }
    }
}

impl fmt::Display for Gender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Age
// =============================================================================

/// Canonical voice age band. ElevenLabs uses exactly `young`/`middle_aged`/`old`;
/// Azure roles are `YoungAdult*`/`OlderAdult*`/`Senior*` (Senior folds into `Old`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Age {
    /// Young / young-adult voice.
    Young,
    /// Middle-aged adult voice.
    MiddleAged,
    /// Older-adult / senior voice.
    Old,
}

impl Age {
    /// Canonical token (`young` / `middle_aged` / `old`).
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Age::Young => "young",
            Age::MiddleAged => "middle_aged",
            Age::Old => "old",
        }
    }

    /// Parse an age band from a provider/client string (case-insensitive), folding
    /// Azure's `young_adult`/`older_adult`/`senior` spellings in.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
            "young" | "young_adult" | "youngadult" | "youthful" | "child" | "teen" => {
                Some(Age::Young)
            }
            "middle_aged" | "middleaged" | "middle" | "adult" => Some(Age::MiddleAged),
            "old" | "older" | "older_adult" | "olderadult" | "senior" | "elderly" => Some(Age::Old),
            _ => None,
        }
    }
}

impl fmt::Display for Age {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// VoiceDescriptor
// =============================================================================

/// A canonical, provider-agnostic description of a desired voice. Resolved
/// SERVER-SIDE to a concrete provider `voice_id` over the `/voices` catalog.
///
/// All fields are optional; the resolver scores candidates on whichever are set.
/// The raw `TTSConfig.voice_id` (when present) ALWAYS wins — this descriptor only
/// drives resolution when no explicit `voice_id` was supplied.
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::voice::{VoiceDescriptor, Gender};
///
/// let d = VoiceDescriptor {
///     gender: Some(Gender::Female),
///     locale: Some("en-US".to_string()),
///     style: Some("warm".to_string()),
///     ..Default::default()
/// };
/// assert!(d.is_set());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VoiceDescriptor {
    /// Desired gender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "female"))]
    pub gender: Option<Gender>,

    /// BCP-47 locale (preferred): `en-US`, `en-GB`, `hi-IN`, … Used for exact-locale
    /// and language-family scoring; the region also yields an accent string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "en-US"))]
    pub locale: Option<String>,

    /// Free accent string when no `locale` is given (`american`, `british`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "american"))]
    pub accent: Option<String>,

    /// Desired age band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "young"))]
    pub age: Option<Age>,

    /// Free descriptive timbre/style matched against provider metadata
    /// (`warm`, `bright`, `deep`, `calm`, `confident`, `energetic`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "warm"))]
    pub style: Option<String>,

    /// Optional preferred voice name (e.g. `asteria`, `jenny`). A substring match
    /// against the voice name/id gives it the strongest score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "asteria"))]
    pub name_hint: Option<String>,
}

impl VoiceDescriptor {
    /// Returns whether ANY field is set (i.e. resolution should be attempted).
    #[inline]
    pub fn is_set(&self) -> bool {
        self.gender.is_some()
            || self.locale.as_ref().is_some_and(|s| !s.trim().is_empty())
            || self.accent.as_ref().is_some_and(|s| !s.trim().is_empty())
            || self.age.is_some()
            || self.style.as_ref().is_some_and(|s| !s.trim().is_empty())
            || self
                .name_hint
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
    }

    /// A compact, human-readable rendering for `config_warning` messages.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(g) = self.gender {
            parts.push(format!("gender={g}"));
        }
        if let Some(l) = self.locale.as_ref().filter(|s| !s.is_empty()) {
            parts.push(format!("locale={l}"));
        }
        if let Some(a) = self.accent.as_ref().filter(|s| !s.is_empty()) {
            parts.push(format!("accent={a}"));
        }
        if let Some(a) = self.age {
            parts.push(format!("age={a}"));
        }
        if let Some(s) = self.style.as_ref().filter(|s| !s.is_empty()) {
            parts.push(format!("style={s}"));
        }
        if let Some(n) = self.name_hint.as_ref().filter(|s| !s.is_empty()) {
            parts.push(format!("name~{n}"));
        }
        if parts.is_empty() {
            "<empty>".to_string()
        } else {
            parts.join(", ")
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gender_from_str_variants() {
        assert_eq!(Gender::from_str("Female"), Some(Gender::Female));
        assert_eq!(Gender::from_str("feminine"), Some(Gender::Female));
        assert_eq!(Gender::from_str("MALE"), Some(Gender::Male));
        assert_eq!(Gender::from_str("nonbinary"), Some(Gender::Neutral));
        assert_eq!(Gender::from_str("alien"), None);
    }

    #[test]
    fn age_from_str_folds_azure_spellings() {
        assert_eq!(Age::from_str("young_adult"), Some(Age::Young));
        assert_eq!(Age::from_str("YoungAdult"), Some(Age::Young));
        assert_eq!(Age::from_str("senior"), Some(Age::Old));
        assert_eq!(Age::from_str("middle-aged"), Some(Age::MiddleAged));
        assert_eq!(Age::from_str("ancient"), None);
    }

    #[test]
    fn descriptor_is_set_and_describe() {
        let empty = VoiceDescriptor::default();
        assert!(!empty.is_set());
        assert_eq!(empty.describe(), "<empty>");

        let d = VoiceDescriptor {
            gender: Some(Gender::Female),
            locale: Some("en-US".into()),
            style: Some("warm".into()),
            ..Default::default()
        };
        assert!(d.is_set());
        let desc = d.describe();
        assert!(desc.contains("gender=female"));
        assert!(desc.contains("locale=en-US"));
        assert!(desc.contains("style=warm"));
    }

    #[test]
    fn descriptor_serde_roundtrip() {
        let d = VoiceDescriptor {
            gender: Some(Gender::Female),
            locale: Some("en-US".into()),
            age: Some(Age::Young),
            style: Some("warm".into()),
            name_hint: Some("asteria".into()),
            accent: None,
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"gender\":\"female\""));
        assert!(json.contains("\"age\":\"young\""));
        // accent omitted when None.
        assert!(!json.contains("accent"));
        let back: VoiceDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn empty_strings_do_not_count_as_set() {
        let d = VoiceDescriptor {
            locale: Some("   ".into()),
            style: Some("".into()),
            ..Default::default()
        };
        assert!(!d.is_set());
    }
}

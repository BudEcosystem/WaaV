//! The session language vote (addendum B7, Release 2).
//!
//! A session that names no language sends none, so a file model guesses it on every segment, and
//! short clips are where it guesses worst. Each segment that comes back with text votes with the
//! language the vendor detected; two agreeing votes pin that language, which is then sent on every
//! later upload of the call (the request's existing language override: the engine is unchanged).
//! Voting stops after the first few segments without agreement, so a genuinely mixed-language call
//! keeps detection. A session that named its language never votes.

/// Segments that may vote; past these without two agreeing, detection stays per segment.
pub const VOTE_WINDOW: usize = 5;
/// Agreeing votes that pin a language.
pub const VOTE_QUORUM: usize = 2;

#[derive(Debug, Clone, Default)]
pub struct LanguageVote {
    votes: Vec<String>,
    pinned: Option<String>,
    closed: bool,
}

impl LanguageVote {
    pub fn new() -> Self {
        Self::default()
    }

    /// The pinned language, once agreed.
    pub fn pinned(&self) -> Option<&str> {
        self.pinned.as_deref()
    }

    /// Record one segment's detected language. `Some(language)` exactly once: when this vote pins it.
    pub fn record(&mut self, detected: Option<&str>) -> Option<String> {
        if self.closed || self.pinned.is_some() {
            return None;
        }
        let lang = detected.and_then(normalize)?;
        self.votes.push(lang.clone());
        if self.votes.iter().filter(|v| **v == lang).count() >= VOTE_QUORUM {
            self.pinned = Some(lang.clone());
            return Some(lang);
        }
        if self.votes.len() >= VOTE_WINDOW {
            self.closed = true;
        }
        None
    }
}

/// A vendor's detected language as an ISO 639-1 code: `en`, `en-US` and Whisper's `english` all
/// read as `en`. `None` for what cannot be read (an unknown name, `auto`, empty).
pub fn normalize(raw: &str) -> Option<String> {
    let l = raw.trim().to_ascii_lowercase();
    if l.is_empty() || l == "auto" || l == "multi" || l == "unknown" {
        return None;
    }
    let primary = l.split(['-', '_']).next().unwrap_or(&l);
    if (2..=3).contains(&primary.len()) && primary.chars().all(|c| c.is_ascii_lowercase()) {
        return Some(primary.to_string());
    }
    NAMES
        .iter()
        .find(|(name, _)| *name == primary)
        .map(|(_, code)| (*code).to_string())
}

/// Whisper's `verbose_json` names the language in English.
const NAMES: &[(&str, &str)] = &[
    ("english", "en"),
    ("spanish", "es"),
    ("french", "fr"),
    ("german", "de"),
    ("italian", "it"),
    ("portuguese", "pt"),
    ("dutch", "nl"),
    ("russian", "ru"),
    ("polish", "pl"),
    ("turkish", "tr"),
    ("arabic", "ar"),
    ("hindi", "hi"),
    ("bengali", "bn"),
    ("tamil", "ta"),
    ("telugu", "te"),
    ("marathi", "mr"),
    ("gujarati", "gu"),
    ("kannada", "kn"),
    ("malayalam", "ml"),
    ("urdu", "ur"),
    ("japanese", "ja"),
    ("korean", "ko"),
    ("chinese", "zh"),
    ("vietnamese", "vi"),
    ("thai", "th"),
    ("indonesian", "id"),
    ("malay", "ms"),
    ("swedish", "sv"),
    ("norwegian", "no"),
    ("danish", "da"),
    ("finnish", "fi"),
    ("greek", "el"),
    ("hebrew", "he"),
    ("ukrainian", "uk"),
    ("czech", "cs"),
    ("romanian", "ro"),
    ("hungarian", "hu"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_agreeing_segments_pin_the_language_once() {
        let mut v = LanguageVote::new();
        assert_eq!(v.record(Some("english")), None);
        assert_eq!(v.record(Some("en-US")), Some("en".to_string()));
        assert_eq!(v.pinned(), Some("en"));
        assert_eq!(v.record(Some("en")), None, "reported once");
        assert_eq!(v.record(Some("fr")), None, "a pinned language stays");
        assert_eq!(v.pinned(), Some("en"));
    }

    #[test]
    fn segments_without_a_language_do_not_vote() {
        let mut v = LanguageVote::new();
        for _ in 0..10 {
            assert_eq!(v.record(None), None);
            assert_eq!(v.record(Some("auto")), None);
        }
        assert_eq!(v.record(Some("de")), None);
        assert_eq!(v.record(Some("de")), Some("de".into()));
    }

    #[test]
    fn a_mixed_call_keeps_detection_after_the_window() {
        let mut v = LanguageVote::new();
        for l in ["en", "hi", "es", "fr", "de"] {
            assert_eq!(v.record(Some(l)), None);
        }
        assert_eq!(
            v.record(Some("en")),
            None,
            "voting closed after five disagreeing segments"
        );
        assert_eq!(v.pinned(), None);
    }

    #[test]
    fn a_late_agreement_inside_the_window_pins() {
        let mut v = LanguageVote::new();
        assert_eq!(v.record(Some("en")), None);
        assert_eq!(v.record(Some("hi")), None);
        assert_eq!(v.record(Some("Hindi")), Some("hi".into()));
    }

    #[test]
    fn names_and_tags_read_as_iso_639_1() {
        assert_eq!(normalize("English").as_deref(), Some("en"));
        assert_eq!(normalize("pt_BR").as_deref(), Some("pt"));
        assert_eq!(normalize("yue").as_deref(), Some("yue"));
        assert_eq!(normalize("klingon"), None);
        assert_eq!(normalize(""), None);
    }
}

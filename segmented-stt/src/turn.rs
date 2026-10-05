//! Joining a turn's text, and the two result shapes the engine can emit.
//!
//! Text parts are joined in sequence order with one space, or none for languages written without
//! spaces. Case and punctuation are not rewritten, except across a hard split (a cut with no pause
//! at all), where one trailing full stop is removed from the earlier part and up to four words that
//! end the first part and begin the second are removed once.

use crate::types::{CutReason, EngineResult};

/// One released unit's contribution to the turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text {
        seq: u32,
        text: String,
        cut: CutReason,
        vendor_confidence: Option<f32>,
        derived_confidence: Option<f32>,
        language: Option<String>,
        request_id: Option<String>,
    },
    /// A lost unit: speech the vendor never transcribed.
    Gap { seq: u32, voiced_ms: u32 },
}

/// The turn's parts, in release order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnText {
    pub parts: Vec<Part>,
    /// Language of the session: no spaces between parts for Chinese, Japanese, Thai, …
    pub no_space: bool,
}

/// Languages written without spaces between words.
pub fn writes_without_spaces(language: Option<&str>) -> bool {
    let Some(lang) = language else { return false };
    let primary = lang
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        primary.as_str(),
        "zh" | "ja" | "th" | "lo" | "km" | "my" | "yue" | "cmn" | "wuu"
    )
}

fn norm_word(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Join `next` onto `acc`. At a hard split, drop the earlier part's trailing full stop and the
/// longest run of up to four words that ends `acc` and begins `next`.
fn join_into(acc: &mut String, next: &str, hard_seam: bool, no_space: bool) {
    let next = next.trim();
    if next.is_empty() {
        return;
    }
    if acc.is_empty() {
        acc.push_str(next);
        return;
    }
    let mut next_words: Vec<&str> = next.split_whitespace().collect();
    if hard_seam {
        if acc.ends_with('.') {
            acc.pop();
        }
        let acc_words: Vec<String> = acc.split_whitespace().map(norm_word).collect();
        let max = 4.min(acc_words.len()).min(next_words.len());
        for k in (1..=max).rev() {
            let tail = &acc_words[acc_words.len() - k..];
            let head: Vec<String> = next_words[..k].iter().map(|w| norm_word(w)).collect();
            if tail == head.as_slice() && tail.iter().all(|w| !w.is_empty()) {
                next_words.drain(..k);
                break;
            }
        }
        if next_words.is_empty() {
            return;
        }
    }
    if !no_space {
        acc.push(' ');
    }
    if no_space {
        acc.push_str(&next_words.concat());
    } else {
        acc.push_str(&next_words.join(" "));
    }
}

impl TurnText {
    pub fn new(language: Option<&str>) -> Self {
        Self {
            parts: Vec::new(),
            no_space: writes_without_spaces(language),
        }
    }

    pub fn push(&mut self, part: Part) {
        self.parts.push(part);
    }

    /// The joined text so far.
    pub fn joined(&self) -> String {
        let mut acc = String::new();
        let mut prev_cut: Option<CutReason> = None;
        let mut after_gap = false;
        for p in &self.parts {
            match p {
                Part::Text { text, cut, .. } => {
                    let hard = prev_cut == Some(CutReason::HardSplit) && !after_gap;
                    join_into(&mut acc, text, hard, self.no_space);
                    prev_cut = Some(*cut);
                    after_gap = false;
                }
                Part::Gap { .. } => after_gap = true,
            }
        }
        acc
    }

    /// Where the next part's text will start in the joined text, in chars.
    pub fn offset(&self) -> u32 {
        let j = self.joined();
        let n = j.chars().count();
        if n == 0 {
            0
        } else {
            n as u32 + u32::from(!self.no_space)
        }
    }

    pub fn has_text(&self) -> bool {
        self.parts
            .iter()
            .any(|p| matches!(p, Part::Text { text, .. } if !text.trim().is_empty()))
    }

    pub fn gaps(&self) -> (u16, u32) {
        self.parts.iter().fold((0, 0), |(n, ms), p| match p {
            Part::Gap { voiced_ms, .. } => (n + 1, ms + voiced_ms),
            _ => (n, ms),
        })
    }

    pub fn text_parts(&self) -> u16 {
        self.parts
            .iter()
            .filter(|p| matches!(p, Part::Text { .. }))
            .count() as u16
    }

    fn confidence(&self) -> (f32, Option<f32>) {
        let vendor = self
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text {
                    vendor_confidence, ..
                } => *vendor_confidence,
                _ => None,
            })
            .fold(None, |acc: Option<f32>, v| {
                Some(acc.map_or(v, |a| a.min(v)))
            });
        let derived = self
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text {
                    derived_confidence, ..
                } => *derived_confidence,
                _ => None,
            })
            .fold(None, |acc: Option<f32>, v| {
                Some(acc.map_or(v, |a| a.min(v)))
            });
        (vendor.or(derived).unwrap_or(1.0), vendor)
    }

    fn language(&self) -> Option<String> {
        self.parts.iter().rev().find_map(|p| match p {
            Part::Text {
                language: Some(l), ..
            } => Some(l.clone()),
            _ => None,
        })
    }

    fn request_id(&self) -> Option<String> {
        self.parts.iter().rev().find_map(|p| match p {
            Part::Text {
                request_id: Some(r),
                ..
            } => Some(r.clone()),
            _ => None,
        })
    }

    /// An interim: the turn so far, not final.
    pub fn interim(&self, turn_id: u64) -> EngineResult {
        let (confidence, vendor) = self.confidence();
        EngineResult {
            turn_id,
            transcript: self.joined(),
            is_final: false,
            is_speech_final: false,
            confidence,
            vendor_confidence: vendor,
            detected_language: self.language(),
            audio_duration: None,
            vendor_request_id: self.request_id(),
        }
    }

    /// The turn's one final: final and end of turn together.
    pub fn final_result(&self, turn_id: u64, audio_duration: Option<f64>) -> EngineResult {
        let (confidence, vendor) = self.confidence();
        EngineResult {
            turn_id,
            transcript: self.joined(),
            is_final: true,
            is_speech_final: true,
            confidence,
            vendor_confidence: vendor,
            detected_language: self.language(),
            audio_duration,
            vendor_request_id: self.request_id(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(seq: u32, t: &str, cut: CutReason) -> Part {
        Part::Text {
            seq,
            text: t.into(),
            cut,
            vendor_confidence: None,
            derived_confidence: None,
            language: None,
            request_id: None,
        }
    }

    #[test]
    fn parts_join_with_one_space_and_keep_their_punctuation() {
        let mut t = TurnText::new(Some("en"));
        t.push(text(1, "I'd like to", CutReason::Pause));
        t.push(text(2, "Change my booking.", CutReason::Pause));
        assert_eq!(t.joined(), "I'd like to Change my booking.");
        assert_eq!(t.interim(3).transcript, t.joined());
        assert!(!t.interim(3).is_final);
        let f = t.final_result(3, Some(2.4));
        assert!(f.is_final && f.is_speech_final);
        assert_eq!(f.confidence, 1.0, "no vendor confidence: exactly 1.0");
    }

    #[test]
    fn chinese_parts_join_without_spaces() {
        let mut t = TurnText::new(Some("zh-CN"));
        t.push(text(1, "你好", CutReason::Pause));
        t.push(text(2, "世界", CutReason::Pause));
        assert_eq!(t.joined(), "你好世界");
    }

    #[test]
    fn a_hard_split_seam_loses_its_full_stop_and_duplicated_words() {
        let mut t = TurnText::new(Some("en"));
        t.push(text(1, "we went to the store.", CutReason::HardSplit));
        t.push(text(2, "the store and bought milk", CutReason::Pause));
        assert_eq!(t.joined(), "we went to the store and bought milk");
    }

    #[test]
    fn a_pause_seam_is_never_rewritten() {
        let mut t = TurnText::new(Some("en"));
        t.push(text(1, "No.", CutReason::Pause));
        t.push(text(2, "No.", CutReason::Pause));
        assert_eq!(t.joined(), "No. No.");
    }

    #[test]
    fn gaps_are_counted_and_never_written_into_the_text() {
        let mut t = TurnText::new(None);
        t.push(text(1, "my account number is", CutReason::Pause));
        let off = t.offset();
        t.push(Part::Gap {
            seq: 2,
            voiced_ms: 1200,
        });
        t.push(text(3, "thanks", CutReason::Pause));
        assert_eq!(t.joined(), "my account number is thanks");
        assert_eq!(off, 21);
        assert_eq!(t.gaps(), (1, 1200));
        assert!(t.has_text());
    }

    #[test]
    fn the_turn_reports_its_lowest_vendor_confidence() {
        let mut t = TurnText::new(None);
        for (i, c) in [(1, 0.9f32), (2, 0.6)] {
            t.push(Part::Text {
                seq: i,
                text: "x".into(),
                cut: CutReason::Pause,
                vendor_confidence: Some(c),
                derived_confidence: None,
                language: Some("en".into()),
                request_id: None,
            });
        }
        let f = t.final_result(1, None);
        assert_eq!(f.confidence, 0.6);
        assert_eq!(f.vendor_confidence, Some(0.6));
        assert_eq!(f.detected_language.as_deref(), Some("en"));
    }
}

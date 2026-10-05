//! The check for invented text, applied when a segment is released in order.
//!
//! File models invent text on short or non-speech clips (Whisper produced text for 40% of
//! non-speech clips and 52% of one-second clips in the research), so this defence is required.
//! Release 1 drops only what is safe to drop: explicit no-speech, empty or tag-only text, the
//! Whisper no-speech signal where the row trusts it, credit lines and video outros, and an echo of
//! the vocabulary prompt. Loops are collapsed. Low confidence on its own and the suspect phrases
//! ("thank you", "okay") are only counted (shadow mode): dropping a real "Okay." is a functional
//! failure for a voice agent.

use crate::transcriber::SegmentTranscript;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterReason {
    NoSpeech,
    LowConfidence,
    AudioEventOnly,
    NonLexical,
    HallucinationPhrase,
    PromptEcho,
    SuspectCorroborated,
}

impl FilterReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoSpeech => "no_speech",
            Self::LowConfidence => "low_confidence",
            Self::AudioEventOnly => "audio_event_only",
            Self::NonLexical => "non_lexical",
            Self::HallucinationPhrase => "hallucination_phrase",
            Self::PromptEcho => "prompt_echo",
            Self::SuspectCorroborated => "suspect_corroborated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rewrite {
    TagsStripped,
    LoopCollapsed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QualityVerdict {
    Keep {
        text: String,
        rewritten: Option<Rewrite>,
        /// Matched a suspect rule; with the overlap flag, turn-taking may ignore it.
        suspect: bool,
        /// Shadow rules that would have dropped this text.
        shadow: Vec<FilterReason>,
    },
    Empty,
    Filtered(FilterReason),
}

/// What only the engine knows about the segment.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SegmentEvidence {
    pub voiced_ms: u32,
    pub mean_speech_probability: Option<f32>,
    pub overlapped_agent_speech: bool,
    pub agent_spoke_since_previous_segment: bool,
}

/// Built once per session from the row and the request.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityPolicy {
    /// The row lists Whisper's no-speech signal as trustworthy for this model.
    pub no_speech_signal: bool,
    /// The vocabulary prompt the requests carry.
    pub sent_prompt: Option<String>,
    /// Off for a deployment whose subject is video.
    pub video_outros: bool,
    pub no_speech_with_logprob: (f32, f32),
    pub no_speech_alone: f32,
    pub low_logprob: f32,
    pub compression_ratio: f32,
}

impl Default for QualityPolicy {
    fn default() -> Self {
        Self {
            no_speech_signal: false,
            sent_prompt: None,
            video_outros: true,
            no_speech_with_logprob: (0.6, -1.0),
            no_speech_alone: 0.8,
            low_logprob: -1.0,
            compression_ratio: 2.4,
        }
    }
}

/// Whole-text credit lines and video outros that file models produce on silence and noise.
const HALLUCINATIONS: &[&str] = &[
    "thank you for watching",
    "thanks for watching",
    "thank you so much for watching",
    "thank you very much for watching",
    "thanks for watching and see you next time",
    "please subscribe",
    "please like and subscribe",
    "like and subscribe",
    "don't forget to like and subscribe",
    "subscribe to my channel",
    "see you in the next video",
    "see you next time",
    "subtitles by the amara.org community",
    "subtitles by",
    "subtitled by",
    "transcribed by",
    "transcription by castingwords",
    "captions by",
    "amara.org",
    "www.mooji.org",
    "untertitel der amara.org-community",
    "untertitel im auftrag des zdf, 2017",
    "sous-titres réalisés par la communauté d'amara.org",
    "sous-titrage st' 501",
    "subtítulos realizados por la comunidad de amara.org",
    "ご視聴ありがとうございました",
    "字幕由amara.org社区提供",
    "продолжение следует",
];

/// Short polite words that are often invented, but often said. Counted, never dropped in Release 1.
const SUSPECTS: &[&str] = &["thank you", "thanks", "you", "bye", "okay", "ok", "yeah", "so", "oh"];

const NUMBER_WORDS: &[&str] = &[
    "zero", "oh", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "double", "triple",
];

/// Case-folded words with punctuation removed (apostrophes kept), for whole-text comparisons.
fn normalise(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Remove `[music]`, `(laughter)`, `*applause*` and musical-note characters.
fn strip_tags(s: &str) -> (String, bool) {
    let mut out = String::with_capacity(s.len());
    let mut depth_sq = 0;
    let mut depth_par = 0;
    let mut star = false;
    let mut stripped = false;
    for c in s.chars() {
        match c {
            '[' => {
                depth_sq += 1;
                stripped = true;
            }
            ']' if depth_sq > 0 => depth_sq -= 1,
            '(' => {
                depth_par += 1;
                stripped = true;
            }
            ')' if depth_par > 0 => depth_par -= 1,
            '*' => {
                star = !star;
                stripped = true;
            }
            '♪' | '♫' | '♬' | '♩' => stripped = true,
            _ if depth_sq == 0 && depth_par == 0 && !star => out.push(c),
            _ => {}
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (collapsed, stripped)
}

fn is_number_or_letter(w: &str) -> bool {
    let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    w.chars().all(|c| c.is_ascii_digit()) || w.chars().count() == 1 || NUMBER_WORDS.contains(&w.as_str())
}

/// Collapse a run of two or more words repeated four or more times in a row, never on digits,
/// number words or single letters.
fn collapse_loops(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let n = words.len();
    for len in 2..=(n / 4).max(2) {
        let mut i = 0;
        while i + len * 4 <= n {
            let unit: Vec<String> = words[i..i + len].iter().map(|w| normalise(w)).collect();
            if unit.iter().all(|w| is_number_or_letter(w)) {
                i += 1;
                continue;
            }
            let mut reps = 1;
            while i + len * (reps + 1) <= n
                && words[i + len * reps..i + len * (reps + 1)]
                    .iter()
                    .map(|w| normalise(w))
                    .collect::<Vec<_>>()
                    == unit
            {
                reps += 1;
            }
            if reps >= 4 {
                let mut out: Vec<&str> = words[..i + len].to_vec();
                out.extend_from_slice(&words[i + len * reps..]);
                return Some(collapse_loops(&out.join(" ")).unwrap_or_else(|| out.join(" ")));
            }
            i += 1;
        }
    }
    None
}

/// The verdict for one released transcript.
pub fn evaluate(t: &SegmentTranscript, ev: &SegmentEvidence, p: &QualityPolicy) -> QualityVerdict {
    // 1. The vendor said there was no speech.
    if t.vendor_said_no_speech {
        return QualityVerdict::Empty;
    }
    // 2. Empty, whitespace or punctuation only.
    if !t.text.chars().any(|c| c.is_alphanumeric()) {
        return QualityVerdict::Empty;
    }
    // 3. Tags.
    let (stripped, had_tags) = strip_tags(&t.text);
    if had_tags && !stripped.chars().any(|c| c.is_alphanumeric()) {
        return QualityVerdict::Filtered(FilterReason::NonLexical);
    }
    let mut text = if had_tags { stripped } else { t.text.trim().to_string() };
    let mut rewritten = had_tags.then_some(Rewrite::TagsStripped);
    // 4 and 5. Whisper's no-speech signal, where the row trusts it.
    if p.no_speech_signal
        && let Some(ns) = t.no_speech_prob
    {
        if ns > p.no_speech_with_logprob.0 && t.avg_logprob.is_some_and(|l| l < p.no_speech_with_logprob.1) {
            return QualityVerdict::Filtered(FilterReason::NoSpeech);
        }
        if ns > p.no_speech_alone {
            return QualityVerdict::Filtered(FilterReason::NoSpeech);
        }
    }
    let norm = normalise(&text);
    // 6. Credit lines and outros.
    if p.video_outros && HALLUCINATIONS.iter().any(|h| normalise(h) == norm) {
        return QualityVerdict::Filtered(FilterReason::HallucinationPhrase);
    }
    // 7. The vocabulary prompt echoed back.
    if let Some(prompt) = &p.sent_prompt {
        let pn = normalise(prompt);
        if pn.split_whitespace().count() >= 3 && pn == norm {
            return QualityVerdict::Filtered(FilterReason::PromptEcho);
        }
    }
    // 8. Loops.
    if let Some(collapsed) = collapse_loops(&text) {
        text = collapsed;
        rewritten = Some(Rewrite::LoopCollapsed);
    }
    // 9 and 10, shadow: counted and kept.
    let mut shadow = Vec::new();
    if t.avg_logprob.is_some_and(|l| l < p.low_logprob) {
        shadow.push(FilterReason::LowConfidence);
    }
    let suspect = SUSPECTS.contains(&norm.as_str()) || is_bare_web_address(&text);
    if suspect && (ev.voiced_ms < 400 || ev.overlapped_agent_speech || ev.mean_speech_probability.is_some_and(|m| m < 0.6)) {
        shadow.push(FilterReason::SuspectCorroborated);
    }
    QualityVerdict::Keep {
        text,
        rewritten,
        suspect,
        shadow,
    }
}

fn is_bare_web_address(text: &str) -> bool {
    let t = text.trim().trim_end_matches('.').to_lowercase();
    !t.contains(' ') && (t.starts_with("www.") || t.ends_with(".com") || t.ends_with(".org"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> SegmentTranscript {
        SegmentTranscript {
            text: text.to_string(),
            ..Default::default()
        }
    }

    fn keep(v: &QualityVerdict) -> &str {
        match v {
            QualityVerdict::Keep { text, .. } => text,
            other => panic!("expected Keep, got {other:?}"),
        }
    }

    #[test]
    fn empty_and_punctuation_only_text_is_empty() {
        let p = QualityPolicy::default();
        assert_eq!(evaluate(&t(""), &SegmentEvidence::default(), &p), QualityVerdict::Empty);
        assert_eq!(evaluate(&t(" ... "), &SegmentEvidence::default(), &p), QualityVerdict::Empty);
        let mut ns = t("hello");
        ns.vendor_said_no_speech = true;
        assert_eq!(evaluate(&ns, &SegmentEvidence::default(), &p), QualityVerdict::Empty);
    }

    #[test]
    fn tag_only_text_is_filtered_and_tags_are_stripped_from_real_text() {
        let p = QualityPolicy::default();
        assert_eq!(evaluate(&t("[MUSIC]"), &SegmentEvidence::default(), &p), QualityVerdict::Filtered(FilterReason::NonLexical));
        assert_eq!(evaluate(&t("(laughter) ♪"), &SegmentEvidence::default(), &p), QualityVerdict::Filtered(FilterReason::NonLexical));
        let v = evaluate(&t("(laughs) I want a refund"), &SegmentEvidence::default(), &p);
        assert_eq!(keep(&v), "I want a refund");
    }

    #[test]
    fn credit_lines_and_outros_are_dropped() {
        let p = QualityPolicy::default();
        for s in ["Thank you for watching!", "Subtitles by the Amara.org community", "ご視聴ありがとうございました"] {
            assert_eq!(evaluate(&t(s), &SegmentEvidence::default(), &p), QualityVerdict::Filtered(FilterReason::HallucinationPhrase), "{s}");
        }
        let video = QualityPolicy { video_outros: false, ..QualityPolicy::default() };
        assert!(matches!(evaluate(&t("Thanks for watching"), &SegmentEvidence::default(), &video), QualityVerdict::Keep { .. }));
    }

    #[test]
    fn the_no_speech_signal_drops_only_where_the_row_trusts_it() {
        let mut x = t("Bye.");
        x.no_speech_prob = Some(0.9);
        let untrusted = QualityPolicy::default();
        assert!(matches!(evaluate(&x, &SegmentEvidence::default(), &untrusted), QualityVerdict::Keep { .. }));
        let trusted = QualityPolicy { no_speech_signal: true, ..QualityPolicy::default() };
        assert_eq!(evaluate(&x, &SegmentEvidence::default(), &trusted), QualityVerdict::Filtered(FilterReason::NoSpeech));
        x.no_speech_prob = Some(0.7);
        x.avg_logprob = Some(-1.2);
        assert_eq!(evaluate(&x, &SegmentEvidence::default(), &trusted), QualityVerdict::Filtered(FilterReason::NoSpeech));
        x.avg_logprob = Some(-0.3);
        assert!(matches!(evaluate(&x, &SegmentEvidence::default(), &trusted), QualityVerdict::Keep { .. }));
    }

    #[test]
    fn a_prompt_echo_of_three_or_more_words_is_dropped() {
        let p = QualityPolicy { sent_prompt: Some("Acme order status refund".into()), ..QualityPolicy::default() };
        assert_eq!(evaluate(&t("Acme, order status, refund."), &SegmentEvidence::default(), &p), QualityVerdict::Filtered(FilterReason::PromptEcho));
        let short = QualityPolicy { sent_prompt: Some("Acme".into()), ..QualityPolicy::default() };
        assert!(matches!(evaluate(&t("Acme"), &SegmentEvidence::default(), &short), QualityVerdict::Keep { .. }));
    }

    #[test]
    fn loops_are_collapsed_but_numbers_never_are() {
        let p = QualityPolicy::default();
        let v = evaluate(&t("I want to I want to I want to I want to cancel"), &SegmentEvidence::default(), &p);
        assert_eq!(keep(&v), "I want to cancel");
        assert!(matches!(v, QualityVerdict::Keep { rewritten: Some(Rewrite::LoopCollapsed), .. }));
        let digits = evaluate(&t("zero zero zero zero zero zero zero zero one"), &SegmentEvidence::default(), &p);
        assert_eq!(keep(&digits), "zero zero zero zero zero zero zero zero one");
        let account = evaluate(&t("one two one two one two one two"), &SegmentEvidence::default(), &p);
        assert_eq!(keep(&account), "one two one two one two one two");
    }

    #[test]
    fn a_short_okay_is_kept_and_only_counted_as_a_suspect() {
        let p = QualityPolicy::default();
        let ev = SegmentEvidence { voiced_ms: 320, ..Default::default() };
        let v = evaluate(&t("Okay."), &ev, &p);
        assert_eq!(keep(&v), "Okay.");
        assert!(matches!(&v, QualityVerdict::Keep { suspect: true, shadow, .. } if shadow.contains(&FilterReason::SuspectCorroborated)));
    }

    #[test]
    fn low_confidence_alone_is_only_shadowed() {
        let mut x = t("I need a refund");
        x.avg_logprob = Some(-1.5);
        let v = evaluate(&x, &SegmentEvidence::default(), &QualityPolicy::default());
        assert!(matches!(&v, QualityVerdict::Keep { shadow, .. } if shadow == &vec![FilterReason::LowConfidence]));
    }
}

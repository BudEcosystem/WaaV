//! What the user actually HEARD of an agent's reply (spec 025 D-9, D-10, DEG-5).
//!
//! When the user talks over the agent, budprompt must remember only what was spoken — otherwise the
//! model believes the user heard an answer they never did, and the next turn builds on it. WaaV knows
//! which chunks it sent to TTS, in order, and how much audio has played; this module turns that into
//! the spoken prefix of the reply's TEXT.
//!
//! Precision: every chunk sent before the cut chunk was heard whole. Inside the cut chunk the split
//! is proportional to the audio played (cut back to a word boundary), at a per-session speaking rate
//! calibrated from the session's own completed replies. A GA client that sends
//! `conversation.item.truncate` with `audio_end_ms` replaces the playback estimate with its own.
use super::text::{chunk_separator, is_unspaced};

/// Default speaking rate before a session has calibrated its own: ~14.5 characters per second, the
/// middle of what neural TTS voices produce at speed 1.0.
pub const DEFAULT_CHARS_PER_MS: f64 = 0.0145;

/// One chunk sent to TTS this turn.
#[derive(Debug, Clone)]
struct Entry {
    /// What the agent said (the transcript's text for this chunk).
    transcript: String,
    /// How many characters the TTS vendor received for it (after the speech transforms).
    speech_chars: usize,
    /// `false` for a filler or greeting: it takes playback time but is not part of the reply.
    reply: bool,
}

/// The chunks of one turn, in the order they were sent to TTS.
#[derive(Debug, Clone, Default)]
pub struct SpokenLedger {
    entries: Vec<Entry>,
    /// The gateway's audio-out counter when this turn's first chunk was sent.
    audio_mark_ms: Option<u64>,
}

/// The heard part of a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct SpokenPrefix {
    pub text: String,
    /// The whole reply was heard: there is nothing to truncate.
    pub complete: bool,
    /// Characters of the reply text that were heard.
    pub chars: usize,
}

impl SpokenLedger {
    /// Record a chunk sent to TTS.
    pub fn push(&mut self, transcript: &str, speech: &str, reply: bool, audio_out_ms_now: u64) {
        if self.audio_mark_ms.is_none() {
            self.audio_mark_ms = Some(audio_out_ms_now);
        }
        self.entries.push(Entry {
            transcript: transcript.to_string(),
            speech_chars: speech.chars().count(),
            reply,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The full reply text that was sent to TTS.
    pub fn reply_text(&self) -> String {
        join(
            self.entries
                .iter()
                .filter(|e| e.reply)
                .map(|e| e.transcript.as_str()),
        )
    }

    /// Every character the TTS vendor received this turn.
    pub fn speech_chars(&self) -> usize {
        self.entries.iter().map(|e| e.speech_chars).sum()
    }

    /// Milliseconds of this turn's audio the user has heard, from the gateway's counters.
    pub fn played_ms(&self, audio_out_ms_now: u64, playout_remaining_ms: u64) -> u64 {
        let Some(mark) = self.audio_mark_ms else {
            return 0;
        };
        audio_out_ms_now
            .saturating_sub(mark)
            .saturating_sub(playout_remaining_ms)
    }

    /// The emitted audio length of this turn (for rate calibration once it has all played).
    pub fn emitted_ms(&self, audio_out_ms_now: u64) -> u64 {
        self.audio_mark_ms
            .map(|mark| audio_out_ms_now.saturating_sub(mark))
            .unwrap_or(0)
    }

    /// The reply text heard after `played_ms` of audio at `chars_per_ms`.
    pub fn spoken_prefix(&self, played_ms: u64, chars_per_ms: f64) -> SpokenPrefix {
        let budget = (played_ms as f64 * chars_per_ms.max(0.0)).round() as usize;
        let mut left = budget;
        let mut heard: Vec<String> = Vec::new();
        let total_reply: usize = self
            .entries
            .iter()
            .filter(|e| e.reply)
            .map(|e| e.transcript.chars().count())
            .sum();
        let mut complete = true;
        for e in &self.entries {
            if left >= e.speech_chars {
                left -= e.speech_chars;
                if e.reply {
                    heard.push(e.transcript.clone());
                }
                continue;
            }
            complete = false;
            if e.reply && e.speech_chars > 0 && left > 0 {
                let ratio = left as f64 / e.speech_chars as f64;
                let take = (e.transcript.chars().count() as f64 * ratio).floor() as usize;
                let partial = cut_at_word(&e.transcript, take);
                if !partial.is_empty() {
                    heard.push(partial);
                }
            }
            break;
        }
        // A later reply chunk never reached is also unheard.
        let text = join(heard.iter().map(String::as_str));
        let chars = text.chars().count();
        SpokenPrefix {
            complete: complete && chars >= total_reply,
            text,
            chars,
        }
    }
}

fn join<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for p in parts {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        out.push_str(chunk_separator(&out, p));
        out.push_str(p);
    }
    out
}

/// The first `chars` characters of `text`, cut back to the last whole word.
fn cut_at_word(text: &str, chars: usize) -> String {
    let prefix: String = text.chars().take(chars).collect();
    if prefix.chars().count() >= text.chars().count() {
        return text.trim().to_string();
    }
    // The next character decides whether the cut landed on a word boundary.
    let next = text.chars().nth(chars);
    if next.is_some_and(char::is_whitespace) {
        return prefix.trim().to_string();
    }
    // In a script written without spaces every character is a syllable or a word: a cut next to
    // one is a boundary.
    if prefix.chars().last().is_some_and(is_unspaced) || next.is_some_and(is_unspaced) {
        return prefix.trim().to_string();
    }
    match prefix.rfind(char::is_whitespace) {
        Some(i) => prefix[..i].trim().to_string(),
        None => String::new(),
    }
}

/// A session's speaking rate, calibrated from replies that played to the end.
#[derive(Debug, Clone)]
pub struct SpeechRate {
    chars_per_ms: f64,
    samples: u32,
}

impl SpeechRate {
    /// The default rate scaled by the agent's TTS speed.
    pub fn new(speed: Option<f64>) -> Self {
        let speed = speed.filter(|s| *s > 0.0).unwrap_or(1.0).clamp(0.25, 4.0);
        Self {
            chars_per_ms: DEFAULT_CHARS_PER_MS * speed,
            samples: 0,
        }
    }

    pub fn chars_per_ms(&self) -> f64 {
        self.chars_per_ms
    }

    /// Fold in a reply of `chars` characters that produced `audio_ms` of audio. Implausible samples
    /// (a vendor that reported no duration, a one-word reply) are ignored.
    pub fn observe(&mut self, chars: usize, audio_ms: u64) {
        if chars < 40 || audio_ms < 1000 {
            return;
        }
        let sample = chars as f64 / audio_ms as f64;
        if !(0.004..=0.06).contains(&sample) {
            return;
        }
        self.samples += 1;
        let alpha = if self.samples == 1 { 0.7 } else { 0.3 };
        self.chars_per_ms = alpha * sample + (1.0 - alpha) * self.chars_per_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger(chunks: &[(&str, bool)]) -> SpokenLedger {
        let mut l = SpokenLedger::default();
        for (text, reply) in chunks {
            l.push(text, text, *reply, 1000);
        }
        l
    }

    #[test]
    fn whole_chunks_heard_before_the_cut_and_a_word_cut_inside_it() {
        let l = ledger(&[
            ("Your order shipped Monday.", true),
            ("It should arrive by Friday afternoon.", true),
        ]);
        // 26 chars of the first chunk + 12 into the second = "It should ar" -> "It should".
        let p = l.spoken_prefix(38_000, 0.001);
        assert_eq!(p.text, "Your order shipped Monday. It should");
        assert!(!p.complete);
    }

    /// Scripts written without spaces: sentences join with no space, and a cut mid-sentence keeps
    /// the characters heard (each is a syllable or word) instead of backing off to nothing.
    #[test]
    fn unspaced_scripts_join_tight_and_cut_by_character() {
        let l = ledger(&[("日本語のテキストです。", true), ("続けます！", true)]);
        let all = l.spoken_prefix(100_000, 0.001);
        assert_eq!(all.text, "日本語のテキストです。続けます！");
        assert_eq!(l.reply_text(), "日本語のテキストです。続けます！");
        // 11 chars of the first chunk + 2 of the second.
        let p = l.spoken_prefix(13_000, 0.001);
        assert_eq!(p.text, "日本語のテキストです。続け");
        assert_eq!(cut_at_word("你好世界，今天很好", 4), "你好世界");
        // Mixed: a Latin word in progress is still cut back to the last whole word.
        assert_eq!(cut_at_word("Bud です hello world", 10), "Bud です");
        assert_eq!(cut_at_word("Bud です hello", 5), "Bud で");
    }

    #[test]
    fn everything_played_is_complete() {
        let l = ledger(&[("Done.", true)]);
        let p = l.spoken_prefix(10_000, 0.01);
        assert_eq!(p.text, "Done.");
        assert!(p.complete);
    }

    #[test]
    fn nothing_played_is_empty() {
        let l = ledger(&[("Hello there.", true)]);
        let p = l.spoken_prefix(0, 0.0145);
        assert_eq!(p.text, "");
        assert!(!p.complete);
    }

    #[test]
    fn a_filler_takes_time_but_is_not_the_reply() {
        let l = ledger(&[("One moment.", false), ("Found it: order 42.", true)]);
        // 11 chars of filler + 6 into the reply.
        let p = l.spoken_prefix(17_000, 0.001);
        assert_eq!(p.text, "Found");
        assert_eq!(l.reply_text(), "Found it: order 42.");
    }

    #[test]
    fn played_time_is_emitted_minus_still_queued() {
        let mut l = SpokenLedger::default();
        l.push("a", "a", true, 5_000);
        assert_eq!(l.played_ms(9_000, 1_500), 2_500);
        assert_eq!(
            l.played_ms(4_000, 0),
            0,
            "a counter behind the mark is zero, not negative"
        );
        assert_eq!(SpokenLedger::default().played_ms(9_000, 0), 0);
    }

    #[test]
    fn a_cut_on_a_space_keeps_the_word() {
        assert_eq!(cut_at_word("hello world again", 5), "hello");
        assert_eq!(cut_at_word("hello world again", 8), "hello");
        assert_eq!(cut_at_word("hello", 2), "");
        assert_eq!(cut_at_word("hello", 99), "hello");
    }

    #[test]
    fn the_rate_calibrates_from_completed_replies_and_ignores_noise() {
        let mut r = SpeechRate::new(Some(1.0));
        let start = r.chars_per_ms();
        r.observe(10, 5_000); // too short to trust
        assert_eq!(r.chars_per_ms(), start);
        r.observe(200, 10_000); // 20 chars/s
        assert!(r.chars_per_ms() > start);
        r.observe(200, 10); // implausible
        assert!(r.chars_per_ms() < 0.03);
        assert!(
            (SpeechRate::new(Some(2.0)).chars_per_ms() - 2.0 * DEFAULT_CHARS_PER_MS).abs() < 1e-9
        );
    }
}

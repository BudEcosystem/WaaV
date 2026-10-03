//! Text on its way from an agent to a TTS deployment (spec 025 D-13, D-15, FR-TTS-1/2/4).
//!
//! Four pieces, each a small streaming filter that is unit-testable without a voice stack:
//!
//! * [`SpeechChunker`] — the first chunk goes out at the first CLAUSE boundary (or 60 characters),
//!   every later one at a sentence boundary. The first spoken audio is what the caller waits for;
//!   holding it for a whole sentence adds a sentence of latency to every turn.
//! * [`CodeFenceFilter`] — a fenced code block is never read aloud character by character; it is
//!   omitted, or replaced by its contents when the agent says `read`.
//! * [`transform_for_speech`] — deterministic clean-up of one chunk: URLs, markdown, emoji. Prompts
//!   drift; these are exact and cheap. The TRANSCRIPT keeps the original text.
//! * [`SpeakFieldExtractor`] — a structured-output agent streams JSON; only the designated string
//!   field is spoken, decoded incrementally as the JSON arrives.

use std::sync::OnceLock;

use bud_auth::voice_agent::AgentTextTransforms;
use regex::Regex;

use crate::core::text::{SentenceAggregator, strip_markdown_for_tts};

// =============================================================================================
// SpeechChunker
// =============================================================================================

/// Below this many characters a clause is too short to be worth its own TTS request.
const MIN_FIRST_CLAUSE_CHARS: usize = 12;
/// The first chunk never waits for more than this many characters.
const MAX_FIRST_CHUNK_CHARS: usize = 60;

/// First chunk at the first clause boundary, then whole sentences.
#[derive(Debug)]
pub struct SpeechChunker {
    agg: SentenceAggregator,
    /// The text before the first chunk is emitted; `None` once it has been.
    head: Option<String>,
}

impl Default for SpeechChunker {
    fn default() -> Self {
        Self {
            agg: SentenceAggregator::default(),
            head: Some(String::new()),
        }
    }
}

/// Byte index just past the first clause boundary worth cutting at, if any.
fn first_clause_cut(head: &str) -> Option<usize> {
    let mut chars = 0usize;
    let mut iter = head.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        chars += 1;
        if matches!(c, ',' | ';' | ':' | '—' | '–') && chars >= MIN_FIRST_CLAUSE_CHARS {
            // A clause boundary is punctuation FOLLOWED BY whitespace: "3,000" and "10:30" are not.
            if let Some((_, next)) = iter.peek()
                && next.is_whitespace()
            {
                return Some(i + c.len_utf8());
            }
        }
    }
    None
}

/// Byte index of the last whitespace at or before `max_chars` characters, for a run-on head.
fn word_cut(head: &str, max_chars: usize) -> Option<usize> {
    let mut last_space = None;
    for (n, (i, c)) in head.char_indices().enumerate() {
        if n >= max_chars {
            break;
        }
        if c.is_whitespace() && n >= MIN_FIRST_CLAUSE_CHARS {
            last_space = Some(i);
        }
    }
    last_space
}

/// Whether `c` belongs to a script written without spaces between words (CJK ideographs, kana,
/// their punctuation and full-width forms, and the Southeast Asian scripts).
pub(crate) fn is_unspaced(c: char) -> bool {
    matches!(c as u32,
        0x0E00..=0x0EFF      // Thai, Lao
        | 0x1000..=0x109F    // Myanmar
        | 0x1780..=0x17FF    // Khmer
        | 0x3000..=0x30FF    // CJK punctuation, Hiragana, Katakana
        | 0x3400..=0x4DBF    // CJK extension A
        | 0x4E00..=0x9FFF    // CJK unified ideographs
        | 0xF900..=0xFAFF    // CJK compatibility ideographs
        | 0xFF00..=0xFFEF    // Full-width forms
        | 0x20000..=0x2FFFF  // CJK extensions B–F
    )
}

/// What goes between two spoken chunks of one reply: a space, except where both sides are written
/// without spaces ("です。" + "続けます" — chunking dropped no whitespace there).
pub fn chunk_separator(before: &str, next: &str) -> &'static str {
    match (before.chars().last(), next.chars().next()) {
        (Some(a), Some(b)) if is_unspaced(a) && is_unspaced(b) => "",
        (Some(a), Some(b)) if a.is_whitespace() || b.is_whitespace() => "",
        (Some(_), Some(_)) => " ",
        _ => "",
    }
}

fn has_sentence_terminator(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '\n'))
}

impl SpeechChunker {
    /// Feed a delta; returns 0+ chunks ready to be spoken.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        let Some(head) = self.head.as_mut() else {
            return self.agg.push_str(delta);
        };
        head.push_str(delta);

        // A sentence end already in hand beats a clause cut: the whole sentence is one natural TTS
        // request, and the aggregator disambiguates abbreviations and decimals.
        if has_sentence_terminator(head) {
            let all = std::mem::take(head);
            self.head = None;
            return self.agg.push_str(&all);
        }
        if let Some(cut) = first_clause_cut(head) {
            let (first, rest) = head.split_at(cut);
            let first = first.trim().to_string();
            let rest = rest.to_string();
            self.head = None;
            let mut out = Vec::new();
            if !first.is_empty() {
                out.push(first);
            }
            out.extend(self.agg.push_str(&rest));
            return out;
        }
        if head.chars().count() >= MAX_FIRST_CHUNK_CHARS
            && let Some(cut) = word_cut(head, MAX_FIRST_CHUNK_CHARS)
        {
            let (first, rest) = head.split_at(cut);
            let first = first.trim().to_string();
            let rest = rest.trim_start().to_string();
            self.head = None;
            let mut out = vec![first];
            out.extend(self.agg.push_str(&rest));
            return out;
        }
        Vec::new()
    }

    /// End of the reply: whatever is held.
    pub fn flush(&mut self) -> Option<String> {
        if let Some(head) = self.head.take() {
            let t = head.trim();
            return (!t.is_empty()).then(|| t.to_string());
        }
        self.agg.flush()
    }
}

// =============================================================================================
// CodeFenceFilter
// =============================================================================================

const FENCE: &str = "```";

/// Removes fenced code blocks from a streamed reply, tags split across deltas included.
#[derive(Debug, Default)]
pub struct CodeFenceFilter {
    /// `read` mode: speak the block's contents (without the fences and language tag).
    read: bool,
    in_block: bool,
    /// Inside a block, before its first newline: the language tag, never spoken.
    in_tag: bool,
    /// A trailing fragment that might be the start of a fence.
    pending: String,
    omitted_any: bool,
}

impl CodeFenceFilter {
    pub fn new(code_blocks: &str) -> Self {
        Self {
            read: code_blocks == "read",
            ..Default::default()
        }
    }

    /// Whether a block was dropped this reply (so the caller can say "I've put the code in the chat").
    pub fn omitted_any(&self) -> bool {
        self.omitted_any
    }

    fn partial_fence_tail(s: &str) -> usize {
        for n in (1..FENCE.len()).rev() {
            if s.len() >= n && s.ends_with(&FENCE[..n]) {
                return n;
            }
        }
        0
    }

    /// Filter one delta.
    pub fn push(&mut self, delta: &str) -> String {
        let mut buf = std::mem::take(&mut self.pending);
        buf.push_str(delta);
        let mut out = String::new();
        loop {
            match buf.find(FENCE) {
                Some(pos) => {
                    let before = &buf[..pos];
                    self.emit(before, &mut out);
                    buf.drain(..pos + FENCE.len());
                    self.in_block = !self.in_block;
                    self.in_tag = self.in_block;
                    if self.in_block && !self.read {
                        self.omitted_any = true;
                    }
                }
                None => {
                    let hold = Self::partial_fence_tail(&buf);
                    let cut = buf.len() - hold;
                    let visible = buf[..cut].to_string();
                    self.emit(&visible, &mut out);
                    self.pending = buf[cut..].to_string();
                    break;
                }
            }
        }
        out
    }

    fn emit(&mut self, text: &str, out: &mut String) {
        if !self.in_block {
            out.push_str(text);
            return;
        }
        let mut text = text;
        if self.in_tag {
            match text.find('\n') {
                Some(nl) => {
                    text = &text[nl + 1..];
                    self.in_tag = false;
                }
                None => return,
            }
        }
        if self.read {
            out.push_str(text);
        }
    }

    /// End of stream: an unterminated fence never leaks its contents.
    pub fn flush(&mut self) -> String {
        let tail = std::mem::take(&mut self.pending);
        if self.in_block { String::new() } else { tail }
    }
}

// =============================================================================================
// transform_for_speech
// =============================================================================================

fn url_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:https?://|www\.)[^\s<>()\[\]"']+"#)
            .map_err(|e| tracing::warn!(error = %e, "speech URL regex unavailable"))
            .ok()
    })
    .as_ref()
}

/// The host of a URL match, without `www.` and without trailing punctuation.
fn url_domain(url: &str) -> String {
    let no_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = no_scheme
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or(no_scheme);
    host.trim_start_matches("www.")
        .trim_end_matches(['.', ',', ';', '!', '?'])
        .to_string()
}

/// A URL's trailing sentence punctuation belongs to the sentence, not the URL.
fn split_trailing_punctuation(url: &str) -> (&str, &str) {
    let trimmed = url.trim_end_matches(['.', ',', ';', '!', '?', ':']);
    (trimmed, &url[trimmed.len()..])
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF   // pictographs, emoticons, transport, flags, symbols & pictographs ext.
        | 0x2600..=0x27BF   // misc symbols, dingbats
        | 0x2B00..=0x2BFF   // arrows & stars (⭐)
        | 0xFE0E..=0xFE0F   // variation selectors
        | 0x200D            // zero-width joiner
        | 0x20E3            // combining keycap
        | 0xE0020..=0xE007F // tags
    )
}

/// Clean one chunk for TTS. The input is what the agent said; the output is what the caller hears.
pub fn transform_for_speech(text: &str, t: &AgentTextTransforms) -> String {
    let mut out = text.to_string();

    if t.urls != "full"
        && let Some(re) = url_regex()
        && re.is_match(&out)
    {
        out = re
            .replace_all(&out, |caps: &regex::Captures<'_>| {
                let (url, tail) = split_trailing_punctuation(&caps[0]);
                match t.urls.as_str() {
                    "omit" => tail.to_string(),
                    _ => format!("{}{tail}", url_domain(url)),
                }
            })
            .into_owned();
    }
    if t.strip_markdown {
        out = strip_markdown_for_tts(&out);
    }
    if t.strip_emoji && out.chars().any(is_emoji) {
        out = out.chars().filter(|c| !is_emoji(*c)).collect();
    }
    // Collapse what the removals left behind ("Great  !" → "Great!").
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_space = false;
    for c in out.chars() {
        if c.is_whitespace() {
            if !prev_space {
                collapsed.push(' ');
            }
            prev_space = true;
        } else {
            if prev_space && matches!(c, '.' | ',' | '!' | '?' | ';' | ':') {
                collapsed.pop();
            }
            collapsed.push(c);
            prev_space = false;
        }
    }
    collapsed.trim().to_string()
}

// =============================================================================================
// SpeakFieldExtractor
// =============================================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JState {
    /// Before the opening `{`.
    Start,
    /// Inside the top-level object, expecting a key (or `}`).
    ExpectKey,
    /// Reading a top-level key.
    InKey,
    /// After a key, expecting `:`.
    ExpectColon,
    /// After `:`, expecting a value.
    ExpectValue,
    /// Reading the TARGET field's string value.
    InTargetString,
    /// Reading some other string (a nested key or value); `depth` says where.
    InOtherString,
    /// Inside a non-target value (number, literal, nested object/array).
    InOtherValue,
    /// The top-level object closed.
    Done,
}

/// Decodes one top-level string field of a JSON object as the JSON streams in.
#[derive(Debug)]
pub struct SpeakFieldExtractor {
    field: String,
    state: JState,
    depth: usize,
    key: String,
    escape: bool,
    /// `\uXXXX` in progress.
    unicode: Option<String>,
    /// A high surrogate awaiting its low half.
    surrogate: Option<u16>,
    found: bool,
    /// The whole JSON text seen, for the `agent_output` event at the end.
    raw: String,
}

impl SpeakFieldExtractor {
    pub fn new(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            state: JState::Start,
            depth: 0,
            key: String::new(),
            escape: false,
            unicode: None,
            surrogate: None,
            found: false,
            raw: String::new(),
        }
    }

    /// Whether the field has been seen (its value is being or was decoded).
    pub fn found(&self) -> bool {
        self.found
    }

    /// The JSON text seen so far.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// The whole output as JSON, when it parses.
    pub fn output(&self) -> Option<serde_json::Value> {
        let text = self.raw.trim();
        let text = text
            .strip_prefix("```json")
            .or_else(|| text.strip_prefix("```"))
            .map(|t| t.trim_end_matches("```").trim())
            .unwrap_or(text);
        serde_json::from_str(text).ok()
    }

    /// Feed one delta of the JSON text; returns the newly decoded characters of the field.
    pub fn push(&mut self, delta: &str) -> String {
        self.raw.push_str(delta);
        let mut out = String::new();
        for c in delta.chars() {
            self.step(c, &mut out);
        }
        out
    }

    fn decode_into(&mut self, c: char, out: &mut String) {
        // Called only inside the target string.
        if let Some(hex) = self.unicode.as_mut() {
            hex.push(c);
            if hex.len() == 4 {
                let code = u16::from_str_radix(hex, 16).unwrap_or(0xFFFD);
                self.unicode = None;
                if (0xD800..0xDC00).contains(&code) {
                    self.surrogate = Some(code);
                } else if (0xDC00..0xE000).contains(&code) {
                    if let Some(high) = self.surrogate.take() {
                        let combined =
                            0x10000 + (((high as u32) - 0xD800) << 10) + ((code as u32) - 0xDC00);
                        out.push(char::from_u32(combined).unwrap_or('\u{FFFD}'));
                    }
                } else {
                    out.push(char::from_u32(code as u32).unwrap_or('\u{FFFD}'));
                }
            }
            return;
        }
        if self.escape {
            self.escape = false;
            match c {
                'n' => out.push('\n'),
                't' => out.push(' '),
                'r' => {}
                'b' | 'f' => {}
                'u' => self.unicode = Some(String::new()),
                other => out.push(other),
            }
            return;
        }
        match c {
            '\\' => self.escape = true,
            '"' => self.state = JState::ExpectKey,
            _ => out.push(c),
        }
    }

    fn step(&mut self, c: char, out: &mut String) {
        match self.state {
            JState::Done => {}
            JState::Start => {
                if c == '{' {
                    self.state = JState::ExpectKey;
                    self.depth = 1;
                }
            }
            JState::ExpectKey => match c {
                '"' => {
                    self.key.clear();
                    self.state = JState::InKey;
                }
                '}' => self.state = JState::Done,
                _ => {}
            },
            JState::InKey => {
                if self.escape {
                    self.escape = false;
                    self.key.push(c);
                } else if c == '\\' {
                    self.escape = true;
                } else if c == '"' {
                    self.state = JState::ExpectColon;
                } else {
                    self.key.push(c);
                }
            }
            JState::ExpectColon => {
                if c == ':' {
                    self.state = JState::ExpectValue;
                }
            }
            JState::ExpectValue => {
                if c.is_whitespace() {
                    return;
                }
                if c == '"' && self.key == self.field {
                    self.found = true;
                    self.state = JState::InTargetString;
                } else if c == '"' {
                    self.state = JState::InOtherString;
                } else if c == '{' || c == '[' {
                    self.depth = 2;
                    self.state = JState::InOtherValue;
                } else {
                    self.state = JState::InOtherValue;
                    self.depth = 1;
                }
            }
            JState::InTargetString => self.decode_into(c, out),
            JState::InOtherString => {
                if self.escape {
                    self.escape = false;
                } else if c == '\\' {
                    self.escape = true;
                } else if c == '"' {
                    self.state = if self.depth > 1 {
                        JState::InOtherValue
                    } else {
                        JState::ExpectKey
                    };
                }
            }
            JState::InOtherValue => match c {
                '"' => self.state = JState::InOtherString,
                '{' | '[' => self.depth += 1,
                '}' | ']' => {
                    self.depth -= 1;
                    if self.depth == 0 {
                        self.state = JState::Done;
                    } else if self.depth == 1 {
                        self.state = JState::ExpectKey;
                    }
                }
                ',' if self.depth == 1 => self.state = JState::ExpectKey,
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` cut into deltas of `step` characters (never inside a character).
    fn deltas(text: &str, step: usize) -> Vec<&str> {
        let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        starts
            .iter()
            .step_by(step)
            .enumerate()
            .map(|(n, &start)| {
                let end = starts.get((n + 1) * step).copied().unwrap_or(text.len());
                &text[start..end]
            })
            .collect()
    }

    fn squash(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Every streaming stage the engine runs a reply through (think → fence → chunker → speech
    /// transforms, and the speak-field extractor), over multilingual text cut at every character
    /// boundary: nothing panics and no text is lost or duplicated. A multi-byte character at the
    /// end of a delta panicked the live turn task ("Hi, I’m").
    #[test]
    fn multilingual_deltas_at_every_boundary_never_panic_or_lose_text() {
        let samples = [
            "Hi, I’m Bud — your assistant. ¿Cómo estás? 日本語のテキストです。続けます！ Ça va, très bien… 😀👍🏽 done.",
            "Привет, как дела? Всё хорошо. नमस्ते, आप कैसे हैं? मैं ठीक हूँ। مرحبا، كيف حالك؟ 안녕하세요, 반갑습니다.",
            "Ünïcödé ‘quotes’ and “double” — dashes – and ellipses… plus 3,000 at 10:30, ok?",
        ];
        let transforms = AgentTextTransforms::default();
        for text in samples {
            for step in [1usize, 2, 3, 5] {
                let mut think = crate::core::text::ThinkStripper::default();
                let mut fence = CodeFenceFilter::new("omit");
                let mut chunker = SpeechChunker::default();
                let mut chunks = Vec::new();
                for d in deltas(text, step) {
                    let t = think.push(d);
                    let t = fence.push(&t);
                    chunks.extend(chunker.push(&t));
                }
                let mut rest = fence.push(&think.flush());
                rest.push_str(&fence.flush());
                chunks.extend(chunker.push(&rest));
                chunks.extend(chunker.flush());
                for c in &chunks {
                    let _ = transform_for_speech(c, &transforms);
                }
                let joined = chunks.iter().fold(String::new(), |mut acc, c| {
                    acc.push_str(chunk_separator(&acc, c));
                    acc.push_str(c);
                    acc
                });
                assert_eq!(squash(&joined), squash(text), "step {step}");

                let json = serde_json::json!({"reply": text, "n": 1}).to_string();
                let mut x = SpeakFieldExtractor::new("reply");
                let mut spoken = String::new();
                for d in deltas(&json, step) {
                    spoken.push_str(&x.push(d));
                }
                assert_eq!(spoken, text, "speak field, step {step}");
            }
        }
    }

    fn chunks(deltas: &[&str]) -> Vec<String> {
        let mut c = SpeechChunker::default();
        let mut out = Vec::new();
        for d in deltas {
            out.extend(c.push(d));
        }
        out.extend(c.flush());
        out
    }

    #[test]
    fn chunks_join_with_a_space_except_between_unspaced_scripts() {
        assert_eq!(chunk_separator("Hello.", "World"), " ");
        assert_eq!(chunk_separator("です。", "続けます"), "");
        assert_eq!(chunk_separator("续。", "好"), "");
        assert_eq!(chunk_separator("続けます！", "Ça va"), " ");
        assert_eq!(
            chunk_separator("안녕하세요.", "반갑습니다"),
            " ",
            "Korean is spaced"
        );
        assert_eq!(chunk_separator("สวัสดีครับ", "ยินดี"), "", "Thai is not");
        assert_eq!(chunk_separator("", "First"), "");
    }

    #[test]
    fn the_first_chunk_goes_at_the_first_clause() {
        let out = chunks(&[
            "Your order shipped on Monday",
            ", and it should arrive",
            " by Friday. Anything else?",
        ]);
        assert_eq!(out[0], "Your order shipped on Monday,");
        assert_eq!(out[1], "and it should arrive by Friday.");
        assert_eq!(out[2], "Anything else?");
    }

    #[test]
    fn a_short_clause_is_not_worth_its_own_request() {
        let out = chunks(&["Yes, of course. ", "I can help with that."]);
        assert_eq!(out, vec!["Yes, of course.", "I can help with that."]);
    }

    #[test]
    fn numbers_and_times_are_not_clause_boundaries() {
        let out = chunks(&["The total was 3,000 dollars at 10:30 today"]);
        assert_eq!(out, vec!["The total was 3,000 dollars at 10:30 today"]);
    }

    #[test]
    fn a_run_on_first_chunk_is_cut_at_sixty_characters_on_a_word() {
        let text =
            "this reply never uses any punctuation at all and keeps going and going for a while";
        let out = chunks(&[text]);
        assert!(out[0].chars().count() <= 60, "{:?}", out);
        assert_eq!(out.join(" "), text);
    }

    #[test]
    fn later_chunks_are_whole_sentences() {
        let out = chunks(&[
            "Hello there, friend. ",
            "This is sentence two, which has a comma. ",
            "Three.",
        ]);
        assert_eq!(
            out,
            vec![
                "Hello there, friend.",
                "This is sentence two, which has a comma.",
                "Three."
            ]
        );
    }

    #[test]
    fn code_fences_are_omitted_across_deltas() {
        let mut f = CodeFenceFilter::new("omit");
        let mut out = String::new();
        for d in [
            "Here you go: ``",
            "`python\nprint('x')\n",
            "``",
            "` That runs it.",
        ] {
            out.push_str(&f.push(d));
        }
        out.push_str(&f.flush());
        assert_eq!(out, "Here you go:  That runs it.");
        assert!(f.omitted_any());
    }

    #[test]
    fn code_fences_can_be_read() {
        let mut f = CodeFenceFilter::new("read");
        let mut out = f.push("Run ```bash\nls -la\n``` now.");
        out.push_str(&f.flush());
        assert_eq!(out, "Run ls -la\n now.");
    }

    #[test]
    fn an_unterminated_fence_never_leaks() {
        let mut f = CodeFenceFilter::new("omit");
        let mut out = f.push("Look: ```js\nconst secret = 1;");
        out.push_str(&f.flush());
        assert_eq!(out, "Look: ");
    }

    fn defaults() -> AgentTextTransforms {
        AgentTextTransforms::default()
    }

    #[test]
    fn urls_are_spoken_as_their_domain() {
        let out = transform_for_speech(
            "See https://www.docs.bud.studio/agents?x=1 for more.",
            &defaults(),
        );
        assert_eq!(out, "See docs.bud.studio for more.");
    }

    #[test]
    fn urls_can_be_omitted_or_kept() {
        let mut t = defaults();
        t.urls = "omit".into();
        assert_eq!(transform_for_speech("Go to https://a.com/x.", &t), "Go to.");
        t.urls = "full".into();
        t.strip_markdown = false;
        assert_eq!(
            transform_for_speech("Go to https://a.com/x.", &t),
            "Go to https://a.com/x."
        );
    }

    #[test]
    fn markdown_and_emoji_are_removed() {
        let out = transform_for_speech("**Great** news 🎉 — it *works* ✅!", &defaults());
        assert_eq!(out, "Great news — it works!");
    }

    #[test]
    fn transforms_can_be_switched_off() {
        let t = AgentTextTransforms {
            strip_markdown: false,
            strip_emoji: false,
            code_blocks: "read".into(),
            urls: "full".into(),
        };
        assert_eq!(transform_for_speech("**hi** 🎉", &t), "**hi** 🎉");
    }

    fn extract(field: &str, deltas: &[&str]) -> (String, SpeakFieldExtractor) {
        let mut x = SpeakFieldExtractor::new(field);
        let mut spoken = String::new();
        for d in deltas {
            spoken.push_str(&x.push(d));
        }
        (spoken, x)
    }

    #[test]
    fn the_spoken_field_streams_as_the_json_arrives() {
        let (spoken, x) = extract(
            "answer",
            &[
                r#"{"score": 0.9, "tags": ["a", "b"], "nested": {"answer": "no"}, "ans"#,
                r#"wer": "Your order "#,
                r#"has shipped.", "id": 7}"#,
            ],
        );
        assert_eq!(spoken, "Your order has shipped.");
        assert!(x.found());
        assert_eq!(x.output().unwrap()["id"], 7);
    }

    #[test]
    fn escapes_and_unicode_are_decoded() {
        let (spoken, _) = extract(
            "t",
            &[r#"{"t": "He said \"hi\"\nthen \u00e9 \ud83d\ude00 \\o/"}"#],
        );
        assert_eq!(spoken, "He said \"hi\"\nthen é 😀 \\o/");
    }

    #[test]
    fn a_missing_field_speaks_nothing() {
        let (spoken, x) = extract("answer", &[r#"{"other": "text", "n": 1}"#]);
        assert_eq!(spoken, "");
        assert!(!x.found());
    }

    #[test]
    fn a_non_string_field_speaks_nothing() {
        let (spoken, x) = extract("answer", &[r#"{"answer": {"text": "x"}, "b": "y"}"#]);
        assert_eq!(spoken, "");
        assert!(!x.found());
    }

    #[test]
    fn a_fenced_json_reply_still_parses_as_output() {
        let (spoken, x) = extract("a", &["```json\n{\"a\": \"hi\"}\n```"]);
        assert_eq!(spoken, "hi");
        assert_eq!(x.output().unwrap()["a"], "hi");
    }
}

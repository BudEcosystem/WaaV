//! The single sentence aggregator for all LLM→TTS text chunking.
//!
//! Both streaming paths — the conversation pump ([`crate::core::conversation`])
//! and the DAG LLM node ([`crate::dag::nodes::llm`]) — feed LLM token deltas
//! through one [`SentenceAggregator`] so sentence-boundary semantics never
//! diverge again (PIPECAT_FIX_PLAN C-G1+2).
//!
//! Algorithm (Pipecat `SimpleTextAggregator` parity, adapted):
//! - Characters accumulate into an internal buffer (**push-then-check**: the
//!   just-arrived char is in the buffer before any boundary decision).
//! - An **unambiguous** terminator (newline, Devanagari danda, CJK stops,
//!   Arabic, …) flushes the whole buffer immediately — these scripts never
//!   use the character mid-token, so no lookahead is needed.
//! - An **ambiguous Latin** terminator (`.` `!` `?` `…`) is *held* until the
//!   first non-whitespace lookahead char arrives, then confirmed via
//!   [`is_real_boundary`]: a `.` between digits is a decimal (`3.14`), and a
//!   `.` ending a curated abbreviation (`Mr.`, `e.g.`) or a single Latin
//!   initial (`A.`) is not a sentence end. Failure mode is deliberately
//!   conservative: we may split *late* (one extra sentence buffered), never
//!   *wrong* (mid-number/abbreviation flush to TTS).
//! - A **max-hold cap** (default 160 chars — a WaaV robustness advantage
//!   Pipecat lacks) force-emits unpunctuated run-ons so TTS latency stays
//!   bounded. The char count is maintained incrementally (O(1) per char).
//! - Runs of terminators (`!?`, `...`) extend the held boundary so they emit
//!   as one unit instead of fragmenting.
//!
//! Emitted sentences are trimmed of *spaces only* (Pipecat `strip(" ")`
//! parity) so meaningful trailing newlines survive. Whitespace-only segments
//! are dropped.

/// Ambiguous Latin terminators — may be an abbreviation/decimal/ellipsis run,
/// so the boundary decision is deferred one non-whitespace char (lookahead).
/// NOTE: `;` is deliberately absent (it would fragment ordinary clauses — a
/// behavior change neither legacy splitter had); `…` is here (not in the
/// unambiguous set) for Pipecat parity.
const LATIN_TERMINATORS: &[char] = &['.', '!', '?', '…'];

/// Unambiguous terminators — never appear mid-token in their scripts, so they
/// flush immediately with no lookahead (Pipecat's UNAMBIGUOUS set).
const UNAMBIGUOUS_TERMINATORS: &[char] = &[
    '\n', // newline: an explicit break the model chose to emit
    '।', '॥', // Devanagari danda / double danda
    '。', '？', '！', '；', '．', '｡', // CJK fullwidth stops
    '؟', '؛', '۔', // Arabic question/semicolon, Urdu full stop
    '។', '៕', // Khmer
    '။', '၊', // Myanmar
    '።', '፧', // Ethiopic
    '։', // Armenian
];

/// Curated abbreviations (WITHOUT the final dot) whose trailing `.` does not
/// end a sentence. Conservative failure: a sentence genuinely ending in one of
/// these splits one sentence late (bounded by the cap).
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "a.m", "p.m",
    "u.s", "u.k", "ph.d", "approx", "dept",
];

/// Abbreviations that are real words when sentence-final ("No." is one of the
/// most common voice replies) — treated as abbreviations ONLY when the next
/// non-whitespace char is a digit ("No. 5", "Fig. 3"), otherwise they end the
/// sentence normally.
const NUMERIC_ABBREVIATIONS: &[&str] = &["no", "fig"];

/// Trailing closers absorbed into a held boundary (`He said "Stop." Then…`
/// keeps the closing quote with its sentence — Pipecat's end-of-sentence
/// pattern absorbs `)]"'”’` after the punctuation).
const TRAILING_CLOSERS: &[char] = &['"', '\'', ')', ']', '”', '’', '»', '】', '）'];

/// Configuration for [`SentenceAggregator`].
#[derive(Debug, Clone)]
pub struct SentenceConfig {
    /// Force-emit when the buffer reaches this many chars with no boundary.
    /// Bounds TTS latency on unpunctuated/list-y LLM output. `0` disables.
    pub max_hold_chars: usize,
    /// Apply lookahead + decimal/abbreviation disambiguation to the Latin
    /// terminator set. `false` flushes Latin terminators immediately (the
    /// legacy behavior, kept for A/B comparison).
    pub disambiguate_latin: bool,
}

impl Default for SentenceConfig {
    fn default() -> Self {
        Self { max_hold_chars: 160, disambiguate_latin: true }
    }
}

/// Streaming sentence-boundary aggregator. Feed token deltas with
/// [`push_str`](Self::push_str); each call returns the sentences completed by
/// that delta (usually zero or one — the returned `Vec` does not allocate
/// when empty). Call [`flush`](Self::flush) at stream end and
/// [`reset`](Self::reset) on barge-in.
#[derive(Debug)]
pub struct SentenceAggregator {
    buf: String,
    /// Byte offset just past a held Latin terminator awaiting a
    /// non-whitespace lookahead char. Always a char boundary.
    pending: Option<usize>,
    /// Incrementally maintained `buf.chars().count()` (the cap check must not
    /// recount per char — that would be O(n²) per turn).
    char_count: usize,
    cfg: SentenceConfig,
}

impl Default for SentenceAggregator {
    fn default() -> Self {
        Self::new(SentenceConfig::default())
    }
}

impl SentenceAggregator {
    pub fn new(cfg: SentenceConfig) -> Self {
        Self { buf: String::with_capacity(256), pending: None, char_count: 0, cfg }
    }

    /// Feed a token delta; returns 0+ completed sentences (space-trimmed).
    pub fn push_str(&mut self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for ch in text.chars() {
            // Push-then-check: the char is in the buffer before any decision
            // (the lookahead guards must be able to see it).
            self.buf.push(ch);
            self.char_count += 1;

            // 1) A held Latin boundary resolves on the first non-whitespace
            //    lookahead char — unless that char extends the terminator run
            //    ("!?", "...") or is a trailing closer (`."`, `.)`), in which
            //    case the boundary grows to absorb it.
            if let Some(term_end) = self.pending {
                if ch.is_whitespace() {
                    // Whitespace is not meaningful lookahead; keep waiting.
                    self.maybe_cap(&mut out);
                    continue;
                }
                if LATIN_TERMINATORS.contains(&ch) || UNAMBIGUOUS_TERMINATORS.contains(&ch) {
                    // Terminator run: move the held boundary past this char.
                    self.pending = Some(self.buf.len());
                    self.maybe_cap(&mut out);
                    continue;
                }
                // Closers directly after the terminator (no intervening
                // whitespace) belong to the sentence: `He said "Stop." Then`
                // must emit `He said "Stop."`, not orphan the quote.
                if TRAILING_CLOSERS.contains(&ch) && self.buf.len() - ch.len_utf8() == term_end {
                    self.pending = Some(self.buf.len());
                    self.maybe_cap(&mut out);
                    continue;
                }
                self.pending = None;
                if self.is_real_boundary(term_end) {
                    self.emit_to(term_end, &mut out);
                }
                // Fall through: `ch` (now possibly at the head of the
                // remainder) is an ordinary char — no terminator check needed
                // since terminator chars were handled above.
                self.maybe_cap(&mut out);
                continue;
            }

            // 2) Fresh terminator?
            if UNAMBIGUOUS_TERMINATORS.contains(&ch) {
                let end = self.buf.len();
                self.emit_to(end, &mut out);
            } else if LATIN_TERMINATORS.contains(&ch) {
                if self.cfg.disambiguate_latin {
                    self.pending = Some(self.buf.len());
                } else {
                    let end = self.buf.len();
                    self.emit_to(end, &mut out);
                }
            }

            // 3) Safety cap (bounds latency on unpunctuated run-ons).
            self.maybe_cap(&mut out);
        }
        out
    }

    /// Stream end: emit any held/buffered text (resolves an unconfirmed
    /// pending boundary — `"Done."` with no following token must still speak).
    pub fn flush(&mut self) -> Option<String> {
        self.pending = None;
        self.char_count = 0;
        let s = std::mem::take(&mut self.buf);
        if s.trim().is_empty() {
            return None;
        }
        let t = s.trim_matches(' ');
        // Avoid the copy when there was nothing to trim (the common case).
        if t.len() == s.len() { Some(s) } else { Some(t.to_string()) }
    }

    /// Discard buffered text (barge-in / interruption).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.pending = None;
        self.char_count = 0;
    }

    /// Chars currently buffered (for tests/diagnostics).
    pub fn buffered_chars(&self) -> usize {
        self.char_count
    }

    // --- internals ---

    /// Emit `buf[..end]` (space-trimmed; dropped if whitespace-only) and keep
    /// the remainder — the lookahead char stays at the head of the new buffer.
    fn emit_to(&mut self, end: usize, out: &mut Vec<String>) {
        debug_assert!(self.buf.is_char_boundary(end));
        {
            // Trim on the slice BEFORE copying: exactly one allocation per
            // emitted sentence (this is the per-sentence hot path).
            let trimmed = self.buf[..end].trim_matches(' ');
            if !trimmed.trim().is_empty() {
                out.push(trimmed.to_string());
            }
        }
        self.buf.drain(..end);
        // Remainder = whitespace run + lookahead char in the common case, but
        // bounded only by max_hold_chars in the worst (the recount below — and
        // the word scans in is_real_boundary — scale with the cap; raise the
        // cap knowingly).
        self.char_count = self.buf.chars().count();
    }

    fn maybe_cap(&mut self, out: &mut Vec<String>) {
        if self.cfg.max_hold_chars > 0 && self.char_count >= self.cfg.max_hold_chars {
            self.pending = None;
            // Back off to the last whitespace so the forced split never lands
            // mid-word / mid-number / between a base char and its combining
            // mark. A whitespace-free buffer (one giant token) hard-cuts —
            // unavoidable.
            let end = match self.buf.rfind(|c: char| c.is_whitespace()) {
                Some(idx) if idx > 0 => idx + self.buf[idx..].chars().next().map_or(1, char::len_utf8),
                _ => self.buf.len(),
            };
            self.emit_to(end, out);
            // If the backoff emitted only whitespace/nothing (e.g. ws at the
            // very head), hard-cut the rest to guarantee forward progress.
            if self.char_count >= self.cfg.max_hold_chars {
                let end = self.buf.len();
                self.emit_to(end, out);
            }
        }
    }

    /// Decide whether the Latin terminator ending at byte `term_end` is a true
    /// sentence end. Only `.` carries ambiguity (token-internal dots,
    /// abbreviations, initials, ordinals); `!` `?` `…` confirm
    /// unconditionally.
    fn is_real_boundary(&self, term_end: usize) -> bool {
        let term_char = match self.buf[..term_end].chars().next_back() {
            Some(c) => c,
            None => return false,
        };
        if term_char != '.' {
            return true;
        }
        let before_term = term_end - term_char.len_utf8();

        // Tight-bond rule: a dot DIRECTLY followed by an alphanumeric char
        // (no intervening whitespace) binds tokens together — decimals
        // ("3.14", Devanagari "३.५" — `is_alphanumeric` covers all scripts),
        // domains/files ("example.com", "main.rs"), versions ("v1.x"), and
        // dotted abbreviations mid-token ("Ph.D"). Never a sentence end.
        // A real sentence boundary always has whitespace after the dot.
        let next_char = self.buf[term_end..].chars().next();
        if matches!(next_char, Some(c) if !c.is_whitespace() && c.is_alphanumeric()) {
            return false;
        }

        // Word-based guards: the last whitespace-delimited word before the
        // dot (stripped of leading punctuation like `(` or `"`).
        let word_region = &self.buf[..before_term];
        let last_word = word_region.rsplit(char::is_whitespace).next().unwrap_or("");
        let lw = last_word.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '.');
        if !lw.is_empty() {
            // Ordinal guard: an all-digit word that OPENS the buffer is a
            // list ordinal ("1. First item") — keep it with its item instead
            // of speaking "One." as a standalone utterance. Mid-sentence
            // numbers ("costs 42.") still end the sentence normally.
            let all_digits = lw.chars().all(|c| c.is_ascii_digit());
            if all_digits && word_region.trim() == lw {
                return false;
            }
            // Curated abbreviations (zero-alloc, case-insensitive).
            if ABBREVIATIONS.iter().any(|a| a.eq_ignore_ascii_case(lw)) {
                return false;
            }
            // "No."/"Fig." are real sentence enders unless followed by a
            // number ("No. 5", "Fig. 3").
            let next_non_ws = self.buf[term_end..].chars().find(|c| !c.is_whitespace());
            if NUMERIC_ABBREVIATIONS.iter().any(|a| a.eq_ignore_ascii_case(lw))
                && matches!(next_non_ws, Some(d) if d.is_numeric())
            {
                return false;
            }
            // Single Latin initial ("John A. Smith"). Known trade-off: also
            // holds the pronoun "I." one sentence late — conservative, bounded
            // by the cap/flush, never a wrong split.
            let mut chars = lw.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_alphabetic() {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed the whole string in one delta; collect emitted sentences
    /// (flush NOT applied — held text stays held).
    fn feed(s: &str) -> Vec<String> {
        SentenceAggregator::default().push_str(s)
    }

    /// Feed char-by-char (the real LLM-token granularity worst case).
    fn feed_chars(s: &str) -> Vec<String> {
        let mut agg = SentenceAggregator::default();
        let mut out = Vec::new();
        for ch in s.chars() {
            out.extend(agg.push_str(&ch.to_string()));
        }
        out
    }

    fn strip_ws(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    // --- lookahead defers the boundary decision (the P0 core) ---

    #[test]
    fn boundary_held_without_lookahead() {
        assert_eq!(feed("The price is $29."), Vec::<String>::new());
    }

    #[test]
    fn lookahead_confirms_boundary() {
        assert_eq!(feed("The price is $29. Next"), vec!["The price is $29."]);
    }

    // --- decimal guard ---

    #[test]
    fn decimal_not_split() {
        // '.' between digits is a decimal; only the real boundary splits.
        assert_eq!(feed("3.14 is pi. Yes"), vec!["3.14 is pi."]);
    }

    #[test]
    fn decimal_not_split_char_streamed() {
        // The bug class the v2 critique pinned: at char granularity the
        // lookahead digit must be visible to the guard (push-then-check).
        assert_eq!(feed_chars("3.14 is pi. Yes"), vec!["3.14 is pi."]);
    }

    #[test]
    fn thousands_and_decimal_not_split() {
        assert_eq!(feed("Cost 1,234.5 ok. Done"), vec!["Cost 1,234.5 ok."]);
    }

    // --- abbreviation guard ---

    #[test]
    fn abbreviation_not_split() {
        assert_eq!(feed("Call Mr. Smith now. Bye"), vec!["Call Mr. Smith now."]);
    }

    #[test]
    fn eg_not_split() {
        assert_eq!(feed("See e.g. this one. End"), vec!["See e.g. this one."]);
    }

    #[test]
    fn single_initial_not_split() {
        assert_eq!(feed("Ask Dr. A. now. Ok"), vec!["Ask Dr. A. now."]);
    }

    #[test]
    fn abbreviation_char_streamed() {
        assert_eq!(feed_chars("Call Mr. Smith now. Bye"), vec!["Call Mr. Smith now."]);
    }

    // --- multilingual unambiguous set: immediate flush, no lookahead ---

    #[test]
    fn devanagari_danda_immediate() {
        assert_eq!(feed("नमस्ते।"), vec!["नमस्ते।"]);
    }

    #[test]
    fn cjk_full_stops_immediate() {
        assert_eq!(feed("你好。世界。"), vec!["你好。", "世界。"]);
    }

    #[test]
    fn arabic_question_mark_immediate() {
        assert_eq!(feed("مرحبا؟"), vec!["مرحبا؟"]);
    }

    #[test]
    fn newline_immediate() {
        assert_eq!(feed("Line one\nLine two\n"), vec!["Line one\n", "Line two\n"]);
    }

    // --- ambiguous Latin terminators with lookahead ---

    #[test]
    fn bang_and_question_split_on_lookahead() {
        assert_eq!(feed("Hi! How are you? Good"), vec!["Hi!", "How are you?"]);
    }

    #[test]
    fn ellipsis_char_is_latin_held() {
        // '…' is ambiguous (trailing-off speech) → held, emitted on lookahead.
        assert_eq!(feed("Wait… ok"), vec!["Wait…"]);
        assert_eq!(feed("Wait…"), Vec::<String>::new()); // held until flush
    }

    #[test]
    fn terminator_run_emits_as_one_unit() {
        // "!?" must not fragment into "Hi!" + "?".
        assert_eq!(feed("Hi!? Good"), vec!["Hi!?"]);
        assert_eq!(feed("Well... ok"), vec!["Well..."]);
    }

    #[test]
    fn semicolon_is_not_a_boundary() {
        // v2 decision: `;` would fragment ordinary clauses — not a terminator.
        assert_eq!(feed("first part; second part. End"), vec!["first part; second part."]);
    }

    #[test]
    fn single_letter_sentence_end_held_late_by_design() {
        // Documented trade-off: a sentence ending in a single capital letter
        // ("option B.") is indistinguishable from an initial ("John B. Smith"),
        // so it is held — emitted late (with the next boundary or at flush),
        // never split wrong.
        let mut agg = SentenceAggregator::default();
        let out = agg.push_str("Pick option B. Then go. Now");
        // "B." rejected as boundary → the first emit happens at "go." instead.
        assert_eq!(out, vec!["Pick option B. Then go."]);
        assert_eq!(agg.flush().as_deref(), Some("Now"));
    }

    // --- safety cap (WaaV advantage; Pipecat lacks it) ---

    #[test]
    fn cap_force_emits_unpunctuated_runon() {
        let long = "x".repeat(200);
        let out = feed(&long);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chars().count(), 160);
    }

    #[test]
    fn cap_disabled_when_zero() {
        let mut agg = SentenceAggregator::new(SentenceConfig {
            max_hold_chars: 0,
            disambiguate_latin: true,
        });
        assert!(agg.push_str(&"y".repeat(500)).is_empty());
        assert_eq!(agg.flush().unwrap().chars().count(), 500);
    }

    #[test]
    fn cap_counts_chars_not_bytes() {
        // 200 multibyte chars must trip the 160-CHAR cap exactly once.
        let long = "ध".repeat(200);
        let out = feed(&long);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chars().count(), 160);
    }

    // --- flush & reset ---

    #[test]
    fn flush_emits_held_boundary_at_stream_end() {
        let mut agg = SentenceAggregator::default();
        assert!(agg.push_str("All done.").is_empty()); // held for lookahead
        assert_eq!(agg.flush().as_deref(), Some("All done."));
        assert_eq!(agg.flush(), None); // idempotent
    }

    #[test]
    fn flush_trims_spaces_only() {
        let mut agg = SentenceAggregator::default();
        agg.push_str("Hello. ");
        assert_eq!(agg.push_str("World"), vec!["Hello."]);
        assert_eq!(agg.flush().as_deref(), Some("World"));
    }

    #[test]
    fn reset_discards_buffer() {
        let mut agg = SentenceAggregator::default();
        agg.push_str("half a sent");
        agg.reset();
        assert_eq!(agg.flush(), None);
        assert_eq!(agg.buffered_chars(), 0);
    }

    #[test]
    fn whitespace_only_segment_dropped() {
        assert_eq!(feed("   \n"), Vec::<String>::new());
    }

    // --- remainder retention (no drop / no dup across the boundary) ---

    #[test]
    fn remainder_keeps_lookahead_char() {
        let mut agg = SentenceAggregator::default();
        let out = agg.push_str("Mr. X");
        assert!(out.is_empty()); // "Mr." rejected as boundary, all retained
        let out2 = agg.push_str(" did. Yes");
        assert_eq!(out2, vec!["Mr. X did."]);
        assert_eq!(agg.flush().as_deref(), Some("Yes"));
    }

    #[test]
    fn split_across_deltas() {
        let mut agg = SentenceAggregator::default();
        assert!(agg.push_str("The price is $29").is_empty());
        assert!(agg.push_str(". ").is_empty()); // boundary held (ws lookahead)
        assert_eq!(agg.push_str("Next"), vec!["The price is $29."]);
        assert_eq!(agg.flush().as_deref(), Some("Next"));
    }

    // --- no-drop/no-dup property: deterministic seeded fuzz ---

    #[test]
    fn fuzz_no_drop_no_dup() {
        // xorshift64* — deterministic, no external dep.
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut rng = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state = state.wrapping_mul(0x2545F4914F6CDD1D);
            state
        };
        let alphabet: Vec<char> = "abc XY12.!?…।。\n,;$ नম你…"
            .chars()
            .collect();
        for _case in 0..200 {
            // Build a random input and split it into random-sized deltas.
            let len = (rng() % 120) as usize + 1;
            let input: String =
                (0..len).map(|_| alphabet[(rng() % alphabet.len() as u64) as usize]).collect();
            let mut agg = SentenceAggregator::default();
            let mut got = String::new();
            let mut idx = 0;
            let chars: Vec<char> = input.chars().collect();
            while idx < chars.len() {
                let take = ((rng() % 7) + 1) as usize;
                let delta: String = chars[idx..(idx + take).min(chars.len())].iter().collect();
                idx += take;
                for s in agg.push_str(&delta) {
                    got.push_str(&s);
                    got.push(' ');
                }
            }
            if let Some(t) = agg.flush() {
                got.push_str(&t);
            }
            assert_eq!(
                strip_ws(&got),
                strip_ws(&input),
                "drop/dup detected for input {input:?}"
            );
        }
    }

    // --- brutal-review regression pins (workflow wf_0cc69d62) ---

    #[test]
    fn domains_and_files_never_split() {
        // Tight-bond: dot directly followed by alphanumeric binds the token.
        assert_eq!(
            feed("Visit docs.python.org. Anything else? Ok"),
            vec!["Visit docs.python.org.", "Anything else?"]
        );
        assert_eq!(feed_chars("Open main.rs now. Done"), vec!["Open main.rs now."]);
        assert_eq!(feed("Use Node.js here. Ok"), vec!["Use Node.js here."]);
    }

    #[test]
    fn phd_dotted_abbrev_not_split() {
        assert_eq!(feed("Get your Ph.D. now. End"), vec!["Get your Ph.D. now."]);
    }

    #[test]
    fn no_as_sentence_end_splits() {
        // "No." is one of the most common voice replies — it must NOT be
        // swallowed by the abbreviation list.
        assert_eq!(feed("No. The store closes at nine. End"), vec!["No.", "The store closes at nine."]);
    }

    #[test]
    fn no_with_number_held_as_abbreviation() {
        assert_eq!(feed("See No. 5 here. End"), vec!["See No. 5 here."]);
        assert_eq!(feed("Check Fig. 3 above. End"), vec!["Check Fig. 3 above."]);
    }

    #[test]
    fn strict_decimal_adjacency_splits_spaced_digits() {
        // digit-dot-SPACE-digit is two sentences, not a decimal.
        assert_eq!(feed("It costs 42. 7 are left. End"), vec!["It costs 42.", "7 are left."]);
    }

    #[test]
    fn devanagari_decimal_not_split() {
        // Hindi uses '.' as the decimal separator with Devanagari digits;
        // is_alphanumeric covers all scripts (the EN→Hindi DAG flagship path).
        assert_eq!(feed("मूल्य ३.५ है। ठीक"), vec!["मूल्य ३.५ है।"]);
    }

    #[test]
    fn ordinal_list_kept_with_item() {
        // "1." opening the buffer is a list ordinal, not a standalone "One."
        assert_eq!(
            feed("1. First item\n2. Second item\n"),
            vec!["1. First item\n", "2. Second item\n"]
        );
        // Mid-sentence numbers still end sentences normally.
        assert_eq!(feed("The total is 42. Next topic"), vec!["The total is 42."]);
    }

    #[test]
    fn trailing_closer_absorbed_into_sentence() {
        assert_eq!(
            feed("He said \"Stop.\" Then left. Bye"),
            vec!["He said \"Stop.\"", "Then left."]
        );
        assert_eq!(feed("It works (mostly.) Right? Ok"), vec!["It works (mostly.)", "Right?"]);
    }

    #[test]
    fn opening_quote_of_next_sentence_not_absorbed() {
        // A quote AFTER whitespace opens the next sentence — it must not be
        // pulled into the previous one.
        assert_eq!(
            feed("She agreed. \"Fine,\" she said. End"),
            vec!["She agreed.", "\"Fine,\" she said."]
        );
    }

    #[test]
    fn cap_backs_off_to_whitespace() {
        // The forced cap split must not land mid-word/mid-number.
        let input = format!("{} 3.14159", "x".repeat(156));
        let mut agg = SentenceAggregator::default();
        let out = agg.push_str(&input);
        assert_eq!(out, vec!["x".repeat(156)]);
        assert_eq!(agg.flush().as_deref(), Some("3.14159"), "number survives the cap intact");
    }

    // --- legacy mode (disambiguate_latin = false) ---

    #[test]
    fn legacy_mode_flushes_latin_immediately() {
        let mut agg = SentenceAggregator::new(SentenceConfig {
            max_hold_chars: 160,
            disambiguate_latin: false,
        });
        assert_eq!(agg.push_str("Hello."), vec!["Hello."]);
    }

    // --- integration-shaped stream from the plan ---

    #[test]
    fn plan_integration_stream() {
        let mut agg = SentenceAggregator::default();
        let mut out = Vec::new();
        for tok in ["Pi is 3", ".", "14. Mr", ". Tau is bigger", ". ", "नमस्ते। Done"] {
            out.extend(agg.push_str(tok));
        }
        out.extend(agg.flush());
        assert_eq!(
            out,
            vec!["Pi is 3.14.", "Mr. Tau is bigger.", "नमस्ते।", "Done"]
        );
    }
}

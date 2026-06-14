//! Strip reasoning-model chain-of-thought (`<think>…</think>`) from a reply
//! before it is spoken.
//!
//! Reasoning models (DeepSeek-R1, QwQ, and other `<think>`-emitting families,
//! including via the ollama / OpenAI-compatible wire) interleave their hidden
//! reasoning as `<think>…</think>` blocks INLINE in the assistant content. On a
//! voice call that chain-of-thought must never reach TTS — the caller would hear
//! the model talking to itself ("okay, the user is asking… let me reconsider…")
//! instead of the answer, which destroys both the experience and the perceived
//! intelligence of the system.
//!
//! [`ThinkStripper`] is a STATEFUL streaming filter: it suppresses every token
//! between `<think>` and `</think>`, correctly handling tags split across token
//! deltas (e.g. `"<thi"` in one delta, `"nk>"` in the next). [`strip_think`] is
//! the whole-text convenience for non-streaming paths.

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

/// Stateful `<think>…</think>` filter for a streamed reply. Feed each token
/// delta to [`push`](Self::push) and speak/aggregate only what it returns; call
/// [`flush`](Self::flush) once at end-of-stream.
#[derive(Default)]
pub struct ThinkStripper {
    in_think: bool,
    /// A trailing fragment of the current chunk that could be the START of a tag
    /// split across deltas — held back so a partial `<think>`/`</think>` is never
    /// emitted (nor mistaken for plain text).
    pending: String,
}

/// The longest suffix of `s` that is a (proper or full-but-we-handle-full-
/// elsewhere) PREFIX of `tag` — i.e. how much trailing text to hold back because
/// it might be the start of `tag` continued in the next delta.
fn partial_tag_tail_len(s: &str, tag: &str) -> usize {
    // Try the longest possible partial first (tag.len()-1 down to 1).
    let max = (tag.len() - 1).min(s.len());
    for n in (1..=max).rev() {
        let suffix = &s[s.len() - n..];
        if tag.as_bytes().starts_with(suffix.as_bytes()) {
            return n;
        }
    }
    0
}

impl ThinkStripper {
    /// Filter one delta, returning only the visible (non-`<think>`) portion.
    pub fn push(&mut self, delta: &str) -> String {
        let mut buf = std::mem::take(&mut self.pending);
        buf.push_str(delta);
        let mut out = String::new();
        loop {
            if self.in_think {
                match buf.find(CLOSE) {
                    Some(pos) => {
                        buf.drain(..pos + CLOSE.len());
                        self.in_think = false;
                    }
                    None => {
                        // Still inside the block — emit nothing, but hold back a
                        // tail that might be a partial `</think>`.
                        let keep = partial_tag_tail_len(&buf, CLOSE);
                        self.pending = buf[buf.len() - keep..].to_string();
                        return out;
                    }
                }
            } else {
                match buf.find(OPEN) {
                    Some(pos) => {
                        out.push_str(&buf[..pos]);
                        buf.drain(..pos + OPEN.len());
                        self.in_think = true;
                    }
                    None => {
                        // No open tag — emit everything except a tail that might
                        // be a partial `<think>`.
                        let keep = partial_tag_tail_len(&buf, OPEN);
                        out.push_str(&buf[..buf.len() - keep]);
                        self.pending = buf[buf.len() - keep..].to_string();
                        return out;
                    }
                }
            }
        }
    }

    /// End of stream: emit any held-back non-think tail (a partial tag that never
    /// completed is just literal text); content still inside an unclosed
    /// `<think>` is dropped (an unterminated block is never spoken).
    pub fn flush(&mut self) -> String {
        if self.in_think {
            self.pending.clear();
            String::new()
        } else {
            std::mem::take(&mut self.pending)
        }
    }
}

/// Strip every `<think>…</think>` block from a complete reply (non-streaming /
/// fallback paths). Returns the input unchanged when there is no `<think>`.
pub fn strip_think(text: &str) -> String {
    if !text.contains(OPEN) {
        return text.to_string();
    }
    let mut s = ThinkStripper::default();
    let mut out = s.push(text);
    out.push_str(&s.flush());
    // Reasoning blocks are usually followed by a blank line; tidy leading
    // whitespace so the spoken answer doesn't start with an awkward pause.
    out.trim_start().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_whole_block() {
        assert_eq!(
            strip_think("<think>okay 15% of 60 is 9, let me check… yes</think>\n\nIt's nine dollars."),
            "It's nine dollars."
        );
        // No think block → unchanged.
        assert_eq!(strip_think("It's nine dollars."), "It's nine dollars.");
        // Unterminated block → dropped (never speak partial reasoning).
        assert_eq!(strip_think("<think>still thinking and then cut off"), "");
    }

    #[test]
    fn streaming_filter_handles_split_tags() {
        // Tags split arbitrarily across deltas must still be filtered, and only
        // the post-</think> answer emitted.
        let deltas = ["<thi", "nk>okay ", "the tip is ", "nine</thi", "nk>", " It is ", "nine ", "dollars."];
        let mut s = ThinkStripper::default();
        let mut spoken = String::new();
        for d in deltas {
            spoken.push_str(&s.push(d));
        }
        spoken.push_str(&s.flush());
        assert_eq!(spoken, " It is nine dollars.");
        // The reasoning text never leaked.
        assert!(!spoken.contains("okay"));
        assert!(!spoken.contains("tip"));
    }

    #[test]
    fn passthrough_when_no_think() {
        let deltas = ["The answer ", "is ", "nine dollars."];
        let mut s = ThinkStripper::default();
        let mut out = String::new();
        for d in deltas {
            out.push_str(&s.push(d));
        }
        out.push_str(&s.flush());
        assert_eq!(out, "The answer is nine dollars.");
    }

    #[test]
    fn partial_tag_tail() {
        assert_eq!(partial_tag_tail_len("abc<thi", OPEN), 4); // "<thi"
        assert_eq!(partial_tag_tail_len("abc<", OPEN), 1); // "<"
        assert_eq!(partial_tag_tail_len("abcdef", OPEN), 0);
        assert_eq!(partial_tag_tail_len("done</thin", CLOSE), 6); // "</thin"
    }
}

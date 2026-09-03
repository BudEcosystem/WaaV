//! Space-aware text concatenation (PIPECAT_FIX_PLAN C-G3).
//!
//! Reassembling text from streams with DIFFERENT spacing conventions is
//! quietly lossy: STT providers emit fragments WITHOUT inter-part spaces
//! ("hello" + "world"), LLM deltas arrive WITH them ("hello" + " world").
//! Naive `push_str` yields "helloworld" for the former; naive
//! space-joining yields "hello  world" for the latter. Direct port of
//! Pipecat's `concatenate_aggregated_text`:
//!
//! - within a run of `includes_spaces = true` parts: concat directly;
//! - within a run of `includes_spaces = false` parts: insert one space;
//! - at a TRANSITION between conventions: insert a space only when neither
//!   boundary character is already whitespace;
//! - empty parts are skipped entirely (they carry no convention).

/// One piece of text plus its spacing convention.
#[derive(Debug, Clone, PartialEq)]
pub struct TextPart {
    pub text: String,
    /// `true`: the part embeds its own spacing (LLM deltas).
    /// `false`: bare token — a joiner space is needed (STT fragments).
    pub includes_spaces: bool,
}

impl TextPart {
    pub fn with_spaces(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            includes_spaces: true,
        }
    }
    pub fn without_spaces(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            includes_spaces: false,
        }
    }
}

/// Concatenate parts under their spacing conventions. See module docs.
pub fn concatenate(parts: &[TextPart]) -> String {
    let mut out = String::new();
    let mut prev_includes: Option<bool> = None;
    for part in parts {
        if part.text.is_empty() {
            continue; // no convention, no content
        }
        match prev_includes {
            None => {}
            Some(prev) => {
                let boundary_ws = out.chars().next_back().is_some_and(char::is_whitespace)
                    || part.text.chars().next().is_some_and(char::is_whitespace);
                let need_space = if prev && part.includes_spaces {
                    // spaces-included run: the parts carry their own.
                    false
                } else if !prev && !part.includes_spaces {
                    // bare-token run: always join with one space.
                    !boundary_ws
                } else {
                    // convention TRANSITION: space only if neither boundary
                    // char already is one.
                    !boundary_ws
                };
                if need_space {
                    out.push(' ');
                }
            }
        }
        out.push_str(&part.text);
        prev_includes = Some(part.includes_spaces);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(t: &str) -> TextPart {
        TextPart::with_spaces(t)
    }
    fn bare(t: &str) -> TextPart {
        TextPart::without_spaces(t)
    }

    #[test]
    fn bare_tokens_get_joiner_spaces() {
        assert_eq!(concatenate(&[bare("hello"), bare("world")]), "hello world");
        assert_eq!(
            concatenate(&[bare("one"), bare("two"), bare("three")]),
            "one two three"
        );
    }

    #[test]
    fn spaced_parts_concat_directly() {
        assert_eq!(concatenate(&[ws("hello"), ws(" world")]), "hello world");
        // Never double a space the delta already carries.
        assert_eq!(concatenate(&[ws("hello "), ws("world")]), "hello world");
    }

    #[test]
    fn transitions_insert_space_only_when_needed() {
        // bare → spaced, no boundary whitespace: insert.
        assert_eq!(concatenate(&[bare("hello"), ws("world")]), "hello world");
        // bare → spaced with leading space on the delta: don't double.
        assert_eq!(concatenate(&[bare("hello"), ws(" world")]), "hello world");
        // spaced → bare with trailing space: don't double.
        assert_eq!(concatenate(&[ws("hello "), bare("world")]), "hello world");
        // spaced → bare, no whitespace: insert.
        assert_eq!(concatenate(&[ws("hello"), bare("world")]), "hello world");
    }

    #[test]
    fn empty_parts_are_skipped() {
        assert_eq!(
            concatenate(&[bare("hello"), bare(""), bare("world")]),
            "hello world",
            "an empty part must not contribute a joiner"
        );
        assert_eq!(concatenate(&[]), "");
        assert_eq!(concatenate(&[ws("")]), "");
    }
}

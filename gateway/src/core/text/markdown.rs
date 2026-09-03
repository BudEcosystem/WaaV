//! Markdown stripping for TTS (PIPECAT_FIX_PLAN C-G4).
//!
//! LLMs answer voice prompts in markdown; TTS reads "asterisk asterisk
//! bold asterisk asterisk" and full URLs aloud. This is the targeted-strip
//! port (NOT Pipecat's full MD→HTML pipeline): a handful of cached regexes
//! over a COMPLETE sentence (the aggregator's emitted unit — applying them
//! per token would tear `**` pairs across deltas), with a plain-text fast
//! path for the common no-markdown case.

use std::sync::OnceLock;

use regex::{NoExpand, Regex};

fn regex_for(
    pattern: &'static str,
    cell: &'static OnceLock<Result<Regex, regex::Error>>,
) -> Option<&'static Regex> {
    match cell.get_or_init(|| Regex::new(pattern)) {
        Ok(regex) => Some(regex),
        Err(err) => {
            tracing::warn!(
                pattern,
                error = %err,
                "markdown strip regex unavailable; leaving matching text unchanged"
            );
            None
        }
    }
}

fn replace_all(
    input: String,
    pattern: &'static str,
    cell: &'static OnceLock<Result<Regex, regex::Error>>,
    replacement: &str,
) -> String {
    match regex_for(pattern, cell) {
        Some(regex) => regex.replace_all(&input, replacement).into_owned(),
        None => input,
    }
}

fn remove_all(
    input: String,
    pattern: &'static str,
    cell: &'static OnceLock<Result<Regex, regex::Error>>,
) -> String {
    match regex_for(pattern, cell) {
        Some(regex) => regex.replace_all(&input, NoExpand("")).into_owned(),
        None => input,
    }
}

static BOLD: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
static WORD_STAR: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
static UNDERSCORE_EM: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
static CODE: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
static MD_LINK: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
static URL_SCHEME: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
static HEADING: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
static LIST_BULLET: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
static STRIKETHROUGH: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
static REPEAT_RUN: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();

/// Strip the markdown constructs that ruin spoken output. Returns the
/// input unchanged (no alloc beyond the clone) when no markdown chars are
/// present.
pub fn strip_markdown_for_tts(text: &str) -> String {
    // Fast path: nothing markdown-ish at all. (`+` covers `+` list bullets;
    // numbered-list bullets only strip when another marker is also present.)
    if !text
        .chars()
        .any(|c| matches!(c, '*' | '`' | '[' | '#' | '_' | ':' | '~' | '-' | '=' | '+'))
    {
        return text.to_string();
    }

    let mut out = text.to_string();

    // Markdown links FIRST ([label](url) → label) so the URL strip below
    // doesn't leave "[label]()".
    out = replace_all(out, r"\[([^\]]+)\]\([^)]*\)", &MD_LINK, "$1");
    // Bare URLs: drop the scheme + www (the host is speakable, the scheme
    // is noise).
    out = remove_all(out, r"https?://(www\.)?", &URL_SCHEME);
    // **bold** / __bold__ markers.
    out = replace_all(out, r"\*\*([^*]+)\*\*|__([^_]+)__", &BOLD, "$1$2");
    // Word-boundary single * (emphasis) — math like "2 * 3" survives
    // because that asterisk has whitespace on BOTH sides.
    out = replace_all(
        out,
        r"(^|\s)\*(\S[^*]*?\S|\S)\*([\s.,;:!?]|$)",
        &WORD_STAR,
        "$1$2$3",
    );
    // Word-boundary _emphasis_ (snake_case identifiers survive).
    out = replace_all(
        out,
        r"(^|\s)_(\S[^_]*?\S|\S)_([\s.,;:!?]|$)",
        &UNDERSCORE_EM,
        "$1$2$3",
    );
    // `inline code` markers.
    out = replace_all(out, r"`([^`]*)`", &CODE, "$1");
    // Leading heading markers.
    out = remove_all(out, r"(?m)^\s*#{1,6}\s+", &HEADING);
    // ~~strikethrough~~ markers (before the divider strip so `~~~~~` runs
    // aren't half-eaten).
    out = replace_all(out, r"~~([^~]+)~~", &STRIKETHROUGH, "$1");
    // Leading list bullets (review wf_d43814c3: "* Option one" / "- first" /
    // "+ x" / "1. step" was read aloud as "asterisk option one"). Strip the
    // marker, keep the text. (`(?m)^` anchors to line starts so a mid-word
    // `-`/`*` is untouched.)
    out = remove_all(out, r"(?m)^[ \t]*(?:[-*+]|\d{1,3}[.)])[ \t]+", &LIST_BULLET);
    // 5+ repeated punctuation (divider lines like ----- or =====). The
    // regex crate has no backreferences: one alternation per divider char.
    out = remove_all(out, r"-{5,}|={5,}|_{5,}|~{5,}|\*{5,}|#{5,}", &REPEAT_RUN);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_regexes_compile_without_static_expect() {
        assert!(regex_for(r"\[([^\]]+)\]\([^)]*\)", &MD_LINK).is_some());
        assert!(regex_for(r"https?://(www\.)?", &URL_SCHEME).is_some());
        assert!(regex_for(r"\*\*([^*]+)\*\*|__([^_]+)__", &BOLD).is_some());
        assert!(regex_for(r"(^|\s)\*(\S[^*]*?\S|\S)\*([\s.,;:!?]|$)", &WORD_STAR).is_some());
        assert!(regex_for(r"(^|\s)_(\S[^_]*?\S|\S)_([\s.,;:!?]|$)", &UNDERSCORE_EM).is_some());
        assert!(regex_for(r"`([^`]*)`", &CODE).is_some());
        assert!(regex_for(r"(?m)^\s*#{1,6}\s+", &HEADING).is_some());
        assert!(regex_for(r"~~([^~]+)~~", &STRIKETHROUGH).is_some());
        assert!(regex_for(r"(?m)^[ \t]*(?:[-*+]|\d{1,3}[.)])[ \t]+", &LIST_BULLET).is_some());
        assert!(regex_for(r"-{5,}|={5,}|_{5,}|~{5,}|\*{5,}|#{5,}", &REPEAT_RUN).is_some());
    }

    #[test]
    fn invalid_regex_helper_leaves_text_unchanged_without_panic() {
        static BAD: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();

        let got = replace_all("**bold**".to_string(), "(", &BAD, "$1");

        assert_eq!(got, "**bold**");
    }

    #[test]
    fn strips_bold_and_emphasis() {
        assert_eq!(strip_markdown_for_tts("**bold** text"), "bold text");
        assert_eq!(
            strip_markdown_for_tts("some *emphasis* here"),
            "some emphasis here"
        );
        assert_eq!(strip_markdown_for_tts("__also bold__ ok"), "also bold ok");
    }

    #[test]
    fn strips_code_and_links() {
        assert_eq!(strip_markdown_for_tts("use `code` here"), "use code here");
        assert_eq!(
            strip_markdown_for_tts("see [the docs](https://example.com/x) for more"),
            "see the docs for more"
        );
        assert_eq!(
            strip_markdown_for_tts("see https://example.com"),
            "see example.com"
        );
        assert_eq!(
            strip_markdown_for_tts("at http://www.foo.org/a"),
            "at foo.org/a"
        );
    }

    #[test]
    fn math_and_identifiers_survive() {
        assert_eq!(strip_markdown_for_tts("2 * 3 is 6"), "2 * 3 is 6");
        assert_eq!(
            strip_markdown_for_tts("call snake_case_fn now"),
            "call snake_case_fn now"
        );
    }

    #[test]
    fn headings_and_dividers_stripped() {
        assert_eq!(strip_markdown_for_tts("## Heading text"), "Heading text");
        assert_eq!(
            strip_markdown_for_tts("before ----- after"),
            "before  after"
        );
    }

    #[test]
    fn list_bullets_and_strikethrough_stripped() {
        // review wf_d43814c3: list markers must not be spoken.
        assert_eq!(strip_markdown_for_tts("* Option one"), "Option one");
        assert_eq!(strip_markdown_for_tts("- first item"), "first item");
        assert_eq!(strip_markdown_for_tts("+ plus item"), "plus item");
        // Numbered bullets strip when the text also carries a markdown char
        // (the fast-path gate doesn't trip on pure "1. " to avoid scanning
        // every numeric sentence) — e.g. alongside a bold marker.
        assert_eq!(strip_markdown_for_tts("1. **step** one"), "step one");
        assert_eq!(
            strip_markdown_for_tts("~~scratch that~~ done"),
            "scratch that done"
        );
        // A subtraction (no trailing-space bullet shape) is untouched.
        assert_eq!(strip_markdown_for_tts("5-3 equals 2"), "5-3 equals 2");
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(strip_markdown_for_tts("plain"), "plain");
        assert_eq!(
            strip_markdown_for_tts("Hello there, how are you?"),
            "Hello there, how are you?"
        );
    }
}

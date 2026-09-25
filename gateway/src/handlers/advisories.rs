//! The warning channel for `/v1/audio/*` (FRD-018 Part III W1).
//!
//! WaaV's rule for an unsupported configuration is **degrade with a warning, never a 400** — a
//! deployment-time choice must not become a serve-time outage. On the WebSocket that rule is real:
//! `ConfigWarning` is a variant of `OutgoingMessage` and the caller receives it. On the REST plane
//! there was no warning channel at all: the handlers called `warn!()` into WaaV's own log and
//! returned 200. So on this plane the rule read "degrade **silently**", which is precisely the
//! failure mode Part III exists to remove, reproduced inside its own design rule.
//!
//! Two carriers, because neither covers every case on its own:
//!
//! * **A response header**, [`WARNING_HEADER`], repeated once per advisory. This is the only
//!   option that works everywhere: `/v1/audio/speech` answers with audio bytes, and three of the
//!   five transcription response formats (`text`, `srt`, `vtt`) have no JSON body to put an
//!   array in.
//! * **A `warnings` array on `verbose_json`**, where a JSON body does exist and an SDK can read
//!   it without reaching for response headers.
//!
//! The `warn!()` logging stays. The header is for the caller; the log is for us, and the two
//! answer different questions.

use axum::http::{HeaderMap, HeaderValue};

/// Repeated once per advisory. HTTP permits a repeated field name, and repetition avoids
/// inventing a separator that a message might itself contain.
pub const WARNING_HEADER: &str = "x-bud-config-warning";

/// How many advisories were raised, so a client can tell "none" from "the header was stripped".
///
/// Proxies do drop unknown headers. A count that disagrees with the number of `x-bud-config-warning`
/// headers received is a legible symptom; silence is not.
pub const WARNING_COUNT_HEADER: &str = "x-bud-config-warning-count";

/// Advisories raised while assembling a request's configuration.
#[derive(Debug, Clone, Default)]
pub struct Advisories(Vec<String>);

impl Advisories {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one advisory, and log it.
    ///
    /// Both, always, from one call: a warning that reaches the caller and not the log cannot be
    /// correlated with anything when they ask about it, and one that reaches the log and not the
    /// caller is the state this module was built to end.
    pub fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        tracing::warn!(advisory = %message, "audio request configuration degraded");
        self.0.push(message);
    }

    /// Record several, e.g. the `warnings` a language mapping produced.
    pub fn extend(&mut self, messages: impl IntoIterator<Item = String>) {
        for message in messages {
            self.warn(message);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Write the advisories onto a response's headers.
    pub fn apply(&self, headers: &mut HeaderMap) {
        if self.0.is_empty() {
            return;
        }
        if let Ok(count) = HeaderValue::from_str(&self.0.len().to_string()) {
            headers.insert(WARNING_COUNT_HEADER, count);
        }
        for message in &self.0 {
            if let Ok(value) = HeaderValue::from_str(&header_safe(message)) {
                headers.append(WARNING_HEADER, value);
            }
        }
    }
}

/// Render a message as something a header value can legally carry.
///
/// Header values are visible ASCII plus space and horizontal tab; a vendor voice id or a language
/// name can be neither (`元気な女性` is a real Cartesia voice). Substituting rather than dropping
/// keeps the advisory — a caller who sees `voice '???' is not published by cartesia` still learns
/// what went wrong and which knob to look at, whereas a silently omitted header is the thing this
/// module exists to prevent. `verbose_json` carries the message intact.
fn header_safe(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut substituted = false;
    for c in message.chars() {
        // 0x20..=0x7E is visible ASCII plus space. Tab is legal too but reads badly in a header.
        if (' '..='~').contains(&c) {
            out.push(c);
            substituted = false;
        } else if !substituted {
            // One `?` per run, so a Japanese voice name does not become forty question marks.
            out.push('?');
            substituted = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_set_writes_no_headers() {
        let mut headers = HeaderMap::new();
        Advisories::new().apply(&mut headers);
        assert!(headers.is_empty(), "silence must stay silent");
    }

    #[test]
    fn every_advisory_gets_its_own_header_plus_a_count() {
        let mut advisories = Advisories::new();
        advisories.warn("emotion 'calm' is not supported by deepgram; synthesising normally");
        advisories.warn("sample_rate cleared: mp3 carries its own rate");

        let mut headers = HeaderMap::new();
        advisories.apply(&mut headers);

        assert_eq!(headers.get_all(WARNING_HEADER).iter().count(), 2);
        assert_eq!(headers.get(WARNING_COUNT_HEADER).unwrap(), "2");
    }

    #[test]
    fn the_count_matches_what_was_sent_so_a_stripping_proxy_is_visible() {
        let mut advisories = Advisories::new();
        for i in 0..5 {
            advisories.warn(format!("advisory {i}"));
        }
        let mut headers = HeaderMap::new();
        advisories.apply(&mut headers);

        assert_eq!(
            headers.get(WARNING_COUNT_HEADER).unwrap(),
            headers
                .get_all(WARNING_HEADER)
                .iter()
                .count()
                .to_string()
                .as_str()
        );
    }

    #[test]
    fn a_non_ascii_message_still_produces_a_header() {
        // `HeaderValue::from_str` REFUSES non-visible-ASCII, so without the substitution this
        // advisory would be dropped entirely — and a dropped warning is indistinguishable from
        // no warning, which is the defect W1 exists to fix.
        let mut advisories = Advisories::new();
        advisories.warn("voice '元気な女性' is not published by cartesia");

        let mut headers = HeaderMap::new();
        advisories.apply(&mut headers);

        let value = headers
            .get(WARNING_HEADER)
            .expect("the advisory must survive");
        let rendered = value.to_str().unwrap();
        assert!(rendered.contains("is not published by cartesia"));
        assert!(rendered.contains('?'));
    }

    #[test]
    fn a_run_of_unrenderable_characters_collapses_to_one_marker() {
        assert_eq!(header_safe("voice '元気な女性' here"), "voice '?' here");
    }

    #[test]
    fn a_newline_cannot_smuggle_a_second_header() {
        // Header injection: a message carrying CRLF would otherwise terminate the value and let
        // the rest be read as another header. `from_str` would reject it, dropping the advisory;
        // substituting keeps it and makes injection impossible.
        let rendered = header_safe("bad\r\nx-injected: yes");
        assert!(!rendered.contains('\r') && !rendered.contains('\n'));
        assert!(
            rendered.contains("x-injected: yes"),
            "the text is kept, the framing is not"
        );
    }

    #[test]
    fn ordinary_text_is_untouched() {
        let message = "emotion 'calm' is not supported by deepgram (0.25-4.0)";
        assert_eq!(header_safe(message), message);
    }
}

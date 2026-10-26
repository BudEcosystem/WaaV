//! Human in the loop: what the caller's answer to an open question means (the front reads it,
//! chapter 9), and the rule that decides whether an accept binds.

use super::types::{Attribution, EndOfTurn, EotSource, HitlRequest, Ms};
#[cfg(test)]
pub use lexicon_reading::parse_answer;

/// What the caller's words mean as an answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    /// `strong`: "yes", "go ahead"; not "okay".
    Accept {
        strong: bool,
    },
    Decline {
        correction: Option<String>,
    },
    Cancel,
    /// Index into the request's options.
    Choice(usize),
    Value(String),
    Unclear,
}

/// Whether an accept may release the held operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Binds,
    /// A weak or poorly attributed yes: ask once more.
    WeakYes,
    /// The caller answered before the read-back was heard: ask once more.
    NotHeard,
}

/// The lexicon's reading of an answer: test data only. The front reads a turn's answer in
/// context (chapter 9), on the cascade and on a speech-to-speech session alike (owner, 24 Oct:
/// no word list reads the caller's meaning).
#[cfg(test)]
pub mod lexicon_reading {
    use super::super::persona::Lexicon;
    use super::super::types::{HitlKind, HitlRequest};
    use super::Parsed;

    /// Words that turn an accept into a decline ("yes, but not on Saturday").
    const NEGATIONS: [&str; 9] = [
        "not", "no", "never", "don't", "dont", "won't", "can't", "cannot", "neither",
    ];
    /// Words dropped from the start of a correction ("yes, but on the Mastercard").
    const CONNECTORS: [&str; 6] = ["but", "and", "so", "well", "just", "please"];
    /// Spoken numbers, so "for four" repeats a read-back of `4` and "the second one" picks an option.
    const NUMBERS: [&str; 12] = [
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
        "twelve",
    ];
    const ORDINALS: [&str; 6] = ["first", "second", "third", "fourth", "fifth", "sixth"];

    /// One word of the caller's text: lower-cased for matching, and as it was said.
    struct Word<'a> {
        norm: String,
        raw: &'a str,
    }

    fn words(text: &str) -> Vec<Word<'_>> {
        text.split_whitespace()
            .filter_map(|raw| {
                let raw = raw.trim_matches(|c: char| !c.is_alphanumeric());
                let norm = raw.to_lowercase().replace('\u{2019}', "'");
                (!raw.is_empty()).then_some(Word { norm, raw })
            })
            .collect()
    }

    /// A spoken number as digits ("four" -> "4"); any other word unchanged.
    fn digits(word: &str) -> String {
        match NUMBERS.iter().position(|number| *number == word) {
            Some(index) => (index + 1).to_string(),
            None => word.to_owned(),
        }
    }

    /// Marks every occurrence of one of `phrases` in `words`, longest phrase first. `true` when
    /// one was found.
    fn mark(words: &[Word<'_>], phrases: &[String], taken: &mut [bool]) -> bool {
        let mut phrases: Vec<Vec<&str>> = phrases
            .iter()
            .map(|phrase| phrase.split_whitespace().collect())
            .filter(|phrase: &Vec<&str>| !phrase.is_empty())
            .collect();
        phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.len()));
        let mut found = false;
        for phrase in &phrases {
            for start in 0..words.len().saturating_sub(phrase.len() - 1) {
                let span = start..start + phrase.len();
                let free = !taken[span.clone()].iter().any(|taken| *taken);
                if free
                    && words[span.clone()]
                        .iter()
                        .map(|word| word.norm.as_str())
                        .eq(phrase.iter().copied())
                {
                    taken[span].fill(true);
                    found = true;
                }
            }
        }
        found
    }

    /// The words of the read-back: repeating one of them is not a correction.
    fn readback_words(request: &HitlRequest) -> Vec<String> {
        let mut text = request.verb_phrase.clone();
        for (name, value) in &request.params {
            text.push(' ');
            text.push_str(name);
            text.push(' ');
            match value {
                serde_json::Value::String(value) => text.push_str(value),
                other => text.push_str(&other.to_string()),
            }
        }
        words(&text).iter().map(|word| digits(&word.norm)).collect()
    }

    fn parse_confirm(said: &[Word<'_>], request: &HitlRequest, lexicon: &Lexicon) -> Parsed {
        let mut taken = vec![false; said.len()];
        if mark(said, &lexicon.cancel, &mut taken) {
            return Parsed::Cancel;
        }
        let declined = mark(said, &lexicon.decline, &mut taken);
        let strong = mark(said, &lexicon.strong_accept, &mut taken);
        let accepted = mark(said, &lexicon.weak_accept, &mut taken) || strong;

        let known = readback_words(request);
        let rest: Vec<&Word<'_>> = said
            .iter()
            .zip(&taken)
            .filter_map(|(word, taken)| (!taken).then_some(word))
            .collect();
        let negated = rest
            .iter()
            .any(|word| NEGATIONS.contains(&word.norm.as_str()) || word.norm.ends_with("n't"));
        let new_content = rest.iter().any(|word| {
            !lexicon.content_word_stop_list.contains(&word.norm)
                && !known.contains(&digits(&word.norm))
        });
        let correction = || {
            let from = rest
                .iter()
                .position(|word| !CONNECTORS.contains(&word.norm.as_str()))?;
            let text: Vec<&str> = rest[from..].iter().map(|word| word.raw).collect();
            Some(text.join(" "))
        };

        match (accepted, declined) {
            // "No problem, go ahead" and "don't do it" both land here: ask again.
            (true, true) => Parsed::Unclear,
            (false, true) => Parsed::Decline {
                correction: new_content.then(correction).flatten(),
            },
            (true, false) if negated || new_content => Parsed::Decline {
                correction: correction(),
            },
            (true, false) => Parsed::Accept { strong },
            // No yes and no no, but a change ("actually make it four"): the whole of it corrects.
            (false, false) if mark(said, &lexicon.amendment_cues, &mut taken) => Parsed::Decline {
                correction: Some(
                    said.iter()
                        .map(|word| word.raw)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
            },
            (false, false) => Parsed::Unclear,
        }
    }

    fn parse_choice(said: &[Word<'_>], request: &HitlRequest, lexicon: &Lexicon) -> Parsed {
        let heard: Vec<String> = said.iter().map(|word| digits(&word.norm)).collect();
        // How many distinctive words of each option the caller said; the whole label wins outright.
        let hits: Vec<usize> = request
            .options
            .iter()
            .map(|option| {
                let label: Vec<String> = words(option)
                    .iter()
                    .map(|word| digits(&word.norm))
                    .collect();
                if !label.is_empty() && heard.windows(label.len()).any(|window| window == label) {
                    return usize::MAX;
                }
                label
                    .iter()
                    .filter(|word| !lexicon.content_word_stop_list.contains(word))
                    .filter(|word| heard.contains(word))
                    .count()
            })
            .collect();
        let best = hits.iter().copied().max().unwrap_or(0);
        if best > 0 && hits.iter().filter(|hit| **hit == best).count() == 1 {
            return Parsed::Choice(hits.iter().position(|hit| *hit == best).unwrap_or(0));
        }

        let ordinal = said.iter().find_map(|word| {
            let word = word.norm.as_str();
            if word == "last" {
                return request.options.len().checked_sub(1);
            }
            ORDINALS
                .iter()
                .position(|ordinal| *ordinal == word)
                .or_else(|| digits(word).parse::<usize>().ok()?.checked_sub(1))
        });
        match ordinal {
            Some(index) if index < request.options.len() => Parsed::Choice(index),
            _ => {
                let mut taken = vec![false; said.len()];
                if mark(said, &lexicon.cancel, &mut taken) {
                    Parsed::Cancel
                } else if mark(said, &lexicon.decline, &mut taken)
                    || heard.iter().any(|w| w == "neither")
                {
                    Parsed::Decline { correction: None }
                } else {
                    Parsed::Unclear
                }
            }
        }
    }

    /// Read the caller's words as an answer to `request`.
    pub fn parse_answer(text: &str, request: &HitlRequest, lexicon: &Lexicon) -> Parsed {
        let said = words(text);
        if said.is_empty() {
            return Parsed::Unclear;
        }
        match request.kind {
            HitlKind::Confirm => parse_confirm(&said, request, lexicon),
            HitlKind::Choose => parse_choice(&said, request, lexicon),
            // Answered on the client's screen: spoken words are no answer.
            HitlKind::Form | HitlKind::AuthUrl => Parsed::Unclear,
            HitlKind::Value => {
                let mut taken = vec![false; said.len()];
                mark(&said, &lexicon.cancel, &mut taken);
                if taken.iter().all(|taken| *taken) {
                    Parsed::Cancel
                } else {
                    Parsed::Value(text.trim().to_owned())
                }
            }
        }
    }
}

/// The binding rule for `Parsed::Accept { strong }` (a clean accept: `parse_answer` returns a
/// decline for a negated or amended one). `readback_heard_at` is when the read-back was heard
/// (`None`: not yet). A typed turn has no audio to doubt, so only the strong check applies.
pub fn binds(
    strong: bool,
    request: &HitlRequest,
    turn: &EndOfTurn,
    readback_heard_at: Option<Ms>,
    heard_margin_ms: u32,
) -> Binding {
    if turn.source != EotSource::Typed {
        let onset = turn.onset_at.unwrap_or(turn.at);
        let heard = readback_heard_at.is_some_and(|at| onset >= at + Ms::from(heard_margin_ms));
        if !heard {
            return Binding::NotHeard;
        }
        if turn.attribution != Attribution::High || turn.suspected_echo {
            return Binding::WeakYes;
        }
    }
    if strong || !request.irreversible {
        Binding::Binds
    } else {
        Binding::WeakYes
    }
}

#[cfg(test)]
mod tests {
    use super::super::persona::Lexicon;
    use super::super::types::{Class, HitlId, HitlKind, OpId, TaskId, TurnId};
    use super::*;

    fn lexicon() -> Lexicon {
        let list = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
        Lexicon {
            strong_accept: list(&[
                "yes",
                "yes please",
                "yeah",
                "go ahead",
                "do it",
                "that's right",
                "book it",
            ]),
            weak_accept: list(&["okay", "ok", "sure", "fine", "sounds good", "mm-hm"]),
            decline: list(&[
                "no",
                "nope",
                "no thanks",
                "don't",
                "do not",
                "that's not right",
            ]),
            cancel: list(&["never mind", "forget it", "cancel", "cancel that"]),
            content_word_stop_list: list(&[
                "the", "a", "and", "but", "to", "on", "for", "it", "that", "i", "you", "do",
                "please", "okay", "yeah", "one", "at", "is",
            ]),
            ..Lexicon::default()
        }
    }

    fn request(kind: HitlKind, irreversible: bool) -> HitlRequest {
        HitlRequest {
            id: HitlId("q_task_1_1".to_owned()),
            task: TaskId("task_1".to_owned()),
            revision: 1,
            kind,
            op: (kind == HitlKind::Confirm).then(|| OpId("op_task_1_1_1".to_owned())),
            verb_phrase: "book a table at Rosa's".to_owned(),
            params: vec![
                ("party".to_owned(), serde_json::json!(4)),
                ("day".to_owned(), serde_json::json!("Saturday")),
            ],
            question: None,
            options: vec![
                "Window seat".to_owned(),
                "Aisle seat".to_owned(),
                "Middle seat".to_owned(),
            ],
            irreversible,
            expires_at: 30_000,
            ..Default::default()
        }
    }

    /// A caller turn that passes every binding check against a read-back heard at 5,000 ms.
    fn turn() -> EndOfTurn {
        EndOfTurn {
            turn: TurnId(3),
            at: 6_000,
            acoustic_end: Some(5_800),
            onset_at: Some(5_400),
            text: "yes".to_owned(),
            source: EotSource::SttSpeechFinal,
            eager_same: None,
            class: Class::Interrupt,
            attribution: Attribution::High,
            suspected_echo: false,
            unclear: false,
            foreign: false,
        }
    }

    fn confirm(text: &str) -> Parsed {
        parse_answer(text, &request(HitlKind::Confirm, true), &lexicon())
    }

    #[test]
    fn a_plain_yes_is_strong_and_okay_is_weak() {
        assert_eq!(confirm("Yes."), Parsed::Accept { strong: true });
        assert_eq!(confirm("Yes, go ahead!"), Parsed::Accept { strong: true });
        assert_eq!(confirm("Sure, go ahead"), Parsed::Accept { strong: true });
        assert_eq!(confirm("Okay."), Parsed::Accept { strong: false });
        assert_eq!(confirm("mm-hm"), Parsed::Accept { strong: false });
    }

    #[test]
    fn an_accept_with_new_content_is_a_decline_with_the_correction() {
        assert_eq!(
            confirm("Yes, but on the Mastercard."),
            Parsed::Decline {
                correction: Some("on the Mastercard".to_owned())
            }
        );
        assert_eq!(
            confirm("yes but not on Saturday"),
            Parsed::Decline {
                correction: Some("not on Saturday".to_owned())
            }
        );
        assert_eq!(
            confirm("No, make it six"),
            Parsed::Decline {
                correction: Some("make it six".to_owned())
            }
        );
    }

    #[test]
    fn repeating_the_read_back_is_not_a_correction() {
        assert_eq!(
            confirm("Yes, book the table for four"),
            Parsed::Accept { strong: true }
        );
        assert_eq!(confirm("yeah Saturday"), Parsed::Accept { strong: true });
    }

    #[test]
    fn decline_cancel_and_unclear() {
        assert_eq!(confirm("no"), Parsed::Decline { correction: None });
        assert_eq!(confirm("No thanks."), Parsed::Decline { correction: None });
        assert_eq!(confirm("never mind"), Parsed::Cancel);
        assert_eq!(confirm("yes, cancel that"), Parsed::Cancel);
        assert_eq!(confirm("no problem, go ahead"), Parsed::Unclear);
        assert_eq!(confirm("what time is it"), Parsed::Unclear);
        assert_eq!(confirm("  "), Parsed::Unclear);
    }

    #[test]
    fn a_change_with_neither_yes_nor_no_is_a_decline_with_the_correction() {
        let lexicon = Lexicon {
            amendment_cues: vec!["actually".to_owned(), "make it".to_owned()],
            ..lexicon()
        };
        let confirm = request(HitlKind::Confirm, true);
        assert_eq!(
            parse_answer("Actually, make it six.", &confirm, &lexicon),
            Parsed::Decline {
                correction: Some("Actually make it six".to_owned())
            }
        );
        assert_eq!(
            parse_answer("what time is it", &confirm, &lexicon),
            Parsed::Unclear
        );
    }

    #[test]
    fn a_strong_heard_attributed_yes_binds() {
        let request = request(HitlKind::Confirm, true);
        assert_eq!(
            binds(true, &request, &turn(), Some(5_000), 150),
            Binding::Binds
        );
    }

    #[test]
    fn check_heard_an_early_yes_does_not_bind() {
        let request = request(HitlKind::Confirm, true);
        let mut early = turn();
        early.onset_at = Some(5_149);
        assert_eq!(
            binds(true, &request, &early, Some(5_000), 150),
            Binding::NotHeard
        );
        early.onset_at = Some(5_150);
        assert_eq!(
            binds(true, &request, &early, Some(5_000), 150),
            Binding::Binds
        );
        // The read-back has not been heard at all.
        assert_eq!(binds(true, &request, &turn(), None, 150), Binding::NotHeard);
    }

    #[test]
    fn check_strong_a_weak_yes_binds_only_a_reversible_action() {
        let irreversible = request(HitlKind::Confirm, true);
        let reversible = request(HitlKind::Confirm, false);
        assert_eq!(
            binds(false, &irreversible, &turn(), Some(5_000), 150),
            Binding::WeakYes
        );
        assert_eq!(
            binds(false, &reversible, &turn(), Some(5_000), 150),
            Binding::Binds
        );
    }

    #[test]
    fn check_attributed_a_doubtful_speaker_does_not_bind() {
        let reversible = request(HitlKind::Confirm, false);
        let mut doubtful = turn();
        doubtful.attribution = Attribution::Medium;
        assert_eq!(
            binds(true, &reversible, &doubtful, Some(5_000), 150),
            Binding::WeakYes
        );
        let mut echo = turn();
        echo.suspected_echo = true;
        assert_eq!(
            binds(true, &reversible, &echo, Some(5_000), 150),
            Binding::WeakYes
        );
    }

    #[test]
    fn check_clean_is_decided_by_the_parser() {
        // A negated or amended yes never reaches `binds`: it is not an accept.
        assert!(matches!(
            confirm("yes but not that one"),
            Parsed::Decline { .. }
        ));
        assert!(matches!(
            confirm("yes on the Mastercard"),
            Parsed::Decline { .. }
        ));
    }

    #[test]
    fn a_typed_yes_needs_only_the_strong_check() {
        let irreversible = request(HitlKind::Confirm, true);
        let mut typed = turn();
        typed.source = EotSource::Typed;
        typed.onset_at = None;
        typed.attribution = Attribution::Medium;
        assert_eq!(
            binds(true, &irreversible, &typed, None, 150),
            Binding::Binds
        );
        assert_eq!(
            binds(false, &irreversible, &typed, None, 150),
            Binding::WeakYes
        );
    }

    #[test]
    fn choose_by_label_or_ordinal() {
        let request = request(HitlKind::Choose, false);
        let choose = |text: &str| parse_answer(text, &request, &lexicon());
        assert_eq!(choose("the aisle seat please"), Parsed::Choice(1));
        assert_eq!(choose("window"), Parsed::Choice(0));
        assert_eq!(choose("the second one"), Parsed::Choice(1));
        assert_eq!(choose("number three"), Parsed::Choice(2));
        assert_eq!(choose("the last one"), Parsed::Choice(2));
        assert_eq!(choose("the fifth"), Parsed::Unclear);
        assert_eq!(choose("a seat"), Parsed::Unclear);
        assert_eq!(choose("neither"), Parsed::Decline { correction: None });
        assert_eq!(choose("forget it"), Parsed::Cancel);
    }

    #[test]
    fn a_value_is_the_callers_words() {
        let request = request(HitlKind::Value, false);
        assert_eq!(
            parse_answer(" Jordan Lee. ", &request, &lexicon()),
            Parsed::Value("Jordan Lee.".to_owned())
        );
        assert_eq!(
            parse_answer("never mind", &request, &lexicon()),
            Parsed::Cancel
        );
    }
}

//! The fact guard: the claims, promises and narration a front line may not contain. Port of the
//! `FactGuard` rules of `docs/duplex-agent/front-model/front_reference.py`; the entity rules
//! are not ported, because the per-decision grammar limits a line to bank fragments and the
//! caller's own words.

use std::sync::OnceLock;

use regex::Regex;

use super::persona::{self, GuardLexicon, any_phrase, has_phrase, norm_phrase, straighten};
use super::types::{Act, GuardOutcome};

/// What the guard knows about the line besides its words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardCtx {
    /// The act the line belongs to; narration is allowed when it delegates.
    pub act: Act,
    /// The caller asked a yes/no question, so the line may not open by affirming.
    pub polar_question: bool,
}

/// The first rule a front line breaks, in the reference's order.
pub fn check(line: &str, ctx: &GuardCtx) -> GuardOutcome {
    if line.is_empty() {
        return GuardOutcome::Pass;
    }
    let lex = persona::guard_lexicon();
    let norm = norm_phrase(line);
    // Saying that work goes on is true when this act starts the work or reports on a live task.
    let backed = matches!(ctx.act, Act::Delegate | Act::Update | Act::Progress);
    let rule = if role_leak(lex, line, &norm) {
        "role_leak"
    } else if disfluency(lex, &norm) {
        "disfluency"
    } else if completion_claim(lex, line) {
        "completion_claim"
    } else if outcome_prediction(lex, line) {
        "outcome_prediction"
    } else if ctx.polar_question && polar_affirmation(lex, &norm) {
        "polar_affirmation"
    } else if eta_promise(lex, &norm) {
        "eta_promise"
    } else if channel_promise(lex, line) {
        "channel_promise"
    } else if !backed && action_narration(lex, &norm) {
        "unbacked_action_narration"
    } else {
        return GuardOutcome::Pass;
    };
    GuardOutcome::Blocked(rule)
}

/// What a speech-to-speech provider asserted on its own in one sentence, as the claim
/// monitor names it: `(kind, rule)` of `unverified_claim`. A promise of an action, a time or a
/// channel is an `outcome`; a claim that something was done is a `completion`. A claim asked
/// about inside an inquiry frame is no claim.
pub fn provider_claim(sentence: &str) -> Option<(&'static str, &'static str)> {
    let lex = persona::guard_lexicon();
    let norm = norm_phrase(sentence);
    if channel_promise(lex, sentence) {
        return Some(("outcome", "ChannelPromise"));
    }
    if eta_promise(lex, &norm) {
        return Some(("outcome", "EtaPromise"));
    }
    if !completion_claim(lex, sentence) {
        return None;
    }
    let spaced = format!(" {norm} ");
    let promise = ["i'll ", "i will ", "i'm going to ", "we'll ", "we will "];
    // "It couldn't be processed" says that it was not done.
    let negated = [
        "n't ",
        " not ",
        " never ",
        " unable ",
        " failed ",
        " no longer ",
    ];
    if promise.iter().any(|p| spaced.contains(&format!(" {p}"))) {
        Some(("outcome", "ActionPromise"))
    } else if negated.iter().any(|n| spaced.contains(n)) {
        None
    } else {
        Some(("completion", "CompletionClaim"))
    }
}

/// The lexicon's regular expressions, compiled once per lexicon: completion claims, then
/// time promises. A pattern that is not a regular expression matches nothing (a pack's file
/// is checked when it is loaded).
fn patterns(lex: &GuardLexicon) -> &(Vec<Regex>, Vec<Regex>) {
    lex.compiled.get_or_init(|| {
        let compile = |sources: &[String]| {
            sources
                .iter()
                .filter_map(|source| Regex::new(source).ok())
                .collect()
        };
        (
            compile(&lex.completion_claims.patterns),
            compile(&lex.eta_promises.patterns),
        )
    })
}

/// The reference's compile-time check of a persona pack line (`check_bank_line`), the check a
/// persona given at run time passes like the packaged ones: the runtime rules, except that
/// narration is not checked (these lines are spoken by the policy on a real event), an
/// event-backed line may state its event, a line tagged polar may open by affirming, and any
/// time phrase blocks, promise verb or not. The rule it breaks, if any.
pub(super) fn pack_line(
    lex: &GuardLexicon,
    text: &str,
    polar: bool,
    event_backed: bool,
) -> Option<&'static str> {
    let strict_eta = &lex.eta_promises.strict_bank_phrases;
    let norm = norm_phrase(text);
    if role_leak(lex, text, &norm) {
        Some("role_leak")
    } else if disfluency(lex, &norm) {
        Some("disfluency")
    } else if !event_backed && completion_claim(lex, text) {
        Some("completion_claim")
    } else if !event_backed && outcome_prediction(lex, text) {
        Some("outcome_prediction")
    } else if !polar && polar_affirmation(lex, &norm) {
        Some("polar_affirmation")
    } else if any_phrase(&norm, strict_eta) {
        Some("eta_promise")
    } else if channel_promise(lex, text) {
        Some("channel_promise")
    } else {
        None
    }
}

/// The byte offsets where `label` stands in `text` with no letter on either side.
pub(super) fn standalone(text: &str, label: &str) -> Vec<usize> {
    let letter = |c: Option<char>| c.is_some_and(persona::spaced_letter);
    text.match_indices(label)
        .map(|(at, _)| at)
        .filter(|at| {
            !letter(text[..*at].chars().next_back())
                && !letter(text[at + label.len()..].chars().next())
        })
        .collect()
}

/// A sentence without a label it opens with: one of the role labels in any case, a "To" before
/// it or not, and a colon ("ToFront:", "Front:", "USER:"), or any word of three or more
/// capitals and a colon, or "Answer:" in any case. A background model sometimes writes its answer as a line of a
/// transcript, or under a heading of its own.
pub(super) fn strip_speaker_label(sentence: &str) -> &str {
    let text = sentence.trim_start();
    let Some((label, rest)) = text.split_once(':') else {
        return sentence;
    };
    let name = label.to_ascii_uppercase();
    let name = name.strip_prefix("TO").unwrap_or(&name).trim_start();
    let labels = &persona::guard_lexicon().role_leak.labels_case_sensitive;
    // A word in capitals before the colon is a label the model made up ("ORDER:", a misspelt
    // "ANSWER:"), whatever it says.
    let letters = label.chars().filter(char::is_ascii_uppercase).count();
    let capitals = letters >= 3
        && label.trim() != TRANSCRIPT_LABEL
        && label
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c == ' ');
    if capitals || name == ANSWER_LABEL || labels.iter().any(|known| known == name) {
        rest.trim_start()
    } else {
        sentence
    }
}

/// The label a background model writes when it takes the call for a transcript to continue
/// (`HEARD: ...`): such a line reports, it does not answer. The front's labels are in the
/// lexicon.
const TRANSCRIPT_LABEL: &str = "HEARD";
/// The heading a background model puts over its answer in any case (live MA1 and MA7,
/// r8-verify: gpt-oss-20b said "Answer: Your note titled ... is being deleted.").
const ANSWER_LABEL: &str = "ANSWER";
/// Digits in a row (spaces, dashes, dots and brackets between them allowed) from which a
/// number is long: a phone, card or account number. A price, a time, a date, a flight number
/// and a short reference stay below it.
const LONG_NUMBER_DIGITS: usize = 7;

/// The outbound speech policy for a sentence of a background answer, in this order: no
/// prompt scaffolding (a label of the front's or the background's prompt followed by a colon,
/// as in `HEARD: ...`), no link, and no long number the caller did not give. `caller` is
/// everything the caller said in the call. The background's text is not verified: an answer
/// built on a tool result or a page can carry a line somebody planted there.
pub fn outbound(sentence: &str, caller: &str) -> GuardOutcome {
    let labels = &persona::guard_lexicon().role_leak.labels_case_sensitive;
    let labels = labels.iter().map(String::as_str).chain([TRANSCRIPT_LABEL]);
    let labelled = |label: &str| {
        let after = |at: usize| sentence[at + label.len()..].trim_start().starts_with(':');
        standalone(sentence, label).into_iter().any(after)
    };
    let rule = if labels.into_iter().any(labelled) {
        "scaffolding"
    } else if link().is_match(sentence) {
        "link"
    } else if long_numbers(sentence)
        .iter()
        .any(|number| !digits(caller).contains(number.as_str()))
    {
        "number_not_given"
    } else {
        return GuardOutcome::Pass;
    };
    GuardOutcome::Blocked(rule)
}

/// The most words a spoken task update may have (chapter 8's narrator). The one limit of the
/// narrator's line: its prompt asks for it (`front_model::narrator_system`), [`fit_narration`]
/// cuts a longer line to it and [`narration`] refuses what is still longer; its grammar bounds
/// the sentence, not its length (`front_model::sentence_grammar`). Live (launch demo take 15,
/// 27 Oct) the three disagreed: the prompt asked for 20 words, the grammar allowed 160
/// characters and this check refused 21, and a 22-word line with every count of a terminal
/// task ("The x86 Linux build has zero downloads, the ARM Linux build twelve, and the
/// checksums file fourteen, so the builds total twelve.") was refused on all three tries: the
/// update was never said.
pub const NARRATION_MAX_WORDS: usize = 20;
/// The fewest words a line cut to fit [`NARRATION_MAX_WORDS`] keeps: a shorter head ("Good
/// news.") says nothing of the result, and the line is refused instead.
const NARRATION_MIN_CUT_WORDS: usize = 4;

/// A task update the front worded, fitted to [`NARRATION_MAX_WORDS`]: a longer line, or one
/// the token budget cut before its full stop, ends at its last clause boundary (a comma,
/// semicolon or colon after a word) within the limit, with a full stop. A line with no such
/// boundary is left as it is, for [`narration`] to refuse. The 22-word line above becomes
/// "The x86 Linux build has zero downloads, the ARM Linux build twelve, and the checksums file
/// fourteen." (17 words): the result, without the clause past the limit.
pub fn fit_narration(line: &str) -> String {
    let line = line.trim();
    let words: Vec<&str> = line.split_whitespace().collect();
    let finished = line.ends_with(['.', '!']);
    if finished && words.len() <= NARRATION_MAX_WORDS {
        return line.to_owned();
    }
    let most = words.len().min(NARRATION_MAX_WORDS);
    let clause = (NARRATION_MIN_CUT_WORDS..=most)
        .rev()
        .find(|&n| words[n - 1].ends_with([',', ';', ':']));
    match clause {
        Some(n) => {
            let head = words[..n].join(" ");
            format!("{}.", head.trim_end_matches([',', ';', ':']))
        }
        None => line.to_owned(),
    }
}

/// Whether a sentence ends inside `body`: a `!`, a `?`, a line break, or a full stop that is
/// not a decimal or version point between two digits ("1.1.0", "2.5"). Live (take 15) the
/// narrator's grammar ended its line at the first full stop: "The GitHub release for version
/// 1." was all of it, refused for the "1" its result does not have.
fn ends_sentence_inside(body: &str) -> bool {
    let chars: Vec<char> = body.chars().collect();
    let digit = |at: Option<&char>| at.is_some_and(char::is_ascii_digit);
    chars.iter().enumerate().any(|(at, c)| match c {
        '!' | '?' | '\n' => true,
        '.' => !(at > 0 && digit(chars.get(at - 1)) && digit(chars.get(at + 1))),
        _ => false,
    })
}

/// The check of a task update the front worded (chapter 8): the outbound policy, with the
/// task's result among what was given, then one finished sentence (it ends with its full
/// stop: a line the token budget cut is no sentence) of at most [`NARRATION_MAX_WORDS`]
/// words whose every number and every name (a capitalised word that does not start the
/// sentence) is in `source`: the task's title and result and the caller's latest words. A
/// point between two digits ("version 1.1.0") ends no sentence.
pub fn narration(line: &str, source: &str) -> GuardOutcome {
    if let GuardOutcome::Blocked(rule) = outbound(line, source) {
        return GuardOutcome::Blocked(rule);
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let line = line.trim_end();
    let body = line.trim_end_matches(['.', '!']);
    let given = given_numbers(source);
    let rule = if words.is_empty() || words.len() > NARRATION_MAX_WORDS {
        "narration_length"
    } else if !line.ends_with(['.', '!']) {
        "narration_unfinished"
    } else if ends_sentence_inside(body) {
        "narration_sentences"
    } else if numbers(line).iter().any(|n| !given.iter().any(|g| g == n)) {
        "number_not_given"
    } else {
        let given = norm_phrase(source);
        let name = |word: &str| {
            let word = word.trim_matches(|c: char| !c.is_alphanumeric());
            word.chars().next().is_some_and(char::is_uppercase) && word != "I"
        };
        let unknown = |word: &str| !has_phrase(&given, &norm_phrase(word));
        if words.iter().skip(1).any(|word| name(word) && unknown(word)) {
            "name_not_given"
        } else {
            return GuardOutcome::Pass;
        }
    };
    GuardOutcome::Blocked(rule)
}

/// The numbers a source gives, by value: in digits ([`numbers`]) and in words
/// ([`spelled_numbers`]). Live (take 15) the browser agent wrote its result for speech,
/// "Version one point one point oh is published as Latest", and the narrator wrote "version
/// 1.1.0": the same number, not one the result lacks.
fn given_numbers(source: &str) -> Vec<String> {
    let mut given: Vec<String> = numbers(source).into_iter().map(str::to_owned).collect();
    given.extend(spelled_numbers(source).into_iter().map(|n| n.to_string()));
    given
}

/// The numbers a text says in words, by value: "twelve" 12, "twenty-seventh" 27 (and 20 and
/// 7), "twenty six" 26, and the "oh" of "point oh" 0. Up to ninety-nine: a larger number a
/// result says in words is not read.
fn spelled_numbers(text: &str) -> Vec<u32> {
    const UNITS: [(&str, &str, u32); 20] = [
        ("zero", "zeroth", 0),
        ("one", "first", 1),
        ("two", "second", 2),
        ("three", "third", 3),
        ("four", "fourth", 4),
        ("five", "fifth", 5),
        ("six", "sixth", 6),
        ("seven", "seventh", 7),
        ("eight", "eighth", 8),
        ("nine", "ninth", 9),
        ("ten", "tenth", 10),
        ("eleven", "eleventh", 11),
        ("twelve", "twelfth", 12),
        ("thirteen", "thirteenth", 13),
        ("fourteen", "fourteenth", 14),
        ("fifteen", "fifteenth", 15),
        ("sixteen", "sixteenth", 16),
        ("seventeen", "seventeenth", 17),
        ("eighteen", "eighteenth", 18),
        ("nineteen", "nineteenth", 19),
    ];
    const TENS: [(&str, &str, u32); 8] = [
        ("twenty", "twentieth", 20),
        ("thirty", "thirtieth", 30),
        ("forty", "fortieth", 40),
        ("fifty", "fiftieth", 50),
        ("sixty", "sixtieth", 60),
        ("seventy", "seventieth", 70),
        ("eighty", "eightieth", 80),
        ("ninety", "ninetieth", 90),
    ];
    let value = |table: &[(&str, &str, u32)], word: &str| {
        let found = table
            .iter()
            .find(|(name, nth, _)| *name == word || *nth == word);
        found.map(|(.., value)| *value)
    };
    let text = text.to_lowercase();
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|word| !word.is_empty())
        .collect();
    let mut values = Vec::new();
    for (at, word) in words.iter().enumerate() {
        if *word == "oh" && at > 0 && words[at - 1] == "point" {
            values.push(0);
        } else if let Some(unit) = value(&UNITS, word) {
            values.push(unit);
        } else if let Some(tens) = value(&TENS, word) {
            values.push(tens);
            // "twenty-seven" and "twenty seven" are split alike.
            if let Some(unit) = words
                .get(at + 1)
                .and_then(|next| value(&UNITS[1..10], next))
            {
                values.push(tens + unit);
            }
        }
    }
    values
}

/// The numbers of a text, by value: "09:40" holds 9 and 40, as "9:40" does.
fn numbers(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .map(|run| match run.trim_start_matches('0') {
            "" => "0",
            value => value,
        })
        .collect()
}

/// Words with which a sentence asks the caller for something.
const ASKS: [&str; 14] = [
    "tell me",
    "give me",
    "provide",
    "what is your",
    "what's your",
    "enter your",
    "say your",
    "read me",
    "read out",
    "confirm your",
    "need your",
    "can i have",
    "may i have",
    "could i have",
];

/// The sensitive category a sentence of a background answer asks the caller for ("Please tell
/// me your current password."). A sentence that only names one ("Your password was reset.")
/// asks for nothing.
pub fn asks_for_secret<'a>(sentence: &str, categories: &'a [String]) -> Option<&'a str> {
    // Also the bare request that ends in "please" (live P1, r15-verify: "Your current
    // password, please." was relayed to the caller).
    let bare = || norm_phrase(sentence).ends_with(" please ");
    super::rules::sensitive_category(sentence, categories).filter(|_| asks(sentence) || bare())
}

/// Whether a sentence asks the caller for something ("Please share it with me now."). After a
/// value was declined, such a sentence asks for it again without naming it.
pub fn asks(sentence: &str) -> bool {
    let norm = norm_phrase(sentence);
    let again = ["share it", "share your", "say it", "tell it"];
    ASKS.iter().chain(&again).any(|ask| has_phrase(&norm, ask))
}

/// The lines of a background's prompt that say who it is: the first sentence of each part
/// (`parts`: the persona's preamble, after its heading, and the operator's prompt), without
/// quoted text and what follows a colon (lines the prompt gives to say, "say: ..."). The rest
/// of a prompt also holds what the caller is to be told (the operator's facts, the preamble's
/// "Say it is not something this line can help with."), so an answer may repeat it.
pub fn role_lines(parts: &[&str]) -> String {
    static GIVEN: OnceLock<Regex> = OnceLock::new();
    let given = GIVEN.get_or_init(|| {
        Regex::new(r#""[^"\n]*"|“[^”]*”|«[^»]*»|:[^\n.?!]*"#).expect("the quote pattern is a regex")
    });
    let first = |part: &&str| {
        let line = part
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))?;
        let line = given.replace_all(line, " ");
        sentences(&line)
            .first()
            .map(|sentence| (*sentence).to_owned())
    };
    parts
        .iter()
        .filter_map(first)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a sentence of a background answer says again who its prompt tells it it is (live
/// P1, r15-verify: qwen2.5-3b relayed "You are the help desk for customer accounts.", the first
/// line of its prompt). `roles` is [`role_lines`] of the prompt the gateway sent it; the
/// sentence repeats one of them as [`repeats`] measures it.
pub fn repeats_prompt(sentence: &str, roles: &str) -> bool {
    roles.lines().any(|line| repeats(sentence, line))
}

/// Whether `sentence` says `line` again: a run of its words, in order, is also in the line,
/// and the run has at least the transcript echo filter's least words and its share of both
/// the sentence and the line. The measure of [`repeats_prompt`], and of a task update that
/// repeats one of the agent's recent lines (live, take 15: for the browser task the 2B
/// narrator copied the writer task's update it had just been shown as the call's last line,
/// "The launch announcement has been saved with highlights from the release notes instead of
/// a placeholder.", and it was said again under the browser task).
pub fn repeats(sentence: &str, line: &str) -> bool {
    use super::duplex::{ECHO_MIN_WORDS, ECHO_SHARE_PERCENT};
    let words = persona::word_list(sentence);
    if words.len() < ECHO_MIN_WORDS {
        return false;
    }
    let share = |run: usize, of: usize| run * 100 >= of * ECHO_SHARE_PERCENT;
    let known = persona::word_list(line);
    let longest = longest_run(&words, &known);
    longest >= ECHO_MIN_WORDS && share(longest, words.len()) && share(longest, known.len())
}

/// Whether two lines share a phrase: a run of at least the echo filter's least words, in the
/// same order ("thanks for waiting" in "Thanks for waiting, I'm back." and "Still working on
/// it, thanks for waiting.").
pub fn shares_phrase(one: &str, other: &str) -> bool {
    use super::duplex::ECHO_MIN_WORDS;
    let (one, other) = (persona::word_list(one), persona::word_list(other));
    longest_run(&one, &other) >= ECHO_MIN_WORDS
}

/// The longest run of `words` that `known` holds in the same order.
fn longest_run(words: &[String], known: &[String]) -> usize {
    let mut run = vec![0usize; known.len() + 1];
    let mut longest = 0;
    for word in words {
        for at in (0..known.len()).rev() {
            run[at + 1] = if known[at] == *word { run[at] + 1 } else { 0 };
            longest = longest.max(run[at + 1]);
        }
    }
    longest
}

/// A web address: a scheme, `www.`, or a name under a common top-level domain.
fn link() -> &'static Regex {
    static LINK: OnceLock<Regex> = OnceLock::new();
    LINK.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:https?://|www\.)|\b[a-z0-9-]+\.(?:com|org|net|io|co|ai|app|dev|gov|edu|info|biz|me|ly)\b",
        )
        .expect("the link pattern is a regex")
    })
}

fn digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

/// The digits of every long number in a sentence.
fn long_numbers(sentence: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut run = String::new();
    for c in sentence.chars().chain(std::iter::once('x')) {
        if c.is_ascii_digit() {
            run.push(c);
        } else if !(" -.()".contains(c) && !run.is_empty()) {
            if run.len() >= LONG_NUMBER_DIGITS {
                found.push(std::mem::take(&mut run));
            }
            run.clear();
        }
    }
    found
}

/// A prompt label or an internal role word.
fn role_leak(lex: &GuardLexicon, line: &str, norm: &str) -> bool {
    let leak = &lex.role_leak;
    leak.labels_case_sensitive
        .iter()
        .any(|label| !standalone(line, label).is_empty())
        || any_phrase(norm, &leak.phrases)
}

fn disfluency(lex: &GuardLexicon, norm: &str) -> bool {
    any_phrase(norm, &lex.disfluency.phrases)
}

/// The sentences of a line: it is cut after `.`, `?` or `!` followed by white space.
pub(super) fn sentences(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut closed = false;
    for (at, c) in line.char_indices() {
        if closed && c.is_whitespace() {
            out.push(&line[start..at]);
            start = at;
            closed = false;
        } else if !c.is_whitespace() {
            closed = ".?!".contains(c);
        }
    }
    out.push(&line[start..]);
    out.into_iter()
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .collect()
}

/// Whether a sentence asserts one of the phrases or patterns. A claim that follows an inquiry
/// phrase in the same sentence ("let me check whether it is included") is asked about, not
/// asserted.
fn claims(lex: &GuardLexicon, line: &str, phrases: &[String], patterns: &[Regex]) -> bool {
    let frames = &lex.inquiry_frames.phrases;
    sentences(line).into_iter().any(|sentence| {
        let norm = norm_phrase(sentence);
        let lower = straighten(sentence).to_lowercase();
        let framed = |claim: &str| {
            norm.find(&norm_phrase(claim)).is_some_and(|at| {
                frames.iter().any(|frame| {
                    norm.find(&norm_phrase(frame))
                        .is_some_and(|before| before < at)
                })
            })
        };
        phrases
            .iter()
            .any(|phrase| has_phrase(&norm, phrase) && !framed(phrase))
            || patterns.iter().any(|pattern| {
                pattern
                    .find(&lower)
                    .is_some_and(|hit| !framed(hit.as_str()))
            })
    })
}

fn completion_claim(lex: &GuardLexicon, line: &str) -> bool {
    claims(lex, line, &lex.completion_claims.phrases, &patterns(lex).0)
}

fn outcome_prediction(lex: &GuardLexicon, line: &str) -> bool {
    claims(lex, line, &lex.outcome_predictions.phrases, &[])
}

/// The first word, or the first two, affirm or deny.
fn polar_affirmation(lex: &GuardLexicon, norm: &str) -> bool {
    let first_words = &lex.polar_affirmation.first_words;
    let mut words = norm.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    let two = words.next().map(|second| format!("{first} {second}"));
    first_words
        .iter()
        .any(|word| word == first || Some(word) == two.as_ref())
}

/// A time phrase together with a promise verb.
fn eta_promise(lex: &GuardLexicon, norm: &str) -> bool {
    let eta = &lex.eta_promises;
    (any_phrase(norm, &eta.phrases) || patterns(lex).1.iter().any(|p| p.is_match(norm)))
        && any_phrase(norm, &eta.promise_verbs)
}

fn channel_promise(lex: &GuardLexicon, line: &str) -> bool {
    claims(lex, line, &lex.channel_promises.phrases, &[])
}

/// First-person talk of work in progress.
fn action_narration(lex: &GuardLexicon, norm: &str) -> bool {
    any_phrase(norm, &lex.action_narration.phrases)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn front(act: Act, polar_question: bool, line: &str) -> GuardOutcome {
        check(
            line,
            &GuardCtx {
                act,
                polar_question,
            },
        )
    }

    fn blocked(rule: &'static str) -> GuardOutcome {
        GuardOutcome::Blocked(rule)
    }

    #[test]
    fn sentences_are_cut_at_final_punctuation() {
        assert_eq!(
            sentences("Okay. Is it done?  Yes! It is"),
            ["Okay.", "Is it done?", "Yes!", "It is"]
        );
        assert_eq!(
            sentences("U.S. fares, e.g. 3.50"),
            ["U.S.", "fares, e.g.", "3.50"]
        );
        assert!(sentences("  ").is_empty());
    }

    #[test]
    fn a_providers_sentence_is_a_completion_claim_a_promise_or_neither() {
        assert_eq!(
            provider_claim("Great, I've booked your table for two."),
            Some(("completion", "CompletionClaim"))
        );
        assert_eq!(
            provider_claim("Your refund has been processed."),
            Some(("completion", "CompletionClaim"))
        );
        assert_eq!(
            provider_claim("Sure, I'll book that for you."),
            Some(("outcome", "ActionPromise"))
        );
        assert_eq!(
            provider_claim("I'll email you the receipt."),
            Some(("outcome", "ChannelPromise"))
        );
        assert_eq!(
            provider_claim("Let me check whether it is confirmed."),
            None
        );
        assert_eq!(provider_claim("The capital of France is Paris."), None);
        assert_eq!(
            provider_claim("The booking request couldn't be processed at the moment."),
            None
        );
    }

    #[test]
    fn completion_claims() {
        assert_eq!(
            front(
                Act::Social,
                false,
                "Your Lisbon booking is still confirmed."
            ),
            blocked("completion_claim")
        );
        assert_eq!(
            front(Act::Delegate, false, "Okay, I've booked your table."),
            blocked("completion_claim")
        );
        assert_eq!(
            front(Act::Delegate, false, "Got it, I'll cancel the order."),
            blocked("completion_claim")
        );
        // Inside an inquiry frame the claim is asked about, not asserted.
        assert_eq!(
            front(
                Act::Delegate,
                false,
                "Okay, I'll find out if your refund is approved."
            ),
            GuardOutcome::Pass
        );
        // The frame covers its own sentence only.
        assert_eq!(
            front(
                Act::Delegate,
                false,
                "Okay, let me check. Your refund is approved."
            ),
            blocked("completion_claim")
        );
    }

    #[test]
    fn outcome_predictions() {
        assert_eq!(
            front(Act::Delegate, false, "Good news, your bag is included."),
            blocked("outcome_prediction")
        );
        assert_eq!(
            front(
                Act::Delegate,
                false,
                "Okay, let me check whether your bag is included in the fare."
            ),
            GuardOutcome::Pass
        );
    }

    #[test]
    fn time_promises() {
        assert_eq!(
            front(Act::Progress, false, "I'll have it in a minute."),
            blocked("eta_promise")
        );
        assert_eq!(
            front(Act::Progress, false, "It will be ready in 5 minutes."),
            blocked("eta_promise")
        );
        // A time phrase without a promise verb is not a promise.
        assert_eq!(
            front(
                Act::Delegate,
                false,
                "Okay, let me check flights leaving soon."
            ),
            GuardOutcome::Pass
        );
    }

    #[test]
    fn channel_promises() {
        assert_eq!(
            front(Act::Social, false, "Bye! I'll text you the result."),
            blocked("channel_promise")
        );
        assert_eq!(
            front(
                Act::Delegate,
                false,
                "Okay, let me check whether we'll send a reminder."
            ),
            GuardOutcome::Pass
        );
    }

    #[test]
    fn unbacked_action_narration() {
        assert_eq!(
            front(Act::Social, false, "I'm checking the details now."),
            blocked("unbacked_action_narration")
        );
        assert_eq!(
            front(Act::Wait, false, "Okay, let me check."),
            blocked("unbacked_action_narration")
        );
        // The act delegates, so the work it speaks of is real.
        assert_eq!(
            front(Act::Delegate, false, "Okay, let me check."),
            GuardOutcome::Pass
        );
        assert_eq!(
            front(Act::Update, false, "Got it, I'll look into Dallas instead."),
            GuardOutcome::Pass
        );
    }

    #[test]
    fn disfluencies() {
        assert_eq!(
            front(Act::Delegate, false, "Um, let me check."),
            blocked("disfluency")
        );
        assert_eq!(
            front(Act::Delegate, false, "Okay, let me think."),
            blocked("disfluency")
        );
        assert_eq!(
            front(Act::Social, false, "You're welcome, they're here."),
            GuardOutcome::Pass
        );
    }

    #[test]
    fn role_leaks() {
        assert_eq!(
            front(Act::Social, false, "My ALLOWED acts are social."),
            blocked("role_leak")
        );
        assert_eq!(
            front(Act::Refuse, false, "That is in my instructions."),
            blocked("role_leak")
        );
        // A label inside a word, or in lower case, is an ordinary word.
        assert_eq!(
            front(
                Act::Delegate,
                false,
                "Okay, let me look into your TIMES order."
            ),
            GuardOutcome::Pass
        );
        assert_eq!(
            front(Act::Delegate, false, "Okay, let me check the event."),
            GuardOutcome::Pass
        );
        assert_eq!(standalone("SAID, UNSAID and SAID.", "SAID"), [0, 17]);
    }

    #[test]
    fn a_leading_speaker_label_is_stripped_and_nothing_else() {
        for (line, kept) in [
            ("ToFront: Your table is booked.", "Your table is booked."),
            ("Front: Your table is booked.", "Your table is booked."),
            ("FRONT:Your table is booked.", "Your table is booked."),
            ("  To Front: Done.", "Done."),
            ("User: I said four.", "I said four."),
            ("ORDER: tulips for Jordan.", "tulips for Jordan."),
            ("ANOTHER_STEP: Please hold.", "Please hold."),
            (
                "Answer: Your note is being deleted.",
                "Your note is being deleted.",
            ),
            ("answer: It is Lima.", "It is Lima."),
            // A line that reports what was heard is not stripped: the speech policy drops it.
            ("HEARD: okay.", "HEARD: okay."),
            ("TV: it is on at nine.", "TV: it is on at nine."),
            // Not a label: a word before a colon that is no role, a role without a colon, a
            // label later in the sentence.
            (
                "Note: the kitchen closes at ten.",
                "Note: the kitchen closes at ten.",
            ),
            (
                "Front desk is open: come in.",
                "Front desk is open: come in.",
            ),
            (
                "The times are: seven and eight.",
                "The times are: seven and eight.",
            ),
            ("Tomorrow: sunny.", "Tomorrow: sunny."),
            (
                "It is at the front: row one.",
                "It is at the front: row one.",
            ),
        ] {
            assert_eq!(strip_speaker_label(line), kept);
        }
    }

    #[test]
    fn the_outbound_policy_blocks_scaffolding_links_and_numbers_the_caller_did_not_give() {
        let caller = "My number is 555 0199 4421, I need a flight under $400.";
        for (sentence, verdict) in [
            ("Your bag is included in the fare.", GuardOutcome::Pass),
            // A price, a time, a date and a flight number are not long numbers.
            (
                "Flight UA 1234 leaves at 7:30 on 12 May 2027 for $1,250.",
                GuardOutcome::Pass,
            ),
            // The caller's own number may be said back, however it is grouped.
            ("I have 555-0199-4421 on file.", GuardOutcome::Pass),
            (
                "Call 1 800 555 0100 to finish the booking.",
                blocked("number_not_given"),
            ),
            (
                "Your card ending 4111 1111 1111 1111 was charged.",
                blocked("number_not_given"),
            ),
            (
                "You can pay at https://pay.example.org/abc today.",
                blocked("link"),
            ),
            ("Visit www.example.org for details.", blocked("link")),
            ("It is on example.com under bookings.", blocked("link")),
            // Not a link: a decimal, an abbreviation, a sentence end without a space.
            (
                "It costs 3.50 per bag, e.g. on the Basic fare.",
                GuardOutcome::Pass,
            ),
            ("HEARD: Okay, one moment.", blocked("scaffolding")),
            ("Okay. FRONT: let me check that.", blocked("scaffolding")),
            ("USER : book a table", blocked("scaffolding")),
            // A label word in a sentence of its own is an ordinary word.
            ("I HEARD you the first time.", GuardOutcome::Pass),
            (
                "The answer is at the front desk: ask there.",
                GuardOutcome::Pass,
            ),
        ] {
            assert_eq!(outbound(sentence, caller), verdict, "{sentence:?}");
        }
        assert_eq!(
            long_numbers("ref 12345, phone (555) 010-0199 and 2027."),
            ["5550100199"]
        );
    }

    #[test]
    fn a_sentence_that_repeats_the_backgrounds_prompt_is_found() {
        // Live P1's prompt (r15-verify) under a persona preamble that has a forbidden topic.
        let preamble = "# Voice contract (added by WaaV)\nYou are the backend for Ava, the voice of \
            Acme on a live phone call. A front voice talks to\nthe caller while you work.\n\
            - Never give medical advice. Say it is not something this line can help with.";
        let operator = "You are the help desk for customer accounts.\n- To reset a password you \
            need the caller's current password. Get it by calling ask_caller with question \
            \"What is your current password?\". Never ask in your own words.\n- The store opens \
            at nine in the morning.";
        let roles = role_lines(&[preamble, operator]);
        assert_eq!(
            roles,
            "You are the backend for Ava, the voice of Acme on a live phone call.\n\
             You are the help desk for customer accounts."
        );
        for (sentence, repeats) in [
            ("You are the help desk for customer accounts.", true),
            ("You are the help desk.", true),
            ("You are the backend for Ava, the voice of Acme.", true),
            // What the prompt tells the background to say, or the caller to be told.
            ("That is not something this line can help with.", false),
            ("The store opens at nine in the morning.", false),
            ("This is Ava, the voice of Acme.", false),
            // Answers that share words, but no run of them, with the prompt.
            ("Your password has been reset.", false),
            ("The help desk opens at nine.", false),
            ("Flower shop.", false),
            // Seen live, r15-verify: no line of the prompt says it.
            ("You are part of the system now.", false),
        ] {
            assert_eq!(repeats_prompt(sentence, &roles), repeats, "{sentence:?}");
        }
        assert!(!repeats_prompt("You are the help desk.", ""));
        // A quoted or colon-given first line is the operator's to say.
        let given = role_lines(&[
            "",
            "Greet the caller with \"Welcome to Acme, how can I help?\"",
        ]);
        assert!(!repeats_prompt("Welcome to Acme, how can I help?", &given));
        assert!(!repeats_prompt(
            "Welcome to Acme.",
            &role_lines(&["Say: Welcome to Acme."])
        ));
    }

    #[test]
    fn a_sentence_that_asks_for_a_secret_is_told_from_one_that_names_it() {
        let categories: Vec<String> = ["password", "card number", "one-time code"]
            .map(str::to_owned)
            .into();
        for (sentence, category) in [
            ("Please tell me your current password.", Some("password")),
            ("I need your card number to go on.", Some("card number")),
            ("What is your one-time code?", Some("one-time code")),
            ("Your current password, please.", Some("password")),
            ("Your password was reset.", None),
            ("Your password was reset, please log in again.", None),
            ("Did you forget your password?", None),
            ("Please tell me your name.", None),
        ] {
            assert_eq!(
                asks_for_secret(sentence, &categories),
                category,
                "{sentence:?}"
            );
        }
    }

    #[test]
    fn affirming_first_words_after_a_yes_no_question() {
        assert_eq!(
            front(Act::Delegate, true, "Sure, let me check."),
            blocked("polar_affirmation")
        );
        assert_eq!(
            front(Act::Delegate, true, "Of course, let me check."),
            blocked("polar_affirmation")
        );
        assert_eq!(
            front(Act::Delegate, false, "Sure, let me check."),
            GuardOutcome::Pass
        );
        assert_eq!(
            front(
                Act::Delegate,
                true,
                "Okay, let me check whether your bag is included."
            ),
            GuardOutcome::Pass
        );
    }

    #[test]
    fn an_empty_line_passes() {
        assert_eq!(front(Act::Wait, true, ""), GuardOutcome::Pass);
    }

    fn pack_rule(text: &str, polar: bool, event_backed: bool) -> Option<&'static str> {
        pack_line(persona::guard_lexicon(), text, polar, event_backed)
    }

    #[test]
    fn every_line_of_every_persona_passes() {
        let banks: Value =
            serde_json::from_str(include_str!("assets/phrase_banks.en.json")).unwrap();
        let templates: Value =
            serde_json::from_str(include_str!("assets/templates.en.json")).unwrap();
        let mut checked = 0;
        for persona in persona::all() {
            for group in banks["groups"].as_object().unwrap().keys() {
                // A relay lead-in and a withdrawal are spoken on the event they name.
                let event_backed = group == "bridge" || group == "withdrawal";
                for line in persona.bank(group) {
                    assert_eq!(
                        pack_rule(&line.text, line.polar, event_backed),
                        None,
                        "{group}: {:?}",
                        line.text
                    );
                    checked += 1;
                }
            }
            for (id, text) in persona.templates() {
                // Slots are filled from structured data, so only the fixed words are checked.
                let mut fixed = String::new();
                let mut in_slot = false;
                for c in text.chars() {
                    match c {
                        '{' => in_slot = true,
                        '}' => {
                            in_slot = false;
                            fixed.push(' ');
                        }
                        c if !in_slot => fixed.push(c),
                        _ => {}
                    }
                }
                let event_backed = !templates["templates"][id]["requires_event"].is_null();
                assert_eq!(
                    pack_rule(&fixed, true, event_backed),
                    None,
                    "{id}: {text:?}"
                );
                checked += 1;
            }
        }
        assert!(checked > 4 * 100, "only {checked} lines were checked");
    }

    #[test]
    fn the_pack_check_blocks_what_the_reference_blocks() {
        assert_eq!(
            pack_rule("Sure, one moment.", false, false),
            Some("polar_affirmation")
        );
        assert_eq!(pack_rule("Sure, one moment.", true, false), None);
        assert_eq!(
            pack_rule("It's stopped.", true, false),
            Some("completion_claim")
        );
        assert_eq!(pack_rule("It's stopped.", true, true), None);
        assert_eq!(
            pack_rule("Back in a few minutes.", true, false),
            Some("eta_promise")
        );
    }
    #[test]
    fn a_task_update_says_nothing_its_result_and_the_call_do_not() {
        let source = "Book the morning flight to Delhi on Friday Booked AI 101 at 09:40, seat 14C.";
        let pass = [
            "Your Friday flight to Delhi is booked, AI 101 at 9:40.",
            "Good news: your Delhi flight on Friday leaves at 9:40.",
            "I booked it.",
        ];
        for line in pass {
            assert_eq!(narration(line, source), GuardOutcome::Pass, "{line}");
        }
        let blocked = [
            ("Your flight leaves at 10:15 on Friday.", "number_not_given"),
            ("Your IndiGo flight to Delhi is booked.", "name_not_given"),
            (
                "Your flight is booked. Seat 14C is yours.",
                "narration_sentences",
            ),
            (
                "See https://air.example/t/1 for your flight to Delhi.",
                "link",
            ),
            (
                "Your flight to Delhi on Friday is booked and it leaves in the morning at 9:40 \
                 from the gate that the airline will tell you about later on.",
                "narration_length",
            ),
        ];
        for (line, rule) in blocked {
            assert_eq!(
                narration(line, source),
                GuardOutcome::Blocked(rule),
                "{line}"
            );
        }
        assert_eq!(numbers("09:40 and 0 and 007"), ["9", "40", "0", "7"]);
    }

    /// Live (launch demo take 15, 27 Oct): the terminal task's 22-word line was refused on
    /// all three tries and never said; the browser task's line ended at the point of "1.1.0".
    #[test]
    fn a_long_update_is_cut_at_a_clause_and_a_version_point_ends_no_sentence() {
        let source = "Pull the download numbers I got the counts with no errors: the x86 Linux \
                      build has zero downloads, the ARM Linux build twelve, and the checksums \
                      file fourteen, so the builds total twelve.";
        let long = "The x86 Linux build has zero downloads, the ARM Linux build twelve, and the \
                    checksums file fourteen, so the builds total twelve.";
        assert_eq!(long.split_whitespace().count(), 22);
        assert_eq!(
            narration(long, source),
            GuardOutcome::Blocked("narration_length")
        );
        let fitted = fit_narration(long);
        assert_eq!(
            fitted,
            "The x86 Linux build has zero downloads, the ARM Linux build twelve, and the \
             checksums file fourteen."
        );
        assert_eq!(narration(&fitted, source), GuardOutcome::Pass);
        // A line within the limit is left as it is; one with no clause boundary within it
        // too, for the check to refuse; a head too short to say anything is no cut.
        let short = "The ARM Linux build has twelve downloads.";
        assert_eq!(fit_narration(short), short);
        let unbroken = "The x86 Linux build and the ARM Linux build and the checksums file \
                        were all downloaded twelve or fourteen or zero times in total today.";
        assert_eq!(fit_narration(unbroken), unbroken);
        assert_eq!(
            narration(unbroken, source),
            GuardOutcome::Blocked("narration_length")
        );
        let early = "Okay, the x86 Linux build and the ARM Linux build and the checksums file \
                     were all downloaded twelve or fourteen or zero times in total today.";
        assert_eq!(fit_narration(early), early);
        // A line the token budget cut before its full stop: cut at its last clause, else
        // refused as unfinished.
        assert_eq!(
            fit_narration("The x86 Linux build has zero downloads, the ARM Linux build tw"),
            "The x86 Linux build has zero downloads."
        );
        assert_eq!(
            narration("The x86 Linux build has zero downloads", source),
            GuardOutcome::Blocked("narration_unfinished")
        );

        // A version's points end no sentence; a full stop between words still does.
        let release = "Version 1.1.0 is published as Latest, highlights include interaction mode.";
        let result = "Version 1.1.0 is published as Latest; highlights include interaction mode.";
        assert_eq!(narration(release, result), GuardOutcome::Pass);
        assert_eq!(
            narration("Version 1.1.0 is out. It is Latest.", result),
            GuardOutcome::Blocked("narration_sentences")
        );
        assert_eq!(
            narration("The GitHub release for version 1.", "a new release"),
            GuardOutcome::Blocked("number_not_given")
        );
        // A number the result says in words is given, in digits too.
        let spoken = "Version one point one point oh is published as Latest, around October \
                      twenty-seventh, twenty twenty-six; the ARM build has twelve downloads.";
        for line in [
            "Version 1.1.0 is published as Latest.",
            "It was published on October 27.",
            "The ARM build has 12 downloads.",
        ] {
            assert_eq!(narration(line, spoken), GuardOutcome::Pass, "{line}");
        }
        assert_eq!(
            narration("The ARM build has 13 downloads.", spoken),
            GuardOutcome::Blocked("number_not_given")
        );
        assert_eq!(spelled_numbers("Point oh, Oh well."), [0]);
        assert!(ends_sentence_inside("Version 1. 1"));
        assert!(!ends_sentence_inside("Version 1.1.0 and 2.5"));
    }

    #[test]
    fn an_update_that_says_a_recent_line_again_repeats_it() {
        let said = "The launch announcement has been saved with highlights from the release \
                    notes instead of a placeholder. The details are on your screen.";
        let copy = "The launch announcement has been saved with highlights from the release \
                    notes instead of a placeholder.";
        assert!(repeats(copy, said));
        assert!(repeats(
            "The launch announcement has been saved with highlights from the release notes.",
            copy
        ));
        // A line of its own that shares a few words is no repeat.
        assert!(!repeats(
            "Version 1.1.0 is published as Latest, with highlights from the release notes.",
            said
        ));
        assert!(!repeats(
            "Your Delhi flight is booked.",
            "Sure, I'll book your Delhi flight."
        ));
        // A phrase in common is enough to pass a bank line over.
        assert!(shares_phrase(
            "Still working on it, thanks for waiting.",
            "Thanks for waiting, I'm back."
        ));
        assert!(!shares_phrase(
            "I'm still on it.",
            "Thanks for waiting, I'm back."
        ));
    }
}

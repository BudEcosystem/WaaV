//! What the gateway reads off a caller turn besides its meaning: whether the caller asked a
//! yes/no question (so no acknowledgement affirms it), whether the words are the agent's own
//! heard back, the language, sensitive values, repeated requests and spoken titles. What the
//! caller meant (a request, a cancel, a stop, a repeat, a continuer, side talk) is the front
//! model's to decide in context (chapter 9): the lexicon's phrase lists are prompt examples
//! and test data only.

use std::collections::BTreeSet;

#[cfg(test)]
use super::persona::Lexicon;
use super::persona::{self, Persona, has_phrase, norm_phrase, word_list};
use super::types::{Board, Ms};

/// Words skipped before the first word of a question.
const DISCOURSE_WORDS: [&str; 20] = [
    "so", "and", "oh", "sorry", "um", "uh", "well", "hey", "but", "also", "okay", "ok", "quick",
    "question", "actually", "yeah", "hi", "hello", "then", "just",
];
const WH_WORDS: [&str; 9] = [
    "what", "when", "where", "which", "who", "whom", "whose", "why", "how",
];
/// The words that carry a request: not stop words, at least three letters, and no number
/// words or digits (a spoken value is not a request). Test data (see [`near_duplicate`]).
#[cfg(test)]
fn content_words(text: &str, lexicon: &Lexicon) -> BTreeSet<String> {
    let numbers = &persona::guard_lexicon().normalisation;
    word_list(text)
        .into_iter()
        .filter(|word| {
            word.chars().count() >= 3
                && !word.starts_with(|c: char| c.is_ascii_digit())
                && !lexicon.content_word_stop_list.contains(word)
                && ![
                    &numbers.units,
                    &numbers.tens,
                    &numbers.scales,
                    &numbers.ordinals,
                ]
                .iter()
                .any(|table| table.contains_key(word))
        })
        .collect()
}

/// The words of a text after its leading discourse words.
fn after_discourse(text: &str) -> Vec<String> {
    let words = word_list(text);
    let skip = words
        .iter()
        .take_while(|word| DISCOURSE_WORDS.contains(&word.as_str()))
        .count();
    words[skip..].to_vec()
}

/// Whether the caller asked a yes/no question: a clause that starts with a yes/no starter and
/// ends the utterance (or the utterance ends in a question mark), or "know if", "tell me
/// whether". "Can you <verb>" is a request, not a question about a fact.
pub fn detect_polar_question(text: &str) -> bool {
    let starters = &persona::guard_lexicon()
        .polar_affirmation
        .polar_question_starters;
    // Clauses end at punctuation and at the whole words "and" and "or".
    let lower = persona::straighten(text).to_lowercase();
    let mut clauses = vec![String::new()];
    let mut word = String::new();
    for c in lower.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        if word == "and" || word == "or" {
            clauses.push(String::new());
        } else if let Some(clause) = clauses.last_mut() {
            clause.push_str(&word);
        }
        word.clear();
        if ",.?!;".contains(c) {
            clauses.push(String::new());
        } else if let Some(clause) = clauses.last_mut() {
            clause.push(c);
        }
    }
    clauses.retain(|clause| !clause.trim().is_empty());
    let asked = text.trim().ends_with('?');
    let last = clauses.len().saturating_sub(1);
    for (index, clause) in clauses.iter().enumerate() {
        let words = after_discourse(clause);
        let request = words.len() > 1
            && ["can", "could", "would", "will"].contains(&words[0].as_str())
            && words[1] == "you";
        if request {
            continue;
        }
        if words.first().is_some_and(|word| starters.contains(word)) && (asked || index == last) {
            return true;
        }
    }
    let norm = norm_phrase(text);
    ["know", "tell me", "check"].iter().any(|verb| {
        ["if", "whether"]
            .iter()
            .any(|link| norm.contains(&format!(" {verb} {link} ")))
    })
}

/// What the front's request knows about the call besides the caller's words.
#[derive(Debug, Clone)]
pub struct RuleCtx<'a> {
    pub now: Ms,
    /// `false` when the caller was talking to someone else.
    pub addressed: bool,
    /// The speech-to-text confidence was too low to act on.
    pub unclear: bool,
    /// A question is open or the answer window is (the policy's `answer_expected`).
    pub answer_expected: bool,
    pub board: &'a Board,
    /// Agent lines with the time their playout ended, for the self-echo check.
    pub recent_agent_lines: &'a [(Ms, String)],
    /// The words came while the agent spoke (an interim over it).
    pub over_agent: bool,
}

/// The facts of one caller utterance the front's request carries: none of them is what the
/// caller meant.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleView {
    pub addressed: bool,
    pub unclear: bool,
    /// A yes/no question: the acknowledgement must not affirm it ("Sure!").
    pub polar_question: bool,
    pub pending_question: bool,
    /// Most of the words are those of an agent line that just ended: its echo.
    pub self_echo: bool,
    pub answer_window: bool,
}

/// How long after an agent line ended one or two of its words heard in a row are still taken
/// for its echo (the line that plays "ends now").
const SHORT_ECHO_MS: Ms = 1_500;

/// A word as the echo check compares it: a spoken number as its digits, so the recogniser's
/// "10" is the agent's "ten".
fn echo_word(word: &str) -> String {
    const NUMBERS: [&str; 12] = [
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
        "twelve",
    ];
    match NUMBERS.iter().position(|number| *number == word) {
        Some(at) => (at + 1).to_string(),
        None => word.to_owned(),
    }
}

pub fn rule_view(text: &str, ctx: &RuleCtx<'_>, _persona: &Persona) -> RuleView {
    let cue_table = &persona::guard_lexicon().rule_layer_cues;
    let words = word_list(text);
    // Self echo: most of the words are those of an agent line that ended in the last seconds.
    let heard: Vec<String> = words
        .iter()
        .filter(|word| !["um", "uh"].contains(&word.as_str()))
        .map(|word| echo_word(word))
        .collect();
    let window = cue_table.self_echo_window_s * 1000;
    // One or two words over the agent are too few to count: they are its echo when the line
    // that plays (or just ended) has them in a row (live S4: the interims "while i" and "10"
    // over "While I look that up ... until ten in the evening" cut the agent with no echo
    // defence). Not in a turn of its own: "Aisle." answers "Window or aisle?".
    let short = ctx.over_agent && !heard.is_empty() && heard.len() < 3;
    let self_echo = (short || heard.len() >= 3)
        && ctx
            .recent_agent_lines
            .iter()
            .filter(|(ended, _)| ctx.now.saturating_sub(*ended) <= window)
            .any(|(ended, line)| {
                let agent: Vec<String> = word_list(line).iter().map(|w| echo_word(w)).collect();
                if short {
                    return ctx.now.saturating_sub(*ended) <= SHORT_ECHO_MS
                        && agent
                            .windows(heard.len())
                            .any(|run| run == heard.as_slice());
                }
                let shared = heard.iter().filter(|word| agent.contains(word)).count();
                shared as f64 / heard.len() as f64 >= cue_table.self_echo_threshold
            });
    RuleView {
        addressed: ctx.addressed,
        unclear: ctx.unclear,
        polar_question: detect_polar_question(text),
        pending_question: ctx.board.open_question.is_some(),
        self_echo,
        answer_window: ctx.answer_expected,
    }
}

// ---- language, sensitive values, repeated requests, spoken titles

/// Whether a caller turn is in another language than the session's pack by the recogniser's
/// own reading (`detected` is its language field: a code such as `es` or `en-US`, or a name
/// such as `english`). The words themselves are the front's to read (`wait` `foreign`): no
/// word list or script check decides (owner, 24 Oct).
pub fn foreign_language(detected: Option<&str>) -> bool {
    // A code (`en-US`) or a name (`english`) starts with the pack's language.
    let pack = persona::language();
    detected.is_some_and(|language| {
        let language = language.trim().to_ascii_lowercase();
        !language.is_empty() && !language.starts_with(pack)
    })
}

/// A digit, or a number word below ten ("four one one one").
fn digit_like(word: &str) -> usize {
    let digits = word.chars().filter(char::is_ascii_digit).count();
    let spoken = persona::guard_lexicon()
        .normalisation
        .units
        .contains_key(word);
    digits + usize::from(spoken)
}

/// The sensitive category a text names (a card number, a password, a one-time code).
pub fn sensitive_category<'a>(text: &str, categories: &'a [String]) -> Option<&'a str> {
    let norm = norm_phrase(text);
    // Also in the plural: live P1 (r15-verify) heard "my card number is 4111 ..." as "my card
    // numbers 4111 ...", and the digits were taken by voice.
    let named = categories.iter().filter(|category| {
        has_phrase(&norm, category) || has_phrase(&norm, &format!("{category}s"))
    });
    // "card number" before "card": the longest name says the most.
    named
        .max_by_key(|category| category.len())
        .map(String::as_str)
}

/// The sensitive category of a value the caller is saying out loud: the turn names the
/// category and carries at least three digits. Such a value is never taken by voice.
pub fn sensitive_value<'a>(text: &str, categories: &'a [String]) -> Option<&'a str> {
    sensitive_category(text, categories).filter(|_| says_digits(text))
}

/// The text carries at least three digits, written or spoken.
pub fn says_digits(text: &str) -> bool {
    word_list(text)
        .iter()
        .map(|word| digit_like(word))
        .sum::<usize>()
        >= 3
}

/// Words that a spoken title adds and a request does not carry.
#[cfg(test)]
const TITLE_WORDS: [&str; 2] = ["request", "question"];

/// Whether a new request nearly repeats a task: they share at least two content words, and
/// three quarters of the shorter one's. Numbers are no content words, so "a table for two"
/// and "a table for four" repeat each other. Test data only: whether a request repeats a task
/// is the front's to read in context (chapter 9).
#[cfg(test)]
pub fn near_duplicate(text: &str, title: &str, lexicon: &Lexicon) -> bool {
    let words = |text: &str| {
        let mut words = content_words(text, lexicon);
        words.retain(|word| !TITLE_WORDS.contains(&word.as_str()));
        words
    };
    let (new, old) = (words(text), words(title));
    let shared = new.intersection(&old).count();
    shared >= 2 && shared * 4 >= new.len().min(old.len()) * 3
}

/// The shortest description word that can name an agent ("downloads", "announcements"):
/// shorter ones ("check", "notes", "pages") are said of any agent's work.
const AGENT_WORD_MIN_CHARS: usize = 6;

/// The other agent of the session the caller's words name, when a finished task of `agent`
/// (its title `title`) is the one the front named a follow-up of: by its registered name
/// ("terminal"), or by a word of its description no other agent's description has and the
/// finished task's agent and title do not ("download" for an agent that "downloads,
/// verifies checksums"), plurals read as their singular. `None` when they name no other agent, or
/// only the finished task's own. `topics`: the session's agents and their descriptions.
///
/// Live (launch demo take 23, 28 Oct): with the browser's release check finished, "Great.
/// Can you also get the download numbers in the terminal?" was named a follow-up of it; the
/// reading apart had no label in its 1.5 s on the loaded GPU, so the front's word stood, and
/// the browser agent was asked for the download numbers (it asked which project was meant).
/// No terminal task started until the caller asked again 110 s later.
pub fn other_agent_named<'a>(
    text: &str,
    agent: &str,
    title: &str,
    topics: &'a [(String, String)],
) -> Option<&'a str> {
    let stem = |word: &str| -> String {
        let plural = word.len() > 4 && word.ends_with('s') && !word.ends_with("ss");
        if plural {
            word[..word.len() - 1].to_owned()
        } else {
            word.to_owned()
        }
    };
    let words =
        |text: &str| -> BTreeSet<String> { word_list(text).iter().map(|w| stem(w)).collect() };
    let said = words(text);
    let norm = norm_phrase(text);
    let own: BTreeSet<String> = topics
        .iter()
        .filter(|(name, _)| name == agent)
        .flat_map(|(_, what)| words(what))
        .chain(words(title))
        .chain(words(agent))
        .collect();
    topics
        .iter()
        .filter(|(name, _)| name != agent)
        .find(|(name, what)| {
            if has_phrase(&norm, name) {
                return true;
            }
            let others = topics.iter().filter(|(other, _)| other != name);
            let elsewhere: BTreeSet<String> = others.flat_map(|(_, what)| words(what)).collect();
            words(what).into_iter().any(|word| {
                word.chars().count() >= AGENT_WORD_MIN_CHARS
                    && !own.contains(&word)
                    && !elsewhere.contains(&word)
                    && said.contains(&word)
            })
        })
        .map(|(name, _)| name.as_str())
}

/// What a caller says before the request itself, longest first.
const LEAD_INS: [&str; 19] = [
    "i would like you to",
    "i would like to",
    "can you please",
    "could you please",
    "i'd like you to",
    "i need you to",
    "i want you to",
    "i would like",
    "i'd like to",
    "i need to",
    "i want to",
    "could you",
    "would you",
    "i'd like",
    "can you",
    "will you",
    "i need",
    "i want",
    "please",
];
/// The caller's words about themselves, as the agent says them back.
const SECOND_PERSON: [(&str, &str); 9] = [
    ("i", "you"),
    ("me", "you"),
    ("my", "your"),
    ("mine", "yours"),
    ("am", "are"),
    ("i'm", "you're"),
    ("i've", "you've"),
    ("i'd", "you'd"),
    ("i'll", "you'll"),
];
const DETERMINERS: [&str; 11] = [
    "a", "an", "the", "my", "your", "our", "their", "some", "any", "this", "that",
];

/// Words a cut title must not end on.
const HANGING: [&str; 17] = [
    "for", "at", "on", "in", "to", "of", "the", "a", "an", "and", "or", "with", "from", "by",
    "about", "your", "next",
];

/// A task title (the start of the caller's words) as a phrase that can follow "working on":
/// "Can you check if my bag is included" becomes "your request to check if your bag is
/// included", "I need a flight to Denver" becomes "a flight to Denver", and a question becomes
/// "your question, what time does the store open".
pub fn spoken_title(title: &str) -> String {
    let raw: Vec<&str> = title.split_whitespace().collect();
    let norm = word_list(title);
    if raw.len() != norm.len() {
        // A word of punctuation only: the words cannot be lined up, so nothing is quoted.
        return "your request".to_owned();
    }
    let mut from = 0;
    loop {
        let rest = &norm[from..];
        let discourse = rest
            .first()
            .is_some_and(|word| DISCOURSE_WORDS.contains(&word.as_str()));
        let lead_in = LEAD_INS
            .iter()
            .map(|lead_in| word_list(lead_in))
            .find(|lead_in| rest.starts_with(lead_in));
        match (lead_in, discourse) {
            (Some(lead_in), _) => from += lead_in.len(),
            (None, true) => from += 1,
            (None, false) => break,
        }
    }
    let Some(first) = norm.get(from) else {
        return "your request".to_owned();
    };
    let starters = &persona::guard_lexicon()
        .polar_affirmation
        .polar_question_starters;
    let question = first
        .split('\'')
        .next()
        .is_some_and(|stem| WH_WORDS.contains(&stem))
        || starters.contains(first);
    let noun_phrase = DETERMINERS.contains(&first.as_str());
    let words: Vec<String> = raw[from..]
        .iter()
        .zip(&norm[from..])
        .enumerate()
        .map(|(index, (raw, norm))| {
            let swapped = SECOND_PERSON.iter().find(|(first, _)| first == norm);
            match swapped {
                Some((_, second)) => (*second).to_owned(),
                // The title no longer starts a sentence.
                None if index == 0 => raw.to_lowercase(),
                None => (*raw).to_owned(),
            }
        })
        .collect();
    // A title is cut at a length: a word left hanging at its end is not said.
    let mut words = words;
    while words.len() > 1
        && words
            .last()
            .is_some_and(|last| HANGING.contains(&last.to_lowercase().as_str()))
    {
        words.pop();
    }
    let words = words.join(" ");
    let words = words.trim_end_matches(['.', ',', '?', '!']);
    if question {
        format!("your question, {words}")
    } else if noun_phrase {
        words.to_owned()
    } else {
        format!("your request to {words}")
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::{Effects, TaskId, TaskRef, TaskState, TaskView};
    use super::*;

    const NOW: Ms = 1_000_000;

    fn task(alias: &str, state: TaskState, title: &str) -> TaskView {
        TaskView {
            id: TaskId(format!("task_{alias}")),
            task_ref: TaskRef(alias.to_owned()),
            revision: 1,
            title: title.to_owned(),
            state,
            cancelling: false,
            effects: Effects::NoneCommitted,
            open_question: None,
            side_effects: false,
            started_at: 0,
            lineage: Default::default(),
        }
    }

    fn working() -> Board {
        Board {
            tasks: vec![task("t1", TaskState::Working, "your flight to Lisbon")],
            ..Board::default()
        }
    }

    fn ctx<'a>(board: &'a Board, recent: &'a [(Ms, String)]) -> RuleCtx<'a> {
        RuleCtx {
            now: NOW,
            addressed: true,
            unclear: false,
            answer_expected: false,
            board,
            recent_agent_lines: recent,
            over_agent: false,
        }
    }

    fn travel() -> std::sync::Arc<Persona> {
        Persona::load("northwind-travel-ava")
    }

    #[test]
    fn polar_questions_are_detected() {
        for text in [
            "Is my bag included?",
            "so, is it refundable",
            "I booked it yesterday, and is it confirmed",
            "can you check if my bag is included in the fare",
            "I need to know whether I can add a bag",
            "Okay, yes, do that.",
        ] {
            assert!(detect_polar_question(text), "{text}");
        }
        for text in [
            "Can you book a flight to Denver?",
            "Where's my refund?",
            "I need a hotel in Paris",
            "Is it Friday, I think so anyway",
        ] {
            assert!(!detect_polar_question(text), "{text}");
        }
    }

    #[test]
    fn self_echo_needs_a_recent_agent_line() {
        let board = working();
        let persona = travel();
        let text = "Okay, let me check your booking to Lisbon";
        let line = "Okay, let me check your booking to Lisbon.".to_owned();
        let recent = [(NOW - 1_000, line.clone())];
        let view = rule_view(text, &ctx(&board, &recent), &persona);
        assert!(view.self_echo);
        let old = [(NOW - 6_000, line)];
        assert!(!rule_view(text, &ctx(&board, &old), &persona).self_echo);
    }

    /// Live S4: one or two words of the line that plays, heard back over it, are its echo;
    /// a cue that is not in the line is not, nor a short reply in a turn of its own.
    #[test]
    fn one_or_two_words_over_the_agent_are_its_echo_when_the_line_has_them_in_a_row() {
        let board = working();
        let persona = travel();
        let line = "While I look that up, our kitchen is open until ten in the evening.";
        let playing = [(NOW, line.to_owned())];
        let mut over = ctx(&board, &playing);
        over.over_agent = true;
        for echo in ["while i", "10", "Ten in"] {
            assert!(rule_view(echo, &over, &persona).self_echo, "{echo}");
        }
        for cue in ["Stop, stop.", "Enough.", "i while", "wait"] {
            assert!(!rule_view(cue, &over, &persona).self_echo, "{cue}");
        }
        let ended = [(NOW - 3_000, line.to_owned())];
        let mut later = ctx(&board, &ended);
        later.over_agent = true;
        assert!(!rule_view("while i", &later, &persona).self_echo);
        // In a turn of its own a short reply is the caller's.
        assert!(!rule_view("evening", &ctx(&board, &playing), &persona).self_echo);
    }

    /// Only the recogniser's own language field names another language here; the words are
    /// the front's to read (no word list, owner, 24 Oct).
    #[test]
    fn another_language_is_named_by_the_recogniser() {
        assert!(!foreign_language(None));
        for field in ["en", "en-US", "english", "English", ""] {
            assert!(!foreign_language(Some(field)), "{field:?}");
        }
        for field in ["es", "hi-IN", "spanish"] {
            assert!(foreign_language(Some(field)), "{field:?}");
        }
    }

    #[test]
    fn a_sensitive_value_names_a_category_and_carries_digits() {
        let persona = Persona::load("default");
        let categories = persona.sensitive_categories();
        for (text, category) in [
            ("my card number is 4111 1111 1111 1111", Some("card number")),
            // A recogniser's "number is" heard as "numbers" (live P1).
            ("my card numbers 4111 1111 1111 1111", Some("card number")),
            ("the passwords are 4417 and 2210", Some("password")),
            ("the security code is one two three", Some("security code")),
            ("my PIN is 4 4 1 7", Some("pin")),
            ("the one-time code is 889 201", Some("one-time code")),
            // The category without a value, and a value without the category.
            ("my card number is not working", None),
            ("I forgot my password", None),
            ("a table for 4 at 7 on the 12th", None),
        ] {
            assert_eq!(sensitive_value(text, categories), category, "{text:?}");
        }
        assert_eq!(
            sensitive_category("What is your account password?", categories),
            Some("password")
        );
        assert_eq!(
            sensitive_category("Which day works for you?", categories),
            None
        );
    }

    #[test]
    fn a_near_duplicate_shares_most_of_its_content_words() {
        let persona = Persona::load("default");
        let lexicon = persona.lexicon();
        let table = "Book a table for two at Rosa's on Saturday";
        for (text, title, repeats) in [
            ("Book a table for four at Rosa's on Saturday", table, true),
            (table, table, true),
            (
                "check flights to Denver on Saturday",
                "Can you check flights to Denver on Friday",
                true,
            ),
            (
                "What is the capital of Germany?",
                "What is the capital of France",
                false,
            ),
            (
                "Can you check if my bag is included in the fare?",
                table,
                false,
            ),
            (
                "Can you check the availability at Rosa's for Saturday evening?",
                table,
                false,
            ),
            ("Yes please", table, false),
        ] {
            assert_eq!(near_duplicate(text, title, lexicon), repeats, "{text:?}");
        }
    }

    #[test]
    fn a_title_is_spoken_as_a_phrase() {
        for (title, spoken) in [
            (
                "Can you check if my bag is included in the fare",
                "your request to check if your bag is included in the fare",
            ),
            (
                "Hi, I need a flight from Boston to Denver next Friday",
                "a flight from Boston to Denver next Friday",
            ),
            (
                "Please find me a hotel in Paris for two nights",
                "your request to find you a hotel in Paris for two nights",
            ),
            (
                "I'd like to change my seat to an aisle seat",
                "your request to change your seat to an aisle seat",
            ),
            (
                "What is the capital of France",
                "your question, what is the capital of France",
            ),
            ("Is my bag included", "your question, is your bag included"),
            (
                "Can you check the availability at the Olive Garden for",
                "your request to check the availability at the Olive Garden",
            ),
            ("your flight search", "your flight search"),
            ("a flight to Denver", "a flight to Denver"),
            ("Okay so please", "your request"),
            ("", "your request"),
        ] {
            assert_eq!(spoken_title(title), spoken, "{title:?}");
        }
    }

    /// What the launch demo's desk agents say they do.
    fn desk() -> Vec<(String, String)> {
        [
            (
                "browser",
                "Operates a real web browser: opens pages, reads them, follows links, and \
                 reports what it found.",
            ),
            (
                "terminal",
                "Runs shell commands on a sandboxed Linux machine with internet access: \
                 downloads, verifies checksums, unpacks, starts and health-checks programs.",
            ),
            (
                "writer",
                "Drafts documents such as announcements, emails and notes, and saves them in \
                 the word processor on the caller's screen after the caller approves.",
            ),
        ]
        .into_iter()
        .map(|(name, what)| (name.to_owned(), what.to_owned()))
        .collect()
    }

    /// Live take 23: "... the download numbers in the terminal?" after the browser's release
    /// check was handed to the browser agent.
    #[test]
    fn words_that_name_another_agent_are_not_a_follow_up_of_this_one() {
        let desk = desk();
        let title = "Check the GitHub release: Open";
        let named = |text: &str, agent: &str| other_agent_named(text, agent, title, &desk);
        // By its name, or by a word only its description has.
        for (text, other) in [
            (
                "Great. Can you also get the download numbers in the terminal?",
                "terminal",
            ),
            ("Oh, and grab the downloads too.", "terminal"),
            (
                "Let's draft the launch announcement and mention those numbers.",
                "writer",
            ),
            ("Can the writer put that in an email?", "writer"),
        ] {
            assert_eq!(named(text, "browser"), Some(other), "{text}");
        }
        // About the release check itself, thanks, a status question: no other agent.
        for text in [
            "Does the release mention interaction mode?",
            "Which files does it list?",
            "Can you open the release notes and read me the highlights?",
            "Can you check it again?",
            "How's it looking?",
            "Thanks, Eva.",
            "Can the browser open the release page?",
        ] {
            assert_eq!(named(text, "browser"), None, "{text}");
        }
        // The finished task's own agent, and words of its own description or title, are not
        // another agent's.
        assert_eq!(
            other_agent_named(
                "Can the terminal run it again?",
                "terminal",
                "Pull the numbers",
                &desk
            ),
            None
        );
        let mut overlap = desk.clone();
        overlap[0].1.push_str(" It downloads pages too.");
        assert_eq!(
            other_agent_named("Grab the downloads.", "browser", title, &overlap),
            None,
            "a word the finished task's agent also does"
        );
        assert_eq!(
            other_agent_named("Grab the terminal output.", "browser", title, &[]),
            None
        );
    }
}

//! The persona pack: the embedded phrase banks, templates and lexicon, and the words they
//! become.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use regex::Regex;
use serde::Deserialize;

use super::clips::FailureLine;
use super::config::{BackchannelTiming, FrontConfig, Intensity};
use super::guard;
use super::health::LineKind;
use super::types::{Act, HitlKind, HitlRequest, Pace, Say, Verbosity};

/// The id of the persona that speaks the base banks unchanged.
pub const DEFAULT_ID: &str = "default";

/// The intent of a `Say::Bank` that tells the persona the caller asked a yes/no question, so a
/// line that opens by affirming ("Sure, ...") is skipped.
pub const POLAR_QUESTION: &str = "polar_question";

/// The embedded personas; `default` is built in code from the base banks.
const PERSONA_FILES: [&str; 3] = [
    include_str!("assets/personas/travel_northwind.json"),
    include_str!("assets/personas/clinic_harbourview.json"),
    include_str!("assets/personas/retail_larkspur.json"),
];

/// The template families this build speaks; the rest of `templates.en.json` belongs to
/// releases that are not built.
const TEMPLATE_FAMILIES: [&str; 8] = [
    "confirm.",
    "cancel.",
    "hitl.",
    "status.",
    "update.",
    "question.",
    "followup.",
    "control.",
];
const TEMPLATE_IDS: [&str; 19] = [
    "offer.result",
    "sensitive.dtmf",
    "choose",
    "value",
    "choice_offer",
    "sensitive.screen",
    "sensitive.dont_say",
    "auth_url.before",
    "auth_url.after",
    "approval_needed",
    "failure.retryable",
    "failure.with_effects",
    "failure.effects_unknown",
    "stale_outcome",
    "presence",
    "duplicate.ask",
    "sensitive.decline",
    "repeat_request",
    "regime_c_correction",
];

/// The `withdrawal` lines for a caller's amendment, the only reason this build withdraws an
/// answer with a bank line.
const WITHDRAWAL_SITUATION: &str = "caller_amended";
/// The bank group of the pre-made lines said when a service fails (plan chapter 10).
pub(super) const FAILURE_GROUP: &str = "failure";

/// The word lists the deterministic layers share (`guard_lexicon.en.json` `rule_layer_cues`
/// and `copy_spans.dangling_end_words`, `templates.en.json` `answer_lexicon_for_validator`).
/// Phrases are lower-case.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lexicon {
    pub continuers: Vec<String>,
    pub cancel_cues: Vec<String>,
    pub repeat_cues: Vec<String>,
    pub status_cues: Vec<String>,
    pub amendment_cues: Vec<String>,
    pub request_cues: Vec<String>,
    pub side_talk_cues: Vec<String>,
    pub stop_talking_cues: Vec<String>,
    /// Intent name (`greeting`, `thanks`, `goodbye`, `presence`, `identity`, `how_are_you`)
    /// and its phrases.
    pub social_intents: Vec<(String, Vec<String>)>,
    pub content_word_stop_list: Vec<String>,
    pub dangling_end_words: Vec<String>,
    pub strong_accept: Vec<String>,
    pub weak_accept: Vec<String>,
    pub decline: Vec<String>,
    pub cancel: Vec<String>,
}

// ---- whole-word phrase matching

/// Curly quotes and long dashes as their plain forms, so one spelling matches.
pub fn straighten(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201c}' | '\u{201d}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            other => other,
        })
        .collect()
}

/// A character of a script written without spaces between its words (Chinese, Japanese, Thai,
/// Lao, Khmer, Myanmar, Tibetan). Each such letter is a word of its own, so a phrase in one of
/// these scripts matches as a run of characters.
pub(super) fn unspaced(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0E00..=0x0FFF // Thai, Lao, Tibetan
            | 0x1000..=0x109F // Myanmar
            | 0x1780..=0x17FF // Khmer
            | 0x3005..=0x3007 // 々 〆 〇
            | 0x3040..=0x30FF // Hiragana, Katakana
            | 0x31F0..=0x31FF // Katakana phonetic extensions
            | 0x3400..=0x4DBF // CJK extension A
            | 0x4E00..=0x9FFF // CJK unified ideographs
            | 0xF900..=0xFAFF // CJK compatibility ideographs
            | 0xFF66..=0xFF9F // half-width Katakana
            | 0x20000..=0x3134F // CJK extensions B to G
    )
}

/// A combining mark (an accent written apart, a vowel sign, a virama): part of its word.
fn mark(c: char) -> bool {
    static MARK: OnceLock<Regex> = OnceLock::new();
    !c.is_ascii()
        && MARK
            .get_or_init(|| Regex::new(r"^\p{M}$").expect("a mark class is a regex"))
            .is_match(c.encode_utf8(&mut [0; 4]))
}

/// A letter, digit or mark of any script: what words are made of. The letters of a script
/// without spaces are words on their own (see [`unspaced`]).
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || mark(c)
}

/// A letter of a spaced script (not a digit), any script: what an apostrophe suffix and a
/// label's neighbours are judged by.
pub(super) fn spaced_letter(c: char) -> bool {
    !unspaced(c) && (c.is_alphabetic() || mark(c))
}

/// The lower-case words of a text, an apostrophe suffix kept with its word ("don't").
/// Letters and digits of every script count, accents kept; each letter of a script without
/// spaces is a word (see [`unspaced`]).
pub fn word_list(text: &str) -> Vec<String> {
    let chars: Vec<char> = straighten(text).to_lowercase().chars().collect();
    let in_word = |c: char| word_char(c) && !unspaced(c);
    let mut words = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if unspaced(chars[i]) && word_char(chars[i]) {
            words.push(chars[i].to_string());
            i += 1;
            continue;
        }
        if !in_word(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && in_word(chars[i]) {
            i += 1;
        }
        if i + 1 < chars.len() && chars[i] == '\'' && spaced_letter(chars[i + 1]) {
            i += 1;
            while i < chars.len() && spaced_letter(chars[i]) {
                i += 1;
            }
        }
        words.push(chars[start..i].iter().collect());
    }
    words
}

/// Lower-case, punctuation to spaces, one space on each side: the form phrases are matched in.
/// Letters and digits of every script are kept, accents too; each letter of a script without
/// spaces stands apart, as in [`word_list`].
pub fn norm_phrase(text: &str) -> String {
    let mut spaced = String::with_capacity(text.len() + 2);
    for c in straighten(text).to_lowercase().chars() {
        if unspaced(c) && word_char(c) {
            spaced.push(' ');
            spaced.push(c);
            spaced.push(' ');
        } else if word_char(c) || "'$\u{20ac}\u{a3}\u{20b9}%:-".contains(c) {
            spaced.push(c);
        } else {
            spaced.push(' ');
        }
    }
    format!(
        " {} ",
        spaced.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

/// Whether `phrase` occurs as whole words in a text already passed through [`norm_phrase`].
/// A phrase with no word in it (punctuation only) is found nowhere.
pub fn has_phrase(norm: &str, phrase: &str) -> bool {
    let phrase = norm_phrase(phrase);
    !phrase.trim().is_empty() && norm.contains(&phrase)
}

pub fn any_phrase(norm: &str, phrases: &[String]) -> bool {
    phrases.iter().any(|phrase| has_phrase(norm, phrase))
}

// ---- the packaged files

/// The parts of `guard_lexicon.en.json` the rule router, the fact guard and the grammar
/// builder read.
#[derive(Debug, Deserialize)]
pub(super) struct GuardLexicon {
    pub normalisation: Normalisation,
    pub completion_claims: Phrases,
    pub outcome_predictions: Phrases,
    pub polar_affirmation: PolarAffirmation,
    pub eta_promises: EtaPromises,
    pub channel_promises: Phrases,
    pub action_narration: Phrases,
    pub disfluency: Phrases,
    pub role_leak: RoleLeak,
    pub inquiry_frames: Phrases,
    pub copy_spans: CopySpans,
    pub sanitiser: Sanitiser,
    pub language_check: LanguageCheck,
    pub sensitive_categories: SensitiveCategories,
    pub rule_layer_cues: RuleLayerCues,
    /// Its completion-claim and time-promise patterns, compiled at first use (`guard.rs`).
    #[serde(skip)]
    pub compiled: OnceLock<(Vec<Regex>, Vec<Regex>)>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Normalisation {
    pub units: BTreeMap<String, u64>,
    pub tens: BTreeMap<String, u64>,
    pub scales: BTreeMap<String, u64>,
    pub ordinals: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Phrases {
    pub phrases: Vec<String>,
    /// Regular expressions, matched on lower-case text.
    #[serde(default)]
    pub patterns: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PolarAffirmation {
    pub first_words: Vec<String>,
    pub polar_question_starters: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct EtaPromises {
    pub phrases: Vec<String>,
    pub patterns: Vec<String>,
    pub promise_verbs: Vec<String>,
    /// Time phrases a pack line may not hold, promise verb or not.
    pub strict_bank_phrases: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RoleLeak {
    pub labels_case_sensitive: Vec<String>,
    pub phrases: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CopySpans {
    pub max_span_words: usize,
    pub max_utterance_words: usize,
    pub pronoun_turn: BTreeMap<String, String>,
    pub pronoun_turn_pairs: BTreeMap<String, String>,
    pub second_person_never_copied: Vec<String>,
    pub restate_stop_words: Vec<String>,
    pub restate_stop_suffixes: Vec<String>,
    pub start_word_stop_list: Vec<String>,
    pub dangling_end_words: Vec<String>,
    pub allowed_word_pattern: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Sanitiser {
    pub special_tokens: Vec<String>,
    pub removed_characters: Vec<String>,
    pub reserved_labels: Vec<String>,
    pub truncate_chars: TruncateChars,
}

#[derive(Debug, Deserialize)]
pub(super) struct TruncateChars {
    pub caller_utterance: usize,
    pub task_title: usize,
}

#[derive(Debug, Deserialize)]
pub(super) struct LanguageCheck {
    /// Written in lower case at the start of a sentence in a copy span. Another language is
    /// the front's reading (`wait` `foreign`), never a word list (owner, 24 Oct).
    pub english_function_words: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SensitiveCategories {
    pub categories: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RuleLayerCues {
    pub continuers: Vec<String>,
    pub social_intents: BTreeMap<String, Vec<String>>,
    pub status_cues: Vec<String>,
    pub cancel_cues: Vec<String>,
    pub repeat_cues: Vec<String>,
    pub amendment_cues: Vec<String>,
    pub request_cues: Vec<String>,
    pub side_talk_cues: Vec<String>,
    pub stop_talking_cues: Vec<String>,
    pub content_word_stop_list: Vec<String>,
    pub self_echo_threshold: f64,
    pub self_echo_window_s: u64,
    /// A recogniser confidence below this makes a turn unclear.
    pub unclear_confidence: f64,
}

#[derive(Debug, Deserialize)]
struct BanksFile {
    rules: BankRules,
    groups: BTreeMap<String, BankGroup>,
}

#[derive(Debug, Deserialize)]
struct BankRules {
    ack_clip_max_chars: usize,
}

#[derive(Debug, Deserialize)]
struct BankGroup {
    acts: Vec<Act>,
    lines: Vec<BankLine>,
    /// `backchannel`: the clip id of the line that replaces a clip failing its checks.
    #[serde(default)]
    fallback: Option<String>,
    /// `backchannel`: the emitter's `medium` numbers in this language (English's when absent).
    #[serde(default)]
    timing: Option<BackchannelTiming>,
}

/// One line or fragment of a phrase bank.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct BankLine {
    pub text: String,
    /// The line opens by affirming or denying.
    #[serde(default)]
    pub polar: bool,
    /// The social intent of the line, or its situation within the group.
    #[serde(default, alias = "subsituation")]
    pub intent: Option<String>,
    /// The front acts whose grammar may use the line; the group's acts when the line names none.
    #[serde(default)]
    pub acts: Vec<Act>,
    /// Spoken by the policy only, never offered to the model.
    #[serde(default, rename = "w4_only")]
    pub policy_only: bool,
    /// The clip id of a listening sound.
    #[serde(default)]
    pub clip: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TemplatesFile {
    answer_lexicon_for_validator: AnswerLexicon,
    templates: BTreeMap<String, Template>,
}

#[derive(Debug, Deserialize)]
struct AnswerLexicon {
    strong_accept: Vec<String>,
    weak_accept: Vec<String>,
    decline: Vec<String>,
    cancel: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Template {
    text: String,
    /// Spoken only on the event it names, so it may state that event.
    #[serde(default)]
    requires_event: Option<serde_json::Value>,
    #[serde(default)]
    slots: BTreeMap<String, SlotSpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct SlotSpec {
    #[serde(default)]
    optional: bool,
    #[serde(default)]
    default: Option<String>,
}

/// A persona file (`persona.schema.json`); a missing field takes the default persona's value.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct Spec {
    id: String,
    name: String,
    org: String,
    role: String,
    channel: String,
    language: String,
    tone: String,
    formality: String,
    max_words: u32,
    answer_words: u32,
    backend_capabilities: Vec<String>,
    delegate_when: Vec<String>,
    do_not_delegate_when: Vec<String>,
    forbidden_topics: Vec<String>,
    social_scope: Vec<String>,
    backchannel_intensity: Option<String>,
    /// Clip ids of the `backchannel` bank the persona's listening sounds are drawn from.
    backchannel_clips: Vec<String>,
    interruption_notes: Vec<String>,
    sensitive: Sensitive,
    banks: BTreeMap<String, Vec<BankLine>>,
    template_overrides: BTreeMap<String, String>,
    protected_terms: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Sensitive {
    extra_categories: Vec<String>,
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            id: DEFAULT_ID.to_owned(),
            name: "the assistant".to_owned(),
            org: "this help line".to_owned(),
            role: "general assistant".to_owned(),
            channel: "phone".to_owned(),
            language: "en-US".to_owned(),
            tone: "warm".to_owned(),
            formality: "informal".to_owned(),
            max_words: 15,
            answer_words: 40,
            backend_capabilities: Vec::new(),
            delegate_when: Vec::new(),
            do_not_delegate_when: Vec::new(),
            forbidden_topics: Vec::new(),
            social_scope: Vec::new(),
            backchannel_intensity: None,
            backchannel_clips: Vec::new(),
            interruption_notes: Vec::new(),
            sensitive: Sensitive::default(),
            banks: BTreeMap::new(),
            template_overrides: BTreeMap::new(),
            protected_terms: Vec::new(),
        }
    }
}

/// One language's phrase banks, templates and lexicon, parsed once, and its personas.
pub(super) struct Pack {
    /// The language it speaks: `en`, or the primary subtag of a pack loaded from the
    /// operator's directory (`hi`).
    language: String,
    guard: GuardLexicon,
    banks: BanksFile,
    templates: TemplatesFile,
    lexicon: Lexicon,
    personas: Vec<Arc<Persona>>,
}

impl Pack {
    fn new(
        language: &str,
        guard: GuardLexicon,
        banks: BanksFile,
        templates: TemplatesFile,
        specs: Vec<Spec>,
    ) -> Pack {
        let lexicon = lexicon(&guard, &templates.answer_lexicon_for_validator);
        let personas = specs
            .into_iter()
            .map(|spec| Persona::compile(spec, language, &banks, &templates, &guard, &lexicon))
            .map(Arc::new)
            .collect();
        Pack {
            language: language.to_owned(),
            guard,
            banks,
            templates,
            lexicon,
            personas,
        }
    }
}

/// The embedded English pack with the packaged personas.
fn english() -> Pack {
    let guard: GuardLexicon = serde_json::from_str(include_str!("assets/guard_lexicon.en.json"))
        .expect("guard_lexicon.en.json is the packaged lexicon");
    let banks: BanksFile = serde_json::from_str(include_str!("assets/phrase_banks.en.json"))
        .expect("phrase_banks.en.json is the packaged bank file");
    let templates: TemplatesFile = serde_json::from_str(include_str!("assets/templates.en.json"))
        .expect("templates.en.json is the packaged template file");
    let specs = PERSONA_FILES.iter().map(|file| {
        serde_json::from_str::<Spec>(file).expect("an embedded persona file is a persona")
    });
    let specs = std::iter::once(Spec::default()).chain(specs).collect();
    Pack::new("en", guard, banks, templates, specs)
}

/// The pack of `<dir>/{guard_lexicon,phrase_banks,templates}.<language>.json`, held to the
/// English pack's checks: every template this build speaks and every bank group is there,
/// the patterns are regular expressions, and every bank line and template passes the fact
/// guard's pack check under the pack's own lexicon. Its one persona is the default persona.
fn load_pack(dir: &Path, language: &str, english: &Pack) -> Result<Pack, String> {
    fn read<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Result<T, String> {
        let text = std::fs::read_to_string(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))
    }
    let guard: GuardLexicon = read(dir, &format!("guard_lexicon.{language}.json"))?;
    let banks: BanksFile = read(dir, &format!("phrase_banks.{language}.json"))?;
    let templates: TemplatesFile = read(dir, &format!("templates.{language}.json"))?;
    let patterns = guard.completion_claims.patterns.iter();
    if let Some(bad) = patterns
        .chain(&guard.eta_promises.patterns)
        .find(|source| Regex::new(source).is_err())
    {
        return Err(format!(
            "guard_lexicon.{language}.json: {bad:?} is not a pattern"
        ));
    }
    if let Some(group) = english
        .banks
        .groups
        .keys()
        .find(|g| !banks.groups.contains_key(*g))
    {
        return Err(format!("phrase_banks.{language}.json has no group {group}"));
    }
    let listening = &banks.groups["backchannel"];
    if let Some(id) = &listening.fallback
        && !listening
            .lines
            .iter()
            .any(|line| line.clip.as_ref() == Some(id))
    {
        return Err(format!(
            "backchannel: fallback {id} is not a clip of the group"
        ));
    }
    if let Some(timing) = &listening.timing
        && (timing.skip_every == 1 || timing.gap_ms == 0)
    {
        return Err("backchannel: timing skips every pause or has no gap".to_owned());
    }
    failure_problem(&banks.groups[FAILURE_GROUP].lines).map_or(Ok(()), Err)?;
    // The templates this build speaks (the English default persona's).
    let spoken = || english.personas[0].templates.keys();
    if let Some(id) = spoken().find(|id| !templates.templates.contains_key(*id)) {
        return Err(format!("templates.{language}.json has no template {id}"));
    }
    let default = Spec {
        language: language.to_owned(),
        ..Spec::default()
    };
    let pack = Pack::new(language, guard, banks, templates, vec![default]);
    // The words its persona speaks, checked as `guard.rs` checks the English ones.
    let persona = &pack.personas[0];
    for group in pack.banks.groups.keys() {
        let event_backed = group == "bridge" || group == "withdrawal";
        for line in persona.bank(group) {
            let rule = guard::pack_line(&pack.guard, &line.text, line.polar, event_backed);
            if let Some(rule) = rule {
                return Err(format!("{group}: {:?} breaks {rule}", line.text));
            }
        }
    }
    for (id, template) in &persona.templates {
        let event_backed = pack.templates.templates[id].requires_event.is_some();
        let fixed = fixed_words(&template.text);
        if let Some(rule) = guard::pack_line(&pack.guard, &fixed, true, event_backed) {
            return Err(format!("{id}: {:?} breaks {rule}", template.text));
        }
    }
    Ok(pack)
}

/// A `failure` line with its id (`clip`) and kind (`subsituation`); `None` without them.
fn failure_line(line: &BankLine) -> Option<FailureLine> {
    let id = line.clip.clone().filter(|id| !id.is_empty())?;
    let kind = LineKind::parse(line.intent.as_deref()?)?;
    Some(FailureLine {
        id,
        kind,
        text: line.text.clone(),
    })
}

fn failure_lines(lines: &[BankLine]) -> Vec<FailureLine> {
    lines.iter().filter_map(failure_line).collect()
}

/// What makes a pack's `failure` group unusable: a line without its id or kind, or a kind
/// with no line.
fn failure_problem(lines: &[BankLine]) -> Option<String> {
    if let Some(bad) = lines.iter().find(|line| failure_line(line).is_none()) {
        return Some(format!(
            "failure: {:?} needs a clip id and a subsituation",
            bad.text
        ));
    }
    let lines = failure_lines(lines);
    let missing = LineKind::ALL
        .into_iter()
        .find(|kind| !lines.iter().any(|line| line.kind == *kind))?;
    Some(format!("failure: no {} line", missing.as_str()))
}

/// Every pack of `dir` (one per `templates.<language>.json`, English excepted), and what was
/// wrong with the ones left out.
fn load_packs(dir: &Path, english: &Pack) -> (Vec<Pack>, Vec<String>) {
    let mut languages: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let language = name.strip_prefix("templates.")?.strip_suffix(".json")?;
            let ok = !language.is_empty()
                && language != "en"
                && language.chars().all(|c| c.is_ascii_lowercase());
            ok.then(|| language.to_owned())
        })
        .collect();
    languages.sort();
    let mut problems = Vec::new();
    let packs = languages
        .iter()
        .filter_map(|language| match load_pack(dir, language, english) {
            Ok(pack) => Some(pack),
            Err(problem) => {
                problems.push(format!("language pack {language} left out: {problem}"));
                None
            }
        })
        .collect();
    (packs, problems)
}

/// English, then the packs of the operator's `interaction.packs_dir`, read once.
fn packs() -> &'static [Pack] {
    static PACKS: OnceLock<Vec<Pack>> = OnceLock::new();
    PACKS.get_or_init(|| {
        let mut all = vec![english()];
        if let Some(dir) = super::config::server().packs_dir.clone() {
            let (more, problems) = load_packs(Path::new(&dir), &all[0]);
            for problem in problems {
                tracing::warn!(%dir, "{problem}");
            }
            for pack in &more {
                tracing::info!(%dir, language = %pack.language, "language pack loaded");
            }
            all.extend(more);
        }
        all
    })
}

tokio::task_local! {
    /// The pack of the session whose actor runs on this task.
    static SESSION_PACK: &'static Pack;
}

/// The pack of the session this task runs (see [`scope`]); English elsewhere.
fn pack() -> &'static Pack {
    SESSION_PACK.try_with(|pack| *pack).unwrap_or(&packs()[0])
}

/// The pack that speaks a session language (`stt_config.language`): English for none, `en*`
/// or a recogniser that names the language itself (`multi`, `auto`: each turn's detected
/// language is then checked against the pack), else a loaded pack whose language is the
/// primary subtag (`hi-IN` -> `hi`).
pub fn pack_of(language: &str) -> Option<&'static str> {
    let language = language.trim().to_ascii_lowercase();
    if language.is_empty()
        || language.starts_with("en")
        || language == "multi"
        || language == "auto"
    {
        return Some("en");
    }
    let primary = language.split(['-', '_']).next().unwrap_or_default();
    packs()
        .iter()
        .find(|pack| pack.language == primary)
        .map(|pack| pack.language.as_str())
}

/// The rows of a pack's lexicon the front model is shown as examples of the caller's words
/// (chapter 9: the phrase lists are prompt examples, no word decides): its key and the act
/// its words stand for.
fn example_rows(cues: &RuleLayerCues) -> [(&'static str, &'static str, &[String]); 8] {
    [
        ("stop_talking_cues", "control stop", &cues.stop_talking_cues),
        ("repeat_cues", "control repeat", &cues.repeat_cues),
        ("cancel_cues", "abort", &cues.cancel_cues),
        ("status_cues", "progress", &cues.status_cues),
        ("amendment_cues", "update", &cues.amendment_cues),
        ("request_cues", "delegate", &cues.request_cues),
        ("continuers", "wait continuer", &cues.continuers),
        ("side_talk_cues", "wait side_talk", &cues.side_talk_cues),
    ]
}

/// The words of each row the front is shown.
const EXAMPLES_PER_ROW: usize = 4;

/// The front prompt's lines on a pack of another language than English: the caller's language
/// and the pack's words for the acts, so the (multilingual) front reads the caller's language
/// with the English instructions. Empty for English, whose prompt has its own examples.
fn language_examples(pack_language: &str, name: &str, cues: &RuleLayerCues) -> String {
    if pack_language == "en" {
        return String::new();
    }
    let rows: Vec<String> = example_rows(cues)
        .into_iter()
        .filter(|(_, _, words)| !words.is_empty())
        .map(|(_, act, words)| {
            let words: Vec<String> = words
                .iter()
                .take(EXAMPLES_PER_ROW)
                .map(|word| format!("\u{ab}{word}\u{bb}"))
                .collect();
            format!("  {act}: {}", words.join(", "))
        })
        .collect();
    format!(
        "- The caller speaks {name}; read their words in {name}. Their words for some acts, as \
         examples only (decide from the context):\n{}",
        rows.join("\n")
    )
}

/// What the pack of `language` lacks for the front model: the key of the first lexicon row
/// the front is shown as examples that has no words. `None` when it has them all, for English
/// (its prompt has examples of its own) and for a language without a pack.
pub fn front_gap(language: &str) -> Option<&'static str> {
    let pack = packs().iter().find(|pack| pack.language == language)?;
    pack.front_gap()
}

impl Pack {
    /// See [`front_gap`].
    fn front_gap(&self) -> Option<&'static str> {
        if self.language == "en" {
            return None;
        }
        example_rows(&self.guard.rule_layer_cues)
            .into_iter()
            .find(|(_, _, words)| words.is_empty())
            .map(|(key, _, _)| key)
    }
}

/// Runs a session's work with its pack's lexicon as the one the rules and the guard read.
pub fn scope<F: Future>(language: &str, work: F) -> impl Future<Output = F::Output> + use<F> {
    let pack = packs()
        .iter()
        .find(|pack| pack.language == language)
        .unwrap_or(&packs()[0]);
    SESSION_PACK.scope(pack, work)
}

/// The language of the pack in use: the session's (see [`scope`]), else `en`.
pub fn language() -> &'static str {
    &pack().language
}

/// The guard lexicon of the pack in use.
pub(super) fn guard_lexicon() -> &'static GuardLexicon {
    &pack().guard
}

fn lexicon(guard: &GuardLexicon, answers: &AnswerLexicon) -> Lexicon {
    let cues = &guard.rule_layer_cues;
    Lexicon {
        continuers: cues.continuers.clone(),
        cancel_cues: cues.cancel_cues.clone(),
        repeat_cues: cues.repeat_cues.clone(),
        status_cues: cues.status_cues.clone(),
        amendment_cues: cues.amendment_cues.clone(),
        request_cues: cues.request_cues.clone(),
        side_talk_cues: cues.side_talk_cues.clone(),
        stop_talking_cues: cues.stop_talking_cues.clone(),
        social_intents: cues.social_intents.clone().into_iter().collect(),
        content_word_stop_list: cues.content_word_stop_list.clone(),
        dangling_end_words: guard.copy_spans.dangling_end_words.clone(),
        strong_accept: answers.strong_accept.clone(),
        weak_accept: answers.weak_accept.clone(),
        decline: answers.decline.clone(),
        cancel: answers.cancel.clone(),
    }
}

/// `front_system_prompt.md`; in the unit tests the file `WAAV_FRONT_PROMPT_FILE` names when
/// it is set, so the live gate sets can try a prompt without a rebuild.
fn prompt_source() -> &'static str {
    #[cfg(test)]
    {
        static TRIED: OnceLock<Option<&'static str>> = OnceLock::new();
        let tried = TRIED.get_or_init(|| {
            let text = std::fs::read_to_string(std::env::var("WAAV_FRONT_PROMPT_FILE").ok()?);
            text.ok().map(|text| &*Box::leak(text.into_boxed_str()))
        });
        if let Some(text) = tried {
            return text;
        }
    }
    include_str!("assets/front_system_prompt.md")
}

/// The text of one fenced block of `front_system_prompt.md`.
fn prompt_block(name: &str) -> &'static str {
    let open = format!("<!-- template:{name} -->");
    prompt_source()
        .split_once(&open)
        .and_then(|(_, rest)| rest.split_once("```text\n"))
        .and_then(|(_, body)| body.split_once("\n```"))
        .map(|(body, _)| body)
        .expect("front_system_prompt.md holds the named template block")
}

/// Fill `{{slot}}`s. A line that is only a slot with an empty value is removed.
fn render_slots(template: &str, slots: &BTreeMap<String, String>) -> String {
    let mut lines = Vec::new();
    for line in template.lines() {
        let alone = line
            .trim()
            .strip_prefix("{{")
            .and_then(|rest| rest.strip_suffix("}}"));
        if alone.is_some_and(|name| slots.get(name).is_some_and(String::is_empty)) {
            continue;
        }
        let mut out = line.to_owned();
        for (name, value) in slots {
            out = out.replace(&format!("{{{{{name}}}}}"), value);
        }
        lines.push(out);
    }
    lines.join("\n")
}

fn bullets(items: &[String], lead: &str) -> String {
    items
        .iter()
        .map(|item| format!("- {lead}{}.", item.trim_end_matches('.')))
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct Persona {
    id: String,
    /// The language of its pack.
    language: String,
    /// The persona's listening sounds and their clip ids, and how often it makes them
    /// (`backchannel_intensity`; `None` is the line's default).
    backchannel: Vec<String>,
    backchannel_ids: Vec<String>,
    backchannel_intensity: Option<Intensity>,
    /// The pack's fallback listening sound (clip id, words) and its emitter numbers.
    backchannel_fallback: Option<(String, String)>,
    backchannel_timing: BackchannelTiming,
    answer_words: u32,
    banks: BTreeMap<String, Vec<BankLine>>,
    templates: BTreeMap<String, Template>,
    lexicon: Lexicon,
    terms: Vec<String>,
    social_scope: Vec<String>,
    sensitive_categories: Vec<String>,
    front_system: String,
    call_header: String,
    background_preamble: String,
}

impl Persona {
    /// The persona with this id; an unknown id gives the default persona.
    pub fn load(id: &str) -> Arc<Persona> {
        Self::load_in(id, language())
    }

    /// The persona with this id in the pack of `language` (see [`pack_of`]); an id the pack
    /// does not have gives its default persona, and a language without a pack English.
    pub fn load_in(id: &str, language: &str) -> Arc<Persona> {
        let all = packs();
        let pack = all
            .iter()
            .find(|p| p.language == language)
            .unwrap_or(&all[0]);
        let personas = &pack.personas;
        let found = personas.iter().find(|persona| persona.id == id);
        Arc::clone(found.unwrap_or(&personas[0]))
    }

    /// The language of the persona's pack.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Whether a packaged persona has this id.
    pub fn packaged(id: &str) -> bool {
        pack().personas.iter().any(|persona| persona.id == id)
    }

    /// A persona given as an object (`persona.schema.json`), compiled like the packaged ones
    /// and held to the same checks: a known id shape, known bank groups, and every bank line
    /// and template wording passing the pack check of the fact guard with its slots kept.
    /// The error names the first thing that fails.
    pub fn from_spec(value: &serde_json::Value) -> Result<Arc<Persona>, String> {
        let spec: Spec = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let id_ok = !spec.id.is_empty()
            && spec.id.len() <= 64
            && spec
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
        if !id_ok || spec.id == DEFAULT_ID {
            return Err(
                "id must be 1 to 64 letters, digits, '_', '.' or '-', and not default".into(),
            );
        }
        for (field, text) in [
            ("name", &spec.name),
            ("org", &spec.org),
            ("role", &spec.role),
        ] {
            if text.trim().is_empty() || text.chars().count() > 80 || text.contains(['{', '}']) {
                return Err(format!("{field} must be 1 to 80 characters with no braces"));
            }
        }
        let all = packs();
        let pack = pack_of(&spec.language)
            .and_then(|language| all.iter().find(|p| p.language == language))
            .unwrap_or(&all[0]);
        for (group, lines) in &spec.banks {
            if !pack.banks.groups.contains_key(group) {
                return Err(format!("banks.{group} is not a bank group"));
            }
            if lines.is_empty() {
                return Err(format!("banks.{group} has no lines"));
            }
            if group == FAILURE_GROUP
                && let Some(bad) = lines.iter().find(|line| failure_line(line).is_none())
            {
                return Err(format!(
                    "banks.failure: {:?} needs a clip id and a subsituation of {}",
                    bad.text,
                    LineKind::ALL.map(LineKind::as_str).join(", ")
                ));
            }
            // A relay lead-in and a withdrawal are spoken on the event they name.
            let event_backed = group == "bridge" || group == "withdrawal";
            for line in lines {
                let text = line.text.replace("{{org}}", "").replace("{{name}}", "");
                if text.trim().is_empty() || text.chars().count() > 200 || text.contains(['{', '}'])
                {
                    return Err(format!("banks.{group}: {:?} is not a line", line.text));
                }
                if let Some(rule) = guard::pack_line(&pack.guard, &text, line.polar, event_backed) {
                    return Err(format!("banks.{group}: {:?} breaks {rule}", line.text));
                }
                // An acknowledgement is a clip: short enough not to hold up a ready answer.
                let most = pack.banks.rules.ack_clip_max_chars;
                if group == "ack" && line.text.chars().count() > most {
                    return Err(format!(
                        "banks.ack: {:?} is over {most} characters",
                        line.text
                    ));
                }
            }
        }
        for (id, text) in &spec.template_overrides {
            let Some(base) = pack.templates.templates.get(id) else {
                return Err(format!("template_overrides.{id} is not a template"));
            };
            if slots(text) != slots(&base.text) {
                return Err(format!(
                    "template_overrides.{id} must keep the slots of {:?}",
                    base.text
                ));
            }
            let event_backed = id.starts_with("cancel.confirmed") || id == "stale_outcome";
            let fixed = fixed_words(text);
            if let Some(rule) = guard::pack_line(&pack.guard, &fixed, true, event_backed) {
                return Err(format!("template_overrides.{id}: {text:?} breaks {rule}"));
            }
        }
        if let Some(clip) = spec.backchannel_clips.iter().find(|clip| {
            !pack.banks.groups["backchannel"]
                .lines
                .iter()
                .any(|l| l.clip.as_ref() == Some(clip))
        }) {
            return Err(format!(
                "backchannel_clips: {clip} is not a clip of the backchannel bank"
            ));
        }
        Ok(Arc::new(Persona::compile(
            spec,
            &pack.language,
            &pack.banks,
            &pack.templates,
            &pack.guard,
            &pack.lexicon,
        )))
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The words of the persona's listening sounds; none when it makes none.
    pub fn backchannel_lines(&self) -> &[String] {
        if self.backchannel_intensity == Some(Intensity::Off) {
            &[]
        } else {
            &self.backchannel
        }
    }

    /// The clip ids and words of the persona's listening sounds; none when it makes none.
    pub fn backchannel_clips(&self) -> Vec<(String, String)> {
        let ids = self.backchannel_ids.iter().cloned();
        ids.zip(self.backchannel_lines().iter().cloned()).collect()
    }

    /// The pack's fallback listening sound (clip id, words): it replaces a clip that fails
    /// its checks.
    pub fn backchannel_fallback(&self) -> Option<(String, String)> {
        self.backchannel_fallback.clone()
    }

    /// The pack's `medium` listening-sound numbers, which the intensity scales.
    pub fn backchannel_timing(&self) -> BackchannelTiming {
        self.backchannel_timing
    }

    /// How often the persona makes listening sounds; `None` leaves it to the line (web
    /// `medium`, phone `off`).
    pub fn backchannel_intensity(&self) -> Option<Intensity> {
        self.backchannel_intensity
    }

    /// The lines said when a service fails (plan chapter 10), each with its id and kind.
    pub fn failure_lines(&self) -> Vec<FailureLine> {
        failure_lines(self.bank(FAILURE_GROUP))
    }

    /// The front model's call header for this session: the persona's, then what is known
    /// about the caller (`front.caller`, caller memory).
    pub fn call_header_for(&self, front: &FrontConfig) -> String {
        let mut header = self.call_header.clone();
        if let Some(name) = front.preferred_name.as_deref() {
            header.push_str(&format!(
                "\nCaller name: «{}» (given by the application)",
                quoted(name)
            ));
        }
        let prefers = preferences(front);
        if !prefers.is_empty() {
            header.push_str(&format!("\nCaller prefers: {}", prefers.join(", ")));
        }
        if !front.memory.is_empty() {
            let facts: Vec<String> = front
                .memory
                .iter()
                .map(|f| format!("«{}»", quoted(f)))
                .collect();
            header.push_str(&format!(
                "\nKnown about the caller (from earlier calls): {}",
                facts.join("; ")
            ));
        }
        header
    }

    /// The background's `CALLER CONTEXT (data)` block for this session; empty when nothing
    /// is known about the caller.
    pub fn caller_context(&self, front: &FrontConfig) -> String {
        let mut lines = Vec::new();
        if let Some(name) = front.preferred_name.as_deref() {
            lines.push(format!("- The caller's name: {}.", quoted(name)));
        }
        match front.verbosity {
            Some(Verbosity::Brief) => {
                lines.push("- The caller prefers brief answers: at most 20 words.".to_owned())
            }
            Some(Verbosity::Detailed) => lines.push(format!(
                "- The caller prefers detailed answers: up to {} words.",
                self.answer_words * 2
            )),
            Some(Verbosity::Normal) | None => {}
        }
        match front.pace {
            Some(Pace::Slower) => lines
                .push("- The caller prefers a slower pace: short, simple sentences.".to_owned()),
            Some(Pace::Faster) => {
                lines.push("- The caller prefers a faster pace: get to the point.".to_owned())
            }
            Some(Pace::Normal) | None => {}
        }
        if !front.memory.is_empty() {
            let facts: Vec<String> = front.memory.iter().map(|fact| quoted(fact)).collect();
            lines.push(format!("- From earlier calls: {}.", facts.join("; ")));
        }
        if lines.is_empty() {
            return String::new();
        }
        format!("CALLER CONTEXT (data)\n{}", lines.join("\n"))
    }

    fn compile(
        spec: Spec,
        pack_language: &str,
        base: &BanksFile,
        templates: &TemplatesFile,
        guard: &GuardLexicon,
        lexicon: &Lexicon,
    ) -> Persona {
        let mut banks = BTreeMap::new();
        for (name, group) in &base.groups {
            let mut lines = spec.banks.get(name).unwrap_or(&group.lines).clone();
            for line in &mut lines {
                line.text = line
                    .text
                    .replace("{{org}}", &spec.org)
                    .replace("{{name}}", &spec.name);
                if line.acts.is_empty() {
                    line.acts.clone_from(&group.acts);
                }
            }
            match name.as_str() {
                "ack" => {
                    lines.retain(|line| line.text.chars().count() <= base.rules.ack_clip_max_chars)
                }
                "withdrawal" => {
                    lines.retain(|line| line.intent.as_deref() == Some(WITHDRAWAL_SITUATION))
                }
                // A persona's failure lines replace the pack's of their kinds; the kinds it
                // leaves out keep the pack's.
                FAILURE_GROUP => {
                    let kinds: Vec<Option<String>> =
                        lines.iter().map(|line| line.intent.clone()).collect();
                    let kept = group
                        .lines
                        .iter()
                        .filter(|line| !kinds.contains(&line.intent));
                    lines.extend(kept.cloned());
                }
                _ => {}
            }
            banks.insert(name.clone(), lines);
        }

        let mut spoken: BTreeMap<String, Template> = templates
            .templates
            .iter()
            .filter(|(id, _)| {
                TEMPLATE_IDS.contains(&id.as_str())
                    || TEMPLATE_FAMILIES
                        .iter()
                        .any(|family| id.starts_with(family))
            })
            .map(|(id, template)| (id.clone(), template.clone()))
            .collect();
        for (id, text) in &spec.template_overrides {
            if let Some(template) = spoken.get_mut(id) {
                template.text.clone_from(text);
            }
        }

        let social = banks.get("social").map(Vec::as_slice).unwrap_or_default();
        let refuse = banks.get("refuse").and_then(|lines| lines.first());
        // Another pack's prompt names its language ("Hindi" for `hi`); the English pack's
        // prompt is as it was.
        let language = if pack_language != "en" {
            super::controls::language_name(pack_language)
        } else if spec.language.starts_with("en") {
            "English".to_owned()
        } else {
            spec.language.clone()
        };
        let intensity = spec
            .backchannel_intensity
            .as_deref()
            .unwrap_or(if spec.channel == "web" {
                "medium"
            } else {
                "off"
            });
        let join_or = |items: &[String], empty: &str| {
            if items.is_empty() {
                empty.to_owned()
            } else {
                items.join("; ")
            }
        };
        let topics = spec
            .forbidden_topics
            .iter()
            .map(|topic| topic.trim_end_matches('.'))
            .collect::<Vec<_>>()
            .join("; ");
        let mut slots: BTreeMap<String, String> = [
            ("name", spec.name.clone()),
            ("org", spec.org.clone()),
            ("role", spec.role.clone()),
            ("channel", spec.channel.clone()),
            ("tone", spec.tone.clone()),
            ("formality", spec.formality.clone()),
            (
                "formality_sentence",
                match spec.formality.as_str() {
                    "formal" => "Polite and formal, but plain everyday words.",
                    _ => "Friendly and relaxed, plain everyday words.",
                }
                .to_owned(),
            ),
            (
                "language_examples",
                language_examples(pack_language, &language, &guard.rule_layer_cues),
            ),
            ("language_name", language),
            ("max_words", spec.max_words.to_string()),
            ("answer_words", spec.answer_words.to_string()),
            ("social_scope", join_or(&spec.social_scope, "this call")),
            (
                "refuse.first",
                refuse.map(|line| line.text.clone()).unwrap_or_default(),
            ),
            (
                "backchannel_policy",
                match intensity {
                    "low" => "Rarely. Only during a long pause in a long story.",
                    "medium" => "Now and then during a long story.",
                    "high" => "Often, while the caller tells a story.",
                    _ => "Never.",
                }
                .to_owned(),
            ),
            (
                "interruption_notes",
                spec.interruption_notes
                    .iter()
                    .map(|note| format!("- {note}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            (
                "backend_capabilities",
                join_or(&spec.backend_capabilities, "the organisation's systems"),
            ),
            (
                "delegate_when",
                bullets(&spec.delegate_when, "Also delegate when "),
            ),
            (
                "do_not_delegate_when",
                bullets(&spec.do_not_delegate_when, "Also do not delegate when "),
            ),
            (
                "refuse_when",
                if topics.is_empty() {
                    String::new()
                } else {
                    format!("- Refuse, and do not delegate, when the caller asks for: {topics}.")
                },
            ),
            (
                "never_give_line",
                if spec.forbidden_topics.is_empty() {
                    String::new()
                } else {
                    format!(
                        "- Never give {}. Say it is not something this line can help with.",
                        spec.forbidden_topics.join("; ")
                    )
                },
            ),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
        // Nothing is known about the caller before the call, so these lines are left out.
        for name in [
            "caller_name_line",
            "caller_prefers_line",
            "accessibility_line",
            "pronunciations_line",
            "memory_line",
            "caller_context_block",
        ] {
            slots.insert(name.to_owned(), String::new());
        }
        for line in social {
            if let Some(intent) = &line.intent {
                slots
                    .entry(format!("social.{intent}"))
                    .or_insert_with(|| line.text.clone());
            }
        }

        // The default persona's name and organisation are common words, not protected terms.
        let named: &[&String] = if spec.id == DEFAULT_ID {
            &[]
        } else {
            &[&spec.name, &spec.org]
        };
        let mut terms: Vec<String> = named
            .iter()
            .copied()
            .chain(&spec.protected_terms)
            .flat_map(|term| word_list(term))
            .collect();
        terms.sort();
        terms.dedup();
        let sensitive_categories = guard
            .sensitive_categories
            .categories
            .iter()
            .chain(&spec.sensitive.extra_categories)
            .map(|category| category.to_lowercase())
            .collect();

        let group = &base.groups["backchannel"];
        let clip_of = |line: &BankLine| line.clip.clone().unwrap_or_default();
        let (backchannel_ids, backchannel) = group
            .lines
            .iter()
            .filter(|line| {
                let clip = clip_of(line);
                (spec.backchannel_clips.is_empty() && !line.polar)
                    || spec.backchannel_clips.contains(&clip)
            })
            .map(|line| (clip_of(line), line.text.clone()))
            .unzip();
        let backchannel_fallback = group.fallback.as_ref().and_then(|id| {
            let line = group
                .lines
                .iter()
                .find(|line| line.clip.as_ref() == Some(id))?;
            Some((id.clone(), line.text.clone()))
        });
        Persona {
            front_system: render_slots(prompt_block("stable_prefix"), &slots),
            call_header: render_slots(prompt_block("call_header"), &slots),
            background_preamble: render_slots(prompt_block("background_preamble"), &slots),
            backchannel,
            backchannel_ids,
            backchannel_fallback,
            backchannel_timing: group.timing.unwrap_or_default(),
            backchannel_intensity: spec
                .backchannel_intensity
                .as_deref()
                .and_then(Intensity::parse),
            answer_words: spec.answer_words,
            id: spec.id,
            language: pack_language.to_owned(),
            banks,
            templates: spoken,
            lexicon: lexicon.clone(),
            terms,
            social_scope: spec.social_scope,
            sensitive_categories,
        }
    }

    /// The words of an utterance. `n` rotates a bank; `None` for [`Say::Repeat`], which the
    /// floor answers with the last line heard, for an empty bank and for a template with a
    /// missing slot.
    pub fn resolve(&self, say: &Say, n: u64) -> Option<String> {
        match say {
            Say::Bank { group, intent } => {
                let lines = self.bank_lines(group, *intent);
                let pick = (n % lines.len().max(1) as u64) as usize;
                lines.get(pick).map(|line| line.text.clone())
            }
            Say::Template { id, slots } => self.render(id, slots),
            Say::Front(text) | Say::Verbatim(text) => Some(text.clone()),
            Say::Repeat => None,
        }
    }

    /// How many lines [`Persona::resolve`] chooses among for `say`: its bank's lines for the
    /// intent; 1 for any other line.
    pub fn bank_choices(&self, say: &Say) -> usize {
        match say {
            Say::Bank { group, intent } => self.bank_lines(group, *intent).len(),
            _ => 1,
        }
    }

    /// The lines of bank `group` that may be said for `intent`.
    fn bank_lines(&self, group: &str, intent: Option<&str>) -> Vec<&BankLine> {
        self.bank(group)
            .iter()
            .filter(|line| match intent {
                Some(POLAR_QUESTION) => !line.polar && !line.policy_only,
                Some(intent) => line.intent.as_deref() == Some(intent),
                None => !line.policy_only,
            })
            .collect()
    }

    fn render(&self, id: &str, slots: &[(&'static str, String)]) -> Option<String> {
        let template = self.templates.get(id)?;
        let mut out = template.text.clone();
        for (name, spec) in &template.slots {
            let given = slots.iter().find(|(slot, _)| slot == name);
            let value = match given {
                Some((_, value)) => value.clone(),
                None if spec.optional => String::new(),
                None => spec.default.clone()?,
            };
            out = out.replace(&format!("{{{name}}}"), &value);
        }
        Some(out.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    /// Lead-in, read-back and question of a confirmation. The read-back is the action and
    /// every parameter value, in the order the request holds them, or the tool's own read-back
    /// (`question`) with its slots filled.
    pub fn render_confirm(&self, request: &HitlRequest) -> [String; 3] {
        if request.kind == HitlKind::Confirm
            && let Some(own) = request.question.as_deref()
        {
            let line =
                |id, slots: &[(&'static str, String)]| self.render(id, slots).unwrap_or_default();
            let mut read = own.trim().to_owned();
            if let Some(first) = read.get(..1) {
                read = format!("{}{}", first.to_uppercase(), &read[1..]);
            }
            if !read.ends_with(['.', '?', '!']) {
                read.push('.');
            }
            return [
                line("confirm.lead_in", &[("lead_in", "To confirm:".to_owned())]),
                read,
                line("confirm.question", &[]),
            ];
        }
        let params: String = request
            .params
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(_, value)| format!(", {}", Self::speak_value(value)))
            .collect();
        // The read-back ends the sentence itself: "7 p.m." must not become "7 p.m..".
        let params = params.trim_end_matches('.').to_owned();
        let line =
            |id, slots: &[(&'static str, String)]| self.render(id, slots).unwrap_or_default();
        [
            line("confirm.lead_in", &[("lead_in", "To confirm:".to_owned())]),
            line(
                "confirm.readback",
                &[
                    ("verb_phrase", request.verb_phrase.clone()),
                    ("params", params),
                ],
            ),
            line("confirm.question", &[]),
        ]
    }

    /// A parameter value as it is read back: text as it is, a whole number without a decimal
    /// point, the items of a list in order.
    pub fn speak_value(value: &serde_json::Value) -> String {
        use serde_json::Value;
        match value {
            Value::Null => String::new(),
            Value::String(text) => text.clone(),
            Value::Number(number) => match number.as_f64() {
                Some(whole) if whole.fract() == 0.0 && whole.abs() < 1e15 => format!("{whole:.0}"),
                _ => number.to_string(),
            },
            Value::Array(items) => items
                .iter()
                .map(Self::speak_value)
                .collect::<Vec<_>>()
                .join(", "),
            Value::Object(fields) => fields
                .iter()
                .map(|(name, value)| {
                    format!("{} {}", name.replace('_', " "), Self::speak_value(value))
                })
                .collect::<Vec<_>>()
                .join(", "),
            Value::Bool(_) => value.to_string(),
        }
    }

    /// What the background model is told about who speaks for it.
    pub fn background_preamble(&self) -> String {
        self.background_preamble.clone()
    }

    /// The lines synthesised into the TTS cache at session start, acknowledgements first.
    /// Every line of a group that answers the caller within about a second is warmed, because
    /// the floor rotates over the whole group and a line that misses the cache is late. The
    /// holding, check-in and stalled lines come seconds into a wait, where one synthesis is
    /// not noticed: warming one line in four of them would only cost requests.
    pub fn warm_lines(&self) -> Vec<String> {
        let bank = |group, n| {
            self.resolve(
                &Say::Bank {
                    group,
                    intent: None,
                },
                n,
            )
        };
        let template = |id| {
            self.resolve(
                &Say::Template {
                    id,
                    slots: Vec::new(),
                },
                0,
            )
        };
        let rotating = ["ack", "update_ack", "withdrawal"]
            .into_iter()
            .flat_map(|group| (0..self.bank(group).len() as u64).map(move |n| bank(group, n)));
        let fixed = ["cancel.intent", "confirm.question", "failure.retryable"]
            .into_iter()
            .map(template);
        let mut lines = Vec::new();
        for line in rotating.chain(fixed).flatten() {
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
        lines
    }

    pub fn lexicon(&self) -> &Lexicon {
        &self.lexicon
    }

    /// The lines of a bank group, the persona's own where it overrides the base bank.
    pub(super) fn bank(&self, group: &str) -> &[BankLine] {
        self.banks.get(group).map(Vec::as_slice).unwrap_or_default()
    }

    /// The first social line with this intent.
    pub(super) fn social_line(&self, intent: &str) -> Option<&BankLine> {
        self.bank("social")
            .iter()
            .find(|line| line.intent.as_deref() == Some(intent))
    }

    /// The words of the persona's name, organisation and protected terms, sorted.
    pub(super) fn terms(&self) -> &[String] {
        &self.terms
    }

    /// The small-talk topics the persona answers itself.
    pub(super) fn social_scope(&self) -> &[String] {
        &self.social_scope
    }

    /// The value categories that are never read back, lower-case.
    pub(super) fn sensitive_categories(&self) -> &[String] {
        &self.sensitive_categories
    }

    /// The front model's system message.
    pub(super) fn front_system(&self) -> &str {
        &self.front_system
    }

    /// The first lines of the front model's user message.
    pub(super) fn call_header(&self) -> &str {
        &self.call_header
    }

    #[cfg(test)]
    pub(super) fn templates(&self) -> impl Iterator<Item = (&str, &str)> {
        self.templates
            .iter()
            .map(|(id, template)| (id.as_str(), template.text.as_str()))
    }
}

/// The `{slot}` names of a template, in order.
fn slots(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some((_, after)) = rest.split_once('{') {
        let Some((name, tail)) = after.split_once('}') else {
            break;
        };
        found.push(name.to_owned());
        rest = tail;
    }
    found.sort();
    found
}

/// A template's words without its slots: what the pack check reads.
fn fixed_words(text: &str) -> String {
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
    fixed
}

/// Caller-given text as the prompts quote it: one line, no quote marks of the prompt's own.
fn quoted(text: &str) -> String {
    let clean: String = text
        .chars()
        .map(|c| {
            if c.is_control() || c == '«' || c == '»' {
                ' '
            } else {
                c
            }
        })
        .collect();
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `Caller prefers: ...` of the call header.
fn preferences(front: &FrontConfig) -> Vec<&'static str> {
    let verbosity = match front.verbosity {
        Some(Verbosity::Brief) => Some("brief replies"),
        Some(Verbosity::Detailed) => Some("detailed replies"),
        _ => None,
    };
    let pace = match front.pace {
        Some(Pace::Slower) => Some("slower speech"),
        Some(Pace::Faster) => Some("faster speech"),
        _ => None,
    };
    verbosity.into_iter().chain(pace).collect()
}

/// Checks a tool's own read-back (`confirm_template`): every `{slot}` names an argument the
/// tool takes, and its own words pass the pack check and hold no word of the answer lexicon,
/// so the agent's read-back can never sound like the caller's answer.
pub fn check_confirm_template(template: &str, known: impl Fn(&str) -> bool) -> Result<(), String> {
    if let Some(slot) = slots(template).iter().find(|slot| !known(slot)) {
        return Err(format!("{{{slot}}} is not an argument of the tool"));
    }
    let fixed = fixed_words(template);
    if let Some(rule) = guard::pack_line(guard_lexicon(), &fixed, true, false) {
        return Err(format!("it breaks {rule}"));
    }
    let norm = norm_phrase(&fixed);
    let lexicon = &pack().lexicon;
    let answers = [
        &lexicon.strong_accept,
        &lexicon.weak_accept,
        &lexicon.decline,
        &lexicon.cancel,
    ];
    if let Some(word) = answers
        .into_iter()
        .flatten()
        .find(|phrase| has_phrase(&norm, phrase))
    {
        return Err(format!("it says {word:?}, an answer word"));
    }
    Ok(())
}

/// Every embedded persona, the default first.
#[cfg(test)]
pub(super) fn all() -> &'static [Arc<Persona>] {
    &pack().personas
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::types::{HitlId, HitlKind, TaskId};
    use super::*;

    fn bank(group: &'static str, intent: Option<&'static str>) -> Say {
        Say::Bank { group, intent }
    }

    /// The crate's assets are copies of the plan's files (the crate must build alone): an edit
    /// to one side that does not reach the other fails here.
    #[test]
    fn the_embedded_assets_equal_the_plan_documents() {
        macro_rules! same {
            ($($file:literal),*) => {$(assert!(
                include_str!(concat!("assets/", $file))
                    == include_str!(concat!("../../../../docs/duplex-agent/front-model/", $file)),
                "{} differs from its plan document", $file
            );)*};
        }
        same!(
            "guard_lexicon.en.json",
            "phrase_banks.en.json",
            "templates.en.json",
            "front_system_prompt.md",
            "personas/travel_northwind.json",
            "personas/clinic_harbourview.json",
            "personas/retail_larkspur.json"
        );
    }

    /// A pack directory with the English files under other language codes: `xx` with one
    /// template reworded, `yy` without one template, `zz` with a bank line the guard blocks.
    fn pack_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("waav-packs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let guard = include_str!("assets/guard_lexicon.en.json");
        let banks = include_str!("assets/phrase_banks.en.json");
        let templates = include_str!("assets/templates.en.json");
        let reworded = templates.replace(
            "Sorry, I didn't catch that. Could you say it again?",
            "Pardon, once more please?",
        );
        assert_ne!(reworded, templates);
        let mut without: serde_json::Value = serde_json::from_str(templates).unwrap();
        without["templates"]
            .as_object_mut()
            .unwrap()
            .remove("presence");
        let mut blocked: serde_json::Value = serde_json::from_str(banks).unwrap();
        blocked["groups"]["ack"]["lines"][0]["text"] = json!("All done, it's booked.");
        for (language, banks, templates) in [
            ("xx", banks.to_owned(), reworded),
            ("yy", banks.to_owned(), without.to_string()),
            ("zz", blocked.to_string(), templates.to_owned()),
        ] {
            std::fs::write(dir.join(format!("guard_lexicon.{language}.json")), guard).unwrap();
            std::fs::write(dir.join(format!("phrase_banks.{language}.json")), banks).unwrap();
            std::fs::write(dir.join(format!("templates.{language}.json")), templates).unwrap();
        }
        dir
    }

    #[test]
    fn a_language_pack_is_three_files_in_the_operators_directory() {
        let dir = pack_dir();
        let english = english();
        let (packs, problems) = load_packs(&dir, &english);
        std::fs::remove_dir_all(&dir).unwrap();
        let languages: Vec<&str> = packs.iter().map(|p| p.language.as_str()).collect();
        assert_eq!(languages, ["xx"], "{problems:?}");
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("yy") && problems[0].contains("presence"));
        assert!(problems[1].contains("zz") && problems[1].contains("breaks"));
        // Its persona speaks its own words, and the session's rules read its lexicon.
        let pack: &'static Pack = Box::leak(Box::new(packs.into_iter().next().unwrap()));
        let persona = &pack.personas[0];
        assert_eq!((persona.id(), persona.language()), (DEFAULT_ID, "xx"));
        let again = Say::Template {
            id: "repeat_request",
            slots: Vec::new(),
        };
        assert_eq!(
            persona.resolve(&again, 0).as_deref(),
            Some("Pardon, once more please?")
        );
        SESSION_PACK.sync_scope(pack, || {
            assert_eq!(language(), "xx");
            assert!(std::ptr::eq(guard_lexicon(), &pack.guard));
            assert!(super::super::rules::foreign_language(Some("en-US")));
            assert!(!super::super::rules::foreign_language(Some("xx-YY")));
        });
        assert_eq!(language(), "en");
        assert!(!super::super::rules::foreign_language(Some("en-US")));
        // The front reads the caller's language: its prompt names it and shows the pack's
        // lexicon rows as examples of the acts; English's prompt has no such lines.
        let system = persona.front_system();
        assert!(system.contains("- The caller speaks xx; read their words in xx."));
        assert!(
            system.contains("\n  control stop: \u{ab}stop\u{bb}, "),
            "{system}"
        );
        assert!(system.contains("\n  abort: \u{ab}never mind\u{bb}, "));
        assert!(system.contains("- Speak xx only."));
        assert_eq!(pack.front_gap(), None);
        let english_system = english.personas[0].front_system();
        assert!(!english_system.contains("The caller speaks"));
        assert!(english_system.contains("- Speak English only."));
        assert_eq!(english.front_gap(), None);
        // A pack without one of those rows has no front.
        let mut guard: serde_json::Value =
            serde_json::from_str(include_str!("assets/guard_lexicon.en.json")).unwrap();
        guard["rule_layer_cues"]["repeat_cues"] = json!([]);
        let bare = Pack::new(
            "xx",
            serde_json::from_value(guard).unwrap(),
            serde_json::from_str(include_str!("assets/phrase_banks.en.json")).unwrap(),
            serde_json::from_str(include_str!("assets/templates.en.json")).unwrap(),
            vec![],
        );
        assert_eq!(bare.front_gap(), Some("repeat_cues"));
        // Without a loaded pack a language has none; English always has one.
        assert_eq!(pack_of("xx"), None);
        assert_eq!(pack_of(" EN-gb"), Some("en"));
        assert_eq!(pack_of(""), Some("en"));
        // A multilingual recogniser names each turn's language: English, checked per turn.
        assert_eq!(pack_of("multi"), Some("en"));
        assert_eq!(pack_of("Auto"), Some("en"));
    }

    /// Chapter 5, "Clips per language and voice": the words, the fallback word and the
    /// emitter's numbers are the pack's; the persona's intensity scales the numbers.
    #[test]
    fn listening_sound_timing_comes_from_the_language_pack() {
        let english = english();
        let persona = &english.personas[0];
        assert_eq!(
            persona.backchannel_clips(),
            [("mhm", "mhm"), ("okay", "okay"), ("i-see", "I see")]
                .map(|(id, text)| (id.to_owned(), text.to_owned()))
        );
        assert_eq!(
            persona.backchannel_fallback(),
            Some(("okay".to_owned(), "okay".to_owned()))
        );
        // English's numbers: 6,000 ms, 1 in 3, 1,500 ms.
        let medium = persona.backchannel_timing();
        assert_eq!(medium, BackchannelTiming::default());
        assert_eq!(
            (medium.gap_ms, medium.skip_every, medium.min_voiced_ms),
            (6_000, 3, 1_500)
        );
        // A language whose listeners give them more often, and one whose fallback is no clip.
        let dir = std::env::temp_dir().join(format!("waav-clip-packs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut banks: serde_json::Value =
            serde_json::from_str(include_str!("assets/phrase_banks.en.json")).unwrap();
        let group = &mut banks["groups"]["backchannel"];
        group["timing"] = json!({ "gap_ms": 1_200, "skip_every": 10, "min_voiced_ms": 420 });
        group["fallback"] = json!("mhm");
        group["lines"][0]["text"] = json!("un");
        let mut broken = banks.clone();
        broken["groups"]["backchannel"]["fallback"] = json!("hmm");
        for (language, banks) in [("qq", banks), ("qr", broken)] {
            let files = [
                (
                    "guard_lexicon",
                    include_str!("assets/guard_lexicon.en.json").to_owned(),
                ),
                ("phrase_banks", banks.to_string()),
                (
                    "templates",
                    include_str!("assets/templates.en.json").to_owned(),
                ),
            ];
            for (name, text) in files {
                std::fs::write(dir.join(format!("{name}.{language}.json")), text).unwrap();
            }
        }
        let (packs, problems) = load_packs(&dir, &english);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(packs.len(), 1, "{problems:?}");
        assert!(problems[0].contains("qr") && problems[0].contains("fallback hmm"));
        let persona = &packs[0].personas[0];
        assert_eq!(
            persona.backchannel_fallback(),
            Some(("mhm".to_owned(), "un".to_owned()))
        );
        let medium = persona.backchannel_timing();
        let at = |level: Intensity| {
            let t = level.timing(medium).unwrap();
            (t.gap_ms, t.skip_every, t.per_turn, t.min_voiced_ms)
        };
        assert_eq!(at(Intensity::Medium), (1_200, 10, None, 420));
        assert_eq!(at(Intensity::Low), (2_400, 4, Some(1), 600));
        assert_eq!(at(Intensity::High), (864, 20, None, 300));
    }

    /// Chapter 10: the failure lines are the pack's `failure` group with their ids and kinds;
    /// a persona's lines replace those of their kinds; another language's pack brings its own
    /// (test only: no other pack ships), and a pack without a kind is left out.
    #[test]
    fn failure_lines_come_from_the_pack_and_the_persona() {
        let english = english();
        let lines = english.personas[0].failure_lines();
        let of = |lines: &[FailureLine], kind: LineKind| -> Vec<String> {
            let ids = lines.iter().filter(|line| line.kind == kind);
            ids.map(|line| line.id.clone()).collect()
        };
        assert_eq!(
            of(&lines, LineKind::Stall),
            ["stall-1", "stall-2", "stall-3"]
        );
        assert_eq!(of(&lines, LineKind::Hearing).len(), 3);
        assert_eq!(of(&lines, LineKind::Exit), ["exit"]);
        assert_eq!(of(&lines, LineKind::ExitFollowUp), ["exit-follow-up"]);
        assert_eq!(of(&lines, LineKind::Back), ["back"]);
        // Only the exit line that may promise it says it will get back.
        let promises: Vec<&str> = lines
            .iter()
            .filter(|line| line.text.contains("get back to you"))
            .map(|line| line.id.as_str())
            .collect();
        assert_eq!(promises, ["exit-follow-up"]);
        let spec = |failure: serde_json::Value| {
            json!({
                "id": "failure-test", "name": "Ava", "org": "Northwind", "role": "travel agent",
                "banks": { "failure": failure },
            })
        };
        let own = json!([{ "text": "One moment, my line is slow.", "clip": "slow", "subsituation": "stall" }]);
        let persona = Persona::from_spec(&spec(own)).expect("a persona");
        let lines = persona.failure_lines();
        assert_eq!(of(&lines, LineKind::Stall), ["slow"]);
        assert_eq!(
            of(&lines, LineKind::Hearing).len(),
            3,
            "the pack's other kinds stay"
        );
        let unnamed = json!([{ "text": "One moment, my line is slow." }]);
        let refused = Persona::from_spec(&spec(unnamed)).err().unwrap_or_default();
        assert!(refused.starts_with("banks.failure"), "{refused}");
        // Another language.
        let dir = std::env::temp_dir().join(format!("waav-failure-packs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut banks: serde_json::Value =
            serde_json::from_str(include_str!("assets/phrase_banks.en.json")).unwrap();
        let spanish = [
            (
                "Perdone, mis sistemas van lentos. Un momento.",
                "stall-1",
                "stall",
            ),
            (
                "Perdone, ahora no le oigo bien. Un momento.",
                "hearing-1",
                "hearing",
            ),
            (
                "Lo siento mucho, mis sistemas han caido y tengo que colgar.",
                "exit",
                "exit",
            ),
            (
                "Lo siento mucho, tengo que colgar. Le llamaremos cuando vuelvan.",
                "exit-follow-up",
                "exit_follow_up",
            ),
            ("Gracias por esperar, ya estoy.", "back", "back"),
        ];
        let lines: Vec<serde_json::Value> = spanish
            .iter()
            .map(|(text, id, kind)| json!({ "text": text, "clip": id, "subsituation": kind }))
            .collect();
        banks["groups"]["failure"]["lines"] = json!(lines);
        let mut without = banks.clone();
        without["groups"]["failure"]["lines"] = json!(lines[..3]);
        for (language, banks) in [("qs", banks), ("qt", without)] {
            let files = [
                (
                    "guard_lexicon",
                    include_str!("assets/guard_lexicon.en.json").to_owned(),
                ),
                ("phrase_banks", banks.to_string()),
                (
                    "templates",
                    include_str!("assets/templates.en.json").to_owned(),
                ),
            ];
            for (name, text) in files {
                std::fs::write(dir.join(format!("{name}.{language}.json")), text).unwrap();
            }
        }
        let (packs, problems) = load_packs(&dir, &english);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(packs.len(), 1, "{problems:?}");
        assert!(problems[0].contains("qt") && problems[0].contains("no exit_follow_up line"));
        let lines = packs[0].personas[0].failure_lines();
        assert_eq!(lines.len(), 5);
        assert_eq!(
            lines[0].text,
            "Perdone, mis sistemas van lentos. Un momento."
        );
    }

    #[test]
    fn word_list_keeps_apostrophe_suffixes_and_drops_punctuation() {
        assert_eq!(
            word_list("Don\u{2019}t stop, it's 4-4-1-7!"),
            ["don't", "stop", "it's", "4", "4", "1", "7"]
        );
        assert_eq!(
            word_list("rock'n'roll users' "),
            ["rock'n", "roll", "users"]
        );
    }

    /// Any script: accented letters stay in their words, case folds, and each letter of a
    /// script without spaces is a word, so a Japanese phrase matches as a run of characters
    /// and an empty one is never found everywhere.
    #[test]
    fn words_and_phrases_of_every_script() {
        assert_eq!(
            word_list("Ça coûte 3 €, d'accord ?"),
            ["ça", "coûte", "3", "d'accord"]
        );
        assert_eq!(word_list("ÜBER Straße"), ["über", "straße"]);
        assert_eq!(
            word_list("はい、お願いします。"),
            ["は", "い", "お", "願", "い", "し", "ま", "す"]
        );
        assert_eq!(word_list("Привет, мир"), ["привет", "мир"]);
        // A combining accent stays with its letter; a Devanagari word keeps its virama.
        assert_eq!(word_list("cafe\u{301} क्या"), ["cafe\u{301}", "क्या"]);
        assert_eq!(norm_phrase("Où est-ce ?"), " où est-ce ");
        assert_eq!(
            norm_phrase("フランスの首都は？"),
            " フ ラ ン ス の 首 都 は "
        );
        let norm = norm_phrase("少々お待ちください、確認します。");
        assert!(has_phrase(&norm, "確認します"));
        assert!(has_phrase(&norm, "少々"));
        assert!(!has_phrase(&norm, "承知しました"));
        assert!(!has_phrase(&norm_phrase("élève"), "lève"));
        assert!(has_phrase(&norm_phrase("Je l'ai réservé."), "réservé"));
        assert!(!has_phrase(&norm_phrase("Je l'ai réservé."), "réserve"));
        assert!(!has_phrase(&norm_phrase("。"), "、"));
        assert!(!has_phrase(&norm_phrase(""), ""));
    }

    #[test]
    fn phrases_match_on_whole_words_only() {
        let norm = norm_phrase("Actually, can you cancel that?");
        assert_eq!(norm, " actually can you cancel that ");
        assert!(has_phrase(&norm, "cancel that"));
        assert!(has_phrase(&norm, "Can you"));
        assert!(!has_phrase(&norm, "act"));
        assert!(!has_phrase(&norm_phrase("they're here"), "er"));
    }

    #[test]
    fn unknown_id_gives_the_default_persona() {
        let default = Persona::load(DEFAULT_ID);
        assert!(Arc::ptr_eq(&default, &Persona::load("no-such-persona")));
        assert_eq!(
            default
                .resolve(&bank("social", Some("goodbye")), 0)
                .unwrap(),
            "Thanks for calling this help line. Goodbye."
        );
        assert!(default.terms().is_empty());
        let travel = Persona::load("northwind-travel-ava");
        assert_eq!(travel.terms(), ["ava", "northwind", "travel"]);
    }

    #[test]
    fn a_bank_rotates_through_its_lines() {
        let persona = Persona::load(DEFAULT_ID);
        let say = bank("update_ack", None);
        let lines: Vec<String> = (0..5).map(|n| persona.resolve(&say, n).unwrap()).collect();
        assert_eq!(lines[0], "Okay, I'll change that.");
        assert_eq!(lines[1], "Got it, I'll update it.");
        assert_eq!(lines[3], "Alright, I'll update that.");
        assert_eq!(lines[4], lines[0]);
        assert_eq!(persona.resolve(&bank("no_such_group", None), 0), None);
        assert_eq!(persona.resolve(&Say::Repeat, 0), None);
        assert_eq!(
            persona
                .resolve(&Say::Verbatim("Two flights.".to_owned()), 9)
                .unwrap(),
            "Two flights."
        );
    }

    #[test]
    fn ack_lines_fit_the_clip_limit() {
        for persona in all() {
            let acks = persona.bank("ack");
            assert!(acks.len() >= 3, "{}", persona.id);
            for line in acks {
                assert!(line.text.chars().count() <= 24, "{:?}", line.text);
            }
        }
        // A line over the limit is dropped when the persona is compiled.
        let pack = pack();
        let banks: BanksFile =
            serde_json::from_str(include_str!("assets/phrase_banks.en.json")).unwrap();
        let templates: TemplatesFile =
            serde_json::from_str(include_str!("assets/templates.en.json")).unwrap();
        let spec: Spec = serde_json::from_value(json!({
            "id": "long", "name": "Kit", "org": "Kite",
            "banks": { "ack": [
                { "text": "One moment." },
                { "text": "Let me look into that for you right now." }
            ] }
        }))
        .unwrap();
        let lexicon = Lexicon::default();
        let persona = Persona::compile(spec, "en", &banks, &templates, &pack.guard, &lexicon);
        assert_eq!(persona.bank("ack").len(), 1);
        assert_eq!(persona.bank("ack")[0].acts, [Act::Delegate]);
    }

    #[test]
    fn a_yes_no_question_skips_polar_lines() {
        let persona = Persona::load(DEFAULT_ID);
        let plain: Vec<String> = (0..9)
            .map(|n| persona.resolve(&bank("ack", None), n).unwrap())
            .collect();
        assert_eq!(plain[0], "Sure, let me check.");
        assert!(plain.contains(&"Sure, I'll find out.".to_owned()));
        let polar: Vec<String> = (0..9)
            .map(|n| {
                persona
                    .resolve(&bank("ack", Some(POLAR_QUESTION)), n)
                    .unwrap()
            })
            .collect();
        assert_eq!(polar[0], "Okay, one moment.");
        assert!(polar.iter().all(|line| !line.starts_with("Sure")));
        assert_eq!(polar[7], polar[0]);
    }

    #[test]
    fn an_intent_picks_its_social_line() {
        let clinic = Persona::load("harbourview-clinic-nora");
        assert_eq!(
            clinic
                .resolve(&bank("social", Some("presence")), 3)
                .unwrap(),
            "Yes, I'm still here."
        );
        assert_eq!(
            clinic
                .resolve(&bank("social", Some("goodbye_open_tasks")), 0)
                .unwrap(),
            "Before you go, I'm still working on your request."
        );
        // Without an intent the policy-only lines are never picked.
        let all: Vec<String> = (0..12)
            .map(|n| clinic.resolve(&bank("social", None), n).unwrap())
            .collect();
        assert!(all.iter().all(|line| !line.starts_with("Before you go")));
        assert_eq!(
            clinic.resolve(&bank("withdrawal", None), 4).unwrap(),
            "Scratch that."
        );
    }

    #[test]
    fn templates_fill_their_slots() {
        let travel = Persona::load("northwind-travel-ava");
        let say = Say::Template {
            id: "status.working",
            slots: vec![("task_title", "your flight search".to_owned())],
        };
        assert_eq!(
            travel.resolve(&say, 0).unwrap(),
            "I'm still working on your flight search."
        );
        assert_eq!(
            Persona::load(DEFAULT_ID).resolve(&say, 0).unwrap(),
            "Still working on your flight search."
        );
        let missing = Say::Template {
            id: "status.working",
            slots: Vec::new(),
        };
        assert_eq!(travel.resolve(&missing, 0), None);
        let unbuilt = Say::Template {
            id: "sensitive.dtmf",
            slots: Vec::new(),
        };
        assert_eq!(travel.resolve(&unbuilt, 0), None);
        let correction = Say::Template {
            id: "regime_c_correction",
            slots: Vec::new(),
        };
        assert_eq!(
            travel.resolve(&correction, 0).unwrap(),
            "Just to be clear, that isn't finished yet. I'll tell you when it is."
        );
    }

    #[test]
    fn a_persona_object_is_checked_like_the_packaged_ones() {
        let ok = json!({
            "id": "lumen-lumi", "name": "Lumi", "org": "Lumen Air", "role": "booking assistant",
            "backchannel_clips": ["mhm"],
            "banks": { "ack": [{ "text": "Let me look into it." }] },
            "template_overrides": { "hitl.accept": "Alright." }
        });
        let lumi = Persona::from_spec(&ok).expect("a valid persona");
        assert_eq!(lumi.id(), "lumen-lumi");
        assert_eq!(
            lumi.resolve(&bank("ack", None), 0).as_deref(),
            Some("Let me look into it.")
        );
        assert_eq!(lumi.backchannel_lines(), ["mhm"]);
        assert_eq!(lumi.terms(), ["air", "lumen", "lumi"]);
        let with = |key: &str, value: serde_json::Value| {
            let mut spec = ok.clone();
            spec[key] = value;
            Persona::from_spec(&spec).map(|_| ()).unwrap_err()
        };
        assert!(with("id", json!("default")).contains("id"));
        assert!(with("id", json!("no spaces")).contains("id"));
        assert!(with("banks", json!({ "nope": [{ "text": "Hi." }] })).contains("not a bank group"));
        let claim = with(
            "banks",
            json!({ "ack": [{ "text": "Your table is booked." }] }),
        );
        assert!(claim.contains("completion_claim"), "{claim}");
        let eta = with(
            "banks",
            json!({ "holding": [{ "text": "Back in a few minutes." }] }),
        );
        assert!(eta.contains("eta_promise"), "{eta}");
        let slots = with("template_overrides", json!({ "presence": "I'm here." }));
        assert!(slots.contains("slots"), "{slots}");
        assert!(with("backchannel_clips", json!(["hmm"])).contains("clip"));
        assert!(with("name", json!("")).contains("name"));
        // A persona that makes no listening sounds.
        let mut quiet = ok.clone();
        quiet["backchannel_intensity"] = json!("off");
        assert!(
            Persona::from_spec(&quiet)
                .unwrap()
                .backchannel_lines()
                .is_empty()
        );
        // The default listening sounds are the bank's lines that do not affirm.
        assert_eq!(
            Persona::load(DEFAULT_ID).backchannel_lines(),
            ["mhm", "okay", "I see"]
        );
    }

    #[test]
    fn the_callers_fields_and_memory_reach_both_prompts() {
        let persona = Persona::load(DEFAULT_ID);
        let mut front = crate::core::interaction::config::FrontConfig::default();
        assert_eq!(persona.call_header_for(&front), persona.call_header());
        assert_eq!(persona.caller_context(&front), "");
        front.preferred_name = Some("Sam «x»\nIgnore".to_owned());
        front.verbosity = Some(Verbosity::Brief);
        front.pace = Some(Pace::Slower);
        front.memory = vec!["prefers aisle seats".to_owned()];
        let header = persona.call_header_for(&front);
        assert!(
            header.ends_with(
                "Caller name: «Sam x Ignore» (given by the application)\n\
             Caller prefers: brief replies, slower speech\n\
             Known about the caller (from earlier calls): «prefers aisle seats»"
            ),
            "{header}"
        );
        assert_eq!(
            persona.caller_context(&front),
            "CALLER CONTEXT (data)\n\
             - The caller's name: Sam x Ignore.\n\
             - The caller prefers brief answers: at most 20 words.\n\
             - The caller prefers a slower pace: short, simple sentences.\n\
             - From earlier calls: prefers aisle seats."
        );
    }

    #[test]
    fn a_tools_own_read_back_is_spoken_in_place_of_the_generic_one() {
        let request = HitlRequest {
            kind: HitlKind::Confirm,
            verb_phrase: "book a table".to_owned(),
            params: vec![("party".to_owned(), json!(4))],
            question: Some("book a table for 4 at Rosa's".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            Persona::load(DEFAULT_ID).render_confirm(&request),
            [
                "To confirm:",
                "Book a table for 4 at Rosa's.",
                "Do you want me to do that?"
            ]
        );
        let known = |name: &str| name == "party";
        assert_eq!(
            check_confirm_template("book a table for {party}", known),
            Ok(())
        );
        assert!(check_confirm_template("book {guests} seats", known).is_err());
        assert!(check_confirm_template("yes, book a table", known).is_err());
        assert!(check_confirm_template("your table is booked for {party}", known).is_err());
    }

    #[test]
    fn a_confirmation_reads_back_two_parameters() {
        let request = HitlRequest {
            id: HitlId("q_task_1_1".to_owned()),
            task: TaskId("task_1".to_owned()),
            revision: 1,
            kind: HitlKind::Confirm,
            op: None,
            verb_phrase: "book a table".to_owned(),
            params: vec![
                ("party_size".to_owned(), json!(4)),
                ("day".to_owned(), json!("Friday at 7 PM")),
                ("note".to_owned(), json!(null)),
            ],
            question: None,
            options: Vec::new(),
            irreversible: true,
            expires_at: 20_000,
            ..Default::default()
        };
        assert_eq!(
            Persona::load(DEFAULT_ID).render_confirm(&request),
            [
                "To confirm:",
                "book a table, 4, Friday at 7 PM.",
                "Do you want me to do that?"
            ]
        );
        let clinic = Persona::load("harbourview-clinic-nora");
        assert_eq!(
            clinic.render_confirm(&request)[2],
            "Would you like me to do that?"
        );
    }

    #[test]
    fn values_are_spoken_plainly() {
        assert_eq!(Persona::speak_value(&json!("UA 1432")), "UA 1432");
        assert_eq!(Persona::speak_value(&json!(2.0)), "2");
        assert_eq!(Persona::speak_value(&json!(312.5)), "312.5");
        assert_eq!(Persona::speak_value(&json!(["aisle", 2])), "aisle, 2");
        assert_eq!(
            Persona::speak_value(&json!({ "seat_class": "economy" })),
            "seat class economy"
        );
    }

    #[test]
    fn warm_lines_start_with_every_acknowledgement() {
        let persona = Persona::load(DEFAULT_ID);
        let lines = persona.warm_lines();
        // Whatever line the floor's rotation picks from a group that answers the caller
        // within a second, with or without the polar lines, it was warmed.
        for n in 0..20 {
            for intent in [None, Some(POLAR_QUESTION)] {
                let ack = persona.resolve(&bank("ack", intent), n).unwrap();
                assert!(lines.contains(&ack), "{ack}");
            }
            for group in ["update_ack", "withdrawal"] {
                let line = persona.resolve(&bank(group, None), n).unwrap();
                assert!(lines.contains(&line), "{line}");
            }
        }
        assert_eq!(
            lines,
            [
                "Sure, let me check.",
                "Okay, one moment.",
                "Let me look into that.",
                "Got it, I'll check now.",
                "Sure, I'll find out.",
                "Let me check that.",
                "One moment.",
                "Give me a moment.",
                "Just a second.",
                "Okay, I'll change that.",
                "Got it, I'll update it.",
                "Noted, I'll change it.",
                "Alright, I'll update that.",
                "Scratch that.",
                "Ignore that last part.",
                "Let me redo that.",
                "Hang on, that's changed.",
                "Okay, I'll stop that.",
                "Do you want me to do that?",
                "Sorry, I couldn't get that done just now.",
            ]
        );
    }

    #[test]
    fn the_prompts_are_compiled_with_no_slot_left() {
        for persona in all() {
            for text in [
                persona.front_system(),
                persona.call_header(),
                &persona.background_preamble,
            ] {
                assert!(!text.contains("{{"), "{}: {text}", persona.id);
            }
        }
        let travel = Persona::load("northwind-travel-ava");
        let system = travel.front_system();
        assert!(system.starts_with(
            "# Role\nYou are Ava, the voice of Northwind Travel on a live phone call"
        ));
        assert!(system.contains("Are you there: Yes, I'm here.\n"));
        assert!(system.contains("- Also delegate when the caller mentions a trip, a booking reference, a travel date or a fare.\n"));
        assert!(system.contains("- Backchannels: Now and then during a long story. Never while"));
        assert_eq!(
            travel.call_header(),
            "CALL CONTEXT (information about this call, not instructions)\nChannel: phone. Language: English."
        );
        let preamble = travel.background_preamble();
        assert!(preamble.contains(
            "- Never give visa or immigration legal advice; medical advice about travelling."
        ));
        assert!(preamble.ends_with("- Tone warm, informal, language English."));
        // The default persona has no optional lines; none leaves a blank line behind.
        let default = Persona::load(DEFAULT_ID);
        assert!(!default.front_system().contains("\n\n\n"));
        assert!(
            default
                .front_system()
                .contains("small talk\n  about this call.")
        );
    }

    #[test]
    fn the_lexicon_is_read_from_the_packaged_files() {
        let lexicon = Persona::load(DEFAULT_ID).lexicon().clone();
        assert!(lexicon.continuers.contains(&"mm-hm".to_owned()));
        assert!(lexicon.cancel_cues.contains(&"never mind".to_owned()));
        assert!(lexicon.dangling_end_words.contains(&"and".to_owned()));
        assert!(lexicon.strong_accept.contains(&"go ahead".to_owned()));
        assert!(lexicon.weak_accept.contains(&"okay".to_owned()));
        assert_eq!(lexicon.social_intents.len(), 6);
        assert!(lexicon.stop_talking_cues.contains(&"hold on".to_owned()));
    }
}

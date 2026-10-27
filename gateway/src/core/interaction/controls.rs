//! Conversation controls (plan chapter 9): the caller steering the conversation itself ("stop",
//! "slower please", "say that again", "talk to me in Spanish"). One typed [`Control`] comes from
//! the front model's `control` act, the orchestrator's `conversation_control` tool and the
//! client's `control` message alike; [`apply`] checks it against the operator's bounds
//! (`interaction.controls`) and the session's [`Caps`], updates the versioned
//! [`ConversationState`] and says what the actor has to do ([`Effect`]). Nothing here decides
//! what the caller meant: that is the front model's, in context.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{Announce, Say, Verbosity};
use crate::core::voice::{Age, Gender};
use crate::handlers::voices::Voice;

/// The closed list of control kinds (chapter 9, "The controls").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Stop,
    Hold,
    Resume,
    Repeat,
    NotHeard,
    Clarify,
    Spell,
    Rate,
    Volume,
    Verbosity,
    Language,
    Voice,
    Backchannels,
    Updates,
    ToScreen,
    End,
    Human,
}

/// The slots a repeat, a spelling or a screen copy may name.
pub const SLOTS: [&str; 5] = ["number", "time", "name", "address", "code"];

/// What a `voice` control may ask for: a gender, an age and an accent, matched against the
/// provider's voice list (one or two of them, as "male british").
pub const VOICE_WORDS: [&str; 9] = [
    "male",
    "female",
    "young",
    "old",
    "american",
    "british",
    "australian",
    "irish",
    "indian",
];

impl ControlKind {
    pub const ALL: [ControlKind; 17] = [
        ControlKind::Stop,
        ControlKind::Hold,
        ControlKind::Resume,
        ControlKind::Repeat,
        ControlKind::NotHeard,
        ControlKind::Clarify,
        ControlKind::Spell,
        ControlKind::Rate,
        ControlKind::Volume,
        ControlKind::Verbosity,
        ControlKind::Language,
        ControlKind::Voice,
        ControlKind::Backchannels,
        ControlKind::Updates,
        ControlKind::ToScreen,
        ControlKind::End,
        ControlKind::Human,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ControlKind::Stop => "stop",
            ControlKind::Hold => "hold",
            ControlKind::Resume => "resume",
            ControlKind::Repeat => "repeat",
            ControlKind::NotHeard => "not_heard",
            ControlKind::Clarify => "clarify",
            ControlKind::Spell => "spell",
            ControlKind::Rate => "rate",
            ControlKind::Volume => "volume",
            ControlKind::Verbosity => "verbosity",
            ControlKind::Language => "language",
            ControlKind::Voice => "voice",
            ControlKind::Backchannels => "backchannels",
            ControlKind::Updates => "updates",
            ControlKind::ToScreen => "to_screen",
            ControlKind::End => "end",
            ControlKind::Human => "human",
        }
    }

    pub fn parse(name: &str) -> Option<ControlKind> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// The closed values of the kind's argument, and whether one must be given. A language is
    /// a BCP-47 code and a hold a number of seconds: both are checked apart.
    pub fn values(self) -> (&'static [&'static str], bool) {
        match self {
            // `slower` and `louder`: the line said again, one step slower or louder, that
            // line only ("say that last part slower").
            ControlKind::Repeat => (
                &[
                    "last", "answer", "number", "time", "name", "address", "code", "slower",
                    "louder",
                ],
                false,
            ),
            ControlKind::Spell | ControlKind::ToScreen => (&SLOTS, false),
            ControlKind::Rate => (&["slower", "faster", "normal"], true),
            ControlKind::Volume => (&["louder", "quieter", "normal"], true),
            ControlKind::Verbosity => (&["brief", "normal", "detailed"], true),
            ControlKind::Voice => (&VOICE_WORDS, false),
            ControlKind::Backchannels => (&["on", "off"], true),
            ControlKind::Updates => (&["ask", "tell", "screen"], true),
            _ => (&[], false),
        }
    }

    /// `repeat` and `not_heard` are for the next line only; the rest for the session.
    pub fn default_scope(self) -> Scope {
        match self {
            ControlKind::Repeat | ControlKind::NotHeard | ControlKind::Spell => Scope::Turn,
            _ => Scope::Session,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Turn,
    Session,
}

/// Who asked: the caller (the front model read it from their words), an agent or the
/// orchestrator (`conversation_control`), or the client app (`control`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlSource {
    Caller,
    Agent,
    Client,
    /// The caller's wish of an earlier call, kept by caller memory (a language), applied at
    /// the start of this one.
    Memory,
}

/// One control, the same from every source.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Control {
    pub kind: ControlKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub scope: Scope,
    pub source: ControlSource,
    /// The caller turn it came with, when it came with one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
}

/// A BCP-47 language code as the controls take it: a 2 or 3 letter language, optionally a
/// region (`es`, `zh-CN`, `pt-BR`), lower-cased language and upper-cased region.
pub fn language_code(raw: &str) -> Option<String> {
    let mut parts = raw.trim().split(['-', '_']);
    let language = parts.next()?.to_ascii_lowercase();
    let region = parts.next();
    let ok = |part: &str, len: std::ops::RangeInclusive<usize>| {
        len.contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
    };
    if !ok(&language, 2..=3) || parts.next().is_some() {
        return None;
    }
    match region {
        Some(region) if ok(region, 2..=4) => Some(format!("{language}-{}", region.to_uppercase())),
        Some(_) => None,
        None => Some(language),
    }
}

impl Control {
    /// A control with its value checked: `Err` names what is wrong.
    pub fn new(
        kind: ControlKind,
        value: Option<&str>,
        source: ControlSource,
        turn: Option<u64>,
    ) -> Result<Control, String> {
        let value = value.map(str::trim).filter(|value| !value.is_empty());
        let value = match kind {
            ControlKind::Language => Some(
                value
                    .and_then(language_code)
                    .ok_or("language takes a BCP-47 code such as \"es\"")?,
            ),
            // One or two of the voice words, as "male british".
            ControlKind::Voice => match value.map(str::to_ascii_lowercase) {
                Some(words) => {
                    let parts: Vec<&str> = words.split_whitespace().collect();
                    if parts.is_empty()
                        || parts.len() > 2
                        || !parts.iter().all(|word| VOICE_WORDS.contains(word))
                    {
                        return Err(format!(
                            "voice takes one or two of: {}",
                            VOICE_WORDS.join(", ")
                        ));
                    }
                    Some(parts.join(" "))
                }
                None => None,
            },
            ControlKind::Hold => match value {
                Some(seconds) => match seconds.parse::<u32>() {
                    Ok(1..=300) => Some(seconds.to_owned()),
                    _ => return Err("hold takes a number of seconds from 1 to 300".to_owned()),
                },
                None => None,
            },
            _ => {
                let (values, required) = kind.values();
                match value {
                    Some(value) if values.contains(&value) => Some(value.to_owned()),
                    Some(_) | None if values.is_empty() => None,
                    None if !required => None,
                    _ => {
                        return Err(format!(
                            "{} takes one of: {}",
                            kind.as_str(),
                            values.join(", ")
                        ));
                    }
                }
            }
        };
        Ok(Control {
            kind,
            value,
            scope: kind.default_scope(),
            source,
            turn,
        })
    }

    /// From the wire: `{"kind": "rate", "value": "slower"}` (the client's `control` message and
    /// the `conversation_control` tool's arguments).
    pub fn from_json(raw: &Value, source: ControlSource) -> Result<Control, String> {
        let kind = raw["kind"].as_str().unwrap_or_default();
        let kind = ControlKind::parse(kind).ok_or_else(|| {
            let kinds: Vec<&str> = ControlKind::ALL.iter().map(|k| k.as_str()).collect();
            format!("kind must be one of: {}", kinds.join(", "))
        })?;
        let value = match &raw["value"] {
            Value::String(value) => Some(value.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        };
        Control::new(kind, value.as_deref(), source, None)
    }
}

/// The operator's bounds (`interaction.controls`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ControlsConfig {
    /// The kinds that may be applied; the rest are `refused`.
    pub allowed: Vec<ControlKind>,
    pub rate_min: f32,
    pub rate_max: f32,
    pub volume_min_db: f32,
    pub volume_max_db: f32,
    /// The languages a session may switch to, besides its own.
    pub languages: Vec<String>,
    /// Whether the orchestrator and agents may issue controls, and the client app.
    pub from_agents: bool,
    pub from_clients: bool,
    /// Without a reviewed language pack, the models answer in the language and the gateway's
    /// own lines stay in the session's language (`partial`).
    pub answers_only: bool,
    /// Where a `human` control puts the caller through. Only whether one is set is echoed.
    #[serde(rename = "human", serialize_with = "human_set")]
    pub human: HumanConfig,
}

/// `controls.human`: the operator's transfer to a person.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HumanConfig {
    /// A SIP session's caller is referred there (a phone number or extension).
    pub transfer_to: Option<String>,
    /// Any other session's request goes there, signed (`human_requested`).
    pub url: Option<String>,
    /// How long the caller waits for the operator's `human_connected` or `human_unavailable`
    /// after a request to `url` (`None`: [`HUMAN_WAIT_S`]; 0: no wait, the caller is handed
    /// over at once).
    pub wait_s: Option<u32>,
}

/// The default of `controls.human.wait_s`.
pub const HUMAN_WAIT_S: u32 = 60;

impl HumanConfig {
    pub fn is_set(&self) -> bool {
        self.transfer_to.is_some() || self.url.is_some()
    }

    pub fn wait_s(&self) -> u32 {
        self.wait_s.unwrap_or(HUMAN_WAIT_S)
    }
}

fn human_set<S: serde::Serializer>(human: &HumanConfig, out: S) -> Result<S::Ok, S::Error> {
    out.serialize_bool(human.is_set())
}

impl Default for ControlsConfig {
    fn default() -> Self {
        ControlsConfig {
            allowed: ControlKind::ALL.to_vec(),
            rate_min: 0.7,
            rate_max: 1.3,
            volume_min_db: -9.0,
            volume_max_db: 9.0,
            languages: Vec::new(),
            from_agents: true,
            from_clients: true,
            answers_only: false,
            human: HumanConfig::default(),
        }
    }
}

/// What the session can do (chapter 9, "One executor").
#[derive(Debug, Clone, Default)]
pub struct Caps {
    /// The text-to-speech takes a rate mid-session.
    pub rate: bool,
    /// The recogniser hears any language (a multilingual model), or these.
    pub hears_any: bool,
    pub hears: Vec<String>,
    /// A voice of the session's provider per language, the persona's gender first.
    pub voices: Vec<(String, String)>,
    /// Reviewed language packs.
    pub packs: Vec<String>,
    /// A screen is attached, a transfer is configured.
    pub screen: bool,
    pub transfer: bool,
    /// The provider's voice list (empty until it was read), and the session's own voice.
    pub catalog: Vec<Voice>,
    pub own_voice: Option<String>,
}

impl Caps {
    fn hears(&self, language: &str) -> bool {
        self.hears_any || self.hears.iter().any(|code| same_language(code, language))
    }

    fn voice(&self, language: &str) -> Option<&str> {
        let found = self
            .voices
            .iter()
            .find(|(code, _)| same_language(code, language));
        found.map(|(_, voice)| voice.as_str())
    }
}

/// `es` and `es-MX` are the same language here.
pub fn same_language(a: &str, b: &str) -> bool {
    let base = |code: &str| {
        code.split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    base(a) == base(b)
}

/// The language's English name, for the history note and the call header.
pub fn language_name(code: &str) -> String {
    let found = NAMES.iter().find(|(known, _)| same_language(known, code));
    found.map_or_else(|| code.to_owned(), |(_, name)| (*name).to_owned())
}

/// The code of a language named in English ("Spanish", "spanish (Mexico)").
pub fn language_of_name(name: &str) -> Option<String> {
    let first = name.split([' ', '(']).next()?.to_ascii_lowercase();
    let found = NAMES
        .iter()
        .find(|(_, known)| known.to_ascii_lowercase() == first);
    found.map(|(code, _)| (*code).to_owned())
}

const NAMES: [(&str, &str); 16] = [
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("zh", "Chinese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("hi", "Hindi"),
    ("ar", "Arabic"),
    ("ru", "Russian"),
    ("tr", "Turkish"),
    ("pl", "Polish"),
    ("sv", "Swedish"),
];

/// The languages a front's `control` act may name: the operator's and these common ones, so
/// a wish the session cannot meet is still heard and answered honestly.
pub fn common_languages() -> impl Iterator<Item = &'static str> {
    NAMES.iter().map(|(code, _)| *code)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Applied,
    Partial,
    Unsupported,
    Refused,
}

/// One applied control as the state lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Record {
    pub kind: ControlKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub source: ControlSource,
    pub outcome: Outcome,
}

/// The applied controls the state keeps.
const RECORDS_KEPT: usize = 16;

/// The conversation state the gateway owns (chapter 9): sent to the client in
/// `interaction_updated`, to the front in its call header and to the orchestrator and every
/// agent with each request.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConversationState {
    pub version: u64,
    pub language: String,
    /// The speaking rate against the session's own (1.0).
    pub rate: f32,
    /// The playback gain, dB.
    pub volume_db: f32,
    pub verbosity: Verbosity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    pub backchannels: bool,
    pub updates: Announce,
    pub held: bool,
    pub controls: Vec<Record>,
}

impl ConversationState {
    pub fn new(
        language: &str,
        rate: f32,
        verbosity: Option<Verbosity>,
        backchannels: bool,
    ) -> Self {
        ConversationState {
            version: 0,
            language: language.to_owned(),
            rate,
            volume_db: 0.0,
            verbosity: verbosity.unwrap_or(Verbosity::Normal),
            voice: None,
            backchannels,
            updates: Announce::Ask,
            held: false,
            controls: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// The state as a short note for a model: a system note for the LLM tier. It comes after
    /// the persona's preamble, which names the session's language: live, a 3B background that
    /// read "answer in Spanish (es)" answered in English and copied the history's bracketed
    /// notes, and answered in Spanish (3 of 3 questions, 3 seeds) with this wording.
    pub fn note(&self) -> String {
        let verbosity = match self.verbosity {
            Verbosity::Brief => "brief",
            Verbosity::Normal => "normal",
            Verbosity::Detailed => "detailed",
        };
        let name = language_name(&self.language);
        let language = if same_language(&self.language, "en") {
            format!("answer in {name} ({})", self.language)
        } else {
            format!(
                "the caller asked for {name}, so every answer is in {name} ({}), whatever \
                 language is named above",
                self.language
            )
        };
        format!(
            "Conversation state (set by the gateway, version {}): {language}; replies \
             {verbosity}. Lines in [brackets] are the gateway's notes: never write one.",
            self.version,
        )
    }
}

/// What the actor does for an applied control.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    /// Chapter 5's cut.
    Cut,
    /// Cut, keep the tasks running, check in after the seconds or when the caller speaks.
    Hold(Option<u32>),
    /// Say the unheard rest of the cut or held line.
    Resume,
    /// Say the last heard line again, or only its slot; that line only at this rate (where
    /// the voice takes one) and this much louder (dB).
    Repeat {
        slot: Option<String>,
        rate: Option<f32>,
        gain_db: f32,
    },
    /// The background rephrases its last answer more simply.
    Clarify,
    /// The slot of the last heard line, character by character.
    Spell(Option<String>),
    Rate(f32),
    Volume(f32),
    Verbosity(Verbosity),
    /// The new language, and the voice for it.
    Language {
        code: String,
        voice: Option<String>,
    },
    /// Another voice of the provider, in the session's language.
    Voice(String),
    Backchannels(bool),
    Updates(Announce),
    ToScreen(Option<String>),
    End,
    /// The operator's transfer to a person.
    Human,
}

/// The executor's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub outcome: Outcome,
    pub detail: String,
    pub effect: Effect,
    /// The honest line when the control was not applied in full.
    pub line: Option<Say>,
}

impl Applied {
    fn done(effect: Effect, detail: impl Into<String>) -> Applied {
        Applied {
            outcome: Outcome::Applied,
            detail: detail.into(),
            effect,
            line: None,
        }
    }

    fn not(outcome: Outcome, detail: impl Into<String>, what: Option<&str>) -> Applied {
        let line = match outcome {
            Outcome::Refused => Say::Template {
                id: "control.refused",
                slots: Vec::new(),
            },
            _ => Say::Template {
                id: "control.unsupported",
                slots: vec![("what", what.unwrap_or("do that").to_owned())],
            },
        };
        Applied {
            outcome,
            detail: detail.into(),
            effect: Effect::None,
            line: Some(line),
        }
    }
}

/// One step of the speaking rate, and of the volume.
pub const RATE_STEP: f32 = 0.15;
pub const VOLUME_STEP_DB: f32 = 3.0;

/// Apply one control: the operator's bounds, then the session's capabilities. The state moves
/// on (a new version) for every control that was applied at least in part.
pub fn apply(
    control: &Control,
    state: &mut ConversationState,
    bounds: &ControlsConfig,
    caps: &Caps,
) -> Applied {
    let kind = control.kind;
    let source_allowed = match control.source {
        ControlSource::Caller | ControlSource::Memory => true,
        ControlSource::Agent => bounds.from_agents,
        ControlSource::Client => bounds.from_clients,
    };
    let applied = if !bounds.allowed.contains(&kind) || !source_allowed {
        Applied::not(Outcome::Refused, "not allowed by the operator", None)
    } else {
        execute(control, state, bounds, caps)
    };
    if applied.outcome != Outcome::Refused && applied.outcome != Outcome::Unsupported {
        state.held = match kind {
            ControlKind::Hold => true,
            ControlKind::Resume | ControlKind::Stop => false,
            _ => state.held,
        };
    }
    state.version += 1;
    state.controls.push(Record {
        kind,
        value: control.value.clone(),
        source: control.source,
        outcome: applied.outcome,
    });
    if state.controls.len() > RECORDS_KEPT {
        state.controls.remove(0);
    }
    applied
}

fn execute(
    control: &Control,
    state: &mut ConversationState,
    bounds: &ControlsConfig,
    caps: &Caps,
) -> Applied {
    let value = control.value.as_deref();
    match control.kind {
        ControlKind::Stop => Applied::done(Effect::Cut, "cut"),
        ControlKind::Hold => {
            let seconds = value.and_then(|seconds| seconds.parse().ok());
            Applied::done(Effect::Hold(seconds), "held")
        }
        ControlKind::Resume => Applied::done(Effect::Resume, "resumed"),
        ControlKind::Repeat | ControlKind::NotHeard => repeat(control, state, bounds, caps),
        ControlKind::Clarify => Applied::done(Effect::Clarify, "rephrased"),
        ControlKind::Spell => Applied::done(Effect::Spell(value.map(str::to_owned)), "spelt"),
        ControlKind::Rate if !caps.rate => Applied::not(
            Outcome::Unsupported,
            "the voice takes no rate mid-session",
            Some("change my speed"),
        ),
        ControlKind::Rate => {
            let rate = match value {
                Some("slower") => state.rate - RATE_STEP,
                Some("faster") => state.rate + RATE_STEP,
                _ => 1.0,
            };
            let bounded = rate.clamp(bounds.rate_min, bounds.rate_max);
            // Rounded, so steps add up to the operator's bounds (0.85, 0.70).
            let bounded = (bounded * 100.0).round() / 100.0;
            if (bounded - state.rate).abs() < f32::EPSILON && value != Some("normal") {
                let detail = format!("the rate is at the operator's bound {bounded}");
                return Applied::not(Outcome::Unsupported, detail, Some("change my speed"));
            }
            state.rate = bounded;
            Applied::done(Effect::Rate(bounded), format!("rate {bounded}"))
        }
        ControlKind::Volume => {
            let volume = match value {
                Some("louder") => state.volume_db + VOLUME_STEP_DB,
                Some("quieter") => state.volume_db - VOLUME_STEP_DB,
                _ => 0.0,
            };
            let bounded = volume.clamp(bounds.volume_min_db, bounds.volume_max_db);
            if (bounded - state.volume_db).abs() < f32::EPSILON && value != Some("normal") {
                let detail = format!("the volume is at the operator's bound {bounded} dB");
                return Applied::not(Outcome::Unsupported, detail, Some("change my volume"));
            }
            state.volume_db = bounded;
            Applied::done(Effect::Volume(bounded), format!("volume {bounded} dB"))
        }
        ControlKind::Verbosity => {
            let verbosity = match value {
                Some("brief") => Verbosity::Brief,
                Some("detailed") => Verbosity::Detailed,
                _ => Verbosity::Normal,
            };
            state.verbosity = verbosity;
            Applied::done(
                Effect::Verbosity(verbosity),
                format!("verbosity {}", value.unwrap_or("normal")),
            )
        }
        ControlKind::Language => language(value.unwrap_or_default(), state, bounds, caps),
        ControlKind::Voice => voice(value, state, caps),
        ControlKind::Backchannels => {
            let on = value == Some("on");
            state.backchannels = on;
            Applied::done(
                Effect::Backchannels(on),
                format!("backchannels {}", if on { "on" } else { "off" }),
            )
        }
        ControlKind::Updates => {
            let announce = match value {
                Some("tell") => Announce::Tell,
                Some("screen") => Announce::Screen,
                _ => Announce::Ask,
            };
            state.updates = announce;
            Applied::done(
                Effect::Updates(announce),
                format!("updates {}", value.unwrap_or("ask")),
            )
        }
        ControlKind::ToScreen if !caps.screen => Applied::not(
            Outcome::Unsupported,
            "no screen is attached",
            Some("send that to your screen"),
        ),
        ControlKind::ToScreen => Applied::done(
            Effect::ToScreen(value.map(str::to_owned)),
            "sent to the screen",
        ),
        ControlKind::End => Applied::done(Effect::End, "the call ends"),
        ControlKind::Human if !caps.transfer => Applied {
            line: Some(Say::Template {
                id: "control.human_unavailable",
                slots: Vec::new(),
            }),
            ..Applied::not(Outcome::Unsupported, "no transfer is configured", None)
        },
        ControlKind::Human => Applied::done(Effect::Human, "put through to a person"),
    }
}

/// `repeat` and `not_heard`: the last heard line again; `slower` and `louder` (both for
/// `not_heard`) change that line only, one step within the operator's bounds. A step that
/// cannot be taken (no rate on this voice, a bound reached) makes it `partial`; the line is
/// said again all the same.
fn repeat(
    control: &Control,
    state: &ConversationState,
    bounds: &ControlsConfig,
    caps: &Caps,
) -> Applied {
    let value = control.value.as_deref();
    let slot = value.filter(|what| SLOTS.contains(what)).map(str::to_owned);
    let not_heard = control.kind == ControlKind::NotHeard;
    let slower = not_heard || value == Some("slower");
    let louder = not_heard || value == Some("louder");
    let rate = ((state.rate - RATE_STEP).max(bounds.rate_min) * 100.0).round() / 100.0;
    let rate = (slower && caps.rate && rate < state.rate).then_some(rate);
    let gain_db = if louder {
        (state.volume_db + VOLUME_STEP_DB).min(bounds.volume_max_db) - state.volume_db
    } else {
        0.0
    };
    let mut missing = Vec::new();
    if slower && rate.is_none() {
        missing.push("no slower rate on this voice");
    }
    if louder && gain_db <= 0.0 {
        missing.push("the volume is at the operator's bound");
    }
    let effect = Effect::Repeat {
        slot,
        rate,
        gain_db: gain_db.max(0.0),
    };
    let mut detail = "repeated".to_owned();
    if let Some(rate) = rate {
        detail.push_str(&format!(", rate {rate} for the line"));
    }
    if gain_db > 0.0 {
        detail.push_str(&format!(", {gain_db} dB louder for the line"));
    }
    if missing.is_empty() {
        return Applied::done(effect, detail);
    }
    Applied {
        outcome: Outcome::Partial,
        detail: format!("{detail}; {}", missing.join("; ")),
        effect,
        line: None,
    }
}

/// The language of a voice of a provider's list: its id's last part when that is a code
/// (`aura-2-celeste-es`), else its language's name.
pub fn voice_language(voice: &Voice) -> Option<String> {
    let suffix = voice.id.rsplit(['-', '_']).next();
    let suffix = suffix.filter(|part| part.len() == 2);
    suffix
        .and_then(language_code)
        .or_else(|| language_of_name(&voice.language))
}

/// A provider's voices as (language, voice id), the voices of the gender of the session's own
/// voice first.
pub fn voices_by_language(voices: &[Voice], current: Option<&str>) -> Vec<(String, String)> {
    let gender = current
        .and_then(|id| voices.iter().find(|voice| voice.id == id))
        .map(|voice| voice.gender.clone());
    let mut found: Vec<(bool, String, String)> = voices
        .iter()
        .filter_map(|voice| {
            let other = gender
                .as_ref()
                .is_some_and(|gender| *gender != voice.gender);
            Some((other, voice_language(voice)?, voice.id.clone()))
        })
        .collect();
    found.sort_by_key(|(other, ..)| *other);
    found.into_iter().map(|(_, code, id)| (code, id)).collect()
}

/// `voice`: another voice of the provider's list in the session's language, with the gender,
/// age and accent asked for (each one asked must match the list's label); the gender of the
/// voice in use when none is asked. `unsupported` when the list has none.
fn voice(value: Option<&str>, state: &mut ConversationState, caps: &Caps) -> Applied {
    let unsupported =
        |detail: String| Applied::not(Outcome::Unsupported, detail, Some("change to that voice"));
    if caps.catalog.is_empty() {
        return unsupported("the voice provider's voice list is not available".to_owned());
    }
    let words: Vec<&str> = value.unwrap_or_default().split_whitespace().collect();
    let gender = words.iter().find_map(|word| Gender::from_str(word));
    let age = words.iter().find_map(|word| Age::from_str(word));
    let accent = words
        .iter()
        .find(|word| Gender::from_str(word).is_none() && Age::from_str(word).is_none());
    let current = state.voice.clone().or_else(|| caps.own_voice.clone());
    let in_use = current
        .as_deref()
        .and_then(|id| caps.catalog.iter().find(|voice| voice.id == id));
    let fits = |voice: &&Voice| {
        Some(voice.id.as_str()) != current.as_deref()
            && voice_language(voice).is_some_and(|code| same_language(&code, &state.language))
            && gender.is_none_or(|gender| Gender::from_str(&voice.gender) == Some(gender))
            && age.is_none_or(|age| Age::from_str(&voice.age) == Some(age))
            && accent.is_none_or(|accent| voice.accent.to_ascii_lowercase().contains(accent))
    };
    let mut found: Vec<&Voice> = caps.catalog.iter().filter(fits).collect();
    // Unasked, the gender stays the voice's own; then a voice of the same family as the one in
    // use (`aura-2-` before `aura-`), then the list's order (stable).
    let family = |id: &str| id.splitn(3, '-').take(2).collect::<Vec<_>>().join("-");
    found.sort_by_key(|voice| {
        let other_gender = in_use.is_some_and(|own| own.gender != voice.gender);
        let other_family = current
            .as_deref()
            .is_some_and(|own| family(own) != family(&voice.id));
        (gender.is_none() && other_gender, other_family)
    });
    let Some(chosen) = found.first() else {
        let asked = if words.is_empty() {
            "other".to_owned()
        } else {
            words.join(" ")
        };
        return unsupported(format!(
            "no {asked} voice of this provider in {}",
            language_name(&state.language)
        ));
    };
    state.voice = Some(chosen.id.clone());
    Applied {
        line: Some(Say::Template {
            id: "control.voice_changed",
            slots: Vec::new(),
        }),
        ..Applied::done(
            Effect::Voice(chosen.id.clone()),
            format!("voice {}", chosen.id),
        )
    }
}

/// A language switch: allowed by the operator, heard by the recogniser, a voice for it, and a
/// reviewed pack (or `answers_only`, which makes it `partial`).
fn language(
    code: &str,
    state: &mut ConversationState,
    bounds: &ControlsConfig,
    caps: &Caps,
) -> Applied {
    let name = language_name(code);
    if same_language(code, &state.language) {
        return Applied::done(Effect::None, format!("already {code}"));
    }
    // Why the session cannot speak a language; `None` when it can.
    let missing = |code: &str| {
        let name = language_name(code);
        if !bounds
            .languages
            .iter()
            .any(|allowed| same_language(allowed, code))
        {
            Some(format!("{name} is not among the operator's languages"))
        } else if !caps.hears(code) {
            Some(format!("the recogniser does not hear {name}"))
        } else if caps.voice(code).is_none() {
            Some(format!("the voice provider has no {name} voice"))
        } else if !bounds.answers_only && !caps.packs.iter().any(|pack| same_language(pack, code)) {
            Some(format!("no reviewed language pack for {name}"))
        } else {
            None
        }
    };
    if let Some(detail) = missing(code) {
        // The caller hears which languages the line has: its reviewed packs' (the gateway's
        // own lines stay in them), the one it speaks now and those it can switch to.
        let mut names: Vec<String> = Vec::new();
        let others = bounds
            .languages
            .iter()
            .filter(|other| missing(other).is_none());
        let codes = caps.packs.iter().chain([&state.language]).chain(others);
        for name in codes.map(|code| language_name(code)) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let languages = match names.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            _ => names.concat(),
        };
        return Applied {
            line: Some(Say::Template {
                id: "control.language_unsupported",
                slots: vec![("languages", languages)],
            }),
            ..Applied::not(Outcome::Unsupported, detail, None)
        };
    }
    let voice = caps.voice(code).map(str::to_owned).unwrap_or_default();
    let reviewed = caps.packs.iter().any(|pack| same_language(pack, code));
    state.language = code.to_owned();
    state.voice = Some(voice.clone());
    let effect = Effect::Language {
        code: code.to_owned(),
        voice: Some(voice),
    };
    if reviewed {
        return Applied::done(effect, format!("language {code}"));
    }
    Applied {
        outcome: Outcome::Partial,
        detail: format!(
            "language {code}: the answers only; the gateway's own lines stay in English"
        ),
        effect,
        line: Some(Say::Template {
            id: "control.language_answers_only",
            slots: vec![("language", name)],
        }),
    }
}

/// The note the history gets for an applied control (`[language switched to es]`).
pub fn history_note(control: &Control, applied: &Applied, state: &ConversationState) -> String {
    let value = control.value.as_deref().unwrap_or_default();
    let what = match control.kind {
        ControlKind::Stop => "the caller asked the agent to stop talking".to_owned(),
        ControlKind::Hold => "the caller asked the agent to hold on".to_owned(),
        ControlKind::Resume => "the caller asked the agent to go on".to_owned(),
        ControlKind::Repeat | ControlKind::NotHeard if value.is_empty() || value == "last" => {
            "the caller asked to hear the last line again".to_owned()
        }
        ControlKind::Repeat if value == "slower" || value == "louder" => {
            format!("the caller asked to hear the last line again, {value}")
        }
        ControlKind::Repeat | ControlKind::NotHeard => {
            format!("the caller asked to hear the {value} again")
        }
        ControlKind::Clarify => "the caller asked what the agent meant".to_owned(),
        ControlKind::Spell => "the caller asked for a spelling".to_owned(),
        ControlKind::Rate => format!("the caller asked to speak {value}; rate {}", state.rate),
        ControlKind::Volume => format!(
            "the caller asked for {value}; volume {} dB",
            state.volume_db
        ),
        ControlKind::Verbosity => format!("the caller asked for {value} replies"),
        ControlKind::Language if control.source == ControlSource::Memory => {
            let wish = format!(
                "the caller asked for {} on an earlier call",
                language_name(value)
            );
            match applied.outcome {
                Outcome::Applied | Outcome::Partial => {
                    format!("{wish}; language switched to {}", state.language)
                }
                _ => format!("{wish}; not available"),
            }
        }
        ControlKind::Language => match applied.outcome {
            Outcome::Applied | Outcome::Partial => {
                format!("language switched to {}", state.language)
            }
            _ => format!(
                "the caller asked for {}; not available",
                language_name(value)
            ),
        },
        ControlKind::Voice => match &applied.effect {
            Effect::Voice(id) => format!("the caller asked for another voice; voice {id}"),
            _ => "the caller asked for another voice".to_owned(),
        },
        ControlKind::Backchannels => format!("listening sounds {value}"),
        ControlKind::Updates => format!("updates: {value}"),
        ControlKind::ToScreen => "the caller asked for it on the screen".to_owned(),
        ControlKind::End => "the caller ended the call".to_owned(),
        ControlKind::Human => "the caller asked for a person".to_owned(),
    };
    let outcome = match applied.outcome {
        Outcome::Applied => "",
        Outcome::Partial => " (partly)",
        Outcome::Unsupported => " (not possible here)",
        Outcome::Refused => " (not allowed)",
    };
    format!("[{what}{outcome}]")
}

// ---- the slots of a heard line

fn slot_pattern(slot: &str) -> Option<&'static Regex> {
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    static TIME: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    let (cell, pattern) = match slot {
        // Digits with the separators a spoken number carries: "$1,240.50", "555 0134".
        "number" => (&NUMBER, r"[$£€]?\d(?:[\d,.\- ]*\d)?"),
        "time" => (
            &TIME,
            r"(?i)\b\d{1,2}(?::\d{2})?\s*(?:[ap]\.m\.|[ap]m\b)|\b\d{1,2}:\d{2}\b",
        ),
        "code" => (&CODE, r"\b(?:[A-Z]+\d|\d+[A-Z])[A-Z0-9]*\b"),
        _ => return None,
    };
    Some(cell.get_or_init(|| Regex::new(pattern).expect("a valid slot pattern")))
}

/// The named slot of a line the caller heard ("repeat the number": the number only). `None`
/// when the line has none, or the slot is one no pattern finds (a name, an address).
pub fn slot_of(line: &str, slot: &str) -> Option<String> {
    let found = slot_pattern(slot)?.find(line);
    // A code may be digits alone ("your code is 4 4 1 7").
    let found = match found {
        None if slot == "code" => slot_pattern("number")?.find(line),
        found => found,
    }?;
    Some(found.as_str().trim().to_owned())
}

/// A slot spelt out, one character at a time ("4, 1, 1"; "A, B, 1").
pub fn spelt(text: &str) -> String {
    let chars: Vec<String> = text
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase().to_string())
        .collect();
    chars.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> ConversationState {
        ConversationState::new("en", 1.0, None, true)
    }

    fn caps() -> Caps {
        Caps {
            rate: true,
            hears_any: true,
            voices: vec![("es".to_owned(), "aura-2-celeste-es".to_owned())],
            packs: vec!["en".to_owned()],
            screen: true,
            ..Caps::default()
        }
    }

    fn control(kind: ControlKind, value: Option<&str>, source: ControlSource) -> Control {
        Control::new(kind, value, source, None).expect("a valid control")
    }

    #[test]
    fn every_source_produces_the_same_control() {
        let from_client = Control::from_json(
            &json!({"kind": "rate", "value": "slower"}),
            ControlSource::Client,
        )
        .expect("client");
        let from_tool = Control::from_json(
            &json!({"kind": "rate", "value": "slower"}),
            ControlSource::Agent,
        )
        .expect("tool");
        let from_front = control(ControlKind::Rate, Some("slower"), ControlSource::Caller);
        let strip = |c: &Control| (c.kind, c.value.clone(), c.scope);
        assert_eq!(strip(&from_client), strip(&from_front));
        assert_eq!(strip(&from_tool), strip(&from_front));
        assert!(
            Control::from_json(
                &json!({"kind": "rate", "value": "loud"}),
                ControlSource::Client
            )
            .is_err()
        );
        assert!(Control::from_json(&json!({"kind": "dance"}), ControlSource::Client).is_err());
        let spanish = Control::from_json(
            &json!({"kind": "language", "value": "es_mx"}),
            ControlSource::Client,
        );
        assert_eq!(spanish.expect("language").value.as_deref(), Some("es-MX"));
        assert_eq!(
            control(ControlKind::Repeat, None, ControlSource::Caller).scope,
            Scope::Turn
        );
    }

    #[test]
    fn rate_steps_stay_within_the_operators_bounds() {
        let (mut state, bounds) = (state(), ControlsConfig::default());
        let slower = control(ControlKind::Rate, Some("slower"), ControlSource::Caller);
        assert_eq!(
            apply(&slower, &mut state, &bounds, &caps()).effect,
            Effect::Rate(0.85)
        );
        assert_eq!(
            apply(&slower, &mut state, &bounds, &caps()).effect,
            Effect::Rate(0.7)
        );
        let third = apply(&slower, &mut state, &bounds, &caps());
        assert_eq!(third.outcome, Outcome::Unsupported);
        assert!(third.line.is_some(), "said honestly");
        assert_eq!(state.rate, 0.7);
        let normal = control(ControlKind::Rate, Some("normal"), ControlSource::Caller);
        assert_eq!(
            apply(&normal, &mut state, &bounds, &caps()).effect,
            Effect::Rate(1.0)
        );
        // A voice that takes no rate: unsupported, said.
        let fixed = Caps {
            rate: false,
            ..caps()
        };
        let refused = apply(&slower, &mut state, &bounds, &fixed);
        assert_eq!(refused.outcome, Outcome::Unsupported);
        assert_eq!(state.rate, 1.0);
        assert_eq!(state.version, 5);
        let louder = control(ControlKind::Volume, Some("louder"), ControlSource::Caller);
        for _ in 0..3 {
            apply(&louder, &mut state, &bounds, &caps());
        }
        assert_eq!(
            apply(&louder, &mut state, &bounds, &caps()).outcome,
            Outcome::Unsupported
        );
        assert_eq!(state.volume_db, 9.0);
    }

    #[test]
    fn an_agent_cannot_issue_a_control_the_operator_did_not_allow() {
        let bounds = ControlsConfig {
            allowed: vec![ControlKind::Stop, ControlKind::Rate],
            ..ControlsConfig::default()
        };
        let mut state = state();
        let language = control(ControlKind::Language, Some("es"), ControlSource::Agent);
        let refused = apply(&language, &mut state, &bounds, &caps());
        assert_eq!(refused.outcome, Outcome::Refused);
        assert_eq!(state.language, "en");
        let no_agents = ControlsConfig {
            from_agents: false,
            ..ControlsConfig::default()
        };
        let rate = control(ControlKind::Rate, Some("slower"), ControlSource::Agent);
        assert_eq!(
            apply(&rate, &mut state, &no_agents, &caps()).outcome,
            Outcome::Refused
        );
        let by_caller = control(ControlKind::Rate, Some("slower"), ControlSource::Caller);
        assert_eq!(
            apply(&by_caller, &mut state, &no_agents, &caps()).outcome,
            Outcome::Applied
        );
    }

    #[test]
    fn a_language_without_a_pack_is_reported_unsupported_and_said_honestly() {
        let bounds = ControlsConfig {
            languages: vec!["es".to_owned(), "zh".to_owned()],
            ..ControlsConfig::default()
        };
        let mut state = state();
        let chinese = control(ControlKind::Language, Some("zh"), ControlSource::Caller);
        let answer = apply(&chinese, &mut state, &bounds, &caps());
        assert_eq!(answer.outcome, Outcome::Unsupported);
        assert_eq!(
            answer.line,
            Some(Say::Template {
                id: "control.language_unsupported",
                slots: vec![("languages", "English".to_owned())]
            })
        );
        assert_eq!(state.language, "en");
        // Spanish has a voice but no reviewed pack: unsupported, unless answers_only.
        let spanish = control(ControlKind::Language, Some("es"), ControlSource::Caller);
        assert_eq!(
            apply(&spanish, &mut state, &bounds, &caps()).outcome,
            Outcome::Unsupported
        );
        let answers_only = ControlsConfig {
            answers_only: true,
            ..bounds.clone()
        };
        let partial = apply(&spanish, &mut state, &answers_only, &caps());
        assert_eq!(partial.outcome, Outcome::Partial);
        assert_eq!(
            partial.effect,
            Effect::Language {
                code: "es".to_owned(),
                voice: Some("aura-2-celeste-es".to_owned())
            }
        );
        assert_eq!(state.language, "es");
        assert_eq!(
            history_note(&spanish, &partial, &state),
            "[language switched to es (partly)]"
        );
        // Not among the operator's languages at all; the line names the reviewed pack's
        // language (the gateway's own lines) and the one it answers in now (live CT2 named
        // only Spanish).
        let german = control(ControlKind::Language, Some("de"), ControlSource::Caller);
        let refused = apply(&german, &mut state, &answers_only, &caps());
        assert_eq!(refused.outcome, Outcome::Unsupported);
        assert_eq!(
            refused.line,
            Some(Say::Template {
                id: "control.language_unsupported",
                slots: vec![("languages", "English and Spanish".to_owned())]
            })
        );
    }

    #[test]
    fn repeat_the_number_says_only_the_number() {
        let line = "Your confirmation number is 4 4 1 7, and the flight leaves at 10:30 am.";
        assert_eq!(slot_of(line, "number").as_deref(), Some("4 4 1 7"));
        assert_eq!(slot_of(line, "time").as_deref(), Some("10:30 am"));
        assert_eq!(
            slot_of("The total is $1,240.50 today.", "number").as_deref(),
            Some("$1,240.50")
        );
        assert_eq!(
            slot_of("Your code is AB12C.", "code").as_deref(),
            Some("AB12C")
        );
        assert_eq!(slot_of("No digits here.", "number"), None);
        assert_eq!(slot_of(line, "name"), None);
        assert_eq!(slot_of(line, "code").as_deref(), Some("4 4 1 7"));
        assert_eq!(spelt("AB12C"), "A, B, 1, 2, C");
    }

    #[test]
    fn a_repeat_slower_or_louder_changes_that_line_only() {
        let (mut state, bounds) = (state(), ControlsConfig::default());
        let slower = control(ControlKind::Repeat, Some("slower"), ControlSource::Caller);
        let applied = apply(&slower, &mut state, &bounds, &caps());
        assert_eq!(
            applied.effect,
            Effect::Repeat {
                slot: None,
                rate: Some(0.85),
                gain_db: 0.0
            }
        );
        // The session's own rate and volume do not move.
        assert_eq!((state.rate, state.volume_db), (1.0, 0.0));
        assert_eq!(
            history_note(&slower, &applied, &state),
            "[the caller asked to hear the last line again, slower]"
        );
        let louder = control(ControlKind::Repeat, Some("louder"), ControlSource::Caller);
        assert_eq!(
            apply(&louder, &mut state, &bounds, &caps()).effect,
            Effect::Repeat {
                slot: None,
                rate: None,
                gain_db: 3.0
            }
        );
        // Not heard: both, one step each from where the session is.
        state.volume_db = 7.5;
        let missed = control(ControlKind::NotHeard, None, ControlSource::Caller);
        let both = apply(&missed, &mut state, &bounds, &caps());
        assert_eq!(both.outcome, Outcome::Applied);
        assert_eq!(
            both.effect,
            Effect::Repeat {
                slot: None,
                rate: Some(0.85),
                gain_db: 1.5
            }
        );
        // A voice without a rate, a volume at its bound: said again all the same, partial.
        state.volume_db = 9.0;
        let fixed = Caps {
            rate: false,
            ..caps()
        };
        let partial = apply(&missed, &mut state, &bounds, &fixed);
        assert_eq!(partial.outcome, Outcome::Partial);
        assert_eq!(
            partial.effect,
            Effect::Repeat {
                slot: None,
                rate: None,
                gain_db: 0.0
            }
        );
        assert!(partial.line.is_none(), "the line itself is said again");
        assert!(
            Control::new(
                ControlKind::Repeat,
                Some("quicker"),
                ControlSource::Caller,
                None
            )
            .is_err()
        );
    }

    fn listed(id: &str, gender: &str, accent: &str, age: &str, language: &str) -> Voice {
        Voice {
            id: id.to_owned(),
            sample: String::new(),
            name: id.to_owned(),
            accent: accent.to_owned(),
            gender: gender.to_owned(),
            language: language.to_owned(),
            age: age.to_owned(),
            description: String::new(),
            use_case: String::new(),
        }
    }

    #[test]
    fn a_voice_is_chosen_from_the_providers_list_in_the_sessions_language() {
        let catalog = vec![
            listed("aura-2-thalia-en", "Female", "American", "", "English"),
            listed("aura-2-andromeda-en", "Female", "American", "", "English"),
            listed("aura-orion-en", "Male", "American", "", "English"),
            listed("aura-2-draco-en", "Male", "British", "", "English"),
            listed("aura-2-zeus-en", "Male", "American", "", "English"),
            listed("aura-2-nestor-es", "Male", "Peninsular", "", "es"),
            listed("george", "Male", "british", "middle_aged", "English"),
            listed("old-mary", "Female", "american", "old", "English"),
        ];
        let caps = Caps {
            catalog,
            own_voice: Some("aura-2-thalia-en".to_owned()),
            ..caps()
        };
        let bounds = ControlsConfig::default();
        let chosen = |value: Option<&str>, state: &mut ConversationState| {
            let voice = control(ControlKind::Voice, value, ControlSource::Caller);
            apply(&voice, state, &bounds, &caps)
        };
        let mut state = state();
        // Another voice, nothing asked: the same gender as the voice in use.
        let other = chosen(None, &mut state);
        assert_eq!(
            other.effect,
            Effect::Voice("aura-2-andromeda-en".to_owned())
        );
        assert_eq!(state.voice.as_deref(), Some("aura-2-andromeda-en"));
        assert_eq!(
            other.line,
            Some(Say::Template {
                id: "control.voice_changed",
                slots: Vec::new()
            })
        );
        // A male voice: English, never the Spanish one.
        let male = chosen(Some("male"), &mut state);
        assert_eq!(male.effect, Effect::Voice("aura-2-draco-en".to_owned()));
        let male_american = chosen(Some("male american"), &mut state);
        assert_eq!(
            male_american.effect,
            Effect::Voice("aura-2-zeus-en".to_owned())
        );
        assert_eq!(
            chosen(Some("old"), &mut state).effect,
            Effect::Voice("old-mary".to_owned())
        );
        // Nothing fits: said honestly, the voice stays.
        let irish = chosen(Some("irish"), &mut state);
        assert_eq!(irish.outcome, Outcome::Unsupported);
        assert!(irish.detail.contains("no irish voice"), "{}", irish.detail);
        assert!(irish.line.is_some());
        assert_eq!(state.voice.as_deref(), Some("old-mary"));
        // Without the list nothing is chosen.
        let mut fresh = ConversationState::new("en", 1.0, None, true);
        let none = apply(
            &control(ControlKind::Voice, Some("male"), ControlSource::Caller),
            &mut fresh,
            &bounds,
            &self::caps(),
        );
        assert_eq!(none.outcome, Outcome::Unsupported);
        assert!(
            Control::new(
                ControlKind::Voice,
                Some("robot"),
                ControlSource::Caller,
                None
            )
            .is_err()
        );
        assert!(
            Control::new(
                ControlKind::Voice,
                Some("male old british"),
                ControlSource::Caller,
                None
            )
            .is_err()
        );
        assert_eq!(
            voices_by_language(&caps.catalog, Some("aura-2-thalia-en"))[..2],
            [
                ("en".to_owned(), "aura-2-thalia-en".to_owned()),
                ("en".to_owned(), "aura-2-andromeda-en".to_owned())
            ]
        );
    }

    #[test]
    fn a_person_is_put_through_only_where_a_transfer_is_configured() {
        let (mut state, bounds) = (state(), ControlsConfig::default());
        let human = control(ControlKind::Human, None, ControlSource::Caller);
        let none = apply(&human, &mut state, &bounds, &caps());
        assert_eq!(none.outcome, Outcome::Unsupported);
        assert_eq!(
            none.line,
            Some(Say::Template {
                id: "control.human_unavailable",
                slots: Vec::new()
            })
        );
        let configured = Caps {
            transfer: true,
            ..caps()
        };
        let put_through = apply(&human, &mut state, &bounds, &configured);
        assert_eq!(put_through.effect, Effect::Human);
        assert_eq!(put_through.outcome, Outcome::Applied);
    }

    #[test]
    fn a_language_wish_of_an_earlier_call_is_noted_as_such() {
        let bounds = ControlsConfig {
            languages: vec!["es".to_owned()],
            answers_only: true,
            ..ControlsConfig::default()
        };
        let mut state = state();
        let wish = control(ControlKind::Language, Some("es"), ControlSource::Memory);
        let applied = apply(&wish, &mut state, &bounds, &caps());
        assert_eq!(applied.outcome, Outcome::Partial);
        assert_eq!(
            history_note(&wish, &applied, &state),
            "[the caller asked for Spanish on an earlier call; language switched to es (partly)]"
        );
        let german = control(ControlKind::Language, Some("de"), ControlSource::Memory);
        let refused = apply(&german, &mut state, &bounds, &caps());
        assert_eq!(
            history_note(&german, &refused, &state),
            "[the caller asked for German on an earlier call; not available (not possible here)]"
        );
    }

    #[test]
    fn the_state_and_notes_follow_each_control() {
        let (mut state, bounds) = (state(), ControlsConfig::default());
        let brief = control(ControlKind::Verbosity, Some("brief"), ControlSource::Client);
        let applied = apply(&brief, &mut state, &bounds, &caps());
        assert_eq!(applied.effect, Effect::Verbosity(Verbosity::Brief));
        assert_eq!(
            history_note(&brief, &applied, &state),
            "[the caller asked for brief replies]"
        );
        let hold = control(ControlKind::Hold, None, ControlSource::Caller);
        apply(&hold, &mut state, &bounds, &caps());
        assert!(state.held);
        apply(
            &control(ControlKind::Resume, None, ControlSource::Caller),
            &mut state,
            &bounds,
            &caps(),
        );
        assert!(!state.held);
        let json = state.to_json();
        assert_eq!(json["version"], 3);
        assert_eq!(json["verbosity"], "brief");
        assert_eq!(json["controls"][0]["kind"], "verbosity");
        assert!(state.note().contains("answer in English (en)"));
        let human = apply(
            &control(ControlKind::Human, None, ControlSource::Caller),
            &mut state,
            &bounds,
            &caps(),
        );
        assert_eq!(human.outcome, Outcome::Unsupported);
    }
}

//! Interaction-mode configuration: the session block (`interaction: true | {...}` on the
//! `/ws` config message), the server block (YAML key `interaction` plus `WAAV_INTERACTION_*`),
//! mid-session updates and the echo sent in `ready.interaction`.
//!
//! The session parser is lenient: it walks the JSON by hand, and a wrong type, an unknown key,
//! an out-of-range number or a refused key produces one [`ConfigWarning`] with `detail.path`
//! while the default is kept. A `null` value counts as "not set".

use std::collections::BTreeMap;
use std::fmt;
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::utils::parse_bool;
use crate::handlers::ws::messages::INTERACTION_PROTOCOL_VERSION;

use super::controls::{ControlKind, ControlsConfig, language_code};
use super::persona::{self, Persona};
use super::types::{Announce, Pace, Verbosity};

/// The path of the block itself in `detail.path`.
const ROOT: &str = "interaction";
/// Keys of the server block; a session that sends one gets `interaction_override_refused`.
const SERVER_ONLY_KEYS: [&str; 17] = [
    "enabled",
    "realtime_legacy_wire",
    "provider_fixes",
    "provider_fixes_per_session",
    "packs_dir",
    "clips_dir",
    "failure_voice",
    "default",
    "agent_default",
    "realtime_default",
    "truth_ack",
    "front_model",
    "tool_secret",
    "allow_client_tools",
    "task_redis_url",
    "tenant_personas",
    "caller_memory_key",
];
/// The values of `filler_owner`, `backchannel_owner` and `idle_owner`.
const OWNERS: [&str; 4] = ["auto", "gateway", "provider", "none"];
/// The built-in tool a background run uses to ask the caller; a configured tool cannot take its name.
pub(super) const ASK_CALLER: &str = "ask_caller";

// ---- session block

#[derive(Debug, Clone, PartialEq)]
pub struct InteractionConfig {
    pub front: FrontConfig,
    pub tasks: TasksConfig,
    pub policy: PolicyConfig,
    pub duplex: DuplexConfig,
    /// `events.floor`: send `floor_state` events.
    pub events_floor: bool,
    /// `events.segments`: send `audio_segment` events.
    pub events_segments: bool,
    /// `hitl.channels` holds `client`: the client shows questions on a screen, so a form, a
    /// sign-in link and a sensitive value can be asked there.
    pub hitl_client: bool,
    /// `hitl.channels` holds `dtmf`: a caller on a phone line answers a confirmation and types
    /// a sensitive value on the keypad.
    pub hitl_dtmf: bool,
    /// `regime`, the owners and `speech.*`: read by the speech-to-speech surfaces only.
    pub speech: SpeechConfig,
    /// `speech.voice`: the gateway's own voice on a speech-to-speech session (a `tts_config`
    /// object): the half-cascade speaks with it and the gateway's clips are rendered in it.
    /// `None`: the server's `failure_voice`.
    pub speech_voice: Option<Value>,
    /// `controls`: the operator's bounds of the conversation controls (chapter 9).
    pub controls: ControlsConfig,
    /// `failure`: when the lines said while a service fails play (chapter 10).
    pub failure: FailureConfig,
}

/// `failure` (plan chapter 10): how long a service may be silent before a stall line, how
/// long an outage may last before the call ends, the operator's own words for the lines, and
/// whether the operator follows up on a call that ended so.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FailureConfig {
    /// The background's first token, a synthesis' first audio: later than this, a stall.
    pub stall_after_ms: u32,
    /// An outage this long after the first stall line ends the call.
    pub max_wait_s: u32,
    /// The operator's words of lines of the pack's `failure` group, by line id. Only the
    /// operator sets them.
    #[serde(skip)]
    pub lines: BTreeMap<String, String>,
    /// The operator's system follows up on a call that ended on a failure: the exit line may
    /// promise to get back. Only the operator sets it.
    pub follow_up: bool,
}

impl Default for FailureConfig {
    fn default() -> Self {
        Self {
            stall_after_ms: 2_500,
            max_wait_s: 30,
            lines: BTreeMap::new(),
            follow_up: false,
        }
    }
}

/// The keys of the regime resolver (`regime::resolve_session`): which regime a
/// speech-to-speech session may run, who fills silences, how true its speech must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SpeechConfig {
    /// `auto`, `cascade`, `provider_speech` or `delegating`.
    pub regime: &'static str,
    /// `auto`, `gateway`, `provider` or `none`, each.
    pub filler_owner: &'static str,
    pub backchannel_owner: &'static str,
    pub idle_owner: &'static str,
    /// `speech.voice_owner`: `auto`, `provider` or `gateway`.
    pub voice_owner: &'static str,
    /// `speech.truth`: `monitored` or `enforced`.
    pub truth: &'static str,
    /// `speech.narrator`: `templates` or `front_model`.
    pub narrator: &'static str,
    /// `speech.voice_mismatch`: `never`, `confirmations_only` or `always`.
    pub voice_mismatch: &'static str,
    /// `speech.claim_policy`: `report` or `correct`.
    pub claim_policy: &'static str,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            regime: "auto",
            filler_owner: "auto",
            backchannel_owner: "auto",
            idle_owner: "auto",
            voice_owner: "auto",
            truth: "monitored",
            narrator: "templates",
            voice_mismatch: "confirmations_only",
            claim_policy: "report",
        }
    }
}

impl SpeechConfig {
    /// The keys as the resolver reads a session block.
    pub fn block(&self) -> Value {
        json!({
            "regime": self.regime,
            "filler_owner": self.filler_owner,
            "backchannel_owner": self.backchannel_owner,
            "idle_owner": self.idle_owner,
            "speech": {
                "voice_owner": self.voice_owner,
                "truth": self.truth,
                "narrator": self.narrator,
                "voice_mismatch": self.voice_mismatch,
            },
        })
    }
}

impl Default for InteractionConfig {
    fn default() -> Self {
        Self {
            front: FrontConfig::default(),
            tasks: TasksConfig::default(),
            policy: PolicyConfig::default(),
            duplex: DuplexConfig::default(),
            events_floor: true,
            events_segments: true,
            hitl_client: true,
            hitl_dtmf: false,
            speech: SpeechConfig::default(),
            speech_voice: None,
            controls: ControlsConfig::default(),
            failure: FailureConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontProfile {
    /// What a session may ask for; [`parse`] resolves it, so a parsed config never holds it.
    #[default]
    Auto,
    Model,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct FrontConfig {
    pub profile: FrontProfile,
    /// The persona's id; an unknown id falls back to the default persona.
    pub persona: String,
    /// The persona itself, when a trusted block or the tenant's registry gave one: checked
    /// like the packaged personas when it was read.
    #[serde(skip)]
    pub spec: Option<Value>,
    /// May only be lowered.
    pub act_budget_ms: u32,
    pub total_budget_ms: u32,
    pub preferred_name: Option<String>,
    pub verbosity: Option<Verbosity>,
    pub pace: Option<Pace>,
    /// The operator-signed caller identity that keys the caller's memory. Never echoed.
    #[serde(skip)]
    pub identity_token: Option<String>,
    /// What is known about the caller from earlier calls (caller memory), read when the
    /// session starts. Never echoed.
    #[serde(skip)]
    pub memory: Vec<String>,
    /// The caller the identity token names, once verified: after the call the background
    /// adds what it learnt to the caller's memory. Never echoed.
    #[serde(skip)]
    pub caller: Option<String>,
    /// The language that caller asked for on an earlier call (caller memory), applied at the
    /// session's start. Never echoed.
    #[serde(skip)]
    pub language_wish: Option<String>,
    /// The language of the session's pack (`persona::pack_of` of its speech-to-text
    /// language); set by the surface, never by a block.
    #[serde(skip)]
    pub language: String,
}

impl Default for FrontConfig {
    fn default() -> Self {
        Self {
            profile: FrontProfile::Auto,
            persona: "default".to_owned(),
            spec: None,
            act_budget_ms: 300,
            total_budget_ms: 1500,
            preferred_name: None,
            verbosity: None,
            pace: None,
            identity_token: None,
            memory: Vec::new(),
            caller: None,
            language_wish: None,
            language: "en".to_owned(),
        }
    }
}

impl FrontConfig {
    /// The session's persona: the one it was given, else the one with its id in the
    /// session's pack.
    pub fn load_persona(&self) -> Arc<Persona> {
        self.spec
            .as_ref()
            .and_then(|spec| Persona::from_spec(spec).ok())
            .unwrap_or_else(|| Persona::load_in(&self.persona, &self.language))
    }
}

// The identity token and what is known of the caller stay out of logs.
impl fmt::Debug for FrontConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrontConfig")
            .field("profile", &self.profile)
            .field("persona", &self.persona)
            .field("spec", &self.spec.is_some())
            .field("act_budget_ms", &self.act_budget_ms)
            .field("total_budget_ms", &self.total_budget_ms)
            .field("verbosity", &self.verbosity)
            .field("pace", &self.pace)
            .field("identity_token", &self.identity_token.is_some())
            .field("memory", &self.memory.len())
            .field("caller", &self.caller.is_some())
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tri {
    #[default]
    Auto,
    On,
    Off,
}

#[derive(Clone, PartialEq)]
pub struct TasksConfig {
    /// Raw `/ws` sessions only: the same JSON shape as `conversation_config`. The `/ws`
    /// handler deserialises it; this module only checks that it is an object.
    pub llm: Option<Value>,
    pub tools: Vec<ToolSpec>,
    pub speculative_start: Tri,
    /// `background.answer_marker`: the line a Bud agent is known to write before its final
    /// answer. Only the agent's entry or the operator can declare it.
    pub answer_marker: Option<String>,
    /// How long a caller whose connection dropped may come back, with the same `stream_id`,
    /// for the result of a task that went on without them.
    pub reconnect_grace_s: u32,
    /// Where that result is pushed when nobody came back for it. The operator declares it
    /// (the push carries the server's signature), or, where the operator allows it
    /// (`interaction.allow_client_push`), the session with its own `push_secret`.
    pub push_url: Option<String>,
    /// `tasks.push_secret`: the secret a session's own `push_url` is signed with (the
    /// client's, so the client can check its pushes). Never echoed or logged.
    pub push_secret: Option<String>,
    /// `tasks.backends`: the catalog entries the session's tasks may run on. The `/ws` handler
    /// resolves them: one alone is the `default_backend`; several are an orchestrator's
    /// agents (the session's own LLM orchestrates when it names no orchestrator).
    pub backends: Vec<String>,
    /// `tasks.default_backend`: the entry of the operator's catalog that does the session's
    /// tasks. A session names it; the address and the secret stay on the server.
    pub backend: Option<String>,
    /// Which live tasks run on when the session ends (sign-off 15). Only the operator sets it.
    pub session_end: SessionEnd,
    /// How long a task may run on after its session ended. Only the operator sets it.
    pub orphan_max_s: u32,
    /// `tasks.orchestrator`: the model that does the session's tasks on the LLM tier with the
    /// session's `agents` as tools: a model name on the session's own LLM, or an object of
    /// the `tasks.llm` shape. The `/ws` handler resolves it.
    pub orchestrator: Option<Value>,
    /// `tasks.agents`: the catalog entries the orchestrator may start tasks on (names only).
    pub agents: Vec<String>,
    pub limits: TaskLimits,
    /// `tasks.announce`: how a task's update reaches the caller while the conversation goes
    /// on (an idle conversation hears it in any case but `screen`).
    pub announce: Announce,
    /// `tasks.follow_up_ttl_s`: how long after a task ended a follow-up continues it on its
    /// agent with its continuation handle (chapter 8).
    pub follow_up_ttl_s: u32,
    /// `tasks.resume_key`: what a caller who calls back, or moves to another device, comes
    /// back with on a new `stream_id`. A session sends it only where the operator allows it
    /// (`interaction.allow_resume_key`). Never echoed.
    pub resume_key: Option<String>,
}

/// `tasks.limits`: how many tasks run at once. Work beyond a limit waits in a queue; nothing
/// is refused for count. `None` is no limit (the default).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TaskLimits {
    /// The caller's requests of one session that run at once. A session may lower the
    /// operator's value, never raise it.
    pub max_live: Option<u32>,
    /// The caller's requests of all the tenant's sessions in this process that run at once.
    /// Only the operator sets it.
    pub max_live_tenant: Option<u32>,
}

/// `tasks.session_end`: which of a session's live tasks run on after it ends. A task whose
/// tool call is on the wire always finishes that call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEnd {
    /// A task that may change something, or whose result has a reader (a push address, or
    /// the caller of a lost connection for the grace), runs on; any other is cancelled.
    #[default]
    Auto,
    CancelAll,
    ContinueAll,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            llm: None,
            tools: Vec::new(),
            speculative_start: Tri::default(),
            answer_marker: None,
            reconnect_grace_s: 60,
            push_url: None,
            push_secret: None,
            backends: Vec::new(),
            backend: None,
            session_end: SessionEnd::Auto,
            orphan_max_s: 1800,
            orchestrator: None,
            agents: Vec::new(),
            limits: TaskLimits::default(),
            announce: Announce::Ask,
            follow_up_ttl_s: 1800,
            resume_key: None,
        }
    }
}

// Secrets stay out of logs: the push address and secret, the resume key, and the LLM and
// orchestrator objects (their `api_key`) show only whether they are set.
impl fmt::Debug for TasksConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TasksConfig")
            .field("llm", &self.llm.is_some())
            .field("tools", &self.tools)
            .field("speculative_start", &self.speculative_start)
            .field("answer_marker", &self.answer_marker)
            .field("reconnect_grace_s", &self.reconnect_grace_s)
            // A push address may carry a credential of its own (its query, its user info).
            .field("push_url", &self.push_url.is_some())
            .field("push_secret", &self.push_secret.is_some())
            .field("backends", &self.backends)
            .field("backend", &self.backend)
            .field("session_end", &self.session_end)
            .field("orphan_max_s", &self.orphan_max_s)
            .field("orchestrator", &self.orchestrator.is_some())
            .field("agents", &self.agents)
            .field("limits", &self.limits)
            .field("announce", &self.announce)
            .field("follow_up_ttl_s", &self.follow_up_ttl_s)
            .field("resume_key", &self.resume_key.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments.
    pub parameters: Value,
    pub url: String,
    /// The call is gated by a confirmation. Always `true` for an irreversible tool.
    pub side_effects: bool,
    pub irreversible: bool,
    pub confirm_phrase: Option<String>,
    /// The read-back of a confirmation in the tool author's words, with `{argument}` slots:
    /// "book a table for {party} at {time}".
    pub confirm_template: Option<String>,
    pub progress_phrase: Option<String>,
    /// The operator declared it (the server's default block): only such a call carries the
    /// server's signature. A session's own tool never does, or the signature would vouch
    /// for a URL and a confirmation rule the operator did not write.
    pub signed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PolicyConfig {
    pub ack_due_ms: u32,
    pub fast_answer_ms: u32,
    pub fast_answer_min_turns: u32,
    pub ack_hard_deadline_ms: u32,
    pub amend_ack_target_ms: u32,
    pub answer_window_ms: u32,
    pub heard_margin_ms: u32,
    pub progress_min_wait_ms: u32,
    pub progress_min_gap_ms: u32,
    pub updates_max_per_wait: u32,
    pub hold_after_silence_ms: u32,
    pub quiet_checkin_ms: u32,
    pub agreed_wait_max_ms: u32,
    pub hitl_reprompt_after_ms: u32,
    pub hitl_answer_timeout_ms: u32,
    /// A value typed on the keypad with no key for this long is answered with the keys so far.
    pub hitl_key_gap_ms: u32,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            ack_due_ms: 300,
            fast_answer_ms: 700,
            fast_answer_min_turns: 3,
            ack_hard_deadline_ms: 2000,
            amend_ack_target_ms: 1200,
            answer_window_ms: 4000,
            heard_margin_ms: 150,
            progress_min_wait_ms: 4000,
            progress_min_gap_ms: 5000,
            updates_max_per_wait: 3,
            hold_after_silence_ms: 8000,
            quiet_checkin_ms: 30_000,
            agreed_wait_max_ms: 600_000,
            hitl_reprompt_after_ms: 8000,
            hitl_answer_timeout_ms: 20_000,
            hitl_key_gap_ms: 5000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplexConfig {
    pub barge_in: bool,
    pub duck_db: f32,
    pub duck_ramp_ms: u32,
    pub pause_after_ms: u32,
    pub decide_by_ms: u32,
    pub max_pause_ms: u32,
    /// Eager end of turn.
    pub eager: bool,
    pub echo: EchoConfig,
    /// The agent's listening sounds while the caller talks. `Auto` is on unless the caller
    /// is on a phone line.
    pub backchannel: Tri,
    /// `duplex.backchannel.intensity`: may lower the persona's intensity, never raise it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backchannel_intensity: Option<Intensity>,
}

/// How often the agent makes listening sounds: the persona's `backchannel_intensity`, which
/// the session's `duplex.backchannel.intensity` may lower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Intensity {
    Off,
    Low,
    Medium,
    High,
}

/// The listening-sound numbers of one intensity (chapter 5's table; "probability" is a
/// deterministic schedule: every `skip_every`-th eligible pause passes without a sound, and
/// 0 skips none). A language pack's `backchannel.timing` holds its `medium` numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackchannelTiming {
    pub skip_every: u64,
    pub gap_ms: u64,
    /// The most in one caller turn; `None` is no limit.
    #[serde(default)]
    pub per_turn: Option<u32>,
    /// The caller's voiced speech in the turn before the first sound.
    pub min_voiced_ms: u32,
}

impl Default for BackchannelTiming {
    /// English (`phrase_banks.en.json`), for a pack that names no timing. Chapter 5 had
    /// 2,500 ms, 1 in 5 skipped and 700 ms; measured on Full-Duplex-Bench's ICC listeners
    /// that made about 6 a minute of caller speech (9 to 10 on its pause sets), above the
    /// 3 to 6 that listeners give, so English waits 6 s, skips 1 in 3 and needs 1.5 s.
    fn default() -> Self {
        BackchannelTiming {
            skip_every: 3,
            gap_ms: 6_000,
            per_turn: None,
            min_voiced_ms: 1_500,
        }
    }
}

impl Intensity {
    pub const WORDS: [(&'static str, Intensity); 4] = [
        ("off", Intensity::Off),
        ("low", Intensity::Low),
        ("medium", Intensity::Medium),
        ("high", Intensity::High),
    ];

    pub fn parse(word: &str) -> Option<Self> {
        Self::WORDS
            .iter()
            .find(|(name, _)| *name == word)
            .map(|(_, level)| *level)
    }

    /// `None` for `off`. `medium` is the language pack's numbers; the other levels scale them
    /// as chapter 5's table scales its English (p 0.8, 2.5 s, no limit, 700 ms): low skips every
    /// 2nd pause (p 0.5), waits twice as long, makes one a turn and needs 10/7 of the speech;
    /// high skips half as often (p 0.9), waits 18/25 as long and needs 5/7 of the speech.
    pub fn timing(self, medium: BackchannelTiming) -> Option<BackchannelTiming> {
        let m = medium;
        Some(match self {
            Intensity::Off => return None,
            Intensity::Low => BackchannelTiming {
                skip_every: (m.skip_every * 2 / 5).max(2),
                gap_ms: m.gap_ms * 2,
                per_turn: Some(1),
                min_voiced_ms: m.min_voiced_ms * 10 / 7,
            },
            Intensity::Medium => m,
            Intensity::High => BackchannelTiming {
                skip_every: m.skip_every * 2,
                gap_ms: m.gap_ms * 18 / 25,
                per_turn: m.per_turn,
                min_voiced_ms: m.min_voiced_ms * 5 / 7,
            },
        })
    }
}

/// The defences against the agent's own audio coming back on the caller's channel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EchoConfig {
    /// Voiced audio that the agent's delivered audio explains is not the caller.
    pub correlation_gate: bool,
    /// Echo cancellation ahead of the voice activity model and the speech-to-text. `Auto` is
    /// on when the caller is on a phone line, when the client says it cancels no echo
    /// itself (`client_aec: false`), and after `auto_enable_after_echo_episodes`.
    pub aec: Tri,
    /// A `/ws` client says whether it cancels echo itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_aec: Option<bool>,
    /// A `/ws` client says whether its caller is on a phone line; unset, an 8 kHz input is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_line: Option<bool>,
    /// For this long after echo was last heard, words with no sound of the caller's own are
    /// taken for the agent's.
    pub tail_ms: u32,
    /// The longest delay at which the echo probe looks for the agent's audio.
    pub max_bulk_delay_ms: u32,
    /// With `aec: auto`, the canceller is turned on for the rest of the session after this
    /// many stretches of echo within 30 s; 0 never.
    pub auto_enable_after_echo_episodes: u32,
}

impl Default for EchoConfig {
    fn default() -> Self {
        Self {
            correlation_gate: true,
            aec: Tri::Auto,
            client_aec: None,
            phone_line: None,
            tail_ms: 2_000,
            max_bulk_delay_ms: 1_000,
            auto_enable_after_echo_episodes: 2,
        }
    }
}

impl EchoConfig {
    /// Whether the echo canceller runs from the start, given whether the caller is on a
    /// phone line.
    pub fn aec_on(&self, phone_leg: bool) -> bool {
        match self.aec {
            Tri::On => true,
            Tri::Off => false,
            Tri::Auto => phone_leg || self.client_aec == Some(false),
        }
    }

    /// Whether the caller is on a phone line: a room's SIP participant, a client that says
    /// so, or (unless the client says otherwise) an 8 kHz input.
    pub fn phone_line(&self, sip: bool, input_rate: u32) -> bool {
        sip || self.phone_line.unwrap_or(input_rate == 8_000)
    }
}

impl Default for DuplexConfig {
    fn default() -> Self {
        Self {
            barge_in: true,
            duck_db: -15.0,
            duck_ramp_ms: 60,
            pause_after_ms: 400,
            decide_by_ms: 900,
            max_pause_ms: 2500,
            eager: true,
            echo: EchoConfig::default(),
            backchannel: Tri::Auto,
            backchannel_intensity: None,
        }
    }
}

/// What the parser knows about the session besides the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParseCtx {
    pub bud_mode: bool,
    /// The block is the operator's own (an alias definition): its tools are taken and signed
    /// like those of the server's default block.
    pub operator: bool,
}

/// One non-fatal finding, in the shape of the `/ws` `config_warning` message.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigWarning {
    pub code: &'static str,
    pub message: String,
    /// Always an object with `path`; a refused update also carries `reason`.
    pub detail: Value,
}

/// Collects the findings of one walk and the dotted paths that were set.
#[derive(Default)]
struct Walk {
    warnings: Vec<ConfigWarning>,
    applied: Vec<String>,
    /// The names in the operator's backend catalog.
    backends: Vec<String>,
    /// The operator lets a session bring `tasks.resume_key`.
    resume_allowed: bool,
    /// The operator lets a session name its own `tasks.push_url`, signed with its own
    /// `tasks.push_secret`.
    push_allowed: bool,
}

impl Walk {
    fn warn(&mut self, code: &'static str, path: &str, message: impl Into<String>) {
        self.warnings.push(ConfigWarning {
            code,
            message: message.into(),
            detail: json!({ "path": path }),
        });
    }

    fn bad(&mut self, path: &str, expected: &str) {
        self.warn(
            "interaction_value_ignored",
            path,
            format!("{path} must be {expected}; the default is kept."),
        );
    }

    fn unknown(&mut self, path: &str) {
        self.warn(
            "unknown_config_keys",
            path,
            format!("{path} is not an interaction setting; it is ignored."),
        );
    }

    fn set<T>(&mut self, path: &str, field: &mut T, value: T) {
        *field = value;
        self.applied.push(path.to_owned());
    }

    /// The non-null members of an object, each with its dotted path.
    fn members<'a>(&mut self, value: &'a Value, path: &str) -> Vec<(&'a str, &'a Value, String)> {
        let Some(map) = value.as_object() else {
            self.bad(path, "an object");
            return Vec::new();
        };
        map.iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, value)| (key.as_str(), value, format!("{path}.{key}")))
            .collect()
    }

    /// `false` when the value is not a boolean (the field keeps its default).
    fn flag(&mut self, value: &Value, path: &str, field: &mut bool) -> bool {
        match value.as_bool() {
            Some(flag) => self.set(path, field, flag),
            None => self.bad(path, "true or false"),
        }
        value.is_boolean()
    }

    fn number(&mut self, value: &Value, path: &str, range: RangeInclusive<u32>, field: &mut u32) {
        match value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| range.contains(n))
        {
            Some(n) => self.set(path, field, n),
            None => self.bad(
                path,
                &format!("a whole number from {} to {}", range.start(), range.end()),
            ),
        }
    }

    fn text(&mut self, value: &Value, path: &str, max_chars: usize) -> Option<String> {
        let text = value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty() && text.chars().count() <= max_chars);
        if text.is_none() {
            self.bad(path, &format!("a string of 1 to {max_chars} characters"));
        }
        text.map(str::to_owned)
    }

    fn choice<T: Copy>(&mut self, value: &Value, path: &str, choices: &[(&str, T)], field: &mut T) {
        match choices
            .iter()
            .find(|(name, _)| value.as_str() == Some(*name))
        {
            Some((_, choice)) => self.set(path, field, *choice),
            None => {
                let names: Vec<&str> = choices.iter().map(|(name, _)| *name).collect();
                self.bad(path, &format!("one of {}", names.join(", ")));
            }
        }
    }

    /// `tasks.announce`: `ask`, `tell` or `screen`.
    fn announce(&mut self, value: &Value, path: &str, field: &mut Announce) {
        let choices = [
            ("ask", Announce::Ask),
            ("tell", Announce::Tell),
            ("screen", Announce::Screen),
        ];
        self.choice(value, path, &choices, field);
    }

    /// One of a closed list of words.
    fn word(
        &mut self,
        value: &Value,
        path: &str,
        words: &[&'static str],
        field: &mut &'static str,
    ) {
        let choices: Vec<(&str, &'static str)> = words.iter().map(|w| (*w, *w)).collect();
        self.choice(value, path, &choices, field);
    }

    fn speech(&mut self, value: &Value, path: &str, cfg: &mut InteractionConfig) {
        let speech = &mut cfg.speech;
        for (key, value, path) in self.members(value, path) {
            match key {
                "voice_owner" => self.word(
                    value,
                    &path,
                    &["auto", "provider", "gateway"],
                    &mut speech.voice_owner,
                ),
                "truth" => self.word(value, &path, &["monitored", "enforced"], &mut speech.truth),
                "narrator" => self.word(
                    value,
                    &path,
                    &["templates", "front_model"],
                    &mut speech.narrator,
                ),
                "voice_mismatch" => self.word(
                    value,
                    &path,
                    &["never", "confirmations_only", "always"],
                    &mut speech.voice_mismatch,
                ),
                "claim_policy" => self.word(
                    value,
                    &path,
                    &["report", "correct"],
                    &mut speech.claim_policy,
                ),
                "truth_ack" => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path} is a server setting (interaction.truth_ack); it is ignored."),
                ),
                "voice" if value.is_object() => cfg.speech_voice = Some(value.clone()),
                "voice" => self.bad(&path, "a tts_config object"),
                // Read by a `/ws` LiveKit session before the block comes here
                // (`handlers/ws/realtime_voice.rs`); anywhere else it means nothing.
                "realtime" => self.warn(
                    "interaction_value_ignored",
                    &path,
                    format!(
                        "{path} is read on /ws sessions with a livekit block only; it is ignored."
                    ),
                ),
                _ => self.unknown(&path),
            }
        }
    }

    /// `trusted`: the block is the operator's or the agent author's, not a session's.
    fn block(
        &mut self,
        raw: &Value,
        cfg: &mut InteractionConfig,
        tools_allowed: bool,
        backends_allowed: bool,
        trusted: bool,
    ) {
        if raw.as_bool() == Some(true) {
            return;
        }
        for (key, value, path) in self.members(raw, ROOT) {
            match key {
                "front" => self.front(value, &path, &mut cfg.front, trusted),
                "tasks" => self.tasks(
                    value,
                    &path,
                    &mut cfg.tasks,
                    tools_allowed,
                    backends_allowed,
                    trusted,
                ),
                "background" if trusted => self.background(value, &path, &mut cfg.tasks),
                // A marker a session made up would make the agent's drafts the answer.
                "background" => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path} is set by the agent or the operator; it is ignored."),
                ),
                "policy" => self.policy(value, &path, &mut cfg.policy),
                "duplex" => self.duplex(value, &path, &mut cfg.duplex),
                "events" => self.events(value, &path, cfg),
                "hitl" => self.hitl(value, &path, cfg),
                "regime" => self.word(
                    value,
                    &path,
                    &["auto", "cascade", "provider_speech", "delegating"],
                    &mut cfg.speech.regime,
                ),
                "filler_owner" => self.word(value, &path, &OWNERS, &mut cfg.speech.filler_owner),
                "backchannel_owner" => {
                    self.word(value, &path, &OWNERS, &mut cfg.speech.backchannel_owner)
                }
                "idle_owner" => self.word(value, &path, &OWNERS, &mut cfg.speech.idle_owner),
                "speech" => self.speech(value, &path, cfg),
                "failure" => self.failure(value, &path, &mut cfg.failure, trusted),
                "controls" if trusted => self.controls(value, &path, &mut cfg.controls),
                // A session never widens what the operator allows.
                "controls" => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path} is set by the agent or the operator; it is ignored."),
                ),
                key if SERVER_ONLY_KEYS.contains(&key) => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path} is a server setting; it is ignored."),
                ),
                _ => self.unknown(&path),
            }
        }
    }

    /// `failure`: the stall and exit times; the lines' words and the follow-up are the
    /// operator's (an operator's line must pass the checks of the pack's lines).
    fn failure(&mut self, value: &Value, path: &str, failure: &mut FailureConfig, trusted: bool) {
        for (key, value, path) in self.members(value, path) {
            match key {
                "stall_after_ms" => {
                    self.number(value, &path, 1_000..=10_000, &mut failure.stall_after_ms)
                }
                "max_wait_s" => self.number(value, &path, 5..=600, &mut failure.max_wait_s),
                "lines" | "follow_up" if !trusted => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path} is set by the agent or the operator; it is ignored."),
                ),
                "follow_up" => {
                    self.flag(value, &path, &mut failure.follow_up);
                }
                "lines" => {
                    for (id, text, path) in self.members(value, &path) {
                        let Some(text) = self.text(text, &path, 200) else {
                            continue;
                        };
                        let lexicon = persona::guard_lexicon();
                        match super::guard::pack_line(lexicon, &text, true, false) {
                            Some(rule) => self.warn(
                                "interaction_value_ignored",
                                &path,
                                format!("{path} breaks {rule}; the pack's line is kept."),
                            ),
                            None => {
                                let mut lines = failure.lines.clone();
                                lines.insert(id.to_owned(), text);
                                self.set(&path, &mut failure.lines, lines);
                            }
                        }
                    }
                }
                _ => self.unknown(&path),
            }
        }
    }

    /// `controls`: which controls apply, the rate and volume bounds, the languages.
    fn controls(&mut self, value: &Value, path: &str, controls: &mut ControlsConfig) {
        for (key, value, path) in self.members(value, path) {
            match key {
                "allowed" => {
                    let kinds: Option<Vec<ControlKind>> = value.as_array().and_then(|items| {
                        items
                            .iter()
                            .map(|item| item.as_str().and_then(ControlKind::parse))
                            .collect()
                    });
                    match kinds {
                        Some(kinds) => self.set(&path, &mut controls.allowed, kinds),
                        None => self.bad(&path, "a list of control kinds"),
                    }
                }
                "languages" => {
                    let codes: Option<Vec<String>> = value.as_array().and_then(|items| {
                        items
                            .iter()
                            .map(|item| item.as_str().and_then(language_code))
                            .collect()
                    });
                    match codes {
                        Some(codes) => self.set(&path, &mut controls.languages, codes),
                        None => self.bad(&path, "a list of BCP-47 language codes"),
                    }
                }
                "rate" | "volume_db" => {
                    let (low, high, range) = if key == "rate" {
                        (&mut controls.rate_min, &mut controls.rate_max, 0.5..=2.0)
                    } else {
                        (
                            &mut controls.volume_min_db,
                            &mut controls.volume_max_db,
                            -20.0..=20.0,
                        )
                    };
                    let pair = value
                        .as_array()
                        .filter(|pair| pair.len() == 2)
                        .and_then(|pair| {
                            let a = pair[0].as_f64().filter(|n| range.contains(n))?;
                            let b = pair[1].as_f64().filter(|n| range.contains(n))?;
                            (a <= b).then_some((a as f32, b as f32))
                        });
                    match pair {
                        Some((a, b)) => {
                            *low = a;
                            self.set(&path, high, b);
                        }
                        None => self.bad(&path, "[low, high] within the allowed range"),
                    }
                }
                "from_agents" => {
                    self.flag(value, &path, &mut controls.from_agents);
                }
                "from_clients" => {
                    self.flag(value, &path, &mut controls.from_clients);
                }
                "answers_only" => {
                    self.flag(value, &path, &mut controls.answers_only);
                }
                // Where a `human` control puts the caller through: a SIP session's number to
                // refer the call to, and the address any other session's request is signed to.
                "human" => {
                    for (key, value, path) in self.members(value, &path) {
                        let text = value.as_str().map(str::trim).unwrap_or_default();
                        match key {
                            "transfer_to" if !text.is_empty() && text.len() <= 64 => {
                                let to = Some(text.to_owned());
                                self.set(&path, &mut controls.human.transfer_to, to);
                            }
                            "transfer_to" => self.bad(&path, "a phone number or extension"),
                            "url"
                                if text.starts_with("https://") || text.starts_with("http://") =>
                            {
                                let url = Some(text.to_owned());
                                self.set(&path, &mut controls.human.url, url);
                            }
                            "url" => self.bad(&path, "an http or https URL"),
                            "wait_s" => match value.as_u64().filter(|s| *s <= 600) {
                                Some(seconds) => {
                                    let seconds = Some(seconds as u32);
                                    self.set(&path, &mut controls.human.wait_s, seconds);
                                }
                                None => self.bad(&path, "0 to 600 seconds"),
                            },
                            _ => self.unknown(&path),
                        }
                    }
                }
                _ => self.unknown(&path),
            }
        }
    }

    fn front(&mut self, value: &Value, path: &str, front: &mut FrontConfig, trusted: bool) {
        for (key, value, path) in self.members(value, path) {
            match key {
                // The rule router is gone (chapter 9): a scripted front is never run.
                "profile" if value.as_str() == Some("scripted") => self.warn(
                    "front_scripted_refused",
                    &path,
                    format!(
                        "{path}: the scripted front is no longer offered; interaction mode runs \
                         on the front model only, and is refused where none is ready."
                    ),
                ),
                "profile" => self.choice(
                    value,
                    &path,
                    &[("auto", FrontProfile::Auto), ("model", FrontProfile::Model)],
                    &mut front.profile,
                ),
                // A persona itself comes from the operator, the agent's author or the tenant's
                // registry, never from a client.
                "persona" if value.is_object() && !trusted => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!(
                        "{path}: a session selects a registered persona by its id; the persona itself is ignored."
                    ),
                ),
                "persona" if value.is_object() => match Persona::from_spec(value) {
                    Ok(persona) => {
                        front.spec = Some(value.clone());
                        self.set(&path, &mut front.persona, persona.id().to_owned());
                    }
                    Err(problem) => self.bad(&path, &format!("a valid persona ({problem})")),
                },
                "persona" => {
                    if let Some(id) = self.text(value, &path, 80) {
                        front.spec = None;
                        self.set(&path, &mut front.persona, id);
                    }
                }
                "act_budget_ms" => self.number(value, &path, 0..=300, &mut front.act_budget_ms),
                "total_budget_ms" => {
                    self.number(value, &path, 0..=1500, &mut front.total_budget_ms)
                }
                "preferred_name" => self.preferred_name(value, &path, front),
                // The published schema's place for the caller's fields.
                "caller" => self.caller(value, &path, front, true),
                _ => self.unknown(&path),
            }
        }
    }

    /// `front.caller`: the fields a client may set. `start`: the session's own block, the only
    /// place the identity token may come from.
    fn caller(&mut self, value: &Value, path: &str, front: &mut FrontConfig, start: bool) {
        for (key, value, path) in self.members(value, path) {
            match key {
                "preferred_name" => self.preferred_name(value, &path, front),
                "verbosity" => {
                    let choices = [
                        ("brief", Some(Verbosity::Brief)),
                        ("normal", Some(Verbosity::Normal)),
                        ("detailed", Some(Verbosity::Detailed)),
                    ];
                    self.choice(value, &path, &choices, &mut front.verbosity);
                }
                "pace" => {
                    let choices = [
                        ("slower", Some(Pace::Slower)),
                        ("normal", Some(Pace::Normal)),
                        ("faster", Some(Pace::Faster)),
                    ];
                    self.choice(value, &path, &choices, &mut front.pace);
                }
                "identity_token" if start => {
                    if let Some(token) = self.text(value, &path, 512) {
                        front.identity_token = Some(token);
                    }
                }
                _ => self.unknown(&path),
            }
        }
    }

    fn preferred_name(&mut self, value: &Value, path: &str, front: &mut FrontConfig) {
        if let Some(name) = self.text(value, path, 60) {
            self.set(path, &mut front.preferred_name, Some(name));
        }
    }

    fn tasks(
        &mut self,
        value: &Value,
        path: &str,
        tasks: &mut TasksConfig,
        tools_allowed: bool,
        backends_allowed: bool,
        trusted: bool,
    ) {
        for (key, value, path) in self.members(value, path) {
            match key {
                "reconnect_grace_s" => {
                    self.number(value, &path, 0..=600, &mut tasks.reconnect_grace_s)
                }
                "push_url" if trusted || self.push_allowed => {
                    let url = value.as_str().map(str::trim).unwrap_or_default();
                    if url.starts_with("https://") || url.starts_with("http://") {
                        self.set(&path, &mut tasks.push_url, Some(url.to_owned()));
                    } else {
                        self.bad(&path, "an http or https URL");
                    }
                }
                // The client's own key for the pushes to its own address.
                "push_secret" if trusted || self.push_allowed => {
                    let fits = |secret: &&str| (16..=256).contains(&secret.len());
                    match value.as_str().filter(fits) {
                        Some(secret) => {
                            self.set(&path, &mut tasks.push_secret, Some(secret.to_owned()))
                        }
                        None => self.bad(&path, "a secret of 16 to 256 characters"),
                    }
                }
                // The push is signed with the server's secret: a session does not choose
                // where it goes.
                "session_end" if trusted => {
                    let choices = [
                        ("auto", SessionEnd::Auto),
                        ("cancel_all", SessionEnd::CancelAll),
                        ("continue_all", SessionEnd::ContinueAll),
                    ];
                    self.choice(value, &path, &choices, &mut tasks.session_end);
                }
                "orphan_max_s" if trusted => {
                    self.number(value, &path, 0..=86_400, &mut tasks.orphan_max_s)
                }
                // Without `allow_client_push` the push is signed with the server's secret, and
                // a task that runs on after the call runs on the operator's account: a session
                // decides neither.
                "push_url" | "push_secret" | "session_end" | "orphan_max_s" => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path} is set by the operator; it is ignored."),
                ),
                // The catalog is the operator's: a session only names an entry, and only
                // where the operator lets sessions choose.
                "default_backend" if !backends_allowed => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path}: this server does not let a session name its backend; it is ignored."),
                ),
                "default_backend" => {
                    let known = |name: &&str| self.backends.iter().any(|backend| backend == name);
                    match value.as_str().filter(known) {
                        Some(name) => self.set(&path, &mut tasks.backend, Some(name.to_owned())),
                        None => self.bad(&path, "the name of a backend this server lists"),
                    }
                }
                "llm" if value.is_object() => self.set(&path, &mut tasks.llm, Some(value.clone())),
                "llm" => self.bad(&path, "an object"),
                "orchestrator" if value.is_object() => {
                    self.set(&path, &mut tasks.orchestrator, Some(value.clone()))
                }
                "orchestrator" => {
                    if let Some(model) = self.text(value, &path, 200) {
                        self.set(&path, &mut tasks.orchestrator, Some(Value::String(model)));
                    }
                }
                "backends" if !backends_allowed => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path}: this server does not let a session name its backends; it is ignored."),
                ),
                "backends" => {
                    let known = |name: &str| self.backends.iter().any(|backend| backend == name);
                    let names: Option<Vec<String>> = value.as_array().and_then(|items| {
                        items
                            .iter()
                            .map(|item| item.as_str().filter(|name| known(name)).map(str::to_owned))
                            .collect()
                    });
                    match names {
                        Some(mut names) if !names.is_empty() => {
                            names.dedup();
                            self.set(&path, &mut tasks.backends, names)
                        }
                        _ => self.bad(&path, "a list of backend names this server lists"),
                    }
                }
                "agents" if !backends_allowed => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path}: this server does not let a session name its agents; it is ignored."),
                ),
                "agents" => {
                    let known = |name: &str| self.backends.iter().any(|backend| backend == name);
                    let names: Option<Vec<String>> = value.as_array().and_then(|items| {
                        items
                            .iter()
                            .map(|item| item.as_str().filter(|name| known(name)).map(str::to_owned))
                            .collect()
                    });
                    match names {
                        Some(mut names) => {
                            names.dedup();
                            self.set(&path, &mut tasks.agents, names)
                        }
                        None => self.bad(&path, "a list of backend names this server lists"),
                    }
                }
                "limits" => self.limits(value, &path, &mut tasks.limits, trusted),
                "follow_up_ttl_s" => {
                    self.number(value, &path, 0..=86_400, &mut tasks.follow_up_ttl_s)
                }
                // A caller's key: the operator lets sessions bring one, or names it itself.
                "resume_key" if !(trusted || self.resume_allowed) => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path}: this server does not let a session bring a resume key; it is ignored."),
                ),
                "resume_key" => {
                    let fits = |key: &&str| {
                        (8..=128).contains(&key.len())
                            && key
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
                    };
                    match value.as_str().filter(fits) {
                        Some(key) => self.set(&path, &mut tasks.resume_key, Some(key.to_owned())),
                        None => self.bad(
                            &path,
                            "8 to 128 letters, digits, '-', '_', '.' or ':'",
                        ),
                    }
                }
                "announce" => self.announce(value, &path, &mut tasks.announce),
                "tools" if !tools_allowed => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path}: this server does not accept tools from a session; they are ignored."),
                ),
                "tools" => {
                    let Some(items) = value.as_array() else {
                        self.bad(&path, "a list of tools");
                        continue;
                    };
                    let mut tools: Vec<ToolSpec> = Vec::new();
                    for (index, item) in items.iter().enumerate() {
                        let item_path = format!("{path}[{index}]");
                        match self.tool(item, &item_path) {
                            Some(tool) if tools.iter().any(|t| t.name == tool.name) => {
                                self.bad(&format!("{item_path}.name"), "a name no other tool has");
                            }
                            Some(tool) => tools.push(tool),
                            None => {}
                        }
                    }
                    self.set(&path, &mut tasks.tools, tools);
                }
                "speculative_start" => self.choice(
                    value,
                    &path,
                    &[("auto", Tri::Auto), ("on", Tri::On), ("off", Tri::Off)],
                    &mut tasks.speculative_start,
                ),
                _ => self.unknown(&path),
            }
        }
    }

    fn limits(&mut self, value: &Value, path: &str, limits: &mut TaskLimits, trusted: bool) {
        for (key, value, path) in self.members(value, path) {
            match key {
                "max_live" => {
                    // A session may lower the operator's limit, never raise it.
                    let most = if trusted { None } else { limits.max_live };
                    let top = most.unwrap_or(10_000);
                    let mut n = 0;
                    self.number(value, &path, 1..=top, &mut n);
                    if n > 0 {
                        limits.max_live = Some(n);
                    }
                }
                "max_live_tenant" if trusted => {
                    let mut n = 0;
                    self.number(value, &path, 1..=1_000_000, &mut n);
                    if n > 0 {
                        limits.max_live_tenant = Some(n);
                    }
                }
                "max_live_tenant" => self.warn(
                    "interaction_override_refused",
                    &path,
                    format!("{path} is set by the operator; it is ignored."),
                ),
                _ => self.unknown(&path),
            }
        }
    }

    fn background(&mut self, value: &Value, path: &str, tasks: &mut TasksConfig) {
        for (key, value, path) in self.members(value, path) {
            match key {
                "answer_marker" => {
                    if let Some(marker) = self.text(value, &path, 40) {
                        self.set(&path, &mut tasks.answer_marker, Some(marker));
                    }
                }
                _ => self.unknown(&path),
            }
        }
    }

    /// One tool; `None` (with a warning) when its name or URL is unusable.
    fn tool(&mut self, value: &Value, path: &str) -> Option<ToolSpec> {
        let mut tool = ToolSpec {
            name: String::new(),
            description: String::new(),
            parameters: json!({ "type": "object", "properties": {} }),
            url: String::new(),
            side_effects: false,
            irreversible: false,
            confirm_phrase: None,
            confirm_template: None,
            progress_phrase: None,
            signed: false,
        };
        let mut gate_known = true;
        for (key, value, path) in self.members(value, path) {
            match key {
                // Both are checked below, so a wrong type and a missing key warn alike.
                "name" => tool.name = value.as_str().unwrap_or_default().trim().to_owned(),
                "url" => tool.url = value.as_str().unwrap_or_default().trim().to_owned(),
                "description" => match value.as_str() {
                    Some(text) => tool.description = text.to_owned(),
                    None => self.bad(&path, "a string"),
                },
                "parameters" if value.is_object() => tool.parameters = value.clone(),
                "parameters" => self.bad(&path, "a JSON Schema object"),
                "side_effects" => gate_known &= self.flag(value, &path, &mut tool.side_effects),
                "irreversible" => gate_known &= self.flag(value, &path, &mut tool.irreversible),
                "confirm_phrase" => tool.confirm_phrase = self.text(value, &path, 200),
                "confirm_template" => tool.confirm_template = self.text(value, &path, 200),
                "progress_phrase" => tool.progress_phrase = self.text(value, &path, 200),
                _ => self.unknown(&path),
            }
        }
        let name_ok = !tool.name.is_empty()
            && tool.name.len() <= 64
            && tool.name != ASK_CALLER
            && tool
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !name_ok {
            self.bad(
                &format!("{path}.name"),
                "1 to 64 letters, digits, '_' or '-', and not ask_caller",
            );
            return None;
        }
        if !(tool.url.starts_with("https://") || tool.url.starts_with("http://")) {
            self.bad(&format!("{path}.url"), "an http or https URL");
            return None;
        }
        if !gate_known {
            // `side_effects: "true"` must not leave a tool that runs without a confirmation.
            return None;
        }
        tool.side_effects |= tool.irreversible;
        if let Some(template) = &tool.confirm_template {
            let fields = tool.parameters["properties"].as_object();
            let known = |name: &str| fields.is_some_and(|fields| fields.contains_key(name));
            if let Err(problem) = persona::check_confirm_template(template, known) {
                self.bad(
                    &format!("{path}.confirm_template"),
                    &format!("a read-back the confirmation rules accept ({problem})"),
                );
                tool.confirm_template = None;
            }
        }
        Some(tool)
    }

    fn policy(&mut self, value: &Value, path: &str, policy: &mut PolicyConfig) {
        for (key, value, path) in self.members(value, path) {
            let (range, field) = match key {
                "ack_due_ms" => (0..=800, &mut policy.ack_due_ms),
                "fast_answer_ms" => (0..=2000, &mut policy.fast_answer_ms),
                "fast_answer_min_turns" => (1..=20, &mut policy.fast_answer_min_turns),
                "ack_hard_deadline_ms" => (800..=4000, &mut policy.ack_hard_deadline_ms),
                "amend_ack_target_ms" => (300..=2000, &mut policy.amend_ack_target_ms),
                "answer_window_ms" => (2000..=10_000, &mut policy.answer_window_ms),
                "heard_margin_ms" => (0..=1000, &mut policy.heard_margin_ms),
                // The second name is the published schema's.
                "progress_min_wait_ms" | "progress_first_after_ms" => {
                    (2000..=10_000, &mut policy.progress_min_wait_ms)
                }
                "progress_min_gap_ms" => (3000..=15_000, &mut policy.progress_min_gap_ms),
                "updates_max_per_wait" => (0..=5, &mut policy.updates_max_per_wait),
                "hold_after_silence_ms" => (4000..=20_000, &mut policy.hold_after_silence_ms),
                "quiet_checkin_ms" => (10_000..=120_000, &mut policy.quiet_checkin_ms),
                "agreed_wait_max_ms" => (60_000..=3_600_000, &mut policy.agreed_wait_max_ms),
                "hitl_reprompt_after_ms" => (3000..=30_000, &mut policy.hitl_reprompt_after_ms),
                "hitl_answer_timeout_ms" => (5000..=120_000, &mut policy.hitl_answer_timeout_ms),
                "hitl_key_gap_ms" => (2000..=20_000, &mut policy.hitl_key_gap_ms),
                _ => {
                    self.unknown(&path);
                    continue;
                }
            };
            self.number(value, &path, range, field);
        }
    }

    /// The published schema nests the barge-in numbers in a `barge_in` object and the eager
    /// switch under `endpoint.eager.enabled`; both forms set the same fields.
    fn duplex(&mut self, value: &Value, path: &str, duplex: &mut DuplexConfig) {
        for (key, value, path) in self.members(value, path) {
            match key {
                "barge_in" if value.is_object() => self.duplex(value, &path, duplex),
                "barge_in" => {
                    self.flag(value, &path, &mut duplex.barge_in);
                }
                "endpoint" => {
                    for (key, value, path) in self.members(value, &path) {
                        match (key, value.get("enabled")) {
                            ("eager", Some(enabled)) => self.choice(
                                enabled,
                                &format!("{path}.enabled"),
                                &[("auto", true), ("on", true), ("off", false)],
                                &mut duplex.eager,
                            ),
                            _ => self.unknown(&path),
                        }
                    }
                }
                "duck_db" => match value.as_f64().filter(|db| (-60.0..=0.0).contains(db)) {
                    Some(db) => self.set(&path, &mut duplex.duck_db, db as f32),
                    None => self.bad(&path, "a number from -60 to 0"),
                },
                "duck_ramp_ms" => self.number(value, &path, 0..=500, &mut duplex.duck_ramp_ms),
                "pause_after_ms" => {
                    self.number(value, &path, 100..=2000, &mut duplex.pause_after_ms)
                }
                "decide_by_ms" => self.number(value, &path, 300..=3000, &mut duplex.decide_by_ms),
                "max_pause_ms" => self.number(value, &path, 500..=10_000, &mut duplex.max_pause_ms),
                "eager" => {
                    self.flag(value, &path, &mut duplex.eager);
                }
                "backchannel" => {
                    // `true`, `false`, or the published schema's `{"enabled": ...,
                    // "intensity": ...}`.
                    let (enabled, path) = if value.is_object() {
                        let mut enabled = None;
                        for (key, value, path) in self.members(value, &path) {
                            match key {
                                "enabled" => enabled = Some((value, path)),
                                "intensity" => {
                                    let words =
                                        Intensity::WORDS.map(|(word, level)| (word, Some(level)));
                                    self.choice(
                                        value,
                                        &path,
                                        &words,
                                        &mut duplex.backchannel_intensity,
                                    )
                                }
                                _ => self.unknown(&path),
                            }
                        }
                        match enabled {
                            Some(enabled) => enabled,
                            None => continue,
                        }
                    } else {
                        (value, path)
                    };
                    let tri = match enabled {
                        Value::Bool(true) => Some(Tri::On),
                        Value::Bool(false) => Some(Tri::Off),
                        Value::String(auto) if auto == "auto" => Some(Tri::Auto),
                        _ => None,
                    };
                    match tri {
                        Some(tri) => self.set(&path, &mut duplex.backchannel, tri),
                        None => self.bad(&path, "true, false or \"auto\""),
                    }
                }
                "echo" => {
                    for (key, value, path) in self.members(value, &path) {
                        match key {
                            "correlation_gate" => {
                                self.flag(value, &path, &mut duplex.echo.correlation_gate);
                            }
                            "aec" => self.choice(
                                value,
                                &path,
                                &[("auto", Tri::Auto), ("on", Tri::On), ("off", Tri::Off)],
                                &mut duplex.echo.aec,
                            ),
                            "client_aec" | "phone_line" => {
                                let mut said = false;
                                if self.flag(value, &path, &mut said) {
                                    let field = if key == "client_aec" {
                                        &mut duplex.echo.client_aec
                                    } else {
                                        &mut duplex.echo.phone_line
                                    };
                                    *field = Some(said);
                                }
                            }
                            "tail_ms" => {
                                self.number(value, &path, 0..=5_000, &mut duplex.echo.tail_ms)
                            }
                            "max_bulk_delay_ms" => self.number(
                                value,
                                &path,
                                200..=1_500,
                                &mut duplex.echo.max_bulk_delay_ms,
                            ),
                            "auto_enable_after_echo_episodes" => self.number(
                                value,
                                &path,
                                0..=10,
                                &mut duplex.echo.auto_enable_after_echo_episodes,
                            ),
                            _ => self.unknown(&path),
                        }
                    }
                }
                _ => self.unknown(&path),
            }
        }
    }

    fn events(&mut self, value: &Value, path: &str, cfg: &mut InteractionConfig) {
        for (key, value, path) in self.members(value, path) {
            match key {
                "floor" => {
                    self.flag(value, &path, &mut cfg.events_floor);
                }
                "segments" => {
                    self.flag(value, &path, &mut cfg.events_segments);
                }
                _ => self.unknown(&path),
            }
        }
    }

    /// `hitl.channels`: where a question may be answered. Voice always; `client` when the
    /// client shows questions on a screen; `dtmf` when a caller on a phone line may use the
    /// keypad.
    fn hitl(&mut self, value: &Value, path: &str, cfg: &mut InteractionConfig) {
        for (key, value, path) in self.members(value, path) {
            match key {
                "channels" => {
                    let names: Option<Vec<&str>> = value
                        .as_array()
                        .and_then(|all| all.iter().map(Value::as_str).collect());
                    let known = |names: &Vec<&str>| {
                        names
                            .iter()
                            .all(|name| ["voice", "client", "dtmf"].contains(name))
                    };
                    match names.filter(known) {
                        Some(names) => {
                            cfg.hitl_dtmf = names.contains(&"dtmf");
                            self.set(&path, &mut cfg.hitl_client, names.contains(&"client"));
                        }
                        None => self.bad(&path, "a list of \"voice\", \"client\" and \"dtmf\""),
                    }
                }
                _ => self.unknown(&path),
            }
        }
    }
}

/// Parse a session's `interaction` value.
///
/// `None` means the session runs without interaction mode: the value is absent or `false`, it
/// is not `true` or an object, or the server switch is off (`interaction_disabled_by_server`).
///
/// The server's `default` block is the base and the session's block is laid over it. Tools
/// are taken from the server's block always, and from a session only when the server allows
/// client tools and the session is not in Bud mode. `front.profile` is always `model` here:
/// `scripted` gives `front_scripted_refused`, and a server without a front model is refused
/// by the surface (`interaction_refused`, reason `front_unavailable`).
pub fn parse(
    raw: &Value,
    server: &ServerInteraction,
    ctx: ParseCtx,
) -> (Option<InteractionConfig>, Vec<ConfigWarning>) {
    let names = || {
        server
            .task_backends
            .iter()
            .map(|b| b.name.clone())
            .collect()
    };
    let mut walk = Walk {
        backends: names(),
        resume_allowed: server.allow_resume_key,
        push_allowed: server.allow_client_push && !ctx.bud_mode,
        ..Walk::default()
    };
    match raw {
        Value::Null | Value::Bool(false) => return (None, walk.warnings),
        Value::Bool(true) | Value::Object(_) => {}
        _ => {
            walk.bad(ROOT, "true or an object");
            return (None, walk.warnings);
        }
    }
    if !server.enabled {
        walk.warn(
            "interaction_disabled_by_server",
            ROOT,
            "Interaction mode is switched off on this server; the session runs without it.",
        );
        return (None, walk.warnings);
    }

    let mut cfg = InteractionConfig::default();
    // The session block is the server's own default when the session sent none.
    let from_server = server.default.as_ref() == Some(raw);
    if let Some(default) = server.default.as_ref().filter(|_| !from_server) {
        // Findings in the operator's block are reported once, at startup.
        let mut base = Walk {
            backends: names(),
            ..Walk::default()
        };
        base.block(default, &mut cfg, true, true, true);
    }
    // What is in the list now is the operator's; a session's `tasks.tools` replaces it whole.
    let operators = |cfg: &mut InteractionConfig| {
        cfg.tasks
            .tools
            .iter_mut()
            .for_each(|tool| tool.signed = true)
    };
    operators(&mut cfg);
    let trusted = from_server || ctx.operator;
    let tools_allowed = trusted || (server.allow_client_tools && !ctx.bud_mode);
    let backends_allowed = trusted || (server.allow_client_backends && !ctx.bud_mode);
    let operator_push = (cfg.tasks.push_url.clone(), cfg.tasks.push_secret.clone());
    walk.block(raw, &mut cfg, tools_allowed, backends_allowed, trusted);
    if trusted {
        operators(&mut cfg);
    } else {
        client_push(&mut walk, &mut cfg, operator_push);
    }

    // No fallback (chapter 9): the front model decides every turn. A session without one is
    // refused where it starts (`interaction_refused`, reason `front_unavailable`).
    cfg.front.profile = FrontProfile::Model;
    (Some(cfg), walk.warnings)
}

/// A session's own push (`allow_client_push`): its `push_url` and its `push_secret` go
/// together, so a client's address is never signed with the server's secret and the
/// operator's address never with a client's. Either alone is dropped with a finding, and the
/// operator's pair (`operator`) stays.
fn client_push(
    walk: &mut Walk,
    cfg: &mut InteractionConfig,
    operator: (Option<String>, Option<String>),
) {
    let set = |walk: &Walk, key: &str| {
        let path = format!("{ROOT}.tasks.{key}");
        walk.applied.contains(&path)
    };
    let (url, secret) = (set(walk, "push_url"), set(walk, "push_secret"));
    if url == secret {
        return;
    }
    let (path, why) = if url {
        ("push_url", "needs the session's own tasks.push_secret")
    } else {
        (
            "push_secret",
            "is only for the session's own tasks.push_url",
        )
    };
    let path = format!("{ROOT}.tasks.{path}");
    walk.warn(
        "interaction_value_ignored",
        &path,
        format!("{path} {why}; it is ignored."),
    );
    (cfg.tasks.push_url, cfg.tasks.push_secret) = operator;
}

/// Apply an `interaction_update` patch mid-session. Only `policy.*`, `duplex.*`, `events.*` and
/// `front.preferred_name` (`null` clears it) may change; every other path is refused with
/// `detail.reason: "not_mid_session"`. Returns the dotted paths applied and the findings.
pub fn apply_update(
    cfg: &mut InteractionConfig,
    patch: &Value,
) -> (Vec<String>, Vec<ConfigWarning>) {
    let mut walk = Walk::default();
    let refuse = |walk: &mut Walk, path: &str| {
        walk.warnings.push(ConfigWarning {
            code: "interaction_override_refused",
            message: format!("{path} cannot change during a session."),
            detail: json!({ "path": path, "reason": "not_mid_session" }),
        });
    };
    let Some(map) = patch.as_object() else {
        walk.bad(ROOT, "an object");
        return (walk.applied, walk.warnings);
    };
    for (key, value) in map {
        let path = format!("{ROOT}.{key}");
        match (key.as_str(), value.as_object()) {
            ("policy", _) => walk.policy(value, &path, &mut cfg.policy),
            ("duplex", _) => {
                // The analyser set its echo defence up when the session started.
                let mut rest = value.clone();
                if let Some(echo) = rest.as_object_mut().and_then(|map| map.remove("echo"))
                    && !echo.is_null()
                {
                    refuse(&mut walk, &format!("{path}.echo"));
                }
                walk.duplex(&rest, &path, &mut cfg.duplex)
            }
            ("events", _) => walk.events(value, &path, cfg),
            ("hitl", _) => walk.hitl(value, &path, cfg),
            ("front", Some(front)) => {
                for (key, value) in front {
                    let path = format!("{path}.{key}");
                    match (key.as_str(), value.as_object()) {
                        ("preferred_name", _) if value.is_null() => {
                            walk.set(&path, &mut cfg.front.preferred_name, None)
                        }
                        ("preferred_name", _) => walk.preferred_name(value, &path, &mut cfg.front),
                        ("caller", Some(caller)) => {
                            let mut rest = caller.clone();
                            if rest.remove("identity_token").is_some() {
                                refuse(&mut walk, &format!("{path}.identity_token"));
                            }
                            if rest.get("preferred_name") == Some(&Value::Null) {
                                rest.remove("preferred_name");
                                let name = format!("{path}.preferred_name");
                                walk.set(&name, &mut cfg.front.preferred_name, None);
                            }
                            walk.caller(&Value::Object(rest), &path, &mut cfg.front, false)
                        }
                        _ => refuse(&mut walk, &path),
                    }
                }
            }
            ("front", None) => walk.bad(&path, "an object"),
            // How the tasks' updates reach the caller from now on; the rest of `tasks` was
            // settled when the session started.
            ("tasks", Some(tasks)) => {
                for (key, value) in tasks {
                    let path = format!("{path}.{key}");
                    match key.as_str() {
                        "announce" => walk.announce(value, &path, &mut cfg.tasks.announce),
                        _ => refuse(&mut walk, &path),
                    }
                }
            }
            ("tasks", None) => walk.bad(&path, "an object"),
            _ => refuse(&mut walk, &path),
        }
    }
    let applied = walk
        .applied
        .iter()
        .map(|path| path.trim_start_matches("interaction.").to_owned())
        .collect();
    (applied, walk.warnings)
}

/// The resolved settings sent as `ready.interaction`. `degraded` lists the warning codes that
/// changed behaviour. `backend` is `llm`, or `agent:<name>` on a voice-agent session. The task
/// backend's URL and keys and the tool URLs are never echoed.
pub fn echo(cfg: &InteractionConfig, backend: &str, degraded: &[&str]) -> Value {
    let tools: Vec<&str> = cfg
        .tasks
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    json!({
        "enabled": true,
        "protocol_version": INTERACTION_PROTOCOL_VERSION,
        "regime": "cascade",
        "front": cfg.front,
        "tasks": {
            "backend": backend,
            "speculative_start": cfg.tasks.speculative_start,
            "tools": tools,
            "reconnect_grace_s": cfg.tasks.reconnect_grace_s,
            "push": cfg.tasks.push_url.is_some(),
            "session_end": cfg.tasks.session_end,
            "orphan_max_s": cfg.tasks.orphan_max_s,
            "orchestrator": cfg.tasks.orchestrator.is_some(),
            "agents": cfg.tasks.agents,
            "limits": cfg.tasks.limits,
            "announce": cfg.tasks.announce,
            "follow_up_ttl_s": cfg.tasks.follow_up_ttl_s,
            "resume": cfg.tasks.resume_key.is_some(),
        },
        "policy": cfg.policy,
        "duplex": cfg.duplex,
        "events": { "floor": cfg.events_floor, "segments": cfg.events_segments },
        "hitl": { "channels": hitl_channels(cfg) },
        "controls": cfg.controls,
        "failure": cfg.failure,
        "degraded": degraded,
    })
}

/// The echo of `hitl.channels`.
fn hitl_channels(cfg: &InteractionConfig) -> Vec<&'static str> {
    let client = cfg.hitl_client.then_some("client");
    let dtmf = cfg.hitl_dtmf.then_some("dtmf");
    std::iter::once("voice").chain(client).chain(dtmf).collect()
}

// ---- server block

/// The server's `interaction` block. Off unless `enabled` is set.
#[derive(Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerInteraction {
    pub enabled: bool,
    /// The session block used when a session sends none, and the base of every session's block.
    pub default: Option<Value>,
    /// The block of a Bud voice-agent session whose agent entry carries none.
    pub agent_default: Option<Value>,
    /// The block of a `/v1/realtime` relay session (a Bud realtime deployment).
    pub realtime_default: Option<Value>,
    /// The speech-to-speech providers (`provider` or `provider:model`) whose own speech the
    /// operator accepts as monitored, not enforced: regimes B and C run only for them.
    pub truth_ack: Vec<String>,
    pub front_model: Option<FrontModelServer>,
    /// Signs the HTTP tool calls.
    pub tool_secret: Option<String>,
    pub allow_client_tools: bool,
    /// A Redis that the gateway processes of one deployment share for their tasks: a task is
    /// then listed by any of them, its result reaches a caller who reconnects to another, and
    /// the tasks of a process that died are swept. Its own setting: `WAAV_REDIS_URL` is the
    /// Bud control plane's.
    pub task_redis_url: Option<String>,
    /// The operator's catalog of backgrounds other than an LLM of the session's: a session
    /// names one with `tasks.default_backend`.
    pub task_backends: Vec<BackendSpec>,
    /// Let a session name a catalog entry itself. An alias, an agent entry and the server's
    /// own blocks always may.
    pub allow_client_backends: bool,
    /// Let a session bring `tasks.resume_key`: its tasks and their updates follow a caller who
    /// comes back with the same key on another `stream_id`.
    pub allow_resume_key: bool,
    /// Let a session name its own `tasks.push_url`, signed with its own `tasks.push_secret`
    /// (never in Bud mode).
    pub allow_client_push: bool,
    /// Tenants register their own personas (`PUT /v1/interaction/personas/{id}`), and a
    /// session of the tenant selects one by its id.
    pub tenant_personas: bool,
    /// Caller memory: the key that signs callers' identity tokens. Memory is off without it.
    pub caller_memory_key: Option<String>,
    /// Escape flags: each restores today's frames of one provider wire fix for every session
    /// (names in [`LEGACY_WIRE`]); read by [`provider_fixes`].
    pub realtime_legacy_wire: Vec<String>,
    /// Provider fixes the operator turns on (names in [`PROVIDER_FIXES`]); each is off by
    /// default until its vendor's live probe passes or its vendor's reference refuses today's
    /// frame ([`FIXES_ON_BY_DEFAULT`] are on). Read
    /// with [`provider_fixes`].
    pub provider_fixes: Vec<String>,
    /// A `/realtime` session may also turn a fix on that is not in force
    /// ([`session_fixes`]); without it a session may only turn fixes off.
    pub provider_fixes_per_session: bool,
    /// A directory of language packs (`guard_lexicon.<lang>.json`, `phrase_banks.<lang>.json`,
    /// `templates.<lang>.json`), read once: a session in a language with a pack runs the mode
    /// on it.
    pub packs_dir: Option<String>,
    /// Recorded listening sounds, `<provider>/<voice>/<language>/<clip id>.wav`: one replaces
    /// the clip rendered in that voice when it passes the clip checks. Also the failure lines
    /// (`.../failure/<line id>.wav`) and the cache of the rendered ones (`.../failure/cache/`).
    pub clips_dir: Option<String>,
    /// The server's default voice (a `tts_config` object): the failure lines are rendered in
    /// it once at server start, for a session whose own voice cannot give them (chapter 10).
    pub failure_voice: Option<Value>,
}

/// The escape flags of the provider wire fixes (plan chapter 6, schema
/// `realtime_legacy_wire`). A flag keeps today's wire even when its fix is listed in
/// [`PROVIDER_FIXES`] by the operator; each names one fix of that list.
pub const LEGACY_WIRE: [&str; 38] = [
    // Nova Sonic, Ultravox, ElevenLabs Agents, Hume EVI, Inworld, Yandex (r9).
    "nova_stream_drop_on_close",
    "nova_audio_before_history",
    "nova_tool_result_as_given",
    "nova_omit_turn_detection",
    "ultravox_new_call_on_redial",
    "ultravox_legacy_model",
    "ultravox_omit_call_settings",
    "elevenlabs_omit_voice",
    "hume_text_input",
    "hume_settings_when_needed",
    "hume_stop_assistant",
    "hume_omit_tools",
    "hume_voice_query",
    "inworld_encode_key",
    "inworld_whisper_model",
    "openai_clone_voice_alloy",
    "inworld_omit_model",
    "inworld_auto_tool_response",
    "inworld_openai_response",
    "yandex_omit_languages",
    "deepgram_text_as_agent_message",
    "nova_text_non_interactive",
    "gemini_omit_tool_behavior",
    "gemini_media_chunks",
    "gemini_response_id_only",
    "allow_setup_resend",
    "deepgram_settings_resend",
    "deepgram_omit_tools",
    "ultravox_omit_tools",
    "openai_session_resend",
    "grok_openai_session",
    "grok_openai_events",
    "grok_legacy_model",
    // The round-9 fixes on by default since 24 October (documentation evidence).
    "openai_turn_detection_none",
    "openai_replay_text",
    "azure_deployment_query",
    "hume_drop_id_less_messages",
    "inworld_endpoint_session_only",
];

/// The provider wire fixes and default moves the plan ships only after a live probe of the
/// vendor (plan section 11 Lifecycle track, sign-offs 9 and 22): each is built and off until
/// the operator lists it in `provider_fixes`. `(fix, escape flag)`: the escape flag (one of
/// [`LEGACY_WIRE`]) keeps today's wire even when the fix is listed.
pub const PROVIDER_FIXES: [(&str, Option<&str>); 59] = [
    ("gemini_tool_behavior", Some("gemini_omit_tool_behavior")),
    ("gemini_tool_call_cancellation", None),
    ("gemini_interaction_status", None),
    // An interaction session's delegate results carry Gemini's `scheduling` (`INTERRUPT`,
    // `WHEN_IDLE`, `SILENT`); the plan probes the values first (chapter 6, live probes).
    ("gemini_response_scheduling", None),
    // The caller's audio as `realtimeInput.audio` (`mediaChunks` is deprecated).
    ("gemini_realtime_audio", Some("gemini_media_chunks")),
    // A function response names its function (`FunctionResponse.name`).
    ("gemini_response_name", Some("gemini_response_id_only")),
    // A provider that takes its setup once (Gemini Live) refuses a mid-session change.
    ("setup_once", Some("allow_setup_resend")),
    ("deepgram_user_text", Some("deepgram_text_as_agent_message")),
    ("deepgram_tools", Some("deepgram_omit_tools")),
    // A mid-session change is `UpdatePrompt` / `UpdateSpeak`, not a second `Settings`.
    ("deepgram_update_prompt", Some("deepgram_settings_resend")),
    ("nova_interactive_text", Some("nova_text_non_interactive")),
    ("ultravox_tools", Some("ultravox_omit_tools")),
    ("openai_realtime_default", None),
    // Kept so an operator's list still parses: an empty OpenAI speech-to-text model is
    // `gpt-transcribe` everywhere now (addendum B6), so the fix changes nothing.
    ("openai_stt_default", None),
    // Round 9 (each against the vendor's own reference; no key here, so none is probed).
    // OpenAI Realtime GA (and Azure, Grok, Inworld, which reuse its wire): manual turns are
    // `turn_detection: null`; a replayed assistant line is `output_text`; an update names
    // only what changed; `modalities` choose `output_modalities`; a function's name comes
    // from `.done` and the harmless `response_cancel_not_active` error is not passed on.
    (
        "openai_manual_turn_null",
        Some("openai_turn_detection_none"),
    ),
    ("openai_replay_output_text", Some("openai_replay_text")),
    ("openai_update_delta", Some("openai_session_resend")),
    ("openai_modalities", None),
    ("openai_inbound", None),
    // Azure's GA path takes the deployment as `model=` (`deployment=` is the preview's: 404).
    ("azure_ga_url", Some("azure_deployment_query")),
    // xAI's own `session.update` / `response.create` fields, its server events, its model.
    ("grok_session", Some("grok_openai_session")),
    ("grok_events", Some("grok_openai_events")),
    ("grok_voice_default", Some("grok_legacy_model")),
    // Gemini Live: final transcripts per turn, context compression, the session's turn
    // detection, nothing before `setupComplete`, an update by resuming, JSON Schema tool
    // parameters, `maxOutputTokens`, thought parts and `voiceActivity`.
    ("gemini_turn_transcripts", None),
    ("gemini_context_compression", None),
    ("gemini_turn_detection", None),
    ("gemini_await_setup", None),
    ("gemini_update_resume", None),
    ("gemini_json_schema", None),
    ("gemini_max_output_tokens", None),
    ("gemini_inbound", None),
    // Nova Sonic: the documented close, history before the audio content, the barge-in text
    // marker, JSON-object tool results, `endpointingSensitivity`.
    ("nova_close_sequence", Some("nova_stream_drop_on_close")),
    (
        "nova_history_before_audio",
        Some("nova_audio_before_history"),
    ),
    ("nova_interrupted_marker", None),
    ("nova_tool_result_object", Some("nova_tool_result_as_given")),
    ("nova_turn_detection", Some("nova_omit_turn_detection")),
    // Ultravox: `text` / `delta` per `ordinal`, `priorCallId` on a re-dial, the API's default
    // model, `temperature` and `vadSettings`.
    ("ultravox_transcripts", None),
    ("ultravox_prior_call", Some("ultravox_new_call_on_redial")),
    ("ultravox_default_model", Some("ultravox_legacy_model")),
    (
        "ultravox_call_settings",
        Some("ultravox_omit_call_settings"),
    ),
    // ElevenLabs Agents: the server events ignored today, the voice override.
    ("elevenlabs_server_events", None),
    ("elevenlabs_voice_override", Some("elevenlabs_omit_voice")),
    // Hume EVI: the documented client messages and settings, and the server's message shapes.
    ("hume_user_input", Some("hume_text_input")),
    ("hume_audio_settings", Some("hume_settings_when_needed")),
    ("hume_cancel_local", Some("hume_stop_assistant")),
    ("hume_tools", Some("hume_omit_tools")),
    ("hume_voice_settings", Some("hume_voice_query")),
    ("hume_message_ids", Some("hume_drop_id_less_messages")),
    ("hume_wav_output", None),
    ("hume_interruption", None),
    // Inworld (and Yandex for the voice): the key as given, its own session id, its STT
    // model, voice, LLM, `auto_tool_response: false`, transcripts without `content_index`,
    // its `response.create`; Yandex's `languages`.
    ("inworld_basic_key", Some("inworld_encode_key")),
    ("inworld_session_key", Some("inworld_endpoint_session_only")),
    ("inworld_stt_default", Some("inworld_whisper_model")),
    ("openai_clone_voice", Some("openai_clone_voice_alloy")),
    ("inworld_model", Some("inworld_omit_model")),
    ("inworld_tool_response", Some("inworld_auto_tool_response")),
    ("inworld_transcript_index", None),
    ("inworld_response_shape", Some("inworld_openai_response")),
    ("yandex_languages", Some("yandex_omit_languages")),
];

/// The fixes whose vendor's live probe passed: in force without being listed (sign-off 9:
/// interaction-off changes ship with their escape flag, which turns one off).
/// `deepgram_user_text`: probed 2026-10-15 and 2026-10-19 against the Deepgram Voice Agent.
/// `deepgram_tools` and `deepgram_update_prompt`: probed 2026-10-20 through the gateway (the
/// agent called the session's function and answered from its result; a second `Settings` is
/// refused by Deepgram, `UpdatePrompt` is taken).
/// `grok_session`, `grok_events` and `grok_voice_default`: probed 2026-10-23 through the
/// gateway's `/realtime` against xAI (`grok-voice-latest`, which xAI reports as
/// `grok-voice-3.0`): the chosen voice speaks (`rex`), the caller's audio is transcribed once
/// and answered in audio with its transcript, typed text is answered as the user's, a session
/// function is called and answered from its result, new instructions apply mid-session, a
/// cancel ends the response (`cancelled`), manual turns (`turn_detection` `{"type": null}`)
/// answer only after a commit, and `response_done` reaches the client. Without them xAI uses
/// its default voice, refuses the manual session (`invalid_event`: `turn_detection.type`
/// `none`), logs the caller's line once per in-progress `.completed`, and no `response_done`
/// is sent.
///
/// On by documentation evidence, not a live probe (24 October, the lead's decision on the
/// owner's standing instruction to fix the providers in full; each reversible with its escape
/// flag): the round-9 fixes whose today's frame the vendor's own reference refuses, so that no
/// session that works today can change.
/// - `azure_ga_url`: "Use the `/openai/v1/realtime` path with `model=` as the query parameter.
///   Don't include an `api-version` parameter." and "A `model` query string parameter with the
///   name of your deployed realtime model." (the page's note and URI list, read 24 October,
///   page updated 23 October 2026:
///   <https://learn.microsoft.com/en-us/azure/ai-foundry/openai/how-to/realtime-audio-websockets>);
///   today's URL is the GA path with the preview's `deployment=`, which the page no longer names.
/// - `openai_manual_turn_null`: GA's `turn_detection` is `server_vad` or `semantic_vad`; "To
///   clear a field like `turn_detection`, pass `null`"
///   (<https://github.com/openai/openai-python/blob/main/src/openai/types/realtime/session_update_event_param.py>);
///   today's `{"type": "none"}` is neither (xAI refused it live, 23 October). Yandex, whose
///   reference is silent, keeps today's frame.
/// - `openai_replay_output_text`: an assistant message's content is "`output_text` or
///   `output_audio`" (openai-python `realtime_conversation_item_assistant_message_param.py`);
///   Inworld: "Assistant item snapshots use output_text/output_audio" (its legacy `text` is
///   also accepted, <https://docs.inworld.ai/api-reference/realtimeAPI/realtime/realtime-websocket.md>).
///   Grok (xAI's types are `text` / `audio`) and Yandex keep `text`.
/// - `hume_user_input`: EVI's client messages are `audio_input`, `session_settings`,
///   `user_input`, `assistant_input`, `tool_response`, `tool_error`, `pause_assistant_message`,
///   `resume_assistant_message`; "Text sent through a User Input message is treated as the
///   user's speech to EVI" (<https://dev.hume.ai/reference/speech-to-speech-evi/chat>); today's
///   `text_input` is not among them.
/// - `hume_audio_settings`: "Linear 16 PCM is a headerless audio format ... you must first send
///   a session_settings message" (<https://dev.hume.ai/docs/speech-to-speech-evi/guides/audio>);
///   the gateway always streams linear16 and today declares it only with instructions.
/// - `hume_cancel_local`: today's `stop_assistant` is not an EVI client message (list above).
/// - `hume_message_ids`: EVI's `user_message` has no `id` (its fields: `custom_session_id`,
///   `from_text`, `interim`, `language`, `message`, `models`, `time`, `type`); today every
///   caller transcript is dropped.
/// - `inworld_basic_key`: "Do not encode a copied Base64 credential again"
///   (<https://docs.inworld.ai/portal/api-key-auth>); a `key_id:secret` pair is still encoded
///   once, as today.
/// - `inworld_session_key`: `key` is the "Session ID from your app"
///   (<https://docs.inworld.ai/docs/realtime/connect/websocket>); today a session without an
///   `endpoint` is refused, and one with an `endpoint` still dials it.
pub const FIXES_ON_BY_DEFAULT: [&str; 15] = [
    "deepgram_user_text",
    "deepgram_tools",
    "deepgram_update_prompt",
    "openai_manual_turn_null",
    "openai_replay_output_text",
    "azure_ga_url",
    "grok_session",
    "grok_events",
    "grok_voice_default",
    "hume_user_input",
    "hume_audio_settings",
    "hume_cancel_local",
    "hume_message_ids",
    "inworld_basic_key",
    "inworld_session_key",
];

/// The fixes in force: on by default or listed by the operator, and not turned back off by
/// an escape flag.
pub fn provider_fixes() -> Vec<String> {
    fixes_in_force(&server())
}

/// A `/realtime` session's own switches of the provider fixes (its config's
/// `provider_fixes`: fix name to `true` or `false`) over the fixes in force. `false` always
/// turns a fix off; `true` turns one on only with the operator's `provider_fixes_per_session`,
/// and never past its escape flag. The fixes the session runs with, and one finding per entry
/// not taken.
pub fn session_fixes(asked: Option<&Value>) -> (Vec<String>, Vec<ConfigWarning>) {
    session_fixes_of(&server(), asked)
}

fn session_fixes_of(
    server: &ServerInteraction,
    asked: Option<&Value>,
) -> (Vec<String>, Vec<ConfigWarning>) {
    let mut fixes = fixes_in_force(server);
    let mut walk = Walk::default();
    let Some(asked) = asked.filter(|asked| !asked.is_null()) else {
        return (fixes, Vec::new());
    };
    for (name, value, path) in walk.members(asked, "provider_fixes") {
        let Some((fix, escape)) = PROVIDER_FIXES.iter().find(|(fix, _)| *fix == name) else {
            walk.warn(
                "unknown_config_keys",
                &path,
                format!("{path} is not a provider fix; it is ignored."),
            );
            continue;
        };
        let escaped =
            escape.is_some_and(|flag| server.realtime_legacy_wire.iter().any(|f| f == flag));
        match value.as_bool() {
            Some(false) => fixes.retain(|on| on != fix),
            Some(true) if fixes.iter().any(|on| on == fix) => {}
            Some(true) if server.provider_fixes_per_session && !escaped => {
                fixes.push((*fix).to_owned());
            }
            Some(true) => walk.warn(
                "interaction_override_refused",
                &path,
                format!("{path}: the operator does not let a session turn this provider fix on; it stays off."),
            ),
            None => walk.bad(&path, "true or false"),
        }
    }
    // In the order of the list, as the operator's are.
    let order = |fix: &String| PROVIDER_FIXES.iter().position(|(name, _)| name == fix);
    fixes.sort_by_key(order);
    (fixes, walk.warnings)
}

/// Whether the provider fix `fix` (one of [`PROVIDER_FIXES`]) is in force.
pub fn provider_fix(fix: &str) -> bool {
    provider_fixes().iter().any(|on| on == fix)
}

fn fixes_in_force(server: &ServerInteraction) -> Vec<String> {
    PROVIDER_FIXES
        .iter()
        .filter(|(fix, escape)| {
            (FIXES_ON_BY_DEFAULT.contains(fix) || server.provider_fixes.iter().any(|on| on == fix))
                && !escape.is_some_and(|flag| server.realtime_legacy_wire.iter().any(|f| f == flag))
        })
        .map(|(fix, _)| (*fix).to_owned())
        .collect()
}

/// One entry of the operator's backend catalog. Server-side only.
#[derive(Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendSpec {
    /// What a session calls it: lower-case letters, digits, `_` or `-`.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: BackendKind,
    pub url: String,
    /// `openai_responses`: the model the requests name.
    #[serde(default)]
    pub model: Option<String>,
    /// `mcp`: the tool that does the task, and the argument that carries the request
    /// (`request` when not set).
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub argument: Option<String>,
    /// The environment variable that holds its secret: the signing secret of a webhook, the
    /// bearer token of the others.
    #[serde(default, alias = "bearer_env", alias = "api_key_env")]
    pub secret_env: Option<String>,
    /// `none`: a task changes nothing outside the backend, so a cancel need not be confirmed.
    #[serde(default)]
    pub side_effects: SideEffects,
    /// `openai_responses`: `poll` reads the response until it is done; anything else streams it.
    #[serde(default)]
    pub mode: Option<BackendMode>,
    /// One sentence an orchestrator reads to choose the entry ("Operates websites in a
    /// browser: bookings, forms, purchases").
    #[serde(default)]
    pub description: Option<String>,
    /// Tasks of one session that run on it at once; more wait in a queue. No limit when not
    /// set.
    #[serde(default)]
    pub max_concurrent: Option<u32>,
    /// How long the caller has to answer a question of this entry's, unless the question
    /// says itself (at most the session's `policy.hitl_answer_timeout_ms`).
    #[serde(default)]
    pub question_deadline_ms: Option<u64>,
    /// The entry that takes an orchestrator's new tasks while this one's breaker is open.
    #[serde(default)]
    pub fallback: Option<String>,
    /// `webhook`, `a2a`, `mcp`: how long a task may take before it fails (600 when not set).
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// `mcp`, as an orchestrator's agent: the server's tools it may call, each a tool of its
    /// own with the server's name, description and input schema: `["*"]` for every tool the
    /// server lists, or their names.
    #[serde(default)]
    pub tools: Vec<String>,
    /// `a2a`, `mcp`: how long the Agent Card or the tool list read at a session's start serves
    /// later sessions of this process (300 when not set).
    #[serde(default)]
    pub catalog_ttl_s: Option<u64>,
    /// Whether an orchestrator's call of this entry is confirmed with the caller before it is
    /// sent, over what `side_effects` and an MCP tool's annotations say: one answer for every
    /// call, or one per MCP tool name.
    #[serde(default)]
    pub confirm: Option<Confirm>,
    /// `mcp`, `a2a`: an OAuth 2.0 client of the server's authorisation server. The gateway
    /// fetches its access token (client credentials grant), renews it before it expires, and
    /// sends it as `Authorization: Bearer`, in place of `secret_env`. An A2A entry may leave
    /// `token_url` out: the agent card's OAuth 2.0 client-credentials flow names it.
    #[serde(default)]
    pub oauth: Option<OAuthClient>,
    /// `a2a`: the gateway's own address as the agent reaches it (`https://gateway.example.com`).
    /// With it, and a card that says `capabilities.pushNotifications`, a task that does not
    /// end on its first reply is followed through the agent's pushes to
    /// `<push_base_url>/v1/tasks/<task_id>/callback`.
    #[serde(default)]
    pub push_base_url: Option<String>,
    /// The value of `secret_env`, read when the server starts.
    #[serde(skip)]
    pub secret: Option<String>,
}

/// An OAuth 2.0 client (RFC 6749 section 4.4, client credentials) of a backend entry.
#[derive(Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthClient {
    /// The authorisation server's token endpoint. Empty on an A2A entry: the agent card's
    /// `clientCredentials` flow gives it.
    #[serde(default)]
    pub token_url: String,
    pub client_id: String,
    /// The environment variable that holds the client secret.
    pub client_secret_env: String,
    /// Space-separated scopes asked for.
    #[serde(default)]
    pub scope: Option<String>,
    /// The resource the token is for (RFC 8707 `resource`), normally the server's URL.
    #[serde(default)]
    pub resource: Option<String>,
    /// The value of `client_secret_env`, read when the server starts.
    #[serde(skip)]
    pub client_secret: Option<String>,
}

// Secrets stay out of logs.
impl fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthClient")
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.is_some())
            .finish()
    }
}

/// `confirm` of a catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Confirm {
    Every(bool),
    /// By MCP tool name; a tool not named is decided by its annotations.
    Tools(BTreeMap<String, bool>),
}

/// Most of the entry's tools an orchestrator gets.
const ENTRY_TOOLS_MAX: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Webhook,
    A2a,
    Mcp,
    OpenaiResponses,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::A2a => "a2a",
            Self::Mcp => "mcp",
            Self::OpenaiResponses => "openai_responses",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffects {
    #[default]
    Possible,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendMode {
    Callback,
    Poll,
    Stream,
}

impl BackendSpec {
    /// `tasks.backend` of the settings echo, and `background` of `task_created`.
    pub fn label(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.name)
    }

    /// What makes the entry unusable.
    fn problem(&self) -> Option<&'static str> {
        let named = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        if self.name.is_empty() || self.name.len() > 64 || !self.name.chars().all(named) {
            Some("its name must be 1 to 64 lower-case letters, digits, '_' or '-'")
        } else if !(self.url.starts_with("https://") || self.url.starts_with("http://")) {
            Some("its url must be an http or https URL")
        } else if self.kind == BackendKind::Mcp && self.tool.is_none() && self.tools.is_empty() {
            Some("an mcp backend needs `tool` or `tools`")
        } else if !self.tools.is_empty() && self.kind != BackendKind::Mcp {
            Some("only an mcp backend has `tools`")
        } else if self.tools.len() > ENTRY_TOOLS_MAX || self.tools.iter().any(String::is_empty) {
            Some("`tools` is `[\"*\"]` or at most 32 tool names")
        } else if self.catalog_ttl_s == Some(0) {
            Some("catalog_ttl_s must be at least 1")
        } else if self.kind == BackendKind::OpenaiResponses && self.model.is_none() {
            Some("an openai_responses backend needs `model`")
        } else if self.max_concurrent == Some(0) || self.timeout_s == Some(0) {
            Some("max_concurrent and timeout_s must be at least 1")
        } else if self.fallback.as_ref() == Some(&self.name) {
            Some("its fallback must be another entry")
        } else if self.oauth.is_some() && !matches!(self.kind, BackendKind::Mcp | BackendKind::A2a)
        {
            Some("only an mcp or a2a backend has `oauth`")
        } else if self.oauth.is_some() && self.secret_env.is_some() {
            Some("an entry has `secret_env` or `oauth`, not both")
        } else if self.oauth.as_ref().is_some_and(|oauth| {
            // An A2A agent's card may name the token endpoint; an MCP entry names it.
            let from_card = oauth.token_url.is_empty() && self.kind == BackendKind::A2a;
            !from_card && !web(&oauth.token_url)
        }) {
            Some("oauth.token_url must be an http or https URL")
        } else if self.push_base_url.is_some() && self.kind != BackendKind::A2a {
            Some("only an a2a backend has `push_base_url`")
        } else if self.push_base_url.as_deref().is_some_and(|url| !web(url)) {
            Some("push_base_url must be an http or https URL")
        } else {
            None
        }
    }
}

fn web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

// Secrets stay out of logs.
impl fmt::Debug for BackendSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackendSpec")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("url", &self.url)
            .field("secret", &self.secret.is_some())
            .field("oauth", &self.oauth)
            .finish()
    }
}

/// The llama-server that runs the front model. Server-side only.
#[derive(Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontModelServer {
    pub url: String,
    /// More servers (spec 029 SF-3): every gateway shares them with `url`. A hostname with
    /// several addresses is one server per address, in either.
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    /// The gateway runs the server itself with this command (program and arguments), and
    /// restarts it (`front_supervisor`). It must listen where `url` says.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// `lfm_open_eligible` or `liquid_commercial`: the operator may run LFM weights.
    #[serde(default)]
    pub licence: Option<String>,
    /// How long a decision's line may take once its act is known, in milliseconds (default
    /// [`LINE_BUDGET_MS`], at most [`MAX_LINE_BUDGET_MS`]). The act settles the turn and the
    /// caller waits for its line: a slow server's line is late, not lost.
    #[serde(default)]
    pub line_budget_ms: Option<u32>,
}

/// A decision's line's time after its act, unless the operator sets another.
pub const LINE_BUDGET_MS: u32 = 1_500;
/// The longest an operator may give a line.
pub const MAX_LINE_BUDGET_MS: u32 = 10_000;

impl FrontModelServer {
    /// How long a decision's line may take once its act is known.
    pub fn line_budget(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.line_budget_ms.unwrap_or(LINE_BUDGET_MS).into())
    }
}

// Secrets stay out of logs.
impl fmt::Debug for ServerInteraction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerInteraction")
            .field("enabled", &self.enabled)
            .field("default", &self.default.is_some())
            .field("agent_default", &self.agent_default.is_some())
            .field("front_model", &self.front_model)
            .field("tool_secret", &self.tool_secret.is_some())
            .field("allow_client_tools", &self.allow_client_tools)
            .field("task_redis_url", &self.task_redis_url.is_some())
            .field("task_backends", &self.task_backends)
            .field("allow_client_backends", &self.allow_client_backends)
            .field("allow_resume_key", &self.allow_resume_key)
            .field("allow_client_push", &self.allow_client_push)
            .field("tenant_personas", &self.tenant_personas)
            .field("caller_memory_key", &self.caller_memory_key.is_some())
            .field("realtime_legacy_wire", &self.realtime_legacy_wire)
            .field("provider_fixes", &self.provider_fixes)
            .field(
                "provider_fixes_per_session",
                &self.provider_fixes_per_session,
            )
            .field("packs_dir", &self.packs_dir)
            .field("clips_dir", &self.clips_dir)
            .field("failure_voice", &self.failure_voice.is_some())
            .finish()
    }
}

impl fmt::Debug for FrontModelServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrontModelServer")
            .field("url", &self.url)
            .field("urls", &self.urls)
            .field("model", &self.model)
            .field("api_key", &self.api_key.is_some())
            .field("command", &self.command)
            .field("licence", &self.licence)
            .field("line_budget_ms", &self.line_budget_ms)
            .finish()
    }
}

impl ServerInteraction {
    /// The block from `WAAV_INTERACTION_*` alone; `get` returns a variable's value.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut server = Self::default();
        server.apply_lookup(get);
        server
    }

    /// Lay the environment over this block. Returns what could not be read.
    fn apply_lookup(&mut self, get: impl Fn(&str) -> Option<String>) -> Vec<String> {
        let mut problems = Vec::new();
        let get = |key: &str| {
            get(key)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let mut flag = |key: &str, field: &mut bool| match get(key).map(|value| parse_bool(&value))
        {
            None => {}
            Some(Some(flag)) => *field = flag,
            Some(None) => problems.push(format!("{key} must be true or false")),
        };
        flag("WAAV_INTERACTION_ENABLED", &mut self.enabled);
        flag(
            "WAAV_INTERACTION_ALLOW_CLIENT_TOOLS",
            &mut self.allow_client_tools,
        );
        flag(
            "WAAV_INTERACTION_ALLOW_CLIENT_BACKENDS",
            &mut self.allow_client_backends,
        );
        flag(
            "WAAV_INTERACTION_ALLOW_RESUME_KEY",
            &mut self.allow_resume_key,
        );
        flag(
            "WAAV_INTERACTION_ALLOW_CLIENT_PUSH",
            &mut self.allow_client_push,
        );
        flag(
            "WAAV_INTERACTION_TENANT_PERSONAS",
            &mut self.tenant_personas,
        );
        flag(
            "WAAV_PROVIDER_FIXES_PER_SESSION",
            &mut self.provider_fixes_per_session,
        );
        match get("WAAV_INTERACTION_FAILURE_VOICE").map(|text| serde_json::from_str(&text)) {
            None => {}
            Some(Ok(voice @ Value::Object(_))) => self.failure_voice = Some(voice),
            Some(_) => problems.push(
                "WAAV_INTERACTION_FAILURE_VOICE must be JSON: a tts_config object".to_owned(),
            ),
        }
        for (key, field) in [
            ("WAAV_INTERACTION_DEFAULT", &mut self.default),
            ("WAAV_INTERACTION_AGENT_DEFAULT", &mut self.agent_default),
            (
                "WAAV_INTERACTION_REALTIME_DEFAULT",
                &mut self.realtime_default,
            ),
        ] {
            match get(key).map(|text| serde_json::from_str::<Value>(&text)) {
                None => {}
                Some(Ok(block)) if block.is_object() || block == Value::Bool(true) => {
                    *field = Some(block)
                }
                Some(_) => problems.push(format!("{key} must be JSON: true or an object")),
            }
        }
        // One URL, or several separated by commas: the servers every gateway shares.
        if let Some(urls) = get("WAAV_INTERACTION_FRONT_URL") {
            let mut urls = urls
                .split(',')
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_owned);
            let url = urls.next().unwrap_or_default();
            let more: Vec<String> = urls.collect();
            match &mut self.front_model {
                Some(front) => {
                    front.url = url;
                    front.urls = more;
                }
                None => {
                    self.front_model = Some(FrontModelServer {
                        url,
                        urls: more,
                        ..FrontModelServer::default()
                    })
                }
            }
        }
        // The front's key from the environment (spec 029 SF-6): a key in the config file
        // would sit in a ConfigMap.
        if let Some(key) = get("WAAV_INTERACTION_FRONT_API_KEY").filter(|key| !key.is_empty()) {
            match &mut self.front_model {
                Some(front) => front.api_key = Some(key),
                None => problems.push(
                    "WAAV_INTERACTION_FRONT_API_KEY is set but no front model URL is".to_owned(),
                ),
            }
        }
        for (key, is_command) in [
            ("WAAV_INTERACTION_FRONT_COMMAND", true),
            ("WAAV_FRONT_LICENCE", false),
        ] {
            let Some(value) = get(key) else {
                continue;
            };
            match &mut self.front_model {
                Some(front) if is_command => {
                    front.command = Some(value.split_whitespace().map(str::to_owned).collect())
                }
                Some(front) => front.licence = Some(value),
                None => problems.push(format!("{key} is set but no front model URL is")),
            }
        }
        if let Some(front) = &mut self.front_model
            && front
                .licence
                .take_if(|licence| !super::front_supervisor::LICENCES.contains(&licence.as_str()))
                .is_some()
        {
            problems.push(
                "interaction.front_model.licence must be lfm_open_eligible or liquid_commercial"
                    .to_owned(),
            );
        }
        if let Some(value) = get("WAAV_INTERACTION_FRONT_LINE_BUDGET_MS") {
            match (&mut self.front_model, value.trim().parse::<u32>()) {
                (Some(front), Ok(ms)) if ms <= MAX_LINE_BUDGET_MS => {
                    front.line_budget_ms = Some(ms)
                }
                (Some(_), _) => problems.push(format!(
                    "WAAV_INTERACTION_FRONT_LINE_BUDGET_MS must be 0 to {MAX_LINE_BUDGET_MS} \
                     milliseconds"
                )),
                (None, _) => problems.push(
                    "WAAV_INTERACTION_FRONT_LINE_BUDGET_MS is set but no front model URL is"
                        .to_owned(),
                ),
            }
        }
        if let Some(front) = &mut self.front_model
            && front
                .line_budget_ms
                .take_if(|ms| *ms > MAX_LINE_BUDGET_MS)
                .is_some()
        {
            problems.push(format!(
                "interaction.front_model.line_budget_ms must be at most {MAX_LINE_BUDGET_MS}"
            ));
        }
        if let Some(model) = get("WAAV_INTERACTION_FRONT_MODEL") {
            match &mut self.front_model {
                Some(front) => front.model = Some(model),
                None => problems.push(
                    "WAAV_INTERACTION_FRONT_MODEL is set but no front model URL is".to_owned(),
                ),
            }
        }
        for (key, field) in [
            ("WAAV_INTERACTION_TOOL_SECRET", &mut self.tool_secret),
            ("WAAV_INTERACTION_PACKS_DIR", &mut self.packs_dir),
            ("WAAV_INTERACTION_CLIPS_DIR", &mut self.clips_dir),
            ("WAAV_TASK_REDIS_URL", &mut self.task_redis_url),
            (
                "WAAV_INTERACTION_CALLER_MEMORY_KEY",
                &mut self.caller_memory_key,
            ),
        ] {
            if let Some(value) = get(key) {
                *field = Some(value);
            }
        }
        let list = |text: String| {
            text.split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_owned)
                .collect()
        };
        if let Some(acks) = get("WAAV_INTERACTION_TRUTH_ACK") {
            self.truth_ack = list(acks);
        }
        if let Some(flags) = get("WAAV_REALTIME_LEGACY_WIRE") {
            self.realtime_legacy_wire = list(flags);
        }
        self.realtime_legacy_wire.retain(|flag| {
            let known = LEGACY_WIRE.contains(&flag.as_str());
            if !known {
                problems.push(format!(
                    "realtime_legacy_wire: `{flag}` is not an escape flag"
                ));
            }
            known
        });
        if let Some(fixes) = get("WAAV_PROVIDER_FIXES") {
            self.provider_fixes = list(fixes);
        }
        self.provider_fixes.retain(|fix| {
            let known = PROVIDER_FIXES.iter().any(|(name, _)| name == fix);
            if !known {
                problems.push(format!("provider_fixes: `{fix}` is not a provider fix"));
            }
            known
        });
        if let Some(text) = get("WAAV_INTERACTION_TASK_BACKENDS") {
            match serde_json::from_str(&text) {
                Ok(backends) => self.task_backends = backends,
                Err(error) => problems.push(format!(
                    "WAAV_INTERACTION_TASK_BACKENDS must be a JSON list of backends: {error}"
                )),
            }
        }
        // An entry that cannot work is dropped, so a session that names it is told.
        let mut names = std::collections::HashSet::new();
        self.task_backends.retain_mut(|backend| {
            let problem = backend.problem().or_else(|| {
                (!names.insert(backend.name.clone())).then_some("its name is used twice")
            });
            if let Some(problem) = problem {
                problems.push(format!("task backend `{}`: {problem}", backend.name));
                return false;
            }
            if let Some(variable) = &backend.secret_env {
                backend.secret = get(variable);
                if backend.secret.is_none() {
                    problems.push(format!(
                        "task backend `{}`: {variable} is not set",
                        backend.name
                    ));
                }
            }
            if let Some(oauth) = &mut backend.oauth {
                oauth.client_secret = get(&oauth.client_secret_env);
                if oauth.client_secret.is_none() {
                    problems.push(format!(
                        "task backend `{}`: {} is not set",
                        backend.name, oauth.client_secret_env
                    ));
                }
            }
            true
        });
        // A fallback names an entry that is left.
        let names: Vec<String> = self.task_backends.iter().map(|b| b.name.clone()).collect();
        for backend in &mut self.task_backends {
            if let Some(fallback) = backend.fallback.take_if(|name| !names.contains(name)) {
                problems.push(format!(
                    "task backend `{}`: its fallback `{fallback}` is not in the list",
                    backend.name
                ));
            }
        }
        problems
    }
}

/// The `interaction` key of the server's YAML document. A block that does not parse leaves
/// interaction mode off.
fn server_from_yaml(text: &str) -> Result<ServerInteraction, String> {
    let document: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| format!("the config file is not YAML: {e}"))?;
    match document.get(ROOT) {
        Some(block) if !block.is_null() => serde_yaml::from_value(block.clone()).map_err(|e| {
            format!("the `interaction` block is invalid, so interaction mode is off: {e}")
        }),
        _ => Ok(ServerInteraction::default()),
    }
}

/// The server block from the YAML text (when there is a file) with the environment laid over
/// it, and what was wrong with either.
fn load_server(
    yaml: Option<Result<String, String>>,
    get: impl Fn(&str) -> Option<String>,
) -> (ServerInteraction, Vec<String>) {
    let mut problems = Vec::new();
    let mut server = match yaml.map(|text| text.and_then(|text| server_from_yaml(&text))) {
        Some(Ok(server)) => server,
        Some(Err(problem)) => {
            problems.push(problem);
            ServerInteraction::default()
        }
        None => ServerInteraction::default(),
    };
    problems.extend(server.apply_lookup(get));
    let probe = ServerInteraction {
        enabled: true,
        ..server.clone()
    };
    let operator = ParseCtx {
        operator: true,
        ..ParseCtx::default()
    };
    for (name, block) in [
        ("default", &server.default),
        ("agent_default", &server.agent_default),
    ] {
        if let Some(block) = block {
            let (_, warnings) = parse(block, &probe, operator);
            let found = warnings
                .into_iter()
                .map(|w| format!("{name} block: {}", w.message));
            problems.extend(found);
        }
    }
    (server, problems)
}

static SERVER: RwLock<Option<Arc<ServerInteraction>>> = RwLock::new(None);

/// Read the server block at startup: the YAML key `interaction` of the server config file
/// (when there is one), then the `WAAV_INTERACTION_*` variables over it. Returns the problems
/// found, for the caller to log; a problem never stops the server.
pub fn init_server(path: Option<&Path>) -> Vec<String> {
    let yaml = path.map(|path| {
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    });
    let (server, problems) = load_server(yaml, |key| std::env::var(key).ok());
    store(server);
    problems
}

/// The server block. Read from the environment alone when [`init_server`] never ran.
pub fn server() -> Arc<ServerInteraction> {
    if let Some(server) = SERVER
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
    {
        return server.clone();
    }
    SERVER
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(|| {
            Arc::new(ServerInteraction::from_lookup(|key| {
                std::env::var(key).ok()
            }))
        })
        .clone()
}

fn store(server: ServerInteraction) {
    *SERVER.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(server));
}

/// Replace the server block. For tests, which must serialise on one lock of their own.
#[doc(hidden)]
pub fn set_server_for_tests(server: ServerInteraction) {
    store(server);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> ServerInteraction {
        ServerInteraction {
            enabled: true,
            ..ServerInteraction::default()
        }
    }

    fn with_front_model() -> ServerInteraction {
        ServerInteraction {
            front_model: Some(FrontModelServer {
                url: "http://127.0.0.1:18741".to_owned(),
                urls: Vec::new(),
                model: None,
                api_key: None,
                command: None,
                licence: None,
                line_budget_ms: None,
            }),
            ..on()
        }
    }

    /// The defaults of a parsed session: the front is always the model's.
    fn defaults() -> InteractionConfig {
        let mut cfg = InteractionConfig::default();
        cfg.front.profile = FrontProfile::Model;
        cfg
    }

    /// No config type prints a secret through `{:?}`: the session's push secret, resume key,
    /// LLM key, identity token, a backend's secret and OAuth client secret, the server's keys.
    #[test]
    fn secrets_never_print_through_debug() {
        let mut cfg = InteractionConfig::default();
        cfg.tasks.push_url = Some("https://hooks.example.com/p?token=SECRET-0".to_owned());
        cfg.tasks.push_secret = Some("SECRET-1-push-secret".to_owned());
        cfg.tasks.resume_key = Some("SECRET-2-resume".to_owned());
        cfg.tasks.llm = Some(json!({"model": "m", "api_key": "SECRET-3"}));
        cfg.tasks.orchestrator = Some(json!({"model": "o", "api_key": "SECRET-4"}));
        cfg.front.identity_token = Some("SECRET-5-identity".to_owned());
        cfg.front.memory = vec!["SECRET-6 the caller's allergy".to_owned()];
        let oauth = OAuthClient {
            token_url: "https://auth.example.com/token".to_owned(),
            client_id: "client".to_owned(),
            client_secret_env: "ENV".to_owned(),
            scope: None,
            resource: None,
            client_secret: Some("SECRET-7-oauth".to_owned()),
        };
        let backend: BackendSpec = serde_json::from_value(
            json!({"name": "b", "type": "a2a", "url": "https://agent.example.com"}),
        )
        .unwrap();
        let backend = BackendSpec {
            secret: Some("SECRET-8-backend".to_owned()),
            oauth: Some(oauth),
            ..backend
        };
        let server = ServerInteraction {
            tool_secret: Some("SECRET-9-tool".to_owned()),
            caller_memory_key: Some("SECRET-10-memory".to_owned()),
            task_backends: vec![backend],
            ..with_front_model()
        };
        let session = super::super::store::Session {
            owner: "o".to_owned(),
            id: "s".to_owned(),
            push_url: cfg.tasks.push_url.clone(),
            grace: std::time::Duration::ZERO,
            resume: cfg.tasks.resume_key.clone(),
        };
        let printed = format!("{cfg:?} {:?} {server:?} {session:?}", cfg.tasks);
        assert!(!printed.contains("SECRET"), "a secret printed: {printed}");
        // What is set is still told.
        assert!(printed.contains("push_secret: true") && printed.contains("resume_key: true"));
        assert!(
            printed.contains("identity_token: true") && printed.contains("client_secret: true")
        );
    }

    /// `(code, detail.path)` of each warning, sorted: the order of an object's keys is not
    /// part of the contract.
    fn paths(warnings: &[ConfigWarning]) -> Vec<(&'static str, String)> {
        let mut paths: Vec<_> = warnings
            .iter()
            .map(|w| (w.code, w.detail["path"].as_str().unwrap().to_owned()))
            .collect();
        paths.sort();
        paths
    }

    fn tool(name: &str) -> Value {
        json!({ "name": name, "url": "https://tools.example.com/hook" })
    }

    #[test]
    fn true_gives_the_plan_defaults() {
        let (cfg, warnings) = parse(&json!(true), &on(), ParseCtx::default());
        assert!(warnings.is_empty());
        let cfg = cfg.unwrap();
        assert_eq!(cfg, defaults());
        assert_eq!(
            (cfg.front.act_budget_ms, cfg.front.total_budget_ms),
            (300, 1500)
        );
        let p = &cfg.policy;
        assert_eq!(
            [
                p.ack_due_ms,
                p.fast_answer_ms,
                p.fast_answer_min_turns,
                p.ack_hard_deadline_ms,
                p.amend_ack_target_ms,
                p.answer_window_ms,
                p.heard_margin_ms,
                p.progress_min_wait_ms,
                p.progress_min_gap_ms,
                p.updates_max_per_wait,
                p.hold_after_silence_ms,
                p.quiet_checkin_ms,
                p.agreed_wait_max_ms,
                p.hitl_reprompt_after_ms,
                p.hitl_answer_timeout_ms,
            ],
            [
                300, 700, 3, 2000, 1200, 4000, 150, 4000, 5000, 3, 8000, 30_000, 600_000, 8000,
                20_000
            ]
        );
        let d = &cfg.duplex;
        assert!(d.barge_in && d.eager);
        assert_eq!(d.duck_db, -15.0);
        assert_eq!(
            [
                d.duck_ramp_ms,
                d.pause_after_ms,
                d.decide_by_ms,
                d.max_pause_ms
            ],
            [60, 400, 900, 2500]
        );
        assert_eq!(cfg.tasks, TasksConfig::default());
    }

    /// `speech.realtime` is read by a `/ws` session in a room before the block is parsed:
    /// anywhere else it is ignored, and the rest of the block stands.
    #[test]
    fn a_realtime_voice_outside_a_room_is_ignored() {
        let raw =
            json!({"speech": {"realtime": {"provider": "openai"}, "claim_policy": "correct"}});
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_value_ignored",
                "interaction.speech.realtime".to_owned()
            )]
        );
        assert_eq!(cfg.unwrap().speech.claim_policy, "correct");
    }

    /// `speech.voice`: the gateway's own voice on a speech-to-speech session, a `tts_config`
    /// object; anything else is ignored with a warning.
    #[test]
    fn the_gateways_voice_is_a_tts_config() {
        let voice = json!({"provider": "deepgram", "voice_id": "aura-2-thalia-en"});
        let raw = json!({"speech": {"voice": voice, "voice_owner": "gateway"}});
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        let cfg = cfg.unwrap();
        assert_eq!(cfg.speech_voice, Some(voice));
        assert_eq!(cfg.speech.voice_owner, "gateway");
        let (cfg, warnings) = parse(
            &json!({"speech": {"voice": "x"}}),
            &on(),
            ParseCtx::default(),
        );
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_value_ignored",
                "interaction.speech.voice".to_owned()
            )]
        );
        assert_eq!(cfg.unwrap().speech_voice, None);
    }

    #[test]
    fn absent_false_and_wrong_type_are_off() {
        for raw in [Value::Null, json!(false)] {
            assert_eq!(parse(&raw, &on(), ParseCtx::default()), (None, Vec::new()));
        }
        let (cfg, warnings) = parse(&json!("yes"), &on(), ParseCtx::default());
        assert_eq!(cfg, None);
        assert_eq!(
            paths(&warnings),
            [("interaction_value_ignored", "interaction".to_owned())]
        );
    }

    #[test]
    fn a_full_object_is_read() {
        let server = ServerInteraction {
            allow_client_tools: true,
            ..with_front_model()
        };
        let raw = json!({
            "front": {
                "profile": "model", "persona": "northwind-travel-ava",
                "act_budget_ms": 250, "total_budget_ms": 1200, "preferred_name": "Sam"
            },
            "tasks": {
                "llm": { "provider": "openai", "model": "qwen3b-q4_k_m" },
                "tools": [{
                    "name": "book_table", "description": "Book a table",
                    "parameters": { "type": "object", "properties": { "party": { "type": "integer" } } },
                    "url": "https://tools.example.com/book", "irreversible": true,
                    "confirm_phrase": "book a table", "progress_phrase": "Booking the table"
                }, {
                    "name": "search_flights", "url": "http://tools.example.com/search"
                }],
                "speculative_start": "off"
            },
            "policy": {
                "ack_due_ms": 0, "fast_answer_ms": 600, "fast_answer_min_turns": 4,
                "ack_hard_deadline_ms": 1800, "amend_ack_target_ms": 1000, "answer_window_ms": 5000,
                "heard_margin_ms": 200, "progress_min_wait_ms": 3000, "progress_min_gap_ms": 6000,
                "updates_max_per_wait": 2, "hold_after_silence_ms": 9000, "quiet_checkin_ms": 20000,
                "agreed_wait_max_ms": 120000, "hitl_reprompt_after_ms": 6000,
                "hitl_answer_timeout_ms": 30000
            },
            "duplex": {
                "barge_in": false, "duck_db": -12, "duck_ramp_ms": 80, "pause_after_ms": 500,
                "decide_by_ms": 1000, "max_pause_ms": 3000, "eager": false,
                "echo": { "correlation_gate": false, "aec": "on" }
            },
            "events": { "floor": false }
        });
        let (cfg, warnings) = parse(&raw, &server, ParseCtx::default());
        assert_eq!(warnings, Vec::new());
        let cfg = cfg.unwrap();
        assert_eq!(
            cfg.front,
            FrontConfig {
                profile: FrontProfile::Model,
                persona: "northwind-travel-ava".to_owned(),
                act_budget_ms: 250,
                total_budget_ms: 1200,
                preferred_name: Some("Sam".to_owned()),
                ..FrontConfig::default()
            }
        );
        assert_eq!(cfg.tasks.llm, Some(raw["tasks"]["llm"].clone()));
        assert_eq!(cfg.tasks.speculative_start, Tri::Off);
        assert_eq!(
            cfg.tasks.tools,
            [
                ToolSpec {
                    name: "book_table".to_owned(),
                    description: "Book a table".to_owned(),
                    parameters: raw["tasks"]["tools"][0]["parameters"].clone(),
                    url: "https://tools.example.com/book".to_owned(),
                    side_effects: true,
                    irreversible: true,
                    confirm_phrase: Some("book a table".to_owned()),
                    progress_phrase: Some("Booking the table".to_owned()),
                    signed: false,
                    confirm_template: None,
                },
                ToolSpec {
                    name: "search_flights".to_owned(),
                    description: String::new(),
                    parameters: json!({ "type": "object", "properties": {} }),
                    url: "http://tools.example.com/search".to_owned(),
                    side_effects: false,
                    irreversible: false,
                    confirm_phrase: None,
                    progress_phrase: None,
                    signed: false,
                    confirm_template: None,
                },
            ]
        );
        assert_eq!(
            cfg.policy,
            PolicyConfig {
                ack_due_ms: 0,
                fast_answer_ms: 600,
                fast_answer_min_turns: 4,
                ack_hard_deadline_ms: 1800,
                amend_ack_target_ms: 1000,
                answer_window_ms: 5000,
                heard_margin_ms: 200,
                progress_min_wait_ms: 3000,
                progress_min_gap_ms: 6000,
                updates_max_per_wait: 2,
                hold_after_silence_ms: 9000,
                quiet_checkin_ms: 20_000,
                agreed_wait_max_ms: 120_000,
                hitl_reprompt_after_ms: 6000,
                hitl_answer_timeout_ms: 30_000,
                hitl_key_gap_ms: 5000,
            }
        );
        assert_eq!(
            cfg.duplex,
            DuplexConfig {
                barge_in: false,
                duck_db: -12.0,
                duck_ramp_ms: 80,
                pause_after_ms: 500,
                decide_by_ms: 1000,
                max_pause_ms: 3000,
                eager: false,
                echo: EchoConfig {
                    correlation_gate: false,
                    aec: Tri::On,
                    ..EchoConfig::default()
                },
                backchannel: Tri::Auto,
                backchannel_intensity: None,
            }
        );
        assert!(!cfg.events_floor);
    }

    #[test]
    fn each_bad_field_warns_once_and_keeps_its_default() {
        let server = ServerInteraction {
            allow_client_tools: true,
            ..on()
        };
        // (section, key, bad value): a wrong type, an out-of-range number or an unknown name.
        let cases = [
            ("front", "profile", json!("lfm")),
            ("front", "persona", json!(7)),
            ("front", "act_budget_ms", json!(301)),
            ("front", "total_budget_ms", json!("fast")),
            ("front", "preferred_name", json!("")),
            ("tasks", "llm", json!("gpt")),
            ("tasks", "tools", json!({})),
            ("tasks", "speculative_start", json!(true)),
            ("policy", "ack_due_ms", json!(801)),
            ("policy", "fast_answer_ms", json!(-1)),
            ("policy", "fast_answer_min_turns", json!(0)),
            ("policy", "ack_hard_deadline_ms", json!(100)),
            ("policy", "amend_ack_target_ms", json!(1.5)),
            ("policy", "answer_window_ms", json!("4000")),
            ("policy", "heard_margin_ms", json!(1001)),
            ("policy", "progress_min_wait_ms", json!(1999)),
            ("policy", "progress_min_gap_ms", json!(15001)),
            ("policy", "updates_max_per_wait", json!(6)),
            ("policy", "hold_after_silence_ms", json!(3999)),
            ("policy", "quiet_checkin_ms", json!(9999)),
            ("policy", "agreed_wait_max_ms", json!(5_000_000_000u64)),
            ("policy", "hitl_reprompt_after_ms", json!(false)),
            ("policy", "hitl_answer_timeout_ms", json!([20000])),
            ("duplex", "barge_in", json!("two_stage")),
            ("duplex", "duck_db", json!(3)),
            ("duplex", "duck_ramp_ms", json!(501)),
            ("duplex", "pause_after_ms", json!(99)),
            ("duplex", "decide_by_ms", json!(299)),
            ("duplex", "max_pause_ms", json!(10001)),
            ("duplex", "eager", json!(1)),
            ("events", "floor", json!("yes")),
        ];
        for (section, key, bad) in cases {
            let raw = json!({ section: { key: bad } });
            let (cfg, warnings) = parse(&raw, &server, ParseCtx::default());
            assert_eq!(
                paths(&warnings),
                [(
                    "interaction_value_ignored",
                    format!("interaction.{section}.{key}")
                )],
                "{raw}"
            );
            assert_eq!(cfg.unwrap(), defaults(), "{raw}");
        }

        // A section of the wrong type, and unknown keys at both levels.
        let raw = json!({ "policy": 5, "duplex": { "barge": true }, "runtime": {} });
        let (cfg, warnings) = parse(&raw, &server, ParseCtx::default());
        assert_eq!(cfg.unwrap(), defaults());
        assert_eq!(
            paths(&warnings),
            [
                ("interaction_value_ignored", "interaction.policy".to_owned()),
                ("unknown_config_keys", "interaction.duplex.barge".to_owned()),
                ("unknown_config_keys", "interaction.runtime".to_owned()),
            ]
        );

        // A good value beside a bad one is kept; `null` is "not set".
        let raw = json!({ "policy": { "ack_due_ms": 200, "fast_answer_ms": 9999, "answer_window_ms": null } });
        let (cfg, warnings) = parse(&raw, &server, ParseCtx::default());
        assert_eq!(warnings.len(), 1);
        let policy = cfg.unwrap().policy;
        assert_eq!((policy.ack_due_ms, policy.fast_answer_ms), (200, 700));
    }

    #[test]
    fn a_bad_tool_is_dropped_and_the_others_are_kept() {
        let server = ServerInteraction {
            allow_client_tools: true,
            ..on()
        };
        let raw = json!({ "tasks": { "tools": [
            tool("lookup"),
            { "name": "no url" },
            { "name": "ftp_tool", "url": "ftp://example.com/x" },
            tool("ask_caller"),
            tool("lookup"),
            { "name": "extra", "url": "https://tools.example.com/x", "colour": "red", "side_effects": "yes" },
        ] } });
        let (cfg, warnings) = parse(&raw, &server, ParseCtx::default());
        let names: Vec<String> = cfg
            .unwrap()
            .tasks
            .tools
            .into_iter()
            .map(|t| t.name)
            .collect();
        // The last one's `side_effects` is not a boolean: kept, it would run with no
        // confirmation, so it goes too.
        assert_eq!(names, ["lookup"]);
        assert_eq!(
            paths(&warnings),
            [
                (
                    "interaction_value_ignored",
                    "interaction.tasks.tools[1].name".to_owned()
                ),
                (
                    "interaction_value_ignored",
                    "interaction.tasks.tools[2].url".to_owned()
                ),
                (
                    "interaction_value_ignored",
                    "interaction.tasks.tools[3].name".to_owned()
                ),
                (
                    "interaction_value_ignored",
                    "interaction.tasks.tools[4].name".to_owned()
                ),
                (
                    "interaction_value_ignored",
                    "interaction.tasks.tools[5].side_effects".to_owned()
                ),
                (
                    "unknown_config_keys",
                    "interaction.tasks.tools[5].colour".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn only_the_operators_tools_are_signed() {
        let server = ServerInteraction {
            allow_client_tools: true,
            default: Some(json!({ "tasks": { "tools": [tool("lookup")] } })),
            ..on()
        };
        let signed = |raw: &Value| -> Vec<(String, bool)> {
            let (cfg, _) = parse(raw, &server, ParseCtx::default());
            let tools = cfg.unwrap().tasks.tools;
            tools.into_iter().map(|t| (t.name, t.signed)).collect()
        };
        // The operator's block, when the session sends none or leaves the tools alone.
        let operators = [("lookup".to_owned(), true)];
        assert_eq!(signed(server.default.as_ref().unwrap()), operators);
        assert_eq!(signed(&json!(true)), operators);
        assert_eq!(
            signed(&json!({ "policy": { "ack_due_ms": 200 } })),
            operators
        );
        // A session's list replaces it whole: a tool it declares under the operator's name
        // and URL is still its own, with its own confirmation rule.
        let own = json!({ "tasks": { "tools": [tool("lookup"), tool("mine")] } });
        assert_eq!(
            signed(&own),
            [("lookup".to_owned(), false), ("mine".to_owned(), false)]
        );
    }

    #[test]
    fn the_release_2_keys_are_read_and_a_persona_comes_only_from_a_trusted_block() {
        let raw = json!({
            "front": { "caller": { "verbosity": "brief", "pace": "faster",
                                   "identity_token": "v1.abc.def" } },
            "duplex": { "backchannel": { "enabled": false } },
            "events": { "segments": false },
            "hitl": { "channels": ["voice"] },
        });
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(warnings, Vec::new());
        let cfg = cfg.unwrap();
        assert_eq!(
            (cfg.front.verbosity, cfg.front.pace),
            (Some(Verbosity::Brief), Some(Pace::Faster))
        );
        assert_eq!(cfg.front.identity_token.as_deref(), Some("v1.abc.def"));
        assert_eq!(cfg.duplex.backchannel, Tri::Off);
        assert!(!cfg.events_segments && !cfg.hitl_client);
        let echoed = echo(&cfg, "llm", &[]);
        assert_eq!(echoed["duplex"]["backchannel"], "off");
        assert_eq!(echoed["events"]["segments"], false);
        assert_eq!(echoed["hitl"]["channels"], json!(["voice"]));
        assert_eq!(echoed["front"]["verbosity"], "brief");
        assert!(echoed["front"].get("identity_token").is_none());

        // `true` is the short form; a wrong value keeps the default.
        let (cfg, _) = parse(
            &json!({ "duplex": { "backchannel": true } }),
            &on(),
            ParseCtx::default(),
        );
        assert_eq!(cfg.unwrap().duplex.backchannel, Tri::On);
        let bad =
            json!({ "duplex": { "backchannel": "sometimes" }, "hitl": { "channels": ["sms"] } });
        let (cfg, warnings) = parse(&bad, &on(), ParseCtx::default());
        assert_eq!(cfg.unwrap().duplex.backchannel, Tri::Auto);
        assert_eq!(warnings.len(), 2);

        // A persona object: from the operator, checked; from a session, refused.
        let persona = json!({ "id": "acme-ivy", "name": "Ivy", "org": "Acme",
                              "role": "booking assistant" });
        let block = json!({ "front": { "persona": persona.clone() } });
        let operator = ParseCtx {
            operator: true,
            ..ParseCtx::default()
        };
        let (cfg, warnings) = parse(&block, &on(), operator);
        let cfg = cfg.unwrap();
        assert_eq!(warnings, Vec::new());
        assert_eq!(cfg.front.persona, "acme-ivy");
        assert_eq!(cfg.front.load_persona().id(), "acme-ivy");
        let (cfg, warnings) = parse(&block, &on(), ParseCtx::default());
        assert_eq!(cfg.unwrap().front.persona, "default");
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_override_refused",
                "interaction.front.persona".to_owned()
            )]
        );
        let liar = json!({ "front": { "persona": { "id": "liar", "name": "L", "org": "O",
            "role": "r", "banks": { "ack": [{ "text": "It's all booked." }] } } } });
        let (cfg, warnings) = parse(&liar, &on(), operator);
        assert_eq!(cfg.unwrap().front.persona, "default");
        assert_eq!(paths(&warnings)[0].0, "interaction_value_ignored");

        // The caller's fields change mid-session; the identity token does not.
        let mut cfg = defaults();
        let patch = json!({ "front": { "caller": { "verbosity": "detailed", "pace": "slower",
                                                   "preferred_name": null,
                                                   "identity_token": "v1.x.y" } } });
        let (applied, warnings) = apply_update(&mut cfg, &patch);
        assert_eq!(
            (cfg.front.verbosity, cfg.front.pace),
            (Some(Verbosity::Detailed), Some(Pace::Slower))
        );
        assert!(
            applied.contains(&"front.caller.verbosity".to_owned()),
            "{applied:?}"
        );
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_override_refused",
                "interaction.front.caller.identity_token".to_owned()
            )]
        );
    }

    #[test]
    fn a_tools_own_read_back_must_name_its_arguments_and_not_sound_like_an_answer() {
        let tool = |template: &str| {
            json!({ "tasks": { "tools": [{
                "name": "book_table", "url": "https://tools.example.com/book",
                "side_effects": true, "confirm_template": template,
                "parameters": { "type": "object", "properties": {
                    "party": { "type": "integer" }, "time": { "type": "string" } } }
            }] } })
        };
        let operator = ParseCtx {
            operator: true,
            ..ParseCtx::default()
        };
        let (cfg, warnings) = parse(&tool("book a table for {party} at {time}"), &on(), operator);
        assert_eq!(warnings, Vec::new());
        assert_eq!(
            cfg.unwrap().tasks.tools[0].confirm_template.as_deref(),
            Some("book a table for {party} at {time}")
        );
        for bad in [
            "book a table for {guests}",
            "okay, book a table for {party}",
        ] {
            let (cfg, warnings) = parse(&tool(bad), &on(), operator);
            assert_eq!(cfg.unwrap().tasks.tools[0].confirm_template, None, "{bad}");
            assert_eq!(
                paths(&warnings),
                [(
                    "interaction_value_ignored",
                    "interaction.tasks.tools[0].confirm_template".to_owned()
                )]
            );
        }
    }

    #[test]
    fn the_published_schemas_paths_set_the_same_fields() {
        let raw = json!({
            "front": { "caller": { "preferred_name": "Sam", "pace": "slower" } },
            "policy": { "progress_first_after_ms": 3000 },
            "duplex": {
                "barge_in": { "duck_db": -20, "pause_after_ms": 500, "mode": "legacy" },
                "endpoint": { "eager": { "enabled": "off" }, "smart_turn": true }
            },
        });
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        let cfg = cfg.unwrap();
        assert_eq!(cfg.front.preferred_name.as_deref(), Some("Sam"));
        assert_eq!(cfg.front.pace, Some(Pace::Slower));
        assert_eq!(cfg.policy.progress_min_wait_ms, 3000);
        let duplex = &cfg.duplex;
        assert_eq!(
            (duplex.duck_db, duplex.pause_after_ms, duplex.eager),
            (-20.0, 500, false)
        );
        assert!(duplex.barge_in, "the object form leaves barge-in on");
        // What this build does not have is still named.
        let unknown = |path: &str| ("unknown_config_keys", path.to_owned());
        assert_eq!(
            paths(&warnings),
            [
                unknown("interaction.duplex.barge_in.mode"),
                unknown("interaction.duplex.endpoint.smart_turn"),
            ]
        );
    }

    #[test]
    fn an_orchestrator_names_its_agents_and_limits_only_ever_lower() {
        let listed = json!([
            { "name": "browser", "type": "a2a", "url": "https://browser.example.com/a2a",
              "description": "Operates websites in a browser.", "max_concurrent": 2,
              "question_deadline_ms": 5000, "fallback": "backup", "timeout_s": 120 },
            { "name": "backup", "type": "webhook", "url": "https://backup.example.com/hook" },
            // A fallback that is not in the list is dropped with a finding; the entry stays.
            { "name": "coder", "type": "webhook", "url": "https://coder.example.com/hook",
              "fallback": "nobody" },
            { "name": "zero", "type": "webhook", "url": "https://zero.example.com/hook",
              "max_concurrent": 0 },
        ]);
        let env = std::collections::HashMap::from([
            ("WAAV_INTERACTION_ENABLED", "1".to_owned()),
            ("WAAV_INTERACTION_TASK_BACKENDS", listed.to_string()),
            ("WAAV_INTERACTION_ALLOW_CLIENT_BACKENDS", "true".to_owned()),
        ]);
        let (mut server, problems) = load_server(None, |key| env.get(key).cloned());
        assert_eq!(problems.len(), 2, "{problems:?}");
        let names: Vec<&str> = server
            .task_backends
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(names, ["browser", "backup", "coder"]);
        let browser = &server.task_backends[0];
        assert_eq!(
            (
                browser.max_concurrent,
                browser.question_deadline_ms,
                browser.timeout_s
            ),
            (Some(2), Some(5_000), Some(120))
        );
        assert_eq!(browser.fallback.as_deref(), Some("backup"));
        assert_eq!(server.task_backends[2].fallback, None);

        let block = json!({ "tasks": {
            "orchestrator": "gpt-oss-20b", "agents": ["browser", "coder"],
            "limits": { "max_live": 4, "max_live_tenant": 9 },
        } });
        let (cfg, warnings) = parse(&block, &server, ParseCtx::default());
        let tasks = cfg.expect("on").tasks;
        assert_eq!(tasks.orchestrator, Some(json!("gpt-oss-20b")));
        assert_eq!(tasks.agents, ["browser", "coder"]);
        // A session does not set its tenant's limit.
        assert_eq!(tasks.limits.max_live, Some(4));
        assert_eq!(tasks.limits.max_live_tenant, None);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code, "interaction_override_refused");

        // An unknown agent, and a raise above the operator's limit, are not taken.
        server.default =
            Some(json!({ "tasks": { "limits": { "max_live": 2, "max_live_tenant": 6 } } }));
        let block = json!({ "tasks": {
            "orchestrator": { "base_url": "http://127.0.0.1:9", "model": "big" },
            "agents": ["browser", "mailer"], "limits": { "max_live": 3 },
        } });
        let (cfg, warnings) = parse(&block, &server, ParseCtx::default());
        let tasks = cfg.expect("on").tasks;
        assert!(tasks.orchestrator.is_some_and(|o| o["model"] == "big"));
        assert!(tasks.agents.is_empty());
        assert_eq!(
            tasks.limits,
            TaskLimits {
                max_live: Some(2),
                max_live_tenant: Some(6)
            }
        );
        let paths: Vec<&Value> = warnings.iter().map(|w| &w.detail["path"]).collect();
        assert_eq!(
            paths,
            [
                &json!("interaction.tasks.agents"),
                &json!("interaction.tasks.limits.max_live")
            ]
        );
        // Lowering is a session's to do.
        let lower = json!({ "tasks": { "limits": { "max_live": 1 } } });
        let (cfg, warnings) = parse(&lower, &server, ParseCtx::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.expect("on").tasks.limits.max_live, Some(1));
        // Without a switch a session does not name its agents.
        server.allow_client_backends = false;
        let (cfg, warnings) = parse(&block, &server, ParseCtx::default());
        assert!(cfg.expect("on").tasks.agents.is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.code == "interaction_override_refused")
        );
    }

    #[test]
    fn an_mcp_entry_may_sign_in_with_oauth_and_an_a2a_entry_take_pushes() {
        let listed = json!([
            { "name": "notes", "type": "mcp", "url": "https://mcp.example.com/notes", "tool": "t",
              "oauth": { "token_url": "https://auth.example.com/token", "client_id": "waav",
                         "client_secret_env": "NOTES_CLIENT_SECRET", "scope": "notes.read" } },
            { "name": "agent", "type": "a2a", "url": "https://agent.example.com/a2a",
              "push_base_url": "https://gateway.example.com" },
            // Unusable: OAuth off MCP, OAuth with a secret too, a token URL that is no web
            // address, pushes off A2A.
            { "name": "hook", "type": "webhook", "url": "https://h.example.com",
              "oauth": { "token_url": "https://a.example.com", "client_id": "c",
                         "client_secret_env": "S" } },
            { "name": "both", "type": "mcp", "url": "https://m.example.com", "tool": "t",
              "secret_env": "S", "oauth": { "token_url": "https://a.example.com",
                                             "client_id": "c", "client_secret_env": "S" } },
            { "name": "ftp", "type": "mcp", "url": "https://m.example.com", "tool": "t",
              "oauth": { "token_url": "ftp://a.example.com", "client_id": "c",
                         "client_secret_env": "S" } },
            { "name": "pushy", "type": "mcp", "url": "https://m.example.com", "tool": "t",
              "push_base_url": "https://gateway.example.com" },
        ]);
        let env = std::collections::HashMap::from([
            ("WAAV_INTERACTION_ENABLED", "1".to_owned()),
            ("WAAV_INTERACTION_TASK_BACKENDS", listed.to_string()),
            ("NOTES_CLIENT_SECRET", "client-secret-value".to_owned()),
        ]);
        let (server, problems) = load_server(None, |key| env.get(key).cloned());
        let names: Vec<&str> = server
            .task_backends
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(names, ["notes", "agent"], "{problems:?}");
        assert_eq!(problems.len(), 4, "{problems:?}");
        let oauth = server.task_backends[0].oauth.as_ref().unwrap();
        assert_eq!(oauth.client_secret.as_deref(), Some("client-secret-value"));
        assert_eq!(oauth.scope.as_deref(), Some("notes.read"));
        assert_eq!(
            server.task_backends[1].push_base_url.as_deref(),
            Some("https://gateway.example.com")
        );
        assert!(!format!("{server:?}").contains("client-secret-value"));
        // A client secret that is not in the environment is a finding; the entry stays.
        let (server, problems) = load_server(None, |key| {
            (key != "NOTES_CLIENT_SECRET")
                .then(|| env.get(key).cloned())
                .flatten()
        });
        assert_eq!(server.task_backends.len(), 2);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("NOTES_CLIENT_SECRET is not set"))
        );
    }

    #[test]
    fn the_catalog_is_the_operators_and_a_session_only_names_an_entry() {
        let listed = json!([
            { "name": "crm", "type": "webhook", "url": "https://crm.example.com/waav",
              "secret_env": "CRM_SECRET" },
            { "name": "agent", "type": "a2a", "url": "https://agent.example.com/a2a",
              "bearer_env": "AGENT_TOKEN", "side_effects": "none" },
            { "name": "research", "type": "openai_responses", "url": "https://api.example.com/v1",
              "model": "big", "api_key_env": "MISSING_KEY", "mode": "poll" },
            // Unusable: no tool, a name used twice, no model, not an http address.
            { "name": "stock", "type": "mcp", "url": "https://mcp.example.com/mcp" },
            { "name": "crm", "type": "mcp", "url": "https://mcp.example.com/mcp", "tool": "t" },
            { "name": "model", "type": "openai_responses", "url": "https://api.example.com/v1" },
            { "name": "ftp", "type": "webhook", "url": "ftp://crm.example.com" },
            // An orchestrator's MCP server: its tools, not one `tool`, and confirmations.
            { "name": "notes", "type": "mcp", "url": "https://mcp.example.com/notes",
              "tools": ["*"], "catalog_ttl_s": 60, "confirm": { "delete_note": true } },
            // Unusable: `tools` on an A2A agent, a TTL of 0, an empty tool name.
            { "name": "card", "type": "a2a", "url": "https://agent.example.com/", "tools": ["*"] },
            { "name": "ttl", "type": "a2a", "url": "https://agent.example.com/", "catalog_ttl_s": 0 },
            { "name": "blank", "type": "mcp", "url": "https://mcp.example.com/", "tools": [""] },
        ]);
        let env = std::collections::HashMap::from([
            ("WAAV_INTERACTION_ENABLED", "1".to_owned()),
            ("WAAV_INTERACTION_TASK_BACKENDS", listed.to_string()),
            ("CRM_SECRET", "hook-secret".to_owned()),
            ("AGENT_TOKEN", "agent-token".to_owned()),
        ]);
        let (mut server, problems) = load_server(None, |key| env.get(key).cloned());
        let names: Vec<&str> = server
            .task_backends
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(names, ["crm", "agent", "research", "notes"]);
        assert_eq!(problems.len(), 8, "{problems:?}");
        let notes = &server.task_backends[3];
        assert_eq!(
            (notes.tools.as_slice(), notes.catalog_ttl_s),
            (&["*".to_owned()][..], Some(60))
        );
        let named = BTreeMap::from([("delete_note".to_owned(), true)]);
        assert_eq!(notes.confirm, Some(Confirm::Tools(named)));
        server.task_backends.pop();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("MISSING_KEY is not set"))
        );
        let crm = &server.task_backends[0];
        assert_eq!(
            (crm.label().as_str(), crm.secret.as_deref()),
            ("webhook:crm", Some("hook-secret"))
        );
        assert_eq!(crm.side_effects, SideEffects::Possible);
        let agent = &server.task_backends[1];
        assert_eq!(
            (agent.side_effects, agent.secret.as_deref()),
            (SideEffects::None, Some("agent-token"))
        );
        assert_eq!(server.task_backends[2].mode, Some(BackendMode::Poll));
        // The secret is not in the debug output.
        assert!(!format!("{server:?}").contains("hook-secret"));

        // The same list in the server's YAML file.
        let yaml = "interaction:\n  enabled: true\n  allow_client_backends: true\n  task_backends:\n    \
                    - name: stock\n      type: mcp\n      url: https://mcp.example.com/mcp\n      \
                    tool: find_item\n      argument: question\n      side_effects: none\n";
        let (file, problems) = load_server(Some(Ok(yaml.to_owned())), |_| None);
        assert!(problems.is_empty(), "{problems:?}");
        let stock = &file.task_backends[0];
        assert_eq!(
            (stock.kind, stock.tool.as_deref()),
            (BackendKind::Mcp, Some("find_item"))
        );
        assert!(file.allow_client_backends);

        // A session does not name its backend unless the operator lets sessions choose.
        let named = json!({ "tasks": { "default_backend": "crm" } });
        let (cfg, warnings) = parse(&named, &server, ParseCtx::default());
        assert_eq!(cfg.unwrap().tasks.backend, None);
        assert_eq!(warnings[0].code, "interaction_override_refused");
        // An alias, an agent entry and the server's own blocks always may.
        let operator = ParseCtx {
            operator: true,
            ..ParseCtx::default()
        };
        let (cfg, warnings) = parse(&named, &server, operator);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.unwrap().tasks.backend.as_deref(), Some("crm"));
        server.default = Some(named.clone());
        let (cfg, _) = parse(&json!(true), &server, ParseCtx::default());
        assert_eq!(cfg.unwrap().tasks.backend.as_deref(), Some("crm"));

        // With the switch a session picks any entry by name, and never under Bud.
        server.allow_client_backends = true;
        let other = json!({ "tasks": { "default_backend": "agent" } });
        let (cfg, warnings) = parse(&other, &server, ParseCtx::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.unwrap().tasks.backend.as_deref(), Some("agent"));
        let bud = ParseCtx {
            bud_mode: true,
            ..ParseCtx::default()
        };
        let (cfg, warnings) = parse(&other, &server, bud);
        assert_eq!(cfg.unwrap().tasks.backend.as_deref(), Some("crm"));
        assert_eq!(warnings[0].code, "interaction_override_refused");
        // A name the catalog does not have keeps the operator's choice.
        for unknown in [json!("stock"), json!("https://evil.example.com/"), json!(7)] {
            let sent = json!({ "tasks": { "default_backend": unknown } });
            let (cfg, warnings) = parse(&sent, &server, ParseCtx::default());
            assert_eq!(cfg.unwrap().tasks.backend.as_deref(), Some("crm"));
            assert_eq!(warnings[0].code, "interaction_value_ignored");
        }
    }

    /// Chapter 10: a session may set the stall and exit times; the lines' words and the
    /// follow-up are the operator's, and an operator's line is checked like a pack's.
    #[test]
    fn failure_settings_and_who_may_set_them() {
        let (cfg, warnings) = parse(&json!(true), &on(), ParseCtx::default());
        assert!(warnings.is_empty());
        assert_eq!(cfg.unwrap().failure, FailureConfig::default());
        let operator = json!({ "failure": {
            "follow_up": true,
            "lines": {
                "exit": "I'm sorry, our systems are down. Please call us back later.",
                "back": "All done, it's booked.",
            },
        } });
        let server = ServerInteraction {
            default: Some(operator),
            ..on()
        };
        let session = json!({ "failure": {
            "stall_after_ms": 4_000, "max_wait_s": 3, "follow_up": false, "lines": { "back": "Hi." },
        } });
        let (cfg, warnings) = parse(&session, &server, ParseCtx::default());
        let failure = cfg.unwrap().failure;
        assert_eq!((failure.stall_after_ms, failure.max_wait_s), (4_000, 30));
        assert!(failure.follow_up, "the operator's");
        assert_eq!(
            failure.lines.len(),
            1,
            "the operator's line that passes: {:?}",
            failure.lines
        );
        assert!(failure.lines["exit"].starts_with("I'm sorry"));
        let mut paths: Vec<(&str, &str)> = warnings
            .iter()
            .map(|w| (w.code, w.detail["path"].as_str().unwrap_or_default()))
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                (
                    "interaction_override_refused",
                    "interaction.failure.follow_up"
                ),
                ("interaction_override_refused", "interaction.failure.lines"),
                (
                    "interaction_value_ignored",
                    "interaction.failure.max_wait_s"
                ),
            ]
        );
        // The operator's line that claims a booking was found when the server's block was read.
        let mut base = Walk::default();
        let mut cfg = InteractionConfig::default();
        base.block(server.default.as_ref().unwrap(), &mut cfg, true, true, true);
        assert_eq!(
            base.warnings[0].detail["path"],
            "interaction.failure.lines.back"
        );
        // Not a mid-session setting.
        let (applied, warnings) =
            apply_update(&mut cfg, &json!({ "failure": { "max_wait_s": 60 } }));
        assert!(applied.is_empty());
        assert_eq!(warnings[0].detail["reason"], "not_mid_session");
    }

    #[test]
    fn a_session_pushes_to_its_own_address_with_its_own_secret_only_where_the_operator_allows() {
        let operator_block = json!({ "tasks": { "push_url": "https://hooks.example.com/tasks" } });
        let server = |allow: bool| ServerInteraction {
            default: Some(operator_block.clone()),
            allow_client_push: allow,
            ..on()
        };
        let own = json!({ "tasks": { "push_url": "https://client.example.com/pushes",
                                     "push_secret": "the-clients-own-secret" } });
        // Allowed: the session's pair replaces the operator's.
        let (cfg, warnings) = parse(&own, &server(true), ParseCtx::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        let tasks = cfg.expect("on").tasks;
        assert_eq!(
            tasks.push_url.as_deref(),
            Some("https://client.example.com/pushes")
        );
        assert_eq!(tasks.push_secret.as_deref(), Some("the-clients-own-secret"));
        // Not allowed, or in Bud mode: both refused, the operator's address stays.
        let bud = ParseCtx {
            bud_mode: true,
            ..ParseCtx::default()
        };
        for (server, ctx) in [(server(false), ParseCtx::default()), (server(true), bud)] {
            let (cfg, warnings) = parse(&own, &server, ctx);
            let tasks = cfg.expect("on").tasks;
            assert_eq!(
                tasks.push_url.as_deref(),
                Some("https://hooks.example.com/tasks")
            );
            assert_eq!(tasks.push_secret, None);
            let codes: Vec<_> = warnings.iter().map(|w| w.code).collect();
            assert_eq!(codes, ["interaction_override_refused"; 2]);
        }
        // One without the other: a client's address is never signed with the server's secret,
        // nor the operator's with a client's.
        for half in [
            json!({ "tasks": { "push_url": "https://client.example.com/pushes" } }),
            json!({ "tasks": { "push_secret": "the-clients-own-secret" } }),
        ] {
            let (cfg, warnings) = parse(&half, &server(true), ParseCtx::default());
            let tasks = cfg.expect("on").tasks;
            assert_eq!(
                tasks.push_url.as_deref(),
                Some("https://hooks.example.com/tasks")
            );
            assert_eq!(tasks.push_secret, None);
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert_eq!(warnings[0].code, "interaction_value_ignored");
        }
        // A short secret is no secret; the switch is read from the environment too.
        let short =
            json!({ "tasks": { "push_url": "https://c.example.com/", "push_secret": "short" } });
        let (cfg, warnings) = parse(&short, &server(true), ParseCtx::default());
        assert_eq!(cfg.expect("on").tasks.push_secret, None);
        assert!(
            warnings
                .iter()
                .any(|w| w.detail["path"] == "interaction.tasks.push_secret")
        );
        let env = ServerInteraction::from_lookup(|key| {
            (key == "WAAV_INTERACTION_ALLOW_CLIENT_PUSH").then(|| "true".to_owned())
        });
        assert!(env.allow_client_push);
    }

    #[test]
    fn a_session_lists_the_backends_its_tasks_may_run_on() {
        let entry = |name: &str| {
            serde_json::from_value::<BackendSpec>(json!({ "name": name, "type": "webhook",
                "url": format!("https://{name}.example.com/") }))
            .expect("an entry")
        };
        let server = |allow: bool| ServerInteraction {
            task_backends: vec![entry("crm"), entry("trips")],
            allow_client_backends: allow,
            ..on()
        };
        let listed = json!({ "tasks": { "backends": ["crm", "trips"] } });
        let (cfg, warnings) = parse(&listed, &server(true), ParseCtx::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.expect("on").tasks.backends, ["crm", "trips"]);
        // Only names the catalog has; only where the operator lets a session name them.
        let unknown = json!({ "tasks": { "backends": ["crm", "nope"] } });
        let (cfg, warnings) = parse(&unknown, &server(true), ParseCtx::default());
        assert!(cfg.expect("on").tasks.backends.is_empty());
        assert_eq!(warnings[0].code, "interaction_value_ignored");
        let (cfg, warnings) = parse(&listed, &server(false), ParseCtx::default());
        assert!(cfg.expect("on").tasks.backends.is_empty());
        assert_eq!(warnings[0].code, "interaction_override_refused");
    }

    #[test]
    fn only_the_operator_says_where_a_result_is_pushed() {
        let raw = json!({ "tasks": {
            "push_url": " https://hooks.example.com/tasks ",
            "reconnect_grace_s": 20,
        } });
        let pushed = Some("https://hooks.example.com/tasks".to_owned());
        // The server's default block, with a session that sends none or one of its own.
        let server = ServerInteraction {
            default: Some(raw.clone()),
            ..on()
        };
        for sent in [
            raw.clone(),
            json!(true),
            json!({ "policy": { "ack_due_ms": 250 } }),
        ] {
            let (cfg, warnings) = parse(&sent, &server, ParseCtx::default());
            assert!(warnings.is_empty(), "{warnings:?}");
            let tasks = cfg.unwrap().tasks;
            assert_eq!(
                (tasks.push_url, tasks.reconnect_grace_s),
                (pushed.clone(), 20)
            );
        }
        // An alias definition is the operator's too.
        let operator = ParseCtx {
            operator: true,
            ..ParseCtx::default()
        };
        let (cfg, warnings) = parse(&raw, &on(), operator);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.unwrap().tasks.push_url, pushed);

        // A session's own address is refused, and the operator's stays; its grace is taken.
        let own =
            json!({ "tasks": { "push_url": "https://evil.example.com/", "reconnect_grace_s": 0 } });
        let (cfg, warnings) = parse(&own, &server, ParseCtx::default());
        let tasks = cfg.unwrap().tasks;
        assert_eq!((tasks.push_url, tasks.reconnect_grace_s), (pushed, 0));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "interaction_override_refused");
        assert_eq!(warnings[0].detail["path"], "interaction.tasks.push_url");

        // The defaults, and values that are not usable.
        let (cfg, _) = parse(&json!(true), &on(), ParseCtx::default());
        let tasks = cfg.unwrap().tasks;
        assert_eq!((tasks.push_url, tasks.reconnect_grace_s), (None, 60));
        let bad =
            json!({ "tasks": { "push_url": "ftp://hooks.example.com", "reconnect_grace_s": 601 } });
        let (cfg, warnings) = parse(&bad, &on(), operator);
        let tasks = cfg.unwrap().tasks;
        assert_eq!((tasks.push_url, tasks.reconnect_grace_s), (None, 60));
        let codes: Vec<_> = warnings.iter().map(|w| w.code).collect();
        assert_eq!(
            codes,
            ["interaction_value_ignored", "interaction_value_ignored"]
        );
        // Neither changes during a session.
        let mut cfg = InteractionConfig::default();
        let (applied, refused) =
            apply_update(&mut cfg, &json!({ "tasks": { "reconnect_grace_s": 5 } }));
        assert!(applied.is_empty());
        assert_eq!(refused[0].detail["reason"], "not_mid_session");
    }

    #[test]
    fn a_session_says_how_task_updates_reach_its_caller() {
        assert_eq!(InteractionConfig::default().tasks.announce, Announce::Ask);
        for (value, announce) in [("tell", Announce::Tell), ("screen", Announce::Screen)] {
            let raw = json!({ "tasks": { "announce": value } });
            let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
            assert!(warnings.is_empty(), "{warnings:?}");
            let cfg = cfg.unwrap();
            assert_eq!(cfg.tasks.announce, announce);
            assert_eq!(echo(&cfg, "llm", &[])["tasks"]["announce"], value);
        }
        let (cfg, warnings) = parse(
            &json!({ "tasks": { "announce": "shout" } }),
            &on(),
            ParseCtx::default(),
        );
        assert_eq!(cfg.unwrap().tasks.announce, Announce::Ask);
        assert_eq!(warnings[0].code, "interaction_value_ignored");

        // It may change during the session; nothing else of `tasks` may.
        let mut cfg = InteractionConfig::default();
        let patch = json!({ "tasks": { "announce": "screen", "default_backend": "crm" } });
        let (applied, refused) = apply_update(&mut cfg, &patch);
        assert_eq!(applied, ["tasks.announce"]);
        assert_eq!(cfg.tasks.announce, Announce::Screen);
        assert_eq!(
            paths(&refused),
            [(
                "interaction_override_refused",
                "interaction.tasks.default_backend".to_owned()
            )]
        );
        let (applied, refused) =
            apply_update(&mut cfg, &json!({ "tasks": { "announce": "shout" } }));
        assert!(applied.is_empty());
        assert_eq!(cfg.tasks.announce, Announce::Screen);
        assert_eq!(refused[0].code, "interaction_value_ignored");
    }

    #[test]
    fn follow_ups_last_their_ttl_and_a_resume_key_needs_the_operator() {
        assert_eq!(InteractionConfig::default().tasks.follow_up_ttl_s, 1800);
        let raw =
            json!({ "tasks": { "follow_up_ttl_s": 600, "resume_key": "caller-42:device-a" } });
        // Without the operator's leave the key is refused; the TTL is the session's.
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        let cfg = cfg.unwrap();
        assert_eq!(cfg.tasks.follow_up_ttl_s, 600);
        assert_eq!(cfg.tasks.resume_key, None);
        assert_eq!(
            (warnings[0].code, &warnings[0].detail["path"]),
            (
                "interaction_override_refused",
                &json!("interaction.tasks.resume_key")
            )
        );
        let mut server = on();
        server.allow_resume_key = true;
        let (cfg, warnings) = parse(&raw, &server, ParseCtx::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        let cfg = cfg.unwrap();
        assert_eq!(cfg.tasks.resume_key.as_deref(), Some("caller-42:device-a"));
        // Never echoed: only that there is one.
        let echoed = echo(&cfg, "llm", &[]);
        assert_eq!(echoed["tasks"]["resume"], true);
        assert!(!echoed.to_string().contains("caller-42"));
        // A key too short, or with other characters, is not taken.
        for bad in [json!("short"), json!("has space in it"), json!(42)] {
            let raw = json!({ "tasks": { "resume_key": bad, "follow_up_ttl_s": 90_000 } });
            let (cfg, warnings) = parse(&raw, &server, ParseCtx::default());
            let cfg = cfg.unwrap();
            assert_eq!(
                (cfg.tasks.resume_key, cfg.tasks.follow_up_ttl_s),
                (None, 1800)
            );
            assert_eq!(warnings.len(), 2, "{warnings:?}");
        }
        let env = ServerInteraction::from_lookup(|key| {
            (key == "WAAV_INTERACTION_ALLOW_RESUME_KEY").then(|| "true".to_owned())
        });
        assert!(env.allow_resume_key);
    }

    #[test]
    fn only_the_operator_says_which_tasks_run_on_after_the_call() {
        let defaults = InteractionConfig::default().tasks;
        assert_eq!(
            (defaults.session_end, defaults.orphan_max_s),
            (SessionEnd::Auto, 1800)
        );
        let raw = json!({ "tasks": { "session_end": "continue_all", "orphan_max_s": 120 } });
        let operator = ParseCtx {
            operator: true,
            ..ParseCtx::default()
        };
        let (cfg, warnings) = parse(&raw, &on(), operator);
        assert!(warnings.is_empty(), "{warnings:?}");
        let tasks = cfg.unwrap().tasks;
        assert_eq!(
            (tasks.session_end, tasks.orphan_max_s),
            (SessionEnd::ContinueAll, 120)
        );
        // A session's own is refused: the runs after the call are the operator's to pay for.
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        let tasks = cfg.unwrap().tasks;
        assert_eq!(
            (tasks.session_end, tasks.orphan_max_s),
            (SessionEnd::Auto, 1800)
        );
        let codes: Vec<_> = warnings.iter().map(|w| w.code).collect();
        assert_eq!(codes, ["interaction_override_refused"; 2]);
        // Values that are not usable keep the defaults.
        let bad = json!({ "tasks": { "session_end": "forever", "orphan_max_s": 86_401 } });
        let (cfg, warnings) = parse(&bad, &on(), operator);
        let tasks = cfg.unwrap().tasks;
        assert_eq!(
            (tasks.session_end, tasks.orphan_max_s),
            (SessionEnd::Auto, 1800)
        );
        assert_eq!(warnings.len(), 2);
        let echoed = echo(&InteractionConfig::default(), "llm", &[]);
        assert_eq!(echoed["tasks"]["session_end"], "auto");
        assert_eq!(echoed["tasks"]["orphan_max_s"], 1800);
    }

    #[test]
    fn only_the_agent_or_the_operator_declares_an_answer_marker() {
        let raw = json!({ "background": { "answer_marker": " FINAL: ", "resume": true } });
        let operator = ParseCtx {
            operator: true,
            bud_mode: true,
        };
        let (cfg, warnings) = parse(&raw, &on(), operator);
        assert_eq!(cfg.unwrap().tasks.answer_marker.as_deref(), Some("FINAL:"));
        assert_eq!(
            paths(&warnings),
            [(
                "unknown_config_keys",
                "interaction.background.resume".to_owned()
            )]
        );
        // The server's own blocks are the operator's too.
        let server = ServerInteraction {
            default: Some(raw.clone()),
            ..on()
        };
        let (cfg, _) = parse(&json!(true), &server, ParseCtx::default());
        assert_eq!(cfg.unwrap().tasks.answer_marker.as_deref(), Some("FINAL:"));

        // A session's own block cannot: the marker says which text of the agent is spoken.
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(cfg.unwrap().tasks.answer_marker, None);
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_override_refused",
                "interaction.background".to_owned()
            )]
        );
        for bad in [json!(""), json!(7), json!("x".repeat(41))] {
            let raw = json!({ "background": { "answer_marker": bad } });
            let (cfg, warnings) = parse(&raw, &on(), operator);
            assert_eq!(cfg.unwrap().tasks.answer_marker, None);
            assert_eq!(warnings[0].code, "interaction_value_ignored");
        }
    }

    #[test]
    fn the_agent_default_is_read_from_the_file_and_the_environment() {
        let yaml = "interaction:\n  enabled: true\n  agent_default:\n    background:\n      answer_marker: \"ANSWER:\"\n";
        let (server, problems) = load_server(Some(Ok(yaml.to_owned())), |_| None);
        assert_eq!(problems, Vec::<String>::new());
        let block = json!({ "background": { "answer_marker": "ANSWER:" } });
        assert_eq!(server.agent_default, Some(block));
        assert_eq!(
            server.default, None,
            "it is not the block of other sessions"
        );

        let env = |key: &str| (key == "WAAV_INTERACTION_AGENT_DEFAULT").then(|| "true".to_owned());
        let (server, problems) = load_server(Some(Ok(yaml.to_owned())), env);
        assert!(problems.is_empty());
        assert_eq!(server.agent_default, Some(json!(true)));

        // A value the block cannot hold is found at startup, by name.
        let env = |key: &str| {
            let block = r#"{"policy": {"ack_due_ms": 9000}}"#;
            (key == "WAAV_INTERACTION_AGENT_DEFAULT").then(|| block.to_owned())
        };
        let (_, problems) = load_server(None, env);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("agent_default block:"),
            "{problems:?}"
        );
        let env = |key: &str| (key == "WAAV_INTERACTION_AGENT_DEFAULT").then(|| "[1]".to_owned());
        let (server, problems) = load_server(None, env);
        assert_eq!(server.agent_default, None);
        assert_eq!(
            problems,
            ["WAAV_INTERACTION_AGENT_DEFAULT must be JSON: true or an object"]
        );
    }

    #[test]
    fn server_only_keys_from_a_session_are_refused() {
        let raw = json!({
            "front_model": { "url": "http://attacker.example.com" },
            "tool_secret": "mine",
            "allow_client_tools": true,
            "policy": { "ack_due_ms": 250 }
        });
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(
            paths(&warnings),
            [
                (
                    "interaction_override_refused",
                    "interaction.allow_client_tools".to_owned()
                ),
                (
                    "interaction_override_refused",
                    "interaction.front_model".to_owned()
                ),
                (
                    "interaction_override_refused",
                    "interaction.tool_secret".to_owned()
                ),
            ]
        );
        assert_eq!(cfg.unwrap().policy.ack_due_ms, 250);
    }

    #[test]
    fn client_tools_need_the_server_switch_and_never_work_in_bud_mode() {
        let raw = json!({ "tasks": { "tools": [tool("lookup")], "speculative_start": "on" } });
        let refused = [(
            "interaction_override_refused",
            "interaction.tasks.tools".to_owned(),
        )];

        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(paths(&warnings), refused);
        let tasks = cfg.unwrap().tasks;
        assert!(tasks.tools.is_empty());
        assert_eq!(tasks.speculative_start, Tri::On);

        let allowing = ServerInteraction {
            allow_client_tools: true,
            ..on()
        };
        let (cfg, warnings) = parse(&raw, &allowing, ParseCtx::default());
        assert!(warnings.is_empty());
        assert_eq!(cfg.unwrap().tasks.tools.len(), 1);

        let (cfg, warnings) = parse(
            &raw,
            &allowing,
            ParseCtx {
                bud_mode: true,
                ..ParseCtx::default()
            },
        );
        assert_eq!(paths(&warnings), refused);
        assert!(cfg.unwrap().tasks.tools.is_empty());
    }

    #[test]
    fn the_server_default_is_the_base_of_every_session() {
        let default = json!({
            "tasks": { "tools": [tool("lookup")] },
            "policy": { "ack_due_ms": 250, "fast_answer_ms": 650 }
        });
        let server = ServerInteraction {
            default: Some(default.clone()),
            ..on()
        };
        // A session that sent no block gets the server's own, tools included.
        let (cfg, warnings) = parse(
            &default,
            &server,
            ParseCtx {
                bud_mode: true,
                ..ParseCtx::default()
            },
        );
        assert!(warnings.is_empty());
        let cfg = cfg.unwrap();
        assert_eq!(cfg.tasks.tools.len(), 1);
        assert_eq!(cfg.policy.ack_due_ms, 250);

        // `true` keeps the base; an object overrides field by field and cannot add tools.
        let (cfg, _) = parse(&json!(true), &server, ParseCtx::default());
        assert_eq!(cfg.unwrap().policy.fast_answer_ms, 650);
        let raw = json!({ "policy": { "ack_due_ms": 100 }, "tasks": { "tools": [tool("mine")] } });
        let (cfg, warnings) = parse(&raw, &server, ParseCtx::default());
        let cfg = cfg.unwrap();
        assert_eq!(
            (cfg.policy.ack_due_ms, cfg.policy.fast_answer_ms),
            (100, 650)
        );
        assert_eq!(cfg.tasks.tools[0].name, "lookup");
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_override_refused",
                "interaction.tasks.tools".to_owned()
            )]
        );

        // The same block from an alias definition is the operator's: its tools are taken,
        // and signed, over the default block's values.
        let operator = ParseCtx {
            operator: true,
            ..ParseCtx::default()
        };
        let (cfg, warnings) = parse(&raw, &server, operator);
        let cfg = cfg.unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.policy.fast_answer_ms, 650);
        let tools: Vec<(&str, bool)> = cfg
            .tasks
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool.signed))
            .collect();
        assert_eq!(tools, [("mine", true)]);
    }

    #[test]
    fn the_front_is_always_the_model_and_scripted_is_refused() {
        let profile = |raw: Value, server: &ServerInteraction| {
            let (cfg, warnings) = parse(&raw, server, ParseCtx::default());
            (cfg.unwrap().front.profile, paths(&warnings))
        };
        let model = json!({ "front": { "profile": "model" } });
        let scripted = json!({ "front": { "profile": "scripted" } });
        assert_eq!(profile(json!(true), &on()), (FrontProfile::Model, vec![]));
        assert_eq!(
            profile(json!(true), &with_front_model()),
            (FrontProfile::Model, vec![])
        );
        assert_eq!(
            profile(scripted, &with_front_model()),
            (
                FrontProfile::Model,
                vec![(
                    "front_scripted_refused",
                    "interaction.front.profile".to_owned()
                )]
            )
        );
        assert_eq!(profile(model, &on()), (FrontProfile::Model, vec![]));
    }

    #[test]
    fn the_operator_bounds_the_controls_and_a_session_cannot_widen_them() {
        let server = ServerInteraction {
            default: Some(json!({ "controls": {
                "allowed": ["stop", "rate", "language"], "rate": [0.8, 1.2],
                "languages": ["es", "zh_cn"], "answers_only": true, "from_agents": false,
                "human": { "transfer_to": "+15551230000", "url": "https://ops.example.com/h",
                           "wait_s": 45 }
            }})),
            ..on()
        };
        let (cfg, warnings) = parse(&json!(true), &server, ParseCtx::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        let cfg = cfg.expect("on");
        // Where the caller is put through is the operator's: the echo says only that it is set.
        assert_eq!(echo(&cfg, "llm", &[])["controls"]["human"], json!(true));
        let controls = cfg.controls;
        assert_eq!(controls.human.transfer_to.as_deref(), Some("+15551230000"));
        assert_eq!(
            controls.human.url.as_deref(),
            Some("https://ops.example.com/h")
        );
        assert_eq!(controls.human.wait_s(), 45);
        assert_eq!(super::super::controls::HumanConfig::default().wait_s(), 60);
        assert_eq!(controls.allowed.len(), 3);
        assert_eq!((controls.rate_min, controls.rate_max), (0.8, 1.2));
        assert_eq!(controls.languages, ["es", "zh-CN"]);
        assert!(controls.answers_only && !controls.from_agents && controls.from_clients);
        let widen = json!({ "controls": { "allowed": ["human"] } });
        let (cfg, warnings) = parse(&widen, &server, ParseCtx::default());
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_override_refused",
                "interaction.controls".to_owned()
            )]
        );
        assert_eq!(cfg.expect("on").controls.allowed.len(), 3);
    }

    #[test]
    fn the_switch_off_gives_interaction_disabled() {
        for raw in [json!(true), json!({ "policy": { "ack_due_ms": 9999 } })] {
            let (cfg, warnings) = parse(&raw, &ServerInteraction::default(), ParseCtx::default());
            assert_eq!(cfg, None);
            assert_eq!(
                paths(&warnings),
                [("interaction_disabled_by_server", "interaction".to_owned())]
            );
        }
        // A session that did not ask is not warned.
        let (cfg, warnings) = parse(
            &Value::Null,
            &ServerInteraction::default(),
            ParseCtx::default(),
        );
        assert_eq!((cfg, warnings.len()), (None, 0));
    }

    #[test]
    fn apply_update_changes_policy_duplex_and_the_name_and_refuses_the_rest() {
        let mut cfg = defaults();
        let patch = json!({
            "policy": { "ack_due_ms": 150, "quiet_checkin_ms": 5 },
            "duplex": { "eager": false },
            "front": { "preferred_name": "Ana", "persona": "other", "profile": "model" },
            "tasks": { "tools": [tool("late")], "speculative_start": "off" },
            "events": { "floor": false },
            "regime": "delegating"
        });
        let (mut applied, warnings) = apply_update(&mut cfg, &patch);
        applied.sort();
        assert_eq!(
            applied,
            [
                "duplex.eager",
                "events.floor",
                "front.preferred_name",
                "policy.ack_due_ms"
            ]
        );
        assert_eq!(
            paths(&warnings),
            [
                (
                    "interaction_override_refused",
                    "interaction.front.persona".to_owned()
                ),
                (
                    "interaction_override_refused",
                    "interaction.front.profile".to_owned()
                ),
                (
                    "interaction_override_refused",
                    "interaction.regime".to_owned()
                ),
                (
                    "interaction_override_refused",
                    "interaction.tasks.speculative_start".to_owned()
                ),
                (
                    "interaction_override_refused",
                    "interaction.tasks.tools".to_owned()
                ),
                (
                    "interaction_value_ignored",
                    "interaction.policy.quiet_checkin_ms".to_owned()
                ),
            ]
        );
        assert!(warnings.iter().all(|w| {
            (w.code == "interaction_override_refused") == (w.detail["reason"] == "not_mid_session")
        }));

        let mut expected = defaults();
        expected.policy.ack_due_ms = 150;
        expected.duplex.eager = false;
        expected.events_floor = false;
        expected.front.preferred_name = Some("Ana".to_owned());
        assert_eq!(cfg, expected);

        let (applied, warnings) =
            apply_update(&mut cfg, &json!({ "front": { "preferred_name": null } }));
        assert_eq!((applied.len(), warnings.len()), (1, 0));
        assert_eq!(cfg.front.preferred_name, None);

        let (applied, warnings) = apply_update(&mut cfg, &json!(true));
        assert!(applied.is_empty());
        assert_eq!(
            paths(&warnings),
            [("interaction_value_ignored", "interaction".to_owned())]
        );
    }

    #[test]
    fn the_published_echo_keys_are_read_and_checked() {
        let raw = json!({ "duplex": { "echo": {
            "client_aec": false, "phone_line": true, "tail_ms": 1500,
            "max_bulk_delay_ms": 600, "auto_enable_after_echo_episodes": 0
        } } });
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(warnings, Vec::new());
        let echo = cfg.expect("parsed").duplex.echo;
        assert_eq!(
            echo,
            EchoConfig {
                client_aec: Some(false),
                phone_line: Some(true),
                tail_ms: 1500,
                max_bulk_delay_ms: 600,
                auto_enable_after_echo_episodes: 0,
                ..EchoConfig::default()
            }
        );
        // A client that cancels no echo itself gets the canceller under `auto`.
        assert!(echo.aec_on(false));
        assert!(!EchoConfig::default().aec_on(false));
        // A phone line: a room's SIP caller, a client that says so, else an 8 kHz input.
        let default = EchoConfig::default();
        assert!(default.phone_line(true, 16_000) && default.phone_line(false, 8_000));
        assert!(!default.phone_line(false, 16_000) && echo.phone_line(false, 16_000));
        let not_phone = EchoConfig {
            phone_line: Some(false),
            ..default
        };
        assert!(!not_phone.phone_line(false, 8_000) && not_phone.phone_line(true, 8_000));

        let raw = json!({ "duplex": { "echo": {
            "client_aec": "no", "tail_ms": 6000, "max_bulk_delay_ms": 100,
            "auto_enable_after_echo_episodes": 11
        } } });
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(cfg.expect("parsed").duplex.echo, EchoConfig::default());
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        // The echo shows what was set and the numbers.
        let echoed = serde_json::to_value(echo).expect("serialises");
        assert_eq!(echoed["client_aec"], false);
        assert_eq!(echoed["max_bulk_delay_ms"], 600);
        let echoed = serde_json::to_value(EchoConfig::default()).expect("serialises");
        assert!(echoed.get("client_aec").is_none() && echoed.get("phone_line").is_none());
    }

    #[test]
    fn listening_sounds_take_an_intensity_that_only_lowers() {
        let raw = json!({ "duplex": { "backchannel": { "enabled": true, "intensity": "low" } } });
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(warnings, Vec::new());
        let cfg = cfg.expect("parsed");
        assert_eq!(cfg.duplex.backchannel, Tri::On);
        assert_eq!(cfg.duplex.backchannel_intensity, Some(Intensity::Low));
        // Intensity alone keeps `enabled` at its default.
        let raw = json!({ "duplex": { "backchannel": { "intensity": "off" } } });
        let (cfg, _) = parse(&raw, &on(), ParseCtx::default());
        let cfg = cfg.expect("parsed");
        assert_eq!(
            (cfg.duplex.backchannel, cfg.duplex.backchannel_intensity),
            (Tri::Auto, Some(Intensity::Off))
        );
        let raw = json!({ "duplex": { "backchannel": { "intensity": "loud", "rate": 1 } } });
        let (_, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(
            paths(&warnings),
            [
                (
                    "interaction_value_ignored",
                    "interaction.duplex.backchannel.intensity".to_owned()
                ),
                (
                    "unknown_config_keys",
                    "interaction.duplex.backchannel.rate".to_owned()
                ),
            ]
        );
        // Mid-session.
        let mut cfg = defaults();
        let patch = json!({ "duplex": { "backchannel": { "intensity": "high" } } });
        let (applied, warnings) = apply_update(&mut cfg, &patch);
        assert_eq!(
            (applied, warnings.len()),
            (vec!["duplex.backchannel.intensity".to_owned()], 0)
        );
        // The table of chapter 5, from English's medium numbers.
        let english = BackchannelTiming::default();
        assert_eq!(Intensity::Off.timing(english), None);
        assert_eq!(Intensity::Medium.timing(english), Some(english));
        let low = Intensity::Low.timing(english).expect("low");
        assert_eq!(
            (low.skip_every, low.gap_ms, low.per_turn, low.min_voiced_ms),
            (2, 12_000, Some(1), 2_142)
        );
        let high = Intensity::High.timing(english).expect("high");
        assert_eq!(
            (
                high.skip_every,
                high.gap_ms,
                high.per_turn,
                high.min_voiced_ms
            ),
            (6, 4_320, None, 1_071)
        );
        assert!(Intensity::Low < Intensity::Medium && Intensity::Medium < Intensity::High);
    }

    #[test]
    fn the_echo_defences_default_on_and_are_set_at_session_start_only() {
        let echo = defaults().duplex.echo;
        assert!(echo.correlation_gate);
        // The canceller follows the line: on for a phone, off otherwise, unless it is set.
        assert!(echo.aec_on(true) && !echo.aec_on(false));
        let set = |aec| EchoConfig { aec, ..echo };
        assert!(set(Tri::On).aec_on(false) && !set(Tri::Off).aec_on(true));

        let raw =
            json!({ "duplex": { "echo": { "aec": "always", "correlation_gate": 1, "tail": 9 } } });
        let (cfg, warnings) = parse(&raw, &on(), ParseCtx::default());
        assert_eq!(
            cfg.expect("parsed").duplex.echo,
            echo,
            "the defaults are kept"
        );
        let ignored = |key: &str| {
            (
                "interaction_value_ignored",
                format!("interaction.duplex.echo.{key}"),
            )
        };
        assert_eq!(
            paths(&warnings),
            [
                ignored("aec"),
                ignored("correlation_gate"),
                (
                    "unknown_config_keys",
                    "interaction.duplex.echo.tail".to_owned()
                ),
            ]
        );

        let mut cfg = defaults();
        let patch = json!({ "duplex": { "echo": { "aec": "on" }, "duck_db": -9 } });
        let (applied, warnings) = apply_update(&mut cfg, &patch);
        assert_eq!(applied, ["duplex.duck_db"]);
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_override_refused",
                "interaction.duplex.echo".to_owned()
            )]
        );
        assert_eq!(cfg.duplex.echo, echo);
    }

    #[test]
    fn the_echo_round_trips_and_holds_no_secret() {
        let server = ServerInteraction {
            allow_client_tools: true,
            ..with_front_model()
        };
        let raw = json!({
            "front": { "persona": "larkspur-home-sam", "act_budget_ms": 200, "preferred_name": "Sam" },
            "tasks": {
                "llm": { "api_key": "sk-secret", "base_url": "https://llm.internal.example.com/v1" },
                "tools": [{ "name": "refund", "url": "https://tools.internal.example.com/refund", "side_effects": true }],
                "speculative_start": "off"
            },
            "policy": { "ack_due_ms": 120, "hitl_answer_timeout_ms": 15000 },
            "duplex": { "duck_db": -9.5, "eager": false },
            "events": { "floor": false }
        });
        let (cfg, warnings) = parse(&raw, &server, ParseCtx::default());
        assert!(warnings.is_empty());
        let cfg = cfg.unwrap();
        let echoed = echo(&cfg, "llm", &["front_unavailable"]);

        assert_eq!(echoed["enabled"], true);
        assert_eq!(echoed["protocol_version"], "1.1");
        assert_eq!(echoed["regime"], "cascade");
        assert_eq!(echoed["front"]["profile"], "model");
        assert_eq!(echoed["degraded"], json!(["front_unavailable"]));
        assert_eq!(
            echoed["tasks"],
            json!({
                "backend": "llm", "speculative_start": "off", "tools": ["refund"],
                "reconnect_grace_s": 60, "push": false,
                "session_end": "auto", "orphan_max_s": 1800,
                "orchestrator": false, "agents": [],
                "limits": { "max_live": null, "max_live_tenant": null },
                "announce": "ask", "follow_up_ttl_s": 1800, "resume": false,
            })
        );
        let text = echoed.to_string();
        assert!(!text.contains("sk-secret") && !text.contains("internal.example.com"));

        // The settings a session may send come back as the same config.
        let again = json!({
            "front": echoed["front"], "policy": echoed["policy"],
            "duplex": echoed["duplex"], "events": echoed["events"],
            "tasks": { "speculative_start": echoed["tasks"]["speculative_start"] }
        });
        let (round, warnings) = parse(&again, &server, ParseCtx::default());
        assert!(warnings.is_empty());
        let mut expected = cfg;
        expected.tasks.llm = None;
        expected.tasks.tools.clear();
        assert_eq!(round.unwrap(), expected);
    }

    #[test]
    fn escape_flags_are_the_operators_and_only_the_plans_names() {
        let mut server = ServerInteraction::default();
        let problems = server.apply_lookup(|key| {
            (key == "WAAV_REALTIME_LEGACY_WIRE")
                .then(|| " deepgram_text_as_agent_message, no_such_flag,ultravox_omit_tools".into())
        });
        assert_eq!(
            server.realtime_legacy_wire,
            ["deepgram_text_as_agent_message", "ultravox_omit_tools"]
        );
        assert_eq!(
            problems,
            ["realtime_legacy_wire: `no_such_flag` is not an escape flag"]
        );
        let yaml = "interaction:\n  realtime_legacy_wire: [gemini_omit_tool_behavior]\n";
        assert_eq!(
            server_from_yaml(yaml).unwrap().realtime_legacy_wire,
            ["gemini_omit_tool_behavior"]
        );
        // A session cannot set one.
        let (_, warnings) = parse(
            &json!({"realtime_legacy_wire": ["nova_text_non_interactive"]}),
            &ServerInteraction {
                enabled: true,
                ..Default::default()
            },
            ParseCtx::default(),
        );
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_override_refused",
                "interaction.realtime_legacy_wire".to_owned()
            )]
        );
    }

    /// The fixes on by default and `extra`, in the list's order.
    fn defaults_and(extra: &[&str]) -> Vec<String> {
        PROVIDER_FIXES
            .iter()
            .map(|(fix, _)| *fix)
            .filter(|fix| FIXES_ON_BY_DEFAULT.contains(fix) || extra.contains(fix))
            .map(str::to_owned)
            .collect()
    }

    /// The fixes on by default but those of `off`.
    fn defaults_less(off: &[&str]) -> Vec<String> {
        let mut fixes = defaults_and(&[]);
        fixes.retain(|fix| !off.contains(&fix.as_str()));
        fixes
    }

    #[test]
    fn provider_fixes_are_off_until_listed_and_an_escape_flag_wins() {
        // Nothing listed: every fix is off but those whose vendor probe passed.
        assert_eq!(
            fixes_in_force(&ServerInteraction::default()),
            FIXES_ON_BY_DEFAULT
        );
        // Their escape flag turns them off.
        let escaped = ServerInteraction {
            realtime_legacy_wire: ["deepgram_text_as_agent_message", "deepgram_omit_tools"]
                .map(str::to_owned)
                .to_vec(),
            ..Default::default()
        };
        assert_eq!(
            fixes_in_force(&escaped),
            defaults_less(&["deepgram_user_text", "deepgram_tools"])
        );
        let mut server = ServerInteraction::default();
        let problems = server.apply_lookup(|key| match key {
            "WAAV_PROVIDER_FIXES" => {
                Some("deepgram_user_text, nova_interactive_text,bogus,openai_stt_default".into())
            }
            "WAAV_REALTIME_LEGACY_WIRE" => Some("nova_text_non_interactive".into()),
            _ => None,
        });
        assert_eq!(problems, ["provider_fixes: `bogus` is not a provider fix"]);
        assert_eq!(
            fixes_in_force(&server),
            defaults_and(&["openai_stt_default"]),
            "nova's escape flag keeps today's wire although its fix is listed"
        );
        let yaml = "interaction:\n  provider_fixes: [gemini_tool_behavior, ultravox_tools]\n";
        assert_eq!(
            fixes_in_force(&server_from_yaml(yaml).unwrap()),
            defaults_and(&["gemini_tool_behavior", "ultravox_tools"])
        );
        // Every escape flag a fix names is one of the plan's, and each of the plan's has its fix.
        for (_, escape) in PROVIDER_FIXES {
            assert!(escape.is_none_or(|flag| LEGACY_WIRE.contains(&flag)));
        }
        for flag in LEGACY_WIRE {
            assert!(
                PROVIDER_FIXES
                    .iter()
                    .any(|(_, escape)| *escape == Some(flag)),
                "{flag}"
            );
        }
        // A session cannot turn one on.
        let (_, warnings) = parse(
            &json!({"provider_fixes": ["deepgram_user_text"]}),
            &ServerInteraction {
                enabled: true,
                ..Default::default()
            },
            ParseCtx::default(),
        );
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_override_refused",
                "interaction.provider_fixes".to_owned()
            )]
        );
    }

    /// The round-9 fixes on by documentation evidence (24 October): each in force with no
    /// operator setting, and each escape flag restores today's frame on its own.
    #[test]
    fn the_documented_round_nine_fixes_are_on_and_each_has_its_escape_flag() {
        let documented = [
            ("azure_ga_url", "azure_deployment_query"),
            ("openai_manual_turn_null", "openai_turn_detection_none"),
            ("openai_replay_output_text", "openai_replay_text"),
            ("hume_user_input", "hume_text_input"),
            ("hume_audio_settings", "hume_settings_when_needed"),
            ("hume_cancel_local", "hume_stop_assistant"),
            ("hume_message_ids", "hume_drop_id_less_messages"),
            ("inworld_basic_key", "inworld_encode_key"),
            ("inworld_session_key", "inworld_endpoint_session_only"),
        ];
        let on = fixes_in_force(&ServerInteraction::default());
        for (fix, flag) in documented {
            assert!(on.iter().any(|f| f == fix), "{fix} on by default");
            assert!(
                PROVIDER_FIXES.contains(&(fix, Some(flag))),
                "{fix} escaped by {flag}"
            );
            let escaped = ServerInteraction {
                realtime_legacy_wire: vec![flag.to_owned()],
                ..Default::default()
            };
            assert_eq!(fixes_in_force(&escaped), defaults_less(&[fix]), "{flag}");
        }
    }

    #[test]
    fn a_session_turns_a_fix_off_and_on_only_where_the_operator_lets_it() {
        let server = ServerInteraction {
            provider_fixes: vec!["gemini_tool_behavior".to_owned()],
            realtime_legacy_wire: vec!["nova_text_non_interactive".to_owned()],
            ..Default::default()
        };
        // Nothing asked: the server's.
        assert_eq!(
            session_fixes_of(&server, None).0,
            defaults_and(&["gemini_tool_behavior"])
        );
        let asked = json!({"deepgram_user_text": false, "gemini_tool_behavior": false,
                           "ultravox_tools": true, "bogus": false, "deepgram_tools": "yes"});
        let (fixes, warnings) = session_fixes_of(&server, Some(&asked));
        assert_eq!(fixes, defaults_less(&["deepgram_user_text"]));
        assert_eq!(
            paths(&warnings),
            [
                (
                    "interaction_override_refused",
                    "provider_fixes.ultravox_tools".to_owned()
                ),
                (
                    "interaction_value_ignored",
                    "provider_fixes.deepgram_tools".to_owned()
                ),
                ("unknown_config_keys", "provider_fixes.bogus".to_owned()),
            ]
        );
        // With the operator's leave a session turns one on, never past its escape flag.
        let allowed = ServerInteraction {
            provider_fixes_per_session: true,
            ..server.clone()
        };
        let asked = json!({"ultravox_tools": true, "nova_interactive_text": true,
                           "gemini_tool_behavior": true});
        let (fixes, warnings) = session_fixes_of(&allowed, Some(&asked));
        assert_eq!(
            fixes,
            defaults_and(&["gemini_tool_behavior", "ultravox_tools"])
        );
        assert_eq!(
            paths(&warnings),
            [(
                "interaction_override_refused",
                "provider_fixes.nova_interactive_text".to_owned()
            )]
        );
        let mut env = ServerInteraction::default();
        assert!(
            env.apply_lookup(|key| (key == "WAAV_PROVIDER_FIXES_PER_SESSION").then(|| "1".into()))
                .is_empty()
        );
        assert!(env.provider_fixes_per_session);
        let yaml = "interaction:\n  provider_fixes_per_session: true\n";
        assert!(server_from_yaml(yaml).unwrap().provider_fixes_per_session);
    }

    #[test]
    fn from_lookup_reads_the_environment_names() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(
            ServerInteraction::from_lookup(env(&[])),
            ServerInteraction::default()
        );
        assert!(!ServerInteraction::default().enabled);

        let server = ServerInteraction::from_lookup(env(&[
            ("WAAV_INTERACTION_ENABLED", "1"),
            (
                "WAAV_INTERACTION_DEFAULT",
                r#"{"policy":{"ack_due_ms":250}}"#,
            ),
            ("WAAV_INTERACTION_FRONT_URL", " http://127.0.0.1:18741 "),
            ("WAAV_INTERACTION_FRONT_MODEL", "qwen1.5b"),
            ("WAAV_INTERACTION_TOOL_SECRET", "s3cret"),
            ("WAAV_INTERACTION_ALLOW_CLIENT_TOOLS", "TRUE"),
            (
                "WAAV_TASK_REDIS_URL",
                "redis://tasks:secret@redis.internal:6379/2",
            ),
        ]));
        assert_eq!(
            server,
            ServerInteraction {
                enabled: true,
                default: Some(json!({ "policy": { "ack_due_ms": 250 } })),
                agent_default: None,
                front_model: Some(FrontModelServer {
                    url: "http://127.0.0.1:18741".to_owned(),
                    urls: Vec::new(),
                    model: Some("qwen1.5b".to_owned()),
                    api_key: None,
                    command: None,
                    licence: None,
                    line_budget_ms: None,
                }),
                tool_secret: Some("s3cret".to_owned()),
                allow_client_tools: true,
                task_redis_url: Some("redis://tasks:secret@redis.internal:6379/2".to_owned()),
                task_backends: Vec::new(),
                allow_client_backends: false,
                ..Default::default()
            }
        );
        let shown = format!("{server:?}");
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(!shown.contains("redis.internal"), "{shown}");

        // Unreadable values keep the default; an empty value is "not set".
        let server = ServerInteraction::from_lookup(env(&[
            ("WAAV_INTERACTION_ENABLED", "maybe"),
            ("WAAV_INTERACTION_DEFAULT", "{not json"),
            ("WAAV_INTERACTION_FRONT_MODEL", "orphan"),
            ("WAAV_INTERACTION_TOOL_SECRET", "  "),
        ]));
        assert_eq!(server, ServerInteraction::default());
    }

    /// Spec 029 SF-3 and SF-6: several front servers, and the front's key from the
    /// environment, where a config file's would sit in a ConfigMap. It is never shown.
    #[test]
    fn the_front_urls_and_its_key_come_from_the_environment() {
        let lookup = |pairs: &'static [(&'static str, &'static str)]| {
            ServerInteraction::from_lookup(move |key: &str| {
                let found = pairs.iter().find(|(name, _)| *name == key);
                found.map(|(_, value)| (*value).to_owned())
            })
        };
        let server = lookup(&[
            (
                "WAAV_INTERACTION_FRONT_URL",
                "http://front-a:8080, http://front-b:8080,",
            ),
            ("WAAV_INTERACTION_FRONT_API_KEY", "front-k3y"),
        ]);
        let front = server.front_model.as_ref().expect("a front");
        assert_eq!(front.url, "http://front-a:8080");
        assert_eq!(front.urls, ["http://front-b:8080"]);
        assert_eq!(front.api_key.as_deref(), Some("front-k3y"));
        let shown = format!("{server:?}");
        assert!(!shown.contains("front-k3y"), "{shown}");
        // A key with no front configures nothing.
        let mut alone = ServerInteraction::default();
        let problems = alone.apply_lookup(|key| {
            (key == "WAAV_INTERACTION_FRONT_API_KEY").then(|| "front-k3y".to_owned())
        });
        assert!(alone.front_model.is_none());
        assert_eq!(problems.len(), 1, "{problems:?}");
        // The YAML file's key gives way to the environment's.
        let yaml =
            "interaction:\n  front_model:\n    url: http://front:8080\n    api_key: file-key\n";
        let mut server = server_from_yaml(yaml).unwrap();
        server.apply_lookup(|key| {
            (key == "WAAV_INTERACTION_FRONT_API_KEY").then(|| "env-key".to_owned())
        });
        let front = server.front_model.expect("a front");
        assert_eq!(front.api_key.as_deref(), Some("env-key"));
    }

    /// Once a decision's act is known, its line has the front server's own deadline (a slow CPU
    /// front's line ran past the decision's budget and was never said): 1.5 s by default, the
    /// operator's `line_budget_ms` from the file or `WAAV_INTERACTION_FRONT_LINE_BUDGET_MS`
    /// (the environment's wins), at most 10 s.
    #[test]
    fn the_front_lines_deadline_is_the_operators() {
        use std::time::Duration;
        let budget =
            |server: &ServerInteraction| server.front_model.as_ref().unwrap().line_budget();
        let env = |value: &'static str| {
            move |key: &str| {
                (key == "WAAV_INTERACTION_FRONT_LINE_BUDGET_MS").then(|| value.to_owned())
            }
        };
        assert_eq!(
            FrontModelServer::default().line_budget(),
            Duration::from_millis(1_500)
        );
        let yaml =
            "interaction:\n  front_model:\n    url: http://front:8080\n    line_budget_ms: 2500\n";
        let mut server = server_from_yaml(yaml).unwrap();
        assert!(server.apply_lookup(|_| None).is_empty());
        assert_eq!(budget(&server), Duration::from_millis(2_500));
        assert!(server.apply_lookup(env("4000")).is_empty());
        assert_eq!(budget(&server), Duration::from_millis(4_000));
        // A value that is no number of milliseconds, or more than 10 s, is a problem and
        // changes nothing.
        for bad in ["soon", "10001", "-1"] {
            let mut server = server_from_yaml(yaml).unwrap();
            let problems = server.apply_lookup(env(bad));
            assert_eq!(problems.len(), 1, "{bad}: {problems:?}");
            assert_eq!(budget(&server), Duration::from_millis(2_500), "{bad}");
        }
        // Too long in the file: the default, and a problem.
        let long =
            "interaction:\n  front_model:\n    url: http://front:8080\n    line_budget_ms: 60000\n";
        let mut server = server_from_yaml(long).unwrap();
        assert_eq!(server.apply_lookup(|_| None).len(), 1);
        assert_eq!(budget(&server), Duration::from_millis(1_500));
        // No front, no deadline to set.
        let mut alone = ServerInteraction::default();
        assert_eq!(alone.apply_lookup(env("2000")).len(), 1);
        assert!(alone.front_model.is_none());
    }

    #[test]
    fn the_yaml_block_is_read_and_the_environment_wins() {
        let yaml = r#"
server:
  port: 3001
interaction:
  enabled: true
  default:
    policy:
      ack_due_ms: 250
  front_model:
    url: http://127.0.0.1:18741
    api_key: front-key
  tool_secret: from-yaml
"#;
        let none = |_: &str| None;
        let (server, problems) = load_server(Some(Ok(yaml.to_owned())), none);
        assert_eq!(problems, Vec::<String>::new());
        assert!(server.enabled && !server.allow_client_tools);
        assert_eq!(
            server.default,
            Some(json!({ "policy": { "ack_due_ms": 250 } }))
        );
        assert_eq!(server.tool_secret.as_deref(), Some("from-yaml"));
        let front = server.front_model.unwrap();
        assert_eq!(
            (front.url.as_str(), front.model, front.api_key.as_deref()),
            ("http://127.0.0.1:18741", None, Some("front-key"))
        );

        let env = |key: &str| match key {
            "WAAV_INTERACTION_ENABLED" => Some("false".to_owned()),
            "WAAV_INTERACTION_TOOL_SECRET" => Some("from-env".to_owned()),
            "WAAV_INTERACTION_FRONT_MODEL" => Some("qwen".to_owned()),
            _ => None,
        };
        let (server, problems) = load_server(Some(Ok(yaml.to_owned())), env);
        assert!(problems.is_empty() && !server.enabled);
        assert_eq!(server.tool_secret.as_deref(), Some("from-env"));
        assert_eq!(server.front_model.unwrap().model.as_deref(), Some("qwen"));

        // No file, no key, or an empty key: the defaults, silently.
        for yaml in [
            None,
            Some(Ok("server:\n  port: 1\n".to_owned())),
            Some(Ok("interaction:\n".to_owned())),
        ] {
            assert_eq!(
                load_server(yaml, none),
                (ServerInteraction::default(), Vec::new())
            );
        }

        // A typo, an unreadable file and a bad default block are reported; the mode stays off
        // or the bad field keeps its default.
        let (server, problems) =
            load_server(Some(Ok("interaction:\n  enabld: true\n".to_owned())), none);
        assert_eq!((server, problems.len()), (ServerInteraction::default(), 1));
        let (server, problems) = load_server(Some(Err("cannot read x.yaml".to_owned())), none);
        assert_eq!(
            (server, problems),
            (
                ServerInteraction::default(),
                vec!["cannot read x.yaml".to_owned()]
            )
        );
        let (server, problems) = load_server(
            Some(Ok(
                "interaction:\n  enabled: true\n  default:\n    policy:\n      ack_due_ms: 9000\n"
                    .to_owned(),
            )),
            none,
        );
        assert!(server.enabled);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("interaction.policy.ack_due_ms"));
    }

    #[test]
    fn the_global_server_block_can_be_replaced() {
        set_server_for_tests(with_front_model());
        assert_eq!(*server(), with_front_model());
        set_server_for_tests(ServerInteraction::default());
        assert!(!server().enabled);
    }
}

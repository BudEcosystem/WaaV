//! Built-in automatic conversation loop (plan W-O2).
//!
//! The [`ConversationOrchestrator`] turns the gateway's separate STT and TTS
//! primitives into a first-class voice agent **without requiring the
//! `dag-routing` feature**:
//!
//! ```text
//!  final STT transcript ──► LlmClient (streaming) ──► VoiceManager.speak()
//!         ▲                                                   │
//!         └────────── barge-in: cancel turn + clear_tts ◄─────┘
//! ```
//!
//! Responsibilities:
//! - On a **final** STT result, run an LLM turn (streaming) and pipe the tokens
//!   to `VoiceManager::speak`, so synthesis starts before the completion ends.
//! - Maintain **per-session conversation history** (owned by the shared
//!   [`LlmClient`], keyed by session id, so turn N+1 includes turns 1..N).
//! - Drive **turn-taking** off the VoiceManager's existing finalized-turn signal
//!   (`is_speech_final`), which is itself fed by smart-turn / VAD / endpointing.
//! - Handle **barge-in** by reusing the VoiceManager's `interruption_state`
//!   (`is_interruption_blocked`) as the single source of truth; the orchestrator
//!   only adds LLM-turn cancellation + `clear_tts()` when new user speech arrives
//!   while the bot is interruptible.
//!
//! It is **opt-in**: instantiated only when a `conversation` config block is
//! present. Absent it, the existing STT/TTS-primitive behavior is unchanged.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex as SyncMutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::core::llm::{LlmClient, LlmClientConfig, LlmError};
use crate::core::stt::STTResult;
use crate::core::voice_manager::{VoiceManager, VoiceManagerError};

/// Errors raised while constructing a conversation orchestrator.
#[derive(Debug, thiserror::Error)]
pub enum ConversationOrchestratorError {
    /// The configured LLM `base_url` failed SSRF validation.
    #[error("conversation LLM base_url rejected: {0}")]
    InvalidLlmUrl(String),

    /// The LLM call itself failed (surfaced from a turn).
    #[error("conversation LLM error: {0}")]
    Llm(#[from] LlmError),

    /// A VoiceManager operation failed.
    #[error("conversation voice error: {0}")]
    Voice(#[from] VoiceManagerError),
}

/// Configuration for the built-in conversation loop.
///
/// Field-compatible with the LLM portion of [`LlmClientConfig`] plus the
/// orchestration knobs. When present on a session, the orchestrator is wired up;
/// when absent, the gateway keeps its raw STT/TTS behavior.
#[derive(Debug, Clone)]
pub struct ConversationConfig {
    /// OpenAI-compatible base URL for the LLM.
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// Optional system prompt (seeds turn 1).
    pub system_prompt: Option<String>,
    /// API key (literal or `${ENV_VAR}`); falls back to `OPENAI_API_KEY`.
    pub api_key: Option<String>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Max tokens per completion.
    pub max_tokens: Option<u32>,
    /// Stream tokens to TTS as they arrive (default true; recommended for low
    /// time-to-first-audio).
    pub streaming: bool,
    /// Max retained history messages.
    pub max_history: usize,
    /// Whether the bot's speech is interruptible (barge-in). Default true.
    pub allow_interruption: bool,
    /// Eager end-of-turn (P1.2b): when a turn-complete prediction arrives
    /// BEFORE the provider's speech_final, start the LLM speculatively with a
    /// STAGED history (no mutation). Confirmed by a matching final → commit +
    /// speak; user kept talking → cancel with zero history pollution. Opt-in:
    /// raises LLM call volume on resumed turns. Default false.
    pub eager_eot: bool,
    /// Vendor wire format for the LLM (B-G1): `None` = OpenAI-compatible
    /// (with canonical-host inference); `Some(Anthropic|Gemini)` speaks the
    /// native Messages / generateContent APIs.
    pub provider_kind: Option<crate::core::llm::AdapterKind>,
    /// MinWords barge-in gate (A-G3): while the bot is audibly speaking,
    /// require ≥ N words to interrupt (1 word when silent). `None`/`0` keeps
    /// the legacy any-speech barge-in. Values < 2 are clamped to 2.
    pub barge_in_min_words: Option<usize>,
    /// Token-aware context compaction (B-G6): when the session context's
    /// token ESTIMATE crosses this value, the oldest messages are
    /// summarized after the turn (system + recent kept). `0` = off.
    pub summarize_target_tokens: usize,
    /// User-mute strategy name (A-G5); see the WS config for values.
    pub mute_strategy: Option<String>,
    /// Strip markdown from sentences before TTS (C-G4): `**bold**`, code
    /// ticks, links, headings — spoken asterisks/URLs ruin voice output.
    /// Default ON.
    pub strip_markdown: bool,
    /// Idle re-engagement (A-G7): after this many ms with no user/bot
    /// activity, the bot speaks a gentle re-engagement turn. `0` = off.
    pub user_idle_timeout_ms: u64,
    /// D1: reasoning/thinking-effort dial. `None` = vendor default. The value
    /// sent to the wire is floor-clamped to the model's floor in
    /// `to_client_config`; the floor is echoed to the client at config time.
    pub reasoning_effort: Option<crate::core::llm::ReasoningEffort>,
    /// D3: latency-masking mode (default `Auto`). One masking utterance/turn when
    /// first audio is slow, so the caller never hears dead air.
    pub latency_filler: LatencyFiller,
    /// D3: override the masking wait threshold (ms). `None` = mode default.
    pub latency_filler_after_ms: Option<u64>,
    /// D3: custom masking phrases. Empty = the built-in action-phrase pool.
    pub latency_filler_phrases: Vec<String>,
    /// S1/S2 (REALTIME_REASONING.md §5): optional slow/reasoning tier. When set,
    /// complex turns are escalated to this model (sharing the conversation
    /// history) while the fast `model` handles the rest and the D3 filler masks
    /// the reasoning latency. `None` = single-tier.
    pub reasoning_model: Option<String>,
    /// S1/S2: reasoning-tier base URL (defaults to `base_url` — e.g. one ollama
    /// endpoint serving both a fast and a reasoning model).
    pub reasoning_base_url: Option<String>,
    /// S1/S2: reasoning-tier API key (defaults to `api_key`).
    pub reasoning_api_key: Option<String>,
    /// S1/S2: reasoning-tier vendor wire format (defaults to `provider_kind`).
    pub reasoning_provider_kind: Option<crate::core::llm::AdapterKind>,
    /// S2: how to route turns between the fast and reasoning tiers.
    pub reasoning_route: RoutingMode,
    /// P1+A7 (REALTIME_REASONING.md §8, FOLLOWUP §2.2): the reasoning tier's
    /// MAX-SILENCE-GAP budget, in ms — the longest the line may go silent BEFORE
    /// first audio OR BETWEEN audio chunks. If exceeded, the reasoner is cancelled
    /// and: with audio already played, the partial is committed (no talk-over
    /// restart); with none yet, the turn DEGRADES to the fast tier. A reasoner
    /// streaming steadily resets the gap on each chunk and is never truncated.
    /// `0` disables the watchdog (the reasoner runs to its own request timeout).
    /// Ignored for single-tier configs. Default [`DEFAULT_REASONING_BUDGET_MS`].
    pub reasoning_budget_ms: u64,
    /// P1: the canned line spoken when EVERY tier fails (or produces no content)
    /// and nothing else has been said this turn — a graceful apology instead of
    /// dead air or a dropped session. `None` ⇒ [`DEFAULT_DEGRADATION_MESSAGE`].
    pub degradation_message: Option<String>,
    /// P2 (REALTIME_REASONING.md §8): per-turn ceiling on LLM re-inference rounds
    /// (the tool-call loop — the dominant spend multiplier). Bounds a pathological
    /// call-me-again loop on a billing gateway. Default
    /// [`DEFAULT_MAX_LLM_CALLS_PER_TURN`].
    pub max_llm_calls_per_turn: u32,
    /// P2: the reasoning tier's output-token budget (thinking + answer) — the
    /// single most direct $ lever, since reasoning models bill every thinking
    /// token. When set it is the reasoning tier's `max_tokens` (a cost ceiling);
    /// `None` ⇒ the reasoning tier defaults to [`REASONING_DEFAULT_MAX_TOKENS`]
    /// (or an explicit global `max_tokens` if larger intent), NEVER the fast
    /// tier's voice cap. The fast tier is unaffected.
    pub max_reasoning_tokens: Option<u32>,
}

/// P2: default per-turn LLM re-inference ceiling. Matches the built-in tool-loop
/// bound (`ToolLoopOptions::max_rounds`) so the default preserves prior behavior.
pub const DEFAULT_MAX_LLM_CALLS_PER_TURN: u32 = 8;

/// The reasoning tier's default output budget when neither `max_reasoning_tokens`
/// nor a global `max_tokens` is set. Generous enough to clear Anthropic's 1024
/// thinking floor (+ answer headroom) and give o-series usable reasoning room —
/// the reasoning tier is decoupled from the fast tier's 256 voice cap.
pub const REASONING_DEFAULT_MAX_TOKENS: u32 = 4096;

/// P1+A7: default reasoning-tier max-silence-gap budget (ms) — the longest the
/// line may go silent BEFORE first audio OR between audio chunks. Generous
/// enough not to truncate legitimate deep reasoning, tight enough that a *stuck*
/// or *stalled* reasoner yields rather than leaving a live call silent.
pub const DEFAULT_REASONING_BUDGET_MS: u64 = 15_000;

/// A7: how often the stall watchdog samples the silence gap (ms). Internal — not
/// operator-tunable; bounds breach-detection latency without busy-looping.
const STALL_POLL_MS: u64 = 250;

/// P1: default spoken apology when all LLM tiers fail — never dead air.
pub const DEFAULT_DEGRADATION_MESSAGE: &str =
    "Sorry, I'm having trouble with that right now. Could you try again?";

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o-mini".to_string(),
            system_prompt: None,
            api_key: None,
            temperature: None,
            max_tokens: None,
            streaming: true,
            max_history: 20,
            allow_interruption: true,
            eager_eot: false,
            provider_kind: None,
            barge_in_min_words: None,
            summarize_target_tokens: 0,
            mute_strategy: None,
            strip_markdown: true,
            user_idle_timeout_ms: 0,
            reasoning_effort: None,
            latency_filler: LatencyFiller::default(),
            latency_filler_after_ms: None,
            latency_filler_phrases: Vec::new(),
            reasoning_model: None,
            reasoning_base_url: None,
            reasoning_api_key: None,
            reasoning_provider_kind: None,
            reasoning_route: RoutingMode::default(),
            reasoning_budget_ms: DEFAULT_REASONING_BUDGET_MS,
            degradation_message: None,
            max_llm_calls_per_turn: DEFAULT_MAX_LLM_CALLS_PER_TURN,
            max_reasoning_tokens: None,
        }
    }
}

/// D3 (REALTIME_REASONING.md §4.3): the unified latency-masking mode. ONE knob
/// covering the action-preamble + pre-rendered gap-filler, deduped to at most one
/// masking utterance per turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum LatencyFiller {
    /// Never speak a masking utterance.
    Off,
    /// (Default) Speak ONE short masking utterance per turn when first audio is
    /// slow — keeps the line alive while the LLM/RAG/tool/reasoning runs.
    #[default]
    Auto,
    /// Lower the wait threshold for known-slow (RAG/agentic/reasoning) routes.
    Aggressive,
}

impl LatencyFiller {
    /// Wait before a masking utterance fires, given an optional operator override.
    /// `Auto` ~800ms (under the ~2s "unnatural" line); `Aggressive` ~400ms.
    pub fn wait_ms(self, override_ms: Option<u64>) -> u64 {
        override_ms.unwrap_or(match self {
            LatencyFiller::Off => u64::MAX,
            LatencyFiller::Auto => 800,
            LatencyFiller::Aggressive => 400,
        })
    }

    /// Whether masking is enabled at all.
    pub fn enabled(self) -> bool {
        !matches!(self, LatencyFiller::Off)
    }
}

/// S2 (REALTIME_REASONING.md §5): when a `reasoning_model` is configured, how to
/// route each turn between the fast `model` and the slow reasoning tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RoutingMode {
    /// (Default) A cheap heuristic escalates only complex turns to the reasoning
    /// tier; everything else stays on the fast model (lowest latency + cost).
    #[default]
    Auto,
    /// Always escalate to the reasoning tier (the fast model only ever speaks the
    /// masking opener).
    Always,
}

/// S2 (REALTIME_REASONING.md §5 + FOLLOWUP §2.1): cheap per-turn escalation
/// heuristic — does this user turn look like it needs the reasoning tier? One
/// word-aware lowercase scan, no model call (single-digit µs): the design's
/// fallback when no trained classifier exists. WORD-AWARE (token equality, not
/// substring) + negation-guarded, so everyday billing/sales speech ("not
/// interested", "no refund", "that's reasonable", "interesting weather") no
/// longer false-escalates onto the 2×-cost reasoning tier.
pub fn turn_needs_reasoning(transcript: &str) -> bool {
    turn_needs_reasoning_ctx(transcript, false)
}

/// Context-aware variant: also keeps a SHORT anaphoric continuation after a
/// reasoning turn on the reasoning tier for ONE turn (`prev_was_reasoning`), so
/// a follow-up like "and the second one?" doesn't drop the reasoning thread.
pub fn turn_needs_reasoning_ctx(transcript: &str, prev_was_reasoning: bool) -> bool {
    // Normalize the curly apostrophe (U+2019, common from STT/keyboards) to ASCII
    // so contractions match the ASCII phrase/lemma literals ("what's the total").
    let t = transcript.to_lowercase().replace('\u{2019}', "'");
    let tokens = reasoning_tokenize(&t);

    // Length signal — a long ask escalates regardless of language.
    if tokens.len() > 28 {
        return true;
    }

    // Single-word lemmas (token equality — NO substring leak) + multi-word
    // phrases (contiguous token window); a hit preceded by a negator is skipped.
    // (Bare "interest"/"refund"/"estimate"/"convert" were dropped: transactional
    // requests need no reasoning — the math cases are caught by calculate/how
    // much/percent/% signals.)
    const SINGLE: &[&str] = &[
        "calculate", "compute", "calculation", "explain", "compare", "reason",
        "prove", "analyze", "analyse", "analysis", "analytical", "percent",
        "percentage", "why",
    ];
    const PHRASES: &[&[&str]] = &[
        &["how", "many"],
        &["how", "much"],
        &["step", "by", "step"],
        &["what's", "the", "total"],
        &["work", "out"],
        &["figure", "out"],
        &["break", "down"],
    ];
    if reasoning_keyword_hit(&tokens, SINGLE, PHRASES) {
        return true;
    }

    // Cross-lingual percentage-MATH signal: a percentage figure being OPERATED on
    // ("20% of", "15% de"). Requiring the "of"/"de" connector keeps the genuine
    // calc intent while excluding the ubiquitous intensifier/discount idioms
    // ("50% off", "100% sure", "80% right now") that merely co-occur with '%'.
    if has_percentage_math(&t) {
        return true;
    }

    // Stickiness: a short continuation of a reasoning thread stays on reasoning.
    prev_was_reasoning && reasoning_is_short_continuation(&tokens)
}

/// Tokenize lowercase text into words, KEEPING intra-word ASCII apostrophes so
/// contractions ("don't", "what's") stay whole — Unicode-aware, so non-Latin
/// scripts (Devanagari/CJK) tokenize correctly too. (The curly apostrophe is
/// normalized to ASCII by the caller before tokenizing.)
fn reasoning_tokenize(t: &str) -> Vec<&str> {
    t.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|s| !s.is_empty())
        .collect()
}

/// A percentage figure being OPERATED on: a digit immediately before `%` (allow
/// one space, "20%" or "20 %") followed by an "of"/"de" connector. This escalates
/// genuine percentage math ("what is 20% of my bill", "calcule 15% de 2400") but
/// NOT the intensifier/discount idioms ("50% off", "100% sure", "80% right now")
/// that merely contain a `%` next to a digit. (English/Romance connectors only by
/// design — `route=always` is the documented full cross-lingual path.)
fn has_percentage_math(t: &str) -> bool {
    for (i, _) in t.match_indices('%') {
        let digit_before = t[..i]
            .trim_end()
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_digit());
        if !digit_before {
            continue;
        }
        let after = t[i + 1..].trim_start();
        if after == "of" || after == "de" || after.starts_with("of ") || after.starts_with("de ") {
            return true;
        }
    }
    false
}

fn reasoning_is_negator(tok: &str) -> bool {
    const NEGATORS: &[&str] = &[
        "no", "not", "never", "without", "cannot", "can't", "won't", "don't",
        "isn't", "aren't", "wasn't", "doesn't", "didn't", "couldn't",
        "shouldn't", "wouldn't", "nor", "zero",
    ];
    NEGATORS.contains(&tok)
}

/// A single-token or contiguous-phrase keyword hit NOT immediately preceded by
/// a negator.
fn reasoning_keyword_hit(tokens: &[&str], single: &[&str], phrases: &[&[&str]]) -> bool {
    for (i, tok) in tokens.iter().enumerate() {
        if single.contains(tok) && !(i > 0 && reasoning_is_negator(tokens[i - 1])) {
            return true;
        }
    }
    for phrase in phrases {
        let n = phrase.len();
        if n == 0 || tokens.len() < n {
            continue;
        }
        for start in 0..=tokens.len() - n {
            if &tokens[start..start + n] == *phrase
                && !(start > 0 && reasoning_is_negator(tokens[start - 1]))
            {
                return true;
            }
        }
    }
    false
}

/// A short anaphoric follow-up (≤8 tokens, opens with or carries a continuation
/// cue, no closing/negating cue) that should stick to the reasoning tier.
fn reasoning_is_short_continuation(tokens: &[&str]) -> bool {
    if tokens.is_empty() || tokens.len() > 8 {
        return false;
    }
    const CLOSING: &[&str] = &["thanks", "thank", "bye", "goodbye", "stop", "cancel"];
    if tokens
        .iter()
        .any(|t| CLOSING.contains(t) || reasoning_is_negator(t))
    {
        return false; // user is wrapping up / negating — don't stick
    }
    const OPENERS: &[&str] = &["and", "also", "then", "next", "again", "now", "plus", "minus"];
    if tokens.first().is_some_and(|t| OPENERS.contains(t)) {
        return true;
    }
    const CUES: &[&[&str]] = &[
        &["what", "about"],
        &["how", "about"],
        &["the", "second"],
        &["the", "other"],
        &["the", "next"],
        &["do", "it"],
        &["same", "for"],
        &["what", "if"],
    ];
    for cue in CUES {
        let n = cue.len();
        if tokens.len() >= n {
            for s in 0..=tokens.len() - n {
                if &tokens[s..s + n] == *cue {
                    return true;
                }
            }
        }
    }
    false
}

/// Cheap heuristic (single &str scan): is this model id known to do extended
/// reasoning by default? Used to (a) advise against a reasoning model on the
/// spoken path and (b) disable eager speculation for it (D5 — eager + a reasoner
/// is lose-lose: each speculative fire pays full think-time, cancel-on-resume
/// wastes it). Single source of truth, reused by the config-time advisory.
pub fn is_reasoning_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
        || m.contains("deepseek-r1")
        || m.contains("-thinking")
        || m.contains("reasoner")
        || m.contains("qwq")
        // Adaptive-thinking-only families (opus-4.x, gemini-fable/mythos, fable-5)
        // reason by construction wherever they run — for the eager-disable +
        // voice-path advisory we treat them as reasoning models regardless of
        // transport (the conservative Anthropic-floor assumption; A10b only
        // makes the WIRE floor transport-specific, not this classification).
        || crate::core::llm::ReasoningEffort::floor_for_model(model, crate::core::llm::AdapterKind::Anthropic)
            != crate::core::llm::ReasoningEffort::Off
}

/// D3: the built-in action-phrase pool when the operator supplies none. Action
/// wording (never "um"/"hmm"), kept short (<~1s synthesized) so an uninterruptible
/// clip can't talk over a barging user.
pub const DEFAULT_FILLER_PHRASES: &[&str] = &[
    "Let me check that.",
    "One moment.",
    "Let me look into that.",
    "Give me a moment.",
    "Just a second.",
];

/// Default `max_tokens` cap for the VOICE path when the operator doesn't set
/// one (X-G1). A spoken reply of ~30s is ~75 words (~100 tokens); 256 leaves
/// headroom for multilingual scripts and longer answers while preventing a
/// verbose model from generating essays the user must sit through (and pay
/// for). Explicit `max_tokens` always wins. Non-voice paths (DAG JSON flows)
/// are unaffected — this cap lives in the conversation config only.
pub const VOICE_DEFAULT_MAX_TOKENS: u32 = 256;

impl ConversationConfig {
    /// A10b: the resolved wire vendor for THIS config (the same `select_adapter`
    /// resolver `LlmClient::new` uses), so the reasoning floor is honest per
    /// transport — a Claude model fronted by an OpenAI-compatible proxy renders
    /// (and floors) as OpenAI, not Anthropic.
    fn adapter_kind(&self) -> crate::core::llm::AdapterKind {
        let probe = LlmClientConfig {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            provider_kind: self.provider_kind,
            ..Default::default()
        };
        crate::core::llm::adapter::select_adapter(&probe).kind()
    }

    fn to_client_config(&self) -> LlmClientConfig {
        LlmClientConfig {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
            // D3 double-ack guard (critique H2): when masking is on, the gateway
            // may itself speak a "one moment" filler — so instruct the model NOT
            // to open with its own filler, or the caller hears it twice.
            system_prompt: self.system_prompt_with_filler_guard(),
            temperature: self.temperature,
            // X-G1: cap completion length for voice unless explicitly set.
            max_tokens: self.max_tokens.or(Some(VOICE_DEFAULT_MAX_TOKENS)),
            streaming: self.streaming,
            max_history: self.max_history,
            provider_kind: self.provider_kind,
            // D1: send the floor-clamped value (an adaptive-only model can't
            // honor a lower request; the floor is echoed back at config time).
            // A10b: the floor respects the resolved wire vendor.
            reasoning_effort: crate::core::llm::ReasoningEffort::resolve(
                &self.model,
                self.adapter_kind(),
                self.reasoning_effort,
            )
            .0,
            ..Default::default()
        }
    }

    /// D3: the system prompt, plus a one-line "answer directly, don't open with a
    /// filler" guard when masking is enabled (prevents the gateway-filler +
    /// model-filler double-ack). Identity when masking is off.
    fn system_prompt_with_filler_guard(&self) -> Option<String> {
        const GUARD: &str = "Answer directly and concisely. Do not begin your reply with \
            filler such as \"let me check\", \"one moment\", or \"sure\" — a brief holding \
            phrase is already spoken for you when needed.";
        if !self.latency_filler.enabled() {
            return self.system_prompt.clone();
        }
        Some(match &self.system_prompt {
            Some(p) if !p.trim().is_empty() => format!("{p}\n\n{GUARD}"),
            _ => GUARD.to_string(),
        })
    }

    /// D1: the reasoning effort actually applied + the model's floor, for the
    /// session-ack echo (so a clamped/adaptive-only model is observable).
    pub fn resolved_reasoning_effort(
        &self,
    ) -> (
        Option<crate::core::llm::ReasoningEffort>,
        crate::core::llm::ReasoningEffort,
    ) {
        crate::core::llm::ReasoningEffort::resolve(
            &self.model,
            self.adapter_kind(),
            self.reasoning_effort,
        )
    }
}

/// The built-in conversation orchestrator.
///
/// Constructed per session; cloned into the VoiceManager's STT callback so each
/// finalized turn drives the LLM→TTS loop. Cheap to clone (all `Arc`s).
#[derive(Clone)]
pub struct ConversationOrchestrator {
    session_id: String,
    llm: Arc<LlmClient>,
    voice_manager: Arc<VoiceManager>,
    config: Arc<ConversationConfig>,
    /// In-flight LLM turn token + its generation id.
    turn: Arc<SyncMutex<Option<(u64, CancellationToken)>>>,
    /// Monotonic turn id allocator.
    next_turn_id: Arc<std::sync::atomic::AtomicU64>,
    /// In-flight EAGER (speculative) turn, if any (P1.2b).
    eager: Arc<SyncMutex<Option<EagerTurn>>>,
    /// A-G7 idle generation: bumped on EVERY activity; an armed idle task
    /// only fires while its captured generation is current (any activity
    /// disarms stale timers structurally — no cancellation plumbing).
    idle_generation: Arc<std::sync::atomic::AtomicU64>,
    /// D-G3: invoked (once) when a turn error classifies FATAL — the
    /// embedder surfaces it to the client / ends the session. None = log
    /// only (tests, direct embedding).
    fatal_handler: Arc<SyncMutex<Option<Arc<dyn Fn(String) + Send + Sync>>>>,
    /// A-G5 greeting-guard latch (review wf_d43814c3 #4): flipped true the
    /// first time the bot completes a spoken turn, so `MuteUntilFirstBotComplete`
    /// opens for a silently-listening user. None = no greeting guard wired.
    first_bot_complete_latch: Arc<SyncMutex<Option<Arc<std::sync::atomic::AtomicBool>>>>,
    /// D-G3 (review wc71hewlx #3): set once a turn classifies FATAL. Stops
    /// further turns from running — every subsequent utterance would hammer
    /// the same dead key/config, burning latency and money on silence.
    fatal_stopped: Arc<std::sync::atomic::AtomicBool>,
    /// D3: set true when a latency-masking utterance was spoken this turn (the
    /// one-per-turn latch + the recovery-line trigger). Reset in `begin_turn`.
    masking_fired: Arc<std::sync::atomic::AtomicBool>,
    /// D3: the in-flight masking timer task, aborted on first audio / barge-in /
    /// turn end so it never fires after the turn is done.
    masking_handle: Arc<SyncMutex<Option<tokio::task::JoinHandle<()>>>>,
    /// D3: round-robin index into the filler phrase pool (gentle rotation so the
    /// same phrase doesn't repeat back-to-back).
    masking_phrase_idx: Arc<std::sync::atomic::AtomicUsize>,
    /// S1/S2: the slow reasoning tier, sharing history with `llm`. `None` =
    /// single-tier. Built from `config.reasoning_model` via `with_tier_overrides`.
    reasoning_llm: Option<Arc<LlmClient>>,
    /// A5: 1-bit ledger of whether the PREVIOUS turn ran on the reasoning tier,
    /// so a short anaphoric follow-up can stick to the reasoning thread for one
    /// turn (set unconditionally in `select_tier` on every two-tier turn).
    last_turn_was_reasoning: Arc<SyncMutex<bool>>,
}

/// S1/S2: build the reasoning tier (sharing `base.histories`) when configured.
fn build_reasoning_tier(base: &Arc<LlmClient>, config: &ConversationConfig) -> Option<Arc<LlmClient>> {
    let model = config.reasoning_model.clone()?;
    // The reasoning tier needs room to ACTUALLY think — independent of the fast
    // tier's voice max_tokens cap (256), which on Anthropic suppresses the
    // thinking block entirely (it needs budget ≥ 1024 < max_tokens). Default to a
    // generous reasoning budget; an explicit P2 ceiling (max_reasoning_tokens) or
    // an explicit global max_tokens override it (the P2 ceiling stays the value).
    let reasoning_max_tokens = config
        .max_reasoning_tokens
        .or(config.max_tokens)
        .unwrap_or(REASONING_DEFAULT_MAX_TOKENS);
    Some(Arc::new(base.with_tier_overrides(
        model,
        config.reasoning_base_url.clone(),
        config.reasoning_api_key.clone(),
        config.reasoning_provider_kind,
        // The reasoning tier wants to actually reason — let it (no forced Off).
        config.reasoning_effort.or(Some(crate::core::llm::ReasoningEffort::Low)),
        Some(reasoning_max_tokens),
    )))
}

/// S3 (M6): may an async-tool result be VOLUNTEERED now? Only when the line is
/// idle (`!turn_active`) and the spawning turn is still the latest — i.e. no new
/// turn has begun since the tool was spawned (`next_turn_id == spawn + 1`, since
/// `begin_turn` hands out `spawn` then advances `next` to `spawn + 1`). Any newer
/// turn ⇒ a new topic ⇒ record-only (never talk over it).
fn followup_allowed(spawn_turn_id: u64, next_turn_id: u64, turn_active: bool) -> bool {
    !turn_active && next_turn_id == spawn_turn_id.saturating_add(1)
}

/// S3: char-boundary-safe truncation for the history note (avoids unbounded
/// tool output bloating the context); appends an ellipsis when clipped.
fn truncate_for_note(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// A speculative LLM turn started on a turn-complete PREDICTION, before the
/// provider's speech_final. History is staged (never mutated); the reply is
/// held (no TTS) until the final confirms the transcript.
struct EagerTurn {
    transcript: String,
    token: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    response: Arc<SyncMutex<Option<Result<String, ()>>>>,
}

impl std::fmt::Debug for ConversationOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationOrchestrator")
            .field("session_id", &self.session_id)
            .field("model", &self.config.model)
            .finish()
    }
}

impl ConversationOrchestrator {
    /// Create a new orchestrator for `session_id`.
    ///
    /// Validates the LLM `base_url` for SSRF (resolve-then-validate, with the
    /// `WAAV_ALLOW_LOOPBACK_ENDPOINTS=1` test escape hatch) before building the
    /// client, since `base_url` is client-supplied.
    pub fn new(
        session_id: impl Into<String>,
        config: ConversationConfig,
        voice_manager: Arc<VoiceManager>,
    ) -> Result<Self, ConversationOrchestratorError> {
        validate_llm_url(&config.base_url)
            .map_err(ConversationOrchestratorError::InvalidLlmUrl)?;
        // S1/S2: the reasoning tier's base_url is ALSO client-supplied — validate
        // it for SSRF before it is ever used for a request.
        if let Some(rb) = &config.reasoning_base_url {
            validate_llm_url(rb).map_err(ConversationOrchestratorError::InvalidLlmUrl)?;
        }

        let llm = Arc::new(LlmClient::new(config.to_client_config()));
        let reasoning_llm = build_reasoning_tier(&llm, &config);
        Ok(Self {
            session_id: session_id.into(),
            llm,
            reasoning_llm,
            last_turn_was_reasoning: Arc::new(SyncMutex::new(false)),
            voice_manager,
            config: Arc::new(config),
            turn: Arc::new(SyncMutex::new(None)),
            next_turn_id: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            eager: Arc::new(SyncMutex::new(None)),
            idle_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            fatal_handler: Arc::new(SyncMutex::new(None)),
            first_bot_complete_latch: Arc::new(SyncMutex::new(None)),
            fatal_stopped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            masking_fired: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            masking_handle: Arc::new(SyncMutex::new(None)),
            masking_phrase_idx: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }

    /// Construct from a pre-built `LlmClient` (used in tests to inject a mock
    /// endpoint that bypasses SSRF). The orchestrator otherwise behaves
    /// identically.
    pub fn with_client(
        session_id: impl Into<String>,
        config: ConversationConfig,
        llm: Arc<LlmClient>,
        voice_manager: Arc<VoiceManager>,
    ) -> Self {
        let reasoning_llm = build_reasoning_tier(&llm, &config);
        Self {
            session_id: session_id.into(),
            llm,
            reasoning_llm,
            last_turn_was_reasoning: Arc::new(SyncMutex::new(false)),
            voice_manager,
            config: Arc::new(config),
            turn: Arc::new(SyncMutex::new(None)),
            next_turn_id: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            eager: Arc::new(SyncMutex::new(None)),
            idle_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            fatal_handler: Arc::new(SyncMutex::new(None)),
            first_bot_complete_latch: Arc::new(SyncMutex::new(None)),
            fatal_stopped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            masking_fired: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            masking_handle: Arc::new(SyncMutex::new(None)),
            masking_phrase_idx: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// The shared LLM client (for history inspection / cleanup).
    pub fn llm(&self) -> &Arc<LlmClient> {
        &self.llm
    }

    /// S2: pick the LLM tier for this turn. The reasoning tier is used only when
    /// configured AND (route=Always OR the heuristic flags the turn complex);
    /// otherwise the fast `model`. Both tiers SHARE history, so escalation
    /// continues the same conversation.
    /// Pick the tier for this turn. Returns `(tier, is_reasoning)` so the caller
    /// can apply the reasoning-tier first-audio budget (P1) and pick the correct
    /// degradation fallback (the *other* tier).
    fn select_tier(&self, transcript: &str) -> (&Arc<LlmClient>, bool) {
        if let Some(reasoning) = &self.reasoning_llm {
            let escalate = match self.config.reasoning_route {
                RoutingMode::Always => true,
                RoutingMode::Auto => {
                    let prev = *self.last_turn_was_reasoning.lock();
                    // INTRINSIC complexity (keyword/length/%), context-free — this
                    // alone ARMS stickiness for the next turn. A5: arm the ledger
                    // on `intrinsic` only, so stickiness is a true ONE-SHOT (a
                    // sticky continuation runs on reasoning THIS turn but does not
                    // re-arm). Writing the sticky `escalate` instead would let a
                    // chain of "and…"/"now…" follow-ups latch reasoning forever.
                    *self.last_turn_was_reasoning.lock() = turn_needs_reasoning(transcript);
                    // Escalate this turn on intrinsic complexity OR a one-shot
                    // sticky continuation of a prior reasoning turn.
                    turn_needs_reasoning_ctx(transcript, prev)
                }
            };
            if escalate {
                debug!(session = %self.session_id, "S2: escalating turn to the reasoning tier");
                return (reasoning, true);
            }
        }
        (&self.llm, false)
    }

    /// P1 degradation ladder: the tier to fall back to when `primary` fails or
    /// runs over its first-audio budget. The fast tier falls back to the reasoner
    /// (if configured) and vice-versa; single-tier configs have no fallback.
    fn fallback_tier(&self, primary_is_reasoning: bool) -> Option<&Arc<LlmClient>> {
        if primary_is_reasoning {
            Some(&self.llm)
        } else {
            self.reasoning_llm.as_ref()
        }
    }

    /// P1: speak a graceful degraded response when the primary tier failed and
    /// nothing has been spoken this turn. Tries the fallback tier ONCE
    /// (non-streaming — correctness over latency on the exceptional path), and
    /// if that also yields nothing, speaks the canned apology so the caller is
    /// never in silence. Records `waav_degraded_total`.
    ///
    /// Returns `true` only when the fallback tier produced a real answer (the
    /// turn genuinely RECOVERED). Returns `false` when it fell through to the
    /// canned apology — the caller still propagates the original error so the
    /// D-G3 fatal/recoverable classifier can decide whether to stop the session
    /// (a dead API key must not be retried every turn). The apology is spoken
    /// either way, so a fatal-stop is never silent.
    async fn speak_degraded(
        &self,
        primary_is_reasoning: bool,
        epoch: usize,
        allow_interruption: bool,
        token: &CancellationToken,
        spoke: bool,
    ) -> bool {
        // A real barge-in already cleared this turn — degrading would talk over
        // the caller. The epoch guard below is the final backstop.
        if token.is_cancelled() {
            return false;
        }
        // A REAL answer already streamed to TTS this turn (mid-stream provider
        // error after partial audio) — degrading would talk over it. The masking
        // filler never sets `spoke`, so degrade-after-filler still recovers.
        if spoke {
            return false;
        }
        // The masking filler (if any) has served its purpose; stop it so it
        // cannot fire on top of the degraded answer.
        self.abort_masking();

        // Rung 1: the other tier. The failed primary already STAGED the user
        // message into the shared history (prepare_messages runs at request
        // time), so the fallback CONTINUES from history — never re-appending the
        // user turn (no duplicate) — and resolves its OWN credential (None
        // per-call key, so a cross-provider reasoning tier uses reasoning_api_key,
        // not the fast key). Time-boxed so a stuck fallback can't reintroduce the
        // dead air P1 exists to remove.
        if let Some(fallback) = self.fallback_tier(primary_is_reasoning) {
            let fb_token = CancellationToken::new();
            let bound_ms = if self.config.reasoning_budget_ms > 0 {
                self.config.reasoning_budget_ms
            } else {
                DEFAULT_REASONING_BUDGET_MS
            };
            let fb_result = tokio::time::timeout(
                Duration::from_millis(bound_ms),
                fallback.continue_from_history(&self.session_id, None, &fb_token, None),
            )
            .await;
            match fb_result {
                Ok(Ok(resp)) if !resp.content.trim().is_empty() => {
                    let content = crate::core::text::strip_think(&resp.content);
                    let text = if self.config.strip_markdown {
                        crate::core::text::strip_markdown_for_tts(&content)
                    } else {
                        content
                    };
                    crate::core::metrics::bridge::record_degraded(
                        "conversation",
                        if primary_is_reasoning {
                            "reasoning_tier_to_fast"
                        } else {
                            "fast_tier_to_reasoning"
                        },
                    );
                    warn!(
                        session = %self.session_id,
                        primary_is_reasoning,
                        "P1: primary LLM tier failed — degraded to the fallback tier"
                    );
                    let _ = self
                        .voice_manager
                        .speak_if_epoch(&text, true, allow_interruption, epoch)
                        .await;
                    return true; // recovered on the fallback tier
                }
                Ok(other) => {
                    debug!(
                        session = %self.session_id,
                        ok = other.is_ok(),
                        "P1: fallback tier also failed/empty — using canned apology"
                    );
                }
                Err(_elapsed) => {
                    fb_token.cancel(); // stop the stuck fallback request
                    warn!(
                        session = %self.session_id,
                        bound_ms, "P1: fallback tier exceeded its budget — using canned apology"
                    );
                }
            }
        }

        // Rung 2: the canned apology — never dead air. We did NOT recover, so the
        // caller still surfaces the original error to the fatal/recoverable
        // classifier (the apology has already been spoken).
        crate::core::metrics::bridge::record_degraded("conversation", "all_tiers_failed");
        warn!(session = %self.session_id, "P1: all LLM tiers failed — speaking canned apology");
        let msg = self
            .config
            .degradation_message
            .as_deref()
            .unwrap_or(DEFAULT_DEGRADATION_MESSAGE);
        let _ = self
            .voice_manager
            .speak_if_epoch(msg, true, allow_interruption, epoch)
            .await;
        false
    }

    /// S3 (REALTIME_REASONING.md §5): wire the async-tool final sink. When an
    /// async tool (`cancel_on_interruption=false`) delivers its result, it lands
    /// here. Captures a `Weak<Self>` so the detached delivery task can re-enter
    /// the orchestrator without keeping it alive past teardown. The fast and
    /// reasoning tiers SHARE one registry, so wiring once covers both.
    ///
    /// REACHABILITY (FOLLOWUP §2.4): the WS conversation config has NO tool-
    /// registration surface today (`LlmClient::with_functions` has no conv-path
    /// caller), so on the conversation path `self.llm.functions()` is `None` and
    /// this is a NO-OP — async tools are currently reachable only via the DAG LLM
    /// node. Wiring the full webhook-tools surface onto the conversation config is
    /// a scoped opt-in feature (SSRF resolve-and-pin, cross-vendor tool rendering)
    /// gated on an operator request, deferred per the follow-up plan. The sink
    /// machinery below is correct and unit-tested for when that surface lands.
    pub fn wire_async_sink(self: &Arc<Self>) {
        let Some(registry) = self.llm.functions() else {
            // Honest no-op (not a silent one): no registry ⇒ no async tools on
            // this path. See the reachability note above.
            debug!(
                session = %self.session_id,
                "S3 async-tool sink not wired: no tool registry on the conversation path \
                 (async tools are DAG-only today — FOLLOWUP §2.4)"
            );
            return;
        };
        let weak = Arc::downgrade(self);
        registry.set_async_sink(Arc::new(move |r: crate::core::llm::AsyncToolResult| {
            let weak = weak.clone();
            tokio::spawn(async move {
                if let Some(this) = weak.upgrade() {
                    this.handle_async_final(r).await;
                }
            });
        }));
    }

    /// S3: an async tool delivered a result (the public entry point — also wired
    /// automatically via [`Self::wire_async_sink`]). ALWAYS records it to history
    /// (the model must see the result on its next turn — it is never lost), then,
    /// turn-id-gated, optionally VOLUNTEERS it: the bot speaks a follow-up only
    /// while the spawning turn is still the latest and the line is idle (M6 — a
    /// stale RAG answer must never talk over a new topic).
    pub async fn handle_async_final(&self, r: crate::core::llm::AsyncToolResult) {
        if !r.is_final {
            return; // progress updates are not volunteered
        }
        // Record-to-history (always): a compact, model-readable note. Inserted
        // PAIRING-SAFE so it can never wedge between a concurrent turn's
        // assistant{tool_calls} and its tool_result (strict-provider 400 brick).
        let note = format!(
            "(Async tool '{}' completed. Result: {})",
            r.name,
            truncate_for_note(&r.value.to_string(), 600)
        );
        self.llm
            .append_context_pairing_safe(
                &r.session_id,
                vec![crate::core::llm::ChatMessage::system(note)],
            )
            .await;

        // run_llm=false ⇒ record-only (chain tools without re-inference).
        if !r.run_llm {
            crate::core::metrics::bridge::record_async_tool("record_only");
            return;
        }
        // Turn-id gate + turn install, ATOMIC under the turn lock so a real user
        // turn that starts concurrently is never preempted.
        match self.try_begin_followup_turn(r.turn_id) {
            None => {
                // Superseded (new topic) or mid-turn — do not talk over it. The
                // result is already in history for the next natural turn.
                crate::core::metrics::bridge::record_async_tool("gated");
                debug!(
                    session = %self.session_id,
                    tool = %r.name,
                    spawn_turn = r.turn_id,
                    "S3: async result gated (conversation moved on) — recorded, not spoken"
                );
            }
            Some((id, token)) => {
                self.speak_async_followup(id, token).await;
            }
        }
    }

    /// S3: install a follow-up turn for an async result IFF the conversation has
    /// not moved on since the tool was spawned — done under the turn lock so the
    /// decision and the install are atomic (no race with a real user turn).
    /// Returns the new turn's `(id, token)`, or `None` to record-only.
    fn try_begin_followup_turn(&self, spawn_turn_id: u64) -> Option<(u64, CancellationToken)> {
        let mut guard = self.turn.lock();
        let next = self.next_turn_id.load(std::sync::atomic::Ordering::SeqCst);
        if !followup_allowed(spawn_turn_id, next, guard.is_some()) {
            return None;
        }
        let id = self
            .next_turn_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let token = CancellationToken::new();
        *guard = Some((id, token.clone()));
        Some((id, token))
    }

    /// S3: speak the async-tool follow-up. Completes from the (just-augmented)
    /// history and speaks the result as an ordinary interruptible turn — a
    /// barge-in cancels it like any other (its token lives in `self.turn`).
    async fn speak_async_followup(&self, id: u64, token: CancellationToken) {
        let epoch = self.voice_manager.clear_epoch();
        let result = self
            .llm
            .continue_from_history(&self.session_id, self.config.api_key.as_deref(), &token, None)
            .await;
        match result {
            Ok(resp) if !resp.content.trim().is_empty() => {
                let content = crate::core::text::strip_think(&resp.content);
                let text = if self.config.strip_markdown {
                    crate::core::text::strip_markdown_for_tts(&content)
                } else {
                    content
                };
                crate::core::metrics::bridge::record_async_tool("spoke");
                let _ = self
                    .voice_manager
                    .speak_if_epoch(&text, true, self.config.allow_interruption, epoch)
                    .await;
            }
            Ok(_) => debug!(session = %self.session_id, "S3 follow-up produced no content"),
            Err(LlmError::Cancelled) => {
                debug!(session = %self.session_id, "S3 follow-up cancelled (barge-in)")
            }
            Err(e) => warn!(session = %self.session_id, error = %e, "S3 follow-up inference failed"),
        }
        self.end_turn(id);
    }

    /// Cancel any in-flight LLM turn. Returns true if one was running.
    fn cancel_current_turn(&self) -> bool {
        let mut guard = self.turn.lock();
        if let Some((_, token)) = guard.take() {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Begin a new turn: cancel the previous one and install a fresh token.
    fn begin_turn(&self) -> (u64, CancellationToken) {
        let id = self
            .next_turn_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let token = CancellationToken::new();
        // D3: a fresh turn starts with no masking spoken, and any stale masking
        // timer from a prior turn is torn down.
        self.masking_fired
            .store(false, std::sync::atomic::Ordering::Release);
        self.abort_masking();
        let mut guard = self.turn.lock();
        if let Some((_, prev)) = guard.take() {
            prev.cancel();
        }
        *guard = Some((id, token.clone()));
        (id, token)
    }

    /// Mark a turn finished iff it is still the current one.
    fn end_turn(&self, id: u64) {
        let mut guard = self.turn.lock();
        if matches!(guard.as_ref(), Some((cur, _)) if *cur == id) {
            *guard = None;
        }
    }

    /// D3: abort the in-flight masking timer (no-op if none). Called when first
    /// audio arrives, on barge-in, and at turn end so a filler never fires late.
    fn abort_masking(&self) {
        if let Some(h) = self.masking_handle.lock().take() {
            h.abort();
        }
    }

    /// D3: pick the next masking phrase (operator pool or the built-in default),
    /// rotating so the same phrase doesn't repeat back-to-back.
    fn next_filler_phrase(&self) -> Option<String> {
        let custom = &self.config.latency_filler_phrases;
        let len = if custom.is_empty() {
            DEFAULT_FILLER_PHRASES.len()
        } else {
            custom.len()
        };
        if len == 0 {
            return None;
        }
        let i = self
            .masking_phrase_idx
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % len;
        Some(if custom.is_empty() {
            DEFAULT_FILLER_PHRASES[i].to_string()
        } else {
            custom[i].clone()
        })
    }

    /// React to a barge-in (the turn controller already passed the MinWords /
    /// mute gate by emitting `Started { interrupt: true }`): cancel the in-flight
    /// LLM/eager turn and clear queued/playing TTS.
    ///
    /// CRITICAL (REALTIME_REASONING.md §4.4 / critique C3): the COMPUTE cancel
    /// (LLM + eager) happens UNCONDITIONALLY — a barge-in during a protected
    /// (filler/uninterruptible) window must still stop a slow reasoning turn, not
    /// let it burn for the clip's whole duration. The protected-window guard
    /// governs only the AUDIO: `clear_tts` self-protects an in-flight
    /// uninterruptible clip (the A-G6 selective path keeps it and skips the epoch
    /// bump; the legacy path's `can_interrupt()` gate skips the blanket clear
    /// inside the window) — so a disclaimer/filler still finishes playing while
    /// the LLM is cancelled and the queued interruptible audio is dropped.
    pub async fn handle_barge_in(&self) {
        // Cancel compute FIRST, before any protected-window check.
        let had_turn = self.cancel_current_turn();
        // D3: tear down a pending masking timer so no filler fires post-barge-in.
        self.abort_masking();
        if let Some(eager) = self.eager.lock().take() {
            eager.token.cancel();
            debug!(session = %self.session_id, "barge-in cancelled eager speculative turn");
        }
        // Clear queued/in-flight TTS. This is safe to call unconditionally: it
        // preserves a still-playing uninterruptible clip and no-ops the blanket
        // clear when the window forbids it, while dropping interruptible audio.
        if let Err(e) = self.voice_manager.clear_tts().await {
            warn!(session = %self.session_id, error = %e, "clear_tts during barge-in failed");
        }
        debug!(session = %self.session_id, had_turn, "barge-in handled");
    }

    /// Run one LLM turn for `transcript` and stream the reply to TTS.
    ///
    /// This is the core of the loop: cancel any prior turn, then call the LLM
    /// (streaming) with the per-session history and pipe tokens to
    /// `VoiceManager::speak`. On a non-streaming config, the full reply is spoken
    /// once. Cancellation (barge-in/teardown) aborts promptly.
    pub async fn run_turn(&self, transcript: &str) -> Result<(), ConversationOrchestratorError> {
        let (id, token) = self.begin_turn();
        // S2: select the fast or reasoning tier for this turn (both share
        // history; the D3 filler masks the reasoning tier's latency).
        let (llm, is_reasoning) = self.select_tier(transcript);
        // Barge-in clear epoch (review wf_85659e16 #5): captured ONCE at
        // turn start; any clear during the turn invalidates every later
        // sentence enqueue of this turn — checked under the TTS lock, so a
        // pump that lost the lock race to the clear can never leak audio.
        let epoch = self.voice_manager.clear_epoch();

        // Stream tokens to TTS aggregated at SENTENCE boundaries. Per-token
        // `speak()` is pathological: for HTTP TTS providers it is one synthesis
        // request per token; for WebSocket providers per-token flushes ruin
        // prosody. Aggregation keeps latency (first sentence speaks while the
        // LLM still generates) without either failure mode.
        let vm = self.voice_manager.clone();
        let allow_interruption = self.config.allow_interruption;
        let streaming = self.config.streaming;
        let observers = self.voice_manager.observers();

        // Per-turn latency anchor: LLM request starts now.
        if let Some(obs) = &observers {
            obs.notify_llm_request(crate::core::observability::now_monotonic_ns());
        }

        // True once the pump actually delivered any text to TTS (guards the
        // reasoning-model empty-stream failure mode).
        let spoke = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // A7: monotonic ns of the LAST streaming PROGRESS — the last non-empty
        // token delta (0 = none yet). The stall watchdog measures the gap from
        // here (or the request start), so it tracks reasoner LIVENESS, not TTS
        // emit timing: a reasoner streaming tokens (even a long not-yet-spoken
        // sentence, or one whose TTS synthesis is slow) is alive and not
        // degraded, while one that stops producing tokens is. Non-streaming turns
        // never advance it (no deltas) → the watchdog measures from request start,
        // preserving the original first-audio (TTFA) degrade.
        let last_progress_at = Arc::new(std::sync::atomic::AtomicU64::new(0));

        // D3 (REALTIME_REASONING.md §4.3): latency masking. Arm a SINGLE timer on
        // this CONFIRMED-EoT path (never on the speculative/eager path). If no
        // audio has reached TTS by the wait threshold, speak ONE short action
        // phrase (interruptible) to keep the line alive while the LLM/RAG/tool
        // runs IN PARALLEL (it is already dispatched — the timer never defers it).
        // The latch makes it one-utterance-per-turn; first-audio / barge-in / turn
        // end abort it. Off ⇒ zero added work (no task spawned).
        if self.config.latency_filler.enabled() {
            let wait_ms = self
                .config
                .latency_filler
                .wait_ms(self.config.latency_filler_after_ms);
            if let Some(phrase) = self.next_filler_phrase() {
                let vm = self.voice_manager.clone();
                let spoke_t = spoke.clone();
                let token_t = token.clone();
                let latch = self.masking_fired.clone();
                let session = self.session_id.clone();
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                    // Re-check at fire time: not cancelled, no audio yet, not
                    // already fired, and the bot isn't already speaking.
                    if token_t.is_cancelled() || spoke_t.load(std::sync::atomic::Ordering::Acquire)
                    {
                        return;
                    }
                    if latch.swap(true, std::sync::atomic::Ordering::AcqRel) {
                        return; // one masking utterance per turn
                    }
                    // Speak the filler as ORDINARY interruptible audio (epoch-gated
                    // so a barge-in that already cleared drops it). We must NOT flip
                    // the session-global interruption flag here: the filler is
                    // interruptible by design, and a real barge-in during it (or
                    // during the real answer that follows) must be honored — the
                    // brutal review proved that poisoning allow_interruption here
                    // silently disabled barge-in for the WHOLE answer (critique:
                    // re-introduced the §4.4 failure). Echo cancellation, not a
                    // session-wide suppression window, handles the bot's own tail.
                    match vm.speak_if_epoch(&phrase, true, true, epoch).await {
                        Ok(true) => debug!(session = %session, %phrase, "D3 masking filler spoken"),
                        Ok(false) => {
                            // Cleared before we could speak — undo the latch so a
                            // recovery line can still fire if content is empty.
                            latch.store(false, std::sync::atomic::Ordering::Release);
                        }
                        Err(e) => {
                            warn!(session = %session, error = %e, "D3 masking filler failed");
                            latch.store(false, std::sync::atomic::Ordering::Release);
                        }
                    }
                });
                *self.masking_handle.lock() = Some(handle);
            }
        }

        // B-G5: every streamed delta accumulates here so a barge-in can
        // commit the PARTIAL reply to history — the model's next turn must
        // know it was cut off (Pipecat parity; prevents repeat/contradict).
        let streamed_text: Arc<SyncMutex<String>> = Arc::new(SyncMutex::new(String::new()));

        let (on_token, pump) = if streaming {
            let token_for_cb = token.clone();
            // Channel from the (sync) token callback to an async pump that calls speak().
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let first_token_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let obs_cb = observers.clone();
            let streamed_cb = Arc::clone(&streamed_text);
            let progress_cb = last_progress_at.clone();
            let cb: crate::core::llm::TokenCallback = Arc::new(move |delta: &str| {
                if !delta.is_empty() {
                    // A7: heartbeat reasoner LIVENESS on every token (before any
                    // aggregation/synthesis), so the watchdog never mistakes a
                    // long in-progress sentence or slow TTS for a stall.
                    progress_cb.store(
                        crate::core::observability::now_monotonic_ns(),
                        std::sync::atomic::Ordering::Release,
                    );
                    if !first_token_seen.swap(true, std::sync::atomic::Ordering::Relaxed)
                        && let Some(obs) = &obs_cb
                    {
                        obs.notify_llm_first_token(crate::core::observability::now_monotonic_ns());
                    }
                }
                streamed_cb.lock().push_str(delta);
                let _ = tx.send(delta.to_string());
            });

            // Pump task: aggregate deltas into sentences via the shared
            // aggregator (lookahead + decimal/abbrev disambiguation + 160-char
            // cap — core::text::SentenceAggregator, PIPECAT_FIX_PLAN C-G1+2),
            // speaking each completed sentence as soon as it is confirmed.
            let vm_pump = vm.clone();
            let spoke_pump = spoke.clone();
            let obs_pump = observers.clone();
            let strip_md = self.config.strip_markdown;
            let handle = tokio::spawn(async move {
                let mut agg = crate::core::text::SentenceAggregator::default();
                // Reasoning-model chain-of-thought (`<think>…</think>`) must NEVER
                // reach TTS — the caller would hear the model reasoning aloud.
                // Filter each delta BEFORE aggregation (handles tags split across
                // deltas); a non-reasoning model's stream passes through untouched.
                let mut think = crate::core::text::ThinkStripper::default();

                async fn speak_sentence(
                    vm: &VoiceManager,
                    text: String,
                    allow_interruption: bool,
                    epoch: usize,
                    strip_markdown: bool,
                    spoke: &std::sync::atomic::AtomicBool,
                    obs: &Option<Arc<crate::core::observability::ObserverRegistry>>,
                ) -> bool {
                    // C-G4: strip markdown on the COMPLETE sentence (the
                    // aggregator's unit — per-token would tear ** pairs).
                    let text = if strip_markdown {
                        crate::core::text::strip_markdown_for_tts(&text)
                    } else {
                        text
                    };
                    if text.trim().is_empty() {
                        return true;
                    }
                    if !spoke.swap(true, std::sync::atomic::Ordering::Relaxed)
                        && let Some(obs) = obs
                    {
                        obs.notify_llm_first_sentence(
                            crate::core::observability::now_monotonic_ns(),
                        );
                    }
                    match vm.speak_if_epoch(&text, true, allow_interruption, epoch).await {
                        Ok(true) => true,
                        Ok(false) => {
                            // A barge-in cleared TTS since this turn started:
                            // the rest of this turn's sentences are stale.
                            debug!("sentence dropped: barge-in cleared since turn start");
                            false
                        }
                        Err(e) => {
                            warn!(error = %e, "speak() during streamed turn failed");
                            false
                        }
                    }
                }

                'pump: loop {
                    tokio::select! {
                        biased;
                        _ = token_for_cb.cancelled() => break,
                        maybe = rx.recv() => match maybe {
                            Some(chunk) => {
                                // Drop chain-of-thought before it can be spoken.
                                let chunk = think.push(&chunk);
                                if chunk.is_empty() { continue; }
                                for sentence in agg.push_str(&chunk) {
                                    // Re-check cancellation BETWEEN sentences:
                                    // barge-in landing mid-loop must not speak
                                    // the remaining (stale) sentences into the
                                    // freshly cleared TTS queue.
                                    if token_for_cb.is_cancelled() {
                                        break 'pump;
                                    }
                                    if !speak_sentence(&vm_pump, sentence, allow_interruption, epoch, strip_md, &spoke_pump, &obs_pump).await {
                                        break 'pump;
                                    }
                                }
                            }
                            None => {
                                // Stream ended: flush any non-think tail held by
                                // the think filter into the aggregator, then speak
                                // the held remainder (a boundary awaiting lookahead
                                // must still fire) — unless the turn was cancelled.
                                let think_tail = think.flush();
                                if !think_tail.is_empty() {
                                    for sentence in agg.push_str(&think_tail) {
                                        if token_for_cb.is_cancelled() { break; }
                                        let _ = speak_sentence(&vm_pump, sentence, allow_interruption, epoch, strip_md, &spoke_pump, &obs_pump).await;
                                    }
                                }
                                if !token_for_cb.is_cancelled()
                                    && let Some(tail) = agg.flush()
                                {
                                    let _ = speak_sentence(&vm_pump, tail, allow_interruption, epoch, strip_md, &spoke_pump, &obs_pump).await;
                                }
                                break;
                            }
                        }
                    }
                }
            });
            (Some(cb), Some(handle))
        } else {
            (None, None)
        };

        // P1.b + A7: the reasoning tier's MAX-SILENCE-GAP watchdog. The common
        // (fast or single-tier) path is the bare await — zero added machinery.
        // An escalated reasoning turn with a positive budget gets an interval
        // watchdog that measures the gap since the LAST audio reached TTS (or the
        // request start when nothing has yet). If that gap exceeds the budget the
        // reasoner is cancelled (via a CHILD token so the turn token stays alive)
        // and the breach is marked. This subsumes BOTH "no first audio in N ms"
        // (TTFA) AND "audio started then froze N ms" (mid-stream stall) — a
        // reasoner that emits one token then hangs no longer leaves dead air. A
        // reasoner streaming steadily resets the gap on every sentence and never
        // trips.
        let budget_ms = if is_reasoning {
            self.config.reasoning_budget_ms
        } else {
            0
        };
        let mut budget_exceeded = false;
        let result = if budget_ms > 0 && self.reasoning_llm.is_some() {
            let reasoner_token = token.child_token();
            let req_start = crate::core::observability::now_monotonic_ns();
            let budget_ns = budget_ms.saturating_mul(1_000_000);
            let complete_fut = llm.complete(
                &self.session_id,
                transcript,
                self.config.api_key.as_deref(),
                &reasoner_token,
                on_token,
            );
            tokio::pin!(complete_fut);
            // Poll at most every STALL_POLL_MS so breach detection is prompt
            // without busy-looping; cap at the budget for tiny budgets.
            let poll = budget_ms.clamp(1, STALL_POLL_MS);
            let mut ticker = tokio::time::interval(Duration::from_millis(poll));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await; // consume the immediate first tick
            loop {
                tokio::select! {
                    biased;
                    r = &mut complete_fut => break r,
                    _ = ticker.tick() => {
                        let anchor = last_progress_at.load(std::sync::atomic::Ordering::Acquire);
                        let base = if anchor == 0 { req_start } else { anchor };
                        let gap = crate::core::observability::now_monotonic_ns()
                            .saturating_sub(base);
                        if gap >= budget_ns && !token.is_cancelled() {
                            budget_exceeded = true;
                            reasoner_token.cancel(); // stop the stuck/stalled reasoner
                            // complete_fut now resolves to Cancelled on the next poll.
                        }
                    }
                }
            }
        } else {
            llm.complete(
                &self.session_id,
                transcript,
                self.config.api_key.as_deref(),
                &token,
                on_token,
            )
            .await
        };

        // `complete()` returning drops the token callback → channel closes → the
        // pump flushes its remainder and exits. Await it so `spoke` is final and
        // tail text is delivered before we evaluate the empty-content guard.
        if let Some(handle) = pump {
            let _ = handle.await;
        }

        // B-G4: a tool-call response runs the server-side tool loop (every
        // batch's results land in history, then ONE re-inference) while the
        // turn is still active — the bot-busy probe must stay up through
        // tool execution, exactly like the thinking window. The loop's final
        // content lands on the normal speak path below (the streaming pump
        // saw no text for a tool-call response, so the !spoke fallback
        // speaks it).
        let mut ran_tool_loop = false;
        let result = match result {
            Ok(response)
                if !response.tool_calls.is_empty()
                    && llm.functions().is_some_and(|r| !r.is_empty()) =>
            {
                // The initial inference COMPLETED (recorded with its
                // preamble): the streamed accumulator is spent — without
                // this, a cancel inside the tool loop would commit the
                // preamble a SECOND time after the tool results (review
                // wf_6783a4b3: duplicate assistant text).
                streamed_text.lock().clear();
                // The pump spoke (at most) round-0 preamble; the loop's
                // FINAL answer must still reach TTS through the fallback
                // speak below (review wf_6783a4b3: a true `spoke` here left
                // the bot permanently silent after "let me check...").
                ran_tool_loop = true;
                let registry =
                    Arc::clone(llm.functions().expect("guarded by condition"));
                // P2: bound the per-turn re-inference rounds (the dominant spend
                // multiplier) by the configured cost budget. A call-me-again loop
                // can no longer run away on a billing gateway.
                let tool_opts = crate::core::llm::ToolLoopOptions {
                    max_rounds: self.config.max_llm_calls_per_turn as usize,
                    // S3: stamp this turn onto any async tool spawned in the loop,
                    // so its later final is turn-id-gated before being volunteered.
                    turn_id: id,
                    ..Default::default()
                };
                crate::core::llm::run_tool_loop(
                    llm,
                    &registry,
                    &self.session_id,
                    response,
                    self.config.api_key.as_deref(),
                    &token,
                    tool_opts,
                )
                .await
            }
            other => other,
        };

        // The turn stays ACTIVE through the fallback speak below — ending it
        // first re-opened the one-word-backchannel window between LLM
        // completion and TTS enqueue (review wf_85659e16, has_active_turn
        // gap #2).
        // NOTE: no `?` inside this block — every path must reach end_turn()
        // below or has_active_turn() sticks true and the MinWords gate jams.
        let outcome: Result<(), ConversationOrchestratorError> = match result {
            Ok(response) => {
                // Strip reasoning chain-of-thought before the content is spoken
                // OR judged empty: a reply that is ALL `<think>…</think>` and no
                // answer must count as empty (→ recovery line), not be spoken.
                let speakable = crate::core::text::strip_think(&response.content);
                let content_empty = speakable.trim().is_empty();
                let need_fallback_speak = if streaming {
                    // Reasoning-model guard: some models stream only reasoning
                    // deltas and deliver the answer (or nothing) at the end. If
                    // the pump spoke nothing, fall back to the final content; if
                    // that is empty too, say so loudly instead of going silent.
                    // A completed TOOL LOOP always speaks its final answer —
                    // the pump only ever saw the round-0 preamble.
                    ran_tool_loop || !spoke.load(std::sync::atomic::Ordering::Relaxed)
                } else {
                    true
                };
                if need_fallback_speak && !content_empty {
                    let speak_text = if self.config.strip_markdown {
                        crate::core::text::strip_markdown_for_tts(&speakable)
                    } else {
                        speakable.clone()
                    };
                    match self
                        .voice_manager
                        .speak_if_epoch(&speak_text, true, allow_interruption, epoch)
                        .await
                    {
                        Ok(true) => Ok(()),
                        Ok(false) => {
                            debug!(session = %self.session_id,
                                   "speak skipped: barge-in cleared since turn start");
                            Ok(())
                        }
                        Err(e) => Err(e.into()),
                    }
                } else {
                    if need_fallback_speak && content_empty && !token.is_cancelled() {
                        warn!(
                            session = %self.session_id,
                            "LLM turn produced NO speakable content (reasoning-only \
                             model or empty completion) — check the configured \
                             model/max_tokens"
                        );
                        // D3 recovery: if a masking filler already promised an
                        // answer ("one moment") and the turn then produced nothing,
                        // a spoken recovery line beats dead air (critique M3).
                        if self
                            .masking_fired
                            .load(std::sync::atomic::Ordering::Acquire)
                        {
                            let _ = self
                                .voice_manager
                                .speak_if_epoch(
                                    "Sorry, I didn't catch that — could you say it again?",
                                    true,
                                    allow_interruption,
                                    epoch,
                                )
                                .await;
                        }
                    }
                    Ok(())
                }
            }
            // LOW-10 guard: a budget breach cancels ONLY the child reasoner_token
            // (the parent stays alive), so `!token.is_cancelled()` excludes a real
            // barge-in (which cancels the parent) from being mis-handled as a
            // stall — it falls through to the plain Cancelled (barge-in) arm.
            Err(LlmError::Cancelled) if budget_exceeded && !token.is_cancelled() => {
                // P1.b/A7: NOT a barge-in — the reasoner blew its max-silence-gap
                // budget (no first audio, OR audio then a stall) and we cancelled
                // it via the child token (the turn token is still alive).
                let partial = streamed_text.lock().clone();
                let has_partial = !partial.trim().is_empty();
                if spoke.load(std::sync::atomic::Ordering::Acquire) && has_partial {
                    // A7: STALL after a coherent partial — do NOT restart (a fast
                    // draft would talk over / contradict it). Commit the partial
                    // so the model knows it was cut, and end the turn cleanly.
                    // (Guard on `has_partial`: a cleared accumulator must never
                    // commit an empty string while `spoke` suppresses degrade.)
                    self.llm
                        .commit_partial_assistant(&self.session_id, &partial)
                        .await;
                    crate::core::metrics::bridge::record_degraded(
                        "conversation",
                        "reasoning_stall_after_partial",
                    );
                    debug!(
                        session = %self.session_id,
                        "A7: reasoner stalled after partial audio — committed partial, no restart"
                    );
                    Ok(())
                } else {
                    // No audio yet (TTFA breach): degrade to the fast draft /
                    // canned apology rather than leaving the caller on a stuck
                    // reasoner. A deliberate budget cancel is never surfaced.
                    self.speak_degraded(
                        is_reasoning,
                        epoch,
                        allow_interruption,
                        &token,
                        spoke.load(std::sync::atomic::Ordering::Acquire),
                    )
                    .await;
                    Ok(())
                }
            }
            Err(LlmError::Cancelled) => {
                // B-G5: the streamed portion lands in history so the model
                // knows it was cut off. (Normal completion records the full
                // reply via record_assistant — the paths are mutually
                // exclusive, so no double-commit is possible.)
                let partial = streamed_text.lock().clone();
                if !partial.trim().is_empty() {
                    self.llm
                        .commit_partial_assistant(&self.session_id, &partial)
                        .await;
                    debug!(
                        session = %self.session_id,
                        partial_chars = partial.len(),
                        "barge-in: partial assistant reply committed to context"
                    );
                } else {
                    debug!(session = %self.session_id, "LLM turn cancelled (barge-in/teardown)");
                }
                Ok(())
            }
            Err(e) => {
                // P1.a: an LLM tier failed outright. Speak SOMETHING (fallback
                // tier, then a canned apology) so the caller never hears dead air.
                // If the fallback tier produced a real answer the turn genuinely
                // recovered (Ok); otherwise we still surface the original error so
                // the D-G3 classifier can fatal-stop a dead key (the apology has
                // already been spoken, so the stop is never silent).
                warn!(session = %self.session_id, error = %e, "P1: LLM turn failed — degrading");
                if self
                    .speak_degraded(
                        is_reasoning,
                        epoch,
                        allow_interruption,
                        &token,
                        spoke.load(std::sync::atomic::Ordering::Acquire),
                    )
                    .await
                {
                    Ok(())
                } else {
                    Err(e.into())
                }
            }
        };

        // D3: the turn is done — tear down the masking timer (a no-op if it
        // already fired or first audio aborted it; prevents a late stray filler).
        self.abort_masking();
        self.end_turn(id);

        // A-G5 (review wf_d43814c3 #4): the bot completed a turn that ran to
        // completion (not a barge-in cancel) — flip the greeting-guard latch
        // so `MuteUntilFirstBotComplete` opens, including for a silent
        // listener. `!token.is_cancelled()` covers both streaming and
        // non-streaming paths; an interrupted turn does not count.
        if outcome.is_ok()
            && !token.is_cancelled()
            && let Some(latch) = self.first_bot_complete_latch.lock().clone()
        {
            latch.store(true, std::sync::atomic::Ordering::Release);
        }

        // B-G6: compact the context AFTER the turn — DETACHED, so the up-to-5s
        // summary inference never blocks this run_turn (which is awaited on the
        // STT result-forwarding chain, review wf_d43814c3) and never extends
        // the bot-busy window. The CAS replace
        // ([`LlmClient::replace_context_if_unchanged`]) makes the concurrent-
        // turn race safe; failure is loud but never fatal.
        if self.config.summarize_target_tokens > 0 {
            let llm = Arc::clone(&self.llm);
            let session_id = self.session_id.clone();
            let target_tokens = self.config.summarize_target_tokens;
            let api_key = self.config.api_key.clone();
            tokio::spawn(async move {
                let cfg = crate::core::llm::SummaryConfig {
                    target_tokens,
                    ..Default::default()
                };
                if let Err(e) = llm
                    .maybe_summarize(&session_id, &cfg, api_key.as_deref(), &CancellationToken::new())
                    .await
                {
                    warn!(session = %session_id, error = %e, "context summarization failed");
                }
            });
        }

        outcome
    }

    /// Start an EAGER (speculative) LLM turn on a turn-complete PREDICTION
    /// (e.g. smart-turn firing before the provider's speech_final). P1.2b.
    ///
    /// History is STAGED (never mutated) and the reply is HELD — nothing is
    /// spoken and nothing committed until [`Self::on_stt_result`] receives a
    /// confirming final. If the user keeps talking, the speculation cancels
    /// with zero history pollution. One speculative turn at a time.
    pub fn trigger_eager_turn(&self, transcript: &str) {
        if !self.config.eager_eot {
            return;
        }
        // D5 (REALTIME_REASONING.md §4.5): eager speculation pairs with the FAST
        // model ONLY. On a reasoning model each speculative fire pays the full
        // multi-second think-time and a supersede-on-resume throws it away — pure
        // cost. Skip eager structurally for a reasoning model (the config-time
        // advisory tells the operator).
        if is_reasoning_model(&self.config.model) {
            return;
        }
        let text = transcript.trim();
        if text.is_empty() {
            return;
        }
        let mut guard = self.eager.lock();
        // A-G4 supersede-not-discard: predictions arrive as the user keeps
        // speaking. The OLD behavior pinned the FIRST prediction and ignored
        // later (fuller) ones, so a final that extended the first prediction
        // discarded the speculation and ran a full fresh turn — zero latency
        // win. Instead, when a new prediction EXTENDS the in-flight one (or
        // diverges to a longer transcript), CANCEL the stale speculation and
        // re-speculate on the fuller text — the latest prediction is the most
        // likely to match the final, maximizing eager confirmations.
        if let Some(existing) = guard.as_ref() {
            if existing.transcript == text {
                return; // already speculating on exactly this text
            }
            // Only supersede when the new text is a richer continuation
            // (avoids thrashing on a transient shorter interim). The
            // common case — the user appended words — is `text` starting
            // with the old transcript; also supersede any strictly longer
            // divergence.
            let is_extension = text.starts_with(&existing.transcript)
                || text.len() > existing.transcript.len();
            if !is_extension {
                return;
            }
            existing.token.cancel();
            existing.task.abort();
            debug!(session = %self.session_id,
                   "eager speculation superseded by a fuller prediction");
        }
        let token = CancellationToken::new();
        let response: Arc<SyncMutex<Option<Result<String, ()>>>> =
            Arc::new(SyncMutex::new(None));
        let llm = self.llm.clone();
        let session_id = self.session_id.clone();
        let api_key = self.config.api_key.clone();
        let text_owned = text.to_string();
        let response_store = response.clone();
        let task_token = token.clone();
        debug!(session = %self.session_id, "eager speculative turn started");
        let task = tokio::spawn(async move {
            let result = llm
                .complete_staged(&session_id, &text_owned, api_key.as_deref(), &task_token, None)
                .await;
            *response_store.lock() = Some(
                result
                    .map(|r| {
                        if r.tool_calls.is_empty() {
                            r.content
                        } else {
                            // A speculative TOOL turn is never confirmed:
                            // executing side effects on a prediction is
                            // wrong, and committing the text half without
                            // running the tools is worse. Empty content
                            // makes confirmation fall through to a real
                            // turn, where the tool loop runs (B-G4).
                            String::new()
                        }
                    })
                    .map_err(|_| ()),
            );
        });
        *guard = Some(EagerTurn {
            transcript: text.to_string(),
            token,
            task,
            response,
        });
    }

    /// Drive the orchestrator from a single STT result.
    ///
    /// - Any non-empty user speech (interim or final) is treated as a potential
    ///   barge-in: if the bot is mid-utterance and interruptible, cancel its turn
    ///   and clear TTS.
    /// - A **finalized** turn (`is_speech_final`) with content runs a new LLM
    ///   turn whose reply streams to TTS — unless a CONFIRMED eager speculation
    ///   already holds the reply, which is committed and spoken instead.
    pub async fn on_stt_result(&self, result: &STTResult) {
        // D-G3 (review wc71hewlx #3): once a turn classified FATAL, stop —
        // re-running turns against a dead key only burns latency and money.
        if self.fatal_stopped.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        // Derive the same events the legacy policy produces and route them
        // through the SINGLE event handler (A-G0): any non-empty speech is a
        // potential barge-in; a finalized turn with content runs the LLM.
        let turn_text = result.turn_transcript();
        let has_text = !turn_text.trim().is_empty();
        let mut events = Vec::new();
        if has_text {
            events.push(crate::core::turn::TurnEvent::Started { turn_id: 0, interrupt: true });
        }
        if result.is_speech_final && has_text {
            events.push(crate::core::turn::TurnEvent::Stopped {
                turn_id: 0,
                transcript: turn_text.trim().to_string(),
            });
        }
        self.handle_turn_events(&events).await;
    }

    /// Act on turn-decision events (from a [`crate::core::turn::TurnController`]
    /// or the [`Self::on_stt_result`] compatibility derivation) — the SINGLE
    /// place that owns the eager/barge-in ordering: the speculation is taken
    /// BEFORE barge-in handling (which cancels eager turns) whenever the
    /// batch finalizes a turn.
    pub async fn handle_turn_events(&self, events: &[crate::core::turn::TurnEvent]) {
        use crate::core::turn::TurnEvent;

        let finalizing = events.iter().any(|e| matches!(e, TurnEvent::Stopped { .. }));
        let mut eager = if finalizing { self.eager.lock().take() } else { None };

        for event in events {
            match event {
                TurnEvent::Started { interrupt: true, .. } => {
                    self.handle_barge_in().await;
                }
                TurnEvent::Started { .. } => {}
                TurnEvent::Speculate { .. } => {
                    // The signal rarely carries the full segment text; the
                    // VoiceManager's turn buffer is the source of truth.
                    let text = self.voice_manager.current_turn_text();
                    if !text.trim().is_empty() {
                        self.trigger_eager_turn(&text);
                    }
                }
                TurnEvent::Stopped { transcript, .. } => {
                    let transcript = transcript.trim().to_string();
                    if !transcript.is_empty() {
                        self.run_finalized_turn(&transcript, eager.take()).await;
                    }
                }
                TurnEvent::ResetAggregation => {
                    // Deliberately NO VoiceManager action (review wf_5772cd64
                    // #2): MinWords emits this only at a sub-threshold
                    // speech_final, and BOTH speech_final paths (forced fire
                    // + provider real) already reset the segment in the
                    // processor BEFORE delivery — an unscoped reset here
                    // executes after awaits and can wipe a NEW segment armed
                    // in between. The cough is already discarded; the event
                    // remains for observability/consumers that need it.
                }
                TurnEvent::BargeInMopUp => {
                    // The bot is STILL audible after the turn started (an
                    // in-flight speak resolved post-clear): re-run the
                    // idempotent barge-in (review wf_5772cd64 #5 — the legacy
                    // continuous-clear behavior, emitted only when needed).
                    self.handle_barge_in().await;
                }
                // MuteChanged lands with the mute strategies (A-G5).
                TurnEvent::MuteChanged { .. } => {}
            }
        }
        // A speculation taken for a batch whose Stopped carried no usable
        // transcript must not silently leak: put it back untouched.
        if let Some(e) = eager {
            *self.eager.lock() = Some(e);
        }
    }

    /// Run the finalized turn: confirm a held eager speculation when its
    /// transcript matches, otherwise run a fresh LLM turn.
    async fn run_finalized_turn(&self, transcript: &str, eager: Option<EagerTurn>) {
        // Eager confirmation: prediction matched the final transcript →
        // the held speculative reply IS the turn. Commit + speak.
        if let Some(eager) = eager {
            // A-G4: NORMALIZED match (trim + collapse whitespace + case-fold)
            // — the smart-turn PREDICTION transcript and the provider's
            // FINAL routinely differ only in casing/spacing ("what's the
            // weather" vs "What's the weather"); the LLM answer is identical,
            // so an exact `==` needlessly discarded confirmable speculations.
            if eager_transcript_matches(&eager.transcript, transcript) {
                let _ = eager.task.await;
                let held = eager.response.lock().take();
                if let Some(Ok(content)) = held
                    && !content.trim().is_empty()
                {
                    // The confirmed eager reply IS a bot turn: register it so
                    // has_active_turn() keeps the MinWords gate up while it
                    // speaks (review wf_85659e16 — eager turns were invisible
                    // to the bot-busy probe), and epoch-gate each sentence so
                    // a barge-in mid-confirm stops the rest.
                    let (turn_id, _confirm_token) = self.begin_turn();
                    let epoch = self.voice_manager.clear_epoch();
                    self.llm
                        .commit_turn(&self.session_id, transcript, &content)
                        .await;
                    // Speak through the SAME sentence aggregator as the
                    // streaming pump: per-sentence chunks + the 160-char
                    // cap apply to eager replies too (a single monolithic
                    // speak() of a multi-sentence reply has worse first-
                    // audio latency and one giant interruption window —
                    // brutal-review finding, wf_0cc69d62).
                    let mut agg = crate::core::text::SentenceAggregator::default();
                    // Never speak chain-of-thought (eager is fast-tier-only, but
                    // strip defensively so a thinking model can't leak here).
                    let mut sentences = agg.push_str(&crate::core::text::strip_think(&content));
                    sentences.extend(agg.flush());
                    for sentence in sentences {
                        let sentence = if self.config.strip_markdown {
                            crate::core::text::strip_markdown_for_tts(&sentence)
                        } else {
                            sentence
                        };
                        match self
                            .voice_manager
                            .speak_if_epoch(
                                &sentence,
                                true,
                                self.config.allow_interruption,
                                epoch,
                            )
                            .await
                        {
                            Ok(true) => {}
                            Ok(false) => {
                                debug!(session = %self.session_id,
                                       "eager reply truncated: barge-in cleared mid-confirm");
                                break;
                            }
                            Err(e) => {
                                warn!(session = %self.session_id, error = %e,
                                      "speaking confirmed eager reply failed");
                                break;
                            }
                        }
                    }
                    self.end_turn(turn_id);
                    // A5: a confirmed eager reply ALWAYS ran on the FAST tier
                    // (committed via self.llm) but bypassed select_tier, so keep
                    // the reasoning-stickiness ledger honest — otherwise a prior
                    // reasoning turn's `true` would spuriously stick the NEXT
                    // anaphoric follow-up onto the 2× tier.
                    *self.last_turn_was_reasoning.lock() = false;
                    debug!(session = %self.session_id, "eager turn confirmed and spoken");
                    return;
                }
                // staged call failed/empty → fall through to a normal turn
            } else {
                eager.token.cancel();
                debug!(session = %self.session_id,
                       "eager speculation discarded (transcript diverged)");
            }
        }

        if let Err(e) = self.run_turn(transcript).await {
            // D-G3: a RECOVERABLE error degrades this turn only (the call
            // stays up — the next utterance runs normally). A FATAL one
            // (auth/config) is surfaced once: every further turn would fail
            // identically, burning latency and money on silence.
            let class = match &e {
                ConversationOrchestratorError::Llm(le) => classify_llm_error(le),
                _ => StageErrorClass::Recoverable,
            };
            crate::core::metrics::bridge::count_turn_error(match class {
                StageErrorClass::Fatal => "fatal",
                StageErrorClass::Recoverable => "recoverable",
            });
            match class {
                StageErrorClass::Recoverable => {
                    warn!(session = %self.session_id, error = %e, "conversation turn failed (recoverable; call continues)");
                }
                StageErrorClass::Fatal => {
                    tracing::error!(session = %self.session_id, error = %e,
                        "FATAL turn error (auth/config) — stopping the session");
                    // STOP further turns: every subsequent utterance would
                    // fail identically against the dead key (review
                    // wc71hewlx #3). on_stt_result short-circuits once set.
                    self.fatal_stopped
                        .store(true, std::sync::atomic::Ordering::Release);
                    let handler = self.fatal_handler.lock().take();
                    if let Some(handler) = handler {
                        handler(e.to_string());
                    }
                }
            }
        }
    }

    /// D-G3: install the fatal-error handler (invoked once per fatal).
    pub fn set_fatal_handler(&self, handler: Arc<dyn Fn(String) + Send + Sync>) {
        *self.fatal_handler.lock() = Some(handler);
    }

    /// A-G5: install the greeting-guard latch (flipped on the first completed
    /// bot turn) so `MuteUntilFirstBotComplete` can open.
    pub fn set_first_bot_complete_latch(&self, latch: Arc<std::sync::atomic::AtomicBool>) {
        *self.first_bot_complete_latch.lock() = Some(latch);
    }

    /// A-G7: register activity (user speech, bot turn) and re-arm the idle
    /// timer. The armed task fires `user_idle_timeout_ms` later ONLY if its
    /// generation is still current AND nothing is busy at fire time (no
    /// active turn, bot not audibly speaking) — the exact races Pipecat's
    /// idle controller guards. Busy-at-fire re-checks until idle or stale.
    pub fn poke_idle_timer(self: &Arc<Self>) {
        use std::sync::atomic::Ordering;
        let timeout_ms = self.config.user_idle_timeout_ms;
        let generation = self.idle_generation.fetch_add(1, Ordering::AcqRel) + 1;
        if timeout_ms == 0 {
            return;
        }
        // WEAK self-ref (review wf_d43814c3): a STRONG Arc kept the
        // orchestrator (and a spawned task per STT signal) alive past session
        // teardown and fired a post-mortem re-engagement turn on a stopped
        // VoiceManager. With a Weak, the task exits as soon as the session's
        // last strong ref drops.
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
                let Some(orch) = weak.upgrade() else {
                    return; // session torn down — never fire post-mortem
                };
                if orch.idle_generation.load(Ordering::Acquire) != generation {
                    return; // newer activity re-armed; this timer is stale
                }
                // INVARIANT: never fire while a turn is active (incl. tool
                // loops) or the bot is still audibly speaking.
                if orch.has_active_turn() || orch.voice_manager.is_bot_speaking() {
                    continue; // busy: re-check after another period
                }
                debug!(session = %orch.session_id, "user idle; re-engaging");
                if let Err(e) = orch
                    .run_turn(
                        "[The user has been silent for a while. Briefly and \
                         naturally check in with them or offer help — one short \
                         sentence.]",
                    )
                    .await
                {
                    warn!(session = %orch.session_id, error = %e, "idle re-engagement failed");
                }
                // ONE-SHOT per idle window: the re-engagement is deliberately
                // not re-armed here (that would nag a persistently-silent
                // caller). The NEXT real user activity re-arms via its own
                // poke_idle_timer call.
                return;
            }
        });
    }

    /// Whether a bot LLM turn is currently in flight (thinking or speaking).
    /// Combined with the audio playout estimate this forms the bot-busy truth
    /// for MinWords gating — the LLM TTFT window (measured 1-6s) is exactly
    /// when a backchannel must not cancel the in-flight turn (review
    /// wf_5772cd64 #9).
    pub fn has_active_turn(&self) -> bool {
        self.turn.lock().is_some()
    }

    /// Tear down the session: cancel any in-flight turn and drop its history.
    pub async fn shutdown(&self) {
        self.cancel_current_turn();
        self.llm.remove_history(&self.session_id).await;
    }
}

/// D-G3: the fatal/recoverable tier for TURN-level errors (Pipecat
/// `ErrorFrame.fatal`). Recoverable = this turn degrades, the CALL stays
/// up. Fatal = retrying every turn against the same failure (bad key,
/// rejected model) only burns money and silence — surface it and let the
/// session end cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageErrorClass {
    Recoverable,
    Fatal,
}

/// Classify an LLM error. Auth/authz/config shapes are FATAL; transient
/// transport/server shapes are RECOVERABLE. Conservative default:
/// recoverable (a wrong fatal kills a call that might have healed).
pub fn classify_llm_error(error: &LlmError) -> StageErrorClass {
    match error {
        LlmError::Cancelled => StageErrorClass::Recoverable,
        LlmError::Endpoint { error, .. } => {
            let e = error.to_lowercase();
            // A 5xx is the SERVER struggling — transient, ALWAYS recoverable,
            // even if the body happens to echo auth words (review wc71hewlx
            // #5: a 500 whose message contained "authentication" was wrongly
            // surfaced as fatal on a call that could have healed).
            let is_5xx = e.contains("http 5");
            if is_5xx {
                return StageErrorClass::Recoverable;
            }
            // Billing exhaustion is an UNRECOVERABLE wall — retrying every
            // turn against insufficient quota burns latency and money
            // (review wc71hewlx #4). Plain rate-limiting (429 without a quota
            // marker) stays recoverable (it clears).
            let billing = e.contains("insufficient_quota")
                || e.contains("insufficient quota")
                || (e.contains("http 429") && e.contains("quota"))
                || e.contains("billing")
                || e.contains("exceeded your current quota");
            let auth_status = e.contains("http 401") || e.contains("http 403");
            let auth_text = e.contains("invalid api key")
                || e.contains("invalid_api_key")
                || e.contains("incorrect api key")
                || e.contains("api key not valid")
                || e.contains("authentication")
                || e.contains("unauthorized");
            let config = e.contains("model_not_found")
                || e.contains("does not exist")
                || e.contains("model `");
            if billing || auth_status || auth_text || config {
                StageErrorClass::Fatal
            } else {
                StageErrorClass::Recoverable
            }
        }
    }
}

/// A-G4: whether an eager speculation's transcript matches the final closely
/// enough to confirm. Normalizes whitespace and case — the prediction and the
/// provider's final routinely differ only there, and the LLM's answer is
/// insensitive to those, so an exact byte comparison needlessly discarded
/// confirmable speculations. Punctuation/word differences still diverge (the
/// staged answer would be on different input — never speak a wrong reply).
fn eager_transcript_matches(speculation: &str, final_transcript: &str) -> bool {
    fn norm(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
    }
    norm(speculation) == norm(final_transcript)
}

/// Validate a client-supplied LLM base URL for SSRF.
///
/// Thin wrapper over the canonical [`crate::core::net::validate_url_for_ssrf`]
/// (http/https only; resolve-then-validate; no `dag` dependency), which carries
/// the same `WAAV_ALLOW_LOOPBACK_ENDPOINTS=1` test escape hatch used elsewhere,
/// so the in-process mock LLM in the conversation tests can target `127.0.0.1`.
fn validate_llm_url(url: &str) -> Result<(), String> {
    crate::core::net::validate_url_for_ssrf(url, &["http", "https"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_needs_reasoning_heuristic() {
        // Complex asks escalate.
        for t in [
            "can you calculate my refund",
            "why is the sky blue",
            "explain how interest works",
            "compare these two plans step by step",
            "what's the total of these items",
        ] {
            assert!(turn_needs_reasoning(t), "{t:?} should escalate");
        }
        // Trivial chit-chat stays on the fast tier.
        for t in ["hi there", "are you open today", "thanks, bye", "yes please"] {
            assert!(!turn_needs_reasoning(t), "{t:?} should stay fast");
        }
        // A long ask escalates on length alone.
        let long = format!("i want to {}understand this", "really ".repeat(30));
        assert!(turn_needs_reasoning(&long));
        assert_eq!(RoutingMode::default(), RoutingMode::Auto);

        // A5 — word-boundary + negation: everyday billing/sales speech that the
        // old substring scan false-escalated must now stay FAST.
        for t in [
            "i said no refund",     // "refund" no longer a keyword
            "i'm not interested",   // "interested" != "interest" (word-boundary)
            "that's reasonable",    // "reasonable" != "reason"
            "interesting weather",  // "interesting" != "interest"
            "please don't explain", // "explain" immediately negated by "don't"
        ] {
            assert!(!turn_needs_reasoning(t), "{t:?} must NOT false-escalate");
        }

        // A5 — percentage-MATH ("N% of/de") escalates cross-lingually; bare digits
        // (phone/order/date) and '%'-INTENSIFIER/DISCOUNT idioms must NOT escalate.
        assert!(turn_needs_reasoning("calcule 15% de 2400"), "percentage math escalates");
        assert!(turn_needs_reasoning("what is 20% of my bill"), "percentage of X escalates");
        for t in [
            "call me at 5551234",
            "order number 12345",
            "i'll be there on the 15th",
            "is the 50% off deal still on", // discount idiom, not math
            "i'm 100% sure that's fine",    // intensifier
            "my battery is at 80% right now",
            "yeah 1000% i agree",
            "we're 100% done here",
        ] {
            assert!(!turn_needs_reasoning(t), "{t:?} must stay fast (no calc intent)");
        }

        // A5 — apostrophe-safe tokenizer: ASCII and CURLY contractions both match.
        assert!(turn_needs_reasoning("what's the total of these"), "ascii apostrophe");
        assert!(turn_needs_reasoning("what\u{2019}s the total of these"), "curly apostrophe");

        // A5 — stickiness: a short continuation after a reasoning turn sticks; a
        // closing/standalone follow-up does not; nothing sticks without context.
        assert!(turn_needs_reasoning_ctx("and the second one?", true), "continuation sticks");
        assert!(turn_needs_reasoning_ctx("what about next year", true), "continuation sticks");
        assert!(!turn_needs_reasoning_ctx("and the second one?", false), "no stick without context");
        assert!(!turn_needs_reasoning_ctx("thanks that's all", true), "closing does not stick");
        assert!(!turn_needs_reasoning_ctx("no that's wrong", true), "negation does not stick");
    }

    #[test]
    fn is_reasoning_model_matrix() {
        for m in [
            "o1-mini",
            "o3",
            "o4-mini",
            "deepseek-r1:1.5b",
            "gpt-5-thinking",
            "qwq-32b",
            // Adaptive-thinking-only families: the clamp floors them above Off, so
            // is_reasoning_model must also classify them (eager-disable + advisory).
            "claude-opus-4-8",
            "claude-opus-4.7",
            "gemini-fable",
            "gemini-3-mythos",
            "fable-5",
        ] {
            assert!(is_reasoning_model(m), "{m} is a reasoning model");
        }
        for m in ["gpt-4o-mini", "llama3.2:1b", "claude-haiku-4-5", "gemini-3-flash"] {
            assert!(!is_reasoning_model(m), "{m} is not a reasoning model");
        }
    }

    #[test]
    fn s3_followup_allowed_only_when_idle_and_still_latest_turn() {
        // Tool spawned in turn 5; begin_turn handed out 5 and advanced next→6.
        // Idle + next==6 ⇒ still the latest turn ⇒ volunteer.
        assert!(followup_allowed(5, 6, false));
        // A turn is currently active ⇒ never talk over it.
        assert!(!followup_allowed(5, 6, true));
        // A newer turn has since started (next advanced past 6) ⇒ new topic.
        assert!(!followup_allowed(5, 7, false));
        assert!(!followup_allowed(5, 99, false));
        // Degenerate / impossible (next behind spawn) ⇒ never.
        assert!(!followup_allowed(5, 5, false));
    }

    #[test]
    fn s3_truncate_for_note_is_char_safe_and_bounded() {
        assert_eq!(truncate_for_note("short", 600), "short");
        let long: String = "é".repeat(1000); // multi-byte chars
        let out = truncate_for_note(&long, 600);
        assert_eq!(out.chars().count(), 601, "600 chars + ellipsis");
        assert!(out.ends_with('…'));
    }

    #[test]
    fn to_client_config_applies_reasoning_floor_clamp() {
        use crate::core::llm::ReasoningEffort;
        // Ordinary fast model: Off stays Off on the wire.
        let cfg = ConversationConfig {
            model: "gpt-4o-mini".into(),
            reasoning_effort: Some(ReasoningEffort::Off),
            ..Default::default()
        };
        assert_eq!(cfg.to_client_config().reasoning_effort, Some(ReasoningEffort::Off));
        assert_eq!(
            cfg.resolved_reasoning_effort(),
            (Some(ReasoningEffort::Off), ReasoningEffort::Off)
        );

        // Adaptive-only model ON THE ANTHROPIC WIRE: a request below the floor is
        // raised, and the floor is reported for the ack echo.
        let cfg = ConversationConfig {
            model: "claude-opus-4-8".into(),
            provider_kind: Some(crate::core::llm::AdapterKind::Anthropic),
            reasoning_effort: Some(ReasoningEffort::Off),
            ..Default::default()
        };
        assert_eq!(cfg.to_client_config().reasoning_effort, Some(ReasoningEffort::Low));
        assert_eq!(
            cfg.resolved_reasoning_effort(),
            (Some(ReasoningEffort::Low), ReasoningEffort::Low)
        );

        // A10b: the SAME model fronted by an OpenAI-compatible proxy floors at Off
        // — no forced param to a proxy that may reject it, and the ack is honest.
        let cfg = ConversationConfig {
            model: "claude-opus-4-8".into(),
            provider_kind: Some(crate::core::llm::AdapterKind::OpenAi),
            reasoning_effort: Some(ReasoningEffort::Off),
            ..Default::default()
        };
        assert_eq!(cfg.to_client_config().reasoning_effort, Some(ReasoningEffort::Off));
        assert_eq!(
            cfg.resolved_reasoning_effort(),
            (Some(ReasoningEffort::Off), ReasoningEffort::Off)
        );

        // None → no param, floor still reported.
        let cfg = ConversationConfig { model: "gpt-4o-mini".into(), ..Default::default() };
        assert_eq!(cfg.to_client_config().reasoning_effort, None);
    }

    #[test]
    fn eager_match_is_whitespace_and_case_insensitive() {
        // A-G4: prediction vs final differing only in case/spacing confirms.
        assert!(eager_transcript_matches(
            "what's the weather today",
            "What's the weather today"
        ));
        assert!(eager_transcript_matches("hello   world", "Hello world"));
        assert!(eager_transcript_matches(" trailing space ", "trailing space"));
        // But genuinely different content does NOT confirm (never speak a
        // reply staged on different input).
        assert!(!eager_transcript_matches(
            "what's the weather",
            "what's the weather today"
        ));
        assert!(!eager_transcript_matches("book a flight", "cancel a flight"));
    }

    #[test]
    fn test_config_default() {
        let c = ConversationConfig::default();
        assert!(c.streaming);
        assert!(c.allow_interruption);
        assert_eq!(c.base_url, "https://api.openai.com/v1");
    }

    // --- X-G1: voice-path fast-LLM defaults ---

    #[test]
    fn voice_max_tokens_cap_applied_when_unset() {
        let c = ConversationConfig::default();
        assert_eq!(c.max_tokens, None, "operator did not set a cap");
        let client_cfg = c.to_client_config();
        assert_eq!(
            client_cfg.max_tokens,
            Some(VOICE_DEFAULT_MAX_TOKENS),
            "voice path must cap completion length by default (X-G1)"
        );
    }

    #[test]
    fn explicit_max_tokens_respected() {
        let c = ConversationConfig { max_tokens: Some(1024), ..Default::default() };
        assert_eq!(c.to_client_config().max_tokens, Some(1024));
    }

    #[test]
    fn voice_default_model_is_non_reasoning_fast_tier() {
        // The default voice model must stay a fast NON-reasoning tier: a
        // reasoning default would burn seconds of llm_ttft (the measured
        // 79%-of-turn failure mode, AUDIT_REPORT §1). This test is the
        // tripwire against someone "upgrading" the default to a reasoning
        // model.
        let c = ConversationConfig::default();
        assert_eq!(c.model, "gpt-4o-mini");
    }

    #[test]
    fn test_to_client_config_carries_fields() {
        let c = ConversationConfig {
            base_url: "https://example.com/v1".into(),
            model: "m".into(),
            system_prompt: Some("be nice".into()),
            max_history: 7,
            // Isolate the carry-through from the D3 filler-guard addendum.
            latency_filler: LatencyFiller::Off,
            ..Default::default()
        };
        let cc = c.to_client_config();
        assert_eq!(cc.base_url, "https://example.com/v1");
        assert_eq!(cc.model, "m");
        assert_eq!(cc.system_prompt.as_deref(), Some("be nice"));
        assert_eq!(cc.max_history, 7);
    }

    #[test]
    fn d3_filler_guard_appended_only_when_masking_on() {
        // Masking on (default Auto) → the user's prompt is preserved and the
        // anti-double-ack guard is appended.
        let on = ConversationConfig {
            system_prompt: Some("be nice".into()),
            ..Default::default()
        };
        let p = on.to_client_config().system_prompt.unwrap();
        assert!(p.starts_with("be nice"), "user prompt preserved: {p}");
        assert!(p.contains("Do not begin your reply with filler"), "guard appended: {p}");

        // Masking off → identity (the prompt is untouched).
        let off = ConversationConfig {
            system_prompt: Some("be nice".into()),
            latency_filler: LatencyFiller::Off,
            ..Default::default()
        };
        assert_eq!(off.to_client_config().system_prompt.as_deref(), Some("be nice"));

        // No prompt + masking on → just the guard.
        let bare = ConversationConfig { system_prompt: None, ..Default::default() };
        assert!(
            bare.to_client_config()
                .system_prompt
                .unwrap()
                .contains("Answer directly")
        );
    }

    #[test]
    fn test_validate_llm_url_rejects_loopback_by_default() {
        // Ensure the escape hatch is OFF for this assertion. The env var is
        // process-global state: serialize with every other mutator in this test
        // binary via the shared core::net lock.
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // SAFETY: test-only env mutation, serialized by test_env_lock.
        unsafe {
            std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        }
        assert!(validate_llm_url("http://127.0.0.1:8080/v1").is_err());
        assert!(validate_llm_url("http://localhost/v1").is_err());
        assert!(validate_llm_url("https://api.openai.com/v1").is_ok());
    }

    #[test]
    fn test_validate_llm_url_rejects_bad_scheme() {
        assert!(validate_llm_url("ftp://example.com/v1").is_err());
        assert!(validate_llm_url("not a url").is_err());
    }
}

#[cfg(test)]
mod stage_error_tests {
    use super::*;

    fn endpoint_err(msg: &str) -> LlmError {
        LlmError::Endpoint {
            provider: "test".into(),
            model: "m".into(),
            error: msg.into(),
        }
    }

    /// D-G3 tripwire: the classification matrix. A new failure shape lands
    /// RECOVERABLE by default (conservative — a wrong fatal kills a call
    /// that might have healed).
    #[test]
    fn error_classification_matrix() {
        // FATAL: auth / authz / config / billing-exhaustion.
        for msg in [
            "HTTP 401 - {\"error\":\"bad key\"}",
            "HTTP 403 - forbidden",
            "HTTP 400 - Incorrect API key provided",
            "HTTP 400 - invalid_api_key",
            "HTTP 404 - The model `gpt-9` does not exist",
            "HTTP 400 - authentication failed",
            // review wc71hewlx #4: billing exhaustion is an unrecoverable wall.
            "HTTP 429 - You exceeded your current quota; insufficient_quota",
            "HTTP 429 - billing hard limit reached",
        ] {
            assert_eq!(
                classify_llm_error(&endpoint_err(msg)),
                StageErrorClass::Fatal,
                "{msg} must classify FATAL"
            );
        }
        // RECOVERABLE: transient transport/server shapes.
        for msg in [
            "HTTP 500 - internal server error",
            "HTTP 503 - overloaded",
            "HTTP 429 - rate limited", // plain rate-limit (no quota marker) clears
            // review wc71hewlx #5: a 5xx body echoing auth words is still
            // transient — never fatal.
            "HTTP 502 - upstream authentication gateway error",
            "Request failed: connection reset by peer",
            "Request failed: operation timed out",
        ] {
            assert_eq!(
                classify_llm_error(&endpoint_err(msg)),
                StageErrorClass::Recoverable,
                "{msg} must classify RECOVERABLE"
            );
        }
        assert_eq!(classify_llm_error(&LlmError::Cancelled), StageErrorClass::Recoverable);
    }
}

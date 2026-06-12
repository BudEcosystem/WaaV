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
}

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
        }
    }
}

/// Default `max_tokens` cap for the VOICE path when the operator doesn't set
/// one (X-G1). A spoken reply of ~30s is ~75 words (~100 tokens); 256 leaves
/// headroom for multilingual scripts and longer answers while preventing a
/// verbose model from generating essays the user must sit through (and pay
/// for). Explicit `max_tokens` always wins. Non-voice paths (DAG JSON flows)
/// are unaffected — this cap lives in the conversation config only.
pub const VOICE_DEFAULT_MAX_TOKENS: u32 = 256;

impl ConversationConfig {
    fn to_client_config(&self) -> LlmClientConfig {
        LlmClientConfig {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
            system_prompt: self.system_prompt.clone(),
            temperature: self.temperature,
            // X-G1: cap completion length for voice unless explicitly set.
            max_tokens: self.max_tokens.or(Some(VOICE_DEFAULT_MAX_TOKENS)),
            streaming: self.streaming,
            max_history: self.max_history,
            provider_kind: self.provider_kind,
            ..Default::default()
        }
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

        let llm = Arc::new(LlmClient::new(config.to_client_config()));
        Ok(Self {
            session_id: session_id.into(),
            llm,
            voice_manager,
            config: Arc::new(config),
            turn: Arc::new(SyncMutex::new(None)),
            next_turn_id: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            eager: Arc::new(SyncMutex::new(None)),
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
        Self {
            session_id: session_id.into(),
            llm,
            voice_manager,
            config: Arc::new(config),
            turn: Arc::new(SyncMutex::new(None)),
            next_turn_id: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            eager: Arc::new(SyncMutex::new(None)),
        }
    }

    /// The shared LLM client (for history inspection / cleanup).
    pub fn llm(&self) -> &Arc<LlmClient> {
        &self.llm
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

    /// React to a barge-in (new user speech while the bot is interruptible):
    /// cancel the in-flight LLM turn and clear queued/playing TTS.
    ///
    /// Reuses the VoiceManager's `interruption_state` as the source of truth: if
    /// the current bot audio is in a non-interruptible window, this is a no-op
    /// (the VoiceManager will also ignore the new speech).
    pub async fn handle_barge_in(&self) {
        if self.voice_manager.is_interruption_blocked().await {
            debug!(session = %self.session_id, "barge-in ignored (non-interruptible window)");
            return;
        }
        let had_turn = self.cancel_current_turn();
        if let Some(eager) = self.eager.lock().take() {
            eager.token.cancel();
            debug!(session = %self.session_id, "barge-in cancelled eager speculative turn");
        }
        // Clear any queued/in-flight TTS so the bot stops talking immediately.
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
            let cb: crate::core::llm::TokenCallback = Arc::new(move |delta: &str| {
                if !delta.is_empty()
                    && !first_token_seen.swap(true, std::sync::atomic::Ordering::Relaxed)
                    && let Some(obs) = &obs_cb
                {
                    obs.notify_llm_first_token(crate::core::observability::now_monotonic_ns());
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
            let handle = tokio::spawn(async move {
                let mut agg = crate::core::text::SentenceAggregator::default();

                async fn speak_sentence(
                    vm: &VoiceManager,
                    text: String,
                    allow_interruption: bool,
                    epoch: usize,
                    spoke: &std::sync::atomic::AtomicBool,
                    obs: &Option<Arc<crate::core::observability::ObserverRegistry>>,
                ) -> bool {
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
                                if chunk.is_empty() { continue; }
                                for sentence in agg.push_str(&chunk) {
                                    // Re-check cancellation BETWEEN sentences:
                                    // barge-in landing mid-loop must not speak
                                    // the remaining (stale) sentences into the
                                    // freshly cleared TTS queue.
                                    if token_for_cb.is_cancelled() {
                                        break 'pump;
                                    }
                                    if !speak_sentence(&vm_pump, sentence, allow_interruption, epoch, &spoke_pump, &obs_pump).await {
                                        break 'pump;
                                    }
                                }
                            }
                            None => {
                                // Stream ended: speak the held remainder (a
                                // boundary awaiting lookahead must still fire)
                                // — unless the turn was cancelled.
                                if !token_for_cb.is_cancelled()
                                    && let Some(tail) = agg.flush()
                                {
                                    let _ = speak_sentence(&vm_pump, tail, allow_interruption, epoch, &spoke_pump, &obs_pump).await;
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

        let result = self
            .llm
            .complete(
                &self.session_id,
                transcript,
                self.config.api_key.as_deref(),
                &token,
                on_token,
            )
            .await;

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
        let result = match result {
            Ok(response)
                if !response.tool_calls.is_empty() && self.llm.functions().is_some() =>
            {
                let registry =
                    Arc::clone(self.llm.functions().expect("guarded by condition"));
                crate::core::llm::run_tool_loop(
                    &self.llm,
                    &registry,
                    &self.session_id,
                    response,
                    self.config.api_key.as_deref(),
                    &token,
                    crate::core::llm::ToolLoopOptions::default(),
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
                let content_empty = response.content.trim().is_empty();
                let need_fallback_speak = if streaming {
                    // Reasoning-model guard: some models stream only reasoning
                    // deltas and deliver the answer (or nothing) at the end. If
                    // the pump spoke nothing, fall back to the final content; if
                    // that is empty too, say so loudly instead of going silent.
                    !spoke.load(std::sync::atomic::Ordering::Relaxed)
                } else {
                    true
                };
                if need_fallback_speak && !content_empty {
                    match self
                        .voice_manager
                        .speak_if_epoch(&response.content, true, allow_interruption, epoch)
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
                             model or empty completion) — bot stays silent; check the \
                             configured model/max_tokens"
                        );
                    }
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
            Err(e) => Err(e.into()),
        };

        self.end_turn(id);
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
        let text = transcript.trim();
        if text.is_empty() {
            return;
        }
        let mut guard = self.eager.lock();
        if guard.is_some() {
            return; // one in-flight speculation at a time
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
            if eager.transcript == transcript {
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
                    let mut sentences = agg.push_str(&content);
                    sentences.extend(agg.flush());
                    for sentence in sentences {
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
            warn!(session = %self.session_id, error = %e, "conversation turn failed");
        }
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
            ..Default::default()
        };
        let cc = c.to_client_config();
        assert_eq!(cc.base_url, "https://example.com/v1");
        assert_eq!(cc.model, "m");
        assert_eq!(cc.system_prompt.as_deref(), Some("be nice"));
        assert_eq!(cc.max_history, 7);
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

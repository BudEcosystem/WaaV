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
        }
    }
}

impl ConversationConfig {
    fn to_client_config(&self) -> LlmClientConfig {
        LlmClientConfig {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
            system_prompt: self.system_prompt.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            streaming: self.streaming,
            max_history: self.max_history,
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

        // Stream tokens straight to TTS as they arrive. We buffer into sentence-ish
        // flushes to avoid a `speak()` call per character while keeping latency low.
        let vm = self.voice_manager.clone();
        let allow_interruption = self.config.allow_interruption;
        let streaming = self.config.streaming;

        let on_token = if streaming {
            let token_for_cb = token.clone();
            // Channel from the (sync) token callback to an async pump that calls speak().
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let cb: crate::core::llm::TokenCallback = Arc::new(move |delta: &str| {
                let _ = tx.send(delta.to_string());
            });

            // Pump task: forward token chunks to TTS until the channel closes or
            // the turn is cancelled.
            let vm_pump = vm.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = token_for_cb.cancelled() => break,
                        maybe = rx.recv() => match maybe {
                            Some(chunk) => {
                                if chunk.is_empty() { continue; }
                                let res = if allow_interruption {
                                    vm_pump.speak(&chunk, true).await
                                } else {
                                    vm_pump.speak_with_interruption(&chunk, true, false).await
                                };
                                if let Err(e) = res {
                                    warn!(error = %e, "speak() during streamed turn failed");
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            });
            Some(cb)
        } else {
            None
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

        self.end_turn(id);

        match result {
            Ok(response) => {
                // Non-streaming: speak the whole reply now. Streaming already
                // spoke tokens via the pump above.
                if !streaming && !response.content.trim().is_empty() {
                    if allow_interruption {
                        self.voice_manager.speak(&response.content, true).await?;
                    } else {
                        self.voice_manager
                            .speak_with_interruption(&response.content, true, false)
                            .await?;
                    }
                }
                Ok(())
            }
            Err(LlmError::Cancelled) => {
                debug!(session = %self.session_id, "LLM turn cancelled (barge-in/teardown)");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Drive the orchestrator from a single STT result.
    ///
    /// - Any non-empty user speech (interim or final) is treated as a potential
    ///   barge-in: if the bot is mid-utterance and interruptible, cancel its turn
    ///   and clear TTS.
    /// - A **finalized** turn (`is_speech_final`) with content runs a new LLM
    ///   turn whose reply streams to TTS.
    pub async fn on_stt_result(&self, result: &STTResult) {
        let has_text = !result.transcript.trim().is_empty();

        // Barge-in: new user speech arrives while the bot may be talking. Only
        // meaningful when there is text (avoid clearing on empty interims).
        if has_text {
            self.handle_barge_in().await;
        }

        // Only fire the LLM→TTS pipeline on a finalized turn with real content.
        if result.is_speech_final && has_text {
            let transcript = result.transcript.clone();
            if let Err(e) = self.run_turn(&transcript).await {
                warn!(session = %self.session_id, error = %e, "conversation turn failed");
            }
        }
    }

    /// Tear down the session: cancel any in-flight turn and drop its history.
    pub async fn shutdown(&self) {
        self.cancel_current_turn();
        self.llm.remove_history(&self.session_id).await;
    }
}

/// Validate a client-supplied LLM base URL for SSRF.
///
/// Uses a self-contained resolve-then-validate check (no `dag` dependency) with
/// the same `WAAV_ALLOW_LOOPBACK_ENDPOINTS=1` test escape hatch used elsewhere,
/// so the in-process mock LLM in the conversation tests can target `127.0.0.1`.
fn validate_llm_url(url: &str) -> Result<(), String> {
    use crate::utils::url_validation::is_private_ip;
    use std::net::{IpAddr, ToSocketAddrs};

    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL '{}': {}", url, e))?;
    let scheme = parsed.scheme().to_lowercase();
    if !["http", "https"].contains(&scheme.as_str()) {
        return Err(format!("scheme '{}' not allowed (use http/https)", scheme));
    }

    if loopback_endpoints_allowed() {
        return Ok(());
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| format!("URL '{}' has no host", url))?;

    let blocked = [
        "localhost",
        "127.0.0.1",
        "::1",
        "0.0.0.0",
        "169.254.169.254",
        "metadata.google.internal",
        "metadata.gcp.internal",
    ];
    let host_lower = host.to_lowercase();
    if blocked.contains(&host_lower.as_str()) {
        return Err(format!("host '{}' is blocked (SSRF protection)", host));
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(format!("URL points to private IP '{}' (SSRF)", ip));
        }
        return Ok(());
    }

    // Resolve-then-validate DNS names (kills DNS-rebind/TOCTOU).
    if let Ok(addrs) = (host, 0u16).to_socket_addrs() {
        for addr in addrs {
            if is_private_ip(&addr.ip()) {
                return Err(format!(
                    "host '{}' resolves to private IP '{}' (SSRF)",
                    host,
                    addr.ip()
                ));
            }
        }
    }

    Ok(())
}

fn loopback_endpoints_allowed() -> bool {
    matches!(
        std::env::var("WAAV_ALLOW_LOOPBACK_ENDPOINTS").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
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
        // Ensure the escape hatch is OFF for this assertion.
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

//! Token-aware context summarization (PIPECAT_FIX_PLAN B-G6).
//!
//! Long sessions blow past model context windows and inflate latency/cost.
//! Pipecat's native summarizer compacts the OLDEST messages into one summary
//! once a token ESTIMATE crosses a threshold, keeping the most recent
//! messages verbatim. This port mirrors it:
//!
//! - estimate: `chars / 4` + a per-message overhead (no tokenizer dep);
//! - the summary inference runs DETACHED (never touches session history)
//!   and optionally on a CHEAPER model (`summary_llm`), with a hard timeout;
//! - the boundary NEVER splits an assistant tool-call from its results and
//!   the system message is NEVER dropped;
//! - any failure (timeout, error, empty summary) falls back to a plain
//!   count-trim — the session keeps working, just without the summary.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::{ChatMessage, LlmClient, LlmResult, MessageRole};

/// Pipecat parity: ~4 chars per token.
const CHARS_PER_TOKEN: usize = 4;
/// Flat per-message overhead (role/framing tokens).
const PER_MESSAGE_OVERHEAD_TOKENS: usize = 8;

/// Configuration for [`LlmClient::maybe_summarize`].
#[derive(Clone)]
pub struct SummaryConfig {
    /// Estimated-token threshold that triggers compaction.
    pub target_tokens: usize,
    /// The most recent messages kept VERBATIM (the conversational working
    /// set the model still needs raw).
    pub min_messages_after: usize,
    /// The summarization instruction.
    pub prompt: String,
    /// Run the summary on a different (cheaper) client. `None` = same LLM.
    pub summary_llm: Option<Arc<LlmClient>>,
    /// Hard budget for the summary inference (Pipecat: 5 s).
    pub timeout: Duration,
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            target_tokens: 6_000,
            min_messages_after: 4,
            prompt: "Summarize the conversation so far in a compact paragraph. \
                     Preserve every concrete fact, name, number, decision, and \
                     open question — the assistant will rely on this summary \
                     instead of the original messages."
                .to_string(),
            summary_llm: None,
            timeout: Duration::from_secs(5),
        }
    }
}

/// Estimate the token cost of a message list (chars/4 + per-message
/// overhead; tool calls/results count via their JSON text).
pub fn estimate_tokens(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .map(|m| {
            let mut chars = m.content.as_deref().map(str::len).unwrap_or(0);
            if let Some(calls) = &m.tool_calls {
                for c in calls {
                    chars += c.function.name.len() + c.function.arguments.len();
                }
            }
            chars / CHARS_PER_TOKEN + PER_MESSAGE_OVERHEAD_TOKENS
        })
        .sum()
}

/// Split `messages` into (system, head-to-summarize, tail-kept-verbatim).
/// The boundary moves LEFT until the tail does not begin mid tool-exchange
/// (a tool result whose call is in the head, or vice versa).
fn split_for_summary(
    messages: &[ChatMessage],
    min_messages_after: usize,
) -> (Option<ChatMessage>, Vec<ChatMessage>, Vec<ChatMessage>) {
    let (system, rest): (Option<ChatMessage>, &[ChatMessage]) = match messages.first() {
        Some(m) if m.role == MessageRole::System => (Some(m.clone()), &messages[1..]),
        _ => (None, messages),
    };
    if rest.len() <= min_messages_after {
        return (system, Vec::new(), rest.to_vec());
    }
    let mut boundary = rest.len() - min_messages_after;
    // Never start the tail on a tool RESULT (its call would be summarized
    // away) and never end the head on an assistant whose tool calls' results
    // sit in the tail.
    while boundary > 0 {
        let tail_starts_with_result = rest[boundary].role == MessageRole::Tool;
        let head_ends_with_call = rest[boundary - 1]
            .tool_calls
            .as_ref()
            .is_some_and(|c| !c.is_empty());
        if tail_starts_with_result || head_ends_with_call {
            boundary -= 1;
        } else {
            break;
        }
    }
    (system, rest[..boundary].to_vec(), rest[boundary..].to_vec())
}

/// Render the head as a plain transcript for the summarizer.
fn render_transcript(head: &[ChatMessage]) -> String {
    let mut out = String::new();
    for m in head {
        let role = match m.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool | MessageRole::Function => "tool",
        };
        out.push_str(role);
        out.push_str(": ");
        if let Some(c) = &m.content {
            out.push_str(c);
        }
        if let Some(calls) = &m.tool_calls {
            for c in calls {
                out.push_str(&format!(
                    " [called {}({})]",
                    c.function.name, c.function.arguments
                ));
            }
        }
        out.push('\n');
    }
    out
}

impl LlmClient {
    /// Compact the session context once its token ESTIMATE crosses
    /// `config.target_tokens`: the oldest messages become one summary, the
    /// last `min_messages_after` stay verbatim, the system message always
    /// survives. Returns whether compaction happened. Failure of the
    /// summary inference falls back to a count-trim (loud, never fatal).
    pub async fn maybe_summarize(
        &self,
        session_id: &str,
        config: &SummaryConfig,
        api_key: Option<&str>,
        cancel: &CancellationToken,
    ) -> LlmResult<bool> {
        // Snapshot WITH the mutation version: the rewrite at the end only
        // applies if no turn mutated the context during the inference window
        // (review wf_d43814c3 #2 — a lost-update RMW destroyed concurrent
        // appends and orphaned tool messages).
        let Some((snapshot, version)) = self.history_snapshot_versioned(session_id).await else {
            return Ok(false);
        };
        let estimate = estimate_tokens(&snapshot);
        if estimate < config.target_tokens {
            return Ok(false);
        }
        let (system, head, tail) = split_for_summary(&snapshot, config.min_messages_after);
        if head.is_empty() {
            return Ok(false);
        }
        debug!(
            session = session_id,
            estimate,
            head = head.len(),
            tail = tail.len(),
            "context over token target; summarizing"
        );

        let summarizer = config.summary_llm.as_deref().unwrap_or(self);
        let request = vec![
            ChatMessage::system(&config.prompt),
            ChatMessage::user(render_transcript(&head)),
        ];
        let summary = match tokio::time::timeout(
            config.timeout,
            summarizer.complete_detached(&request, api_key, cancel),
        )
        .await
        {
            Ok(Ok(resp)) if !resp.content.trim().is_empty() => Some(resp.content),
            Ok(Ok(_)) => {
                warn!(
                    session = session_id,
                    "summary came back empty; count-trim fallback"
                );
                None
            }
            Ok(Err(e)) => {
                warn!(session = session_id, error = %e, "summary inference failed; count-trim fallback");
                None
            }
            Err(_) => {
                warn!(session = session_id, timeout = ?config.timeout, "summary timed out; count-trim fallback");
                None
            }
        };

        let mut rebuilt = Vec::with_capacity(tail.len() + 2);
        if let Some(system) = system {
            rebuilt.push(system);
        }
        if let Some(summary) = &summary {
            // A user-role message keeps every vendor's alternation rules
            // happy (Anthropic merges consecutive same-role messages).
            rebuilt.push(ChatMessage::user(format!(
                "[Prior conversation summary] {summary}"
            )));
        }
        rebuilt.extend(tail);
        // CAS replace: if a turn appended/changed the context during the
        // summary inference, its version moved — refuse the stale rewrite and
        // leave the (now-current) history intact. The next turn re-checks.
        let applied = self
            .replace_context_if_unchanged(session_id, version, rebuilt)
            .await;
        if applied {
            info!(
                session = session_id,
                summarized = summary.is_some(),
                "context compacted"
            );
        } else {
            warn!(
                session = session_id,
                "context changed during summarization; rewrite skipped (no data lost)"
            );
        }
        Ok(applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::llm::{LlmClientConfig, ToolCall};
    use axum::{Json, Router, extract::State, routing::post};
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    fn msg_of_len(role: MessageRole, len: usize) -> ChatMessage {
        let text = "x".repeat(len);
        match role {
            MessageRole::System => ChatMessage::system(text),
            MessageRole::User => ChatMessage::user(text),
            MessageRole::Assistant => ChatMessage::assistant(text),
            MessageRole::Tool | MessageRole::Function => ChatMessage::tool("id", text),
        }
    }

    #[derive(Clone)]
    struct MockState {
        requests: Arc<parking_lot::Mutex<Vec<Value>>>,
        reply: Arc<parking_lot::Mutex<String>>,
        fail: Arc<std::sync::atomic::AtomicBool>,
        delay_ms: Arc<std::sync::atomic::AtomicU64>,
    }

    async fn start_mock(reply: &str) -> (String, MockState) {
        let st = MockState {
            requests: Arc::new(parking_lot::Mutex::new(Vec::new())),
            reply: Arc::new(parking_lot::Mutex::new(reply.to_string())),
            fail: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            delay_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        async fn chat(
            State(st): State<MockState>,
            Json(req): Json<Value>,
        ) -> impl axum::response::IntoResponse {
            st.requests.lock().push(req);
            let delay = st.delay_ms.load(std::sync::atomic::Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            if st.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error":"boom"})),
                );
            }
            let reply = st.reply.lock().clone();
            (
                axum::http::StatusCode::OK,
                Json(json!({
                    "id":"r","object":"chat.completion","created":0,"model":"m",
                    "choices":[{"index":0,"message":{"role":"assistant","content":reply},"finish_reason":"stop"}]
                })),
            )
        }
        let app = Router::new()
            .route("/chat/completions", post(chat))
            .with_state(st.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://127.0.0.1:{}", addr.port()), st)
    }

    fn client(url: &str) -> LlmClient {
        LlmClient::new(LlmClientConfig {
            base_url: url.into(),
            api_key: Some("k".into()),
            model: "m".into(),
            streaming: false,
            ..Default::default()
        })
    }

    async fn seed(llm: &LlmClient, sid: &str, messages: Vec<ChatMessage>) {
        llm.set_context(sid, messages).await;
    }

    #[tokio::test]
    async fn summarize_triggers_above_token_estimate_only() {
        let (url, st) = start_mock("the user discussed apples and oranges").await;
        let llm = client(&url);
        let cancel = CancellationToken::new();
        let cfg = SummaryConfig {
            target_tokens: 500,
            ..Default::default()
        };

        // Small context: untouched, no inference.
        seed(
            &llm,
            "s",
            vec![
                ChatMessage::system("persona"),
                msg_of_len(MessageRole::User, 100),
                msg_of_len(MessageRole::Assistant, 100),
            ],
        )
        .await;
        assert!(!llm.maybe_summarize("s", &cfg, None, &cancel).await.unwrap());
        assert!(
            st.requests.lock().is_empty(),
            "below target: no summary inference"
        );
        assert_eq!(llm.history_snapshot("s").await.len(), 3);

        // Large context: compacted.
        let mut big = vec![ChatMessage::system("persona")];
        for _ in 0..10 {
            big.push(msg_of_len(MessageRole::User, 400));
            big.push(msg_of_len(MessageRole::Assistant, 400));
        }
        seed(&llm, "s", big).await;
        assert!(llm.maybe_summarize("s", &cfg, None, &cancel).await.unwrap());
        assert_eq!(st.requests.lock().len(), 1, "exactly one summary inference");
        let after = llm.history_snapshot("s").await;
        // system + summary + 4 verbatim
        assert_eq!(after.len(), 1 + 1 + 4);
    }

    #[tokio::test]
    async fn summary_preserves_system_and_recent() {
        let (url, _st) = start_mock("summary text").await;
        let llm = client(&url);
        let cancel = CancellationToken::new();
        let cfg = SummaryConfig {
            target_tokens: 100,
            min_messages_after: 2,
            ..Default::default()
        };

        let mut msgs = vec![ChatMessage::system("THE PERSONA")];
        for i in 0..8 {
            msgs.push(ChatMessage::user(format!(
                "question {i} {}",
                "x".repeat(120)
            )));
            msgs.push(ChatMessage::assistant(format!(
                "answer {i} {}",
                "x".repeat(120)
            )));
        }
        seed(&llm, "s", msgs).await;
        assert!(llm.maybe_summarize("s", &cfg, None, &cancel).await.unwrap());

        let after = llm.history_snapshot("s").await;
        assert_eq!(after[0].role, MessageRole::System);
        assert_eq!(
            after[0].content.as_deref(),
            Some("THE PERSONA"),
            "system NEVER dropped"
        );
        assert!(
            after[1]
                .content
                .as_deref()
                .unwrap()
                .contains("[Prior conversation summary] summary text")
        );
        let tail: Vec<_> = after[after.len() - 2..].iter().collect();
        assert!(
            tail[0]
                .content
                .as_deref()
                .unwrap()
                .starts_with("question 7")
        );
        assert!(tail[1].content.as_deref().unwrap().starts_with("answer 7"));
    }

    #[tokio::test]
    async fn summary_does_not_split_tool_call_result_pair() {
        let (url, _st) = start_mock("s").await;
        let llm = client(&url);
        let cancel = CancellationToken::new();
        // min_after = 2 would place the boundary BETWEEN the assistant
        // tool-call message and its result — the split must move left.
        let cfg = SummaryConfig {
            target_tokens: 100,
            min_messages_after: 2,
            ..Default::default()
        };

        let mut call_msg = ChatMessage::assistant("");
        call_msg.tool_calls = Some(vec![ToolCall {
            id: "call_9".into(),
            call_type: "function".into(),
            function: crate::core::llm::FunctionCall {
                name: "lookup".into(),
                arguments: "{}".into(),
            },
        }]);
        let msgs = vec![
            ChatMessage::system("p"),
            msg_of_len(MessageRole::User, 400),
            msg_of_len(MessageRole::Assistant, 400),
            ChatMessage::user("do the lookup"),
            call_msg,
            ChatMessage::tool("call_9", "{\"answer\":42}"),
        ];
        seed(&llm, "s", msgs).await;
        assert!(llm.maybe_summarize("s", &cfg, None, &cancel).await.unwrap());

        let after = llm.history_snapshot("s").await;
        // The pair stays together in the verbatim tail.
        let call_idx = after
            .iter()
            .position(|m| m.tool_calls.as_ref().is_some_and(|c| !c.is_empty()))
            .expect("tool-call message survived");
        assert_eq!(
            after[call_idx + 1].role,
            MessageRole::Tool,
            "result must directly follow its call: {after:?}"
        );
    }

    #[tokio::test]
    async fn summary_failure_falls_back_to_count_trim() {
        let (url, st) = start_mock("never used").await;
        st.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        let llm = client(&url);
        let cancel = CancellationToken::new();
        let cfg = SummaryConfig {
            target_tokens: 100,
            min_messages_after: 3,
            ..Default::default()
        };

        let mut msgs = vec![ChatMessage::system("p")];
        for i in 0..8 {
            msgs.push(ChatMessage::user(format!("q{i} {}", "x".repeat(150))));
        }
        seed(&llm, "s", msgs).await;
        assert!(llm.maybe_summarize("s", &cfg, None, &cancel).await.unwrap());

        let after = llm.history_snapshot("s").await;
        // system + last 3, NO summary message.
        assert_eq!(after.len(), 4, "count-trim fallback: {after:?}");
        assert_eq!(after[0].role, MessageRole::System);
        assert!(after[1].content.as_deref().unwrap().starts_with("q5"));
        assert!(!after.iter().any(|m| {
            m.content
                .as_deref()
                .unwrap_or_default()
                .contains("Prior conversation summary")
        }));
    }

    /// review wf_d43814c3 #2: a turn that appends DURING the summary inference
    /// window must not be destroyed by the rewrite. The version-checked
    /// replace refuses the stale rewrite; the concurrent message survives.
    #[tokio::test]
    async fn concurrent_append_during_summary_is_not_lost() {
        let (url, st) = start_mock("a summary of the early conversation").await;
        st.delay_ms.store(150, std::sync::atomic::Ordering::SeqCst);
        let llm = Arc::new(client(&url));
        let cancel = CancellationToken::new();
        let cfg = SummaryConfig {
            target_tokens: 100,
            min_messages_after: 2,
            ..Default::default()
        };

        let mut big = vec![ChatMessage::system("persona")];
        for _ in 0..8 {
            big.push(msg_of_len(MessageRole::User, 200));
            big.push(msg_of_len(MessageRole::Assistant, 200));
        }
        seed(&llm, "s", big).await;

        // Kick off summarization (blocks ~150ms on the mock)...
        let llm_bg = Arc::clone(&llm);
        let cfg_bg = cfg.clone();
        let handle =
            tokio::spawn(async move { llm_bg.maybe_summarize("s", &cfg_bg, None, &cancel).await });
        // ...and append a NEW turn while it's in flight (bumps the version).
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        llm.append_context(
            "s",
            vec![ChatMessage::user("URGENT new turn during summary")],
        )
        .await;

        let applied = handle.await.unwrap().unwrap();
        assert!(
            !applied,
            "the stale rewrite must be refused (version moved)"
        );

        let after = llm.history_snapshot("s").await;
        assert!(
            after.iter().any(|m| m
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("URGENT new turn during summary")),
            "the concurrent append must survive: {after:?}"
        );
        assert!(
            !after.iter().any(|m| m
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("Prior conversation summary")),
            "no stale summary rewrite was applied"
        );
    }

    /// The CAS replace applies when the version is unchanged, and refuses on
    /// a stale version (deterministic, no timing).
    #[tokio::test]
    async fn replace_context_if_unchanged_is_a_cas() {
        let (url, _st) = start_mock("x").await;
        let llm = client(&url);
        seed(
            &llm,
            "s",
            vec![ChatMessage::system("p"), ChatMessage::user("hi")],
        )
        .await;
        let (_snap, version) = llm.history_snapshot_versioned("s").await.unwrap();

        // A concurrent mutation moves the version.
        llm.append_context("s", vec![ChatMessage::user("concurrent")])
            .await;
        assert!(
            !llm.replace_context_if_unchanged("s", version, vec![ChatMessage::system("rewritten")])
                .await,
            "stale version: replace refused"
        );
        // The fresh version applies.
        let (_snap, v2) = llm.history_snapshot_versioned("s").await.unwrap();
        assert!(
            llm.replace_context_if_unchanged("s", v2, vec![ChatMessage::system("rewritten")])
                .await
        );
        assert_eq!(llm.history_snapshot("s").await.len(), 1);
    }
}

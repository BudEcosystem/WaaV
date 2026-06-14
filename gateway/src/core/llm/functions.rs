//! Server-side function/tool-call orchestration (PIPECAT_FIX_PLAN B-G4).
//!
//! Mirrors Pipecat's `run_function_calls` semantics, Rust-shaped:
//! - **Batch → ONE re-inference**: every tool call in one assistant response
//!   is a group; all results land in history first, then a single follow-up
//!   inference runs (Pipecat's `group_id` / last-sibling rule — N results,
//!   1 inference).
//! - **Parallel or sequential** execution within the batch.
//! - **Missing handler / bad args / timeout → terminal error result**, so the
//!   turn always completes instead of hanging the conversation.
//! - **Async tools** (`cancel_on_interruption = false`): the handler is
//!   spawned detached, the LLM immediately receives a `started` ack as the
//!   tool result (the turn completes — "let me check that"), and the real
//!   result is delivered later through the [`AsyncFinalSink`], which the
//!   orchestration layer turns into a follow-up inference. A built-in
//!   `cancel_async_tool_call` tool is advertised while ≥1 async tool is
//!   registered and aborts an in-flight call by id.
//! - **Pairing invariant**: every `tool_call_id` in the assistant message
//!   gets EXACTLY one tool-result message, in batch order — the shape every
//!   vendor adapter (OpenAI pass-through, Anthropic `tool_result`, Gemini
//!   `function_response`) renders correctly from (B-G1).
//!
//! The registry rides [`LlmClient`](super::LlmClient) (`with_functions`), so
//! every consumer — conversation orchestrator, DAG LLM node, flows (B-G3) —
//! gets identical behavior; nothing is per-provider or per-path.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::{LlmClient, LlmResponse, LlmResult, ToolCall, ToolDefinition};

/// Name of the auto-injected cancel tool (Pipecat parity).
pub const CANCEL_ASYNC_TOOL_NAME: &str = "cancel_async_tool_call";

/// What a handler receives for one call.
pub struct FunctionCallParams {
    pub session_id: String,
    pub tool_call_id: String,
    pub name: String,
    /// Parsed from the provider's JSON-string arguments.
    pub arguments: Value,
    /// Cancelled on barge-in/teardown for `cancel_on_interruption` tools;
    /// a fresh detached token for async tools (they survive interruption,
    /// cancellable only via `cancel_async_tool_call`).
    pub cancel: CancellationToken,
}

/// Ok(value) becomes the tool result; Err(msg) becomes a terminal
/// `{"error": msg}` result (the turn still completes).
pub type FunctionResult = Result<Value, String>;

/// Async handler. Must be cheap to clone (Arc'd).
pub type FunctionHandler = Arc<
    dyn Fn(FunctionCallParams) -> Pin<Box<dyn Future<Output = FunctionResult> + Send>>
        + Send
        + Sync,
>;

/// A delivered async-tool result (final or progress). The orchestration layer
/// wires a sink to append it to context and — turn-id-gated — volunteer it.
#[derive(Clone, Debug)]
pub struct AsyncToolResult {
    pub session_id: String,
    /// S3 (M6): the turn that SPAWNED the tool. The orchestrator volunteers the
    /// result only while this is still the latest turn — a stale RAG answer must
    /// never talk over a new topic.
    pub turn_id: u64,
    pub tool_call_id: String,
    pub name: String,
    pub is_final: bool,
    /// S3: whether a follow-up inference should run on the final (Pipecat's
    /// `run_llm`). `false` ⇒ record-only (chain tools without re-inference).
    pub run_llm: bool,
    pub value: Value,
}

/// Delivery sink for async-tool finals/progress. Wired by the orchestration
/// layer to append context + (turn-id-gated) trigger a follow-up inference.
pub type AsyncFinalSink = Arc<dyn Fn(AsyncToolResult) + Send + Sync>;

/// One registered function.
pub struct RegistryItem {
    /// Advertised to the LLM in every request while registered.
    pub definition: ToolDefinition,
    pub handler: FunctionHandler,
    /// `true` (default): the call is cancelled by barge-in/teardown.
    /// `false`: an ASYNC tool — spawned detached, acks `started`
    /// immediately, delivers its final through the [`AsyncFinalSink`].
    pub cancel_on_interruption: bool,
    /// Per-call execution budget; expiry yields a terminal error result.
    pub timeout: Duration,
    /// Bookkeeping for auto-registered (derived) tools.
    pub auto_registered: bool,
    /// S3: for an ASYNC tool, whether its final triggers a follow-up inference
    /// (the bot volunteers the result). `false` ⇒ record-only: the result lands
    /// in history but the bot stays silent (chain tools without inference).
    /// Ignored for synchronous tools. Default `true`.
    pub run_llm: bool,
}

impl RegistryItem {
    pub fn new(definition: ToolDefinition, handler: FunctionHandler) -> Self {
        Self {
            definition,
            handler,
            cancel_on_interruption: true,
            timeout: Duration::from_secs(30),
            auto_registered: false,
            run_llm: true,
        }
    }

    pub fn asynchronous(mut self) -> Self {
        self.cancel_on_interruption = false;
        self
    }

    /// S3: mark an async tool record-only — its final lands in history but the
    /// bot does not volunteer it (no follow-up inference).
    pub fn record_only(mut self) -> Self {
        self.run_llm = false;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// In-flight async call bookkeeping (abortable via the builtin cancel tool).
struct InFlightAsync {
    name: String,
    abort: tokio::task::AbortHandle,
}

/// Session-scoped function registry. One per `LlmClient` (or shared).
#[derive(Default)]
pub struct FunctionRegistry {
    items: RwLock<HashMap<String, Arc<RegistryItem>>>,
    async_sink: RwLock<Option<AsyncFinalSink>>,
    in_flight_async: RwLock<HashMap<String, InFlightAsync>>,
}

impl std::fmt::Debug for FunctionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FunctionRegistry")
            .field("functions", &self.items.read().len())
            .field("in_flight_async", &self.in_flight_async.read().len())
            .finish()
    }
}

impl FunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a synchronous-semantics tool (cancelled on interruption).
    pub fn register(&self, definition: ToolDefinition, handler: FunctionHandler) {
        self.register_item(RegistryItem::new(definition, handler));
    }

    /// Register with full control (async tools, timeouts, ...).
    pub fn register_item(&self, item: RegistryItem) {
        let name = item.definition.function.name.clone();
        self.items.write().insert(name, Arc::new(item));
    }

    pub fn unregister(&self, name: &str) -> bool {
        self.items.write().remove(name).is_some()
    }

    /// Replace the ENTIRE tool set in one write-lock acquisition (B-G3
    /// flows: tools must swap atomically with the node's context — a
    /// request rendered mid-swap must never see a mix of old and new
    /// node tools).
    pub fn swap_tools(&self, items: Vec<RegistryItem>) {
        let mut map = self.items.write();
        map.clear();
        for item in items {
            let name = item.definition.function.name.clone();
            map.insert(name, Arc::new(item));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.read().is_empty()
    }

    /// Any registered tool that survives interruption?
    pub fn has_async_tools(&self) -> bool {
        self.items.read().values().any(|i| !i.cancel_on_interruption)
    }

    /// Where async finals/progress land. The orchestration layer wires this
    /// to context-append + follow-up inference.
    pub fn set_async_sink(&self, sink: AsyncFinalSink) {
        *self.async_sink.write() = Some(sink);
    }

    fn get(&self, name: &str) -> Option<Arc<RegistryItem>> {
        self.items.read().get(name).cloned()
    }

    /// The tool definitions to advertise in a request: every registered tool,
    /// plus the builtin `cancel_async_tool_call` while ≥1 async tool exists
    /// (Pipecat parity — present exactly while it could do something).
    pub fn request_tools(&self) -> Vec<ToolDefinition> {
        let items = self.items.read();
        let mut tools: Vec<ToolDefinition> =
            items.values().map(|i| i.definition.clone()).collect();
        // Deterministic order for request reproducibility.
        tools.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        if items.values().any(|i| !i.cancel_on_interruption) {
            tools.push(ToolDefinition::function(
                CANCEL_ASYNC_TOOL_NAME,
                "Cancel an in-flight asynchronous tool call that has not yet \
                 delivered its final result.",
                json!({
                    "type": "object",
                    "properties": {
                        "tool_call_id": {
                            "type": "string",
                            "description": "The id of the async tool call to cancel."
                        }
                    },
                    "required": ["tool_call_id"]
                }),
            ));
        }
        tools
    }

    /// Abort one in-flight async call. Returns whether it existed.
    pub fn cancel_async(&self, tool_call_id: &str) -> bool {
        if let Some(call) = self.in_flight_async.write().remove(tool_call_id) {
            call.abort.abort();
            info!(tool_call_id, name = %call.name, "async tool call cancelled");
            true
        } else {
            false
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn deliver_async(
        &self,
        session_id: &str,
        turn_id: u64,
        tool_call_id: &str,
        name: &str,
        is_final: bool,
        run_llm: bool,
        value: Value,
    ) {
        if is_final {
            self.in_flight_async.write().remove(tool_call_id);
        }
        match &*self.async_sink.read() {
            Some(sink) => sink(AsyncToolResult {
                session_id: session_id.to_string(),
                turn_id,
                tool_call_id: tool_call_id.to_string(),
                name: name.to_string(),
                is_final,
                run_llm,
                value,
            }),
            None => warn!(
                tool_call_id,
                name, "async tool result dropped: no AsyncFinalSink wired"
            ),
        }
    }
}

/// Tool-loop options. `parallel` matches Pipecat's `run_in_parallel=True`
/// default; `max_rounds` bounds pathological call-me-again loops.
#[derive(Clone, Copy, Debug)]
pub struct ToolLoopOptions {
    pub parallel: bool,
    pub max_rounds: usize,
    /// S3 (M6): the conversation turn running this loop. Stamped onto any async
    /// tool spawned in it, so the orchestrator can turn-id-gate the follow-up.
    /// `0` = no gating context (DAG flows, tests).
    pub turn_id: u64,
}

impl Default for ToolLoopOptions {
    fn default() -> Self {
        Self { parallel: true, max_rounds: 8, turn_id: 0 }
    }
}

/// Execute the full tool loop for a response that contained tool calls:
/// run each batch, write every result into the session history (pairing
/// invariant), re-infer ONCE per batch, repeat until a content response (or
/// `max_rounds`). Returns the final response.
///
/// The assistant message carrying the tool calls is already in history
/// (recorded by `complete*`); this only appends results + re-infers.
pub async fn run_tool_loop(
    llm: &LlmClient,
    registry: &Arc<FunctionRegistry>,
    session_id: &str,
    mut response: LlmResponse,
    api_key: Option<&str>,
    cancel: &CancellationToken,
    opts: ToolLoopOptions,
) -> LlmResult<LlmResponse> {
    let mut rounds = 0usize;
    while !response.tool_calls.is_empty() {
        if rounds >= opts.max_rounds {
            // CRITICAL (review wf_6783a4b3): the assistant{tool_calls}
            // message is ALREADY in history — bailing without results leaves
            // it unpaired and every later request 400s on strict providers
            // (session permanently bricked). Write synthetic terminal
            // results so pairing holds, then stop.
            warn!(
                session = session_id,
                rounds, "tool loop hit max_rounds; writing terminal results and stopping"
            );
            let results: Vec<(String, String)> = response
                .tool_calls
                .iter()
                .map(|c| {
                    (
                        c.id.clone(),
                        json!({"error": "aborted: tool loop reached max_rounds"}).to_string(),
                    )
                })
                .collect();
            llm.add_tool_results_batch(session_id, results).await;
            response.tool_calls.clear();
            break;
        }
        rounds += 1;
        let batch = std::mem::take(&mut response.tool_calls);
        debug!(
            session = session_id,
            round = rounds,
            calls = batch.len(),
            parallel = opts.parallel,
            "executing tool-call batch"
        );

        let results =
            execute_batch(registry, session_id, &batch, cancel, opts.parallel, opts.turn_id).await;

        // Pairing invariant: exactly one result per tool_call_id, batch
        // order — appended under ONE history-lock acquisition so a
        // concurrent turn's user message can never interleave between a
        // batch's results (review wf_6783a4b3).
        let rendered: Vec<(String, String)> = batch
            .iter()
            .zip(results)
            .map(|(call, value)| (call.id.clone(), value.to_string()))
            .collect();
        llm.add_tool_results_batch(session_id, rendered).await;

        // ONE re-inference for the whole batch (group_id semantics).
        response = llm
            .continue_from_history(session_id, api_key, cancel, None)
            .await?;
    }
    Ok(response)
}

/// Execute one batch; returns one JSON value per call, in order.
/// `pub(crate)`: the flow layer (B-G3) runs its OWN loop around this — an
/// edge transition must NOT re-infer on the old node's context.
pub(crate) async fn execute_batch(
    registry: &Arc<FunctionRegistry>,
    session_id: &str,
    batch: &[ToolCall],
    cancel: &CancellationToken,
    parallel: bool,
    turn_id: u64,
) -> Vec<Value> {
    let futs: Vec<_> = batch
        .iter()
        .map(|call| execute_one(registry, session_id, call, cancel, turn_id))
        .collect();
    if parallel {
        futures::future::join_all(futs).await
    } else {
        let mut out = Vec::with_capacity(futs.len());
        for fut in futs {
            out.push(fut.await);
        }
        out
    }
}

async fn execute_one(
    registry: &Arc<FunctionRegistry>,
    session_id: &str,
    call: &ToolCall,
    cancel: &CancellationToken,
    turn_id: u64,
) -> Value {
    let name = call.function.name.as_str();

    // The builtin cancel tool is intercepted before registry lookup.
    if name == CANCEL_ASYNC_TOOL_NAME {
        let target = serde_json::from_str::<Value>(&call.function.arguments)
            .ok()
            .and_then(|v| v.get("tool_call_id").and_then(|s| s.as_str()).map(String::from));
        return match target {
            Some(id) => json!({ "cancelled": registry.cancel_async(&id), "tool_call_id": id }),
            None => json!({ "error": "cancel_async_tool_call requires a tool_call_id argument" }),
        };
    }

    let Some(item) = registry.get(name) else {
        // Missing-handler safety: terminal result so the turn completes.
        warn!(function = name, "tool call to UNREGISTERED function");
        return json!({
            "error": format!("No handler registered for function '{name}'.")
        });
    };

    let arguments = if call.function.arguments.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str::<Value>(&call.function.arguments) {
            Ok(v) => v,
            Err(e) => {
                return json!({
                    "error": format!("Invalid JSON arguments for '{name}': {e}")
                });
            }
        }
    };

    if !item.cancel_on_interruption {
        // ASYNC tool: spawn detached (fresh token — survives barge-in), ack
        // `started` immediately so the turn completes; the final result rides
        // the AsyncFinalSink. Abortable via the builtin cancel tool.
        let params = FunctionCallParams {
            session_id: session_id.to_string(),
            tool_call_id: call.id.clone(),
            name: name.to_string(),
            arguments,
            cancel: CancellationToken::new(),
        };
        let handler = Arc::clone(&item.handler);
        let timeout = item.timeout;
        let run_llm = item.run_llm;
        let reg = Arc::clone(registry);
        let (sid, cid, fname) = (
            session_id.to_string(),
            call.id.clone(),
            name.to_string(),
        );
        let task = tokio::spawn(async move {
            let outcome = match tokio::time::timeout(timeout, handler(params)).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => json!({ "error": e }),
                Err(_) => json!({
                    "error": format!("async function '{fname}' timed out after {timeout:?}")
                }),
            };
            reg.deliver_async(&sid, turn_id, &cid, &fname, true, run_llm, outcome);
        });
        registry.in_flight_async.write().insert(
            call.id.clone(),
            InFlightAsync { name: name.to_string(), abort: task.abort_handle() },
        );
        return json!({
            "status": "started",
            "message": format!(
                "Asynchronous function '{name}' started; the result will be \
                 delivered in a follow-up message when ready."
            )
        });
    }

    // Synchronous-semantics tool: runs under the turn's cancellation token
    // with the item's timeout; every exit shape is a terminal result.
    let params = FunctionCallParams {
        session_id: session_id.to_string(),
        tool_call_id: call.id.clone(),
        name: name.to_string(),
        arguments,
        cancel: cancel.clone(),
    };
    let fut = (item.handler)(params);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => json!({
            "error": format!("function '{name}' cancelled by interruption")
        }),
        r = tokio::time::timeout(item.timeout, fut) => match r {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => json!({ "error": e }),
            Err(_) => json!({
                "error": format!("function '{name}' timed out after {:?}", item.timeout)
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::llm::{LlmClientConfig, MessageRole};
    use axum::{Json, Router, extract::State, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::TcpListener;

    fn handler_returning(value: Value) -> FunctionHandler {
        Arc::new(move |_p: FunctionCallParams| {
            let v = value.clone();
            Box::pin(async move { Ok(v) })
        })
    }

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition::function(name, format!("test tool {name}"), json!({"type":"object"}))
    }

    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: crate::core::llm::FunctionCall {
                name: name.into(),
                arguments: args.into(),
            },
        }
    }

    // ── Mock chat/completions: first N requests return a tool-call batch,
    //    then a content answer. Counts every inference. ──────────────────
    #[derive(Clone)]
    struct MockLlm {
        requests: Arc<parking_lot::Mutex<Vec<Value>>>,
        tool_rounds: Arc<AtomicUsize>,
    }

    async fn start_mock(tool_rounds: usize) -> (String, MockLlm) {
        let state = MockLlm {
            requests: Arc::new(parking_lot::Mutex::new(Vec::new())),
            tool_rounds: Arc::new(AtomicUsize::new(tool_rounds)),
        };
        async fn chat(State(st): State<MockLlm>, Json(req): Json<Value>) -> Json<Value> {
            st.requests.lock().push(req);
            let remaining = st
                .tool_rounds
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)))
                .unwrap();
            let message = if remaining > 0 {
                json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {"id": "call_a", "type": "function",
                         "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}},
                        {"id": "call_b", "type": "function",
                         "function": {"name": "get_time", "arguments": "{}"}}
                    ]
                })
            } else {
                json!({"role": "assistant", "content": "It is sunny and 12:00."})
            };
            Json(json!({
                "id": "chatcmpl-mock", "object": "chat.completion", "created": 0,
                "model": "mock",
                "choices": [{"index": 0, "message": message,
                             "finish_reason": if remaining > 0 {"tool_calls"} else {"stop"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }))
        }
        let app = Router::new().route("/chat/completions", post(chat)).with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://127.0.0.1:{}", addr.port()), state)
    }

    fn client(base_url: &str, registry: Arc<FunctionRegistry>) -> LlmClient {
        LlmClient::new(LlmClientConfig {
            base_url: base_url.into(),
            api_key: Some("test".into()),
            model: "mock".into(),
            streaming: false,
            ..Default::default()
        })
        .with_functions(registry)
    }

    // ── registry semantics ────────────────────────────────────────────────

    #[test]
    fn cancel_async_tool_registered_only_while_async_present() {
        let reg = FunctionRegistry::new();
        reg.register(def("sync_tool"), handler_returning(json!({"ok": true})));
        let names: Vec<String> =
            reg.request_tools().iter().map(|t| t.function.name.clone()).collect();
        assert!(
            !names.contains(&CANCEL_ASYNC_TOOL_NAME.to_string()),
            "no async tools → no cancel builtin"
        );

        reg.register_item(
            RegistryItem::new(def("async_tool"), handler_returning(json!(1))).asynchronous(),
        );
        let names: Vec<String> =
            reg.request_tools().iter().map(|t| t.function.name.clone()).collect();
        assert!(names.contains(&CANCEL_ASYNC_TOOL_NAME.to_string()));

        reg.unregister("async_tool");
        let names: Vec<String> =
            reg.request_tools().iter().map(|t| t.function.name.clone()).collect();
        assert!(
            !names.contains(&CANCEL_ASYNC_TOOL_NAME.to_string()),
            "last async tool unregistered → builtin pruned"
        );
    }

    // ── batch execution ───────────────────────────────────────────────────

    #[tokio::test]
    async fn unknown_tool_gets_terminal_result_and_turn_completes() {
        let (url, mock) = start_mock(1).await;
        let reg = Arc::new(FunctionRegistry::new());
        // NOTHING registered: both calls must yield terminal error results,
        // and the loop must still re-infer to a final answer.
        let llm = client(&url, Arc::clone(&reg));
        let token = CancellationToken::new();
        let first = llm.complete("s1", "what's the weather?", None, &token, None).await.unwrap();
        assert_eq!(first.tool_calls.len(), 2);

        let final_resp =
            run_tool_loop(&llm, &reg, "s1", first, None, &token, ToolLoopOptions::default())
                .await
                .unwrap();
        assert_eq!(final_resp.content, "It is sunny and 12:00.");
        assert_eq!(mock.requests.lock().len(), 2, "initial + exactly ONE re-inference");

        // Pairing invariant: one tool message per call id, error-shaped.
        let history = llm.history_snapshot("s1").await;
        let tool_msgs: Vec<_> =
            history.iter().filter(|m| matches!(m.role, MessageRole::Tool)).collect();
        assert_eq!(tool_msgs.len(), 2);
        for m in &tool_msgs {
            assert!(
                m.content.as_deref().unwrap_or_default().contains("No handler registered"),
                "missing handler must produce a terminal error result"
            );
        }
    }

    #[tokio::test]
    async fn batch_of_tool_calls_reinfers_once_per_round() {
        // TWO tool rounds: round 1 (2 calls) → 1 re-inference which returns
        // 2 more calls → round 2 → 1 re-inference with the final answer.
        // Total inferences: initial + 2 = 3 (never one per result = 5).
        let (url, mock) = start_mock(2).await;
        let reg = Arc::new(FunctionRegistry::new());
        reg.register(def("get_weather"), handler_returning(json!({"weather": "sunny"})));
        reg.register(def("get_time"), handler_returning(json!({"time": "12:00"})));
        let llm = client(&url, Arc::clone(&reg));
        let token = CancellationToken::new();

        let first = llm.complete("s1", "weather and time?", None, &token, None).await.unwrap();
        let final_resp =
            run_tool_loop(&llm, &reg, "s1", first, None, &token, ToolLoopOptions::default())
                .await
                .unwrap();
        assert_eq!(final_resp.content, "It is sunny and 12:00.");
        assert_eq!(
            mock.requests.lock().len(),
            3,
            "N results per batch must trigger ONE re-inference per batch"
        );

        // The re-inference request must carry the tool results (pairing).
        let second_req = mock.requests.lock()[1].clone();
        let msgs = second_req["messages"].as_array().unwrap().clone();
        let tool_msgs: Vec<_> = msgs.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 2);
        assert_eq!(tool_msgs[0]["tool_call_id"], "call_a");
        assert_eq!(tool_msgs[1]["tool_call_id"], "call_b");
    }

    #[tokio::test]
    async fn registry_tools_are_advertised_in_requests() {
        let (url, mock) = start_mock(0).await;
        let reg = Arc::new(FunctionRegistry::new());
        reg.register(def("get_weather"), handler_returning(json!(1)));
        let llm = client(&url, Arc::clone(&reg));
        let token = CancellationToken::new();
        let _ = llm.complete("s1", "hi", None, &token, None).await.unwrap();
        let req = mock.requests.lock()[0].clone();
        let tools = req["tools"].as_array().expect("tools advertised").clone();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "get_weather");
    }

    #[tokio::test]
    async fn sequential_runner_executes_in_order_parallel_concurrently() {
        // Sequential: a slow first call must FINISH before the second starts.
        // Parallel: both run concurrently (wall time ~ slowest, not sum).
        let order: Arc<parking_lot::Mutex<Vec<&'static str>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let reg = Arc::new(FunctionRegistry::new());
        let o1 = Arc::clone(&order);
        reg.register(
            def("slow"),
            Arc::new(move |_p| {
                let o = Arc::clone(&o1);
                Box::pin(async move {
                    o.lock().push("slow_start");
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    o.lock().push("slow_end");
                    Ok(json!(1))
                })
            }),
        );
        let o2 = Arc::clone(&order);
        reg.register(
            def("fast"),
            Arc::new(move |_p| {
                let o = Arc::clone(&o2);
                Box::pin(async move {
                    o.lock().push("fast_start");
                    Ok(json!(2))
                })
            }),
        );
        let batch = vec![call("c1", "slow", "{}"), call("c2", "fast", "{}")];
        let token = CancellationToken::new();

        let _ = execute_batch(&reg, "s", &batch, &token, false, 0).await;
        assert_eq!(
            *order.lock(),
            vec!["slow_start", "slow_end", "fast_start"],
            "sequential: strict batch order"
        );

        order.lock().clear();
        let _ = execute_batch(&reg, "s", &batch, &token, true, 0).await;
        assert_eq!(
            *order.lock(),
            vec!["slow_start", "fast_start", "slow_end"],
            "parallel: fast starts while slow is still running"
        );
    }

    #[tokio::test]
    async fn tool_timeout_and_bad_args_yield_terminal_results() {
        let reg = Arc::new(FunctionRegistry::new());
        reg.register_item(
            RegistryItem::new(
                def("hang"),
                Arc::new(|_p| {
                    Box::pin(async {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        Ok(json!(1))
                    })
                }),
            )
            .with_timeout(Duration::from_millis(50)),
        );
        reg.register(def("typed"), handler_returning(json!(1)));
        let token = CancellationToken::new();

        let out =
            execute_batch(&reg, "s", &[call("c1", "hang", "{}")], &token, true, 0).await;
        assert!(out[0]["error"].as_str().unwrap().contains("timed out"));

        let out =
            execute_batch(&reg, "s", &[call("c2", "typed", "{not json")], &token, true, 0).await;
        assert!(out[0]["error"].as_str().unwrap().contains("Invalid JSON arguments"));
    }

    #[tokio::test]
    async fn interruption_cancels_sync_tools() {
        let reg = Arc::new(FunctionRegistry::new());
        reg.register(
            def("slow"),
            Arc::new(|_p| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(json!(1))
                })
            }),
        );
        let token = CancellationToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            t2.cancel();
        });
        let started = std::time::Instant::now();
        let out = execute_batch(&reg, "s", &[call("c1", "slow", "{}")], &token, true, 0).await;
        assert!(out[0]["error"].as_str().unwrap().contains("cancelled by interruption"));
        assert!(started.elapsed() < Duration::from_secs(5), "cancel must be prompt");
    }

    #[tokio::test]
    async fn async_tool_acks_started_and_delivers_final_via_sink() {
        let reg = Arc::new(FunctionRegistry::new());
        reg.register_item(
            RegistryItem::new(
                def("lookup"),
                Arc::new(|_p| {
                    Box::pin(async {
                        tokio::time::sleep(Duration::from_millis(40)).await;
                        Ok(json!({"answer": 42}))
                    })
                }),
            )
            .asynchronous(),
        );
        let finals: Arc<parking_lot::Mutex<Vec<AsyncToolResult>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink_store = Arc::clone(&finals);
        reg.set_async_sink(Arc::new(move |r: AsyncToolResult| {
            sink_store.lock().push(r);
        }));

        let token = CancellationToken::new();
        // The ack returns IMMEDIATELY (well under the 40ms handler).
        let started = std::time::Instant::now();
        // turn_id 7 must round-trip to the sink for turn-id gating (S3).
        let out = execute_batch(&reg, "s", &[call("c9", "lookup", "{}")], &token, true, 7).await;
        assert!(started.elapsed() < Duration::from_millis(35), "started ack must not wait");
        assert_eq!(out[0]["status"], "started");

        // Interruption does NOT cancel it; the final arrives via the sink.
        token.cancel();
        tokio::time::sleep(Duration::from_millis(120)).await;
        let finals = finals.lock();
        assert_eq!(finals.len(), 1, "exactly one final");
        let r = &finals[0];
        assert_eq!(r.tool_call_id, "c9");
        assert_eq!(r.name, "lookup");
        assert_eq!(r.turn_id, 7, "the spawning turn id must reach the sink");
        assert!(r.run_llm, "default async tool runs the follow-up");
        assert!(r.is_final);
        assert_eq!(r.value["answer"], 42);
    }

    #[tokio::test]
    async fn cancel_async_tool_call_aborts_in_flight() {
        let reg = Arc::new(FunctionRegistry::new());
        reg.register_item(
            RegistryItem::new(
                def("eternal"),
                Arc::new(|_p| {
                    Box::pin(async {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        Ok(json!(1))
                    })
                }),
            )
            .asynchronous(),
        );
        let finals = Arc::new(AtomicUsize::new(0));
        let fcount = Arc::clone(&finals);
        reg.set_async_sink(Arc::new(move |_r: AsyncToolResult| {
            fcount.fetch_add(1, Ordering::SeqCst);
        }));
        let token = CancellationToken::new();
        let _ = execute_batch(&reg, "s", &[call("c1", "eternal", "{}")], &token, true, 0).await;

        // The LLM calls the builtin to cancel it.
        let out = execute_batch(
            &reg,
            "s",
            &[call("c2", CANCEL_ASYNC_TOOL_NAME, r#"{"tool_call_id":"c1"}"#)],
            &token,
            true,
            0,
        )
        .await;
        assert_eq!(out[0]["cancelled"], true);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(finals.load(Ordering::SeqCst), 0, "aborted task delivers no final");
        // Cancelling again: nothing in flight.
        assert!(!reg.cancel_async("c1"));
    }

    #[tokio::test]
    async fn max_rounds_bounds_pathological_loops() {
        // Mock that ALWAYS returns tool calls: the loop must stop at
        // max_rounds instead of spinning forever.
        let (url, mock) = start_mock(usize::MAX).await;
        let reg = Arc::new(FunctionRegistry::new());
        reg.register(def("get_weather"), handler_returning(json!(1)));
        reg.register(def("get_time"), handler_returning(json!(2)));
        let llm = client(&url, Arc::clone(&reg));
        let token = CancellationToken::new();
        let first = llm.complete("s1", "go", None, &token, None).await.unwrap();
        let opts = ToolLoopOptions { parallel: true, max_rounds: 3, turn_id: 0 };
        let resp = run_tool_loop(&llm, &reg, "s1", first, None, &token, opts).await.unwrap();
        assert!(
            resp.tool_calls.is_empty(),
            "capped loop must not hand back live tool calls (they were terminally answered)"
        );
        assert_eq!(mock.requests.lock().len(), 1 + 3, "initial + max_rounds inferences");

        // CRITICAL pairing pin (review wf_6783a4b3): the bail must leave NO
        // unpaired assistant{tool_calls} — every call id in history has a
        // tool result, so the session is NOT bricked on strict providers.
        let history = llm.history_snapshot("s1").await;
        let result_ids: std::collections::HashSet<&str> = history
            .iter()
            .filter(|m| matches!(m.role, MessageRole::Tool))
            .filter_map(|m| m.tool_call_id.as_deref())
            .collect();
        for m in &history {
            if let Some(calls) = &m.tool_calls {
                for c in calls {
                    assert!(
                        result_ids.contains(c.id.as_str()),
                        "unpaired tool call {} after max_rounds bail: {history:?}",
                        c.id
                    );
                }
            }
        }
        let last_results: Vec<_> = history
            .iter()
            .rev()
            .take_while(|m| matches!(m.role, MessageRole::Tool))
            .collect();
        assert_eq!(last_results.len(), 2, "the final batch got terminal results");
        assert!(
            last_results[0]
                .content
                .as_deref()
                .unwrap()
                .contains("max_rounds"),
            "terminal results say why"
        );
    }

    #[test]
    fn history_trim_never_orphans_tool_messages() {
        // review wf_6783a4b3: count-based eviction crossing a tool exchange
        // must take the whole exchange, never leaving a leading orphan tool
        // result (strict providers 400 on it forever).
        use crate::core::llm::{ChatMessage, ConversationHistory, MessageRole};
        let mut h = ConversationHistory::new(4);
        h.add(ChatMessage::system("p"));
        h.add(ChatMessage::user("q1"));
        let mut call = ChatMessage::assistant("");
        call.tool_calls = Some(vec![crate::core::llm::ToolCall {
            id: "c1".into(),
            call_type: "function".into(),
            function: crate::core::llm::FunctionCall { name: "f".into(), arguments: "{}".into() },
        }]);
        h.add(call);
        h.add(ChatMessage::tool("c1", "result"));
        // Over capacity: evictions start. q1 goes, then the assistant{call}
        // — its orphaned result MUST go with it.
        h.add(ChatMessage::user("q2"));
        h.add(ChatMessage::assistant("a2"));
        let msgs = h.messages();
        assert!(
            !msgs
                .iter()
                .enumerate()
                .any(|(i, m)| m.role == MessageRole::Tool
                    && !msgs[..i].iter().any(|p| p
                        .tool_calls
                        .as_ref()
                        .is_some_and(|cs| cs.iter().any(|c| Some(c.id.as_str())
                            == m.tool_call_id.as_deref())))),
            "orphan tool message survived the trim: {msgs:?}"
        );
        assert_eq!(msgs[0].role, MessageRole::System, "system always survives");
    }
}

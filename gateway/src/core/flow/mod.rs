//! Structured conversation flows (PIPECAT_FIX_PLAN B-G3).
//!
//! A conversation STATE MACHINE layered on the canonical LLM context — not
//! the data-flow DAG. Mirrors `pipecat-flows`' `FlowManager`:
//!
//! - A [`NodeConfig`] describes one conversation state: its objective
//!   (`task_messages`), persona (`role_message` → the session's system
//!   instruction, persisting until changed), the functions available THERE,
//!   pre/post actions, how its messages combine with prior context
//!   ([`ContextStrategy`]), and whether entering it immediately runs an
//!   inference (`respond_immediately`).
//! - A [`FlowFunction`] returns `(result, Option<NodeConfig>)`. `Some(next)`
//!   makes it an EDGE function: the result lands in context but NO inference
//!   runs on the old node — the transition fires first (two-phase), then the
//!   NEW node decides (`respond_immediately`). `None` is a NODE function:
//!   stay put and re-infer with the result (B-G4 loop semantics).
//! - Tools swap ATOMICALLY with the node (one registry write-lock), so a
//!   request rendered mid-transition can never see a mix of two nodes'
//!   tools.
//!
//! The flow OWNS the session context through the vendor-neutral
//! [`LlmClient`] mutation APIs, so per-vendor rendering (B-G1) keeps
//! working, and it DRIVES the same [`FunctionRegistry`] as B-G4 — one tool
//! machinery for every path.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::core::llm::functions::{FunctionRegistry, RegistryItem, execute_batch};
use crate::core::llm::{
    ChatMessage, LlmClient, LlmResponse, LlmResult, MessageRole, ToolDefinition,
};

/// How a node's `task_messages` combine with the existing context.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextStrategy {
    /// Keep history; append the node's task messages.
    Append,
    /// Replace the context with system + task messages (prior turns gone).
    Reset,
    /// Reserved for B-G6 (native summarizer). Until then behaves as
    /// [`Self::Reset`] with a loud log — matching pipecat-flows, where this
    /// strategy is deprecated in favor of the context summarizer.
    ResetWithSummary { summary_prompt: String },
}

/// A side effect executed on node entry/exit (speak a canned line, clear
/// TTS, ...). Interpreted by the [`ActionSink`] the embedder wires.
#[derive(Clone, Debug, PartialEq)]
pub enum ActionConfig {
    /// Speak a fixed utterance (pre-action greeting, disclaimers, ...).
    TtsSay(String),
    /// Clear queued/playing TTS.
    ClearTts,
}

/// Executes [`ActionConfig`]s. Wired by the orchestration layer (it owns
/// the VoiceManager); flows stay core-pure and testable.
pub type ActionSink =
    Arc<dyn Fn(ActionConfig) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A flow function's outcome: the tool result the LLM sees, plus an
/// optional transition target (EDGE function).
pub type FlowFnResult = (Value, Option<NodeConfig>);

/// Flow function handler.
pub type FlowFunction = Arc<
    dyn Fn(crate::core::llm::FunctionCallParams) -> Pin<Box<dyn Future<Output = FlowFnResult> + Send>>
        + Send
        + Sync,
>;

/// One callable function attached to a node (or globally).
#[derive(Clone)]
pub struct FlowFunctionSchema {
    pub definition: ToolDefinition,
    pub handler: FlowFunction,
}

impl FlowFunctionSchema {
    pub fn new(definition: ToolDefinition, handler: FlowFunction) -> Self {
        Self { definition, handler }
    }
}

/// One conversation state.
#[derive(Clone)]
pub struct NodeConfig {
    pub name: Option<String>,
    /// Persona → the session's SYSTEM instruction. Persists across nodes
    /// until another node sets one (pipecat-flows `role_message`).
    pub role_message: Option<String>,
    /// The node's objective, appended (or reset-into) the context on entry.
    pub task_messages: Vec<ChatMessage>,
    pub functions: Vec<FlowFunctionSchema>,
    pub pre_actions: Vec<ActionConfig>,
    pub post_actions: Vec<ActionConfig>,
    pub context_strategy: ContextStrategy,
    /// Run an inference immediately on entry (greeting nodes). `false` =
    /// wait for user input.
    pub respond_immediately: bool,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            name: None,
            role_message: None,
            task_messages: Vec::new(),
            functions: Vec::new(),
            pre_actions: Vec::new(),
            post_actions: Vec::new(),
            context_strategy: ContextStrategy::Append,
            respond_immediately: true,
        }
    }
}

impl std::fmt::Debug for NodeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeConfig")
            .field("name", &self.name)
            .field("functions", &self.functions.len())
            .field("strategy", &self.context_strategy)
            .field("respond_immediately", &self.respond_immediately)
            .finish()
    }
}

/// The conversation state machine. One per session.
pub struct FlowManager {
    llm: Arc<LlmClient>,
    registry: Arc<FunctionRegistry>,
    session_id: String,
    api_key: Option<String>,
    current: Mutex<Option<NodeConfig>>,
    /// Set by an EDGE function during a tool batch; consumed by the flow
    /// loop AFTER the batch's results land (two-phase transition).
    pending_transition: Arc<Mutex<Option<NodeConfig>>>,
    /// Available at every node, alongside the node's own functions.
    global_functions: Vec<FlowFunctionSchema>,
    action_sink: Option<ActionSink>,
}

impl std::fmt::Debug for FlowManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowManager")
            .field("session", &self.session_id)
            .field("node", &self.current.lock().as_ref().and_then(|n| n.name.clone()))
            .finish()
    }
}

impl FlowManager {
    pub fn new(
        llm: Arc<LlmClient>,
        registry: Arc<FunctionRegistry>,
        session_id: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            llm,
            registry,
            session_id: session_id.into(),
            api_key,
            current: Mutex::new(None),
            pending_transition: Arc::new(Mutex::new(None)),
            global_functions: Vec::new(),
            action_sink: None,
        }
    }

    /// Functions available at EVERY node (set before `initialize`).
    pub fn with_global_functions(mut self, functions: Vec<FlowFunctionSchema>) -> Self {
        self.global_functions = functions;
        self
    }

    pub fn with_action_sink(mut self, sink: ActionSink) -> Self {
        self.action_sink = Some(sink);
        self
    }

    /// The current node's name (observability/tests).
    pub fn current_node(&self) -> Option<String> {
        self.current.lock().as_ref().and_then(|n| n.name.clone())
    }

    /// Enter the first node. Returns the greeting inference when the node
    /// responds immediately (the caller speaks it), else `None`.
    pub async fn initialize(
        &self,
        first: NodeConfig,
        cancel: &CancellationToken,
    ) -> LlmResult<Option<LlmResponse>> {
        self.set_node(first, cancel).await
    }

    /// Drive one USER turn through the flow: inference on the current
    /// context, then the flow-aware tool loop (node functions re-infer in
    /// place; edge functions transition first, two-phase). Returns the
    /// response the caller should speak — `None` when a transition landed
    /// on a `respond_immediately = false` node (the flow now waits for the
    /// user).
    pub async fn handle_user_turn(
        &self,
        transcript: &str,
        cancel: &CancellationToken,
    ) -> LlmResult<Option<LlmResponse>> {
        let response = self
            .llm
            .complete(&self.session_id, transcript, self.api_key.as_deref(), cancel, None)
            .await?;
        self.run_flow_loop(response, cancel).await
    }

    /// The flow-aware tool loop (replaces B-G4's `run_tool_loop` here —
    /// an EDGE transition must NOT re-infer on the old node's context).
    async fn run_flow_loop(
        &self,
        mut response: LlmResponse,
        cancel: &CancellationToken,
    ) -> LlmResult<Option<LlmResponse>> {
        const MAX_ROUNDS: usize = 8;
        let mut rounds = 0usize;
        loop {
            if response.tool_calls.is_empty() {
                return Ok(Some(response));
            }
            if rounds >= MAX_ROUNDS {
                warn!(session = %self.session_id, "flow loop hit max rounds");
                return Ok(Some(response));
            }
            rounds += 1;

            let batch = std::mem::take(&mut response.tool_calls);
            // Flow handlers run through the SAME registry/batch machinery as
            // B-G4 (timeouts, terminal results, cancellation); transitions
            // are captured into pending_transition by the wrappers.
            let results =
                execute_batch(&self.registry, &self.session_id, &batch, cancel, true).await;
            for (call, value) in batch.iter().zip(results) {
                self.llm
                    .add_tool_result(&self.session_id, &call.id, &value.to_string())
                    .await;
            }

            // TWO-PHASE: results are in context; NOW the transition fires.
            let next = self.pending_transition.lock().take();
            if let Some(next) = next {
                match self.set_node(next, cancel).await? {
                    Some(resp) => {
                        // respond_immediately inference on the NEW node may
                        // itself call tools — keep looping.
                        response = resp;
                        continue;
                    }
                    // The new node waits for the user: turn over, nothing
                    // to speak from the LLM (pre-actions covered audio).
                    None => return Ok(None),
                }
            }

            // NODE function(s): stay put, re-infer with the results.
            response = self
                .llm
                .continue_from_history(&self.session_id, self.api_key.as_deref(), cancel, None)
                .await?;
        }
    }

    /// Enter a node: pre-actions → tools swapped atomically → persona →
    /// context strategy → post-actions → optional immediate inference.
    async fn set_node(
        &self,
        node: NodeConfig,
        cancel: &CancellationToken,
    ) -> LlmResult<Option<LlmResponse>> {
        info!(session = %self.session_id, node = ?node.name, "flow: entering node");
        // Re-entrancy guard: entering a node clears any stale transition.
        *self.pending_transition.lock() = None;

        // A node boundary makes the previous node's COMPLETED tool exchanges
        // facts, not replayable calls: inline them as assistant text. Two
        // reasons (found by the LIVE 3-node gate): (1) strict
        // OpenAI-compatible providers reject histories containing tool
        // messages when no tools array is sent (HTTP 400 "Tool messages
        // found but no tools provided") — and after the swap the OLD node's
        // tools are rightly gone; (2) leaving the raw calls visible invites
        // the model to call tools that no longer exist.
        self.inline_completed_tool_exchanges().await;

        self.run_actions(&node.pre_actions).await;

        // Tools: the node's functions + globals, swapped in ONE lock.
        let mut items: Vec<RegistryItem> = Vec::new();
        let mut seen: HashMap<String, ()> = HashMap::new();
        for f in node.functions.iter().chain(self.global_functions.iter()) {
            let name = f.definition.function.name.clone();
            if seen.insert(name, ()).is_some() {
                continue; // node overrides a global with the same name
            }
            items.push(RegistryItem::new(
                f.definition.clone(),
                self.wrap_flow_function(f.handler.clone()),
            ));
        }
        self.registry.swap_tools(items);

        // Persona persists until another node changes it.
        if let Some(role) = &node.role_message {
            self.llm.upsert_system(&self.session_id, role).await;
        }

        // Context strategy.
        match &node.context_strategy {
            ContextStrategy::Append => {
                self.llm
                    .append_context(&self.session_id, node.task_messages.clone())
                    .await;
            }
            strategy @ (ContextStrategy::Reset
            | ContextStrategy::ResetWithSummary { .. }) => {
                if matches!(strategy, ContextStrategy::ResetWithSummary { .. }) {
                    warn!(
                        session = %self.session_id,
                        "ResetWithSummary: native summarizer (B-G6) not built; behaving as Reset"
                    );
                }
                // Rebuild: persona system message (the node's, else the one
                // already in context) + the node's task messages.
                let system = match &node.role_message {
                    Some(r) => Some(r.clone()),
                    None => self
                        .llm
                        .history_snapshot(&self.session_id)
                        .await
                        .first()
                        .filter(|m| m.role == crate::core::llm::MessageRole::System)
                        .and_then(|m| m.content.clone()),
                };
                let mut messages = Vec::new();
                if let Some(system) = system {
                    messages.push(ChatMessage::system(system));
                }
                messages.extend(node.task_messages.clone());
                self.llm.set_context(&self.session_id, messages).await;
            }
        }

        let respond = node.respond_immediately;
        *self.current.lock() = Some(node.clone());

        self.run_actions(&node.post_actions).await;

        if respond {
            let resp = self
                .llm
                .continue_from_history(&self.session_id, self.api_key.as_deref(), cancel, None)
                .await?;
            debug!(session = %self.session_id, node = ?node.name, "flow: entry inference done");
            Ok(Some(resp))
        } else {
            Ok(None)
        }
    }

    /// Rewrite completed tool exchanges (assistant tool_calls + their tool
    /// results) into plain assistant text. Role alternation stays valid for
    /// every vendor; the facts stay in context.
    async fn inline_completed_tool_exchanges(&self) {
        let history = self.llm.history_snapshot(&self.session_id).await;
        if !history.iter().any(|m| m.role == MessageRole::Tool) {
            return;
        }
        let mut rewritten: Vec<ChatMessage> = Vec::with_capacity(history.len());
        let mut results: HashMap<String, String> = HashMap::new();
        // Collect tool results by id first (they FOLLOW their call).
        for m in &history {
            if m.role == MessageRole::Tool
                && let Some(id) = &m.tool_call_id
            {
                results.insert(id.clone(), m.content.clone().unwrap_or_default());
            }
        }
        for m in history {
            match m.role {
                MessageRole::Tool => {} // folded into the assistant line below
                MessageRole::Assistant if m.tool_calls.is_some() => {
                    let calls = m.tool_calls.as_deref().unwrap_or_default();
                    let mut text = m.content.clone().unwrap_or_default();
                    for c in calls {
                        let result = results
                            .get(&c.id)
                            .map(String::as_str)
                            .unwrap_or("(no result)");
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&format!(
                            "[called {}({}) -> {}]",
                            c.function.name, c.function.arguments, result
                        ));
                    }
                    rewritten.push(ChatMessage::assistant(text));
                }
                _ => rewritten.push(m),
            }
        }
        self.llm.set_context(&self.session_id, rewritten).await;
    }

    /// Adapt a [`FlowFunction`] to the registry's handler shape: the
    /// transition target is captured into `pending_transition`; the LLM
    /// sees only the result value.
    fn wrap_flow_function(&self, f: FlowFunction) -> crate::core::llm::FunctionHandler {
        let pending = Arc::clone(&self.pending_transition);
        Arc::new(move |params| {
            let f = Arc::clone(&f);
            let pending = Arc::clone(&pending);
            Box::pin(async move {
                let (value, next) = f(params).await;
                if let Some(next) = next {
                    // Last edge in a batch wins (Pipecat: one transition per
                    // group).
                    *pending.lock() = Some(next);
                }
                Ok(value)
            })
        })
    }

    async fn run_actions(&self, actions: &[ActionConfig]) {
        if actions.is_empty() {
            return;
        }
        match &self.action_sink {
            Some(sink) => {
                for a in actions {
                    sink(a.clone()).await;
                }
            }
            None => warn!(
                session = %self.session_id,
                count = actions.len(),
                "flow actions configured but no ActionSink wired; skipped"
            ),
        }
    }
}

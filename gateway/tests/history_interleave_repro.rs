//! Adversarial-verification repro (review finding): history lock granularity —
//! a concurrent turn's user message can land between the assistant tool_calls
//! message and the batch's tool-result appends, producing a tool-after-user
//! pairing violation that persists in stored history.
//!
//! Turn A: complete() returns tool_calls (assistant recorded), run_tool_loop
//! executes the batch (tool handler gated open by the test). While the batch
//! is in flight, Turn B calls complete() on the SAME session — prepare_messages
//! appends user_B under its own lock acquisition. Then the gate releases and
//! Turn A appends tool results. Nothing serializes the two turns' history
//! mutations at turn scope.

use std::sync::Arc;
use std::time::Duration;

use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use waav_gateway::core::llm::{
    FunctionRegistry, LlmClient, LlmClientConfig, MessageRole, ToolDefinition, ToolLoopOptions,
    run_tool_loop,
};

#[derive(Clone)]
struct MockState {
    hits: Arc<std::sync::atomic::AtomicUsize>,
}

/// First request → 2 tool calls; later requests → plain content.
async fn chat(State(st): State<MockState>, Json(_req): Json<Value>) -> Json<Value> {
    let n = st.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let message = if n == 0 {
        json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [
                {"id": "call_a1", "type": "function",
                 "function": {"name": "slow_tool", "arguments": "{}"}},
                {"id": "call_a2", "type": "function",
                 "function": {"name": "fast_tool", "arguments": "{}"}}
            ]
        })
    } else {
        json!({"role": "assistant", "content": "done"})
    };
    Json(json!({
        "id": "x", "object": "chat.completion", "created": 0, "model": "mock",
        "choices": [{"index": 0, "message": message,
                     "finish_reason": if n == 0 {"tool_calls"} else {"stop"}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
}

#[tokio::test]
async fn concurrent_turn_user_message_interleaves_with_tool_result_appends() {
    let state = MockState { hits: Arc::new(std::sync::atomic::AtomicUsize::new(0)) };
    let app = Router::new().route("/chat/completions", post(chat)).with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // Gate: tool handlers block until the test releases them (models a real
    // tool taking hundreds of ms — the live race window).
    let gate = Arc::new(tokio::sync::Notify::new());
    let registry = Arc::new(FunctionRegistry::new());
    for name in ["slow_tool", "fast_tool"] {
        let g = Arc::clone(&gate);
        registry.register(
            ToolDefinition::function(name, "t", json!({"type":"object"})),
            Arc::new(move |_p| {
                let g = Arc::clone(&g);
                Box::pin(async move {
                    g.notified().await;
                    Ok(json!({"ok": true}))
                })
            }),
        );
    }

    let llm = Arc::new(
        LlmClient::new(LlmClientConfig {
            base_url: base,
            api_key: Some("test".into()),
            model: "mock".into(),
            streaming: false,
            ..Default::default()
        })
        .with_functions(Arc::clone(&registry)),
    );

    // ── Turn A: tool-call turn; the batch parks on the gate. ────────────────
    let token_a = CancellationToken::new();
    let first = llm.complete("s1", "user_A question", None, &token_a, None).await.unwrap();
    assert_eq!(first.tool_calls.len(), 2);
    let llm_a = Arc::clone(&llm);
    let reg_a = Arc::clone(&registry);
    let ta = token_a.clone();
    let turn_a = tokio::spawn(async move {
        run_tool_loop(&llm_a, &reg_a, "s1", first, None, &ta, ToolLoopOptions::default()).await
    });

    // Let turn A reach the gated batch.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // ── Turn B: same session, while A's batch is in flight (the live system
    //    reaches this via a forced-final spawned task + a later provider
    //    final; begin_turn only CANCELS A's token — A's appends still run). ──
    let token_b = CancellationToken::new();
    token_a.cancel(); // what begin_turn()/handle_barge_in does to A
    let _ = llm.complete("s1", "user_B question", None, &token_b, None).await.unwrap();

    // Release the tools; turn A appends its tool results now.
    gate.notify_waiters();
    gate.notify_waiters();
    let _ = tokio::time::timeout(Duration::from_secs(5), turn_a).await.unwrap().unwrap();

    // ── Inspect persisted history ordering. ────────────────────────────────
    let history = llm.history_snapshot("s1").await;
    let render: Vec<String> = history
        .iter()
        .map(|m| {
            format!(
                "{:?}(tc={}, id={:?})",
                m.role,
                m.tool_calls.as_ref().map(|t| t.len()).unwrap_or(0),
                m.tool_call_id
            )
        })
        .collect();
    eprintln!("history: {render:#?}");

    let assistant_tc_idx = history
        .iter()
        .position(|m| m.role == MessageRole::Assistant && m.tool_calls.is_some())
        .expect("assistant tool_calls message present");
    let user_b_idx = history
        .iter()
        .position(|m| m.content.as_deref() == Some("user_B question"))
        .expect("user_B present");
    let tool_idxs: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == MessageRole::Tool)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(tool_idxs.len(), 2, "both tool results were appended (cancel never checked)");

    // The pairing invariant requires tool results IMMEDIATELY after the
    // assistant tool_calls message. If user_B landed between them, every
    // subsequent render of this history 400s on strict providers.
    let violated = tool_idxs.iter().any(|&i| user_b_idx > assistant_tc_idx && user_b_idx < i);
    assert!(
        violated,
        "expected user_B ({user_b_idx}) to interleave between assistant tool_calls \
         ({assistant_tc_idx}) and a tool result ({tool_idxs:?}) — if this fails, a guard \
         serialized the turns and the finding is refuted"
    );
}

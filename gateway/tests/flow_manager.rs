//! B-G3 structured conversation flows — FlowManager semantics against a
//! scripted in-process OpenAI-compatible mock.

use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::post};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use waav_gateway::core::flow::{
    ContextStrategy, FlowFunctionSchema, FlowManager, NodeConfig,
};
use waav_gateway::core::llm::{
    ChatMessage, FunctionRegistry, LlmClient, LlmClientConfig, ToolDefinition,
};

// ─────────────────────────────────────────────────────────────────────────────
// Scripted mock: pops one response per request, records every request body.
// ─────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Default)]
struct Script {
    responses: Arc<Mutex<Vec<Value>>>, // popped front-first
    requests: Arc<Mutex<Vec<Value>>>,
}

fn content_resp(text: &str) -> Value {
    json!({"id":"r","object":"chat.completion","created":0,"model":"m",
        "choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}]})
}

fn tool_resp(name: &str, args: &str) -> Value {
    json!({"id":"r","object":"chat.completion","created":0,"model":"m",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"call_1","type":"function","function":{"name":name,"arguments":args}}]},
            "finish_reason":"tool_calls"}]})
}

async fn start_scripted_mock(responses: Vec<Value>) -> (String, Script) {
    let script = Script {
        responses: Arc::new(Mutex::new(responses)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    async fn chat(State(s): State<Script>, Json(req): Json<Value>) -> Json<Value> {
        s.requests.lock().push(req);
        let resp = {
            let mut r = s.responses.lock();
            if r.is_empty() { content_resp("(script exhausted)") } else { r.remove(0) }
        };
        Json(resp)
    }
    let app = Router::new().route("/chat/completions", post(chat)).with_state(script.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://127.0.0.1:{}", addr.port()), script)
}

fn flow(base_url: &str) -> (Arc<LlmClient>, Arc<FunctionRegistry>, FlowManager) {
    let registry = Arc::new(FunctionRegistry::new());
    let llm = Arc::new(
        LlmClient::new(LlmClientConfig {
            base_url: base_url.into(),
            api_key: Some("test".into()),
            model: "m".into(),
            streaming: false,
            ..Default::default()
        })
        .with_functions(Arc::clone(&registry)),
    );
    let manager =
        FlowManager::new(Arc::clone(&llm), Arc::clone(&registry), "flow-session", None);
    (llm, registry, manager)
}

fn def(name: &str) -> ToolDefinition {
    ToolDefinition::function(name, format!("flow fn {name}"), json!({"type":"object"}))
}

fn node_fn(name: &str, result: Value, next: Option<NodeConfig>) -> FlowFunctionSchema {
    FlowFunctionSchema::new(
        def(name),
        Arc::new(move |_p| {
            let result = result.clone();
            let next = next.clone();
            Box::pin(async move { (result, next) })
        }),
    )
}

fn messages_of(req: &Value) -> Vec<(String, String)> {
    req["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["role"].as_str().unwrap_or_default().to_string(),
                m["content"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn append_strategy_extends_context() {
    let (url, script) = start_scripted_mock(vec![content_resp("hi"), content_resp("ok")]).await;
    let (_llm, _reg, mgr2) = flow(&url);

    let cancel = CancellationToken::new();
    let node_a = NodeConfig {
        name: Some("a".into()),
        role_message: Some("You are an intake agent.".into()),
        task_messages: vec![ChatMessage::user("[task] Ask for the caller's name.")],
        respond_immediately: true,
        ..Default::default()
    };
    mgr2.initialize(node_a, &cancel).await.unwrap();
    mgr2.handle_user_turn("my name is Ada", &cancel).await.unwrap();

    let reqs = script.requests.lock();
    assert_eq!(reqs.len(), 2);
    let msgs = messages_of(&reqs[1]);
    // APPEND kept everything: system, task, (greeting), user turn.
    assert_eq!(msgs[0].0, "system");
    assert!(msgs.iter().any(|(_, c)| c.contains("Ask for the caller's name")));
    assert!(msgs.iter().any(|(r, c)| r == "user" && c.contains("my name is Ada")));
}

#[tokio::test]
async fn reset_strategy_replaces_context_messages() {
    let (url, script) = start_scripted_mock(vec![
        content_resp("what's your name?"),
        tool_resp("collect_name", r#"{"name":"Ada"}"#),
        content_resp("noted, Ada. what do you need?"),
    ])
    .await;
    let (_llm, _reg, mgr) = flow(&url);
    let cancel = CancellationToken::new();

    let node_b = NodeConfig {
        name: Some("intent".into()),
        task_messages: vec![ChatMessage::user("[task] Ask what the caller needs.")],
        context_strategy: ContextStrategy::Reset,
        respond_immediately: true,
        ..Default::default()
    };
    let node_a = NodeConfig {
        name: Some("name".into()),
        role_message: Some("You are an intake agent.".into()),
        task_messages: vec![ChatMessage::user("[task] Ask for the caller's name.")],
        functions: vec![node_fn("collect_name", json!({"ok":true}), Some(node_b))],
        respond_immediately: true,
        ..Default::default()
    };
    mgr.initialize(node_a, &cancel).await.unwrap();
    mgr.handle_user_turn("I'm Ada", &cancel).await.unwrap();

    let reqs = script.requests.lock();
    // Last request = entry inference of the RESET node.
    let msgs = messages_of(reqs.last().unwrap());
    assert!(
        msgs.iter().any(|(_, c)| c.contains("Ask what the caller needs")),
        "new node's task present: {msgs:?}"
    );
    assert!(
        !msgs.iter().any(|(_, c)| c.contains("Ask for the caller's name")),
        "RESET must drop the old node's task messages: {msgs:?}"
    );
    assert!(
        !msgs.iter().any(|(_, c)| c.contains("I'm Ada")),
        "RESET must drop prior turns: {msgs:?}"
    );
    assert_eq!(msgs[0], ("system".into(), "You are an intake agent.".into()),
        "persona survives a RESET");
}

#[tokio::test]
async fn role_message_persists_across_nodes() {
    let (url, script) = start_scripted_mock(vec![
        content_resp("hello!"),
        tool_resp("go_next", "{}"),
        content_resp("next question"),
    ])
    .await;
    let (_llm, _reg, mgr) = flow(&url);
    let cancel = CancellationToken::new();

    let node_b = NodeConfig {
        name: Some("b".into()),
        role_message: None, // does NOT set one — A's persists
        task_messages: vec![ChatMessage::user("[task] B.")],
        respond_immediately: true,
        ..Default::default()
    };
    let node_a = NodeConfig {
        name: Some("a".into()),
        role_message: Some("You are Waav, a terse voice agent.".into()),
        task_messages: vec![ChatMessage::user("[task] A.")],
        functions: vec![node_fn("go_next", json!({}), Some(node_b))],
        respond_immediately: true,
        ..Default::default()
    };
    mgr.initialize(node_a, &cancel).await.unwrap();
    mgr.handle_user_turn("go on", &cancel).await.unwrap();

    let reqs = script.requests.lock();
    let msgs = messages_of(reqs.last().unwrap());
    assert_eq!(
        msgs[0],
        ("system".into(), "You are Waav, a terse voice agent.".into()),
        "A's persona must persist into B"
    );
}

#[tokio::test]
async fn edge_function_defers_inference_until_after_transition() {
    let (url, script) = start_scripted_mock(vec![
        tool_resp("go_b", "{}"),     // user turn → edge call
        content_resp("welcome to B"), // ONLY inference after: B's entry
    ])
    .await;
    let (_llm, _reg, mgr) = flow(&url);
    let cancel = CancellationToken::new();

    let node_b = NodeConfig {
        name: Some("b".into()),
        task_messages: vec![ChatMessage::user("[task] You are now in B.")],
        respond_immediately: true,
        ..Default::default()
    };
    let node_a = NodeConfig {
        name: Some("a".into()),
        task_messages: vec![ChatMessage::user("[task] A.")],
        functions: vec![node_fn("go_b", json!({"status":"acknowledged"}), Some(node_b))],
        respond_immediately: false,
        ..Default::default()
    };
    mgr.initialize(node_a, &cancel).await.unwrap();
    let resp = mgr.handle_user_turn("take me to b", &cancel).await.unwrap();

    assert_eq!(resp.unwrap().content, "welcome to B");
    assert_eq!(mgr.current_node().as_deref(), Some("b"));

    let reqs = script.requests.lock();
    // Exactly 2: the user turn + B's entry. NO old-node re-inference between
    // (that's the two-phase edge contract).
    assert_eq!(reqs.len(), 2, "edge must not re-infer on the old node");
    let msgs = messages_of(&reqs[1]);
    assert!(
        msgs.iter().any(|(_, c)| c.contains("You are now in B")),
        "post-transition inference must run on the NEW node's context"
    );
    // The tool result still landed in context before the transition — as an
    // INLINED assistant fact (node boundaries retire the old node's tool
    // exchanges: strict OpenAI-compatible providers reject tool messages
    // without a tools array, and raw calls invite re-calling retired tools).
    let raw = reqs[1]["messages"].as_array().unwrap();
    assert!(
        raw.iter().any(|m| m["role"] == "assistant"
            && m["content"].as_str().unwrap_or_default().contains("go_b")
            && m["content"].as_str().unwrap_or_default().contains("acknowledged")),
        "edge result must land in context (inlined) BEFORE the transition inference: {raw:?}"
    );
    assert!(
        !raw.iter().any(|m| m["role"] == "tool"),
        "no raw tool messages may cross a node boundary: {raw:?}"
    );
}

#[tokio::test]
async fn node_function_runs_llm_immediately() {
    let (url, script) = start_scripted_mock(vec![
        tool_resp("lookup", "{}"),
        content_resp("found it"),
    ])
    .await;
    let (_llm, _reg, mgr) = flow(&url);
    let cancel = CancellationToken::new();

    let node_a = NodeConfig {
        name: Some("a".into()),
        task_messages: vec![ChatMessage::user("[task] A.")],
        functions: vec![node_fn("lookup", json!({"answer":42}), None)], // NODE fn
        respond_immediately: false,
        ..Default::default()
    };
    mgr.initialize(node_a, &cancel).await.unwrap();
    let resp = mgr.handle_user_turn("look it up", &cancel).await.unwrap();

    assert_eq!(resp.unwrap().content, "found it");
    assert_eq!(mgr.current_node().as_deref(), Some("a"), "node fn stays put");
    let reqs = script.requests.lock();
    assert_eq!(reqs.len(), 2, "node fn re-infers in place");
    let raw = reqs[1]["messages"].as_array().unwrap();
    assert!(raw.iter().any(|m| m["role"] == "tool"));
}

#[tokio::test]
async fn respond_immediately_false_does_not_infer() {
    let (url, script) = start_scripted_mock(vec![
        tool_resp("go_quiet", "{}"),
        // nothing else should be requested
    ])
    .await;
    let (_llm, _reg, mgr) = flow(&url);
    let cancel = CancellationToken::new();

    let quiet = NodeConfig {
        name: Some("quiet".into()),
        task_messages: vec![ChatMessage::user("[task] Wait for the user.")],
        respond_immediately: false,
        ..Default::default()
    };
    let node_a = NodeConfig {
        name: Some("a".into()),
        functions: vec![node_fn("go_quiet", json!({}), Some(quiet))],
        respond_immediately: false,
        ..Default::default()
    };
    mgr.initialize(node_a, &cancel).await.unwrap();
    let resp = mgr.handle_user_turn("switch", &cancel).await.unwrap();

    assert!(resp.is_none(), "quiet node: nothing to speak");
    assert_eq!(mgr.current_node().as_deref(), Some("quiet"));
    assert_eq!(script.requests.lock().len(), 1, "no entry inference on a quiet node");
}

#[tokio::test]
async fn global_functions_available_at_every_node_and_tools_swap_atomically() {
    let (url, _script) = start_scripted_mock(vec![
        tool_resp("go_b", "{}"),
        content_resp("in b"),
    ])
    .await;
    let registry = Arc::new(FunctionRegistry::new());
    let llm = Arc::new(
        LlmClient::new(LlmClientConfig {
            base_url: url,
            api_key: Some("test".into()),
            model: "m".into(),
            streaming: false,
            ..Default::default()
        })
        .with_functions(Arc::clone(&registry)),
    );
    let node_b = NodeConfig {
        name: Some("b".into()),
        functions: vec![node_fn("b_only", json!({}), None)],
        respond_immediately: true,
        ..Default::default()
    };
    let node_a = NodeConfig {
        name: Some("a".into()),
        functions: vec![node_fn("go_b", json!({}), Some(node_b))],
        respond_immediately: false,
        ..Default::default()
    };
    let mgr = FlowManager::new(Arc::clone(&llm), Arc::clone(&registry), "s", None)
        .with_global_functions(vec![node_fn("help", json!({"help":"yes"}), None)]);
    let cancel = CancellationToken::new();
    mgr.initialize(node_a, &cancel).await.unwrap();

    let names = |reg: &FunctionRegistry| -> Vec<String> {
        reg.request_tools().iter().map(|t| t.function.name.clone()).collect()
    };
    let at_a = names(&registry);
    assert!(at_a.contains(&"go_b".to_string()) && at_a.contains(&"help".to_string()));
    assert!(!at_a.contains(&"b_only".to_string()));

    mgr.handle_user_turn("switch", &cancel).await.unwrap();

    let at_b = names(&registry);
    assert!(
        at_b.contains(&"b_only".to_string()) && at_b.contains(&"help".to_string()),
        "global available at B too: {at_b:?}"
    );
    assert!(
        !at_b.contains(&"go_b".to_string()),
        "A's tools must be GONE after the swap: {at_b:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// LIVE gate: a real 3-node intake flow (name → intent → confirm) over an
// OpenAI-compatible LLM. Key-gated.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[ignore = "requires OPENAI_API_KEY (+ optional OPENAI_BASE_URL/OPENAI_MODEL); real billed calls"]
async fn live_three_node_intake_flow() {
    let Ok(key) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("[skip] OPENAI_API_KEY not set");
        return;
    };
    let base = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());

    let registry = Arc::new(FunctionRegistry::new());
    let llm = Arc::new(
        LlmClient::new(LlmClientConfig {
            base_url: base,
            api_key: Some(key),
            model,
            streaming: false,
            max_tokens: Some(2000),
            ..Default::default()
        })
        .with_functions(Arc::clone(&registry)),
    );

    let confirm = NodeConfig {
        name: Some("confirm".into()),
        task_messages: vec![ChatMessage::user(
            "[task] Confirm back to the caller: their NAME and their REQUEST, in one short sentence, then thank them.",
        )],
        context_strategy: ContextStrategy::Append,
        respond_immediately: true,
        ..Default::default()
    };
    let intent = NodeConfig {
        name: Some("intent".into()),
        task_messages: vec![ChatMessage::user(
            "[task] Ask the caller what they need help with. When they tell you, call collect_intent with it.",
        )],
        functions: vec![FlowFunctionSchema::new(
            ToolDefinition::function(
                "collect_intent",
                "Record what the caller needs.",
                json!({"type":"object","properties":{"intent":{"type":"string"}},"required":["intent"]}),
            ),
            {
                let confirm = confirm.clone();
                Arc::new(move |p| {
                    let confirm = confirm.clone();
                    Box::pin(async move {
                        (json!({"recorded": p.arguments["intent"]}), Some(confirm.clone()))
                    })
                })
            },
        )],
        respond_immediately: true,
        ..Default::default()
    };
    let name_node = NodeConfig {
        name: Some("name".into()),
        role_message: Some(
            "You are a terse phone intake agent. One short sentence per reply. Use the provided tools exactly as instructed.".into(),
        ),
        task_messages: vec![ChatMessage::user(
            "[task] Greet the caller and ask for their name. When they give it, call collect_name with it.",
        )],
        functions: vec![FlowFunctionSchema::new(
            ToolDefinition::function(
                "collect_name",
                "Record the caller's name.",
                json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
            ),
            {
                let intent = intent.clone();
                Arc::new(move |p| {
                    let intent = intent.clone();
                    Box::pin(async move {
                        (json!({"recorded": p.arguments["name"]}), Some(intent.clone()))
                    })
                })
            },
        )],
        respond_immediately: true,
        ..Default::default()
    };

    let mgr = FlowManager::new(Arc::clone(&llm), Arc::clone(&registry), "live-flow", None);
    let cancel = CancellationToken::new();

    let greeting = mgr.initialize(name_node, &cancel).await.unwrap().unwrap();
    println!("[flow:name] {}", greeting.content);
    assert!(!greeting.content.trim().is_empty(), "greeting must ask for a name");

    let ask_intent = mgr.handle_user_turn("Hi, my name is Ada Lovelace.", &cancel)
        .await
        .unwrap()
        .expect("intent node responds immediately");
    println!("[flow:intent] {}", ask_intent.content);
    assert_eq!(mgr.current_node().as_deref(), Some("intent"), "edge must fire once");

    let confirm_resp = mgr
        .handle_user_turn("I need to reschedule my appointment to Friday.", &cancel)
        .await
        .unwrap()
        .expect("confirm node responds immediately");
    println!("[flow:confirm] {}", confirm_resp.content);
    assert_eq!(mgr.current_node().as_deref(), Some("confirm"));
    let lower = confirm_resp.content.to_lowercase();
    assert!(lower.contains("ada"), "confirmation must reference the caller's name: {lower}");
    assert!(
        lower.contains("reschedul") || lower.contains("appointment") || lower.contains("friday"),
        "confirmation must reference the intent: {lower}"
    );
}

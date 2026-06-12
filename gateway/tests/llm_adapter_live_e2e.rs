//! LIVE round-trips for the B-G1 vendor adapters (Anthropic Messages, Gemini
//! generateContent) — real billed API calls, key-gated.
//!
//! Run (keys via env ONLY, never written to files):
//! ```sh
//! ANTHROPIC_API_KEY=… GOOGLE_AI_API_KEY=… \
//!   cargo test --test llm_adapter_live_e2e -- --ignored --nocapture
//! ```
//! Each test exercises: history seeding → a streaming completion through the
//! native wire format (system placement, vendor field names, SSE parse) → a
//! second turn proving history threading — the full B-G1 live gate from
//! PIPECAT_FIX_PLAN §2.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;
use waav_gateway::core::llm::{AdapterKind, LlmClient, LlmClientConfig};

fn key(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            eprintln!("[skip] {var} not set — live adapter test skipped");
            None
        }
    }
}

async fn round_trip(kind: AdapterKind, base_url: &str, model: &str, key: String) {
    let config = LlmClientConfig {
        base_url: base_url.to_string(),
        model: model.to_string(),
        api_key: Some(key),
        system_prompt: Some("Answer in exactly one short sentence.".into()),
        max_tokens: Some(64),
        streaming: true,
        provider_kind: Some(kind),
        ..Default::default()
    };
    let client = LlmClient::new(config);
    let cancel = CancellationToken::new();

    // Turn 1 — streaming: tokens must arrive incrementally.
    let tokens: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = tokens.clone();
    let on_token: waav_gateway::core::llm::TokenCallback =
        Arc::new(move |t: &str| sink.lock().unwrap().push(t.to_string()));
    let r1 = client
        .complete("live-e2e", "My name is Waav. Reply OK.", None, &cancel, Some(on_token))
        .await
        .expect("turn 1 must succeed");
    assert!(!r1.content.trim().is_empty(), "turn 1 produced no content: {r1:?}");
    assert!(
        !tokens.lock().unwrap().is_empty(),
        "streaming must deliver token deltas, got none (content: {})",
        r1.content
    );

    // Turn 2 — history threading: the model must remember the name from turn 1
    // (proves the rendered conversation carried prior turns correctly).
    let r2 = client
        .complete("live-e2e", "What is my name? One word only.", None, &cancel, None)
        .await
        .expect("turn 2 must succeed");
    assert!(
        r2.content.to_lowercase().contains("waav"),
        "history not threaded through the {kind:?} adapter — reply: {}",
        r2.content
    );
    eprintln!("[live {kind:?}] turn1={:?} turn2={:?} usage={:?}", r1.content, r2.content, r2.usage);
}

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY; real billed Anthropic Messages calls"]
async fn anthropic_live_round_trip() {
    let Some(k) = key("ANTHROPIC_API_KEY") else { return };
    round_trip(
        AdapterKind::Anthropic,
        "https://api.anthropic.com",
        "claude-haiku-4-5-20251001",
        k,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires GOOGLE_AI_API_KEY; real billed Gemini generateContent calls"]
async fn gemini_live_round_trip() {
    let Some(k) = key("GOOGLE_AI_API_KEY") else { return };
    round_trip(
        AdapterKind::Gemini,
        "https://generativelanguage.googleapis.com",
        "gemini-2.0-flash",
        k,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires OPENAI_API_KEY; real billed OpenAI calls (adapter parity check)"]
async fn openai_live_round_trip() {
    let Some(k) = key("OPENAI_API_KEY") else { return };
    round_trip(AdapterKind::OpenAi, "https://api.openai.com/v1", "gpt-4o-mini", k).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// B-G4 LIVE gate: a real parallel 2-tool call (`get_weather` + `get_time`)
// through the server-side tool loop — exactly one follow-up inference whose
// answer references both results.
// ─────────────────────────────────────────────────────────────────────────────

async fn tool_loop_round_trip(kind: AdapterKind, base_url: &str, model: &str, api_key: String) {
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use waav_gateway::core::llm::{
        FunctionRegistry, ToolDefinition, ToolLoopOptions, run_tool_loop,
    };

    let weather_calls = Arc::new(AtomicUsize::new(0));
    let time_calls = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(FunctionRegistry::new());
    let wc = Arc::clone(&weather_calls);
    registry.register(
        ToolDefinition::function(
            "get_weather",
            "Get the current weather for a city.",
            json!({"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}),
        ),
        Arc::new(move |p| {
            wc.fetch_add(1, Ordering::SeqCst);
            let city = p.arguments["city"].as_str().unwrap_or("unknown").to_string();
            Box::pin(async move { Ok(json!({"city": city, "weather": "heavy snow", "temp_c": -3})) })
        }),
    );
    let tc = Arc::clone(&time_calls);
    registry.register(
        ToolDefinition::function(
            "get_time",
            "Get the current local time for a city.",
            json!({"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}),
        ),
        Arc::new(move |_p| {
            tc.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(json!({"time": "07:45", "tz": "CET"})) })
        }),
    );

    let config = LlmClientConfig {
        base_url: base_url.into(),
        api_key: Some(api_key),
        model: model.into(),
        provider_kind: Some(kind),
        streaming: false,
        max_tokens: Some(300),
        system_prompt: Some(
            "You are a terse assistant. Use the provided tools when asked about \
             weather or time; call both in one response when both are needed."
                .into(),
        ),
        ..Default::default()
    };
    let llm = LlmClient::new(config).with_functions(Arc::clone(&registry));
    let token = CancellationToken::new();

    let first = llm
        .complete(
            "live-tools",
            "What's the weather and the local time in Paris right now? Use your tools.",
            None,
            &token,
            None,
        )
        .await
        .expect("initial completion");
    assert!(
        !first.tool_calls.is_empty(),
        "[{kind:?}] model did not call tools; finish={:?} content={:?}",
        first.finish_reason,
        first.content
    );

    let final_resp = run_tool_loop(
        &llm,
        &registry,
        "live-tools",
        first,
        None,
        &token,
        ToolLoopOptions::default(),
    )
    .await
    .expect("tool loop");

    println!("[{kind:?}] tool-loop answer: {}", final_resp.content);
    assert!(weather_calls.load(Ordering::SeqCst) >= 1, "get_weather executed");
    assert!(time_calls.load(Ordering::SeqCst) >= 1, "get_time executed");
    let answer = final_resp.content.to_lowercase();
    assert!(
        answer.contains("snow") || answer.contains("-3"),
        "[{kind:?}] answer must reference the weather tool result: {answer}"
    );
    assert!(
        answer.contains("7:45") || answer.contains("07:45") || answer.contains("45"),
        "[{kind:?}] answer must reference the time tool result: {answer}"
    );
}

#[tokio::test]
#[ignore = "requires OPENAI_API_KEY; real billed OpenAI calls"]
async fn openai_live_tool_loop() {
    let Some(k) = key("OPENAI_API_KEY") else { return };
    let base = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model =
        std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    tool_loop_round_trip(AdapterKind::OpenAi, &base, &model, k).await;
}

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY; real billed Anthropic Messages calls"]
async fn anthropic_live_tool_loop() {
    let Some(k) = key("ANTHROPIC_API_KEY") else { return };
    tool_loop_round_trip(
        AdapterKind::Anthropic,
        "https://api.anthropic.com",
        "claude-haiku-4-5-20251001",
        k,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires GOOGLE_AI_API_KEY; real billed Gemini calls"]
async fn gemini_live_tool_loop() {
    let Some(k) = key("GOOGLE_AI_API_KEY") else { return };
    tool_loop_round_trip(
        AdapterKind::Gemini,
        "https://generativelanguage.googleapis.com",
        "gemini-2.0-flash",
        k,
    )
    .await;
}

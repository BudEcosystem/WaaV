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
use waav_gateway::core::llm::{AdapterKind, LlmClient, LlmClientConfig, ReasoningEffort};

/// Credential-free local live gate: returns the ollama OpenAI-compatible base
/// URL only when ollama is reachable, else `None` (test self-skips, so CI without
/// ollama still passes). Exercises the OpenAI adapter against a real endpoint.
fn ollama_base() -> Option<String> {
    let addr: std::net::SocketAddr = "127.0.0.1:11434".parse().unwrap();
    match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)) {
        Ok(_) => Some("http://127.0.0.1:11434/v1".to_string()),
        Err(_) => {
            eprintln!("[skip] ollama not reachable at 127.0.0.1:11434 — d1 live test skipped");
            None
        }
    }
}

/// Key-gated (env ONLY, never written to a file): the OpenAI API key, or `None`
/// (test self-skips).
fn openai_key() -> Option<String> {
    match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.trim().is_empty() => Some(k),
        _ => {
            eprintln!("[skip] OPENAI_API_KEY not set — OpenAI live test skipped");
            None
        }
    }
}

/// LIVE (OpenAI, key-gated): a REASONING model (gpt-5-mini) must round-trip
/// through WaaV's OpenAI adapter — this is the regression for the
/// `max_tokens`→`max_completion_tokens` shape (reasoning models 400 on
/// `max_tokens`; live-caught). Validates the fast tier + the two-tier shape too.
#[tokio::test]
async fn openai_reasoning_model_live_round_trip() {
    let Some(key) = openai_key() else { return };
    let cancel = CancellationToken::new();

    // Fast (chat) model — classic max_tokens shape.
    let fast = LlmClient::new(LlmClientConfig {
        base_url: "https://api.openai.com/v1".into(),
        model: "gpt-4o-mini".into(),
        api_key: Some(key.clone()),
        max_tokens: Some(20),
        streaming: false,
        ..Default::default()
    });
    let r = fast
        .complete("oai-fast", "Reply with exactly: OK", None, &cancel, None)
        .await
        .expect("gpt-4o-mini must round-trip");
    assert!(!r.content.trim().is_empty(), "fast model empty");

    // REASONING model — REQUIRES max_completion_tokens (max_tokens would 400).
    // reasoning_effort=low + a real max_tokens (the shape that broke live).
    for streaming in [false, true] {
        let reasoning = LlmClient::new(LlmClientConfig {
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-5-mini".into(),
            api_key: Some(key.clone()),
            reasoning_effort: Some(ReasoningEffort::Low),
            max_tokens: Some(2000),
            temperature: Some(0.7), // must be SUPPRESSED for reasoning models
            streaming,
            ..Default::default()
        });
        let r = reasoning
            .complete(
                &format!("oai-reason-{streaming}"),
                "What is 2+2? Reply with just the number.",
                None,
                &cancel,
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("gpt-5-mini (streaming={streaming}) must NOT 400: {e}"));
        assert!(
            r.content.contains('4'),
            "reasoning model answer (streaming={streaming}): {:?}",
            r.content
        );
        eprintln!(
            "[live openai] gpt-5-mini streaming={streaming} → {:?}",
            r.content.trim()
        );
    }
}

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
        .complete(
            "live-e2e",
            "My name is Waav. Reply OK.",
            None,
            &cancel,
            Some(on_token),
        )
        .await
        .expect("turn 1 must succeed");
    assert!(
        !r1.content.trim().is_empty(),
        "turn 1 produced no content: {r1:?}"
    );
    assert!(
        !tokens.lock().unwrap().is_empty(),
        "streaming must deliver token deltas, got none (content: {})",
        r1.content
    );

    // Turn 2 — history threading: the model must remember the name from turn 1
    // (proves the rendered conversation carried prior turns correctly).
    let r2 = client
        .complete(
            "live-e2e",
            "What is my name? One word only.",
            None,
            &cancel,
            None,
        )
        .await
        .expect("turn 2 must succeed");
    assert!(
        r2.content.to_lowercase().contains("waav"),
        "history not threaded through the {kind:?} adapter — reply: {}",
        r2.content
    );
    eprintln!(
        "[live {kind:?}] turn1={:?} turn2={:?} usage={:?}",
        r1.content, r2.content, r2.usage
    );
}

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY; real billed Anthropic Messages calls"]
async fn anthropic_live_round_trip() {
    let Some(k) = key("ANTHROPIC_API_KEY") else {
        return;
    };
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
    let Some(k) = key("GOOGLE_AI_API_KEY") else {
        return;
    };
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
    let Some(k) = key("OPENAI_API_KEY") else {
        return;
    };
    round_trip(
        AdapterKind::OpenAi,
        "https://api.openai.com/v1",
        "gpt-4o-mini",
        k,
    )
    .await;
}

/// D1 LIVE (credential-free, ollama): the critical end-to-end proof that the
/// reasoning_effort dial is correct on a REAL endpoint —
///  (1) a FAST/non-reasoning model with `Off` emits NO thinking param and does
///      NOT 400 ("model does not support thinking"), and
///  (2) a REASONING model with `Low` is accepted and answers.
/// NOT `#[ignore]` — self-skips when ollama is down so CI is unaffected.
#[tokio::test]
async fn d1_reasoning_effort_live_ollama() {
    let Some(base) = ollama_base() else { return };

    // (1) fast model + Off → no param → must succeed (the bug this guards against
    //     400s every call against a non-reasoning model).
    let fast = LlmClientConfig {
        base_url: base.clone(),
        model: "llama3.2:1b".into(),
        api_key: Some("ollama".into()), // ollama ignores the key
        system_prompt: Some("Reply in one short sentence.".into()),
        max_tokens: Some(40),
        streaming: false,
        reasoning_effort: Some(ReasoningEffort::Off),
        ..Default::default()
    };
    let r = LlmClient::new(fast)
        .complete(
            "d1-fast",
            "say hello",
            None,
            &CancellationToken::new(),
            None,
        )
        .await
        .expect("fast model + reasoning_effort=Off must NOT 400");
    assert!(
        !r.content.trim().is_empty(),
        "fast model produced no content"
    );

    // (2) reasoning model + Low → thinking param accepted → non-empty answer.
    let reason = LlmClientConfig {
        base_url: base,
        model: "deepseek-r1:1.5b".into(),
        api_key: Some("ollama".into()),
        max_tokens: Some(1200),
        streaming: false,
        reasoning_effort: Some(ReasoningEffort::Low),
        ..Default::default()
    };
    let r2 = LlmClient::new(reason)
        .complete(
            "d1-reason",
            "What is 2+2? Reply with just the number.",
            None,
            &CancellationToken::new(),
            None,
        )
        .await
        .expect("reasoning model + reasoning_effort=Low must be accepted");
    assert!(
        !r2.content.trim().is_empty(),
        "reasoning model produced empty content"
    );
    eprintln!("[live d1] fast={:?} reason={:?}", r.content, r2.content);
}

/// S1/S2 LIVE (credential-free, ollama): the two tiers (fast llama + reasoning
/// deepseek-r1) built via with_tier_overrides SHARE conversation history — an
/// escalated turn continues the same conversation rather than starting fresh.
#[tokio::test]
async fn s2_two_tier_shares_history_live_ollama() {
    let Some(base) = ollama_base() else { return };
    let fast = LlmClient::new(LlmClientConfig {
        base_url: base,
        model: "llama3.2:1b".into(),
        api_key: Some("ollama".into()),
        system_prompt: Some("Answer in one short sentence.".into()),
        // A reasoning model spends tokens on its <think> block before answering;
        // a tiny fast-tuned budget is wholly consumed by reasoning and yields empty
        // content. The reasoning tier needs token headroom (real-world tuning note).
        max_tokens: Some(512),
        streaming: false,
        ..Default::default()
    });
    // Reasoning tier shares the fast tier's per-session history.
    let reasoning = fast.with_tier_overrides(
        "deepseek-r1:1.5b".into(),
        None,
        None,
        None,
        Some(ReasoningEffort::Low),
        None,
    );
    let cancel = CancellationToken::new();

    // Turn 1 on the FAST tier seeds a fact into the conversation history.
    fast.complete(
        "2tier",
        "My name is Waav. Reply with just OK.",
        None,
        &cancel,
        None,
    )
    .await
    .expect("fast tier turn 1");
    // Turn 2 ESCALATED to the reasoning tier — runs live against the real model
    // (proves the escalated round-trip works end to end).
    let r = reasoning
        .complete(
            "2tier",
            "What is my name? Reply with one word.",
            None,
            &cancel,
            None,
        )
        .await
        .expect("reasoning tier turn 2");
    eprintln!("[live s2] reasoning tier replied: {:?}", r.content);

    // DETERMINISTIC proof of the shared-history Arc (independent of model quality):
    // both tiers observe the SAME conversation, and the reasoning tier sees the
    // fast tier's turn-1 message — i.e. an escalated turn continues, not restarts.
    let fast_hist = fast.history_snapshot("2tier").await;
    let reason_hist = reasoning.history_snapshot("2tier").await;
    assert_eq!(
        fast_hist.len(),
        reason_hist.len(),
        "both tiers must observe the one shared history (same Arc)"
    );
    assert!(
        fast_hist.len() >= 4,
        "two completed turns ⇒ ≥4 messages, got {}: {fast_hist:?}",
        fast_hist.len()
    );
    assert!(
        reason_hist.iter().any(|m| m
            .content
            .as_deref()
            .is_some_and(|c| c.contains("My name is Waav"))),
        "the reasoning tier must see the fast tier's turn-1 message in shared history: {reason_hist:?}"
    );
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
            let city = p.arguments["city"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            Box::pin(
                async move { Ok(json!({"city": city, "weather": "heavy snow", "temp_c": -3})) },
            )
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
    assert!(
        weather_calls.load(Ordering::SeqCst) >= 1,
        "get_weather executed"
    );
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
    let Some(k) = key("OPENAI_API_KEY") else {
        return;
    };
    let base = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    tool_loop_round_trip(AdapterKind::OpenAi, &base, &model, k).await;
}

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY; real billed Anthropic Messages calls"]
async fn anthropic_live_tool_loop() {
    let Some(k) = key("ANTHROPIC_API_KEY") else {
        return;
    };
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
    let Some(k) = key("GOOGLE_AI_API_KEY") else {
        return;
    };
    tool_loop_round_trip(
        AdapterKind::Gemini,
        "https://generativelanguage.googleapis.com",
        "gemini-2.0-flash",
        k,
    )
    .await;
}

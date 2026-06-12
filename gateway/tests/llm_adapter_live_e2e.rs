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

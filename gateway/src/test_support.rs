//! Shared helpers for the crate's own unit tests.

use std::sync::Arc;

use crate::config::ServerConfig;
use crate::state::AppState;

/// A credential-free config for a real `AppState` (the same literal `state` tests use).
pub(crate) fn minimal_config() -> ServerConfig {
    ServerConfig {
        host: "localhost".to_string(),
        port: 3001,
        tls: None,
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
        livekit_api_key: None,
        livekit_api_secret: None,
        deepgram_api_key: None,
        elevenlabs_api_key: None,
        google_credentials: None,
        azure_speech_subscription_key: None,
        azure_speech_region: None,
        cartesia_api_key: None,
        openai_api_key: None,
        azure_openai_api_key: None,
        azure_openai_endpoint: None,
        grok_api_key: None,
        inworld_api_key: None,
        gemini_api_key: None,
        ultravox_api_key: None,
        speechmatics_api_key: None,
        yandex_api_key: None,
        yandex_folder_id: None,
        assemblyai_api_key: None,
        hume_api_key: None,
        groq_api_key: None,
        ibm_watson_api_key: None,
        ibm_watson_instance_id: None,
        ibm_watson_region: None,
        aws_access_key_id: None,
        aws_secret_access_key: None,
        aws_region: None,
        gnani_token: None,
        gnani_access_key: None,
        gnani_certificate_path: None,
        recording_s3_bucket: None,
        recording_s3_region: None,
        recording_s3_endpoint: None,
        recording_s3_access_key: None,
        recording_s3_secret_key: None,
        recording_s3_prefix: None,
        cache_path: None,
        cache_ttl_seconds: Some(3600),
        auth_service_url: None,
        auth_signing_key_path: None,
        auth_api_secrets: Vec::new(),
        auth_timeout_seconds: 5,
        auth_required: false,
        sip: None,
        cors_allowed_origins: None,
        rate_limit_requests_per_second: 60,
        rate_limit_burst_size: 10,
        max_websocket_connections: None,
        max_connections_per_ip: 100,
        ws_processing_timeout_secs: 10,
        realtime_processing_timeout_secs: 30,
        sip_max_participants: 3,
        realtime_endpoint_overrides: Default::default(),
        aliases: Default::default(),
        plugins: crate::config::PluginConfig::default(),
        dag_timeouts: crate::config::DAGTimeoutsConfig::default(),
    }
}

/// An `AppState` in Bud mode over an in-memory control plane holding `keys` (FRD-023 tests).
///
/// Uses `BudMode::for_plane`, which never connects to Redis and does NOT mark the process as in
/// Bud mode, so it cannot leak into tests running beside it.
pub(crate) async fn bud_state(keys: &[(&str, &str)]) -> Arc<AppState> {
    let store = Arc::new(bud_auth::MemoryStore::new());
    for (k, v) in keys {
        store.set(k, v);
    }
    let plane = Arc::new(bud_auth::BudPlane::new(
        store as Arc<dyn bud_auth::ControlPlaneStore>,
        None,
    ));
    plane.boot().await.expect("plane boots");
    let mut state = AppState::new(minimal_config()).await;
    Arc::get_mut(&mut state)
        .expect("the state is not shared yet")
        .bud_mode = Some(crate::auth::bud_mode::BudMode::for_plane(plane).expect("bud mode"));
    state
}

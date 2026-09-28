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

/// [`bud_state`] whose plane opens `voice_table` credentials with the test key pair
/// ([`test_credential`] decrypts to [`TEST_CREDENTIAL_PLAIN`]) and enforces deployment policies
/// (FRD-023 RT6 tests). Returns the store, to mutate the control plane mid-test.
pub(crate) async fn bud_state_with_credentials(
    keys: &[(&str, &str)],
) -> (Arc<AppState>, Arc<bud_auth::MemoryStore>) {
    let store = Arc::new(bud_auth::MemoryStore::new());
    for (k, v) in keys {
        store.set(k, v);
    }
    let plane = Arc::new(bud_auth::BudPlane::with_decryptor(
        store.clone() as Arc<dyn bud_auth::ControlPlaneStore>,
        None,
        bud_auth::CredentialDecryptor::from_pem(&credential_fixture().0)
            .expect("fixture key parses"),
    ));
    plane.boot().await.expect("plane boots");
    let mut state = AppState::new(minimal_config()).await;
    {
        let s = Arc::get_mut(&mut state).expect("the state is not shared yet");
        s.bud_mode = Some(crate::auth::bud_mode::BudMode::for_plane(plane).expect("bud mode"));
        s.policies = Some(crate::core::deployment_policy::DeploymentPolicies::local());
    }
    (state, store)
}

/// The plaintext of [`test_credential`].
pub(crate) const TEST_CREDENTIAL_PLAIN: &str = "dg_vendor_key_abc123";

/// A test key pair (PKCS#8 PEM) and [`TEST_CREDENTIAL_PLAIN`] encrypted to it the way budapp encrypts
/// credentials (RSA-OAEP-SHA-256, hex), made once per test binary. bud-auth's `*.pem` fixtures are
/// git-ignored, so a clean checkout -- CI -- has no key to read and no way to open a committed
/// ciphertext.
fn credential_fixture() -> &'static (String, String) {
    static FIXTURE: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    FIXTURE.get_or_init(|| {
        use rsa::pkcs8::{EncodePrivateKey, LineEnding};
        let mut rng = rsa::rand_core::OsRng;
        let key = rsa::RsaPrivateKey::new(&mut rng, 2048).expect("test key generates");
        let pem = key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("test key encodes")
            .to_string();
        let ciphertext = key
            .to_public_key()
            .encrypt(
                &mut rng,
                rsa::Oaep::new::<sha2::Sha256>(),
                TEST_CREDENTIAL_PLAIN.as_bytes(),
            )
            .expect("test credential encrypts");
        (pem, hex::encode(ciphertext))
    })
}

/// The test credential's ciphertext, for `voice_table` entries in tests.
pub(crate) fn test_credential() -> &'static str {
    &credential_fixture().1
}

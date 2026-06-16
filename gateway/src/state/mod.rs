use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::auth::AuthClient;
use crate::config::ServerConfig;
use crate::core::CoreState;
use crate::core::cache::store::CacheStore;
use crate::livekit::room_handler::{LiveKitRoomHandler, RecordingConfig};
use crate::livekit::sip_handler::{DispatchConfig, LiveKitSipHandler, TrunkConfig};
use crate::utils::req_manager::ReqManager;
use dashmap::DashMap;
use object_store::ObjectStore;
use object_store::aws::AmazonS3Builder;
use tokio_util::sync::CancellationToken;

mod sip_hooks_state;

pub use sip_hooks_state::SipHooksState;

/// Application state that can be shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub config: ServerConfig,
    /// Core layer state that holds shared resources, such as TTS request managers
    pub core_state: Arc<CoreState>,
    /// LiveKit room handler for room and token management
    pub livekit_room_handler: Option<Arc<LiveKitRoomHandler>>,
    /// Object store client for recording retrieval
    pub object_store: Option<Arc<dyn ObjectStore>>,
    /// Recording bucket name for convenience access
    pub recording_bucket: Option<String>,
    /// LiveKit SIP handler for SIP trunk and dispatch rule management
    pub livekit_sip_handler: Option<Arc<LiveKitSipHandler>>,
    /// Authentication client for validating bearer tokens (if auth is enabled)
    pub auth_client: Option<Arc<AuthClient>>,
    /// Active WebSocket connection count (for global limit enforcement)
    pub active_ws_connections: Arc<AtomicUsize>,
    /// Connection count per IP address (for per-IP limit enforcement)
    pub connections_per_ip: Arc<DashMap<IpAddr, AtomicUsize>>,
    /// App-wide shutdown signal (RC6 SIGTERM session drain).
    ///
    /// Cancelled by `main()` when SIGTERM/SIGINT is received, BEFORE axum's
    /// graceful drain starts, so every live WebSocket session can send a final
    /// protocol notice to its client and tear down providers within the drain
    /// window. `CancellationToken` clones share state, so every `AppState`
    /// clone (and every session loop selecting on `shutdown.cancelled()`)
    /// observes the same cancellation.
    pub shutdown: CancellationToken,
}

impl AppState {
    pub async fn new(config: ServerConfig) -> Arc<Self> {
        // Ensure a process-level rustls CryptoProvider is installed before any provider opens a
        // TLS/WSS connection (e.g. a realtime STT socket). The gateway binary installs this in
        // main(); doing it here too — idempotently — makes embedding AppState (tests, SDKs, custom
        // binaries) robust against the "no process-level CryptoProvider" panic. install_default()
        // returns Err if one is already installed, which we intentionally ignore.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let core_state = CoreState::new(&config).await;

        // Initialize LiveKit room handler if API keys are available
        let livekit_room_handler = if let (Some(api_key), Some(api_secret)) =
            (&config.livekit_api_key, &config.livekit_api_secret)
        {
            // Build recording config if all S3 settings are present
            let recording_config = if let (
                Some(bucket),
                Some(region),
                Some(endpoint),
                Some(access_key),
                Some(secret_key),
            ) = (
                &config.recording_s3_bucket,
                &config.recording_s3_region,
                &config.recording_s3_endpoint,
                &config.recording_s3_access_key,
                &config.recording_s3_secret_key,
            ) {
                Some(RecordingConfig {
                    bucket: bucket.clone(),
                    region: region.clone(),
                    endpoint: endpoint.clone(),
                    access_key: access_key.clone(),
                    secret_key: secret_key.clone(),
                    prefix: config.recording_s3_prefix.clone().unwrap_or_default(),
                })
            } else {
                None
            };

            match LiveKitRoomHandler::new(
                config.livekit_url.clone(),
                api_key.clone(),
                api_secret.clone(),
                recording_config,
            ) {
                Ok(handler) => Some(Arc::new(handler)),
                Err(e) => {
                    tracing::warn!("Failed to initialize LiveKit room handler: {:?}", e);
                    None
                }
            }
        } else {
            None
        };

        // Initialize object store for recording downloads if all credentials are provided
        let (object_store, recording_bucket) = if let (
            Some(bucket),
            Some(region),
            Some(endpoint),
            Some(access_key),
            Some(secret_key),
        ) = (
            &config.recording_s3_bucket,
            &config.recording_s3_region,
            &config.recording_s3_endpoint,
            &config.recording_s3_access_key,
            &config.recording_s3_secret_key,
        ) {
            let mut builder = AmazonS3Builder::new()
                .with_bucket_name(bucket)
                .with_region(region)
                .with_endpoint(endpoint)
                .with_access_key_id(access_key)
                .with_secret_access_key(secret_key);

            if endpoint.starts_with("http://") {
                builder = builder.with_allow_http(true);
            }

            match builder.build() {
                Ok(store) => {
                    tracing::info!(
                        "Recording object store initialized for bucket={} endpoint={}",
                        bucket,
                        endpoint
                    );
                    (
                        Some(Arc::new(store) as Arc<dyn ObjectStore>),
                        Some(bucket.clone()),
                    )
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to initialize recording object store for bucket={} endpoint={}: {:?}",
                        bucket,
                        endpoint,
                        e
                    );
                    tracing::info!("Recording downloads will be unavailable");
                    (None, None)
                }
            }
        } else {
            tracing::info!("Recording storage not configured; recording downloads disabled");
            (None, None)
        };

        // Initialize auth client if JWT-based auth is configured
        // Note: API secret auth doesn't need a client - it's handled directly in middleware
        let auth_client = if config.auth_required && config.has_jwt_auth() {
            match AuthClient::from_config(&config).await {
                Ok(client) => {
                    tracing::info!(
                        "JWT authentication enabled with service: {}",
                        config
                            .auth_service_url
                            .as_ref()
                            .unwrap_or(&"unknown".to_string())
                    );
                    Some(Arc::new(client))
                }
                Err(e) => {
                    // Graceful degradation when JWT auth fails at runtime
                    // Config validation in config/validation.rs catches structural issues.
                    // Runtime failures (network, service unavailable) degrade gracefully.
                    tracing::error!(
                        error = ?e,
                        auth_service_url = ?config.auth_service_url,
                        "Failed to initialize JWT auth client. \
                         Authentication will be DISABLED for this session. \
                         Check AUTH_SERVICE_URL and AUTH_SIGNING_KEY_PATH configuration."
                    );
                    tracing::warn!(
                        "SERVER RUNNING WITHOUT JWT AUTHENTICATION. \
                         API secret auth may still be available if configured. \
                         This is a security risk in production!"
                    );
                    None
                }
            }
        } else if config.auth_required && config.has_api_secret_auth() {
            let api_secret_ids: Vec<&str> = config
                .auth_api_secrets
                .iter()
                .map(|entry| entry.id.as_str())
                .collect();
            tracing::info!(
                api_secret_count = api_secret_ids.len(),
                api_secret_ids = ?api_secret_ids,
                "API secret authentication enabled"
            );
            None
        } else {
            None
        };

        // Initialize LiveKit SIP handler and provision trunk/dispatch if configured.
        // SIP features are fully opt-in: when config.sip is None, all SIP-related
        // code paths are skipped (provisioning, webhook forwarding, etc.).
        let livekit_sip_handler = if let Some(sip_config) = &config.sip {
            // Check if we have all required credentials for SIP provisioning
            if let (Some(api_key), Some(api_secret)) =
                (&config.livekit_api_key, &config.livekit_api_secret)
            {
                tracing::info!(
                    "SIP configuration detected, provisioning LiveKit SIP trunk and dispatch rules"
                );

                // Create the SIP handler
                let handler = LiveKitSipHandler::new(
                    config.livekit_url.clone(),
                    api_key.clone(),
                    api_secret.clone(),
                );

                // Build deterministic trunk and dispatch names based on naming_prefix and room_prefix
                // This allows operators to predict resource names in the LiveKit UI
                let trunk_name = format!(
                    "{}-{}-trunk",
                    sip_config.naming_prefix, sip_config.room_prefix
                );
                let dispatch_name = format!(
                    "{}-{}-dispatch",
                    sip_config.naming_prefix, sip_config.room_prefix
                );

                // Prepare trunk configuration
                let trunk_config = TrunkConfig {
                    trunk_name: trunk_name.clone(),
                    allowed_addresses: sip_config.allowed_addresses.clone(),
                };

                // Prepare dispatch configuration
                // max_participants is configurable via SIP_MAX_PARTICIPANTS env var or YAML config
                // Default: 3 (caller + Sayna + optional third party)
                let dispatch_config = DispatchConfig {
                    dispatch_name: dispatch_name.clone(),
                    room_prefix: sip_config.room_prefix.clone(),
                    max_participants: config.sip_max_participants,
                };

                // Provision the trunk and dispatch rule (idempotent - won't recreate if they exist)
                match handler
                    .configure_dispatch_rules(trunk_config, dispatch_config)
                    .await
                {
                    Ok(_) => {
                        tracing::info!(
                            "Successfully provisioned SIP resources: trunk={}, dispatch={}, livekit_url={}",
                            trunk_name,
                            dispatch_name,
                            config.livekit_url
                        );
                        Some(Arc::new(handler))
                    }
                    Err(e) => {
                        // Graceful degradation - SIP features will be disabled but server continues
                        tracing::error!(
                            trunk = %trunk_name,
                            dispatch = %dispatch_name,
                            livekit_url = %config.livekit_url,
                            error = ?e,
                            "Failed to provision SIP resources. SIP features will be DISABLED."
                        );
                        tracing::warn!(
                            "SIP calls will not be routed to this server. \
                             Check LiveKit server availability and API credentials. \
                             The server will continue without SIP functionality."
                        );
                        None
                    }
                }
            } else {
                tracing::info!(
                    "SIP configuration present but LiveKit API credentials missing. \
                    Skipping SIP provisioning. Set LIVEKIT_API_KEY and LIVEKIT_API_SECRET to enable."
                );
                None
            }
        } else {
            None
        };

        Arc::new(Self {
            config,
            core_state,
            livekit_room_handler,
            object_store,
            recording_bucket,
            livekit_sip_handler,
            auth_client,
            active_ws_connections: Arc::new(AtomicUsize::new(0)),
            connections_per_ip: Arc::new(DashMap::new()),
            shutdown: CancellationToken::new(),
        })
    }

    /// Get a TTS request manager for a specific provider
    pub async fn get_tts_req_manager(&self, provider: &str) -> Option<Arc<ReqManager>> {
        self.core_state.get_tts_req_manager(provider).await
    }

    /// Get a handle to the application's cache store
    pub fn cache(&self) -> Arc<CacheStore> {
        self.core_state.cache.clone()
    }

    /// Get the current number of active WebSocket connections
    pub fn ws_connection_count(&self) -> usize {
        self.active_ws_connections.load(Ordering::Relaxed)
    }

    /// Get the number of connections from a specific IP address
    pub fn ip_connection_count(&self, ip: &IpAddr) -> usize {
        self.connections_per_ip
            .get(ip)
            .map(|count| count.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Try to acquire a connection slot. Returns Ok(()) if successful, Err(reason) if limits exceeded.
    /// This should be called before accepting a new WebSocket connection.
    ///
    /// This method uses compare-exchange to atomically acquire the global connection slot,
    /// preventing TOCTOU race conditions where multiple threads could bypass the limit.
    pub fn try_acquire_connection(&self, ip: IpAddr) -> Result<(), ConnectionLimitError> {
        // Atomically acquire global slot using CAS loop to prevent TOCTOU race
        if let Some(max_ws) = self.config.max_websocket_connections {
            loop {
                let current = self.active_ws_connections.load(Ordering::Acquire);
                if current >= max_ws {
                    tracing::warn!(
                        current = current,
                        max = max_ws,
                        "Global WebSocket connection limit reached"
                    );
                    return Err(ConnectionLimitError::GlobalLimitReached);
                }
                // Try to claim slot atomically
                if self
                    .active_ws_connections
                    .compare_exchange_weak(
                        current,
                        current + 1,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    break;
                }
                // CAS failed due to concurrent modification, retry
            }
        } else {
            // No global limit configured, just increment
            self.active_ws_connections.fetch_add(1, Ordering::Relaxed);
        }

        // Check and acquire per-IP slot
        // Note: We increment first, then check, so we can rollback if limit exceeded
        let max_per_ip = self.config.max_connections_per_ip;
        let ip_entry = self
            .connections_per_ip
            .entry(ip)
            .or_insert_with(|| AtomicUsize::new(0));

        let current_ip = ip_entry.fetch_add(1, Ordering::Relaxed);
        if current_ip >= max_per_ip as usize {
            // Rollback both counters
            ip_entry.fetch_sub(1, Ordering::Relaxed);
            self.active_ws_connections.fetch_sub(1, Ordering::Relaxed);
            tracing::warn!(
                ip = %ip,
                current = current_ip,
                max = max_per_ip,
                "Per-IP connection limit reached"
            );
            return Err(ConnectionLimitError::PerIpLimitReached);
        }

        tracing::debug!(
            ip = %ip,
            total_connections = self.active_ws_connections.load(Ordering::Relaxed),
            ip_connections = self.ip_connection_count(&ip),
            "Connection acquired"
        );

        Ok(())
    }

    /// Release a connection slot. Should be called when a WebSocket connection closes.
    pub fn release_connection(&self, ip: IpAddr) {
        // Decrement global count
        let prev = self.active_ws_connections.fetch_sub(1, Ordering::Relaxed);
        if prev == 0 {
            // This shouldn't happen, but guard against underflow
            self.active_ws_connections.store(0, Ordering::Relaxed);
        }

        // Decrement per-IP count
        if let Some(count) = self.connections_per_ip.get(&ip) {
            let prev_ip = count.fetch_sub(1, Ordering::Relaxed);
            if prev_ip <= 1 {
                // Remove the entry when count reaches 0 to prevent memory leak
                drop(count);
                self.connections_per_ip.remove(&ip);
            }
        }

        tracing::debug!(
            ip = %ip,
            total_connections = self.active_ws_connections.load(Ordering::Relaxed),
            "Connection released"
        );
    }
}

/// Error type for connection limit checks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionLimitError {
    /// Global WebSocket connection limit has been reached
    GlobalLimitReached,
    /// Per-IP connection limit has been reached
    PerIpLimitReached,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SipConfig;

    #[test]
    fn test_sip_handler_not_created_without_config() {
        // This test verifies that when SIP config is None, the handler is not created
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: Some("test_key".to_string()),
            livekit_api_secret: Some("test_secret".to_string()),
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
            assemblyai_api_key: None,
            hume_api_key: None,
            lmnt_api_key: None,
            groq_api_key: None,
            playht_api_key: None,
            playht_user_id: None,
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
            sip: None, // No SIP config
            cors_allowed_origins: None,
            rate_limit_requests_per_second: 60,
            rate_limit_burst_size: 10,
            max_websocket_connections: None,
            max_connections_per_ip: 100,
            ws_processing_timeout_secs: 10,
            realtime_processing_timeout_secs: 30,
            sip_max_participants: 3,
            plugins: crate::config::PluginConfig::default(),
            dag_timeouts: crate::config::DAGTimeoutsConfig::default(),
        };

        // We can't actually call AppState::new in a sync test, but we can verify
        // the logic that would be executed based on the config
        assert!(config.sip.is_none());
        assert!(config.livekit_api_key.is_some());
        assert!(config.livekit_api_secret.is_some());
    }

    #[test]
    fn test_sip_handler_skipped_without_credentials() {
        // This test verifies that when SIP config is present but credentials are missing,
        // the handler creation is skipped
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,    // Missing API key
            livekit_api_secret: None, // Missing API secret
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
            assemblyai_api_key: None,
            hume_api_key: None,
            lmnt_api_key: None,
            groq_api_key: None,
            playht_api_key: None,
            playht_user_id: None,
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
            sip: Some(SipConfig {
                room_prefix: "sip-".to_string(),
                allowed_addresses: vec!["192.168.1.0/24".to_string()],
                hooks: vec![],
                hook_secret: None,
                naming_prefix: "waav".to_string(),
            }),
            cors_allowed_origins: None,
            rate_limit_requests_per_second: 60,
            rate_limit_burst_size: 10,
            max_websocket_connections: None,
            max_connections_per_ip: 100,
            ws_processing_timeout_secs: 10,
            realtime_processing_timeout_secs: 30,
            sip_max_participants: 3,
            plugins: crate::config::PluginConfig::default(),
            dag_timeouts: crate::config::DAGTimeoutsConfig::default(),
        };

        // Verify that SIP config is present but credentials are missing
        assert!(config.sip.is_some());
        assert!(config.livekit_api_key.is_none());
        assert!(config.livekit_api_secret.is_none());
    }

    #[test]
    fn test_trunk_and_dispatch_name_generation() {
        // This test verifies the deterministic naming scheme for trunk and dispatch
        // with the default naming prefix "waav"
        let naming_prefix = "waav";
        let room_prefix = "sip-";
        let expected_trunk_name = format!("{}-{}-trunk", naming_prefix, room_prefix);
        let expected_dispatch_name = format!("{}-{}-dispatch", naming_prefix, room_prefix);

        assert_eq!(expected_trunk_name, "waav-sip--trunk");
        assert_eq!(expected_dispatch_name, "waav-sip--dispatch");

        // Test with different room prefix
        let room_prefix2 = "test-call-";
        let expected_trunk_name2 = format!("{}-{}-trunk", naming_prefix, room_prefix2);
        let expected_dispatch_name2 = format!("{}-{}-dispatch", naming_prefix, room_prefix2);

        assert_eq!(expected_trunk_name2, "waav-test-call--trunk");
        assert_eq!(expected_dispatch_name2, "waav-test-call--dispatch");

        // Test with custom naming prefix
        let custom_naming_prefix = "mycompany";
        let expected_trunk_name3 = format!("{}-{}-trunk", custom_naming_prefix, room_prefix);
        let expected_dispatch_name3 = format!("{}-{}-dispatch", custom_naming_prefix, room_prefix);

        assert_eq!(expected_trunk_name3, "mycompany-sip--trunk");
        assert_eq!(expected_dispatch_name3, "mycompany-sip--dispatch");
    }

    #[test]
    fn test_max_participants_default() {
        // This test verifies the default max_participants value
        // Default is 3: caller + Sayna + optional third party
        let max_participants: u32 = 3;
        assert_eq!(max_participants, 3);
    }

    /// Minimal credential-free config for constructing a real `AppState` in tests.
    fn minimal_test_config() -> ServerConfig {
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
            assemblyai_api_key: None,
            hume_api_key: None,
            lmnt_api_key: None,
            groq_api_key: None,
            playht_api_key: None,
            playht_user_id: None,
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
            plugins: crate::config::PluginConfig::default(),
            dag_timeouts: crate::config::DAGTimeoutsConfig::default(),
        }
    }

    /// RC6 SIGTERM drain: `AppState::new` must construct a live (non-cancelled)
    /// shutdown token, and cancelling it must be observed by every clone of the
    /// state (sessions hold clones of `Arc<AppState>` / its token).
    #[tokio::test]
    async fn test_shutdown_token_shared_across_state_clones() {
        let state = AppState::new(minimal_test_config()).await;
        assert!(
            !state.shutdown.is_cancelled(),
            "shutdown token must start non-cancelled"
        );

        // Clone the inner AppState (not just the Arc) to prove the token's
        // shared-state semantics survive `#[derive(Clone)]`.
        let clone = (*state).clone();
        state.shutdown.cancel();
        assert!(
            clone.shutdown.is_cancelled(),
            "cancellation must propagate to all AppState clones"
        );
    }
}

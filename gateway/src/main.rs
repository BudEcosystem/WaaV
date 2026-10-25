use std::net::SocketAddr;
use std::path::PathBuf;

#[cfg(feature = "openapi")]
use std::fs;

use tracing::{error, info, warn};

use axum::{Router, middleware};
use axum_server::Handle;
use axum_server::tls_rustls::RustlsConfig;
use clap::{Parser, Subcommand};
use http::{
    HeaderName, HeaderValue, Method,
    header::{AUTHORIZATION, CONTENT_TYPE},
};
use tokio::net::TcpListener;
use tower_governor::{
    GovernorLayer, governor::GovernorConfigBuilder, key_extractor::PeerIpKeyExtractor,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;

use anyhow::anyhow;

use waav_gateway::{
    ServerConfig, global_registry, init,
    middleware::{auth_middleware, connection_limit_middleware, request_id_middleware},
    routes,
    state::AppState,
};

#[cfg(feature = "plugins-dynamic")]
use waav_gateway::plugin::DynamicPluginLoader;

fn parse_cors_allowed_origins(origins: &str) -> anyhow::Result<Vec<HeaderValue>> {
    let mut parsed = Vec::new();

    for (index, origin) in origins.split(',').enumerate() {
        let origin = origin.trim();
        if origin.is_empty() {
            anyhow::bail!(
                "Invalid CORS allowed origin at position {}: empty origin",
                index + 1
            );
        }

        let value = HeaderValue::from_str(origin).map_err(|e| {
            anyhow!(
                "Invalid CORS allowed origin at position {} ({origin:?}): {e}",
                index + 1
            )
        })?;
        parsed.push(value);
    }

    if parsed.is_empty() {
        anyhow::bail!("CORS allowed origins must contain at least one origin or '*'");
    }

    Ok(parsed)
}

/// WaaV Gateway - Real-time voice processing server
#[derive(Parser, Debug)]
#[command(name = "waav-gateway")]
#[command(version, about, long_about = None)]
struct Cli {
    /// Path to configuration file (YAML)
    #[arg(short = 'c', long = "config", value_name = "FILE", global = true)]
    config: Option<PathBuf>,

    /// Environment file to load before configuration (`KEY=value` lines; variables already
    /// set in the process environment win). Also `WAAV_ENV_FILE`. Default: `.env` in the
    /// current directory only, when it exists. An empty value disables env-file loading.
    #[arg(long = "env-file", value_name = "FILE", global = true)]
    env_file: Option<PathBuf>,

    /// Subcommand to run
    #[command(subcommand)]
    command: Option<Commands>,
}

/// Outcome of [`load_env_file`], reported once tracing is initialised.
enum EnvFile {
    Loaded(PathBuf),
    /// Disabled, or no `./.env` present.
    None,
    /// An explicitly requested file could not be read or parsed: startup fails.
    Failed(PathBuf, dotenvy::Error),
    /// The default `./.env` exists but could not be parsed: warned, startup continues.
    DefaultUnreadable(PathBuf, dotenvy::Error),
}

/// Load `explicit` if given (an empty path disables loading), else `./.env` if it exists.
///
/// Never searches parent directories. Existing process variables are not overridden.
fn load_env_file(explicit: Option<&std::path::Path>) -> EnvFile {
    match explicit {
        Some(p) if p.as_os_str().is_empty() => EnvFile::None,
        Some(p) => match dotenvy::from_path(p) {
            Ok(()) => EnvFile::Loaded(p.to_path_buf()),
            Err(e) => EnvFile::Failed(p.to_path_buf(), e),
        },
        None => {
            let p = PathBuf::from(".env");
            if !p.is_file() {
                return EnvFile::None;
            }
            match dotenvy::from_path(&p) {
                Ok(()) => EnvFile::Loaded(p),
                Err(e) => EnvFile::DefaultUnreadable(p, e),
            }
        }
    }
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize turn detection models
    Init,

    /// Check the configuration without starting the server: the environment and env file, the
    /// config file (`-c`, unknown keys included), operator credentials, DAG templates, and the
    /// auth, metrics and segmented speech-to-text settings. Prints each problem; exits 1 if any.
    Check,

    /// Generate OpenAPI specification
    #[cfg(feature = "openapi")]
    Openapi {
        /// Output format (yaml or json)
        #[arg(short = 'f', long = "format", default_value = "yaml")]
        format: String,

        /// Output file path (prints to stdout if not specified)
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
}

/// Every check `main` makes before it binds, without binding, loading models or dialling
/// anything: what `waav-gateway check` reports. Each entry names its source.
fn check_configuration(config_path: Option<&PathBuf>) -> Vec<String> {
    let mut problems = Vec::new();
    let config = match config_path {
        Some(path) => {
            match ServerConfig::unknown_yaml_keys(path) {
                Ok(keys) => problems.extend(
                    keys.into_iter()
                        .map(|k| format!("{}: unknown key `{k}`", path.display())),
                ),
                Err(e) => problems.push(e),
            }
            ServerConfig::from_file(path).map_err(|e| e.to_string())
        }
        None => ServerConfig::from_env().map_err(|e| e.to_string()),
    };
    let config = match config {
        Ok(config) => Some(config),
        Err(e) => {
            problems.push(format!("configuration: {e}"));
            None
        }
    };
    if let Some(config) = config.as_ref()
        && let Err(e) = waav_gateway::core::alias::validate_aliases(&config.aliases)
    {
        problems.push(format!("aliases: {e}"));
    }
    if let Err(e) = waav_gateway::core::credentials::initialize_credentials() {
        problems.push(format!("credentials: {e}"));
    }
    #[cfg(feature = "dag-routing")]
    if let Ok(dir) = std::env::var("WAAV_DAG_TEMPLATES_DIR")
        && !dir.trim().is_empty()
        && let Err(e) =
            waav_gateway::dag::initialize_templates(&waav_gateway::dag::TemplatesConfig {
                templates: Default::default(),
                templates_dir: Some(PathBuf::from(dir)),
                templates_dir_required: true,
            })
    {
        problems.push(format!("DAG templates: {e}"));
    }
    if let Err(e) = waav_gateway::auth::AuthPolicy::from_env() {
        problems.push(format!("auth: {e}"));
    }
    // A malformed value refuses every client endpoint at runtime (fail closed): say so here.
    use waav_gateway::core::client_destination::{CLIENT_ENDPOINTS_ENV, parse_client_endpoints};
    if let Err(e) = parse_client_endpoints(std::env::var(CLIENT_ENDPOINTS_ENV).ok().as_deref()) {
        problems.push(format!(
            "{e} (a malformed value refuses every client-chosen endpoint)"
        ));
    }
    if let Some(config) = config.as_ref() {
        let api_requires_auth = config.auth_required
            || config.has_api_secret_auth()
            || waav_gateway::auth::bud_mode::BudModeConfig::from_env().is_some();
        if let Err(e) = waav_gateway::handlers::api::MetricsPolicy::from_env(api_requires_auth) {
            problems.push(format!("metrics: {e}"));
        }
    }
    if let Err(e) = waav_gateway::core::stt::segmented::live::SttLiveShared::from_env(None) {
        problems.push(format!("segmented speech-to-text: {e}"));
    }
    problems
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI arguments first: `--env-file` decides which env file (if any) is loaded.
    let cli = Cli::parse();

    // Load the env file before tracing and config read the environment. Not
    // `dotenvy::dotenv()`: that searches the current directory AND EVERY PARENT for a `.env`,
    // so a gateway started in /srv/app/gateway silently picked up /srv/app/.env (or ~/.env)
    // -- credentials and settings from an unrelated project.
    let explicit_env_file = cli
        .env_file
        .clone()
        .or_else(|| std::env::var_os("WAAV_ENV_FILE").map(PathBuf::from));
    let env_file = load_env_file(explicit_env_file.as_deref());

    // Initialize tracing
    // Exports to OTLP when the chart supplies an endpoint, stdout otherwise. The guard is
    // held for the process lifetime so shutdown can flush: dropping it silently loses whatever
    // is still batched, which is exactly the spans from the request that caused the shutdown.
    let mut tracing_guard = waav_gateway::observability::tracing_init::init();

    // Initialize crypto provider for TLS connections
    // This must be done before any TLS connections are attempted
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow!("Failed to install default crypto provider"))?;

    // Reported only now that tracing is up; values are never logged.
    match env_file {
        EnvFile::Loaded(path) => info!("Loaded environment file {}", path.display()),
        EnvFile::None => {}
        EnvFile::Failed(path, e) => {
            anyhow::bail!("Failed to load environment file {}: {e}", path.display())
        }
        EnvFile::DefaultUnreadable(path, e) => {
            warn!("Ignoring environment file {}: {e}", path.display())
        }
    }

    // Handle subcommands
    if let Some(command) = cli.command {
        match command {
            Commands::Init => {
                init::run().await?;
                return Ok(());
            }
            Commands::Check => {
                let problems = check_configuration(cli.config.as_ref());
                if problems.is_empty() {
                    println!("configuration OK");
                    return Ok(());
                }
                for problem in &problems {
                    eprintln!("error: {problem}");
                }
                eprintln!("{} problem(s) found", problems.len());
                std::process::exit(1);
            }
            #[cfg(feature = "openapi")]
            Commands::Openapi { format, output } => {
                // Validate format
                if format != "yaml" && format != "json" {
                    anyhow::bail!("Invalid format '{}'. Must be 'yaml' or 'json'", format);
                }

                // Generate the spec in the requested format
                let spec_content = match format.as_str() {
                    "yaml" => waav_gateway::docs::openapi::spec_yaml()
                        .map_err(|e| anyhow!("Failed to generate OpenAPI YAML: {}", e))?,
                    "json" => waav_gateway::docs::openapi::spec_json()
                        .map_err(|e| anyhow!("Failed to generate OpenAPI JSON: {}", e))?,
                    other => anyhow::bail!("Invalid format '{}'. Must be 'yaml' or 'json'", other),
                };

                // Write to file or stdout
                if let Some(output_path) = output {
                    fs::write(&output_path, &spec_content).map_err(|e| {
                        anyhow!("Failed to write to {}: {}", output_path.display(), e)
                    })?;
                    println!("OpenAPI spec written to {}", output_path.display());
                } else {
                    println!("{}", spec_content);
                }

                return Ok(());
            }
        }
    }

    // Load configuration from file or environment
    let config = if let Some(config_path) = cli.config {
        println!("Loading configuration from {}", config_path.display());
        ServerConfig::from_file(&config_path).map_err(|e| anyhow!(e.to_string()))?
    } else {
        ServerConfig::from_env().map_err(|e| anyhow!(e.to_string()))?
    };

    // P3: register the server-side alias registry from config (the `aliases:` section).
    // Mirrors the DAG template registry initialization; definitions are server-config-
    // only (SSRF-safe). A client may NAME an alias on its `config` message but never
    // DEFINE one. Done before serving so the first connection resolves correctly.
    {
        // A leg's own server must pass the egress rules: a mistake stops the server here rather
        // than failing every session that names the alias.
        waav_gateway::core::alias::validate_aliases(&config.aliases)
            .map_err(|e| anyhow!("aliases: {e}"))?;
        let n = waav_gateway::core::alias::initialize_aliases(&config.aliases);
        if n > 0 {
            info!("Registered {} server-side config alias(es)", n);
        }
    }

    // Operator credentials (WAAV_CREDENTIALS_FILE / WAAV_CREDENTIALS_JSON): the ONLY secrets a
    // client config may name, each bound to the hosts it may be sent to. A malformed file stops
    // the server (it must never degrade to "no credentials" silently).
    {
        let n = waav_gateway::core::credentials::initialize_credentials()
            .map_err(|e| anyhow!("credentials: {e}"))?;
        if n > 0 {
            info!("Registered {} operator credential(s)", n);
        }
    }

    // Operator DAG templates (WAAV_DAG_TEMPLATES_DIR). `initialize_templates` existed but was
    // never called, so a standalone gateway had NO templates and every DAG — with its
    // `${VAR}` keys — had to arrive inline from the client.
    #[cfg(feature = "dag-routing")]
    {
        if let Ok(dir) = std::env::var("WAAV_DAG_TEMPLATES_DIR")
            && !dir.trim().is_empty()
        {
            let n = waav_gateway::dag::initialize_templates(&waav_gateway::dag::TemplatesConfig {
                templates: Default::default(),
                templates_dir: Some(std::path::PathBuf::from(dir)),
                templates_dir_required: true,
            })
            .map_err(|e| anyhow!("DAG templates: {e}"))?;
            info!("Registered {} operator DAG template(s)", n);
        }
    }

    // Initialize the plugin registry (including built-in plugins)
    let registry = global_registry();

    // Load dynamic plugins if the feature is enabled and a plugin directory is configured
    #[cfg(feature = "plugins-dynamic")]
    {
        if config.plugins.enabled {
            if let Some(ref plugin_dir) = config.plugins.plugin_dir {
                info!("Loading dynamic plugins from: {}", plugin_dir.display());
                let mut loader = DynamicPluginLoader::new();
                match loader.load_all_from_directory(plugin_dir, registry) {
                    Ok(count) => {
                        if count > 0 {
                            info!("Loaded {} dynamic plugin(s)", count);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load dynamic plugins: {}", e);
                    }
                }
            }
        }
    }
    // Suppress unused variable warning when plugins-dynamic feature is not enabled
    let _ = registry;

    let address = config.address();
    let tls_config = config.tls.clone();
    let is_tls_enabled = config.is_tls_enabled();
    let rate_limit_rps = config.rate_limit_requests_per_second;
    let rate_limit_burst = config.rate_limit_burst_size;
    let cors_origins = config.cors_allowed_origins.clone();
    println!("Starting server on {address}");

    // `/metrics` exposure, resolved before `config` moves into the state. An API that
    // authenticates its callers must not leave the exposition as the one open door.
    let api_requires_auth = config.auth_required
        || config.has_api_secret_auth()
        || waav_gateway::auth::bud_mode::BudModeConfig::from_env().is_some();
    let metrics_policy = waav_gateway::handlers::api::MetricsPolicy::from_env(api_requires_auth)
        .map_err(|e| anyhow!("invalid metrics configuration: {e}"))?;
    if metrics_policy.main == waav_gateway::handlers::api::MetricsAccess::Locked {
        warn!(
            "/metrics answers 403: the API requires authentication and no metrics access is \
             configured. Set WAAV_METRICS_TOKEN (scrape with a bearer token), WAAV_METRICS_ADDR \
             (a separate, unexposed listener), or WAAV_METRICS_PUBLIC=true"
        );
    }
    metrics_policy.clone().install();

    // Create application state
    let mut app_state = AppState::try_new(config)
        .await
        .map_err(|e| anyhow!("invalid app state config: {e}"))?;

    // FRD-018: bring up the Bud control plane, if configured.
    //
    // Boot failure is fatal on purpose. A WaaV that starts with an empty auth snapshot answers
    // 401 to every valid credential — a total outage from a pod that looks perfectly healthy.
    // Failing here stalls the rollout instead, which is the outcome an operator can act on.
    if let Some(bud_cfg) = waav_gateway::auth::bud_mode::BudModeConfig::from_env() {
        let (policy_redis_url, policy_redis_db) = (bud_cfg.redis_url.clone(), bud_cfg.redis_db);
        match waav_gateway::auth::bud_mode::BudMode::start(bud_cfg).await {
            Ok(bud) => {
                bud.spawn_keyspace_loop();
                // FRD-022: each deployment's rate limits, concurrency cap, retry, fallback and
                // breakers, enforced across replicas through the same Redis.
                let policies =
                    match waav_gateway::core::deployment_policy::DeploymentPolicies::connect(
                        &policy_redis_url,
                        policy_redis_db,
                    )
                    .await
                    {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!(
                                "WARNING: deployment limits fall back to per-replica enforcement: {e}"
                            );
                            waav_gateway::core::deployment_policy::DeploymentPolicies::local()
                        }
                    };
                if let Some(state) = std::sync::Arc::get_mut(&mut app_state) {
                    state.bud_mode = Some(bud);
                    state.policies = Some(policies);
                } else {
                    eprintln!(
                        "FATAL: application state was already shared; cannot install the Bud control plane"
                    );
                    std::process::exit(1);
                }
                println!("Bud control plane active: identity and voice endpoints resolved locally");
            }
            Err(e) => {
                eprintln!("FATAL: Bud control plane failed to start: {e}");
                std::process::exit(1);
            }
        }
    } else {
        println!("Bud control plane not configured (WAAV_REDIS_URL unset); running standalone");
    }
    let app_state = app_state;

    // Each TTS deployment's voice list, for budapp's settings page (see `voice_catalog`). Needs
    // the final shared state — the publisher fetches with the plaintext credentials only this
    // process holds — so it starts here, after the control plane is installed.
    if waav_gateway::handlers::voice_catalog::spawn(app_state.clone()).is_some() {
        println!("Voice catalog publisher active: voice lists written to voice_catalog:*");
    }
    // How each transcription deployment is served on a live call, for budapp (Release 2).
    if waav_gateway::handlers::stt_capability::spawn(app_state.clone()).is_some() {
        println!("Speech-to-text capability publisher active: written to voice_capability:*");
    }

    // Create protected API routes with authentication middleware
    let protected_routes = routes::api::create_api_router().layer(middleware::from_fn_with_state(
        app_state.clone(),
        auth_middleware,
    ));

    // Create WebSocket routes with connection limit and auth middleware
    // Layer order (outer to inner): connection_limit -> auth -> handler
    // - connection_limit_middleware: Enforces max connections (global and per-IP)
    // - auth_middleware: Validates auth token (when enabled) or sets empty context
    let ws_routes = routes::ws::create_ws_router()
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            connection_limit_middleware,
        ));

    // Create Realtime WebSocket routes for audio-to-audio streaming (OpenAI Realtime API)
    // Also uses connection limit middleware for capacity management
    let realtime_routes = routes::realtime::create_realtime_router()
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            connection_limit_middleware,
        ));

    // FRD-023: `/v1/realtime` (OpenAI Realtime GA on Bud deployments) and the client-secret mint.
    // No auth_middleware: the handlers authenticate themselves (subprotocol and `api-key`
    // credentials, `ek_bud_` secrets, `?token=` refused). The connection limit still applies, so
    // every session holds a slot (FRD-022).
    let openai_realtime_routes = routes::openai_realtime::create_openai_realtime_router().layer(
        middleware::from_fn_with_state(app_state.clone(), connection_limit_middleware),
    );

    // Live latency-profile debug surface (`WAAV_DEBUG_PROFILE=1`): JSON snapshot
    // + per-turn SSE. Auth-gated exactly like the protected API; without the env
    // flag the routes are not mounted at all (double lock, never public).
    let debug_profile_routes = if app_state.core_state.profiling.debug_routes {
        Router::new()
            .route(
                "/debug/profile",
                axum::routing::get(waav_gateway::handlers::debug_profile::profile_snapshot),
            )
            .route(
                "/debug/profile/stream",
                axum::routing::get(waav_gateway::handlers::debug_profile::profile_stream),
            )
            .layer(middleware::from_fn_with_state(
                app_state.clone(),
                auth_middleware,
            ))
    } else {
        Router::new()
    };

    // Create webhook routes (no auth - uses LiveKit signature verification)
    let webhook_routes = routes::webhooks::create_webhook_router();

    // Create public, no-auth operability routes (health/readiness/metrics).
    // - `/` and `/livez`: liveness (process up); never touches upstreams.
    // - `/readyz`: readiness (config + enabled-provider credentials + cached TCP reachability);
    //   returns 503 + a per-provider JSON breakdown when an enabled provider is unreachable.
    // - `/metrics`: Prometheus text exposition (waav_provider_* + waav_circuit_breaker_state).
    // - `/ready`: FRD-018 readiness. Distinct from `/readyz`, which reports on vendor
    //   reachability: this one reports whether the Bud control-plane snapshot has hydrated.
    //   Before first hydration every valid credential resolves to 401, so a pod in that state
    //   must stay out of the load balancer. It is also the path the Bud chart's probe uses.
    let public_routes = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .route(
            "/ready",
            axum::routing::get(waav_gateway::handlers::api::readiness_check),
        )
        .route(
            "/livez",
            axum::routing::get(waav_gateway::handlers::api::livez),
        )
        .route(
            "/readyz",
            axum::routing::get(waav_gateway::handlers::api::readyz),
        )
        .route(
            "/metrics",
            axum::routing::get(waav_gateway::handlers::api::metrics_handler),
        );

    // Configure rate limiting.
    // SECURITY (S7): key on the real TCP peer IP (PeerIpKeyExtractor), NOT SmartIpKeyExtractor,
    // which trusts client-supplied X-Forwarded-For / X-Real-IP and therefore lets an attacker
    // mint a fresh token bucket per request by rotating the header. Behind a trusted reverse
    // proxy, set up a dedicated trusted-proxy XFF extractor (tracked in W4) — do not re-enable
    // SmartIpKeyExtractor on an internet-facing listener.
    // Disable ONLY via an explicit `rate_limit_requests_per_second: 0` (no silent magic-number
    // threshold that could ship a limiter-off build by accident).
    let governor_layer = if rate_limit_rps == 0 {
        tracing::warn!(
            "Rate limiting EXPLICITLY DISABLED (rate_limit_requests_per_second = 0); \
             do not run this configuration on an internet-facing deployment"
        );
        None
    } else {
        // `GovernorConfigBuilder::per_second(n)` does NOT mean "n requests per second". It
        // sets the REPLENISH INTERVAL: `self.period = Duration::from_secs(n)`. So the old
        // `.per_second(rate_limit_rps)` turned a configured 60 rps into one request every 60
        // seconds -- roughly 3600x tighter than the number says. With the burst spent, the
        // Kubernetes readiness probe got 429 and the pod never became ready.
        //
        // The interval for N requests per second is 1000/N milliseconds, floored at 1ms
        // (anything faster than 1000 rps is effectively unlimited here, and a zero interval
        // is rejected by the builder).
        let period_ms = waav_gateway::rate_limit_period_ms(rate_limit_rps);
        let governor_config = GovernorConfigBuilder::default()
            .per_millisecond(period_ms)
            .burst_size(rate_limit_burst)
            .key_extractor(PeerIpKeyExtractor)
            .finish()
            .ok_or_else(|| anyhow!("Failed to build rate limiter config"))?;
        println!(
            "Rate limiting: {rate_limit_rps} req/s per IP (one token per {period_ms}ms, burst {rate_limit_burst})"
        );
        Some(GovernorLayer::new(governor_config))
    };

    // Configure CORS
    let cors_layer = if let Some(ref origins) = cors_origins {
        if origins == "*" {
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers([
                    AUTHORIZATION,
                    CONTENT_TYPE,
                    HeaderName::from_static("x-provider-api-key"),
                ])
                .allow_credentials(false)
        } else {
            // Parse comma-separated origins
            let origins = parse_cors_allowed_origins(origins)?;
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers([
                    AUTHORIZATION,
                    CONTENT_TYPE,
                    HeaderName::from_static("x-provider-api-key"),
                ])
                .allow_credentials(true)
        }
    } else {
        // No CORS configured - strict same-origin only for production security
        // Cross-origin requests will be blocked. To enable CORS, set CORS_ALLOWED_ORIGINS
        // environment variable or configure security.cors_allowed_origins in YAML.
        info!(
            "CORS not configured, defaulting to same-origin only. \
             Set CORS_ALLOWED_ORIGINS to enable cross-origin access."
        );
        CorsLayer::new()
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([
                AUTHORIZATION,
                CONTENT_TYPE,
                HeaderName::from_static("x-provider-api-key"),
            ])
            .allow_credentials(false)
        // No allow_origin = same-origin only (browsers block cross-origin requests)
    };

    // Security headers
    let security_headers = tower::ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            http::header::X_CONTENT_TYPE_OPTIONS,
            http::HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            http::header::X_FRAME_OPTIONS,
            http::HeaderValue::from_static("DENY"),
        ));

    // RC6 SIGTERM session drain: clone the app-wide shutdown token before `app_state`
    // moves into the router. On SIGTERM/SIGINT it is cancelled *before* axum's graceful
    // drain begins, so every live WebSocket session loop (which selects on this token)
    // can send a final protocol notice and tear down its providers within the drain window.
    let shutdown_token = app_state.shutdown.clone();
    let policies_for_shutdown = app_state.policies.clone();

    // Combine all routes: webhook + protected + websocket + realtime + debug, rate-limited;
    // then the public operability routes merged on top, deliberately OUTSIDE the governor.
    //
    // `/`, `/ready`, `/livez` and `/readyz` are what the kubelet polls, and `/metrics` is what
    // Prometheus scrapes. Every one of those arrives from the node's own address — and because
    // the limiter keys on the real TCP peer IP (see the S7 note above), behind Traefik that is
    // the SAME bucket as all real traffic. Counting probes against it means a busy pod fails
    // its own liveness check and is restarted for being popular, which is precisely backwards.
    // These are also the endpoints that must answer when the pod is in trouble.
    let limited_routes = webhook_routes
        .merge(protected_routes)
        .merge(ws_routes)
        .merge(realtime_routes)
        .merge(openai_realtime_routes)
        .merge(debug_profile_routes)
        .with_state(app_state.clone())
        .layer(tower::util::option_layer(governor_layer));

    // The operator listener (WAAV_METRICS_ADDR): Prometheus and the probes, bound to an address
    // the ingress does not route to. `/readyz` there carries the per-provider breakdown, which the
    // public port then omits (it named the vendor credentials this gateway holds). The probes stay
    // on the public port too: the kubelet and the Bud chart poll them there.
    if let Some((metrics_addr, _)) = metrics_policy.dedicated {
        let metrics_app = Router::new()
            .route(
                "/metrics",
                axum::routing::get(waav_gateway::handlers::api::dedicated_metrics_handler),
            )
            .route(
                "/livez",
                axum::routing::get(waav_gateway::handlers::api::livez),
            )
            .route(
                "/ready",
                axum::routing::get(waav_gateway::handlers::api::readiness_check),
            )
            .route(
                "/readyz",
                axum::routing::get(waav_gateway::handlers::api::admin_readyz),
            )
            .with_state(app_state.clone());
        let metrics_listener = TcpListener::bind(metrics_addr)
            .await
            .map_err(|e| anyhow!("Failed to bind metrics listener {metrics_addr}: {e}"))?;
        info!("Operator listener on http://{metrics_addr}: /metrics, /livez, /ready, /readyz");
        tokio::spawn(async move {
            if let Err(e) = axum::serve(metrics_listener, metrics_app).await {
                error!(error = %e, "metrics listener stopped");
            }
        });
    }

    let app = public_routes
        .merge(limited_routes)
        .with_state(app_state)
        .layer(cors_layer)
        .layer(security_headers)
        // Request-id/trace-context correlation is the OUTERMOST layer so the id exists before
        // auth, rate-limiting, or any handler logs — every nested `tracing` event inherits it,
        // and it is echoed on the response `x-request-id` header (W-C1 / E13).
        .layer(axum::middleware::from_fn(request_id_middleware));

    // Parse socket address
    let socket_addr: SocketAddr = address
        .parse()
        .map_err(|e| anyhow!("Invalid server address '{}': {}", address, e))?;

    // Start server with or without TLS
    if is_tls_enabled {
        let Some(tls) = tls_config else {
            anyhow::bail!("TLS is enabled but TLS config is missing");
        };

        // Load TLS configuration from certificate and key files
        let rustls_config = RustlsConfig::from_pem_file(&tls.cert_path, &tls.key_path)
            .await
            .map_err(|e| {
                anyhow!(
                    "Failed to load TLS certificates from {} and {}: {}",
                    tls.cert_path.display(),
                    tls.key_path.display(),
                    e
                )
            })?;

        println!("Server listening on https://{} (TLS enabled)", socket_addr);

        // Create a handle for graceful shutdown
        let handle = Handle::new();
        let handle_clone = handle.clone();

        // Spawn a task to handle shutdown signals
        tokio::spawn(async move {
            shutdown_signal().await;
            // RC6: signal in-flight WS sessions to drain BEFORE the HTTP-level drain
            // begins, so each session sends its shutdown notice and closes providers
            // within the 30-second window below.
            shutdown_token.cancel();
            // Trigger graceful shutdown - wait up to 30 seconds for connections to drain
            handle_clone.graceful_shutdown(Some(std::time::Duration::from_secs(30)));
        });

        // Create and run the server
        // TCP_NODELAY on accept: see `server` (FRD-023 TC-PERF-01).
        waav_gateway::server::tls_server(socket_addr, rustls_config)
            .handle(handle)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .map_err(|e| anyhow!("TLS server error: {}", e))?;
    } else {
        println!("Server listening on http://{}", socket_addr);

        let listener = TcpListener::bind(&socket_addr).await?;

        // Use axum::serve with graceful shutdown; TCP_NODELAY on accept (FRD-023 TC-PERF-01).
        axum::serve(
            waav_gateway::server::nodelay_listener(listener),
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            // RC6: cancel the session-drain token BEFORE this future resolves (which is
            // what starts axum's graceful drain), so WS sessions observe shutdown first.
            shutdown_token.cancel();
        })
        .await?;
    }

    // FRD-022: push the hits this replica admitted and hand its reserved share of every
    // deployment's budget back to the other replicas.
    if let Some(policies) = policies_for_shutdown {
        policies.shutdown().await;
    }

    // Flush batched spans BEFORE returning. Without this the exporter is dropped with its
    // batch unsent, and the spans lost are the ones from the requests immediately before
    // shutdown -- exactly the ones someone investigating a bad rollout is looking for.
    tracing_guard.shutdown();

    info!("Server shutdown complete");
    Ok(())
}

/// Wait for shutdown signal (SIGINT or SIGTERM).
/// Returns when a shutdown signal is received.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!(error = %e, "Failed to install Ctrl+C handler");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                error!(error = %e, "Failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received SIGINT (Ctrl+C), initiating graceful shutdown...");
        }
        _ = terminate => {
            info!("Received SIGTERM, initiating graceful shutdown...");
        }
    }

    warn!("Shutdown signal received. Draining connections...");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_env_file_is_loaded_without_overriding_the_process_env() {
        let dir = std::env::temp_dir().join(format!("waav-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("custom.env");
        std::fs::write(&file, "WAAV_TEST_ENV_FILE_A=from-file\n").unwrap();

        assert!(matches!(load_env_file(Some(&file)), EnvFile::Loaded(p) if p == file));
        assert_eq!(
            std::env::var("WAAV_TEST_ENV_FILE_A").as_deref(),
            Ok("from-file")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_explicit_env_file_fails_and_an_empty_one_disables_loading() {
        let missing = std::path::Path::new("/nonexistent/waav/never.env");
        assert!(matches!(load_env_file(Some(missing)), EnvFile::Failed(..)));
        assert!(matches!(
            load_env_file(Some(std::path::Path::new(""))),
            EnvFile::None
        ));
    }

    #[test]
    fn parse_cors_allowed_origins_accepts_trimmed_origins() {
        let origins = parse_cors_allowed_origins(" https://app.example , https://admin.example ")
            .expect("valid CORS origins parse");

        assert_eq!(origins.len(), 2);
        assert_eq!(origins[0].to_str().unwrap(), "https://app.example");
        assert_eq!(origins[1].to_str().unwrap(), "https://admin.example");
    }

    #[test]
    fn parse_cors_allowed_origins_rejects_empty_entries() {
        let err = parse_cors_allowed_origins("https://app.example,,https://admin.example")
            .expect_err("empty CORS entries must fail startup");

        assert!(err.to_string().contains("empty origin"), "{err}");
    }

    #[test]
    fn parse_cors_allowed_origins_rejects_malformed_header_values() {
        let err = parse_cors_allowed_origins("https://app.example,bad\norigin")
            .expect_err("malformed CORS origins must fail startup");

        let message = err.to_string();
        assert!(message.contains("position 2"), "{message}");
        assert!(message.contains("bad\\norigin"), "{message}");
    }
}

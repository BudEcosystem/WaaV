use std::net::SocketAddr;
use std::path::PathBuf;

#[cfg(feature = "openapi")]
use std::fs;

use tracing::info;

use axum::{Router, middleware};
use axum_server::tls_rustls::RustlsConfig;
use clap::{Parser, Subcommand};
use http::{
    HeaderName, Method,
    header::{AUTHORIZATION, CONTENT_TYPE},
};
use tokio::net::TcpListener;
use tower_governor::{
    GovernorLayer, governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;

use anyhow::anyhow;

use waav_gateway::{
    ServerConfig, global_registry, init,
    middleware::{auth_middleware, connection_limit_middleware},
    routes,
    state::AppState,
};

#[cfg(feature = "plugins-dynamic")]
use waav_gateway::plugin::DynamicPluginLoader;

/// WaaV Gateway - Real-time voice processing server
#[derive(Parser, Debug)]
#[command(name = "waav-gateway")]
#[command(version, about, long_about = None)]
struct Cli {
    /// Path to configuration file (YAML)
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    config: Option<PathBuf>,

    /// Subcommand to run
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize turn detection models
    Init,

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if it exists (must be done before config loading)
    let _ = dotenvy::dotenv();

    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Initialize crypto provider for TLS connections
    // This must be done before any TLS connections are attempted
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow!("Failed to install default crypto provider"))?;

    // Parse CLI arguments
    let cli = Cli::parse();

    // Handle subcommands
    if let Some(command) = cli.command {
        match command {
            Commands::Init => {
                init::run().await?;
                return Ok(());
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
                    _ => unreachable!(),
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

    // Create application state
    let app_state = AppState::new(config).await;

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

    // Create webhook routes (no auth - uses LiveKit signature verification)
    let webhook_routes = routes::webhooks::create_webhook_router();

    // Create public health check route (no auth)
    let public_routes = Router::new().route(
        "/",
        axum::routing::get(waav_gateway::handlers::api::health_check),
    );

    // Configure rate limiting (disabled when rate >= 100000 for performance testing)
    let governor_layer = if rate_limit_rps < 100000 {
        let governor_config = GovernorConfigBuilder::default()
            .per_second(rate_limit_rps as u64)
            .burst_size(rate_limit_burst)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("Failed to build rate limiter config");
        Some(GovernorLayer::new(governor_config))
    } else {
        println!("Rate limiting disabled (rate >= 100000/s)");
        None
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
            let origins: Vec<_> = origins
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
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

    // Combine all routes: public + webhook + protected + websocket + realtime
    let app = public_routes
        .merge(webhook_routes)
        .merge(protected_routes)
        .merge(ws_routes)
        .merge(realtime_routes)
        .with_state(app_state)
        .layer(cors_layer)
        .layer(tower::util::option_layer(governor_layer))
        .layer(security_headers);

    // Parse socket address
    let socket_addr: SocketAddr = address
        .parse()
        .map_err(|e| anyhow!("Invalid server address '{}': {}", address, e))?;

    // Start server with or without TLS
    if is_tls_enabled {
        let tls = tls_config.expect("TLS config must be present when TLS is enabled");

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

        axum_server::bind_rustls(socket_addr, rustls_config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .map_err(|e| anyhow!("TLS server error: {}", e))?;
    } else {
        println!("Server listening on http://{}", socket_addr);

        let listener = TcpListener::bind(&socket_addr).await?;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
    }

    Ok(())
}

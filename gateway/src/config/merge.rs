use std::env;
use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use super::parse_auth_api_secrets_json;
use super::sip::{SipConfig, SipHookConfig};
use super::utils::parse_bool;
use super::yaml::YamlConfig;
use super::{AuthApiSecret, DAGTimeoutsConfig, PluginConfig, ServerConfig, TlsConfig};

fn parse_env_value<T>(name: &str) -> Result<Option<T>, Box<dyn std::error::Error>>
where
    T: FromStr,
    T::Err: Display,
{
    match env::var(name) {
        Ok(value) => value
            .parse::<T>()
            .map(Some)
            .map_err(|e| format!("Invalid {name} environment variable: {e}").into()),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(format!("{name} environment variable must be valid UTF-8").into())
        }
    }
}

fn parse_env_bool(name: &str) -> Result<Option<bool>, Box<dyn std::error::Error>> {
    match env::var(name) {
        Ok(value) => parse_bool(&value).map(Some).ok_or_else(|| {
            format!("Invalid {name} environment variable: expected true/false/1/0/yes/no").into()
        }),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(format!("{name} environment variable must be valid UTF-8").into())
        }
    }
}

fn path_from_config_value(value: String) -> Option<PathBuf> {
    if value.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

fn non_empty_string_from_config_value(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn parse_env_path(name: &str) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    match env::var(name) {
        Ok(value) => Ok(path_from_config_value(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(format!("{name} environment variable must be valid UTF-8").into())
        }
    }
}

fn parse_env_non_empty_string(name: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match env::var(name) {
        Ok(value) => Ok(non_empty_string_from_config_value(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(format!("{name} environment variable must be valid UTF-8").into())
        }
    }
}

fn yaml_or_env_path(
    yaml_value: Option<String>,
    env_var: &str,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    match yaml_value.and_then(path_from_config_value) {
        Some(path) => Ok(Some(path)),
        None => parse_env_path(env_var),
    }
}

fn yaml_or_env_non_empty_string(
    yaml_value: Option<String>,
    env_var: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match yaml_value.and_then(non_empty_string_from_config_value) {
        Some(value) => Ok(Some(value)),
        None => parse_env_non_empty_string(env_var),
    }
}

fn yaml_or_env<T>(
    yaml_value: Option<T>,
    env_var: &str,
) -> Result<Option<T>, Box<dyn std::error::Error>>
where
    T: FromStr,
    T::Err: Display,
{
    match yaml_value {
        Some(value) => Ok(Some(value)),
        None => parse_env_value(env_var),
    }
}

fn yaml_or_env_bool(
    yaml_value: Option<bool>,
    env_var: &str,
) -> Result<Option<bool>, Box<dyn std::error::Error>> {
    match yaml_value {
        Some(value) => Ok(Some(value)),
        None => parse_env_bool(env_var),
    }
}

/// Merge YAML configuration with environment variables
///
/// Priority order (highest to lowest):
/// 1. YAML configuration values
/// 2. Environment variables
/// 3. Default values
///
/// This allows environment variables to provide base configuration while YAML
/// can override specific values for different deployment environments.
///
/// # Arguments
/// * `yaml_config` - Optional YAML configuration to use as overrides
///
/// # Returns
/// * `Result<ServerConfig, Box<dyn std::error::Error>>` - The merged configuration or an error
pub fn merge_config(
    yaml_config: Option<YamlConfig>,
) -> Result<ServerConfig, Box<dyn std::error::Error>> {
    let yaml = yaml_config.unwrap_or_default();

    // Helper macro to get value with priority: YAML > ENV > Default
    macro_rules! get_value {
        ($env_var:expr, $yaml_value:expr, $default:expr) => {
            $yaml_value
                .or_else(|| env::var($env_var).ok())
                .unwrap_or_else(|| $default.to_string())
        };
    }

    // Helper macro for optional values: YAML > ENV
    macro_rules! get_optional {
        ($env_var:expr, $yaml_value:expr) => {
            $yaml_value.or_else(|| env::var($env_var).ok())
        };
    }

    // Helper macro for CREDENTIALS: same YAML > ENV precedence, but a
    // placeholder YAML value (the shipped "your-…-api-key" style) — AND an
    // empty-string value from EITHER YAML or env — is treated as UNSET so the
    // `# ENV:` fallback documented in config.yaml actually applies and the
    // handler's missing-key guard fires cleanly. `is_placeholder_credential`
    // already classifies an empty/whitespace string as a placeholder, so the
    // YAML branch handled empty; the env fallback previously did NOT, leaking
    // `Some("")` to the handler (e.g. `HUME_API_KEY=""` → an attempted connect /
    // HTTP 429 instead of "API key not configured"). Run the env value through
    // the SAME placeholder/empty filter. Real values keep their precedence.
    macro_rules! get_credential {
        ($env_var:expr, $yaml_value:expr) => {
            $yaml_value
                .filter(|v: &String| {
                    if super::utils::is_placeholder_credential(v) {
                        tracing::warn!(
                            field = $env_var,
                            "config credential is a placeholder — ignoring it ({} env var applies if set)",
                            $env_var
                        );
                        false
                    } else {
                        true
                    }
                })
                .or_else(|| {
                    env::var($env_var)
                        .ok()
                        // Empty / placeholder env credential = unset (review: hume
                        // empty-key 429). Keeps `Some("")` from reaching handlers.
                        .filter(|v: &String| !super::utils::is_placeholder_credential(v))
                })
        };
    }

    // Server configuration
    let host = get_value!(
        "HOST",
        yaml.server.as_ref().and_then(|s| s.host.clone()),
        "0.0.0.0"
    );

    let port = if let Some(yaml_port) = yaml.server.as_ref().and_then(|s| s.port) {
        yaml_port
    } else if let Ok(port_str) = env::var("PORT") {
        port_str
            .parse::<u16>()
            .map_err(|e| format!("Invalid PORT environment variable: {e}"))?
    } else {
        3001
    };

    // TLS configuration
    let tls_enabled = yaml_or_env_bool(
        yaml.server
            .as_ref()
            .and_then(|s| s.tls.as_ref())
            .and_then(|t| t.enabled),
        "TLS_ENABLED",
    )?
    .unwrap_or(false);

    let tls = if tls_enabled {
        let cert_path = yaml_or_env_path(
            yaml.server
                .as_ref()
                .and_then(|s| s.tls.as_ref())
                .and_then(|t| t.cert_path.clone()),
            "TLS_CERT_PATH",
        )?
        .ok_or("TLS_CERT_PATH is required when TLS is enabled")?;

        let key_path = yaml_or_env_path(
            yaml.server
                .as_ref()
                .and_then(|s| s.tls.as_ref())
                .and_then(|t| t.key_path.clone()),
            "TLS_KEY_PATH",
        )?
        .ok_or("TLS_KEY_PATH is required when TLS is enabled")?;

        Some(TlsConfig {
            cert_path,
            key_path,
        })
    } else {
        None
    };

    // LiveKit configuration
    let livekit_url = get_value!(
        "LIVEKIT_URL",
        yaml.livekit.as_ref().and_then(|l| l.url.clone()),
        "ws://localhost:7880"
    );

    let livekit_public_url = get_value!(
        "LIVEKIT_PUBLIC_URL",
        yaml.livekit.as_ref().and_then(|l| l.public_url.clone()),
        "http://localhost:7880"
    );

    let livekit_api_key = get_credential!(
        "LIVEKIT_API_KEY",
        yaml.livekit.as_ref().and_then(|l| l.api_key.clone())
    );

    let livekit_api_secret = get_credential!(
        "LIVEKIT_API_SECRET",
        yaml.livekit.as_ref().and_then(|l| l.api_secret.clone())
    );

    // Provider API keys
    let deepgram_api_key = get_credential!(
        "DEEPGRAM_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.deepgram_api_key.clone())
    );

    let elevenlabs_api_key = get_credential!(
        "ELEVENLABS_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.elevenlabs_api_key.clone())
    );

    // Google Cloud credentials (can be path, JSON content, or empty for ADC)
    let google_credentials = get_credential!(
        "GOOGLE_APPLICATION_CREDENTIALS",
        yaml.providers
            .as_ref()
            .and_then(|p| p.google_credentials.clone())
    );

    // Azure Speech Services configuration
    let azure_speech_subscription_key = get_credential!(
        "AZURE_SPEECH_SUBSCRIPTION_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.azure_speech_subscription_key.clone())
    );

    let azure_speech_region = get_optional!(
        "AZURE_SPEECH_REGION",
        yaml.providers
            .as_ref()
            .and_then(|p| p.azure_speech_region.clone())
    );

    // Cartesia STT API key
    let cartesia_api_key = get_credential!(
        "CARTESIA_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.cartesia_api_key.clone())
    );

    // OpenAI API key (STT, TTS, and Realtime API)
    let openai_api_key = get_credential!(
        "OPENAI_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.openai_api_key.clone())
    );

    // Azure OpenAI Realtime (OpenAI-protocol clone): api-key + resource/endpoint.
    let azure_openai_api_key = get_credential!(
        "AZURE_OPENAI_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.azure_openai_api_key.clone())
    );
    let azure_openai_endpoint = get_credential!(
        "AZURE_OPENAI_ENDPOINT",
        yaml.providers
            .as_ref()
            .and_then(|p| p.azure_openai_endpoint.clone())
    );

    // Grok / xAI Realtime (OpenAI GA-compatible wire).
    let grok_api_key = get_credential!(
        "GROK_API_KEY",
        yaml.providers.as_ref().and_then(|p| p.grok_api_key.clone())
    );

    // Inworld Realtime (OpenAI GA wire).
    let inworld_api_key = get_credential!(
        "INWORLD_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.inworld_api_key.clone())
    );

    // Google Gemini Live (BidiGenerateContent S2S; `?key=` query auth).
    let gemini_api_key = get_credential!(
        "GEMINI_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.gemini_api_key.clone())
    );

    // Ultravox hosted S2S realtime (`X-API-Key` create-call auth).
    let ultravox_api_key = get_credential!(
        "ULTRAVOX_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.ultravox_api_key.clone())
    );

    // Speechmatics API key (JWT / temp-token) — STT/TTS + Flow (Voice AI) realtime
    // (`Authorization: Bearer <token>`).
    let speechmatics_api_key = get_credential!(
        "SPEECHMATICS_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.speechmatics_api_key.clone())
    );

    // Yandex Cloud AI Studio Realtime (OpenAI-protocol clone; GA wire, Bearer auth).
    // The key is a Yandex IAM token / static API key; the folder id (non-secret,
    // like azure's endpoint) builds the `gpt://<folder>/<model>` connect URI.
    let yandex_api_key = get_credential!(
        "YANDEX_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.yandex_api_key.clone())
    );
    let yandex_folder_id = get_credential!(
        "YANDEX_FOLDER_ID",
        yaml.providers
            .as_ref()
            .and_then(|p| p.yandex_folder_id.clone())
    );

    // AssemblyAI API key (streaming STT)
    let assemblyai_api_key = get_credential!(
        "ASSEMBLYAI_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.assemblyai_api_key.clone())
    );

    // Hume AI API key (TTS and EVI)
    let hume_api_key = get_credential!(
        "HUME_API_KEY",
        yaml.providers.as_ref().and_then(|p| p.hume_api_key.clone())
    );

    // LMNT API key (TTS and voice cloning)
    let lmnt_api_key = get_credential!(
        "LMNT_API_KEY",
        yaml.providers.as_ref().and_then(|p| p.lmnt_api_key.clone())
    );

    // Groq API key (ultra-fast Whisper STT)
    let groq_api_key = get_credential!(
        "GROQ_API_KEY",
        yaml.providers.as_ref().and_then(|p| p.groq_api_key.clone())
    );

    // Play.ht credentials (TTS with voice cloning)
    let playht_api_key = get_credential!(
        "PLAYHT_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.playht_api_key.clone())
    );
    let playht_user_id = get_credential!(
        "PLAYHT_USER_ID",
        yaml.providers
            .as_ref()
            .and_then(|p| p.playht_user_id.clone())
    );

    // IBM Watson credentials (STT/TTS)
    let ibm_watson_api_key = get_credential!(
        "IBM_WATSON_API_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.ibm_watson_api_key.clone())
    );
    let ibm_watson_instance_id = get_credential!(
        "IBM_WATSON_INSTANCE_ID",
        yaml.providers
            .as_ref()
            .and_then(|p| p.ibm_watson_instance_id.clone())
    );
    let ibm_watson_region = get_optional!(
        "IBM_WATSON_REGION",
        yaml.providers
            .as_ref()
            .and_then(|p| p.ibm_watson_region.clone())
    );

    // AWS credentials (Transcribe/Polly)
    let aws_access_key_id = get_optional!(
        "AWS_ACCESS_KEY_ID",
        yaml.providers
            .as_ref()
            .and_then(|p| p.aws_access_key_id.clone())
    );
    let aws_secret_access_key = get_credential!(
        "AWS_SECRET_ACCESS_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.aws_secret_access_key.clone())
    );
    let aws_region = get_optional!(
        "AWS_REGION",
        yaml.providers.as_ref().and_then(|p| p.aws_region.clone())
    );

    // Gnani.ai credentials
    let gnani_token = get_credential!(
        "GNANI_TOKEN",
        yaml.providers.as_ref().and_then(|p| p.gnani_token.clone())
    );
    let gnani_access_key = get_credential!(
        "GNANI_ACCESS_KEY",
        yaml.providers
            .as_ref()
            .and_then(|p| p.gnani_access_key.clone())
    );
    let gnani_certificate_path = yaml_or_env_path(
        yaml.providers
            .as_ref()
            .and_then(|p| p.gnani_certificate_path.clone()),
        "GNANI_CERTIFICATE_PATH",
    )?;

    // Recording S3 configuration
    let recording_s3_bucket = get_optional!(
        "RECORDING_S3_BUCKET",
        yaml.recording.as_ref().and_then(|r| r.s3_bucket.clone())
    );

    let recording_s3_region = get_optional!(
        "RECORDING_S3_REGION",
        yaml.recording.as_ref().and_then(|r| r.s3_region.clone())
    );

    let recording_s3_endpoint = get_optional!(
        "RECORDING_S3_ENDPOINT",
        yaml.recording.as_ref().and_then(|r| r.s3_endpoint.clone())
    );

    // S3 creds are credentials — empty/placeholder ⇒ None (review wf_e8eaad72 #1,
    // consistency with the other credential fields + the from_env path).
    let recording_s3_access_key = get_credential!(
        "RECORDING_S3_ACCESS_KEY",
        yaml.recording
            .as_ref()
            .and_then(|r| r.s3_access_key.clone())
    );

    let recording_s3_secret_key = get_credential!(
        "RECORDING_S3_SECRET_KEY",
        yaml.recording
            .as_ref()
            .and_then(|r| r.s3_secret_key.clone())
    );

    let recording_s3_prefix = get_optional!(
        "RECORDING_S3_PREFIX",
        yaml.recording.as_ref().and_then(|r| r.s3_prefix.clone())
    );

    // Cache configuration
    let cache_path = yaml_or_env_path(
        yaml.cache.as_ref().and_then(|c| c.path.clone()),
        "CACHE_PATH",
    )?;

    let cache_ttl_seconds = yaml_or_env::<u64>(
        yaml.cache.as_ref().and_then(|c| c.ttl_seconds),
        "CACHE_TTL_SECONDS",
    )?
    .or(Some(30 * 24 * 60 * 60)); // Default to 30 days

    // Authentication configuration
    let auth_service_url = yaml_or_env_non_empty_string(
        yaml.auth.as_ref().and_then(|a| a.service_url.clone()),
        "AUTH_SERVICE_URL",
    )?;

    let auth_signing_key_path = yaml_or_env_path(
        yaml.auth.as_ref().and_then(|a| a.signing_key_path.clone()),
        "AUTH_SIGNING_KEY_PATH",
    )?;

    // API secret auth precedence:
    // 1) YAML auth.api_secrets (when non-empty)
    // 2) AUTH_API_SECRETS_JSON
    // 3) Legacy auth.api_secret or AUTH_API_SECRET (mapped to a single entry)
    let auth_api_secrets = if let Some(yaml_auth) = yaml.auth.as_ref()
        && !yaml_auth.api_secrets.is_empty()
    {
        yaml_auth
            .api_secrets
            .iter()
            .map(|entry| AuthApiSecret {
                id: entry.id.clone(),
                secret: entry.secret.clone(),
            })
            .collect()
    } else if let Ok(json) = env::var("AUTH_API_SECRETS_JSON") {
        parse_auth_api_secrets_json(&json)?
    } else {
        let legacy_secret = yaml
            .auth
            .as_ref()
            .and_then(|a| a.api_secret.clone())
            .or_else(|| env::var("AUTH_API_SECRET").ok());
        // AUTH_API_SECRET_ID provides the id for legacy single-secret configs.
        let legacy_id = env::var("AUTH_API_SECRET_ID").unwrap_or_else(|_| "default".to_string());

        if let Some(secret) = legacy_secret {
            vec![AuthApiSecret {
                id: legacy_id,
                secret,
            }]
        } else {
            Vec::new()
        }
    };

    let auth_timeout_seconds = yaml_or_env::<u64>(
        yaml.auth.as_ref().and_then(|a| a.timeout_seconds),
        "AUTH_TIMEOUT_SECONDS",
    )?
    .unwrap_or(5);

    let auth_required =
        yaml_or_env_bool(yaml.auth.as_ref().and_then(|a| a.required), "AUTH_REQUIRED")?
            .unwrap_or(false);

    // SIP configuration (merge YAML and ENV)
    let sip = merge_sip_config(yaml.sip.as_ref())?;

    // Security configuration
    let cors_allowed_origins = get_optional!(
        "CORS_ALLOWED_ORIGINS",
        yaml.security
            .as_ref()
            .and_then(|s| s.cors_allowed_origins.clone())
    );

    // Rate limiting configuration
    let rate_limit_requests_per_second = yaml_or_env::<u32>(
        yaml.security
            .as_ref()
            .and_then(|s| s.rate_limit_requests_per_second),
        "RATE_LIMIT_REQUESTS_PER_SECOND",
    )?
    .unwrap_or(60);

    let rate_limit_burst_size = yaml_or_env::<u32>(
        yaml.security.as_ref().and_then(|s| s.rate_limit_burst_size),
        "RATE_LIMIT_BURST_SIZE",
    )?
    .unwrap_or(10);

    // Connection limits
    let max_websocket_connections = yaml_or_env::<usize>(
        yaml.security
            .as_ref()
            .and_then(|s| s.max_websocket_connections),
        "MAX_WEBSOCKET_CONNECTIONS",
    )?;

    let max_connections_per_ip = yaml_or_env::<u32>(
        yaml.security
            .as_ref()
            .and_then(|s| s.max_connections_per_ip),
        "MAX_CONNECTIONS_PER_IP",
    )?
    .unwrap_or(100);

    // Timeout configuration
    let ws_processing_timeout_secs = yaml_or_env::<u64>(
        yaml.security
            .as_ref()
            .and_then(|s| s.ws_processing_timeout_secs),
        "WS_PROCESSING_TIMEOUT_SECS",
    )?
    .unwrap_or(10);

    let realtime_processing_timeout_secs = yaml_or_env::<u64>(
        yaml.security
            .as_ref()
            .and_then(|s| s.realtime_processing_timeout_secs),
        "REALTIME_PROCESSING_TIMEOUT_SECS",
    )?
    .unwrap_or(30);

    // SIP configuration limits
    let sip_max_participants = yaml_or_env::<u32>(
        yaml.sip.as_ref().and_then(|s| s.max_participants),
        "SIP_MAX_PARTICIPANTS",
    )?
    .unwrap_or(3);

    // SERVER-SIDE realtime upstream URL overrides (`<PROVIDER>_REALTIME_URL`).
    // Env-only (TRUSTED config; not a YAML/client surface) — same reader as
    // `ServerConfig::from_env`. SSRF-safe (see the field docs).
    let realtime_endpoint_overrides = super::env::read_realtime_endpoint_overrides();

    // Plugin configuration (backward compatible: enabled by default)
    let plugins_enabled = yaml_or_env_bool(
        yaml.plugins.as_ref().and_then(|p| p.enabled),
        "PLUGINS_ENABLED",
    )?
    .unwrap_or(true); // Enabled by default for backward compatibility

    let plugins_dir = yaml_or_env_path(
        yaml.plugins.as_ref().and_then(|p| p.plugin_dir.clone()),
        "PLUGINS_DIR",
    )?;

    let plugins_provider_config = yaml
        .plugins
        .as_ref()
        .map(|p| p.providers.clone())
        .unwrap_or_default();

    let plugins = PluginConfig {
        enabled: plugins_enabled,
        plugin_dir: plugins_dir,
        provider_config: plugins_provider_config,
    };

    // DAG timeout configuration (uses defaults if not specified)
    let dag_timeouts = DAGTimeoutsConfig {
        node_execution_secs: yaml_or_env::<u64>(
            yaml.dag_timeouts
                .as_ref()
                .and_then(|d| d.node_execution_secs),
            "DAG_NODE_EXECUTION_SECS",
        )?
        .unwrap_or(30),
        provider_operation_secs: yaml_or_env::<u64>(
            yaml.dag_timeouts
                .as_ref()
                .and_then(|d| d.provider_operation_secs),
            "DAG_PROVIDER_OPERATION_SECS",
        )?
        .unwrap_or(30),
        stt_endpoint_secs: yaml_or_env::<u64>(
            yaml.dag_timeouts.as_ref().and_then(|d| d.stt_endpoint_secs),
            "DAG_STT_ENDPOINT_SECS",
        )?
        .unwrap_or(60),
        tts_endpoint_secs: yaml_or_env::<u64>(
            yaml.dag_timeouts.as_ref().and_then(|d| d.tts_endpoint_secs),
            "DAG_TTS_ENDPOINT_SECS",
        )?
        .unwrap_or(60),
        llm_endpoint_secs: yaml_or_env::<u64>(
            yaml.dag_timeouts.as_ref().and_then(|d| d.llm_endpoint_secs),
            "DAG_LLM_ENDPOINT_SECS",
        )?
        .unwrap_or(120),
        websocket_operation_secs: yaml_or_env::<u64>(
            yaml.dag_timeouts
                .as_ref()
                .and_then(|d| d.websocket_operation_secs),
            "DAG_WEBSOCKET_OPERATION_SECS",
        )?
        .unwrap_or(30),
    };

    // P3: server-side alias registry. config.yaml is the ONLY definition source (no env
    // spelling for a whole {stt,tts,llm} bundle), so this is taken verbatim from the
    // parsed YAML `aliases:` section (empty when absent). A client can never define one.
    let aliases = crate::core::alias::AliasConfig {
        aliases: yaml.aliases.clone().unwrap_or_default(),
    };

    Ok(ServerConfig {
        host,
        port,
        tls,
        livekit_url,
        livekit_public_url,
        livekit_api_key,
        livekit_api_secret,
        deepgram_api_key,
        elevenlabs_api_key,
        google_credentials,
        azure_speech_subscription_key,
        azure_speech_region,
        cartesia_api_key,
        openai_api_key,
        azure_openai_api_key,
        azure_openai_endpoint,
        grok_api_key,
        inworld_api_key,
        gemini_api_key,
        ultravox_api_key,
        speechmatics_api_key,
        yandex_api_key,
        yandex_folder_id,
        assemblyai_api_key,
        hume_api_key,
        lmnt_api_key,
        groq_api_key,
        playht_api_key,
        playht_user_id,
        ibm_watson_api_key,
        ibm_watson_instance_id,
        ibm_watson_region,
        aws_access_key_id,
        aws_secret_access_key,
        aws_region,
        gnani_token,
        gnani_access_key,
        gnani_certificate_path,
        recording_s3_bucket,
        recording_s3_region,
        recording_s3_endpoint,
        recording_s3_access_key,
        recording_s3_secret_key,
        recording_s3_prefix,
        cache_path,
        cache_ttl_seconds,
        auth_service_url,
        auth_signing_key_path,
        auth_api_secrets,
        auth_timeout_seconds,
        auth_required,
        sip,
        cors_allowed_origins,
        rate_limit_requests_per_second,
        rate_limit_burst_size,
        max_websocket_connections,
        max_connections_per_ip,
        // Timeout configuration
        ws_processing_timeout_secs,
        realtime_processing_timeout_secs,
        // SIP configuration limits
        sip_max_participants,
        plugins,
        dag_timeouts,
        realtime_endpoint_overrides,
        aliases,
    })
}

/// Merge SIP configuration from YAML and environment variables
///
/// Priority: YAML > ENV
/// For hook secrets: per-hook secret > global hook_secret (YAML > ENV)
fn merge_sip_config(
    yaml_sip: Option<&super::yaml::SipYaml>,
) -> Result<Option<SipConfig>, Box<dyn std::error::Error>> {
    // Check if any SIP env vars are set
    let env_room_prefix = env::var("SIP_ROOM_PREFIX").ok();
    let env_allowed_addresses = env::var("SIP_ALLOWED_ADDRESSES").ok();
    let env_hooks_json = env::var("SIP_HOOKS_JSON").ok();
    let env_hook_secret = env::var("SIP_HOOK_SECRET").ok();
    let env_naming_prefix = env::var("SIP_NAMING_PREFIX").ok();

    let has_env_sip = env_room_prefix.is_some()
        || env_allowed_addresses.is_some()
        || env_hooks_json.is_some()
        || env_hook_secret.is_some()
        || env_naming_prefix.is_some();

    // If no YAML and no ENV, return None
    if yaml_sip.is_none() && !has_env_sip {
        return Ok(None);
    }

    // Merge room_prefix (YAML > ENV)
    let room_prefix = yaml_sip
        .and_then(|s| s.room_prefix.clone())
        .or(env_room_prefix)
        .ok_or("SIP room_prefix is required when SIP configuration is present")?;

    // Merge allowed_addresses (YAML > ENV)
    let allowed_addresses = if let Some(yaml_sip) = yaml_sip
        && !yaml_sip.allowed_addresses.is_empty()
    {
        // Use YAML addresses if present
        yaml_sip.allowed_addresses.clone()
    } else if let Some(addresses_str) = env_allowed_addresses {
        // Parse from ENV (comma-separated)
        addresses_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![]
    };

    // Merge hooks (YAML > ENV)
    let hooks = if let Some(yaml_sip) = yaml_sip
        && !yaml_sip.hooks.is_empty()
    {
        // Convert from YAML hooks if present
        yaml_sip
            .hooks
            .iter()
            .map(|h| SipHookConfig {
                host: h.host.clone(),
                url: h.url.clone(),
                secret: h.secret.clone(),
            })
            .collect()
    } else if let Some(hooks_json) = env_hooks_json {
        // Parse from ENV JSON
        parse_sip_hooks_json(&hooks_json)?
    } else {
        vec![]
    };

    // Merge hook_secret (YAML > ENV)
    let hook_secret = yaml_sip
        .and_then(|s| s.hook_secret.clone())
        .or(env_hook_secret);

    // Merge naming_prefix (YAML > ENV, defaults to "waav" in SipConfig::new)
    let naming_prefix = yaml_sip
        .and_then(|s| s.naming_prefix.clone())
        .or(env_naming_prefix);

    Ok(Some(SipConfig::new(
        room_prefix,
        allowed_addresses,
        hooks,
        hook_secret,
        naming_prefix,
    )))
}

/// Parse SIP hooks from JSON string
fn parse_sip_hooks_json(json_str: &str) -> Result<Vec<SipHookConfig>, Box<dyn std::error::Error>> {
    #[derive(serde::Deserialize)]
    struct HookJson {
        host: String,
        url: String,
        #[serde(default)]
        secret: Option<String>,
    }

    let hooks: Vec<HookJson> = serde_json::from_str(json_str)
        .map_err(|e| format!("Invalid SIP_HOOKS_JSON format: {e}"))?;

    Ok(hooks
        .into_iter()
        .map(|h| SipHookConfig {
            host: h.host,
            url: h.url,
            secret: h.secret,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::yaml::AuthApiSecretYaml;
    use super::super::yaml::SipHookYaml;
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    // Helper to clean up environment variables
    fn cleanup_env_vars() {
        unsafe {
            env::remove_var("HOST");
            env::remove_var("PORT");
            env::remove_var("TLS_ENABLED");
            env::remove_var("TLS_CERT_PATH");
            env::remove_var("TLS_KEY_PATH");
            env::remove_var("LIVEKIT_URL");
            env::remove_var("LIVEKIT_PUBLIC_URL");
            env::remove_var("DEEPGRAM_API_KEY");
            env::remove_var("ELEVENLABS_API_KEY");
            env::remove_var("CACHE_PATH");
            env::remove_var("GNANI_CERTIFICATE_PATH");
            env::remove_var("CACHE_TTL_SECONDS");
            env::remove_var("AUTH_REQUIRED");
            env::remove_var("AUTH_SERVICE_URL");
            env::remove_var("AUTH_SIGNING_KEY_PATH");
            env::remove_var("AUTH_API_SECRETS_JSON");
            env::remove_var("AUTH_API_SECRET");
            env::remove_var("AUTH_API_SECRET_ID");
            env::remove_var("AUTH_TIMEOUT_SECONDS");
            env::remove_var("SIP_ROOM_PREFIX");
            env::remove_var("SIP_ALLOWED_ADDRESSES");
            env::remove_var("SIP_HOOKS_JSON");
            env::remove_var("SIP_HOOK_SECRET");
            env::remove_var("RECORDING_S3_PREFIX");
            env::remove_var("RATE_LIMIT_REQUESTS_PER_SECOND");
            env::remove_var("RATE_LIMIT_BURST_SIZE");
            env::remove_var("MAX_WEBSOCKET_CONNECTIONS");
            env::remove_var("MAX_CONNECTIONS_PER_IP");
            env::remove_var("WS_PROCESSING_TIMEOUT_SECS");
            env::remove_var("REALTIME_PROCESSING_TIMEOUT_SECS");
            env::remove_var("SIP_MAX_PARTICIPANTS");
            env::remove_var("PLUGINS_ENABLED");
            env::remove_var("PLUGINS_DIR");
            env::remove_var("DAG_NODE_EXECUTION_SECS");
            env::remove_var("DAG_PROVIDER_OPERATION_SECS");
            env::remove_var("DAG_STT_ENDPOINT_SECS");
            env::remove_var("DAG_TTS_ENDPOINT_SECS");
            env::remove_var("DAG_LLM_ENDPOINT_SECS");
            env::remove_var("DAG_WEBSOCKET_OPERATION_SECS");
        }
    }

    /// Empty-key consistency (review: hume empty-key 429). An EMPTY-string
    /// credential — whether it arrives from YAML or from an exported-but-empty
    /// env var — must resolve to `None` so the realtime handler's missing-key
    /// guard returns a clean "API key not configured" error instead of dialing
    /// the provider with `Some("")` (the YAML branch already handled empty via
    /// `is_placeholder_credential`; the env fallback used to leak `Some("")`).
    #[test]
    #[serial]
    fn empty_credential_from_yaml_or_env_resolves_to_none() {
        cleanup_env_vars();
        unsafe {
            env::remove_var("HUME_API_KEY");
        }

        // 1) YAML empty string, no env ⇒ None.
        let yaml = YamlConfig {
            providers: Some(super::super::yaml::ProvidersYaml {
                hume_api_key: Some(String::new()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            merge_config(Some(yaml)).unwrap().hume_api_key,
            None,
            "empty YAML credential must merge to None"
        );

        // 2) Exported-but-empty env var, no YAML ⇒ None (the bug: was Some("")).
        unsafe {
            env::set_var("HUME_API_KEY", "");
        }
        assert_eq!(
            merge_config(None).unwrap().hume_api_key,
            None,
            "empty env credential must merge to None"
        );

        // 3) Whitespace-only env var ⇒ None.
        unsafe {
            env::set_var("HUME_API_KEY", "   ");
        }
        assert_eq!(
            merge_config(None).unwrap().hume_api_key,
            None,
            "whitespace-only env credential must merge to None"
        );

        // 4) A REAL key still flows through untouched (no false-positive nulling).
        unsafe {
            env::set_var("HUME_API_KEY", "hk-real-secret");
        }
        assert_eq!(
            merge_config(None).unwrap().hume_api_key,
            Some("hk-real-secret".to_string()),
            "a real env credential must survive the empty/placeholder filter"
        );

        unsafe {
            env::remove_var("HUME_API_KEY");
        }
        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn empty_optional_path_values_are_unset_and_blank_yaml_falls_back_to_env() {
        cleanup_env_vars();
        unsafe {
            env::set_var("CACHE_PATH", "");
            env::set_var("GNANI_CERTIFICATE_PATH", "   ");
            env::set_var("AUTH_SIGNING_KEY_PATH", "");
            env::set_var("PLUGINS_DIR", "   ");
        }

        let config = merge_config(None).unwrap();
        assert_eq!(config.cache_path, None, "empty CACHE_PATH must be unset");
        assert_eq!(
            config.gnani_certificate_path, None,
            "whitespace GNANI_CERTIFICATE_PATH must be unset"
        );
        assert_eq!(
            config.auth_signing_key_path, None,
            "empty AUTH_SIGNING_KEY_PATH must be unset"
        );
        assert_eq!(
            config.plugins.plugin_dir, None,
            "whitespace PLUGINS_DIR must be unset"
        );

        let temp_dir = TempDir::new().unwrap();
        let env_cache = temp_dir.path().join("cache");
        let env_plugin_dir = temp_dir.path().join("plugins");
        unsafe {
            env::set_var("CACHE_PATH", env_cache.to_str().unwrap());
            env::set_var("PLUGINS_DIR", env_plugin_dir.to_str().unwrap());
        }
        let yaml = YamlConfig {
            cache: Some(super::super::yaml::CacheYaml {
                path: Some("   ".to_string()),
                ttl_seconds: None,
            }),
            plugins: Some(super::super::yaml::PluginsYaml {
                plugin_dir: Some(String::new()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let config = merge_config(Some(yaml)).unwrap();
        assert_eq!(
            config.cache_path,
            Some(env_cache),
            "blank YAML cache.path must not mask CACHE_PATH"
        );
        assert_eq!(
            config.plugins.plugin_dir,
            Some(env_plugin_dir),
            "blank YAML plugins.plugin_dir must not mask PLUGINS_DIR"
        );

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn empty_auth_service_url_values_are_unset_and_blank_yaml_falls_back_to_env() {
        cleanup_env_vars();
        unsafe {
            env::set_var("AUTH_SERVICE_URL", "");
        }

        let config = merge_config(None).unwrap();
        assert_eq!(
            config.auth_service_url, None,
            "empty AUTH_SERVICE_URL must be unset"
        );

        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("key.pem");
        fs::write(&key_path, "fake key").unwrap();
        unsafe {
            env::set_var("AUTH_SERVICE_URL", "https://auth.env.com");
            env::set_var("AUTH_SIGNING_KEY_PATH", key_path.to_str().unwrap());
        }
        let yaml = YamlConfig {
            auth: Some(super::super::yaml::AuthYaml {
                service_url: Some("   ".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let config = merge_config(Some(yaml)).unwrap();
        assert_eq!(
            config.auth_service_url,
            Some("https://auth.env.com".to_string()),
            "blank YAML auth.service_url must not mask AUTH_SERVICE_URL"
        );

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_yaml_only() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            server: Some(super::super::yaml::ServerYaml {
                host: Some("127.0.0.1".to_string()),
                port: Some(8080),
                tls: None,
            }),
            cache: Some(super::super::yaml::CacheYaml {
                path: Some("/tmp/cache".to_string()),
                ttl_seconds: Some(3600),
            }),
            ..Default::default()
        };

        let config = merge_config(Some(yaml)).unwrap();

        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.cache_path, Some(PathBuf::from("/tmp/cache")));
        assert_eq!(config.cache_ttl_seconds, Some(3600));

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_yaml_overrides_env() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            server: Some(super::super::yaml::ServerYaml {
                host: Some("127.0.0.1".to_string()),
                port: Some(8080),
                tls: None,
            }),
            ..Default::default()
        };

        unsafe {
            env::set_var("HOST", "0.0.0.0");
            env::set_var("PORT", "9000");
        }

        let config = merge_config(Some(yaml)).unwrap();

        // YAML overrides ENV
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_defaults_when_no_yaml_or_env() {
        cleanup_env_vars();

        let config = merge_config(None).unwrap();

        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 3001);
        assert_eq!(config.livekit_url, "ws://localhost:7880");
        assert!(!config.auth_required);

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_invalid_bool_env_fails_when_yaml_absent() {
        cleanup_env_vars();

        unsafe {
            env::set_var("PLUGINS_ENABLED", "maybe");
        }

        let result = merge_config(None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PLUGINS_ENABLED"));

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_invalid_numeric_env_fails_when_yaml_absent() {
        cleanup_env_vars();

        unsafe {
            env::set_var("RATE_LIMIT_REQUESTS_PER_SECOND", "many");
        }

        let result = merge_config(None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("RATE_LIMIT_REQUESTS_PER_SECOND")
        );

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_yaml_value_skips_malformed_env_for_same_field() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            security: Some(super::super::yaml::SecurityYaml {
                rate_limit_requests_per_second: Some(12),
                ..Default::default()
            }),
            ..Default::default()
        };

        unsafe {
            env::set_var("RATE_LIMIT_REQUESTS_PER_SECOND", "many");
        }

        let config = merge_config(Some(yaml)).unwrap();
        assert_eq!(config.rate_limit_requests_per_second, 12);

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_partial_yaml() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            server: Some(super::super::yaml::ServerYaml {
                port: Some(8080),
                ..Default::default()
            }),
            ..Default::default()
        };

        let config = merge_config(Some(yaml)).unwrap();

        assert_eq!(config.host, "0.0.0.0"); // default
        assert_eq!(config.port, 8080); // from yaml

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_auth_config() {
        cleanup_env_vars();
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("key.pem");
        fs::write(&key_path, "fake key").unwrap();

        let yaml = YamlConfig {
            auth: Some(super::super::yaml::AuthYaml {
                required: Some(true),
                service_url: Some("https://auth.yaml.com".to_string()),
                signing_key_path: Some(key_path.to_string_lossy().to_string()),
                timeout_seconds: Some(10),
                ..Default::default()
            }),
            ..Default::default()
        };

        unsafe {
            env::set_var("AUTH_SERVICE_URL", "https://auth.env.com");
        }

        let config = merge_config(Some(yaml)).unwrap();

        assert!(config.auth_required);
        assert_eq!(
            config.auth_service_url,
            Some("https://auth.yaml.com".to_string())
        ); // YAML overrides ENV
        assert_eq!(config.auth_signing_key_path, Some(key_path)); // from YAML
        assert_eq!(config.auth_timeout_seconds, 10); // from YAML

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_auth_api_secrets_yaml_over_env() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            auth: Some(super::super::yaml::AuthYaml {
                api_secrets: vec![
                    AuthApiSecretYaml {
                        id: "yaml-a".to_string(),
                        secret: "secret-a".to_string(),
                    },
                    AuthApiSecretYaml {
                        id: "yaml-b".to_string(),
                        secret: "secret-b".to_string(),
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        };

        unsafe {
            env::set_var(
                "AUTH_API_SECRETS_JSON",
                r#"[{"id":"env-a","secret":"env-secret"}]"#,
            );
            env::set_var("AUTH_API_SECRET", "legacy-secret");
        }

        let config = merge_config(Some(yaml)).unwrap();

        assert_eq!(config.auth_api_secrets.len(), 2);
        assert_eq!(config.auth_api_secrets[0].id, "yaml-a");
        assert_eq!(config.auth_api_secrets[1].id, "yaml-b");

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_auth_api_secrets_env_over_legacy() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            auth: Some(super::super::yaml::AuthYaml {
                api_secret: Some("legacy-secret".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        unsafe {
            env::set_var(
                "AUTH_API_SECRETS_JSON",
                r#"[{"id":"env-a","secret":"token-a"}]"#,
            );
        }

        let config = merge_config(Some(yaml)).unwrap();

        assert_eq!(config.auth_api_secrets.len(), 1);
        assert_eq!(config.auth_api_secrets[0].id, "env-a");
        assert_eq!(config.auth_api_secrets[0].secret, "token-a");

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_auth_api_secrets_empty_yaml_uses_legacy() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            auth: Some(super::super::yaml::AuthYaml {
                api_secrets: Vec::new(),
                api_secret: Some("legacy-secret".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let config = merge_config(Some(yaml)).unwrap();

        assert_eq!(config.auth_api_secrets.len(), 1);
        assert_eq!(config.auth_api_secrets[0].id, "default");
        assert_eq!(config.auth_api_secrets[0].secret, "legacy-secret");

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_recording_prefix_yaml_overrides_env() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            recording: Some(super::super::yaml::RecordingYaml {
                s3_prefix: Some("yaml-prefix".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        unsafe {
            env::set_var("RECORDING_S3_PREFIX", "env-prefix");
        }

        let config = merge_config(Some(yaml)).unwrap();

        assert_eq!(config.recording_s3_prefix, Some("yaml-prefix".to_string()));

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_recording_prefix_env_only() {
        cleanup_env_vars();

        unsafe {
            env::set_var("RECORDING_S3_PREFIX", "env-only");
        }

        let config = merge_config(None).unwrap();

        assert_eq!(config.recording_s3_prefix, Some("env-only".to_string()));

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_recording_prefix_none_when_unset() {
        cleanup_env_vars();

        let config = merge_config(None).unwrap();

        assert_eq!(config.recording_s3_prefix, None);

        cleanup_env_vars();
    }

    // SIP configuration merge tests

    #[test]
    #[serial]
    fn test_merge_sip_yaml_only() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            sip: Some(super::super::yaml::SipYaml {
                room_prefix: Some("sip-".to_string()),
                allowed_addresses: vec!["192.168.1.0/24".to_string()],
                hooks: vec![SipHookYaml {
                    host: "example.com".to_string(),
                    url: "https://webhook.example.com/events".to_string(),
                    secret: None,
                }],
                hook_secret: Some("global-secret".to_string()),
                naming_prefix: None,
                max_participants: None,
            }),
            ..Default::default()
        };

        let config = merge_config(Some(yaml)).unwrap();
        let sip = config.sip.clone().expect("SIP config should be present");

        assert_eq!(sip.room_prefix, "sip-");
        assert_eq!(sip.allowed_addresses.len(), 1);
        assert_eq!(sip.allowed_addresses[0], "192.168.1.0/24");
        assert_eq!(sip.hooks.len(), 1);
        assert_eq!(sip.hooks[0].host, "example.com");
        assert_eq!(sip.hook_secret, Some("global-secret".to_string()));
        assert_eq!(sip.naming_prefix, "waav"); // default

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_sip_yaml_overrides_env() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            sip: Some(super::super::yaml::SipYaml {
                room_prefix: Some("yaml-prefix-".to_string()),
                allowed_addresses: vec!["192.168.1.0/24".to_string()],
                hooks: vec![],
                hook_secret: Some("yaml-secret".to_string()),
                naming_prefix: None,
                max_participants: None,
            }),
            ..Default::default()
        };

        unsafe {
            env::set_var("SIP_ROOM_PREFIX", "env-prefix-");
            env::set_var("SIP_ALLOWED_ADDRESSES", "10.0.0.1, 10.0.0.2");
            env::set_var("SIP_HOOK_SECRET", "env-secret");
        }

        let config = merge_config(Some(yaml)).unwrap();
        let sip = config.sip.clone().expect("SIP config should be present");

        assert_eq!(sip.room_prefix, "yaml-prefix-"); // YAML overrides ENV
        assert_eq!(sip.allowed_addresses.len(), 1); // YAML overrides ENV
        assert_eq!(sip.allowed_addresses[0], "192.168.1.0/24");
        assert_eq!(sip.hook_secret, Some("yaml-secret".to_string())); // YAML overrides ENV

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_sip_env_only() {
        cleanup_env_vars();

        unsafe {
            env::set_var("SIP_ROOM_PREFIX", "sip-");
            env::set_var("SIP_ALLOWED_ADDRESSES", "192.168.1.0/24");
            env::set_var(
                "SIP_HOOKS_JSON",
                r#"[{"host": "example.com", "url": "https://webhook.example.com/events"}]"#,
            );
        }

        let config = merge_config(None).unwrap();
        let sip = config.sip.clone().expect("SIP config should be present");

        assert_eq!(sip.room_prefix, "sip-");
        assert_eq!(sip.allowed_addresses.len(), 1);
        assert_eq!(sip.hooks.len(), 1);

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_sip_no_config() {
        cleanup_env_vars();

        let config = merge_config(None).unwrap();
        assert!(config.sip.is_none());

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_sip_partial_yaml_with_env() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            sip: Some(super::super::yaml::SipYaml {
                room_prefix: Some("sip-".to_string()),
                allowed_addresses: vec![],
                hooks: vec![],
                hook_secret: None,
                naming_prefix: None,
                max_participants: None,
            }),
            ..Default::default()
        };

        unsafe {
            env::set_var("SIP_ALLOWED_ADDRESSES", "10.0.0.1");
            env::set_var("SIP_HOOK_SECRET", "env-secret");
        }

        let config = merge_config(Some(yaml)).unwrap();
        let sip = config.sip.clone().expect("SIP config should be present");

        assert_eq!(sip.room_prefix, "sip-"); // from YAML
        // YAML has empty array, so ENV is used as fallback
        assert_eq!(sip.allowed_addresses.len(), 1); // from ENV (YAML is empty)
        assert_eq!(sip.allowed_addresses[0], "10.0.0.1");
        assert_eq!(sip.hook_secret, Some("env-secret".to_string())); // from ENV (YAML is None)

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_merge_sip_per_hook_secret_precedence() {
        cleanup_env_vars();

        let yaml = YamlConfig {
            sip: Some(super::super::yaml::SipYaml {
                room_prefix: Some("sip-".to_string()),
                allowed_addresses: vec!["192.168.1.0/24".to_string()],
                hooks: vec![
                    SipHookYaml {
                        host: "example.com".to_string(),
                        url: "https://webhook.example.com/events".to_string(),
                        secret: None, // uses global
                    },
                    SipHookYaml {
                        host: "override.com".to_string(),
                        url: "https://webhook.override.com/events".to_string(),
                        secret: Some("per-hook-override".to_string()), // overrides global
                    },
                ],
                hook_secret: Some("global-secret".to_string()),
                naming_prefix: Some("custom".to_string()), // test custom naming_prefix
                max_participants: None,
            }),
            ..Default::default()
        };

        let config = merge_config(Some(yaml)).unwrap();
        let sip = config.sip.clone().expect("SIP config should be present");

        assert_eq!(sip.hook_secret, Some("global-secret".to_string()));
        assert_eq!(sip.hooks.len(), 2);
        assert_eq!(sip.hooks[0].secret, None); // will use global
        assert_eq!(sip.hooks[1].secret, Some("per-hook-override".to_string())); // overrides global
        assert_eq!(sip.naming_prefix, "custom"); // custom naming prefix

        cleanup_env_vars();
    }
}

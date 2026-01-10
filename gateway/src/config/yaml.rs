use serde::Deserialize;
use std::path::PathBuf;

/// Complete YAML configuration structure
///
/// This structure represents the full configuration that can be loaded from a YAML file.
/// All fields are optional to allow partial configuration. Environment variables can
/// override any values specified here.
///
/// # Example YAML structure
/// ```yaml
/// server:
///   host: "0.0.0.0"
///   port: 3001
///
/// livekit:
///   url: "ws://localhost:7880"
///   public_url: "http://localhost:7880"
///   api_key: "your-api-key"
///   api_secret: "your-api-secret"
///
/// providers:
///   deepgram_api_key: "your-deepgram-key"
///   elevenlabs_api_key: "your-elevenlabs-key"
///
/// recording:
///   s3_bucket: "my-bucket"
///   s3_region: "us-west-2"
///   s3_prefix: "recordings/production"
///   s3_endpoint: "https://s3.amazonaws.com"
///   s3_access_key: "access-key"
///   s3_secret_key: "secret-key"
///
/// cache:
///   path: "/var/cache/waav-gateway"
///   ttl_seconds: 2592000
///
/// auth:
///   required: true
///   service_url: "https://auth.example.com"
///   signing_key_path: "/path/to/key.pem"
///   api_secrets:
///     - id: "client-a"
///       secret: "your-api-secret"
///   timeout_seconds: 5
///
/// sip:
///   room_prefix: "sip-"
///   allowed_addresses:
///     - "192.168.1.0/24"
///     - "10.0.0.1"
///   hook_secret: "global-signing-secret"
///   hooks:
///     - host: "example.com"
///       url: "https://webhook.example.com/events"
///     - host: "other.com"
///       url: "https://webhook.other.com/events"
///       secret: "per-hook-override-secret"
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct YamlConfig {
    pub server: Option<ServerYaml>,
    pub livekit: Option<LiveKitYaml>,
    pub providers: Option<ProvidersYaml>,
    pub recording: Option<RecordingYaml>,
    pub cache: Option<CacheYaml>,
    pub auth: Option<AuthYaml>,
    pub sip: Option<SipYaml>,
    pub security: Option<SecurityYaml>,
    pub plugins: Option<PluginsYaml>,
    pub vad: Option<VadYaml>,
}

/// Server configuration from YAML
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ServerYaml {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub tls: Option<TlsYaml>,
}

/// TLS configuration from YAML
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct TlsYaml {
    pub enabled: Option<bool>,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
}

/// LiveKit configuration from YAML
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct LiveKitYaml {
    pub url: Option<String>,
    pub public_url: Option<String>,
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
}

/// Provider API keys from YAML
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ProvidersYaml {
    pub deepgram_api_key: Option<String>,
    pub elevenlabs_api_key: Option<String>,
    /// Google Cloud credentials - can be:
    /// - Path to service account JSON file
    /// - Inline JSON content (for secrets management)
    /// - Empty string to use Application Default Credentials
    pub google_credentials: Option<String>,
    /// Azure Speech Services subscription key from Azure Portal
    /// (Azure Portal → Speech resource → Keys and Endpoint → Key 1 or Key 2)
    pub azure_speech_subscription_key: Option<String>,
    /// Azure region where the Speech resource is deployed (e.g., "eastus", "westus2")
    /// The subscription key is tied to this specific region
    pub azure_speech_region: Option<String>,
    /// Cartesia API key for STT (ink-whisper model)
    pub cartesia_api_key: Option<String>,
    /// OpenAI API key for STT (Whisper), TTS, and Realtime API
    pub openai_api_key: Option<String>,
    /// AssemblyAI API key for streaming STT
    pub assemblyai_api_key: Option<String>,
    /// Hume AI API key for TTS (Octave) and EVI
    pub hume_api_key: Option<String>,
    /// LMNT API key for ultra-low latency TTS and voice cloning
    pub lmnt_api_key: Option<String>,
    /// Groq API key for ultra-fast Whisper STT
    pub groq_api_key: Option<String>,
    /// Play.ht API key for low-latency TTS with voice cloning
    pub playht_api_key: Option<String>,
    /// Play.ht user ID (required alongside playht_api_key)
    pub playht_user_id: Option<String>,
    /// IBM Watson API key for STT/TTS
    pub ibm_watson_api_key: Option<String>,
    /// IBM Watson service instance ID
    pub ibm_watson_instance_id: Option<String>,
    /// IBM Watson region (e.g., "us-south", "eu-gb")
    pub ibm_watson_region: Option<String>,
    /// AWS access key ID (for Transcribe/Polly)
    pub aws_access_key_id: Option<String>,
    /// AWS secret access key (for Transcribe/Polly)
    pub aws_secret_access_key: Option<String>,
    /// AWS region (e.g., "us-east-1", "eu-west-1")
    pub aws_region: Option<String>,
    /// Gnani.ai authentication token (required for Gnani STT/TTS)
    pub gnani_token: Option<String>,
    /// Gnani.ai access key (required for Gnani STT/TTS)
    pub gnani_access_key: Option<String>,
    /// Path to Gnani SSL certificate file (for mTLS authentication)
    pub gnani_certificate_path: Option<String>,
}

/// Recording S3 configuration from YAML
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RecordingYaml {
    pub s3_bucket: Option<String>,
    pub s3_region: Option<String>,
    pub s3_endpoint: Option<String>,
    pub s3_access_key: Option<String>,
    pub s3_secret_key: Option<String>,
    pub s3_prefix: Option<String>,
}

/// Cache configuration from YAML
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct CacheYaml {
    pub path: Option<String>,
    pub ttl_seconds: Option<u64>,
}

/// Authentication configuration from YAML
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AuthYaml {
    pub required: Option<bool>,
    pub service_url: Option<String>,
    pub signing_key_path: Option<String>,
    /// Preferred multi-secret form. If non-empty, it takes precedence over api_secret.
    #[serde(default)]
    pub api_secrets: Vec<AuthApiSecretYaml>,
    /// Legacy single-secret alias. Ignored when api_secrets is non-empty.
    pub api_secret: Option<String>,
    pub timeout_seconds: Option<u64>,
}

/// API secret authentication entry in YAML
#[derive(Debug, Clone, Deserialize)]
pub struct AuthApiSecretYaml {
    pub id: String,
    pub secret: String,
}

/// SIP configuration from YAML
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SipYaml {
    pub room_prefix: Option<String>,
    #[serde(default)]
    pub allowed_addresses: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<SipHookYaml>,
    pub hook_secret: Option<String>,
    /// Prefix for SIP trunk and dispatch naming (defaults to "waav")
    pub naming_prefix: Option<String>,
}

/// SIP webhook hook configuration from YAML
#[derive(Debug, Clone, Deserialize)]
pub struct SipHookYaml {
    pub host: String,
    pub url: String,
    #[serde(default)]
    pub secret: Option<String>,
}

/// Security configuration from YAML
///
/// # Example YAML structure
/// ```yaml
/// security:
///   cors_allowed_origins: "https://example.com,https://app.example.com"
///   rate_limit_requests_per_second: 60
///   rate_limit_burst_size: 10
///   max_websocket_connections: 1000
///   max_connections_per_ip: 100
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SecurityYaml {
    /// CORS allowed origins (comma-separated list or "*" for all)
    pub cors_allowed_origins: Option<String>,
    /// Maximum requests per second per IP address
    pub rate_limit_requests_per_second: Option<u32>,
    /// Maximum burst size for rate limiting
    pub rate_limit_burst_size: Option<u32>,
    /// Maximum concurrent WebSocket connections
    pub max_websocket_connections: Option<usize>,
    /// Maximum connections per IP address
    pub max_connections_per_ip: Option<u32>,
}

/// Plugin configuration from YAML
///
/// # Example YAML structure
/// ```yaml
/// plugins:
///   enabled: true
///   plugin_dir: "/opt/waav/plugins"
///   providers:
///     deepgram:
///       custom_endpoint: "https://custom.deepgram.com"
///     my_custom_stt:
///       api_key: "custom-key"
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PluginsYaml {
    /// Whether the plugin system is enabled (default: true for backward compatibility)
    pub enabled: Option<bool>,
    /// Directory to load external plugins from (optional)
    pub plugin_dir: Option<String>,
    /// Provider-specific configuration (keyed by provider name)
    #[serde(default)]
    pub providers: std::collections::HashMap<String, serde_json::Value>,
}

/// Voice Activity Detection configuration from YAML
///
/// # Example YAML structure
/// ```yaml
/// vad:
///   enabled: true
///   backend: "silero"
///   threshold: 0.5
///   min_speech_duration_ms: 250
///   min_silence_duration_ms: 300
///   pre_speech_padding_ms: 100
///   post_speech_padding_ms: 100
///   sample_rate: 16000
///   emit_probability_events: false
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct VadYaml {
    /// Enable/disable VAD processing
    pub enabled: Option<bool>,
    /// VAD backend: "silero", "webrtc", or "energy"
    pub backend: Option<String>,
    /// Speech probability threshold (0.0 - 1.0)
    pub threshold: Option<f32>,
    /// Minimum speech duration before triggering speech_start (ms)
    pub min_speech_duration_ms: Option<u32>,
    /// Minimum silence duration before triggering speech_end (ms)
    pub min_silence_duration_ms: Option<u32>,
    /// Pre-speech audio padding (ms)
    pub pre_speech_padding_ms: Option<u32>,
    /// Post-speech audio padding (ms)
    pub post_speech_padding_ms: Option<u32>,
    /// Sample rate for audio processing (Hz)
    pub sample_rate: Option<u32>,
    /// Frame size in samples
    pub frame_size: Option<usize>,
    /// Path to the ONNX model file
    pub model_path: Option<String>,
    /// Number of threads for ONNX inference
    pub num_threads: Option<usize>,
    /// Emit speech probability events
    pub emit_probability_events: Option<bool>,
}

impl YamlConfig {
    /// Load configuration from a YAML file
    ///
    /// # Arguments
    /// * `path` - Path to the YAML configuration file
    ///
    /// # Returns
    /// * `Result<YamlConfig, Box<dyn std::error::Error>>` - The loaded configuration or an error
    ///
    /// # Errors
    /// Returns an error if:
    /// - The file cannot be read
    /// - The YAML is malformed
    /// - Required fields have invalid types
    pub fn from_file(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))?;

        let config: YamlConfig = serde_yaml::from_str(&contents)
            .map_err(|e| format!("Failed to parse YAML config: {e}"))?;

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_yaml_config_full() {
        let yaml = r#"
server:
  host: "127.0.0.1"
  port: 8080

livekit:
  url: "ws://livekit.example.com"
  public_url: "https://livekit.example.com"
  api_key: "test-key"
  api_secret: "test-secret"

providers:
  deepgram_api_key: "dg-key"
  elevenlabs_api_key: "el-key"

recording:
  s3_bucket: "my-recordings"
  s3_region: "us-east-1"
  s3_prefix: "test-prefix"
  s3_endpoint: "https://s3.amazonaws.com"
  s3_access_key: "access"
  s3_secret_key: "secret"

cache:
  path: "/tmp/cache"
  ttl_seconds: 3600

auth:
  required: true
  service_url: "https://auth.example.com"
  signing_key_path: "/path/to/key.pem"
  api_secrets:
    - id: "client-a"
      secret: "auth-secret"
  timeout_seconds: 10
"#;

        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(
            config.server.as_ref().unwrap().host,
            Some("127.0.0.1".to_string())
        );
        assert_eq!(config.server.as_ref().unwrap().port, Some(8080));
        assert_eq!(
            config.livekit.as_ref().unwrap().url,
            Some("ws://livekit.example.com".to_string())
        );
        assert_eq!(
            config.providers.as_ref().unwrap().deepgram_api_key,
            Some("dg-key".to_string())
        );
        assert_eq!(
            config.recording.as_ref().unwrap().s3_bucket,
            Some("my-recordings".to_string())
        );
        assert_eq!(
            config.recording.as_ref().unwrap().s3_prefix,
            Some("test-prefix".to_string())
        );
        assert_eq!(
            config.cache.as_ref().unwrap().path,
            Some("/tmp/cache".to_string())
        );
        assert_eq!(config.auth.as_ref().unwrap().required, Some(true));
        let auth = config.auth.as_ref().unwrap();
        assert_eq!(auth.api_secrets.len(), 1);
        assert_eq!(auth.api_secrets[0].id, "client-a");
        assert_eq!(auth.api_secrets[0].secret, "auth-secret");
    }

    #[test]
    fn test_yaml_config_auth_multiple_api_secrets() {
        let yaml = r#"
auth:
  api_secrets:
    - id: "client-a"
      secret: "secret-a"
    - id: "client-b"
      secret: "secret-b"
"#;

        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        let auth = config.auth.as_ref().unwrap();
        assert_eq!(auth.api_secrets.len(), 2);
        assert_eq!(auth.api_secrets[0].id, "client-a");
        assert_eq!(auth.api_secrets[1].id, "client-b");
    }

    #[test]
    fn test_yaml_config_auth_legacy_api_secret() {
        let yaml = r#"
auth:
  api_secret: "legacy-secret"
"#;

        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        let auth = config.auth.as_ref().unwrap();
        assert!(auth.api_secrets.is_empty());
        assert_eq!(auth.api_secret.as_deref(), Some("legacy-secret"));
    }

    #[test]
    fn test_yaml_config_partial() {
        let yaml = r#"
server:
  port: 9000

cache:
  ttl_seconds: 7200
"#;

        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        assert!(config.server.as_ref().unwrap().host.is_none());
        assert_eq!(config.server.as_ref().unwrap().port, Some(9000));
        assert!(config.livekit.is_none());
        assert_eq!(config.cache.as_ref().unwrap().ttl_seconds, Some(7200));
        assert!(config.cache.as_ref().unwrap().path.is_none());
    }

    #[test]
    fn test_yaml_config_empty() {
        let yaml = "";

        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        assert!(config.server.is_none());
        assert!(config.livekit.is_none());
        assert!(config.providers.is_none());
        assert!(config.recording.is_none());
        assert!(config.cache.is_none());
        assert!(config.auth.is_none());
    }

    #[test]
    fn test_yaml_config_recording_prefix_only() {
        let yaml = r#"
recording:
  s3_prefix: "recordings/production"
"#;

        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        let recording = config.recording.expect("recording should be present");
        assert_eq!(
            recording.s3_prefix,
            Some("recordings/production".to_string())
        );
        assert!(recording.s3_bucket.is_none());
        assert!(recording.s3_region.is_none());
        assert!(recording.s3_endpoint.is_none());
        assert!(recording.s3_access_key.is_none());
        assert!(recording.s3_secret_key.is_none());
    }

    #[test]
    fn test_from_file() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");

        let yaml_content = r#"
server:
  host: "localhost"
  port: 3000
"#;

        fs::write(&config_path, yaml_content).unwrap();

        let config = YamlConfig::from_file(&config_path).unwrap();

        assert_eq!(
            config.server.as_ref().unwrap().host,
            Some("localhost".to_string())
        );
        assert_eq!(config.server.as_ref().unwrap().port, Some(3000));
    }

    #[test]
    fn test_from_file_not_found() {
        let path = PathBuf::from("/nonexistent/config.yaml");
        let result = YamlConfig::from_file(&path);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to read config file")
        );
    }

    #[test]
    fn test_from_file_invalid_yaml() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("invalid.yaml");

        fs::write(&config_path, "invalid: yaml: content:").unwrap();

        let result = YamlConfig::from_file(&config_path);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to parse YAML")
        );
    }

    #[test]
    fn test_yaml_config_with_sip() {
        let yaml = r#"
sip:
  room_prefix: "sip-"
  allowed_addresses:
    - "192.168.1.0/24"
    - "10.0.0.1"
  hook_secret: "global-secret"
  hooks:
    - host: "example.com"
      url: "https://webhook.example.com/events"
    - host: "another.com"
      url: "https://webhook2.example.com/events"
      secret: "per-hook-secret"
"#;

        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        let sip = config.sip.as_ref().unwrap();
        assert_eq!(sip.room_prefix, Some("sip-".to_string()));
        assert_eq!(sip.allowed_addresses.len(), 2);
        assert_eq!(sip.allowed_addresses[0], "192.168.1.0/24");
        assert_eq!(sip.allowed_addresses[1], "10.0.0.1");
        assert_eq!(sip.hook_secret, Some("global-secret".to_string()));
        assert_eq!(sip.hooks.len(), 2);
        assert_eq!(sip.hooks[0].host, "example.com");
        assert_eq!(sip.hooks[0].url, "https://webhook.example.com/events");
        assert_eq!(sip.hooks[0].secret, None);
        assert_eq!(sip.hooks[1].host, "another.com");
        assert_eq!(sip.hooks[1].url, "https://webhook2.example.com/events");
        assert_eq!(sip.hooks[1].secret, Some("per-hook-secret".to_string()));
    }

    #[test]
    fn test_yaml_config_sip_empty_arrays() {
        let yaml = r#"
sip:
  room_prefix: "sip-"
  allowed_addresses: []
  hooks: []
"#;

        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        let sip = config.sip.as_ref().unwrap();
        assert_eq!(sip.room_prefix, Some("sip-".to_string()));
        assert!(sip.allowed_addresses.is_empty());
        assert!(sip.hooks.is_empty());
    }

    #[test]
    fn test_yaml_config_sip_missing_fields() {
        let yaml = r#"
sip:
  room_prefix: "sip-"
"#;

        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();

        let sip = config.sip.as_ref().unwrap();
        assert_eq!(sip.room_prefix, Some("sip-".to_string()));
        assert!(sip.allowed_addresses.is_empty()); // default to empty vec
        assert!(sip.hooks.is_empty()); // default to empty vec
    }
}

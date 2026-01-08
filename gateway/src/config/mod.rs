//! Configuration module for Sayna server
//!
//! This module handles server configuration from various sources: .env files, YAML files,
//! and environment variables. Priority: YAML > ENV vars > .env values > defaults.
//! The configuration is split into logical submodules for maintainability and extensibility.
//!
//! # Modules
//! - `yaml`: YAML configuration file loading
//! - `env`: Environment variable loading
//! - `merge`: Merging YAML and environment configurations
//! - `validation`: Configuration validation logic
//! - `utils`: Utility functions for configuration parsing
//!
//! # Example
//! ```rust,no_run
//! use waav_gateway::config::ServerConfig;
//! use std::path::PathBuf;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Load from environment variables only
//! let config = ServerConfig::from_env()?;
//!
//! // Load from YAML file with environment variable overrides
//! let config_path = PathBuf::from("config.yaml");
//! let config = ServerConfig::from_file(&config_path)?;
//!
//! println!("Server listening on {}", config.address());
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

mod env;
mod merge;
pub mod pricing;
mod sip;
mod utils;
mod validation;
mod yaml;

pub use pricing::{
    ModelPricing, PricingUnit, estimate_stt_cost, estimate_tts_cost, get_stt_price_per_hour,
    get_stt_pricing, get_tts_pricing, list_stt_models, list_tts_models,
};
pub use sip::{SipConfig, SipHookConfig};

/// TLS configuration for HTTPS and WSS
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Path to the TLS certificate file (PEM format)
    pub cert_path: PathBuf,
    /// Path to the TLS private key file (PEM format)
    pub key_path: PathBuf,
}

/// API secret authentication entry with a client identifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthApiSecret {
    pub id: String,
    pub secret: String,
}

/// Plugin system configuration
///
/// Controls how the plugin registry discovers and loads providers.
/// This configuration is backward compatible - if not specified,
/// the plugin system is enabled with all built-in providers.
///
/// # Example YAML
/// ```yaml
/// plugins:
///   enabled: true
///   plugin_dir: "/opt/waav/plugins"
///   providers:
///     deepgram:
///       custom_endpoint: "https://custom.deepgram.com"
/// ```
#[derive(Debug, Clone, Default)]
pub struct PluginConfig {
    /// Whether the plugin system is enabled (default: true)
    pub enabled: bool,
    /// Directory to load external plugins from (optional, requires `plugins-dynamic` feature)
    pub plugin_dir: Option<PathBuf>,
    /// Provider-specific configuration (keyed by provider name)
    /// This allows passing custom settings to individual providers
    pub provider_config: HashMap<String, serde_json::Value>,
}

/// Server configuration
///
/// Contains all configuration needed to run the WaaV Gateway server, including:
/// - Server settings (host, port, TLS)
/// - LiveKit integration settings
/// - Provider API keys (Deepgram, ElevenLabs, Google, Azure)
/// - Recording configuration (S3)
/// - Cache settings
/// - Authentication settings
/// - SIP configuration
/// - Security settings (CORS, rate limiting, connection limits)
#[derive(Debug, Clone)]
pub struct ServerConfig {
    // Server settings
    pub host: String,
    pub port: u16,

    // TLS configuration (optional)
    pub tls: Option<TlsConfig>,

    // LiveKit settings
    pub livekit_url: String,
    pub livekit_public_url: String,
    pub livekit_api_key: Option<String>,
    pub livekit_api_secret: Option<String>,

    // Provider API keys
    pub deepgram_api_key: Option<String>,
    pub elevenlabs_api_key: Option<String>,
    /// Google Cloud credentials - can be:
    /// - Empty string: Use Application Default Credentials (ADC)
    /// - JSON string starting with '{': Service account credentials inline
    /// - File path: Path to service account JSON file
    pub google_credentials: Option<String>,
    /// Azure Speech Services subscription key from Azure Portal
    /// (Azure Portal → Speech resource → Keys and Endpoint → Key 1 or Key 2)
    pub azure_speech_subscription_key: Option<String>,
    /// Azure region where the Speech resource is deployed (e.g., "eastus", "westus2")
    /// The subscription key is tied to this specific region
    pub azure_speech_region: Option<String>,
    /// Cartesia API key for both STT (ink-whisper model) and TTS (sonic-3 model)
    pub cartesia_api_key: Option<String>,
    /// OpenAI API key for STT (Whisper), TTS, and Realtime API
    pub openai_api_key: Option<String>,
    /// AssemblyAI API key for streaming STT
    pub assemblyai_api_key: Option<String>,
    /// Hume AI API key for TTS (Octave) and EVI (Empathic Voice Interface)
    pub hume_api_key: Option<String>,
    /// LMNT API key for ultra-low latency TTS and voice cloning
    pub lmnt_api_key: Option<String>,
    /// Groq API key for ultra-fast Whisper STT (216x real-time)
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
    pub gnani_certificate_path: Option<PathBuf>,

    // LiveKit recording configuration
    pub recording_s3_bucket: Option<String>,
    pub recording_s3_region: Option<String>,
    pub recording_s3_endpoint: Option<String>,
    pub recording_s3_access_key: Option<String>,
    pub recording_s3_secret_key: Option<String>,
    /// Optional S3 path prefix for recordings.
    /// Combined with stream_id to form full path: `{prefix}/{stream_id}/audio.ogg`
    pub recording_s3_prefix: Option<String>,

    // Cache configuration (filesystem or memory)
    pub cache_path: Option<PathBuf>, // if None, use in-memory cache
    pub cache_ttl_seconds: Option<u64>,

    // Authentication configuration
    pub auth_service_url: Option<String>,
    pub auth_signing_key_path: Option<PathBuf>,
    pub auth_api_secrets: Vec<AuthApiSecret>,
    pub auth_timeout_seconds: u64,
    pub auth_required: bool,

    // SIP configuration (optional)
    pub sip: Option<SipConfig>,

    // Security configuration
    /// CORS allowed origins (comma-separated list or "*" for all)
    /// Default: None (CORS disabled, same-origin only)
    pub cors_allowed_origins: Option<String>,

    // Rate limiting configuration
    /// Maximum requests per second per IP address
    /// Default: 60
    pub rate_limit_requests_per_second: u32,
    /// Maximum burst size for rate limiting
    /// Default: 10
    pub rate_limit_burst_size: u32,

    // Connection limits
    /// Maximum concurrent WebSocket connections
    /// Default: None (unlimited)
    pub max_websocket_connections: Option<usize>,
    /// Maximum connections per IP address
    /// Default: 100
    pub max_connections_per_ip: u32,

    // Plugin configuration
    /// Plugin system configuration (optional, backward compatible)
    /// If not specified, the plugin system is enabled with built-in providers only
    pub plugins: PluginConfig,
}

/// Implement Drop to zeroize all secret fields when ServerConfig is dropped.
/// This ensures sensitive data is cleared from memory immediately after use.
impl Drop for ServerConfig {
    fn drop(&mut self) {
        use zeroize::Zeroize;

        // Zeroize all API keys and secrets to prevent memory leaks of sensitive data
        if let Some(ref mut key) = self.livekit_api_key {
            key.zeroize();
        }
        if let Some(ref mut secret) = self.livekit_api_secret {
            secret.zeroize();
        }
        if let Some(ref mut key) = self.deepgram_api_key {
            key.zeroize();
        }
        if let Some(ref mut key) = self.elevenlabs_api_key {
            key.zeroize();
        }
        if let Some(ref mut creds) = self.google_credentials {
            creds.zeroize();
        }
        if let Some(ref mut key) = self.azure_speech_subscription_key {
            key.zeroize();
        }
        if let Some(ref mut key) = self.cartesia_api_key {
            key.zeroize();
        }
        if let Some(ref mut key) = self.openai_api_key {
            key.zeroize();
        }
        if let Some(ref mut key) = self.assemblyai_api_key {
            key.zeroize();
        }
        if let Some(ref mut key) = self.recording_s3_access_key {
            key.zeroize();
        }
        if let Some(ref mut secret) = self.recording_s3_secret_key {
            secret.zeroize();
        }
        if let Some(ref mut key) = self.groq_api_key {
            key.zeroize();
        }
        if let Some(ref mut key) = self.playht_api_key {
            key.zeroize();
        }
        if let Some(ref mut key) = self.ibm_watson_api_key {
            key.zeroize();
        }
        if let Some(ref mut key) = self.aws_access_key_id {
            key.zeroize();
        }
        if let Some(ref mut secret) = self.aws_secret_access_key {
            secret.zeroize();
        }
        if let Some(ref mut token) = self.gnani_token {
            token.zeroize();
        }
        if let Some(ref mut key) = self.gnani_access_key {
            key.zeroize();
        }
        // Zeroize auth API secrets
        for secret in &mut self.auth_api_secrets {
            secret.secret.zeroize();
        }
        // Zeroize SIP hook secrets if present
        if let Some(ref mut sip) = self.sip {
            if let Some(ref mut hook_secret) = sip.hook_secret {
                hook_secret.zeroize();
            }
            for hook in &mut sip.hooks {
                if let Some(ref mut secret) = hook.secret {
                    secret.zeroize();
                }
            }
        }
    }
}

impl ServerConfig {
    /// Load configuration from a YAML file with environment variable base
    ///
    /// Loads .env file (if present), then merges environment variables (with defaults),
    /// and finally applies YAML overrides. This allows .env and environment variables
    /// to provide base configuration while YAML can override specific values.
    ///
    /// Priority order (highest to lowest):
    /// 1. YAML file values
    /// 2. Environment variables (actual ENV vars override .env values)
    /// 3. .env file values
    /// 4. Default values
    ///
    /// After loading and merging, performs validation on the final configuration.
    ///
    /// # Arguments
    /// * `path` - Path to the YAML configuration file
    ///
    /// # Returns
    /// * `Result<Self, Box<dyn std::error::Error>>` - The loaded configuration or an error
    ///
    /// # Errors
    /// Returns an error if:
    /// - The YAML file cannot be read or is malformed
    /// - Environment variables have invalid formats
    /// - Configuration validation fails
    ///
    /// # Example
    /// ```rust,no_run
    /// use waav_gateway::config::ServerConfig;
    /// use std::path::PathBuf;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let config_path = PathBuf::from("config.yaml");
    /// let config = ServerConfig::from_file(&config_path)?;
    /// println!("Server listening on {}", config.address());
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_file(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        // The configuration priority is: YAML > Environment Variables (.env + actual ENV) > Defaults
        // Note: .env file is loaded in main.rs at application startup
        // This gives predictable behavior where:
        // 1. .env file values are loaded as environment variables (in main.rs)
        // 2. Actual environment variables override .env values
        // 3. YAML file overrides all environment variables

        // Load YAML configuration
        let yaml_config = yaml::YamlConfig::from_file(path)?;

        // Merge environment variables (base) with YAML overrides
        let config = merge::merge_config(Some(yaml_config))?;

        // Validate configuration
        validation::validate_jwt_auth(&config.auth_service_url, &config.auth_signing_key_path)?;
        validation::validate_auth_api_secrets(&config.auth_api_secrets)?;
        validation::validate_auth_required(
            config.auth_required,
            &config.auth_service_url,
            &config.auth_signing_key_path,
            &config.auth_api_secrets,
        )?;
        validation::validate_sip_config(&config.sip)?;

        Ok(config)
    }

    /// Get the server address as a string
    ///
    /// Returns the address in the format "host:port"
    ///
    /// # Example
    /// ```rust,no_run
    /// use waav_gateway::config::ServerConfig;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = ServerConfig::from_env()?;
    /// println!("Listening on {}", config.address());
    /// # Ok(())
    /// # }
    /// ```
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Check if TLS is enabled
    ///
    /// Returns true if TLS configuration is present
    pub fn is_tls_enabled(&self) -> bool {
        self.tls.is_some()
    }

    /// Check if JWT-based authentication is configured
    ///
    /// Returns true if both AUTH_SERVICE_URL and AUTH_SIGNING_KEY_PATH are set
    pub fn has_jwt_auth(&self) -> bool {
        self.auth_service_url.is_some() && self.auth_signing_key_path.is_some()
    }

    /// Check if API secret authentication is configured
    ///
    /// Returns true if at least one API secret entry is configured
    pub fn has_api_secret_auth(&self) -> bool {
        !self.auth_api_secrets.is_empty()
    }

    /// Find the API secret identifier that matches a bearer token
    ///
    /// Returns the configured id when the token matches a known secret.
    pub fn find_api_secret_id(&self, token: &str) -> Option<&str> {
        self.auth_api_secrets
            .iter()
            .find(|entry| entry.secret == token)
            .map(|entry| entry.id.as_str())
    }

    /// Get API key for a specific provider
    ///
    /// # Arguments
    /// * `provider` - The name of the provider (e.g., "deepgram", "elevenlabs")
    ///
    /// # Returns
    /// * `Result<String, String>` - The API key on success, or an error message on failure
    ///
    /// # Example
    /// ```rust,no_run
    /// use waav_gateway::config::ServerConfig;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = ServerConfig::from_env()?;
    /// let api_key = config.get_api_key("deepgram")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn get_api_key(&self, provider: &str) -> Result<String, String> {
        match provider.to_lowercase().as_str() {
            "deepgram" => {
                self.deepgram_api_key.as_ref().cloned().ok_or_else(|| {
                    "Deepgram API key not configured in server environment".to_string()
                })
            }
            "elevenlabs" => self.elevenlabs_api_key.as_ref().cloned().ok_or_else(|| {
                "ElevenLabs API key not configured in server environment".to_string()
            }),
            "google" => {
                // Google uses credentials that can be:
                // - Empty string: Use Application Default Credentials (ADC)
                // - JSON content: Service account credentials inline
                // - File path: Path to service account JSON file
                //
                // If google_credentials is None, return empty string to trigger ADC.
                // This allows Google STT to work with GOOGLE_APPLICATION_CREDENTIALS
                // environment variable or gcloud auth.
                Ok(self.google_credentials.clone().unwrap_or_default())
            }
            "microsoft-azure" => {
                // Azure Speech Services uses subscription key authentication
                // The key is tied to a specific Azure region
                self.azure_speech_subscription_key
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| {
                        "Azure Speech subscription key not configured in server environment"
                            .to_string()
                    })
            }
            "cartesia" => {
                // Cartesia uses API key authentication for both STT and TTS
                self.cartesia_api_key.as_ref().cloned().ok_or_else(|| {
                    "Cartesia API key not configured in server environment".to_string()
                })
            }
            "openai" => {
                // OpenAI uses API key authentication for STT (Whisper), TTS, and Realtime API
                self.openai_api_key.as_ref().cloned().ok_or_else(|| {
                    "OpenAI API key not configured in server environment".to_string()
                })
            }
            "assemblyai" => {
                // AssemblyAI uses API key authentication for streaming STT
                self.assemblyai_api_key.as_ref().cloned().ok_or_else(|| {
                    "AssemblyAI API key not configured in server environment".to_string()
                })
            }
            "hume" => {
                // Hume AI uses API key authentication for TTS (Octave) and EVI
                self.hume_api_key
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| "Hume API key not configured in server environment".to_string())
            }
            "lmnt" | "lmnt-ai" | "lmnt_ai" => {
                // LMNT uses API key authentication for TTS and voice cloning
                self.lmnt_api_key
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| "LMNT API key not configured in server environment".to_string())
            }
            "groq" => {
                // Groq uses API key authentication for ultra-fast Whisper STT
                self.groq_api_key
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| "Groq API key not configured in server environment".to_string())
            }
            "playht" | "play.ht" | "play-ht" => {
                // Play.ht uses API key authentication for TTS
                self.playht_api_key.as_ref().cloned().ok_or_else(|| {
                    "Play.ht API key not configured in server environment".to_string()
                })
            }
            "ibm-watson" | "ibm_watson" | "ibm" | "watson" => {
                // IBM Watson uses API key authentication for STT/TTS
                self.ibm_watson_api_key.as_ref().cloned().ok_or_else(|| {
                    "IBM Watson API key not configured in server environment".to_string()
                })
            }
            "aws" | "aws-transcribe" | "aws-polly" | "amazon" => {
                // AWS uses access key ID for Transcribe/Polly
                self.aws_access_key_id.as_ref().cloned().ok_or_else(|| {
                    "AWS access key ID not configured in server environment".to_string()
                })
            }
            "azure" => {
                // Azure Speech Services alias for microsoft-azure
                self.azure_speech_subscription_key
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| {
                        "Azure Speech subscription key not configured in server environment"
                            .to_string()
                    })
            }
            "gnani" | "gnani-ai" | "gnani.ai" | "vachana" => {
                // Gnani.ai uses token-based authentication for STT/TTS
                // Returns token as the "api_key" - access_key and certificate are handled separately
                self.gnani_token.as_ref().cloned().ok_or_else(|| {
                    "Gnani token not configured in server environment (GNANI_TOKEN)".to_string()
                })
            }
            _ => Err(format!("Unsupported provider: {provider}")),
        }
    }

    /// Get Azure Speech Services region
    ///
    /// Returns the configured Azure region, or "eastus" as default if not specified.
    /// The region must match where the subscription key was created.
    ///
    /// # Returns
    /// * `String` - The Azure region identifier (e.g., "eastus", "westus2")
    pub fn get_azure_speech_region(&self) -> String {
        self.azure_speech_region
            .clone()
            .unwrap_or_else(|| "eastus".to_string())
    }

    /// Get Play.ht credentials (API key and user ID)
    ///
    /// Play.ht uses dual-header authentication requiring both API key and user ID.
    ///
    /// # Returns
    /// * `Result<(String, String), String>` - Tuple of (api_key, user_id) on success
    pub fn get_playht_credentials(&self) -> Result<(String, String), String> {
        let api_key = self
            .playht_api_key
            .as_ref()
            .cloned()
            .ok_or_else(|| "Play.ht API key not configured".to_string())?;
        let user_id = self
            .playht_user_id
            .as_ref()
            .cloned()
            .ok_or_else(|| "Play.ht user ID not configured".to_string())?;
        Ok((api_key, user_id))
    }

    /// Get AWS credentials (access key ID, secret access key, region)
    ///
    /// AWS Transcribe and Polly require all three components for authentication.
    ///
    /// # Returns
    /// * `Result<(String, String, String), String>` - Tuple of (access_key_id, secret_access_key, region)
    pub fn get_aws_credentials(&self) -> Result<(String, String, String), String> {
        let access_key_id = self
            .aws_access_key_id
            .as_ref()
            .cloned()
            .ok_or_else(|| "AWS access key ID not configured".to_string())?;
        let secret_access_key = self
            .aws_secret_access_key
            .as_ref()
            .cloned()
            .ok_or_else(|| "AWS secret access key not configured".to_string())?;
        let region = self
            .aws_region
            .clone()
            .unwrap_or_else(|| "us-east-1".to_string());
        Ok((access_key_id, secret_access_key, region))
    }

    /// Get IBM Watson credentials (API key, instance ID, region)
    ///
    /// IBM Watson STT/TTS require API key and service instance ID.
    ///
    /// # Returns
    /// * `Result<(String, String, String), String>` - Tuple of (api_key, instance_id, region)
    pub fn get_ibm_watson_credentials(&self) -> Result<(String, String, String), String> {
        let api_key = self
            .ibm_watson_api_key
            .as_ref()
            .cloned()
            .ok_or_else(|| "IBM Watson API key not configured".to_string())?;
        let instance_id = self
            .ibm_watson_instance_id
            .as_ref()
            .cloned()
            .ok_or_else(|| "IBM Watson instance ID not configured".to_string())?;
        let region = self
            .ibm_watson_region
            .clone()
            .unwrap_or_else(|| "us-south".to_string());
        Ok((api_key, instance_id, region))
    }
}

pub(crate) fn parse_auth_api_secrets_json(
    json_str: &str,
) -> Result<Vec<AuthApiSecret>, Box<dyn std::error::Error>> {
    #[derive(serde::Deserialize)]
    struct AuthApiSecretJson {
        id: String,
        secret: String,
    }

    let secrets: Vec<AuthApiSecretJson> = serde_json::from_str(json_str)
        .map_err(|e| format!("Invalid AUTH_API_SECRETS_JSON format: {e}"))?;

    Ok(secrets
        .into_iter()
        .map(|entry| AuthApiSecret {
            id: entry.id,
            secret: entry.secret,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use std::fs;
    use tempfile::TempDir;

    /// Helper function to create a test ServerConfig with defaults
    fn test_config() -> ServerConfig {
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
            plugins: PluginConfig::default(),
        }
    }

    #[test]
    fn test_get_api_key_deepgram_success() {
        let mut config = test_config();
        config.deepgram_api_key = Some("test-deepgram-key".to_string());
        config.recording_s3_prefix = Some("recordings/base".to_string());

        let result = config.get_api_key("deepgram");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-deepgram-key");
        assert_eq!(
            config.recording_s3_prefix,
            Some("recordings/base".to_string())
        );
    }

    #[test]
    fn test_get_api_key_elevenlabs_success() {
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: None,
            elevenlabs_api_key: Some("test-elevenlabs-key".to_string()),
            google_credentials: None,
            azure_speech_subscription_key: None,
            azure_speech_region: None,
            cartesia_api_key: None,
            openai_api_key: None,
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
            plugins: PluginConfig::default(),
        };

        let result = config.get_api_key("elevenlabs");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-elevenlabs-key");
    }

    #[test]
    fn test_get_api_key_deepgram_missing() {
        let config = ServerConfig {
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
            plugins: PluginConfig::default(),
        };

        let result = config.get_api_key("deepgram");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Deepgram API key not configured in server environment"
        );
    }

    #[test]
    fn test_get_api_key_unsupported_provider() {
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: Some("test-key".to_string()),
            elevenlabs_api_key: None,
            google_credentials: None,
            azure_speech_subscription_key: None,
            azure_speech_region: None,
            cartesia_api_key: None,
            openai_api_key: None,
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
            plugins: PluginConfig::default(),
        };

        let result = config.get_api_key("unsupported_provider");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Unsupported provider: unsupported_provider"
        );
    }

    #[test]
    fn test_get_api_key_case_insensitive() {
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: Some("test-deepgram-key".to_string()),
            elevenlabs_api_key: Some("test-elevenlabs-key".to_string()),
            google_credentials: None,
            azure_speech_subscription_key: None,
            azure_speech_region: None,
            cartesia_api_key: None,
            openai_api_key: None,
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
            plugins: PluginConfig::default(),
        };

        // Test uppercase
        let result1 = config.get_api_key("DEEPGRAM");
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), "test-deepgram-key");

        // Test mixed case
        let result2 = config.get_api_key("ElevenLabs");
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap(), "test-elevenlabs-key");
    }

    #[test]
    fn test_has_jwt_auth() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("test_key.pem");

        let config_with_jwt = ServerConfig {
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
            auth_service_url: Some("http://auth.example.com".to_string()),
            auth_signing_key_path: Some(key_path),
            auth_api_secrets: Vec::new(),
            auth_timeout_seconds: 5,
            auth_required: true,
            sip: None,
            cors_allowed_origins: None,
            rate_limit_requests_per_second: 60,
            rate_limit_burst_size: 10,
            max_websocket_connections: None,
            max_connections_per_ip: 100,
            plugins: PluginConfig::default(),
        };

        assert!(config_with_jwt.has_jwt_auth());
        assert!(!config_with_jwt.has_api_secret_auth());

        let config_without_jwt = ServerConfig {
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
            plugins: PluginConfig::default(),
        };

        assert!(!config_without_jwt.has_jwt_auth());
    }

    #[test]
    fn test_has_api_secret_auth() {
        let config_with_api_secret = ServerConfig {
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
            auth_api_secrets: vec![AuthApiSecret {
                id: "default".to_string(),
                secret: "my-secret-token".to_string(),
            }],
            auth_timeout_seconds: 5,
            auth_required: true,
            sip: None,
            cors_allowed_origins: None,
            rate_limit_requests_per_second: 60,
            rate_limit_burst_size: 10,
            max_websocket_connections: None,
            max_connections_per_ip: 100,
            plugins: PluginConfig::default(),
        };

        assert!(config_with_api_secret.has_api_secret_auth());
        assert!(!config_with_api_secret.has_jwt_auth());

        let config_without_api_secret = ServerConfig {
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
            plugins: PluginConfig::default(),
        };

        assert!(!config_without_api_secret.has_api_secret_auth());
    }

    #[test]
    fn test_find_api_secret_id() {
        let config = ServerConfig {
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
            auth_api_secrets: vec![
                AuthApiSecret {
                    id: "client-a".to_string(),
                    secret: "token-a".to_string(),
                },
                AuthApiSecret {
                    id: "client-b".to_string(),
                    secret: "token-b".to_string(),
                },
            ],
            auth_timeout_seconds: 5,
            auth_required: true,
            sip: None,
            cors_allowed_origins: None,
            rate_limit_requests_per_second: 60,
            rate_limit_burst_size: 10,
            max_websocket_connections: None,
            max_connections_per_ip: 100,
            plugins: PluginConfig::default(),
        };

        assert_eq!(config.find_api_secret_id("token-a"), Some("client-a"));
        assert_eq!(config.find_api_secret_id("missing"), None);
    }

    #[test]
    fn test_get_api_key_google_with_credentials() {
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: None,
            elevenlabs_api_key: None,
            google_credentials: Some("/path/to/service-account.json".to_string()),
            azure_speech_subscription_key: None,
            azure_speech_region: None,
            cartesia_api_key: None,
            openai_api_key: None,
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
            plugins: PluginConfig::default(),
        };

        // Google returns the credentials path/content when configured
        let result = config.get_api_key("google");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/path/to/service-account.json");
    }

    #[test]
    fn test_get_api_key_google_with_json_content() {
        let json_credentials = r#"{"type": "service_account", "project_id": "test-project"}"#;
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: None,
            elevenlabs_api_key: None,
            google_credentials: Some(json_credentials.to_string()),
            azure_speech_subscription_key: None,
            azure_speech_region: None,
            cartesia_api_key: None,
            openai_api_key: None,
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
            plugins: PluginConfig::default(),
        };

        // Google returns the inline JSON credentials when configured
        let result = config.get_api_key("google");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), json_credentials);
    }

    #[test]
    fn test_get_api_key_google_none_returns_empty_for_adc() {
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: None,
            elevenlabs_api_key: None,
            google_credentials: None, // Not configured - will use ADC
            azure_speech_subscription_key: None,
            azure_speech_region: None,
            cartesia_api_key: None,
            openai_api_key: None,
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
            plugins: PluginConfig::default(),
        };

        // Google returns empty string when not configured, allowing ADC to be used
        let result = config.get_api_key("google");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_get_api_key_google_case_insensitive() {
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: None,
            elevenlabs_api_key: None,
            google_credentials: Some("/path/to/creds.json".to_string()),
            azure_speech_subscription_key: None,
            azure_speech_region: None,
            cartesia_api_key: None,
            openai_api_key: None,
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
            plugins: PluginConfig::default(),
        };

        // Test uppercase
        let result1 = config.get_api_key("GOOGLE");
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), "/path/to/creds.json");

        // Test mixed case
        let result2 = config.get_api_key("Google");
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap(), "/path/to/creds.json");
    }

    #[test]
    fn test_get_api_key_azure_success() {
        let config = ServerConfig {
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
            azure_speech_subscription_key: Some("test-azure-key".to_string()),
            azure_speech_region: Some("westus2".to_string()),
            cartesia_api_key: None,
            openai_api_key: None,
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
            plugins: PluginConfig::default(),
        };

        let result = config.get_api_key("microsoft-azure");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-azure-key");
    }

    #[test]
    fn test_get_api_key_azure_missing() {
        let config = ServerConfig {
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
            plugins: PluginConfig::default(),
        };

        let result = config.get_api_key("microsoft-azure");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Azure Speech subscription key not configured in server environment"
        );
    }

    #[test]
    fn test_get_azure_speech_region_configured() {
        let config = ServerConfig {
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
            azure_speech_subscription_key: Some("test-key".to_string()),
            azure_speech_region: Some("westeurope".to_string()),
            cartesia_api_key: None,
            openai_api_key: None,
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
            plugins: PluginConfig::default(),
        };

        assert_eq!(config.get_azure_speech_region(), "westeurope");
    }

    #[test]
    fn test_get_azure_speech_region_default() {
        let config = ServerConfig {
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
            plugins: PluginConfig::default(),
        };

        // Default is "eastus"
        assert_eq!(config.get_azure_speech_region(), "eastus");
    }

    // Helper to clean up environment variables
    fn cleanup_env_vars() {
        unsafe {
            env::remove_var("HOST");
            env::remove_var("PORT");
            env::remove_var("LIVEKIT_URL");
            env::remove_var("LIVEKIT_PUBLIC_URL");
            env::remove_var("DEEPGRAM_API_KEY");
            env::remove_var("ELEVENLABS_API_KEY");
            env::remove_var("CACHE_PATH");
            env::remove_var("CACHE_TTL_SECONDS");
            env::remove_var("AUTH_REQUIRED");
            env::remove_var("AUTH_SERVICE_URL");
            env::remove_var("AUTH_SIGNING_KEY_PATH");
            env::remove_var("AUTH_API_SECRETS_JSON");
            env::remove_var("AUTH_API_SECRET");
            env::remove_var("AUTH_API_SECRET_ID");
            env::remove_var("AUTH_TIMEOUT_SECONDS");
            env::remove_var("RECORDING_S3_PREFIX");
        }
    }

    #[test]
    #[serial]
    fn test_from_file_yaml_only() {
        cleanup_env_vars();

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");

        let yaml_content = r#"
server:
  host: "127.0.0.1"
  port: 8080

providers:
  deepgram_api_key: "yaml-dg-key"
  elevenlabs_api_key: "yaml-el-key"

cache:
  path: "/tmp/yaml-cache"
  ttl_seconds: 7200
"#;

        fs::write(&config_path, yaml_content).unwrap();

        let config = ServerConfig::from_file(&config_path).unwrap();

        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.deepgram_api_key, Some("yaml-dg-key".to_string()));
        assert_eq!(config.elevenlabs_api_key, Some("yaml-el-key".to_string()));
        assert_eq!(config.cache_path, Some(PathBuf::from("/tmp/yaml-cache")));
        assert_eq!(config.cache_ttl_seconds, Some(7200));

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_from_file_yaml_overrides_env() {
        cleanup_env_vars();

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");

        let yaml_content = r#"
server:
  host: "127.0.0.1"
  port: 8080

providers:
  deepgram_api_key: "yaml-key"
"#;

        fs::write(&config_path, yaml_content).unwrap();

        unsafe {
            env::set_var("HOST", "0.0.0.0");
            env::set_var("DEEPGRAM_API_KEY", "env-key");
        }

        let config = ServerConfig::from_file(&config_path).unwrap();

        // YAML overrides ENV
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.deepgram_api_key, Some("yaml-key".to_string()));
        // YAML value
        assert_eq!(config.port, 8080);

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_from_file_missing_file() {
        cleanup_env_vars();

        let config_path = PathBuf::from("/nonexistent/config.yaml");
        let result = ServerConfig::from_file(&config_path);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to read config file")
        );

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_from_file_invalid_yaml() {
        cleanup_env_vars();

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("invalid.yaml");

        fs::write(&config_path, "invalid: yaml: [content").unwrap();

        let result = ServerConfig::from_file(&config_path);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to parse YAML")
        );

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_from_file_with_auth() {
        cleanup_env_vars();

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");
        let key_path = temp_dir.path().join("key.pem");
        fs::write(&key_path, "fake key").unwrap();

        let yaml_content = format!(
            r#"
auth:
  required: true
  service_url: "https://auth.example.com"
  signing_key_path: "{}"
  timeout_seconds: 10
"#,
            key_path.display()
        );

        fs::write(&config_path, yaml_content).unwrap();

        let config = ServerConfig::from_file(&config_path).unwrap();

        assert!(config.auth_required);
        assert_eq!(
            config.auth_service_url,
            Some("https://auth.example.com".to_string())
        );
        assert_eq!(config.auth_signing_key_path, Some(key_path));
        assert_eq!(config.auth_timeout_seconds, 10);

        cleanup_env_vars();
    }

    #[test]
    #[serial]
    fn test_from_file_partial_config() {
        cleanup_env_vars();

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");

        let yaml_content = r#"
server:
  port: 9000

cache:
  ttl_seconds: 1800
"#;

        fs::write(&config_path, yaml_content).unwrap();

        // Ensure we get default values by setting them explicitly
        // (in case there's a .env file in the project directory)
        unsafe {
            env::set_var("LIVEKIT_URL", "ws://localhost:7880");
        }

        let config = ServerConfig::from_file(&config_path).unwrap();

        // YAML values
        assert_eq!(config.port, 9000);
        assert_eq!(config.cache_ttl_seconds, Some(1800));

        // Values from ENV (which we set to defaults)
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.livekit_url, "ws://localhost:7880");
        assert!(!config.auth_required);

        cleanup_env_vars();
    }
}

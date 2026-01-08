//! Plugin and Provider Metadata Types
//!
//! This module defines the metadata structures for plugins and providers,
//! including version information, capabilities, and configuration requirements.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Plugin manifest containing metadata about a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier (e.g., "deepgram-stt")
    pub id: String,

    /// Human-readable plugin name (e.g., "Deepgram Speech-to-Text")
    pub name: String,

    /// Semantic version of the plugin
    pub version: semver::Version,

    /// Plugin author or organization
    pub author: String,

    /// Brief description of the plugin
    pub description: String,

    /// Required gateway version (semver range)
    pub gateway_version: semver::VersionReq,

    /// Plugin dependencies (other plugin IDs with version requirements)
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,

    /// Whether this plugin can run in WASM sandbox
    #[serde(default)]
    pub sandboxable: bool,
}

impl PluginManifest {
    /// Create a new plugin manifest with minimal required fields
    ///
    /// # Arguments
    /// * `id` - Unique plugin identifier
    /// * `name` - Human-readable plugin name
    /// * `version` - Semantic version string (e.g., "1.0.0"). Falls back to 1.0.0 if invalid.
    pub fn new(id: impl Into<String>, name: impl Into<String>, version: &str) -> Self {
        let parsed_version = match semver::Version::parse(version) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    version = %version,
                    error = %e,
                    "Invalid plugin version, falling back to 1.0.0"
                );
                semver::Version::new(1, 0, 0)
            }
        };

        // This parse should never fail as it's a hardcoded valid string
        // but we handle the error gracefully anyway
        let gateway_version =
            semver::VersionReq::parse(">=1.0.0").unwrap_or(semver::VersionReq::STAR);

        Self {
            id: id.into(),
            name: name.into(),
            version: parsed_version,
            author: String::new(),
            description: String::new(),
            gateway_version,
            dependencies: Vec::new(),
            sandboxable: false,
        }
    }

    /// Set the author
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = author.into();
        self
    }

    /// Set the description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

/// Plugin dependency specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDependency {
    /// Plugin ID of the dependency
    pub plugin_id: String,

    /// Version requirement (semver range)
    pub version_req: semver::VersionReq,
}

/// Provider metadata for discovery and documentation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderMetadata {
    /// Provider identifier (e.g., "deepgram", "elevenlabs")
    pub name: String,

    /// Display name (e.g., "Deepgram Nova-3")
    pub display_name: String,

    /// Brief description
    pub description: String,

    /// Version string
    pub version: String,

    /// Required configuration keys (for validation)
    pub required_config_keys: Vec<String>,

    /// Optional configuration keys
    pub optional_config_keys: Vec<String>,

    /// Provider aliases (e.g., ["azure", "microsoft-azure"])
    pub aliases: Vec<String>,

    /// Supported languages (ISO 639-1 codes)
    #[serde(default)]
    pub supported_languages: Vec<String>,

    /// Supported models
    #[serde(default)]
    pub supported_models: Vec<String>,

    /// Provider features (e.g., "streaming", "word-timestamps", "speaker-diarization")
    #[serde(default)]
    pub features: HashSet<String>,

    /// Provider type (stt, tts, realtime)
    pub provider_type: ProviderType,
}

impl ProviderMetadata {
    /// Create new provider metadata with minimal required fields
    pub fn new(name: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            version: "1.0.0".to_string(),
            required_config_keys: vec!["api_key".to_string()],
            ..Default::default()
        }
    }

    /// Create STT provider metadata
    pub fn stt(name: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            provider_type: ProviderType::STT,
            ..Self::new(name, display_name)
        }
    }

    /// Create TTS provider metadata
    pub fn tts(name: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            provider_type: ProviderType::TTS,
            ..Self::new(name, display_name)
        }
    }

    /// Create Realtime provider metadata
    pub fn realtime(name: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            provider_type: ProviderType::Realtime,
            ..Self::new(name, display_name)
        }
    }

    /// Set the description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Add an alias
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    /// Add multiple aliases
    pub fn with_aliases(mut self, aliases: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.aliases.extend(aliases.into_iter().map(Into::into));
        self
    }

    /// Set required config keys
    pub fn with_required_config(
        mut self,
        keys: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.required_config_keys = keys.into_iter().map(Into::into).collect();
        self
    }

    /// Add a feature
    pub fn with_feature(mut self, feature: impl Into<String>) -> Self {
        self.features.insert(feature.into());
        self
    }

    /// Add multiple features
    pub fn with_features(mut self, features: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.features.extend(features.into_iter().map(Into::into));
        self
    }

    /// Set supported languages
    pub fn with_languages(
        mut self,
        languages: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.supported_languages = languages.into_iter().map(Into::into).collect();
        self
    }

    /// Set supported models
    pub fn with_models(mut self, models: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.supported_models = models.into_iter().map(Into::into).collect();
        self
    }
}

/// Provider type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    #[default]
    STT,
    TTS,
    Realtime,
    Processor,
    Middleware,
    Auth,
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderType::STT => write!(f, "stt"),
            ProviderType::TTS => write!(f, "tts"),
            ProviderType::Realtime => write!(f, "realtime"),
            ProviderType::Processor => write!(f, "processor"),
            ProviderType::Middleware => write!(f, "middleware"),
            ProviderType::Auth => write!(f, "auth"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_metadata_builder() {
        let metadata = ProviderMetadata::stt("deepgram", "Deepgram Nova-3")
            .with_description("Real-time STT with high accuracy")
            .with_alias("dg")
            .with_features(["streaming", "word-timestamps"])
            .with_languages(["en", "es", "fr"]);

        assert_eq!(metadata.name, "deepgram");
        assert_eq!(metadata.display_name, "Deepgram Nova-3");
        assert_eq!(metadata.provider_type, ProviderType::STT);
        assert!(metadata.features.contains("streaming"));
        assert_eq!(metadata.supported_languages.len(), 3);
    }

    #[test]
    fn test_plugin_manifest() {
        let manifest = PluginManifest::new("my-plugin", "My Plugin", "1.0.0")
            .with_author("Test Author")
            .with_description("A test plugin");

        assert_eq!(manifest.id, "my-plugin");
        assert_eq!(manifest.author, "Test Author");
    }
}

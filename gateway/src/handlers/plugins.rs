//! Plugin Discovery REST Endpoints
//!
//! This module provides REST endpoints for plugin discovery, allowing SDKs
//! and dashboards to dynamically discover available providers and their capabilities.
//!
//! # Endpoints
//!
//! - `GET /plugins` - List all plugins grouped by type
//! - `GET /plugins/stt` - List STT providers with metadata
//! - `GET /plugins/tts` - List TTS providers with metadata
//! - `GET /plugins/realtime` - List realtime providers with metadata
//! - `GET /plugins/processors` - List audio processors
//! - `GET /plugins/{id}` - Get specific provider info
//! - `GET /plugins/{id}/health` - Get provider health status

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::plugin::global_registry;
use crate::state::AppState;

/// Response for GET /plugins - all plugins grouped by type
#[derive(Debug, Serialize)]
pub struct PluginListResponse {
    /// STT providers
    pub stt: Vec<ProviderInfo>,
    /// TTS providers
    pub tts: Vec<ProviderInfo>,
    /// Realtime providers
    pub realtime: Vec<ProviderInfo>,
    /// Audio processors
    pub processors: Vec<ProcessorInfo>,
    /// Total count of all plugins
    pub total_count: usize,
}

/// Provider information for SDK discovery
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    /// Provider identifier (e.g., "deepgram")
    pub id: String,
    /// Display name (e.g., "Deepgram Nova-3")
    pub display_name: String,
    /// Provider type (stt, tts, realtime)
    pub provider_type: String,
    /// Brief description
    pub description: String,
    /// Version string
    pub version: String,
    /// Provider features (e.g., ["streaming", "word-timestamps"])
    pub features: Vec<String>,
    /// Supported languages with human-readable names
    pub languages: Vec<LanguageInfo>,
    /// Supported models
    pub models: Vec<String>,
    /// Provider aliases (e.g., ["dg", "deepgram-nova"])
    pub aliases: Vec<String>,
    /// Required configuration keys
    pub required_config: Vec<String>,
    /// Optional configuration keys
    pub optional_config: Vec<String>,
    /// Health status
    pub health: String,
    /// Usage metrics (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<ProviderMetrics>,
}

/// Audio processor information
#[derive(Debug, Clone, Serialize)]
pub struct ProcessorInfo {
    /// Processor identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Description
    pub description: String,
    /// Supported audio formats
    pub supported_formats: Vec<String>,
}

/// Language information with human-readable name
#[derive(Debug, Clone, Serialize)]
pub struct LanguageInfo {
    /// ISO 639-1 code (e.g., "en")
    pub code: String,
    /// Human-readable name (e.g., "English")
    pub name: String,
}

/// Provider usage metrics
#[derive(Debug, Clone, Serialize)]
pub struct ProviderMetrics {
    /// Total number of calls
    pub call_count: u64,
    /// Number of errors
    pub error_count: u64,
    /// Error rate (0.0 to 1.0)
    pub error_rate: f64,
    /// Uptime in seconds
    pub uptime_seconds: u64,
}

/// Query parameters for filtering providers
#[derive(Debug, Deserialize)]
pub struct ProviderFilterQuery {
    /// Filter by language (ISO 639-1 code)
    pub language: Option<String>,
    /// Filter by feature
    pub feature: Option<String>,
    /// Filter by model
    pub model: Option<String>,
}

/// Health status response
#[derive(Debug, Serialize)]
pub struct ProviderHealthResponse {
    /// Provider ID
    pub id: String,
    /// Health status
    pub health: String,
    /// Health status details
    pub details: ProviderHealthDetails,
}

/// Health status details
#[derive(Debug, Serialize)]
pub struct ProviderHealthDetails {
    /// Call count
    pub call_count: u64,
    /// Error count
    pub error_count: u64,
    /// Error rate
    pub error_rate: f64,
    /// Last error message (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Idle time in seconds
    pub idle_seconds: u64,
}

/// List all plugins grouped by type
///
/// Returns all registered plugins including STT, TTS, realtime providers,
/// and audio processors with their metadata.
pub async fn list_plugins(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<PluginListResponse>, StatusCode> {
    let registry = global_registry();

    // Collect STT providers
    let stt_names = registry.get_stt_provider_names();
    let stt: Vec<ProviderInfo> = stt_names
        .iter()
        .filter_map(|name| {
            registry.get_stt_metadata(name).map(|meta| {
                provider_metadata_to_info(name, &meta, "stt", registry)
            })
        })
        .collect();

    // Collect TTS providers
    let tts_names = registry.get_tts_provider_names();
    let tts: Vec<ProviderInfo> = tts_names
        .iter()
        .filter_map(|name| {
            registry.get_tts_metadata(name).map(|meta| {
                provider_metadata_to_info(name, &meta, "tts", registry)
            })
        })
        .collect();

    // Collect realtime providers
    let realtime_names = registry.get_realtime_provider_names();
    let realtime: Vec<ProviderInfo> = realtime_names
        .iter()
        .filter_map(|name| {
            registry.get_realtime_metadata(name).map(|meta| {
                provider_metadata_to_info(name, &meta, "realtime", registry)
            })
        })
        .collect();

    // Collect audio processors
    let processor_names = registry.get_audio_processor_names();
    let processors: Vec<ProcessorInfo> = processor_names
        .iter()
        .filter_map(|name| {
            registry.get_audio_processor_metadata(name).map(|meta| {
                ProcessorInfo {
                    id: name.clone(),
                    name: meta.name.clone(),
                    description: meta.description.clone(),
                    supported_formats: meta
                        .supported_formats
                        .iter()
                        .map(|f| format!("{:?}", f))
                        .collect(),
                }
            })
        })
        .collect();

    let total_count = stt.len() + tts.len() + realtime.len() + processors.len();

    Ok(Json(PluginListResponse {
        stt,
        tts,
        realtime,
        processors,
        total_count,
    }))
}

/// List STT providers with optional filtering
pub async fn list_stt_providers(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<ProviderFilterQuery>,
) -> Result<Json<Vec<ProviderInfo>>, StatusCode> {
    let registry = global_registry();
    let names = registry.get_stt_provider_names();

    let providers: Vec<ProviderInfo> = names
        .iter()
        .filter_map(|name| {
            registry.get_stt_metadata(name).map(|meta| {
                provider_metadata_to_info(name, &meta, "stt", registry)
            })
        })
        .filter(|p| filter_provider(p, &query))
        .collect();

    Ok(Json(providers))
}

/// List TTS providers with optional filtering
pub async fn list_tts_providers(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<ProviderFilterQuery>,
) -> Result<Json<Vec<ProviderInfo>>, StatusCode> {
    let registry = global_registry();
    let names = registry.get_tts_provider_names();

    let providers: Vec<ProviderInfo> = names
        .iter()
        .filter_map(|name| {
            registry.get_tts_metadata(name).map(|meta| {
                provider_metadata_to_info(name, &meta, "tts", registry)
            })
        })
        .filter(|p| filter_provider(p, &query))
        .collect();

    Ok(Json(providers))
}

/// List realtime providers with optional filtering
pub async fn list_realtime_providers(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<ProviderFilterQuery>,
) -> Result<Json<Vec<ProviderInfo>>, StatusCode> {
    let registry = global_registry();
    let names = registry.get_realtime_provider_names();

    let providers: Vec<ProviderInfo> = names
        .iter()
        .filter_map(|name| {
            registry.get_realtime_metadata(name).map(|meta| {
                provider_metadata_to_info(name, &meta, "realtime", registry)
            })
        })
        .filter(|p| filter_provider(p, &query))
        .collect();

    Ok(Json(providers))
}

/// List audio processors
pub async fn list_processors(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<ProcessorInfo>>, StatusCode> {
    let registry = global_registry();
    let names = registry.get_audio_processor_names();

    let processors: Vec<ProcessorInfo> = names
        .iter()
        .filter_map(|name| {
            registry.get_audio_processor_metadata(name).map(|meta| {
                ProcessorInfo {
                    id: name.clone(),
                    name: meta.name.clone(),
                    description: meta.description.clone(),
                    supported_formats: meta
                        .supported_formats
                        .iter()
                        .map(|f| format!("{:?}", f))
                        .collect(),
                }
            })
        })
        .collect();

    Ok(Json(processors))
}

/// Get specific provider information by ID
///
/// Searches across all provider types (STT, TTS, realtime) for the given ID.
pub async fn get_provider_info(
    State(_state): State<Arc<AppState>>,
    Path(provider_id): Path<String>,
) -> Result<Json<ProviderInfo>, StatusCode> {
    let registry = global_registry();
    let id = provider_id.to_lowercase();

    // Try STT first
    if let Some(meta) = registry.get_stt_metadata(&id) {
        return Ok(Json(provider_metadata_to_info(&id, &meta, "stt", registry)));
    }

    // Try TTS
    if let Some(meta) = registry.get_tts_metadata(&id) {
        return Ok(Json(provider_metadata_to_info(&id, &meta, "tts", registry)));
    }

    // Try Realtime
    if let Some(meta) = registry.get_realtime_metadata(&id) {
        return Ok(Json(provider_metadata_to_info(&id, &meta, "realtime", registry)));
    }

    Err(StatusCode::NOT_FOUND)
}

/// Get provider health status
pub async fn get_provider_health(
    State(_state): State<Arc<AppState>>,
    Path(provider_id): Path<String>,
) -> Result<Json<ProviderHealthResponse>, StatusCode> {
    let registry = global_registry();
    let id = provider_id.to_lowercase();

    // Check if provider exists
    let exists = registry.has_stt_provider(&id)
        || registry.has_tts_provider(&id)
        || registry.has_realtime_provider(&id);

    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    // Get actual health metrics from plugin entries
    let (health, details) = if let Some(metrics) = registry.get_plugin_metrics(&id) {
        // Determine health status based on error rate
        let health = if metrics.error_rate > 0.5 {
            "unhealthy"
        } else if metrics.error_rate > 0.1 {
            "degraded"
        } else {
            "healthy"
        };

        let details = ProviderHealthDetails {
            call_count: metrics.call_count,
            error_count: metrics.error_count,
            error_rate: metrics.error_rate,
            last_error: metrics.last_error,
            uptime_seconds: metrics.uptime_seconds,
            idle_seconds: metrics.idle_seconds,
        };

        (health.to_string(), details)
    } else {
        // Provider exists but no metrics yet (hasn't been called)
        (
            "healthy".to_string(),
            ProviderHealthDetails {
                call_count: 0,
                error_count: 0,
                error_rate: 0.0,
                last_error: None,
                uptime_seconds: 0,
                idle_seconds: 0,
            },
        )
    };

    Ok(Json(ProviderHealthResponse {
        id: provider_id,
        health,
        details,
    }))
}

// Helper function to convert ProviderMetadata to ProviderInfo
fn provider_metadata_to_info(
    id: &str,
    meta: &crate::plugin::metadata::ProviderMetadata,
    provider_type: &str,
    _registry: &crate::plugin::registry::PluginRegistry,
) -> ProviderInfo {
    // Convert language codes to human-readable names
    let languages: Vec<LanguageInfo> = meta
        .supported_languages
        .iter()
        .map(|code| LanguageInfo {
            code: code.clone(),
            name: language_code_to_name(code),
        })
        .collect();

    // Get features as sorted vec for consistent ordering
    let mut features: Vec<String> = meta.features.iter().cloned().collect();
    features.sort();

    // Get health status (simplified - always healthy if registered)
    let health = "healthy".to_string();

    ProviderInfo {
        id: id.to_string(),
        display_name: meta.display_name.clone(),
        provider_type: provider_type.to_string(),
        description: meta.description.clone(),
        version: meta.version.clone(),
        features,
        languages,
        models: meta.supported_models.clone(),
        aliases: meta.aliases.clone(),
        required_config: meta.required_config_keys.clone(),
        optional_config: meta.optional_config_keys.clone(),
        health,
        metrics: None,
    }
}

// Helper function to filter providers based on query parameters
fn filter_provider(provider: &ProviderInfo, query: &ProviderFilterQuery) -> bool {
    // Filter by language
    if let Some(ref lang) = query.language {
        let lang_lower = lang.to_lowercase();
        let has_language = provider.languages.iter().any(|l| {
            l.code.to_lowercase() == lang_lower || l.name.to_lowercase().contains(&lang_lower)
        });
        if !has_language {
            return false;
        }
    }

    // Filter by feature
    if let Some(ref feature) = query.feature {
        let feature_lower = feature.to_lowercase();
        let has_feature = provider.features.iter().any(|f| {
            f.to_lowercase().contains(&feature_lower)
        });
        if !has_feature {
            return false;
        }
    }

    // Filter by model
    if let Some(ref model) = query.model {
        let model_lower = model.to_lowercase();
        let has_model = provider.models.iter().any(|m| {
            m.to_lowercase().contains(&model_lower)
        });
        if !has_model {
            return false;
        }
    }

    true
}

/// Convert ISO 639-1 language code to human-readable name
///
/// Returns the code itself if not found in the mapping.
fn language_code_to_name(code: &str) -> String {
    match code.to_lowercase().as_str() {
        // Common languages
        "en" | "en-us" | "en-gb" | "en-au" => "English".to_string(),
        "es" | "es-es" | "es-mx" | "es-419" => "Spanish".to_string(),
        "fr" | "fr-fr" | "fr-ca" => "French".to_string(),
        "de" | "de-de" | "de-at" | "de-ch" => "German".to_string(),
        "it" | "it-it" => "Italian".to_string(),
        "pt" | "pt-br" | "pt-pt" => "Portuguese".to_string(),
        "nl" | "nl-nl" | "nl-be" => "Dutch".to_string(),
        "ru" | "ru-ru" => "Russian".to_string(),
        "zh" | "zh-cn" | "zh-tw" | "zh-hk" | "cmn" => "Chinese".to_string(),
        "ja" | "ja-jp" => "Japanese".to_string(),
        "ko" | "ko-kr" => "Korean".to_string(),
        "ar" | "ar-sa" | "ar-eg" => "Arabic".to_string(),
        "hi" | "hi-in" => "Hindi".to_string(),
        "pl" | "pl-pl" => "Polish".to_string(),
        "tr" | "tr-tr" => "Turkish".to_string(),
        "vi" | "vi-vn" => "Vietnamese".to_string(),
        "th" | "th-th" => "Thai".to_string(),
        "id" | "id-id" => "Indonesian".to_string(),
        "ms" | "ms-my" => "Malay".to_string(),
        "uk" | "uk-ua" => "Ukrainian".to_string(),
        "cs" | "cs-cz" => "Czech".to_string(),
        "sv" | "sv-se" => "Swedish".to_string(),
        "da" | "da-dk" => "Danish".to_string(),
        "fi" | "fi-fi" => "Finnish".to_string(),
        "no" | "nb" | "nn" | "no-no" => "Norwegian".to_string(),
        "el" | "el-gr" => "Greek".to_string(),
        "he" | "he-il" | "iw" => "Hebrew".to_string(),
        "ro" | "ro-ro" => "Romanian".to_string(),
        "hu" | "hu-hu" => "Hungarian".to_string(),
        "sk" | "sk-sk" => "Slovak".to_string(),
        "bg" | "bg-bg" => "Bulgarian".to_string(),
        "hr" | "hr-hr" => "Croatian".to_string(),
        "sr" | "sr-rs" => "Serbian".to_string(),
        "sl" | "sl-si" => "Slovenian".to_string(),
        "et" | "et-ee" => "Estonian".to_string(),
        "lv" | "lv-lv" => "Latvian".to_string(),
        "lt" | "lt-lt" => "Lithuanian".to_string(),
        "ca" | "ca-es" => "Catalan".to_string(),
        "eu" | "eu-es" => "Basque".to_string(),
        "gl" | "gl-es" => "Galician".to_string(),
        "ta" | "ta-in" => "Tamil".to_string(),
        "te" | "te-in" => "Telugu".to_string(),
        "mr" | "mr-in" => "Marathi".to_string(),
        "bn" | "bn-in" | "bn-bd" => "Bengali".to_string(),
        "gu" | "gu-in" => "Gujarati".to_string(),
        "kn" | "kn-in" => "Kannada".to_string(),
        "ml" | "ml-in" => "Malayalam".to_string(),
        "pa" | "pa-in" => "Punjabi".to_string(),
        "ur" | "ur-pk" => "Urdu".to_string(),
        "fa" | "fa-ir" => "Persian".to_string(),
        "sw" | "sw-ke" | "sw-tz" => "Swahili".to_string(),
        "af" | "af-za" => "Afrikaans".to_string(),
        "zu" | "zu-za" => "Zulu".to_string(),
        "xh" | "xh-za" => "Xhosa".to_string(),
        "fil" | "tl" => "Filipino".to_string(),
        // Fallback: return the code as-is
        _ => code.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_code_to_name() {
        assert_eq!(language_code_to_name("en"), "English");
        assert_eq!(language_code_to_name("EN-US"), "English");
        assert_eq!(language_code_to_name("es"), "Spanish");
        assert_eq!(language_code_to_name("zh-cn"), "Chinese");
        assert_eq!(language_code_to_name("unknown"), "unknown");
    }

    #[test]
    fn test_filter_provider_by_language() {
        let provider = ProviderInfo {
            id: "test".to_string(),
            display_name: "Test".to_string(),
            provider_type: "stt".to_string(),
            description: "".to_string(),
            version: "1.0.0".to_string(),
            features: vec![],
            languages: vec![
                LanguageInfo { code: "en".to_string(), name: "English".to_string() },
                LanguageInfo { code: "es".to_string(), name: "Spanish".to_string() },
            ],
            models: vec![],
            aliases: vec![],
            required_config: vec![],
            optional_config: vec![],
            health: "healthy".to_string(),
            metrics: None,
        };

        // Should match by code
        let query = ProviderFilterQuery {
            language: Some("en".to_string()),
            feature: None,
            model: None,
        };
        assert!(filter_provider(&provider, &query));

        // Should match by name
        let query = ProviderFilterQuery {
            language: Some("Spanish".to_string()),
            feature: None,
            model: None,
        };
        assert!(filter_provider(&provider, &query));

        // Should not match
        let query = ProviderFilterQuery {
            language: Some("French".to_string()),
            feature: None,
            model: None,
        };
        assert!(!filter_provider(&provider, &query));
    }

    #[test]
    fn test_filter_provider_by_feature() {
        let provider = ProviderInfo {
            id: "test".to_string(),
            display_name: "Test".to_string(),
            provider_type: "stt".to_string(),
            description: "".to_string(),
            version: "1.0.0".to_string(),
            features: vec!["streaming".to_string(), "word-timestamps".to_string()],
            languages: vec![],
            models: vec![],
            aliases: vec![],
            required_config: vec![],
            optional_config: vec![],
            health: "healthy".to_string(),
            metrics: None,
        };

        let query = ProviderFilterQuery {
            language: None,
            feature: Some("streaming".to_string()),
            model: None,
        };
        assert!(filter_provider(&provider, &query));

        let query = ProviderFilterQuery {
            language: None,
            feature: Some("timestamps".to_string()),
            model: None,
        };
        assert!(filter_provider(&provider, &query));

        let query = ProviderFilterQuery {
            language: None,
            feature: Some("diarization".to_string()),
            model: None,
        };
        assert!(!filter_provider(&provider, &query));
    }
}

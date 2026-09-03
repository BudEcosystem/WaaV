use axum::{extract::State, http::StatusCode, response::Json};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

use crate::core::providers::azure::AzureRegion;
use crate::core::providers::google::{
    CredentialSource, GOOGLE_CLOUD_PLATFORM_SCOPE, GoogleAuthClient, TokenProvider,
};
use crate::state::AppState;

/// Maximum decoded bytes for any single base64 voice-clone sample.
const MAX_VOICE_CLONE_SAMPLE_BYTES: usize = 25 * 1024 * 1024;
/// Maximum total decoded bytes across all voice-clone samples in one request.
const MAX_VOICE_CLONE_TOTAL_AUDIO_BYTES: usize = 250 * 1024 * 1024;
/// JSON body budget for base64 clone samples plus request metadata.
pub const VOICE_CLONE_JSON_BODY_LIMIT_BYTES: usize =
    ((MAX_VOICE_CLONE_TOTAL_AUDIO_BYTES + 2) / 3) * 4 + (1024 * 1024);

fn voice_handler_http_client() -> Result<reqwest::Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES).build()
}

fn voice_catalog_http_client() -> Result<reqwest::Client, Box<dyn std::error::Error + Send + Sync>>
{
    voice_handler_http_client()
        .map_err(|e| format!("Failed to create voice handler HTTP client: {e}").into())
}

fn voice_clone_http_client() -> Result<reqwest::Client, VoiceCloneError> {
    voice_handler_http_client().map_err(|e| VoiceCloneError {
        code: "INTERNAL_ERROR".to_string(),
        message: format!("Failed to create voice clone HTTP client: {e}"),
        details: None,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Voice {
    /// Voice ID or canonical name
    #[cfg_attr(feature = "openapi", schema(example = "aura-asteria-en"))]
    pub id: String,
    /// URL to sample audio
    #[cfg_attr(
        feature = "openapi",
        schema(example = "https://example.com/sample.mp3")
    )]
    pub sample: String,
    /// Display name of the voice
    #[cfg_attr(feature = "openapi", schema(example = "Asteria"))]
    pub name: String,
    /// Accent or dialect
    #[cfg_attr(feature = "openapi", schema(example = "American"))]
    pub accent: String,
    /// Gender of the voice
    #[cfg_attr(feature = "openapi", schema(example = "Female"))]
    pub gender: String,
    /// Language supported by the voice
    #[cfg_attr(feature = "openapi", schema(example = "English"))]
    pub language: String,
}

pub type VoicesResponse = HashMap<String, Vec<Voice>>;

// ElevenLabs API response structures
#[derive(Debug, Deserialize)]
struct ElevenLabsVoicesResponse {
    voices: Vec<ElevenLabsVoice>,
}

#[derive(Debug, Deserialize)]
struct ElevenLabsVoice {
    voice_id: String,
    name: String,
    preview_url: Option<String>,
    description: Option<String>,
    labels: Option<HashMap<String, String>>,
    verified_languages: Option<Vec<ElevenLabsLanguage>>,
}

#[derive(Debug, Deserialize)]
struct ElevenLabsLanguage {
    language: String,
    accent: Option<String>,
}

// Deepgram API response structures
#[derive(Debug, Deserialize)]
struct DeepgramModelsResponse {
    tts: Option<Vec<DeepgramTtsModel>>,
}

#[derive(Debug, Deserialize)]
struct DeepgramTtsModel {
    name: String,
    canonical_name: String,
    languages: Vec<String>,
    metadata: Option<DeepgramMetadata>,
}

#[derive(Debug, Deserialize)]
struct DeepgramMetadata {
    accent: Option<String>,
    sample: Option<String>,
    tags: Option<Vec<String>>,
}

// Google TTS API response structures
#[derive(Debug, Deserialize)]
struct GoogleVoicesResponse {
    voices: Option<Vec<GoogleVoice>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleVoice {
    language_codes: Vec<String>,
    name: String,
    ssml_gender: Option<String>,
}

// LMNT API response structures
#[derive(Debug, Deserialize)]
struct LmntVoice {
    id: String,
    name: String,
    owner: String,
    state: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    gender: Option<String>,
    #[serde(default)]
    preview_url: Option<String>,
}

// Azure TTS Voices API response structures
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AzureVoice {
    /// Full voice name, e.g., "Microsoft Server Speech Text to Speech Voice (en-US, JennyNeural)"
    #[allow(dead_code)]
    name: String,
    /// Display name, e.g., "Jenny"
    display_name: String,
    /// Short name used as voice ID, e.g., "en-US-JennyNeural"
    short_name: String,
    /// Gender: "Female" or "Male"
    gender: String,
    /// Locale code, e.g., "en-US"
    locale: String,
    /// Voice type, e.g., "Neural"
    #[allow(dead_code)]
    voice_type: String,
}

/// Maps a language code (e.g., "en-US") to a human-readable language name.
fn language_code_to_name(code: &str) -> String {
    // Extract the primary language code (e.g., "en" from "en-US")
    let primary = code.split('-').next().unwrap_or(code);

    match primary {
        "af" => "Afrikaans",
        "am" => "Amharic",
        "ar" => "Arabic",
        "bg" => "Bulgarian",
        "bn" => "Bengali",
        "ca" => "Catalan",
        "cmn" | "zh" => "Chinese",
        "cs" => "Czech",
        "cy" => "Welsh",
        "da" => "Danish",
        "de" => "German",
        "el" => "Greek",
        "en" => "English",
        "es" => "Spanish",
        "et" => "Estonian",
        "eu" => "Basque",
        "fa" => "Persian",
        "fi" => "Finnish",
        "fil" => "Filipino",
        "fr" => "French",
        "ga" => "Irish",
        "gl" => "Galician",
        "gu" => "Gujarati",
        "he" | "iw" => "Hebrew",
        "hi" => "Hindi",
        "hr" => "Croatian",
        "hu" => "Hungarian",
        "id" => "Indonesian",
        "is" => "Icelandic",
        "it" => "Italian",
        "ja" => "Japanese",
        "jv" => "Javanese",
        "kn" => "Kannada",
        "ko" => "Korean",
        "lt" => "Lithuanian",
        "lv" => "Latvian",
        "ml" => "Malayalam",
        "mr" => "Marathi",
        "ms" => "Malay",
        "nb" => "Norwegian Bokmål",
        "nl" => "Dutch",
        "pa" => "Punjabi",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sk" => "Slovak",
        "sl" => "Slovenian",
        "sr" => "Serbian",
        "su" => "Sundanese",
        "sv" => "Swedish",
        "sw" => "Swahili",
        "ta" => "Tamil",
        "te" => "Telugu",
        "th" => "Thai",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "ur" => "Urdu",
        "vi" => "Vietnamese",
        "yue" => "Cantonese",
        _ => code, // Return the code itself if unknown
    }
    .to_string()
}

/// Extracts accent/region from a language code (e.g., "US" from "en-US").
fn extract_accent_from_code(code: &str) -> String {
    let parts: Vec<&str> = code.split('-').collect();
    if parts.len() >= 2 {
        // Map region codes to readable names
        match parts[1].to_uppercase().as_str() {
            "US" => "American",
            "GB" => "British",
            "AU" => "Australian",
            "IN" => "Indian",
            "CA" => "Canadian",
            "IE" => "Irish",
            "NZ" => "New Zealand",
            "ZA" => "South African",
            "ES" => "Spain",
            "MX" => "Mexican",
            "AR" => "Argentinian",
            "CL" => "Chilean",
            "CO" => "Colombian",
            "PE" => "Peruvian",
            "VE" => "Venezuelan",
            "BR" => "Brazilian",
            "PT" => "Portuguese",
            "FR" => "French",
            "BE" => "Belgian",
            "CH" => "Swiss",
            "DE" => "German",
            "AT" => "Austrian",
            "IT" => "Italian",
            "CN" => "Mainland China",
            "TW" => "Taiwanese",
            "HK" => "Hong Kong",
            "JP" => "Japanese",
            "KR" => "Korean",
            "RU" => "Russian",
            "UA" => "Ukrainian",
            "PL" => "Polish",
            "NL" => "Dutch",
            "SE" => "Swedish",
            "NO" => "Norwegian",
            "DK" => "Danish",
            "FI" => "Finnish",
            "TR" => "Turkish",
            "SA" => "Saudi",
            "EG" => "Egyptian",
            "IL" => "Israeli",
            "PH" => "Filipino",
            "ID" => "Indonesian",
            "MY" => "Malaysian",
            "TH" => "Thai",
            "VN" => "Vietnamese",
            _ => parts[1],
        }
        .to_string()
    } else {
        "Standard".to_string()
    }
}

// Helper function to fetch voices from ElevenLabs API
async fn fetch_elevenlabs_voices(
    api_key: &str,
) -> Result<Vec<Voice>, Box<dyn std::error::Error + Send + Sync>> {
    let client = voice_catalog_http_client()?;

    let response = client
        .get("https://api.elevenlabs.io/v2/voices")
        .header("xi-api-key", api_key)
        .send()
        .await?;

    let elevenlabs_response: ElevenLabsVoicesResponse = response.json().await?;

    let voices = elevenlabs_response
        .voices
        .into_iter()
        .map(|voice| {
            // Extract language and accent information from verified_languages
            let (language, accent) = if let Some(verified_languages) = &voice.verified_languages {
                if let Some(first_lang) = verified_languages.first() {
                    (
                        first_lang.language.clone(),
                        first_lang
                            .accent
                            .clone()
                            .unwrap_or_else(|| "Unknown".to_string()),
                    )
                } else {
                    ("Unknown".to_string(), "Unknown".to_string())
                }
            } else {
                ("Unknown".to_string(), "Unknown".to_string())
            };

            // Extract gender from labels or description
            let gender = voice
                .labels
                .as_ref()
                .and_then(|labels| {
                    // Check common gender keys in labels
                    for key in ["gender", "sex", "voice_type"] {
                        if let Some(value) = labels.get(key) {
                            let value_lower = value.to_lowercase();
                            if value_lower.contains("male") && !value_lower.contains("female") {
                                return Some("Male".to_string());
                            }
                            if value_lower.contains("female") && !value_lower.contains("male") {
                                return Some("Female".to_string());
                            }
                        }
                    }
                    None
                })
                .or_else(|| {
                    // Check description for gender keywords
                    voice.description.as_ref().and_then(|desc| {
                        let desc_lower = desc.to_lowercase();
                        if (desc_lower.contains("male") && !desc_lower.contains("female"))
                            || desc_lower.contains("masculine")
                            || desc_lower.contains(" man ")
                            || desc_lower.contains("gentleman")
                        {
                            Some("Male".to_string())
                        } else if (desc_lower.contains("female") && !desc_lower.contains("male"))
                            || desc_lower.contains("feminine")
                            || desc_lower.contains(" woman ")
                            || desc_lower.contains("lady")
                        {
                            Some("Female".to_string())
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or_else(|| "Unknown".to_string());

            Voice {
                id: voice.voice_id,
                sample: voice.preview_url.unwrap_or_default(),
                name: voice.name,
                accent,
                gender,
                language,
            }
        })
        .collect();

    Ok(voices)
}

// Helper function to fetch voices from Deepgram API
async fn fetch_deepgram_voices(
    api_key: &str,
) -> Result<Vec<Voice>, Box<dyn std::error::Error + Send + Sync>> {
    let client = voice_catalog_http_client()?;

    let response = client
        .get("https://api.deepgram.com/v1/models")
        .header("Authorization", format!("Token {api_key}"))
        .send()
        .await?;

    let deepgram_response: DeepgramModelsResponse = response.json().await?;

    let voices = deepgram_response
        .tts
        .unwrap_or_default()
        .into_iter()
        .map(|model| {
            let metadata = model.metadata.as_ref();

            // Extract accent
            let accent = metadata
                .and_then(|m| m.accent.clone())
                .unwrap_or_else(|| "Unknown".to_string());

            // Extract sample URL
            let sample = metadata.and_then(|m| m.sample.clone()).unwrap_or_default();

            // Determine gender from tags
            let gender = metadata
                .and_then(|m| m.tags.as_ref())
                .and_then(|tags| {
                    for tag in tags {
                        let tag_lower = tag.to_lowercase();
                        if tag_lower.contains("masculine") || tag_lower.contains("male") {
                            return Some("Male".to_string());
                        }
                        if tag_lower.contains("feminine") || tag_lower.contains("female") {
                            return Some("Female".to_string());
                        }
                    }
                    None
                })
                .unwrap_or_else(|| "Unknown".to_string());

            // Extract language (use first available language)
            let language = model
                .languages
                .first()
                .map(|lang| {
                    // Convert language codes like "en" or "en-US" to readable format
                    if lang.starts_with("en") {
                        "English".to_string()
                    } else {
                        lang.clone()
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string());

            Voice {
                id: model.canonical_name,
                sample,
                name: model.name,
                accent,
                gender,
                language,
            }
        })
        .collect();

    Ok(voices)
}

// Helper function to fetch voices from Google TTS API
async fn fetch_google_voices(
    credentials: &str,
) -> Result<Vec<Voice>, Box<dyn std::error::Error + Send + Sync>> {
    // Create credential source and auth client
    let credential_source = CredentialSource::from_api_key(credentials);
    let auth_client = GoogleAuthClient::new(credential_source, &[GOOGLE_CLOUD_PLATFORM_SCOPE])?;

    // Get OAuth2 token
    let token = auth_client.get_token().await?;

    let client = voice_catalog_http_client()?;

    let response = client
        .get("https://texttospeech.googleapis.com/v1/voices")
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("Google TTS API error ({}): {}", status, error_body).into());
    }

    let google_response: GoogleVoicesResponse = response.json().await?;

    let voices = google_response
        .voices
        .unwrap_or_default()
        .into_iter()
        .map(|voice| {
            // Use first language code for language and accent
            let primary_lang = voice.language_codes.first().cloned().unwrap_or_default();
            let language = language_code_to_name(&primary_lang);
            let accent = extract_accent_from_code(&primary_lang);

            // Map SSML gender to our format
            let gender = match voice.ssml_gender.as_deref() {
                Some("MALE") => "Male".to_string(),
                Some("FEMALE") => "Female".to_string(),
                Some("NEUTRAL") => "Neutral".to_string(),
                _ => "Unknown".to_string(),
            };

            // Extract display name from voice name (e.g., "en-US-Wavenet-D" -> "Wavenet D")
            let display_name = voice
                .name
                .split('-')
                .skip(2) // Skip language and region
                .collect::<Vec<&str>>()
                .join(" ");
            let display_name = if display_name.is_empty() {
                voice.name.clone()
            } else {
                display_name
            };

            Voice {
                id: voice.name,
                sample: String::new(), // Google TTS doesn't provide sample URLs
                name: display_name,
                accent,
                gender,
                language,
            }
        })
        .collect();

    Ok(voices)
}

// Helper function to fetch voices from Azure TTS API
async fn fetch_azure_voices(
    subscription_key: &str,
    region: &str,
) -> Result<Vec<Voice>, Box<dyn std::error::Error + Send + Sync>> {
    let client = voice_catalog_http_client()?;

    let url = azure_voices_list_url(region)
        .map_err(|msg| -> Box<dyn std::error::Error + Send + Sync> { msg.into() })?;

    let response = client
        .get(&url)
        .header("Ocp-Apim-Subscription-Key", subscription_key)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("Azure TTS API error ({}): {}", status, error_body).into());
    }

    let azure_voices: Vec<AzureVoice> = response.json().await?;

    let voices = azure_voices
        .into_iter()
        .map(|voice| {
            let language = language_code_to_name(&voice.locale);
            let accent = extract_accent_from_code(&voice.locale);

            Voice {
                id: voice.short_name,
                sample: String::new(), // Azure doesn't provide sample URLs in this API
                name: voice.display_name,
                accent,
                gender: voice.gender,
                language,
            }
        })
        .collect();

    Ok(voices)
}

fn azure_voices_list_url(region: &str) -> Result<String, String> {
    let region = region
        .parse::<AzureRegion>()
        .map_err(|msg| format!("Azure TTS region rejected (SSRF protection): {msg}"))?;
    Ok(region.voices_list_url())
}

// Helper function to fetch voices from LMNT API
async fn fetch_lmnt_voices(
    api_key: &str,
) -> Result<Vec<Voice>, Box<dyn std::error::Error + Send + Sync>> {
    let client = voice_catalog_http_client()?;

    // LMNT voice list endpoint
    let response = client
        .get("https://api.lmnt.com/v1/ai/voice/list")
        .header("X-API-Key", api_key)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("LMNT API error ({}): {}", status, error_body).into());
    }

    let lmnt_voices: Vec<LmntVoice> = response.json().await?;

    let voices = lmnt_voices
        .into_iter()
        .filter(|v| v.state == "ready") // Only include ready voices
        .map(|voice| {
            // Extract gender from the gender field or description
            let gender = voice
                .gender
                .clone()
                .map(|g| {
                    let g_lower = g.to_lowercase();
                    if g_lower.contains("male") && !g_lower.contains("female") {
                        "Male".to_string()
                    } else if g_lower.contains("female") {
                        "Female".to_string()
                    } else {
                        g
                    }
                })
                .or_else(|| {
                    voice.description.as_ref().and_then(|desc| {
                        let desc_lower = desc.to_lowercase();
                        if desc_lower.contains("male") && !desc_lower.contains("female") {
                            Some("Male".to_string())
                        } else if desc_lower.contains("female") {
                            Some("Female".to_string())
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or_else(|| "Unknown".to_string());

            // Determine accent based on owner type
            let accent = match voice.owner.as_str() {
                "system" => "Standard".to_string(),
                "me" => "Custom".to_string(),
                _ => "Shared".to_string(),
            };

            Voice {
                id: voice.id,
                sample: voice.preview_url.unwrap_or_default(),
                name: voice.name,
                accent,
                gender,
                language: "English".to_string(), // LMNT supports 22+ languages, default to English
            }
        })
        .collect();

    Ok(voices)
}

/// Handler for GET /voices - returns available voices per provider
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/voices",
        responses(
            (status = 200, description = "Available voices grouped by provider", body = HashMap<String, Vec<Voice>>),
            (status = 500, description = "Internal server error")
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "voices"
    )
)]
pub async fn list_voices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<VoicesResponse>, StatusCode> {
    let mut voices_response = HashMap::new();

    // Fetch ElevenLabs voices - skip if not configured
    if let Ok(api_key) = state.config.get_api_key("elevenlabs") {
        match fetch_elevenlabs_voices(&api_key).await {
            Ok(voices) => {
                voices_response.insert("elevenlabs".to_string(), voices);
            }
            Err(e) => {
                tracing::warn!("Failed to fetch ElevenLabs voices: {}", e);
            }
        }
    } else {
        tracing::debug!("ElevenLabs API key not configured, skipping");
    }

    // Fetch Deepgram voices - skip if not configured
    if let Ok(api_key) = state.config.get_api_key("deepgram") {
        match fetch_deepgram_voices(&api_key).await {
            Ok(voices) => {
                voices_response.insert("deepgram".to_string(), voices);
            }
            Err(e) => {
                tracing::warn!("Failed to fetch Deepgram voices: {}", e);
            }
        }
    } else {
        tracing::debug!("Deepgram API key not configured, skipping");
    }

    // Fetch Google TTS voices - skip if not configured
    // Note: Google returns empty string for ADC which is valid
    if let Ok(credentials) = state.config.get_api_key("google") {
        match fetch_google_voices(&credentials).await {
            Ok(voices) => {
                voices_response.insert("google".to_string(), voices);
            }
            Err(e) => {
                tracing::warn!("Failed to fetch Google TTS voices: {}", e);
            }
        }
    } else {
        tracing::debug!("Google credentials not configured, skipping");
    }

    // Fetch Azure TTS voices - skip if not configured
    if let Ok(subscription_key) = state.config.get_api_key("microsoft-azure") {
        let region = state.config.get_azure_speech_region();
        match fetch_azure_voices(&subscription_key, &region).await {
            Ok(voices) => {
                voices_response.insert("azure".to_string(), voices);
            }
            Err(e) => {
                tracing::warn!("Failed to fetch Azure TTS voices: {}", e);
            }
        }
    } else {
        tracing::debug!("Azure Speech credentials not configured, skipping");
    }

    // Fetch LMNT voices - skip if not configured
    if let Ok(api_key) = state.config.get_api_key("lmnt") {
        match fetch_lmnt_voices(&api_key).await {
            Ok(voices) => {
                voices_response.insert("lmnt".to_string(), voices);
            }
            Err(e) => {
                tracing::warn!("Failed to fetch LMNT voices: {}", e);
            }
        }
    } else {
        tracing::debug!("LMNT API key not configured, skipping");
    }

    Ok(Json(voices_response))
}

// =============================================================================
// Server-side voice-descriptor resolution support (P4)
// =============================================================================

/// TTL for the per-provider voice-catalog cache used by descriptor resolution.
/// The catalog changes rarely; a 10-minute cache avoids a live provider hit on
/// every session that uses a [`crate::core::voice::VoiceDescriptor`].
const VOICE_CATALOG_CACHE_TTL_SECS: u64 = 600;

/// The provider's DEFAULT `voice_id`, returned by descriptor resolution when no
/// catalog voice matches (the hard "never a 400" requirement). These mirror each
/// provider's documented default voice.
pub(crate) fn provider_default_voice(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "deepgram" => "aura-2-thalia-en",
        "elevenlabs" | "eleven_labs" => "21m00Tcm4TlvDq8ikWAM", // Rachel
        "azure" | "microsoft-azure" | "microsoft_azure" => "en-US-JennyNeural",
        "google" | "google-tts" => "en-US-Standard-C",
        "lmnt" => "lily",
        "cartesia" => "a0e99841-438c-4a64-b679-ae501e7d6091",
        "openai" | "openai-tts" => "alloy",
        "hume" => "",
        _ => "",
    }
}

/// Fetch the voice catalog for a SINGLE provider (cached). Returns an empty Vec
/// when the provider is not configured / unreachable / has no catalog endpoint —
/// the resolver maps empty → provider default + warning, so this never errors.
pub(crate) async fn fetch_provider_catalog(state: &Arc<AppState>, provider: &str) -> Vec<Voice> {
    let provider_key = provider.to_lowercase();
    let cache_key = format!("voice_catalog:{provider_key}");

    // Cache hit?
    if let Ok(Some(bytes)) = state.core_state.cache.get(&cache_key).await
        && let Ok(voices) = serde_json::from_slice::<Vec<Voice>>(&bytes)
    {
        return voices;
    }

    // Cache miss → fetch live for the one provider.
    let voices: Vec<Voice> = match provider_key.as_str() {
        "elevenlabs" | "eleven_labs" => match state.config.get_api_key("elevenlabs") {
            Ok(key) => fetch_elevenlabs_voices(&key).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        },
        "deepgram" => match state.config.get_api_key("deepgram") {
            Ok(key) => fetch_deepgram_voices(&key).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        },
        "google" | "google-tts" => match state.config.get_api_key("google") {
            Ok(creds) => fetch_google_voices(&creds).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        },
        "azure" | "microsoft-azure" | "microsoft_azure" => {
            match state.config.get_api_key("microsoft-azure") {
                Ok(key) => {
                    let region = state.config.get_azure_speech_region();
                    fetch_azure_voices(&key, &region).await.unwrap_or_default()
                }
                Err(_) => Vec::new(),
            }
        }
        "lmnt" => match state.config.get_api_key("lmnt") {
            Ok(key) => fetch_lmnt_voices(&key).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    };

    // Cache the result (even empty — short-circuits repeated misses within the TTL).
    if let Ok(bytes) = serde_json::to_vec(&voices) {
        let _ = state
            .core_state
            .cache
            .put_with_ttl(
                &cache_key,
                bytes,
                std::time::Duration::from_secs(VOICE_CATALOG_CACHE_TTL_SECS),
            )
            .await;
    }

    voices
}

// =============================================================================
// Voice Cloning Types
// =============================================================================

/// Voice cloning provider selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum VoiceCloneProvider {
    /// Hume AI Octave voice DESIGN (description→save; not audio cloning).
    Hume,
    /// ElevenLabs — instant IVC, or professional PVC (async) when `mode=professional`.
    ElevenLabs,
    /// LMNT instant voice cloning (5+ seconds of audio).
    Lmnt,
    /// Cartesia instant clip-mode clone.
    Cartesia,
    /// PlayHT — instant or professional (async) cloning.
    #[serde(rename = "playht")]
    PlayHt,
    /// Speechify instant clone (consent REQUIRED).
    Speechify,
    /// Resemble AI professional clone (async; consent/voice-talent proof required).
    Resemble,
}

impl std::fmt::Display for VoiceCloneProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hume => write!(f, "hume"),
            Self::ElevenLabs => write!(f, "elevenlabs"),
            Self::Lmnt => write!(f, "lmnt"),
            Self::Cartesia => write!(f, "cartesia"),
            Self::PlayHt => write!(f, "playht"),
            Self::Speechify => write!(f, "speechify"),
            Self::Resemble => write!(f, "resemble"),
        }
    }
}

impl VoiceCloneProvider {
    /// The provider's config/credentials key (for `get_api_key`).
    fn credential_key(&self) -> &'static str {
        match self {
            Self::Hume => "hume",
            Self::ElevenLabs => "elevenlabs",
            Self::Lmnt => "lmnt",
            Self::Cartesia => "cartesia",
            Self::PlayHt => "playht",
            Self::Speechify => "speechify",
            Self::Resemble => "resemble",
        }
    }

    /// Whether this provider's [`CloneMode::Professional`] path is an ASYNC job
    /// (returns a non-`ready` status that must be polled).
    fn supports_professional(&self) -> bool {
        matches!(self, Self::ElevenLabs | Self::PlayHt | Self::Resemble)
    }
}

/// Voice-clone mode: instant (near-immediate, usable now) vs professional
/// (high-fidelity, ASYNC — returns a queued/training status to poll).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum CloneMode {
    /// Instant cloning (ElevenLabs IVC / Cartesia clip / PlayHT instant / LMNT /
    /// Speechify). Returns `ready` immediately or near-instantly.
    #[default]
    Instant,
    /// Professional cloning (ElevenLabs PVC / Resemble / PlayHT PVC). ASYNC — the
    /// returned `voice_id` is polled until `ready`.
    Professional,
}

impl std::fmt::Display for CloneMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Instant => write!(f, "instant"),
            Self::Professional => write!(f, "professional"),
        }
    }
}

/// Canonical voice-clone lifecycle status (returned + polled).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum CloneStatus {
    /// Usable now — the `voice_id` works as a TTS `voice_id`.
    Ready,
    /// Awaiting verification (captcha / voice-talent proof) before training.
    Verifying,
    /// Training/fine-tuning in progress.
    Training,
    /// Queued for processing.
    Queued,
    /// Terminal failure.
    Failed,
}

impl CloneStatus {
    /// Canonical lowercase token.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Verifying => "verifying",
            Self::Training => "training",
            Self::Queued => "queued",
            Self::Failed => "failed",
        }
    }

    /// Maps an ElevenLabs PVC `fine_tuning.state` to the canonical status.
    /// `not_verified`→verifying, `queued`/`not_started`→queued,
    /// `fine_tuning`/`delayed`→training, `fine_tuned`→ready, `failed`→failed,
    /// `draft`→queued (created, samples not yet trained).
    pub fn from_elevenlabs_fine_tuning_state(state: &str) -> Self {
        match state.to_lowercase().as_str() {
            "fine_tuned" => Self::Ready,
            "not_verified" => Self::Verifying,
            "queued" | "not_started" | "draft" => Self::Queued,
            "fine_tuning" | "delayed" => Self::Training,
            "failed" => Self::Failed,
            _ => Self::Queued,
        }
    }

    /// Maps a Resemble voice `status` string to the canonical status.
    pub fn from_resemble_status(status: &str) -> Self {
        match status.to_lowercase().as_str() {
            "ready" | "completed" | "finished" => Self::Ready,
            "training" | "processing" | "building" => Self::Training,
            "queued" | "pending" | "created" => Self::Queued,
            "failed" | "error" => Self::Failed,
            _ => Self::Queued,
        }
    }
}

impl std::fmt::Display for CloneStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Structured labels for a cloned voice (canonical; the flat `labels` HashMap is
/// still accepted for backward compatibility and merged with these).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CloneLabels {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gender: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age: Option<String>,
}

impl CloneLabels {
    /// Whether any structured label is set.
    fn is_set(&self) -> bool {
        self.language.is_some()
            || self.accent.is_some()
            || self.gender.is_some()
            || self.age.is_some()
    }

    /// Render to the flat `{key: value}` map ElevenLabs / LMNT expect.
    fn to_map(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        if let Some(v) = &self.language {
            m.insert("language".to_string(), v.clone());
        }
        if let Some(v) = &self.accent {
            m.insert("accent".to_string(), v.clone());
        }
        if let Some(v) = &self.gender {
            m.insert("gender".to_string(), v.clone());
        }
        if let Some(v) = &self.age {
            m.insert("age".to_string(), v.clone());
        }
        m
    }
}

/// Consent/verification block. MANDATORY for Speechify (`full_name` + `email`);
/// the `verified` flag gates the ElevenLabs-PVC / Resemble verification step.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CloneConsent {
    /// Whether the caller affirms they own/represent the voice (gates PVC/Resemble).
    #[serde(default)]
    pub verified: bool,
    /// Optional consent statement text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
    /// Full name of the consenting voice owner (Speechify requires this).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    /// Email of the consenting voice owner (Speechify requires this).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Request body for voice cloning endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VoiceCloneRequest {
    /// Provider to use for voice cloning.
    #[cfg_attr(feature = "openapi", schema(example = "elevenlabs"))]
    pub provider: VoiceCloneProvider,

    /// Name for the cloned voice.
    #[cfg_attr(feature = "openapi", schema(example = "My Custom Voice"))]
    pub name: String,

    /// Description of the voice (used by Hume for voice design).
    /// For ElevenLabs, this becomes the voice description label.
    #[cfg_attr(
        feature = "openapi",
        schema(example = "A warm, friendly voice with a slight accent")
    )]
    pub description: Option<String>,

    /// Audio samples for voice cloning (base64-encoded).
    /// ElevenLabs: Supports mp3, wav, m4a formats. 1-2 minutes recommended.
    /// Hume: Optional - if provided, used for instant cloning; otherwise uses description.
    #[serde(default)]
    #[cfg_attr(
        feature = "openapi",
        schema(example = json!(["base64_encoded_audio_data"]))
    )]
    pub audio_samples: Vec<String>,

    /// Sample text for voice generation (Hume only).
    /// Used when generating voice from description without audio samples.
    #[cfg_attr(
        feature = "openapi",
        schema(example = "Hello, this is a sample of my voice.")
    )]
    pub sample_text: Option<String>,

    /// Remove background noise from samples (ElevenLabs IVC / LMNT `enhance`).
    #[serde(default)]
    pub remove_background_noise: bool,

    /// Flat labels for the voice (ElevenLabs). Backward-compatible; merged with
    /// the structured [`Self::structured_labels`].
    #[serde(default)]
    pub labels: Option<HashMap<String, String>>,

    /// Clone MODE: `instant` (default) or `professional` (async high-fidelity).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(example = "instant"))]
    pub mode: CloneMode,

    /// Canonical structured labels `{language, accent, gender, age}` (preferred over
    /// the flat `labels` map; the two are merged, structured winning on conflict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_labels: Option<CloneLabels>,

    /// Design-from-existing: the base voice to clone/derive from (provider-specific).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_voice_id: Option<String>,

    /// Consent/verification block. MANDATORY for Speechify; the `verified` flag gates
    /// the ElevenLabs-PVC / Resemble verification step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<CloneConsent>,
}

impl VoiceCloneRequest {
    /// Merge the flat `labels` map and the structured `structured_labels` into one
    /// `{key: value}` map (structured wins on key conflict). Empty → `None`.
    fn merged_labels(&self) -> Option<HashMap<String, String>> {
        let mut merged = self.labels.clone().unwrap_or_default();
        if let Some(s) = self.structured_labels.as_ref().filter(|s| s.is_set()) {
            merged.extend(s.to_map());
        }
        if merged.is_empty() {
            None
        } else {
            Some(merged)
        }
    }

    /// Validate the request against per-provider/mode requirements WITHOUT a network
    /// call. Returns the first violation as a [`VoiceCloneError`] (→ 400), else `Ok`.
    /// This is the unit-testable consent / mode / sample gate.
    pub(crate) fn validate(&self) -> Result<(), VoiceCloneError> {
        if self.name.trim().is_empty() {
            return Err(VoiceCloneError {
                code: "INVALID_NAME".to_string(),
                message: "Voice name cannot be empty".to_string(),
                details: None,
            });
        }

        // Hume's public REST API is voice DESIGN, not audio-sample cloning. Validate
        // this before credential resolution so malformed client requests are 400s
        // instead of being masked by missing API-key errors or deferred provider calls.
        if self.provider == VoiceCloneProvider::Hume {
            if !self.audio_samples.is_empty() {
                return Err(VoiceCloneError {
                    code: "AUDIO_SAMPLES_NOT_SUPPORTED".to_string(),
                    message: "Hume API does not support audio-based voice cloning via REST API; \
                              use the description field for voice design"
                        .to_string(),
                    details: Some(serde_json::json!({
                        "hint": "Provide a non-empty 'description' field with natural language voice characteristics"
                    })),
                });
            }

            if self
                .description
                .as_deref()
                .is_none_or(|d| d.trim().is_empty())
            {
                return Err(VoiceCloneError {
                    code: "MISSING_DESCRIPTION".to_string(),
                    message: "Hume voice design requires a non-empty 'description' field"
                        .to_string(),
                    details: Some(serde_json::json!({
                        "hint": "Describe the voice you want to create"
                    })),
                });
            }
        }

        // Speechify mandates consent {full_name, email} at create time.
        if self.provider == VoiceCloneProvider::Speechify {
            let ok = self
                .consent
                .as_ref()
                .is_some_and(|c| c.full_name.is_some() && c.email.is_some());
            if !ok {
                return Err(VoiceCloneError {
                    code: "CONSENT_REQUIRED".to_string(),
                    message: "Speechify voice cloning requires consent with full_name and email \
                              (you must own or represent the voice)"
                        .to_string(),
                    details: Some(serde_json::json!({
                        "required": ["consent.full_name", "consent.email"]
                    })),
                });
            }
        }

        // Professional mode is only an async job on providers that offer it.
        if self.mode == CloneMode::Professional && !self.provider.supports_professional() {
            return Err(VoiceCloneError {
                code: "PROFESSIONAL_UNSUPPORTED".to_string(),
                message: format!(
                    "provider '{}' does not offer professional cloning; use mode=instant",
                    self.provider
                ),
                details: None,
            });
        }

        if self.audio_samples.is_empty() {
            let missing_audio = match self.provider {
                VoiceCloneProvider::ElevenLabs if self.mode == CloneMode::Professional => {
                    Some("ElevenLabs PVC requires audio samples (30-60 min recommended)")
                }
                VoiceCloneProvider::ElevenLabs => {
                    Some("ElevenLabs voice cloning requires at least one audio sample")
                }
                VoiceCloneProvider::Lmnt => {
                    Some("LMNT voice cloning requires at least one audio sample (5+ seconds)")
                }
                VoiceCloneProvider::Cartesia => {
                    Some("Cartesia voice cloning requires one audio clip (~5-20s)")
                }
                VoiceCloneProvider::PlayHt => {
                    Some("PlayHT voice cloning requires at least one audio sample")
                }
                VoiceCloneProvider::Speechify => {
                    Some("Speechify voice cloning requires one audio sample (10-30s)")
                }
                VoiceCloneProvider::Hume | VoiceCloneProvider::Resemble => None,
            };

            if let Some(message) = missing_audio {
                return Err(VoiceCloneError {
                    code: "MISSING_AUDIO".to_string(),
                    message: message.to_string(),
                    details: Some(serde_json::json!({
                        "provider": self.provider.to_string(),
                        "mode": self.mode.to_string(),
                    })),
                });
            }
        }

        if self.provider == VoiceCloneProvider::Lmnt && self.audio_samples.len() > 20 {
            return Err(VoiceCloneError {
                code: "TOO_MANY_FILES".to_string(),
                message: format!(
                    "LMNT supports max 20 audio files, got {}",
                    self.audio_samples.len()
                ),
                details: None,
            });
        }

        validate_voice_clone_audio_size_limits(&self.audio_samples)?;

        // ElevenLabs/Resemble professional require the consent.verified gate before
        // the verification/training step.
        if self.mode == CloneMode::Professional
            && matches!(
                self.provider,
                VoiceCloneProvider::ElevenLabs | VoiceCloneProvider::Resemble
            )
            && !self.consent.as_ref().is_some_and(|c| c.verified)
        {
            return Err(VoiceCloneError {
                code: "CONSENT_NOT_VERIFIED".to_string(),
                message: format!(
                    "professional cloning on '{}' requires consent.verified=true \
                     (voice-talent / captcha verification gates training)",
                    self.provider
                ),
                details: None,
            });
        }

        Ok(())
    }
}

/// Response from voice cloning endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VoiceCloneResponse {
    /// Unique identifier for the cloned voice.
    #[cfg_attr(feature = "openapi", schema(example = "voice_abc123"))]
    pub voice_id: String,

    /// Name of the cloned voice.
    #[cfg_attr(feature = "openapi", schema(example = "My Custom Voice"))]
    pub name: String,

    /// Provider that created the voice.
    #[cfg_attr(feature = "openapi", schema(example = "elevenlabs"))]
    pub provider: VoiceCloneProvider,

    /// Canonical lifecycle status: `ready` | `verifying` | `training` | `queued` |
    /// `failed`. A `ready` voice_id is directly usable as a TTS `voice_id`.
    #[cfg_attr(feature = "openapi", schema(example = "ready"))]
    pub status: String,

    /// Whether the provider flagged that this voice still needs verification before
    /// it can be used (ElevenLabs IVC `requires_verification`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_verification: Option<bool>,

    /// Timestamp when the voice was created.
    #[cfg_attr(feature = "openapi", schema(example = "2026-01-06T12:00:00Z"))]
    pub created_at: String,

    /// Additional metadata from the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Error response for voice cloning.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VoiceCloneError {
    /// Error code.
    pub code: String,
    /// Error message.
    pub message: String,
    /// Additional details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

// =============================================================================
// ElevenLabs Voice Cloning
// =============================================================================

/// ElevenLabs voice creation response.
#[derive(Debug, Deserialize)]
struct ElevenLabsVoiceCreateResponse {
    voice_id: String,
    name: Option<String>,
    #[serde(default)]
    requires_verification: Option<bool>,
}

/// Clone a voice using ElevenLabs API.
async fn clone_voice_elevenlabs(
    api_key: &str,
    request: &VoiceCloneRequest,
) -> Result<VoiceCloneResponse, VoiceCloneError> {
    use reqwest::multipart::{Form, Part};

    // Validate audio samples
    if request.audio_samples.is_empty() {
        return Err(VoiceCloneError {
            code: "MISSING_AUDIO".to_string(),
            message: "ElevenLabs voice cloning requires at least one audio sample".to_string(),
            details: None,
        });
    }

    let client = voice_clone_http_client()?;

    // Build multipart form
    let mut form = Form::new().text("name", request.name.clone());

    // Add description if provided
    if let Some(desc) = &request.description {
        form = form.text("description", desc.clone());
    }

    // Add background noise removal flag
    if request.remove_background_noise {
        form = form.text("remove_background_noise", "true");
    }

    // Add labels if provided
    if let Some(labels) = &request.labels {
        let labels_json = serde_json::to_string(labels).unwrap_or_default();
        form = form.text("labels", labels_json);
    }

    // Decode and add audio samples
    for (i, sample_b64) in request.audio_samples.iter().enumerate() {
        let decoded = decode_audio_sample(sample_b64, i)?;

        // Detect format from magic bytes
        let (mime_type, extension) = detect_audio_format(&decoded);

        let part = Part::bytes(decoded)
            .file_name(format!("sample_{}.{}", i, extension))
            .mime_str(mime_type)
            .map_err(|e| VoiceCloneError {
                code: "INTERNAL_ERROR".to_string(),
                message: format!("Failed to set MIME type: {}", e),
                details: None,
            })?;

        form = form.part("files", part);
    }

    // Make API request
    let response = client
        .post("https://api.elevenlabs.io/v1/voices/add")
        .header("xi-api-key", api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to send request to ElevenLabs: {}", e),
            details: None,
        })?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("ELEVENLABS_{}", status.as_u16()),
            message: format!("ElevenLabs API error: {}", error_body),
            details: Some(serde_json::json!({ "status": status.as_u16() })),
        });
    }

    let el_response: ElevenLabsVoiceCreateResponse =
        response.json().await.map_err(|e| VoiceCloneError {
            code: "PARSE_ERROR".to_string(),
            message: format!("Failed to parse ElevenLabs response: {}", e),
            details: None,
        })?;

    Ok(VoiceCloneResponse {
        voice_id: el_response.voice_id,
        name: el_response.name.unwrap_or_else(|| request.name.clone()),
        provider: VoiceCloneProvider::ElevenLabs,
        status: CloneStatus::Ready.to_string(),
        requires_verification: el_response.requires_verification,
        created_at: now_rfc3339(),
        metadata: None,
    })
}

// =============================================================================
// Hume Voice Cloning
// =============================================================================

/// Hume TTS generation response (partial).
#[derive(Debug, Deserialize)]
struct HumeTTSResponse {
    generations: Vec<HumeGeneration>,
}

#[derive(Debug, Deserialize)]
struct HumeGeneration {
    generation_id: String,
    #[allow(dead_code)]
    audio: Option<String>,
}

/// Hume voice save response.
#[derive(Debug, Deserialize)]
struct HumeVoiceSaveResponse {
    id: String,
    name: String,
    #[allow(dead_code)]
    provider: Option<String>,
}

/// Design a custom voice using Hume AI's Voice Design API.
///
/// **Important**: Hume's API supports **voice design** (description-based), not audio-based
/// voice cloning. Audio-based cloning is only available through Hume's Platform UI at
/// https://app.hume.ai/voices
///
/// Hume uses a two-step process:
/// 1. Generate TTS with voice description to get a generation_id
/// 2. Save the voice using the generation_id
///
/// See: https://dev.hume.ai/docs/voice/voice-design
async fn clone_voice_hume(
    api_key: &str,
    request: &VoiceCloneRequest,
) -> Result<VoiceCloneResponse, VoiceCloneError> {
    // Hume API only supports description-based voice design, not audio-based cloning
    // Audio cloning is only available through Hume's Platform UI
    if !request.audio_samples.is_empty() {
        return Err(VoiceCloneError {
            code: "AUDIO_SAMPLES_NOT_SUPPORTED".to_string(),
            message: "Hume API does not support audio-based voice cloning via REST API. \
                      Audio cloning is only available through Hume's Platform UI at \
                      https://app.hume.ai/voices. Use the 'description' field for voice design instead."
                .to_string(),
            details: Some(serde_json::json!({
                "hint": "Provide a 'description' field with natural language voice characteristics",
                "example": "A warm, friendly female voice with a slight British accent",
                "platform_url": "https://app.hume.ai/voices"
            })),
        });
    }

    // Validate we have a non-empty description for voice design. The public route
    // already enforces this via `VoiceCloneRequest::validate`; keep the helper
    // defensive for direct unit/helper calls.
    let Some(description) = request
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(str::to_string)
    else {
        return Err(VoiceCloneError {
            code: "MISSING_DESCRIPTION".to_string(),
            message: "Hume voice design requires a 'description' field with natural language \
                      voice characteristics (e.g., 'A warm, energetic male voice')"
                .to_string(),
            details: Some(serde_json::json!({
                "hint": "Describe the voice you want to create",
                "examples": [
                    "A calm, professional female voice",
                    "An enthusiastic male voice with American accent",
                    "A warm, gentle voice suitable for storytelling"
                ]
            })),
        });
    };

    let client = voice_clone_http_client()?;

    // Step 1: Generate TTS with voice description to get generation_id
    let sample_text = request
        .sample_text
        .clone()
        .unwrap_or_else(|| "Hello, this is a sample of my custom voice.".to_string());

    // Build TTS request body
    let tts_request = serde_json::json!({
        "utterances": [{
            "text": sample_text,
            "description": description
        }],
        "num_generations": 1,
        "instant_mode": false
    });

    let response = client
        .post("https://api.hume.ai/v0/tts")
        .header("X-Hume-Api-Key", api_key)
        .header("Content-Type", "application/json")
        .json(&tts_request)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to generate voice sample: {}", e),
            details: None,
        })?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("HUME_{}", status.as_u16()),
            message: format!("Hume TTS API error: {}", error_body),
            details: Some(serde_json::json!({ "status": status.as_u16() })),
        });
    }

    let tts_response: HumeTTSResponse = response.json().await.map_err(|e| VoiceCloneError {
        code: "PARSE_ERROR".to_string(),
        message: format!("Failed to parse Hume TTS response: {}", e),
        details: None,
    })?;

    let generation = tts_response
        .generations
        .first()
        .ok_or_else(|| VoiceCloneError {
            code: "NO_GENERATION".to_string(),
            message: "Hume TTS did not return a generation".to_string(),
            details: None,
        })?;

    // Step 2: Save the voice using the generation_id
    let save_request = serde_json::json!({
        "generation_id": generation.generation_id,
        "name": request.name
    });

    let save_response = client
        .post("https://api.hume.ai/v0/tts/voices")
        .header("X-Hume-Api-Key", api_key)
        .header("Content-Type", "application/json")
        .json(&save_request)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to save voice: {}", e),
            details: None,
        })?;

    let save_status = save_response.status();
    if !save_status.is_success() {
        let error_body = save_response.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("HUME_{}", save_status.as_u16()),
            message: format!("Hume voice save error: {}", error_body),
            details: Some(serde_json::json!({ "status": save_status.as_u16() })),
        });
    }

    let voice_response: HumeVoiceSaveResponse =
        save_response.json().await.map_err(|e| VoiceCloneError {
            code: "PARSE_ERROR".to_string(),
            message: format!("Failed to parse Hume voice save response: {}", e),
            details: None,
        })?;

    Ok(VoiceCloneResponse {
        voice_id: voice_response.id,
        name: voice_response.name,
        provider: VoiceCloneProvider::Hume,
        status: CloneStatus::Ready.to_string(),
        requires_verification: None,
        created_at: now_rfc3339(),
        metadata: Some(serde_json::json!({
            "generation_id": generation.generation_id,
            "description": description
        })),
    })
}

// =============================================================================
// LMNT Voice Cloning
// =============================================================================

/// LMNT voice creation response.
#[derive(Debug, Deserialize)]
struct LmntVoiceCreateResponse {
    id: String,
    name: String,
    state: String,
}

/// Clone a voice using LMNT API.
///
/// LMNT voice cloning requires:
/// - Audio samples: 5+ seconds, max 20 files, 250MB total
/// - Supported formats: wav, mp3, mp4, m4a, webm
async fn clone_voice_lmnt(
    api_key: &str,
    request: &VoiceCloneRequest,
) -> Result<VoiceCloneResponse, VoiceCloneError> {
    use reqwest::multipart::{Form, Part};

    // Validate audio samples (LMNT requires at least 5 seconds of audio)
    if request.audio_samples.is_empty() {
        return Err(VoiceCloneError {
            code: "MISSING_AUDIO".to_string(),
            message: "LMNT voice cloning requires at least one audio sample (5+ seconds)"
                .to_string(),
            details: Some(serde_json::json!({
                "hint": "Provide 5+ seconds of clear audio for best results",
                "max_files": 20,
                "max_total_size": "250MB",
                "supported_formats": ["wav", "mp3", "mp4", "m4a", "webm"]
            })),
        });
    }

    // LMNT limits: max 20 files, 250MB total
    if request.audio_samples.len() > 20 {
        return Err(VoiceCloneError {
            code: "TOO_MANY_FILES".to_string(),
            message: format!(
                "LMNT supports max 20 audio files, got {}",
                request.audio_samples.len()
            ),
            details: None,
        });
    }

    let client = voice_clone_http_client()?;

    // Build multipart form
    let mut form = Form::new().text("name", request.name.clone());

    // Add enhancement option if specified (process noisy audio)
    // LMNT uses "enhance" parameter to clean up audio
    if request.remove_background_noise {
        form = form.text("enhance", "true");
    }

    // Decode and add audio samples
    for (i, sample_b64) in request.audio_samples.iter().enumerate() {
        let decoded = decode_audio_sample(sample_b64, i)?;

        // Detect format from magic bytes
        let (mime_type, extension) = detect_audio_format(&decoded);

        let part = Part::bytes(decoded)
            .file_name(format!("sample_{}.{}", i, extension))
            .mime_str(mime_type)
            .map_err(|e| VoiceCloneError {
                code: "INTERNAL_ERROR".to_string(),
                message: format!("Failed to set MIME type: {}", e),
                details: None,
            })?;

        form = form.part("files", part);
    }

    // Make API request to LMNT voice clone endpoint
    let response = client
        .post("https://api.lmnt.com/v1/ai/voice")
        .header("X-API-Key", api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to send request to LMNT: {}", e),
            details: None,
        })?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("LMNT_{}", status.as_u16()),
            message: format!("LMNT API error: {}", error_body),
            details: Some(serde_json::json!({ "status": status.as_u16() })),
        });
    }

    let lmnt_response: LmntVoiceCreateResponse =
        response.json().await.map_err(|e| VoiceCloneError {
            code: "PARSE_ERROR".to_string(),
            message: format!("Failed to parse LMNT response: {}", e),
            details: None,
        })?;

    // LMNT voice states: "ready" or "training"
    let status_str = if lmnt_response.state == "ready" {
        "ready"
    } else {
        "processing"
    };

    Ok(VoiceCloneResponse {
        voice_id: lmnt_response.id,
        name: lmnt_response.name,
        provider: VoiceCloneProvider::Lmnt,
        status: status_str.to_string(),
        requires_verification: None,
        created_at: now_rfc3339(),
        metadata: None,
    })
}

// =============================================================================
// Cartesia Voice Cloning (instant clip-mode)
// =============================================================================

#[derive(Debug, Deserialize)]
struct CartesiaVoiceCreateResponse {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

/// Clone a voice using Cartesia's clip-mode endpoint (instant). Multipart:
/// `name`, `clip` (one audio file ~5-20s), `language`, `mode` (similarity|stability).
async fn clone_voice_cartesia(
    api_key: &str,
    request: &VoiceCloneRequest,
) -> Result<VoiceCloneResponse, VoiceCloneError> {
    use reqwest::multipart::{Form, Part};

    let sample = request
        .audio_samples
        .first()
        .ok_or_else(|| VoiceCloneError {
            code: "MISSING_AUDIO".to_string(),
            message: "Cartesia voice cloning requires one audio clip (~5-20s)".to_string(),
            details: None,
        })?;
    let decoded = decode_audio_sample(sample, 0)?;
    let (mime, ext) = detect_audio_format(&decoded);

    let language = request
        .structured_labels
        .as_ref()
        .and_then(|l| l.language.clone())
        .or_else(|| {
            request
                .labels
                .as_ref()
                .and_then(|m| m.get("language").cloned())
        })
        .unwrap_or_else(|| "en".to_string());

    let clip = Part::bytes(decoded)
        .file_name(format!("clip.{ext}"))
        .mime_str(mime)
        .map_err(|e| VoiceCloneError {
            code: "INTERNAL_ERROR".to_string(),
            message: format!("Failed to set MIME type: {e}"),
            details: None,
        })?;

    let mut form = Form::new()
        .text("name", request.name.clone())
        .text("language", language)
        .text("mode", "similarity")
        .part("clip", clip);
    if let Some(desc) = &request.description {
        form = form.text("description", desc.clone());
    }

    let client = voice_clone_http_client()?;
    let response = client
        .post("https://api.cartesia.ai/voices/clone")
        .header("X-API-Key", api_key)
        .header("Cartesia-Version", "2024-06-10")
        .multipart(form)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to send request to Cartesia: {e}"),
            details: None,
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("CARTESIA_{}", status.as_u16()),
            message: format!("Cartesia API error: {body}"),
            details: Some(serde_json::json!({ "status": status.as_u16() })),
        });
    }

    let parsed: CartesiaVoiceCreateResponse =
        response.json().await.map_err(|e| VoiceCloneError {
            code: "PARSE_ERROR".to_string(),
            message: format!("Failed to parse Cartesia response: {e}"),
            details: None,
        })?;

    Ok(VoiceCloneResponse {
        voice_id: parsed.id,
        name: parsed.name.unwrap_or_else(|| request.name.clone()),
        provider: VoiceCloneProvider::Cartesia,
        status: CloneStatus::Ready.to_string(),
        requires_verification: None,
        created_at: now_rfc3339(),
        metadata: None,
    })
}

// =============================================================================
// PlayHT Voice Cloning (instant + professional)
// =============================================================================

#[derive(Debug, Deserialize)]
struct PlayHtVoiceCreateResponse {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

/// Clone a voice using PlayHT. `instant` → `/v2/cloned-voices/instant` (multipart
/// `voice_name` + `sample_file`); `professional` → `/v2/cloned-voices` (async).
async fn clone_voice_playht(
    api_key: &str,
    user_id: &str,
    request: &VoiceCloneRequest,
) -> Result<VoiceCloneResponse, VoiceCloneError> {
    use reqwest::multipart::{Form, Part};

    let sample = request
        .audio_samples
        .first()
        .ok_or_else(|| VoiceCloneError {
            code: "MISSING_AUDIO".to_string(),
            message: "PlayHT voice cloning requires at least one audio sample".to_string(),
            details: None,
        })?;
    let decoded = decode_audio_sample(sample, 0)?;
    let (mime, ext) = detect_audio_format(&decoded);

    let sample_part = Part::bytes(decoded)
        .file_name(format!("sample.{ext}"))
        .mime_str(mime)
        .map_err(|e| VoiceCloneError {
            code: "INTERNAL_ERROR".to_string(),
            message: format!("Failed to set MIME type: {e}"),
            details: None,
        })?;

    let mut form = Form::new()
        .text("voice_name", request.name.clone())
        .part("sample_file", sample_part);
    if let Some(g) = request
        .structured_labels
        .as_ref()
        .and_then(|l| l.gender.clone())
    {
        form = form.text("gender", g);
    }

    let professional = request.mode == CloneMode::Professional;
    let url = if professional {
        "https://api.play.ht/api/v2/cloned-voices"
    } else {
        "https://api.play.ht/api/v2/cloned-voices/instant"
    };

    let client = voice_clone_http_client()?;
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-User-ID", user_id)
        .header("accept", "application/json")
        .multipart(form)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to send request to PlayHT: {e}"),
            details: None,
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("PLAYHT_{}", status.as_u16()),
            message: format!("PlayHT API error: {body}"),
            details: Some(serde_json::json!({ "status": status.as_u16() })),
        });
    }

    let parsed: PlayHtVoiceCreateResponse = response.json().await.map_err(|e| VoiceCloneError {
        code: "PARSE_ERROR".to_string(),
        message: format!("Failed to parse PlayHT response: {e}"),
        details: None,
    })?;

    Ok(VoiceCloneResponse {
        voice_id: parsed.id,
        name: parsed.name.unwrap_or_else(|| request.name.clone()),
        provider: VoiceCloneProvider::PlayHt,
        // Instant → ready; professional → async (poll the cloned-voices list).
        status: if professional {
            CloneStatus::Training.to_string()
        } else {
            CloneStatus::Ready.to_string()
        },
        requires_verification: None,
        created_at: now_rfc3339(),
        metadata: None,
    })
}

// =============================================================================
// Speechify Voice Cloning (instant; consent REQUIRED)
// =============================================================================

#[derive(Debug, Deserialize)]
struct SpeechifyVoiceCreateResponse {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
}

/// Clone a voice using Speechify. Multipart `name`, `sample` (one audio file),
/// `consent` (JSON `{fullName, email}` — MANDATORY). Returns an instantly-usable id.
async fn clone_voice_speechify(
    api_key: &str,
    request: &VoiceCloneRequest,
) -> Result<VoiceCloneResponse, VoiceCloneError> {
    use reqwest::multipart::{Form, Part};

    // Consent is validated up-front in `VoiceCloneRequest::validate`, but re-extract
    // the required fields here to build the mandatory `consent` JSON part.
    let consent = request.consent.as_ref().ok_or_else(|| VoiceCloneError {
        code: "CONSENT_REQUIRED".to_string(),
        message: "Speechify requires consent with full_name and email".to_string(),
        details: None,
    })?;
    let (full_name, email) = match (&consent.full_name, &consent.email) {
        (Some(n), Some(e)) => (n.clone(), e.clone()),
        _ => {
            return Err(VoiceCloneError {
                code: "CONSENT_REQUIRED".to_string(),
                message: "Speechify consent requires both full_name and email".to_string(),
                details: None,
            });
        }
    };

    let sample = request
        .audio_samples
        .first()
        .ok_or_else(|| VoiceCloneError {
            code: "MISSING_AUDIO".to_string(),
            message: "Speechify voice cloning requires one audio sample (10-30s)".to_string(),
            details: None,
        })?;
    let decoded = decode_audio_sample(sample, 0)?;
    let (mime, ext) = detect_audio_format(&decoded);

    let sample_part = Part::bytes(decoded)
        .file_name(format!("sample.{ext}"))
        .mime_str(mime)
        .map_err(|e| VoiceCloneError {
            code: "INTERNAL_ERROR".to_string(),
            message: format!("Failed to set MIME type: {e}"),
            details: None,
        })?;

    let consent_json = serde_json::json!({ "fullName": full_name, "email": email }).to_string();
    let mut form = Form::new()
        .text("name", request.name.clone())
        .text("consent", consent_json)
        .part("sample", sample_part);
    if let Some(locale) = request
        .structured_labels
        .as_ref()
        .and_then(|l| l.language.clone())
    {
        form = form.text("locale", locale);
    }
    if let Some(g) = request
        .structured_labels
        .as_ref()
        .and_then(|l| l.gender.clone())
    {
        form = form.text("gender", g);
    }

    let client = voice_clone_http_client()?;
    let response = client
        .post("https://api.sws.speechify.com/v1/voices")
        .header("Authorization", format!("Bearer {api_key}"))
        .multipart(form)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to send request to Speechify: {e}"),
            details: None,
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("SPEECHIFY_{}", status.as_u16()),
            message: format!("Speechify API error: {body}"),
            details: Some(serde_json::json!({ "status": status.as_u16() })),
        });
    }

    let parsed: SpeechifyVoiceCreateResponse =
        response.json().await.map_err(|e| VoiceCloneError {
            code: "PARSE_ERROR".to_string(),
            message: format!("Failed to parse Speechify response: {e}"),
            details: None,
        })?;

    Ok(VoiceCloneResponse {
        voice_id: parsed.id,
        name: parsed.display_name.unwrap_or_else(|| request.name.clone()),
        provider: VoiceCloneProvider::Speechify,
        status: CloneStatus::Ready.to_string(),
        requires_verification: None,
        created_at: now_rfc3339(),
        metadata: None,
    })
}

// =============================================================================
// ElevenLabs PVC (professional, async multi-step)
// =============================================================================

#[derive(Debug, Deserialize)]
struct ElevenLabsPvcCreateResponse {
    voice_id: String,
}

/// Begin a professional ElevenLabs PVC clone (ASYNC). Step 1 creates the PVC voice
/// (`POST /v1/voices/pvc` with name+language+labels); step 2 uploads the samples
/// (`POST /v1/voices/pvc/{id}/samples`). Verification + training are subsequent
/// steps (gated by `consent.verified`); the returned status is `verifying`/`queued`
/// and the SDK polls `GET /v1/voices/{id}` → `fine_tuning.state`.
async fn clone_voice_elevenlabs_pvc(
    api_key: &str,
    request: &VoiceCloneRequest,
) -> Result<VoiceCloneResponse, VoiceCloneError> {
    use reqwest::multipart::{Form, Part};

    if request.audio_samples.is_empty() {
        return Err(VoiceCloneError {
            code: "MISSING_AUDIO".to_string(),
            message: "ElevenLabs PVC requires audio samples (30-60 min recommended)".to_string(),
            details: None,
        });
    }

    let language = request
        .structured_labels
        .as_ref()
        .and_then(|l| l.language.clone())
        .or_else(|| {
            request
                .labels
                .as_ref()
                .and_then(|m| m.get("language").cloned())
        })
        .unwrap_or_else(|| "en".to_string());

    let client = voice_clone_http_client()?;

    // Step 1: create the PVC voice resource.
    let mut create_body = serde_json::json!({ "name": request.name, "language": language });
    if let Some(desc) = &request.description {
        create_body["description"] = serde_json::json!(desc);
    }
    if let Some(labels) = request.merged_labels() {
        create_body["labels"] = serde_json::json!(labels);
    }
    let create = client
        .post("https://api.elevenlabs.io/v1/voices/pvc")
        .header("xi-api-key", api_key)
        .json(&create_body)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to create ElevenLabs PVC voice: {e}"),
            details: None,
        })?;
    let create_status = create.status();
    if !create_status.is_success() {
        let body = create.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("ELEVENLABS_PVC_{}", create_status.as_u16()),
            message: format!("ElevenLabs PVC create error: {body}"),
            details: Some(serde_json::json!({ "status": create_status.as_u16() })),
        });
    }
    let created: ElevenLabsPvcCreateResponse =
        create.json().await.map_err(|e| VoiceCloneError {
            code: "PARSE_ERROR".to_string(),
            message: format!("Failed to parse ElevenLabs PVC create response: {e}"),
            details: None,
        })?;
    let voice_id = created.voice_id;

    // Step 2: upload samples.
    let mut form = Form::new();
    if request.remove_background_noise {
        form = form.text("remove_background_noise", "true");
    }
    for (i, sample_b64) in request.audio_samples.iter().enumerate() {
        let decoded = decode_audio_sample(sample_b64, i)?;
        let (mime, ext) = detect_audio_format(&decoded);
        let part = Part::bytes(decoded)
            .file_name(format!("sample_{i}.{ext}"))
            .mime_str(mime)
            .map_err(|e| VoiceCloneError {
                code: "INTERNAL_ERROR".to_string(),
                message: format!("Failed to set MIME type: {e}"),
                details: None,
            })?;
        form = form.part("files", part);
    }
    let upload = client
        .post(format!(
            "https://api.elevenlabs.io/v1/voices/pvc/{voice_id}/samples"
        ))
        .header("xi-api-key", api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to upload ElevenLabs PVC samples: {e}"),
            details: None,
        })?;
    if !upload.status().is_success() {
        let st = upload.status();
        let body = upload.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("ELEVENLABS_PVC_{}", st.as_u16()),
            message: format!("ElevenLabs PVC sample upload error: {body}"),
            details: Some(serde_json::json!({ "status": st.as_u16(), "voice_id": voice_id })),
        });
    }

    // Verification + training are subsequent steps; the voice is now created with
    // samples but not yet trained → `verifying` (caller polls fine_tuning.state).
    Ok(VoiceCloneResponse {
        voice_id,
        name: request.name.clone(),
        provider: VoiceCloneProvider::ElevenLabs,
        status: CloneStatus::Verifying.to_string(),
        requires_verification: Some(true),
        created_at: now_rfc3339(),
        metadata: Some(serde_json::json!({
            "mode": "professional",
            "next": "verify (captcha/voice) then POST /v1/voices/pvc/{voice_id}/train; \
                     poll GET /v1/voices/{voice_id}.fine_tuning.state",
        })),
    })
}

// =============================================================================
// Resemble AI (professional, async)
// =============================================================================

#[derive(Debug, Deserialize)]
struct ResembleVoiceCreateResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    item: Option<ResembleVoiceItem>,
}

#[derive(Debug, Deserialize)]
struct ResembleVoiceItem {
    uuid: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// Begin a professional Resemble AI clone (ASYNC; consent/voice-talent proof
/// required, gated by `consent.verified`). Creates the voice; training runs async
/// (webhook via `callback_uri` or poll the voice resource).
async fn clone_voice_resemble(
    api_key: &str,
    request: &VoiceCloneRequest,
) -> Result<VoiceCloneResponse, VoiceCloneError> {
    let language = request
        .structured_labels
        .as_ref()
        .and_then(|l| l.language.clone())
        .unwrap_or_else(|| "en".to_string());

    // Resemble's create-voice endpoint takes JSON; audio datasets are attached via a
    // subsequent recordings upload. We create the voice resource here.
    let body = serde_json::json!({
        "name": request.name,
        "dataset_url": serde_json::Value::Null,
        "consent": request.consent.as_ref().map(|c| serde_json::json!({
            "verified": c.verified,
            "full_name": c.full_name,
            "email": c.email,
        })),
        "default_language": language,
    });

    let client = voice_clone_http_client()?;
    let response = client
        .post("https://app.resemble.ai/api/v2/voices")
        .header("Authorization", format!("Token {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| VoiceCloneError {
            code: "REQUEST_FAILED".to_string(),
            message: format!("Failed to send request to Resemble: {e}"),
            details: None,
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(VoiceCloneError {
            code: format!("RESEMBLE_{}", status.as_u16()),
            message: format!("Resemble API error: {body}"),
            details: Some(serde_json::json!({ "status": status.as_u16() })),
        });
    }

    let parsed: ResembleVoiceCreateResponse =
        response.json().await.map_err(|e| VoiceCloneError {
            code: "PARSE_ERROR".to_string(),
            message: format!("Failed to parse Resemble response: {e}"),
            details: None,
        })?;
    let item = parsed.item.ok_or_else(|| VoiceCloneError {
        code: "RESEMBLE_NO_ITEM".to_string(),
        message: "Resemble response missing voice item".to_string(),
        details: None,
    })?;
    let canonical = item
        .status
        .as_deref()
        .map(CloneStatus::from_resemble_status)
        .unwrap_or(CloneStatus::Queued);

    Ok(VoiceCloneResponse {
        voice_id: item.uuid,
        name: item.name.unwrap_or_else(|| request.name.clone()),
        provider: VoiceCloneProvider::Resemble,
        status: canonical.to_string(),
        requires_verification: Some(true),
        created_at: now_rfc3339(),
        metadata: Some(serde_json::json!({
            "mode": "professional",
            "poll": "GET /api/v2/voices/{uuid} or use callback_uri webhook",
        })),
    })
}

// =============================================================================
// Shared clone helpers
// =============================================================================

/// Current UTC time as an RFC-3339 string (the `created_at` field).
fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Decode one (optionally data-URL-prefixed) base64 audio sample to bytes.
fn decode_audio_sample(sample_b64: &str, index: usize) -> Result<Vec<u8>, VoiceCloneError> {
    decode_audio_sample_with_limit(sample_b64, index, MAX_VOICE_CLONE_SAMPLE_BYTES)
}

fn decode_audio_sample_with_limit(
    sample_b64: &str,
    index: usize,
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, VoiceCloneError> {
    let audio_data = voice_clone_audio_payload(sample_b64);
    let estimated = estimated_decoded_base64_len(audio_data);
    if estimated > max_decoded_bytes {
        return Err(voice_clone_audio_too_large(
            index,
            estimated,
            max_decoded_bytes,
        ));
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(audio_data)
        .map_err(|e| VoiceCloneError {
            code: "INVALID_AUDIO".to_string(),
            message: format!("Failed to decode audio sample {index}: {e}"),
            details: None,
        })?;

    if decoded.len() > max_decoded_bytes {
        return Err(voice_clone_audio_too_large(
            index,
            decoded.len(),
            max_decoded_bytes,
        ));
    }

    Ok(decoded)
}

fn voice_clone_audio_payload(sample_b64: &str) -> &str {
    sample_b64
        .split_once(',')
        .map(|x| x.1)
        .unwrap_or(sample_b64)
        .trim()
}

fn estimated_decoded_base64_len(payload: &str) -> usize {
    let padding = payload
        .as_bytes()
        .iter()
        .rev()
        .take_while(|&&b| b == b'=')
        .count()
        .min(2);
    ((payload.len().saturating_add(3)) / 4)
        .saturating_mul(3)
        .saturating_sub(padding)
}

fn validate_voice_clone_audio_size_limits(samples: &[String]) -> Result<(), VoiceCloneError> {
    validate_voice_clone_audio_size_limits_with_limits(
        samples,
        MAX_VOICE_CLONE_SAMPLE_BYTES,
        MAX_VOICE_CLONE_TOTAL_AUDIO_BYTES,
    )
}

fn validate_voice_clone_audio_size_limits_with_limits(
    samples: &[String],
    max_sample_bytes: usize,
    max_total_bytes: usize,
) -> Result<(), VoiceCloneError> {
    let mut total = 0usize;
    for (index, sample) in samples.iter().enumerate() {
        let decoded_estimate = estimated_decoded_base64_len(voice_clone_audio_payload(sample));
        if decoded_estimate > max_sample_bytes {
            return Err(voice_clone_audio_too_large(
                index,
                decoded_estimate,
                max_sample_bytes,
            ));
        }

        total = total
            .checked_add(decoded_estimate)
            .ok_or_else(|| voice_clone_audio_total_too_large(usize::MAX, max_total_bytes))?;
        if total > max_total_bytes {
            return Err(voice_clone_audio_total_too_large(total, max_total_bytes));
        }
    }

    Ok(())
}

fn voice_clone_audio_too_large(
    index: usize,
    decoded_bytes: usize,
    limit: usize,
) -> VoiceCloneError {
    VoiceCloneError {
        code: "AUDIO_TOO_LARGE".to_string(),
        message: format!(
            "Voice clone audio sample {index} exceeds decoded size limit of {limit} bytes"
        ),
        details: Some(serde_json::json!({
            "sample_index": index,
            "decoded_bytes": decoded_bytes,
            "limit_bytes": limit,
        })),
    }
}

fn voice_clone_audio_total_too_large(decoded_bytes: usize, limit: usize) -> VoiceCloneError {
    VoiceCloneError {
        code: "AUDIO_TOO_LARGE".to_string(),
        message: format!(
            "Voice clone audio samples exceed decoded total size limit of {limit} bytes"
        ),
        details: Some(serde_json::json!({
            "decoded_total_bytes": decoded_bytes,
            "limit_bytes": limit,
        })),
    }
}

// =============================================================================
// Audio Format Detection
// =============================================================================

/// Detect audio format from magic bytes.
fn detect_audio_format(data: &[u8]) -> (&'static str, &'static str) {
    if data.len() < 12 {
        return ("application/octet-stream", "bin");
    }

    // Check for common audio format signatures
    if data.starts_with(b"ID3") || (data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xE0) == 0xE0)
    {
        return ("audio/mpeg", "mp3");
    }
    if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WAVE" {
        return ("audio/wav", "wav");
    }
    if data.starts_with(b"ftyp") || (data.len() >= 8 && &data[4..8] == b"ftyp") {
        return ("audio/mp4", "m4a");
    }
    if data.starts_with(b"OggS") {
        return ("audio/ogg", "ogg");
    }
    if data.starts_with(b"fLaC") {
        return ("audio/flac", "flac");
    }

    // Default to wav if unknown
    ("audio/wav", "wav")
}

// =============================================================================
// Voice Clone Handler
// =============================================================================

/// Handler for POST /voices/clone - Clone a voice from audio samples or description.
///
/// This endpoint supports multiple providers:
/// - **ElevenLabs**: Instant voice cloning from audio samples (1-2 minutes recommended)
/// - **Hume**: Voice design from description, or instant cloning from audio
///
/// # Request Body
///
/// ```json
/// {
///   "provider": "elevenlabs",
///   "name": "My Custom Voice",
///   "description": "A warm, friendly voice",
///   "audio_samples": ["base64_encoded_audio_data"],
///   "remove_background_noise": false
/// }
/// ```
///
/// # Response
///
/// ```json
/// {
///   "voice_id": "voice_abc123",
///   "name": "My Custom Voice",
///   "provider": "elevenlabs",
///   "status": "ready",
///   "created_at": "2026-01-06T12:00:00Z"
/// }
/// ```
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/voices/clone",
        request_body = VoiceCloneRequest,
        responses(
            (status = 200, description = "Voice cloned successfully", body = VoiceCloneResponse),
            (status = 400, description = "Invalid request", body = VoiceCloneError),
            (status = 401, description = "Unauthorized - missing or invalid API key"),
            (status = 500, description = "Internal server error", body = VoiceCloneError)
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "voices"
    )
)]
pub async fn clone_voice(
    State(state): State<Arc<AppState>>,
    Json(request): Json<VoiceCloneRequest>,
) -> Result<Json<VoiceCloneResponse>, (StatusCode, Json<VoiceCloneError>)> {
    // Up-front, network-free validation: name, Speechify consent, mode capability,
    // PVC/Resemble consent.verified gate. Violations → 400 (never reach a provider).
    request
        .validate()
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(e)))?;

    // Resolve the provider credential once (UNAUTHORIZED on missing).
    let key = request.provider.credential_key();
    let missing_key = |provider: VoiceCloneProvider| {
        (
            StatusCode::UNAUTHORIZED,
            Json(VoiceCloneError {
                code: "MISSING_API_KEY".to_string(),
                message: format!("{provider} API key not configured"),
                details: None,
            }),
        )
    };

    // Route by provider + mode. ElevenLabs forks instant (IVC) vs professional (PVC).
    let result = match request.provider {
        VoiceCloneProvider::ElevenLabs => {
            let api_key = state
                .config
                .get_api_key(key)
                .map_err(|_| missing_key(request.provider))?;
            if request.mode == CloneMode::Professional {
                clone_voice_elevenlabs_pvc(&api_key, &request).await
            } else {
                clone_voice_elevenlabs(&api_key, &request).await
            }
        }
        VoiceCloneProvider::Hume => {
            let api_key = state
                .config
                .get_api_key(key)
                .map_err(|_| missing_key(request.provider))?;
            clone_voice_hume(&api_key, &request).await
        }
        VoiceCloneProvider::Lmnt => {
            let api_key = state
                .config
                .get_api_key(key)
                .map_err(|_| missing_key(request.provider))?;
            clone_voice_lmnt(&api_key, &request).await
        }
        VoiceCloneProvider::Cartesia => {
            let api_key = state
                .config
                .get_api_key(key)
                .map_err(|_| missing_key(request.provider))?;
            clone_voice_cartesia(&api_key, &request).await
        }
        VoiceCloneProvider::PlayHt => {
            let (api_key, user_id) = state
                .config
                .get_playht_credentials()
                .map_err(|_| missing_key(request.provider))?;
            clone_voice_playht(&api_key, &user_id, &request).await
        }
        VoiceCloneProvider::Speechify => {
            let api_key = state
                .config
                .get_api_key(key)
                .map_err(|_| missing_key(request.provider))?;
            clone_voice_speechify(&api_key, &request).await
        }
        VoiceCloneProvider::Resemble => {
            let api_key = state
                .config
                .get_api_key(key)
                .map_err(|_| missing_key(request.provider))?;
            clone_voice_resemble(&api_key, &request).await
        }
    };

    result
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(e)))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn test_voice_clone_provider_display() {
        assert_eq!(VoiceCloneProvider::Hume.to_string(), "hume");
        assert_eq!(VoiceCloneProvider::ElevenLabs.to_string(), "elevenlabs");
        assert_eq!(VoiceCloneProvider::Lmnt.to_string(), "lmnt");
    }

    #[test]
    fn test_voice_clone_provider_serde() {
        let hume: VoiceCloneProvider = serde_json::from_str("\"hume\"").unwrap();
        assert_eq!(hume, VoiceCloneProvider::Hume);

        let el: VoiceCloneProvider = serde_json::from_str("\"elevenlabs\"").unwrap();
        assert_eq!(el, VoiceCloneProvider::ElevenLabs);

        let lmnt: VoiceCloneProvider = serde_json::from_str("\"lmnt\"").unwrap();
        assert_eq!(lmnt, VoiceCloneProvider::Lmnt);

        let hume_json = serde_json::to_string(&VoiceCloneProvider::Hume).unwrap();
        assert_eq!(hume_json, "\"hume\"");

        let lmnt_json = serde_json::to_string(&VoiceCloneProvider::Lmnt).unwrap();
        assert_eq!(lmnt_json, "\"lmnt\"");
    }

    #[test]
    fn test_voice_clone_request_deserialization() {
        let json = r#"{
            "provider": "elevenlabs",
            "name": "My Voice",
            "description": "A warm voice",
            "audio_samples": ["base64data"],
            "remove_background_noise": true
        }"#;

        let request: VoiceCloneRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.provider, VoiceCloneProvider::ElevenLabs);
        assert_eq!(request.name, "My Voice");
        assert_eq!(request.description, Some("A warm voice".to_string()));
        assert_eq!(request.audio_samples.len(), 1);
        assert!(request.remove_background_noise);
    }

    #[test]
    fn test_voice_clone_request_minimal() {
        let json = r#"{
            "provider": "hume",
            "name": "Test Voice"
        }"#;

        let request: VoiceCloneRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.provider, VoiceCloneProvider::Hume);
        assert_eq!(request.name, "Test Voice");
        assert!(request.audio_samples.is_empty());
        assert!(!request.remove_background_noise);
    }

    #[test]
    fn test_voice_clone_response_serialization() {
        let response = VoiceCloneResponse {
            voice_id: "voice_123".to_string(),
            name: "My Voice".to_string(),
            provider: VoiceCloneProvider::ElevenLabs,
            status: "ready".to_string(),
            requires_verification: None,
            created_at: "2026-01-06T12:00:00Z".to_string(),
            metadata: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"voice_id\":\"voice_123\""));
        assert!(json.contains("\"provider\":\"elevenlabs\""));
        assert!(json.contains("\"status\":\"ready\""));
    }

    #[test]
    fn test_detect_audio_format_mp3() {
        // MP3 ID3v2 header (needs at least 12 bytes)
        let mp3_id3 = b"ID3\x04\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(detect_audio_format(mp3_id3), ("audio/mpeg", "mp3"));

        // MP3 sync word (needs at least 12 bytes)
        let mp3_sync = &[
            0xFF, 0xFB, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(detect_audio_format(mp3_sync), ("audio/mpeg", "mp3"));
    }

    #[test]
    fn test_detect_audio_format_wav() {
        let wav = b"RIFF\x00\x00\x00\x00WAVEfmt ";
        assert_eq!(detect_audio_format(wav), ("audio/wav", "wav"));
    }

    #[test]
    fn test_detect_audio_format_ogg() {
        let ogg = b"OggS\x00\x02\x00\x00\x00\x00\x00\x00";
        assert_eq!(detect_audio_format(ogg), ("audio/ogg", "ogg"));
    }

    #[test]
    fn test_detect_audio_format_flac() {
        let flac = b"fLaC\x00\x00\x00\x22\x10\x00\x10\x00";
        assert_eq!(detect_audio_format(flac), ("audio/flac", "flac"));
    }

    #[test]
    fn test_detect_audio_format_unknown() {
        let unknown = b"unknown format data";
        assert_eq!(detect_audio_format(unknown), ("audio/wav", "wav"));
    }

    #[test]
    fn test_detect_audio_format_short_data() {
        let short = b"short";
        assert_eq!(
            detect_audio_format(short),
            ("application/octet-stream", "bin")
        );
    }

    #[test]
    fn voice_clone_audio_decode_is_size_bounded_before_allocation() {
        let ok = decode_audio_sample_with_limit("data:audio/wav;base64,QUJD", 0, 3).unwrap();
        assert_eq!(ok, b"ABC");

        let err = decode_audio_sample_with_limit("QUJD", 0, 2).unwrap_err();
        assert_eq!(err.code, "AUDIO_TOO_LARGE");
        assert_eq!(err.details.unwrap()["sample_index"], 0);
    }

    #[test]
    fn voice_clone_validation_enforces_total_decoded_audio_limit() {
        let samples = vec!["QUJD".to_string(), "QUJD".to_string()];
        let err = validate_voice_clone_audio_size_limits_with_limits(&samples, 3, 5).unwrap_err();
        assert_eq!(err.code, "AUDIO_TOO_LARGE");
        assert!(err.message.contains("total"));
    }

    #[test]
    fn test_voice_clone_error_serialization() {
        let error = VoiceCloneError {
            code: "TEST_ERROR".to_string(),
            message: "Test error message".to_string(),
            details: Some(serde_json::json!({"key": "value"})),
        };

        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("\"code\":\"TEST_ERROR\""));
        assert!(json.contains("\"message\":\"Test error message\""));
        assert!(json.contains("\"details\":{\"key\":\"value\"}"));
    }

    #[test]
    fn test_voice_clone_request_with_labels() {
        let json = r#"{
            "provider": "elevenlabs",
            "name": "Test",
            "labels": {"accent": "british", "gender": "male"}
        }"#;

        let request: VoiceCloneRequest = serde_json::from_str(json).unwrap();
        let labels = request.labels.unwrap();
        assert_eq!(labels.get("accent"), Some(&"british".to_string()));
        assert_eq!(labels.get("gender"), Some(&"male".to_string()));
    }

    #[test]
    fn test_voice_clone_request_lmnt() {
        let json = r#"{
            "provider": "lmnt",
            "name": "My LMNT Voice",
            "audio_samples": ["base64data"],
            "remove_background_noise": true
        }"#;

        let request: VoiceCloneRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.provider, VoiceCloneProvider::Lmnt);
        assert_eq!(request.name, "My LMNT Voice");
        assert_eq!(request.audio_samples.len(), 1);
        assert!(request.remove_background_noise);
    }

    #[test]
    fn test_voice_clone_response_lmnt() {
        let response = VoiceCloneResponse {
            voice_id: "voice_lmnt_123".to_string(),
            name: "LMNT Voice".to_string(),
            provider: VoiceCloneProvider::Lmnt,
            status: "ready".to_string(),
            requires_verification: None,
            created_at: "2026-01-07T12:00:00Z".to_string(),
            metadata: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"voice_id\":\"voice_lmnt_123\""));
        assert!(json.contains("\"provider\":\"lmnt\""));
        assert!(json.contains("\"status\":\"ready\""));
    }

    #[test]
    fn azure_voices_region_is_ssrf_checked() {
        let _env = crate::core::net::ssrf_env_lock();
        assert_eq!(
            azure_voices_list_url(" westeurope ").unwrap(),
            "https://westeurope.tts.speech.microsoft.com/cognitiveservices/voices/list"
        );

        let err = azure_voices_list_url("127.0.0.1:9000/foo").unwrap_err();
        assert!(err.contains("SSRF protection"), "{err}");

        let err = azure_voices_list_url("evil.com@127.0.0.1").unwrap_err();
        assert!(err.contains("single DNS label"), "{err}");
    }

    #[tokio::test]
    async fn voice_handler_http_client_redirect_policy_rejects_private_hop() {
        let _env = crate::core::net::ssrf_env_lock();
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(err) => {
                if err.kind() == ErrorKind::PermissionDenied {
                    eprintln!(
                        "Skipping voice_handler_http_client_redirect_policy_rejects_private_hop: {err}"
                    );
                    return;
                }
                panic!("Failed to bind redirect test server listener: {err}");
            }
        };
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });

        let err = voice_handler_http_client()
            .unwrap()
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .expect_err("private redirect target must be rejected");
        let mut error_chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(error) = source {
            error_chain.push_str(": ");
            error_chain.push_str(&error.to_string());
            source = error.source();
        }
        assert!(
            error_chain.contains("redirect URL rejected"),
            "unexpected redirect error: {error_chain}"
        );
    }

    // ---- P4 canonical VoiceClone widening ----------------------------------

    #[test]
    fn p4_new_providers_serde() {
        for (s, p) in [
            ("cartesia", VoiceCloneProvider::Cartesia),
            ("playht", VoiceCloneProvider::PlayHt),
            ("speechify", VoiceCloneProvider::Speechify),
            ("resemble", VoiceCloneProvider::Resemble),
        ] {
            let parsed: VoiceCloneProvider =
                serde_json::from_str(&format!("\"{s}\"")).unwrap_or_else(|_| panic!("{s}"));
            assert_eq!(parsed, p);
            assert_eq!(serde_json::to_string(&p).unwrap(), format!("\"{s}\""));
            assert_eq!(p.to_string(), s);
        }
    }

    #[test]
    fn p4_clone_mode_defaults_to_instant() {
        let json = r#"{ "provider": "cartesia", "name": "V" }"#;
        let req: VoiceCloneRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.mode, CloneMode::Instant);
    }

    #[test]
    fn p4_clone_request_full_canonical_shape_deserializes() {
        let json = r#"{
            "provider": "elevenlabs",
            "name": "Pro Voice",
            "mode": "professional",
            "audio_samples": ["YWJj"],
            "structured_labels": { "language": "en", "accent": "american", "gender": "female", "age": "young" },
            "base_voice_id": "base123",
            "remove_background_noise": true,
            "consent": { "verified": true, "full_name": "Jane Doe", "email": "jane@example.com" }
        }"#;
        let req: VoiceCloneRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.mode, CloneMode::Professional);
        assert_eq!(req.base_voice_id.as_deref(), Some("base123"));
        let labels = req.merged_labels().unwrap();
        assert_eq!(labels.get("gender").map(String::as_str), Some("female"));
        assert_eq!(labels.get("age").map(String::as_str), Some("young"));
        assert!(req.consent.as_ref().unwrap().verified);
    }

    #[test]
    fn p4_clone_status_serializes_canonically() {
        assert_eq!(CloneStatus::Ready.to_string(), "ready");
        assert_eq!(CloneStatus::Verifying.to_string(), "verifying");
        assert_eq!(CloneStatus::Training.to_string(), "training");
        assert_eq!(CloneStatus::Queued.to_string(), "queued");
        assert_eq!(CloneStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn p4_elevenlabs_fine_tuning_state_mapping() {
        use CloneStatus::*;
        let cases = [
            ("fine_tuned", Ready),
            ("not_verified", Verifying),
            ("queued", Queued),
            ("not_started", Queued),
            ("draft", Queued),
            ("fine_tuning", Training),
            ("delayed", Training),
            ("failed", Failed),
            ("unknown_state", Queued),
        ];
        for (state, expected) in cases {
            assert_eq!(
                CloneStatus::from_elevenlabs_fine_tuning_state(state),
                expected,
                "state {state}"
            );
        }
    }

    #[test]
    fn p4_resemble_status_mapping() {
        use CloneStatus::*;
        assert_eq!(CloneStatus::from_resemble_status("completed"), Ready);
        assert_eq!(CloneStatus::from_resemble_status("processing"), Training);
        assert_eq!(CloneStatus::from_resemble_status("pending"), Queued);
        assert_eq!(CloneStatus::from_resemble_status("error"), Failed);
    }

    #[test]
    fn p4_speechify_consent_required_validation() {
        // No consent → 400 CONSENT_REQUIRED.
        let req: VoiceCloneRequest = serde_json::from_str(
            r#"{ "provider": "speechify", "name": "V", "audio_samples": ["YWJj"] }"#,
        )
        .unwrap();
        let err = req.validate().unwrap_err();
        assert_eq!(err.code, "CONSENT_REQUIRED");

        // With consent {full_name, email} → OK.
        let ok: VoiceCloneRequest = serde_json::from_str(
            r#"{ "provider": "speechify", "name": "V", "audio_samples": ["YWJj"],
                 "consent": { "full_name": "Jane", "email": "j@x.com" } }"#,
        )
        .unwrap();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn p4_professional_unsupported_provider_validation() {
        // Cartesia has no professional mode → 400.
        let req: VoiceCloneRequest = serde_json::from_str(
            r#"{ "provider": "cartesia", "name": "V", "mode": "professional", "audio_samples": ["YWJj"] }"#,
        )
        .unwrap();
        let err = req.validate().unwrap_err();
        assert_eq!(err.code, "PROFESSIONAL_UNSUPPORTED");
    }

    #[test]
    fn p4_hume_voice_design_requires_description_and_rejects_audio_samples() {
        let missing: VoiceCloneRequest =
            serde_json::from_str(r#"{ "provider": "hume", "name": "V" }"#).unwrap();
        let err = missing.validate().unwrap_err();
        assert_eq!(err.code, "MISSING_DESCRIPTION");

        let blank: VoiceCloneRequest =
            serde_json::from_str(r#"{ "provider": "hume", "name": "V", "description": "   " }"#)
                .unwrap();
        let err = blank.validate().unwrap_err();
        assert_eq!(err.code, "MISSING_DESCRIPTION");

        let audio: VoiceCloneRequest = serde_json::from_str(
            r#"{ "provider": "hume", "name": "V", "description": "warm", "audio_samples": ["YWJj"] }"#,
        )
        .unwrap();
        let err = audio.validate().unwrap_err();
        assert_eq!(err.code, "AUDIO_SAMPLES_NOT_SUPPORTED");

        let ok: VoiceCloneRequest = serde_json::from_str(
            r#"{ "provider": "hume", "name": "V", "description": "warm narrator" }"#,
        )
        .unwrap();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn p4_audio_clone_providers_require_audio_before_credentials() {
        let cases = [
            (r#"{ "provider": "elevenlabs", "name": "V" }"#, "elevenlabs"),
            (r#"{ "provider": "lmnt", "name": "V" }"#, "lmnt"),
            (r#"{ "provider": "cartesia", "name": "V" }"#, "cartesia"),
            (r#"{ "provider": "playht", "name": "V" }"#, "playht"),
            (
                r#"{ "provider": "speechify", "name": "V",
                     "consent": { "full_name": "Jane", "email": "j@x.com" } }"#,
                "speechify",
            ),
            (
                r#"{ "provider": "elevenlabs", "name": "V", "mode": "professional",
                     "consent": { "verified": true } }"#,
                "elevenlabs",
            ),
        ];

        for (json, provider) in cases {
            let req: VoiceCloneRequest = serde_json::from_str(json).unwrap();
            let err = req.validate().unwrap_err();
            assert_eq!(err.code, "MISSING_AUDIO", "provider {provider}");
            assert!(err.message.to_lowercase().contains(provider));
        }
    }

    #[test]
    fn p4_lmnt_rejects_too_many_audio_samples_before_credentials() {
        let samples = (0..21).map(|_| "\"YWJj\"").collect::<Vec<_>>().join(",");
        let json =
            format!(r#"{{ "provider": "lmnt", "name": "V", "audio_samples": [{samples}] }}"#);
        let req: VoiceCloneRequest = serde_json::from_str(&json).unwrap();
        let err = req.validate().unwrap_err();
        assert_eq!(err.code, "TOO_MANY_FILES");
    }

    #[test]
    fn p4_pvc_requires_verified_consent_validation() {
        // ElevenLabs professional without consent.verified → 400.
        let req: VoiceCloneRequest = serde_json::from_str(
            r#"{ "provider": "elevenlabs", "name": "V", "mode": "professional", "audio_samples": ["YWJj"] }"#,
        )
        .unwrap();
        let err = req.validate().unwrap_err();
        assert_eq!(err.code, "CONSENT_NOT_VERIFIED");

        // With verified consent → OK.
        let ok: VoiceCloneRequest = serde_json::from_str(
            r#"{ "provider": "elevenlabs", "name": "V", "mode": "professional",
                 "audio_samples": ["YWJj"], "consent": { "verified": true } }"#,
        )
        .unwrap();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn p4_response_with_status_enum_and_requires_verification() {
        let response = VoiceCloneResponse {
            voice_id: "pvc_1".to_string(),
            name: "Pro".to_string(),
            provider: VoiceCloneProvider::ElevenLabs,
            status: CloneStatus::Verifying.to_string(),
            requires_verification: Some(true),
            created_at: now_rfc3339(),
            metadata: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"status\":\"verifying\""));
        assert!(json.contains("\"requires_verification\":true"));
    }

    #[test]
    fn p4_backward_compat_old_request_still_deserializes() {
        // The pre-P4 request shape (no mode/consent/structured_labels) must still work.
        let json = r#"{ "provider": "elevenlabs", "name": "V", "audio_samples": ["YWJj"],
                        "remove_background_noise": true,
                        "labels": { "accent": "british" } }"#;
        let req: VoiceCloneRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.mode, CloneMode::Instant);
        assert!(req.consent.is_none());
        assert_eq!(
            req.merged_labels()
                .unwrap()
                .get("accent")
                .map(String::as_str),
            Some("british")
        );
        assert!(req.validate().is_ok());
    }
}

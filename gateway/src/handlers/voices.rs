use axum::{extract::State, http::StatusCode, response::Json};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

use crate::core::providers::google::{
    CredentialSource, GOOGLE_CLOUD_PLATFORM_SCOPE, GoogleAuthClient, TokenProvider,
};
use crate::state::AppState;

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
    let client = reqwest::Client::new();

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
    let client = reqwest::Client::new();

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

    let client = reqwest::Client::new();

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
    let client = reqwest::Client::new();

    // Azure TTS voices list endpoint
    let url = format!(
        "https://{}.tts.speech.microsoft.com/cognitiveservices/voices/list",
        region
    );

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

// Helper function to fetch voices from LMNT API
async fn fetch_lmnt_voices(
    api_key: &str,
) -> Result<Vec<Voice>, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();

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
// Voice Cloning Types
// =============================================================================

/// Voice cloning provider selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum VoiceCloneProvider {
    /// Hume AI Octave voice design
    Hume,
    /// ElevenLabs instant voice cloning
    ElevenLabs,
    /// LMNT instant voice cloning (5+ seconds of audio)
    Lmnt,
}

impl std::fmt::Display for VoiceCloneProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hume => write!(f, "hume"),
            Self::ElevenLabs => write!(f, "elevenlabs"),
            Self::Lmnt => write!(f, "lmnt"),
        }
    }
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

    /// Remove background noise from samples (ElevenLabs only).
    #[serde(default)]
    pub remove_background_noise: bool,

    /// Labels for the voice (ElevenLabs only).
    #[serde(default)]
    pub labels: Option<HashMap<String, String>>,
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

    /// Status of the voice (ready, processing, failed).
    #[cfg_attr(feature = "openapi", schema(example = "ready"))]
    pub status: String,

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

    let client = reqwest::Client::new();

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
        // Handle potential data URL prefix
        let audio_data = if sample_b64.contains(',') {
            // Data URL format: data:audio/wav;base64,xxxxx
            let parts: Vec<&str> = sample_b64.splitn(2, ',').collect();
            if parts.len() == 2 {
                parts[1]
            } else {
                sample_b64.as_str()
            }
        } else {
            sample_b64.as_str()
        };

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(audio_data)
            .map_err(|e| VoiceCloneError {
                code: "INVALID_AUDIO".to_string(),
                message: format!("Failed to decode audio sample {}: {}", i, e),
                details: None,
            })?;

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
        status: "ready".to_string(),
        created_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
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

    // Validate we have a description for voice design
    if request.description.is_none() {
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
    }

    let client = reqwest::Client::new();

    // Step 1: Generate TTS with voice description to get generation_id
    let sample_text = request
        .sample_text
        .clone()
        .unwrap_or_else(|| "Hello, this is a sample of my custom voice.".to_string());

    // Description is guaranteed to be Some due to validation above
    let description = request.description.clone().unwrap();

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
        status: "ready".to_string(),
        created_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
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

    let client = reqwest::Client::new();

    // Build multipart form
    let mut form = Form::new().text("name", request.name.clone());

    // Add enhancement option if specified (process noisy audio)
    // LMNT uses "enhance" parameter to clean up audio
    if request.remove_background_noise {
        form = form.text("enhance", "true");
    }

    // Decode and add audio samples
    for (i, sample_b64) in request.audio_samples.iter().enumerate() {
        // Handle potential data URL prefix
        let audio_data = if sample_b64.contains(',') {
            // Data URL format: data:audio/wav;base64,xxxxx
            let parts: Vec<&str> = sample_b64.splitn(2, ',').collect();
            if parts.len() == 2 {
                parts[1]
            } else {
                sample_b64.as_str()
            }
        } else {
            sample_b64.as_str()
        };

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(audio_data)
            .map_err(|e| VoiceCloneError {
                code: "INVALID_AUDIO".to_string(),
                message: format!("Failed to decode audio sample {}: {}", i, e),
                details: None,
            })?;

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
        created_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        metadata: None,
    })
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
    // Validate name
    if request.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(VoiceCloneError {
                code: "INVALID_NAME".to_string(),
                message: "Voice name cannot be empty".to_string(),
                details: None,
            }),
        ));
    }

    // Route to appropriate provider
    match request.provider {
        VoiceCloneProvider::ElevenLabs => {
            let api_key = state.config.get_api_key("elevenlabs").map_err(|_| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(VoiceCloneError {
                        code: "MISSING_API_KEY".to_string(),
                        message: "ElevenLabs API key not configured".to_string(),
                        details: None,
                    }),
                )
            })?;

            clone_voice_elevenlabs(&api_key, &request)
                .await
                .map(Json)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(e)))
        }
        VoiceCloneProvider::Hume => {
            let api_key = state.config.get_api_key("hume").map_err(|_| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(VoiceCloneError {
                        code: "MISSING_API_KEY".to_string(),
                        message: "Hume API key not configured".to_string(),
                        details: None,
                    }),
                )
            })?;

            clone_voice_hume(&api_key, &request)
                .await
                .map(Json)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(e)))
        }
        VoiceCloneProvider::Lmnt => {
            let api_key = state.config.get_api_key("lmnt").map_err(|_| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(VoiceCloneError {
                        code: "MISSING_API_KEY".to_string(),
                        message: "LMNT API key not configured".to_string(),
                        details: None,
                    }),
                )
            })?;

            clone_voice_lmnt(&api_key, &request)
                .await
                .map(Json)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(e)))
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
            created_at: "2026-01-07T12:00:00Z".to_string(),
            metadata: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"voice_id\":\"voice_lmnt_123\""));
        assert!(json.contains("\"provider\":\"lmnt\""));
        assert!(json.contains("\"status\":\"ready\""));
    }
}

//! Which vendors' voices WaaV can list, with which credential, and how — one table for every
//! caller.
//!
//! `GET /voices` (the operator's credentials), the descriptor resolver (a session's own key, else
//! the operator's) and the per-deployment catalog budapp reads ([`super::voice_catalog`]) all ask
//! the same question: "can this vendor's voices be listed, and what are they?". Each used to keep
//! its own `match` over vendor names, and they drifted: `/voices` listed four vendors, the
//! resolver four, the deployment catalog two. [`CatalogVendor`] is now the only list.
//!
//! A vendor is here when it publishes a voice-list API WaaV can call, or when its voices are a
//! fixed, documented set (OpenAI, Speechmatics, Unreal Speech). Vendors whose code carries only a sample of their
//! voices are left out rather than published as if complete.

use std::time::Duration;

use serde::Deserialize;

use super::voices::{
    Voice, extract_accent_from_code, fetch_azure_voices, fetch_deepgram_voices,
    fetch_elevenlabs_voices, fetch_google_voices, google_voice_credentials, language_code_to_name,
};
use crate::core::voice::MAX_CATALOG_PAGES;
use crate::state::AppState;

/// How long one vendor's listing may take inside `GET /voices`. The vendors are asked at once, so
/// this bounds the whole response; a vendor that does not answer in time is left out and logged.
pub(crate) const VENDOR_LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// What a vendor's listing authenticates with.
#[derive(Clone)]
pub(crate) enum CatalogCredential {
    /// One API key.
    Key(String),
    /// Google: service-account JSON, a path to one, or empty for Application Default Credentials.
    Google(String),
    /// Azure Speech: subscription key and region.
    Azure { key: String, region: String },
    /// IBM Watson: API key, service instance and region.
    Ibm {
        key: String,
        instance: String,
        region: String,
    },
    /// AWS: access key pair and region.
    Aws {
        access_key: String,
        secret_key: String,
        region: String,
    },
}

/// A vendor WaaV can list voices for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogVendor {
    ElevenLabs,
    Deepgram,
    Google,
    Azure,
    Cartesia,
    OpenAi,
    Hume,
    Speechmatics,
    Murf,
    Speechify,
    Resemble,
    WellSaid,
    Smallest,
    IbmWatson,
    AwsPolly,
    UnrealSpeech,
}

impl CatalogVendor {
    pub(crate) const ALL: [Self; 16] = [
        Self::ElevenLabs,
        Self::Deepgram,
        Self::Google,
        Self::Azure,
        Self::Cartesia,
        Self::OpenAi,
        Self::Hume,
        Self::Speechmatics,
        Self::Murf,
        Self::Speechify,
        Self::Resemble,
        Self::WellSaid,
        Self::Smallest,
        Self::IbmWatson,
        Self::AwsPolly,
        Self::UnrealSpeech,
    ];

    /// The key this vendor's voices appear under in `GET /voices`.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::ElevenLabs => "elevenlabs",
            Self::Deepgram => "deepgram",
            Self::Google => "google",
            Self::Azure => "azure",
            Self::Cartesia => "cartesia",
            Self::OpenAi => "openai",
            Self::Hume => "hume",
            Self::Speechmatics => "speechmatics",
            Self::Murf => "murf",
            Self::Speechify => "speechify",
            Self::Resemble => "resemble",
            Self::WellSaid => "wellsaid",
            Self::Smallest => "smallest",
            Self::IbmWatson => "ibm_watson",
            Self::AwsPolly => "aws_polly",
            Self::UnrealSpeech => "unrealspeech",
        }
    }

    /// The vendor a TTS provider name names — the same spellings the TTS factory accepts.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_lowercase().as_str() {
            "elevenlabs" | "eleven_labs" | "11labs" => Self::ElevenLabs,
            "deepgram" => Self::Deepgram,
            "google" | "google-tts" => Self::Google,
            "azure" | "microsoft-azure" | "microsoft_azure" => Self::Azure,
            "cartesia" => Self::Cartesia,
            "openai" | "openai-tts" => Self::OpenAi,
            "hume" => Self::Hume,
            "speechmatics" => Self::Speechmatics,
            "murf" | "murf-ai" | "murf_ai" | "murf.ai" => Self::Murf,
            "speechify" => Self::Speechify,
            "resemble" | "resemble_ai" | "resemble-ai" => Self::Resemble,
            "wellsaid" | "wellsaid_labs" | "wellsaid-labs" => Self::WellSaid,
            "smallest" | "smallest_ai" | "smallest-ai" | "smallest.ai" | "waves" => Self::Smallest,
            "ibm_watson" | "ibm-watson" | "ibm" | "watson" => Self::IbmWatson,
            "aws_polly" | "aws-polly" | "amazon-polly" | "polly" => Self::AwsPolly,
            "unrealspeech" | "unreal_speech" | "unreal-speech" => Self::UnrealSpeech,
            _ => return None,
        })
    }

    /// Whether one API key is all a listing needs, so a session's or deployment's own key can
    /// stand in for the operator's. Google, Azure, IBM and Polly need more than a key.
    pub(crate) fn takes_a_key(self) -> bool {
        !matches!(
            self,
            Self::Google | Self::Azure | Self::IbmWatson | Self::AwsPolly
        )
    }

    /// The credential the OPERATOR configured for this vendor, read where its TTS provider reads
    /// it: the server config, or the environment variable the provider falls back to. `None`
    /// when there is none (Smallest has no operator key at all).
    pub(crate) async fn operator_credential(self, state: &AppState) -> Option<CatalogCredential> {
        let config_key = |name: &str| {
            state
                .config
                .get_api_key(name)
                .ok()
                .map(CatalogCredential::Key)
        };
        let env_key =
            |var: &str| crate::core::credentials::env_api_key(var).map(CatalogCredential::Key);
        match self {
            Self::ElevenLabs => config_key("elevenlabs"),
            Self::Deepgram => config_key("deepgram"),
            Self::Cartesia => config_key("cartesia"),
            Self::OpenAi => config_key("openai"),
            Self::Hume => config_key("hume"),
            Self::Google => google_voice_credentials(state)
                .await
                .map(CatalogCredential::Google),
            Self::Azure => state.config.get_api_key("microsoft-azure").ok().map(|key| {
                CatalogCredential::Azure {
                    key,
                    region: state.config.get_azure_speech_region(),
                }
            }),
            Self::IbmWatson => {
                state
                    .config
                    .get_ibm_watson_credentials()
                    .ok()
                    .map(|(key, instance, region)| CatalogCredential::Ibm {
                        key,
                        instance,
                        region,
                    })
            }
            Self::AwsPolly => {
                state
                    .config
                    .get_aws_credentials()
                    .ok()
                    .map(|(access_key, secret_key, region)| CatalogCredential::Aws {
                        access_key,
                        secret_key,
                        region,
                    })
            }
            Self::Speechmatics => env_key("SPEECHMATICS_API_KEY"),
            Self::Murf => env_key("MURF_API_KEY"),
            Self::Speechify => env_key("SPEECHIFY_API_KEY"),
            Self::Resemble => env_key("RESEMBLE_API_KEY"),
            Self::WellSaid => env_key("WELLSAID_API_KEY"),
            Self::UnrealSpeech => env_key("UNREALSPEECH_API_KEY"),
            Self::Smallest => None,
        }
    }

    /// The vendor's voices, listed with `credential`. Never logs the credential.
    pub(crate) async fn fetch(self, credential: &CatalogCredential) -> Result<Vec<Voice>, String> {
        let fetched = match (self, credential) {
            (Self::Google, CatalogCredential::Google(creds)) => {
                fetch_google_voices(creds).await.map_err(|e| e.to_string())
            }
            (Self::Azure, CatalogCredential::Azure { key, region }) => {
                fetch_azure_voices(key, region)
                    .await
                    .map_err(|e| e.to_string())
            }
            (
                Self::IbmWatson,
                CatalogCredential::Ibm {
                    key,
                    instance,
                    region,
                },
            ) => fetch_ibm_voices(key, instance, region).await,
            (
                Self::AwsPolly,
                CatalogCredential::Aws {
                    access_key,
                    secret_key,
                    region,
                },
            ) => fetch_polly_voices(access_key, secret_key, region).await,
            (vendor, CatalogCredential::Key(key)) if vendor.takes_a_key() => {
                vendor.fetch_with_key(key).await
            }
            _ => Err(format!(
                "{} cannot be listed with this credential",
                self.key()
            )),
        }?;
        Ok(fetched)
    }

    async fn fetch_with_key(self, key: &str) -> Result<Vec<Voice>, String> {
        match self {
            Self::ElevenLabs => fetch_elevenlabs_voices(key)
                .await
                .map_err(|e| e.to_string()),
            Self::Deepgram => fetch_deepgram_voices(key).await.map_err(|e| e.to_string()),
            Self::Cartesia => fetch_cartesia_voices(key).await,
            Self::OpenAi => Ok(openai_voices()),
            Self::Hume => fetch_hume_voices(key).await,
            Self::Speechmatics => Ok(speechmatics_voices()),
            Self::UnrealSpeech => Ok(unrealspeech_voices()),
            Self::Murf => crate::core::tts::murf::MurfTts::list_voices(key)
                .await
                .map(|voices| voices.into_iter().map(voice_from_murf).collect())
                .map_err(|e| e.to_string()),
            Self::Speechify => crate::core::tts::speechify::SpeechifyTts::list_voices(key)
                .await
                .map(|voices| voices.into_iter().map(voice_from_speechify).collect())
                .map_err(|e| e.to_string()),
            Self::Resemble => fetch_resemble_voices(key).await,
            Self::WellSaid => crate::core::tts::wellsaid::WellSaidTts::list_avatars(key)
                .await
                .map(|avatars| avatars.into_iter().map(voice_from_wellsaid).collect())
                .map_err(|e| e.to_string()),
            Self::Smallest => crate::core::tts::smallest::SmallestTts::list_voices(
                key,
                &crate::core::tts::smallest::SmallestModel::default(),
            )
            .await
            .map(|voices| voices.into_iter().map(voice_from_smallest).collect())
            .map_err(|e| e.to_string()),
            Self::Google | Self::Azure | Self::IbmWatson | Self::AwsPolly => {
                Err(format!("{} needs more than an API key", self.key()))
            }
        }
    }
}

fn catalog_client() -> Result<reqwest::Client, String> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(VENDOR_LIST_TIMEOUT)
        .build()
        .map_err(|e| format!("failed to build the voice catalog HTTP client: {e}"))
}

/// A non-2xx answer as an error naming the vendor's status and its own reason.
async fn refused(vendor: &str, response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let reason = crate::core::vendor_error::vendor_message(&body).unwrap_or(body);
    format!("{vendor} voice list answered {status}: {reason}")
}

/// Normalise a vendor's gender word to the resolver's (`male` / `female` / `neutral`); empty when
/// the vendor says nothing usable (`not_specified`).
fn gender_word(raw: Option<&str>) -> String {
    match raw.map(|g| g.trim().to_lowercase()).as_deref() {
        Some("male" | "masculine" | "m") => "male".into(),
        Some("female" | "feminine" | "f") => "female".into(),
        Some("neutral" | "gender_neutral" | "nonbinary" | "non-binary") => "neutral".into(),
        _ => String::new(),
    }
}

/// Language name and accent from a BCP-47 locale (`en-US` → `English`, `American`), the way the
/// Azure and Google catalogs already report them; empty for no locale.
fn language_and_accent(locale: Option<&str>) -> (String, String) {
    match locale
        .map(|l| l.trim().replace('_', "-"))
        .filter(|l| !l.is_empty())
    {
        Some(l) => (language_code_to_name(&l), extract_accent_from_code(&l)),
        None => (String::new(), String::new()),
    }
}

// ---------------------------------------------------------------------------------------------
// Cartesia — GET https://api.cartesia.ai/voices (cursor-paged by voice id)
// ---------------------------------------------------------------------------------------------

const CARTESIA_VOICES_URL: &str = "https://api.cartesia.ai/voices";

#[derive(Deserialize)]
struct CartesiaVoicesPage {
    #[serde(default)]
    data: Vec<CartesiaVoice>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    next_page: Option<String>,
}

#[derive(Deserialize)]
struct CartesiaVoice {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    gender: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    accents: Vec<CartesiaAccent>,
}

#[derive(Deserialize)]
struct CartesiaAccent {
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    is_native: bool,
}

fn voice_from_cartesia(v: CartesiaVoice) -> Voice {
    // The native accent's locale is the most specific thing Cartesia says about a voice's
    // language; `language` (ISO 639-1) is the deprecated fallback.
    let native = v
        .accents
        .iter()
        .find(|a| a.is_native)
        .and_then(|a| a.locale.clone());
    let (language, accent) = language_and_accent(native.as_deref().or(v.language.as_deref()));
    Voice {
        id: v.id,
        name: v.name,
        gender: gender_word(v.gender.as_deref()),
        language,
        accent,
        description: v.description.unwrap_or_default(),
        ..Default::default()
    }
}

async fn fetch_cartesia_voices(key: &str) -> Result<Vec<Voice>, String> {
    let client = catalog_client()?;
    let mut voices = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_CATALOG_PAGES {
        let mut request = client
            .get(CARTESIA_VOICES_URL)
            .query(&[("limit", "100")])
            .bearer_auth(key)
            // The version WaaV synthesizes with, so the catalog and synthesis see one API.
            .header(
                "Cartesia-Version",
                crate::core::tts::cartesia::DEFAULT_API_VERSION,
            );
        if let Some(cursor) = after.as_deref() {
            request = request.query(&[("starting_after", cursor)]);
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(refused("cartesia", response).await);
        }
        let page: CartesiaVoicesPage = response.json().await.map_err(|e| e.to_string())?;
        let last = page.data.last().map(|v| v.id.clone());
        voices.extend(page.data.into_iter().map(voice_from_cartesia));
        match (page.has_more, page.next_page.or(last)) {
            (true, Some(next)) if !next.is_empty() => after = Some(next),
            _ => break,
        }
    }
    Ok(voices)
}

// ---------------------------------------------------------------------------------------------
// OpenAI, Speechmatics and Unreal Speech — fixed, documented sets
// ---------------------------------------------------------------------------------------------

fn openai_voices() -> Vec<Voice> {
    waav_openai_audio::speech::OPENAI_VOICES
        .iter()
        .map(|id| Voice {
            id: id.to_string(),
            name: id.to_string(),
            ..Default::default()
        })
        .collect()
}

fn speechmatics_voices() -> Vec<Voice> {
    use crate::core::tts::speechmatics::SpeechmaticsVoice;
    SpeechmaticsVoice::all()
        .iter()
        .map(|v| {
            let accent = match v.accent() {
                "UK" => "British",
                "US" => "American",
                other => other,
            };
            Voice {
                id: v.as_str().to_string(),
                name: v.to_string(),
                gender: v.gender().to_string(),
                language: "English".to_string(),
                accent: accent.to_string(),
                description: v.description().to_string(),
                ..Default::default()
            }
        })
        .collect()
}

/// Unreal Speech's V8 voices: the API has no list endpoint, and its documentation names each
/// voice with a gender and a language.
fn unrealspeech_voices() -> Vec<Voice> {
    crate::core::tts::unrealspeech::V8_VOICES
        .iter()
        .map(|(id, gender, lang)| Voice {
            id: id.to_string(),
            name: id.to_string(),
            gender: gender.to_string(),
            language: language_code_to_name(lang),
            accent: if lang.contains('-') {
                extract_accent_from_code(lang)
            } else {
                String::new()
            },
            ..Default::default()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Hume — GET https://api.hume.ai/v0/tts/voices?provider=… (page_number-paged)
// ---------------------------------------------------------------------------------------------

const HUME_VOICES_URL: &str = "https://api.hume.ai/v0/tts/voices";

#[derive(Deserialize)]
struct HumeVoicesPage {
    #[serde(default)]
    total_pages: u32,
    #[serde(default)]
    voices_page: Vec<HumeVoice>,
}

#[derive(Deserialize)]
struct HumeVoice {
    name: String,
}

/// Hume's shared library and the account's own voices. Synthesis names a voice by NAME (see
/// `HumeVoiceSpec::by_name`), so the name is the id published here. Hume publishes no gender or
/// language for a voice, so these are for picking, not for matching a description.
async fn fetch_hume_voices(key: &str) -> Result<Vec<Voice>, String> {
    let client = catalog_client()?;
    let mut voices = Vec::new();
    for (library, description) in [
        ("HUME_AI", "Hume voice library"),
        ("CUSTOM_VOICE", "custom voice"),
    ] {
        for page_number in 0..MAX_CATALOG_PAGES as u32 {
            let response = client
                .get(HUME_VOICES_URL)
                .query(&[
                    ("provider", library),
                    ("page_number", &page_number.to_string()),
                    ("page_size", "100"),
                ])
                .header("X-Hume-Api-Key", key)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !response.status().is_success() {
                return Err(refused("hume", response).await);
            }
            let page: HumeVoicesPage = response.json().await.map_err(|e| e.to_string())?;
            let empty = page.voices_page.is_empty();
            voices.extend(page.voices_page.into_iter().map(|v| Voice {
                id: v.name.clone(),
                name: v.name,
                description: description.to_string(),
                ..Default::default()
            }));
            if empty || page_number + 1 >= page.total_pages {
                break;
            }
        }
    }
    Ok(voices)
}

// ---------------------------------------------------------------------------------------------
// Murf, Speechify, WellSaid, Smallest — the providers' own list functions
// ---------------------------------------------------------------------------------------------

fn voice_from_murf(v: crate::core::tts::murf::MurfVoice) -> Voice {
    let (language, accent) = language_and_accent(Some(&v.locale));
    let description = v.description.unwrap_or_default();
    Voice {
        id: v.voice_id,
        name: v.display_name,
        gender: gender_word(v.gender.as_deref()),
        language,
        // Murf's own accent label ("US & Canada") when it has one.
        accent: v.accent.filter(|a| !a.is_empty()).unwrap_or(accent),
        // Murf's description IS the age band ("Middle-Aged", "Young Adult").
        age: crate::core::voice::Age::from_str(&description)
            .map(|a| a.to_string())
            .unwrap_or_default(),
        use_case: v
            .supported_locales
            .get(&v.locale)
            .map(|l| l.available_styles.join(", "))
            .unwrap_or_default(),
        description,
        ..Default::default()
    }
}

fn voice_from_speechify(v: crate::core::tts::speechify::SpeechifyVoice) -> Voice {
    let (language, accent) = language_and_accent(v.locale.as_deref());
    Voice {
        id: v.id,
        name: v.display_name,
        sample: v.preview_audio.unwrap_or_default(),
        gender: gender_word(v.gender.as_deref()),
        language,
        accent,
        description: v.tags.unwrap_or_default().join(", "),
        ..Default::default()
    }
}

fn voice_from_wellsaid(a: crate::core::tts::wellsaid::WellSaidAvatar) -> Voice {
    let (language, _) = language_and_accent(a.locale.as_deref());
    Voice {
        // The numeric speaker id is what a WellSaid synthesis request takes.
        id: a.speaker_id.to_string(),
        name: a.name,
        sample: a.preview_audio.unwrap_or_default(),
        gender: gender_word(a.gender.as_deref()),
        language: a.language.filter(|l| !l.is_empty()).unwrap_or(language),
        accent: a.accent_type.unwrap_or_default(),
        description: a.characteristics.join(", "),
        use_case: a.style.unwrap_or_default(),
        ..Default::default()
    }
}

fn voice_from_smallest(v: crate::core::tts::smallest::SmallestVoice) -> Voice {
    let capitalise = |s: &str| {
        let mut c = s.chars();
        c.next()
            .map(|f| f.to_uppercase().chain(c).collect())
            .unwrap_or_default()
    };
    Voice {
        id: v.voice_id,
        name: v.display_name,
        sample: v.audio_preview.unwrap_or_default(),
        gender: gender_word(v.tags.gender.as_deref()),
        language: v
            .tags
            .language
            .first()
            .map(|l| capitalise(l))
            .unwrap_or_default(),
        accent: v.tags.accent.as_deref().map(capitalise).unwrap_or_default(),
        age: v
            .tags
            .age
            .as_deref()
            .and_then(crate::core::voice::Age::from_str)
            .map(|a| a.to_string())
            .unwrap_or_default(),
        description: v.tags.emotions.join(", "),
        use_case: v.tags.usecases.join(", "),
    }
}

// ---------------------------------------------------------------------------------------------
// Resemble — GET https://app.resemble.ai/api/v2/voices (page-numbered)
// ---------------------------------------------------------------------------------------------

async fn fetch_resemble_voices(key: &str) -> Result<Vec<Voice>, String> {
    let mut voices = Vec::new();
    for page in 1..=MAX_CATALOG_PAGES as u32 {
        let response = crate::core::tts::resemble::ResembleTts::list_voices(key, page, 100)
            .await
            .map_err(|e| e.to_string())?;
        let empty = response.items.is_empty();
        voices.extend(response.items.into_iter().map(|v| {
            let (language, accent) = language_and_accent(v.default_language.as_deref());
            Voice {
                id: v.uuid,
                name: v.name,
                language,
                accent,
                ..Default::default()
            }
        }));
        let last_page = response.num_pages.map(|n| page >= n).unwrap_or(empty);
        if empty || last_page {
            break;
        }
    }
    Ok(voices)
}

// ---------------------------------------------------------------------------------------------
// IBM Watson — GET https://{region host}/instances/{instance}/v1/voices
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct IbmVoices {
    #[serde(default)]
    voices: Vec<IbmVoiceEntry>,
}

#[derive(Deserialize)]
struct IbmVoiceEntry {
    name: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    gender: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// The display name inside an IBM voice id: `en-US_LisaV3Voice` → `Lisa`.
fn ibm_display_name(id: &str) -> String {
    let after_locale = id.split_once('_').map(|(_, rest)| rest).unwrap_or(id);
    let trimmed = after_locale.trim_end_matches("Voice");
    let without_version = trimmed
        .rfind('V')
        .filter(|&i| trimmed[i + 1..].chars().all(|c| c.is_ascii_digit()) && i + 1 < trimmed.len())
        .map(|i| &trimmed[..i])
        .unwrap_or(trimmed);
    without_version.to_string()
}

fn ibm_voices_url(instance: &str, region: &str) -> Result<String, String> {
    use crate::core::stt::ibm_watson::IbmRegion;
    let region =
        IbmRegion::parse(region).ok_or_else(|| format!("unknown IBM Watson region `{region}`"))?;
    crate::core::net::validate_path_segment("ibm_watson_instance_id", instance)?;
    Ok(format!(
        "https://{}/instances/{instance}/v1/voices",
        region.tts_hostname()
    ))
}

async fn fetch_ibm_voices(key: &str, instance: &str, region: &str) -> Result<Vec<Voice>, String> {
    let url = ibm_voices_url(instance, region)?;
    // IBM Cloud service keys authenticate directly as Basic `apikey:<key>`.
    let response = catalog_client()?
        .get(url)
        .basic_auth("apikey", Some(key))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(refused("ibm_watson", response).await);
    }
    let listed: IbmVoices = response.json().await.map_err(|e| e.to_string())?;
    Ok(listed
        .voices
        .into_iter()
        .map(|v| {
            let (language, accent) = language_and_accent(v.language.as_deref());
            Voice {
                name: ibm_display_name(&v.name),
                id: v.name,
                gender: gender_word(v.gender.as_deref()),
                language,
                accent,
                description: v.description.unwrap_or_default(),
                ..Default::default()
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------------------------
// Amazon Polly — DescribeVoices (NextToken-paged), through the provider's own client
// ---------------------------------------------------------------------------------------------

async fn fetch_polly_voices(
    access_key: &str,
    secret_key: &str,
    region: &str,
) -> Result<Vec<Voice>, String> {
    let credentials =
        aws_credential_types::Credentials::new(access_key, secret_key, None, None, "waav");
    let client = crate::core::tts::aws_polly::polly_client(
        aws_config::Region::new(region.to_string()),
        Some(credentials),
        None,
    )
    .await;
    let mut voices = Vec::new();
    let mut next: Option<String> = None;
    for _ in 0..MAX_CATALOG_PAGES {
        let page = client
            .describe_voices()
            .set_next_token(next.take())
            .send()
            .await
            .map_err(|e| {
                format!(
                    "aws_polly DescribeVoices failed: {}",
                    aws_sdk_polly::error::DisplayErrorContext(&e)
                )
            })?;
        voices.extend(page.voices().iter().map(|v| {
            let (_, accent) = language_and_accent(v.language_code().map(|c| c.as_str()));
            Voice {
                id: v.id().map(|id| id.as_str().to_string()).unwrap_or_default(),
                name: v.name().unwrap_or_default().to_string(),
                gender: gender_word(v.gender().map(|g| g.as_str())),
                language: v.language_name().unwrap_or_default().to_string(),
                accent,
                ..Default::default()
            }
        }));
        match page.next_token() {
            Some(token) if !token.is_empty() => next = Some(token.to_string()),
            _ => break,
        }
    }
    Ok(voices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vendor_round_trips_through_its_key() {
        for vendor in CatalogVendor::ALL {
            assert_eq!(
                CatalogVendor::from_name(vendor.key()),
                Some(vendor),
                "{vendor:?}"
            );
        }
        assert_eq!(
            CatalogVendor::from_name("microsoft-azure"),
            Some(CatalogVendor::Azure)
        );
        assert_eq!(
            CatalogVendor::from_name("polly"),
            Some(CatalogVendor::AwsPolly)
        );
        assert_eq!(CatalogVendor::from_name("baidu"), None);
    }

    /// Every vendor named here is one the TTS factory can synthesize with, under the same name —
    /// a catalog for a vendor WaaV cannot speak with would be a list of unusable voices.
    #[test]
    fn every_vendor_is_a_tts_provider() {
        for vendor in CatalogVendor::ALL {
            assert!(
                crate::plugin::global_registry().has_tts_provider(vendor.key()),
                "{} is not a TTS provider name",
                vendor.key()
            );
        }
    }

    #[test]
    fn multi_part_credentials_never_take_a_bare_key() {
        for vendor in [
            CatalogVendor::Google,
            CatalogVendor::Azure,
            CatalogVendor::IbmWatson,
            CatalogVendor::AwsPolly,
        ] {
            assert!(!vendor.takes_a_key());
        }
        assert!(CatalogVendor::Cartesia.takes_a_key());
    }

    /// Live Cartesia `GET /voices` (version 2025-04-16), trimmed to the fields read.
    #[test]
    fn a_cartesia_page_maps_to_voices_with_the_native_locale() {
        let page: CartesiaVoicesPage = serde_json::from_str(
            r#"{"data": [{"id": "db6b0ed5", "name": "Skylar - Friendly Guide",
                "gender": "feminine", "language": "en",
                "description": "Approachable American female ideal for customer care",
                "accents": [{"accent": "standard-hindi", "locale": "hi-IN", "is_native": false},
                            {"accent": "general-american", "locale": "en-US", "is_native": true}]}],
                "has_more": true, "next_page": "db6b0ed5"}"#,
        )
        .unwrap();
        assert!(page.has_more);
        let v = voice_from_cartesia(page.data.into_iter().next().unwrap());
        assert_eq!(v.id, "db6b0ed5");
        assert_eq!(v.gender, "female");
        assert_eq!(v.language, "English");
        assert_eq!(v.accent, "American");
    }

    /// Murf's documented `ApiVoice` example.
    #[test]
    fn the_documented_murf_voice_maps_with_its_age_band() {
        let voices: Vec<crate::core::tts::murf::MurfVoice> = serde_json::from_str(
            r#"[{"description": "Middle-Aged", "displayName": "Ken (M)", "gender": "Male",
                "locale": "en-US",
                "supportedLocales": {"en-US": {"availableStyles": ["Conversational", "Promo"],
                                               "detail": "English (US & Canada)"}},
                "voiceId": "Ken", "accent": "US & Canada",
                "availableStyles": ["Conversational", "Promo"], "displayLanguage": "English"}]"#,
        )
        .unwrap();
        let v = voice_from_murf(voices.into_iter().next().unwrap());
        assert_eq!(v.id, "Ken");
        assert_eq!(v.name, "Ken (M)");
        assert_eq!(v.gender, "male");
        assert_eq!(v.language, "English");
        assert_eq!(v.accent, "US & Canada");
        assert_eq!(v.age, "middle_aged");
        assert_eq!(v.use_case, "Conversational, Promo");
    }

    /// Speechify's documented `GET /v1/voices` example.
    #[test]
    fn the_documented_speechify_page_parses_and_maps() {
        let page: crate::core::tts::speechify::SpeechifyVoicesResponse = serde_json::from_str(
            r#"{"next_cursor": "c1", "has_more": true, "voices": [{
                "display_name": "Geffen", "gender": "male", "locale": "en-US", "id": "geffen_32",
                "models": [{"languages": [{"locale": "en-US", "preview_audio": "https://p"}],
                            "name": "simba-3.5"}],
                "type": "shared", "avatar_image": null, "preview_audio": "https://p",
                "tags": ["calm"]}]}"#,
        )
        .unwrap();
        assert!(page.has_more);
        assert_eq!(page.next_cursor.as_deref(), Some("c1"));
        let raw = page.voices.into_iter().next().unwrap();
        assert!(!raw.is_cloned());
        let v = voice_from_speechify(raw);
        assert_eq!(v.id, "geffen_32");
        assert_eq!(v.name, "Geffen");
        assert_eq!(v.accent, "American");
        assert_eq!(v.description, "calm");
    }

    /// Live Smallest `lightning-v3.1/get_voices` entry (the endpoint is public).
    #[test]
    fn a_smallest_voice_maps_its_tags() {
        let page: crate::core::tts::smallest::SmallestVoicesResponse = serde_json::from_str(
            r#"{"voices": [{"voiceId": "olivia", "displayName": "Olivia",
                "tags": {"age": "young", "emotions": ["Friendly"], "language": ["english", "spanish"],
                         "usecases": ["Narration"], "accent": "american", "gender": "female"},
                "audioPreview": "https://x/olivia.wav"}]}"#,
        )
        .unwrap();
        let v = voice_from_smallest(page.voices.into_iter().next().unwrap());
        assert_eq!(v.id, "olivia");
        assert_eq!(v.gender, "female");
        assert_eq!(v.language, "English");
        assert_eq!(v.accent, "American");
        assert_eq!(v.age, "young");
        assert_eq!(v.sample, "https://x/olivia.wav");
    }

    #[test]
    fn ibm_names_and_urls() {
        assert_eq!(ibm_display_name("en-US_LisaV3Voice"), "Lisa");
        assert_eq!(ibm_display_name("en-GB_CharlotteV3Voice"), "Charlotte");
        assert_eq!(ibm_display_name("de-DE_BirgitVoice"), "Birgit");
        assert_eq!(
            ibm_voices_url("inst-1", "eu-de").unwrap(),
            "https://api.eu-de.text-to-speech.watson.cloud.ibm.com/instances/inst-1/v1/voices"
        );
        assert!(ibm_voices_url("../x", "us-south").is_err());
        assert!(ibm_voices_url("inst-1", "mars-1").is_err());
    }

    #[test]
    fn the_fixed_sets_are_complete() {
        assert_eq!(
            openai_voices().len(),
            waav_openai_audio::speech::OPENAI_VOICES.len()
        );
        let sm = speechmatics_voices();
        assert_eq!(sm.len(), 4);
        assert!(sm.iter().any(|v| v.id == "megan" && v.accent == "American"));
        let unreal = unrealspeech_voices();
        assert_eq!(
            unreal.len(),
            crate::core::tts::unrealspeech::V8_VOICES.len()
        );
        assert!(unreal.iter().any(|v| v.id == "Sierra"
            && v.gender == "female"
            && v.language == "English"
            && v.accent == "American"));
        assert!(
            unreal
                .iter()
                .any(|v| v.id == "Arjun" && v.language == "Hindi" && v.accent.is_empty())
        );
    }

    #[test]
    fn gender_words_normalise() {
        assert_eq!(gender_word(Some("Male")), "male");
        assert_eq!(gender_word(Some("feminine")), "female");
        assert_eq!(gender_word(Some("gender_neutral")), "neutral");
        assert_eq!(gender_word(Some("not_specified")), "");
        assert_eq!(gender_word(None), "");
    }
}

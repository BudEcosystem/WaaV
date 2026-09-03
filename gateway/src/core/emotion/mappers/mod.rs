//! Provider-specific emotion mappers.
//!
//! This module contains emotion mapper implementations for each TTS provider,
//! translating the unified `EmotionConfig` to provider-specific formats.
//!
//! # Available Mappers
//!
//! | Mapper | Provider | Method | Description |
//! |--------|----------|--------|-------------|
//! | `HumeEmotionMapper` | Hume AI | Natural language | Free-form descriptions |
//! | `ElevenLabsEmotionMapper` | ElevenLabs | Voice settings | stability, style |
//! | `AzureEmotionMapper` | Azure | SSML | express-as styles |
//! | `FallbackEmotionMapper` | Others | None | Warning generation |
//!
//! # Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::emotion::{EmotionConfig, Emotion};
//! use waav_gateway::core::emotion::mappers::{
//!     HumeEmotionMapper, ElevenLabsEmotionMapper, AzureEmotionMapper,
//!     get_mapper_for_provider,
//! };
//! use waav_gateway::core::emotion::EmotionMapper;
//!
//! let config = EmotionConfig::with_emotion(Emotion::Happy);
//!
//! // Get mapper for a specific provider
//! let mapper = get_mapper_for_provider("hume");
//! let mapped = mapper.map_emotion(&config);
//!
//! println!("Hume description: {:?}", mapped.description);
//! ```

mod azure;
mod cartesia;
mod elevenlabs;
mod fallback;
mod hume;
mod openai;

pub use azure::AzureEmotionMapper;
pub use cartesia::CartesiaEmotionMapper;
pub use elevenlabs::ElevenLabsEmotionMapper;
pub use fallback::FallbackEmotionMapper;
pub use hume::{HumeEmotionMapper, MAX_DESCRIPTION_LENGTH};
pub use openai::OpenAiEmotionMapper;

use super::mapper::EmotionMapper;

/// Returns the appropriate emotion mapper for a given provider.
///
/// # Arguments
///
/// * `provider` - The provider name (case-insensitive)
///
/// # Returns
///
/// A boxed emotion mapper trait object.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::emotion::mappers::get_mapper_for_provider;
/// use waav_gateway::core::emotion::EmotionMapper;
///
/// let hume_mapper = get_mapper_for_provider("hume");
/// assert!(hume_mapper.supports_free_description());
///
/// let deepgram_mapper = get_mapper_for_provider("deepgram");
/// assert!(!deepgram_mapper.get_support().supports_emotions);
/// ```
pub fn get_mapper_for_provider(provider: &str) -> Box<dyn EmotionMapper> {
    match provider.to_lowercase().as_str() {
        "hume" | "hume-ai" | "hume_ai" => Box::new(HumeEmotionMapper::new()),
        "elevenlabs" | "eleven_labs" => Box::new(ElevenLabsEmotionMapper::new()),
        "azure" | "microsoft-azure" | "microsoft_azure" => Box::new(AzureEmotionMapper::new()),
        // P4: Cartesia is now a REAL native-emotion-string mapper (was a fallback stub).
        "cartesia" => Box::new(CartesiaEmotionMapper::new()),
        // P4: OpenAI's instruction models synthesize a natural-language `instructions`
        // string. NOTE: the older tts-1 / tts-1-hd models do NOT accept instructions —
        // those route to `FallbackEmotionMapper::openai_legacy()` via the model-aware
        // helper `get_mapper_for_provider_model`.
        "openai" | "openai-tts" | "gpt-4o-mini-tts" | "gpt-realtime" => {
            Box::new(OpenAiEmotionMapper::new())
        }
        // Natural-language S2S / prompt-steered TTS reuse the OpenAI instruction
        // synthesizer; the S2S handler injects `instruction_text` into the session
        // system prompt (Nova-Sonic) / `setup.system_instruction` (Gemini-Live), and
        // gemini-tts into the per-turn prompt.
        "gemini-tts" | "gemini_tts" | "nova-sonic" | "nova_sonic" | "gemini-live"
        | "gemini_live" => Box::new(OpenAiEmotionMapper::new()),
        "deepgram" => Box::new(FallbackEmotionMapper::deepgram()),
        // Plain google-standard / Chirp3-HD have no emotion knob (gemini-tts above does).
        "google" => Box::new(FallbackEmotionMapper::google()),
        "ibm-watson" | "ibm_watson" | "watson" | "ibm" => {
            Box::new(FallbackEmotionMapper::ibm_watson())
        }
        // AWS Polly NEURAL honors two DELIVERY domains (newscast/conversational) but no
        // affective emotion → a dedicated PARTIAL mapper (not a bare fallback).
        "aws-polly" | "aws_polly" | "amazon-polly" | "polly" => {
            Box::new(FallbackEmotionMapper::aws_polly())
        }
        "lmnt" | "lmnt-ai" | "lmnt_ai" => Box::new(FallbackEmotionMapper::lmnt()),
        _ => Box::new(FallbackEmotionMapper::new("unknown")),
    }
}

/// Model-aware variant of [`get_mapper_for_provider`]. The ONLY provider whose
/// emotion mechanism depends on the model is OpenAI: `gpt-4o-mini-tts` / `gpt-realtime`
/// accept `instructions` (→ [`OpenAiEmotionMapper`]), while the legacy `tts-1` /
/// `tts-1-hd` do NOT (→ fallback-warn). Everything else ignores `model`.
pub fn get_mapper_for_provider_model(provider: &str, model: &str) -> Box<dyn EmotionMapper> {
    let p = provider.to_lowercase();
    if p == "openai" || p == "openai-tts" {
        let m = model.to_lowercase();
        if m.starts_with("tts-1") {
            return Box::new(FallbackEmotionMapper::openai_legacy());
        }
    }
    get_mapper_for_provider(provider)
}

/// Returns whether a provider has any emotion support.
///
/// # Arguments
///
/// * `provider` - The provider name (case-insensitive)
///
/// # Returns
///
/// `true` if the provider supports at least some emotion control.
#[inline]
pub fn provider_supports_emotions(provider: &str) -> bool {
    get_mapper_for_provider(provider)
        .get_support()
        .has_any_support()
}

/// Returns the list of providers with full emotion support.
///
/// These providers support custom emotions, intensity, and/or free descriptions.
#[inline]
pub fn providers_with_emotion_support() -> &'static [&'static str] {
    &[
        "hume",
        "elevenlabs",
        "azure",
        "cartesia",
        "openai",
        "gemini-tts",
        "nova-sonic",
        "gemini-live",
    ]
}

/// Returns the list of providers without emotion support.
///
/// These providers only support voice selection, not emotional expression.
/// Note: LMNT supports `top_p`/`temperature` for expressiveness but not emotions.
/// `google` here is the plain/Chirp3-HD path (gemini-tts DOES support emotion);
/// `aws-polly` honors two delivery domains but no affective emotion (partial).
#[inline]
pub fn providers_without_emotion_support() -> &'static [&'static str] {
    &["deepgram", "google", "ibm-watson", "lmnt"]
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::emotion::mapper::EmotionMethod;

    #[test]
    fn test_get_mapper_hume() {
        let mapper = get_mapper_for_provider("hume");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "hume");
        assert!(support.supports_free_description);
        assert_eq!(support.method, EmotionMethod::NaturalLanguage);
    }

    #[test]
    fn test_get_mapper_hume_variants() {
        for provider in ["hume", "hume-ai", "hume_ai", "HUME"] {
            let mapper = get_mapper_for_provider(provider);
            assert_eq!(mapper.get_support().provider_id, "hume");
        }
    }

    #[test]
    fn test_get_mapper_elevenlabs() {
        let mapper = get_mapper_for_provider("elevenlabs");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "elevenlabs");
        assert_eq!(support.method, EmotionMethod::VoiceSettings);
    }

    #[test]
    fn test_get_mapper_azure() {
        let mapper = get_mapper_for_provider("azure");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "azure");
        assert_eq!(support.method, EmotionMethod::Ssml);
    }

    #[test]
    fn test_get_mapper_azure_variants() {
        for provider in ["azure", "microsoft-azure", "microsoft_azure", "AZURE"] {
            let mapper = get_mapper_for_provider(provider);
            assert_eq!(mapper.get_support().provider_id, "azure");
        }
    }

    #[test]
    fn test_get_mapper_deepgram_fallback() {
        let mapper = get_mapper_for_provider("deepgram");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "deepgram");
        assert!(!support.supports_emotions);
        assert_eq!(support.method, EmotionMethod::None);
    }

    #[test]
    fn test_get_mapper_cartesia_real() {
        // P4: Cartesia is now a REAL native-emotion-string mapper.
        let mapper = get_mapper_for_provider("cartesia");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "cartesia");
        assert!(support.supports_emotions);
        assert_eq!(support.method, EmotionMethod::NativeEmotionString);
    }

    #[test]
    fn test_get_mapper_google_fallback() {
        let mapper = get_mapper_for_provider("google");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "google");
        assert!(!support.supports_emotions);
    }

    #[test]
    fn test_get_mapper_ibm_fallback() {
        for provider in ["ibm-watson", "ibm_watson", "watson", "ibm"] {
            let mapper = get_mapper_for_provider(provider);
            assert_eq!(mapper.get_support().provider_id, "ibm-watson");
        }
    }

    #[test]
    fn test_get_mapper_aws_fallback() {
        for provider in ["aws-polly", "aws_polly", "amazon-polly", "polly"] {
            let mapper = get_mapper_for_provider(provider);
            assert_eq!(mapper.get_support().provider_id, "aws-polly");
        }
    }

    #[test]
    fn test_get_mapper_openai_real() {
        // P4: modern OpenAI (gpt-4o-mini-tts/gpt-realtime) synthesizes `instructions`.
        let mapper = get_mapper_for_provider("openai");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "openai");
        assert!(support.supports_emotions);
        assert!(support.supports_free_description);
        assert_eq!(support.method, EmotionMethod::InstructionText);
    }

    #[test]
    fn test_get_mapper_openai_legacy_model_is_fallback() {
        // tts-1 / tts-1-hd do NOT accept instructions → fallback-warn.
        let legacy = get_mapper_for_provider_model("openai", "tts-1-hd");
        assert!(!legacy.get_support().supports_emotions);
        // gpt-4o-mini-tts → real instruction mapper.
        let modern = get_mapper_for_provider_model("openai", "gpt-4o-mini-tts");
        assert!(modern.get_support().supports_emotions);
    }

    #[test]
    fn test_get_mapper_aws_polly_partial_style() {
        // Polly NEURAL honors newscast/conversational delivery domains (no affect).
        let mapper = get_mapper_for_provider("aws-polly");
        let support = mapper.get_support();
        assert!(!support.supports_emotions);
        assert!(support.supports_style);
    }

    #[test]
    fn test_get_mapper_lmnt_fallback() {
        let mapper = get_mapper_for_provider("lmnt");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "lmnt");
        assert!(!support.supports_emotions);
        assert_eq!(support.method, EmotionMethod::None);
    }

    #[test]
    fn test_get_mapper_lmnt_variants() {
        for provider in ["lmnt", "lmnt-ai", "lmnt_ai", "LMNT"] {
            let mapper = get_mapper_for_provider(provider);
            assert_eq!(mapper.get_support().provider_id, "lmnt");
        }
    }

    #[test]
    fn test_get_mapper_unknown() {
        let mapper = get_mapper_for_provider("unknown-provider");
        let support = mapper.get_support();

        assert_eq!(support.provider_id, "unknown");
        assert!(!support.supports_emotions);
    }

    #[test]
    fn test_provider_supports_emotions() {
        assert!(provider_supports_emotions("hume"));
        assert!(provider_supports_emotions("elevenlabs"));
        assert!(provider_supports_emotions("azure"));
        // P4: Cartesia + OpenAI are now real.
        assert!(provider_supports_emotions("cartesia"));
        assert!(provider_supports_emotions("openai"));

        assert!(!provider_supports_emotions("deepgram"));
        assert!(!provider_supports_emotions("google"));
    }

    #[test]
    fn test_providers_with_emotion_support() {
        let providers = providers_with_emotion_support();
        assert!(providers.contains(&"hume"));
        assert!(providers.contains(&"elevenlabs"));
        assert!(providers.contains(&"azure"));
        assert!(providers.contains(&"cartesia"));
        assert!(providers.contains(&"openai"));
        assert!(!providers.contains(&"deepgram"));
    }

    #[test]
    fn test_providers_without_emotion_support() {
        let providers = providers_without_emotion_support();
        assert!(providers.contains(&"deepgram"));
        assert!(providers.contains(&"google"));
        assert!(providers.contains(&"lmnt"));
        assert!(!providers.contains(&"hume"));
        // P4: cartesia/openai are no longer in the no-support list.
        assert!(!providers.contains(&"cartesia"));
        assert!(!providers.contains(&"openai"));
    }

    #[test]
    fn test_lmnt_provider_no_emotion_support() {
        assert!(!provider_supports_emotions("lmnt"));
        assert!(!provider_supports_emotions("lmnt-ai"));
        assert!(!provider_supports_emotions("lmnt_ai"));
    }
}

//! Built-in Provider Registrations
//!
//! This module registers all built-in STT, TTS, and Realtime providers
//! with the plugin registry using the `inventory` crate.
//!
//! # Providers
//!
//! ## STT Providers (11)
//! - Deepgram, Google, ElevenLabs, Azure, Cartesia, OpenAI, AssemblyAI,
//!   AWS Transcribe, IBM Watson, Groq, Gnani
//!
//! ## TTS Providers (12)
//! - Deepgram, ElevenLabs, Google, Azure, Cartesia, OpenAI, AWS Polly,
//!   IBM Watson, Hume, LMNT, PlayHT, Gnani
//!
//! ## Realtime Providers (2)
//! - OpenAI, Hume EVI

use crate::core::realtime::{BaseRealtime, HumeEVI, OpenAIRealtime, RealtimeConfig, RealtimeError};
use crate::core::stt::{
    AssemblyAISTT, AwsTranscribeSTT, AzureSTT, BaseSTT, CartesiaSTT, DeepgramSTT, ElevenLabsSTT,
    GnaniSTT, GoogleSTT, GroqSTT, IbmWatsonSTT, OpenAISTT, STTConfig, STTError,
};
use crate::core::tts::{
    AwsPollyTTS, AzureTTS, BaseTTS, CartesiaTTS, DeepgramTTS, ElevenLabsTTS, GnaniTTS, GoogleTTS,
    HumeTTS, IbmWatsonTTS, LmntTts, OpenAITTS, PlayHtTts, TTSConfig,
};
use crate::plugin::metadata::ProviderMetadata;
use crate::plugin::registry::PluginConstructor;

// ============================================================================
// STT Provider Metadata Functions
// ============================================================================

fn deepgram_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("deepgram", "Deepgram Nova-3")
        .with_description("Real-time streaming STT with high accuracy")
        .with_features([
            "streaming",
            "word-timestamps",
            "speaker-diarization",
            "punctuation",
        ])
        .with_languages(["en", "es", "fr", "de", "it", "pt", "nl", "ja", "ko", "zh"])
}

fn google_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("google", "Google Speech-to-Text v2")
        .with_description("Google Cloud Speech-to-Text v2 API with enhanced models")
        .with_features([
            "streaming",
            "word-timestamps",
            "speaker-diarization",
            "punctuation",
        ])
        .with_languages(["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"])
}

fn elevenlabs_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("elevenlabs", "ElevenLabs STT")
        .with_description("Real-time streaming STT from ElevenLabs")
        .with_features(["streaming", "word-timestamps"])
}

fn azure_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("microsoft-azure", "Microsoft Azure Speech")
        .with_description("Microsoft Azure Cognitive Services Speech-to-Text")
        .with_alias("azure")
        .with_features(["streaming", "word-timestamps", "punctuation"])
        .with_languages(["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"])
}

fn cartesia_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("cartesia", "Cartesia Ink-Whisper")
        .with_description("Low-latency streaming STT using Ink-Whisper model")
        .with_features(["streaming", "low-latency"])
}

fn openai_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("openai", "OpenAI Whisper")
        .with_description("OpenAI Whisper API for speech recognition")
        .with_features(["word-timestamps", "translation"])
        .with_models(["whisper-1"])
}

fn assemblyai_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("assemblyai", "AssemblyAI v3")
        .with_description("AssemblyAI Streaming Speech-to-Text v3 API")
        .with_features([
            "streaming",
            "word-timestamps",
            "speaker-diarization",
            "sentiment-analysis",
        ])
}

fn aws_transcribe_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("aws-transcribe", "Amazon Transcribe")
        .with_description("Amazon Transcribe Streaming API")
        .with_alias("transcribe")
        .with_features(["streaming", "word-timestamps", "speaker-diarization"])
        .with_languages(["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"])
}

fn ibm_watson_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("ibm-watson", "IBM Watson STT")
        .with_description("IBM Watson Speech-to-Text WebSocket API")
        .with_alias("watson")
        .with_features(["streaming", "word-timestamps", "speaker-diarization"])
}

fn groq_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("groq", "Groq Whisper")
        .with_description("Ultra-fast Whisper inference on Groq (216x real-time)")
        .with_features(["fast-inference", "translation"])
        .with_models([
            "whisper-large-v3",
            "whisper-large-v3-turbo",
            "distil-whisper-large-v3-en",
        ])
}

fn gnani_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("gnani", "Gnani Vachana STT")
        .with_description("Indic speech-to-text with 14 language support via REST API")
        .with_alias("vachana")
        .with_features(["indic-languages", "interim-results", "word-timestamps"])
        .with_languages([
            "kn-IN",
            "hi-IN",
            "ta-IN",
            "te-IN",
            "gu-IN",
            "mr-IN",
            "bn-IN",
            "ml-IN",
            "pa-guru-IN",
            "ur-IN",
            "en-IN",
            "en-GB",
            "en-US",
            "en-SG",
        ])
}

// ============================================================================
// TTS Provider Metadata Functions
// ============================================================================

fn deepgram_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("deepgram", "Deepgram Aura")
        .with_description("Real-time TTS with Aura voice models")
        .with_features(["streaming", "websocket"])
}

fn elevenlabs_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("elevenlabs", "ElevenLabs TTS")
        .with_description("High-quality voice synthesis with emotion control")
        .with_features(["streaming", "voice-cloning", "emotion-control"])
}

fn google_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("google", "Google Cloud TTS")
        .with_description("Google Cloud Text-to-Speech API with WaveNet voices")
        .with_features(["ssml", "neural-voices"])
}

fn azure_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("microsoft-azure", "Microsoft Azure TTS")
        .with_description("Microsoft Azure Cognitive Services Text-to-Speech")
        .with_alias("azure")
        .with_features(["streaming", "ssml", "neural-voices"])
}

fn cartesia_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("cartesia", "Cartesia Sonic")
        .with_description("Low-latency TTS with Sonic voice models")
        .with_features(["streaming", "low-latency", "voice-cloning"])
}

fn openai_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("openai", "OpenAI TTS")
        .with_description("OpenAI Text-to-Speech API")
        .with_models(["tts-1", "tts-1-hd", "gpt-4o-mini-tts"])
        .with_features(["streaming"])
}

fn aws_polly_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("aws-polly", "Amazon Polly")
        .with_description("Amazon Polly Text-to-Speech API")
        .with_alias("polly")
        .with_features(["ssml", "neural-voices"])
}

fn ibm_watson_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("ibm-watson", "IBM Watson TTS")
        .with_description("IBM Watson Text-to-Speech API")
        .with_alias("watson")
        .with_features(["streaming", "ssml"])
}

fn hume_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("hume", "Hume AI Octave")
        .with_description("Empathic TTS with natural language emotion control")
        .with_alias("hume-ai")
        .with_features(["streaming", "emotion-control"])
}

fn lmnt_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("lmnt", "LMNT TTS")
        .with_description("Ultra-low latency TTS (~150ms)")
        .with_alias("lmnt-ai")
        .with_features(["streaming", "low-latency", "voice-cloning"])
}

fn playht_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("playht", "Play.ht TTS")
        .with_description("Voice cloning TTS with ultra-realistic voices (~190ms)")
        .with_alias("play.ht")
        .with_features(["streaming", "voice-cloning"])
}

fn gnani_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("gnani", "Gnani TTS")
        .with_description("Multi-speaker Indic TTS with 12 languages and SSML gender support")
        .with_alias("gnani-ai")
        .with_features(["multi-speaker", "ssml-gender", "indic-languages"])
        .with_languages([
            "En-IN", "Hi-IN", "Hi-IN-al", "Kn-IN", "Ta-IN", "Te-IN", "Mr-IN", "Ml-IN", "Gu-IN",
            "Bn-IN", "Pa-IN", "Ne-NP",
        ])
}

// ============================================================================
// Realtime Provider Metadata Functions
// ============================================================================

fn openai_realtime_metadata() -> ProviderMetadata {
    ProviderMetadata::realtime("openai", "OpenAI Realtime")
        .with_description("OpenAI Realtime API with GPT-4o for bidirectional audio")
        .with_models([
            "gpt-4o-realtime-preview",
            "gpt-4o-realtime-preview-2024-10-01",
        ])
        .with_features(["full-duplex", "function-calling", "turn-detection"])
}

fn hume_evi_realtime_metadata() -> ProviderMetadata {
    ProviderMetadata::realtime("hume", "Hume EVI")
        .with_description("Hume Empathic Voice Interface with emotion analysis")
        .with_alias("evi")
        .with_features(["full-duplex", "emotion-analysis", "prosody-scores"])
}

// ============================================================================
// STT Factory Functions
// ============================================================================

fn create_deepgram_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(DeepgramSTT::new(config)?))
}

fn create_google_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(GoogleSTT::new(config)?))
}

fn create_elevenlabs_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(ElevenLabsSTT::new(config)?))
}

fn create_azure_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(AzureSTT::new(config)?))
}

fn create_cartesia_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(CartesiaSTT::new(config)?))
}

fn create_openai_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(OpenAISTT::new(config)?))
}

fn create_assemblyai_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(AssemblyAISTT::new(config)?))
}

fn create_aws_transcribe_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(AwsTranscribeSTT::new(config)?))
}

fn create_ibm_watson_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(IbmWatsonSTT::new(config)?))
}

fn create_groq_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(GroqSTT::new(config)?))
}

fn create_gnani_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(GnaniSTT::new(config)?))
}

// ============================================================================
// TTS Factory Functions
// ============================================================================

fn create_deepgram_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(DeepgramTTS::new(config)?))
}

fn create_elevenlabs_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(ElevenLabsTTS::new(config)?))
}

fn create_google_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(GoogleTTS::new(config)?))
}

fn create_azure_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(AzureTTS::new(config)?))
}

fn create_cartesia_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(CartesiaTTS::new(config)?))
}

fn create_openai_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(OpenAITTS::new(config)?))
}

fn create_aws_polly_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(AwsPollyTTS::new(config)?))
}

fn create_ibm_watson_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(IbmWatsonTTS::new(config)?))
}

fn create_hume_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(HumeTTS::new(config)?))
}

fn create_lmnt_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(LmntTts::new(config)?))
}

fn create_playht_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(PlayHtTts::new(config)?))
}

fn create_gnani_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(GnaniTTS::new(config)?))
}

// ============================================================================
// Realtime Factory Functions
// ============================================================================

fn create_openai_realtime(config: RealtimeConfig) -> Result<Box<dyn BaseRealtime>, RealtimeError> {
    Ok(Box::new(OpenAIRealtime::new(config)?))
}

fn create_hume_evi_realtime(
    config: RealtimeConfig,
) -> Result<Box<dyn BaseRealtime>, RealtimeError> {
    Ok(Box::new(HumeEVI::new(config)?))
}

// ============================================================================
// STT Provider Registrations
// ============================================================================

inventory::submit! {
    PluginConstructor::stt("deepgram", deepgram_stt_metadata, create_deepgram_stt)
}

inventory::submit! {
    PluginConstructor::stt("google", google_stt_metadata, create_google_stt)
}

inventory::submit! {
    PluginConstructor::stt("elevenlabs", elevenlabs_stt_metadata, create_elevenlabs_stt)
}

inventory::submit! {
    PluginConstructor::stt("microsoft-azure", azure_stt_metadata, create_azure_stt)
        .with_aliases(&["azure"])
}

inventory::submit! {
    PluginConstructor::stt("cartesia", cartesia_stt_metadata, create_cartesia_stt)
}

inventory::submit! {
    PluginConstructor::stt("openai", openai_stt_metadata, create_openai_stt)
}

inventory::submit! {
    PluginConstructor::stt("assemblyai", assemblyai_stt_metadata, create_assemblyai_stt)
}

inventory::submit! {
    PluginConstructor::stt("aws-transcribe", aws_transcribe_stt_metadata, create_aws_transcribe_stt)
        .with_aliases(&["aws_transcribe", "amazon-transcribe", "transcribe"])
}

inventory::submit! {
    PluginConstructor::stt("ibm-watson", ibm_watson_stt_metadata, create_ibm_watson_stt)
        .with_aliases(&["ibm_watson", "watson", "ibm"])
}

inventory::submit! {
    PluginConstructor::stt("groq", groq_stt_metadata, create_groq_stt)
}

inventory::submit! {
    PluginConstructor::stt("gnani", gnani_stt_metadata, create_gnani_stt)
        .with_aliases(&["gnani-ai", "gnani.ai", "vachana"])
}

// ============================================================================
// TTS Provider Registrations
// ============================================================================

inventory::submit! {
    PluginConstructor::tts("deepgram", deepgram_tts_metadata, create_deepgram_tts)
}

inventory::submit! {
    PluginConstructor::tts("elevenlabs", elevenlabs_tts_metadata, create_elevenlabs_tts)
}

inventory::submit! {
    PluginConstructor::tts("google", google_tts_metadata, create_google_tts)
}

inventory::submit! {
    PluginConstructor::tts("microsoft-azure", azure_tts_metadata, create_azure_tts)
        .with_aliases(&["azure"])
}

inventory::submit! {
    PluginConstructor::tts("cartesia", cartesia_tts_metadata, create_cartesia_tts)
}

inventory::submit! {
    PluginConstructor::tts("openai", openai_tts_metadata, create_openai_tts)
}

inventory::submit! {
    PluginConstructor::tts("aws-polly", aws_polly_tts_metadata, create_aws_polly_tts)
        .with_aliases(&["aws_polly", "amazon-polly", "polly"])
}

inventory::submit! {
    PluginConstructor::tts("ibm-watson", ibm_watson_tts_metadata, create_ibm_watson_tts)
        .with_aliases(&["ibm_watson", "watson", "ibm"])
}

inventory::submit! {
    PluginConstructor::tts("hume", hume_tts_metadata, create_hume_tts)
        .with_aliases(&["hume-ai", "hume_ai"])
}

inventory::submit! {
    PluginConstructor::tts("lmnt", lmnt_tts_metadata, create_lmnt_tts)
        .with_aliases(&["lmnt-ai", "lmnt_ai"])
}

inventory::submit! {
    PluginConstructor::tts("playht", playht_tts_metadata, create_playht_tts)
        .with_aliases(&["play-ht", "play_ht", "play.ht"])
}

inventory::submit! {
    PluginConstructor::tts("gnani", gnani_tts_metadata, create_gnani_tts)
        .with_aliases(&["gnani-ai", "gnani.ai"])
}

// ============================================================================
// Realtime Provider Registrations
// ============================================================================

inventory::submit! {
    PluginConstructor::realtime("openai", openai_realtime_metadata, create_openai_realtime)
}

inventory::submit! {
    PluginConstructor::realtime("hume", hume_evi_realtime_metadata, create_hume_evi_realtime)
        .with_aliases(&["hume_evi", "hume-evi", "evi"])
}

#[cfg(test)]
mod tests {
    use crate::plugin::registry::global_registry;

    #[test]
    fn test_builtin_stt_providers_registered() {
        let registry = global_registry();

        // All 11 STT providers should be registered
        assert!(registry.has_stt_provider("deepgram"));
        assert!(registry.has_stt_provider("google"));
        assert!(registry.has_stt_provider("elevenlabs"));
        assert!(registry.has_stt_provider("microsoft-azure"));
        assert!(registry.has_stt_provider("azure")); // alias
        assert!(registry.has_stt_provider("cartesia"));
        assert!(registry.has_stt_provider("openai"));
        assert!(registry.has_stt_provider("assemblyai"));
        assert!(registry.has_stt_provider("aws-transcribe"));
        assert!(registry.has_stt_provider("ibm-watson"));
        assert!(registry.has_stt_provider("groq"));
        assert!(registry.has_stt_provider("gnani"));
    }

    #[test]
    fn test_builtin_tts_providers_registered() {
        let registry = global_registry();

        // All 12 TTS providers should be registered
        assert!(registry.has_tts_provider("deepgram"));
        assert!(registry.has_tts_provider("elevenlabs"));
        assert!(registry.has_tts_provider("google"));
        assert!(registry.has_tts_provider("microsoft-azure"));
        assert!(registry.has_tts_provider("cartesia"));
        assert!(registry.has_tts_provider("openai"));
        assert!(registry.has_tts_provider("aws-polly"));
        assert!(registry.has_tts_provider("ibm-watson"));
        assert!(registry.has_tts_provider("hume"));
        assert!(registry.has_tts_provider("lmnt"));
        assert!(registry.has_tts_provider("playht"));
        assert!(registry.has_tts_provider("gnani"));
    }

    #[test]
    fn test_builtin_realtime_providers_registered() {
        let registry = global_registry();

        // Both realtime providers should be registered
        assert!(registry.has_realtime_provider("openai"));
        assert!(registry.has_realtime_provider("hume"));
        assert!(registry.has_realtime_provider("evi")); // alias
    }

    #[test]
    fn test_provider_aliases() {
        let registry = global_registry();

        // Test STT aliases
        assert!(registry.has_stt_provider("azure")); // alias for microsoft-azure
        assert!(registry.has_stt_provider("watson")); // alias for ibm-watson
        assert!(registry.has_stt_provider("transcribe")); // alias for aws-transcribe
        assert!(registry.has_stt_provider("vachana")); // alias for gnani
        assert!(registry.has_stt_provider("gnani-ai")); // alias for gnani

        // Test TTS aliases
        assert!(registry.has_tts_provider("polly")); // alias for aws-polly
        assert!(registry.has_tts_provider("play.ht")); // alias for playht
        assert!(registry.has_tts_provider("gnani-ai")); // alias for gnani

        // Test Realtime aliases
        assert!(registry.has_realtime_provider("evi")); // alias for hume
    }
}

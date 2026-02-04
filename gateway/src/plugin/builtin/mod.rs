//! Built-in Provider Registrations
//!
//! This module registers all built-in STT, TTS, and Realtime providers
//! with the plugin registry using the `inventory` crate.
//!
//! # Providers
//!
//! ## STT Providers (32)
//! - Alibaba Cloud, AmiVoice, Baidu, Deepgram, Google, ElevenLabs, Azure, Cartesia, OpenAI, AssemblyAI,
//!   AWS Transcribe, IBM Watson, Groq, Gnani, Sarvam, Speechmatics, Gladia, Rev AI,
//!   Phonexia, Reverie, Yandex, Tinkoff, SberDevices, Tencent, Huawei Cloud, NAVER CLOVA,
//!   Bhashini, iFlytek, FPT.AI, Viettel AI, Prosa.ai, NECTEC
//!
//! ## TTS Providers (37)
//! - Deepgram, ElevenLabs, Google, Azure, Cartesia, OpenAI, AWS Polly,
//!   IBM Watson, Hume, LMNT, PlayHT, Gnani, Murf, WellSaid, Resemble, Speechify,
//!   Unreal Speech, Speechmatics, Acapela, CereProc, Reverie, Yandex, Smallest.ai,
//!   Tinkoff, SberDevices, Bhashini, iFlytek, Alibaba Cloud, Baidu, Tencent,
//!   Huawei Cloud, NAVER CLOVA, Zalo AI, FPT.AI, Viettel AI, Prosa.ai, NECTEC
//!
//! ## Realtime Providers (2)
//! - OpenAI, Hume EVI

use crate::core::realtime::{BaseRealtime, HumeEVI, OpenAIRealtime, RealtimeConfig, RealtimeError};
use crate::core::stt::{
    AmiVoiceSTT, AssemblyAISTT, AwsTranscribeSTT, AzureSTT, BaiduStt, BaseSTT, BhashiniStt,
    CartesiaSTT, DashScopeStt, DeepgramSTT, ElevenLabsSTT, FptStt, GladiaSTT, GnaniSTT, GoogleSTT,
    GroqSTT, HuaweiCloudStt, IFlytekStt, IbmWatsonSTT, NaverClovaStt, NectecStt, OpenAISTT,
    PhonexiaSTT, ProsaStt, RevAISTT, ReverieSTT, STTConfig, STTError, SarvamSTT, SberDevicesSTT,
    SpeechmaticsSTT, TencentStt, TinkoffStt, ViettelStt, YandexSTT,
};
use crate::core::tts::{
    AcapelaTts, AwsPollyTTS, AzureTTS, BaiduTts, BaseTTS, BhashiniTts, CartesiaTTS, CereprocTts,
    DashScopeTts, DeepgramTTS, ElevenLabsTTS, FptTts, GnaniTTS, GoogleTTS, HuaweiCloudTts, HumeTTS,
    IFlytekTts, IbmWatsonTTS, LmntTts, MurfTts, NaverClovaTts, NectecTts, OpenAITTS, PlayHtTts,
    ProsaTts, ResembleTts, ReverieTts, SberDevicesTts, SmallestTts, SpeechifyTts, SpeechmaticsTts,
    TTSConfig, TencentTts, TinkoffTts, UnrealSpeechTts, ViettelTts, WellSaidTts, YandexTts,
    ZaloTts,
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

fn sarvam_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("sarvam", "Sarvam.ai Saarika STT")
        .with_description("Indian language streaming STT with Saarika model")
        .with_alias("sarvam-ai")
        .with_features(["streaming", "vad-signals", "indic-languages", "code-mixing"])
        .with_languages([
            "hi-IN", "bn-IN", "ta-IN", "te-IN", "gu-IN", "kn-IN", "ml-IN", "mr-IN", "od-IN",
            "pa-IN", "en-IN",
        ])
        .with_models(["saarika:v2.5", "saaras:v2.5"])
}

fn speechmatics_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("speechmatics", "Speechmatics Real-time STT")
        .with_description("Real-time streaming STT with 55+ languages and enhanced accuracy")
        .with_alias("speech-matics")
        .with_features([
            "streaming",
            "word-timestamps",
            "speaker-diarization",
            "auto-language-detect",
            "custom-vocabulary",
        ])
        .with_languages([
            "en", "es", "fr", "de", "it", "pt", "nl", "ja", "ko", "zh", "ru", "ar", "hi", "th",
            "vi", "pl", "cs", "tr", "sv", "da", "no", "fi",
        ])
        .with_models(["standard", "enhanced"])
}

fn gladia_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("gladia", "Gladia Live STT v2")
        .with_description(
            "Real-time streaming STT with 110+ languages, code-switching, and <300ms latency",
        )
        .with_alias("gladia.io")
        .with_features([
            "streaming",
            "word-timestamps",
            "speaker-diarization",
            "code-switching",
            "translation",
            "low-latency",
        ])
        .with_languages([
            "en", "es", "fr", "de", "it", "pt", "nl", "ja", "ko", "zh", "ru", "ar", "hi", "th",
            "vi", "pl", "cs", "tr", "sv", "da", "no", "fi", "el", "he", "id", "ms", "ro", "uk",
        ])
        .with_models(["solaria-1"])
}

fn revai_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("revai", "Rev AI Streaming STT")
        .with_description(
            "Real-time streaming STT with 9+ languages, speaker detection, and custom vocabulary",
        )
        .with_alias("rev.ai")
        .with_features([
            "streaming",
            "word-timestamps",
            "speaker-detection",
            "custom-vocabulary",
            "profanity-filter",
            "disfluency-removal",
        ])
        .with_languages(["en", "es", "fr", "de", "pt", "cmn", "ja", "ru", "ar", "hi"])
        .with_models(["machine", "machine_v2", "human"])
}

fn phonexia_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("phonexia", "Phonexia On-Premises STT")
        .with_description(
            "On-premises speech-to-text with 57-64 languages, voice biometrics, and speaker identification",
        )
        .with_alias("phonexia-stt")
        .with_features([
            "streaming",
            "on-premises",
            "word-timestamps",
            "speaker-identification",
            "language-identification",
            "voice-biometrics",
            "custom-vocabulary",
        ])
        .with_languages([
            "en-US", "en-GB", "cs", "sk", "pl", "hu", "ro", "bg", "sr", "hr", "sl", "uk", "ru",
            "de", "fr", "es", "it", "pt", "nl", "da", "sv", "no", "fi", "ar", "tr", "fa", "he",
            "zh", "ja", "ko", "th", "vi", "id",
        ])
        .with_models(["one_best", "n_best", "confusion_network"])
}

fn reverie_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("reverie", "Reverie Language Technologies STT")
        .with_description(
            "Real-time streaming STT optimized for 22+ Indian languages with dialect-agnostic recognition",
        )
        .with_alias("reverieinc")
        .with_features([
            "streaming",
            "indic-languages",
            "code-mixing",
            "dialect-agnostic",
            "punctuation",
            "silence-detection",
            "continuous-mode",
        ])
        .with_languages([
            "hi", "en", "ta", "te", "kn", "ml", "bn", "gu", "mr", "pa", "or", "as", "ur", "ne",
            "sd", "ks", "kok", "mni", "brx", "doi", "mai", "sat", "sa",
        ])
}

fn yandex_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("yandex", "Yandex SpeechKit STT")
        .with_description(
            "Speech recognition for Russia/CIS region with auto-detection and multiple languages",
        )
        .with_alias("speechkit")
        .with_features([
            "streaming",
            "auto-language-detect",
            "profanity-filter",
            "custom-vocabulary",
            "russia-cis-languages",
        ])
        .with_languages([
            "ru-RU", "en-US", "de-DE", "fr-FR", "fi-FI", "sv-SE", "nl-NL", "pl-PL", "pt-PT",
            "tr-TR", "uk-UA", "kk-KZ", "uz-UZ", "he-IL",
        ])
        .with_models(["general", "general:rc", "deferred"])
}

fn tinkoff_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("tinkoff", "Tinkoff VoiceKit STT")
        .with_description(
            "Russian-specialized gRPC streaming STT with high accuracy and configurable VAD",
        )
        .with_alias("voicekit")
        .with_features([
            "streaming",
            "grpc",
            "vad",
            "interim-results",
            "russian-specialized",
            "multi-channel",
        ])
        .with_languages(["ru-RU"])
}

fn sberdevices_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("sberdevices", "SberDevices SaluteSpeech STT")
        .with_description(
            "Russian/CIS speech recognition via REST API with OAuth 2.0 authentication",
        )
        .with_alias("salutespeech")
        .with_alias("smartspeech")
        .with_features([
            "rest-api",
            "oauth2",
            "auto-token-refresh",
            "punctuation",
            "cis-languages",
        ])
        .with_languages(["ru-RU", "en-US", "kk-KZ", "ky-KG", "uz-UZ"])
}

fn bhashini_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("bhashini", "Bhashini ULCA STT")
        .with_description(
            "Government of India (MeitY) speech recognition for 22+ Indian languages via ULCA APIs",
        )
        .with_alias("ulca")
        .with_alias("ai4bharat")
        .with_alias("meity")
        .with_features([
            "rest-api",
            "pipeline-auth",
            "indic-languages",
            "multi-provider",
            "dialect-support",
        ])
        .with_languages([
            "hi", "ta", "te", "kn", "ml", "bn", "mr", "gu", "pa", "or", "ur", "as", "sa", "en",
            "ne", "mni", "brx", "doi", "ks", "kok", "mai", "sat", "sd", "gom",
        ])
}

fn iflytek_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("iflytek", "iFlytek STT (科大讯飞)")
        .with_description(
            "Chinese AI leader for 18+ languages with real-time streaming, dynamic word correction, and dialect support",
        )
        .with_alias("xfyun")
        .with_alias("xunfei")
        .with_alias("讯飞")
        .with_features([
            "streaming",
            "dynamic-correction",
            "dialect-support",
            "vad",
            "interim-results",
            "punctuation",
        ])
        .with_languages([
            "zh_cn", "zh_tw", "en_us", "ja_jp", "ko_kr", "ru_ru", "fr_fr", "es_es", "de_de",
            "th_th", "vi_vn", "id_id", "ms_my", "pt_pt", "ar_sa", "hi_in", "ur_pk", "nl_nl",
        ])
}

fn alibaba_cloud_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("alibaba-cloud", "Alibaba Cloud DashScope STT (阿里云)")
        .with_description(
            "Alibaba Cloud DashScope Model Studio STT with 25+ languages, Chinese dialects, and multiple models",
        )
        .with_alias("dashscope")
        .with_alias("alibabacloud")
        .with_alias("aliyun")
        .with_alias("阿里云")
        .with_alias("qwen-asr")
        .with_features([
            "streaming",
            "word-timestamps",
            "emotion-recognition",
            "server-vad",
            "chinese-dialects",
            "context-biasing",
            "turn-detection",
        ])
        .with_languages([
            "zh", "en", "ja", "ko", "ru", "fr", "de", "es", "pt", "it", "ar", "hi", "th", "vi",
            "id", "ms", "tr", "uk", "pl", "nl", "sv", "da", "fi", "no", "cs", "is", "yue", "wuu",
        ])
        .with_models([
            "qwen3-asr-flash-realtime",
            "paraformer-realtime-v2",
            "paraformer-realtime-8k-v2",
            "fun-asr-realtime",
        ])
}

fn baidu_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("baidu", "Baidu AI Cloud Speech (百度语音)")
        .with_description(
            "Baidu AI Cloud Speech-to-Text with Chinese dialects, real-time streaming, and REST API",
        )
        .with_alias("baidu-ai")
        .with_alias("baidu_ai")
        .with_alias("baiduai")
        .with_alias("百度")
        .with_alias("百度语音")
        .with_alias("baidu-speech")
        .with_features([
            "streaming",
            "rest-api",
            "chinese-dialects",
            "custom-vocabulary",
            "interim-results",
            "punctuation",
        ])
        .with_languages(["zh", "en", "yue", "zh-sichuan"])
        .with_models(["1537", "1536", "1737", "1637", "1837", "1936"])
}

fn tencent_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("tencent", "Tencent Cloud ASR (腾讯云语音)")
        .with_description(
            "Tencent Cloud real-time streaming ASR with 97% accuracy, Chinese dialects support",
        )
        .with_alias("tencent-cloud")
        .with_alias("tencent_cloud")
        .with_alias("tencentcloud")
        .with_alias("腾讯云")
        .with_alias("腾讯")
        .with_features([
            "streaming",
            "custom-vocabulary",
            "word-timestamps",
            "vad",
            "interim-results",
        ])
        .with_languages(["zh", "en", "yue", "ja", "ko", "th", "vi", "id"])
        .with_models([
            "16k_zh", "16k_en", "16k_ca", "16k_ja", "16k_ko", "16k_th", "16k_vi", "16k_id", "8k_zh",
        ])
}

fn huawei_cloud_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("huawei-cloud", "Huawei Cloud SIS (华为云语音)")
        .with_description(
            "Huawei Cloud Speech Interaction Service with Mandarin, Chinese dialects, and minority languages",
        )
        .with_alias("huawei_cloud")
        .with_alias("huaweicloud")
        .with_alias("huawei")
        .with_alias("华为云")
        .with_alias("华为")
        .with_alias("sis")
        .with_alias("huawei-sis")
        .with_features([
            "streaming",
            "short-audio",
            "continuous-mode",
            "custom-vocabulary",
            "word-timestamps",
            "interim-results",
            "minority-languages",
        ])
        .with_languages([
            "zh-CN", "zh-HK", "zh-SC", "zh-MN", "mn", "bo", "ug",
        ])
        .with_models([
            "chinese_16k_general",
            "chinese_8k_general",
            "chinese_16k_common",
            "cantonese_16k_general",
            "sichuan_16k_general",
            "minnan_16k_general",
            "mongolian_16k_general",
            "tibetan_16k_general",
            "uyghur_16k_general",
        ])
}

fn naver_clova_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("naver-clova", "NAVER CLOVA CSR (네이버 클로바)")
        .with_description(
            "NAVER Cloud Platform CLOVA Speech Recognition with highest Korean accuracy and REST API",
        )
        .with_alias("naver_clova")
        .with_alias("naverclova")
        .with_alias("naver")
        .with_alias("clova")
        .with_alias("csr")
        .with_alias("네이버")
        .with_alias("클로바")
        .with_features([
            "rest-api",
            "batch-processing",
            "korean-optimized",
            "multi-format",
        ])
        .with_languages(["ko", "en", "ja", "zh"])
}

fn amivoice_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("amivoice", "AmiVoice Cloud Platform (アミボイス)")
        .with_description(
            "Advanced Media AmiVoice with 19+ engines including E2E and domain-specific models for Japanese",
        )
        .with_alias("amivoice-stt")
        .with_alias("ami")
        .with_alias("advanced-media")
        .with_alias("アミボイス")
        .with_alias("acp")
        .with_features([
            "streaming",
            "websocket",
            "word-timestamps",
            "speaker-diarization",
            "sentiment-analysis",
            "medical-domain",
            "finance-domain",
            "custom-vocabulary",
            "japanese-optimized",
        ])
        .with_languages(["ja", "en", "zh", "ko"])
        .with_models([
            "-a-general",
            "-a-medical",
            "-a-finance",
            "-a-insurance",
            "-a2-ja-general",
            "-a2-multi-general",
        ])
}

fn fpt_ai_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("fpt-ai", "FPT.AI STT (FPT Corporation)")
        .with_description(
            "FPT Corporation's FPT.AI Speech-to-Text service optimized for Vietnamese language",
        )
        .with_alias("fpt_ai-stt")
        .with_alias("fpt-stt")
        .with_alias("fpt")
        .with_features(["rest-api", "vietnamese-optimized", "http-file-upload"])
        .with_languages(["vi"])
}

fn viettel_ai_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("viettel-ai", "Viettel AI STT (Viettel Group)")
        .with_description(
            "Viettel Group's AI Speech-to-Text service with 96% accuracy for Vietnamese language",
        )
        .with_alias("viettel_ai-stt")
        .with_alias("viettel-stt")
        .with_alias("viettel")
        .with_alias("vtai")
        .with_features([
            "rest-api",
            "vietnamese-optimized",
            "96-percent-accuracy",
            "regional-accent-detection",
            "multipart-file-upload",
        ])
        .with_languages(["vi"])
}

fn prosa_ai_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("prosa-ai", "Prosa.ai STT (Indonesian NLP)")
        .with_description(
            "Indonesian AI speech-to-text with streaming WebSocket API, optimized for Bahasa Indonesia",
        )
        .with_alias("prosa_ai-stt")
        .with_alias("prosa-stt")
        .with_alias("prosa")
        .with_alias("prosaid")
        .with_features([
            "websocket-streaming",
            "rest-api",
            "indonesian-optimized",
            "partial-results",
            "word-timestamps",
            "speaker-diarization",
            "model-stt-general",
            "model-stt-general-online",
            "opus-format",
            "mp3-format",
            "wav-format",
        ])
        .with_languages(["id", "en"])
}

fn nectec_stt_metadata() -> ProviderMetadata {
    ProviderMetadata::stt("nectec", "NECTEC AI for Thai STT (Partii)")
        .with_description(
            "Thai government speech-to-text service via AI for Thai platform, supporting Partii4 and Partii5 engines",
        )
        .with_alias("aiforthai")
        .with_alias("ai4thai")
        .with_alias("partii")
        .with_alias("partii5")
        .with_alias("partii4")
        .with_features([
            "rest-api",
            "thai-optimized",
            "free-service",
            "government-backed",
            "partii4-legacy",
            "partii5-recommended",
            "wav-format",
            "16khz-mono",
            "30-second-max",
        ])
        .with_languages(["th"])
        .with_models(["partii5", "partii4"])
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

fn murf_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("murf", "Murf.ai TTS")
        .with_description("Ultra-low latency TTS (~130ms) with 150+ voices and 35+ languages")
        .with_alias("murf-ai")
        .with_features([
            "streaming",
            "low-latency",
            "multi-language",
            "speaking-styles",
        ])
        .with_models(["Falcon", "Gen2"])
        .with_languages([
            "en-US", "en-GB", "en-AU", "en-IN", "es-ES", "es-MX", "fr-FR", "de-DE", "it-IT",
            "pt-BR", "hi-IN", "bn-IN", "ta-IN", "nl-NL", "ko-KR", "zh-CN", "pl-PL",
        ])
}

fn wellsaid_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("wellsaid", "WellSaid Labs TTS")
        .with_description("Premium AI voices with 200+ avatars and 20+ languages")
        .with_alias("wellsaid-labs")
        .with_features([
            "streaming",
            "multi-language",
            "ai-director",
            "studio-quality",
        ])
        .with_models(["legacy", "caruso"])
        .with_languages([
            "en-US", "en-GB", "es-ES", "fr-FR", "de-DE", "it-IT", "pt-BR", "nl-NL", "ja-JP",
            "ko-KR", "zh-CN", "ar-SA", "hi-IN", "ru-RU", "pl-PL", "sv-SE", "da-DK", "fi-FI",
            "no-NO", "tr-TR",
        ])
}

fn resemble_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("resemble", "Resemble AI TTS")
        .with_description("Voice cloning TTS with 149+ languages and deepfake detection")
        .with_alias("resemble-ai")
        .with_features([
            "streaming",
            "voice-cloning",
            "speech-to-speech",
            "multi-language",
            "paralinguistic-tags",
        ])
        .with_models(["chatterbox", "chatterbox-turbo", "chatterbox-multilingual"])
        .with_languages([
            "en-US", "ar-SA", "da-DK", "de-DE", "el-GR", "es-ES", "fi-FI", "fr-FR", "he-IL",
            "hi-IN", "it-IT", "ja-JP", "ko-KR", "ms-MY", "nl-NL", "no-NO", "pl-PL", "pt-BR",
            "ru-RU", "sv-SE", "sw-KE", "tr-TR", "zh-CN",
        ])
}

fn speechify_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("speechify", "Speechify TTS")
        .with_description("Consumer-focused TTS with voice cloning and SSML support (~300ms)")
        .with_features([
            "streaming",
            "voice-cloning",
            "ssml",
            "multi-language",
            "loudness-normalization",
            "text-normalization",
        ])
        .with_models([
            "simba-english",
            "simba-turbo",
            "simba-multilingual",
            "simba-base",
        ])
        .with_languages([
            "en-US", "en-GB", "es-ES", "es-MX", "fr-FR", "de-DE", "it-IT", "pt-BR", "pt-PT",
            "nl-NL", "pl-PL", "ru-RU", "tr-TR", "ja-JP", "ko-KR", "zh-CN", "zh-TW", "ar-SA",
            "hi-IN", "id-ID", "ms-MY", "th-TH", "vi-VN", "sv-SE", "da-DK", "fi-FI", "no-NO",
        ])
}

fn unrealspeech_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("unrealspeech", "Unreal Speech TTS")
        .with_description("Cost-effective TTS with ultra-low latency (~300ms)")
        .with_alias("unreal-speech")
        .with_features([
            "streaming",
            "low-latency",
            "speed-control",
            "pitch-control",
            "bitrate-options",
        ])
        .with_languages(["en-US", "en-GB"])
}

fn speechmatics_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("speechmatics", "Speechmatics TTS")
        .with_description("High-quality TTS with expressive prosody")
        .with_features(["streaming", "low-latency", "natural-prosody"])
        .with_languages(["en-US", "en-GB"])
}

fn acapela_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("acapela", "Acapela Cloud TTS")
        .with_description("AI neural TTS with 250+ voices across 30+ languages")
        .with_alias("acapela-cloud")
        .with_features([
            "streaming",
            "word-timestamps",
            "viseme-data",
            "custom-dictionaries",
            "speed-control",
            "volume-control",
        ])
        .with_languages([
            "en-US", "en-GB", "en-AU", "en-IN", "fr-FR", "fr-CA", "de-DE", "it-IT", "es-ES",
            "pt-PT", "pt-BR", "nl-NL", "nl-BE", "pl-PL", "ru-RU", "tr-TR", "ja-JP", "ko-KR",
            "zh-CN", "ar-SA", "hi-IN", "da-DK", "sv-SE", "no-NO", "fi-FI", "cs-CZ", "el-GR",
            "ca-ES", "fo-FO",
        ])
}

fn cereproc_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("cereproc", "CereProc CereVoice Cloud TTS")
        .with_description("Characterful TTS with emotional voices and Celtic language support")
        .with_alias("cerevoice")
        .with_features([
            "emotional-voices",
            "ssml",
            "custom-lexicons",
            "celtic-languages",
            "vocal-gestures",
        ])
        .with_languages([
            "en-GB", "en-SC", "en-US", "en-IE", "cy-GB", "gd-GB", "ga-IE", "fr-FR", "de-DE",
            "nl-NL", "es-ES", "it-IT", "sv-SE",
        ])
}

fn reverie_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("reverie", "Reverie Language Technologies TTS")
        .with_description(
            "Indian language TTS optimized for 22+ languages with 36+ male/female voices",
        )
        .with_alias("reverieinc")
        .with_features([
            "indic-languages",
            "ssml",
            "speed-control",
            "pitch-control",
            "multiple-voices",
            "wav-mp3-output",
        ])
        .with_languages([
            "hi", "en", "ta", "te", "kn", "ml", "bn", "gu", "mr", "pa", "or", "as", "ur", "ne",
            "sd", "ks", "kok", "mni", "brx", "doi", "mai", "sat", "sa",
        ])
}

fn yandex_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("yandex", "Yandex SpeechKit TTS")
        .with_description(
            "Neural TTS with 29+ voices, emotional variations, and Russia/CIS language support",
        )
        .with_alias("speechkit")
        .with_features([
            "emotional-voices",
            "ssml",
            "speed-control",
            "russia-cis-languages",
            "premium-voices",
        ])
        .with_languages(["ru-RU", "en-US", "de-DE", "he-IL", "kk-KK", "uz-UZ"])
}

fn smallest_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("smallest", "Smallest.ai Waves TTS")
        .with_description("Ultra-low latency TTS (~100ms) with voice cloning and 16+ languages")
        .with_alias("smallest-ai")
        .with_features([
            "streaming",
            "ultra-low-latency",
            "voice-cloning",
            "multi-language",
            "websocket",
            "speed-control",
        ])
        .with_models(["lightning", "lightning-large", "lightning-v2"])
        .with_languages([
            "en", "hi", "mr", "kn", "ta", "bn", "gu", "de", "fr", "es", "it", "pl", "nl", "ru",
            "ar", "he",
        ])
}

fn tinkoff_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("tinkoff", "Tinkoff VoiceKit TTS")
        .with_description("Russian TTS via gRPC with JWT authentication and SSML support")
        .with_alias("tinkoff-voicekit")
        .with_features([
            "streaming",
            "ssml",
            "speaking-rate",
            "pitch-control",
            "volume-gain",
            "grpc",
        ])
        .with_models(["alyona", "dorofeev"])
        .with_languages(["ru-RU"])
}

fn sberdevices_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("sberdevices", "SberDevices SaluteSpeech TTS")
        .with_description(
            "Russian/English TTS via REST API with OAuth 2.0 authentication and SSML support",
        )
        .with_alias("salutespeech-tts")
        .with_alias("smartspeech-tts")
        .with_features(["rest-api", "oauth2", "auto-token-refresh", "ssml"])
        .with_models(["Nec", "Bys", "May", "Tur", "Ost", "Pon", "Kin"])
        .with_languages(["ru-RU", "en-US"])
}

fn bhashini_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("bhashini", "Bhashini ULCA TTS")
        .with_description(
            "Government of India (MeitY) text-to-speech for 22+ Indian languages via ULCA APIs",
        )
        .with_alias("ulca-tts")
        .with_alias("ai4bharat-tts")
        .with_alias("meity-tts")
        .with_features([
            "rest-api",
            "pipeline-auth",
            "indic-languages",
            "multi-provider",
            "gender-selection",
        ])
        .with_languages([
            "hi", "ta", "te", "kn", "ml", "bn", "mr", "gu", "pa", "or", "ur", "as", "sa", "en",
            "ne", "mni", "brx", "doi", "ks", "kok", "mai", "sat", "sd", "gom",
        ])
}

fn iflytek_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("iflytek", "iFlytek TTS (科大讯飞)")
        .with_description(
            "Chinese AI leader for 15+ languages with WebSocket streaming, multiple voices, and prosody control",
        )
        .with_alias("xfyun-tts")
        .with_alias("xunfei-tts")
        .with_alias("讯飞-tts")
        .with_features([
            "streaming",
            "websocket",
            "speed-control",
            "volume-control",
            "pitch-control",
            "multi-encoding",
        ])
        .with_languages([
            "zh", "en", "ja", "id", "ru", "fr", "de", "ar", "vi", "th", "ko", "pt", "ms", "hi", "ur",
        ])
}

fn alibaba_cloud_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("alibaba-cloud", "Alibaba Cloud DashScope TTS (阿里云)")
        .with_description(
            "Alibaba Cloud DashScope Model Studio TTS with CosyVoice and Qwen3-TTS models for Chinese and 25+ languages",
        )
        .with_alias("dashscope-tts")
        .with_alias("alibabacloud-tts")
        .with_alias("aliyun-tts")
        .with_alias("阿里云-tts")
        .with_alias("cosyvoice")
        .with_alias("qwen-tts")
        .with_features([
            "streaming",
            "websocket",
            "speed-control",
            "volume-control",
            "pitch-control",
            "chinese-dialects",
            "voice-cloning",
        ])
        .with_models([
            "cosyvoice-v3-flash",
            "cosyvoice-v3-plus",
            "cosyvoice-v2",
            "qwen3-tts-flash-realtime",
        ])
        .with_languages([
            "zh", "en", "ja", "ko", "ru", "fr", "de", "es", "pt", "it", "ar", "hi", "th", "vi",
            "id", "ms", "tr", "uk", "pl", "nl", "sv", "da", "fi", "no", "cs", "yue", "wuu",
        ])
}

fn baidu_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("baidu", "Baidu AI Cloud TTS (百度语音)")
        .with_description(
            "Baidu AI Cloud Speech TTS with 40+ voices across Basic, Premium, Premium+, and Large Model categories",
        )
        .with_alias("baidu-ai-tts")
        .with_alias("baidu-speech")
        .with_alias("百度语音-tts")
        .with_features([
            "rest-api",
            "oauth2",
            "multiple-voices",
            "speed-control",
            "pitch-control",
            "volume-control",
            "chinese-dialects",
            "premium-voices",
            "ai-voices",
        ])
        .with_models([
            "basic",
            "premium",
            "premium-plus",
            "large-model",
        ])
        .with_languages(["zh", "en"])
}

fn tencent_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("tencent", "Tencent Cloud TTS (腾讯云语音)")
        .with_description(
            "Tencent Cloud Text-to-Speech with 70+ voices including standard, premium, emotional, dialect, child, and English voices",
        )
        .with_alias("tencent-tts")
        .with_alias("腾讯云")
        .with_alias("腾讯语音")
        .with_features([
            "rest-api",
            "tc3-auth",
            "multiple-voices",
            "speed-control",
            "volume-control",
            "emotion-control",
            "word-timestamps",
            "chinese-dialects",
            "premium-voices",
            "multilingual",
        ])
        .with_models([
            "standard",
            "premium",
            "emotional",
            "dialect",
            "child",
            "english",
        ])
        .with_languages(["zh", "en", "yue", "sc", "ms", "ar"])
}

fn huawei_cloud_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("huawei-cloud", "Huawei Cloud TTS (华为云语音)")
        .with_description(
            "Huawei Cloud Speech Interaction Service with 10+ standard voices and premium voices for sales, customer service, and news styles",
        )
        .with_alias("huawei-cloud-tts")
        .with_alias("华为云")
        .with_alias("华为语音")
        .with_alias("huawei-sis")
        .with_features([
            "rest-api",
            "websocket-streaming",
            "iam-token-auth",
            "multiple-voices",
            "speed-control",
            "pitch-control",
            "volume-control",
            "premium-voices",
            "child-voices",
            "english-voice",
            "text-splitting",
        ])
        .with_models([
            "standard",
            "premium",
            "child",
            "english",
        ])
        .with_languages(["zh", "en"])
}

fn naver_clova_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("naver-clova", "NAVER CLOVA Voice (네이버 클로바)")
        .with_description(
            "NAVER Cloud Platform CLOVA Voice Premium TTS with 100+ neural voices and NeuVis technology",
        )
        .with_alias("naver_clova-tts")
        .with_alias("naver-tts")
        .with_alias("naverclova-tts")
        .with_alias("clova-tts")
        .with_alias("네이버-tts")
        .with_alias("클로바-tts")
        .with_features([
            "rest-api",
            "neural-voices",
            "multiple-voices",
            "speed-control",
            "pitch-control",
            "volume-control",
            "emotion-control",
            "korean-optimized",
        ])
        .with_models(["premium"])
        .with_languages(["ko", "en", "ja", "zh", "es"])
}

fn zalo_ai_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("zalo-ai", "Zalo AI TTS (VNG Corporation)")
        .with_description(
            "VNG Corporation's Zalo AI Text-to-Speech service optimized for Vietnamese language with regional accents",
        )
        .with_alias("zalo_ai-tts")
        .with_alias("zalo-tts")
        .with_alias("zalo")
        .with_alias("vng-tts")
        .with_features([
            "rest-api",
            "vietnamese-optimized",
            "northern-accent",
            "southern-accent",
            "speed-control",
            "wav-output",
            "voice-1:female-south",
            "voice-2:female-north",
            "voice-3:male-south",
            "voice-4:male-north",
        ])
        .with_languages(["vi"])
}

fn fpt_ai_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("fpt-ai", "FPT.AI TTS (FPT Corporation)")
        .with_description(
            "FPT Corporation's FPT.AI Text-to-Speech service optimized for Vietnamese language",
        )
        .with_alias("fpt_ai-tts")
        .with_alias("fpt-tts")
        .with_features([
            "rest-api",
            "vietnamese-optimized",
            "northern-accent",
            "speed-control",
            "mp3-output",
            "wav-output",
            "voice-banmai:female-north",
            "voice-lannhi:female-north",
            "voice-leminh:male-north",
            "voice-myan:female",
            "voice-thuminh:female",
            "voice-giahuy:male",
            "voice-linhsan:female",
        ])
        .with_languages(["vi"])
}

fn viettel_ai_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("viettel-ai", "Viettel AI TTS (Viettel Group)")
        .with_description(
            "Viettel Group's AI Text-to-Speech service with 12 Vietnamese voices and regional accents",
        )
        .with_alias("viettel_ai-tts")
        .with_alias("viettel-tts")
        .with_alias("vtai-tts")
        .with_features([
            "rest-api",
            "vietnamese-optimized",
            "northern-accent",
            "central-accent",
            "southern-accent",
            "speed-control",
            "wav-output",
            "voice-hcm-diemmy:female-south",
            "voice-hcm-minhquan:male-south",
            "voice-hcm-phuongtrang:female-south",
            "voice-hcm-anhtuan:male-south",
            "voice-hn-phuongly:female-north",
            "voice-hn-maianh:female-north",
            "voice-hn-namkhanh:male-north",
            "voice-hn-thanhtung:male-north",
            "voice-hn-linhsan:female-north",
            "voice-hue-baokhang:male-central",
            "voice-hue-ngoclam:female-central",
            "voice-hue-mytam:female-central",
        ])
        .with_languages(["vi"])
}

fn prosa_ai_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("prosa-ai", "Prosa.ai TTS (Indonesian NLP)")
        .with_description(
            "Indonesian AI text-to-speech with 40+ voices, optimized for Bahasa Indonesia and English",
        )
        .with_alias("prosa_ai-tts")
        .with_alias("prosa-tts")
        .with_features([
            "rest-api",
            "indonesian-optimized",
            "english-support",
            "pitch-control",
            "tempo-control",
            "opus-format",
            "mp3-format",
            "wav-format",
            "async-synthesis",
            "voice-dimas-formal:male-id",
            "voice-dimas-expressive:male-id",
            "voice-ocha-friendly:female-id",
            "voice-dini:female-id-audiobook",
            "voice-kinanti:female-id-news",
            "voice-darah:female-id-kids",
            "voice-abimana:male-id-news",
            "voice-roger:male-en-news",
            "voice-jennifer:female-en-news",
        ])
        .with_languages(["id", "en"])
}

fn nectec_tts_metadata() -> ProviderMetadata {
    ProviderMetadata::tts("nectec", "NECTEC AI for Thai TTS (VAJA9)")
        .with_description(
            "Thai government text-to-speech service via AI for Thai platform, VAJA9 TTS engine",
        )
        .with_alias("aiforthai-tts")
        .with_alias("ai4thai-tts")
        .with_alias("vaja9")
        .with_alias("vaja")
        .with_features([
            "rest-api",
            "thai-optimized",
            "free-service",
            "government-backed",
            "male-voice",
            "female-voice",
            "wav-format",
            "22khz-pcm16",
            "auto-chunking",
            "300-char-max",
        ])
        .with_languages(["th"])
        .with_models(["vaja9"])
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

fn create_sarvam_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(SarvamSTT::new(config)?))
}

fn create_speechmatics_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(SpeechmaticsSTT::new(config)?))
}

fn create_gladia_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(GladiaSTT::new(config)?))
}

fn create_revai_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(RevAISTT::new(config)?))
}

fn create_phonexia_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(PhonexiaSTT::new(config)?))
}

fn create_reverie_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(ReverieSTT::new(config)?))
}

fn create_yandex_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(YandexSTT::new(config)?))
}

fn create_tinkoff_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(TinkoffStt::new(config)?))
}

fn create_sberdevices_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(SberDevicesSTT::new(config)?))
}

fn create_bhashini_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(BhashiniStt::new(config)?))
}

fn create_iflytek_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(IFlytekStt::new(config)?))
}

fn create_alibaba_cloud_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(DashScopeStt::new(config)?))
}

fn create_baidu_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(BaiduStt::new(config)?))
}

fn create_tencent_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(TencentStt::new(config)?))
}

fn create_huawei_cloud_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(HuaweiCloudStt::new(config)?))
}

fn create_naver_clova_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(NaverClovaStt::new(config)?))
}

fn create_amivoice_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(AmiVoiceSTT::new(config)?))
}

fn create_fpt_ai_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(FptStt::new(config)?))
}

fn create_viettel_ai_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(ViettelStt::new(config)?))
}

fn create_prosa_ai_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(ProsaStt::new(config)?))
}

fn create_nectec_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    Ok(Box::new(NectecStt::new(config)?))
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

fn create_murf_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(MurfTts::new(config)?))
}

fn create_wellsaid_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(WellSaidTts::new(config)?))
}

fn create_resemble_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(ResembleTts::new(config)?))
}

fn create_speechify_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(SpeechifyTts::new(config)?))
}

fn create_unrealspeech_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(UnrealSpeechTts::new(config)?))
}

fn create_speechmatics_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(SpeechmaticsTts::new(config)?))
}

fn create_acapela_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(AcapelaTts::new(config)?))
}

fn create_cereproc_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(CereprocTts::new(config)?))
}

fn create_reverie_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(ReverieTts::new(config)?))
}

fn create_yandex_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(YandexTts::new(config)?))
}

fn create_smallest_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(SmallestTts::new(config)?))
}

fn create_tinkoff_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(TinkoffTts::new(config)?))
}

fn create_sberdevices_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(SberDevicesTts::new(config)?))
}

fn create_bhashini_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(BhashiniTts::new(config)?))
}

fn create_iflytek_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(IFlytekTts::new(config)?))
}

fn create_alibaba_cloud_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(DashScopeTts::new(config)?))
}

fn create_baidu_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(BaiduTts::new(config)?))
}

fn create_tencent_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(TencentTts::new(config)?))
}

fn create_huawei_cloud_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(HuaweiCloudTts::new(config)?))
}

fn create_naver_clova_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(NaverClovaTts::new(config)?))
}

fn create_zalo_ai_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(ZaloTts::new(config)?))
}

fn create_fpt_ai_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(FptTts::new(config)?))
}

fn create_viettel_ai_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(ViettelTts::new(config)?))
}

fn create_prosa_ai_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(ProsaTts::new(config)?))
}

fn create_nectec_tts(config: TTSConfig) -> crate::core::tts::TTSResult<Box<dyn BaseTTS>> {
    Ok(Box::new(NectecTts::new(config)?))
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

inventory::submit! {
    PluginConstructor::stt("sarvam", sarvam_stt_metadata, create_sarvam_stt)
        .with_aliases(&["sarvam-ai", "sarvam.ai", "saarika"])
}

inventory::submit! {
    PluginConstructor::stt("speechmatics", speechmatics_stt_metadata, create_speechmatics_stt)
        .with_aliases(&["speech-matics", "speech_matics"])
}

inventory::submit! {
    PluginConstructor::stt("gladia", gladia_stt_metadata, create_gladia_stt)
        .with_aliases(&["gladia.io", "gladia-io", "gladia_io"])
}

inventory::submit! {
    PluginConstructor::stt("revai", revai_stt_metadata, create_revai_stt)
        .with_aliases(&["rev-ai", "rev_ai", "rev.ai"])
}

inventory::submit! {
    PluginConstructor::stt("phonexia", phonexia_stt_metadata, create_phonexia_stt)
        .with_aliases(&["phonexia-stt", "phonexia_stt"])
}

inventory::submit! {
    PluginConstructor::stt("reverie", reverie_stt_metadata, create_reverie_stt)
        .with_aliases(&["reverie-ai", "reverie_ai", "reverie-stt", "reverieinc"])
}

inventory::submit! {
    PluginConstructor::stt("yandex", yandex_stt_metadata, create_yandex_stt)
        .with_aliases(&["yandex-stt", "yandex_stt", "speechkit", "yandex-speechkit"])
}

inventory::submit! {
    PluginConstructor::stt("tinkoff", tinkoff_stt_metadata, create_tinkoff_stt)
        .with_aliases(&["tinkoff-stt", "tinkoff_stt", "voicekit", "tinkoff-voicekit"])
}

inventory::submit! {
    PluginConstructor::stt("sberdevices", sberdevices_stt_metadata, create_sberdevices_stt)
        .with_aliases(&["sber", "sber-devices", "sber_devices", "salutespeech", "salute-speech", "smartspeech"])
}

inventory::submit! {
    PluginConstructor::stt("bhashini", bhashini_stt_metadata, create_bhashini_stt)
        .with_aliases(&["bhashini-stt", "bhashini_stt", "ulca", "ai4bharat", "ai4bharat-stt", "meity", "meity-stt"])
}

inventory::submit! {
    PluginConstructor::stt("iflytek", iflytek_stt_metadata, create_iflytek_stt)
        .with_aliases(&["iflytek-stt", "iflytek_stt", "xfyun", "xunfei", "讯飞", "科大讯飞"])
}

inventory::submit! {
    PluginConstructor::stt("alibaba-cloud", alibaba_cloud_stt_metadata, create_alibaba_cloud_stt)
        .with_aliases(&["alibaba_cloud", "alibabacloud", "alibaba", "dashscope", "aliyun", "阿里云", "qwen-asr"])
}

inventory::submit! {
    PluginConstructor::stt("baidu", baidu_stt_metadata, create_baidu_stt)
        .with_aliases(&["baidu-ai", "baidu_ai", "baiduai", "百度", "百度语音", "baidu-speech", "baidu_speech"])
}

inventory::submit! {
    PluginConstructor::stt("tencent", tencent_stt_metadata, create_tencent_stt)
        .with_aliases(&["tencent-cloud", "tencent_cloud", "tencentcloud", "腾讯云", "腾讯"])
}

inventory::submit! {
    PluginConstructor::stt("huawei-cloud", huawei_cloud_stt_metadata, create_huawei_cloud_stt)
        .with_aliases(&["huawei_cloud", "huaweicloud", "huawei", "华为云", "华为", "sis", "huawei-sis"])
}

inventory::submit! {
    PluginConstructor::stt("naver-clova", naver_clova_stt_metadata, create_naver_clova_stt)
        .with_aliases(&["naver_clova", "naverclova", "naver", "clova", "csr", "네이버", "클로바"])
}

inventory::submit! {
    PluginConstructor::stt("amivoice", amivoice_stt_metadata, create_amivoice_stt)
        .with_aliases(&["amivoice-stt", "ami", "advanced-media", "アミボイス", "acp"])
}

inventory::submit! {
    PluginConstructor::stt("fpt-ai", fpt_ai_stt_metadata, create_fpt_ai_stt)
        .with_aliases(&["fpt_ai-stt", "fpt-stt", "fpt", "fptai", "fpt_ai"])
}

inventory::submit! {
    PluginConstructor::stt("viettel-ai", viettel_ai_stt_metadata, create_viettel_ai_stt)
        .with_aliases(&["viettel_ai-stt", "viettel-stt", "viettel", "vtai", "viettelai"])
}

inventory::submit! {
    PluginConstructor::stt("prosa-ai", prosa_ai_stt_metadata, create_prosa_ai_stt)
        .with_aliases(&["prosa_ai-stt", "prosa-stt", "prosa", "prosaid", "prosaai"])
}

inventory::submit! {
    PluginConstructor::stt("nectec", nectec_stt_metadata, create_nectec_stt)
        .with_aliases(&["aiforthai", "ai4thai", "partii", "partii5", "partii4", "nectec-stt"])
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

inventory::submit! {
    PluginConstructor::tts("murf", murf_tts_metadata, create_murf_tts)
        .with_aliases(&["murf-ai", "murf_ai", "murf.ai"])
}

inventory::submit! {
    PluginConstructor::tts("wellsaid", wellsaid_tts_metadata, create_wellsaid_tts)
        .with_aliases(&["wellsaid-labs", "wellsaid_labs", "well-said"])
}

inventory::submit! {
    PluginConstructor::tts("resemble", resemble_tts_metadata, create_resemble_tts)
        .with_aliases(&["resemble-ai", "resemble_ai", "resembleai"])
}

inventory::submit! {
    PluginConstructor::tts("speechify", speechify_tts_metadata, create_speechify_tts)
}

inventory::submit! {
    PluginConstructor::tts("unrealspeech", unrealspeech_tts_metadata, create_unrealspeech_tts)
        .with_aliases(&["unreal-speech", "unreal_speech"])
}

inventory::submit! {
    PluginConstructor::tts("speechmatics", speechmatics_tts_metadata, create_speechmatics_tts)
}

inventory::submit! {
    PluginConstructor::tts("acapela", acapela_tts_metadata, create_acapela_tts)
        .with_aliases(&["acapela-cloud", "acapela_cloud", "acapela-group"])
}

inventory::submit! {
    PluginConstructor::tts("cereproc", cereproc_tts_metadata, create_cereproc_tts)
        .with_aliases(&["cerevoice", "cerevoice-cloud", "cereproc-tts"])
}

inventory::submit! {
    PluginConstructor::tts("reverie", reverie_tts_metadata, create_reverie_tts)
        .with_aliases(&["reverie-tts", "reverie_tts", "reverieinc", "reverie-ai"])
}

inventory::submit! {
    PluginConstructor::tts("yandex", yandex_tts_metadata, create_yandex_tts)
        .with_aliases(&["yandex-tts", "yandex_tts", "speechkit", "yandex-speechkit"])
}

inventory::submit! {
    PluginConstructor::tts("smallest", smallest_tts_metadata, create_smallest_tts)
        .with_aliases(&["smallest-ai", "smallest_ai", "waves", "smallest.ai"])
}

inventory::submit! {
    PluginConstructor::tts("tinkoff", tinkoff_tts_metadata, create_tinkoff_tts)
        .with_aliases(&["tinkoff-voicekit", "tinkoff_voicekit", "tinkoff-tts", "voicekit"])
}

inventory::submit! {
    PluginConstructor::tts("sberdevices", sberdevices_tts_metadata, create_sberdevices_tts)
        .with_aliases(&["salutespeech-tts", "smartspeech-tts", "sber-tts", "salute-tts"])
}

inventory::submit! {
    PluginConstructor::tts("bhashini", bhashini_tts_metadata, create_bhashini_tts)
        .with_aliases(&["bhashini-tts", "bhashini_tts", "ulca-tts", "ai4bharat-tts", "meity-tts"])
}

inventory::submit! {
    PluginConstructor::tts("iflytek", iflytek_tts_metadata, create_iflytek_tts)
        .with_aliases(&["iflytek-tts", "iflytek_tts", "xfyun-tts", "xunfei-tts", "讯飞-tts"])
}

inventory::submit! {
    PluginConstructor::tts("alibaba-cloud", alibaba_cloud_tts_metadata, create_alibaba_cloud_tts)
        .with_aliases(&["alibaba_cloud-tts", "alibabacloud-tts", "alibaba-tts", "dashscope-tts", "aliyun-tts", "阿里云-tts", "cosyvoice", "qwen-tts"])
}

inventory::submit! {
    PluginConstructor::tts("baidu", baidu_tts_metadata, create_baidu_tts)
        .with_aliases(&["baidu-tts", "baidu_tts", "baidu-ai", "baidu-ai-tts", "baidu-speech", "百度语音", "百度语音-tts"])
}

inventory::submit! {
    PluginConstructor::tts("tencent", tencent_tts_metadata, create_tencent_tts)
        .with_aliases(&["tencent-tts", "tencent_tts", "tencent-cloud", "tencent-cloud-tts", "腾讯云", "腾讯语音", "腾讯云语音"])
}

inventory::submit! {
    PluginConstructor::tts("huawei-cloud", huawei_cloud_tts_metadata, create_huawei_cloud_tts)
        .with_aliases(&["huawei-cloud-tts", "huawei_cloud-tts", "huawei-tts", "huawei-sis", "华为云", "华为语音", "华为云语音"])
}

inventory::submit! {
    PluginConstructor::tts("naver-clova", naver_clova_tts_metadata, create_naver_clova_tts)
        .with_aliases(&["naver_clova-tts", "naverclova-tts", "naver-tts", "clova-tts", "naver-voice", "clova-voice", "네이버-tts", "클로바-tts"])
}

inventory::submit! {
    PluginConstructor::tts("zalo-ai", zalo_ai_tts_metadata, create_zalo_ai_tts)
        .with_aliases(&["zalo_ai-tts", "zalo-tts", "zalo", "vng-tts", "vng", "zaloai"])
}

inventory::submit! {
    PluginConstructor::tts("fpt-ai", fpt_ai_tts_metadata, create_fpt_ai_tts)
        .with_aliases(&["fpt_ai-tts", "fpt-tts", "fptai-tts"])
}

inventory::submit! {
    PluginConstructor::tts("viettel-ai", viettel_ai_tts_metadata, create_viettel_ai_tts)
        .with_aliases(&["viettel_ai-tts", "viettel-tts", "vtai-tts", "viettelai-tts"])
}

inventory::submit! {
    PluginConstructor::tts("prosa-ai", prosa_ai_tts_metadata, create_prosa_ai_tts)
        .with_aliases(&["prosa_ai-tts", "prosa-tts", "prosaid-tts", "prosaai-tts"])
}

inventory::submit! {
    PluginConstructor::tts("nectec", nectec_tts_metadata, create_nectec_tts)
        .with_aliases(&["aiforthai-tts", "ai4thai-tts", "vaja9", "vaja", "nectec-tts"])
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

        // All 30 STT providers should be registered
        assert!(registry.has_stt_provider("alibaba-cloud"));
        assert!(registry.has_stt_provider("dashscope")); // alias
        assert!(registry.has_stt_provider("aliyun")); // alias
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
        assert!(registry.has_stt_provider("sarvam"));
        assert!(registry.has_stt_provider("speechmatics"));
        assert!(registry.has_stt_provider("speech-matics")); // alias
        assert!(registry.has_stt_provider("gladia"));
        assert!(registry.has_stt_provider("gladia.io")); // alias
        assert!(registry.has_stt_provider("revai"));
        assert!(registry.has_stt_provider("rev.ai")); // alias
        assert!(registry.has_stt_provider("phonexia"));
        assert!(registry.has_stt_provider("phonexia-stt")); // alias
        assert!(registry.has_stt_provider("reverie"));
        assert!(registry.has_stt_provider("reverie-ai")); // alias
        assert!(registry.has_stt_provider("yandex"));
        assert!(registry.has_stt_provider("speechkit")); // alias
        assert!(registry.has_stt_provider("tinkoff"));
        assert!(registry.has_stt_provider("voicekit")); // alias
        assert!(registry.has_stt_provider("sberdevices"));
        assert!(registry.has_stt_provider("salutespeech")); // alias
        assert!(registry.has_stt_provider("tencent"));
        assert!(registry.has_stt_provider("腾讯云")); // alias
        assert!(registry.has_stt_provider("huawei-cloud"));
        assert!(registry.has_stt_provider("华为云")); // alias
        assert!(registry.has_stt_provider("naver-clova"));
        assert!(registry.has_stt_provider("naver")); // alias
        assert!(registry.has_stt_provider("clova")); // alias
        assert!(registry.has_stt_provider("네이버")); // alias
        assert!(registry.has_stt_provider("클로바")); // alias
        assert!(registry.has_stt_provider("amivoice"));
        assert!(registry.has_stt_provider("ami")); // alias
        assert!(registry.has_stt_provider("アミボイス")); // alias (Japanese)
        assert!(registry.has_stt_provider("bhashini"));
        assert!(registry.has_stt_provider("ulca")); // alias
        assert!(registry.has_stt_provider("ai4bharat")); // alias
        assert!(registry.has_stt_provider("iflytek"));
        assert!(registry.has_stt_provider("xfyun")); // alias
        assert!(registry.has_stt_provider("xunfei")); // alias
        assert!(registry.has_stt_provider("fpt-ai"));
        assert!(registry.has_stt_provider("fpt-stt")); // alias
        assert!(registry.has_stt_provider("fpt")); // alias
        assert!(registry.has_stt_provider("viettel-ai")); // Viettel AI STT
        assert!(registry.has_stt_provider("viettel-stt")); // alias
        assert!(registry.has_stt_provider("viettel")); // alias
        assert!(registry.has_stt_provider("vtai")); // alias
        assert!(registry.has_stt_provider("prosa-ai")); // Prosa.ai STT
        assert!(registry.has_stt_provider("prosa-stt")); // alias
        assert!(registry.has_stt_provider("prosa")); // alias
        assert!(registry.has_stt_provider("prosaid")); // alias
    }

    #[test]
    fn test_builtin_tts_providers_registered() {
        let registry = global_registry();

        // All 37 TTS providers should be registered
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
        assert!(registry.has_tts_provider("murf"));
        assert!(registry.has_tts_provider("wellsaid"));
        assert!(registry.has_tts_provider("resemble"));
        assert!(registry.has_tts_provider("speechify"));
        assert!(registry.has_tts_provider("unrealspeech"));
        assert!(registry.has_tts_provider("speechmatics"));
        assert!(registry.has_tts_provider("acapela"));
        assert!(registry.has_tts_provider("cereproc"));
        assert!(registry.has_tts_provider("reverie"));
        assert!(registry.has_tts_provider("reverie-tts")); // alias
        assert!(registry.has_tts_provider("yandex"));
        assert!(registry.has_tts_provider("speechkit")); // alias
        assert!(registry.has_tts_provider("smallest"));
        assert!(registry.has_tts_provider("smallest-ai")); // alias
        assert!(registry.has_tts_provider("tinkoff"));
        assert!(registry.has_tts_provider("sberdevices"));
        assert!(registry.has_tts_provider("bhashini"));
        assert!(registry.has_tts_provider("bhashini-tts")); // alias
        assert!(registry.has_tts_provider("ulca-tts")); // alias
        assert!(registry.has_tts_provider("iflytek"));
        assert!(registry.has_tts_provider("iflytek-tts")); // alias
        assert!(registry.has_tts_provider("xfyun-tts")); // alias
        assert!(registry.has_tts_provider("alibaba-cloud"));
        assert!(registry.has_tts_provider("dashscope-tts")); // alias
        assert!(registry.has_tts_provider("aliyun-tts")); // alias
        assert!(registry.has_tts_provider("cosyvoice")); // alias
        assert!(registry.has_tts_provider("qwen-tts")); // alias
        assert!(registry.has_tts_provider("baidu"));
        assert!(registry.has_tts_provider("baidu-tts")); // alias
        assert!(registry.has_tts_provider("baidu-ai")); // alias
        assert!(registry.has_tts_provider("baidu-speech")); // alias
        assert!(registry.has_tts_provider("百度语音")); // Chinese alias
        assert!(registry.has_tts_provider("tencent"));
        assert!(registry.has_tts_provider("tencent-tts")); // alias
        assert!(registry.has_tts_provider("tencent-cloud")); // alias
        assert!(registry.has_tts_provider("腾讯云")); // Chinese alias
        assert!(registry.has_tts_provider("huawei-cloud"));
        assert!(registry.has_tts_provider("huawei-cloud-tts")); // alias
        assert!(registry.has_tts_provider("huawei-tts")); // alias
        assert!(registry.has_tts_provider("huawei-sis")); // alias
        assert!(registry.has_tts_provider("华为云")); // Chinese alias
        assert!(registry.has_tts_provider("naver-clova"));
        assert!(registry.has_tts_provider("naver-tts")); // alias
        assert!(registry.has_tts_provider("clova-tts")); // alias
        assert!(registry.has_tts_provider("네이버-tts")); // Korean alias
        assert!(registry.has_tts_provider("클로바-tts")); // Korean alias
        assert!(registry.has_tts_provider("zalo-ai")); // Zalo AI TTS
        assert!(registry.has_tts_provider("zalo-tts")); // alias
        assert!(registry.has_tts_provider("zalo")); // alias
        assert!(registry.has_tts_provider("vng-tts")); // alias
        assert!(registry.has_tts_provider("zaloai")); // alias
        assert!(registry.has_tts_provider("fpt-ai")); // FPT.AI TTS
        assert!(registry.has_tts_provider("fpt-tts")); // alias
        assert!(registry.has_tts_provider("fptai-tts")); // alias
        assert!(registry.has_tts_provider("viettel-ai")); // Viettel AI TTS
        assert!(registry.has_tts_provider("viettel-tts")); // alias
        assert!(registry.has_tts_provider("vtai-tts")); // alias
        assert!(registry.has_tts_provider("viettelai-tts")); // alias
        assert!(registry.has_tts_provider("prosa-ai")); // Prosa.ai TTS
        assert!(registry.has_tts_provider("prosa-tts")); // alias
        assert!(registry.has_tts_provider("prosaid-tts")); // alias
        assert!(registry.has_tts_provider("prosaai-tts")); // alias
        assert!(registry.has_tts_provider("nectec")); // NECTEC AI for Thai TTS
        assert!(registry.has_tts_provider("nectec-tts")); // alias
        assert!(registry.has_tts_provider("aiforthai-tts")); // alias
        assert!(registry.has_tts_provider("ai4thai-tts")); // alias
        assert!(registry.has_tts_provider("vaja9")); // alias
        assert!(registry.has_tts_provider("vaja")); // alias
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
        assert!(registry.has_stt_provider("alibaba_cloud")); // alias for alibaba-cloud
        assert!(registry.has_stt_provider("alibabacloud")); // alias for alibaba-cloud
        assert!(registry.has_stt_provider("alibaba")); // alias for alibaba-cloud
        assert!(registry.has_stt_provider("dashscope")); // alias for alibaba-cloud
        assert!(registry.has_stt_provider("aliyun")); // alias for alibaba-cloud
        assert!(registry.has_stt_provider("qwen-asr")); // alias for alibaba-cloud
        assert!(registry.has_stt_provider("azure")); // alias for microsoft-azure
        assert!(registry.has_stt_provider("watson")); // alias for ibm-watson
        assert!(registry.has_stt_provider("transcribe")); // alias for aws-transcribe
        assert!(registry.has_stt_provider("vachana")); // alias for gnani
        assert!(registry.has_stt_provider("gnani-ai")); // alias for gnani
        assert!(registry.has_stt_provider("sarvam-ai")); // alias for sarvam
        assert!(registry.has_stt_provider("saarika")); // alias for sarvam
        assert!(registry.has_stt_provider("gladia.io")); // alias for gladia
        assert!(registry.has_stt_provider("gladia-io")); // alias for gladia
        assert!(registry.has_stt_provider("gladia_io")); // alias for gladia
        assert!(registry.has_stt_provider("rev-ai")); // alias for revai
        assert!(registry.has_stt_provider("rev_ai")); // alias for revai
        assert!(registry.has_stt_provider("rev.ai")); // alias for revai
        assert!(registry.has_stt_provider("reverie-ai")); // alias for reverie
        assert!(registry.has_stt_provider("reverie_ai")); // alias for reverie
        assert!(registry.has_stt_provider("reverie-stt")); // alias for reverie
        assert!(registry.has_stt_provider("reverieinc")); // alias for reverie
        assert!(registry.has_stt_provider("yandex-stt")); // alias for yandex
        assert!(registry.has_stt_provider("yandex_stt")); // alias for yandex
        assert!(registry.has_stt_provider("speechkit")); // alias for yandex
        assert!(registry.has_stt_provider("yandex-speechkit")); // alias for yandex
        assert!(registry.has_stt_provider("tinkoff-stt")); // alias for tinkoff
        assert!(registry.has_stt_provider("tinkoff_stt")); // alias for tinkoff
        assert!(registry.has_stt_provider("voicekit")); // alias for tinkoff
        assert!(registry.has_stt_provider("tinkoff-voicekit")); // alias for tinkoff
        assert!(registry.has_stt_provider("salutespeech")); // alias for sberdevices
        assert!(registry.has_stt_provider("smartspeech")); // alias for sberdevices
        assert!(registry.has_stt_provider("bhashini-stt")); // alias for bhashini
        assert!(registry.has_stt_provider("bhashini_stt")); // alias for bhashini
        assert!(registry.has_stt_provider("ulca")); // alias for bhashini
        assert!(registry.has_stt_provider("ai4bharat")); // alias for bhashini
        assert!(registry.has_stt_provider("ai4bharat-stt")); // alias for bhashini
        assert!(registry.has_stt_provider("meity")); // alias for bhashini
        assert!(registry.has_stt_provider("meity-stt")); // alias for bhashini
        assert!(registry.has_stt_provider("iflytek-stt")); // alias for iflytek
        assert!(registry.has_stt_provider("iflytek_stt")); // alias for iflytek
        assert!(registry.has_stt_provider("xfyun")); // alias for iflytek
        assert!(registry.has_stt_provider("xunfei")); // alias for iflytek
        assert!(registry.has_stt_provider("naver-clova")); // naver-clova STT
        assert!(registry.has_stt_provider("naver_clova")); // alias for naver-clova
        assert!(registry.has_stt_provider("naverclova")); // alias for naver-clova
        assert!(registry.has_stt_provider("naver")); // alias for naver-clova
        assert!(registry.has_stt_provider("clova")); // alias for naver-clova
        assert!(registry.has_stt_provider("csr")); // alias for naver-clova
        assert!(registry.has_stt_provider("네이버")); // alias for naver-clova (Korean)
        assert!(registry.has_stt_provider("클로바")); // alias for naver-clova (Korean)
        assert!(registry.has_stt_provider("amivoice")); // amivoice STT
        assert!(registry.has_stt_provider("amivoice-stt")); // alias for amivoice
        assert!(registry.has_stt_provider("ami")); // alias for amivoice
        assert!(registry.has_stt_provider("advanced-media")); // alias for amivoice
        assert!(registry.has_stt_provider("アミボイス")); // alias for amivoice (Japanese)
        assert!(registry.has_stt_provider("acp")); // alias for amivoice

        // Test TTS aliases
        assert!(registry.has_tts_provider("polly")); // alias for aws-polly
        assert!(registry.has_tts_provider("play.ht")); // alias for playht
        assert!(registry.has_tts_provider("gnani-ai")); // alias for gnani
        assert!(registry.has_tts_provider("murf-ai")); // alias for murf
        assert!(registry.has_tts_provider("murf.ai")); // alias for murf
        assert!(registry.has_tts_provider("wellsaid-labs")); // alias for wellsaid
        assert!(registry.has_tts_provider("well-said")); // alias for wellsaid
        assert!(registry.has_tts_provider("resemble-ai")); // alias for resemble
        assert!(registry.has_tts_provider("resembleai")); // alias for resemble
        assert!(registry.has_tts_provider("unreal-speech")); // alias for unrealspeech
        assert!(registry.has_tts_provider("unreal_speech")); // alias for unrealspeech
        assert!(registry.has_tts_provider("acapela-cloud")); // alias for acapela
        assert!(registry.has_tts_provider("acapela_cloud")); // alias for acapela
        assert!(registry.has_tts_provider("acapela-group")); // alias for acapela
        assert!(registry.has_tts_provider("cerevoice")); // alias for cereproc
        assert!(registry.has_tts_provider("cerevoice-cloud")); // alias for cereproc
        assert!(registry.has_tts_provider("reverie-tts")); // alias for reverie
        assert!(registry.has_tts_provider("reverie_tts")); // alias for reverie
        assert!(registry.has_tts_provider("reverieinc")); // alias for reverie
        assert!(registry.has_tts_provider("reverie-ai")); // alias for reverie
        assert!(registry.has_tts_provider("yandex-tts")); // alias for yandex
        assert!(registry.has_tts_provider("yandex_tts")); // alias for yandex
        assert!(registry.has_tts_provider("speechkit")); // alias for yandex
        assert!(registry.has_tts_provider("yandex-speechkit")); // alias for yandex
        assert!(registry.has_tts_provider("smallest-ai")); // alias for smallest
        assert!(registry.has_tts_provider("smallest_ai")); // alias for smallest
        assert!(registry.has_tts_provider("waves")); // alias for smallest
        assert!(registry.has_tts_provider("smallest.ai")); // alias for smallest
        assert!(registry.has_tts_provider("bhashini-tts")); // alias for bhashini
        assert!(registry.has_tts_provider("bhashini_tts")); // alias for bhashini
        assert!(registry.has_tts_provider("ulca-tts")); // alias for bhashini
        assert!(registry.has_tts_provider("ai4bharat-tts")); // alias for bhashini
        assert!(registry.has_tts_provider("meity-tts")); // alias for bhashini
        assert!(registry.has_tts_provider("iflytek-tts")); // alias for iflytek
        assert!(registry.has_tts_provider("iflytek_tts")); // alias for iflytek
        assert!(registry.has_tts_provider("xfyun-tts")); // alias for iflytek
        assert!(registry.has_tts_provider("xunfei-tts")); // alias for iflytek
        assert!(registry.has_tts_provider("alibaba-cloud")); // alibaba-cloud TTS
        assert!(registry.has_tts_provider("alibaba_cloud-tts")); // alias for alibaba-cloud
        assert!(registry.has_tts_provider("alibabacloud-tts")); // alias for alibaba-cloud
        assert!(registry.has_tts_provider("alibaba-tts")); // alias for alibaba-cloud
        assert!(registry.has_tts_provider("dashscope-tts")); // alias for alibaba-cloud
        assert!(registry.has_tts_provider("aliyun-tts")); // alias for alibaba-cloud
        assert!(registry.has_tts_provider("cosyvoice")); // alias for alibaba-cloud
        assert!(registry.has_tts_provider("qwen-tts")); // alias for alibaba-cloud
        assert!(registry.has_tts_provider("naver-clova")); // naver-clova TTS
        assert!(registry.has_tts_provider("naver_clova-tts")); // alias for naver-clova
        assert!(registry.has_tts_provider("naverclova-tts")); // alias for naver-clova
        assert!(registry.has_tts_provider("naver-tts")); // alias for naver-clova
        assert!(registry.has_tts_provider("clova-tts")); // alias for naver-clova
        assert!(registry.has_tts_provider("naver-voice")); // alias for naver-clova
        assert!(registry.has_tts_provider("clova-voice")); // alias for naver-clova
        assert!(registry.has_tts_provider("네이버-tts")); // alias for naver-clova (Korean)
        assert!(registry.has_tts_provider("클로바-tts")); // alias for naver-clova (Korean)

        // Test Realtime aliases
        assert!(registry.has_realtime_provider("evi")); // alias for hume
    }
}

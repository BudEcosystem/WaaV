pub mod acapela;
pub mod alibaba_cloud;
pub mod aws_polly;
pub mod azure;
pub mod baidu;
mod base;
pub mod bhashini;
pub mod cartesia;
pub mod cereproc;
pub mod deepgram;
pub mod elevenlabs;
pub mod fpt_ai;
pub mod gnani;
pub mod google;
pub mod hume;
pub mod huawei_cloud;
pub mod ibm_watson;
pub mod iflytek;
pub mod lmnt;
pub mod murf;
pub mod naver_clova;
pub mod nectec;
pub mod openai;
pub mod playht;
pub mod prosa_ai;
pub mod provider;
pub mod resemble;
pub mod reverie;
pub mod sberdevices;
pub mod smallest;
pub mod speechify;
pub mod speechmatics;
pub mod tencent;
pub mod tinkoff;
pub mod unrealspeech;
pub mod wellsaid;
pub mod yandex;
pub mod viettel_ai;
pub mod zalo_ai;

pub use acapela::{
    AcapelaAccountInfo, AcapelaAudioFormat, AcapelaCredentials, AcapelaOutputMode,
    AcapelaRequestBuilder, AcapelaTts, AcapelaTtsConfig, AcapelaTtsConfigBuilder, AcapelaVoice,
    EventsData, PhonemeEvent, StreamChunk, StreamParser, Viseme, WordEvent, ACAPELA_COMMAND_URL,
};
pub use aws_polly::{
    AWS_POLLY_TTS_URL, AwsPollyTTS, AwsPollyTTSConfig, PollyEngine, PollyOutputFormat, PollyVoice,
    TextType,
};
pub use azure::{AZURE_TTS_URL, AzureAudioEncoding, AzureTTS, AzureTTSConfig};
pub use base::{
    AudioCallback, AudioData, BaseTTS, BoxedTTS, ConnectionState, Pronunciation, TTSConfig,
    TTSError, TTSFactory, TTSResult,
};
pub use cartesia::{CARTESIA_TTS_URL, CartesiaTTS};
pub use cereproc::{
    CereprocAudioFormat, CereprocCredentials, CereprocTts, CereprocTtsConfig, CEREVOICE_SPEAK_URL,
};
pub use deepgram::{DEEPGRAM_TTS_URL, DeepgramTTS};
pub use elevenlabs::{ELEVENLABS_TTS_URL, ElevenLabsTTS};
pub use google::{GOOGLE_TTS_URL, GoogleTTS};
pub use hume::{HUME_TTS_STREAM_URL, HumeTTS, HumeTTSConfig};
pub use ibm_watson::{
    IBM_WATSON_TTS_URL, IbmOutputFormat, IbmVoice, IbmWatsonTTS, IbmWatsonTTSConfig,
};
pub use lmnt::{LMNT_TTS_URL, LmntAudioFormat, LmntTts, LmntTtsConfig, LmntVoice};
pub use murf::{
    MURF_TTS_STREAM_URL, MurfAudioFormat, MurfModel, MurfRegion, MurfRequestBuilder, MurfTts,
    MurfTtsConfig,
};
pub use openai::{AudioOutputFormat, OPENAI_TTS_URL, OpenAITTS, OpenAITTSModel, OpenAIVoice};
pub use playht::{
    PLAYHT_TTS_URL, PlayHtAudioFormat, PlayHtModel, PlayHtTts, PlayHtTtsConfig, PlayHtVoice,
};
pub use provider::{TTSProvider, TTSRequestBuilder};
pub use resemble::{
    RESEMBLE_TTS_STREAM_URL, RESEMBLE_VOICES_URL, ResembleModel, ResembleOutputFormat,
    ResemblePrecision, ResembleRequestBuilder, ResembleStreamRequest, ResembleTts,
    ResembleTtsConfig, ResembleVoice,
};
pub use reverie::{
    ReverieGender, ReverieSpeaker, ReverieTts, ReverieTtsAudioFormat, ReverieTtsConfig,
    REVERIE_TTS_URL,
};
pub use smallest::{
    SmallestLanguage, SmallestModel, SmallestOutputFormat, SmallestRequestBuilder, SmallestTts,
    SmallestTtsConfig, SmallestTtsRequest, SmallestVoice, SmallestVoicesResponse,
    SmallestWsRequest, SMALLEST_ADD_VOICE_URL, SMALLEST_API_BASE_URL, SMALLEST_TTS_URL,
    SMALLEST_TTS_WS_URL, SMALLEST_VOICES_URL_TEMPLATE,
};
pub use speechify::{
    SpeechifyAudioFormat, SpeechifyModel, SpeechifyRequestBuilder, SpeechifyStreamRequest,
    SpeechifyTts, SpeechifyTtsConfig, SpeechifyVoice, SPEECHIFY_TTS_STREAM_URL,
    SPEECHIFY_VOICES_URL,
};
pub use speechmatics::{
    SpeechmaticsGenerateRequest, SpeechmaticsOutputFormat, SpeechmaticsRequestBuilder,
    SpeechmaticsTts, SpeechmaticsTtsConfig, SpeechmaticsVoice, SPEECHMATICS_GENERATE_URL,
    SPEECHMATICS_TTS_BASE_URL,
};
pub use unrealspeech::{
    UnrealSpeechBitrate, UnrealSpeechCodec, UnrealSpeechRequestBuilder, UnrealSpeechStreamRequest,
    UnrealSpeechTts, UnrealSpeechTtsConfig, UnrealSpeechVoice, UNREALSPEECH_STREAM_URL,
};
pub use wellsaid::{
    WellSaidAvatar, WellSaidModel, WellSaidRequestBuilder, WellSaidStreamRequest, WellSaidTts,
    WellSaidTtsConfig, WELLSAID_AVATARS_URL, WELLSAID_TTS_STREAM_URL,
};
pub use yandex::{
    YandexAudioFormat, YandexEmotion, YandexTts, YandexTtsConfig, YandexVoice,
    YANDEX_TTS_SYNTHESIZE_URL,
};

// Re-export SberDevices SaluteSpeech TTS implementation
pub use sberdevices::{
    SberDevicesTts, SberTtsAudioFormat, SberTtsConfig, SberTtsScope, SberTtsVoice,
    SBER_TTS_SYNTHESIZE_URL,
};

// Re-export Tinkoff VoiceKit TTS implementation
pub use tinkoff::{
    TinkoffAudioEncoding, TinkoffTts, TinkoffTtsConfig, TinkoffTtsGrpcError, TinkoffVoice,
    TINKOFF_GRPC_ENDPOINT,
};

// Re-export Gnani.ai implementation
pub use gnani::{GnaniGender, GnaniTTS, GnaniTTSConfig, GnaniTTSLanguage};

// Re-export Bhashini ULCA TTS implementation
pub use bhashini::{
    BhashiniTts, BhashiniTtsAudioFormat, BhashiniTtsConfig, BhashiniTtsGender,
    BHASHINI_COMPUTE_URL as BHASHINI_TTS_URL, DEFAULT_TTS_SAMPLE_RATE as BHASHINI_TTS_SAMPLE_RATE,
};

// Re-export iFlytek TTS implementation
pub use iflytek::{
    IFlytekTts, IFlytekTtsConfig, IFlytekTtsEncoding, IFlytekTextEncoding, IFlytekVoice,
    IFLYTEK_TTS_ENDPOINT, IFLYTEK_TTS_HOST, IFLYTEK_TTS_PATH,
    DEFAULT_TTS_SAMPLE_RATE as IFLYTEK_TTS_SAMPLE_RATE,
    DEFAULT_SPEED as IFLYTEK_DEFAULT_SPEED,
    DEFAULT_VOLUME as IFLYTEK_DEFAULT_VOLUME,
    DEFAULT_PITCH as IFLYTEK_DEFAULT_PITCH,
};

// Re-export Alibaba Cloud DashScope TTS implementation
pub use alibaba_cloud::{
    DashScopeTts, DashScopeTtsConfig, DashScopeTtsModel as DashScopeTtsModelEnum,
    DashScopeAudioFormat as DashScopeTtsAudioFormat, DashScopeRegion as DashScopeTtsRegion,
    DASHSCOPE_BEIJING_REALTIME_URL as ALIBABA_TTS_BEIJING_REALTIME_URL,
    DASHSCOPE_SINGAPORE_REALTIME_URL as ALIBABA_TTS_SINGAPORE_REALTIME_URL,
    DASHSCOPE_BEIJING_INFERENCE_URL as ALIBABA_TTS_BEIJING_INFERENCE_URL,
    DASHSCOPE_SINGAPORE_INFERENCE_URL as ALIBABA_TTS_SINGAPORE_INFERENCE_URL,
    DEFAULT_TTS_MODEL as ALIBABA_DEFAULT_TTS_MODEL,
    DEFAULT_SAMPLE_RATE as ALIBABA_TTS_DEFAULT_SAMPLE_RATE,
    DEFAULT_VOICE as ALIBABA_TTS_DEFAULT_VOICE,
};

// Re-export Baidu AI Cloud TTS implementation
pub use baidu::{
    BaiduTts, BaiduTtsConfig, BaiduTtsVoice, BaiduTtsAudioFormat, BaiduVoiceCategory,
    BaiduOAuthResponse, BaiduOAuthError, BaiduTtsErrorResponse,
    BAIDU_TTS_URL, BAIDU_TTS_URL_HTTPS, BAIDU_OAUTH_URL,
    DEFAULT_VOICE as BAIDU_TTS_DEFAULT_VOICE,
    DEFAULT_SPEED as BAIDU_TTS_DEFAULT_SPEED,
    DEFAULT_PITCH as BAIDU_TTS_DEFAULT_PITCH,
    DEFAULT_VOLUME as BAIDU_TTS_DEFAULT_VOLUME,
    MAX_TEXT_LENGTH_GBK as BAIDU_TTS_MAX_TEXT_LENGTH,
};

// Re-export Tencent Cloud TTS implementation
pub use tencent::{
    TencentTts, TencentTtsConfig, TencentTtsVoice, TencentTtsAudioFormat, TencentVoiceCategory,
    TencentTtsResponse, TencentTtsSampleRate, TencentTtsSubtitle,
    TENCENT_TTS_URL, TENCENT_TTS_INTL_URL, TTS_ACTION, TTS_VERSION,
    DEFAULT_VOICE_TYPE as TENCENT_TTS_DEFAULT_VOICE,
    DEFAULT_SPEED as TENCENT_TTS_DEFAULT_SPEED,
    DEFAULT_VOLUME as TENCENT_TTS_DEFAULT_VOLUME,
    MAX_TEXT_LENGTH as TENCENT_TTS_MAX_TEXT_LENGTH,
};

// Re-export Huawei Cloud TTS implementation
pub use huawei_cloud::{
    HuaweiCloudTts, HuaweiCloudTtsConfig, HuaweiTtsVoice, HuaweiTtsAudioFormat,
    HuaweiCloudRegion as HuaweiTtsRegion, HuaweiTtsMode,
    MAX_TEXT_LENGTH as HUAWEI_TTS_MAX_TEXT_LENGTH,
};

// Re-export NAVER CLOVA Voice TTS implementation
pub use naver_clova::{
    NaverClovaTts, NaverClovaTtsConfig, NaverClovaVoice, NaverClovaTtsFormat,
    NAVER_TTS_ENDPOINT, MAX_TEXT_LENGTH as NAVER_TTS_MAX_TEXT_LENGTH,
    DEFAULT_SAMPLE_RATE as NAVER_TTS_DEFAULT_SAMPLE_RATE,
};

// Re-export Zalo AI TTS implementation
pub use zalo_ai::{
    ZaloTts, ZaloTtsConfig, ZaloVoice, ZaloTtsResponse, ZaloTtsData,
    ZALO_TTS_ENDPOINT, AUDIO_SAMPLE_RATE as ZALO_TTS_SAMPLE_RATE,
    DEFAULT_SPEED as ZALO_TTS_DEFAULT_SPEED,
    MIN_SPEED as ZALO_TTS_MIN_SPEED, MAX_SPEED as ZALO_TTS_MAX_SPEED,
};

// Re-export FPT.AI TTS implementation
pub use fpt_ai::{
    FptTts, FptTtsConfig, FptVoice, FptTtsResponse, FptAudioFormat,
    FPT_TTS_ENDPOINT, AUDIO_SAMPLE_RATE as FPT_TTS_SAMPLE_RATE,
    DEFAULT_SPEED as FPT_TTS_DEFAULT_SPEED,
    MIN_SPEED as FPT_TTS_MIN_SPEED, MAX_SPEED as FPT_TTS_MAX_SPEED,
    MAX_TEXT_LENGTH as FPT_TTS_MAX_TEXT_LENGTH,
};

// Re-export Viettel AI TTS implementation
pub use viettel_ai::{
    ViettelTts, ViettelTtsConfig, ViettelVoice, ViettelTtsResponse,
    VIETTEL_TTS_ENDPOINT, VIETTEL_VOICES_ENDPOINT,
    AUDIO_SAMPLE_RATE as VIETTEL_TTS_SAMPLE_RATE,
    DEFAULT_SPEED as VIETTEL_TTS_DEFAULT_SPEED,
    MIN_SPEED as VIETTEL_TTS_MIN_SPEED, MAX_SPEED as VIETTEL_TTS_MAX_SPEED,
    MAX_TEXT_LENGTH as VIETTEL_TTS_MAX_TEXT_LENGTH,
};

// Re-export Prosa.ai TTS implementation
pub use prosa_ai::{
    ProsaTts, ProsaTtsConfig, ProsaTtsVoice, ProsaTtsAudioFormat, ProsaTtsResponse,
    ProsaTtsResult, ProsaTtsError, ProsaTtsRequest, ProsaTtsRequestConfig, ProsaTtsRequestData,
    PROSA_TTS_BASE_URL,
    DEFAULT_SAMPLE_RATE as PROSA_TTS_SAMPLE_RATE,
    DEFAULT_PITCH as PROSA_TTS_DEFAULT_PITCH,
    DEFAULT_TEMPO as PROSA_TTS_DEFAULT_TEMPO,
    MIN_PITCH as PROSA_TTS_MIN_PITCH, MAX_PITCH as PROSA_TTS_MAX_PITCH,
    MIN_TEMPO as PROSA_TTS_MIN_TEMPO, MAX_TEMPO as PROSA_TTS_MAX_TEMPO,
    MAX_SYNC_TEXT_LENGTH as PROSA_TTS_MAX_SYNC_TEXT_LENGTH,
    MAX_ASYNC_TEXT_LENGTH as PROSA_TTS_MAX_ASYNC_TEXT_LENGTH,
};

// Re-export NECTEC AI for Thai TTS implementation
pub use nectec::{
    NectecTts, NectecTtsConfig, NectecTtsError, NectecVoice,
    Vaja9Request, Vaja9Response, chunk_text as nectec_chunk_text,
    VAJA9_ENDPOINT as NECTEC_TTS_ENDPOINT,
    DEFAULT_REQUEST_TIMEOUT as NECTEC_TTS_DEFAULT_REQUEST_TIMEOUT,
    DEFAULT_SAMPLE_RATE as NECTEC_TTS_DEFAULT_SAMPLE_RATE,
    MAX_TEXT_LENGTH as NECTEC_TTS_MAX_TEXT_LENGTH,
    API_KEY_HEADER as NECTEC_TTS_API_KEY_HEADER,
};

use std::collections::HashMap;

/// Factory function to create a TTS provider.
///
/// # Supported Providers
///
/// - `"deepgram"` - Deepgram TTS API
/// - `"elevenlabs"` - ElevenLabs TTS API
/// - `"google"` - Google Cloud Text-to-Speech API
/// - `"azure"` or `"microsoft-azure"` - Microsoft Azure Text-to-Speech API
/// - `"cartesia"` - Cartesia TTS API (Sonic voice models)
/// - `"openai"` - OpenAI TTS API (tts-1, tts-1-hd, gpt-4o-mini-tts)
/// - `"aws-polly"` or `"amazon-polly"` or `"polly"` - Amazon Polly TTS API
/// - `"ibm-watson"` or `"ibm_watson"` or `"watson"` or `"ibm"` - IBM Watson TTS API
/// - `"hume"` or `"hume-ai"` - Hume AI Octave TTS API (natural language emotions)
/// - `"lmnt"` or `"lmnt-ai"` - LMNT TTS API (ultra-low latency ~150ms)
/// - `"playht"` or `"play-ht"` or `"play.ht"` - Play.ht TTS API (voice cloning, ~190ms)
/// - `"murf"` or `"murf-ai"` - Murf.ai TTS API (ultra-low latency ~130ms)
/// - `"wellsaid"` or `"wellsaid-labs"` - WellSaid Labs TTS API (200+ voices, 20+ languages)
/// - `"resemble"` or `"resemble-ai"` - Resemble AI TTS API (149+ languages, voice cloning)
/// - `"speechify"` - Speechify TTS API (50+ languages, voice cloning, ~300ms latency)
/// - `"speechmatics"` - Speechmatics TTS API (4 English voices, <200ms latency, preview)
/// - `"unrealspeech"` or `"unreal-speech"` - Unreal Speech TTS API (~300ms latency, cost-effective)
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::{create_tts_provider, TTSConfig};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("en-US-JennyNeural".to_string()),
///     ..Default::default()
/// };
///
/// let provider = create_tts_provider("azure", config)?;
/// ```
pub fn create_tts_provider(provider_type: &str, config: TTSConfig) -> TTSResult<Box<dyn BaseTTS>> {
    // Delegate to the plugin registry for provider creation
    // This enables extensibility: new providers can be registered without modifying this function
    crate::plugin::global_registry().create_tts(provider_type, config)
}

/// Returns a map of provider names to their default API endpoint URLs.
///
/// Note: Azure uses regional endpoints. The URL returned here is for the
/// default region (eastus). For specific regions, use `AzureRegion::tts_rest_url()`.
/// Note: AWS Polly uses regional endpoints. The URL returned here is a template.
/// Note: IBM Watson uses regional endpoints. The URL returned here is for us-south.
pub fn get_tts_provider_urls() -> HashMap<String, String> {
    let mut urls = HashMap::new();
    urls.insert("acapela".to_string(), ACAPELA_COMMAND_URL.to_string());
    urls.insert("alibaba-cloud".to_string(), ALIBABA_TTS_BEIJING_INFERENCE_URL.to_string());
    urls.insert("baidu".to_string(), BAIDU_TTS_URL_HTTPS.to_string());
    urls.insert("deepgram".to_string(), DEEPGRAM_TTS_URL.to_string());
    urls.insert("elevenlabs".to_string(), ELEVENLABS_TTS_URL.to_string());
    urls.insert("google".to_string(), GOOGLE_TTS_URL.to_string());
    urls.insert("azure".to_string(), AZURE_TTS_URL.to_string());
    urls.insert("cartesia".to_string(), CARTESIA_TTS_URL.to_string());
    urls.insert("cereproc".to_string(), CEREVOICE_SPEAK_URL.to_string());
    urls.insert("openai".to_string(), OPENAI_TTS_URL.to_string());
    urls.insert("aws-polly".to_string(), AWS_POLLY_TTS_URL.to_string());
    urls.insert("ibm-watson".to_string(), IBM_WATSON_TTS_URL.to_string());
    urls.insert("hume".to_string(), HUME_TTS_STREAM_URL.to_string());
    urls.insert("lmnt".to_string(), LMNT_TTS_URL.to_string());
    urls.insert("murf".to_string(), MURF_TTS_STREAM_URL.to_string());
    urls.insert("playht".to_string(), PLAYHT_TTS_URL.to_string());
    urls.insert("wellsaid".to_string(), WELLSAID_TTS_STREAM_URL.to_string());
    urls.insert("resemble".to_string(), RESEMBLE_TTS_STREAM_URL.to_string());
    urls.insert("reverie".to_string(), REVERIE_TTS_URL.to_string());
    urls.insert("speechify".to_string(), SPEECHIFY_TTS_STREAM_URL.to_string());
    urls.insert("speechmatics".to_string(), SPEECHMATICS_GENERATE_URL.to_string());
    urls.insert("unrealspeech".to_string(), UNREALSPEECH_STREAM_URL.to_string());
    urls.insert("yandex".to_string(), YANDEX_TTS_SYNTHESIZE_URL.to_string());
    urls.insert("tinkoff".to_string(), TINKOFF_GRPC_ENDPOINT.to_string());
    urls.insert("sberdevices".to_string(), SBER_TTS_SYNTHESIZE_URL.to_string());
    urls.insert("bhashini".to_string(), BHASHINI_TTS_URL.to_string());
    urls.insert("iflytek".to_string(), IFLYTEK_TTS_ENDPOINT.to_string());
    urls.insert("tencent".to_string(), TENCENT_TTS_INTL_URL.to_string());
    urls.insert("huawei-cloud".to_string(), "https://sis.cn-north-4.myhuaweicloud.com/v1/{project_id}/tts".to_string());
    urls.insert("zalo-ai".to_string(), ZALO_TTS_ENDPOINT.to_string());
    urls.insert("fpt-ai".to_string(), FPT_TTS_ENDPOINT.to_string());
    urls.insert("viettel-ai".to_string(), VIETTEL_TTS_ENDPOINT.to_string());
    urls.insert("prosa-ai".to_string(), PROSA_TTS_BASE_URL.to_string());
    urls.insert("nectec".to_string(), NECTEC_TTS_ENDPOINT.to_string());
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_tts_provider() {
        let config = TTSConfig::default();
        let result = create_tts_provider("deepgram", config);
        assert!(result.is_ok());

        let invalid_result = create_tts_provider("invalid", TTSConfig::default());
        assert!(invalid_result.is_err());
    }

    #[tokio::test]
    async fn test_create_elevenlabs_tts_provider() {
        let config = TTSConfig {
            provider: "elevenlabs".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("test_voice_id".to_string()),
            ..Default::default()
        };
        let result = create_tts_provider("elevenlabs", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_azure_tts_provider() {
        let config = TTSConfig {
            provider: "azure".to_string(),
            api_key: "test_subscription_key".to_string(),
            voice_id: Some("en-US-JennyNeural".to_string()),
            ..Default::default()
        };
        let result = create_tts_provider("azure", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_azure_tts_provider_alias() {
        let config = TTSConfig {
            provider: "microsoft-azure".to_string(),
            api_key: "test_subscription_key".to_string(),
            voice_id: Some("en-US-JennyNeural".to_string()),
            ..Default::default()
        };
        // Both "azure" and "microsoft-azure" should work
        let result = create_tts_provider("microsoft-azure", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_azure_tts_provider_case_insensitive() {
        let config = TTSConfig {
            provider: "azure".to_string(),
            api_key: "test_subscription_key".to_string(),
            voice_id: Some("en-US-JennyNeural".to_string()),
            ..Default::default()
        };
        // Case should not matter
        let result = create_tts_provider("AZURE", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("Azure", config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_tts_provider_urls_includes_azure() {
        let urls = get_tts_provider_urls();
        assert!(urls.contains_key("azure"));
        assert_eq!(urls.get("azure").unwrap(), AZURE_TTS_URL);
    }

    #[test]
    fn test_invalid_provider_error_message_includes_azure() {
        let config = TTSConfig::default();
        let result = create_tts_provider("invalid_provider", config);

        match result {
            Err(TTSError::InvalidConfiguration(msg)) => {
                assert!(
                    msg.contains("azure"),
                    "Error message should mention azure as a supported provider"
                );
            }
            Err(other) => panic!("Expected InvalidConfiguration error, got: {:?}", other),
            Ok(_) => panic!("Expected error for invalid provider"),
        }
    }

    #[tokio::test]
    async fn test_create_openai_tts_provider() {
        let config = TTSConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("nova".to_string()),
            model: "tts-1-hd".to_string(),
            ..Default::default()
        };
        let result = create_tts_provider("openai", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_openai_tts_provider_case_insensitive() {
        let config = TTSConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        // Case should not matter
        let result = create_tts_provider("OPENAI", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("OpenAI", config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_tts_provider_urls_includes_openai() {
        let urls = get_tts_provider_urls();
        assert!(urls.contains_key("openai"));
        assert_eq!(urls.get("openai").unwrap(), OPENAI_TTS_URL);
    }

    #[test]
    fn test_invalid_provider_error_message_includes_openai() {
        let config = TTSConfig::default();
        let result = create_tts_provider("invalid_provider", config);

        match result {
            Err(TTSError::InvalidConfiguration(msg)) => {
                assert!(
                    msg.contains("openai"),
                    "Error message should mention openai as a supported provider"
                );
            }
            Err(other) => panic!("Expected InvalidConfiguration error, got: {:?}", other),
            Ok(_) => panic!("Expected error for invalid provider"),
        }
    }

    #[tokio::test]
    async fn test_create_aws_polly_tts_provider() {
        let config = TTSConfig {
            provider: "aws-polly".to_string(),
            voice_id: Some("Joanna".to_string()),
            model: "neural".to_string(),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(16000),
            ..Default::default()
        };
        let result = create_tts_provider("aws-polly", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_aws_polly_tts_provider_aliases() {
        let config = TTSConfig {
            provider: "aws-polly".to_string(),
            voice_id: Some("Joanna".to_string()),
            ..Default::default()
        };

        // All aliases should work
        let result = create_tts_provider("aws_polly", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("amazon-polly", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("polly", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_aws_polly_tts_provider_case_insensitive() {
        let config = TTSConfig {
            provider: "aws-polly".to_string(),
            voice_id: Some("Joanna".to_string()),
            ..Default::default()
        };
        // Case should not matter
        let result = create_tts_provider("AWS-POLLY", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("Aws-Polly", config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_tts_provider_urls_includes_aws_polly() {
        let urls = get_tts_provider_urls();
        assert!(urls.contains_key("aws-polly"));
        assert_eq!(urls.get("aws-polly").unwrap(), AWS_POLLY_TTS_URL);
    }

    #[test]
    fn test_invalid_provider_error_message_includes_aws_polly() {
        let config = TTSConfig::default();
        let result = create_tts_provider("invalid_provider", config);

        match result {
            Err(TTSError::InvalidConfiguration(msg)) => {
                assert!(
                    msg.contains("aws-polly"),
                    "Error message should mention aws-polly as a supported provider"
                );
            }
            Err(other) => panic!("Expected InvalidConfiguration error, got: {:?}", other),
            Ok(_) => panic!("Expected error for invalid provider"),
        }
    }

    #[tokio::test]
    async fn test_create_ibm_watson_tts_provider() {
        let config = TTSConfig {
            provider: "ibm-watson".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("en-US_AllisonV3Voice".to_string()),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(22050),
            ..Default::default()
        };
        let result = create_tts_provider("ibm-watson", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_ibm_watson_tts_provider_aliases() {
        let config = TTSConfig {
            provider: "ibm-watson".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("en-US_AllisonV3Voice".to_string()),
            ..Default::default()
        };

        // All aliases should work
        let result = create_tts_provider("ibm_watson", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("watson", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("ibm", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_ibm_watson_tts_provider_case_insensitive() {
        let config = TTSConfig {
            provider: "ibm-watson".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("en-US_AllisonV3Voice".to_string()),
            ..Default::default()
        };
        // Case should not matter
        let result = create_tts_provider("IBM-WATSON", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("Ibm-Watson", config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_tts_provider_urls_includes_ibm_watson() {
        let urls = get_tts_provider_urls();
        assert!(urls.contains_key("ibm-watson"));
        assert_eq!(urls.get("ibm-watson").unwrap(), IBM_WATSON_TTS_URL);
    }

    #[test]
    fn test_invalid_provider_error_message_includes_ibm_watson() {
        let config = TTSConfig::default();
        let result = create_tts_provider("invalid_provider", config);

        match result {
            Err(TTSError::InvalidConfiguration(msg)) => {
                assert!(
                    msg.contains("ibm-watson"),
                    "Error message should mention ibm-watson as a supported provider"
                );
            }
            Err(other) => panic!("Expected InvalidConfiguration error, got: {:?}", other),
            Ok(_) => panic!("Expected error for invalid provider"),
        }
    }

    #[tokio::test]
    async fn test_create_lmnt_tts_provider() {
        let config = TTSConfig {
            provider: "lmnt".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("lily".to_string()),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        };
        let result = create_tts_provider("lmnt", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_lmnt_tts_provider_aliases() {
        let config = TTSConfig {
            provider: "lmnt".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("lily".to_string()),
            ..Default::default()
        };

        // All aliases should work
        let result = create_tts_provider("lmnt-ai", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("lmnt_ai", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_lmnt_tts_provider_case_insensitive() {
        let config = TTSConfig {
            provider: "lmnt".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("lily".to_string()),
            ..Default::default()
        };
        // Case should not matter
        let result = create_tts_provider("LMNT", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("Lmnt", config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_tts_provider_urls_includes_lmnt() {
        let urls = get_tts_provider_urls();
        assert!(urls.contains_key("lmnt"));
        assert_eq!(urls.get("lmnt").unwrap(), LMNT_TTS_URL);
    }

    #[test]
    fn test_invalid_provider_error_message_includes_lmnt() {
        let config = TTSConfig::default();
        let result = create_tts_provider("invalid_provider", config);

        match result {
            Err(TTSError::InvalidConfiguration(msg)) => {
                assert!(
                    msg.contains("lmnt"),
                    "Error message should mention lmnt as a supported provider"
                );
            }
            Err(other) => panic!("Expected InvalidConfiguration error, got: {:?}", other),
            Ok(_) => panic!("Expected error for invalid provider"),
        }
    }

    #[tokio::test]
    async fn test_create_playht_tts_provider() {
        // Set required environment variable for Play.ht auth
        // SAFETY: Test-only environment setup, no concurrent access in tests
        unsafe {
            std::env::set_var("PLAYHT_USER_ID", "test-user-id");
        }

        let config = TTSConfig {
            provider: "playht".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("s3://voice-cloning-zero-shot/test/manifest.json".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(48000),
            ..Default::default()
        };
        let result = create_tts_provider("playht", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_playht_tts_provider_aliases() {
        // Set required environment variable for Play.ht auth
        // SAFETY: Test-only environment setup, no concurrent access in tests
        unsafe {
            std::env::set_var("PLAYHT_USER_ID", "test-user-id");
        }

        let config = TTSConfig {
            provider: "playht".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("s3://voice-cloning-zero-shot/test/manifest.json".to_string()),
            ..Default::default()
        };

        // All aliases should work
        let result = create_tts_provider("play-ht", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("play_ht", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("play.ht", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_playht_tts_provider_case_insensitive() {
        // Set required environment variable for Play.ht auth
        // SAFETY: Test-only environment setup, no concurrent access in tests
        unsafe {
            std::env::set_var("PLAYHT_USER_ID", "test-user-id");
        }

        let config = TTSConfig {
            provider: "playht".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("s3://voice-cloning-zero-shot/test/manifest.json".to_string()),
            ..Default::default()
        };
        // Case should not matter
        let result = create_tts_provider("PLAYHT", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("PlayHt", config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_tts_provider_urls_includes_playht() {
        let urls = get_tts_provider_urls();
        assert!(urls.contains_key("playht"));
        assert_eq!(urls.get("playht").unwrap(), PLAYHT_TTS_URL);
    }

    #[test]
    fn test_invalid_provider_error_message_includes_playht() {
        let config = TTSConfig::default();
        let result = create_tts_provider("invalid_provider", config);

        match result {
            Err(TTSError::InvalidConfiguration(msg)) => {
                assert!(
                    msg.contains("playht"),
                    "Error message should mention playht as a supported provider"
                );
            }
            Err(other) => panic!("Expected InvalidConfiguration error, got: {:?}", other),
            Ok(_) => panic!("Expected error for invalid provider"),
        }
    }

    #[tokio::test]
    async fn test_create_wellsaid_tts_provider() {
        let config = TTSConfig {
            provider: "wellsaid".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("3".to_string()), // Alana B.
            model: "legacy".to_string(),
            ..Default::default()
        };
        let result = create_tts_provider("wellsaid", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_wellsaid_tts_provider_aliases() {
        let config = TTSConfig {
            provider: "wellsaid".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("3".to_string()),
            ..Default::default()
        };

        // All aliases should work
        let result = create_tts_provider("wellsaid-labs", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("wellsaid_labs", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("well-said", config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_wellsaid_tts_provider_case_insensitive() {
        let config = TTSConfig {
            provider: "wellsaid".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("3".to_string()),
            ..Default::default()
        };
        // Case should not matter
        let result = create_tts_provider("WELLSAID", config.clone());
        assert!(result.is_ok());

        let result = create_tts_provider("WellSaid", config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_tts_provider_urls_includes_wellsaid() {
        let urls = get_tts_provider_urls();
        assert!(urls.contains_key("wellsaid"));
        assert_eq!(urls.get("wellsaid").unwrap(), WELLSAID_TTS_STREAM_URL);
    }

    #[test]
    fn test_invalid_provider_error_message_includes_wellsaid() {
        let config = TTSConfig::default();
        let result = create_tts_provider("invalid_provider", config);

        match result {
            Err(TTSError::InvalidConfiguration(msg)) => {
                assert!(
                    msg.contains("wellsaid"),
                    "Error message should mention wellsaid as a supported provider"
                );
            }
            Err(other) => panic!("Expected InvalidConfiguration error, got: {:?}", other),
            Ok(_) => panic!("Expected error for invalid provider"),
        }
    }
}

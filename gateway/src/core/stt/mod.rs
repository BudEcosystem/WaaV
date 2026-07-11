pub mod alibaba_cloud;
pub mod amivoice;
pub mod assemblyai;
pub mod aws_transcribe;
pub mod azure;
pub mod baidu;
mod base;
/// Batched / async STT (P5): canonical envelope + per-provider prerecorded request builders.
pub mod batch;
pub mod bhashini;
pub mod cartesia;
pub mod deepgram;
pub mod elevenlabs;
pub mod fpt_ai;
pub mod gladia;
pub mod gnani;
pub mod google;
pub mod groq;
/// Shared circuit-breaker wiring for request/response (HTTP) STT providers.
pub(crate) mod http_resilience;
pub mod huawei_cloud;
pub mod ibm_watson;
pub mod iflytek;
/// WaaV-Infer self-hosted cascade STT adapter (`provider = "waav-infer"`).
pub mod infer;
pub mod naver_clova;
pub mod nectec;
pub mod openai;
pub mod phonexia;
pub mod prosa_ai;
pub mod revai;
pub mod reverie;
pub mod sarvam;
pub mod sberdevices;
pub mod speechmatics;
/// Standardized capability-rich STT config (W1 keystone, additive).
pub mod standard;
pub mod tencent;
pub mod tinkoff;
pub mod viettel_ai;
pub(crate) mod wav;
pub mod yandex;

// Re-export public types and traits
pub use base::{
    BaseSTT, STTConfig, STTConnectionState, STTError, STTErrorCallback, STTFactory, STTHelper,
    STTResult, STTResultCallback, STTStats,
};

// Re-export Alibaba Cloud DashScope implementation
pub use alibaba_cloud::{
    DASHSCOPE_BEIJING_INFERENCE_URL, DASHSCOPE_BEIJING_REALTIME_URL,
    DASHSCOPE_SINGAPORE_INFERENCE_URL, DASHSCOPE_SINGAPORE_REALTIME_URL,
    DEFAULT_AUDIO_FORMAT as DASHSCOPE_DEFAULT_AUDIO_FORMAT,
    DEFAULT_SAMPLE_RATE as DASHSCOPE_DEFAULT_SAMPLE_RATE,
    DEFAULT_SILENCE_DURATION_MS as DASHSCOPE_DEFAULT_SILENCE_DURATION_MS,
    DEFAULT_STT_MODEL as DASHSCOPE_DEFAULT_STT_MODEL, DashScopeAudioFormat, DashScopeErrorCode,
    DashScopeLanguage, DashScopeRegion, DashScopeStt, DashScopeSttConfig, DashScopeSttModel,
    ParaformerFinishTask, ParaformerResponse, ParaformerRunTask, QwenAudioBufferAppend,
    QwenAudioBufferCommit, QwenServerMessage, QwenSessionFinish, QwenSessionUpdate,
    TurnDetectionMode,
};

// Re-export Deepgram implementation
pub use deepgram::{DeepgramSTT, DeepgramSTTConfig};

// Re-export WaaV-Infer self-hosted implementation
pub use infer::{INFER_STT_ALIASES, INFER_STT_PROVIDER_ID, InferSTT};

// Re-export ElevenLabs implementation
pub use elevenlabs::{
    CommitStrategy, ElevenLabsAudioFormat, ElevenLabsMessage, ElevenLabsRegion, ElevenLabsSTT,
    ElevenLabsSTTConfig,
};

// Re-export Google implementation
pub use google::{GoogleSTT, GoogleSTTConfig, STTGoogleAuthClient};

// Re-export Azure implementation
pub use azure::{AzureOutputFormat, AzureProfanityOption, AzureRegion, AzureSTT, AzureSTTConfig};

// Re-export Cartesia implementation
pub use cartesia::{CartesiaAudioEncoding, CartesiaMessage, CartesiaSTT, CartesiaSTTConfig};

// Re-export OpenAI implementation
pub use openai::{
    AudioInputFormat as OpenAIAudioInputFormat, FlushStrategy as OpenAIFlushStrategy, OpenAISTT,
    OpenAISTTConfig, OpenAISTTModel, ResponseFormat as OpenAIResponseFormat,
    TimestampGranularity as OpenAITimestampGranularity,
};

// Re-export AssemblyAI implementation
pub use assemblyai::{
    AssemblyAIEncoding, AssemblyAIMessage, AssemblyAIRegion, AssemblyAISTT, AssemblyAISTTConfig,
    AssemblyAISpeechModel,
};

// Re-export AWS Transcribe implementation
pub use aws_transcribe::{
    AwsRegion, AwsTranscribeSTT, AwsTranscribeSTTConfig, MediaEncoding as AwsMediaEncoding,
    PartialResultsStability,
};

// Re-export AmiVoice implementation
pub use amivoice::{
    AMIVOICE_HTTP_NOLOG_URL, AMIVOICE_HTTP_URL, AMIVOICE_WS_NOLOG_URL, AMIVOICE_WS_URL,
    AmiVoiceAudioFormat, AmiVoiceEngine, AmiVoiceSTT, AmiVoiceSTTConfig,
    DEFAULT_ENGINE as AMIVOICE_DEFAULT_ENGINE,
};

// Re-export IBM Watson implementation
pub use ibm_watson::{
    IBM_IAM_URL, IBM_WATSON_STT_URL, IbmAudioEncoding, IbmModel, IbmRegion, IbmWatsonSTT,
    IbmWatsonSTTConfig,
};

// Re-export Groq implementation
pub use groq::{
    AudioInputFormat as GroqAudioInputFormat, DEFAULT_MAX_FILE_SIZE as GROQ_DEFAULT_MAX_FILE_SIZE,
    DEV_TIER_MAX_FILE_SIZE as GROQ_DEV_TIER_MAX_FILE_SIZE, FlushStrategy as GroqFlushStrategy,
    GROQ_STT_URL, GROQ_TRANSLATION_URL, GroqResponseFormat, GroqSTT, GroqSTTConfig, GroqSTTModel,
    MAX_PROMPT_TOKENS as GROQ_MAX_PROMPT_TOKENS,
    SilenceDetectionConfig as GroqSilenceDetectionConfig,
    TimestampGranularity as GroqTimestampGranularity,
};

// Re-export Gnani.ai implementation
pub use gnani::{
    DecodeError as GnaniDecodeError, GnaniAudioFormat, GnaniGrpcError, GnaniLanguage, GnaniSTT,
    GnaniSTTConfig, SpeechChunk as GnaniSpeechChunk, StreamingError as GnaniStreamingError,
    StreamingRecognitionResponse as GnaniStreamingResponse,
    TranscriptChunk as GnaniTranscriptChunk,
};

// Re-export Sarvam.ai implementation
pub use sarvam::{
    SARVAM_STT_WS_URL, SUPPORTED_LANGUAGES as SarvamSupportedLanguages, SarvamSTT, SarvamSTTConfig,
};

// Re-export Speechmatics implementation
pub use speechmatics::{
    SPEECHMATICS_WS_URL_EU, SPEECHMATICS_WS_URL_US, SpeechmaticsEncoding, SpeechmaticsLanguage,
    SpeechmaticsOperatingPoint, SpeechmaticsRegion, SpeechmaticsSTT, SpeechmaticsSTTConfig,
};

// Re-export Gladia implementation
pub use gladia::{
    GLADIA_LIVE_URL, GladiaBitDepth, GladiaEncoding, GladiaLanguageConfig, GladiaMessagesConfig,
    GladiaRegion, GladiaSTT, GladiaSTTConfig,
};

// Re-export Rev AI implementation
pub use revai::{REVAI_STREAM_URL, RevAISTT, RevAISTTConfig, RevAISampleFormat, RevAITranscriber};

// Re-export Phonexia implementation
pub use phonexia::{
    PhonexiaAuth, PhonexiaResultType, PhonexiaSTT, PhonexiaSTTConfig,
    WEBSOCKET_PATH as PHONEXIA_WEBSOCKET_PATH,
};

// Re-export Reverie implementation
pub use reverie::{
    REVERIE_STREAM_URL, ReverieAudioFormat, ReverieLanguage, ReverieLogging, ReverieSTT,
    ReverieSTTConfig,
};

// Re-export Yandex implementation
pub use yandex::{
    YANDEX_STT_RECOGNIZE_URL, YandexSTT, YandexSTTAudioFormat, YandexSTTConfig, YandexSTTLanguage,
    YandexSTTModel,
};

// Re-export Tinkoff implementation
pub use tinkoff::{
    GRPC_SERVICE_PATH as TINKOFF_GRPC_SERVICE_PATH, TINKOFF_GRPC_ENDPOINT, TinkoffAudioEncoding,
    TinkoffGrpcError, TinkoffStt, TinkoffSttConfig, VadConfig as TinkoffVadConfig,
};

// Re-export SberDevices implementation
pub use sberdevices::{
    OAUTH_ENDPOINT as SBER_OAUTH_ENDPOINT, STT_RECOGNIZE_ENDPOINT as SBER_STT_ENDPOINT,
    SberDevicesSTT, SberSTTAudioFormat, SberSTTConfig, SberSTTLanguage, SberScope,
};

// Re-export Bhashini implementation
pub use bhashini::{
    AI4BHARAT_PIPELINE_ID, BHASHINI_COMPUTE_URL, BHASHINI_CONFIG_URL, BhashiniAudioFormat,
    BhashiniLanguage, BhashiniPipelineProvider, BhashiniStt, BhashiniSttConfig,
    LanguageFamily as BhashiniLanguageFamily, MEITY_PIPELINE_ID,
};

// Re-export iFlytek implementation
pub use iflytek::{
    DEFAULT_FRAME_INTERVAL_MS as IFLYTEK_FRAME_INTERVAL_MS,
    DEFAULT_FRAME_SIZE as IFLYTEK_DEFAULT_FRAME_SIZE,
    DEFAULT_SAMPLE_RATE as IFLYTEK_DEFAULT_SAMPLE_RATE, DataStatus as IFlytekDataStatus,
    IFLYTEK_IAT_ENDPOINT, IFLYTEK_IAT_HOST, IFLYTEK_IST_ENDPOINT, IFLYTEK_IST_HOST,
    IFlytekAsrDomain, IFlytekAsrMode, IFlytekAudioEncoding, IFlytekAuth, IFlytekErrorCode,
    IFlytekLanguage, IFlytekStt, IFlytekSttConfig,
    MAX_REALTIME_DURATION_SECS as IFLYTEK_MAX_REALTIME_DURATION,
    MAX_SHORT_FORM_DURATION_SECS as IFLYTEK_MAX_SHORT_DURATION,
};

// Re-export Baidu implementation
pub use baidu::{
    BAIDU_OAUTH_URL, BAIDU_REALTIME_ASR_URL, BAIDU_SHORT_ASR_URL, BAIDU_SHORT_ASR_URL_HTTPS,
    BaiduAudioFormat, BaiduCancelFrame, BaiduErrorCode, BaiduFinishFrame, BaiduOAuthError,
    BaiduOAuthResponse, BaiduRealtimeResponse, BaiduSampleRate, BaiduShortAsrRequest,
    BaiduShortAsrResponse, BaiduStartFrame, BaiduStt, BaiduSttConfig, BaiduSttModel,
    DEFAULT_AUDIO_FORMAT as BAIDU_DEFAULT_AUDIO_FORMAT, DEFAULT_MODEL as BAIDU_DEFAULT_MODEL,
    DEFAULT_SAMPLE_RATE as BAIDU_DEFAULT_SAMPLE_RATE,
    MAX_REALTIME_AUDIO_DURATION_SECS as BAIDU_MAX_REALTIME_DURATION,
    MAX_SHORT_AUDIO_DURATION_SECS as BAIDU_MAX_SHORT_DURATION,
    RECOMMENDED_CHUNK_DURATION_MS as BAIDU_CHUNK_DURATION_MS,
    TOKEN_VALIDITY_SECS as BAIDU_TOKEN_VALIDITY_SECS,
};

// Re-export Tencent implementation
pub use tencent::{
    DEFAULT_ENGINE_MODEL as TENCENT_DEFAULT_ENGINE_MODEL,
    DEFAULT_SAMPLE_RATE as TENCENT_DEFAULT_SAMPLE_RATE,
    DEFAULT_VOICE_FORMAT as TENCENT_DEFAULT_VOICE_FORMAT,
    RECOMMENDED_CHUNK_DURATION_MS as TENCENT_CHUNK_DURATION_MS,
    SIGNATURE_VALIDITY_SECS as TENCENT_SIGNATURE_VALIDITY_SECS,
    SignatureError as TencentSignatureError, TENCENT_ASR_WS_URL, TencentAsrErrorCode,
    TencentAsrResponse, TencentAsrResult, TencentAudioFormat, TencentEngineModel,
    TencentSignatureBuilder, TencentSliceType, TencentStt, TencentSttConfig, TencentWord,
    TencentWordInfo, VAD_SILENCE_TIME_MAX as TENCENT_VAD_SILENCE_MAX,
    VAD_SILENCE_TIME_MIN as TENCENT_VAD_SILENCE_MIN,
};

// Re-export Huawei Cloud implementation
pub use huawei_cloud::{
    DEFAULT_AUDIO_FORMAT as HUAWEI_DEFAULT_AUDIO_FORMAT, DEFAULT_MODEL as HUAWEI_DEFAULT_MODEL,
    DEFAULT_SAMPLE_RATE as HUAWEI_DEFAULT_SAMPLE_RATE, HuaweiAsrResult as HuaweiAsrResultType,
    HuaweiCancelFrame, HuaweiCloudAsrMode, HuaweiCloudAudioFormat, HuaweiCloudRegion,
    HuaweiCloudStt, HuaweiCloudSttConfig, HuaweiCloudSttModel, HuaweiEndFrame,
    HuaweiRealtimeResponse, HuaweiResponseType, HuaweiShortAsrRequest, HuaweiShortAsrResponse,
    HuaweiSisErrorCode, HuaweiStartFrame, HuaweiTokenManager, HuaweiWordInfo as HuaweiWordInfoType,
    MAX_CONTINUOUS_DURATION_SECS as HUAWEI_MAX_CONTINUOUS_DURATION,
    MAX_SHORT_AUDIO_DURATION_SECS as HUAWEI_MAX_SHORT_DURATION,
    MAX_STREAMING_DURATION_SECS as HUAWEI_MAX_STREAMING_DURATION,
    RECOMMENDED_CHUNK_DURATION_MS as HUAWEI_CHUNK_DURATION_MS, SIS_CHINA_ENDPOINT_FORMAT,
    SIS_INTL_ENDPOINT_FORMAT, TOKEN_VALIDITY_SECS as HUAWEI_TOKEN_VALIDITY_SECS,
};

// Re-export NAVER CLOVA implementation
pub use naver_clova::{
    DEFAULT_SAMPLE_RATE as NAVER_DEFAULT_SAMPLE_RATE,
    MAX_AUDIO_DURATION_SECONDS as NAVER_MAX_AUDIO_DURATION,
    MIN_SAMPLE_RATE as NAVER_MIN_SAMPLE_RATE, NAVER_CSR_ENDPOINT, NaverClovaAudioFormat,
    NaverClovaErrorResponse, NaverClovaLanguage, NaverClovaStt, NaverClovaSttConfig,
    NaverClovaSttResponse,
};

// Re-export FPT.AI implementation
pub use fpt_ai::{
    DEFAULT_REQUEST_TIMEOUT as FPT_DEFAULT_REQUEST_TIMEOUT, FPT_STT_ENDPOINT, FptStt, FptSttConfig,
    FptSttHypothesis, FptSttResponse, MAX_AUDIO_DURATION_MS as FPT_MAX_AUDIO_DURATION_MS,
    MIN_AUDIO_DURATION_MS as FPT_MIN_AUDIO_DURATION_MS,
};

// Re-export Viettel AI implementation
pub use viettel_ai::{
    DEFAULT_CHANNELS as VIETTEL_DEFAULT_CHANNELS,
    DEFAULT_REQUEST_TIMEOUT as VIETTEL_DEFAULT_REQUEST_TIMEOUT,
    DEFAULT_SAMPLE_RATE as VIETTEL_DEFAULT_SAMPLE_RATE, PCM_FORMAT_S16LE as VIETTEL_PCM_FORMAT,
    VIETTEL_STT_ENDPOINT, ViettelStt, ViettelSttConfig, ViettelSttResponse,
};

// Re-export Prosa.ai implementation
pub use prosa_ai::{
    DEFAULT_CHANNELS as PROSA_DEFAULT_CHANNELS, DEFAULT_CHUNK_SIZE as PROSA_DEFAULT_CHUNK_SIZE,
    DEFAULT_REQUEST_TIMEOUT as PROSA_DEFAULT_REQUEST_TIMEOUT,
    DEFAULT_SAMPLE_RATE as PROSA_DEFAULT_SAMPLE_RATE,
    MAX_ASYNC_DURATION_SECS as PROSA_MAX_ASYNC_DURATION,
    MAX_SYNC_DURATION_SECS as PROSA_MAX_SYNC_DURATION, MAX_SYNC_SIZE_BYTES as PROSA_MAX_SYNC_SIZE,
    MIN_AUDIO_BUFFER_SIZE as PROSA_MIN_AUDIO_BUFFER_SIZE, PROSA_STT_BASE_URL,
    PROSA_STT_WS_ENDPOINT, ProsaAudioFormat, ProsaStt, ProsaSttConfig, ProsaSttModel,
    ProsaSttResponse, ProsaSttResult, ProsaSttSegment, ProsaSttWsMessage,
};

// Re-export NECTEC AI for Thai implementation
pub use nectec::{
    API_KEY_HEADER as NECTEC_API_KEY_HEADER, DEFAULT_CHANNELS as NECTEC_DEFAULT_CHANNELS,
    DEFAULT_REQUEST_TIMEOUT as NECTEC_DEFAULT_REQUEST_TIMEOUT,
    DEFAULT_SAMPLE_RATE as NECTEC_DEFAULT_SAMPLE_RATE,
    MAX_AUDIO_DURATION_MS as NECTEC_MAX_AUDIO_DURATION_MS,
    MAX_AUDIO_SIZE_BYTES as NECTEC_MAX_AUDIO_SIZE_BYTES, NectecStt, NectecSttConfig,
    NectecSttError, NectecSttModel, PARTII4_ENDPOINT, PARTII5_ENDPOINT, Partii4OutputFormat,
    Partii4OutputLevel, Partii4Response, Partii5Response,
};

/// Supported STT providers
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum STTProvider {
    /// Alibaba Cloud DashScope STT WebSocket API (25+ languages, Chinese dialects)
    AlibabaCloud,
    /// Baidu AI Cloud Speech STT WebSocket/REST API (百度语音, Chinese dialects)
    Baidu,
    /// Deepgram STT WebSocket API
    Deepgram,
    /// Google Speech-to-Text v2 API
    Google,
    /// ElevenLabs STT Real-Time WebSocket API
    ElevenLabs,
    /// Microsoft Azure Speech-to-Text WebSocket API
    Azure,
    /// Cartesia STT WebSocket API (ink-whisper)
    Cartesia,
    /// OpenAI Whisper STT REST API
    OpenAI,
    /// AssemblyAI Streaming STT v3 WebSocket API
    AssemblyAI,
    /// Amazon Transcribe Streaming STT API
    AwsTranscribe,
    /// IBM Watson Speech-to-Text WebSocket API
    IbmWatson,
    /// Groq Whisper STT REST API (ultra-fast)
    Groq,
    /// Sarvam.ai Saarika STT WebSocket API (Indian languages)
    Sarvam,
    /// Speechmatics Real-time STT WebSocket API (55+ languages)
    Speechmatics,
    /// Gladia Live STT v2 WebSocket API (110+ languages, EU-based)
    Gladia,
    /// Rev AI Streaming STT WebSocket API (9+ streaming languages)
    RevAI,
    /// Phonexia On-Premises STT WebSocket API (57-64 languages, voice biometrics)
    Phonexia,
    /// Reverie Language Technologies STT WebSocket API (22+ Indian languages)
    Reverie,
    /// Yandex SpeechKit STT API (Russia/CIS region)
    Yandex,
    /// Tinkoff VoiceKit STT gRPC API (Russian language specialized)
    Tinkoff,
    /// SberDevices SaluteSpeech STT REST API (Russian, CIS languages)
    SberDevices,
    /// Tencent Cloud ASR WebSocket API (11 languages, Chinese dialects)
    Tencent,
    /// Huawei Cloud SIS STT WebSocket/REST API (华为云语音, Chinese + minority languages)
    HuaweiCloud,
    /// NAVER CLOVA Speech Recognition REST API (Korean, Japanese, English, Chinese)
    NaverClova,
    /// Bhashini (ULCA) STT REST API (22+ Indian languages)
    Bhashini,
    /// iFlytek (科大讯飞) STT WebSocket API (30+ languages, Chinese leader)
    IFlytek,
    /// FPT.AI STT REST API (Vietnamese language, FPT Corporation)
    FptAi,
    /// Viettel AI STT REST API (Vietnamese language, 96% accuracy)
    ViettelAi,
    /// Prosa.ai STT WebSocket/REST API (Indonesian, Javanese, Sundanese, English)
    ProsaAi,
    /// NECTEC AI for Thai STT REST API (Partii4/Partii5, Thai language)
    Nectec,
}

impl std::fmt::Display for STTProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            STTProvider::AlibabaCloud => write!(f, "alibaba-cloud"),
            STTProvider::Baidu => write!(f, "baidu"),
            STTProvider::Deepgram => write!(f, "deepgram"),
            STTProvider::Google => write!(f, "google"),
            STTProvider::ElevenLabs => write!(f, "elevenlabs"),
            STTProvider::Azure => write!(f, "microsoft-azure"),
            STTProvider::Cartesia => write!(f, "cartesia"),
            STTProvider::OpenAI => write!(f, "openai"),
            STTProvider::AssemblyAI => write!(f, "assemblyai"),
            STTProvider::AwsTranscribe => write!(f, "aws-transcribe"),
            STTProvider::IbmWatson => write!(f, "ibm-watson"),
            STTProvider::Groq => write!(f, "groq"),
            STTProvider::Sarvam => write!(f, "sarvam"),
            STTProvider::Speechmatics => write!(f, "speechmatics"),
            STTProvider::Gladia => write!(f, "gladia"),
            STTProvider::RevAI => write!(f, "revai"),
            STTProvider::Phonexia => write!(f, "phonexia"),
            STTProvider::Reverie => write!(f, "reverie"),
            STTProvider::Yandex => write!(f, "yandex"),
            STTProvider::Tinkoff => write!(f, "tinkoff"),
            STTProvider::SberDevices => write!(f, "sberdevices"),
            STTProvider::Tencent => write!(f, "tencent"),
            STTProvider::HuaweiCloud => write!(f, "huawei-cloud"),
            STTProvider::NaverClova => write!(f, "naver-clova"),
            STTProvider::Bhashini => write!(f, "bhashini"),
            STTProvider::IFlytek => write!(f, "iflytek"),
            STTProvider::FptAi => write!(f, "fpt-ai"),
            STTProvider::ViettelAi => write!(f, "viettel-ai"),
            STTProvider::ProsaAi => write!(f, "prosa-ai"),
            STTProvider::Nectec => write!(f, "nectec"),
        }
    }
}

impl std::str::FromStr for STTProvider {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "alibaba-cloud" | "alibaba_cloud" | "alibabacloud" | "alibaba" | "dashscope"
            | "aliyun" | "阿里云" | "qwen-asr" => Ok(STTProvider::AlibabaCloud),
            "baidu" | "baidu-ai" | "baidu_ai" | "baiduai" | "百度" | "百度语音"
            | "baidu-speech" | "baidu_speech" => Ok(STTProvider::Baidu),
            "deepgram" => Ok(STTProvider::Deepgram),
            "google" => Ok(STTProvider::Google),
            "elevenlabs" => Ok(STTProvider::ElevenLabs),
            "microsoft-azure" | "azure" => Ok(STTProvider::Azure),
            "cartesia" => Ok(STTProvider::Cartesia),
            "openai" => Ok(STTProvider::OpenAI),
            "assemblyai" => Ok(STTProvider::AssemblyAI),
            "aws-transcribe" | "aws_transcribe" | "amazon-transcribe" | "transcribe" => {
                Ok(STTProvider::AwsTranscribe)
            }
            "ibm-watson" | "ibm_watson" | "watson" | "ibm" => Ok(STTProvider::IbmWatson),
            "groq" => Ok(STTProvider::Groq),
            "sarvam" | "sarvam-ai" | "sarvam.ai" | "saarika" => Ok(STTProvider::Sarvam),
            "speechmatics" | "speech-matics" | "speech_matics" => Ok(STTProvider::Speechmatics),
            "gladia" | "gladia.io" | "gladia-io" | "gladia_io" => Ok(STTProvider::Gladia),
            "revai" | "rev-ai" | "rev_ai" | "rev.ai" => Ok(STTProvider::RevAI),
            "phonexia" | "phonexia-stt" | "phonexia_stt" => Ok(STTProvider::Phonexia),
            "reverie" | "reverie-ai" | "reverie_ai" | "reverie-stt" | "reverieinc" => {
                Ok(STTProvider::Reverie)
            }
            "yandex" | "yandex-stt" | "yandex_stt" | "speechkit" | "yandex-speechkit" => {
                Ok(STTProvider::Yandex)
            }
            "tinkoff" | "tinkoff-stt" | "tinkoff_stt" | "voicekit" | "tinkoff-voicekit" => {
                Ok(STTProvider::Tinkoff)
            }
            "sberdevices" | "sber" | "sber-devices" | "sber_devices" | "salutespeech"
            | "salute-speech" | "smartspeech" => Ok(STTProvider::SberDevices),
            "tencent" | "tencent-cloud" | "tencent_cloud" | "tencentcloud" | "腾讯云" | "腾讯" => {
                Ok(STTProvider::Tencent)
            }
            "huawei-cloud" | "huawei_cloud" | "huaweicloud" | "huawei" | "华为云" | "华为"
            | "sis" | "huawei-sis" => Ok(STTProvider::HuaweiCloud),
            "naver-clova" | "naver_clova" | "naverclova" | "naver" | "clova" | "csr" | "네이버"
            | "클로바" => Ok(STTProvider::NaverClova),
            "bhashini" | "ulca" | "ai4bharat" | "meity" => Ok(STTProvider::Bhashini),
            "iflytek" | "ifly" | "xfyun" | "xunfei" | "科大讯飞" | "讯飞" => {
                Ok(STTProvider::IFlytek)
            }
            "fpt-ai" | "fpt_ai" | "fptai" | "fpt" => Ok(STTProvider::FptAi),
            "viettel-ai" | "viettel_ai" | "viettelai" | "viettel" | "vtai" => {
                Ok(STTProvider::ViettelAi)
            }
            "prosa-ai" | "prosa_ai" | "prosai" | "prosa" | "prosa.ai" => Ok(STTProvider::ProsaAi),
            "nectec" | "aiforthai" | "ai4thai" | "partii" | "partii5" | "partii4" => {
                Ok(STTProvider::Nectec)
            }
            _ => Err(STTError::ConfigurationError(format!(
                "Unsupported STT provider: {s}. Supported providers: alibaba-cloud, baidu, deepgram, google, elevenlabs, microsoft-azure, cartesia, openai, assemblyai, aws-transcribe, ibm-watson, groq, sarvam, speechmatics, gladia, revai, phonexia, reverie, yandex, tinkoff, sberdevices, tencent, huawei-cloud, naver-clova, bhashini, iflytek, fpt-ai, viettel-ai, prosa-ai, nectec"
            ))),
        }
    }
}

/// Factory function to create STT providers by name
///
/// # Arguments
/// * `provider` - The name of the STT provider (e.g., "deepgram")
/// * `config` - Configuration for the STT provider
///
/// # Returns
/// * `Result<Box<dyn BaseSTT>, STTError>` - A boxed STT provider or error
///
/// # Examples
/// ```rust,no_run
/// use waav_gateway::core::stt::{create_stt_provider, STTConfig};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = STTConfig {
///         provider: "deepgram".to_string(),
///         api_key: "your-deepgram-api-key".to_string(),
///         language: "en-US".to_string(),
///         sample_rate: 16000,
///         channels: 1,
///         punctuation: true,
///         encoding: "linear16".to_string(),
///         model: "nova-3".to_string(),
///     };
///
///     // Create a Deepgram STT provider
///     let mut stt = create_stt_provider("deepgram", config)?;
///
///     // Use the provider
///     if stt.is_ready() {
///         let audio_data = vec![0u8; 1024];
///         stt.send_audio(audio_data.into()).await?;
///     }
///
///     Ok(())
/// }
/// ```
pub fn create_stt_provider(
    provider: &str,
    config: STTConfig,
) -> Result<Box<dyn BaseSTT>, STTError> {
    // Delegate to the plugin registry
    // This enables dynamic provider registration while maintaining backward compatibility
    crate::plugin::global_registry().create_stt(provider, config)
}

/// Factory function to create STT providers using the enum directly
///
/// # Arguments
/// * `provider` - The STT provider enum
/// * `config` - Configuration for the STT provider
///
/// # Returns
/// * `Result<Box<dyn BaseSTT>, STTError>` - A boxed STT provider or error
///
/// # Examples
/// ```rust,no_run
/// use waav_gateway::core::stt::{create_stt_provider_from_enum, STTProvider, STTConfig};
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = STTConfig {
///         provider: "deepgram".to_string(),
///         api_key: "your-deepgram-api-key".to_string(),
///         language: "en-US".to_string(),
///         sample_rate: 16000,
///         channels: 1,
///         punctuation: true,
///         encoding: "linear16".to_string(),
///         model: "nova-3".to_string(),
///     };
///
///     // Create a Deepgram STT provider using enum
///     let mut stt = create_stt_provider_from_enum(STTProvider::Deepgram, config)?;
///
///     Ok(())
/// }
/// ```
pub fn create_stt_provider_from_enum(
    provider: STTProvider,
    config: STTConfig,
) -> Result<Box<dyn BaseSTT>, STTError> {
    // Delegate to the plugin registry using the provider's string representation
    crate::plugin::global_registry().create_stt(&provider.to_string(), config)
}

/// Get a list of all supported STT providers
///
/// # Returns
/// * `Vec<&'static str>` - List of supported provider names
///
/// # Examples
/// ```rust
/// use waav_gateway::core::stt::get_supported_stt_providers;
///
/// let providers = get_supported_stt_providers();
/// println!("Supported STT providers: {:?}", providers);
/// // Output: ["deepgram", "google", "elevenlabs"]
/// ```
pub fn get_supported_stt_providers() -> Vec<&'static str> {
    vec![
        "alibaba-cloud",
        "baidu",
        "deepgram",
        "google",
        "elevenlabs",
        "microsoft-azure",
        "cartesia",
        "openai",
        "assemblyai",
        "aws-transcribe",
        "ibm-watson",
        "groq",
        "sarvam",
        "speechmatics",
        "gladia",
        "revai",
        "phonexia",
        "reverie",
        "yandex",
        "tinkoff",
        "sberdevices",
        "tencent",
        "huawei-cloud",
        "naver-clova",
        "bhashini",
        "iflytek",
        "fpt-ai",
        "viettel-ai",
        "prosa-ai",
        "nectec",
    ]
}

#[cfg(test)]
mod factory_tests {
    use super::*;

    #[test]
    fn test_stt_provider_enum_from_string() {
        // Test valid provider names - Deepgram
        assert_eq!(
            "deepgram".parse::<STTProvider>().unwrap(),
            STTProvider::Deepgram
        );
        assert_eq!(
            "Deepgram".parse::<STTProvider>().unwrap(),
            STTProvider::Deepgram
        );
        assert_eq!(
            "DEEPGRAM".parse::<STTProvider>().unwrap(),
            STTProvider::Deepgram
        );

        // Test valid provider names - Google
        assert_eq!(
            "google".parse::<STTProvider>().unwrap(),
            STTProvider::Google
        );
        assert_eq!(
            "Google".parse::<STTProvider>().unwrap(),
            STTProvider::Google
        );
        assert_eq!(
            "GOOGLE".parse::<STTProvider>().unwrap(),
            STTProvider::Google
        );

        // Test valid provider names - ElevenLabs
        assert_eq!(
            "elevenlabs".parse::<STTProvider>().unwrap(),
            STTProvider::ElevenLabs
        );
        assert_eq!(
            "ElevenLabs".parse::<STTProvider>().unwrap(),
            STTProvider::ElevenLabs
        );
        assert_eq!(
            "ELEVENLABS".parse::<STTProvider>().unwrap(),
            STTProvider::ElevenLabs
        );

        // Test valid provider names - Azure (both canonical and shorthand)
        assert_eq!(
            "microsoft-azure".parse::<STTProvider>().unwrap(),
            STTProvider::Azure
        );
        assert_eq!(
            "Microsoft-Azure".parse::<STTProvider>().unwrap(),
            STTProvider::Azure
        );
        assert_eq!(
            "MICROSOFT-AZURE".parse::<STTProvider>().unwrap(),
            STTProvider::Azure
        );
        assert_eq!("azure".parse::<STTProvider>().unwrap(), STTProvider::Azure);
        assert_eq!("Azure".parse::<STTProvider>().unwrap(), STTProvider::Azure);
        assert_eq!("AZURE".parse::<STTProvider>().unwrap(), STTProvider::Azure);

        // Test invalid provider name
        let result = "invalid".parse::<STTProvider>();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("Unsupported STT provider: invalid"));
        }
    }

    #[test]
    fn test_stt_provider_enum_display() {
        assert_eq!(STTProvider::Deepgram.to_string(), "deepgram");
        assert_eq!(STTProvider::Google.to_string(), "google");
        assert_eq!(STTProvider::ElevenLabs.to_string(), "elevenlabs");
        assert_eq!(STTProvider::Azure.to_string(), "microsoft-azure");
        assert_eq!(STTProvider::Cartesia.to_string(), "cartesia");
    }

    #[test]
    fn test_get_supported_stt_providers() {
        let providers = get_supported_stt_providers();
        assert_eq!(
            providers,
            vec![
                "alibaba-cloud",
                "baidu",
                "deepgram",
                "google",
                "elevenlabs",
                "microsoft-azure",
                "cartesia",
                "openai",
                "assemblyai",
                "aws-transcribe",
                "ibm-watson",
                "groq",
                "sarvam",
                "speechmatics",
                "gladia",
                "revai",
                "phonexia",
                "reverie",
                "yandex",
                "tinkoff",
                "sberdevices",
                "tencent",
                "huawei-cloud",
                "naver-clova",
                "bhashini",
                "iflytek",
                "fpt-ai",
                "viettel-ai",
                "prosa-ai",
                "nectec",
            ]
        );
        assert!(providers.contains(&"alibaba-cloud"));
        assert!(providers.contains(&"baidu"));
        assert!(providers.contains(&"deepgram"));
        assert!(providers.contains(&"google"));
        assert!(providers.contains(&"elevenlabs"));
        assert!(providers.contains(&"microsoft-azure"));
        assert!(providers.contains(&"openai"));
        assert!(providers.contains(&"cartesia"));
        assert!(providers.contains(&"assemblyai"));
        assert!(providers.contains(&"aws-transcribe"));
        assert!(providers.contains(&"ibm-watson"));
        assert!(providers.contains(&"groq"));
        assert!(providers.contains(&"phonexia"));
        assert!(providers.contains(&"gladia"));
        assert!(providers.contains(&"revai"));
        assert!(providers.contains(&"reverie"));
        assert!(providers.contains(&"yandex"));
        assert!(providers.contains(&"tinkoff"));
        assert!(providers.contains(&"sberdevices"));
        assert!(providers.contains(&"fpt-ai"));
        assert!(providers.contains(&"viettel-ai"));
        assert!(providers.contains(&"prosa-ai"));
    }

    #[tokio::test]
    async fn test_create_stt_provider_with_invalid_config() {
        let config = STTConfig {
            model: "nova-3".to_string(),
            provider: "deepgram".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
        };

        let result = create_stt_provider("deepgram", config);
        assert!(result.is_err());
        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key is required"));
        }
    }

    #[test]
    fn test_create_stt_provider_from_enum() {
        let config = STTConfig {
            model: "nova-3".to_string(),
            provider: "deepgram".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
        };

        let result = create_stt_provider_from_enum(STTProvider::Deepgram, config);
        assert!(result.is_err());
        // Should fail because of empty API key
    }

    #[test]
    fn test_create_stt_provider_elevenlabs_valid() {
        let config = STTConfig {
            provider: "elevenlabs".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "".to_string(),
        };

        let result = create_stt_provider("elevenlabs", config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(
            stt.get_provider_info(),
            "ElevenLabs STT Real-Time WebSocket"
        );
    }

    #[test]
    fn test_create_stt_provider_elevenlabs_empty_api_key() {
        let config = STTConfig {
            provider: "elevenlabs".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "".to_string(),
        };

        let result = create_stt_provider("elevenlabs", config);
        assert!(result.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key is required"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_create_stt_provider_from_enum_elevenlabs() {
        let config = STTConfig {
            provider: "elevenlabs".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "".to_string(),
        };

        let result = create_stt_provider_from_enum(STTProvider::ElevenLabs, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_message_includes_elevenlabs() {
        let result = "invalid".parse::<STTProvider>();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("elevenlabs"));
        }
    }

    #[test]
    fn test_create_stt_provider_azure_valid() {
        let config = STTConfig {
            provider: "microsoft-azure".to_string(),
            api_key: "test_subscription_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "".to_string(),
        };

        let result = create_stt_provider("microsoft-azure", config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "Microsoft Azure Speech-to-Text");
        assert!(!stt.is_ready()); // Not connected yet
    }

    #[test]
    fn test_create_stt_provider_azure_shorthand() {
        let config = STTConfig {
            provider: "azure".to_string(),
            api_key: "test_subscription_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "".to_string(),
        };

        // Test that "azure" shorthand also works
        let result = create_stt_provider("azure", config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "Microsoft Azure Speech-to-Text");
    }

    #[test]
    fn test_create_stt_provider_azure_empty_api_key() {
        let config = STTConfig {
            provider: "microsoft-azure".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "".to_string(),
        };

        let result = create_stt_provider("microsoft-azure", config);
        assert!(result.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("subscription key"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_create_stt_provider_from_enum_azure() {
        let config = STTConfig {
            provider: "microsoft-azure".to_string(),
            api_key: "test_subscription_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "".to_string(),
        };

        let result = create_stt_provider_from_enum(STTProvider::Azure, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_message_includes_microsoft_azure() {
        let result = "invalid".parse::<STTProvider>();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("microsoft-azure"));
        }
    }

    // Cartesia STT provider tests

    #[test]
    fn test_stt_provider_enum_cartesia_from_string() {
        // Test valid provider names - Cartesia
        assert_eq!(
            "cartesia".parse::<STTProvider>().unwrap(),
            STTProvider::Cartesia
        );
        assert_eq!(
            "Cartesia".parse::<STTProvider>().unwrap(),
            STTProvider::Cartesia
        );
        assert_eq!(
            "CARTESIA".parse::<STTProvider>().unwrap(),
            STTProvider::Cartesia
        );
    }

    #[test]
    fn test_create_stt_provider_cartesia_valid() {
        let config = STTConfig {
            provider: "cartesia".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm_s16le".to_string(),
            model: "ink-whisper".to_string(),
        };

        let result = create_stt_provider("cartesia", config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "Cartesia STT (ink-whisper)");
        assert!(!stt.is_ready()); // Not connected yet
    }

    #[test]
    fn test_create_stt_provider_cartesia_empty_api_key() {
        let config = STTConfig {
            provider: "cartesia".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm_s16le".to_string(),
            model: "ink-whisper".to_string(),
        };

        let result = create_stt_provider("cartesia", config);
        assert!(result.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key is required"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_create_stt_provider_from_enum_cartesia() {
        let config = STTConfig {
            provider: "cartesia".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm_s16le".to_string(),
            model: "ink-whisper".to_string(),
        };

        let result = create_stt_provider_from_enum(STTProvider::Cartesia, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_message_includes_cartesia() {
        let result = "invalid".parse::<STTProvider>();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("cartesia"));
        }
    }

    // AssemblyAI STT provider tests

    #[test]
    fn test_stt_provider_enum_assemblyai_from_string() {
        // Test valid provider names - AssemblyAI
        assert_eq!(
            "assemblyai".parse::<STTProvider>().unwrap(),
            STTProvider::AssemblyAI
        );
        assert_eq!(
            "AssemblyAI".parse::<STTProvider>().unwrap(),
            STTProvider::AssemblyAI
        );
        assert_eq!(
            "ASSEMBLYAI".parse::<STTProvider>().unwrap(),
            STTProvider::AssemblyAI
        );
    }

    #[test]
    fn test_stt_provider_enum_assemblyai_display() {
        assert_eq!(STTProvider::AssemblyAI.to_string(), "assemblyai");
    }

    #[test]
    fn test_create_stt_provider_assemblyai_valid() {
        let config = STTConfig {
            provider: "assemblyai".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm_s16le".to_string(),
            model: "".to_string(),
        };

        let result = create_stt_provider("assemblyai", config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "AssemblyAI Streaming STT v3");
        assert!(!stt.is_ready()); // Not connected yet
    }

    #[test]
    fn test_create_stt_provider_assemblyai_empty_api_key() {
        let config = STTConfig {
            provider: "assemblyai".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm_s16le".to_string(),
            model: "".to_string(),
        };

        let result = create_stt_provider("assemblyai", config);
        assert!(result.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key is required"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_create_stt_provider_from_enum_assemblyai() {
        let config = STTConfig {
            provider: "assemblyai".to_string(),
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm_s16le".to_string(),
            model: "".to_string(),
        };

        let result = create_stt_provider_from_enum(STTProvider::AssemblyAI, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_message_includes_assemblyai() {
        let result = "invalid".parse::<STTProvider>();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("assemblyai"));
        }
    }

    // Groq STT provider tests

    #[test]
    fn test_stt_provider_enum_groq_from_string() {
        // Test valid provider names - Groq
        assert_eq!("groq".parse::<STTProvider>().unwrap(), STTProvider::Groq);
        assert_eq!("Groq".parse::<STTProvider>().unwrap(), STTProvider::Groq);
        assert_eq!("GROQ".parse::<STTProvider>().unwrap(), STTProvider::Groq);
    }

    #[test]
    fn test_stt_provider_enum_groq_display() {
        assert_eq!(STTProvider::Groq.to_string(), "groq");
    }

    #[test]
    fn test_create_stt_provider_groq_valid() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: "gsk_test_key_12345".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
        };

        let result = create_stt_provider("groq", config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "Groq Whisper STT");
        assert!(!stt.is_ready()); // Not connected yet (REST-based, always ready after connect)
    }

    #[test]
    fn test_create_stt_provider_groq_empty_api_key() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
        };

        let result = create_stt_provider("groq", config);
        assert!(result.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key is required"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_create_stt_provider_from_enum_groq() {
        let config = STTConfig {
            provider: "groq".to_string(),
            api_key: "gsk_test_key_12345".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
        };

        let result = create_stt_provider_from_enum(STTProvider::Groq, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_message_includes_groq() {
        let result = "invalid".parse::<STTProvider>();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("groq"));
        }
    }

    // IBM Watson STT provider tests

    #[test]
    fn test_stt_provider_enum_ibm_watson_from_string() {
        // Test valid provider names - IBM Watson
        assert_eq!(
            "ibm-watson".parse::<STTProvider>().unwrap(),
            STTProvider::IbmWatson
        );
        assert_eq!(
            "ibm_watson".parse::<STTProvider>().unwrap(),
            STTProvider::IbmWatson
        );
        assert_eq!(
            "watson".parse::<STTProvider>().unwrap(),
            STTProvider::IbmWatson
        );
        assert_eq!(
            "ibm".parse::<STTProvider>().unwrap(),
            STTProvider::IbmWatson
        );
    }

    #[test]
    fn test_stt_provider_enum_ibm_watson_display() {
        assert_eq!(STTProvider::IbmWatson.to_string(), "ibm-watson");
    }

    #[test]
    fn test_create_stt_provider_ibm_watson_valid() {
        let config = STTConfig {
            provider: "ibm-watson".to_string(),
            api_key: "test_api_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "audio/l16".to_string(),
            model: "en-US_Telephony".to_string(),
        };

        let result = create_stt_provider("ibm-watson", config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "IBM Watson Speech-to-Text");
        assert!(!stt.is_ready()); // Not connected yet
    }

    #[test]
    fn test_create_stt_provider_ibm_watson_empty_api_key() {
        let config = STTConfig {
            provider: "ibm-watson".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "audio/l16".to_string(),
            model: "en-US_Telephony".to_string(),
        };

        let result = create_stt_provider("ibm-watson", config);
        assert!(result.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key is required"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_create_stt_provider_from_enum_ibm_watson() {
        let config = STTConfig {
            provider: "ibm-watson".to_string(),
            api_key: "test_api_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "audio/l16".to_string(),
            model: "en-US_Telephony".to_string(),
        };

        let result = create_stt_provider_from_enum(STTProvider::IbmWatson, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_message_includes_ibm_watson() {
        let result = "invalid".parse::<STTProvider>();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("ibm-watson"));
        }
    }

    // Gladia STT provider tests

    #[test]
    fn test_stt_provider_enum_gladia_from_string() {
        // Test valid provider names - Gladia
        assert_eq!(
            "gladia".parse::<STTProvider>().unwrap(),
            STTProvider::Gladia
        );
        assert_eq!(
            "Gladia".parse::<STTProvider>().unwrap(),
            STTProvider::Gladia
        );
        assert_eq!(
            "GLADIA".parse::<STTProvider>().unwrap(),
            STTProvider::Gladia
        );
        assert_eq!(
            "gladia.io".parse::<STTProvider>().unwrap(),
            STTProvider::Gladia
        );
    }

    #[test]
    fn test_stt_provider_enum_gladia_display() {
        assert_eq!(STTProvider::Gladia.to_string(), "gladia");
    }

    #[test]
    fn test_create_stt_provider_gladia_valid() {
        let config = STTConfig {
            provider: "gladia".to_string(),
            api_key: "test_api_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm".to_string(),
            model: "solaria-1".to_string(),
        };

        let result = create_stt_provider("gladia", config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "Gladia STT (solaria-1)");
        assert!(!stt.is_ready()); // Not connected yet
    }

    #[test]
    fn test_create_stt_provider_gladia_empty_api_key() {
        let config = STTConfig {
            provider: "gladia".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm".to_string(),
            model: "solaria-1".to_string(),
        };

        let result = create_stt_provider("gladia", config);
        assert!(result.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_create_stt_provider_from_enum_gladia() {
        let config = STTConfig {
            provider: "gladia".to_string(),
            api_key: "test_api_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm".to_string(),
            model: "solaria-1".to_string(),
        };

        let result = create_stt_provider_from_enum(STTProvider::Gladia, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_message_includes_gladia() {
        let result = "invalid".parse::<STTProvider>();
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("gladia"));
        }
    }
}

/// Example usage of the STT trait abstraction
///
/// This demonstrates how to create a custom STT provider implementation
/// and use it with the unified interface.
///
/// ```rust,no_run
/// use waav_gateway::core::stt::{BaseSTT, STTConfig, STTResult, STTResultCallback, create_stt_provider};
/// use std::sync::Arc;
/// use std::pin::Pin;
/// use std::future::Future;
///
/// // Usage example:
/// async fn example_usage() {
///     // Configure the provider
///     let config = STTConfig {
///         model: "nova-3".to_string(),
///         provider: "deepgram".to_string(),
///         api_key: "your-api-key".to_string(),
///         language: "en-US".to_string(),
///         sample_rate: 16000,
///         channels: 1,
///         punctuation: true,
///         encoding: "linear16".to_string(),
///     };
///
///     // Create provider using factory function
///     let mut stt_provider = create_stt_provider("deepgram", config).unwrap();
///
///     // Register a callback for results
///     let callback = Arc::new(|result: STTResult| {
///         Box::pin(async move {
///             println!("Transcription: {}", result.transcript);
///             println!("Final: {}, Confidence: {:.2}", result.is_final, result.confidence);
///         }) as Pin<Box<dyn Future<Output = ()> + Send>>
///     });
///
///     stt_provider.on_result(callback).await.unwrap();
///
///     // Send audio data
///     let audio_data = vec![0u8; 1024]; // Your audio bytes here
///     stt_provider.send_audio(audio_data.into()).await.unwrap();
///
///     // Disconnect when done
///     stt_provider.disconnect().await.unwrap();
/// }
/// ```
#[cfg(doc)]
pub mod example {
    use super::*;

    /// Example implementation showing how to create a custom STT provider
    pub struct ExampleSTTProvider {
        // Implementation details would go here
    }

    /// Factory function to create STT providers
    ///
    /// # Deprecated
    /// This is an example placeholder. Use `create_stt_provider_from_enum` with a specific
    /// provider type instead of this function.
    ///
    /// # Panics
    /// This function always panics as it is not meant to be called directly.
    /// It exists only for documentation purposes.
    #[deprecated(
        since = "0.1.0",
        note = "Use create_stt_provider_from_enum() with a specific provider type instead"
    )]
    pub fn create_stt_provider() -> Box<dyn BaseSTT> {
        // This is a documentation example only (#[cfg(doc)] module).
        // In production, use the new API pattern with trait method and config:
        // let config = STTConfig { ... };
        // let stt = <DeepgramSTT as BaseSTT>::new(config).await.unwrap();
        // Box::new(stt)
        unreachable!(
            "create_stt_provider() is not implemented. \
             Use create_stt_provider_from_enum() with a specific provider type."
        )
    }
}

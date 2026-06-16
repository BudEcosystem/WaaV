//! Optimized Provider Dispatch
//!
//! This module provides optimized O(1) lookup for built-in providers using
//! PHF (Perfect Hash Function) static maps. For runtime-registered providers,
//! it falls back to DashMap lookup.
//!
//! # Performance Characteristics
//!
//! - Built-in provider lookup: O(1) guaranteed (PHF compile-time hash)
//! - Alias resolution: O(1) guaranteed (PHF)
//! - Case-insensitive lookup: Stack-allocated SmallString avoids heap allocs
//! - Runtime provider lookup: O(1) amortized (DashMap)
//!
//! # Architecture
//!
//! ```text
//! Provider Name → SmallString (stack-alloc lowercase) → PHF Map → Canonical Name
//!                                                           ↓
//!                                                      DashMap → Factory → Provider
//! ```
//!
//! # Design Notes: enum_dispatch
//!
//! The `enum_dispatch` crate was evaluated for optimizing trait object dispatch
//! (avoiding vtable lookups). However, it was not implemented because:
//!
//! 1. **async_trait compatibility**: The `BaseSTT`, `BaseTTS`, and `BaseRealtime`
//!    traits use `async_trait`, which wraps async methods in `Pin<Box<dyn Future>>`.
//!    This inherent boxing reduces the benefit of avoiding vtable lookups.
//!
//! 2. **API stability**: Factory functions return `Box<dyn BaseSTT>` which is
//!    required for runtime polymorphism and backward compatibility.
//!
//! 3. **Hot path analysis**: The provider lookup (optimized by PHF) happens once
//!    per connection. The actual hot path is audio processing (send_audio),
//!    where the async_trait overhead dominates.
//!
//! 4. **Complexity vs benefit**: enum_dispatch would require significant refactoring
//!    for marginal gains given points 1-3.
//!
//! The enum types (`BuiltinSTTProvider`, etc.) are retained for potential future
//! optimization opportunities, such as match-based dispatch for synchronous helpers.

use phf::phf_map;

/// Provider type for dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    STT,
    TTS,
    Realtime,
}

/// Built-in STT provider indices for fast dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BuiltinSTTProvider {
    Deepgram = 0,
    Google = 1,
    ElevenLabs = 2,
    Azure = 3,
    Cartesia = 4,
    OpenAI = 5,
    AssemblyAI = 6,
    AwsTranscribe = 7,
    IbmWatson = 8,
    Groq = 9,
    Gnani = 10,
    Sarvam = 11,
    Speechmatics = 12,
    Gladia = 13,
    RevAI = 14,
    Phonexia = 15,
    Reverie = 16,
    Yandex = 17,
    Tinkoff = 18,
    SberDevices = 19,
    Bhashini = 20,
    IFlytek = 21,
    AlibabaCloud = 22,
    Baidu = 23,
    Tencent = 24,
}

impl BuiltinSTTProvider {
    /// Get the canonical name for this provider
    #[inline]
    pub const fn canonical_name(&self) -> &'static str {
        match self {
            Self::Deepgram => "deepgram",
            Self::Google => "google",
            Self::ElevenLabs => "elevenlabs",
            Self::Azure => "microsoft-azure",
            Self::Cartesia => "cartesia",
            Self::OpenAI => "openai",
            Self::AssemblyAI => "assemblyai",
            Self::AwsTranscribe => "aws-transcribe",
            Self::IbmWatson => "ibm-watson",
            Self::Groq => "groq",
            Self::Gnani => "gnani",
            Self::Sarvam => "sarvam",
            Self::Speechmatics => "speechmatics",
            Self::Gladia => "gladia",
            Self::RevAI => "revai",
            Self::Phonexia => "phonexia",
            Self::Reverie => "reverie",
            Self::Yandex => "yandex",
            Self::Tinkoff => "tinkoff",
            Self::SberDevices => "sberdevices",
            Self::Bhashini => "bhashini",
            Self::IFlytek => "iflytek",
            Self::AlibabaCloud => "alibaba-cloud",
            Self::Baidu => "baidu",
            Self::Tencent => "tencent",
        }
    }
}

/// Built-in TTS provider indices for fast dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BuiltinTTSProvider {
    Deepgram = 0,
    ElevenLabs = 1,
    Google = 2,
    Azure = 3,
    Cartesia = 4,
    OpenAI = 5,
    AwsPolly = 6,
    IbmWatson = 7,
    Hume = 8,
    Lmnt = 9,
    PlayHt = 10,
    Gnani = 11,
    Murf = 12,
    WellSaid = 13,
    Resemble = 14,
    Speechify = 15,
    Smallest = 16,
    UnrealSpeech = 17,
    Acapela = 18,
    Cereproc = 19,
    Speechmatics = 20,
    AlibabaCloud = 21,
    Baidu = 22,
    Bhashini = 23,
    IFlytek = 24,
    Reverie = 25,
    SberDevices = 26,
    Tinkoff = 27,
    Yandex = 28,
    Tencent = 29,
}

impl BuiltinTTSProvider {
    /// Get the canonical name for this provider
    #[inline]
    pub const fn canonical_name(&self) -> &'static str {
        match self {
            Self::Deepgram => "deepgram",
            Self::ElevenLabs => "elevenlabs",
            Self::Google => "google",
            Self::Azure => "microsoft-azure",
            Self::Cartesia => "cartesia",
            Self::OpenAI => "openai",
            Self::AwsPolly => "aws-polly",
            Self::IbmWatson => "ibm-watson",
            Self::Hume => "hume",
            Self::Lmnt => "lmnt",
            Self::PlayHt => "playht",
            Self::Gnani => "gnani",
            Self::Murf => "murf",
            Self::WellSaid => "wellsaid",
            Self::Resemble => "resemble",
            Self::Speechify => "speechify",
            Self::Smallest => "smallest",
            Self::UnrealSpeech => "unrealspeech",
            Self::Acapela => "acapela",
            Self::Cereproc => "cereproc",
            Self::Speechmatics => "speechmatics",
            Self::AlibabaCloud => "alibaba-cloud",
            Self::Baidu => "baidu",
            Self::Bhashini => "bhashini",
            Self::IFlytek => "iflytek",
            Self::Reverie => "reverie",
            Self::SberDevices => "sberdevices",
            Self::Tinkoff => "tinkoff",
            Self::Yandex => "yandex",
            Self::Tencent => "tencent",
        }
    }
}

/// Built-in Realtime provider indices for fast dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BuiltinRealtimeProvider {
    OpenAI = 0,
    Hume = 1,
    /// Azure OpenAI Realtime — OpenAI-protocol clone (GA wire, `api-key` header).
    Azure = 2,
    /// Grok / xAI Realtime — OpenAI-protocol clone (GA-compatible wire).
    Grok = 3,
    /// Inworld Realtime — OpenAI-protocol clone (GA wire).
    Inworld = 4,
    /// Deepgram Voice Agent — speech-to-speech (raw linear16 binary frames; its
    /// own protocol, NOT an OpenAI clone).
    Deepgram = 5,
    /// ElevenLabs Conversational AI — speech-to-speech (base64+JSON; its own
    /// protocol, the OpenAI-family wire shape, NOT an OpenAI clone).
    ElevenLabs = 6,
}

impl BuiltinRealtimeProvider {
    /// Get the canonical name for this provider
    #[inline]
    pub const fn canonical_name(&self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Hume => "hume",
            Self::Azure => "azure",
            Self::Grok => "grok",
            Self::Inworld => "inworld",
            Self::Deepgram => "deepgram",
            Self::ElevenLabs => "elevenlabs",
        }
    }
}

// =============================================================================
// PHF Static Maps for O(1) Provider Lookup
// =============================================================================

/// PHF map for STT provider name resolution (including aliases)
/// Maps provider name/alias → BuiltinSTTProvider
pub static STT_PROVIDER_MAP: phf::Map<&'static str, BuiltinSTTProvider> = phf_map! {
    // Primary names
    "deepgram" => BuiltinSTTProvider::Deepgram,
    "google" => BuiltinSTTProvider::Google,
    "elevenlabs" => BuiltinSTTProvider::ElevenLabs,
    "microsoft-azure" => BuiltinSTTProvider::Azure,
    "cartesia" => BuiltinSTTProvider::Cartesia,
    "openai" => BuiltinSTTProvider::OpenAI,
    "assemblyai" => BuiltinSTTProvider::AssemblyAI,
    "aws-transcribe" => BuiltinSTTProvider::AwsTranscribe,
    "ibm-watson" => BuiltinSTTProvider::IbmWatson,
    "groq" => BuiltinSTTProvider::Groq,
    "gnani" => BuiltinSTTProvider::Gnani,
    "sarvam" => BuiltinSTTProvider::Sarvam,
    "speechmatics" => BuiltinSTTProvider::Speechmatics,
    "gladia" => BuiltinSTTProvider::Gladia,
    "revai" => BuiltinSTTProvider::RevAI,
    "phonexia" => BuiltinSTTProvider::Phonexia,
    "reverie" => BuiltinSTTProvider::Reverie,
    "yandex" => BuiltinSTTProvider::Yandex,
    "tinkoff" => BuiltinSTTProvider::Tinkoff,
    "sberdevices" => BuiltinSTTProvider::SberDevices,
    "bhashini" => BuiltinSTTProvider::Bhashini,
    "iflytek" => BuiltinSTTProvider::IFlytek,
    "alibaba-cloud" => BuiltinSTTProvider::AlibabaCloud,
    "baidu" => BuiltinSTTProvider::Baidu,
    "tencent" => BuiltinSTTProvider::Tencent,
    // Aliases
    "azure" => BuiltinSTTProvider::Azure,
    "aws_transcribe" => BuiltinSTTProvider::AwsTranscribe,
    "amazon-transcribe" => BuiltinSTTProvider::AwsTranscribe,
    "transcribe" => BuiltinSTTProvider::AwsTranscribe,
    "ibm_watson" => BuiltinSTTProvider::IbmWatson,
    "watson" => BuiltinSTTProvider::IbmWatson,
    "ibm" => BuiltinSTTProvider::IbmWatson,
    "gnani-ai" => BuiltinSTTProvider::Gnani,
    "gnani.ai" => BuiltinSTTProvider::Gnani,
    "vachana" => BuiltinSTTProvider::Gnani,
    "sarvam-ai" => BuiltinSTTProvider::Sarvam,
    "sarvam.ai" => BuiltinSTTProvider::Sarvam,
    "saarika" => BuiltinSTTProvider::Sarvam,
    // Speechmatics aliases
    "speechmatics-stt" => BuiltinSTTProvider::Speechmatics,
    // Gladia aliases
    "gladia-stt" => BuiltinSTTProvider::Gladia,
    // Rev AI aliases
    "rev-ai" => BuiltinSTTProvider::RevAI,
    "rev_ai" => BuiltinSTTProvider::RevAI,
    "rev.ai" => BuiltinSTTProvider::RevAI,
    // Phonexia aliases
    "phonexia-stt" => BuiltinSTTProvider::Phonexia,
    // Reverie aliases
    "reverie-stt" => BuiltinSTTProvider::Reverie,
    // Yandex aliases
    "yandex-stt" => BuiltinSTTProvider::Yandex,
    "yandex_stt" => BuiltinSTTProvider::Yandex,
    "speechkit" => BuiltinSTTProvider::Yandex,
    // Tinkoff aliases
    "tinkoff-stt" => BuiltinSTTProvider::Tinkoff,
    "tinkoff_stt" => BuiltinSTTProvider::Tinkoff,
    "voicekit" => BuiltinSTTProvider::Tinkoff,
    // SberDevices aliases
    "sber" => BuiltinSTTProvider::SberDevices,
    "sber-devices" => BuiltinSTTProvider::SberDevices,
    "sber_devices" => BuiltinSTTProvider::SberDevices,
    "salutespeech" => BuiltinSTTProvider::SberDevices,
    // Bhashini aliases
    "ulca" => BuiltinSTTProvider::Bhashini,
    "ai4bharat" => BuiltinSTTProvider::Bhashini,
    "meity" => BuiltinSTTProvider::Bhashini,
    // iFlytek aliases
    "ifly" => BuiltinSTTProvider::IFlytek,
    "xfyun" => BuiltinSTTProvider::IFlytek,
    "xunfei" => BuiltinSTTProvider::IFlytek,
    // Alibaba Cloud aliases
    "alibaba_cloud" => BuiltinSTTProvider::AlibabaCloud,
    "alibabacloud" => BuiltinSTTProvider::AlibabaCloud,
    "alibaba" => BuiltinSTTProvider::AlibabaCloud,
    "dashscope" => BuiltinSTTProvider::AlibabaCloud,
    "aliyun" => BuiltinSTTProvider::AlibabaCloud,
    // Baidu aliases
    "baidu-ai" => BuiltinSTTProvider::Baidu,
    "baidu_ai" => BuiltinSTTProvider::Baidu,
    "baiduai" => BuiltinSTTProvider::Baidu,
    "baidu-speech" => BuiltinSTTProvider::Baidu,
    // Tencent aliases
    "tencent-cloud" => BuiltinSTTProvider::Tencent,
    "tencent_cloud" => BuiltinSTTProvider::Tencent,
    "tencentcloud" => BuiltinSTTProvider::Tencent,
};

/// PHF map for TTS provider name resolution (including aliases)
pub static TTS_PROVIDER_MAP: phf::Map<&'static str, BuiltinTTSProvider> = phf_map! {
    // Primary names
    "deepgram" => BuiltinTTSProvider::Deepgram,
    "elevenlabs" => BuiltinTTSProvider::ElevenLabs,
    "google" => BuiltinTTSProvider::Google,
    "microsoft-azure" => BuiltinTTSProvider::Azure,
    "cartesia" => BuiltinTTSProvider::Cartesia,
    "openai" => BuiltinTTSProvider::OpenAI,
    "aws-polly" => BuiltinTTSProvider::AwsPolly,
    "ibm-watson" => BuiltinTTSProvider::IbmWatson,
    "hume" => BuiltinTTSProvider::Hume,
    "lmnt" => BuiltinTTSProvider::Lmnt,
    "playht" => BuiltinTTSProvider::PlayHt,
    "gnani" => BuiltinTTSProvider::Gnani,
    "murf" => BuiltinTTSProvider::Murf,
    "wellsaid" => BuiltinTTSProvider::WellSaid,
    "resemble" => BuiltinTTSProvider::Resemble,
    "speechify" => BuiltinTTSProvider::Speechify,
    "smallest" => BuiltinTTSProvider::Smallest,
    "unrealspeech" => BuiltinTTSProvider::UnrealSpeech,
    "acapela" => BuiltinTTSProvider::Acapela,
    "cereproc" => BuiltinTTSProvider::Cereproc,
    "speechmatics" => BuiltinTTSProvider::Speechmatics,
    "alibaba-cloud" => BuiltinTTSProvider::AlibabaCloud,
    "baidu" => BuiltinTTSProvider::Baidu,
    "bhashini" => BuiltinTTSProvider::Bhashini,
    "iflytek" => BuiltinTTSProvider::IFlytek,
    "reverie" => BuiltinTTSProvider::Reverie,
    "sberdevices" => BuiltinTTSProvider::SberDevices,
    "tinkoff" => BuiltinTTSProvider::Tinkoff,
    "yandex" => BuiltinTTSProvider::Yandex,
    "tencent" => BuiltinTTSProvider::Tencent,
    // Aliases
    "azure" => BuiltinTTSProvider::Azure,
    "aws_polly" => BuiltinTTSProvider::AwsPolly,
    "amazon-polly" => BuiltinTTSProvider::AwsPolly,
    "polly" => BuiltinTTSProvider::AwsPolly,
    "ibm_watson" => BuiltinTTSProvider::IbmWatson,
    "watson" => BuiltinTTSProvider::IbmWatson,
    "ibm" => BuiltinTTSProvider::IbmWatson,
    "hume-ai" => BuiltinTTSProvider::Hume,
    "hume_ai" => BuiltinTTSProvider::Hume,
    "lmnt-ai" => BuiltinTTSProvider::Lmnt,
    "lmnt_ai" => BuiltinTTSProvider::Lmnt,
    "play-ht" => BuiltinTTSProvider::PlayHt,
    "play_ht" => BuiltinTTSProvider::PlayHt,
    "play.ht" => BuiltinTTSProvider::PlayHt,
    "gnani-ai" => BuiltinTTSProvider::Gnani,
    "gnani.ai" => BuiltinTTSProvider::Gnani,
    "murf-ai" => BuiltinTTSProvider::Murf,
    "murf_ai" => BuiltinTTSProvider::Murf,
    "murf.ai" => BuiltinTTSProvider::Murf,
    "wellsaid-labs" => BuiltinTTSProvider::WellSaid,
    "wellsaidlabs" => BuiltinTTSProvider::WellSaid,
    "wellsaid_labs" => BuiltinTTSProvider::WellSaid,
    "well-said" => BuiltinTTSProvider::WellSaid,
    "resemble-ai" => BuiltinTTSProvider::Resemble,
    "resemble_ai" => BuiltinTTSProvider::Resemble,
    "resembleai" => BuiltinTTSProvider::Resemble,
    "resemble.ai" => BuiltinTTSProvider::Resemble,
    "speechify-ai" => BuiltinTTSProvider::Speechify,
    "speechify_ai" => BuiltinTTSProvider::Speechify,
    "speechify.com" => BuiltinTTSProvider::Speechify,
    // Smallest aliases
    "smallest-ai" => BuiltinTTSProvider::Smallest,
    "smallest.ai" => BuiltinTTSProvider::Smallest,
    // UnrealSpeech aliases
    "unreal-speech" => BuiltinTTSProvider::UnrealSpeech,
    "unreal_speech" => BuiltinTTSProvider::UnrealSpeech,
    // Acapela aliases
    "acapela-tts" => BuiltinTTSProvider::Acapela,
    // Cereproc aliases
    "cereproc-tts" => BuiltinTTSProvider::Cereproc,
    "cerevoice" => BuiltinTTSProvider::Cereproc,
    // Speechmatics TTS aliases
    "speechmatics-tts" => BuiltinTTSProvider::Speechmatics,
    // Alibaba Cloud TTS aliases
    "alibaba_cloud" => BuiltinTTSProvider::AlibabaCloud,
    "alibabacloud" => BuiltinTTSProvider::AlibabaCloud,
    "alibaba" => BuiltinTTSProvider::AlibabaCloud,
    "dashscope" => BuiltinTTSProvider::AlibabaCloud,
    "aliyun" => BuiltinTTSProvider::AlibabaCloud,
    // Baidu TTS aliases
    "baidu-ai" => BuiltinTTSProvider::Baidu,
    "baidu_ai" => BuiltinTTSProvider::Baidu,
    "baiduai" => BuiltinTTSProvider::Baidu,
    // Tencent TTS aliases
    "tencent-cloud" => BuiltinTTSProvider::Tencent,
    "tencent_cloud" => BuiltinTTSProvider::Tencent,
    "tencentcloud" => BuiltinTTSProvider::Tencent,
    "tencent-tts" => BuiltinTTSProvider::Tencent,
    "tencent_tts" => BuiltinTTSProvider::Tencent,
    // Bhashini TTS aliases
    "ulca" => BuiltinTTSProvider::Bhashini,
    "ai4bharat" => BuiltinTTSProvider::Bhashini,
    // iFlytek TTS aliases
    "ifly" => BuiltinTTSProvider::IFlytek,
    "xfyun" => BuiltinTTSProvider::IFlytek,
    "xunfei" => BuiltinTTSProvider::IFlytek,
    // Reverie TTS aliases
    "reverie-tts" => BuiltinTTSProvider::Reverie,
    // SberDevices TTS aliases
    "sber" => BuiltinTTSProvider::SberDevices,
    "sber-devices" => BuiltinTTSProvider::SberDevices,
    "sber_devices" => BuiltinTTSProvider::SberDevices,
    // Tinkoff TTS aliases
    "tinkoff-tts" => BuiltinTTSProvider::Tinkoff,
    "tinkoff_tts" => BuiltinTTSProvider::Tinkoff,
    // Yandex TTS aliases
    "yandex-tts" => BuiltinTTSProvider::Yandex,
    "yandex_tts" => BuiltinTTSProvider::Yandex,
};

/// PHF map for Realtime provider name resolution (including aliases)
pub static REALTIME_PROVIDER_MAP: phf::Map<&'static str, BuiltinRealtimeProvider> = phf_map! {
    // Primary names
    "openai" => BuiltinRealtimeProvider::OpenAI,
    "hume" => BuiltinRealtimeProvider::Hume,
    "azure" => BuiltinRealtimeProvider::Azure,
    "grok" => BuiltinRealtimeProvider::Grok,
    "inworld" => BuiltinRealtimeProvider::Inworld,
    "deepgram" => BuiltinRealtimeProvider::Deepgram,
    // Aliases
    "hume_evi" => BuiltinRealtimeProvider::Hume,
    "hume-evi" => BuiltinRealtimeProvider::Hume,
    "evi" => BuiltinRealtimeProvider::Hume,
    // Azure OpenAI Realtime aliases.
    "azure-openai" => BuiltinRealtimeProvider::Azure,
    "azure_openai" => BuiltinRealtimeProvider::Azure,
    // xAI alias.
    "xai" => BuiltinRealtimeProvider::Grok,
    // Deepgram Voice Agent aliases.
    "deepgram-agent" => BuiltinRealtimeProvider::Deepgram,
    "deepgram_voice_agent" => BuiltinRealtimeProvider::Deepgram,
    // ElevenLabs Conversational AI.
    "elevenlabs" => BuiltinRealtimeProvider::ElevenLabs,
    "elevenlabs-convai" => BuiltinRealtimeProvider::ElevenLabs,
    "11labs" => BuiltinRealtimeProvider::ElevenLabs,
};

// =============================================================================
// Fast Lookup Functions
// =============================================================================

/// Resolve an STT provider name to its builtin enum (O(1) PHF lookup)
///
/// Returns None if the provider is not a built-in provider.
#[inline]
pub fn resolve_stt_provider(name: &str) -> Option<BuiltinSTTProvider> {
    // PHF maps are case-sensitive, so we need to lowercase first
    // For maximum performance, we use a stack-allocated buffer for short names
    let lowercase = to_lowercase_fast(name);
    STT_PROVIDER_MAP.get(lowercase.as_str()).copied()
}

/// Resolve a TTS provider name to its builtin enum (O(1) PHF lookup)
#[inline]
pub fn resolve_tts_provider(name: &str) -> Option<BuiltinTTSProvider> {
    let lowercase = to_lowercase_fast(name);
    TTS_PROVIDER_MAP.get(lowercase.as_str()).copied()
}

/// Resolve a Realtime provider name to its builtin enum (O(1) PHF lookup)
#[inline]
pub fn resolve_realtime_provider(name: &str) -> Option<BuiltinRealtimeProvider> {
    let lowercase = to_lowercase_fast(name);
    REALTIME_PROVIDER_MAP.get(lowercase.as_str()).copied()
}

/// Check if a provider name is a built-in STT provider
#[inline]
pub fn is_builtin_stt(name: &str) -> bool {
    resolve_stt_provider(name).is_some()
}

/// Check if a provider name is a built-in TTS provider
#[inline]
pub fn is_builtin_tts(name: &str) -> bool {
    resolve_tts_provider(name).is_some()
}

/// Check if a provider name is a built-in Realtime provider
#[inline]
pub fn is_builtin_realtime(name: &str) -> bool {
    resolve_realtime_provider(name).is_some()
}

/// Fast lowercase conversion using stack allocation for short strings
///
/// Most provider names are short (< 32 chars), so we can avoid heap allocation
/// by using a stack buffer. This is a significant optimization for hot paths.
#[inline]
fn to_lowercase_fast(s: &str) -> SmallString {
    SmallString::from_lowercase(s)
}

/// Stack-allocated small string for avoiding heap allocation on short provider names
///
/// Uses 32 bytes on stack, which covers all current provider names.
/// Falls back to heap allocation for longer strings (rare case).
pub struct SmallString {
    // Inline buffer for short strings (covers "microsoft-azure" and all aliases)
    inline: [u8; 32],
    len: u8,
    // Heap fallback for longer strings
    heap: Option<String>,
}

impl SmallString {
    /// Create a lowercase SmallString from a string slice
    #[inline]
    pub fn from_lowercase(s: &str) -> Self {
        let bytes = s.as_bytes();
        if bytes.len() <= 32 {
            let mut inline = [0u8; 32];
            for (i, &b) in bytes.iter().enumerate() {
                inline[i] = b.to_ascii_lowercase();
            }
            Self {
                inline,
                len: bytes.len() as u8,
                heap: None,
            }
        } else {
            Self {
                inline: [0u8; 32],
                len: 0,
                heap: Some(s.to_lowercase()),
            }
        }
    }

    /// Get the string slice
    #[inline]
    pub fn as_str(&self) -> &str {
        if let Some(ref heap) = self.heap {
            heap.as_str()
        } else {
            // SAFETY: We only store valid UTF-8 lowercase ASCII
            unsafe { std::str::from_utf8_unchecked(&self.inline[..self.len as usize]) }
        }
    }
}

// =============================================================================
// Provider Count Constants
// =============================================================================

/// Number of built-in STT providers
pub const BUILTIN_STT_COUNT: usize = 25;

/// Number of built-in TTS providers
pub const BUILTIN_TTS_COUNT: usize = 29;

/// Number of built-in Realtime providers
pub const BUILTIN_REALTIME_COUNT: usize = 7;

/// Total number of built-in providers
pub const TOTAL_BUILTIN_PROVIDERS: usize =
    BUILTIN_STT_COUNT + BUILTIN_TTS_COUNT + BUILTIN_REALTIME_COUNT;

// =============================================================================
// Provider Lists (for iteration)
// =============================================================================

/// All built-in STT provider names (canonical only, no aliases)
pub const BUILTIN_STT_NAMES: [&str; BUILTIN_STT_COUNT] = [
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
    "gnani",
    "sarvam",
    "speechmatics",
    "gladia",
    "revai",
    "phonexia",
    "reverie",
    "yandex",
    "tinkoff",
    "sberdevices",
    "bhashini",
    "iflytek",
    "alibaba-cloud",
    "baidu",
    "tencent",
];

/// All built-in TTS provider names (canonical only, no aliases)
pub const BUILTIN_TTS_NAMES: [&str; BUILTIN_TTS_COUNT] = [
    "deepgram",
    "elevenlabs",
    "google",
    "microsoft-azure",
    "cartesia",
    "openai",
    "aws-polly",
    "ibm-watson",
    "hume",
    "lmnt",
    "playht",
    "gnani",
    "murf",
    "wellsaid",
    "resemble",
    "speechify",
    "smallest",
    "unrealspeech",
    "acapela",
    "cereproc",
    "speechmatics",
    "alibaba-cloud",
    "baidu",
    "bhashini",
    "iflytek",
    "reverie",
    "sberdevices",
    "tinkoff",
    "yandex",
];

/// All built-in Realtime provider names (canonical only, no aliases)
pub const BUILTIN_REALTIME_NAMES: [&str; BUILTIN_REALTIME_COUNT] =
    ["openai", "hume", "azure", "grok", "inworld", "deepgram", "elevenlabs"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stt_provider_lookup() {
        // Test primary names
        assert_eq!(
            resolve_stt_provider("deepgram"),
            Some(BuiltinSTTProvider::Deepgram)
        );
        assert_eq!(
            resolve_stt_provider("google"),
            Some(BuiltinSTTProvider::Google)
        );
        assert_eq!(
            resolve_stt_provider("microsoft-azure"),
            Some(BuiltinSTTProvider::Azure)
        );

        // Test aliases
        assert_eq!(
            resolve_stt_provider("azure"),
            Some(BuiltinSTTProvider::Azure)
        );
        assert_eq!(
            resolve_stt_provider("watson"),
            Some(BuiltinSTTProvider::IbmWatson)
        );
        assert_eq!(
            resolve_stt_provider("transcribe"),
            Some(BuiltinSTTProvider::AwsTranscribe)
        );

        // Test case insensitivity
        assert_eq!(
            resolve_stt_provider("DEEPGRAM"),
            Some(BuiltinSTTProvider::Deepgram)
        );
        assert_eq!(
            resolve_stt_provider("DeepGram"),
            Some(BuiltinSTTProvider::Deepgram)
        );

        // Test unknown provider
        assert_eq!(resolve_stt_provider("unknown"), None);
    }

    #[test]
    fn test_tts_provider_lookup() {
        // Test primary names
        assert_eq!(
            resolve_tts_provider("deepgram"),
            Some(BuiltinTTSProvider::Deepgram)
        );
        assert_eq!(
            resolve_tts_provider("elevenlabs"),
            Some(BuiltinTTSProvider::ElevenLabs)
        );

        // Test aliases
        assert_eq!(
            resolve_tts_provider("polly"),
            Some(BuiltinTTSProvider::AwsPolly)
        );
        assert_eq!(
            resolve_tts_provider("play.ht"),
            Some(BuiltinTTSProvider::PlayHt)
        );

        // Test unknown
        assert_eq!(resolve_tts_provider("unknown"), None);
    }

    #[test]
    fn test_realtime_provider_lookup() {
        assert_eq!(
            resolve_realtime_provider("openai"),
            Some(BuiltinRealtimeProvider::OpenAI)
        );
        assert_eq!(
            resolve_realtime_provider("hume"),
            Some(BuiltinRealtimeProvider::Hume)
        );
        assert_eq!(
            resolve_realtime_provider("evi"),
            Some(BuiltinRealtimeProvider::Hume)
        );
        // Deepgram Voice Agent + aliases (case-insensitive).
        assert_eq!(
            resolve_realtime_provider("deepgram"),
            Some(BuiltinRealtimeProvider::Deepgram)
        );
        assert_eq!(
            resolve_realtime_provider("DEEPGRAM"),
            Some(BuiltinRealtimeProvider::Deepgram)
        );
        assert_eq!(
            resolve_realtime_provider("deepgram-agent"),
            Some(BuiltinRealtimeProvider::Deepgram)
        );
        assert_eq!(
            resolve_realtime_provider("deepgram_voice_agent"),
            Some(BuiltinRealtimeProvider::Deepgram)
        );
        // ElevenLabs Conversational AI + aliases (case-insensitive).
        assert_eq!(
            resolve_realtime_provider("elevenlabs"),
            Some(BuiltinRealtimeProvider::ElevenLabs)
        );
        assert_eq!(
            resolve_realtime_provider("ELEVENLABS"),
            Some(BuiltinRealtimeProvider::ElevenLabs)
        );
        assert_eq!(
            resolve_realtime_provider("elevenlabs-convai"),
            Some(BuiltinRealtimeProvider::ElevenLabs)
        );
        assert_eq!(
            resolve_realtime_provider("11labs"),
            Some(BuiltinRealtimeProvider::ElevenLabs)
        );
        assert_eq!(resolve_realtime_provider("unknown"), None);
    }

    #[test]
    fn test_is_builtin() {
        assert!(is_builtin_stt("deepgram"));
        assert!(is_builtin_stt("azure"));
        assert!(!is_builtin_stt("custom-provider"));

        assert!(is_builtin_tts("elevenlabs"));
        assert!(!is_builtin_tts("custom-tts"));

        assert!(is_builtin_realtime("openai"));
        assert!(!is_builtin_realtime("custom-realtime"));
    }

    #[test]
    fn test_small_string() {
        let s = SmallString::from_lowercase("DeepGram");
        assert_eq!(s.as_str(), "deepgram");

        let s = SmallString::from_lowercase("MICROSOFT-AZURE");
        assert_eq!(s.as_str(), "microsoft-azure");

        // Test long string (heap fallback)
        let long = "a".repeat(50);
        let s = SmallString::from_lowercase(&long);
        assert_eq!(s.as_str(), long.to_lowercase());
    }

    #[test]
    fn test_canonical_names() {
        assert_eq!(BuiltinSTTProvider::Deepgram.canonical_name(), "deepgram");
        assert_eq!(
            BuiltinSTTProvider::Azure.canonical_name(),
            "microsoft-azure"
        );
        assert_eq!(BuiltinTTSProvider::AwsPolly.canonical_name(), "aws-polly");
        assert_eq!(BuiltinRealtimeProvider::Hume.canonical_name(), "hume");
    }

    #[test]
    fn test_provider_counts() {
        assert_eq!(BUILTIN_STT_NAMES.len(), BUILTIN_STT_COUNT);
        assert_eq!(BUILTIN_TTS_NAMES.len(), BUILTIN_TTS_COUNT);
        assert_eq!(BUILTIN_REALTIME_NAMES.len(), BUILTIN_REALTIME_COUNT);
    }
}

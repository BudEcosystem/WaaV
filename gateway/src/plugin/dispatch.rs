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
        }
    }
}

/// Built-in Realtime provider indices for fast dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BuiltinRealtimeProvider {
    OpenAI = 0,
    Hume = 1,
}

impl BuiltinRealtimeProvider {
    /// Get the canonical name for this provider
    #[inline]
    pub const fn canonical_name(&self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Hume => "hume",
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
};

/// PHF map for Realtime provider name resolution (including aliases)
pub static REALTIME_PROVIDER_MAP: phf::Map<&'static str, BuiltinRealtimeProvider> = phf_map! {
    // Primary names
    "openai" => BuiltinRealtimeProvider::OpenAI,
    "hume" => BuiltinRealtimeProvider::Hume,
    // Aliases
    "hume_evi" => BuiltinRealtimeProvider::Hume,
    "hume-evi" => BuiltinRealtimeProvider::Hume,
    "evi" => BuiltinRealtimeProvider::Hume,
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
pub const BUILTIN_STT_COUNT: usize = 11;

/// Number of built-in TTS providers
pub const BUILTIN_TTS_COUNT: usize = 12;

/// Number of built-in Realtime providers
pub const BUILTIN_REALTIME_COUNT: usize = 2;

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
];

/// All built-in Realtime provider names (canonical only, no aliases)
pub const BUILTIN_REALTIME_NAMES: [&str; BUILTIN_REALTIME_COUNT] = ["openai", "hume"];

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

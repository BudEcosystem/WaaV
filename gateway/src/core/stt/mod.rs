pub mod assemblyai;
pub mod aws_transcribe;
pub mod azure;
mod base;
pub mod cartesia;
pub mod deepgram;
pub mod elevenlabs;
pub mod gnani;
pub mod google;
pub mod groq;
pub mod ibm_watson;
pub mod openai;

// Re-export public types and traits
pub use base::{
    BaseSTT, STTConfig, STTConnectionState, STTError, STTErrorCallback, STTFactory, STTHelper,
    STTResult, STTResultCallback, STTStats,
};

// Re-export Deepgram implementation
pub use deepgram::{DeepgramSTT, DeepgramSTTConfig};

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
    StreamingRecognitionResponse as GnaniStreamingResponse, TranscriptChunk as GnaniTranscriptChunk,
};

/// Supported STT providers
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum STTProvider {
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
}

impl std::fmt::Display for STTProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
        }
    }
}

impl std::str::FromStr for STTProvider {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
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
            _ => Err(STTError::ConfigurationError(format!(
                "Unsupported STT provider: {s}. Supported providers: deepgram, google, elevenlabs, microsoft-azure, cartesia, openai, assemblyai, aws-transcribe, ibm-watson, groq"
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
            ]
        );
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
    pub fn create_stt_provider() -> Box<dyn BaseSTT> {
        // This would return an actual provider implementation
        // Use the new API pattern with trait method and config:
        // let config = STTConfig { ... };
        // let stt = <DeepgramSTT as BaseSTT>::new(config).await.unwrap();
        // Box::new(stt)
        todo!("Implement actual STT provider")
    }
}

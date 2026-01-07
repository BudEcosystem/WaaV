pub mod aws_polly;
pub mod azure;
mod base;
pub mod cartesia;
pub mod deepgram;
pub mod elevenlabs;
pub mod google;
pub mod hume;
pub mod ibm_watson;
pub mod lmnt;
pub mod openai;
pub mod playht;
pub mod provider;

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
pub use deepgram::{DEEPGRAM_TTS_URL, DeepgramTTS};
pub use elevenlabs::{ELEVENLABS_TTS_URL, ElevenLabsTTS};
pub use google::{GOOGLE_TTS_URL, GoogleTTS};
pub use hume::{HUME_TTS_STREAM_URL, HumeTTS, HumeTTSConfig};
pub use ibm_watson::{
    IBM_WATSON_TTS_URL, IbmOutputFormat, IbmVoice, IbmWatsonTTS, IbmWatsonTTSConfig,
};
pub use lmnt::{LMNT_TTS_URL, LmntAudioFormat, LmntTts, LmntTtsConfig, LmntVoice};
pub use openai::{AudioOutputFormat, OPENAI_TTS_URL, OpenAITTS, OpenAITTSModel, OpenAIVoice};
pub use playht::{
    PLAYHT_TTS_URL, PlayHtAudioFormat, PlayHtModel, PlayHtTts, PlayHtTtsConfig, PlayHtVoice,
};
pub use provider::{TTSProvider, TTSRequestBuilder};
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
    match provider_type.to_lowercase().as_str() {
        "deepgram" => Ok(Box::new(DeepgramTTS::new(config)?)),
        "elevenlabs" => Ok(Box::new(ElevenLabsTTS::new(config)?)),
        "google" => Ok(Box::new(GoogleTTS::new(config)?)),
        "azure" | "microsoft-azure" => Ok(Box::new(AzureTTS::new(config)?)),
        "cartesia" => Ok(Box::new(CartesiaTTS::new(config)?)),
        "openai" => Ok(Box::new(OpenAITTS::new(config)?)),
        "aws-polly" | "aws_polly" | "amazon-polly" | "polly" => {
            Ok(Box::new(AwsPollyTTS::new(config)?))
        }
        "ibm-watson" | "ibm_watson" | "watson" | "ibm" => Ok(Box::new(IbmWatsonTTS::new(config)?)),
        "hume" | "hume-ai" | "hume_ai" => Ok(Box::new(HumeTTS::new(config)?)),
        "lmnt" | "lmnt-ai" | "lmnt_ai" => Ok(Box::new(LmntTts::new(config)?)),
        "playht" | "play-ht" | "play_ht" | "play.ht" => Ok(Box::new(PlayHtTts::new(config)?)),
        _ => Err(TTSError::InvalidConfiguration(format!(
            "Unsupported TTS provider: {provider_type}. Supported providers: deepgram, elevenlabs, google, azure, cartesia, openai, aws-polly, ibm-watson, hume, lmnt, playht"
        ))),
    }
}

/// Returns a map of provider names to their default API endpoint URLs.
///
/// Note: Azure uses regional endpoints. The URL returned here is for the
/// default region (eastus). For specific regions, use `AzureRegion::tts_rest_url()`.
/// Note: AWS Polly uses regional endpoints. The URL returned here is a template.
/// Note: IBM Watson uses regional endpoints. The URL returned here is for us-south.
pub fn get_tts_provider_urls() -> HashMap<String, String> {
    let mut urls = HashMap::new();
    urls.insert("deepgram".to_string(), DEEPGRAM_TTS_URL.to_string());
    urls.insert("elevenlabs".to_string(), ELEVENLABS_TTS_URL.to_string());
    urls.insert("google".to_string(), GOOGLE_TTS_URL.to_string());
    urls.insert("azure".to_string(), AZURE_TTS_URL.to_string());
    urls.insert("cartesia".to_string(), CARTESIA_TTS_URL.to_string());
    urls.insert("openai".to_string(), OPENAI_TTS_URL.to_string());
    urls.insert("aws-polly".to_string(), AWS_POLLY_TTS_URL.to_string());
    urls.insert("ibm-watson".to_string(), IBM_WATSON_TTS_URL.to_string());
    urls.insert("hume".to_string(), HUME_TTS_STREAM_URL.to_string());
    urls.insert("lmnt".to_string(), LMNT_TTS_URL.to_string());
    urls.insert("playht".to_string(), PLAYHT_TTS_URL.to_string());
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
}

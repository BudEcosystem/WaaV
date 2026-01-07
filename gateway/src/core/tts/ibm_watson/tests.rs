//! Tests for IBM Watson Text-to-Speech provider.
//!
//! This module contains comprehensive unit tests for the IBM Watson TTS
//! implementation, covering configuration, voice selection, audio formats,
//! and request building.

use super::config::*;
use super::provider::*;
use crate::core::stt::ibm_watson::IbmRegion;
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig};

// =============================================================================
// Voice Tests
// =============================================================================

mod voice_tests {
    use super::*;

    #[test]
    fn test_voice_as_str() {
        assert_eq!(
            IbmVoice::EnUsAllisonV3Voice.as_str(),
            "en-US_AllisonV3Voice"
        );
        assert_eq!(
            IbmVoice::EnGbCharlotteV3Voice.as_str(),
            "en-GB_CharlotteV3Voice"
        );
        assert_eq!(IbmVoice::DeDeBirgitV3Voice.as_str(), "de-DE_BirgitV3Voice");
        assert_eq!(IbmVoice::JaJpEmiV3Voice.as_str(), "ja-JP_EmiV3Voice");
    }

    #[test]
    fn test_voice_language_code() {
        assert_eq!(IbmVoice::EnUsAllisonV3Voice.language_code(), "en-US");
        assert_eq!(IbmVoice::EnGbCharlotteV3Voice.language_code(), "en-GB");
        assert_eq!(IbmVoice::EnAuCraigV3Voice.language_code(), "en-AU");
        assert_eq!(IbmVoice::DeDeBirgitV3Voice.language_code(), "de-DE");
        assert_eq!(IbmVoice::FrFrReneeV3Voice.language_code(), "fr-FR");
        assert_eq!(IbmVoice::JaJpEmiV3Voice.language_code(), "ja-JP");
        assert_eq!(IbmVoice::ZhCnLiNaV3Voice.language_code(), "zh-CN");
    }

    #[test]
    fn test_voice_from_str_full_name() {
        assert_eq!(
            IbmVoice::from_str_or_default("en-US_AllisonV3Voice"),
            IbmVoice::EnUsAllisonV3Voice
        );
        assert_eq!(
            IbmVoice::from_str_or_default("en-GB_KateV3Voice"),
            IbmVoice::EnGbKateV3Voice
        );
        assert_eq!(
            IbmVoice::from_str_or_default("de-DE_DieterV3Voice"),
            IbmVoice::DeDeDieterV3Voice
        );
    }

    #[test]
    fn test_voice_from_str_short_name() {
        assert_eq!(
            IbmVoice::from_str_or_default("allison"),
            IbmVoice::EnUsAllisonV3Voice
        );
        assert_eq!(
            IbmVoice::from_str_or_default("michael"),
            IbmVoice::EnUsMichaelV3Voice
        );
        assert_eq!(
            IbmVoice::from_str_or_default("charlotte"),
            IbmVoice::EnGbCharlotteV3Voice
        );
    }

    #[test]
    fn test_voice_from_str_custom() {
        let voice = IbmVoice::from_str_or_default("custom-voice-abc");
        assert!(matches!(voice, IbmVoice::Custom(_)));
        assert_eq!(voice.as_str(), "custom-voice-abc");
    }

    #[test]
    fn test_voice_default() {
        assert_eq!(IbmVoice::default(), IbmVoice::EnUsAllisonV3Voice);
    }

    #[test]
    fn test_voices_for_language_en_us() {
        let voices = IbmVoice::voices_for_language("en-US");
        assert!(voices.contains(&IbmVoice::EnUsAllisonV3Voice));
        assert!(voices.contains(&IbmVoice::EnUsMichaelV3Voice));
        assert!(voices.contains(&IbmVoice::EnUsEmilyV3Voice));
        assert!(!voices.contains(&IbmVoice::EnGbCharlotteV3Voice));
    }

    #[test]
    fn test_voices_for_language_de_de() {
        let voices = IbmVoice::voices_for_language("de-DE");
        assert!(voices.contains(&IbmVoice::DeDeBirgitV3Voice));
        assert!(voices.contains(&IbmVoice::DeDeDieterV3Voice));
        assert!(voices.contains(&IbmVoice::DeDeErikaV3Voice));
    }

    #[test]
    fn test_voices_for_language_ko_kr() {
        let voices = IbmVoice::voices_for_language("ko-KR");
        assert!(voices.contains(&IbmVoice::KoKrHyunjunV3Voice));
        assert!(voices.contains(&IbmVoice::KoKrYunaV3Voice));
    }

    #[test]
    fn test_voice_display() {
        assert_eq!(
            format!("{}", IbmVoice::EnUsAllisonV3Voice),
            "en-US_AllisonV3Voice"
        );
    }
}

// =============================================================================
// Output Format Tests
// =============================================================================

mod output_format_tests {
    use super::*;

    #[test]
    fn test_output_format_as_str() {
        assert_eq!(IbmOutputFormat::Wav.as_str(), "wav");
        assert_eq!(IbmOutputFormat::Mp3.as_str(), "mp3");
        assert_eq!(IbmOutputFormat::OggOpus.as_str(), "ogg-opus");
        assert_eq!(IbmOutputFormat::L16.as_str(), "l16");
        assert_eq!(IbmOutputFormat::Mulaw.as_str(), "mulaw");
    }

    #[test]
    fn test_output_format_extension() {
        assert_eq!(IbmOutputFormat::Wav.extension(), "wav");
        assert_eq!(IbmOutputFormat::Mp3.extension(), "mp3");
        assert_eq!(IbmOutputFormat::OggOpus.extension(), "ogg");
        assert_eq!(IbmOutputFormat::Flac.extension(), "flac");
        assert_eq!(IbmOutputFormat::L16.extension(), "raw");
    }

    #[test]
    fn test_output_format_default_sample_rate() {
        assert_eq!(
            IbmOutputFormat::Wav.default_sample_rate(),
            DEFAULT_SAMPLE_RATE
        );
        assert_eq!(
            IbmOutputFormat::Mp3.default_sample_rate(),
            DEFAULT_SAMPLE_RATE
        );
        assert_eq!(IbmOutputFormat::Mulaw.default_sample_rate(), 8000);
        assert_eq!(IbmOutputFormat::Alaw.default_sample_rate(), 8000);
    }

    #[test]
    fn test_output_format_accept_header_wav() {
        assert_eq!(
            IbmOutputFormat::Wav.accept_header(Some(22050)),
            "audio/wav;rate=22050"
        );
        assert_eq!(IbmOutputFormat::Wav.accept_header(None), "audio/wav");
    }

    #[test]
    fn test_output_format_accept_header_mp3() {
        assert_eq!(IbmOutputFormat::Mp3.accept_header(None), "audio/mp3");
        assert_eq!(IbmOutputFormat::Mp3.accept_header(Some(16000)), "audio/mp3");
    }

    #[test]
    fn test_output_format_accept_header_ogg_opus() {
        assert_eq!(
            IbmOutputFormat::OggOpus.accept_header(Some(24000)),
            "audio/ogg;codecs=opus;rate=24000"
        );
        assert_eq!(
            IbmOutputFormat::OggOpus.accept_header(None),
            "audio/ogg;codecs=opus"
        );
    }

    #[test]
    fn test_output_format_accept_header_l16() {
        assert_eq!(
            IbmOutputFormat::L16.accept_header(Some(16000)),
            "audio/l16;rate=16000"
        );
        assert_eq!(
            IbmOutputFormat::L16.accept_header(None),
            format!("audio/l16;rate={}", DEFAULT_SAMPLE_RATE)
        );
    }

    #[test]
    fn test_output_format_accept_header_mulaw() {
        assert_eq!(
            IbmOutputFormat::Mulaw.accept_header(Some(8000)),
            "audio/mulaw;rate=8000"
        );
        assert_eq!(
            IbmOutputFormat::Mulaw.accept_header(None),
            "audio/mulaw;rate=8000"
        );
    }

    #[test]
    fn test_output_format_requires_sample_rate() {
        assert!(IbmOutputFormat::L16.requires_sample_rate());
        assert!(IbmOutputFormat::Mulaw.requires_sample_rate());
        assert!(IbmOutputFormat::Alaw.requires_sample_rate());
        assert!(!IbmOutputFormat::Wav.requires_sample_rate());
        assert!(!IbmOutputFormat::Mp3.requires_sample_rate());
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(
            IbmOutputFormat::from_str_or_default("wav"),
            IbmOutputFormat::Wav
        );
        assert_eq!(
            IbmOutputFormat::from_str_or_default("mp3"),
            IbmOutputFormat::Mp3
        );
        assert_eq!(
            IbmOutputFormat::from_str_or_default("opus"),
            IbmOutputFormat::OggOpus
        );
        assert_eq!(
            IbmOutputFormat::from_str_or_default("pcm"),
            IbmOutputFormat::L16
        );
        assert_eq!(
            IbmOutputFormat::from_str_or_default("linear16"),
            IbmOutputFormat::L16
        );
        assert_eq!(
            IbmOutputFormat::from_str_or_default("mulaw"),
            IbmOutputFormat::Mulaw
        );
        assert_eq!(
            IbmOutputFormat::from_str_or_default("unknown"),
            IbmOutputFormat::Wav
        );
    }

    #[test]
    fn test_output_format_default() {
        assert_eq!(IbmOutputFormat::default(), IbmOutputFormat::Wav);
    }
}

// =============================================================================
// Configuration Tests
// =============================================================================

mod config_tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = IbmWatsonTTSConfig::default();
        assert_eq!(config.voice, IbmVoice::EnUsAllisonV3Voice);
        assert_eq!(config.output_format, IbmOutputFormat::Wav);
        assert_eq!(config.region, IbmRegion::UsSouth);
        assert!(config.instance_id.is_empty());
        assert_eq!(config.base.sample_rate, Some(DEFAULT_SAMPLE_RATE));
    }

    #[test]
    fn test_config_with_voice() {
        let config = IbmWatsonTTSConfig::with_voice(IbmVoice::EnGbJamesV3Voice);
        assert_eq!(config.voice, IbmVoice::EnGbJamesV3Voice);
        assert_eq!(config.base.voice_id, Some("en-GB_JamesV3Voice".to_string()));
    }

    #[test]
    fn test_config_validate_valid() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();
        config.base.api_key = "test-api-key".to_string();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_missing_instance_id() {
        let mut config = IbmWatsonTTSConfig::default();
        config.base.api_key = "test-api-key".to_string();

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Instance ID"));
    }

    #[test]
    fn test_config_validate_missing_api_key() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }

    #[test]
    fn test_config_validate_rate_percentage_too_high() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();
        config.base.api_key = "test-api-key".to_string();
        config.rate_percentage = Some(150);

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Rate percentage"));
    }

    #[test]
    fn test_config_validate_rate_percentage_too_low() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();
        config.base.api_key = "test-api-key".to_string();
        config.rate_percentage = Some(-150);

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Rate percentage"));
    }

    #[test]
    fn test_config_validate_pitch_percentage_out_of_range() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();
        config.base.api_key = "test-api-key".to_string();
        config.pitch_percentage = Some(200);

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Pitch percentage"));
    }

    #[test]
    fn test_config_validate_too_many_customizations() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();
        config.base.api_key = "test-api-key".to_string();
        config.customization_ids = vec![
            "cust1".to_string(),
            "cust2".to_string(),
            "cust3".to_string(),
        ];

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("customization"));
    }

    #[test]
    fn test_config_build_synthesis_url() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance-123".to_string();
        config.region = IbmRegion::UsSouth;

        let url = config.build_synthesis_url();
        assert!(url.contains("api.us-south.text-to-speech.watson.cloud.ibm.com"));
        assert!(url.contains("instances/test-instance-123"));
        assert!(url.contains("/v1/synthesize"));
    }

    #[test]
    fn test_config_build_synthesis_url_different_regions() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();

        config.region = IbmRegion::EuDe;
        assert!(config.build_synthesis_url().contains("eu-de"));

        config.region = IbmRegion::EuGb;
        assert!(config.build_synthesis_url().contains("eu-gb"));

        config.region = IbmRegion::JpTok;
        assert!(config.build_synthesis_url().contains("jp-tok"));

        config.region = IbmRegion::AuSyd;
        assert!(config.build_synthesis_url().contains("au-syd"));
    }

    #[test]
    fn test_config_build_query_params() {
        let config = IbmWatsonTTSConfig::default();
        let params = config.build_query_params();

        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "voice" && v.contains("Allison"))
        );
    }

    #[test]
    fn test_config_build_query_params_with_customization() {
        let mut config = IbmWatsonTTSConfig::default();
        config.customization_ids = vec!["cust-123".to_string()];

        let params = config.build_query_params();
        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "customization_id" && v == "cust-123")
        );
    }

    #[test]
    fn test_config_accept_header() {
        let mut config = IbmWatsonTTSConfig::default();
        config.output_format = IbmOutputFormat::OggOpus;
        config.base.sample_rate = Some(24000);

        let header = config.accept_header();
        assert!(header.contains("audio/ogg;codecs=opus"));
        assert!(header.contains("rate=24000"));
    }

    #[test]
    fn test_config_effective_sample_rate() {
        let config = IbmWatsonTTSConfig::default();
        assert_eq!(config.effective_sample_rate(), DEFAULT_SAMPLE_RATE);

        let mut config_custom = IbmWatsonTTSConfig::default();
        config_custom.base.sample_rate = Some(16000);
        assert_eq!(config_custom.effective_sample_rate(), 16000);

        let mut config_none = IbmWatsonTTSConfig::default();
        config_none.base.sample_rate = None;
        config_none.output_format = IbmOutputFormat::Mulaw;
        assert_eq!(config_none.effective_sample_rate(), 8000);
    }
}

// =============================================================================
// Provider Tests
// =============================================================================

mod provider_tests {
    use super::*;

    #[tokio::test]
    async fn test_provider_creation_from_base_config() {
        let config = TTSConfig {
            provider: "ibm-watson".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("en-US_MichaelV3Voice".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(16000),
            ..Default::default()
        };

        let tts = IbmWatsonTTS::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(tts.voice(), IbmVoice::EnUsMichaelV3Voice);
        assert_eq!(tts.output_format(), IbmOutputFormat::Mp3);
    }

    #[tokio::test]
    async fn test_provider_creation_from_ibm_config() {
        let config = IbmWatsonTTSConfig {
            voice: IbmVoice::FrFrReneeV3Voice,
            output_format: IbmOutputFormat::Flac,
            region: IbmRegion::EuDe,
            instance_id: "test-instance".to_string(),
            base: TTSConfig {
                api_key: "test-key".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        assert_eq!(tts.voice(), IbmVoice::FrFrReneeV3Voice);
        assert_eq!(tts.output_format(), IbmOutputFormat::Flac);
        assert_eq!(tts.region(), IbmRegion::EuDe);
    }

    #[tokio::test]
    async fn test_provider_set_voice() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        tts.set_voice(IbmVoice::JaJpEmiV3Voice);
        assert_eq!(tts.voice(), IbmVoice::JaJpEmiV3Voice);
        assert_eq!(
            tts.ibm_config().base.voice_id,
            Some("ja-JP_EmiV3Voice".to_string())
        );
    }

    #[tokio::test]
    async fn test_provider_set_output_format() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        tts.set_output_format(IbmOutputFormat::Webm);
        assert_eq!(tts.output_format(), IbmOutputFormat::Webm);
        assert_eq!(tts.ibm_config().base.audio_format, Some("webm".to_string()));
    }

    #[tokio::test]
    async fn test_provider_set_region() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        tts.set_region(IbmRegion::KrSeo);
        assert_eq!(tts.region(), IbmRegion::KrSeo);
    }

    #[tokio::test]
    async fn test_provider_set_instance_id() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        tts.set_instance_id("my-instance-456".to_string());
        assert_eq!(tts.ibm_config().instance_id, "my-instance-456");
    }

    #[tokio::test]
    async fn test_provider_set_rate_percentage_valid() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        assert!(tts.set_rate_percentage(0).is_ok());
        assert!(tts.set_rate_percentage(100).is_ok());
        assert!(tts.set_rate_percentage(-100).is_ok());
        assert!(tts.set_rate_percentage(50).is_ok());
        assert!(tts.set_rate_percentage(-50).is_ok());
    }

    #[tokio::test]
    async fn test_provider_set_rate_percentage_invalid() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        assert!(tts.set_rate_percentage(101).is_err());
        assert!(tts.set_rate_percentage(-101).is_err());
        assert!(tts.set_rate_percentage(200).is_err());
    }

    #[tokio::test]
    async fn test_provider_set_pitch_percentage_valid() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        assert!(tts.set_pitch_percentage(0).is_ok());
        assert!(tts.set_pitch_percentage(100).is_ok());
        assert!(tts.set_pitch_percentage(-100).is_ok());
    }

    #[tokio::test]
    async fn test_provider_set_pitch_percentage_invalid() {
        let config = TTSConfig::default();
        let mut tts = IbmWatsonTTS::new(config).unwrap();

        assert!(tts.set_pitch_percentage(101).is_err());
        assert!(tts.set_pitch_percentage(-101).is_err());
    }

    #[test]
    fn test_provider_prepare_request_body_plain_text() {
        let config = IbmWatsonTTSConfig::default();
        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();

        let body = tts.prepare_request_body("Hello, world!");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(parsed["text"], "Hello, world!");
        assert!(!body.contains("<speak"));
    }

    #[test]
    fn test_provider_prepare_request_body_with_rate() {
        let mut config = IbmWatsonTTSConfig::default();
        config.rate_percentage = Some(25);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("Test text");

        // Note: JSON escapes quotes, so we check for escaped versions
        assert!(body.contains("<speak"));
        assert!(body.contains(r#"rate=\"+25%\""#));
    }

    #[test]
    fn test_provider_prepare_request_body_with_negative_rate() {
        let mut config = IbmWatsonTTSConfig::default();
        config.rate_percentage = Some(-30);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("Test text");

        // Note: JSON escapes quotes
        assert!(body.contains(r#"rate=\"-30%\""#));
    }

    #[test]
    fn test_provider_prepare_request_body_with_pitch() {
        let mut config = IbmWatsonTTSConfig::default();
        config.pitch_percentage = Some(15);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("Test text");

        // Note: JSON escapes quotes
        assert!(body.contains("<speak"));
        assert!(body.contains(r#"pitch=\"+15%\""#));
    }

    #[test]
    fn test_provider_prepare_request_body_with_rate_and_pitch() {
        let mut config = IbmWatsonTTSConfig::default();
        config.rate_percentage = Some(50);
        config.pitch_percentage = Some(-20);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("Test text");

        // Note: JSON escapes quotes
        assert!(body.contains("<speak"));
        assert!(body.contains("<prosody"));
        assert!(body.contains(r#"rate=\"+50%\""#));
        assert!(body.contains(r#"pitch=\"-20%\""#));
    }

    #[test]
    fn test_provider_prepare_request_body_escapes_ampersand() {
        let mut config = IbmWatsonTTSConfig::default();
        config.rate_percentage = Some(10);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("Tom & Jerry");

        assert!(body.contains("Tom &amp; Jerry"));
    }

    #[test]
    fn test_provider_prepare_request_body_escapes_less_than() {
        let mut config = IbmWatsonTTSConfig::default();
        config.rate_percentage = Some(10);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("x < y");

        assert!(body.contains("x &lt; y"));
    }

    #[test]
    fn test_provider_prepare_request_body_escapes_greater_than() {
        let mut config = IbmWatsonTTSConfig::default();
        config.rate_percentage = Some(10);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("x > y");

        assert!(body.contains("x &gt; y"));
    }

    #[test]
    fn test_provider_prepare_request_body_escapes_quotes() {
        let mut config = IbmWatsonTTSConfig::default();
        config.rate_percentage = Some(10);

        let tts = IbmWatsonTTS::new_from_ibm_config(config).unwrap();
        let body = tts.prepare_request_body("He said \"hello\"");

        assert!(body.contains("&quot;hello&quot;"));
    }

    #[test]
    fn test_provider_info() {
        let config = TTSConfig::default();
        let tts = IbmWatsonTTS::new(config).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "ibm-watson");
        assert_eq!(info["api_type"], "REST");
        assert_eq!(info["connection_pooling"], true);
        assert!(
            info["supported_formats"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("wav"))
        );
        assert!(
            info["supported_formats"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("mp3"))
        );
        assert!(
            info["supported_formats"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("ogg-opus"))
        );
    }

    #[test]
    fn test_provider_info_features() {
        let config = TTSConfig::default();
        let tts = IbmWatsonTTS::new(config).unwrap();
        let info = tts.get_provider_info();

        assert!(info["features"]["ssml"].as_bool().unwrap());
        assert!(info["features"]["rate_control"].as_bool().unwrap());
        assert!(info["features"]["pitch_control"].as_bool().unwrap());
        assert!(info["features"]["custom_pronunciation"].as_bool().unwrap());
        assert!(info["features"]["multiple_languages"].as_bool().unwrap());
    }

    #[test]
    fn test_provider_info_supported_voices() {
        let config = TTSConfig::default();
        let tts = IbmWatsonTTS::new(config).unwrap();
        let info = tts.get_provider_info();

        let voices = info["supported_voices"].as_array().unwrap();
        assert!(voices.contains(&serde_json::json!("en-US_AllisonV3Voice")));
        assert!(voices.contains(&serde_json::json!("en-GB_CharlotteV3Voice")));
        assert!(voices.contains(&serde_json::json!("de-DE_BirgitV3Voice")));
        assert!(voices.contains(&serde_json::json!("ja-JP_EmiV3Voice")));
    }
}

// =============================================================================
// Region Tests
// =============================================================================

mod region_tests {
    use super::*;

    #[test]
    fn test_region_tts_hostname() {
        assert_eq!(
            IbmRegion::UsSouth.tts_hostname(),
            "api.us-south.text-to-speech.watson.cloud.ibm.com"
        );
        assert_eq!(
            IbmRegion::UsEast.tts_hostname(),
            "api.us-east.text-to-speech.watson.cloud.ibm.com"
        );
        assert_eq!(
            IbmRegion::EuDe.tts_hostname(),
            "api.eu-de.text-to-speech.watson.cloud.ibm.com"
        );
        assert_eq!(
            IbmRegion::EuGb.tts_hostname(),
            "api.eu-gb.text-to-speech.watson.cloud.ibm.com"
        );
        assert_eq!(
            IbmRegion::AuSyd.tts_hostname(),
            "api.au-syd.text-to-speech.watson.cloud.ibm.com"
        );
        assert_eq!(
            IbmRegion::JpTok.tts_hostname(),
            "api.jp-tok.text-to-speech.watson.cloud.ibm.com"
        );
        assert_eq!(
            IbmRegion::KrSeo.tts_hostname(),
            "api.kr-seo.text-to-speech.watson.cloud.ibm.com"
        );
    }
}

// =============================================================================
// Constants Tests
// =============================================================================

mod constants_tests {
    use super::*;

    #[test]
    fn test_iam_url() {
        assert_eq!(IBM_IAM_URL, "https://iam.cloud.ibm.com/identity/token");
    }

    #[test]
    fn test_default_voice() {
        assert_eq!(DEFAULT_VOICE, "en-US_AllisonV3Voice");
    }

    #[test]
    fn test_max_text_length() {
        // IBM Watson counts bytes, not characters (5KB = 5120 bytes)
        assert_eq!(MAX_TEXT_LENGTH, 5120);
    }

    #[test]
    fn test_default_sample_rate() {
        assert_eq!(DEFAULT_SAMPLE_RATE, 22050);
    }

    #[test]
    fn test_ibm_watson_tts_url() {
        assert_eq!(
            IBM_WATSON_TTS_URL,
            "https://api.us-south.text-to-speech.watson.cloud.ibm.com"
        );
    }
}

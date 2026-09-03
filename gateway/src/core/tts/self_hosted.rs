//! TTS against a self-hosted, OpenAI-compatible audio server (FRD-018 §5.2).
//!
//! This is what makes "bring your own audio model" work: a Kokoro or XTTS or vLLM server the
//! operator already runs, reached at a URL they chose, speaking the OpenAI
//! `POST /v1/audio/speech` shape. budapp registers it with source `waav_self_hosted` and
//! publishes vendor `self_hosted` into `voice_table`; this module is the other half.
//!
//! Two things make it different from every other provider here, and both are why it could not
//! simply reuse one:
//!
//! * **The endpoint is data, not a constant.** Every hosted vendor compiles its URL in. A
//!   self-hosted deployment's address arrives per-endpoint in `TTSConfig::api_base`.
//! * **The credential is optional.** An in-cluster server behind network policy commonly has
//!   no auth at all, so an empty key must mean "send no header" rather than "send an empty
//!   bearer", which some servers reject outright.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::base::{AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSResult};
use super::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

/// Join a configured base URL with the OpenAI speech path.
///
/// Operators write the base with and without a trailing slash, and both must land on exactly
/// one. A doubled slash is not cosmetic: some servers route `//audio/speech` to a 404, which
/// surfaces as "the model is wrong" rather than "the URL is wrong".
pub fn speech_url(api_base: &str) -> String {
    format!("{}/audio/speech", api_base.trim_end_matches('/'))
}

/// The OpenAI `response_format` value for a WaaV audio-format name.
///
/// WaaV's internal vocabulary is the union of every vendor's; OpenAI-compatible servers accept
/// a much smaller set. Anything unrecognised falls back to `mp3` rather than being forwarded,
/// because a server that rejects the format fails the whole request, and mp3 is the one value
/// every implementation of this API supports.
pub fn response_format(audio_format: Option<&str>) -> &'static str {
    match audio_format.unwrap_or("mp3") {
        "wav" => "wav",
        "opus" => "opus",
        "aac" => "aac",
        "flac" => "flac",
        "pcm" | "linear16" => "pcm",
        _ => "mp3",
    }
}

#[derive(Clone)]
struct SelfHostedRequestBuilder {
    config: TTSConfig,
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl TTSRequestBuilder for SelfHostedRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        let base = self.config.api_base.as_deref().unwrap_or_default();
        let mut req = client.post(speech_url(base)).json(&json!({
            "model": self.config.model,
            "input": text,
            "voice": self.config.voice_id.as_deref().unwrap_or("alloy"),
            "response_format": response_format(self.config.audio_format.as_deref()),
            "speed": self.config.speaking_rate.unwrap_or(1.0),
        }));

        // An empty key means the server needs none. Sending `Bearer ` with nothing after it is
        // not equivalent: it is a malformed credential, and a server that validates the header
        // shape rejects the request rather than treating it as anonymous.
        if !self.config.api_key.is_empty() {
            req = req.bearer_auth(&self.config.api_key);
        }
        req
    }

    fn get_config(&self) -> &TTSConfig {
        &self.config
    }

    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

/// TTS provider for a self-hosted OpenAI-compatible audio server.
pub struct SelfHostedTTS {
    provider: TTSProvider,
    request_builder: SelfHostedRequestBuilder,
}

impl SelfHostedTTS {
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        if config
            .api_base
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            // Refused at construction, where the message can name the field. Without it the
            // request would go to "/audio/speech" against no host and fail as a transport
            // error, which reads like the deployment is down.
            return Err(crate::core::tts::TTSError::InvalidConfiguration(
                "a self-hosted audio endpoint needs api_base (the deployment URL, including \
                 the version segment); budapp publishes it into voice_table"
                    .to_string(),
            ));
        }

        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };
        Ok(Self {
            provider: TTSProvider::new()?,
            request_builder: SelfHostedRequestBuilder {
                config: config.clone(),
                pronunciation_replacer,
            },
        })
    }

    pub async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        self.provider.set_req_manager(req_manager).await;
    }
}

#[async_trait]
impl BaseTTS for SelfHostedTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        SelfHostedTTS::new(config)
    }

    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        let base = self
            .request_builder
            .config
            .api_base
            .clone()
            .unwrap_or_default();
        self.provider
            .generic_connect_with_config(&speech_url(&base), &self.request_builder.config)
            .await
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.provider.generic_disconnect().await
    }

    fn is_ready(&self) -> bool {
        self.provider.is_ready()
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.provider.get_connection_state()
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if !self.is_ready() {
            tracing::info!("self-hosted TTS not ready, connecting");
            self.connect().await?;
        }
        self.provider
            .generic_speak(self.request_builder.clone(), text, flush)
            .await
    }

    async fn clear(&mut self) -> TTSResult<()> {
        self.provider.generic_clear().await
    }

    async fn flush(&self) -> TTSResult<()> {
        self.provider.generic_flush().await
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.provider.generic_on_audio(callback)
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        self.provider.generic_remove_audio_callback()
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "self_hosted",
            "api_type": "HTTP REST (OpenAI-compatible /audio/speech)",
            "api_base": self.request_builder.config.api_base,
            "supported_formats": ["mp3", "wav", "pcm", "aac", "flac", "opus"],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_slash_does_not_double_up() {
        // Both spellings are what operators actually paste, and `//audio/speech` 404s on some
        // servers -- a failure that reads as a bad model rather than a bad URL.
        assert_eq!(
            speech_url("http://whisper.ns.svc:8000/v1"),
            "http://whisper.ns.svc:8000/v1/audio/speech"
        );
        assert_eq!(
            speech_url("http://whisper.ns.svc:8000/v1/"),
            "http://whisper.ns.svc:8000/v1/audio/speech"
        );
        assert_eq!(
            speech_url("http://whisper.ns.svc:8000/v1///"),
            "http://whisper.ns.svc:8000/v1/audio/speech"
        );
    }

    #[test]
    fn an_unknown_audio_format_falls_back_rather_than_being_forwarded() {
        // WaaV's format vocabulary is the union of every vendor's. Forwarding a name this API
        // does not know fails the whole request; mp3 is the one value every implementation
        // supports.
        assert_eq!(response_format(Some("linear16")), "pcm");
        assert_eq!(response_format(Some("pcm")), "pcm");
        assert_eq!(response_format(Some("wav")), "wav");
        assert_eq!(response_format(Some("mulaw")), "mp3");
        assert_eq!(response_format(Some("ogg_vorbis")), "mp3");
        assert_eq!(response_format(None), "mp3");
    }

    #[test]
    fn a_missing_api_base_is_refused_with_a_message_naming_the_field() {
        // The alternative is a request to "/audio/speech" with no host, which surfaces as a
        // transport error and reads like the deployment is down.
        let err = match SelfHostedTTS::new(TTSConfig {
            provider: "self_hosted".into(),
            api_base: None,
            ..Default::default()
        }) {
            Ok(_) => panic!("no api_base must be refused"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("api_base"), "got: {err}");

        assert!(
            SelfHostedTTS::new(TTSConfig {
                api_base: Some("   ".into()),
                ..Default::default()
            })
            .is_err(),
            "a blank api_base is the shape an unset Helm value takes and must be refused too"
        );
    }

    #[test]
    fn a_configured_api_base_constructs() {
        assert!(
            SelfHostedTTS::new(TTSConfig {
                provider: "self_hosted".into(),
                api_base: Some("http://audiostub/v1".into()),
                ..Default::default()
            })
            .is_ok()
        );
    }

    #[test]
    fn an_absent_credential_sends_no_authorization_header() {
        // An in-cluster server behind network policy commonly has no auth. `Bearer ` with an
        // empty value is a malformed credential, not an anonymous one.
        let client = reqwest::Client::new();
        let builder = SelfHostedRequestBuilder {
            config: TTSConfig {
                api_base: Some("http://audiostub/v1".into()),
                api_key: String::new(),
                ..Default::default()
            },
            pronunciation_replacer: None,
        };
        let req = builder.build_http_request(&client, "hello").build().unwrap();
        assert!(
            req.headers().get(reqwest::header::AUTHORIZATION).is_none(),
            "an empty api_key must send no Authorization header at all"
        );
    }

    #[test]
    fn a_present_credential_is_sent_as_a_bearer() {
        let client = reqwest::Client::new();
        let builder = SelfHostedRequestBuilder {
            config: TTSConfig {
                api_base: Some("http://audiostub/v1".into()),
                api_key: "sk-local".into(),
                ..Default::default()
            },
            pronunciation_replacer: None,
        };
        let req = builder.build_http_request(&client, "hello").build().unwrap();
        assert_eq!(
            req.headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer sk-local")
        );
        assert_eq!(req.url().as_str(), "http://audiostub/v1/audio/speech");
    }
}

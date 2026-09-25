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
//!
//! The same client also serves **Azure OpenAI audio** (vendor `azure_openai`, voice contract
//! §3): OpenAI's request and response bodies, at a deployment URL
//! `{api_base}/openai/deployments/{model}/audio/{speech|transcriptions|translations}?api-version=…`,
//! authenticated with an `api-key` header and never `Authorization`. That is a URL shape and a
//! header, not a new wire protocol, so it is a mode of this provider rather than a second
//! client. Unlike a self-hosted server, an Azure resource is a public endpoint, so its URL is
//! held to https and the shared SSRF rules.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::base::{AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSResult};
use super::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use super::standard::StandardTTSConfig;
use crate::utils::req_manager::ReqManager;

/// Join a configured base URL with the OpenAI speech path.
///
/// Operators write the base with and without a trailing slash, and both must land on exactly
/// one. A doubled slash is not cosmetic: some servers route `//audio/speech` to a 404, which
/// surfaces as "the model is wrong" rather than "the URL is wrong".
pub fn speech_url(api_base: &str) -> String {
    format!("{}/audio/speech", api_base.trim_end_matches('/'))
}

/// Canonical id plus every alias the plugin registry accepts for it.
///
/// MUST stay in step with the `.with_aliases(...)` list on the `self_hosted` registration in
/// `plugin/builtin/mod.rs`. Kept here so the credential pre-check and the provider factory
/// agree on what "self-hosted" means.
pub const SELF_HOSTED_NAMES: &[&str] = &[
    "self_hosted",
    "self-hosted",
    "waav_self_hosted",
    "openai_compatible",
];

/// Whether a `voice_table` vendor names the self-hosted provider.
///
/// This exists because a bare `vendor != "self_hosted"` comparison was load-bearing in the
/// credential pre-check, and the registry accepts three more spellings. Publishing any of
/// them produced a keyless deployment that the factory would have served happily but the
/// pre-check refused with "has no credential configured" -- an error naming the wrong
/// problem entirely. Observed live against a deployment published as `waav_self_hosted`.
pub fn is_self_hosted(vendor: &str) -> bool {
    let v = vendor.trim().to_ascii_lowercase();
    SELF_HOSTED_NAMES.iter().any(|n| *n == v)
}

/// Join a configured base URL with the OpenAI transcription path.
///
/// Same trailing-slash discipline as [`speech_url`], for the same reason.
pub fn transcription_url(api_base: &str) -> String {
    format!("{}/audio/transcriptions", api_base.trim_end_matches('/'))
}

/// Join a configured base URL with the OpenAI translation path.
///
/// A SEPARATE path, not a flag on the transcription one: an OpenAI-compatible server decides
/// "transcribe" versus "translate to English" by route, so collapsing the two would return
/// source-language text from a translation request while looking entirely successful.
pub fn translation_url(api_base: &str) -> String {
    format!("{}/audio/translations", api_base.trim_end_matches('/'))
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

// ------------------------------------------------------------------------------------------- //
// Azure OpenAI audio (voice contract §3)
// ------------------------------------------------------------------------------------------- //

/// Vendor names that mean Azure OpenAI audio.
///
/// NOT `azure`: that is Azure AI Speech, a different service with a different API, served by
/// `core::tts::azure`. budapp publishes `azure/speech/*` models as `azure` and every other
/// `azure` audio model (`tts-1`, `whisper-1`, `gpt-4o-*`) as `azure_openai`. MUST stay in step
/// with the `azure_openai` registration in `plugin/builtin/mod.rs` and the dispatch arm in
/// `core/tts/standard.rs`.
pub const AZURE_OPENAI_NAMES: &[&str] = &["azure_openai", "azure-openai"];

/// The `api-version` sent when a deployment does not name one.
pub const AZURE_OPENAI_DEFAULT_API_VERSION: &str = "2025-04-01-preview";

/// The provider-extras key carrying a deployment's `api-version`.
///
/// The handler copies `voice_table.provider_params.api_version` here; the flat registry path
/// has no extras and always sends [`AZURE_OPENAI_DEFAULT_API_VERSION`].
pub const AZURE_OPENAI_API_VERSION_EXTRA: &str = "api_version";

/// The header Azure OpenAI reads its key from. It ignores `Authorization: Bearer`, and
/// sending both would put the key on the wire twice for no benefit.
pub const AZURE_OPENAI_API_KEY_HEADER: &str = "api-key";

/// Schemes an Azure OpenAI URL may use in production: https only.
const AZURE_OPENAI_URL_SCHEMES: &[&str] = &["https"];

/// Whether a `voice_table` vendor names Azure OpenAI audio (and not Azure AI Speech).
pub fn is_azure_openai(vendor: &str) -> bool {
    let v = vendor.trim().to_ascii_lowercase();
    AZURE_OPENAI_NAMES.iter().any(|n| *n == v)
}

/// Which Azure OpenAI audio operation a URL addresses.
///
/// Transcription and translation are separate ROUTES, as on every OpenAI-shaped server: a
/// translation request sent to the transcription route returns source-language text with a 200.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AzureAudioRoute {
    Speech,
    Transcriptions,
    Translations,
}

impl AzureAudioRoute {
    fn segment(self) -> &'static str {
        match self {
            Self::Speech => "speech",
            Self::Transcriptions => "transcriptions",
            Self::Translations => "translations",
        }
    }
}

/// The `api-version` to send: the deployment's own when it names one, else the default.
pub fn azure_openai_api_version(requested: Option<&str>) -> &str {
    requested
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(AZURE_OPENAI_DEFAULT_API_VERSION)
}

/// `{api_base}/openai/deployments/{deployment}/audio/{route}?api-version={api_version}`.
///
/// `api_base` is the resource endpoint (`https://<resource>.openai.azure.com`), with or without
/// a trailing slash; a path on it (an API Management prefix) is kept. The deployment is ONE
/// path segment and is percent-encoded as one, so a `/` in it cannot re-route the request; `.`
/// and `..` are refused outright, because a URL builder silently drops them.
pub fn azure_openai_audio_url(
    api_base: &str,
    deployment: &str,
    route: AzureAudioRoute,
    api_version: Option<&str>,
) -> Result<String, String> {
    let deployment = deployment.trim();
    if deployment.is_empty() || deployment == "." || deployment == ".." {
        return Err(format!(
            "an azure_openai endpoint needs a deployment name (voice_table `model`: the \
             credential's deployment_id, else the model name); got {deployment:?}"
        ));
    }
    let base = api_base.trim().trim_end_matches('/');
    let mut url = url::Url::parse(base)
        .map_err(|e| format!("azure_openai api_base {base:?} is not a URL: {e}"))?;
    url.path_segments_mut()
        .map_err(|()| format!("azure_openai api_base {base:?} cannot carry a path"))?
        .pop_if_empty()
        .extend([
            "openai",
            "deployments",
            deployment,
            "audio",
            route.segment(),
        ]);
    url.query_pairs_mut()
        .append_pair("api-version", azure_openai_api_version(api_version));
    Ok(url.into())
}

/// The schemes an Azure OpenAI URL (and every redirect from it) may use.
///
/// https only. The one exception is the test-only `WAAV_ALLOW_LOOPBACK_ENDPOINTS` escape hatch,
/// which already disables every host check so an in-process mock can stand in for a vendor; it
/// admits plain http too, because that mock has no certificate. It is never set in production.
pub(crate) fn azure_openai_url_schemes() -> &'static [&'static str] {
    if crate::core::net::loopback_endpoints_allowed() {
        crate::core::net::HTTP_URL_SCHEMES
    } else {
        AZURE_OPENAI_URL_SCHEMES
    }
}

/// SSRF gate for an Azure OpenAI URL: https, and a host that is not loopback, private,
/// link-local or a metadata endpoint (the shared `core::net` rules, resolve-then-validate).
///
/// A self-hosted deployment is deliberately NOT gated this way -- an in-cluster address is its
/// whole point. An Azure OpenAI resource is a public endpoint, so a private target is never
/// legitimate here. (An Azure Private Link endpoint resolves to a private address and is
/// refused; that deployment shape would need an explicit allowance.)
///
/// Resolves DNS synchronously: call it at construction, as the crate's other SSRF checks are,
/// or off the async workers.
pub fn validate_azure_openai_url(url: &str) -> Result<(), String> {
    crate::core::net::validate_url_for_ssrf(url, azure_openai_url_schemes())
}

/// Attach an Azure OpenAI key as `api-key`, marked sensitive so it never renders in a `Debug`.
///
/// A key with bytes a header cannot carry is left for reqwest to refuse as a builder error,
/// whose message does not include the value.
pub(crate) fn with_azure_openai_api_key(
    req: reqwest::RequestBuilder,
    api_key: &str,
) -> reqwest::RequestBuilder {
    match reqwest::header::HeaderValue::from_str(api_key) {
        Ok(mut value) => {
            value.set_sensitive(true);
            req.header(AZURE_OPENAI_API_KEY_HEADER, value)
        }
        Err(_) => req.header(AZURE_OPENAI_API_KEY_HEADER, api_key),
    }
}

/// Where a request goes and how it authenticates.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Upstream {
    /// `{api_base}/audio/speech`; `Authorization: Bearer` when a key is set, nothing when not.
    OpenAiCompatible,
    /// A complete Azure OpenAI speech URL (api-version included); `api-key` header.
    AzureOpenAi { url: String },
}

#[derive(Clone)]
struct SelfHostedRequestBuilder {
    config: TTSConfig,
    pronunciation_replacer: Option<PronunciationReplacer>,
    upstream: Upstream,
}

impl SelfHostedRequestBuilder {
    fn target_url(&self) -> String {
        match &self.upstream {
            Upstream::OpenAiCompatible => {
                speech_url(self.config.api_base.as_deref().unwrap_or_default())
            }
            Upstream::AzureOpenAi { url } => url.clone(),
        }
    }
}

impl TTSRequestBuilder for SelfHostedRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // The same body for both upstreams. On Azure `model` is the deployment name, which the
        // deployment URL already routes on; the OpenAI SDK's Azure client sends it the same way.
        let req = client.post(self.target_url()).json(&json!({
            "model": self.config.model,
            "input": text,
            "voice": self.config.voice_id.as_deref().unwrap_or("alloy"),
            "response_format": response_format(self.config.audio_format.as_deref()),
            "speed": self.config.speaking_rate.unwrap_or(1.0),
        }));

        match &self.upstream {
            // An empty key means the server needs none. Sending `Bearer ` with nothing after it
            // is not equivalent: it is a malformed credential, and a server that validates the
            // header shape rejects the request rather than treating it as anonymous.
            Upstream::OpenAiCompatible if self.config.api_key.is_empty() => req,
            Upstream::OpenAiCompatible => req.bearer_auth(&self.config.api_key),
            Upstream::AzureOpenAi { .. } => {
                with_azure_openai_api_key(req, self.config.api_key.trim())
            }
        }
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

        Ok(Self::with_upstream(config, Upstream::OpenAiCompatible))
    }

    /// Azure OpenAI audio: the OpenAI speech API on an Azure deployment URL, `api-key` auth.
    ///
    /// `config.api_base` is the resource endpoint, `config.model` the deployment name, and
    /// `config.api_key` the resource key; `api_version` falls back to
    /// [`AZURE_OPENAI_DEFAULT_API_VERSION`]. Everything that can be wrong with the target --
    /// no base, no key, no deployment, not https, a private or loopback host -- is refused HERE,
    /// where the message can name the field, rather than surfacing later as a transport error
    /// that reads like Azure is down.
    pub fn new_azure_openai(config: TTSConfig, api_version: Option<&str>) -> TTSResult<Self> {
        use crate::core::tts::TTSError::InvalidConfiguration;

        let base = config
            .api_base
            .as_deref()
            .map(str::trim)
            .unwrap_or_default();
        if base.is_empty() {
            return Err(InvalidConfiguration(
                "an azure_openai audio endpoint needs api_base (the resource endpoint, e.g. \
                 https://<resource>.openai.azure.com); budapp publishes it into voice_table"
                    .to_string(),
            ));
        }
        if config.api_key.trim().is_empty() {
            return Err(InvalidConfiguration(
                "an azure_openai audio endpoint needs the resource's API key; Azure OpenAI has \
                 no anonymous access"
                    .to_string(),
            ));
        }
        let url = azure_openai_audio_url(base, &config.model, AzureAudioRoute::Speech, api_version)
            .map_err(InvalidConfiguration)?;
        validate_azure_openai_url(&url).map_err(|msg| {
            InvalidConfiguration(format!(
                "azure_openai api_base rejected (SSRF protection): {msg}"
            ))
        })?;

        Ok(Self::with_upstream(config, Upstream::AzureOpenAi { url }))
    }

    /// [`Self::new_azure_openai`] from the standard config, reading the deployment's
    /// `api-version` from `extras[AZURE_OPENAI_API_VERSION_EXTRA]`.
    ///
    /// Only that key is read from extras. In particular `endpoint_override` is not honoured: the
    /// target is the deployment URL budapp published, and nothing else.
    pub fn azure_openai_from_standard(std: &StandardTTSConfig) -> TTSResult<Self> {
        let api_version = std
            .extras
            .0
            .get(AZURE_OPENAI_API_VERSION_EXTRA)
            .and_then(|v| v.as_str());
        Self::new_azure_openai(std.base.clone(), api_version)
    }

    fn with_upstream(config: TTSConfig, upstream: Upstream) -> Self {
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };
        Self {
            provider: TTSProvider::new(),
            request_builder: SelfHostedRequestBuilder {
                config,
                pronunciation_replacer,
                upstream,
            },
        }
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
        let url = self.request_builder.target_url();
        self.provider
            .generic_connect_with_config(&url, &self.request_builder.config)
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
        let (provider, api_type) = match self.request_builder.upstream {
            Upstream::OpenAiCompatible => {
                ("self_hosted", "HTTP REST (OpenAI-compatible /audio/speech)")
            }
            Upstream::AzureOpenAi { .. } => (
                "azure_openai",
                "HTTP REST (Azure OpenAI /openai/deployments/{deployment}/audio/speech)",
            ),
        };
        serde_json::json!({
            "provider": provider,
            "api_type": api_type,
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
    fn every_alias_the_registry_accepts_counts_as_self_hosted() {
        // The pre-check and the factory must agree. If the registry gains an alias and this
        // list does not, a keyless deployment published under it is refused for a missing
        // credential it never needed.
        for v in [
            "self_hosted",
            "self-hosted",
            "waav_self_hosted",
            "openai_compatible",
        ] {
            assert!(is_self_hosted(v), "{v} should be recognised as self-hosted");
        }
        assert!(
            is_self_hosted("  Self_Hosted  "),
            "matching must be lenient like the registry"
        );
        assert!(!is_self_hosted("deepgram"));
        assert!(!is_self_hosted(""));
    }

    #[test]
    fn transcription_and_translation_are_distinct_paths() {
        // Collapsing them returns source-language text from a translation request, with a
        // 200 and no sign anything is wrong.
        assert_eq!(
            transcription_url("http://whisper.ns.svc:8000/v1"),
            "http://whisper.ns.svc:8000/v1/audio/transcriptions"
        );
        assert_eq!(
            translation_url("http://whisper.ns.svc:8000/v1"),
            "http://whisper.ns.svc:8000/v1/audio/translations"
        );
        assert_ne!(
            transcription_url("http://x/v1"),
            translation_url("http://x/v1")
        );
    }

    #[test]
    fn transcription_urls_normalise_trailing_slashes_like_speech_does() {
        for base in ["http://w/v1", "http://w/v1/", "http://w/v1///"] {
            assert_eq!(transcription_url(base), "http://w/v1/audio/transcriptions");
            assert_eq!(translation_url(base), "http://w/v1/audio/translations");
        }
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
            upstream: Upstream::OpenAiCompatible,
        };
        let req = builder
            .build_http_request(&client, "hello")
            .build()
            .unwrap();
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
            upstream: Upstream::OpenAiCompatible,
        };
        let req = builder
            .build_http_request(&client, "hello")
            .build()
            .unwrap();
        assert_eq!(
            req.headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer sk-local")
        );
        assert_eq!(req.url().as_str(), "http://audiostub/v1/audio/speech");
    }

    // --------------------------------------------------------------------------------------- //
    // Azure OpenAI mode (voice contract §3).
    // --------------------------------------------------------------------------------------- //

    const AZ: &str = "https://bud-test.openai.azure.com";

    fn azure_config(base: &str) -> TTSConfig {
        TTSConfig {
            provider: "azure_openai".into(),
            api_base: Some(base.into()),
            api_key: "az-test-key".into(),
            model: "gpt-4o-mini-tts".into(),
            voice_id: Some("alloy".into()),
            ..Default::default()
        }
    }

    #[test]
    fn azure_openai_urls_follow_the_deployment_shape() {
        for base in [
            AZ,
            "https://bud-test.openai.azure.com/",
            "https://bud-test.openai.azure.com///",
        ] {
            assert_eq!(
                azure_openai_audio_url(base, "tts-1", AzureAudioRoute::Speech, None).unwrap(),
                "https://bud-test.openai.azure.com/openai/deployments/tts-1/audio/speech\
                 ?api-version=2025-04-01-preview"
            );
        }
        assert_eq!(
            azure_openai_audio_url(
                AZ,
                "whisper-1",
                AzureAudioRoute::Transcriptions,
                Some("2024-06-01")
            )
            .unwrap(),
            "https://bud-test.openai.azure.com/openai/deployments/whisper-1/audio/transcriptions\
             ?api-version=2024-06-01"
        );
        assert_eq!(
            azure_openai_audio_url(AZ, "whisper-1", AzureAudioRoute::Translations, None).unwrap(),
            "https://bud-test.openai.azure.com/openai/deployments/whisper-1/audio/translations\
             ?api-version=2025-04-01-preview"
        );
        // A path on the base (an API Management prefix) is kept, not replaced.
        assert_eq!(
            azure_openai_audio_url(
                "https://apim.example.com/azure-oai/",
                "tts-1",
                AzureAudioRoute::Speech,
                None
            )
            .unwrap(),
            "https://apim.example.com/azure-oai/openai/deployments/tts-1/audio/speech\
             ?api-version=2025-04-01-preview"
        );
    }

    #[test]
    fn the_default_api_version_applies_only_when_none_is_named() {
        assert_eq!(azure_openai_api_version(None), "2025-04-01-preview");
        assert_eq!(azure_openai_api_version(Some("   ")), "2025-04-01-preview");
        assert_eq!(
            azure_openai_api_version(Some(" 2025-03-01-preview ")),
            "2025-03-01-preview"
        );
        assert_eq!(AZURE_OPENAI_DEFAULT_API_VERSION, "2025-04-01-preview");
    }

    #[test]
    fn a_deployment_is_one_path_segment_and_never_empty() {
        // A `/` in the name must not re-route the request to another path on the resource.
        let url = azure_openai_audio_url(AZ, "a/b", AzureAudioRoute::Speech, None).unwrap();
        assert!(
            url.contains("/openai/deployments/a%2Fb/audio/speech?"),
            "got {url}"
        );
        // `.` and `..` would be silently dropped by the URL builder, changing the path.
        for bad in ["", "   ", ".", ".."] {
            assert!(
                azure_openai_audio_url(AZ, bad, AzureAudioRoute::Speech, None).is_err(),
                "deployment {bad:?} must be refused"
            );
        }
        assert!(
            azure_openai_audio_url("not a url", "tts-1", AzureAudioRoute::Speech, None).is_err()
        );
    }

    #[test]
    fn azure_openai_is_neither_self_hosted_nor_azure_speech() {
        for v in ["azure_openai", "azure-openai", " Azure_OpenAI "] {
            assert!(is_azure_openai(v), "{v} is Azure OpenAI");
            // The handler's self-hosted branch sends a Bearer to `{api_base}/audio/...`; an
            // Azure deployment reaching it would 401 against the wrong URL.
            assert!(!is_self_hosted(v), "{v} must not take the self-hosted path");
        }
        for v in ["azure", "microsoft-azure", "openai", "self_hosted", ""] {
            assert!(!is_azure_openai(v), "{v} is not Azure OpenAI");
        }
    }

    #[test]
    fn an_azure_request_carries_api_key_and_no_authorization() {
        let _env = crate::core::net::ssrf_env_lock();
        let tts = SelfHostedTTS::new_azure_openai(azure_config(AZ), Some("2025-03-01-preview"))
            .expect("a public https Azure endpoint constructs");
        let req = tts
            .request_builder
            .build_http_request(&reqwest::Client::new(), "hello")
            .build()
            .unwrap();

        assert_eq!(
            req.url().as_str(),
            "https://bud-test.openai.azure.com/openai/deployments/gpt-4o-mini-tts/audio/speech\
             ?api-version=2025-03-01-preview"
        );
        assert_eq!(
            req.headers()
                .get(AZURE_OPENAI_API_KEY_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some("az-test-key")
        );
        assert!(
            req.headers().get(reqwest::header::AUTHORIZATION).is_none(),
            "Azure OpenAI takes `api-key`; a Bearer would put the key on the wire twice"
        );
        // OpenAI's body, with the deployment as `model`.
        let body: serde_json::Value =
            serde_json::from_slice(req.body().and_then(|b| b.as_bytes()).unwrap()).unwrap();
        assert_eq!(body["model"], "gpt-4o-mini-tts");
        assert_eq!(body["input"], "hello");
        assert_eq!(body["voice"], "alloy");

        assert_eq!(tts.get_provider_info()["provider"], "azure_openai");
    }

    #[test]
    fn the_api_version_comes_from_extras_and_nothing_else_does() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut cfg = StandardTTSConfig::from_base(azure_config(AZ));
        let tts = SelfHostedTTS::azure_openai_from_standard(&cfg).unwrap();
        assert!(
            tts.request_builder
                .target_url()
                .ends_with("?api-version=2025-04-01-preview"),
            "no extras: the default api-version"
        );

        cfg.extras.0.insert(
            AZURE_OPENAI_API_VERSION_EXTRA.to_string(),
            json!("2025-03-01-preview"),
        );
        // Never honoured for this vendor: the target is the deployment URL budapp published.
        cfg.extras.0.insert(
            crate::core::stt::standard::ENDPOINT_OVERRIDE_KEY.to_string(),
            json!("https://elsewhere.example.com"),
        );
        let tts = SelfHostedTTS::azure_openai_from_standard(&cfg).unwrap();
        assert_eq!(
            tts.request_builder.target_url(),
            "https://bud-test.openai.azure.com/openai/deployments/gpt-4o-mini-tts/audio/speech\
             ?api-version=2025-03-01-preview"
        );
    }

    #[test]
    fn an_azure_endpoint_must_be_public_https() {
        let _env = crate::core::net::ssrf_env_lock();
        for base in [
            "https://127.0.0.1",
            "https://127.0.0.1:8443/",
            "https://localhost:8443",
            "https://[::1]",
            "https://169.254.169.254",
            "https://10.0.0.5",
            "http://bud-test.openai.azure.com",
            "ftp://bud-test.openai.azure.com",
        ] {
            let err = match SelfHostedTTS::new_azure_openai(azure_config(base), None) {
                Ok(_) => panic!("{base} must be refused"),
                Err(e) => e,
            };
            assert!(
                err.to_string().contains("SSRF protection"),
                "{base}: the refusal must name the SSRF guard, got: {err}"
            );
        }
        assert!(SelfHostedTTS::new_azure_openai(azure_config(AZ), None).is_ok());
    }

    #[test]
    fn an_azure_endpoint_without_base_key_or_deployment_is_refused_by_name() {
        let refused = |cfg: TTSConfig| match SelfHostedTTS::new_azure_openai(cfg, None) {
            Ok(_) => panic!("must be refused"),
            Err(e) => e.to_string(),
        };
        let mut no_base = azure_config(AZ);
        no_base.api_base = None;
        assert!(refused(no_base).contains("api_base"));

        let mut no_key = azure_config(AZ);
        no_key.api_key = "  ".into();
        assert!(refused(no_key).contains("API key"));

        let mut no_deployment = azure_config(AZ);
        no_deployment.model = String::new();
        assert!(refused(no_deployment).contains("deployment"));
    }
}

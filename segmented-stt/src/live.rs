//! Planning one segmented session: from the resolved capability row and the deployment to a
//! working transcriber, the engine's profile, the limiter's rate and the quality policy.
//!
//! The gateway supplies only what it alone knows (the credential, a deployment's address, the
//! shared HTTP clients); every vendor default and every dialect choice is made here, from the row.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::map::{ContextKind, LanguageShape, QualitySignal, RateMetric, RatePer, Transport};
use crate::profile::{EndpointTuning, SegmentProfile, UploadPolicy};
use crate::transcriber::SegmentTranscriber;
use crate::transcriber::gate::LimitSpec;
use crate::transcriber::http::UploadClients;
use crate::transcriber::quality::QualityPolicy;
use crate::transcriber::wire::{self, Auth, RowLimits};
use crate::types::InterimMode;

/// Who and where to call, as the session knows it.
#[derive(Clone, Default)]
pub struct TargetSpec {
    /// The canonical provider id.
    pub provider: String,
    /// The model id that reaches the vendor (an Azure OpenAI deployment name for that vendor).
    pub model: String,
    pub api_key: String,
    /// A deployment's address (self-hosted, Azure OpenAI, WaaV Infer, a vendor base override).
    pub api_base: Option<String>,
    /// A full endpoint URL the gateway already built and checked; wins over `api_base`.
    pub url: Option<String>,
    /// The address came from a Bud deployment record, so the in-cluster client may be used.
    pub trusted: bool,
    pub region: Option<String>,
    pub api_version: Option<String>,
    /// Vendor parameters a deployment carries (`project`, `location`, `recognizer`, …).
    pub extras: BTreeMap<String, String>,
}

impl std::fmt::Debug for TargetSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TargetSpec")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("api_base", &self.api_base)
            .field("url", &self.url)
            .field("trusted", &self.trusted)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

/// The adapters this gateway can build. Google `Recognize` needs an OAuth token the gateway does
/// not mint for this path yet, so it is built only when the deployment supplies one.
pub fn adapter_built(adapter: &str, spec: &TargetSpec) -> bool {
    match adapter {
        "google_recognize" => {
            spec.extras.contains_key("access_token") && spec.extras.contains_key("project")
        }
        a => wire::built_adapters().contains(&a),
    }
}

fn join(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    // `OPENAI_BASE_URL`-style bases often end in `/v1` already: never `/v1/v1`.
    let path = if base.ends_with("/v1") {
        path.strip_prefix("/v1").unwrap_or(path)
    } else {
        path
    };
    format!("{base}{path}")
}

fn limits_of(t: &Transport) -> RowLimits {
    RowLimits {
        min_audio_ms: t.limits.min_audio_ms.map(|v| v as u32),
        max_audio_ms: t.limits.max_audio_ms.map(|v| v as u32),
        max_upload_bytes: t.limits.max_upload_bytes,
        single_process_server: t.segment_profile.as_ref().and_then(|p| p.max_in_flight) == Some(1)
            && matches!(t.dialect.auth, Some(crate::map::Auth::BearerOptional)),
    }
}

fn context_param(t: &Transport, kinds: &[ContextKind]) -> Option<String> {
    t.dialect
        .context_params
        .iter()
        .find(|c| kinds.contains(&c.kind))
        .map(|c| c.wire_name.clone())
}

fn has_signal(t: &Transport, s: QualitySignal) -> bool {
    t.quality_signals.contains(&s)
}

/// The OpenAI-compatible family (OpenAI, Groq, Azure OpenAI, self-hosted servers, WaaV Infer).
fn openai_compat(
    t: &Transport,
    adapter: &str,
    spec: &TargetSpec,
    clients: &UploadClients,
) -> Result<Arc<dyn SegmentTranscriber>, String> {
    let model = spec.model.trim();
    let lower = model.to_ascii_lowercase();
    let url = match (&spec.url, adapter) {
        (Some(u), _) => u.clone(),
        (None, "groq_transcriptions") => join(
            spec.api_base
                .as_deref()
                .unwrap_or("https://api.groq.com/openai"),
            "/v1/audio/transcriptions",
        ),
        (None, "azure_openai_transcriptions") => {
            let base = spec
                .api_base
                .as_deref()
                .ok_or("an Azure OpenAI deployment needs its resource endpoint")?;
            format!(
                "{}/openai/deployments/{}/audio/transcriptions?api-version={}",
                base.trim_end_matches('/'),
                model,
                spec.api_version.as_deref().unwrap_or("2025-03-01-preview")
            )
        }
        (None, _) if spec.provider == "openai" => join(
            spec.api_base.as_deref().unwrap_or("https://api.openai.com"),
            t.dialect
                .path
                .as_deref()
                .unwrap_or("/v1/audio/transcriptions"),
        ),
        (None, _) => {
            let base = spec
                .api_base
                .as_deref()
                .ok_or("a self-hosted deployment needs its address")?;
            let path = t
                .dialect
                .path
                .as_deref()
                .unwrap_or("/v1/audio/transcriptions");
            let base = base.trim_end_matches('/');
            if base.ends_with("/v1") || path.starts_with("/v1") {
                join(base, path)
            } else {
                format!("{base}/v1{path}")
            }
        }
    };
    let auth = if adapter == "azure_openai_transcriptions" {
        Auth::AzureApiKey(spec.api_key.clone())
    } else {
        Auth::Bearer(spec.api_key.clone())
    };
    let language = match (&t.dialect.language_param, t.dialect.language_shape) {
        (Some(p), Some(LanguageShape::List)) => wire::openai_compat::LanguageDialect::list(p),
        (Some(p), _) => wire::openai_compat::LanguageDialect::single(p),
        // A row with no recorded dialect (an unknown model): the field every server knows.
        (None, _) if lower == "gpt-transcribe" => {
            wire::openai_compat::LanguageDialect::list("languages")
        }
        (None, _) => wire::openai_compat::LanguageDialect::single("language"),
    };
    let mut prompt_param = context_param(t, &[ContextKind::Prompt]);
    let mut keywords_param = context_param(t, &[ContextKind::Keywords, ContextKind::Keyterms]);
    if t.dialect.context_params.is_empty() {
        prompt_param = Some("prompt".into());
        if lower == "gpt-transcribe" {
            keywords_param = Some("keywords".into());
        }
    }
    let formats = &t.dialect.response_formats;
    let mut extra_fields = Vec::new();
    let response_format = if has_signal(t, QualitySignal::SegmentNoSpeechProb)
        && (formats.is_empty() || formats.iter().any(|f| f == "verbose_json"))
    {
        if adapter == "groq_transcriptions" {
            extra_fields.push((
                "timestamp_granularities[]".to_string(),
                "segment".to_string(),
            ));
        }
        Some("verbose_json".to_string())
    } else if has_signal(t, QualitySignal::TokenLogprobs) {
        extra_fields.push(("include[]".to_string(), "logprobs".to_string()));
        Some("json".to_string())
    } else {
        formats
            .iter()
            .find(|f| f.as_str() == "json")
            .cloned()
            .or_else(|| Some("json".into()))
    };
    let tr = wire::openai_compat::OpenAiCompatTranscriber::new(
        wire::openai_compat::OpenAiCompatConfig {
            adapter: adapter.to_string(),
            url: url.clone(),
            auth,
            model: model.to_string(),
            language,
            prompt_param,
            keywords_param,
            response_format,
            extra_fields,
            client: clients.client_for(&url, spec.trusted).clone(),
            limits: limits_of(t),
        },
    )?;
    Ok(Arc::new(tr))
}

/// Build the transcriber for one resolved file transport.
pub fn build_transcriber(
    t: &Transport,
    spec: &TargetSpec,
    clients: &UploadClients,
) -> Result<Arc<dyn SegmentTranscriber>, String> {
    let adapter = t.adapter.as_str();
    // A base the client named on a standalone session is checked; a Bud deployment's is trusted.
    if !spec.trusted
        && let Some(base) = spec.url.as_deref().or(spec.api_base.as_deref())
    {
        crate::transcriber::http::check_untrusted_base(base, clients.public_may_reach_private())?;
    }
    match adapter {
        "openai_transcriptions" | "groq_transcriptions" | "azure_openai_transcriptions" => {
            openai_compat(t, adapter, spec, clients)
        }
        "elevenlabs_batch" => {
            let mut c = wire::elevenlabs::ElevenLabsConfig::new(
                Auth::elevenlabs(spec.api_key.clone()),
                &spec.model,
                clients.client(spec.trusted).clone(),
            );
            if let Some(b) = spec.url.clone().or_else(|| spec.api_base.clone()) {
                c.base_url = b;
            }
            let row = limits_of(t);
            c.limits.max_audio_ms = row.max_audio_ms.or(c.limits.max_audio_ms);
            c.limits.max_upload_bytes = row.max_upload_bytes.or(c.limits.max_upload_bytes);
            Ok(Arc::new(wire::elevenlabs::ElevenLabsTranscriber::new(c)?))
        }
        "deepgram_prerecorded" => {
            let base = spec
                .api_base
                .as_deref()
                .unwrap_or("https://api.deepgram.com");
            let mut c = wire::deepgram::DeepgramPrerecordedConfig::for_model(
                base,
                Auth::deepgram(spec.api_key.clone()),
                &spec.model,
                clients.client(spec.trusted).clone(),
            );
            c.limits = limits_of(t);
            Ok(Arc::new(
                wire::deepgram::DeepgramPrerecordedTranscriber::new(c)?,
            ))
        }
        "assemblyai_sync" => {
            let mut c = wire::assemblyai::AssemblyAiSyncConfig::new(
                Auth::assemblyai(spec.api_key.clone()),
                clients.client(spec.trusted).clone(),
            );
            if !spec.model.trim().is_empty() {
                c.model = spec.model.clone();
            }
            if let Some(b) = spec.api_base.clone() {
                c.base_url = b;
            } else if spec
                .region
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case("eu"))
            {
                c.base_url = "https://sync.eu.assemblyai.com".into();
            }
            let row = limits_of(t);
            c.limits.max_audio_ms = row.max_audio_ms.or(c.limits.max_audio_ms);
            Ok(Arc::new(wire::assemblyai::AssemblyAiSyncTranscriber::new(
                c,
            )?))
        }
        "azure_fast_transcription" => {
            let endpoint =
                match (&spec.api_base, &spec.region) {
                    (Some(b), _) => b.clone(),
                    (None, Some(r)) => format!("https://{r}.api.cognitive.microsoft.com"),
                    (None, None) => return Err(
                        "Azure fast transcription needs the Speech resource's region or endpoint"
                            .into(),
                    ),
                };
            let auth = Auth::azure_speech(spec.api_key.clone());
            let client = clients.client(spec.trusted).clone();
            let m = spec.model.trim();
            let mut c = if m.to_ascii_lowercase().starts_with("mai-transcribe") {
                wire::azure_fast::AzureFastConfig::mai(&endpoint, auth, m, client)
            } else if m.eq_ignore_ascii_case("llm-speech") {
                wire::azure_fast::AzureFastConfig::llm_speech(&endpoint, auth, client)
            } else {
                wire::azure_fast::AzureFastConfig::default_model(&endpoint, auth, client)
            };
            if let Some(v) = spec.api_version.clone() {
                c.api_version = v;
            }
            c.limits = limits_of(t);
            Ok(Arc::new(wire::azure_fast::AzureFastTranscriber::new(c)?))
        }
        "google_recognize" => {
            let token = spec
                .extras
                .get("access_token")
                .cloned()
                .ok_or("Google Recognize needs an access token")?;
            let project = spec
                .extras
                .get("project")
                .cloned()
                .ok_or("Google Recognize needs a project")?;
            let location = spec
                .extras
                .get("location")
                .cloned()
                .unwrap_or_else(|| "global".into());
            let mut c = wire::google_recognize::GoogleRecognizeConfig::new(
                &project,
                &location,
                &spec.model,
                &token,
                clients.client(spec.trusted).clone(),
            );
            if let Some(b) = spec.api_base.clone() {
                c.base_url = b;
            }
            if let Some(r) = spec.extras.get("recognizer") {
                c.recognizer = r.clone();
            }
            Ok(Arc::new(
                wire::google_recognize::GoogleRecognizeTranscriber::new(c)?,
            ))
        }
        other => Err(format!("no gateway client builds the adapter '{other}'")),
    }
}

/// The engine's profile: gateway defaults, the row's segment profile, the session's tuning.
pub fn profile_for(
    t: &Transport,
    tuning: &EndpointTuning,
    interim_results: Option<bool>,
) -> SegmentProfile {
    let mut p = SegmentProfile::default();
    if let Some(sp) = &t.segment_profile {
        if let Some(v) = sp.pre_roll_ms {
            p.pre_roll_ms = v;
        }
        if let Some(v) = sp.trailing_silence_ms {
            p.trailing_silence_ms = v;
        }
        if let Some(v) = sp.max_segment_ms {
            p.max_segment_ms = v.min(p.max_segment_ms);
        }
        if let Some(v) = sp.max_in_flight {
            p.max_in_flight = v.max(1) as usize;
        }
        if let Some(policy) = sp.upload_policy {
            p.upload_policy = match policy {
                crate::map::UploadPolicy::PerTurn => UploadPolicy::PerTurn,
                crate::map::UploadPolicy::PerPause => UploadPolicy::PerPause,
            };
        }
    }
    // Real audio plus the zeros must fit the vendor's longest upload.
    if let Some(max) = t.limits.max_audio_ms {
        let room = (max as u32).saturating_sub(p.trailing_silence_ms + 1000);
        if room >= 5000 {
            p.max_segment_ms = p.max_segment_ms.min(room);
        }
    }
    if let Some(min) = t.limits.min_audio_ms {
        p.min_commit_audio_ms = p.min_commit_audio_ms.max(min as u32);
    }
    p = p.with_tuning(tuning);
    if interim_results == Some(false) || p.upload_policy == UploadPolicy::PerTurn {
        p.interims = InterimMode::Off;
    }
    p
}

/// The limiter's rate: 80% of the row's request limit for the assumed plan.
pub fn limit_spec(t: &Transport) -> LimitSpec {
    let plan = t.limits.assumed_plan.as_deref();
    let applies = |r: &&crate::map::RateLimit| {
        r.plan.is_none() || plan.is_none() || r.plan.as_deref() == plan
    };
    let rpm = t
        .limits
        .rates
        .iter()
        .filter(applies)
        .filter(|r| r.metric == RateMetric::Requests)
        .filter_map(|r| match r.per {
            RatePer::Second => Some(r.value.saturating_mul(60)),
            RatePer::Minute => Some(r.value),
            RatePer::Hour => Some(r.value / 60),
            _ => None,
        })
        .min();
    let concurrent = t
        .limits
        .rates
        .iter()
        .filter(applies)
        .filter(|r| matches!(r.metric, RateMetric::ConcurrentRequests))
        .map(|r| r.value as usize)
        .min()
        .unwrap_or(64);
    match rpm {
        Some(v) if v > 0 => LimitSpec::from_rpm(v.min(u32::MAX as u64) as u32, concurrent),
        _ => LimitSpec {
            max_concurrent: concurrent,
            ..LimitSpec::unlimited()
        },
    }
}

/// The quality policy: the no-speech signal only where the row trusts it.
pub fn quality_policy(t: &Transport, prompt: Option<&str>) -> QualityPolicy {
    QualityPolicy {
        no_speech_signal: has_signal(t, QualitySignal::SegmentNoSpeechProb),
        sent_prompt: prompt.map(str::to_string).filter(|p| !p.trim().is_empty()),
        ..QualityPolicy::default()
    }
}

/// The row's published end-of-speech-to-final P99, the latency store's seed.
pub fn seed_p99(t: &Transport) -> Option<u32> {
    t.latency
        .measurements
        .iter()
        .find(|m| {
            m.percentile == crate::map::Percentile::P99
                && m.quantity == crate::map::Quantity::EndOfSpeechToFinal
        })
        .map(|m| m.value_ms as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::CapabilityMap;
    use crate::resolve::{ResolveRequest, resolve};

    fn clients() -> UploadClients {
        UploadClients::new(&crate::transcriber::http::HttpSettings::default()).unwrap()
    }

    fn transport(provider: &str, model: &str) -> Transport {
        let map = CapabilityMap::embedded();
        let mut req = ResolveRequest::new(provider, model);
        req.release = 6;
        req.covered = true;
        req.bud_leg = matches!(provider, "self_hosted" | "azure_openai" | "waav-infer");
        let r = resolve(map, &req);
        r.transport
            .unwrap_or_else(|| panic!("{provider}/{model}: {:?}", r.refusal))
            .transport
    }

    fn spec(provider: &str, model: &str) -> TargetSpec {
        TargetSpec {
            provider: provider.into(),
            model: model.into(),
            api_key: "k".into(),
            ..Default::default()
        }
    }

    #[test]
    fn every_release_one_vendor_builds_a_transcriber() {
        for (p, m) in [
            ("openai", "gpt-transcribe"),
            ("openai", "whisper-1"),
            ("groq", "whisper-large-v3-turbo"),
            ("elevenlabs", "scribe_v2"),
        ] {
            let t = transport(p, m);
            let tr = build_transcriber(&t, &spec(p, m), &clients()).unwrap();
            assert_eq!(tr.info().adapter, t.adapter, "{p}/{m}");
        }
        let t = transport("self_hosted", "whisper-large-v3");
        let s = TargetSpec {
            api_base: Some("http://whisper.svc:8000/v1".into()),
            trusted: true,
            ..spec("self_hosted", "whisper-large-v3")
        };
        let tr = build_transcriber(&t, &s, &clients()).unwrap();
        assert_eq!(tr.info().host_key, "http://whisper.svc:8000");
        let t = transport("azure_openai", "my-transcriber");
        let s = TargetSpec {
            api_base: Some("https://res.openai.azure.com".into()),
            ..spec("azure_openai", "my-transcriber")
        };
        assert_eq!(
            build_transcriber(&t, &s, &clients())
                .unwrap()
                .info()
                .adapter,
            "azure_openai_transcriptions"
        );
    }

    #[test]
    fn a_base_the_client_named_is_checked_and_a_deployments_is_trusted() {
        for (p, m) in [
            ("openai", "gpt-transcribe"),
            ("elevenlabs", "scribe_v2"),
            ("deepgram", "whisper-large"),
            ("assemblyai", "universal-3-5-pro"),
        ] {
            let t = transport(p, m);
            let named = TargetSpec {
                api_base: Some("http://127.0.0.1:9".into()),
                trusted: false,
                ..spec(p, m)
            };
            let err = build_transcriber(&t, &named, &clients())
                .err()
                .unwrap_or_else(|| panic!("{p}: a loopback base was accepted"));
            assert!(
                err.contains("private") || err.contains("loopback"),
                "{p}: {err}"
            );
            let deployment = TargetSpec {
                trusted: true,
                ..named.clone()
            };
            assert!(
                build_transcriber(&t, &deployment, &clients()).is_ok(),
                "{p}"
            );
        }
    }

    #[test]
    fn later_vendors_build_too() {
        for (p, m) in [
            ("deepgram", "whisper-large"),
            ("assemblyai", "universal-3-5-pro"),
        ] {
            let t = transport(p, m);
            assert!(
                build_transcriber(&t, &spec(p, m), &clients()).is_ok(),
                "{p}/{m}"
            );
        }
    }

    #[test]
    fn a_base_ending_in_v1_never_becomes_v1_v1() {
        assert_eq!(
            join("https://api.openai.com/v1", "/v1/audio/transcriptions"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            join("https://api.openai.com", "/v1/audio/transcriptions"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn a_self_hosted_deployment_without_an_address_is_refused() {
        let t = transport("self_hosted", "whisper-large-v3");
        assert!(
            build_transcriber(&t, &spec("self_hosted", "whisper-large-v3"), &clients()).is_err()
        );
    }

    #[test]
    fn google_is_built_only_with_a_token_and_a_project() {
        assert!(!adapter_built("google_recognize", &TargetSpec::default()));
        let mut s = TargetSpec::default();
        s.extras.insert("access_token".into(), "t".into());
        s.extras.insert("project".into(), "p".into());
        assert!(adapter_built("google_recognize", &s));
        assert!(adapter_built(
            "openai_transcriptions",
            &TargetSpec::default()
        ));
        assert!(!adapter_built("regional_rest", &TargetSpec::default()));
    }

    #[test]
    fn azure_openai_at_default_quota_uploads_once_per_turn_without_interims() {
        let t = transport("azure_openai", "my-transcriber");
        let p = profile_for(&t, &EndpointTuning::default(), None);
        assert_eq!(p.upload_policy, UploadPolicy::PerTurn);
        assert_eq!(p.max_in_flight, 1);
        assert_eq!(p.interims, InterimMode::Off);
    }

    #[test]
    fn the_limiter_targets_eighty_percent_of_the_groq_rate() {
        let t = transport("groq", "whisper-large-v3-turbo");
        let l = limit_spec(&t);
        assert!(
            l.requests_per_minute < 400.0 && l.requests_per_minute > 100.0,
            "{l:?}"
        );
    }

    #[test]
    fn interims_can_be_turned_off_by_the_client() {
        let t = transport("openai", "gpt-transcribe");
        assert_eq!(
            profile_for(&t, &EndpointTuning::default(), Some(false)).interims,
            InterimMode::Off
        );
        assert_eq!(
            profile_for(&t, &EndpointTuning::default(), None).interims,
            InterimMode::PerSegment
        );
    }

    #[test]
    fn the_elevenlabs_seed_is_its_published_p99() {
        assert_eq!(seed_p99(&transport("elevenlabs", "scribe_v2")), Some(2010));
        assert_eq!(seed_p99(&transport("openai", "gpt-transcribe")), None);
    }
}

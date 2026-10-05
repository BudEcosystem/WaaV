//! Planning one segmented session: from the resolved capability row and the deployment to a
//! working transcriber, the engine's profile, the limiter's rate and the quality policy.
//!
//! The gateway supplies only what it alone knows (the credential, a deployment's address, the
//! shared HTTP clients); every vendor default and every dialect choice is made here, from the row.

use crate::vendor::{azure_openai, hosts};
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
    /// The deployment's data region (`stt.data_region: eu`, Release 5). Kept apart from `region`,
    /// which for some vendors names a resource's host.
    pub data_region: Option<String>,
    /// The deployment asked the vendor to keep nothing (`data_retention: none`, Release 5): sent
    /// where the vendor has a request switch (ElevenLabs logging, Deepgram's improvement programme).
    pub no_retention: bool,
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

/// The deployment's region is the EU (`data_region: eu`, or a vendor `region: eu`).
fn is_eu(spec: &TargetSpec) -> bool {
    [&spec.data_region, &spec.region].into_iter().any(|r| {
        r.as_deref()
            .is_some_and(|r| r.trim().eq_ignore_ascii_case("eu"))
    })
}

/// A deployment data setting a transport cannot carry (Release 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnappliedSetting {
    pub setting: &'static str,
    pub reason: &'static str,
}

/// A resource region inside the EU: Azure and Google name their hosts by it.
fn eu_resource_region(region: &str) -> bool {
    const EU: &[&str] = &[
        "europe",
        "france",
        "germany",
        "sweden",
        "poland",
        "italy",
        "spain",
        "austria",
        "belgium",
        "denmark",
        "finland",
        "greece",
        "ireland",
        "netherlands",
    ];
    let r = region.trim().to_ascii_lowercase();
    r == "eu" || r.starts_with("eu-") || EU.iter().any(|c| r.contains(c))
}

/// The deployment's data settings this transport cannot carry. EU processing needs the vendor's
/// EU address, an EU resource region or the deployment's own address; no retention needs a
/// request switch (ElevenLabs logging, Deepgram's improvement programme) or the operator's own
/// server. The caller refuses the session rather than serve it elsewhere.
pub fn unapplied_data_settings(adapter: &str, spec: &TargetSpec) -> Vec<UnappliedSetting> {
    let own_address = spec.api_base.is_some() || spec.url.is_some();
    let own_server = own_address
        && matches!(
            adapter,
            "openai_transcriptions" | "openai_realtime_transcription"
        )
        && spec.provider != "openai";
    let mut out = Vec::new();
    if is_eu(spec) {
        let carried = own_address
            || match adapter {
                "elevenlabs_batch" | "deepgram_prerecorded" | "assemblyai_sync" => true,
                "openai_transcriptions" => spec.provider == "openai",
                "azure_fast_transcription" => {
                    spec.region.as_deref().is_some_and(eu_resource_region)
                }
                "google_recognize" => spec
                    .extras
                    .get("location")
                    .is_some_and(|l| eu_resource_region(l)),
                _ => false,
            };
        if !carried {
            out.push(UnappliedSetting {
                setting: "stt.data_region",
                reason: "no_eu_address",
            });
        }
    }
    if spec.no_retention
        && !own_server
        && !matches!(adapter, "elevenlabs_batch" | "deepgram_prerecorded")
    {
        out.push(UnappliedSetting {
            setting: "stt.data_retention",
            reason: "no_request_switch",
        });
    }
    out
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
            spec.api_base.as_deref().unwrap_or(hosts::GROQ_OPENAI),
            "/v1/audio/transcriptions",
        ),
        (None, "azure_openai_transcriptions") => {
            let base = spec
                .api_base
                .as_deref()
                .ok_or("an Azure OpenAI deployment needs its resource endpoint")?;
            azure_openai::audio_url(
                base,
                model,
                azure_openai::AudioRoute::Transcriptions,
                spec.api_version.as_deref(),
            )?
        }
        (None, _) if spec.provider == "openai" => join(
            spec.api_base.as_deref().unwrap_or(if is_eu(spec) {
                hosts::OPENAI_EU
            } else {
                hosts::OPENAI
            }),
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

/// The realtime transcription socket's address: the deployment's declared `realtime_url`, or the
/// vendor's from the base address (`https` becomes `wss`).
fn realtime_url(spec: &TargetSpec) -> Result<String, String> {
    if let Some(u) = spec
        .extras
        .get("realtime_url")
        .filter(|u| !u.trim().is_empty())
    {
        return Ok(u.trim().to_string());
    }
    let to_ws = |b: &str| {
        b.trim_end_matches('/')
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1)
    };
    match spec.provider.as_str() {
        "azure_openai" => {
            let base = spec
                .api_base
                .as_deref()
                .ok_or("an Azure OpenAI deployment needs its resource endpoint")?;
            Ok(format!(
                "{}/openai/v1/realtime?intent=transcription",
                to_ws(base)
            ))
        }
        "openai" => Ok(join(
            &to_ws(spec.api_base.as_deref().unwrap_or(hosts::OPENAI)),
            "/v1/realtime?intent=transcription",
        )),
        _ => Err("a self-hosted realtime transcription server needs its realtime_url".into()),
    }
}

fn openai_realtime(
    t: &Transport,
    spec: &TargetSpec,
) -> Result<Arc<dyn SegmentTranscriber>, String> {
    // A socket bypasses the upload pool's address rules, so it is opened only to the vendor's own
    // host, a Bud deployment's address or the operator's: never to an address a client named.
    if !spec.trusted && (spec.api_base.is_some() || spec.extras.contains_key("realtime_url")) {
        return Err(
            "a realtime transcription socket is opened only to a deployment's or the operator's address"
                .into(),
        );
    }
    let url = realtime_url(spec)?;
    let auth = if spec.provider == "azure_openai" {
        Auth::AzureApiKey(spec.api_key.clone())
    } else {
        Auth::Bearer(spec.api_key.clone())
    };
    let mut cfg = wire::OpenAiRealtimeConfig::new(&url, auth, spec.model.trim());
    cfg.language = match (&t.dialect.language_param, t.dialect.language_shape) {
        (Some(p), Some(LanguageShape::List)) => wire::openai_compat::LanguageDialect::list(p),
        (Some(p), _) => wire::openai_compat::LanguageDialect::single(p),
        (None, _) => wire::openai_compat::LanguageDialect::single("language"),
    };
    cfg.send_prompt = context_param(t, &[ContextKind::Prompt]).is_some();
    cfg.send_keywords = context_param(t, &[ContextKind::Keywords, ContextKind::Keyterms]).is_some();
    Ok(Arc::new(wire::OpenAiRealtimeTranscriber::new(cfg)?))
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
        "openai_realtime_transcription" => openai_realtime(t, spec),
        "cartesia_manual_finalize" => {
            if !spec.trusted && spec.api_base.is_some() {
                return Err(
                    "a Cartesia socket is opened only to a deployment's or the operator's address"
                        .into(),
                );
            }
            let mut c =
                wire::cartesia_finalize::CartesiaFinalizeConfig::new(&spec.api_key, &spec.model);
            if let Some(b) = spec.api_base.as_deref() {
                c.base = b
                    .trim_end_matches('/')
                    .replacen("https://", "wss://", 1)
                    .replacen("http://", "ws://", 1);
            }
            Ok(Arc::new(
                wire::cartesia_finalize::CartesiaFinalizeTranscriber::new(c)?,
            ))
        }
        "elevenlabs_batch" => {
            let mut c = wire::elevenlabs::ElevenLabsConfig::new(
                Auth::elevenlabs(spec.api_key.clone()),
                &spec.model,
                clients.client(spec.trusted).clone(),
            );
            if let Some(b) = spec.url.clone().or_else(|| spec.api_base.clone()) {
                c.base_url = b;
            } else if is_eu(spec) {
                c.base_url = hosts::ELEVENLABS_EU.into();
            }
            c.zero_retention = spec.no_retention;
            let row = limits_of(t);
            c.limits.max_audio_ms = row.max_audio_ms.or(c.limits.max_audio_ms);
            c.limits.max_upload_bytes = row.max_upload_bytes.or(c.limits.max_upload_bytes);
            Ok(Arc::new(wire::elevenlabs::ElevenLabsTranscriber::new(c)?))
        }
        "deepgram_prerecorded" => {
            let base = spec.api_base.as_deref().unwrap_or(if is_eu(spec) {
                hosts::DEEPGRAM_EU
            } else {
                hosts::DEEPGRAM
            });
            let mut c = wire::deepgram::DeepgramPrerecordedConfig::for_model(
                base,
                Auth::deepgram(spec.api_key.clone()),
                &spec.model,
                clients.client(spec.trusted).clone(),
            );
            c.limits = limits_of(t);
            c.mip_opt_out = spec.no_retention;
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
            } else if is_eu(spec) {
                c.base_url = hosts::ASSEMBLYAI_SYNC_EU.into();
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

/// The row's billing rule: the vendor's minimum per request and its increment.
pub fn billing_rule(billing: &crate::map::Billing) -> crate::types::BillingRule {
    crate::types::BillingRule {
        min_billed_ms: billing.min_billed_ms.unwrap_or(0).min(u32::MAX as u64) as u32,
        increment_ms: billing.increment_ms.unwrap_or(0).min(u32::MAX as u64) as u32,
    }
}

/// The row's hourly and daily limits on requests and audio, for the limiter's long windows.
pub fn long_windows(t: &Transport) -> Vec<crate::transcriber::gate::LongWindow> {
    use crate::transcriber::gate::{LongWindow, WindowMetric};
    let plan = t.limits.assumed_plan.as_deref();
    t.limits
        .rates
        .iter()
        .filter(|r| r.plan.is_none() || plan.is_none() || r.plan.as_deref() == plan)
        .filter_map(|r| {
            let span = match r.per {
                RatePer::Hour => std::time::Duration::from_secs(3_600),
                RatePer::Day => std::time::Duration::from_secs(86_400),
                _ => return None,
            };
            let metric = match r.metric {
                RateMetric::Requests => WindowMetric::Requests,
                RateMetric::AudioSeconds => WindowMetric::AudioSeconds,
                _ => return None,
            };
            Some(LongWindow::new(span, metric, r.value as f64))
        })
        .collect()
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

    /// The commit transport (Release 4): OpenAI's live-only models on its realtime socket.
    #[test]
    fn a_live_only_model_builds_a_commit_transcriber_on_the_vendor_socket() {
        let map = CapabilityMap::embedded();
        let row = map.row("openai:gpt-live-transcribe").expect("row");
        let t = row
            .transports
            .iter()
            .find_map(|e| match e {
                crate::map::TransportEntry::Inline(t)
                    if t.adapter.as_str() == "openai_realtime_transcription" =>
                {
                    Some((**t).clone())
                }
                _ => None,
            })
            .expect("commit transport");
        let tr = build_transcriber(&t, &spec("openai", "gpt-live-transcribe"), &clients()).unwrap();
        assert_eq!(tr.info().kind, crate::transcriber::TranscriberKind::Commit);
        assert_eq!(tr.info().host_key, "https://api.openai.com:443");
        // A client-named base never gets a socket; the operator's does.
        let named = TargetSpec {
            api_base: Some("https://asr.example.com".into()),
            ..spec("openai", "gpt-live-transcribe")
        };
        assert!(build_transcriber(&t, &named, &clients()).is_err());
        let operator = TargetSpec {
            trusted: true,
            ..named
        };
        assert_eq!(
            build_transcriber(&t, &operator, &clients())
                .unwrap()
                .info()
                .host_key,
            "https://asr.example.com:443"
        );
    }

    #[test]
    fn cartesia_builds_its_finalize_transport() {
        let map = CapabilityMap::embedded();
        let row = map.row("cartesia:ink-whisper").expect("row");
        let t = row
            .transports
            .iter()
            .find_map(|e| match e {
                crate::map::TransportEntry::Inline(t)
                    if t.adapter.as_str() == "cartesia_manual_finalize" =>
                {
                    Some((**t).clone())
                }
                _ => None,
            })
            .expect("finalize transport");
        let tr = build_transcriber(&t, &spec("cartesia", "ink-whisper"), &clients()).unwrap();
        assert_eq!(tr.info().kind, crate::transcriber::TranscriberKind::Commit);
        assert_eq!(tr.info().host_key, "https://api.cartesia.ai:443");
    }

    /// The gateway puts a deployment's `stt.data_region: eu` in `data_region`, not `region`, and the
    /// refusal check counts every upload family's EU address as applied: each must then use it.
    #[test]
    fn the_data_region_setting_reaches_every_vendors_eu_host() {
        let eu = |p: &str, m: &str| TargetSpec {
            data_region: Some("eu".into()),
            ..spec(p, m)
        };
        let host = |p: &str, m: &str| {
            build_transcriber(&transport(p, m), &eu(p, m), &clients())
                .unwrap()
                .info()
                .host_key
                .clone()
        };
        assert_eq!(
            host("elevenlabs", "scribe_v2"),
            "https://api.eu.residency.elevenlabs.io:443"
        );
        assert_eq!(
            host("deepgram", "whisper-large"),
            "https://api.eu.deepgram.com:443"
        );
        assert_eq!(
            host("openai", "gpt-transcribe"),
            "https://eu.api.openai.com:443"
        );
        assert_eq!(
            host("assemblyai", "universal-3-5-pro"),
            "https://sync.eu.assemblyai.com:443"
        );
    }

    /// Release 5: the canonical region and retention options reach each vendor's own switch.
    #[test]
    fn region_and_retention_reach_the_vendors_switches() {
        let eu = |p: &str, m: &str| TargetSpec {
            region: Some("eu".into()),
            no_retention: true,
            ..spec(p, m)
        };
        let host = |p: &str, m: &str| {
            build_transcriber(&transport(p, m), &eu(p, m), &clients())
                .unwrap()
                .info()
                .host_key
                .clone()
        };
        assert_eq!(
            host("elevenlabs", "scribe_v2"),
            "https://api.eu.residency.elevenlabs.io:443"
        );
        assert_eq!(
            host("deepgram", "whisper-large"),
            "https://api.eu.deepgram.com:443"
        );
        assert_eq!(
            host("openai", "gpt-transcribe"),
            "https://eu.api.openai.com:443"
        );
        assert_eq!(
            host("assemblyai", "universal-3-5-pro"),
            "https://sync.eu.assemblyai.com:443"
        );
        // A deployment's own address wins over the region.
        let own = TargetSpec {
            api_base: Some("https://proxy.example.com".into()),
            trusted: true,
            ..eu("deepgram", "whisper-large")
        };
        assert_eq!(
            build_transcriber(&transport("deepgram", "whisper-large"), &own, &clients())
                .unwrap()
                .info()
                .host_key,
            "https://proxy.example.com:443"
        );
        assert!(!host("elevenlabs", "scribe_v2").contains("api.elevenlabs.io"));
    }

    /// Release 5: the canonical `data_region` reaches the EU host without touching a vendor's own
    /// region (an Azure Speech resource's region still names its host).
    #[test]
    fn the_canonical_data_region_selects_the_eu_host() {
        let canonical = TargetSpec {
            data_region: Some("eu".into()),
            ..spec("deepgram", "whisper-large")
        };
        assert_eq!(
            build_transcriber(
                &transport("deepgram", "whisper-large"),
                &canonical,
                &clients()
            )
            .unwrap()
            .info()
            .host_key,
            "https://api.eu.deepgram.com:443"
        );
    }

    /// Release 5: a transport that cannot carry a deployment's data setting is named, so the
    /// session is refused rather than served from elsewhere or kept by the vendor.
    #[test]
    fn each_transport_names_the_data_settings_it_cannot_carry() {
        let both = |p: &str, m: &str| TargetSpec {
            data_region: Some("eu".into()),
            no_retention: true,
            ..spec(p, m)
        };
        let unapplied = |p: &str, m: &str, s: &TargetSpec| -> Vec<&'static str> {
            unapplied_data_settings(&transport(p, m).adapter, s)
                .into_iter()
                .map(|u| u.setting)
                .collect()
        };
        // Both carried: an EU host and a request switch.
        assert!(unapplied("elevenlabs", "scribe_v2", &both("elevenlabs", "scribe_v2")).is_empty());
        assert!(
            unapplied(
                "deepgram",
                "whisper-large",
                &both("deepgram", "whisper-large")
            )
            .is_empty()
        );
        // An EU host, but the vendor keeps audio by account, not by request.
        assert_eq!(
            unapplied(
                "openai",
                "gpt-transcribe",
                &both("openai", "gpt-transcribe")
            ),
            vec!["stt.data_retention"]
        );
        assert_eq!(
            unapplied(
                "assemblyai",
                "universal-3-5-pro",
                &both("assemblyai", "universal-3-5-pro")
            ),
            vec!["stt.data_retention"]
        );
        // Neither: no EU host and no switch.
        assert_eq!(
            unapplied(
                "groq",
                "whisper-large-v3",
                &both("groq", "whisper-large-v3")
            ),
            vec!["stt.data_region", "stt.data_retention"]
        );
        // Nothing asked, nothing named.
        assert!(
            unapplied(
                "groq",
                "whisper-large-v3",
                &spec("groq", "whisper-large-v3")
            )
            .is_empty()
        );
        // The operator's own server: its address decides where audio goes and what is kept.
        let own = TargetSpec {
            api_base: Some("https://asr.internal.example.com".into()),
            trusted: true,
            ..both("vllm", "whisper-large-v3")
        };
        assert!(unapplied_data_settings("openai_transcriptions", &own).is_empty());
        // A resource-region vendor: an EU resource region carries it, another does not.
        let azure = |region: &str| TargetSpec {
            region: Some(region.into()),
            data_region: Some("eu".into()),
            ..spec("azure", "mai-transcribe-1")
        };
        assert!(
            unapplied_data_settings("azure_fast_transcription", &azure("westeurope")).is_empty()
        );
        assert!(
            unapplied_data_settings("azure_fast_transcription", &azure("swedencentral")).is_empty()
        );
        assert_eq!(
            unapplied_data_settings("azure_fast_transcription", &azure("eastus"))
                .into_iter()
                .map(|u| u.setting)
                .collect::<Vec<_>>(),
            vec!["stt.data_region"]
        );
    }

    #[test]
    fn realtime_addresses_follow_the_vendor() {
        let mut s = spec("openai", "gpt-live-transcribe");
        assert_eq!(
            realtime_url(&s).unwrap(),
            "wss://api.openai.com/v1/realtime?intent=transcription"
        );
        s.api_base = Some("https://proxy.example.com/v1".into());
        assert_eq!(
            realtime_url(&s).unwrap(),
            "wss://proxy.example.com/v1/realtime?intent=transcription"
        );
        let mut a = spec("azure_openai", "my-live");
        a.api_base = Some("https://res.openai.azure.com/".into());
        assert_eq!(
            realtime_url(&a).unwrap(),
            "wss://res.openai.azure.com/openai/v1/realtime?intent=transcription"
        );
        let mut h = spec("self_hosted", "whisper");
        assert!(realtime_url(&h).is_err());
        h.extras.insert(
            "realtime_url".into(),
            "ws://asr.svc:8000/v1/realtime".into(),
        );
        assert_eq!(realtime_url(&h).unwrap(), "ws://asr.svc:8000/v1/realtime");
    }

    #[test]
    fn groq_has_long_windows_and_a_vendor_without_them_has_none() {
        let groq = long_windows(&transport("groq", "whisper-large-v3-turbo"));
        assert!(
            !groq.is_empty(),
            "Groq publishes hourly audio and daily request caps"
        );
        assert!(groq.iter().all(|w| w.limit > 0.0));
        assert!(
            long_windows(&transport("elevenlabs", "scribe_v2"))
                .iter()
                .all(|w| w.limit > 0.0)
        );
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

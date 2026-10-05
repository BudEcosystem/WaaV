//! Resolution of a live session against the capability map, and the plan of a segmented session.
//!
//! The capability map has one row per provider and model saying how a live call reaches it:
//! today's streaming client, an upload per utterance, or a named refusal. [`resolve_session`] reads
//! it once per session, before anything is built; [`build_plan`] turns a segmented resolution into
//! the engine's plan, which the voice manager's live factory starts.

use std::collections::BTreeMap;
use std::sync::Arc;

use waav_segmented_stt::endpointer::EndOfTurnTextModel;
use waav_segmented_stt::engine::EngineConfig;
use waav_segmented_stt::limits::{LatencyStore, effective_deadline_ms};
use waav_segmented_stt::live::{
    TargetSpec, adapter_built, build_transcriber, limit_spec, profile_for, quality_policy, seed_p99,
};
use waav_segmented_stt::map::{CapabilityMap, Transport};
use waav_segmented_stt::profile::{EndpointTuning, SegmentProfile, UploadPolicy};
use waav_segmented_stt::resolve::{
    Delivery, Evidence, LatencyTier, Outcome, Refusal, Resolution, ResolveRequest, SessionKind,
    TranscriptionMode, Warning, resolve,
};
use waav_segmented_stt::rollout::Rollout;
use waav_segmented_stt::sequencer::AttemptsUpload;
use waav_segmented_stt::transcriber::attempts::{
    RepairMemory, SecondRequestPolicy, SegmentAttempts, SessionHealth,
};
use waav_segmented_stt::transcriber::breaker::{BreakerConfig, BreakerRegistry};
use waav_segmented_stt::transcriber::gate::{BudgetRegistry, LimiterRegistry};
use waav_segmented_stt::transcriber::http::{HttpSettings, UploadClients};

use super::adapter::{GatewayClock, SegmentedPlan};
use super::models::{self, DetectorSupport};

/// Process-wide state of segmented speech-to-text, built once at start-up.
pub struct SttLiveShared {
    pub map: &'static CapabilityMap,
    pub rollout: Rollout,
    /// `WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR=1`: a build whose Silero model failed uses the
    /// loudness detector instead of refusing.
    pub allow_energy_detector: bool,
    /// `WAAV_STT_FILE_ONLY_REFUSAL=off` withdraws the refusals of voice agents on a buffering
    /// model (the warning stays).
    pub file_only_refusal: bool,
    /// `WAAV_STT_COMMIT_TRANSPORT=1` builds the commit transport (OpenAI's realtime transcription
    /// socket, Release 4) once a live probe has passed for the deployment's vendor; off, the
    /// live-only models stay refused by name and `gpt-transcribe` uploads files.
    pub commit_transport: bool,
    /// `WAAV_STT_CARTESIA_MANUAL_FINALIZE=1`: covered Cartesia sessions end utterances on the
    /// gateway's detector (`finalize`) instead of Cartesia's own, a deliberate change to a
    /// streaming session (addendum B8), off until a live probe passes.
    pub cartesia_finalize: bool,
    /// Tests only: the detector support every session sees, instead of the process-wide model
    /// state, which other tests in the same process change.
    pub detector_override: Option<models::DetectorSupport>,
    /// `WAAV_STT_SETUP_PROBE` (default on): probe a guessed row or a self-hosted server with one
    /// silent clip before the caller speaks.
    pub setup_probe: bool,
    pub probe_cache: Arc<waav_segmented_stt::transcriber::probe::ProbeCache>,
    pub latency: Arc<LatencyStore>,
    pub limiters: LimiterRegistry,
    pub breakers: BreakerRegistry,
    pub budgets: BudgetRegistry,
    pub repairs: Arc<RepairMemory>,
    pub clients: UploadClients,
    /// The control record, refreshed from Bud's Redis (see [`spawn_control_refresh`]).
    pub control: Arc<parking_lot::RwLock<waav_segmented_stt::control::ControlRecord>>,
    pub text_model: Option<Arc<dyn EndOfTurnTextModel>>,
}

impl std::fmt::Debug for SttLiveShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SttLiveShared")
            .field("map_version", &self.map.map_version())
            .field("rollout", &self.rollout.mode)
            .field("release", &self.rollout.release)
            .finish_non_exhaustive()
    }
}

fn parse_flag(name: &str, v: Option<String>, default: bool) -> Result<bool, String> {
    match v.as_deref().map(str::trim) {
        None | Some("") => Ok(default),
        Some(s) => crate::config::utils::parse_bool(s)
            .or(match s.to_ascii_lowercase().as_str() {
                "on" => Some(true),
                "off" => Some(false),
                _ => None,
            })
            .ok_or_else(|| format!("{name} must be a boolean (true/false, on/off), got {s:?}")),
    }
}

impl SttLiveShared {
    pub fn from_lookup(
        get: impl Fn(&str) -> Option<String>,
        turn_detector: Option<Arc<tokio::sync::RwLock<crate::core::turn_detect::TurnDetector>>>,
    ) -> Result<Self, String> {
        let rollout = Rollout::from_lookup(&get)?;
        let mut http = HttpSettings::from_lookup(&get)?;
        // The gateway's one escape hatch for loopback and private vendor addresses (local
        // development and tests) applies to segmented uploads too.
        http.public_may_reach_private = get("WAAV_ALLOW_LOOPBACK_ENDPOINTS").is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        });
        Ok(Self {
            map: CapabilityMap::embedded(),
            rollout,
            allow_energy_detector: parse_flag(
                "WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR",
                get("WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR"),
                false,
            )?,
            file_only_refusal: parse_flag(
                "WAAV_STT_FILE_ONLY_REFUSAL",
                get("WAAV_STT_FILE_ONLY_REFUSAL"),
                true,
            )?,
            commit_transport: parse_flag(
                "WAAV_STT_COMMIT_TRANSPORT",
                get("WAAV_STT_COMMIT_TRANSPORT"),
                false,
            )?,
            cartesia_finalize: parse_flag(
                "WAAV_STT_CARTESIA_MANUAL_FINALIZE",
                get("WAAV_STT_CARTESIA_MANUAL_FINALIZE"),
                false,
            )?,
            latency: Arc::new(LatencyStore::new()),
            limiters: LimiterRegistry::new(),
            breakers: BreakerRegistry::new(BreakerConfig::default()),
            budgets: BudgetRegistry::default(),
            repairs: Arc::new(RepairMemory::default()),
            clients: UploadClients::new(&http)?,
            detector_override: None,
            setup_probe: parse_flag("WAAV_STT_SETUP_PROBE", get("WAAV_STT_SETUP_PROBE"), true)?,
            probe_cache: Arc::default(),
            control: Arc::default(),
            text_model: turn_detector
                .map(|t| Arc::new(super::models::TextTurnModel(t)) as Arc<dyn EndOfTurnTextModel>),
        })
    }

    pub fn from_env(
        turn_detector: Option<Arc<tokio::sync::RwLock<crate::core::turn_detect::TurnDetector>>>,
    ) -> Result<Self, String> {
        Self::from_lookup(|k| std::env::var(k).ok(), turn_detector)
    }
}

/// What kind of session asks. It decides who ends a turn, and so whether a buffering client is
/// refused, kept or replaced (Addenda A1, B4, C1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveSessionKind {
    Agent { manual: bool },
    Conversation { turn_detection: bool },
    Dag,
    Plain,
}

impl LiveSessionKind {
    fn resolver_kind(self) -> SessionKind {
        match self {
            Self::Agent { manual: false } => SessionKind::Gateway,
            Self::Agent { manual: true }
            | Self::Dag
            | Self::Conversation {
                turn_detection: true,
            } => SessionKind::PushToTalk,
            Self::Conversation {
                turn_detection: false,
            }
            | Self::Plain => SessionKind::Plain,
        }
    }

    pub fn is_agent(self) -> bool {
        matches!(self, Self::Agent { .. })
    }
}

/// Which refusal site of a Bud leg asks: the codes for an uncovered self-hosted or Azure OpenAI
/// deployment are today's, per site, until Bud stops matching on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegSite {
    Agent,
    Named,
}

/// A Bud deployment leg: everything server-side about the STT leg.
#[derive(Debug, Clone)]
pub struct LiveLeg {
    pub name: String,
    pub id: String,
    pub site: LegSite,
    pub api_base: Option<String>,
    pub provider_params: BTreeMap<String, String>,
    pub segmented: Option<bud_auth::SttSegmented>,
    pub capability_override: Option<bud_auth::SttCapabilityOverride>,
    pub expected_languages: Vec<String>,
}

/// One session's request.
#[derive(Debug, Clone)]
pub struct LiveRequest {
    pub provider: String,
    pub model: String,
    pub language: String,
    pub encoding: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub kind: LiveSessionKind,
    /// The request's `transcription_mode`, as sent.
    pub requested_mode: Option<String>,
    pub leg: Option<LiveLeg>,
    pub interim_results: Option<bool>,
    pub keyterms: Vec<String>,
    pub prompt: Option<String>,
    pub tuning: EndpointTuning,
    /// `extras` the client sent (a standalone session's base URL or region).
    pub extras: BTreeMap<String, String>,
}

/// What the session gets.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveDecision {
    /// Today's client, unchanged.
    Native,
    /// The segmented engine.
    Segmented,
    /// A named refusal at setup.
    Refused(Refusal),
}

/// A fact for the client the resolver alone does not produce.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraWarning {
    pub code: &'static str,
    pub delivery: Delivery,
    pub detail: serde_json::Value,
}

/// The session's resolution and everything derived from it.
#[derive(Debug, Clone)]
pub struct LiveResolution {
    pub resolution: Resolution,
    pub decision: LiveDecision,
    pub covered: bool,
    pub mode: TranscriptionMode,
    /// `request`, `deployment` or `default`.
    pub mode_source: &'static str,
    pub latency_tier: LatencyTier,
    pub deadline_ms: u32,
    pub deadline_raised: bool,
    pub detector: DetectorSupport,
    pub extra: Vec<ExtraWarning>,
    pub kind: LiveSessionKind,
    pub deployment: Option<String>,
    pub language: Option<String>,
}

impl LiveResolution {
    /// The chosen transport, when the session is segmented.
    /// Whether the setup probe should check this session's target: a row the map only guessed
    /// (a pattern or a default), a self-hosted server, or a deployment's declared profile.
    pub fn needs_probe(&self) -> bool {
        self.decision == LiveDecision::Segmented
            && (!matches!(
                self.resolution.layer,
                waav_segmented_stt::resolve::Layer::Exact
            ) || matches!(
                self.resolution.provider.as_str(),
                "self_hosted" | "waav_infer"
            ))
    }

    pub fn transport(&self) -> Option<&Transport> {
        self.resolution.transport.as_ref().map(|t| &t.transport)
    }

    /// The frame-delivered warnings, in contract order.
    pub fn frame_warnings(&self) -> Vec<&Warning> {
        self.resolution
            .warnings
            .iter()
            .filter(|w| w.delivery == Delivery::Frame)
            .collect()
    }

    pub fn notices(&self) -> Vec<&Warning> {
        self.resolution
            .warnings
            .iter()
            .filter(|w| w.delivery == Delivery::Notice)
            .collect()
    }
}

fn parse_mode(s: &str) -> Option<TranscriptionMode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "auto" | "" => Some(TranscriptionMode::Auto),
        "streaming" => Some(TranscriptionMode::Streaming),
        "segmented" => Some(TranscriptionMode::Segmented),
        _ => None,
    }
}

fn language_of(raw: &str) -> Option<String> {
    let l = raw.trim();
    (!l.is_empty() && !l.eq_ignore_ascii_case("auto") && !l.eq_ignore_ascii_case("multi"))
        .then(|| l.to_string())
}

/// The target as the session knows it (no credential).
fn target_spec(req: &LiveRequest, provider: &str, model: &str) -> TargetSpec {
    let mut spec = TargetSpec {
        provider: provider.to_string(),
        model: model.to_string(),
        ..Default::default()
    };
    match &req.leg {
        Some(leg) => {
            // Every address of a Bud deployment comes from its record, never from a client.
            spec.api_base = leg.api_base.clone();
            spec.trusted = true;
            spec.region = leg.provider_params.get("region").cloned();
            spec.api_version = leg.provider_params.get("api_version").cloned();
            spec.extras = leg.provider_params.clone();
            if let Some(url) = leg
                .capability_override
                .as_ref()
                .and_then(|o| o.realtime_url.clone())
            {
                spec.extras.insert("realtime_url".into(), url);
            }
        }
        None => {
            // A base the client named is checked before any upload; the operator's
            // `OPENAI_BASE_URL` is trusted like a deployment record.
            let named = req
                .extras
                .get("base_url")
                .or_else(|| req.extras.get("api_base"))
                .cloned();
            let operator = (provider == "openai")
                .then(|| std::env::var("OPENAI_BASE_URL").ok())
                .flatten()
                .filter(|s| !s.trim().is_empty());
            spec.trusted = named.is_none() && operator.is_some();
            spec.api_base = named.or(operator);
            spec.region = req.extras.get("region").cloned();
            spec.api_version = req.extras.get("api_version").cloned();
            spec.extras = req.extras.clone();
        }
    }
    spec
}

/// Resolve one session. No I/O.
pub fn resolve_session(shared: &SttLiveShared, req: &LiveRequest) -> LiveResolution {
    let mut extra = Vec::new();
    let seg = req.leg.as_ref().and_then(|l| l.segmented.as_ref());
    let ovr = req
        .leg
        .as_ref()
        .and_then(|l| l.capability_override.as_ref());

    // The preference: the request's, then the deployment's, then the default. A voice agent's
    // own settings choose for it; a request preference there is ignored with a warning.
    let deployment_mode = seg
        .and_then(|s| s.transcription_mode.as_deref())
        .and_then(parse_mode);
    let (mut mode, mut mode_source) = match deployment_mode {
        Some(m) => (m, "deployment"),
        None => (TranscriptionMode::Auto, "default"),
    };
    if let Some(raw) = req.requested_mode.as_deref() {
        if req.kind.is_agent() {
            extra.push(ExtraWarning {
                code: "stt_transcription_mode_ignored",
                delivery: Delivery::Frame,
                detail: serde_json::json!({ "received": raw }),
            });
        } else {
            match parse_mode(raw) {
                Some(m) => {
                    mode = m;
                    mode_source = "request";
                }
                None => extra.push(ExtraWarning {
                    code: "stt_transcription_mode_invalid",
                    delivery: Delivery::Frame,
                    detail: serde_json::json!({ "received": raw, "effective": mode.as_str() }),
                }),
            }
        }
    }
    let latency_tier = match seg.and_then(|s| s.latency_tier.as_deref()) {
        Some(t) if t.eq_ignore_ascii_case("low_latency") => LatencyTier::LowLatency,
        _ => LatencyTier::Standard,
    };
    let deployment_name = req.leg.as_ref().map(|l| l.name.clone());
    let deployment_on = seg.and_then(|s| s.enabled) != Some(false);
    let covered = deployment_on
        && (shared.rollout.covers(
            req.leg.as_ref().map(|l| l.name.as_str()),
            &req.provider,
            &req.model,
        ) || req.leg.as_ref().is_some_and(|l| {
            shared
                .rollout
                .covers(Some(&l.id), &req.provider, &req.model)
        }));
    let tuning_profile = SegmentProfile::default().with_tuning(&req.tuning);
    let (deadline_ms, deadline_raised) = effective_deadline_ms(
        seg.and_then(|s| s.deadline_ms),
        tuning_profile.max_endpointing_ms,
    );
    if deadline_raised {
        extra.push(ExtraWarning {
            code: "deployment_setting_not_applied",
            delivery: Delivery::Frame,
            detail: serde_json::json!({ "setting": "stt.segmented.deadline_ms", "applied": deadline_ms }),
        });
    }
    let language = language_of(&req.language);
    let underlying = ovr.and_then(|o| o.underlying_model.clone());
    let profile = ovr.and_then(|o| o.profile.clone());
    let detector = shared
        .detector_override
        .clone()
        .unwrap_or_else(|| models::build_support(shared.allow_energy_detector));

    // The resolver, with this build's adapters.
    let probe_spec = target_spec(req, &req.provider, &req.model);
    let built = |adapter: &str| {
        let switched_on = match adapter {
            "openai_realtime_transcription" => shared.commit_transport,
            "cartesia_manual_finalize" => shared.cartesia_finalize,
            _ => true,
        };
        switched_on && adapter_built(adapter, &probe_spec)
    };
    let mut rreq = ResolveRequest::new(&req.provider, &req.model);
    rreq.release = shared.rollout.release;
    rreq.session = req.kind.resolver_kind();
    rreq.mode = mode;
    rreq.covered = covered;
    rreq.language = language.as_deref();
    rreq.bud_leg = req.leg.is_some();
    rreq.underlying_model = underlying.as_deref();
    rreq.profile = profile.as_deref();
    rreq.encoding = Some(req.encoding.as_str());
    rreq.evidence = Evidence::Map;
    rreq.deadline_ms = deadline_ms;
    rreq.latency_tier = latency_tier;
    rreq.adapter_built = Some(&built);
    let mut resolution = resolve(shared.map, &rreq);
    // The control record narrows the switch at session start: a listed deployment or row is
    // resolved as uncovered (today's path or today's refusal), never more.
    if covered {
        let names: Vec<&str> = req
            .leg
            .iter()
            .flat_map(|l| [l.name.as_str(), l.id.as_str()])
            .collect();
        if shared.control.read().disables(&names, &resolution.row_id) {
            tracing::info!(row = %resolution.row_id, "segmented speech-to-text switched off for this session by the control record");
            rreq.covered = false;
            resolution = resolve(shared.map, &rreq);
        }
    }

    let mut decision = match resolution.outcome {
        Outcome::Native => LiveDecision::Native,
        Outcome::Segmented => LiveDecision::Segmented,
        // The commit transport is the engine with a vendor socket for its transcriber: the same
        // turns and results, text after each pause (`segmented` to the client).
        Outcome::Commit => LiveDecision::Segmented,
        Outcome::Refused => {
            LiveDecision::Refused(resolution.refusal.clone().expect("refused has a refusal"))
        }
    };

    // The withdrawal switch: a voice agent on a buffering model keeps today's client, warned.
    if let LiveDecision::Refused(r) = &decision
        && !shared.file_only_refusal
        && r.code == "stt_live_unsupported"
        && resolution
            .row(shared.map)
            .when_unusable
            .as_ref()
            .is_some_and(|w| w.today == waav_segmented_stt::map::TodayBehaviour::BuffersUntilHangup)
    {
        decision = LiveDecision::Native;
        resolution.warnings.insert(
            0,
            Warning {
                code: "stt_buffered_until_commit".into(),
                delivery: Delivery::Frame,
                detail: serde_json::Map::from_iter([
                    (
                        "provider".to_string(),
                        serde_json::json!(resolution.provider),
                    ),
                    ("model".to_string(), serde_json::json!(req.model)),
                ]),
            },
        );
    }

    // Uncovered self-hosted and Azure OpenAI legs keep today's code per site.
    if let (LiveDecision::Refused(r), Some(leg)) = (&mut decision, &req.leg)
        && (r.code == "stt_not_streaming" || r.code == "unsupported_deployment")
        && mode != TranscriptionMode::Streaming
    {
        r.code = match leg.site {
            LegSite::Agent => "stt_not_streaming".into(),
            LegSite::Named => "unsupported_deployment".into(),
        };
    }

    if decision == LiveDecision::Segmented {
        if let Some(why) = &detector.refused {
            decision = LiveDecision::Refused(Refusal {
                code: "stt_segmentation_unavailable".into(),
                reason: Some("detector_refused".into()),
                text: format!(
                    "The gateway cannot cut this call into utterances: its speech detector is not \
                     available ({why}). Choose a streaming transcription model."
                ),
                details: Default::default(),
            });
        } else {
            if let Some(fallback) = detector.fallback {
                extra.push(ExtraWarning {
                    code: "stt_detector_fallback",
                    delivery: Delivery::Frame,
                    detail: serde_json::json!({ "detector": detector.kind.as_str(), "reason": fallback.as_str() }),
                });
            }
            if language.is_none() {
                extra.push(ExtraWarning {
                    code: "stt_language_unset",
                    delivery: Delivery::Notice,
                    detail: serde_json::json!({}),
                });
            }
        }
    }

    LiveResolution {
        resolution,
        decision,
        covered,
        mode,
        mode_source,
        latency_tier,
        deadline_ms,
        deadline_raised,
        detector,
        extra,
        kind: req.kind,
        deployment: deployment_name,
        language,
    }
}

/// Build the segmented engine's plan. `api_key` is the vendor credential of the leg.
/// What a session starting on this target is told about its key: room in the row's hourly and
/// daily budgets, and whether recent uploads were lost to rate limits.
pub fn admission(
    shared: &SttLiveShared,
    live: &LiveResolution,
    req: &LiveRequest,
    api_key: &str,
) -> waav_segmented_stt::transcriber::gate::Admission {
    use waav_segmented_stt::transcriber::gate::Admission;
    let Some(t) = live.transport() else {
        return Admission::Ok;
    };
    let mut spec = target_spec(req, &live.resolution.provider, &live.resolution.model_sent);
    spec.api_key = api_key.to_string();
    let Ok(transcriber) = build_transcriber(t, &spec, &shared.clients) else {
        return Admission::Ok;
    };
    let info = transcriber.info();
    let key = format!(
        "{}|{}|{}",
        info.host_key,
        credential_tag(&spec.api_key),
        info.model
    );
    let limiter = shared.limiters.get(&key, limit_spec(t));
    limiter.set_windows(&waav_segmented_stt::live::long_windows(t));
    limiter.admission()
}

/// Run the setup probe for a session about to be built (cached per target and credential).
pub async fn probe_session(
    shared: &SttLiveShared,
    live: &LiveResolution,
    req: &LiveRequest,
    api_key: String,
) -> waav_segmented_stt::transcriber::probe::ProbeVerdict {
    use waav_segmented_stt::transcriber::probe::{ProbeVerdict, probe};
    let Some(t) = live.transport() else {
        return ProbeVerdict::Served;
    };
    let mut spec = target_spec(req, &live.resolution.provider, &live.resolution.model_sent);
    spec.api_key = api_key;
    let transcriber = match build_transcriber(t, &spec, &shared.clients) {
        Ok(tr) => tr,
        Err(e) => return ProbeVerdict::Unknown { message: e },
    };
    let info = transcriber.info().clone();
    let key = format!(
        "{}|{}|{}|{}",
        info.host_key,
        info.adapter,
        info.model,
        credential_tag(&spec.api_key)
    );
    if let Some(v) = shared.probe_cache.get(&key) {
        return v;
    }
    let ctx = waav_segmented_stt::transcriber::SegmentContext {
        language: language_of(&req.language),
        prompt: req.prompt.clone(),
        keywords: req.keyterms.clone(),
        candidate_languages: req
            .leg
            .as_ref()
            .map(|l| l.expected_languages.clone())
            .unwrap_or_default(),
        ..Default::default()
    };
    let verdict = probe(
        transcriber.as_ref(),
        &ctx,
        std::time::Duration::from_millis(2_000),
        &shared.repairs,
    )
    .await;
    shared.probe_cache.put(&key, &verdict);
    verdict
}

pub fn build_plan(
    shared: &SttLiveShared,
    live: &LiveResolution,
    req: &LiveRequest,
    api_key: String,
) -> Result<SegmentedPlan, String> {
    let t = live.transport().ok_or("the session is not segmented")?;
    let mut spec = target_spec(req, &live.resolution.provider, &live.resolution.model_sent);
    spec.api_key = api_key;
    let transcriber = build_transcriber(t, &spec, &shared.clients)?;
    let info = transcriber.info().clone();
    let cred_tag = credential_tag(&spec.api_key);
    let row_id = live.resolution.row_id.clone();
    let limiter_key = format!("{}|{}|{}", info.host_key, cred_tag, info.model);
    let mut lspec = limit_spec(t);
    if let Some(l) = req
        .leg
        .as_ref()
        .and_then(|l| l.capability_override.as_ref())
        .and_then(|o| o.limits.as_ref())
    {
        if let Some(rpm) = l.requests_per_minute {
            lspec = waav_segmented_stt::transcriber::gate::LimitSpec::from_rpm(
                rpm,
                lspec.max_concurrent,
            );
        }
        if let Some(c) = l.max_concurrent_requests {
            lspec.max_concurrent = c.max(1) as usize;
        }
    }
    let limiter = shared.limiters.get(&limiter_key, lspec);
    limiter.set_windows(&waav_segmented_stt::live::long_windows(t));
    let breaker = shared
        .breakers
        .get(&format!("{row_id}|{}|{cred_tag}", info.host_key));
    let budget = shared
        .budgets
        .get(&format!("{}|{cred_tag}", live.resolution.provider));
    let low_latency = live.latency_tier == LatencyTier::LowLatency;
    let attempts = SegmentAttempts {
        transcriber,
        breaker,
        limiter: Arc::clone(&limiter),
        budget,
        policy: if low_latency {
            SecondRequestPolicy::Hedge
        } else {
            SecondRequestPolicy::OnFailureOrStall
        },
        health: Arc::new(SessionHealth::default()),
        repairs: Arc::clone(&shared.repairs),
        observer: None,
    };
    let store_key = live.deployment.clone().unwrap_or_else(|| row_id.clone());
    let seed = req
        .leg
        .as_ref()
        .and_then(|l| l.capability_override.as_ref())
        .and_then(|o| o.latency.as_ref())
        .and_then(|l| l.ttfs_p99_ms)
        .or_else(|| seed_p99(t));
    shared.latency.seed(&store_key, seed);
    let seg = req.leg.as_ref().and_then(|l| l.segmented.as_ref());
    let row_timeout = seg.and_then(|s| s.request_timeout_ms).or_else(|| {
        t.segment_profile
            .as_ref()
            .and_then(|p| p.request_timeout_ms)
    });
    let upload = AttemptsUpload {
        attempts,
        store: Arc::clone(&shared.latency),
        key: store_key,
        deadline_ms: live.deadline_ms,
        row_request_timeout_ms: row_timeout,
        low_latency,
        limiter,
    };
    let mut profile = profile_for(t, &req.tuning, req.interim_results);
    if let Some(s) = seg {
        if let Some(v) = s.pre_roll_ms {
            profile.pre_roll_ms = v.min(1000);
        }
        if let Some(v) = s.trailing_silence_ms {
            profile.trailing_silence_ms = v.min(1000);
        }
        if let Some(v) = s.max_segment_ms {
            profile.max_segment_ms = v.clamp(5000, 25_000);
        }
        if let Some(v) = s.max_in_flight {
            profile.max_in_flight = v.clamp(1, 4) as usize;
        }
        if let Some(p) = s.upload_policy.as_deref().and_then(UploadPolicy::parse) {
            profile.upload_policy = p;
            if p == UploadPolicy::PerTurn {
                profile.interims = waav_segmented_stt::types::InterimMode::Off;
            }
        }
    }
    let candidates = req
        .leg
        .as_ref()
        .map(|l| l.expected_languages.clone())
        .unwrap_or_default();
    let mut engine = EngineConfig::new(profile);
    engine.encoding = req.encoding.clone();
    engine.sample_rate = req.sample_rate;
    engine.channels = req.channels.max(1);
    engine.language = live.language.clone();
    engine.candidate_languages = candidates;
    engine.prompt = req.prompt.clone();
    engine.keywords = req.keyterms.clone();
    engine.quality = quality_policy(t, req.prompt.as_deref());
    engine.detector_fallback = live.detector.fallback;
    engine.allow_energy_fallback = shared.allow_energy_detector;
    engine.billing =
        waav_segmented_stt::live::billing_rule(&live.resolution.row(shared.map).billing);
    let audio_model = (engine.profile.endpoint_policy
        == waav_segmented_stt::profile::EndpointPolicy::Auto)
        .then(models::audio_model)
        .flatten();
    Ok(SegmentedPlan {
        engine,
        detector: models::detector_factory(&live.detector),
        upload: Arc::new(upload),
        audio_model,
        text_model: shared.text_model.clone(),
        clock: Arc::new(GatewayClock),
        provider_info: "segmented",
    })
}

/// A short, non-reversible tag of a credential for limiter and breaker keys: the secret itself
/// never becomes a key.
fn credential_tag(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(secret.as_bytes());
    d[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// How often the control record is read from Redis.
pub const CONTROL_REFRESH: std::time::Duration = std::time::Duration::from_secs(15);

/// Keep [`SttLiveShared::control`] in step with Bud's control record (`waav:stt_live:control` in
/// the Bud Redis, `WAAV_REDIS_URL` / `WAAV_REDIS_DB`). An absent key disables nothing; a Redis that
/// cannot be read keeps the last record, so an outage never switches a deployment back on or off.
pub fn spawn_control_refresh(shared: Arc<SttLiveShared>, redis_url: String, db: u8) {
    tokio::spawn(async move {
        use redis::AsyncCommands;
        let client = {
            use redis::IntoConnectionInfo;
            let mut info = match redis_url.as_str().into_connection_info() {
                Ok(i) => i,
                Err(e) => {
                    tracing::warn!(error = %e, "segmented speech-to-text control record: invalid Redis address");
                    return;
                }
            };
            if db != 0 {
                info.redis.db = i64::from(db);
            }
            match redis::Client::open(info) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "segmented speech-to-text control record: invalid Redis config");
                    return;
                }
            }
        };
        let mut conn: Option<redis::aio::MultiplexedConnection> = None;
        loop {
            if conn.is_none() {
                conn = client.get_multiplexed_async_connection().await.ok();
            }
            if let Some(c) = conn.as_mut() {
                let read: redis::RedisResult<Option<String>> =
                    c.get(waav_segmented_stt::control::CONTROL_KEY).await;
                match read {
                    Ok(value) => apply_control(&shared, value.as_deref()),
                    Err(e) => {
                        tracing::debug!(error = %e, "segmented speech-to-text control record: read failed; keeping the last one");
                        conn = None;
                    }
                }
            }
            tokio::time::sleep(CONTROL_REFRESH).await;
        }
    });
}

/// Apply one read of the control record: absent clears it, unreadable keeps the last one.
pub fn apply_control(shared: &SttLiveShared, value: Option<&str>) {
    let next = match value {
        None => waav_segmented_stt::control::ControlRecord::default(),
        Some(json) => match waav_segmented_stt::control::ControlRecord::parse(json) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "segmented speech-to-text control record ignored");
                return;
            }
        },
    };
    let mut current = shared.control.write();
    if *current != next {
        tracing::info!(
            empty = next.is_empty(),
            "segmented speech-to-text control record changed"
        );
        *current = next;
    }
}

/// Whether a model is file-only on a live call: with every release built and the switch covering
/// it, a voice agent with automatic turns would not get a streaming client. `/v1/realtime` without an
/// agent refuses such a deployment with `realtime_needs_agent` (it has no turn-taking to segment for).
pub fn is_file_only(shared: &SttLiveShared, provider: &str, model: &str) -> bool {
    let mut req = waav_segmented_stt::resolve::ResolveRequest::new(provider, model);
    req.release = waav_segmented_stt::rollout::BUILT_RELEASE;
    req.bud_leg = true;
    let r = waav_segmented_stt::resolve::resolve(shared.map, &req);
    r.outcome != Outcome::Native
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Release 4: OpenAI's live-only models on its realtime socket, once the operator turns the
    /// commit transport on after a live probe.
    #[test]
    fn a_live_only_model_is_served_on_the_vendor_socket_only_when_the_transport_is_on() {
        let off = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        let r = resolve_session(&off, &req("openai", "gpt-live-transcribe", AGENT));
        assert!(
            matches!(&r.decision, LiveDecision::Refused(x) if x.code == "stt_live_unsupported"),
            "{:?}",
            r.decision
        );
        let on = shared(&[
            ("WAAV_SEGMENTED_STT", "on"),
            ("WAAV_STT_COMMIT_TRANSPORT", "1"),
        ]);
        let r = resolve_session(&on, &req("openai", "gpt-live-transcribe", AGENT));
        assert_eq!(
            r.decision,
            LiveDecision::Segmented,
            "{:?}",
            r.resolution.notes
        );
        assert_eq!(
            r.transport().unwrap().adapter.as_str(),
            "openai_realtime_transcription"
        );
        let plan = build_plan(
            &on,
            &r,
            &req("openai", "gpt-live-transcribe", AGENT),
            "sk-test".into(),
        );
        assert!(plan.is_ok(), "{:?}", plan.err());
        // gpt-transcribe keeps uploading files unless the deployment asks for the low-latency tier.
        let r = resolve_session(&on, &req("openai", "gpt-transcribe", AGENT));
        assert_eq!(
            r.transport().unwrap().adapter.as_str(),
            "openai_transcriptions"
        );
    }

    #[test]
    fn cartesia_ends_utterances_on_the_gateway_detector_only_after_its_probe_and_flag() {
        // Two gates (addendum B8): the map records the live probe, and the operator sets the flag.
        // This map records no probe yet, so even with the flag the session keeps today's client.
        let off = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        assert_eq!(
            resolve_session(&off, &req("cartesia", "ink-whisper", AGENT)).decision,
            LiveDecision::Native
        );
        let on = shared(&[
            ("WAAV_SEGMENTED_STT", "on"),
            ("WAAV_STT_CARTESIA_MANUAL_FINALIZE", "1"),
        ]);
        let r = resolve_session(&on, &req("cartesia", "ink-whisper", AGENT));
        assert_eq!(
            r.decision,
            LiveDecision::Native,
            "no recorded probe: {:?}",
            r.resolution.notes
        );
        assert!(on.cartesia_finalize);
    }

    #[test]
    fn the_control_record_switches_a_row_or_deployment_off_at_session_start() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        assert_eq!(
            resolve_session(&s, &req("elevenlabs", "scribe_v2", AGENT)).decision,
            LiveDecision::Segmented
        );
        apply_control(&s, Some(r#"{"disabled_rows": ["elevenlabs:scribe_v2"]}"#));
        let r = resolve_session(&s, &req("elevenlabs", "scribe_v2", AGENT));
        assert!(
            matches!(r.decision, LiveDecision::Refused(_)),
            "{:?}",
            r.decision
        );
        // Unreadable keeps the last record; absent clears it.
        apply_control(&s, Some("garbage"));
        assert!(matches!(
            resolve_session(&s, &req("elevenlabs", "scribe_v2", AGENT)).decision,
            LiveDecision::Refused(_)
        ));
        apply_control(&s, None);
        assert_eq!(
            resolve_session(&s, &req("elevenlabs", "scribe_v2", AGENT)).decision,
            LiveDecision::Segmented
        );
        // A deployment by name.
        apply_control(&s, Some(r#"{"disabled_deployments": ["my-scribe"]}"#));
        let mut on_leg = req("elevenlabs", "scribe_v2", AGENT);
        on_leg.leg = Some(LiveLeg {
            name: "my-scribe".into(),
            id: "ep-1".into(),
            site: LegSite::Agent,
            api_base: None,
            provider_params: BTreeMap::new(),
            segmented: None,
            capability_override: None,
            expected_languages: Vec::new(),
        });
        assert!(matches!(
            resolve_session(&s, &on_leg).decision,
            LiveDecision::Refused(_)
        ));
    }

    #[test]
    fn file_only_models_are_told_apart_from_streaming_ones() {
        let s = shared(&[]);
        assert!(is_file_only(&s, "elevenlabs", "scribe_v2"));
        assert!(is_file_only(&s, "openai", "whisper-1"));
        assert!(is_file_only(&s, "groq", "whisper-large-v3-turbo"));
        assert!(!is_file_only(&s, "deepgram", "nova-3"));
        assert!(!is_file_only(&s, "elevenlabs", "scribe_v2_realtime"));
    }

    fn shared(env: &[(&str, &str)]) -> SttLiveShared {
        let m: BTreeMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut s = SttLiveShared::from_lookup(|k| m.get(k).cloned(), None).unwrap();
        // A build whose detector loaded (or, without Silero, the energy detector).
        s.detector_override = Some(models::support_for(
            if cfg!(feature = "silero-vad") {
                models::ModelState::Ready
            } else {
                models::ModelState::NotBuilt
            },
            false,
        ));
        s
    }

    fn req(provider: &str, model: &str, kind: LiveSessionKind) -> LiveRequest {
        LiveRequest {
            provider: provider.into(),
            model: model.into(),
            language: "en".into(),
            encoding: "linear16".into(),
            sample_rate: 16_000,
            channels: 1,
            kind,
            requested_mode: None,
            leg: None,
            interim_results: None,
            keyterms: Vec::new(),
            prompt: None,
            tuning: EndpointTuning::default(),
            extras: BTreeMap::new(),
        }
    }

    const AGENT: LiveSessionKind = LiveSessionKind::Agent { manual: false };

    #[test]
    fn a_streaming_model_keeps_todays_client_whatever_the_switch() {
        for mode in ["off", "allowlist", "on"] {
            let s = shared(&[("WAAV_SEGMENTED_STT", mode)]);
            let r = resolve_session(&s, &req("deepgram", "nova-3", AGENT));
            assert_eq!(r.decision, LiveDecision::Native, "{mode}");
            assert!(r.frame_warnings().is_empty());
        }
    }

    #[test]
    fn a_file_only_model_is_segmented_when_covered_and_refused_by_name_when_not() {
        let on = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        let r = resolve_session(&on, &req("elevenlabs", "scribe_v2", AGENT));
        assert_eq!(r.decision, LiveDecision::Segmented);
        assert!(
            r.frame_warnings()
                .iter()
                .any(|w| w.code == "stt_segmented_mode")
        );
        let off = shared(&[("WAAV_SEGMENTED_STT", "off")]);
        let r = resolve_session(&off, &req("openai", "gpt-transcribe", AGENT));
        assert!(
            matches!(&r.decision, LiveDecision::Refused(x) if x.code == "stt_live_unsupported" && x.reason.as_deref() == Some("not_covered_yet"))
        );
    }

    #[test]
    fn the_allowlist_covers_listed_deployments_and_provider_models_only() {
        let s = shared(&[
            ("WAAV_SEGMENTED_STT", "allowlist"),
            ("WAAV_SEGMENTED_STT_ALLOWLIST", "openai:gpt-transcribe"),
        ]);
        assert_eq!(
            resolve_session(&s, &req("openai", "gpt-transcribe", AGENT)).decision,
            LiveDecision::Segmented
        );
        assert!(matches!(
            resolve_session(&s, &req("groq", "whisper-large-v3", AGENT)).decision,
            LiveDecision::Refused(_)
        ));
    }

    #[test]
    fn a_plain_session_on_a_buffering_model_stays_on_todays_client_unless_it_asks() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        let r = resolve_session(&s, &req("openai", "whisper-1", LiveSessionKind::Plain));
        assert_eq!(r.decision, LiveDecision::Native);
        assert!(
            r.frame_warnings()
                .iter()
                .any(|w| w.code == "stt_buffered_until_commit")
        );
        let mut asking = req("openai", "whisper-1", LiveSessionKind::Plain);
        asking.requested_mode = Some("segmented".into());
        let r = resolve_session(&s, &asking);
        assert_eq!(r.decision, LiveDecision::Segmented);
        assert_eq!(r.mode_source, "request");
    }

    #[test]
    fn a_conversation_loop_with_turn_detection_off_is_treated_as_plain() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        let r = resolve_session(
            &s,
            &req(
                "groq",
                "whisper-large-v3",
                LiveSessionKind::Conversation {
                    turn_detection: false,
                },
            ),
        );
        assert_eq!(r.decision, LiveDecision::Native);
        let r = resolve_session(
            &s,
            &req(
                "groq",
                "whisper-large-v3",
                LiveSessionKind::Conversation {
                    turn_detection: true,
                },
            ),
        );
        assert_eq!(r.decision, LiveDecision::Segmented);
    }

    #[test]
    fn a_voice_agent_ignores_a_request_preference_and_an_unknown_one_is_reported() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        let mut a = req("elevenlabs", "scribe_v2", AGENT);
        a.requested_mode = Some("streaming".into());
        let r = resolve_session(&s, &a);
        assert_eq!(r.decision, LiveDecision::Segmented);
        assert!(
            r.extra
                .iter()
                .any(|w| w.code == "stt_transcription_mode_ignored")
        );
        let mut p = req("deepgram", "nova-3", LiveSessionKind::Plain);
        p.requested_mode = Some("fast".into());
        assert!(
            resolve_session(&s, &p)
                .extra
                .iter()
                .any(|w| w.code == "stt_transcription_mode_invalid")
        );
    }

    #[test]
    fn asking_for_streaming_on_a_file_only_model_is_refused() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        let mut p = req("openai", "gpt-transcribe", LiveSessionKind::Plain);
        p.requested_mode = Some("streaming".into());
        assert!(
            matches!(resolve_session(&s, &p).decision, LiveDecision::Refused(r) if r.code == "stt_not_streaming")
        );
    }

    #[test]
    fn a_compressed_audio_format_cannot_be_segmented() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        let mut a = req("elevenlabs", "scribe_v2", AGENT);
        a.encoding = "opus".into();
        assert!(
            matches!(resolve_session(&s, &a).decision, LiveDecision::Refused(r) if r.code == "stt_segmentation_unavailable")
        );
    }

    #[test]
    fn uncovered_self_hosted_legs_keep_todays_code_per_site() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "off")]);
        for (site, code) in [
            (LegSite::Agent, "stt_not_streaming"),
            (LegSite::Named, "unsupported_deployment"),
        ] {
            let mut r = req(
                "self_hosted",
                "whisper",
                LiveSessionKind::Agent { manual: true },
            );
            r.leg = Some(LiveLeg {
                name: "my-whisper".into(),
                id: "ep-1".into(),
                site,
                api_base: Some("http://whisper.svc:8000/v1".into()),
                provider_params: BTreeMap::new(),
                segmented: None,
                capability_override: None,
                expected_languages: Vec::new(),
            });
            assert!(
                matches!(resolve_session(&s, &r).decision, LiveDecision::Refused(x) if x.code == code),
                "{site:?}"
            );
        }
    }

    #[test]
    fn the_withdrawal_switch_keeps_a_buffering_agent_on_todays_client_with_a_warning() {
        let s = shared(&[
            ("WAAV_SEGMENTED_STT", "off"),
            ("WAAV_STT_FILE_ONLY_REFUSAL", "off"),
        ]);
        let r = resolve_session(&s, &req("openai", "gpt-transcribe", AGENT));
        assert_eq!(r.decision, LiveDecision::Native);
        assert_eq!(r.frame_warnings()[0].code, "stt_buffered_until_commit");
    }

    #[test]
    fn a_deployment_deadline_under_the_ceiling_is_raised_and_reported() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        let mut a = req("elevenlabs", "scribe_v2", AGENT);
        a.leg = Some(LiveLeg {
            name: "scribe".into(),
            id: "ep-2".into(),
            site: LegSite::Agent,
            api_base: None,
            provider_params: BTreeMap::new(),
            segmented: Some(bud_auth::SttSegmented {
                deadline_ms: Some(3000),
                ..Default::default()
            }),
            capability_override: None,
            expected_languages: Vec::new(),
        });
        let r = resolve_session(&s, &a);
        assert_eq!((r.deadline_ms, r.deadline_raised), (4000, true));
        assert!(
            r.extra
                .iter()
                .any(|w| w.code == "deployment_setting_not_applied")
        );
    }

    #[test]
    fn a_segmented_session_with_no_language_gets_the_notice() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        let mut a = req("elevenlabs", "scribe_v2", AGENT);
        a.language = "auto".into();
        assert!(
            resolve_session(&s, &a)
                .extra
                .iter()
                .any(|w| w.code == "stt_language_unset")
        );
    }

    #[tokio::test]
    async fn the_plan_builds_for_every_release_one_vendor() {
        let s = shared(&[("WAAV_SEGMENTED_STT", "on")]);
        for (p, m) in [
            ("openai", "gpt-transcribe"),
            ("groq", "whisper-large-v3-turbo"),
            ("elevenlabs", "scribe_v2"),
        ] {
            let r = req(p, m, AGENT);
            let live = resolve_session(&s, &r);
            assert_eq!(live.decision, LiveDecision::Segmented, "{p}");
            let plan = build_plan(&s, &live, &r, "key".into()).unwrap();
            assert_eq!(plan.engine.language.as_deref(), Some("en"));
            assert_eq!(plan.upload.deadline_ms(), 6000);
        }
    }

    #[test]
    fn credentials_never_appear_in_keys() {
        let t = credential_tag("sk-secret");
        assert_eq!(t.len(), 12);
        assert!(!t.contains("secret"));
    }
}

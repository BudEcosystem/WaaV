//! How a speech-to-text model or deployment is served on a live call (Release 2), for Bud and for
//! operators.
//!
//! * `GET /capabilities/stt?provider=openai&model=gpt-transcribe` answers for one model;
//!   without parameters it lists every model the capability map names exactly.
//! * Under the Bud control plane a publisher writes the same answer for each transcription
//!   deployment to `voice_capability:{endpoint_id}`, beside `voice_table:`, because budapp cannot
//!   call the gateway. It checks every 30 s, rewrites an unchanged record every 600 s, and the
//!   record expires after 1,800 s so a deleted deployment's answer ages out.
//!
//! The answer is the resolver's, for the three kinds of session that take turns differently: a
//! voice agent with automatic turn detection, a push-to-talk session (an agent in manual mode, a
//! conversation loop or a DAG pipeline), and a plain `/ws` session. It never contains a credential.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::core::stt::segmented::live::{
    LegSite, LiveDecision, LiveLeg, LiveRequest, LiveSessionKind, SttLiveShared, resolve_session,
};
use crate::handlers::ws::stt_contract::{ReadyFacts, ready_stt, transcription_mode};
use crate::state::AppState;

/// Redis key prefix, beside `voice_table:`. budapp reads `voice_capability:{endpoint_id}`.
pub const CAPABILITY_KEY_PREFIX: &str = "voice_capability:";
const TICK: Duration = Duration::from_secs(30);
const REFRESH: Duration = Duration::from_secs(600);
const TTL_SECS: u64 = 1800;
/// The capability a transcription deployment serves.
const STT_CAPABILITY: &str = "audio_transcription";

/// One kind of session's answer.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionAnswer {
    /// `streaming`, `segmented`, `buffered` (text only at the client's commit) or `refused`.
    pub transcription_mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The answer for one model, or one deployment.
#[derive(Debug, Clone, Serialize)]
pub struct SttCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub row_id: String,
    pub map_version: String,
    pub release: u8,
    /// Whether the rollout switch (and the control record) covers a voice agent on it.
    pub covered: bool,
    pub voice_agent: SessionAnswer,
    pub push_to_talk: SessionAnswer,
    pub plain: SessionAnswer,
    /// The `ready.stt` latency class of a voice agent's session (`realtime`, `fast`, `slow`,
    /// `unknown`).
    pub latency_class: Option<String>,
    pub final_latency_slow_ms: Option<u64>,
    /// A voice agent gets text only after each pause, not while the caller speaks: Bud labels the
    /// deployment "slower on calls".
    pub slower_on_calls: bool,
    pub streaming_alternatives: Vec<String>,
}

fn request(
    provider: &str,
    model: &str,
    kind: LiveSessionKind,
    leg: Option<LiveLeg>,
) -> LiveRequest {
    LiveRequest {
        provider: provider.to_string(),
        model: model.to_string(),
        language: String::new(),
        encoding: "linear16".into(),
        sample_rate: 16_000,
        channels: 1,
        kind,
        requested_mode: None,
        leg,
        interim_results: None,
        keyterms: Vec::new(),
        prompt: None,
        tuning: Default::default(),
        extras: Default::default(),
    }
}

/// The resolver's answer for one model (and deployment, when `leg` is given).
pub fn capability_for(
    shared: &SttLiveShared,
    provider: &str,
    model: &str,
    leg: Option<LiveLeg>,
) -> SttCapability {
    let answer = |kind: LiveSessionKind| {
        let live = resolve_session(shared, &request(provider, model, kind, leg.clone()));
        let a = match &live.decision {
            LiveDecision::Refused(r) => SessionAnswer {
                transcription_mode: "refused",
                code: Some(r.code.clone()),
                reason: r.reason.clone(),
            },
            _ => SessionAnswer {
                transcription_mode: transcription_mode(&live),
                code: None,
                reason: None,
            },
        };
        (live, a)
    };
    let (agent_live, voice_agent) = answer(LiveSessionKind::Agent { manual: false });
    let (_, push_to_talk) = answer(LiveSessionKind::Agent { manual: true });
    let (_, plain) = answer(LiveSessionKind::Plain);
    let ready = ready_stt(
        &agent_live,
        &ReadyFacts::default(),
        shared.map,
        shared.rollout.release,
    );
    let alternatives = ready
        .get("streaming_alternatives")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    SttCapability {
        endpoint_id: leg.as_ref().map(|l| l.id.clone()),
        provider: agent_live.resolution.provider.clone(),
        model: model.to_string(),
        row_id: agent_live.resolution.row_id.clone(),
        map_version: shared.map.map_version().to_string(),
        release: shared.rollout.release,
        covered: agent_live.resolution.covered,
        slower_on_calls: voice_agent.transcription_mode == "segmented",
        voice_agent,
        push_to_talk,
        plain,
        latency_class: ready
            .get("latency_class")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        final_latency_slow_ms: ready.get("final_latency_slow_ms").and_then(|v| v.as_u64()),
        streaming_alternatives: alternatives,
    }
}

#[derive(Debug, Deserialize)]
pub struct CapabilityQuery {
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// `GET /capabilities/stt`.
pub async fn stt_capabilities(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CapabilityQuery>,
) -> impl IntoResponse {
    let shared = &state.core_state.stt_live;
    match q
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        Some(provider) => {
            let model = q.model.as_deref().map(str::trim).unwrap_or_default();
            Json(serde_json::json!(capability_for(
                shared, provider, model, None
            )))
        }
        None => Json(serde_json::json!({
            "map_version": shared.map.map_version(),
            "release": shared.rollout.release,
            "models": exact_models(shared)
                .into_iter()
                .map(|(p, m)| capability_for(shared, &p, &m, None))
                .collect::<Vec<_>>(),
        })),
    }
}

/// Every provider and model the map names exactly (no pattern, no provider default).
fn exact_models(shared: &SttLiveShared) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    shared
        .map
        .rows()
        .iter()
        .filter_map(|r| {
            let m = r.r#match.model.clone()?;
            (!m.contains(['*', '?', '['])).then(|| (r.r#match.provider.clone(), m))
        })
        .filter(|pm| seen.insert(pm.clone()))
        .collect()
}

/// A deployment as the resolver sees it.
fn deployment_leg(id: &str, ep: &bud_auth::VoiceEndpoint) -> LiveLeg {
    let stt = ep.config.stt();
    LiveLeg {
        name: id.to_string(),
        id: id.to_string(),
        site: LegSite::Agent,
        api_base: ep.api_base.clone(),
        provider_params: ep.provider_params.clone(),
        segmented: stt.segmented.clone(),
        capability_override: stt.capability_override.clone(),
        expected_languages: stt.expected_languages.clone().unwrap_or_default(),
        data: crate::core::stt::data_settings::DataSettings::from_settings(&stt),
        fallbacks: Vec::new(),
    }
}

fn deployment_model(ep: &bud_auth::VoiceEndpoint) -> String {
    ep.config
        .stt()
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| ep.model.clone())
        .unwrap_or_default()
}

/// What a deployment's answer depends on. Hashed in memory only; never written out.
fn fingerprint(shared: &SttLiveShared, ep: &bud_auth::VoiceEndpoint) -> u64 {
    let stt = ep.config.stt();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (
        &ep.vendor,
        deployment_model(ep),
        &ep.api_base,
        format!("{:?}{:?}", stt.segmented, stt.capability_override),
        shared.map.map_version(),
        shared.rollout.release,
    )
        .hash(&mut h);
    h.finish()
}

/// Start the publisher, when WaaV runs under the Bud control plane.
pub fn spawn(state: Arc<AppState>) -> Option<tokio::task::JoinHandle<()>> {
    let bud = state.bud_mode.clone()?;
    Some(tokio::spawn(async move {
        let mut published: HashMap<String, (u64, Instant)> = HashMap::new();
        let mut tick = tokio::time::interval(TICK);
        loop {
            tick.tick().await;
            let shared = Arc::clone(&state.core_state.stt_live);
            let endpoints = bud.plane().auth.voice_endpoints();
            let live: HashSet<&str> = endpoints.iter().map(|(id, _)| &**id).collect();
            published.retain(|id, _| live.contains(id.as_str()));
            for (id, ep) in &endpoints {
                if !ep.serves(STT_CAPABILITY) {
                    continue;
                }
                let fp = fingerprint(&shared, ep);
                if let Some((seen, at)) = published.get(&**id)
                    && *seen == fp
                    && at.elapsed() < REFRESH
                {
                    continue;
                }
                let record = capability_for(
                    &shared,
                    &ep.vendor,
                    &deployment_model(ep),
                    Some(deployment_leg(id, ep)),
                );
                let Ok(json) = serde_json::to_string(&record) else {
                    continue;
                };
                match bud
                    .store()
                    .set_ex(&format!("{CAPABILITY_KEY_PREFIX}{id}"), &json, TTL_SECS)
                    .await
                {
                    Ok(()) => {
                        debug!(endpoint_id = %id, agent = record.voice_agent.transcription_mode, "published speech-to-text capability");
                        published.insert(id.to_string(), (fp, Instant::now()));
                    }
                    Err(e) => {
                        warn!(endpoint_id = %id, error = %e, "could not publish speech-to-text capability")
                    }
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A healthy detector, independent of the process-wide model state other tests change.
    fn healthy(mut s: SttLiveShared) -> SttLiveShared {
        use crate::core::stt::segmented::models::{ModelState, support_for};
        s.detector_override = Some(support_for(
            if cfg!(feature = "silero-vad") {
                ModelState::Ready
            } else {
                ModelState::NotBuilt
            },
            false,
        ));
        s
    }

    fn shared(switch: &str) -> SttLiveShared {
        let switch = switch.to_string();
        healthy(
            SttLiveShared::from_lookup(
                move |k| (k == "WAAV_SEGMENTED_STT").then(|| switch.clone()),
                None,
            )
            .unwrap(),
        )
    }

    #[test]
    fn a_file_only_model_is_segmented_for_agents_and_labelled_slower() {
        let c = capability_for(&shared("on"), "elevenlabs", "scribe_v2", None);
        assert_eq!(c.row_id, "elevenlabs:scribe_v2");
        assert!(c.covered);
        assert_eq!(c.voice_agent.transcription_mode, "segmented");
        assert_eq!(c.push_to_talk.transcription_mode, "segmented");
        assert!(c.slower_on_calls);
        assert!(
            c.streaming_alternatives
                .contains(&"scribe_v2_realtime".to_string()),
            "{c:?}"
        );
        let json = serde_json::to_value(&c).unwrap();
        assert!(json.get("endpoint_id").is_none());
    }

    #[test]
    fn a_streaming_model_streams_everywhere() {
        let c = capability_for(&shared("on"), "deepgram", "nova-3", None);
        for a in [&c.voice_agent, &c.push_to_talk, &c.plain] {
            assert_eq!(a.transcription_mode, "streaming", "{c:?}");
        }
        assert!(!c.slower_on_calls);
    }

    #[test]
    fn an_uncovered_buffering_model_refuses_agents_by_name_and_warns_the_rest() {
        let c = capability_for(&shared("off"), "openai", "gpt-transcribe", None);
        assert!(!c.covered);
        assert_eq!(c.voice_agent.transcription_mode, "refused");
        assert_eq!(c.voice_agent.code.as_deref(), Some("stt_live_unsupported"));
        assert_eq!(c.voice_agent.reason.as_deref(), Some("not_covered_yet"));
        assert_eq!(c.plain.transcription_mode, "buffered");
    }

    #[test]
    fn the_listing_names_every_exact_model_once() {
        let s = shared("on");
        let models = exact_models(&s);
        assert!(models.contains(&("openai".into(), "gpt-transcribe".into())));
        assert!(!models.iter().any(|(_, m)| m.contains('*')));
        let unique: HashSet<_> = models.iter().collect();
        assert_eq!(unique.len(), models.len());
    }
}

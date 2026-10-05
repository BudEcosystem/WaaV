//! The customer contract of segmented speech-to-text: every string and every shape a client sees.
//!
//! Producers (the resolver, the engine, the transcriber layer, turn-taking) supply facts; this
//! module writes the text. The contract is `docs/segmented-stt/customer-contract-reference.md`.

use std::collections::HashMap;

use serde_json::{Value, json};
use waav_segmented_stt::limits::{LatencyBasis, LatencyClass, LatencyEstimate};
use waav_segmented_stt::map::{CapabilityMap, InputMode, LifecycleStatus, ModelString, QualitySignal};
use waav_segmented_stt::resolve::{Delivery, Layer, Refusal};

use super::messages::OutgoingMessage;
use crate::core::stt::segmented::live::{LiveDecision, LiveResolution};
use crate::core::stt::speech_activity::{
    NoticeKind, SegmentOutcome, SegmentResultKind, SpeechActivity, SttLiveFacts, SttNotice, TurnCloseReason,
};

/// What `ready.stt` says about the session's transcription.
fn transcription_mode(live: &LiveResolution) -> &'static str {
    match live.decision {
        LiveDecision::Segmented => "segmented",
        _ if live.resolution.warnings.iter().any(|w| w.code == "stt_buffered_until_commit") => "buffered",
        _ => "streaming",
    }
}

fn capability_source(layer: Layer) -> &'static str {
    match layer {
        Layer::Exact => "exact",
        Layer::Pattern => "glob",
        Layer::ProviderDefault => "provider_default",
        Layer::GlobalDefault => "global_default",
        Layer::ModelUnset | Layer::DeclaredDefault => "model_unset",
        Layer::DeploymentOverride => "deployment_override",
    }
}

fn confidence_source(live: &LiveResolution) -> &'static str {
    let Some(t) = live.transport() else {
        return if transcription_mode(live) == "buffered" { "derived" } else { "vendor" };
    };
    let has = |s: QualitySignal| t.quality_signals.contains(&s);
    if has(QualitySignal::UtteranceConfidence) || has(QualitySignal::WordConfidence) {
        "vendor"
    } else if has(QualitySignal::SegmentAvgLogprob) || has(QualitySignal::TokenLogprobs) || has(QualitySignal::WordLogprob) {
        "derived"
    } else {
        "none"
    }
}

/// Same-provider models that stream on today's client in this release.
pub fn streaming_alternatives(map: &CapabilityMap, live: &LiveResolution, release: u8) -> Vec<String> {
    let provider = live.resolution.provider.as_str();
    let mut out = Vec::new();
    for row in map.rows_for(provider) {
        let Some(model) = row.r#match.model.as_deref() else { continue };
        if row.id == live.resolution.row_id
            || !matches!(row.lifecycle.status, LifecycleStatus::Ga | LifecycleStatus::Preview | LifecycleStatus::Legacy)
        {
            continue;
        }
        let streams = map.transports(row).iter().any(|t| {
            t.transport.adapter == "native"
                && t.transport.input_mode == InputMode::LiveStream
                && t.transport.enabled_from_release.is_some_and(|r| r <= release)
                && t.transport.gateway_client.as_ref().is_none_or(|c| c.status.as_str() != "known_broken")
        });
        if streams {
            out.push(model.to_string());
        }
        if out.len() == 3 {
            break;
        }
    }
    out
}

fn notice_text(code: &str) -> &'static str {
    match code {
        "stt_language_unset" => "No language is set. File models guess the language poorly on short segments; set `language` for reliable transcripts.",
        "stt_capability_assumed" => "This model is not in the gateway's capability map; its capabilities were assumed from the provider's defaults.",
        "stt_model_deprecated" => "This model is deprecated by its vendor and has a shutdown date; move to its replacement.",
        "stt_model_substituted" => "Today's client for this provider runs a different model than the one named.",
        "stt_client_unverified" => "The gateway's client for this model has not been verified on live calls.",
        "stt_placeholder_model_ignored" => "The SDK's placeholder model name was treated as no model; the provider's default runs.",
        "stt_narrowband_audio" => "Audio under 16 kHz is upsampled before transcription.",
        _ => "",
    }
}

/// The text of each `config_warning` code the session receives before `ready`.
pub fn config_warning_text(code: &str, live: &LiveResolution) -> String {
    let model = live.resolution.model_sent.as_str();
    let provider = live.resolution.provider.as_str();
    match code {
        "stt_buffered_until_commit" => format!(
            "Model '{model}' ({provider}) transcribes uploaded files only, and this session uses its \
             buffering client. Text is returned when the client sends audio_end or disconnects. For \
             text after each pause without audio_end, set transcription_mode to segmented or use a \
             streaming model."
        ),
        "stt_segmented_mode" => format!(
            "Model '{model}' ({provider}) transcribes uploaded files only, so the gateway cuts the \
             caller's audio at pauses and uploads each utterance. Text arrives after each pause, about \
             a second later; the gateway gives up a segment {} ms after its pause.",
            live.deadline_ms
        ),
        "stt_min_billed_duration" => format!(
            "The vendor bills each request for a minimum duration; on a segmented session every \
             utterance is a request ({provider})."
        ),
        "stt_transcription_mode_invalid" => "Unknown transcription_mode; treated as auto. Use auto, streaming or segmented.".into(),
        "stt_transcription_mode_ignored" => "A voice agent's transcription settings come from the agent; the request's transcription_mode was ignored.".into(),
        "stt_mode_unavailable" => "The requested transcription mode is not available for this model in this release; the session uses what the model supports.".into(),
        "stt_latency_slow" => "This model's published end-of-speech-to-transcript time is above 2.5 s; turns on calls will feel slow.".into(),
        "stt_detector_fallback" => "The gateway's neural speech detector is not available; a loudness-based detector cuts the audio instead, which is less accurate on noise.".into(),
        "stt_transport_fallback" => "The deployment's declared transport is not available; the session uses its fallback.".into(),
        "deployment_setting_not_applied" => format!(
            "The deployment's segment deadline was below the silence ceiling plus 2.5 s and was raised to {} ms.",
            live.deadline_ms
        ),
        other => notice_text(other).to_string(),
    }
}

/// The `config_warning` messages, in contract order, `stt_setting_not_applied` last.
pub fn config_warnings(live: &LiveResolution) -> Vec<OutgoingMessage> {
    const ORDER: &[&str] = &[
        "stt_buffered_until_commit",
        "stt_transcription_mode_invalid",
        "stt_transcription_mode_ignored",
        "stt_mode_unavailable",
        "stt_placeholder_model_ignored",
        "stt_capability_assumed",
        "stt_model_deprecated",
        "stt_transport_fallback",
        "stt_segmented_mode",
        "stt_latency_slow",
        "stt_fields_reduced",
        "stt_min_billed_duration",
        "stt_capacity_low",
        "stt_detector_fallback",
        "deployment_setting_not_applied",
        "stt_setting_not_applied",
    ];
    let mut by_code: HashMap<&str, Value> = HashMap::new();
    for w in &live.resolution.warnings {
        if w.delivery == Delivery::Frame {
            by_code.entry(w.code.as_str()).or_insert_with(|| Value::Object(w.detail.clone()));
        }
    }
    for w in &live.extra {
        if w.delivery == Delivery::Frame {
            by_code.entry(w.code).or_insert_with(|| w.detail.clone());
        }
    }
    let mut out = Vec::new();
    for code in ORDER {
        if let Some(detail) = by_code.remove(code) {
            out.push(OutgoingMessage::ConfigWarning {
                code: (*code).to_string(),
                message: config_warning_text(code, live),
                detail: (!detail.as_object().is_some_and(|o| o.is_empty())).then_some(detail),
            });
        }
    }
    let mut rest: Vec<_> = by_code.into_iter().collect();
    rest.sort_by(|a, b| a.0.cmp(b.0));
    for (code, detail) in rest {
        out.push(OutgoingMessage::ConfigWarning {
            code: code.to_string(),
            message: config_warning_text(code, live),
            detail: Some(detail),
        });
    }
    out
}

/// A setup refusal as a coded `error`. The socket stays usable for a corrected `config`.
pub fn refusal_error(live: &LiveResolution, refusal: &Refusal, map: &CapabilityMap, release: u8) -> OutgoingMessage {
    let mut details = serde_json::Map::new();
    details.insert("provider".into(), json!(live.resolution.provider));
    if !live.resolution.model_input.is_empty() {
        details.insert("model".into(), json!(live.resolution.model_input));
    }
    if let Some(d) = &live.deployment {
        details.insert("deployment".into(), json!(d));
    }
    if let Some(r) = &refusal.reason {
        details.insert("reason".into(), json!(r));
    }
    for (k, v) in &refusal.details {
        details.entry(k.clone()).or_insert(v.clone());
    }
    details.insert("streaming_alternatives".into(), json!(streaming_alternatives(map, live, release)));
    OutgoingMessage::CodedError {
        message: format!("{}: {}", refusal.code, refusal.text),
        code: refusal.code.clone(),
        recoverable: true,
        details: Some(Value::Object(details)),
    }
}

/// The inputs of `ready.stt` beyond the resolution.
#[derive(Debug, Clone, Default)]
pub struct ReadyFacts {
    pub engine: Option<SttLiveFacts>,
    pub latency: Option<LatencyEstimate>,
    pub speech_events: bool,
    pub barge_in_ms: Option<u32>,
    pub named_model: bool,
}

/// `ready.stt`.
pub fn ready_stt(live: &LiveResolution, facts: &ReadyFacts, map: &CapabilityMap, release: u8) -> Value {
    let classes = map.latency_classes();
    let (realtime_max, fast_max) = (classes.realtime_max_ms, classes.fast_max_ms);
    let row = live.resolution.row(map);
    let mode = transcription_mode(live);
    let gateway_endpointed = mode == "segmented";
    let mut o = serde_json::Map::new();
    o.insert("provider".into(), json!(live.resolution.provider));
    let sensitive = map
        .provider(&live.resolution.provider)
        .is_some_and(|p| p.model_string == ModelString::Sensitive);
    if !sensitive && !live.resolution.model_sent.is_empty() {
        o.insert("model".into(), json!(live.resolution.model_sent));
    }
    let model_source = if live.resolution.warnings.iter().any(|w| w.code == "stt_model_substituted") {
        "substituted"
    } else if matches!(live.resolution.layer, Layer::ModelUnset | Layer::DeclaredDefault) {
        "provider_default"
    } else if live.deployment.is_some() && !facts.named_model {
        "deployment"
    } else {
        "request"
    };
    o.insert("model_source".into(), json!(model_source));
    if let Some(d) = &live.deployment {
        o.insert("deployment".into(), json!(d));
    }
    o.insert("transcription_mode".into(), json!(mode));
    o.insert("requested_mode".into(), json!(live.mode.as_str()));
    o.insert("requested_mode_source".into(), json!(live.mode_source));
    let interims = match mode {
        "segmented" => match facts.engine.as_ref().map(|f| f.interims) {
            Some(waav_segmented_stt::types::InterimMode::Off) => "none",
            _ => "per_segment",
        },
        "buffered" => "none",
        _ => "live",
    };
    o.insert("interim_results".into(), json!(interims));
    o.insert(
        "endpointing".into(),
        json!(match mode {
            "segmented" => "gateway",
            "buffered" => "client",
            _ => "vendor",
        }),
    );
    let speech_events = if gateway_endpointed && facts.speech_events { "detector" } else { "none" };
    o.insert("speech_events".into(), json!(speech_events));
    if speech_events == "detector" {
        o.insert("barge_in_ms".into(), json!(facts.barge_in_ms.unwrap_or(500).max(500)));
    }
    if gateway_endpointed {
        let detector = facts.engine.as_ref().map(|f| f.detector).unwrap_or(live.detector.kind);
        o.insert("detector".into(), json!(detector.as_str()));
    }
    o.insert("confidence_source".into(), json!(confidence_source(live)));
    let est = facts.latency;
    let seed = live.transport().and_then(waav_segmented_stt::live::seed_p99).or_else(|| {
        map.transports(row).iter().find_map(|t| waav_segmented_stt::live::seed_p99(t.transport))
    });
    let (slow, percentile, basis, typical) = match est {
        Some(e) if matches!(e.basis, LatencyBasis::Measured | LatencyBasis::Provisional) => {
            (e.p95_ms, Some(95), e.basis, e.p50_ms)
        }
        _ => (seed, seed.map(|_| 99), if seed.is_some() { LatencyBasis::Seed } else { LatencyBasis::None }, None),
    };
    let class_p99 = est.and_then(|e| e.p99_ms).filter(|_| basis != LatencyBasis::Seed).or(seed);
    o.insert("latency_class".into(), json!(LatencyClass::of(class_p99, realtime_max, fast_max).as_str()));
    o.insert("final_latency_typical_ms".into(), json!(typical));
    o.insert("final_latency_slow_ms".into(), json!(slow));
    if let Some(p) = percentile.filter(|_| slow.is_some()) {
        o.insert("final_latency_slow_percentile".into(), json!(p));
    }
    o.insert("latency_basis".into(), json!(basis.as_str()));
    o.insert(
        "final_deadline_ms".into(),
        json!(gateway_endpointed.then_some(facts.engine.as_ref().map_or(live.deadline_ms, |f| f.final_deadline_ms))),
    );
    let lifecycle = match row.lifecycle.status {
        LifecycleStatus::Preview => "preview",
        LifecycleStatus::Deprecated | LifecycleStatus::Retiring | LifecycleStatus::Legacy => "deprecated",
        _ if row.lifecycle.shutdown_on.is_some() => "deprecated",
        _ => "ga",
    };
    o.insert("lifecycle".into(), json!(lifecycle));
    if let Some(d) = &row.lifecycle.shutdown_on {
        o.insert("shutdown_on".into(), json!(d));
    }
    o.insert("capability_source".into(), json!(capability_source(live.resolution.layer)));
    let alts = streaming_alternatives(map, live, release);
    if !alts.is_empty() && mode != "streaming" {
        o.insert("streaming_alternatives".into(), json!(alts));
    }
    let mut notices: Vec<Value> = live
        .notices()
        .iter()
        .map(|w| json!({"code": w.code, "message": notice_text(&w.code), "detail": Value::Object(w.detail.clone())}))
        .collect();
    for w in &live.extra {
        if w.delivery == Delivery::Notice {
            notices.push(json!({"code": w.code, "message": notice_text(w.code), "detail": w.detail}));
        }
    }
    if let Some(hz) = facts.engine.as_ref().and_then(|f| f.resampled_from_hz).filter(|hz| *hz < 16_000) {
        notices.push(json!({"code": "stt_narrowband_audio", "message": notice_text("stt_narrowband_audio"), "detail": {"input_hz": hz}}));
    }
    if !notices.is_empty() {
        o.insert("notices".into(), json!(notices));
    }
    o.insert("map_version".into(), json!(map.map_version()));
    Value::Object(o)
}

/// Turns the engine's speech events into `vad_event` messages for one session.
#[derive(Debug, Default)]
pub struct VadEvents {
    started: HashMap<u64, bool>,
    turn_started: HashMap<u64, bool>,
    barge_in_ms: u32,
}

impl VadEvents {
    pub fn new(barge_in_ms: u32) -> Self {
        Self {
            barge_in_ms: barge_in_ms.max(500),
            ..Default::default()
        }
    }

    /// `agent_audible`: the agent is speaking, so a turn starts only after `barge_in_ms`.
    pub fn on_activity(&mut self, a: &SpeechActivity, agent_audible: bool) -> Vec<OutgoingMessage> {
        let mut out = Vec::new();
        let ev = |event: &str, turn_id: u64| OutgoingMessage::VadEvent {
            event: event.into(),
            turn_id,
            audio_ms: None,
            sustained_ms: None,
            discarded: None,
            had_transcript: None,
            reason: None,
        };
        match a {
            SpeechActivity::Started { turn_id, at_sample, sustained_ms, .. } => {
                let audio_ms = at_sample / 16;
                if !self.started.insert(*turn_id, true).unwrap_or(false) {
                    let mut m = ev("speech_start", *turn_id);
                    if let OutgoingMessage::VadEvent { audio_ms: a, .. } = &mut m {
                        *a = Some(audio_ms);
                    }
                    out.push(m);
                }
                let turn_now = !agent_audible || *sustained_ms >= self.barge_in_ms;
                if turn_now && !self.turn_started.insert(*turn_id, true).unwrap_or(false) {
                    let mut m = ev("turn_start", *turn_id);
                    if let OutgoingMessage::VadEvent { audio_ms: a, sustained_ms: s, .. } = &mut m {
                        *a = Some(audio_ms);
                        *s = Some(*sustained_ms);
                    }
                    out.push(m);
                }
            }
            SpeechActivity::Stopped { turn_id, at_sample, will_upload, .. } => {
                let mut m = ev("speech_end", *turn_id);
                if let OutgoingMessage::VadEvent { audio_ms, discarded, .. } = &mut m {
                    *audio_ms = Some(at_sample / 16);
                    *discarded = (!will_upload).then_some(true);
                }
                out.push(m);
            }
            SpeechActivity::EndpointDecided { turn_id, .. } => out.push(ev("turn_end", *turn_id)),
            SpeechActivity::TurnClosed { turn_id, had_text, gaps, reason, .. } => {
                self.started.remove(turn_id);
                self.turn_started.remove(turn_id);
                let mut m = ev("turn_closed", *turn_id);
                if let OutgoingMessage::VadEvent { had_transcript, reason: r, .. } = &mut m {
                    *had_transcript = Some(*had_text);
                    if !had_text {
                        *r = Some(if *gaps > 0 {
                            "transcription_failed"
                        } else if *reason == TurnCloseReason::Commit {
                            "no_speech"
                        } else {
                            "no_speech"
                        }
                        .to_string());
                    }
                }
                out.push(m);
            }
        }
        out
    }
}

/// The segmented engine stopped transcribing for good (`stt_unavailable (<reason>): <message>`).
pub fn stt_unavailable(error: &str) -> Option<OutgoingMessage> {
    let rest = error.strip_prefix("Provider error: ").unwrap_or(error);
    let rest = rest.strip_prefix("stt_unavailable (")?;
    let (reason, message) = rest.split_once("): ")?;
    Some(OutgoingMessage::CodedError {
        message: format!("stt_unavailable: transcription stopped ({reason}). {message}"),
        code: "stt_unavailable".into(),
        recoverable: false,
        details: Some(json!({ "reason": reason })),
    })
}

/// `stt_segment_failed` for a lost unit.
pub fn segment_failed(o: &SegmentOutcome) -> Option<OutgoingMessage> {
    let (result, class) = match &o.kind {
        SegmentResultKind::TimedOut => ("timed_out", "timeout"),
        SegmentResultKind::Failed(c) => ("failed", c.as_str()),
        _ => return None,
    };
    Some(OutgoingMessage::SttWarning {
        code: "stt_segment_failed".into(),
        message: "The transcript of one segment of the caller's speech was lost; the turn continues with what was recognised.".into(),
        detail: Some(json!({
            "turn_id": o.turn_id,
            "segment_seq": o.seq,
            "voiced_ms": o.voiced_ms,
            "text_offset": o.joined_text_offset,
            "result": result,
            "class": class,
            "retries": o.retries,
        })),
    })
}

/// An engine notice the client hears about.
pub fn notice_warning(n: &SttNotice) -> Option<OutgoingMessage> {
    match &n.kind {
        NoticeKind::AudioDropped { bytes } => Some(OutgoingMessage::SttWarning {
            code: "stt_audio_dropped".into(),
            message: "Audio arrived faster than the gateway could process it, and some was discarded.".into(),
            detail: Some(json!({ "bytes": bytes })),
        }),
        NoticeKind::Transcriber { code, message } => Some(OutgoingMessage::SttWarning {
            code: code.clone(),
            message: message.clone(),
            detail: None,
        }),
        NoticeKind::DetectorSwitched { to, reason } => Some(OutgoingMessage::SttWarning {
            code: "stt_detector_fallback".into(),
            message: "The neural speech detector failed during the call; a loudness-based detector took over.".into(),
            detail: Some(json!({ "detector": to.as_str(), "reason": reason.as_str() })),
        }),
        NoticeKind::NoiseThresholdRaised { to } => {
            tracing::info!(threshold = to, "segmented session: detector thresholds raised after noise");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::segmented::live::{LiveRequest, LiveSessionKind, SttLiveShared, resolve_session};
    use std::collections::BTreeMap;
    use waav_segmented_stt::profile::EndpointTuning;

    fn shared() -> SttLiveShared {
        SttLiveShared::from_lookup(|k| (k == "WAAV_SEGMENTED_STT").then(|| "on".to_string()), None).unwrap()
    }

    fn live(provider: &str, model: &str, kind: LiveSessionKind) -> LiveResolution {
        resolve_session(
            &shared(),
            &LiveRequest {
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
            },
        )
    }

    #[test]
    fn ready_stt_for_a_segmented_agent_matches_the_worked_example() {
        let s = shared();
        let l = live("elevenlabs", "scribe_v2", LiveSessionKind::Agent { manual: false });
        let v = ready_stt(&l, &ReadyFacts { speech_events: true, barge_in_ms: Some(500), named_model: true, ..Default::default() }, s.map, 6);
        assert_eq!(v["transcription_mode"], "segmented");
        assert_eq!(v["interim_results"], "per_segment");
        assert_eq!(v["endpointing"], "gateway");
        assert_eq!(v["speech_events"], "detector");
        assert_eq!(v["barge_in_ms"], 500);
        assert_eq!(v["final_deadline_ms"], 6000);
        assert_eq!(v["latency_class"], "slow");
        assert_eq!(v["final_latency_slow_ms"], 2010);
        assert_eq!(v["final_latency_slow_percentile"], 99);
        assert_eq!(v["latency_basis"], "seed");
        assert_eq!(v["capability_source"], "exact");
        assert!(v["streaming_alternatives"].as_array().unwrap().iter().any(|m| m == "scribe_v2_realtime"));
        assert_eq!(v["map_version"], s.map.map_version());
    }

    #[test]
    fn ready_stt_for_a_streaming_model_says_streaming_and_vendor() {
        let s = shared();
        let l = live("deepgram", "nova-3", LiveSessionKind::Plain);
        let v = ready_stt(&l, &ReadyFacts::default(), s.map, 6);
        assert_eq!(v["transcription_mode"], "streaming");
        assert_eq!(v["endpointing"], "vendor");
        assert_eq!(v["speech_events"], "none");
        assert!(v["final_deadline_ms"].is_null());
        assert!(v.get("detector").is_none());
    }

    #[test]
    fn a_plain_buffering_session_is_buffered_with_client_endpointing() {
        let s = shared();
        let l = live("openai", "whisper-1", LiveSessionKind::Plain);
        let v = ready_stt(&l, &ReadyFacts::default(), s.map, 6);
        assert_eq!(v["transcription_mode"], "buffered");
        assert_eq!(v["endpointing"], "client");
        assert_eq!(v["interim_results"], "none");
        let w = config_warnings(&l);
        assert!(matches!(&w[0], OutgoingMessage::ConfigWarning { code, .. } if code == "stt_buffered_until_commit"));
    }

    #[test]
    fn a_refusal_is_a_recoverable_coded_error_with_the_reason() {
        let s = SttLiveShared::from_lookup(|_| None, None).unwrap();
        let l = resolve_session(
            &s,
            &LiveRequest {
                provider: "openai".into(),
                model: "gpt-transcribe".into(),
                language: "en".into(),
                encoding: "linear16".into(),
                sample_rate: 16_000,
                channels: 1,
                kind: LiveSessionKind::Agent { manual: false },
                requested_mode: None,
                leg: None,
                interim_results: None,
                keyterms: Vec::new(),
                prompt: None,
                tuning: EndpointTuning::default(),
                extras: BTreeMap::new(),
            },
        );
        let LiveDecision::Refused(r) = &l.decision else { panic!("{:?}", l.decision) };
        let m = refusal_error(&l, r, s.map, 6);
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["type"], "error");
        assert_eq!(j["code"], "stt_live_unsupported");
        assert_eq!(j["recoverable"], true);
        assert_eq!(j["details"]["reason"], "not_covered_yet");
        assert!(j["message"].as_str().unwrap().starts_with("stt_live_unsupported: "));
    }

    #[test]
    fn a_fatal_engine_error_becomes_stt_unavailable() {
        let m = stt_unavailable("Provider error: stt_unavailable (credential_rejected): 401 bad key").unwrap();
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["code"], "stt_unavailable");
        assert_eq!(j["recoverable"], false);
        assert_eq!(j["details"]["reason"], "credential_rejected");
        assert!(stt_unavailable("Provider error: socket closed").is_none());
    }

    #[test]
    fn an_uncoded_error_keeps_its_exact_bytes() {
        let m = OutgoingMessage::Error { message: "boom".into() };
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"type":"error","message":"boom"}"#);
    }

    #[test]
    fn speech_events_follow_the_contract_order() {
        let mut v = VadEvents::new(500);
        let started = |s| SpeechActivity::Started { turn_id: 7, at_sample: 192_000, sustained_ms: s, at_mono_ms: 0 };
        let names = |ms: Vec<OutgoingMessage>| -> Vec<String> {
            ms.into_iter()
                .map(|m| match m {
                    OutgoingMessage::VadEvent { event, .. } => event,
                    _ => unreachable!(),
                })
                .collect()
        };
        assert_eq!(names(v.on_activity(&started(224), true)), vec!["speech_start"]);
        assert_eq!(names(v.on_activity(&started(384), true)), Vec::<String>::new());
        assert_eq!(names(v.on_activity(&started(512), true)), vec!["turn_start"]);
        let stop = SpeechActivity::Stopped { turn_id: 7, at_sample: 230_400, voiced_ms: 2400, will_upload: true, at_mono_ms: 0 };
        assert_eq!(names(v.on_activity(&stop, false)), vec!["speech_end"]);
        let closed = SpeechActivity::TurnClosed {
            turn_id: 7,
            had_text: false,
            result_follows: false,
            segments: 1,
            gaps: 1,
            lost_voiced_ms: 2400,
            reason: TurnCloseReason::MaxEndpointing,
            speech_end_mono_ms: 0,
        };
        let m = v.on_activity(&closed, false);
        let j = serde_json::to_value(&m[0]).unwrap();
        assert_eq!(j, json!({"type":"vad_event","event":"turn_closed","turn_id":7,"had_transcript":false,"reason":"transcription_failed"}));
    }

    #[test]
    fn a_lost_segment_becomes_one_stt_warning_and_text_never_does() {
        let mut o = SegmentOutcome {
            turn_id: 3,
            seq: 9,
            index_in_turn: 1,
            speech_segments: 1,
            turn_final: true,
            voiced_ms: 1200,
            audio_ms: 2000,
            joined_text_offset: 21,
            uploaded_seconds: 2.0,
            billed_seconds: 2.0,
            timings: Default::default(),
            kind: SegmentResultKind::TimedOut,
            retries: 1,
            overlapped_agent_speech: false,
            suspect: false,
            cut: crate::core::stt::speech_activity::CutReason::Pause,
            short: false,
            detector: crate::core::stt::speech_activity::DetectorKind::Silero,
            vendor_request_id: None,
        };
        let j = serde_json::to_value(segment_failed(&o).unwrap()).unwrap();
        assert_eq!(j["type"], "stt_warning");
        assert_eq!(j["detail"]["text_offset"], 21);
        assert_eq!(j["detail"]["result"], "timed_out");
        o.kind = SegmentResultKind::Text;
        assert!(segment_failed(&o).is_none());
    }
}

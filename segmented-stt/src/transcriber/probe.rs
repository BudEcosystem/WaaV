//! The setup probe (Release 2): one real request with a silent clip during setup, before the
//! caller speaks, for a row the map only guessed or a self-hosted server.
//!
//! Without it a mistyped model is admitted and fails at the caller's first utterance. The probe
//! sends half a second of silence with the session's fields; a vendor that refuses an optional
//! field gets the request again without it (then with only the file and the model), and the
//! repair is remembered for the session's uploads. The verdict is cached per target and
//! credential: an answer for half an hour, a refusal for two minutes, an unreachable vendor not at
//! all (the session goes ahead and the attempt loop handles it).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::attempts::RepairMemory;
use super::{RequestProgress, SegmentAudio, SegmentContext, SegmentTranscriber};
use crate::types::ErrorClass;

/// What the probe found.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeVerdict {
    /// The model answered with the session's fields.
    Served,
    /// The model answered once these optional fields were left out (`stt_fields_reduced`).
    FieldsReduced { not_sent: Vec<String> },
    /// The vendor does not serve the model (`stt_live_unsupported`, `model_not_served`).
    ModelNotServed { message: String },
    /// The vendor refused the credential.
    CredentialRejected { message: String },
    /// No usable answer (a timeout, an outage): the session goes ahead.
    Unknown { message: String },
}

/// The silent clip: half a second at 16 kHz.
const CLIP_SAMPLES: usize = 8_000;
const SERVED_TTL: Duration = Duration::from_secs(30 * 60);
const REFUSED_TTL: Duration = Duration::from_secs(2 * 60);

/// Probe one target. At most three requests: the session's fields, without a field the vendor
/// named, and the minimal request.
pub async fn probe(
    transcriber: &dyn SegmentTranscriber,
    ctx: &SegmentContext,
    timeout: Duration,
    repairs: &RepairMemory,
) -> ProbeVerdict {
    let audio = SegmentAudio::new(vec![0; CLIP_SAMPLES]);
    let info = transcriber.info().clone();
    let mut ctx = ctx.clone();
    let mut omitted: Vec<String> = Vec::new();
    for _ in 0..3 {
        let progress = RequestProgress::new();
        match transcriber
            .transcribe(&audio, &ctx, timeout, &progress)
            .await
        {
            Ok(_) if ctx.minimal => {
                repairs.remember_minimal(&info);
                return ProbeVerdict::FieldsReduced {
                    not_sent: info.droppable_fields.clone(),
                };
            }
            Ok(_) if !omitted.is_empty() => {
                repairs.remember_omitted(&info, omitted.clone());
                return ProbeVerdict::FieldsReduced { not_sent: omitted };
            }
            Ok(_) => return ProbeVerdict::Served,
            Err(e) => match e.class {
                ErrorClass::ModelNotServed => {
                    return ProbeVerdict::ModelNotServed { message: e.message };
                }
                ErrorClass::Auth => {
                    return ProbeVerdict::CredentialRejected { message: e.message };
                }
                ErrorClass::BadRequest if !ctx.minimal => match e.refused_field {
                    Some(f) if !omitted.contains(&f) => {
                        ctx.omit_fields.push(f.clone());
                        omitted.push(f);
                    }
                    _ => ctx.minimal = true,
                },
                _ => return ProbeVerdict::Unknown { message: e.message },
            },
        }
    }
    ProbeVerdict::Unknown {
        message: "the vendor refused every form of the request".into(),
    }
}

/// Verdicts per target and credential.
#[derive(Debug, Default)]
pub struct ProbeCache {
    map: Mutex<HashMap<String, (Instant, ProbeVerdict)>>,
}

impl ProbeCache {
    pub fn get(&self, key: &str) -> Option<ProbeVerdict> {
        let mut g = self.map.lock();
        let (at, verdict) = g.get(key)?.clone();
        let ttl = match verdict {
            ProbeVerdict::Served | ProbeVerdict::FieldsReduced { .. } => SERVED_TTL,
            _ => REFUSED_TTL,
        };
        if at.elapsed() > ttl {
            g.remove(key);
            return None;
        }
        Some(verdict)
    }

    pub fn put(&self, key: &str, verdict: &ProbeVerdict) {
        if matches!(verdict, ProbeVerdict::Unknown { .. }) {
            return;
        }
        self.map
            .lock()
            .insert(key.to_string(), (Instant::now(), verdict.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcriber::testing::{FakeStep, FakeTranscriber};
    use crate::transcriber::{SegmentError, SegmentTranscriber, TranscriberInfo};

    fn info() -> TranscriberInfo {
        let mut i = TranscriberInfo::file("openai_transcriptions", "https://h:443", "m");
        i.droppable_fields = vec!["prompt".into(), "language".into()];
        i
    }

    fn fake(steps: Vec<FakeStep>) -> std::sync::Arc<FakeTranscriber> {
        FakeTranscriber::new(info(), steps)
    }

    fn ctx() -> SegmentContext {
        SegmentContext {
            language: Some("en".into()),
            prompt: Some("Bud".into()),
            ..Default::default()
        }
    }

    const T: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn a_served_model_answers_the_silent_clip() {
        let f = fake(vec![FakeStep::text("", Duration::ZERO)]);
        assert_eq!(
            probe(&*f, &ctx(), T, &RepairMemory::default()).await,
            ProbeVerdict::Served
        );
        assert_eq!(f.calls(), 1);
        assert_eq!(f.audio_ms(), vec![500], "half a second");
    }

    #[tokio::test]
    async fn a_mistyped_model_is_not_served_and_a_bad_key_is_named() {
        let f = fake(vec![FakeStep::error(
            SegmentError::from_status(404, "model not found"),
            Duration::ZERO,
        )]);
        assert!(matches!(
            probe(&*f, &ctx(), T, &RepairMemory::default()).await,
            ProbeVerdict::ModelNotServed { .. }
        ));
        let f = fake(vec![FakeStep::error(
            SegmentError::from_status(401, "bad key"),
            Duration::ZERO,
        )]);
        assert!(matches!(
            probe(&*f, &ctx(), T, &RepairMemory::default()).await,
            ProbeVerdict::CredentialRejected { .. }
        ));
    }

    #[tokio::test]
    async fn a_refused_field_is_left_out_and_remembered_for_the_session() {
        let mut refused = SegmentError::from_status(400, "unknown parameter: prompt");
        refused.refused_field = Some("prompt".into());
        let f = fake(vec![
            FakeStep::error(refused, Duration::ZERO),
            FakeStep::text("", Duration::ZERO),
        ]);
        let repairs = RepairMemory::default();
        assert_eq!(
            probe(&*f, &ctx(), T, &repairs).await,
            ProbeVerdict::FieldsReduced {
                not_sent: vec!["prompt".into()]
            }
        );
        assert_eq!(f.contexts()[1].omit_fields, vec!["prompt".to_string()]);
        // The session's uploads omit it too.
        let mut later = ctx();
        repairs.apply(f.info(), &mut later);
        assert_eq!(later.omit_fields, vec!["prompt".to_string()]);
    }

    #[tokio::test]
    async fn a_refusal_naming_no_field_falls_back_to_the_minimal_request() {
        let f = fake(vec![
            FakeStep::error(
                SegmentError::from_status(400, "bad request"),
                Duration::ZERO,
            ),
            FakeStep::text("", Duration::ZERO),
        ]);
        let v = probe(&*f, &ctx(), T, &RepairMemory::default()).await;
        assert_eq!(
            v,
            ProbeVerdict::FieldsReduced {
                not_sent: vec!["prompt".into(), "language".into()]
            }
        );
        assert!(f.contexts()[1].minimal);
    }

    #[tokio::test]
    async fn an_outage_is_unknown_and_never_cached() {
        let f = fake(vec![FakeStep::error(
            SegmentError::from_status(503, "down"),
            Duration::ZERO,
        )]);
        let v = probe(&*f, &ctx(), T, &RepairMemory::default()).await;
        assert!(matches!(v, ProbeVerdict::Unknown { .. }));
        let cache = ProbeCache::default();
        cache.put("k", &v);
        assert_eq!(cache.get("k"), None);
        cache.put("k", &ProbeVerdict::Served);
        assert_eq!(cache.get("k"), Some(ProbeVerdict::Served));
    }

    #[test]
    fn the_fake_reports_its_info() {
        assert_eq!(fake(vec![]).info().adapter, "openai_transcriptions");
    }
}

//! A scripted transcriber for tests: each call plays the next step of a script.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use super::{
    RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo,
};

/// One scripted request.
#[derive(Debug, Clone)]
pub struct FakeStep {
    /// When the response headers arrive; `None` means never (a stalled request).
    pub headers_after: Option<Duration>,
    /// When the whole answer arrives (at least `headers_after`).
    pub answer_after: Duration,
    pub result: Result<SegmentTranscript, SegmentError>,
}

impl FakeStep {
    pub fn text(text: &str, after: Duration) -> Self {
        Self {
            headers_after: Some(after),
            answer_after: after,
            result: Ok(SegmentTranscript {
                text: text.to_string(),
                ..Default::default()
            }),
        }
    }

    pub fn error(err: SegmentError, after: Duration) -> Self {
        Self {
            headers_after: Some(after),
            answer_after: after,
            result: Err(err),
        }
    }

    /// A request that never sends headers.
    pub fn stall() -> Self {
        Self {
            headers_after: None,
            answer_after: Duration::from_secs(3600),
            result: Err(SegmentError::new(crate::types::ErrorClass::Timeout, "stalled")),
        }
    }
}

/// A transcriber that answers from a script. Calls past the end of the script get `fallback`.
pub struct FakeTranscriber {
    info: TranscriberInfo,
    script: Mutex<VecDeque<FakeStep>>,
    fallback: Mutex<Option<Arc<dyn Fn(&SegmentAudio, &SegmentContext) -> FakeStep + Send + Sync>>>,
    calls: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    contexts: Mutex<Vec<SegmentContext>>,
    audio_ms: Mutex<Vec<u32>>,
}

impl FakeTranscriber {
    pub fn new(info: TranscriberInfo, steps: impl IntoIterator<Item = FakeStep>) -> Arc<Self> {
        Arc::new(Self {
            info,
            script: Mutex::new(steps.into_iter().collect()),
            fallback: Mutex::new(None),
            calls: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            contexts: Mutex::new(Vec::new()),
            audio_ms: Mutex::new(Vec::new()),
        })
    }

    pub fn file(steps: impl IntoIterator<Item = FakeStep>) -> Arc<Self> {
        Self::new(TranscriberInfo::file("fake", "https://fake.example:443", "fake-model"), steps)
    }

    /// Answer every unscripted call with this function.
    pub fn set_fallback(&self, f: impl Fn(&SegmentAudio, &SegmentContext) -> FakeStep + Send + Sync + 'static) {
        *self.fallback.lock() = Some(Arc::new(f));
    }

    pub fn push(&self, step: FakeStep) {
        self.script.lock().push_back(step);
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    pub fn contexts(&self) -> Vec<SegmentContext> {
        self.contexts.lock().clone()
    }

    pub fn audio_ms(&self) -> Vec<u32> {
        self.audio_ms.lock().clone()
    }
}

struct InFlight<'a>(&'a AtomicUsize);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl SegmentTranscriber for FakeTranscriber {
    fn info(&self) -> &TranscriberInfo {
        &self.info
    }

    async fn transcribe(
        &self,
        audio: &SegmentAudio,
        ctx: &SegmentContext,
        timeout: Duration,
        progress: &RequestProgress,
    ) -> Result<SegmentTranscript, SegmentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let _guard = InFlight(&self.in_flight);
        self.contexts.lock().push(ctx.clone());
        self.audio_ms.lock().push(audio.audio_ms());
        let step = {
            let next = self.script.lock().pop_front();
            match next {
                Some(s) => s,
                None => match self.fallback.lock().clone() {
                    Some(f) => f(audio, ctx),
                    None => FakeStep::text("", Duration::from_millis(10)),
                },
            }
        };
        progress.mark_sent();
        let run = async {
            if let Some(h) = step.headers_after {
                tokio::time::sleep(h).await;
                progress.mark_headers();
                tokio::time::sleep(step.answer_after.saturating_sub(h)).await;
            } else {
                tokio::time::sleep(step.answer_after).await;
            }
            step.result.clone()
        };
        match tokio::time::timeout(timeout, run).await {
            Ok(r) => r,
            Err(_) => Err(SegmentError::new(crate::types::ErrorClass::Timeout, "request limit")
                .with_phase(if progress.headers_received() {
                    super::RequestPhase::Headers
                } else {
                    super::RequestPhase::Sent
                })),
        }
    }
}

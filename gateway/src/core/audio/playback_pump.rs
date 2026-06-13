//! A-G6: playback pump — meters a [`PlaybackQueue`] to the transport.
//!
//! When uninterruptible playback is enabled, TTS chunks are enqueued instead of
//! handed straight to the transport. This pump pops them in order and delivers
//! each to the egress sink, then **paces** by the chunk's playback duration so
//! the transport buffer never holds more than ~`lead` of audio. That shallow
//! buffer is what makes a selective barge-in possible: the bulk of pending
//! interruptible audio is still in the gateway queue (droppable via
//! [`PlaybackQueue::clear_interruptible`]), and only ≤`lead` of already-delivered
//! audio rides in the transport (the bounded residual, matching E-G2's goal).
//!
//! The pump owns its task and aborts it on drop (mirrors `HeartbeatMonitor`), so
//! holding it in the VoiceManager ties its lifetime to the session.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::playback_queue::{PlaybackQueue, PlaybackUnit};
use crate::core::tts::AudioData;

/// The egress sink the pump delivers each chunk to (the existing transport
/// delivery path — LiveKit / WS).
pub type PlaybackSink =
    Arc<dyn Fn(AudioData) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Default lookahead: deliver the next chunk this far before the current one is
/// estimated to finish, keeping playout gapless while bounding the transport
/// residual cleared on barge-in.
pub const DEFAULT_PLAYBACK_LEAD: Duration = Duration::from_millis(40);

/// A cheap, cloneable handle for feeding the pump from the TTS callback hot path
/// (the `Arc`s are shared with the running pump). Holds no task, so the wrapper
/// closure can capture it without owning the pump.
#[derive(Clone)]
pub struct PlaybackEnqueuer {
    queue: Arc<PlaybackQueue>,
    notify: Arc<Notify>,
}

impl PlaybackEnqueuer {
    /// Append a unit and wake the pump.
    pub fn enqueue(&self, unit: PlaybackUnit) {
        self.queue.enqueue(unit);
        self.notify.notify_one();
    }

    /// The shared queue (for `clear_interruptible` / `clear_all` on barge-in).
    pub fn queue(&self) -> &Arc<PlaybackQueue> {
        &self.queue
    }
}

/// Meters a [`PlaybackQueue`] to a [`PlaybackSink`].
pub struct PlaybackPump {
    enqueuer: PlaybackEnqueuer,
    handle: Option<JoinHandle<()>>,
}

impl PlaybackPump {
    /// Spawn the pump over `queue`, delivering to `sink`. `fallback_rate` feeds
    /// [`AudioData::playback_ms`] when a chunk has no declared duration; `lead`
    /// is the transport lookahead (see [`DEFAULT_PLAYBACK_LEAD`]).
    pub fn spawn(
        queue: Arc<PlaybackQueue>,
        sink: PlaybackSink,
        fallback_rate: u32,
        lead: Duration,
    ) -> Self {
        let notify = Arc::new(Notify::new());
        let q = queue.clone();
        let n = notify.clone();
        let lead_ms = lead.as_millis() as u64;
        let handle = tokio::spawn(async move {
            loop {
                match q.pop_front() {
                    Some(unit) => {
                        let interruptible = unit.interruptible;
                        let dur_ms = unit.audio.playback_ms(fallback_rate) as u64;
                        sink(unit.audio).await;
                        // Pace: hold the next chunk until this one is `lead` from
                        // done, so the transport stays ≤ lead ahead.
                        let pace = dur_ms.saturating_sub(lead_ms);
                        if pace > 0 {
                            tokio::time::sleep(Duration::from_millis(pace)).await;
                        }
                        // Playout complete: an uninterruptible unit is no longer
                        // protected (its protection spanned queue + this playout).
                        q.mark_played(interruptible);
                    }
                    // Queue drained — park until the next enqueue wakes us. The
                    // notify_one stored by a racing enqueue is not lost (tokio
                    // Notify holds one permit).
                    None => n.notified().await,
                }
            }
        });
        Self {
            enqueuer: PlaybackEnqueuer { queue, notify },
            handle: Some(handle),
        }
    }

    /// Enqueue a unit and wake the pump.
    pub fn enqueue(&self, unit: PlaybackUnit) {
        self.enqueuer.enqueue(unit);
    }

    /// A cheap cloneable handle for enqueuing from the TTS callback hot path.
    pub fn enqueuer(&self) -> PlaybackEnqueuer {
        self.enqueuer.clone()
    }

    /// The underlying queue (for `clear_interruptible` / `clear_all` on barge-in).
    pub fn queue(&self) -> &Arc<PlaybackQueue> {
        self.enqueuer.queue()
    }
}

impl Drop for PlaybackPump {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn audio(tag: u8, dur_ms: u32) -> AudioData {
        AudioData {
            data: vec![tag; 8],
            sample_rate: 24_000,
            format: "pcm".to_string(),
            duration_ms: Some(dur_ms),
        }
    }

    /// A sink that records delivered chunk ids and signals each delivery on a
    /// channel so a test can synchronize deterministically.
    fn recording_sink() -> (
        PlaybackSink,
        Arc<Mutex<Vec<u8>>>,
        tokio::sync::mpsc::UnboundedReceiver<u8>,
    ) {
        let delivered = Arc::new(Mutex::new(Vec::<u8>::new()));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let d = delivered.clone();
        let sink: PlaybackSink = Arc::new(move |a: AudioData| {
            let d = d.clone();
            let tx = tx.clone();
            Box::pin(async move {
                let id = a.data[0];
                d.lock().unwrap().push(id);
                let _ = tx.send(id);
            })
        });
        (sink, delivered, rx)
    }

    #[tokio::test(start_paused = true)]
    async fn pump_delivers_in_order() {
        let queue = Arc::new(PlaybackQueue::new());
        let (sink, delivered, mut rx) = recording_sink();
        let pump = PlaybackPump::spawn(queue, sink, 24_000, DEFAULT_PLAYBACK_LEAD);

        pump.enqueue(PlaybackUnit::interruptible(audio(1, 20)));
        pump.enqueue(PlaybackUnit::interruptible(audio(2, 20)));
        pump.enqueue(PlaybackUnit::interruptible(audio(3, 20)));

        for expected in [1u8, 2, 3] {
            assert_eq!(rx.recv().await, Some(expected));
        }
        assert_eq!(*delivered.lock().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test(start_paused = true)]
    async fn clear_interruptible_mid_stream_drops_only_interruptible() {
        // Enqueue [int1, uninterruptible, int2]. After the pump delivers int1
        // (and is pacing), a barge-in clears interruptible: int2 is dropped from
        // the queue, the uninterruptible unit is kept and still delivered.
        let queue = Arc::new(PlaybackQueue::new());
        let (sink, delivered, mut rx) = recording_sink();
        let pump = PlaybackPump::spawn(queue.clone(), sink, 24_000, DEFAULT_PLAYBACK_LEAD);

        pump.enqueue(PlaybackUnit::interruptible(audio(1, 60)));
        pump.enqueue(PlaybackUnit::uninterruptible(audio(2, 60)));
        pump.enqueue(PlaybackUnit::interruptible(audio(3, 60)));

        // Wait until int1 is delivered (pump is now pacing on its 60ms chunk).
        assert_eq!(rx.recv().await, Some(1));

        // Barge-in: drop interruptible. int1 already popped; int2 dropped; the
        // uninterruptible unit (2) survives.
        let dropped = queue.clear_interruptible();
        assert_eq!(dropped, 1, "only int2 is still queued and droppable");
        assert!(queue.has_uninterruptible_pending());

        // The uninterruptible unit is delivered next; int2 never is.
        assert_eq!(rx.recv().await, Some(2));
        // Give the pump a chance to (not) deliver anything else.
        tokio::time::advance(Duration::from_millis(500)).await;
        assert_eq!(
            *delivered.lock().unwrap(),
            vec![1, 2],
            "int2 was dropped; only int1 + the uninterruptible unit played"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn pump_resumes_after_idle() {
        // The pump parks when the queue drains and resumes on the next enqueue
        // (no lost wakeup).
        let queue = Arc::new(PlaybackQueue::new());
        let (sink, _delivered, mut rx) = recording_sink();
        let pump = PlaybackPump::spawn(queue, sink, 24_000, DEFAULT_PLAYBACK_LEAD);

        pump.enqueue(PlaybackUnit::interruptible(audio(1, 20)));
        assert_eq!(rx.recv().await, Some(1));
        // Let it drain and park.
        tokio::time::advance(Duration::from_millis(100)).await;
        // Enqueue again — must wake and deliver.
        pump.enqueue(PlaybackUnit::interruptible(audio(2, 20)));
        assert_eq!(rx.recv().await, Some(2));
    }
}

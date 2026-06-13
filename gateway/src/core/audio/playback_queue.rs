//! A-G6: per-chunk playback queue with uninterruptible units.
//!
//! WaaV normally hands each TTS chunk straight to the transport (lowest
//! latency). That makes barge-in a *blanket* clear: every transport buffer is
//! flushed at once, so there is no way to let one chunk — e.g. a
//! "this call is recorded" disclaimer — survive while the rest of the bot's
//! speech is cut.
//!
//! This queue is the gateway-side playout authority for the uninterruptible
//! path (Pipecat `UninterruptibleFrame` + `FrameQueue.has_uninterruptible`
//! parity). Each [`PlaybackUnit`] is tagged [`interruptible`](PlaybackUnit::interruptible);
//! a pump meters units to the transport so the transport buffer stays shallow,
//! and a barge-in calls [`clear_interruptible`](PlaybackQueue::clear_interruptible)
//! which drops interruptible units while keeping uninterruptible ones queued
//! (they keep pumping). An [`AtomicUsize`] makes "is anything protected still
//! pending?" an O(1) check on the barge-in hot path — no queue scan.
//!
//! This is standardized: the flag lives on the gateway playout unit that EVERY
//! TTS provider feeds through the VoiceManager callback — never per-provider.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::core::tts::AudioData;

/// One unit of audio queued for playout, tagged with whether a barge-in may
/// drop it.
#[derive(Debug, Clone)]
pub struct PlaybackUnit {
    /// The audio chunk to play.
    pub audio: AudioData,
    /// `false` ⇒ this unit survives a barge-in clear (e.g. a legal disclaimer).
    pub interruptible: bool,
}

impl PlaybackUnit {
    /// An interruptible unit (the default — ordinary bot speech).
    pub fn interruptible(audio: AudioData) -> Self {
        Self {
            audio,
            interruptible: true,
        }
    }

    /// A unit that survives barge-in.
    pub fn uninterruptible(audio: AudioData) -> Self {
        Self {
            audio,
            interruptible: false,
        }
    }
}

/// FIFO queue of [`PlaybackUnit`]s with O(1) uninterruptible-presence tracking.
#[derive(Default)]
pub struct PlaybackQueue {
    inner: Mutex<VecDeque<PlaybackUnit>>,
    /// Count of uninterruptible units currently queued — read on the barge-in
    /// hot path without locking/scanning (the queue lock is only for mutation).
    uninterruptible_pending: AtomicUsize,
}

impl PlaybackQueue {
    /// A fresh, empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a unit for playout.
    pub fn enqueue(&self, unit: PlaybackUnit) {
        if !unit.interruptible {
            self.uninterruptible_pending.fetch_add(1, Ordering::Release);
        }
        self.lock().push_back(unit);
    }

    /// Pop the next unit for the pump to deliver, keeping the uninterruptible
    /// count in sync.
    pub fn pop_front(&self) -> Option<PlaybackUnit> {
        let unit = self.lock().pop_front();
        if let Some(u) = &unit {
            if !u.interruptible {
                self.uninterruptible_pending.fetch_sub(1, Ordering::Release);
            }
        }
        unit
    }

    /// Barge-in: drop every interruptible unit, **keep** uninterruptible ones in
    /// order (they keep playing). Returns how many units were dropped. The scan
    /// is O(pending) (the queue is shallow — the pump keeps it drained); the
    /// "anything protected?" check stays O(1).
    pub fn clear_interruptible(&self) -> usize {
        let mut q = self.lock();
        let before = q.len();
        q.retain(|u| !u.interruptible);
        // uninterruptible_pending is unchanged — we kept all of them.
        before - q.len()
    }

    /// Blanket clear (back-compat): drop everything, including uninterruptible
    /// units. Returns how many units were dropped.
    pub fn clear_all(&self) -> usize {
        let mut q = self.lock();
        let n = q.len();
        q.clear();
        self.uninterruptible_pending.store(0, Ordering::Release);
        n
    }

    /// O(1): is at least one uninterruptible unit still queued? Read this on the
    /// barge-in path to decide whether a selective clear is even needed.
    pub fn has_uninterruptible_pending(&self) -> bool {
        self.uninterruptible_pending.load(Ordering::Acquire) > 0
    }

    /// Number of queued units.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<PlaybackUnit>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(tag: u8) -> AudioData {
        AudioData {
            data: vec![tag; 16],
            sample_rate: 24_000,
            format: "pcm".to_string(),
            duration_ms: Some(20),
        }
    }

    /// First byte of a unit's data — used to assert ordering/identity.
    fn id(unit: &PlaybackUnit) -> u8 {
        unit.audio.data[0]
    }

    #[test]
    fn enqueue_pop_is_fifo() {
        let q = PlaybackQueue::new();
        q.enqueue(PlaybackUnit::interruptible(audio(1)));
        q.enqueue(PlaybackUnit::interruptible(audio(2)));
        q.enqueue(PlaybackUnit::interruptible(audio(3)));
        assert_eq!(q.len(), 3);
        assert_eq!(id(&q.pop_front().unwrap()), 1);
        assert_eq!(id(&q.pop_front().unwrap()), 2);
        assert_eq!(id(&q.pop_front().unwrap()), 3);
        assert!(q.pop_front().is_none());
        assert!(q.is_empty());
    }

    #[test]
    fn clear_preserves_uninterruptible_chunk() {
        // The headline A-G6 behaviour: a disclaimer (uninterruptible) survives a
        // barge-in while surrounding interruptible speech is dropped — order kept.
        let q = PlaybackQueue::new();
        q.enqueue(PlaybackUnit::interruptible(audio(1))); // ordinary speech
        q.enqueue(PlaybackUnit::uninterruptible(audio(2))); // "call is recorded"
        q.enqueue(PlaybackUnit::interruptible(audio(3))); // more speech

        let dropped = q.clear_interruptible();
        assert_eq!(dropped, 2, "both interruptible units dropped");
        assert_eq!(q.len(), 1, "the uninterruptible unit remains");
        assert!(q.has_uninterruptible_pending());
        assert_eq!(id(&q.pop_front().unwrap()), 2, "the kept unit is the disclaimer");
        assert!(!q.has_uninterruptible_pending(), "count drops to 0 after it pops");
    }

    #[test]
    fn clear_blanket_still_clears_all() {
        // Back-compat: clear_all drops everything, including uninterruptible.
        let q = PlaybackQueue::new();
        q.enqueue(PlaybackUnit::interruptible(audio(1)));
        q.enqueue(PlaybackUnit::uninterruptible(audio(2)));
        assert!(q.has_uninterruptible_pending());

        let dropped = q.clear_all();
        assert_eq!(dropped, 2);
        assert!(q.is_empty());
        assert!(!q.has_uninterruptible_pending(), "blanket clear resets the count");
    }

    #[test]
    fn has_uninterruptible_tracks_across_ops() {
        let q = PlaybackQueue::new();
        assert!(!q.has_uninterruptible_pending());
        q.enqueue(PlaybackUnit::interruptible(audio(1)));
        assert!(!q.has_uninterruptible_pending(), "interruptible doesn't set the flag");
        q.enqueue(PlaybackUnit::uninterruptible(audio(2)));
        q.enqueue(PlaybackUnit::uninterruptible(audio(3)));
        assert!(q.has_uninterruptible_pending());
        // Pop one uninterruptible — still one left.
        let _ = q.pop_front(); // drops audio(1) (interruptible)
        assert!(q.has_uninterruptible_pending());
        let _ = q.pop_front(); // drops audio(2) (uninterruptible)
        assert!(q.has_uninterruptible_pending(), "one uninterruptible still queued");
        let _ = q.pop_front(); // drops audio(3) (uninterruptible)
        assert!(!q.has_uninterruptible_pending(), "now none");
    }

    #[test]
    fn clear_interruptible_on_all_interruptible_empties() {
        let q = PlaybackQueue::new();
        q.enqueue(PlaybackUnit::interruptible(audio(1)));
        q.enqueue(PlaybackUnit::interruptible(audio(2)));
        assert_eq!(q.clear_interruptible(), 2);
        assert!(q.is_empty());
        assert!(!q.has_uninterruptible_pending());
    }

    #[test]
    fn clear_interruptible_keeps_multiple_uninterruptible_in_order() {
        let q = PlaybackQueue::new();
        q.enqueue(PlaybackUnit::uninterruptible(audio(10)));
        q.enqueue(PlaybackUnit::interruptible(audio(1)));
        q.enqueue(PlaybackUnit::uninterruptible(audio(20)));
        q.enqueue(PlaybackUnit::interruptible(audio(2)));
        q.enqueue(PlaybackUnit::uninterruptible(audio(30)));

        assert_eq!(q.clear_interruptible(), 2);
        assert_eq!(id(&q.pop_front().unwrap()), 10);
        assert_eq!(id(&q.pop_front().unwrap()), 20);
        assert_eq!(id(&q.pop_front().unwrap()), 30);
        assert!(q.is_empty());
    }

    #[test]
    fn barge_in_no_queue_scan_when_nothing_protected() {
        // When no uninterruptible audio is queued, the barge-in fast-path can
        // read has_uninterruptible_pending() (O(1)) and skip straight to a
        // blanket clear without scanning.
        let q = PlaybackQueue::new();
        for i in 0..1000u32 {
            q.enqueue(PlaybackUnit::interruptible(audio(i as u8)));
        }
        assert!(!q.has_uninterruptible_pending(), "O(1) check, no scan");
        assert_eq!(q.clear_all(), 1000);
    }
}

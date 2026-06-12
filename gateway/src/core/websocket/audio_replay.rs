//! Reconnect audio-replay buffer (PIPECAT_FIX_PLAN D-G1).
//!
//! Streaming STT providers are STATELESS across connections: any audio sent
//! after the last finalized transcript but before a transport drop was never
//! durably transcribed — without replay, those words are silently lost (the
//! user spoke, the bot never heard). This buffer keeps exactly that at-risk
//! tail and is replayed into the fresh socket after a reconnect.
//!
//! Contract (the standardized semantics every provider wires identically):
//! - **push** every audio chunk at the moment it is written to the socket
//!   (NOT at `send_audio` — channel-queued audio survives a drop on its own).
//! - **clear on every finalized transcript** (`is_final`): the provider has
//!   durably transcribed everything up to that point, so the buffer only
//!   ever holds the un-finalized tail. During silence (no finals) the byte
//!   cap bounds growth.
//! - **snapshot (not drain) on reconnect**: replay must not clear — the
//!   replayed audio stays at-risk until the provider finalizes it; if the
//!   NEW connection also drops first, the next reconnect replays again.
//!
//! Trade-offs (deliberate):
//! - A final the provider sent that we never received ⇒ the replay can
//!   produce duplicate words once. Rare, and strictly better than the
//!   alternative (losing the words entirely); the voice manager's
//!   duplicate window absorbs the common case.
//! - An utterance longer than the cap with no finals loses its oldest
//!   chunks from the replay (bounded memory beats unbounded).
//!
//! `Sync` by design (internal mutex): one `Arc<AudioReplayBuffer>` per
//! provider instance, shared between the transport's send loop, the
//! response parser (clear), and the reconnect hook (snapshot).

use std::collections::VecDeque;

use bytes::Bytes;
use parking_lot::Mutex;

/// Default byte budget: 5 s of 16 kHz mono PCM16. Generous enough to cover
/// a dial + backoff gap at common voice rates without holding meaningful
/// memory per session.
pub const DEFAULT_REPLAY_CAP_BYTES: usize = 16_000 * 2 * 5;

#[derive(Default)]
struct Inner {
    chunks: VecDeque<Bytes>,
    bytes: usize,
}

/// Bounded ring of the un-finalized audio tail. See module docs.
pub struct AudioReplayBuffer {
    inner: Mutex<Inner>,
    cap_bytes: usize,
}

impl std::fmt::Debug for AudioReplayBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("AudioReplayBuffer")
            .field("chunks", &inner.chunks.len())
            .field("bytes", &inner.bytes)
            .field("cap_bytes", &self.cap_bytes)
            .finish()
    }
}

impl Default for AudioReplayBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_REPLAY_CAP_BYTES)
    }
}

impl AudioReplayBuffer {
    pub fn new(cap_bytes: usize) -> Self {
        Self { inner: Mutex::new(Inner::default()), cap_bytes: cap_bytes.max(1) }
    }

    /// Record a chunk that was just written to the socket. Evicts the OLDEST
    /// whole chunks while over the byte cap (the newest audio is always the
    /// most valuable to replay).
    pub fn push(&self, chunk: Bytes) {
        if chunk.is_empty() {
            return;
        }
        let mut inner = self.inner.lock();
        inner.bytes += chunk.len();
        inner.chunks.push_back(chunk);
        while inner.bytes > self.cap_bytes && inner.chunks.len() > 1 {
            if let Some(old) = inner.chunks.pop_front() {
                inner.bytes -= old.len();
            }
        }
        // A single chunk larger than the cap is kept as-is: replaying it
        // whole beats slicing PCM mid-sample.
    }

    /// The provider finalized everything sent so far — drop the tail.
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.chunks.clear();
        inner.bytes = 0;
    }

    /// The chunks to re-send into a fresh connection, oldest first. Does NOT
    /// clear (see module docs).
    pub fn snapshot(&self) -> Vec<Bytes> {
        self.inner.lock().chunks.iter().cloned().collect()
    }

    /// Bytes currently held (test/observability).
    pub fn len_bytes(&self) -> usize {
        self.inner.lock().bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(tag: u8, len: usize) -> Bytes {
        Bytes::from(vec![tag; len])
    }

    #[test]
    fn holds_tail_until_cleared() {
        let b = AudioReplayBuffer::new(1024);
        b.push(chunk(1, 100));
        b.push(chunk(2, 100));
        assert_eq!(b.len_bytes(), 200);
        let snap = b.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0][0], 1, "oldest first — replay must preserve order");
        assert_eq!(snap[1][0], 2);
        // Snapshot does NOT clear: a second drop before finals replays again.
        assert_eq!(b.len_bytes(), 200);
        // A finalized transcript clears the at-risk tail.
        b.clear();
        assert_eq!(b.len_bytes(), 0);
        assert!(b.snapshot().is_empty());
    }

    #[test]
    fn evicts_oldest_when_over_cap() {
        let b = AudioReplayBuffer::new(250);
        b.push(chunk(1, 100));
        b.push(chunk(2, 100));
        b.push(chunk(3, 100)); // 300 > 250 → evict chunk 1
        let snap = b.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0][0], 2, "the OLDEST chunk is evicted, newest kept");
        assert_eq!(b.len_bytes(), 200);
    }

    #[test]
    fn oversized_single_chunk_kept_whole() {
        let b = AudioReplayBuffer::new(64);
        b.push(chunk(7, 500));
        assert_eq!(b.len_bytes(), 500, "never slice PCM mid-sample");
        assert_eq!(b.snapshot().len(), 1);
        // The next push evicts it (it alone exceeds the cap).
        b.push(chunk(8, 32));
        let snap = b.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0][0], 8);
    }

    #[test]
    fn empty_chunks_ignored() {
        let b = AudioReplayBuffer::default();
        b.push(Bytes::new());
        assert_eq!(b.len_bytes(), 0);
        assert!(b.snapshot().is_empty());
    }
}

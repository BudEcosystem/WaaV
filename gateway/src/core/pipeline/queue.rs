//! Priority queue for audio frames with bounded capacity.
//!
//! This implementation uses a binary heap for efficient priority ordering
//! with O(log n) insert and O(log n) pop operations.

use bytes::Bytes;
use parking_lot::Mutex;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Instant;

// =============================================================================
// Frame Priority
// =============================================================================

/// Priority levels for audio frames.
///
/// Higher priority frames are processed before lower priority frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[derive(Default)]
pub enum FramePriority {
    /// Lowest priority: Background/fill audio, silence frames.
    /// Processed only when no other frames are available.
    Low = 0,

    /// Default priority: Regular audio frames.
    /// Standard processing order.
    #[default]
    Normal = 1,

    /// Elevated priority: User speech during bot speaking.
    /// Processed before normal frames for responsive interruption.
    High = 2,

    /// Highest priority: Interruption signals, control messages.
    /// Always processed first.
    Critical = 3,
}

impl FramePriority {
    /// Get the numeric value of the priority for comparison.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Create priority from numeric value.
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => FramePriority::Low,
            1 => FramePriority::Normal,
            2 => FramePriority::High,
            3 => FramePriority::Critical,
            _ => FramePriority::Normal,
        }
    }
}

impl PartialOrd for FramePriority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FramePriority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_u8().cmp(&other.as_u8())
    }
}

// =============================================================================
// Priority Frame
// =============================================================================

/// Audio frame with priority metadata.
#[derive(Debug, Clone)]
pub struct PriorityFrame {
    /// The audio data bytes.
    pub data: Bytes,

    /// Priority level for processing order.
    pub priority: FramePriority,

    /// Monotonic timestamp when frame was created (nanoseconds).
    pub created_ns: u64,

    /// Sequence number for FIFO ordering within same priority.
    sequence: u64,

    /// Optional identifier for correlation.
    pub frame_id: Option<u64>,

    /// Duration of audio in milliseconds (if known).
    pub duration_ms: Option<u32>,
}

impl PriorityFrame {
    /// Create a new priority frame with the given data and priority.
    pub fn new(data: Bytes, priority: FramePriority) -> Self {
        Self {
            data,
            priority,
            created_ns: now_monotonic_ns(),
            sequence: 0, // Will be set by queue
            frame_id: None,
            duration_ms: None,
        }
    }

    /// Create a new priority frame with normal priority.
    pub fn normal(data: Bytes) -> Self {
        Self::new(data, FramePriority::Normal)
    }

    /// Create a new priority frame with high priority.
    pub fn high(data: Bytes) -> Self {
        Self::new(data, FramePriority::High)
    }

    /// Create a new priority frame with critical priority.
    pub fn critical(data: Bytes) -> Self {
        Self::new(data, FramePriority::Critical)
    }

    /// Set the frame ID.
    pub fn with_id(mut self, id: u64) -> Self {
        self.frame_id = Some(id);
        self
    }

    /// Set the duration.
    pub fn with_duration_ms(mut self, duration_ms: u32) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    /// Set the sequence number (internal use).
    fn with_sequence(mut self, seq: u64) -> Self {
        self.sequence = seq;
        self
    }

    /// Get the age of the frame in nanoseconds.
    pub fn age_ns(&self) -> u64 {
        now_monotonic_ns().saturating_sub(self.created_ns)
    }

    /// Get the age of the frame in milliseconds.
    pub fn age_ms(&self) -> u64 {
        self.age_ns() / 1_000_000
    }

    /// Check if the frame has exceeded the given TTL in milliseconds.
    pub fn is_expired(&self, ttl_ms: u64) -> bool {
        self.age_ms() >= ttl_ms
    }

    /// Get the data length in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if the frame is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

// Implement ordering for BinaryHeap (max-heap)
// Higher priority first, then FIFO order (lower sequence first)
impl PartialEq for PriorityFrame {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl Eq for PriorityFrame {}

impl PartialOrd for PriorityFrame {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PriorityFrame {
    fn cmp(&self, other: &Self) -> Ordering {
        // First compare by priority (higher first)
        match self.priority.cmp(&other.priority) {
            Ordering::Equal => {
                // Same priority: FIFO order (lower sequence first)
                // Reverse because BinaryHeap is a max-heap
                other.sequence.cmp(&self.sequence)
            }
            other_order => other_order,
        }
    }
}

// =============================================================================
// Frame Priority Queue
// =============================================================================

/// Thread-safe priority queue for audio frames.
///
/// Uses a binary heap with bounded capacity. When capacity is exceeded,
/// the lowest priority frames are dropped first.
pub struct FramePriorityQueue {
    /// The priority queue (binary heap).
    heap: Mutex<BinaryHeap<PriorityFrame>>,

    /// Maximum capacity of the queue.
    capacity: usize,

    /// Sequence counter for FIFO ordering.
    sequence_counter: AtomicU64,

    /// Total frames pushed (including dropped).
    total_pushed: AtomicU64,

    /// Total frames popped.
    total_popped: AtomicU64,

    /// Total frames dropped due to capacity.
    total_dropped: AtomicU64,

    /// Total bytes pushed.
    total_bytes_pushed: AtomicU64,

    /// Total bytes popped.
    total_bytes_popped: AtomicU64,
}

impl FramePriorityQueue {
    /// Create a new queue with the specified capacity.
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of frames to hold. A zero-capacity queue drops every frame.
    pub fn new(capacity: usize) -> Self {
        Self {
            heap: Mutex::new(BinaryHeap::with_capacity(capacity)),
            capacity,
            sequence_counter: AtomicU64::new(0),
            total_pushed: AtomicU64::new(0),
            total_popped: AtomicU64::new(0),
            total_dropped: AtomicU64::new(0),
            total_bytes_pushed: AtomicU64::new(0),
            total_bytes_popped: AtomicU64::new(0),
        }
    }

    /// Push a frame onto the queue.
    ///
    /// If the queue is at capacity, the lowest priority frame is dropped.
    /// Returns `true` if the frame was added, `false` if it was dropped.
    pub fn push(&self, frame: PriorityFrame) -> bool {
        let seq = self.sequence_counter.fetch_add(1, AtomicOrdering::Relaxed);
        let frame = frame.with_sequence(seq);
        let frame_bytes = frame.len() as u64;

        self.total_pushed.fetch_add(1, AtomicOrdering::Relaxed);
        self.total_bytes_pushed
            .fetch_add(frame_bytes, AtomicOrdering::Relaxed);

        if self.capacity == 0 {
            self.total_dropped.fetch_add(1, AtomicOrdering::Relaxed);
            return false;
        }

        let mut heap = self.heap.lock();
        if heap.len() >= self.capacity {
            // Queue is full, need to drop something
            // Find the minimum (lowest priority, oldest) frame in the queue
            let min_priority = self.find_min_priority_locked(&heap);
            if let Some(min_prio) = min_priority
                && frame.priority < min_prio
            {
                // New frame has lower priority than everything in queue, drop it
                self.total_dropped.fetch_add(1, AtomicOrdering::Relaxed);
                return false;
            }
            // Evict the lowest priority frame (which is the minimum)
            // Since BinaryHeap is a max-heap, we need to find and remove the min
            // This is O(n) but acceptable for bounded small queues
            if self.evict_min_locked(&mut heap).is_some() {
                self.total_dropped.fetch_add(1, AtomicOrdering::Relaxed);
            }
        }

        heap.push(frame);
        true
    }

    /// Push a frame, returning the dropped frame if capacity was exceeded.
    pub fn push_with_evicted(&self, frame: PriorityFrame) -> (bool, Option<PriorityFrame>) {
        let seq = self.sequence_counter.fetch_add(1, AtomicOrdering::Relaxed);
        let frame = frame.with_sequence(seq);
        let frame_bytes = frame.len() as u64;

        self.total_pushed.fetch_add(1, AtomicOrdering::Relaxed);
        self.total_bytes_pushed
            .fetch_add(frame_bytes, AtomicOrdering::Relaxed);

        if self.capacity == 0 {
            self.total_dropped.fetch_add(1, AtomicOrdering::Relaxed);
            return (false, Some(frame));
        }

        let mut heap = self.heap.lock();
        if heap.len() >= self.capacity {
            // Find the minimum (lowest priority, oldest) frame in the queue
            let min_priority = self.find_min_priority_locked(&heap);
            if let Some(min_prio) = min_priority
                && frame.priority < min_prio
            {
                self.total_dropped.fetch_add(1, AtomicOrdering::Relaxed);
                return (false, Some(frame));
            }
            let evicted = self.evict_min_locked(&mut heap);
            if evicted.is_some() {
                self.total_dropped.fetch_add(1, AtomicOrdering::Relaxed);
            }
            heap.push(frame);
            return (true, evicted);
        }

        heap.push(frame);
        (true, None)
    }

    /// Pop the highest priority frame from the queue.
    pub fn pop(&self) -> Option<PriorityFrame> {
        let mut heap = self.heap.lock();
        if let Some(frame) = heap.pop() {
            self.total_popped.fetch_add(1, AtomicOrdering::Relaxed);
            self.total_bytes_popped
                .fetch_add(frame.len() as u64, AtomicOrdering::Relaxed);
            Some(frame)
        } else {
            None
        }
    }

    /// Pop all frames, returning them in priority order (highest first).
    pub fn pop_all(&self) -> Vec<PriorityFrame> {
        let mut heap = self.heap.lock();
        let mut frames = Vec::with_capacity(heap.len());

        while let Some(frame) = heap.pop() {
            self.total_popped.fetch_add(1, AtomicOrdering::Relaxed);
            self.total_bytes_popped
                .fetch_add(frame.len() as u64, AtomicOrdering::Relaxed);
            frames.push(frame);
        }

        frames
    }

    /// Peek at the highest priority frame without removing it.
    pub fn peek(&self) -> Option<PriorityFrame> {
        let heap = self.heap.lock();
        heap.peek().cloned()
    }

    /// Get the current number of frames in the queue.
    pub fn len(&self) -> usize {
        self.heap.lock().len()
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.heap.lock().is_empty()
    }

    /// Check if the queue is at capacity.
    pub fn is_full(&self) -> bool {
        self.heap.lock().len() >= self.capacity
    }

    /// Get the capacity of the queue.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Clear all frames from the queue.
    pub fn clear(&self) {
        let mut heap = self.heap.lock();
        let count = heap.len() as u64;

        heap.clear();

        self.total_dropped.fetch_add(count, AtomicOrdering::Relaxed);
        // Don't count cleared frames as popped
    }

    /// Clear frames with expired TTL.
    /// Returns the number of frames removed.
    pub fn clear_expired(&self, ttl_ms: u64) -> usize {
        let mut heap = self.heap.lock();
        let original_len = heap.len();

        // Collect non-expired frames
        let frames: Vec<_> = heap.drain().filter(|f| !f.is_expired(ttl_ms)).collect();
        let removed = original_len - frames.len();

        // Rebuild heap with non-expired frames
        for frame in frames {
            heap.push(frame);
        }

        if removed > 0 {
            self.total_dropped
                .fetch_add(removed as u64, AtomicOrdering::Relaxed);
        }

        removed
    }

    /// Get a snapshot of queue statistics.
    pub fn snapshot(&self) -> QueueSnapshot {
        let heap = self.heap.lock();
        QueueSnapshot {
            current_len: heap.len(),
            capacity: self.capacity,
            total_pushed: self.total_pushed.load(AtomicOrdering::Relaxed),
            total_popped: self.total_popped.load(AtomicOrdering::Relaxed),
            total_dropped: self.total_dropped.load(AtomicOrdering::Relaxed),
            total_bytes_pushed: self.total_bytes_pushed.load(AtomicOrdering::Relaxed),
            total_bytes_popped: self.total_bytes_popped.load(AtomicOrdering::Relaxed),
        }
    }

    /// Helper: Find the minimum priority in the queue.
    /// Must be called with the lock held.
    fn find_min_priority_locked(&self, heap: &BinaryHeap<PriorityFrame>) -> Option<FramePriority> {
        heap.iter().map(|f| f.priority).min()
    }

    /// Helper: Evict (remove without counting as "popped") the minimum element.
    /// Used when dropping frames to make room for higher priority frames.
    /// Must be called with the lock held.
    fn evict_min_locked(&self, heap: &mut BinaryHeap<PriorityFrame>) -> Option<PriorityFrame> {
        if heap.is_empty() {
            return None;
        }

        // Convert to vec, find and remove min, convert back
        // This is O(n) but our capacity is bounded
        let mut vec: Vec<_> = heap.drain().collect();
        if vec.is_empty() {
            return None;
        }

        // Find index of minimum (lowest priority, highest sequence for FIFO tie-breaking)
        let mut min_idx = 0;
        for (i, frame) in vec.iter().enumerate() {
            // Inverse comparison: we want the smallest
            if frame.cmp(&vec[min_idx]) == Ordering::Less {
                min_idx = i;
            }
        }

        let removed = vec.swap_remove(min_idx);
        // Note: We do NOT add to total_bytes_popped for evictions (drops)

        // Rebuild heap
        for frame in vec {
            heap.push(frame);
        }

        Some(removed)
    }
}

// =============================================================================
// Snapshot
// =============================================================================

/// Point-in-time snapshot of queue statistics.
#[derive(Debug, Clone)]
pub struct QueueSnapshot {
    /// Current number of frames in queue.
    pub current_len: usize,
    /// Maximum capacity.
    pub capacity: usize,
    /// Total frames pushed (including dropped).
    pub total_pushed: u64,
    /// Total frames successfully popped.
    pub total_popped: u64,
    /// Total frames dropped due to capacity.
    pub total_dropped: u64,
    /// Total bytes pushed.
    pub total_bytes_pushed: u64,
    /// Total bytes popped.
    pub total_bytes_popped: u64,
}

impl QueueSnapshot {
    /// Calculate the drop rate as a percentage.
    pub fn drop_rate(&self) -> f64 {
        if self.total_pushed == 0 {
            0.0
        } else {
            (self.total_dropped as f64 / self.total_pushed as f64) * 100.0
        }
    }

    /// Calculate utilization as a percentage.
    pub fn utilization(&self) -> f64 {
        if self.capacity == 0 {
            0.0
        } else {
            (self.current_len as f64 / self.capacity as f64) * 100.0
        }
    }
}

// =============================================================================
// Utility Functions
// =============================================================================

/// Get current monotonic time in nanoseconds.
fn now_monotonic_ns() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // FramePriority Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_priority_ordering() {
        assert!(FramePriority::Critical > FramePriority::High);
        assert!(FramePriority::High > FramePriority::Normal);
        assert!(FramePriority::Normal > FramePriority::Low);
    }

    #[test]
    fn test_priority_default() {
        assert_eq!(FramePriority::default(), FramePriority::Normal);
    }

    #[test]
    fn test_priority_from_u8() {
        assert_eq!(FramePriority::from_u8(0), FramePriority::Low);
        assert_eq!(FramePriority::from_u8(1), FramePriority::Normal);
        assert_eq!(FramePriority::from_u8(2), FramePriority::High);
        assert_eq!(FramePriority::from_u8(3), FramePriority::Critical);
        assert_eq!(FramePriority::from_u8(99), FramePriority::Normal); // Unknown defaults to Normal
    }

    #[test]
    fn test_priority_as_u8() {
        assert_eq!(FramePriority::Low.as_u8(), 0);
        assert_eq!(FramePriority::Normal.as_u8(), 1);
        assert_eq!(FramePriority::High.as_u8(), 2);
        assert_eq!(FramePriority::Critical.as_u8(), 3);
    }

    // -------------------------------------------------------------------------
    // PriorityFrame Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_frame_creation() {
        let data = Bytes::from_static(b"test audio");
        let frame = PriorityFrame::new(data.clone(), FramePriority::High);

        assert_eq!(frame.data, data);
        assert_eq!(frame.priority, FramePriority::High);
        assert!(frame.created_ns > 0);
    }

    #[test]
    fn test_frame_convenience_constructors() {
        let data = Bytes::from_static(b"test");

        let normal = PriorityFrame::normal(data.clone());
        assert_eq!(normal.priority, FramePriority::Normal);

        let high = PriorityFrame::high(data.clone());
        assert_eq!(high.priority, FramePriority::High);

        let critical = PriorityFrame::critical(data);
        assert_eq!(critical.priority, FramePriority::Critical);
    }

    #[test]
    fn test_frame_with_id() {
        let data = Bytes::from_static(b"test");
        let frame = PriorityFrame::normal(data).with_id(42);

        assert_eq!(frame.frame_id, Some(42));
    }

    #[test]
    fn test_frame_with_duration() {
        let data = Bytes::from_static(b"test");
        let frame = PriorityFrame::normal(data).with_duration_ms(20);

        assert_eq!(frame.duration_ms, Some(20));
    }

    #[test]
    fn test_frame_len() {
        let data = Bytes::from_static(b"hello");
        let frame = PriorityFrame::normal(data);

        assert_eq!(frame.len(), 5);
        assert!(!frame.is_empty());

        let empty_frame = PriorityFrame::normal(Bytes::new());
        assert_eq!(empty_frame.len(), 0);
        assert!(empty_frame.is_empty());
    }

    #[test]
    fn test_frame_age() {
        let data = Bytes::from_static(b"test");
        let frame = PriorityFrame::normal(data);

        // Age should be very small (microseconds) immediately after creation
        assert!(frame.age_ms() < 100);
    }

    #[test]
    fn test_frame_ordering() {
        let data = Bytes::from_static(b"test");

        let low = PriorityFrame::new(data.clone(), FramePriority::Low).with_sequence(1);
        let normal = PriorityFrame::new(data.clone(), FramePriority::Normal).with_sequence(2);
        let high = PriorityFrame::new(data.clone(), FramePriority::High).with_sequence(3);
        let critical = PriorityFrame::new(data, FramePriority::Critical).with_sequence(4);

        // Higher priority should be greater
        assert!(critical > high);
        assert!(high > normal);
        assert!(normal > low);
    }

    #[test]
    fn test_frame_fifo_within_same_priority() {
        let data = Bytes::from_static(b"test");

        let first = PriorityFrame::new(data.clone(), FramePriority::Normal).with_sequence(1);
        let second = PriorityFrame::new(data, FramePriority::Normal).with_sequence(2);

        // First (lower sequence) should be greater in max-heap for FIFO
        assert!(first > second);
    }

    // -------------------------------------------------------------------------
    // FramePriorityQueue Basic Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_queue_new() {
        let queue = FramePriorityQueue::new(100);

        assert_eq!(queue.capacity(), 100);
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
        assert!(!queue.is_full());
    }

    #[test]
    fn test_queue_zero_capacity_drops_every_push() {
        let queue = FramePriorityQueue::new(0);

        assert_eq!(queue.capacity(), 0);
        assert!(queue.is_full());
        assert!(!queue.push(PriorityFrame::critical(Bytes::from_static(b"drop"))));
        assert_eq!(queue.len(), 0);
        assert!(queue.pop().is_none());

        let snapshot = queue.snapshot();
        assert_eq!(snapshot.current_len, 0);
        assert_eq!(snapshot.total_pushed, 1);
        assert_eq!(snapshot.total_dropped, 1);
        assert_eq!(snapshot.total_bytes_pushed, 4);
        assert_eq!(snapshot.drop_rate(), 100.0);
        assert_eq!(snapshot.utilization(), 0.0);
    }

    #[test]
    fn test_queue_zero_capacity_push_with_evicted_returns_rejected_frame() {
        let queue = FramePriorityQueue::new(0);
        let frame = PriorityFrame::high(Bytes::from_static(b"reject")).with_id(7);

        let (added, evicted) = queue.push_with_evicted(frame);

        assert!(!added);
        let evicted = evicted.expect("zero-capacity queue returns the rejected frame");
        assert_eq!(evicted.frame_id, Some(7));
        assert_eq!(evicted.data, Bytes::from_static(b"reject"));
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.snapshot().total_dropped, 1);
    }

    #[test]
    fn test_queue_push_pop() {
        let queue = FramePriorityQueue::new(10);
        let data = Bytes::from_static(b"test");

        assert!(queue.push(PriorityFrame::normal(data.clone())));
        assert_eq!(queue.len(), 1);

        let frame = queue.pop().unwrap();
        assert_eq!(frame.data, data);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_queue_priority_order() {
        let queue = FramePriorityQueue::new(10);

        // Push frames in random priority order
        queue.push(PriorityFrame::new(
            Bytes::from_static(b"normal"),
            FramePriority::Normal,
        ));
        queue.push(PriorityFrame::new(
            Bytes::from_static(b"critical"),
            FramePriority::Critical,
        ));
        queue.push(PriorityFrame::new(
            Bytes::from_static(b"low"),
            FramePriority::Low,
        ));
        queue.push(PriorityFrame::new(
            Bytes::from_static(b"high"),
            FramePriority::High,
        ));

        // Should come out in priority order
        assert_eq!(queue.pop().unwrap().data, Bytes::from_static(b"critical"));
        assert_eq!(queue.pop().unwrap().data, Bytes::from_static(b"high"));
        assert_eq!(queue.pop().unwrap().data, Bytes::from_static(b"normal"));
        assert_eq!(queue.pop().unwrap().data, Bytes::from_static(b"low"));
    }

    #[test]
    fn test_queue_fifo_within_priority() {
        let queue = FramePriorityQueue::new(10);

        // Push multiple frames with same priority
        queue.push(PriorityFrame::normal(Bytes::from_static(b"first")));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"second")));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"third")));

        // Should come out in FIFO order
        assert_eq!(queue.pop().unwrap().data, Bytes::from_static(b"first"));
        assert_eq!(queue.pop().unwrap().data, Bytes::from_static(b"second"));
        assert_eq!(queue.pop().unwrap().data, Bytes::from_static(b"third"));
    }

    #[test]
    fn test_queue_peek() {
        let queue = FramePriorityQueue::new(10);

        assert!(queue.peek().is_none());

        queue.push(PriorityFrame::normal(Bytes::from_static(b"test")));

        let peeked = queue.peek().unwrap();
        assert_eq!(peeked.data, Bytes::from_static(b"test"));

        // Peek shouldn't remove the frame
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_queue_is_full() {
        let queue = FramePriorityQueue::new(2);

        assert!(!queue.is_full());

        queue.push(PriorityFrame::normal(Bytes::from_static(b"1")));
        assert!(!queue.is_full());

        queue.push(PriorityFrame::normal(Bytes::from_static(b"2")));
        assert!(queue.is_full());
    }

    // -------------------------------------------------------------------------
    // Capacity and Eviction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_queue_eviction_on_capacity() {
        let queue = FramePriorityQueue::new(2);

        queue.push(PriorityFrame::normal(Bytes::from_static(b"1")));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"2")));
        assert_eq!(queue.len(), 2);

        // Push another normal frame - should evict oldest normal
        queue.push(PriorityFrame::normal(Bytes::from_static(b"3")));
        assert_eq!(queue.len(), 2);

        // Verify stats
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.total_pushed, 3);
        assert_eq!(snapshot.total_dropped, 1);
    }

    #[test]
    fn test_queue_priority_eviction() {
        let queue = FramePriorityQueue::new(2);

        // Fill with low priority frames
        queue.push(PriorityFrame::new(
            Bytes::from_static(b"low1"),
            FramePriority::Low,
        ));
        queue.push(PriorityFrame::new(
            Bytes::from_static(b"low2"),
            FramePriority::Low,
        ));

        // Push high priority - should evict a low priority
        queue.push(PriorityFrame::high(Bytes::from_static(b"high")));

        assert_eq!(queue.len(), 2);

        // High priority should be first
        let first = queue.pop().unwrap();
        assert_eq!(first.priority, FramePriority::High);
    }

    #[test]
    fn test_queue_low_priority_dropped_when_full_of_high() {
        let queue = FramePriorityQueue::new(2);

        // Fill with high priority frames
        queue.push(PriorityFrame::high(Bytes::from_static(b"high1")));
        queue.push(PriorityFrame::high(Bytes::from_static(b"high2")));

        // Try to push low priority - should be dropped
        let result = queue.push(PriorityFrame::new(
            Bytes::from_static(b"low"),
            FramePriority::Low,
        ));
        assert!(!result);

        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_queue_push_with_evicted() {
        let queue = FramePriorityQueue::new(2);

        queue.push(PriorityFrame::new(
            Bytes::from_static(b"low"),
            FramePriority::Low,
        ));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"normal")));

        // Push high priority, should evict low
        let (added, evicted) =
            queue.push_with_evicted(PriorityFrame::high(Bytes::from_static(b"high")));

        assert!(added);
        assert!(evicted.is_some());
        assert_eq!(evicted.unwrap().priority, FramePriority::Low);
    }

    // -------------------------------------------------------------------------
    // Clear and Pop All Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_queue_clear() {
        let queue = FramePriorityQueue::new(10);

        queue.push(PriorityFrame::normal(Bytes::from_static(b"1")));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"2")));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"3")));

        assert_eq!(queue.len(), 3);

        queue.clear();
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_queue_pop_all() {
        let queue = FramePriorityQueue::new(10);

        queue.push(PriorityFrame::new(
            Bytes::from_static(b"low"),
            FramePriority::Low,
        ));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"normal")));
        queue.push(PriorityFrame::high(Bytes::from_static(b"high")));

        let frames = queue.pop_all();
        assert_eq!(frames.len(), 3);
        assert!(queue.is_empty());

        // Should be in priority order
        assert_eq!(frames[0].priority, FramePriority::High);
        assert_eq!(frames[1].priority, FramePriority::Normal);
        assert_eq!(frames[2].priority, FramePriority::Low);
    }

    #[test]
    fn test_queue_clear_expired() {
        let queue = FramePriorityQueue::new(10);

        // Add some frames
        queue.push(PriorityFrame::normal(Bytes::from_static(b"1")));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"2")));

        // No frames should be expired immediately with a reasonable TTL
        let removed = queue.clear_expired(10000); // 10 second TTL
        assert_eq!(removed, 0);
        assert_eq!(queue.len(), 2);

        // All frames should be expired with 0ms TTL
        let removed = queue.clear_expired(0);
        assert_eq!(removed, 2);
        assert!(queue.is_empty());
    }

    // -------------------------------------------------------------------------
    // Statistics Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_queue_snapshot() {
        let queue = FramePriorityQueue::new(10);

        queue.push(PriorityFrame::normal(Bytes::from_static(b"test1")));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"test2")));
        queue.pop();

        let snapshot = queue.snapshot();

        assert_eq!(snapshot.current_len, 1);
        assert_eq!(snapshot.capacity, 10);
        assert_eq!(snapshot.total_pushed, 2);
        assert_eq!(snapshot.total_popped, 1);
        assert_eq!(snapshot.total_dropped, 0);
    }

    #[test]
    fn test_snapshot_drop_rate() {
        let snapshot = QueueSnapshot {
            current_len: 0,
            capacity: 10,
            total_pushed: 100,
            total_popped: 90,
            total_dropped: 10,
            total_bytes_pushed: 1000,
            total_bytes_popped: 900,
        };

        assert!((snapshot.drop_rate() - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_snapshot_utilization() {
        let snapshot = QueueSnapshot {
            current_len: 5,
            capacity: 10,
            total_pushed: 0,
            total_popped: 0,
            total_dropped: 0,
            total_bytes_pushed: 0,
            total_bytes_popped: 0,
        };

        assert!((snapshot.utilization() - 50.0).abs() < 0.001);
    }

    // -------------------------------------------------------------------------
    // Thread Safety Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_queue_concurrent_push_pop() {
        use std::sync::Arc;
        use std::thread;

        let queue = Arc::new(FramePriorityQueue::new(1000));
        let mut handles = vec![];

        // Multiple producer threads
        for i in 0..4 {
            let q = Arc::clone(&queue);
            handles.push(thread::spawn(move || {
                for j in 0..250 {
                    let data = format!("thread_{}_frame_{}", i, j);
                    q.push(PriorityFrame::normal(Bytes::from(data)));
                }
            }));
        }

        // Multiple consumer threads
        for _ in 0..4 {
            let q = Arc::clone(&queue);
            handles.push(thread::spawn(move || {
                for _ in 0..250 {
                    let _ = q.pop();
                }
            }));
        }

        for handle in handles {
            let _ = handle.join();
        }

        // All operations should complete without panic
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.total_pushed, 1000);
    }

    #[test]
    fn test_queue_concurrent_mixed_priorities() {
        use std::sync::Arc;
        use std::thread;

        let queue = Arc::new(FramePriorityQueue::new(100));
        let mut handles = vec![];

        // Push different priorities concurrently
        for priority in [
            FramePriority::Low,
            FramePriority::Normal,
            FramePriority::High,
            FramePriority::Critical,
        ] {
            let q = Arc::clone(&queue);
            handles.push(thread::spawn(move || {
                for i in 0..25 {
                    let data = format!("{:?}_{}", priority, i);
                    q.push(PriorityFrame::new(Bytes::from(data), priority));
                }
            }));
        }

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // Pop all and verify order is roughly by priority
        let frames = queue.pop_all();
        assert_eq!(frames.len(), 100);

        // First frames should be Critical
        assert_eq!(frames[0].priority, FramePriority::Critical);

        // Last frames should be Low
        assert_eq!(frames[99].priority, FramePriority::Low);
    }

    // -------------------------------------------------------------------------
    // Edge Cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_queue_pop_empty() {
        let queue = FramePriorityQueue::new(10);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn test_queue_capacity_one() {
        let queue = FramePriorityQueue::new(1);

        queue.push(PriorityFrame::normal(Bytes::from_static(b"1")));
        assert!(queue.is_full());

        queue.push(PriorityFrame::normal(Bytes::from_static(b"2")));
        assert_eq!(queue.len(), 1);

        // Second frame should be there (evicted first)
        let frame = queue.pop().unwrap();
        assert_eq!(frame.data, Bytes::from_static(b"2"));
    }

    #[test]
    fn test_queue_large_frames() {
        let queue = FramePriorityQueue::new(10);

        // Create a large frame (1MB)
        let large_data = Bytes::from(vec![0u8; 1024 * 1024]);
        queue.push(PriorityFrame::normal(large_data.clone()));

        let frame = queue.pop().unwrap();
        assert_eq!(frame.len(), 1024 * 1024);

        let snapshot = queue.snapshot();
        assert_eq!(snapshot.total_bytes_pushed, 1024 * 1024);
    }

    #[test]
    fn test_queue_bytes_tracking() {
        let queue = FramePriorityQueue::new(10);

        queue.push(PriorityFrame::normal(Bytes::from_static(b"hello"))); // 5 bytes
        queue.push(PriorityFrame::normal(Bytes::from_static(b"world!"))); // 6 bytes

        let snapshot = queue.snapshot();
        assert_eq!(snapshot.total_bytes_pushed, 11);

        queue.pop();
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.total_bytes_popped, 5);
    }

    // -------------------------------------------------------------------------
    // Regression Tests for Bug Fixes
    // -------------------------------------------------------------------------

    #[test]
    fn test_queue_mixed_priority_eviction_regression() {
        // Regression test for bug where Normal priority was incorrectly dropped
        // when queue contained [Low, High] - should evict Low, not drop Normal
        let queue = FramePriorityQueue::new(2);

        // Fill queue with Low and High priority frames
        queue.push(PriorityFrame::new(
            Bytes::from_static(b"low"),
            FramePriority::Low,
        ));
        queue.push(PriorityFrame::high(Bytes::from_static(b"high")));
        assert_eq!(queue.len(), 2);

        // Push Normal priority - should evict Low (not drop Normal!)
        let result = queue.push(PriorityFrame::normal(Bytes::from_static(b"normal")));
        assert!(result, "Normal priority should be added, not dropped");
        assert_eq!(queue.len(), 2);

        // Pop all and verify contents
        let frames = queue.pop_all();
        assert_eq!(frames.len(), 2);

        // Should have High and Normal (Low was evicted)
        assert_eq!(frames[0].priority, FramePriority::High);
        assert_eq!(frames[0].data, Bytes::from_static(b"high"));
        assert_eq!(frames[1].priority, FramePriority::Normal);
        assert_eq!(frames[1].data, Bytes::from_static(b"normal"));
    }

    #[test]
    fn test_queue_eviction_prefers_lowest_priority() {
        // Verify that eviction always targets the lowest priority frame
        let queue = FramePriorityQueue::new(3);

        // Fill with mixed priorities
        queue.push(PriorityFrame::new(
            Bytes::from_static(b"low"),
            FramePriority::Low,
        ));
        queue.push(PriorityFrame::normal(Bytes::from_static(b"normal")));
        queue.push(PriorityFrame::high(Bytes::from_static(b"high")));
        assert_eq!(queue.len(), 3);

        // Push Critical - should evict Low
        let result = queue.push(PriorityFrame::critical(Bytes::from_static(b"critical")));
        assert!(result);

        let frames = queue.pop_all();
        let priorities: Vec<_> = frames.iter().map(|f| f.priority).collect();

        // Should have Critical, High, Normal (Low was evicted)
        assert!(priorities.contains(&FramePriority::Critical));
        assert!(priorities.contains(&FramePriority::High));
        assert!(priorities.contains(&FramePriority::Normal));
        assert!(!priorities.contains(&FramePriority::Low));
    }

    #[test]
    fn test_queue_push_with_evicted_mixed_priority_regression() {
        // Same regression test but for push_with_evicted
        let queue = FramePriorityQueue::new(2);

        queue.push(PriorityFrame::new(
            Bytes::from_static(b"low"),
            FramePriority::Low,
        ));
        queue.push(PriorityFrame::high(Bytes::from_static(b"high")));

        // Push Normal - should evict Low and return it
        let (added, evicted) =
            queue.push_with_evicted(PriorityFrame::normal(Bytes::from_static(b"normal")));

        assert!(added, "Normal priority should be added");
        assert!(evicted.is_some(), "Low priority should be evicted");
        assert_eq!(evicted.unwrap().priority, FramePriority::Low);
    }
}

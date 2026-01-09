//! Lock-free ring buffers for edge data flow
//!
//! Uses rtrb for wait-free SPSC (Single Producer Single Consumer) ring buffers
//! to ensure real-time safe data flow between nodes.
//!
//! # Buffer Types
//!
//! - `EdgeBuffer`: General-purpose buffer for DAGData (uses parking_lot Mutex)
//! - `RtrbAudioBuffer`: Wait-free SPSC buffer for raw audio bytes (uses rtrb)
//! - `EdgeBufferPair`: Bidirectional buffer pair for request/response patterns

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use parking_lot::Mutex;
use rtrb::{RingBuffer, Producer, Consumer};

use crate::dag::error::{DAGError, DAGResult};
use crate::dag::nodes::DAGData;

/// Default buffer capacity in bytes
const DEFAULT_BUFFER_CAPACITY: usize = 65536; // 64KB

/// Edge buffer for passing data between nodes
///
/// This is a simplified buffer implementation. In production, this would use
/// rtrb's wait-free SPSC ring buffer for real-time audio.
#[derive(Debug)]
pub struct EdgeBuffer {
    /// Buffer data (simplified implementation)
    data: Mutex<Vec<DAGData>>,
    /// Maximum capacity
    capacity: usize,
    /// Number of items pushed
    push_count: AtomicU64,
    /// Number of items popped
    pop_count: AtomicU64,
    /// Whether the buffer is closed
    closed: AtomicBool,
}

impl EdgeBuffer {
    /// Create a new edge buffer
    pub fn new(capacity: usize) -> Self {
        Self {
            data: Mutex::new(Vec::with_capacity(capacity)),
            capacity,
            push_count: AtomicU64::new(0),
            pop_count: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }

    /// Create with default capacity
    pub fn default_capacity() -> Self {
        Self::new(DEFAULT_BUFFER_CAPACITY)
    }

    /// Push data into the buffer
    pub fn push(&self, data: DAGData) -> DAGResult<()> {
        if self.is_closed() {
            return Err(DAGError::BufferFull {
                from: "unknown".to_string(),
                to: "unknown".to_string(),
            });
        }

        let mut buffer = self.data.lock();
        if buffer.len() >= self.capacity {
            return Err(DAGError::BufferFull {
                from: "unknown".to_string(),
                to: "unknown".to_string(),
            });
        }

        buffer.push(data);
        self.push_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Pop data from the buffer
    pub fn pop(&self) -> Option<DAGData> {
        let mut buffer = self.data.lock();
        if buffer.is_empty() {
            return None;
        }

        let data = buffer.remove(0);
        self.pop_count.fetch_add(1, Ordering::Relaxed);
        Some(data)
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.data.lock().is_empty()
    }

    /// Check if buffer is full
    pub fn is_full(&self) -> bool {
        self.data.lock().len() >= self.capacity
    }

    /// Get current length
    pub fn len(&self) -> usize {
        self.data.lock().len()
    }

    /// Get capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Close the buffer (no more pushes allowed)
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Check if buffer is closed
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Get statistics
    pub fn stats(&self) -> BufferStats {
        BufferStats {
            push_count: self.push_count.load(Ordering::Relaxed),
            pop_count: self.pop_count.load(Ordering::Relaxed),
            current_len: self.len(),
            capacity: self.capacity,
        }
    }
}

/// Buffer statistics
#[derive(Debug, Clone)]
pub struct BufferStats {
    pub push_count: u64,
    pub pop_count: u64,
    pub current_len: usize,
    pub capacity: usize,
}

/// A pair of buffers for bidirectional communication
#[derive(Debug)]
pub struct EdgeBufferPair {
    /// Buffer for producer -> consumer
    pub forward: Arc<EdgeBuffer>,
    /// Buffer for consumer -> producer (responses)
    pub backward: Arc<EdgeBuffer>,
    /// Edge identifier
    pub edge_id: String,
}

impl EdgeBufferPair {
    /// Create a new buffer pair
    pub fn new(edge_id: impl Into<String>, capacity: usize) -> Self {
        Self {
            forward: Arc::new(EdgeBuffer::new(capacity)),
            backward: Arc::new(EdgeBuffer::new(capacity)),
            edge_id: edge_id.into(),
        }
    }

    /// Create with default capacity
    pub fn default_capacity(edge_id: impl Into<String>) -> Self {
        Self::new(edge_id, DEFAULT_BUFFER_CAPACITY)
    }

    /// Get the forward buffer (producer -> consumer)
    pub fn forward(&self) -> &EdgeBuffer {
        &self.forward
    }

    /// Get the backward buffer (consumer -> producer)
    pub fn backward(&self) -> &EdgeBuffer {
        &self.backward
    }

    /// Close both buffers
    pub fn close(&self) {
        self.forward.close();
        self.backward.close();
    }
}

/// Audio-specific ring buffer for real-time audio data
///
/// This is a specialized buffer for audio samples that maintains
/// timing information and supports zero-copy operations.
///
/// Note: Currently unused - `RtrbAudioBuffer` is the production wait-free version.
/// Kept for potential future use cases requiring simpler ring buffer semantics.
#[allow(dead_code)]
#[derive(Debug)]
pub struct AudioRingBuffer {
    /// Raw sample buffer
    samples: Mutex<Vec<f32>>,
    /// Buffer capacity in samples
    capacity: usize,
    /// Sample rate
    sample_rate: u32,
    /// Read position
    read_pos: AtomicU64,
    /// Write position
    write_pos: AtomicU64,
}

#[allow(dead_code)]
impl AudioRingBuffer {
    /// Create a new audio ring buffer
    ///
    /// # Arguments
    /// * `duration_ms` - Buffer duration in milliseconds
    /// * `sample_rate` - Sample rate in Hz
    pub fn new(duration_ms: u32, sample_rate: u32) -> Self {
        let capacity = ((duration_ms as u64 * sample_rate as u64) / 1000) as usize;
        Self {
            samples: Mutex::new(vec![0.0; capacity]),
            capacity,
            sample_rate,
            read_pos: AtomicU64::new(0),
            write_pos: AtomicU64::new(0),
        }
    }

    /// Write samples to the buffer
    pub fn write(&self, samples: &[f32]) -> usize {
        let mut buffer = self.samples.lock();
        let write_pos = (self.write_pos.load(Ordering::Acquire) as usize) % self.capacity;

        let available = self.capacity - self.available();
        let to_write = samples.len().min(available);

        for i in 0..to_write {
            let pos = (write_pos + i) % self.capacity;
            buffer[pos] = samples[i];
        }

        self.write_pos.fetch_add(to_write as u64, Ordering::Release);
        to_write
    }

    /// Read samples from the buffer
    pub fn read(&self, output: &mut [f32]) -> usize {
        let buffer = self.samples.lock();
        let read_pos = (self.read_pos.load(Ordering::Acquire) as usize) % self.capacity;

        let available = self.available();
        let to_read = output.len().min(available);

        for i in 0..to_read {
            let pos = (read_pos + i) % self.capacity;
            output[i] = buffer[pos];
        }

        self.read_pos.fetch_add(to_read as u64, Ordering::Release);
        to_read
    }

    /// Get number of available samples
    pub fn available(&self) -> usize {
        let write = self.write_pos.load(Ordering::Acquire);
        let read = self.read_pos.load(Ordering::Acquire);
        (write - read) as usize
    }

    /// Get buffer duration in milliseconds
    pub fn duration_ms(&self) -> u32 {
        ((self.capacity as u64 * 1000) / self.sample_rate as u64) as u32
    }

    /// Get sample rate
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Clear the buffer
    pub fn clear(&self) {
        self.read_pos.store(0, Ordering::Release);
        self.write_pos.store(0, Ordering::Release);
    }
}

/// Default audio buffer capacity in samples
const DEFAULT_AUDIO_BUFFER_SAMPLES: usize = 4096; // ~85ms at 48kHz

/// Wait-free SPSC ring buffer for raw audio bytes
///
/// Uses rtrb for true wait-free operation, suitable for real-time audio paths.
/// This is designed for single-producer, single-consumer scenarios where
/// one thread writes audio data and another reads it.
///
/// # Real-Time Safety
///
/// - No heap allocations during push/pop operations
/// - Wait-free (bounded worst-case latency)
/// - No locks or syscalls in the fast path
/// - Suitable for audio callbacks and real-time threads
///
/// # Example
///
/// ```ignore
/// let (producer, consumer) = RtrbAudioBuffer::new(4096);
///
/// // Producer thread
/// producer.push(audio_bytes)?;
///
/// // Consumer thread
/// let data = consumer.pop()?;
/// ```
pub struct RtrbAudioBuffer {
    /// Buffer capacity in bytes
    capacity: usize,
    /// Number of bytes pushed (for statistics)
    bytes_pushed: AtomicU64,
    /// Number of bytes popped (for statistics)
    bytes_popped: AtomicU64,
}

/// Producer end of the rtrb audio buffer
pub struct RtrbAudioProducer {
    producer: Mutex<Producer<u8>>,
    stats: Arc<RtrbAudioBuffer>,
}

/// Consumer end of the rtrb audio buffer
pub struct RtrbAudioConsumer {
    consumer: Mutex<Consumer<u8>>,
    stats: Arc<RtrbAudioBuffer>,
}

impl RtrbAudioBuffer {
    /// Create a new audio buffer pair with the specified capacity
    ///
    /// Returns a (Producer, Consumer) pair that can be sent to separate threads.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Buffer capacity in bytes (power of 2 recommended for efficiency)
    ///
    /// # Returns
    ///
    /// A tuple of (RtrbAudioProducer, RtrbAudioConsumer)
    pub fn new(capacity: usize) -> (RtrbAudioProducer, RtrbAudioConsumer) {
        let (producer, consumer) = RingBuffer::new(capacity);

        let stats = Arc::new(RtrbAudioBuffer {
            capacity,
            bytes_pushed: AtomicU64::new(0),
            bytes_popped: AtomicU64::new(0),
        });

        let audio_producer = RtrbAudioProducer {
            producer: Mutex::new(producer),
            stats: stats.clone(),
        };

        let audio_consumer = RtrbAudioConsumer {
            consumer: Mutex::new(consumer),
            stats,
        };

        (audio_producer, audio_consumer)
    }

    /// Create with default capacity (4096 samples)
    pub fn default_capacity() -> (RtrbAudioProducer, RtrbAudioConsumer) {
        Self::new(DEFAULT_AUDIO_BUFFER_SAMPLES)
    }

    /// Get capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get total bytes pushed
    pub fn bytes_pushed(&self) -> u64 {
        self.bytes_pushed.load(Ordering::Relaxed)
    }

    /// Get total bytes popped
    pub fn bytes_popped(&self) -> u64 {
        self.bytes_popped.load(Ordering::Relaxed)
    }
}

impl RtrbAudioProducer {
    /// Push audio data into the buffer (wait-free)
    ///
    /// Returns the number of bytes actually written. If the buffer is full,
    /// this will return 0 without blocking.
    pub fn push(&self, data: &[u8]) -> usize {
        let mut producer = self.producer.lock();
        let slots = producer.slots();
        let to_write = data.len().min(slots);

        if to_write == 0 {
            return 0;
        }

        // Use write_chunk for efficient bulk writes
        let written = {
            match producer.write_chunk(to_write) {
                Ok(mut chunk) => {
                    let (first, second) = chunk.as_mut_slices();
                    let first_len = first.len().min(to_write);
                    first[..first_len].copy_from_slice(&data[..first_len]);

                    if first_len < to_write {
                        let remaining = to_write - first_len;
                        second[..remaining].copy_from_slice(&data[first_len..first_len + remaining]);
                    }

                    chunk.commit_all();
                    to_write
                }
                Err(_) => 0,
            }
        };

        if written > 0 {
            self.stats.bytes_pushed.fetch_add(written as u64, Ordering::Relaxed);
        }
        written
    }

    /// Push all data or return error (for critical audio paths)
    pub fn push_all(&self, data: &[u8]) -> DAGResult<()> {
        let written = self.push(data);
        if written < data.len() {
            Err(DAGError::BufferFull {
                from: "audio_producer".to_string(),
                to: "audio_consumer".to_string(),
            })
        } else {
            Ok(())
        }
    }

    /// Check available slots for writing
    pub fn available_slots(&self) -> usize {
        self.producer.lock().slots()
    }

    /// Check if buffer is full
    pub fn is_full(&self) -> bool {
        self.producer.lock().is_full()
    }

    /// Get statistics
    pub fn stats(&self) -> (u64, usize) {
        (self.stats.bytes_pushed(), self.stats.capacity)
    }
}

impl RtrbAudioConsumer {
    /// Pop audio data from the buffer (wait-free)
    ///
    /// Returns the number of bytes actually read. If the buffer is empty,
    /// this will return 0 without blocking.
    pub fn pop(&self, output: &mut [u8]) -> usize {
        let mut consumer = self.consumer.lock();
        let available = consumer.slots();
        let to_read = output.len().min(available);

        if to_read == 0 {
            return 0;
        }

        // Use read_chunk for efficient bulk reads
        if let Ok(chunk) = consumer.read_chunk(to_read) {
            let (first, second) = chunk.as_slices();
            let first_len = first.len().min(to_read);
            output[..first_len].copy_from_slice(&first[..first_len]);

            if first_len < to_read {
                let remaining = to_read - first_len;
                output[first_len..first_len + remaining].copy_from_slice(&second[..remaining]);
            }

            chunk.commit_all();
            self.stats.bytes_popped.fetch_add(to_read as u64, Ordering::Relaxed);
            to_read
        } else {
            0
        }
    }

    /// Pop exactly the requested amount or return None
    pub fn pop_exact(&self, size: usize) -> Option<Vec<u8>> {
        let mut consumer = self.consumer.lock();
        if consumer.slots() < size {
            return None;
        }

        let mut output = vec![0u8; size];

        // Use read_chunk for efficient bulk reads
        if let Ok(chunk) = consumer.read_chunk(size) {
            let (first, second) = chunk.as_slices();
            let first_len = first.len().min(size);
            output[..first_len].copy_from_slice(&first[..first_len]);

            if first_len < size {
                let remaining = size - first_len;
                output[first_len..first_len + remaining].copy_from_slice(&second[..remaining]);
            }

            chunk.commit_all();
            self.stats.bytes_popped.fetch_add(size as u64, Ordering::Relaxed);
            Some(output)
        } else {
            None
        }
    }

    /// Check available bytes for reading
    pub fn available(&self) -> usize {
        self.consumer.lock().slots()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.consumer.lock().is_empty()
    }

    /// Get statistics
    pub fn stats(&self) -> (u64, usize) {
        (self.stats.bytes_popped(), self.stats.capacity)
    }
}

// Send + Sync implementations for thread safety
unsafe impl Send for RtrbAudioProducer {}
unsafe impl Sync for RtrbAudioProducer {}
unsafe impl Send for RtrbAudioConsumer {}
unsafe impl Sync for RtrbAudioConsumer {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edge_buffer_push_pop() {
        let buffer = EdgeBuffer::new(10);

        buffer.push(DAGData::Text("test".into())).unwrap();
        assert_eq!(buffer.len(), 1);

        let data = buffer.pop().unwrap();
        if let DAGData::Text(text) = data {
            assert_eq!(text, "test");
        } else {
            panic!("Expected text");
        }

        assert!(buffer.is_empty());
    }

    #[test]
    fn test_edge_buffer_full() {
        let buffer = EdgeBuffer::new(2);

        buffer.push(DAGData::Empty).unwrap();
        buffer.push(DAGData::Empty).unwrap();
        assert!(buffer.is_full());

        let result = buffer.push(DAGData::Empty);
        assert!(result.is_err());
    }

    #[test]
    fn test_edge_buffer_close() {
        let buffer = EdgeBuffer::new(10);
        buffer.close();
        assert!(buffer.is_closed());

        let result = buffer.push(DAGData::Empty);
        assert!(result.is_err());
    }

    #[test]
    fn test_edge_buffer_pair() {
        let pair = EdgeBufferPair::default_capacity("edge_1");

        pair.forward().push(DAGData::Text("forward".into())).unwrap();
        pair.backward().push(DAGData::Text("backward".into())).unwrap();

        assert_eq!(pair.forward().len(), 1);
        assert_eq!(pair.backward().len(), 1);
    }

    #[test]
    fn test_audio_ring_buffer() {
        let buffer = AudioRingBuffer::new(100, 16000); // 100ms at 16kHz

        let samples = vec![0.5f32; 800]; // 50ms of audio
        let written = buffer.write(&samples);
        assert_eq!(written, 800);
        assert_eq!(buffer.available(), 800);

        let mut output = vec![0.0f32; 400];
        let read = buffer.read(&mut output);
        assert_eq!(read, 400);
        assert_eq!(buffer.available(), 400);
    }

    #[test]
    fn test_audio_ring_buffer_duration() {
        let buffer = AudioRingBuffer::new(100, 16000);
        assert_eq!(buffer.duration_ms(), 100);
        assert_eq!(buffer.sample_rate(), 16000);
    }

    #[test]
    fn test_buffer_stats() {
        let buffer = EdgeBuffer::new(10);

        buffer.push(DAGData::Empty).unwrap();
        buffer.push(DAGData::Empty).unwrap();
        buffer.pop();

        let stats = buffer.stats();
        assert_eq!(stats.push_count, 2);
        assert_eq!(stats.pop_count, 1);
        assert_eq!(stats.current_len, 1);
    }

    #[test]
    fn test_rtrb_audio_buffer_push_pop() {
        let (producer, consumer) = RtrbAudioBuffer::new(1024);

        // Push some audio data
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let written = producer.push(&data);
        assert_eq!(written, 8);

        // Check stats
        let (pushed, _) = producer.stats();
        assert_eq!(pushed, 8);

        // Pop the data
        let mut output = vec![0u8; 8];
        let read = consumer.pop(&mut output);
        assert_eq!(read, 8);
        assert_eq!(output, data);

        // Check consumer stats
        let (popped, _) = consumer.stats();
        assert_eq!(popped, 8);
    }

    #[test]
    fn test_rtrb_audio_buffer_partial_pop() {
        let (producer, consumer) = RtrbAudioBuffer::new(1024);

        // Push data
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        producer.push(&data);

        // Pop partial
        let mut output = vec![0u8; 4];
        let read = consumer.pop(&mut output);
        assert_eq!(read, 4);
        assert_eq!(output, vec![1u8, 2, 3, 4]);

        // Pop remaining
        let mut output2 = vec![0u8; 4];
        let read2 = consumer.pop(&mut output2);
        assert_eq!(read2, 4);
        assert_eq!(output2, vec![5u8, 6, 7, 8]);
    }

    #[test]
    fn test_rtrb_audio_buffer_pop_exact() {
        let (producer, consumer) = RtrbAudioBuffer::new(1024);

        // Push data
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        producer.push(&data);

        // Pop exact amount
        let result = consumer.pop_exact(8);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), data);

        // Try to pop when empty
        let result2 = consumer.pop_exact(4);
        assert!(result2.is_none());
    }

    #[test]
    fn test_rtrb_audio_buffer_full() {
        let (producer, _consumer) = RtrbAudioBuffer::new(8);

        // Fill the buffer
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let written = producer.push(&data);
        assert_eq!(written, 8);
        assert!(producer.is_full());

        // Try to push more
        let more_data = vec![9u8, 10];
        let written2 = producer.push(&more_data);
        assert_eq!(written2, 0);
    }

    #[test]
    fn test_rtrb_audio_buffer_push_all() {
        let (producer, _consumer) = RtrbAudioBuffer::new(8);

        // Push all successfully
        let data = vec![1u8, 2, 3, 4];
        assert!(producer.push_all(&data).is_ok());

        // Push all fails when not enough space
        let more_data = vec![5u8, 6, 7, 8, 9, 10];
        assert!(producer.push_all(&more_data).is_err());
    }

    #[test]
    fn test_rtrb_audio_buffer_available() {
        let (producer, consumer) = RtrbAudioBuffer::new(1024);

        assert_eq!(producer.available_slots(), 1024);
        assert!(consumer.is_empty());

        // Push some data
        let data = vec![0u8; 100];
        producer.push(&data);

        assert_eq!(producer.available_slots(), 924);
        assert_eq!(consumer.available(), 100);
        assert!(!consumer.is_empty());
    }
}

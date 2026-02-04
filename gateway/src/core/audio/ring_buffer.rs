//! Pre-allocated, thread-safe audio ring buffer for real-time audio processing.
//!
//! This module provides a lock-free ring buffer optimized for storing audio samples
//! for the Smart Turn detection system. The buffer maintains the last N seconds of
//! audio data and supports concurrent read/write access.
//!
//! # Design Principles
//!
//! - **Zero allocation in hot path**: All memory is pre-allocated at construction
//! - **Cache-line aligned**: Data structures aligned for optimal CPU cache usage
//! - **Lock-free writes**: Single-producer writes use atomic operations only
//! - **Efficient reads**: Readers can access data without blocking writers
//!
//! # Example
//!
//! ```rust
//! use waav_gateway::core::audio::AudioRingBuffer;
//!
//! let buffer = AudioRingBuffer::new(16000); // 16kHz sample rate
//!
//! // Push audio samples
//! let samples = vec![0.1f32, 0.2, 0.3];
//! buffer.push(&samples);
//!
//! // Read last N samples
//! let mut output = vec![0.0f32; 3];
//! let count = buffer.get_last_n(3, &mut output);
//! ```

use std::sync::atomic::{AtomicUsize, Ordering};

/// Default duration in seconds for the ring buffer (8 seconds for Smart Turn)
pub const DEFAULT_BUFFER_DURATION_SECS: usize = 8;

/// Pre-allocated ring buffer for storing audio samples.
///
/// The buffer is designed for a single-producer, multiple-consumer pattern where:
/// - One thread pushes audio samples (the audio processing thread)
/// - One or more threads read samples for analysis (VAD, Smart Turn)
///
/// # Thread Safety
///
/// - `push()` is thread-safe for a single producer (SPSC pattern)
/// - `get_last_n()` is thread-safe for multiple concurrent readers
/// - Multiple concurrent writers are NOT supported
///
/// # Memory Layout
///
/// The buffer uses a contiguous allocation with wrap-around indexing.
/// Audio samples are stored as f32 normalized to [-1.0, 1.0].
#[repr(align(64))] // Cache-line alignment for better performance
pub struct AudioRingBuffer {
    /// Audio samples stored as f32 normalized to [-1.0, 1.0]
    data: Box<[f32]>,

    /// Capacity of the buffer (number of samples)
    capacity: usize,

    /// Write position (monotonically increasing, modulo capacity for actual index)
    /// Uses Release/Acquire ordering for synchronization with readers
    write_pos: AtomicUsize,

    /// Number of valid samples currently in the buffer [0, capacity]
    /// Saturates at capacity (doesn't wrap)
    valid_samples: AtomicUsize,

    /// Sample rate in Hz (for validation and conversion)
    sample_rate: u32,
}

impl AudioRingBuffer {
    /// Creates a new ring buffer with capacity for `duration_secs` seconds of audio.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Audio sample rate in Hz (typically 16000 for Smart Turn)
    ///
    /// # Returns
    ///
    /// A new `AudioRingBuffer` with capacity for 8 seconds of audio (default).
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::audio::AudioRingBuffer;
    ///
    /// let buffer = AudioRingBuffer::new(16000); // 8 seconds at 16kHz = 128,000 samples
    /// assert_eq!(buffer.capacity(), 128000);
    /// ```
    pub fn new(sample_rate: u32) -> Self {
        Self::with_duration(sample_rate, DEFAULT_BUFFER_DURATION_SECS)
    }

    /// Creates a new ring buffer with a custom duration.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Audio sample rate in Hz
    /// * `duration_secs` - Duration in seconds to store
    ///
    /// # Returns
    ///
    /// A new `AudioRingBuffer` with the specified capacity.
    pub fn with_duration(sample_rate: u32, duration_secs: usize) -> Self {
        let capacity = sample_rate as usize * duration_secs;

        // Pre-allocate and zero-initialize the buffer
        let data = vec![0.0f32; capacity].into_boxed_slice();

        Self {
            data,
            capacity,
            write_pos: AtomicUsize::new(0),
            valid_samples: AtomicUsize::new(0),
            sample_rate,
        }
    }

    /// Returns the capacity of the buffer in samples.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the sample rate of the buffer.
    #[inline]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the number of valid samples currently in the buffer.
    ///
    /// This value increases as samples are pushed until it reaches capacity,
    /// then stays at capacity (saturates).
    #[inline]
    pub fn len(&self) -> usize {
        self.valid_samples.load(Ordering::Acquire)
    }

    /// Returns true if the buffer contains no samples.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the duration of valid audio in the buffer in seconds.
    #[inline]
    pub fn duration_secs(&self) -> f32 {
        self.len() as f32 / self.sample_rate as f32
    }

    /// Pushes audio samples into the ring buffer.
    ///
    /// If the input exceeds the buffer capacity, only the last `capacity` samples
    /// are retained. This is intentional for the Smart Turn use case where we
    /// only need the most recent audio.
    ///
    /// # Arguments
    ///
    /// * `samples` - Slice of f32 audio samples normalized to [-1.0, 1.0]
    ///
    /// # Returns
    ///
    /// The number of samples written to the buffer.
    ///
    /// # Thread Safety
    ///
    /// This method is safe for a single producer. Multiple concurrent calls
    /// from different threads are NOT supported and will cause data corruption.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::audio::AudioRingBuffer;
    ///
    /// let buffer = AudioRingBuffer::new(16000);
    /// let samples = vec![0.5f32; 1000];
    /// let written = buffer.push(&samples);
    /// assert_eq!(written, 1000);
    /// ```
    pub fn push(&self, samples: &[f32]) -> usize {
        if samples.is_empty() {
            return 0;
        }

        let len = samples.len();

        // If input is larger than capacity, only keep the last `capacity` samples
        let (samples_to_write, _skip) = if len > self.capacity {
            (&samples[len - self.capacity..], len - self.capacity)
        } else {
            (samples, 0)
        };

        let write_len = samples_to_write.len();
        let current_write_pos = self.write_pos.load(Ordering::Acquire);
        let start_idx = current_write_pos % self.capacity;

        // Calculate how much fits before wrap-around
        let first_part_len = (self.capacity - start_idx).min(write_len);
        let second_part_len = write_len - first_part_len;

        // SAFETY: We're using interior mutability pattern here.
        // This is safe because:
        // 1. We only have a single writer (enforced by API contract)
        // 2. Readers use atomic loads to get consistent write_pos before reading
        // 3. Memory ordering ensures readers see written data after write_pos update
        let data_ptr = self.data.as_ptr() as *mut f32;

        unsafe {
            // Copy first part (from start_idx to end of buffer or write_len)
            std::ptr::copy_nonoverlapping(
                samples_to_write.as_ptr(),
                data_ptr.add(start_idx),
                first_part_len,
            );

            // Copy second part (wrap around to beginning of buffer)
            if second_part_len > 0 {
                std::ptr::copy_nonoverlapping(
                    samples_to_write.as_ptr().add(first_part_len),
                    data_ptr,
                    second_part_len,
                );
            }
        }

        // Update write position atomically
        // Use Release ordering to ensure data is visible to readers
        self.write_pos
            .store(current_write_pos + write_len, Ordering::Release);

        // Update valid samples count (saturates at capacity)
        let current_valid = self.valid_samples.load(Ordering::Acquire);
        let new_valid = (current_valid + write_len).min(self.capacity);
        self.valid_samples.store(new_valid, Ordering::Release);

        // Return total samples provided (not just stored) - useful for tracking total audio processed
        len
    }

    /// Reads the last N samples from the buffer into the output slice.
    ///
    /// # Arguments
    ///
    /// * `n` - Number of samples to read
    /// * `output` - Output slice to write samples into
    ///
    /// # Returns
    ///
    /// The actual number of samples copied (may be less than `n` if buffer
    /// doesn't contain enough samples or output slice is smaller).
    ///
    /// # Thread Safety
    ///
    /// This method is safe for multiple concurrent readers.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::audio::AudioRingBuffer;
    ///
    /// let buffer = AudioRingBuffer::new(16000);
    /// buffer.push(&[0.1, 0.2, 0.3, 0.4, 0.5]);
    ///
    /// let mut output = vec![0.0f32; 3];
    /// let count = buffer.get_last_n(3, &mut output);
    /// assert_eq!(count, 3);
    /// assert_eq!(output, vec![0.3, 0.4, 0.5]);
    /// ```
    pub fn get_last_n(&self, n: usize, output: &mut [f32]) -> usize {
        let valid = self.valid_samples.load(Ordering::Acquire);
        let actual_n = n.min(valid).min(output.len());

        if actual_n == 0 {
            return 0;
        }

        // Get current write position (this is where the most recent sample was written)
        let write_pos = self.write_pos.load(Ordering::Acquire);

        // Calculate read start position
        // We want the last `actual_n` samples, ending at write_pos
        let read_end = write_pos % self.capacity;
        let read_start = if write_pos >= actual_n {
            (write_pos - actual_n) % self.capacity
        } else {
            // Handle case where write_pos < actual_n (wrap around)
            self.capacity - (actual_n - write_pos % self.capacity) % self.capacity
        };

        // Copy data handling wrap-around
        if read_start < read_end {
            // No wrap-around: data is contiguous
            output[..actual_n].copy_from_slice(&self.data[read_start..read_start + actual_n]);
        } else {
            // Wrap-around: data spans end and beginning of buffer
            let first_part_len = self.capacity - read_start;
            let second_part_len = actual_n - first_part_len;

            output[..first_part_len].copy_from_slice(&self.data[read_start..]);
            output[first_part_len..actual_n].copy_from_slice(&self.data[..second_part_len]);
        }

        actual_n
    }

    /// Reads all valid samples from the buffer into a new Vec.
    ///
    /// This allocates a new vector. For hot paths, prefer `get_last_n` with
    /// a pre-allocated buffer.
    ///
    /// # Returns
    ///
    /// A vector containing all valid samples in chronological order.
    pub fn get_all(&self) -> Vec<f32> {
        let valid = self.len();
        if valid == 0 {
            return Vec::new();
        }

        let mut output = vec![0.0f32; valid];
        self.get_last_n(valid, &mut output);
        output
    }

    /// Resets the buffer, marking all samples as invalid.
    ///
    /// This doesn't zero the memory, it just resets the read/write positions.
    /// Use this when starting a new turn/utterance.
    ///
    /// # Thread Safety
    ///
    /// This should only be called when no other threads are reading or writing.
    pub fn reset(&self) {
        self.write_pos.store(0, Ordering::Release);
        self.valid_samples.store(0, Ordering::Release);
    }

    /// Clears the buffer and zeros all memory.
    ///
    /// This is more expensive than `reset()` but ensures no old data remains.
    ///
    /// # Thread Safety
    ///
    /// This should only be called when no other threads are reading or writing.
    pub fn clear(&mut self) {
        self.write_pos.store(0, Ordering::Release);
        self.valid_samples.store(0, Ordering::Release);

        // Zero the data
        for sample in self.data.iter_mut() {
            *sample = 0.0;
        }
    }
}

// SAFETY: AudioRingBuffer is safe to send between threads
// The atomic operations ensure proper synchronization
unsafe impl Send for AudioRingBuffer {}

// SAFETY: AudioRingBuffer is safe to share between threads
// Readers use atomic loads, single writer uses atomic stores
unsafe impl Sync for AudioRingBuffer {}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    // -------------------------------------------------------------------------
    // Construction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_new_creates_buffer_with_correct_capacity() {
        let buffer = AudioRingBuffer::new(16000);

        // 8 seconds * 16000 Hz = 128000 samples
        assert_eq!(buffer.capacity(), 128000);
        assert_eq!(buffer.sample_rate(), 16000);
        assert_eq!(buffer.len(), 0);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_with_duration_creates_custom_buffer() {
        let buffer = AudioRingBuffer::with_duration(48000, 5);

        // 5 seconds * 48000 Hz = 240000 samples
        assert_eq!(buffer.capacity(), 240000);
        assert_eq!(buffer.sample_rate(), 48000);
    }

    #[test]
    fn test_new_buffer_is_empty() {
        let buffer = AudioRingBuffer::new(16000);

        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.duration_secs(), 0.0);
    }

    // -------------------------------------------------------------------------
    // Push Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_push_single_sample() {
        let buffer = AudioRingBuffer::new(16000);

        let written = buffer.push(&[0.5]);

        assert_eq!(written, 1);
        assert_eq!(buffer.len(), 1);
        assert!(!buffer.is_empty());
    }

    #[test]
    fn test_push_multiple_samples() {
        let buffer = AudioRingBuffer::new(16000);
        let samples: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();

        let written = buffer.push(&samples);

        assert_eq!(written, 1000);
        assert_eq!(buffer.len(), 1000);
    }

    #[test]
    fn test_push_empty_slice() {
        let buffer = AudioRingBuffer::new(16000);

        let written = buffer.push(&[]);

        assert_eq!(written, 0);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_push_accumulates_samples() {
        let buffer = AudioRingBuffer::new(16000);

        buffer.push(&[0.1, 0.2, 0.3]);
        buffer.push(&[0.4, 0.5]);

        assert_eq!(buffer.len(), 5);
    }

    #[test]
    fn test_push_saturates_at_capacity() {
        // Small buffer for testing
        let buffer = AudioRingBuffer::with_duration(10, 1); // 10 samples capacity

        buffer.push(&[0.1; 5]);
        assert_eq!(buffer.len(), 5);

        buffer.push(&[0.2; 7]);
        // Should saturate at 10, not 12
        assert_eq!(buffer.len(), 10);
    }

    #[test]
    fn test_push_wraps_around() {
        let buffer = AudioRingBuffer::with_duration(10, 1); // 10 samples

        // Push 8 samples
        buffer.push(&[1.0; 8]);
        assert_eq!(buffer.len(), 8);

        // Push 5 more - should wrap around
        buffer.push(&[2.0; 5]);
        assert_eq!(buffer.len(), 10); // Saturates at capacity

        // Read last 5 - should all be 2.0
        let mut output = vec![0.0f32; 5];
        buffer.get_last_n(5, &mut output);
        assert!(output.iter().all(|&x| x == 2.0));
    }

    #[test]
    fn test_push_larger_than_capacity() {
        let buffer = AudioRingBuffer::with_duration(10, 1); // 10 samples

        // Push 15 samples - only last 10 should be kept
        let samples: Vec<f32> = (0..15).map(|i| i as f32).collect();
        let written = buffer.push(&samples);

        assert_eq!(written, 15);
        assert_eq!(buffer.len(), 10);

        // Read all - should have samples 5-14
        let output = buffer.get_all();
        assert_eq!(output.len(), 10);
        assert_eq!(output[0], 5.0);
        assert_eq!(output[9], 14.0);
    }

    // -------------------------------------------------------------------------
    // Read Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_get_last_n_basic() {
        let buffer = AudioRingBuffer::new(16000);
        buffer.push(&[0.1, 0.2, 0.3, 0.4, 0.5]);

        let mut output = vec![0.0f32; 3];
        let count = buffer.get_last_n(3, &mut output);

        assert_eq!(count, 3);
        assert_eq!(output, vec![0.3, 0.4, 0.5]);
    }

    #[test]
    fn test_get_last_n_all_samples() {
        let buffer = AudioRingBuffer::new(16000);
        buffer.push(&[0.1, 0.2, 0.3]);

        let mut output = vec![0.0f32; 3];
        let count = buffer.get_last_n(3, &mut output);

        assert_eq!(count, 3);
        assert_eq!(output, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn test_get_last_n_more_than_available() {
        let buffer = AudioRingBuffer::new(16000);
        buffer.push(&[0.1, 0.2, 0.3]);

        let mut output = vec![0.0f32; 10];
        let count = buffer.get_last_n(10, &mut output);

        // Should only get 3 samples
        assert_eq!(count, 3);
        assert_eq!(&output[..3], &[0.1, 0.2, 0.3]);
    }

    #[test]
    fn test_get_last_n_empty_buffer() {
        let buffer = AudioRingBuffer::new(16000);

        let mut output = vec![0.0f32; 10];
        let count = buffer.get_last_n(10, &mut output);

        assert_eq!(count, 0);
    }

    #[test]
    fn test_get_last_n_small_output() {
        let buffer = AudioRingBuffer::new(16000);
        buffer.push(&[0.1, 0.2, 0.3, 0.4, 0.5]);

        let mut output = vec![0.0f32; 2];
        let count = buffer.get_last_n(5, &mut output);

        // Limited by output size
        assert_eq!(count, 2);
        assert_eq!(output, vec![0.4, 0.5]);
    }

    #[test]
    fn test_get_last_n_with_wrap_around() {
        let buffer = AudioRingBuffer::with_duration(10, 1); // 10 samples

        // Fill buffer
        buffer.push(&[1.0; 10]);

        // Push more to cause wrap-around
        buffer.push(&[2.0, 3.0, 4.0]);

        let mut output = vec![0.0f32; 5];
        let count = buffer.get_last_n(5, &mut output);

        assert_eq!(count, 5);
        // Last 5 should be: 1.0, 1.0, 2.0, 3.0, 4.0
        assert_eq!(output, vec![1.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_get_all() {
        let buffer = AudioRingBuffer::new(16000);
        buffer.push(&[0.1, 0.2, 0.3, 0.4, 0.5]);

        let all = buffer.get_all();

        assert_eq!(all, vec![0.1, 0.2, 0.3, 0.4, 0.5]);
    }

    #[test]
    fn test_get_all_empty() {
        let buffer = AudioRingBuffer::new(16000);

        let all = buffer.get_all();

        assert!(all.is_empty());
    }

    // -------------------------------------------------------------------------
    // Reset/Clear Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_reset() {
        let buffer = AudioRingBuffer::new(16000);
        buffer.push(&[0.1, 0.2, 0.3]);

        buffer.reset();

        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_reset_allows_new_data() {
        let buffer = AudioRingBuffer::new(16000);
        buffer.push(&[0.1, 0.2, 0.3]);
        buffer.reset();

        buffer.push(&[0.4, 0.5]);

        assert_eq!(buffer.len(), 2);
        let all = buffer.get_all();
        assert_eq!(all, vec![0.4, 0.5]);
    }

    #[test]
    fn test_clear() {
        let mut buffer = AudioRingBuffer::new(16000);
        buffer.push(&[0.1, 0.2, 0.3]);

        buffer.clear();

        assert!(buffer.is_empty());
    }

    // -------------------------------------------------------------------------
    // Duration Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_duration_secs() {
        let buffer = AudioRingBuffer::new(16000);

        // Push 1.5 seconds of audio (24000 samples at 16kHz)
        buffer.push(&vec![0.0f32; 24000]);

        let duration = buffer.duration_secs();
        assert!((duration - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_duration_secs_empty() {
        let buffer = AudioRingBuffer::new(16000);

        assert_eq!(buffer.duration_secs(), 0.0);
    }

    // -------------------------------------------------------------------------
    // Thread Safety Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_concurrent_read_single_write() {
        let buffer = Arc::new(AudioRingBuffer::with_duration(1000, 1)); // 1000 samples

        // Writer thread
        let buffer_writer = Arc::clone(&buffer);
        let writer = thread::spawn(move || {
            for i in 0..100 {
                let samples: Vec<f32> = (0..10).map(|j| (i * 10 + j) as f32).collect();
                buffer_writer.push(&samples);
                thread::yield_now();
            }
        });

        // Reader threads
        let mut readers = vec![];
        for _ in 0..3 {
            let buffer_reader = Arc::clone(&buffer);
            readers.push(thread::spawn(move || {
                let mut output = vec![0.0f32; 100];
                for _ in 0..50 {
                    let count = buffer_reader.get_last_n(100, &mut output);
                    // Just verify we got some data without panicking
                    assert!(count <= 100);
                    thread::yield_now();
                }
            }));
        }

        writer.join().unwrap();
        for reader in readers {
            reader.join().unwrap();
        }

        // Verify final state
        assert_eq!(buffer.len(), 1000); // Saturated at capacity
    }

    #[test]
    fn test_buffer_maintains_order() {
        let buffer = AudioRingBuffer::with_duration(100, 1); // 100 samples

        // Push sequential values
        for i in 0..50 {
            buffer.push(&[i as f32]);
        }

        let all = buffer.get_all();

        // Verify order is maintained
        for (i, &sample) in all.iter().enumerate() {
            assert_eq!(sample, i as f32);
        }
    }

    #[test]
    fn test_buffer_maintains_order_with_wrap() {
        let buffer = AudioRingBuffer::with_duration(10, 1); // 10 samples

        // Push sequential values that will wrap
        for i in 0..25 {
            buffer.push(&[i as f32]);
        }

        let all = buffer.get_all();

        // Should contain 15-24 (last 10 values)
        assert_eq!(all.len(), 10);
        for (i, &sample) in all.iter().enumerate() {
            assert_eq!(sample, (15 + i) as f32);
        }
    }

    // -------------------------------------------------------------------------
    // Edge Case Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_single_sample_buffer() {
        let buffer = AudioRingBuffer::with_duration(1, 1); // 1 sample capacity

        buffer.push(&[1.0]);
        assert_eq!(buffer.len(), 1);

        buffer.push(&[2.0]);
        assert_eq!(buffer.len(), 1);

        let all = buffer.get_all();
        assert_eq!(all, vec![2.0]);
    }

    #[test]
    fn test_exact_capacity_push() {
        let buffer = AudioRingBuffer::with_duration(10, 1); // 10 samples

        let samples: Vec<f32> = (0..10).map(|i| i as f32).collect();
        buffer.push(&samples);

        assert_eq!(buffer.len(), 10);
        let all = buffer.get_all();
        assert_eq!(all, samples);
    }

    #[test]
    fn test_multiple_wrap_arounds() {
        let buffer = AudioRingBuffer::with_duration(10, 1); // 10 samples

        // Push 35 samples total (3+ complete wrap-arounds)
        for i in 0..7 {
            let samples: Vec<f32> = (0..5).map(|j| (i * 5 + j) as f32).collect();
            buffer.push(&samples);
        }

        // Should have last 10 samples: 25-34
        let all = buffer.get_all();
        assert_eq!(all.len(), 10);
        assert_eq!(all[0], 25.0);
        assert_eq!(all[9], 34.0);
    }

    // -------------------------------------------------------------------------
    // Real-world Scenario Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_smart_turn_scenario() {
        // Simulate Smart Turn: 8 seconds at 16kHz
        let buffer = AudioRingBuffer::new(16000);

        // Simulate streaming audio at 20ms chunks (320 samples per chunk)
        let chunk_size = 320;
        let chunks_for_8_seconds = (8 * 16000) / chunk_size; // 400 chunks

        for i in 0..chunks_for_8_seconds {
            let chunk: Vec<f32> = (0..chunk_size)
                .map(|j| ((i * chunk_size + j) as f32).sin())
                .collect();
            buffer.push(&chunk);
        }

        assert_eq!(buffer.len(), 128000);
        assert!((buffer.duration_secs() - 8.0).abs() < 0.001);

        // Read full buffer for mel spectrogram
        let mut full_audio = vec![0.0f32; 128000];
        let count = buffer.get_last_n(128000, &mut full_audio);
        assert_eq!(count, 128000);
    }

    #[test]
    fn test_vad_chunk_scenario() {
        // Simulate Silero VAD: 512 samples at 16kHz (32ms)
        let buffer = AudioRingBuffer::new(16000);

        // Simulate 10 seconds of streaming
        let chunk_size = 512;
        let total_chunks = (10 * 16000) / chunk_size;

        for _ in 0..total_chunks {
            let chunk: Vec<f32> = vec![0.1; chunk_size];
            buffer.push(&chunk);
        }

        // Buffer should contain last 8 seconds
        assert_eq!(buffer.len(), 128000);
    }
}

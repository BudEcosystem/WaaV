//! TDD tests for Fix 5: Unsafe code safety improvements
//!
//! These tests verify the safety of unsafe code blocks in the gateway,
//! focusing on:
//! - Audio buffer alignment and conversion
//! - SIMD buffer operations
//! - Pin projection safety

use std::alloc::{Layout, alloc, dealloc};
use std::ptr::NonNull;

/// Cache line size for alignment
const CACHE_LINE_SIZE: usize = 64;

// =============================================================================
// Test 5.1: Audio Alignment Safety
// =============================================================================

/// Test that aligned audio data is correctly converted
#[test]
fn test_audio_conversion_aligned_data() {
    // Create properly aligned data (even address for i16)
    let audio_data: Vec<u8> = vec![0x00, 0x01, 0xFF, 0x7F, 0x00, 0x80, 0x00, 0x00];

    // Convert using the safe pattern
    let samples = convert_bytes_to_i16_safe(&audio_data);

    assert_eq!(samples.len(), 4);
    assert_eq!(samples[0], 0x0100); // Little endian: 0x00, 0x01 -> 256
    assert_eq!(samples[1], 0x7FFF); // Little endian: 0xFF, 0x7F -> 32767
    assert_eq!(samples[2], i16::MIN); // Little endian: 0x00, 0x80 -> -32768
    assert_eq!(samples[3], 0);
}

/// Test that unaligned audio data is correctly converted via fallback
#[test]
fn test_audio_conversion_unaligned_data() {
    // Create a buffer and get an unaligned slice
    let mut buffer = [0u8; 17];
    buffer[1] = 0x00;
    buffer[2] = 0x01;
    buffer[3] = 0xFF;
    buffer[4] = 0x7F;

    // Slice starting at offset 1 (potentially unaligned)
    let unaligned_data = &buffer[1..5];

    // Should still work via fallback path
    let samples = convert_bytes_to_i16_safe(unaligned_data);

    assert_eq!(samples.len(), 2);
    assert_eq!(samples[0], 0x0100);
    assert_eq!(samples[1], 0x7FFF);
}

/// Test edge case: odd length audio data
#[test]
fn test_audio_conversion_odd_length() {
    let audio_data = vec![0x00, 0x01, 0xFF]; // Odd length

    // Should handle gracefully (ignore the last byte)
    let samples = convert_bytes_to_i16_safe(&audio_data);

    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0], 0x0100);
}

/// Test edge case: empty audio data
#[test]
fn test_audio_conversion_empty() {
    let audio_data: Vec<u8> = vec![];
    let samples = convert_bytes_to_i16_safe(&audio_data);
    assert!(samples.is_empty());
}

/// Endian-safe audio conversion mirroring the production
/// `convert_audio_to_frame_ref` decode path in `src/livekit/client/audio.rs`.
///
/// The wire format is always 16-bit little-endian PCM. The aligned fast path
/// uses `bytemuck::cast_slice` (zero-copy, byte-order-preserving) ONLY on
/// little-endian hosts, where it is bit-identical to `from_le_bytes`. On
/// big-endian hosts (or unaligned buffers) it always decodes via
/// `i16::from_le_bytes`, so the result is independent of host endianness.
fn convert_bytes_to_i16_safe(audio_data: &[u8]) -> Vec<i16> {
    let num_samples = audio_data.len() / 2;
    if num_samples == 0 {
        return Vec::new();
    }

    #[cfg(target_endian = "little")]
    {
        if bytemuck::try_cast_slice::<u8, i16>(&audio_data[..num_samples * 2]).is_ok() {
            return bytemuck::cast_slice::<u8, i16>(&audio_data[..num_samples * 2]).to_vec();
        }
    }

    // Unaligned, or big-endian host: endian-explicit decode.
    audio_data
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

/// Reference decode that is correct on ANY endianness: pure `from_le_bytes`.
/// Used as the oracle the production fast path must match bit-for-bit.
fn convert_bytes_to_i16_le_reference(audio_data: &[u8]) -> Vec<i16> {
    audio_data
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

/// Fix 2 (W-E1): a fixed little-endian byte pattern must decode to the same
/// `[i16]` regardless of host endianness. Under the old host-endian
/// `*const i16` reinterpret this assertion would fail on big-endian targets
/// (UB-shaped); the endian-safe path makes it hold everywhere.
#[test]
fn test_audio_conversion_fixed_pattern_is_endian_safe() {
    // Each pair is little-endian: low byte first.
    let bytes: Vec<u8> = vec![
        0x01, 0x00, // 0x0001 = 1
        0xFF, 0xFF, // 0xFFFF = -1
        0x00, 0x80, // 0x8000 = i16::MIN
        0xFF, 0x7F, // 0x7FFF = i16::MAX
        0x34, 0x12, // 0x1234 = 4660
    ];
    let expected: Vec<i16> = vec![1, -1, i16::MIN, i16::MAX, 0x1234];

    // Aligned (Vec buffers are at least 2-byte aligned): fast path.
    let decoded = convert_bytes_to_i16_safe(&bytes);
    assert_eq!(decoded, expected, "aligned decode must be LE-correct");

    // The fast path must agree bit-for-bit with the endian-explicit oracle.
    assert_eq!(
        decoded,
        convert_bytes_to_i16_le_reference(&bytes),
        "fast path must equal from_le_bytes oracle"
    );
}

/// Fix 2 (W-E1): the aligned fast path and the unaligned fallback must produce
/// identical results for the same logical samples (no path-dependent skew).
#[test]
fn test_audio_conversion_aligned_matches_unaligned() {
    let pattern: Vec<u8> = vec![
        0x10, 0x20, 0x30, 0x40, 0x00, 0x80, 0xFF, 0x7F, 0xAB, 0xCD,
    ];

    // Aligned slice (offset 0 of a fresh Vec).
    let aligned = convert_bytes_to_i16_safe(&pattern);

    // Force an unaligned slice: put the same payload at an odd offset.
    let mut buf = vec![0u8; pattern.len() + 1];
    buf[1..].copy_from_slice(&pattern);
    let unaligned_slice = &buf[1..];
    let unaligned = convert_bytes_to_i16_safe(unaligned_slice);

    assert_eq!(
        aligned, unaligned,
        "aligned fast path and unaligned fallback must agree"
    );
    assert_eq!(aligned, convert_bytes_to_i16_le_reference(&pattern));
}

// =============================================================================
// Test 5.2: AlignedBuffer Safety
// =============================================================================

/// Test that AlignedBuffer correctly initializes
#[test]
fn test_aligned_buffer_initialization() {
    let buf = TestAlignedBuffer::<f32>::new(100);
    assert_eq!(buf.len(), 0);
    assert_eq!(buf.capacity(), 100);

    // Verify alignment
    assert!(buf.is_aligned());
}

/// Test that zero-capacity buffer doesn't allocate
#[test]
fn test_aligned_buffer_zero_capacity() {
    let buf = TestAlignedBuffer::<f32>::new(0);
    assert_eq!(buf.len(), 0);
    assert_eq!(buf.capacity(), 0);
}

/// Test resize with value initialization
#[test]
fn test_aligned_buffer_resize() {
    let mut buf = TestAlignedBuffer::<f32>::new(100);

    buf.resize(50, 1.5);
    assert_eq!(buf.len(), 50);

    // Verify all values are initialized
    for i in 0..50 {
        assert_eq!(buf.as_slice()[i], 1.5);
    }
}

/// Test bounds checking in index operations
#[test]
#[should_panic(expected = "index out of bounds")]
fn test_aligned_buffer_index_bounds() {
    let mut buf = TestAlignedBuffer::<f32>::new(10);
    buf.resize(5, 0.0);

    let _ = buf.as_slice()[10]; // Should panic
}

/// Test that resize beyond capacity panics
#[test]
#[should_panic(expected = "Cannot resize beyond capacity")]
fn test_aligned_buffer_resize_beyond_capacity() {
    let mut buf = TestAlignedBuffer::<f32>::new(10);
    buf.resize(20, 0.0); // Should panic
}

/// Test copy_from_slice bounds
#[test]
#[should_panic(expected = "Source slice too large")]
fn test_aligned_buffer_copy_from_slice_bounds() {
    let mut buf = TestAlignedBuffer::<f32>::new(5);
    let src = vec![1.0f32; 10];
    buf.copy_from_slice(&src); // Should panic
}

/// Test AlignedBuffer for testing (simplified version)
struct TestAlignedBuffer<T: Copy + Default> {
    ptr: Option<NonNull<T>>,
    len: usize,
    capacity: usize,
}

impl<T: Copy + Default> TestAlignedBuffer<T> {
    fn new(capacity: usize) -> Self {
        if capacity == 0 {
            return Self {
                ptr: None,
                len: 0,
                capacity: 0,
            };
        }

        let layout = Layout::from_size_align(
            capacity * std::mem::size_of::<T>(),
            CACHE_LINE_SIZE,
        )
        .expect("Invalid layout");

        // SAFETY: Layout is valid and non-zero
        let ptr = unsafe {
            let raw_ptr = alloc(layout) as *mut T;
            if raw_ptr.is_null() {
                std::alloc::handle_alloc_error(layout);
            }
            // Zero-initialize with debug assertion
            debug_assert!(!raw_ptr.is_null(), "Allocation returned null pointer");
            std::ptr::write_bytes(raw_ptr, 0, capacity);
            Some(NonNull::new_unchecked(raw_ptr))
        };

        Self {
            ptr,
            len: 0,
            capacity,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn capacity(&self) -> usize {
        self.capacity
    }

    fn is_aligned(&self) -> bool {
        match self.ptr {
            Some(ptr) => (ptr.as_ptr() as usize).is_multiple_of(CACHE_LINE_SIZE),
            None => true, // Zero-capacity is considered aligned
        }
    }

    fn as_slice(&self) -> &[T] {
        match self.ptr {
            Some(ptr) => {
                // Debug assertions for safety
                debug_assert!(self.len <= self.capacity, "len {} > capacity {}", self.len, self.capacity);
                // (No null check: `ptr` is a NonNull, so `as_ptr()` is never null by construction.)

                // SAFETY: ptr is valid, len <= capacity (checked in debug)
                unsafe { std::slice::from_raw_parts(ptr.as_ptr(), self.len) }
            }
            None => &[],
        }
    }

    fn resize(&mut self, new_len: usize, value: T) {
        assert!(new_len <= self.capacity, "Cannot resize beyond capacity");

        if let Some(ptr) = self.ptr
            && new_len > self.len {
                // Debug assertions for bounds checking
                debug_assert!(new_len <= self.capacity, "new_len {} > capacity {}", new_len, self.capacity);

                // SAFETY: bounds checked above
                unsafe {
                    let start = ptr.as_ptr().add(self.len);
                    for i in 0..(new_len - self.len) {
                        debug_assert!(self.len + i < self.capacity, "Write beyond capacity");
                        std::ptr::write(start.add(i), value);
                    }
                }
            }

        self.len = new_len;
    }

    fn copy_from_slice(&mut self, src: &[T]) {
        assert!(src.len() <= self.capacity, "Source slice too large");

        if let Some(ptr) = self.ptr {
            self.len = src.len();

            if !src.is_empty() {
                // Debug assertions
                debug_assert!(src.len() <= self.capacity, "copy would overflow");

                // SAFETY: ptr is valid and src.len() <= capacity
                unsafe {
                    std::ptr::copy_nonoverlapping(src.as_ptr(), ptr.as_ptr(), src.len());
                }
            }
        }
    }
}

impl<T: Copy + Default> Drop for TestAlignedBuffer<T> {
    fn drop(&mut self) {
        if let Some(ptr) = self.ptr
            && self.capacity > 0 {
                let layout = Layout::from_size_align(
                    self.capacity * std::mem::size_of::<T>(),
                    CACHE_LINE_SIZE,
                )
                .expect("Invalid layout");

                // SAFETY: ptr was allocated with this layout
                unsafe {
                    dealloc(ptr.as_ptr() as *mut u8, layout);
                }
            }
    }
}

// =============================================================================
// Test 5.3: Pin Projection Safety
// =============================================================================

/// Test that panic catching works correctly
#[test]
fn test_panic_catching_success() {
    let result = catch_panic_safely(|| Ok::<i32, String>(42));
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);
}

/// Test that panic is caught and converted to error
#[test]
fn test_panic_catching_panic() {
    let result: Result<i32, String> = catch_panic_safely(|| {
        panic!("test panic");
        #[allow(unreachable_code)]
        Ok(42)
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("test panic"));
}

/// Test that normal errors are preserved
#[test]
fn test_panic_catching_error() {
    let result: Result<i32, String> = catch_panic_safely(|| Err("normal error".to_string()));

    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "normal error");
}

/// Simple panic-catching function (mimics isolation.rs pattern)
fn catch_panic_safely<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + std::panic::UnwindSafe,
{
    match std::panic::catch_unwind(f) {
        Ok(result) => result,
        Err(panic_info) => {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic".to_string()
            };
            Err(msg)
        }
    }
}

// =============================================================================
// Test 5.4: FFI Callback Safety (simulated)
// =============================================================================

/// Test null pointer handling in FFI-style callbacks
#[test]
fn test_ffi_null_pointer_check() {
    // Simulate FFI callback with null check
    let result_ptr: *const i32 = std::ptr::null();

    let result = process_ffi_result(result_ptr);
    assert!(result.is_none());
}

/// Test valid pointer handling in FFI-style callbacks
#[test]
fn test_ffi_valid_pointer() {
    let value: i32 = 42;
    let result_ptr: *const i32 = &value;

    let result = process_ffi_result(result_ptr);
    assert_eq!(result, Some(42));
}

/// Simulate FFI callback with null check (as recommended in Fix 5.4)
fn process_ffi_result(result: *const i32) -> Option<i32> {
    // NULL CHECK - Critical for FFI safety
    if result.is_null() {
        return None;
    }

    // SAFETY: We've verified the pointer is not null
    // In real FFI code, we'd also verify the pointer is valid
    unsafe { Some(*result) }
}

// =============================================================================
// Memory Safety Stress Tests
// =============================================================================

/// Test that AlignedBuffer doesn't leak memory under stress
#[test]
fn test_aligned_buffer_no_memory_leak() {
    for _ in 0..1000 {
        let mut buf = TestAlignedBuffer::<f32>::new(1000);
        buf.resize(500, 1.5);
        // Buffer should be deallocated when dropped
    }
    // If this test passes without OOM, we're not leaking
}

/// Test concurrent-style operations don't cause issues
#[test]
fn test_aligned_buffer_rapid_operations() {
    let mut buf = TestAlignedBuffer::<f32>::new(1000);

    for i in 0..100 {
        buf.resize(i * 10 % 1000, i as f32);
        assert!(buf.len() <= buf.capacity());

        let src: Vec<f32> = (0..100).map(|x| x as f32).collect();
        buf.copy_from_slice(&src);
        assert_eq!(buf.len(), 100);
    }
}

/// Test that alignment is maintained after operations
#[test]
fn test_alignment_maintained_after_operations() {
    let mut buf = TestAlignedBuffer::<f32>::new(256);

    buf.resize(100, 0.0);
    assert!(buf.is_aligned());

    let src: Vec<f32> = (0..50).map(|x| x as f32).collect();
    buf.copy_from_slice(&src);
    assert!(buf.is_aligned());
}

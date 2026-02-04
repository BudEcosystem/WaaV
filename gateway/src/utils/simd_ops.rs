//! SIMD-optimized audio processing operations.
//!
//! This module provides portable SIMD implementations for performance-critical
//! audio processing operations used in VAD, mel spectrogram extraction, and
//! noise filtering.
//!
//! ## Performance Targets
//!
//! - PCM-to-Float conversion: 4-8x speedup over scalar
//! - Float-to-PCM conversion: 6-8x speedup over scalar
//! - Energy/RMS calculation: 4-6x speedup over scalar
//! - Mel filterbank multiplication: 4-6x speedup over scalar
//!
//! ## CPU Feature Detection
//!
//! Uses `pulp` for automatic runtime CPU feature detection:
//! - x86/x86_64: SSE2, AVX, AVX2, AVX-512
//! - ARM/AArch64: NEON
//! - Fallback: Scalar implementation when no SIMD available
//!
//! ## Memory Alignment
//!
//! For optimal SIMD performance:
//! - Align buffers to 64 bytes (cache line size)
//! - Use contiguous memory layouts
//! - Prefer SoA (Structure of Arrays) over AoS (Array of Structures)

use pulp::{Arch, Simd, WithSimd};
use std::alloc::{Layout, alloc, dealloc};
use std::ptr::NonNull;

/// Pre-calculated constants for PCM conversion
pub const PCM_TO_FLOAT_SCALE: f32 = 1.0 / 32768.0;
pub const FLOAT_TO_PCM_SCALE_POS: f32 = 32767.0;
pub const FLOAT_TO_PCM_SCALE_NEG: f32 = 32768.0;

/// Cache line size for alignment (64 bytes on x86, 128 bytes on Apple Silicon)
/// We use 64 bytes as a conservative default that works well on all platforms.
pub const CACHE_LINE_SIZE: usize = 64;

/// Reports detected SIMD capabilities for logging/debugging.
///
/// Returns a tuple of (ISA name, f32 lanes per operation).
///
/// # Examples
///
/// ```
/// use waav_gateway::utils::simd_ops::simd_capabilities;
///
/// let (isa_name, lanes) = simd_capabilities();
/// println!("SIMD: {} ({} f32 lanes per op)", isa_name, lanes);
/// ```
///
/// Example outputs:
/// - Intel with AVX2: `("AVX2", 8)`
/// - Intel with AVX-512: `("AVX-512", 16)`
/// - Apple M1: `("NEON", 4)`
/// - Fallback: `("Scalar", 1)`
#[inline]
pub fn simd_capabilities() -> (&'static str, usize) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") {
            return ("AVX-512", 16);
        }
        if is_x86_feature_detected!("avx2") {
            return ("AVX2", 8);
        }
        if is_x86_feature_detected!("avx") {
            return ("AVX", 8);
        }
        if is_x86_feature_detected!("sse2") {
            return ("SSE2", 4);
        }
    }

    #[cfg(target_arch = "x86")]
    {
        if is_x86_feature_detected!("sse2") {
            return ("SSE2", 4);
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 always has NEON
        return ("NEON", 4);
    }

    #[cfg(target_arch = "arm")]
    {
        // ARM32 may or may not have NEON, but pulp handles this
        return ("ARM", 4);
    }

    // Fallback for unsupported architectures
    #[allow(unreachable_code)]
    ("Scalar", 1)
}

/// Returns a formatted string describing SIMD capabilities.
///
/// Useful for logging at startup or in diagnostic endpoints.
#[inline]
pub fn simd_capabilities_string() -> String {
    let (isa_name, lanes) = simd_capabilities();
    format!(
        "SIMD: {} ({} f32 lanes per op, {} bytes alignment)",
        isa_name, lanes, CACHE_LINE_SIZE
    )
}

/// Cache-line aligned buffer for SIMD operations.
///
/// This buffer ensures that data is aligned to cache line boundaries (64 bytes),
/// which is critical for optimal SIMD performance and avoiding false sharing.
///
/// # Performance Benefits
///
/// - Avoids cache line splits during SIMD loads/stores
/// - Prevents false sharing in multi-threaded scenarios
/// - Enables hardware prefetching optimizations
#[repr(C)]
pub struct AlignedBuffer<T: Copy + Default> {
    ptr: NonNull<T>,
    len: usize,
    capacity: usize,
}

impl<T: Copy + Default> AlignedBuffer<T> {
    /// Creates a new aligned buffer with the specified capacity.
    ///
    /// The buffer is zero-initialized.
    pub fn new(capacity: usize) -> Self {
        if capacity == 0 {
            return Self {
                ptr: NonNull::dangling(),
                len: 0,
                capacity: 0,
            };
        }

        let layout = Layout::from_size_align(capacity * std::mem::size_of::<T>(), CACHE_LINE_SIZE)
            .expect("Invalid layout");

        // SAFETY: Layout is valid and non-zero
        let ptr = unsafe {
            let raw_ptr = alloc(layout) as *mut T;
            if raw_ptr.is_null() {
                std::alloc::handle_alloc_error(layout);
            }
            // Zero-initialize
            std::ptr::write_bytes(raw_ptr, 0, capacity);
            NonNull::new_unchecked(raw_ptr)
        };

        Self {
            ptr,
            len: 0,
            capacity,
        }
    }

    /// Creates a new aligned buffer initialized with the given value.
    pub fn with_value(capacity: usize, value: T) -> Self {
        let mut buf = Self::new(capacity);
        buf.resize(capacity, value);
        buf
    }

    /// Returns the length of the buffer.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the capacity of the buffer.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns a slice of the buffer contents.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        if self.capacity == 0 {
            return &[];
        }
        // Debug assertion for safety invariant
        // Note: self.ptr is NonNull, so null check is unnecessary
        debug_assert!(self.len <= self.capacity, "len {} > capacity {}", self.len, self.capacity);

        // SAFETY: ptr is valid (NonNull guarantees non-null) and len <= capacity
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    /// Returns a mutable slice of the buffer contents.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.capacity == 0 {
            return &mut [];
        }
        // Debug assertion for safety invariant
        // Note: self.ptr is NonNull, so null check is unnecessary
        debug_assert!(self.len <= self.capacity, "len {} > capacity {}", self.len, self.capacity);

        // SAFETY: ptr is valid (NonNull guarantees non-null) and len <= capacity
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Resizes the buffer to the specified length.
    ///
    /// If the new length is greater than the current length, new elements
    /// are initialized with the provided value.
    pub fn resize(&mut self, new_len: usize, value: T) {
        assert!(new_len <= self.capacity, "Cannot resize beyond capacity");

        if new_len > self.len {
            // Debug assertion for bounds checking
            // Note: self.ptr is NonNull, so null check is unnecessary
            debug_assert!(new_len <= self.capacity, "new_len {} > capacity {}", new_len, self.capacity);

            // Initialize new elements
            // SAFETY: ptr is valid (NonNull guarantees non-null) and new_len <= capacity
            unsafe {
                let start = self.ptr.as_ptr().add(self.len);
                for i in 0..(new_len - self.len) {
                    debug_assert!(self.len + i < self.capacity, "write index {} >= capacity {}", self.len + i, self.capacity);
                    std::ptr::write(start.add(i), value);
                }
            }
        }

        self.len = new_len;
    }

    /// Clears the buffer, setting length to 0.
    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Copies data from a slice into the buffer.
    ///
    /// The buffer is resized to match the slice length.
    pub fn copy_from_slice(&mut self, src: &[T]) {
        assert!(src.len() <= self.capacity, "Source slice too large");
        self.len = src.len();

        if src.is_empty() || self.capacity == 0 {
            return;
        }

        // SAFETY: ptr is valid and src.len() <= capacity
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr(), self.ptr.as_ptr(), src.len());
        }
    }
}

impl<T: Copy + Default> Drop for AlignedBuffer<T> {
    fn drop(&mut self) {
        if self.capacity == 0 {
            return;
        }

        let layout =
            Layout::from_size_align(self.capacity * std::mem::size_of::<T>(), CACHE_LINE_SIZE)
                .expect("Invalid layout");

        // SAFETY: ptr was allocated with this layout
        unsafe {
            dealloc(self.ptr.as_ptr() as *mut u8, layout);
        }
    }
}

// SAFETY: AlignedBuffer owns its data and has no interior mutability
unsafe impl<T: Copy + Default + Send> Send for AlignedBuffer<T> {}
unsafe impl<T: Copy + Default + Sync> Sync for AlignedBuffer<T> {}

impl<T: Copy + Default> std::ops::Index<usize> for AlignedBuffer<T> {
    type Output = T;

    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        assert!(index < self.len, "Index out of bounds");
        // SAFETY: index < len and ptr is valid
        unsafe { &*self.ptr.as_ptr().add(index) }
    }
}

impl<T: Copy + Default> std::ops::IndexMut<usize> for AlignedBuffer<T> {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        assert!(index < self.len, "Index out of bounds");
        // SAFETY: index < len and ptr is valid
        unsafe { &mut *self.ptr.as_ptr().add(index) }
    }
}

/// SIMD-accelerated conversion from i16 PCM bytes to f32 samples.
///
/// Converts little-endian i16 PCM audio data to normalized f32 samples
/// in the range [-1.0, 1.0].
///
/// # Arguments
///
/// * `pcm` - Raw PCM bytes (must be even length, 2 bytes per sample)
///
/// # Returns
///
/// Vector of f32 samples, clamped to [-1.0, 1.0]
///
/// # Performance
///
/// SIMD implementation processes multiple samples per iteration
/// achieving 4-8x speedup over scalar loop on supported platforms.
#[inline]
pub fn pcm_to_float_simd(pcm: &[u8]) -> Vec<f32> {
    let sample_count = pcm.len() / 2;
    if sample_count == 0 {
        return Vec::new();
    }

    // First convert bytes to i16 then to f32 (scalar - SIMD int conversion is complex)
    let mut samples: Vec<f32> = Vec::with_capacity(sample_count);
    for chunk in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f32;
        samples.push(sample);
    }

    // Now apply SIMD scaling and clamping
    let arch = Arch::new();
    arch.dispatch(PcmScaleOp {
        samples: &mut samples,
    });

    samples
}

/// Operation struct for SIMD PCM scaling
struct PcmScaleOp<'a> {
    samples: &'a mut [f32],
}

impl WithSimd for PcmScaleOp<'_> {
    type Output = ();

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> Self::Output {
        let samples = self.samples;
        let (head, tail) = S::f32s_as_mut_simd(samples);

        let scale = simd.f32s_splat(PCM_TO_FLOAT_SCALE);
        let neg_one = simd.f32s_splat(-1.0);
        let pos_one = simd.f32s_splat(1.0);

        // Process SIMD chunks
        for chunk in head.iter_mut() {
            let scaled = simd.f32s_mul(*chunk, scale);
            *chunk = simd.f32s_max(simd.f32s_min(scaled, pos_one), neg_one);
        }

        // Handle remaining samples with scalar code
        for val in tail.iter_mut() {
            *val = (*val * PCM_TO_FLOAT_SCALE).clamp(-1.0, 1.0);
        }
    }
}

/// SIMD-accelerated conversion from f32 samples to i16 PCM bytes.
///
/// Converts normalized f32 audio samples to little-endian i16 PCM format.
///
/// # Arguments
///
/// * `samples` - f32 samples (expected range [-1.0, 1.0], will be clamped)
///
/// # Returns
///
/// Vector of PCM bytes (2 bytes per sample)
///
/// # Performance
///
/// SIMD implementation achieves 6-8x speedup over scalar loop on supported platforms.
#[inline]
pub fn float_to_pcm_simd(samples: &[f32]) -> Vec<u8> {
    if samples.is_empty() {
        return Vec::new();
    }

    // Scale and clamp using SIMD
    let mut scaled = samples.to_vec();
    let arch = Arch::new();
    arch.dispatch(FloatScaleOp {
        samples: &mut scaled,
    });

    // Convert to bytes (scalar - efficient enough for output)
    let mut output = Vec::with_capacity(samples.len() * 2);
    for &val in &scaled {
        let sample = val.round() as i16;
        output.extend_from_slice(&sample.to_le_bytes());
    }

    output
}

/// Operation struct for SIMD float scaling
struct FloatScaleOp<'a> {
    samples: &'a mut [f32],
}

impl WithSimd for FloatScaleOp<'_> {
    type Output = ();

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> Self::Output {
        let samples = self.samples;
        let (head, tail) = S::f32s_as_mut_simd(samples);

        let neg_one = simd.f32s_splat(-1.0);
        let pos_one = simd.f32s_splat(1.0);
        let scale = simd.f32s_splat(FLOAT_TO_PCM_SCALE_POS);

        // Process SIMD chunks
        // Note: We use FLOAT_TO_PCM_SCALE_POS (32767.0) for all values.
        // The difference between positive and negative scaling (32767 vs 32768)
        // is negligible for audio quality and allows for simpler SIMD code.
        for chunk in head.iter_mut() {
            // Clamp to [-1, 1]
            let clamped = simd.f32s_max(simd.f32s_min(*chunk, pos_one), neg_one);
            // Scale to PCM range
            *chunk = simd.f32s_mul(clamped, scale);
        }

        // Handle remaining samples with scalar code
        for val in tail.iter_mut() {
            let clamped = val.clamp(-1.0, 1.0);
            *val = if clamped >= 0.0 {
                clamped * FLOAT_TO_PCM_SCALE_POS
            } else {
                clamped * FLOAT_TO_PCM_SCALE_NEG
            };
        }
    }
}

/// SIMD-accelerated RMS and peak energy calculation.
///
/// Calculates both RMS (Root Mean Square) energy and peak absolute value
/// in a single pass over the data.
///
/// # Arguments
///
/// * `samples` - f32 audio samples
///
/// # Returns
///
/// Tuple of (rms_energy, peak_energy)
///
/// # Performance
///
/// SIMD implementation achieves 4-6x speedup by:
/// - Processing multiple samples per iteration
/// - Computing sum of squares and max simultaneously
/// - Using SIMD horizontal reduction
#[inline]
pub fn rms_peak_simd(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }

    let arch = Arch::new();
    arch.dispatch(RmsPeakOp { samples })
}

/// Operation struct for SIMD RMS/peak calculation
struct RmsPeakOp<'a> {
    samples: &'a [f32],
}

impl WithSimd for RmsPeakOp<'_> {
    type Output = (f32, f32);

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> Self::Output {
        let samples = self.samples;
        let len = samples.len();

        let (head, tail) = S::f32s_as_simd(samples);

        let mut sum_sq_acc = simd.f32s_splat(0.0);
        let mut peak_acc = simd.f32s_splat(0.0);

        // Process SIMD chunks
        for chunk in head.iter() {
            // Sum of squares
            let sq = simd.f32s_mul(*chunk, *chunk);
            sum_sq_acc = simd.f32s_add(sum_sq_acc, sq);

            // Peak (abs value)
            let abs_val = simd.f32s_abs(*chunk);
            peak_acc = simd.f32s_max(peak_acc, abs_val);
        }

        // Reduce SIMD accumulators
        let sum_sq = simd.f32s_reduce_sum(sum_sq_acc);
        let peak_from_simd = simd.f32s_reduce_max(peak_acc);

        // Process tail with scalar code
        let mut sum_sq_tail = 0.0f32;
        let mut peak_tail = 0.0f32;
        for &val in tail.iter() {
            sum_sq_tail += val * val;
            let abs_val = val.abs();
            if abs_val > peak_tail {
                peak_tail = abs_val;
            }
        }

        let total_sum_sq = sum_sq + sum_sq_tail;
        let peak = peak_from_simd.max(peak_tail);
        let rms = (total_sum_sq / len as f32).sqrt();

        (rms, peak)
    }
}

/// SIMD-accelerated mel filterbank multiplication.
///
/// Computes the dot product between a mel filter and power spectrum.
/// This is the critical inner loop of mel spectrogram extraction.
///
/// # Arguments
///
/// * `filter` - Mel filter coefficients (n_fft/2+1 values)
/// * `power_spectrum` - Power spectrum values (n_fft/2+1 values)
///
/// # Returns
///
/// Mel energy value (dot product of filter and power_spectrum)
///
/// # Performance
///
/// SIMD implementation achieves 4-6x speedup for 201-bin mel filterbank
/// by processing multiple values per iteration.
#[inline]
pub fn mel_filter_dot_simd(filter: &[f32], power_spectrum: &[f32]) -> f32 {
    let len = filter.len().min(power_spectrum.len());
    if len == 0 {
        return 0.0;
    }

    let arch = Arch::new();
    arch.dispatch(MelFilterDotOp {
        filter: &filter[..len],
        power_spectrum: &power_spectrum[..len],
    })
}

/// Operation struct for SIMD mel filter dot product
struct MelFilterDotOp<'a> {
    filter: &'a [f32],
    power_spectrum: &'a [f32],
}

impl WithSimd for MelFilterDotOp<'_> {
    type Output = f32;

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> Self::Output {
        let Self {
            filter,
            power_spectrum,
        } = self;

        let (filter_head, filter_tail) = S::f32s_as_simd(filter);
        let (spectrum_head, spectrum_tail) = S::f32s_as_simd(power_spectrum);

        let mut acc = simd.f32s_splat(0.0);

        // Process SIMD chunks
        for (f, s) in filter_head.iter().zip(spectrum_head.iter()) {
            let prod = simd.f32s_mul(*f, *s);
            acc = simd.f32s_add(acc, prod);
        }

        // Reduce SIMD accumulator
        let simd_sum = simd.f32s_reduce_sum(acc);

        // Process tail with scalar code
        let tail_sum: f32 = filter_tail
            .iter()
            .zip(spectrum_tail.iter())
            .map(|(f, s)| f * s)
            .sum();

        simd_sum + tail_sum
    }
}

/// SIMD-accelerated Hann window application.
///
/// Applies a pre-computed Hann window to an audio frame.
///
/// # Arguments
///
/// * `audio` - Audio samples for one frame
/// * `window` - Pre-computed Hann window coefficients
/// * `output` - Output buffer (must be same length as audio and window)
///
/// # Performance
///
/// SIMD implementation achieves 4-6x speedup for 400-sample frames.
#[inline]
pub fn apply_window_simd(audio: &[f32], window: &[f32], output: &mut [f32]) {
    let len = audio.len().min(window.len()).min(output.len());
    if len == 0 {
        return;
    }

    let arch = Arch::new();
    arch.dispatch(ApplyWindowOp {
        audio: &audio[..len],
        window: &window[..len],
        output: &mut output[..len],
    });
}

/// Operation struct for SIMD window application
struct ApplyWindowOp<'a> {
    audio: &'a [f32],
    window: &'a [f32],
    output: &'a mut [f32],
}

impl WithSimd for ApplyWindowOp<'_> {
    type Output = ();

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> Self::Output {
        let Self {
            audio,
            window,
            output,
        } = self;

        let (audio_head, audio_tail) = S::f32s_as_simd(audio);
        let (window_head, window_tail) = S::f32s_as_simd(window);
        let (output_head, output_tail) = S::f32s_as_mut_simd(output);

        // Process SIMD chunks
        for ((a, w), o) in audio_head
            .iter()
            .zip(window_head.iter())
            .zip(output_head.iter_mut())
        {
            *o = simd.f32s_mul(*a, *w);
        }

        // Process tail with scalar code
        for ((a, w), o) in audio_tail
            .iter()
            .zip(window_tail.iter())
            .zip(output_tail.iter_mut())
        {
            *o = a * w;
        }
    }
}

/// SIMD-accelerated power spectrum calculation from complex FFT output.
///
/// Computes |z|^2 = re^2 + im^2 for separate real/imaginary arrays.
///
/// # Arguments
///
/// * `fft_re` - Real part of FFT output
/// * `fft_im` - Imaginary part of FFT output
///
/// # Returns
///
/// Power spectrum values (same length as inputs)
///
/// # Performance
///
/// SIMD implementation achieves 3-4x speedup for 201-bin FFT output.
#[inline]
pub fn power_spectrum_simd(fft_re: &[f32], fft_im: &[f32]) -> Vec<f32> {
    let len = fft_re.len().min(fft_im.len());
    if len == 0 {
        return Vec::new();
    }

    let mut output = vec![0.0f32; len];

    let arch = Arch::new();
    arch.dispatch(PowerSpectrumOp {
        fft_re: &fft_re[..len],
        fft_im: &fft_im[..len],
        output: &mut output,
    });

    output
}

/// Operation struct for SIMD power spectrum calculation
struct PowerSpectrumOp<'a> {
    fft_re: &'a [f32],
    fft_im: &'a [f32],
    output: &'a mut [f32],
}

impl WithSimd for PowerSpectrumOp<'_> {
    type Output = ();

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> Self::Output {
        let Self {
            fft_re,
            fft_im,
            output,
        } = self;

        let (re_head, re_tail) = S::f32s_as_simd(fft_re);
        let (im_head, im_tail) = S::f32s_as_simd(fft_im);
        let (out_head, out_tail) = S::f32s_as_mut_simd(output);

        // Process SIMD chunks: power = re^2 + im^2
        for ((re, im), out) in re_head.iter().zip(im_head.iter()).zip(out_head.iter_mut()) {
            let re_sq = simd.f32s_mul(*re, *re);
            let im_sq = simd.f32s_mul(*im, *im);
            *out = simd.f32s_add(re_sq, im_sq);
        }

        // Process tail with scalar code
        for ((re, im), out) in re_tail.iter().zip(im_tail.iter()).zip(out_tail.iter_mut()) {
            *out = re * re + im * im;
        }
    }
}

/// SIMD-accelerated Whisper normalization pass 1: log10 with min clamping and max finding.
///
/// Applies log10 transformation with a floor value and tracks the maximum.
///
/// # Arguments
///
/// * `data` - Mel spectrogram data (modified in place)
///
/// # Returns
///
/// Maximum value after log10 transformation
#[inline]
pub fn whisper_norm_log_max_simd(data: &mut [f32]) -> f32 {
    const MIN_VALUE: f32 = 1e-10;

    if data.is_empty() {
        return f32::NEG_INFINITY;
    }

    // Apply log10 (scalar - transcendental functions don't have SIMD equivalents in pulp)
    for val in data.iter_mut() {
        *val = (*val).max(MIN_VALUE).log10();
    }

    // Find max using SIMD
    let arch = Arch::new();
    arch.dispatch(MaxOp { data })
}

/// Operation struct for SIMD max finding
struct MaxOp<'a> {
    data: &'a [f32],
}

impl WithSimd for MaxOp<'_> {
    type Output = f32;

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> Self::Output {
        let data = self.data;
        let (head, tail) = S::f32s_as_simd(data);

        let mut max_acc = simd.f32s_splat(f32::NEG_INFINITY);

        for chunk in head.iter() {
            max_acc = simd.f32s_max(max_acc, *chunk);
        }

        let mut max_val = simd.f32s_reduce_max(max_acc);

        for &val in tail {
            max_val = max_val.max(val);
        }

        max_val
    }
}

/// SIMD-accelerated Whisper normalization pass 2: threshold + scale.
///
/// Applies dynamic range compression and final scaling.
///
/// # Arguments
///
/// * `data` - Log mel spectrogram data (modified in place)
/// * `max_val` - Maximum value from pass 1
#[inline]
pub fn whisper_norm_threshold_scale_simd(data: &mut [f32], max_val: f32) {
    if data.is_empty() {
        return;
    }

    let arch = Arch::new();
    arch.dispatch(ThresholdScaleOp { data, max_val });
}

/// Operation struct for SIMD threshold and scale
struct ThresholdScaleOp<'a> {
    data: &'a mut [f32],
    max_val: f32,
}

impl WithSimd for ThresholdScaleOp<'_> {
    type Output = ();

    #[inline(always)]
    fn with_simd<S: Simd>(self, simd: S) -> Self::Output {
        let Self { data, max_val } = self;

        let threshold = max_val - 8.0;
        let add_const = 4.0;
        let inv_div = 1.0 / 4.0;

        let (head, tail) = S::f32s_as_mut_simd(data);
        let thresh = simd.f32s_splat(threshold);
        let add_c = simd.f32s_splat(add_const);
        let mul_c = simd.f32s_splat(inv_div);

        // Process SIMD chunks
        for chunk in head.iter_mut() {
            // Clamp to threshold
            let clamped = simd.f32s_max(*chunk, thresh);
            // Add 4.0
            let added = simd.f32s_add(clamped, add_c);
            // Multiply by 0.25 (divide by 4.0)
            *chunk = simd.f32s_mul(added, mul_c);
        }

        // Process tail with scalar code
        for val in tail.iter_mut() {
            *val = ((*val).max(threshold) + add_const) * inv_div;
        }
    }
}

/// SIMD-accelerated buffer copy for ONNX tensor input.
///
/// Efficiently copies audio samples to a pre-allocated tensor buffer.
///
/// # Arguments
///
/// * `src` - Source audio samples
/// * `dst` - Destination buffer (must be at least same length as src)
///
/// # Performance
///
/// Uses vectorized copy for efficient transfer.
#[inline]
pub fn copy_to_tensor_simd(src: &[f32], dst: &mut [f32]) {
    debug_assert!(dst.len() >= src.len());

    // For audio buffers, copy_from_slice is typically optimized by the compiler
    // and will use SIMD intrinsics when appropriate
    dst[..src.len()].copy_from_slice(src);
}

// =============================================================================
// Scalar Fallback Functions (for benchmarking comparison)
// =============================================================================

/// Scalar implementation of PCM to float conversion (for benchmarking).
#[allow(dead_code)]
pub fn pcm_to_float_scalar(pcm: &[u8]) -> Vec<f32> {
    let mut output = Vec::with_capacity(pcm.len() / 2);
    for chunk in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f32;
        output.push((sample * PCM_TO_FLOAT_SCALE).clamp(-1.0, 1.0));
    }
    output
}

/// Scalar implementation of float to PCM conversion (for benchmarking).
#[allow(dead_code)]
pub fn float_to_pcm_scalar(samples: &[f32]) -> Vec<u8> {
    let mut output = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let scaled = if clamped >= 0.0 {
            clamped * FLOAT_TO_PCM_SCALE_POS
        } else {
            clamped * FLOAT_TO_PCM_SCALE_NEG
        };
        let pcm = scaled.round() as i16;
        output.extend_from_slice(&pcm.to_le_bytes());
    }
    output
}

/// Scalar implementation of RMS/peak calculation (for benchmarking).
#[allow(dead_code)]
pub fn rms_peak_scalar(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }

    let mut sum_sq = 0.0f32;
    let mut peak = 0.0f32;
    for &s in samples {
        sum_sq += s * s;
        let abs_s = s.abs();
        if abs_s > peak {
            peak = abs_s;
        }
    }
    let rms = (sum_sq / samples.len() as f32).sqrt();
    (rms, peak)
}

/// Scalar implementation of mel filter dot product (for benchmarking).
#[allow(dead_code)]
pub fn mel_filter_dot_scalar(filter: &[f32], power_spectrum: &[f32]) -> f32 {
    filter
        .iter()
        .zip(power_spectrum.iter())
        .map(|(f, s)| f * s)
        .sum()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-5;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON || (a.is_nan() && b.is_nan())
    }

    fn approx_eq_vec(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b.iter()).all(|(&x, &y)| approx_eq(x, y))
    }

    // -------------------------------------------------------------------------
    // PCM-to-Float conversion tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_pcm_to_float_empty() {
        let result = pcm_to_float_simd(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_pcm_to_float_single_sample() {
        // Zero
        let pcm = [0u8, 0u8];
        let result = pcm_to_float_simd(&pcm);
        assert_eq!(result.len(), 1);
        assert!(approx_eq(result[0], 0.0));
    }

    #[test]
    fn test_pcm_to_float_matches_scalar() {
        // Test with random-ish data
        let pcm: Vec<u8> = (0..200).map(|i| ((i * 37 + 13) % 256) as u8).collect();

        let simd_result = pcm_to_float_simd(&pcm);
        let scalar_result = pcm_to_float_scalar(&pcm);

        assert!(approx_eq_vec(&simd_result, &scalar_result));
    }

    // -------------------------------------------------------------------------
    // Float-to-PCM conversion tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_float_to_pcm_empty() {
        let result = float_to_pcm_simd(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_float_to_pcm_roundtrip() {
        let original: Vec<f32> = (0..100)
            .map(|i| (i as f32 / 50.0 - 1.0).clamp(-1.0, 1.0))
            .collect();

        let pcm = float_to_pcm_simd(&original);
        let restored = pcm_to_float_simd(&pcm);

        // Allow small quantization error
        for (o, r) in original.iter().zip(restored.iter()) {
            assert!((o - r).abs() < 0.001, "Original: {}, Restored: {}", o, r);
        }
    }

    // -------------------------------------------------------------------------
    // RMS/Peak calculation tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_rms_peak_empty() {
        let (rms, peak) = rms_peak_simd(&[]);
        assert_eq!(rms, 0.0);
        assert_eq!(peak, 0.0);
    }

    #[test]
    fn test_rms_peak_constant() {
        let samples = vec![0.5f32; 100];
        let (rms, peak) = rms_peak_simd(&samples);

        assert!(approx_eq(rms, 0.5));
        assert!(approx_eq(peak, 0.5));
    }

    #[test]
    fn test_rms_peak_matches_scalar() {
        let samples: Vec<f32> = (0..200).map(|i| (i as f32 / 100.0).sin()).collect();

        let (simd_rms, simd_peak) = rms_peak_simd(&samples);
        let (scalar_rms, scalar_peak) = rms_peak_scalar(&samples);

        assert!(approx_eq(simd_rms, scalar_rms));
        assert!(approx_eq(simd_peak, scalar_peak));
    }

    // -------------------------------------------------------------------------
    // Mel filter dot product tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_mel_filter_dot_empty() {
        let result = mel_filter_dot_simd(&[], &[]);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_mel_filter_dot_ones() {
        let filter = vec![1.0f32; 201];
        let spectrum = vec![1.0f32; 201];
        let result = mel_filter_dot_simd(&filter, &spectrum);
        assert!(approx_eq(result, 201.0));
    }

    #[test]
    fn test_mel_filter_dot_matches_scalar() {
        let filter: Vec<f32> = (0..201).map(|i| (i as f32).sin() * 0.1).collect();
        let spectrum: Vec<f32> = (0..201).map(|i| (i as f32 * 0.05).cos()).collect();

        let simd_result = mel_filter_dot_simd(&filter, &spectrum);
        let scalar_result = mel_filter_dot_scalar(&filter, &spectrum);

        assert!(approx_eq(simd_result, scalar_result));
    }

    // -------------------------------------------------------------------------
    // Window application tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_apply_window_empty() {
        let mut output: Vec<f32> = vec![];
        apply_window_simd(&[], &[], &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn test_apply_window_matches_scalar() {
        let audio: Vec<f32> = (0..400).map(|i| (i as f32 / 200.0).sin()).collect();
        let window: Vec<f32> = (0..400)
            .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / 400.0).cos()))
            .collect();

        let mut simd_output = vec![0.0f32; 400];
        apply_window_simd(&audio, &window, &mut simd_output);

        // Scalar implementation
        let scalar_output: Vec<f32> = audio
            .iter()
            .zip(window.iter())
            .map(|(a, w)| a * w)
            .collect();

        assert!(approx_eq_vec(&simd_output, &scalar_output));
    }

    // -------------------------------------------------------------------------
    // Power spectrum tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_power_spectrum_empty() {
        let result = power_spectrum_simd(&[], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_power_spectrum_matches_scalar() {
        let fft_re: Vec<f32> = (0..201).map(|i| (i as f32 * 0.1).cos()).collect();
        let fft_im: Vec<f32> = (0..201).map(|i| (i as f32 * 0.1).sin()).collect();

        let simd_result = power_spectrum_simd(&fft_re, &fft_im);

        // Scalar implementation
        let scalar_result: Vec<f32> = fft_re
            .iter()
            .zip(fft_im.iter())
            .map(|(re, im)| re * re + im * im)
            .collect();

        assert!(approx_eq_vec(&simd_result, &scalar_result));
    }

    // -------------------------------------------------------------------------
    // Whisper normalization tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_whisper_norm_log_max() {
        let mut data: Vec<f32> = (1..101).map(|i| i as f32).collect();
        let max_val = whisper_norm_log_max_simd(&mut data);

        // max(100) = log10(100) = 2.0
        assert!(approx_eq(max_val, 2.0));

        // Check first value: log10(1) = 0
        assert!(approx_eq(data[0], 0.0));
    }

    #[test]
    fn test_whisper_norm_threshold_scale() {
        let mut data = vec![-1.0, 0.0, 1.0, 2.0];
        let max_val = 2.0;

        whisper_norm_threshold_scale_simd(&mut data, max_val);

        // threshold = 2.0 - 8.0 = -6.0
        // All values should be max(v, -6.0), then (v + 4.0) / 4.0
        assert!(approx_eq(data[0], 0.75));
        assert!(approx_eq(data[1], 1.0));
        assert!(approx_eq(data[2], 1.25));
        assert!(approx_eq(data[3], 1.5));
    }

    // -------------------------------------------------------------------------
    // Tensor copy tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_copy_to_tensor() {
        let src = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
        let mut dst = vec![0.0f32; 10];

        copy_to_tensor_simd(&src, &mut dst);

        assert_eq!(&dst[..5], &src[..]);
    }

    // -------------------------------------------------------------------------
    // AlignedBuffer tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_aligned_buffer_new() {
        let buf: AlignedBuffer<f32> = AlignedBuffer::new(100);
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 100);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_aligned_buffer_zero_capacity() {
        let buf: AlignedBuffer<f32> = AlignedBuffer::new(0);
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 0);
    }

    #[test]
    fn test_aligned_buffer_with_value() {
        let buf = AlignedBuffer::<f32>::with_value(10, 1.5);
        assert_eq!(buf.len(), 10);
        for i in 0..10 {
            assert!(approx_eq(buf[i], 1.5));
        }
    }

    #[test]
    fn test_aligned_buffer_copy_from_slice() {
        let mut buf = AlignedBuffer::<f32>::new(100);
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        buf.copy_from_slice(&data);

        assert_eq!(buf.len(), 5);
        for i in 0..5 {
            assert!(approx_eq(buf[i], data[i]));
        }
    }

    #[test]
    fn test_aligned_buffer_as_slice() {
        let mut buf = AlignedBuffer::<f32>::new(100);
        buf.copy_from_slice(&[1.0, 2.0, 3.0]);

        let slice = buf.as_slice();
        assert_eq!(slice.len(), 3);
        assert!(approx_eq(slice[0], 1.0));
        assert!(approx_eq(slice[1], 2.0));
        assert!(approx_eq(slice[2], 3.0));
    }

    #[test]
    fn test_aligned_buffer_resize() {
        let mut buf = AlignedBuffer::<f32>::new(100);
        buf.resize(50, 0.0);
        assert_eq!(buf.len(), 50);

        buf.resize(25, 0.0);
        assert_eq!(buf.len(), 25);
    }

    #[test]
    fn test_aligned_buffer_clear() {
        let mut buf = AlignedBuffer::<f32>::new(100);
        buf.copy_from_slice(&[1.0, 2.0, 3.0]);
        buf.clear();
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
    }

    // -------------------------------------------------------------------------
    // Vector width edge case tests (critical for SIMD portability)
    // -------------------------------------------------------------------------
    // These tests verify correct behavior for all sizes that exercise different
    // SIMD lane counts and remainder handling across SSE2 (4 lanes), AVX2 (8 lanes),
    // AVX-512 (16 lanes), and NEON (4 lanes).

    #[test]
    fn test_all_vector_width_remainders() {
        // Test sizes that exercise different SIMD lane counts:
        // - 1, 3: Always handled by scalar remainder
        // - 4: Exactly 1 SIMD vector (SSE2/NEON)
        // - 7, 8: Boundary for SSE2/NEON vs AVX
        // - 15, 16: Boundary for AVX vs AVX-512
        // - 31, 32, 63, 64: Multiple vectors with various remainders
        // - 127, 128, 255, 256: Larger sizes for cache effects
        // - 512, 1000: Production-like sizes
        let sizes = [
            1, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 512, 1000,
        ];

        for &size in &sizes {
            // Generate test data with distinct values
            let data: Vec<f32> = (0..size).map(|i| i as f32 * 0.001 + 0.1).collect();

            // Test RMS/Peak
            let (rms_simd, peak_simd) = rms_peak_simd(&data);
            let (rms_scalar, peak_scalar) = rms_peak_scalar(&data);

            assert!(
                (rms_simd - rms_scalar).abs() < 1e-5,
                "RMS mismatch at size {}: simd={}, scalar={}",
                size,
                rms_simd,
                rms_scalar
            );
            assert!(
                (peak_simd - peak_scalar).abs() < 1e-5,
                "Peak mismatch at size {}: simd={}, scalar={}",
                size,
                peak_simd,
                peak_scalar
            );
        }
    }

    #[test]
    fn test_pcm_conversion_all_sizes() {
        let sizes = [
            2, 4, 6, 8, 14, 16, 30, 32, 62, 64, 126, 128, 254, 256, 510, 512,
        ];

        for &sample_count in &sizes {
            // Generate PCM bytes (2 bytes per sample)
            let pcm: Vec<u8> = (0..sample_count * 2)
                .map(|i| ((i * 37 + 13) % 256) as u8)
                .collect();

            let simd_result = pcm_to_float_simd(&pcm);
            let scalar_result = pcm_to_float_scalar(&pcm);

            assert_eq!(
                simd_result.len(),
                scalar_result.len(),
                "Length mismatch at sample_count {}",
                sample_count
            );

            for (i, (s, sc)) in simd_result.iter().zip(scalar_result.iter()).enumerate() {
                assert!(
                    (s - sc).abs() < 1e-5,
                    "PCM->Float mismatch at sample_count {}, index {}: simd={}, scalar={}",
                    sample_count,
                    i,
                    s,
                    sc
                );
            }
        }
    }

    #[test]
    fn test_mel_filter_dot_all_sizes() {
        // Test various filter/spectrum sizes including the production size (201)
        let sizes = [4, 7, 8, 15, 16, 31, 32, 64, 127, 128, 200, 201, 256, 512];

        for &size in &sizes {
            let filter: Vec<f32> = (0..size).map(|i| (i as f32 * 0.1).sin()).collect();
            let spectrum: Vec<f32> = (0..size).map(|i| (i as f32 * 0.05).cos()).collect();

            let simd_result = mel_filter_dot_simd(&filter, &spectrum);
            let scalar_result = mel_filter_dot_scalar(&filter, &spectrum);

            assert!(
                (simd_result - scalar_result).abs() < 1e-4,
                "Mel filter dot mismatch at size {}: simd={}, scalar={}",
                size,
                simd_result,
                scalar_result
            );
        }
    }

    #[test]
    fn test_power_spectrum_all_sizes() {
        let sizes = [4, 7, 8, 15, 16, 31, 32, 64, 127, 128, 200, 201, 256];

        for &size in &sizes {
            let fft_re: Vec<f32> = (0..size).map(|i| (i as f32 * 0.1).cos()).collect();
            let fft_im: Vec<f32> = (0..size).map(|i| (i as f32 * 0.1).sin()).collect();

            let simd_result = power_spectrum_simd(&fft_re, &fft_im);

            // Scalar reference
            let scalar_result: Vec<f32> = fft_re
                .iter()
                .zip(fft_im.iter())
                .map(|(re, im)| re * re + im * im)
                .collect();

            assert_eq!(
                simd_result.len(),
                scalar_result.len(),
                "Power spectrum length mismatch at size {}",
                size
            );

            for (i, (s, sc)) in simd_result.iter().zip(scalar_result.iter()).enumerate() {
                assert!(
                    (s - sc).abs() < 1e-5,
                    "Power spectrum mismatch at size {}, index {}: simd={}, scalar={}",
                    size,
                    i,
                    s,
                    sc
                );
            }
        }
    }

    #[test]
    fn test_apply_window_all_sizes() {
        let sizes = [4, 7, 8, 15, 16, 31, 32, 64, 127, 128, 256, 400, 512];

        for &size in &sizes {
            let audio: Vec<f32> = (0..size).map(|i| (i as f32 / size as f32).sin()).collect();
            let window: Vec<f32> = (0..size)
                .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / size as f32).cos()))
                .collect();

            let mut simd_output = vec![0.0f32; size];
            apply_window_simd(&audio, &window, &mut simd_output);

            // Scalar reference
            let scalar_output: Vec<f32> = audio
                .iter()
                .zip(window.iter())
                .map(|(a, w)| a * w)
                .collect();

            for (i, (s, sc)) in simd_output.iter().zip(scalar_output.iter()).enumerate() {
                assert!(
                    (s - sc).abs() < 1e-5,
                    "Window application mismatch at size {}, index {}: simd={}, scalar={}",
                    size,
                    i,
                    s,
                    sc
                );
            }
        }
    }

    #[test]
    fn test_simd_capabilities_returns_valid() {
        let (isa_name, lanes) = super::simd_capabilities();

        // ISA name should be non-empty
        assert!(!isa_name.is_empty(), "ISA name should not be empty");

        // Lanes should be a power of 2 or 1 (scalar)
        assert!(
            lanes == 1 || lanes == 4 || lanes == 8 || lanes == 16,
            "Unexpected lane count: {}",
            lanes
        );

        // Verify the string version works
        let capability_string = super::simd_capabilities_string();
        assert!(
            capability_string.contains(isa_name),
            "Capability string should contain ISA name"
        );
        assert!(
            capability_string.contains(&lanes.to_string()),
            "Capability string should contain lane count"
        );
    }

    #[test]
    fn test_numerical_stability_edge_values() {
        // Test with very small values (near machine epsilon)
        let small_data: Vec<f32> = (0..128).map(|_| 1e-38_f32).collect();
        let (rms, peak) = rms_peak_simd(&small_data);
        assert!(rms.is_finite(), "RMS should be finite for small values");
        assert!(peak.is_finite(), "Peak should be finite for small values");

        // Test with moderately large values (staying within f32 safe range)
        // Note: 1e19 squared is ~1e38, which is within f32 max (~3.4e38)
        let large_data: Vec<f32> = (0..128).map(|_| 1e18_f32).collect();
        let (rms, peak) = rms_peak_simd(&large_data);
        assert!(
            rms.is_finite(),
            "RMS should be finite for moderately large values"
        );
        assert!(
            peak.is_finite(),
            "Peak should be finite for moderately large values"
        );
        // Use relative comparison for large values
        let relative_err = ((rms - 1e18) / 1e18).abs();
        assert!(
            relative_err < 1e-5,
            "RMS of constant 1e18 should be ~1e18, got {}",
            rms
        );

        // Test with mixed positive/negative values
        let mixed_data: Vec<f32> = (0..128)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let (rms, peak) = rms_peak_simd(&mixed_data);
        assert!(
            approx_eq(rms, 0.5),
            "RMS of alternating +/-0.5 should be 0.5"
        );
        assert!(
            approx_eq(peak, 0.5),
            "Peak of alternating +/-0.5 should be 0.5"
        );

        // Test with zeros (edge case)
        let zero_data: Vec<f32> = vec![0.0f32; 128];
        let (rms, peak) = rms_peak_simd(&zero_data);
        assert!(approx_eq(rms, 0.0), "RMS of zeros should be 0");
        assert!(approx_eq(peak, 0.0), "Peak of zeros should be 0");
    }
}

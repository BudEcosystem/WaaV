#[cfg(feature = "noise-filter")]
pub mod implementation {
    use bytes::Bytes;
    use deep_filter::tract::{DfParams, DfTract, RuntimeParams};
    use ndarray::{Array2, ArrayView2};
    use std::sync::{
        Arc, LazyLock,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tokio::sync::{Mutex, Notify, mpsc, oneshot};
    use tracing::{error, info};

    // Import SIMD-optimized operations
    use crate::utils::simd_ops;

    /// Configuration for DeepFilterNet model parameters.
    ///
    /// This static configuration is loaded once at program startup to avoid repeated initialization.
    /// The parameters are optimized for noise cancellation while preserving speech quality.
    /// Using LazyLock ensures thread-safe initialization exactly once.
    static MODEL_PARAMS: LazyLock<DfParams> = LazyLock::new(DfParams::default);

    /// Create optimized runtime parameters for DeepFilterNet
    /// Based on the official implementation, we use the default parameters
    /// with optional post-filter for improved echo suppression
    fn create_runtime_params() -> RuntimeParams {
        // Start with official defaults
        let mut params = RuntimeParams::default();

        // Enable post-filter for better echo suppression and residual noise removal
        // This is particularly important for mobile phone usage and conference calls
        params = params.with_post_filter(0.02);

        // Optionally adjust attenuation limit for more natural sound
        // Default is 100dB (no limit), but we can be slightly more conservative
        // to prevent over-processing while still removing most noise
        params = params.with_atten_lim(40.0); // Limit to 40dB reduction

        params
    }

    /// Pre-calculated constants for performance optimization
    const SILENCE_THRESHOLD: f32 = 1e-7;
    /// Threshold for detecting high SNR (clean) audio - audio above this receives light processing
    /// Increased from 0.1 to 0.15 to apply full processing to more audio samples
    /// This ensures moderately clean audio still benefits from noise reduction
    const HIGH_SNR_RMS_THRESHOLD: f32 = 0.15;
    /// Peak threshold for high SNR detection - increased from 0.3 to 0.4
    const HIGH_SNR_PEAK_THRESHOLD: f32 = 0.4;
    const LOW_ENERGY_RMS_THRESHOLD: f32 = 0.005;
    const LOW_ENERGY_PEAK_THRESHOLD: f32 = 0.02;
    const SHORT_AUDIO_DURATION: f32 = 1.0;
    const VERY_SHORT_AUDIO_DURATION: f32 = 0.2;
    const VOLUME_SAFETY_MARGIN: f32 = 0.95;
    const LIGHT_PROCESSING_SAFETY_MARGIN: f32 = 0.98;
    const HIGH_PASS_CUTOFF_HZ: f32 = 80.0;
    pub(crate) const PCM_TO_FLOAT_SCALE: f32 = 1.0 / 32768.0;
    pub(crate) const FLOAT_TO_PCM_SCALE_POS: f32 = 32767.0;
    pub(crate) const FLOAT_TO_PCM_SCALE_NEG: f32 = 32768.0;
    const MAX_SKIP_FRAMES: usize = 5; // Skip after 5 consecutive silent frames

    // A task for a worker: audio bytes, sample rate, and a one-shot channel to send the result back.
    type WorkerTask = (
        Bytes,
        u32, // sample_rate
        oneshot::Sender<Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>>,
    );

    // Type aliases for cleaner code
    type WorkerSender = mpsc::Sender<WorkerTask>;
    type PoolSender = mpsc::Sender<WorkerSender>;
    type PoolReceiver = Arc<Mutex<mpsc::Receiver<WorkerSender>>>;
    type SenderPool = (PoolSender, PoolReceiver, Arc<AtomicUsize>, Arc<Notify>);

    // The pool of senders to the worker threads, managed by a channel.
    // This is the core of our thread-safe, async-friendly pool for the !Send models.
    static SENDER_POOL: LazyLock<SenderPool> = LazyLock::new(|| {
        // Use a pool size based on available CPU cores, with a minimum of 2.
        let pool_size = num_cpus::get().max(2);
        let (pool_tx, pool_rx) = mpsc::channel(pool_size);
        let live_workers = Arc::new(AtomicUsize::new(0));
        let worker_death = Arc::new(Notify::new());
        let (init_tx, init_rx) = std::sync::mpsc::channel();
        let mut launched = 0usize;

        for worker_index in 0..pool_size {
            let pool_tx_clone = pool_tx.clone();
            let init_tx_clone = init_tx.clone();

            // Spawn a dedicated OS thread for each worker.
            // This is crucial for pinning the !Send DfTract model to a single thread.
            match std::thread::Builder::new()
                .name(format!("waav-noise-filter-{worker_index}"))
                .spawn(move || {
                let (worker_task_tx, mut worker_task_rx) = mpsc::channel::<WorkerTask>(1);

                // Create the expensive DfTract model ONCE on this thread.
                let df_params = &*MODEL_PARAMS;
                let rt_params = create_runtime_params();
                let mut model = match DfTract::new(df_params.clone(), &rt_params) {
                    Ok(model) => model,
                    Err(err) => {
                        error!(
                            worker_index,
                            error = %err,
                            "Failed to create DfTract model in noise-filter worker"
                        );
                        let _ = init_tx_clone.send(false);
                        return;
                    }
                };

                // Provide this worker's sender to the pool only after the model is ready.
                if pool_tx_clone.blocking_send(worker_task_tx).is_err() {
                    let _ = init_tx_clone.send(false);
                    return;
                }
                let _ = init_tx_clone.send(true);
                drop(init_tx_clone);

                info!(
                    worker_index,
                    "DeepFilterNet initialized with post-filter enabled for optimal noise reduction"
                );
                let mut skip_counter = 0usize;

                // The worker's main loop. It blocks efficiently until a task arrives.
                while let Some((pcm, sample_rate, result_tx)) = worker_task_rx.blocking_recv() {
                    let result =
                        process_audio_chunk(&mut model, &pcm, sample_rate, &mut skip_counter);
                    // Send result back to the caller. Ignore error if caller hung up.
                    let _ = result_tx.send(result);
                }
                })
            {
                Ok(_) => launched += 1,
                Err(err) => {
                    error!(
                        worker_index,
                        error = %err,
                        "Failed to spawn noise-filter worker thread"
                    );
                }
            }
        }

        drop(init_tx);
        let initialized = init_rx
            .into_iter()
            .take(launched)
            .filter(|ready| *ready)
            .count();
        live_workers.store(initialized, Ordering::Release);
        if initialized == 0 {
            error!("Noise-filter worker pool initialized with no live workers");
        } else if initialized < pool_size {
            error!(
                initialized,
                requested = pool_size,
                "Noise-filter worker pool initialized with fewer workers than requested"
            );
        }

        (
            pool_tx,
            Arc::new(Mutex::new(pool_rx)),
            live_workers,
            worker_death,
        )
    });

    fn mark_worker_dead(live_workers: &Arc<AtomicUsize>, worker_death: &Arc<Notify>) {
        let mut current = live_workers.load(Ordering::Acquire);
        while current > 0 {
            match live_workers.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(next) => current = next,
            }
        }
        worker_death.notify_waiters();
        worker_death.notify_one();
    }

    /// RAII guard that holds a worker sender checked out from the pool.
    /// On drop, it automatically returns the sender to the pool for reuse.
    struct PooledWorkerSender {
        sender: Option<mpsc::Sender<WorkerTask>>,
        pool_tx: mpsc::Sender<mpsc::Sender<WorkerTask>>,
        live_workers: Arc<AtomicUsize>,
        worker_death: Arc<Notify>,
    }

    impl Drop for PooledWorkerSender {
        fn drop(&mut self) {
            if let Some(sender) = self.sender.take() {
                if sender.is_closed() {
                    mark_worker_dead(&self.live_workers, &self.worker_death);
                    return;
                }
                // Best-effort attempt to return the sender. If this fails, dropping this sender closes that
                // worker's task channel, so the live count is reduced and waiters are woken.
                if self.pool_tx.try_send(sender).is_err() {
                    mark_worker_dead(&self.live_workers, &self.worker_death);
                }
            }
        }
    }

    /// Asynchronously checks out a worker sender from the pool.
    /// It will wait efficiently if the pool is empty.
    async fn get_sender() -> Result<PooledWorkerSender, Box<dyn std::error::Error + Send + Sync>> {
        let (pool_tx, pool_rx_mutex, live_workers, worker_death) = &*SENDER_POOL;
        loop {
            if live_workers.load(Ordering::Acquire) == 0 {
                return Err("Noise-filter worker pool has no live workers".into());
            }

            // Lock the mutex asynchronously. The guard is Send and can be held across an await.
            let mut pool_rx = pool_rx_mutex.lock().await;
            let Some(sender) = (tokio::select! {
                maybe_sender = pool_rx.recv() => maybe_sender,
                _ = worker_death.notified() => continue,
                _ = tokio::time::sleep(Duration::from_secs(1)) => continue,
            }) else {
                return Err("Noise-filter worker pool channel closed unexpectedly".into());
            };
            drop(pool_rx);

            if sender.is_closed() {
                mark_worker_dead(live_workers, worker_death);
                continue;
            }

            return Ok(PooledWorkerSender {
                sender: Some(sender),
                pool_tx: pool_tx.clone(),
                live_workers: live_workers.clone(),
                worker_death: worker_death.clone(),
            });
        }
    }

    /// Processes audio chunk using DeepFilterNet for noise reduction.
    ///
    /// This function analyzes audio characteristics and applies adaptive processing:
    /// - High SNR audio receives minimal processing to preserve quality
    /// - Short audio clips use light filtering to avoid artifacts
    /// - Full processing includes conservative blending with original audio
    ///
    /// # Arguments
    /// * `df` - Mutable reference to the DeepFilterNet model
    /// * `pcm` - Raw PCM audio data (16-bit little-endian format)
    /// * `sample_rate` - Audio sample rate in Hz (e.g., 16000, 48000)
    ///
    /// # Returns
    /// Processed audio as PCM bytes, or original audio if processing fails
    #[inline]
    fn process_audio_chunk(
        df: &mut DfTract,
        pcm: &[u8],
        sample_rate: u32,
        skip_counter: &mut usize,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        // Early validation to avoid unnecessary processing
        let pcm_len = pcm.len();
        if pcm_len < 2 || !pcm_len.is_multiple_of(2) {
            return Ok(pcm.to_owned());
        }

        // Convert PCM bytes to float samples using SIMD-optimized conversion
        // This achieves 4-8x speedup over scalar loop on supported platforms
        let wav = simd_ops::pcm_to_float_simd(pcm);

        // Early return for empty audio
        if wav.is_empty() {
            return Ok(pcm.to_owned());
        }

        // Check for silence and update skip counter
        if wav.iter().all(|s| s.abs() < SILENCE_THRESHOLD) {
            *skip_counter += 1;
            if *skip_counter > MAX_SKIP_FRAMES {
                // Return silence after too many silent frames
                return Ok(vec![0u8; pcm.len()]);
            }
            return Ok(pcm.to_owned());
        } else {
            *skip_counter = 0; // Reset on non-silent audio
        }

        // Analyze audio characteristics for adaptive processing
        let sample_rate_f32 = sample_rate as f32;
        let audio_duration = wav.len() as f32 / sample_rate_f32;

        // Calculate RMS and peak energy using SIMD-optimized implementation
        // This achieves 4-6x speedup by processing 4-8 samples per iteration
        let (rms_energy, peak_energy) = simd_ops::rms_peak_simd(&wav);

        // Apply adaptive processing based on audio characteristics

        // For high SNR audio (clear speech), apply minimal processing
        if rms_energy > HIGH_SNR_RMS_THRESHOLD && peak_energy > HIGH_SNR_PEAK_THRESHOLD {
            return apply_light_processing(pcm, &wav, sample_rate);
        }

        // For very short audio, use light processing or skip
        if audio_duration < SHORT_AUDIO_DURATION {
            if rms_energy < LOW_ENERGY_RMS_THRESHOLD || peak_energy < LOW_ENERGY_PEAK_THRESHOLD {
                return Ok(pcm.to_owned());
            }
            return apply_light_processing(pcm, &wav, sample_rate);
        }

        // Check minimum samples requirement
        let hop = df.hop_size;
        let min_samples = (512_usize).max(hop * 2);
        if wav.len() < min_samples {
            return apply_light_processing(pcm, &wav, sample_rate);
        }

        // Full DeepFilterNet processing with pre-allocated buffer
        let mut enhanced = Vec::with_capacity(wav.len());
        let mut idx = 0;

        // Process complete frames
        while idx + hop <= wav.len() {
            let frame = ArrayView2::from_shape((1, hop), &wav[idx..idx + hop])?;
            let mut out = Array2::<f32>::zeros((1, hop));

            // Process with DeepFilterNet - it handles everything internally:
            // - LSNR calculation and stage selection
            // - Masking and deep filtering
            // - Post-filter application
            // - Attenuation limiting
            df.process(frame, out.view_mut())?;

            // Direct memory copy for efficiency
            if let Some(out_slice) = out.as_slice() {
                enhanced.extend_from_slice(out_slice);
            } else {
                enhanced.extend(out.iter().copied());
            }
            idx += hop;
        }

        // Handle remaining samples
        let remaining = wav.len() - idx;
        if remaining > 0 {
            if remaining >= hop / 2 {
                let mut padded_frame = vec![0.0f32; hop];
                padded_frame[..remaining].copy_from_slice(&wav[idx..]);
                let frame = ArrayView2::from_shape((1, hop), &padded_frame)?;
                let mut out = Array2::<f32>::zeros((1, hop));
                df.process(frame, out.view_mut())?;
                if let Some(out_slice) = out.as_slice() {
                    enhanced.extend_from_slice(&out_slice[..remaining]);
                } else {
                    enhanced.extend(out.iter().take(remaining).copied());
                }
            } else {
                enhanced.extend_from_slice(&wav[idx..]);
            }
        }

        // NO BLENDING NEEDED - DfTract already handles attenuation limiting internally
        // The model applies sophisticated processing including:
        // - Post-filter for echo suppression (beta=0.015 for mobile)
        // - Attenuation limiting (15dB for mobile config)
        // - SNR-based stage selection

        // Light volume preservation to prevent clipping
        let original_max = peak_energy;
        let enhanced_max = enhanced.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        // Apply gentle volume normalization if needed
        if enhanced_max > 0.0 && original_max > 0.0 && enhanced_max > original_max * 1.2 {
            let volume_ratio = (original_max / enhanced_max) * VOLUME_SAFETY_MARGIN;
            // In-place modification for efficiency
            for sample in &mut enhanced {
                *sample *= volume_ratio;
            }
        }

        // Check for NaN/inf values
        if enhanced.iter().any(|s| !s.is_finite()) {
            return Ok(pcm.to_owned());
        }

        // Convert back to PCM bytes using SIMD-optimized conversion
        // This achieves 6-8x speedup over scalar loop on supported platforms
        let result = simd_ops::float_to_pcm_simd(&enhanced);

        Ok(result)
    }

    /// Applies light processing for short or high-quality audio clips.
    ///
    /// This function applies minimal filtering to preserve audio quality:
    /// - Very short clips (< 200ms) are returned unchanged
    /// - Longer clips receive gentle high-pass filtering at 80Hz
    /// - Original volume levels are preserved
    ///
    /// # Arguments
    /// * `original_pcm` - Original PCM audio data for fallback
    /// * `wav` - Audio samples as floating-point values
    /// * `sample_rate` - Audio sample rate in Hz
    ///
    /// # Returns
    /// Lightly processed audio as PCM bytes
    #[inline]
    fn apply_light_processing(
        original_pcm: &[u8],
        wav: &[f32],
        sample_rate: u32,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        // For very short clips, preserve original
        let sample_rate_f32 = sample_rate as f32;
        let duration = wav.len() as f32 / sample_rate_f32;

        if duration < VERY_SHORT_AUDIO_DURATION {
            // Very short - minimal processing, preserve original volume
            let max_val = wav.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            if max_val > 0.0 {
                // Keep original - no filtering for very short audio
                return Ok(original_pcm.to_vec());
            }
        }

        // Apply gentle high-pass filter for longer short clips
        // Pre-calculate filter coefficients
        let rc = 1.0 / (2.0 * std::f32::consts::PI * HIGH_PASS_CUTOFF_HZ);
        let dt = 1.0 / sample_rate_f32;
        let alpha = rc / (rc + dt);

        // Pre-allocate filtered buffer
        let mut filtered = Vec::with_capacity(wav.len());

        if !wav.is_empty() {
            filtered.push(wav[0]); // First sample unchanged

            // Apply high-pass filter
            let mut prev_input = wav[0];
            let mut prev_output = wav[0];

            for &current_input in &wav[1..] {
                let filtered_sample = alpha * (prev_output + current_input - prev_input);
                filtered.push(filtered_sample);
                prev_input = current_input;
                prev_output = filtered_sample;
            }
        }

        // Preserve original volume level
        let original_max = wav.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let filtered_max = filtered.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        // Apply volume normalization in-place if needed
        if filtered_max > 0.0 && original_max > 0.0 {
            let volume_ratio = (original_max / filtered_max) * LIGHT_PROCESSING_SAFETY_MARGIN;
            for sample in &mut filtered {
                *sample *= volume_ratio;
            }
        }

        // Convert back to PCM bytes using SIMD-optimized conversion
        // This achieves 6-8x speedup over scalar loop on supported platforms
        let result = simd_ops::float_to_pcm_simd(&filtered);

        Ok(result)
    }

    /// Performs asynchronous noise reduction using a worker thread pool.
    ///
    /// This function provides the main public interface for noise reduction.
    /// It efficiently manages CPU-intensive processing across multiple threads
    /// while maintaining an async interface for the caller.
    ///
    /// The function uses a pool of worker threads, each with its own DeepFilterNet
    /// model instance, to process audio in parallel without blocking the async runtime.
    ///
    /// # Arguments
    /// * `pcm` - Audio data in PCM format (16-bit signed integer, little-endian)
    /// * `sample_rate` - Sample rate of the audio in Hz (common values: 16000, 48000)
    ///
    /// # Returns
    /// * `Ok(Vec<u8>)` - Processed audio data with reduced noise
    /// * `Err` - If processing fails or worker threads are unavailable
    ///
    /// # Example
    /// ```ignore
    /// let processed = reduce_noise_async(audio_bytes, 16000).await?;
    /// ```
    #[inline]
    pub async fn reduce_noise_async(
        pcm: Bytes,
        sample_rate: u32,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Asynchronously get a sender to a free worker.
        let sender = get_sender().await?;

        // 2. Create a one-shot channel to receive the result.
        let (response_tx, response_rx) = oneshot::channel();

        // 3. Send the task to the worker. If it fails, the worker thread has likely died.
        let Some(worker_tx) = sender.sender.as_ref() else {
            return Err("Noise-filter worker sender was unavailable after checkout".into());
        };
        worker_tx
            .send((pcm, sample_rate, response_tx))
            .await
            .map_err(|_| "Failed to send task to worker thread.")?;

        // 4. Await the result. The sender is returned to the pool automatically when it's dropped.
        response_rx
            .await
            .map_err(|_| "Worker thread panicked or channel closed while processing.")?
    }

    #[cfg(test)]
    mod worker_pool_tests {
        use super::*;

        #[test]
        fn closed_worker_sender_is_not_returned_to_pool() {
            let (pool_tx, mut pool_rx) = mpsc::channel::<WorkerSender>(1);
            let live_workers = Arc::new(AtomicUsize::new(1));
            let worker_death = Arc::new(Notify::new());
            let (worker_tx, worker_rx) = mpsc::channel::<WorkerTask>(1);
            drop(worker_rx);

            {
                let _checked_out = PooledWorkerSender {
                    sender: Some(worker_tx),
                    pool_tx,
                    live_workers: live_workers.clone(),
                    worker_death: worker_death.clone(),
                };
            }

            assert_eq!(live_workers.load(Ordering::Acquire), 0);
            assert!(pool_rx.try_recv().is_err());
        }

        #[test]
        fn worker_death_count_saturates_at_zero() {
            let live_workers = Arc::new(AtomicUsize::new(0));
            let worker_death = Arc::new(Notify::new());

            mark_worker_dead(&live_workers, &worker_death);

            assert_eq!(live_workers.load(Ordering::Acquire), 0);
        }
    }
}
#[cfg(not(feature = "noise-filter"))]
pub mod stub {
    use bytes::Bytes;

    #[inline]
    pub async fn reduce_noise_async(
        pcm: Bytes,
        _sample_rate: u32,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(pcm.to_vec())
    }
}

#[cfg(feature = "noise-filter")]
pub async fn reduce_noise_async(
    pcm: bytes::Bytes,
    sample_rate: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    implementation::reduce_noise_async(pcm, sample_rate).await
}

#[cfg(not(feature = "noise-filter"))]
pub async fn reduce_noise_async(
    pcm: bytes::Bytes,
    sample_rate: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    stub::reduce_noise_async(pcm, sample_rate).await
}

#[cfg(all(test, feature = "noise-filter"))]
mod tests {
    use super::implementation::*;
    use super::reduce_noise_async;
    use bytes::Bytes;

    fn create_test_pcm(samples: usize) -> Vec<u8> {
        let mut pcm = Vec::with_capacity(samples * 2);
        for i in 0..samples {
            let value = ((i as f32 / samples as f32) * 1_000.0).sin();
            let sample = (value * 16_384.0) as i16;
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        pcm
    }

    #[tokio::test]
    async fn test_silent_audio_passthrough() {
        let silent_pcm = vec![0u8; 320];
        let result = reduce_noise_async(Bytes::from(silent_pcm.clone()), 16_000)
            .await
            .expect("Processing should succeed");
        assert_eq!(result, silent_pcm);
    }

    #[tokio::test]
    async fn test_short_audio_processing() {
        let short_pcm = create_test_pcm(100);
        let result = reduce_noise_async(Bytes::from(short_pcm.clone()), 16_000)
            .await
            .expect("Processing should succeed");
        assert_eq!(result.len(), short_pcm.len());
    }

    #[tokio::test]
    async fn test_normal_audio_processing() {
        let pcm = create_test_pcm(16_000);
        let result = reduce_noise_async(Bytes::from(pcm.clone()), 16_000)
            .await
            .expect("Processing should succeed");
        assert_eq!(result.len(), pcm.len());
    }

    #[tokio::test]
    async fn test_odd_length_pcm() {
        let mut pcm = create_test_pcm(100);
        pcm.push(0);
        let result = reduce_noise_async(Bytes::from(pcm.clone()), 16_000)
            .await
            .expect("Processing should succeed");
        assert_eq!(result, pcm);
    }

    #[tokio::test]
    async fn test_empty_input() {
        let empty_pcm = vec![];
        let result = reduce_noise_async(Bytes::from(empty_pcm.clone()), 16_000)
            .await
            .expect("Processing should succeed");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_parallel_processing() {
        let pcm1 = create_test_pcm(1_000);
        let pcm2 = create_test_pcm(2_000);
        let pcm3 = create_test_pcm(3_000);

        let (res1, res2, res3) = tokio::join!(
            reduce_noise_async(Bytes::from(pcm1.clone()), 16_000),
            reduce_noise_async(Bytes::from(pcm2.clone()), 16_000),
            reduce_noise_async(Bytes::from(pcm3.clone()), 16_000)
        );

        res1.expect("Processing should succeed");
        res2.expect("Processing should succeed");
        res3.expect("Processing should succeed");
    }

    #[tokio::test]
    async fn test_high_snr_audio() {
        let mut pcm = Vec::with_capacity(3_200);
        for i in 0..1_600 {
            let value = (i as f32 / 100.0).sin() * 0.8;
            let sample = (value * 32_767.0) as i16;
            pcm.extend_from_slice(&sample.to_le_bytes());
        }

        let result = reduce_noise_async(Bytes::from(pcm.clone()), 16_000)
            .await
            .expect("Processing should succeed");
        assert_eq!(result.len(), pcm.len());
    }

    #[test]
    fn test_constants_validity() {
        assert!((PCM_TO_FLOAT_SCALE - 1.0 / 32_768.0).abs() < f32::EPSILON);
        assert_eq!(FLOAT_TO_PCM_SCALE_POS, 32_767.0);
        assert_eq!(FLOAT_TO_PCM_SCALE_NEG, 32_768.0);
    }

    #[tokio::test]
    async fn test_livekit_audio_passthrough() {
        let mut pcm = Vec::with_capacity(1_600);
        for i in 0..800 {
            let value = (i as f32 / 100.0).sin() * 0.05;
            let sample = (value * 32_767.0) as i16;
            pcm.extend_from_slice(&sample.to_le_bytes());
        }

        let result = reduce_noise_async(Bytes::from(pcm.clone()), 16_000)
            .await
            .expect("Processing should succeed");

        assert!(!result.is_empty());
        assert_eq!(result.len(), pcm.len());
        let non_zero_count = result.iter().filter(|&&b| b != 0).count();
        assert!(non_zero_count > result.len() / 2);
    }
}

#[cfg(all(test, not(feature = "noise-filter")))]
mod stub_tests {
    use super::reduce_noise_async;
    use bytes::Bytes;

    #[tokio::test]
    async fn stub_returns_input_unmodified() {
        let pcm = vec![1, 2, 3, 4, 5, 6];
        let result = reduce_noise_async(Bytes::from(pcm.clone()), 48_000)
            .await
            .expect("Stub should succeed");
        assert_eq!(result, pcm);
    }
}

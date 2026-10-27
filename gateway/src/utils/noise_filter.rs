#[cfg(feature = "noise-filter")]
pub mod implementation {
    use bytes::Bytes;
    use deep_filter::tract::{DfParams, DfTract, RuntimeParams};
    use ndarray::{Array2, ArrayView2};
    use std::sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tokio::sync::{Mutex, Notify, Semaphore, mpsc, oneshot};
    use tracing::{error, info, warn};

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

    /// The pool of senders to the worker threads, managed by a channel: the core of a
    /// thread-safe, async-friendly pool for the !Send models.
    ///
    /// It starts [`INITIAL_WORKERS`] workers and adds one (a thread holding its own DeepFilterNet
    /// model) whenever a caller finds every worker busy, up to one per core. It used to start
    /// one per core at the first LiveKit track — twenty models on a 20-core host, whatever the
    /// load — with no way to say otherwise.
    struct SenderPool {
        pool_tx: mpsc::Sender<WorkerSender>,
        pool_rx: Arc<Mutex<mpsc::Receiver<WorkerSender>>>,
        live_workers: Arc<AtomicUsize>,
        worker_death: Arc<Notify>,
        /// Workers started (alive or not), for the ceiling.
        started: AtomicUsize,
        max_workers: usize,
    }

    /// Workers started with the pool; the rest come on demand.
    const INITIAL_WORKERS: usize = 2;

    /// Start one worker thread; it adds itself to the pool (and to `live_workers`) once its
    /// model is ready, and reports readiness on `ready` when given.
    fn spawn_worker(
        worker_index: usize,
        pool_tx: mpsc::Sender<WorkerSender>,
        live_workers: Arc<AtomicUsize>,
        ready: Option<std::sync::mpsc::Sender<bool>>,
    ) -> bool {
        let report = move |ok: bool| {
            if let Some(ready) = &ready {
                let _ = ready.send(ok);
            }
        };
        // Spawn a dedicated OS thread for each worker.
        // This is crucial for pinning the !Send DfTract model to a single thread.
        std::thread::Builder::new()
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
                        report(false);
                        return;
                    }
                };

                // Provide this worker's sender to the pool only after the model is ready.
                live_workers.fetch_add(1, Ordering::AcqRel);
                if pool_tx.blocking_send(worker_task_tx).is_err() {
                    live_workers.fetch_sub(1, Ordering::AcqRel);
                    report(false);
                    return;
                }
                report(true);

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
            .inspect_err(|err| {
                error!(
                    worker_index,
                    error = %err,
                    "Failed to spawn noise-filter worker thread"
                );
            })
            .is_ok()
    }

    static SENDER_POOL: LazyLock<SenderPool> = LazyLock::new(|| {
        let max_workers = std::thread::available_parallelism()
            .map_or(INITIAL_WORKERS, |n| n.get())
            .max(INITIAL_WORKERS);
        let (pool_tx, pool_rx) = mpsc::channel(max_workers);
        let live_workers = Arc::new(AtomicUsize::new(0));
        let (init_tx, init_rx) = std::sync::mpsc::channel();
        let launched = (0..INITIAL_WORKERS)
            .filter(|&i| {
                spawn_worker(
                    i,
                    pool_tx.clone(),
                    live_workers.clone(),
                    Some(init_tx.clone()),
                )
            })
            .count();
        drop(init_tx);
        let initialized = init_rx
            .into_iter()
            .take(launched)
            .filter(|ready| *ready)
            .count();
        if initialized == 0 {
            error!("Noise-filter worker pool initialized with no live workers");
        } else if initialized < INITIAL_WORKERS {
            error!(
                initialized,
                requested = INITIAL_WORKERS,
                "Noise-filter worker pool initialized with fewer workers than requested"
            );
        }

        SenderPool {
            pool_tx,
            pool_rx: Arc::new(Mutex::new(pool_rx)),
            live_workers,
            worker_death: Arc::new(Notify::new()),
            started: AtomicUsize::new(INITIAL_WORKERS),
            max_workers,
        }
    });

    /// Workers started so far and the ceiling, for tests.
    #[cfg(test)]
    pub(crate) fn pool_size_for_tests() -> (usize, usize) {
        let pool = &*SENDER_POOL;
        (pool.started.load(Ordering::Acquire), pool.max_workers)
    }

    /// Every worker is busy: start one more if the ceiling allows (it joins the pool when its
    /// model is ready; the caller waits for whichever worker frees first).
    fn grow_pool(pool: &SenderPool) {
        let started = pool.started.fetch_add(1, Ordering::AcqRel);
        if started >= pool.max_workers {
            pool.started.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        info!(
            workers = started + 1,
            max = pool.max_workers,
            "noise-filter pool growing"
        );
        if !spawn_worker(
            started,
            pool.pool_tx.clone(),
            pool.live_workers.clone(),
            None,
        ) {
            pool.started.fetch_sub(1, Ordering::AcqRel);
        }
    }

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
        let pool = &*SENDER_POOL;
        let (pool_tx, pool_rx_mutex, live_workers, worker_death) = (
            &pool.pool_tx,
            &pool.pool_rx,
            &pool.live_workers,
            &pool.worker_death,
        );
        loop {
            if live_workers.load(Ordering::Acquire) == 0 {
                return Err("Noise-filter worker pool has no live workers".into());
            }

            // Lock the mutex asynchronously. The guard is Send and can be held across an await.
            let mut pool_rx = pool_rx_mutex.lock().await;
            // No worker free right now: the pool grows (up to one worker per core) and this
            // caller takes whichever worker frees first.
            if pool_rx.is_empty() {
                grow_pool(pool);
            }
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

    // ── One stream's denoiser: the `/ws` uplink (RCA 2e) ──────────────────────────────────
    //
    // `reduce_noise_async` treats each call as a clip of its own, which suits a recording and
    // not a live stream: a chunk shorter than `SHORT_AUDIO_DURATION` (1 s) never reaches the
    // model (`process_audio_chunk`), the model's state belongs to whichever pool worker took
    // the chunk, and the audio reaches the 48 kHz model at the caller's rate. A `/ws` client
    // sends 10-100 ms chunks, so a stream gets a model instance of its own, whose state follows
    // that stream only; its audio is resampled to the model's rate, cut into the model's hops
    // and resampled back.

    /// The server's cap on streams holding a denoiser at once (each holds one DeepFilterNet
    /// instance on a thread of its own). Default: the number of CPUs; `0` turns the `/ws`
    /// denoiser off for the server.
    pub const WS_NOISE_FILTER_MAX_STREAMS_ENV: &str = "WAAV_WS_NOISE_FILTER_MAX_STREAMS";

    static STREAM_PERMITS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| {
        let max = std::env::var(WS_NOISE_FILTER_MAX_STREAMS_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or_else(num_cpus::get);
        Arc::new(Semaphore::new(max))
    });

    /// How long one chunk may take. A chunk that takes longer turns the denoiser off for the
    /// rest of the stream: the speech-to-text must never fall behind the caller. (Widened in a
    /// debug build, which runs tract unoptimized.)
    const STREAM_CHUNK_DEADLINE: Duration = if cfg!(debug_assertions) {
        Duration::from_secs(5)
    } else {
        Duration::from_millis(250)
    };

    type StreamResult = Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>;
    type StreamJob = (Bytes, oneshot::Sender<StreamResult>);

    /// One audio stream's DeepFilterNet: a model instance on a thread of its own (the model is
    /// `!Send`), fed the stream's chunks in order. Dropping it ends the thread and frees its
    /// place under [`WS_NOISE_FILTER_MAX_STREAMS_ENV`].
    pub struct StreamDenoiser {
        jobs: mpsc::Sender<StreamJob>,
        bypassed: AtomicBool,
        sample_rate: u32,
        /// [`WARMING`] until the stream's model is built, then [`READY`] or [`FAILED`].
        state: Arc<AtomicU8>,
    }

    const WARMING: u8 = 0;
    const READY: u8 = 1;
    const FAILED: u8 = 2;

    impl StreamDenoiser {
        /// A denoiser for a mono PCM16 stream at `sample_rate`. Fails when the server's streams
        /// are all in use; the caller then goes on unfiltered.
        ///
        /// It returns without waiting for the stream's model: building one takes about 0.8 s on a
        /// GB10 core in a release build (seconds in a debug build), and the session's `ready` must
        /// not wait on it. Chunks that arrive while it builds pass through unfiltered (usually the
        /// caller's first second, before they speak); filtering starts with the first chunk after.
        /// A model that fails to build is said once in the log and in
        /// `waav_degraded_total{component="ws"}`, and the stream goes on unfiltered.
        pub async fn open(
            sample_rate: u32,
        ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
            let permit = STREAM_PERMITS
                .clone()
                .try_acquire_owned()
                .map_err(|_| "the server's noise-filter streams are all in use")?;
            let (jobs, mut queued) = mpsc::channel::<StreamJob>(1);
            let state = Arc::new(AtomicU8::new(WARMING));
            let built = state.clone();
            std::thread::Builder::new()
                .name("waav-ws-denoise".to_string())
                .spawn(move || {
                    // The stream holds its place for as long as its thread runs.
                    let _permit = permit;
                    let df_params = &*MODEL_PARAMS;
                    let mut model = match DfTract::new(df_params.clone(), &create_runtime_params())
                    {
                        Ok(model) => model,
                        Err(err) => {
                            error!(error = %err, "/ws noise filter: DeepFilterNet failed to load");
                            built.store(FAILED, Ordering::Release);
                            return;
                        }
                    };
                    let mut stream = StreamState::new(sample_rate, model.sr as u32);
                    built.store(READY, Ordering::Release);
                    while let Some((pcm, result_tx)) = queued.blocking_recv() {
                        let _ = result_tx.send(stream.push(&mut model, &pcm));
                    }
                })?;
            Ok(Self {
                jobs,
                bypassed: AtomicBool::new(false),
                sample_rate,
                state,
            })
        }

        /// Wait up to `limit` for the stream's model to finish building; `true` once it is ready.
        pub async fn wait_ready(&self, limit: Duration) -> bool {
            let deadline = tokio::time::Instant::now() + limit;
            loop {
                match self.state.load(Ordering::Acquire) {
                    READY => return true,
                    FAILED => return false,
                    _ if tokio::time::Instant::now() >= deadline => return false,
                    _ => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
        }

        /// Denoise the stream's next chunk (PCM16 LE mono at the stream's rate).
        ///
        /// `None`: send the chunk as it came. While the stream's model is still building that is
        /// all it means; otherwise the denoiser is off for the rest of the stream, because it
        /// failed or fell behind once (said once, in the log and in
        /// `waav_degraded_total{component="ws"}`). `Some` holds the filtered audio, which may
        /// be shorter or longer than the chunk, or empty: the model works in whole hops and the
        /// resamplers in whole blocks, so up to one block of audio waits for the next chunk.
        pub async fn process(&self, pcm: Bytes) -> Option<Vec<u8>> {
            if self.bypassed.load(Ordering::Acquire) {
                return None;
            }
            let reason = match self.state.load(Ordering::Acquire) {
                // Still building: this chunk goes through as it came, and the denoiser stays on.
                WARMING => return None,
                FAILED => "noise_filter_unavailable",
                _ => {
                    let (result_tx, result_rx) = oneshot::channel();
                    match self.jobs.try_send((pcm, result_tx)) {
                        Ok(()) => {
                            match tokio::time::timeout(STREAM_CHUNK_DEADLINE, result_rx).await {
                                Ok(Ok(Ok(clean))) => return Some(clean),
                                Ok(Ok(Err(err))) => {
                                    error!(error = %err, "noise filter failed on a /ws chunk");
                                    "noise_filter_failed"
                                }
                                Ok(Err(_)) => "noise_filter_gone",
                                Err(_) => "noise_filter_behind",
                            }
                        }
                        Err(_) => "noise_filter_behind",
                    }
                }
            };
            if !self.bypassed.swap(true, Ordering::AcqRel) {
                warn!(
                    reason,
                    sample_rate = self.sample_rate,
                    "/ws noise filter off for the rest of the stream; its audio goes on unfiltered"
                );
                crate::core::metrics::bridge::record_degraded("ws", reason);
            }
            None
        }
    }

    /// A stream's audio on its way to the model's rate and back, and the samples short of a hop.
    struct StreamState {
        rate: u32,
        model_rate: u32,
        to_model: crate::core::audio::StreamResampler,
        from_model: crate::core::audio::StreamResampler,
        pending: Vec<f32>,
    }

    impl StreamState {
        fn new(rate: u32, model_rate: u32) -> Self {
            Self {
                rate,
                model_rate,
                to_model: crate::core::audio::StreamResampler::new(),
                from_model: crate::core::audio::StreamResampler::new(),
                pending: Vec::new(),
            }
        }

        fn push(&mut self, df: &mut DfTract, pcm: &[u8]) -> StreamResult {
            let wav = simd_ops::pcm_to_float_simd(pcm);
            match self.to_model.resample(&wav, self.rate, self.model_rate) {
                Some(at_model) => self.pending.extend_from_slice(&at_model),
                None => self.pending.extend_from_slice(&wav),
            }

            let hop = df.hop_size;
            let mut enhanced = Vec::with_capacity(self.pending.len());
            let mut out = Array2::<f32>::zeros((1, hop));
            let mut idx = 0;
            while idx + hop <= self.pending.len() {
                let frame = ArrayView2::from_shape((1, hop), &self.pending[idx..idx + hop])?;
                df.process(frame, out.view_mut())?;
                enhanced.extend(out.iter().copied());
                idx += hop;
            }
            self.pending.drain(..idx);
            if enhanced.iter().any(|s| !s.is_finite()) {
                return Err("DeepFilterNet produced a non-finite sample".into());
            }

            let back = self
                .from_model
                .resample(&enhanced, self.model_rate, self.rate);
            Ok(simd_ops::float_to_pcm_simd(
                back.as_deref().unwrap_or(&enhanced),
            ))
        }
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
pub use implementation::{StreamDenoiser, WS_NOISE_FILTER_MAX_STREAMS_ENV};

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
    use std::time::Duration;

    fn create_test_pcm(samples: usize) -> Vec<u8> {
        let mut pcm = Vec::with_capacity(samples * 2);
        for i in 0..samples {
            let value = ((i as f32 / samples as f32) * 1_000.0).sin();
            let sample = (value * 16_384.0) as i16;
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        pcm
    }

    /// The pool starts small and grows when callers find every worker busy (it used to start
    /// one DeepFilterNet model per core on the first track, whatever the load).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_pool_grows_under_concurrent_load_and_serves_everyone() {
        let pcm = Bytes::from(create_test_pcm(16_000));
        let calls: Vec<_> = (0..8)
            .map(|_| {
                let pcm = pcm.clone();
                tokio::spawn(async move { reduce_noise_async(pcm, 16_000).await })
            })
            .collect();
        for call in calls {
            let out = call.await.unwrap().expect("every caller is served");
            assert_eq!(out.len(), pcm.len());
        }
        let (started, max) = pool_size_for_tests();
        assert!(started <= max, "{started} > {max}");
        assert!(
            started > 2 || max == 2,
            "8 concurrent callers should grow a pool of 2 (started {started}, max {max})"
        );
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

    /// RCA 2e: a live stream's chunks reach the model. 2 s of noisy 16 kHz speech-band audio in
    /// 20 ms chunks comes out about as long as it went in (less what waits for a whole block),
    /// finite, and quieter. `reduce_noise_async` returns each such chunk unchanged.
    #[tokio::test]
    async fn stream_denoiser_filters_20ms_chunks() {
        let denoiser = StreamDenoiser::open(16_000)
            .await
            .expect("the stream opens");
        // Opening does not wait for the model: a chunk sent at once goes through as it came, and
        // the denoiser stays on for the chunks after the model is built.
        let early = Bytes::from(vec![1u8, 0, 2, 0, 3, 0, 4, 0]);
        assert_eq!(
            denoiser.process(early).await,
            None,
            "passes through while the model builds"
        );
        assert!(
            denoiser.wait_ready(Duration::from_secs(300)).await,
            "the stream's model builds"
        );
        let mut seed = 0x2545_f491_u32;
        let mut noisy = Vec::with_capacity(64_000);
        for i in 0..32_000 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed as f32 / u32::MAX as f32 - 0.5) * 0.6;
            let tone = (i as f32 * 2.0 * std::f32::consts::PI * 300.0 / 16_000.0).sin() * 0.05;
            noisy.extend_from_slice(&(((tone + noise) * 32_767.0) as i16).to_le_bytes());
        }
        let mut clean = Vec::with_capacity(noisy.len());
        for chunk in noisy.chunks(640) {
            let out = denoiser
                .process(Bytes::copy_from_slice(chunk))
                .await
                .expect("the denoiser stays on");
            clean.extend_from_slice(&out);
        }
        // At most one resampler block (20 ms each way) and one hop (10 ms) wait inside.
        assert!(
            clean.len() + 3 * 640 >= noisy.len(),
            "{} of {}",
            clean.len(),
            noisy.len()
        );
        assert!(clean.len() <= noisy.len());
        let rms = |pcm: &[u8]| {
            let samples = pcm
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b) as f64);
            (samples.map(|v| v * v).sum::<f64>() / (pcm.len() / 2).max(1) as f64).sqrt()
        };
        // Broadband noise is most of the input's energy; the model takes most of it away.
        assert!(
            rms(&clean) < 0.7 * rms(&noisy),
            "the model left the noise: rms {} of {}",
            rms(&clean),
            rms(&noisy)
        );

        // RCA 2e: the clip path does not filter a chunk this short.
        let chunk = Bytes::copy_from_slice(&noisy[..640]);
        let passed = reduce_noise_async(chunk.clone(), 16_000).await.expect("ok");
        assert_eq!(passed, chunk.to_vec());
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

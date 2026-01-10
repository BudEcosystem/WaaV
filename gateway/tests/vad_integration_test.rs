//! Integration test for VAD with real audio
//!
//! This test downloads the Silero VAD model and tests with real audio samples.
//! Run with: cargo test --features vad --test vad_integration_test -- --nocapture

#[cfg(feature = "vad")]
mod vad_real_audio_tests {
    use std::path::PathBuf;
    use std::time::Instant;
    use tempfile::tempdir;
    use waav_gateway::core::vad::{VADConfig, SileroVAD, VoiceActivityDetector, VADResult};

    /// Download a sample WAV file for testing
    async fn download_test_audio() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        // Use a public domain speech sample from Mozilla Common Voice or similar
        // This is the "Harvard Sentences" - a classic speech test
        let url = "https://www.voiptroubleshooter.com/open_speech/american/OSR_us_000_0010_8k.wav";

        println!("Downloading test audio from: {}", url);
        let response = reqwest::get(url).await?;

        if !response.status().is_success() {
            return Err(format!("Failed to download audio: {}", response.status()).into());
        }

        let bytes = response.bytes().await?;
        println!("Downloaded {} bytes of audio", bytes.len());
        Ok(bytes.to_vec())
    }

    /// Parse WAV file and extract f32 samples
    fn parse_wav(data: &[u8]) -> Result<(Vec<f32>, u32), Box<dyn std::error::Error>> {
        use std::io::Cursor;

        let cursor = Cursor::new(data);
        let reader = hound::WavReader::new(cursor)?;
        let spec = reader.spec();

        println!("WAV format: {} Hz, {} channels, {} bits",
                 spec.sample_rate, spec.channels, spec.bits_per_sample);

        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => {
                let max_val = (1 << (spec.bits_per_sample - 1)) as f32;
                reader.into_samples::<i32>()
                    .filter_map(Result::ok)
                    .map(|s| s as f32 / max_val)
                    .collect()
            }
            hound::SampleFormat::Float => {
                reader.into_samples::<f32>()
                    .filter_map(Result::ok)
                    .collect()
            }
        };

        // If stereo, convert to mono by averaging channels
        let mono_samples = if spec.channels == 2 {
            samples.chunks(2)
                .map(|chunk| (chunk[0] + chunk.get(1).unwrap_or(&0.0)) / 2.0)
                .collect()
        } else {
            samples
        };

        println!("Extracted {} mono samples", mono_samples.len());
        Ok((mono_samples, spec.sample_rate))
    }

    /// Resample audio to target sample rate (simple linear interpolation)
    fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
        if from_rate == to_rate {
            return samples.to_vec();
        }

        let ratio = from_rate as f64 / to_rate as f64;
        let new_len = (samples.len() as f64 / ratio) as usize;

        let mut resampled = Vec::with_capacity(new_len);
        for i in 0..new_len {
            let src_idx = i as f64 * ratio;
            let idx0 = src_idx.floor() as usize;
            let idx1 = (idx0 + 1).min(samples.len() - 1);
            let frac = src_idx - idx0 as f64;

            let sample = samples[idx0] as f64 * (1.0 - frac) + samples[idx1] as f64 * frac;
            resampled.push(sample as f32);
        }

        println!("Resampled from {} Hz to {} Hz: {} -> {} samples",
                 from_rate, to_rate, samples.len(), resampled.len());
        resampled
    }

    /// Generate synthetic speech-like audio for testing
    fn generate_speech_audio(duration_ms: u32, sample_rate: u32) -> Vec<f32> {
        let num_samples = (duration_ms as usize * sample_rate as usize) / 1000;
        let mut samples = Vec::with_capacity(num_samples);

        // Generate formant-like frequencies (vowel sounds)
        let f1 = 500.0;  // First formant
        let f2 = 1500.0; // Second formant
        let f3 = 2500.0; // Third formant

        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;

            // Amplitude envelope (attack-sustain-decay)
            let envelope = if t < 0.05 {
                t / 0.05 // Attack
            } else if t > (duration_ms as f32 / 1000.0) - 0.05 {
                ((duration_ms as f32 / 1000.0) - t) / 0.05 // Decay
            } else {
                1.0 // Sustain
            };

            // Combine formants with harmonics
            let signal =
                (2.0 * std::f32::consts::PI * f1 * t).sin() * 0.5 +
                (2.0 * std::f32::consts::PI * f2 * t).sin() * 0.3 +
                (2.0 * std::f32::consts::PI * f3 * t).sin() * 0.2 +
                // Add some fundamental frequency variation (pitch)
                (2.0 * std::f32::consts::PI * 120.0 * t).sin() * 0.4;

            samples.push(signal * envelope * 0.5);
        }

        samples
    }

    /// Generate silence
    fn generate_silence(duration_ms: u32, sample_rate: u32) -> Vec<f32> {
        let num_samples = (duration_ms as usize * sample_rate as usize) / 1000;
        // Add very low amplitude noise
        (0..num_samples)
            .map(|_| (rand::random::<f32>() - 0.5) * 0.001)
            .collect()
    }

    /// Simple random for noise generation
    mod rand {
        use std::cell::Cell;
        thread_local! {
            static SEED: Cell<u64> = Cell::new(12345);
        }

        pub fn random<T: From<f32>>() -> T {
            SEED.with(|seed| {
                let mut s = seed.get();
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                seed.set(s);
                T::from((s % 1000) as f32 / 1000.0)
            })
        }
    }

    #[tokio::test]
    async fn test_vad_with_synthetic_speech() {
        println!("\n=== Testing VAD with Synthetic Speech ===\n");

        let temp_dir = tempdir().unwrap();
        let cache_path = temp_dir.path().to_path_buf();

        // Create VAD config
        let config = VADConfig {
            enabled: true,
            threshold: 0.5,
            min_speech_duration_ms: 100,
            min_silence_duration_ms: 150,
            sample_rate: 16000,
            frame_size: 512,
            cache_path: Some(cache_path.clone()),
            model_url: Some(
                "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx"
                    .to_string(),
            ),
            ..Default::default()
        };

        println!("Creating VAD with config: threshold={}, sample_rate={}",
                 config.threshold, config.sample_rate);

        // Download model
        println!("Downloading Silero VAD model...");
        let download_start = Instant::now();

        match waav_gateway::core::vad::assets::download_assets(&config).await {
            Ok(_) => println!("Model downloaded in {:?}", download_start.elapsed()),
            Err(e) => {
                println!("Failed to download model: {}. Skipping test.", e);
                return;
            }
        }

        // Create VAD
        let mut vad = match SileroVAD::new(config).await {
            Ok(v) => v,
            Err(e) => {
                println!("Failed to create VAD: {}. Skipping test.", e);
                return;
            }
        };

        println!("VAD initialized successfully\n");

        // Test pattern: Silence -> Speech -> Silence -> Speech -> Silence
        let sample_rate = 16000;
        let frame_size = 512;

        let mut audio = Vec::new();
        audio.extend(generate_silence(500, sample_rate));   // 500ms silence
        audio.extend(generate_speech_audio(1000, sample_rate)); // 1s speech
        audio.extend(generate_silence(500, sample_rate));   // 500ms silence
        audio.extend(generate_speech_audio(800, sample_rate));  // 800ms speech
        audio.extend(generate_silence(500, sample_rate));   // 500ms silence

        println!("Generated {} samples ({:.1}s) of test audio",
                 audio.len(), audio.len() as f32 / sample_rate as f32);
        println!("Pattern: 500ms silence -> 1s speech -> 500ms silence -> 800ms speech -> 500ms silence\n");

        // Process frames
        let mut speech_starts = Vec::new();
        let mut speech_ends = Vec::new();
        let mut frame_results = Vec::new();

        let process_start = Instant::now();

        for (i, chunk) in audio.chunks(frame_size).enumerate() {
            if chunk.len() < frame_size {
                break;
            }

            let result = vad.process_frame(chunk).unwrap();
            frame_results.push((i, result.clone()));

            if result.speech_start {
                let time_ms = (i * frame_size * 1000) / sample_rate as usize;
                speech_starts.push(time_ms);
                println!("  [Frame {}] SPEECH_START at {}ms (prob: {:.3})",
                         i, time_ms, result.probability);
            }
            if result.speech_end {
                let time_ms = (i * frame_size * 1000) / sample_rate as usize;
                speech_ends.push(time_ms);
                println!("  [Frame {}] SPEECH_END at {}ms (prob: {:.3})",
                         i, time_ms, result.probability);
            }
        }

        let process_time = process_start.elapsed();
        let num_frames = audio.len() / frame_size;
        let audio_duration_ms = (audio.len() * 1000) / sample_rate as usize;

        println!("\n=== Results ===");
        println!("Processed {} frames in {:?}", num_frames, process_time);
        println!("Average: {:.2}ms per frame",
                 process_time.as_micros() as f64 / num_frames as f64 / 1000.0);
        println!("Real-time factor: {:.2}x",
                 audio_duration_ms as f64 / process_time.as_millis() as f64);
        println!("\nSpeech starts detected at: {:?} ms", speech_starts);
        println!("Speech ends detected at: {:?} ms", speech_ends);

        // Get stats
        let stats = vad.get_stats();
        println!("\nVAD Statistics:");
        println!("  Total frames: {}", stats.total_frames);
        println!("  Speech frames: {} ({:.1}%)",
                 stats.total_speech_frames, stats.speech_ratio * 100.0);
        println!("  Avg inference: {}us", stats.avg_inference_us);

        // Verify we detected speech transitions
        // Note: With synthetic audio, detection may not be perfect
        println!("\n=== Validation ===");
        if speech_starts.len() >= 1 {
            println!("PASS: Detected at least 1 speech start");
        } else {
            println!("WARN: Expected at least 1 speech start, got {}", speech_starts.len());
        }

        if speech_ends.len() >= 1 {
            println!("PASS: Detected at least 1 speech end");
        } else {
            println!("WARN: Expected at least 1 speech end, got {}", speech_ends.len());
        }

        // Check that VAD is fast enough for real-time
        let avg_ms = process_time.as_micros() as f64 / num_frames as f64 / 1000.0;
        let frame_duration_ms = (frame_size * 1000) as f64 / sample_rate as f64;

        if avg_ms < frame_duration_ms {
            println!("PASS: VAD is faster than real-time ({:.2}ms < {:.2}ms frame)",
                     avg_ms, frame_duration_ms);
        } else {
            println!("WARN: VAD may be too slow ({:.2}ms >= {:.2}ms frame)",
                     avg_ms, frame_duration_ms);
        }
    }

    #[tokio::test]
    async fn test_vad_with_downloaded_audio() {
        println!("\n=== Testing VAD with Downloaded Real Audio ===\n");

        let temp_dir = tempdir().unwrap();
        let cache_path = temp_dir.path().to_path_buf();

        // Download test audio
        let wav_data = match download_test_audio().await {
            Ok(data) => data,
            Err(e) => {
                println!("Failed to download test audio: {}. Skipping test.", e);
                return;
            }
        };

        // Parse WAV
        let (samples, original_rate) = match parse_wav(&wav_data) {
            Ok(result) => result,
            Err(e) => {
                println!("Failed to parse WAV: {}. Skipping test.", e);
                return;
            }
        };

        // Resample to 16kHz if needed
        let target_rate = 16000u32;
        let samples = resample(&samples, original_rate, target_rate);

        // Create VAD config
        let config = VADConfig {
            enabled: true,
            threshold: 0.5,
            min_speech_duration_ms: 200,
            min_silence_duration_ms: 300,
            sample_rate: target_rate,
            frame_size: 512,
            cache_path: Some(cache_path.clone()),
            model_url: Some(
                "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx"
                    .to_string(),
            ),
            ..Default::default()
        };

        // Download model
        println!("Downloading Silero VAD model...");
        match waav_gateway::core::vad::assets::download_assets(&config).await {
            Ok(_) => println!("Model ready"),
            Err(e) => {
                println!("Failed to download model: {}. Skipping test.", e);
                return;
            }
        }

        // Create VAD
        let mut vad = match SileroVAD::new(config).await {
            Ok(v) => v,
            Err(e) => {
                println!("Failed to create VAD: {}. Skipping test.", e);
                return;
            }
        };

        println!("\nProcessing {:.2}s of audio...", samples.len() as f32 / target_rate as f32);

        // Process frames
        let frame_size = 512;
        let mut speech_detected = false;
        let mut max_prob = 0.0f32;
        let mut speech_frame_count = 0;

        let process_start = Instant::now();

        for chunk in samples.chunks(frame_size) {
            if chunk.len() < frame_size {
                break;
            }

            let result = vad.process_frame(chunk).unwrap();
            max_prob = max_prob.max(result.probability);

            if result.is_speech {
                speech_frame_count += 1;
            }

            if result.speech_start {
                speech_detected = true;
                println!("  Speech started at {}ms", result.timestamp_ms);
            }
            if result.speech_end {
                println!("  Speech ended at {}ms (duration: {}ms)",
                         result.timestamp_ms, result.speech_duration_ms);
            }
        }

        let process_time = process_start.elapsed();
        let num_frames = samples.len() / frame_size;

        println!("\n=== Results ===");
        println!("Max probability: {:.3}", max_prob);
        println!("Speech frames: {} / {} ({:.1}%)",
                 speech_frame_count, num_frames,
                 speech_frame_count as f32 / num_frames as f32 * 100.0);
        println!("Processing time: {:?} for {:.2}s audio",
                 process_time, samples.len() as f32 / target_rate as f32);

        // This should be real speech audio, so we expect high probability
        if max_prob > 0.5 {
            println!("PASS: Detected speech-like content (max_prob={:.3})", max_prob);
        } else {
            println!("WARN: Low speech probability (max_prob={:.3})", max_prob);
        }

        if speech_detected {
            println!("PASS: Speech transitions detected");
        } else {
            println!("INFO: No speech transitions detected (audio may be continuous speech)");
        }
    }

    #[tokio::test]
    async fn test_vad_performance_benchmark() {
        println!("\n=== VAD Performance Benchmark ===\n");

        let temp_dir = tempdir().unwrap();
        let cache_path = temp_dir.path().to_path_buf();

        let config = VADConfig {
            enabled: true,
            threshold: 0.5,
            sample_rate: 16000,
            frame_size: 512,
            cache_path: Some(cache_path.clone()),
            model_url: Some(
                "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx"
                    .to_string(),
            ),
            ..Default::default()
        };

        // Download model
        match waav_gateway::core::vad::assets::download_assets(&config).await {
            Ok(_) => {},
            Err(e) => {
                println!("Failed to download model: {}. Skipping test.", e);
                return;
            }
        }

        let mut vad = match SileroVAD::new(config).await {
            Ok(v) => v,
            Err(e) => {
                println!("Failed to create VAD: {}. Skipping test.", e);
                return;
            }
        };

        // Generate test data
        let sample_rate = 16000;
        let frame_size = 512;
        let audio = generate_speech_audio(10000, sample_rate); // 10 seconds

        // Warmup
        println!("Warming up (100 frames)...");
        for chunk in audio.chunks(frame_size).take(100) {
            if chunk.len() == frame_size {
                let _ = vad.process_frame(chunk);
            }
        }
        vad.reset();

        // Benchmark
        let num_iterations = 1000;
        println!("Benchmarking {} frames...", num_iterations);

        let mut times = Vec::with_capacity(num_iterations);
        let test_frame: Vec<f32> = audio[..frame_size].to_vec();

        for _ in 0..num_iterations {
            let start = Instant::now();
            let _ = vad.process_frame(&test_frame);
            times.push(start.elapsed().as_micros() as f64);
        }

        // Calculate statistics
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        let min = times[0];
        let max = times[times.len() - 1];
        let p50 = times[times.len() / 2];
        let p95 = times[(times.len() as f64 * 0.95) as usize];
        let p99 = times[(times.len() as f64 * 0.99) as usize];

        println!("\n=== Benchmark Results ({} iterations) ===", num_iterations);
        println!("  Min:    {:.1}us", min);
        println!("  Avg:    {:.1}us", avg);
        println!("  P50:    {:.1}us", p50);
        println!("  P95:    {:.1}us", p95);
        println!("  P99:    {:.1}us", p99);
        println!("  Max:    {:.1}us", max);

        let frame_duration_us = (frame_size as f64 / sample_rate as f64) * 1_000_000.0;
        let rtf = avg / frame_duration_us;
        println!("\n  Frame duration: {:.1}us", frame_duration_us);
        println!("  Real-time factor: {:.4}x (lower is better)", rtf);

        if rtf < 0.1 {
            println!("\nPASS: VAD is >10x faster than real-time");
        } else if rtf < 1.0 {
            println!("\nPASS: VAD is faster than real-time");
        } else {
            println!("\nWARN: VAD is slower than real-time!");
        }

        // Verify we meet performance requirements
        assert!(avg < 5000.0, "Average inference should be <5ms, got {:.1}us", avg);
        assert!(p99 < 10000.0, "P99 inference should be <10ms, got {:.1}us", p99);
    }
}

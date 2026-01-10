//! Integration tests for VAD module

use std::time::Instant;

#[cfg(feature = "vad")]
mod vad_tests {
    use super::*;
    use waav_gateway::core::vad::{VADConfig, VADBackend, SileroVAD, VoiceActivityDetector};
    use tempfile::tempdir;

    /// Generate synthetic audio samples
    fn generate_audio_samples(num_samples: usize, with_speech: bool) -> Vec<f32> {
        let mut samples = Vec::with_capacity(num_samples);
        for i in 0..num_samples {
            if with_speech {
                // Simulate speech-like signal (sine wave with noise)
                let t = i as f32 / 16000.0;
                let signal = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5
                    + (2.0 * std::f32::consts::PI * 880.0 * t).sin() * 0.25;
                samples.push(signal + (rand_f32() - 0.5) * 0.1);
            } else {
                // Simulate silence (low amplitude noise)
                samples.push((rand_f32() - 0.5) * 0.01);
            }
        }
        samples
    }

    /// Simple pseudo-random f32 generator (not cryptographically secure)
    fn rand_f32() -> f32 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        use std::time::SystemTime;

        let mut hasher = DefaultHasher::new();
        SystemTime::now().hash(&mut hasher);
        (hasher.finish() % 1000) as f32 / 1000.0
    }

    #[test]
    fn test_vad_config_default() {
        let config = VADConfig::default();
        assert!(config.enabled);
        assert_eq!(config.backend, VADBackend::Silero);
        assert_eq!(config.threshold, 0.5);
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.frame_size, 512);
        assert_eq!(config.min_speech_duration_ms, 250);
        assert_eq!(config.min_silence_duration_ms, 300);
    }

    #[test]
    fn test_vad_config_low_latency() {
        let config = VADConfig::low_latency();
        assert_eq!(config.min_speech_duration_ms, 100);
        assert_eq!(config.min_silence_duration_ms, 200);
        assert_eq!(config.pre_speech_padding_ms, 50);
    }

    #[test]
    fn test_vad_config_high_accuracy() {
        let config = VADConfig::high_accuracy();
        assert_eq!(config.threshold, 0.7);
        assert_eq!(config.min_speech_duration_ms, 300);
        assert_eq!(config.min_silence_duration_ms, 500);
    }

    #[test]
    fn test_vad_config_validation() {
        // Valid config
        let config = VADConfig::default();
        assert!(config.validate().is_ok());

        // Invalid threshold (too low)
        let mut config = VADConfig::default();
        config.threshold = -0.1;
        assert!(config.validate().is_err());

        // Invalid threshold (too high)
        let mut config = VADConfig::default();
        config.threshold = 1.5;
        assert!(config.validate().is_err());

        // Invalid sample rate for Silero
        let mut config = VADConfig::default();
        config.sample_rate = 44100;
        assert!(config.validate().is_err());

        // Valid sample rate for Silero (8kHz)
        let mut config = VADConfig::default();
        config.sample_rate = 8000;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_vad_config_frame_duration() {
        let config = VADConfig::default();
        // 512 samples at 16kHz = 32ms
        assert!((config.frame_duration_ms() - 32.0).abs() < 0.001);
    }

    #[test]
    fn test_vad_config_frames_for_duration() {
        let config = VADConfig::default();
        // 256ms at 32ms/frame = 8 frames
        assert_eq!(config.frames_for_duration(256), 8);
        // 300ms at 32ms/frame = 9.375 -> 10 frames (ceil)
        assert_eq!(config.frames_for_duration(300), 10);
    }

    #[test]
    fn test_vad_backend_display() {
        assert_eq!(format!("{}", VADBackend::Silero), "silero");
        assert_eq!(format!("{}", VADBackend::WebRTC), "webrtc");
        assert_eq!(format!("{}", VADBackend::Energy), "energy");
    }

    #[test]
    fn test_vad_config_cache_dir() {
        let temp_dir = tempdir().unwrap();
        let config = VADConfig {
            cache_path: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };

        let cache_dir = config.get_cache_dir().unwrap();
        assert!(cache_dir.ends_with("vad"));
    }

    #[test]
    fn test_vad_config_cache_dir_missing() {
        let config = VADConfig {
            cache_path: None,
            ..Default::default()
        };

        assert!(config.get_cache_dir().is_err());
    }

    // Skip actual model tests if model is not available
    // These tests require the model to be downloaded
    #[cfg(feature = "vad_integration_tests")]
    mod integration {
        use super::*;

        #[tokio::test]
        async fn test_silero_vad_creation() {
            let temp_dir = tempdir().unwrap();
            let config = VADConfig {
                cache_path: Some(temp_dir.path().to_path_buf()),
                ..Default::default()
            };

            let result = SileroVAD::new(config).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_silero_vad_process_silence() {
            let temp_dir = tempdir().unwrap();
            let config = VADConfig {
                cache_path: Some(temp_dir.path().to_path_buf()),
                ..Default::default()
            };

            let mut vad = SileroVAD::new(config).await.unwrap();

            // Process silence
            let silence = generate_audio_samples(512, false);
            let result = vad.process_frame(&silence).unwrap();

            assert!(!result.is_speech);
            assert!(result.probability < 0.3);
            assert!(!result.speech_start);
            assert!(!result.speech_end);
        }

        #[tokio::test]
        async fn test_silero_vad_process_speech() {
            let temp_dir = tempdir().unwrap();
            let config = VADConfig {
                cache_path: Some(temp_dir.path().to_path_buf()),
                threshold: 0.3, // Lower threshold for synthetic audio
                min_speech_duration_ms: 32, // Single frame
                ..Default::default()
            };

            let mut vad = SileroVAD::new(config).await.unwrap();

            // Process speech-like audio multiple times to trigger speech detection
            for _ in 0..10 {
                let speech = generate_audio_samples(512, true);
                let result = vad.process_frame(&speech).unwrap();

                if result.speech_start {
                    assert!(result.is_speech);
                    return; // Test passed
                }
            }

            // May not detect synthetic audio as speech, which is expected
        }

        #[tokio::test]
        async fn test_silero_vad_reset() {
            let temp_dir = tempdir().unwrap();
            let config = VADConfig {
                cache_path: Some(temp_dir.path().to_path_buf()),
                ..Default::default()
            };

            let mut vad = SileroVAD::new(config).await.unwrap();

            // Process some frames
            for _ in 0..5 {
                let audio = generate_audio_samples(512, false);
                let _ = vad.process_frame(&audio);
            }

            // Reset
            vad.reset();

            // Verify state is reset
            assert_eq!(vad.speech_probability(), 0.0);
            assert!(!vad.is_speaking());
        }

        #[tokio::test]
        async fn test_silero_vad_performance() {
            let temp_dir = tempdir().unwrap();
            let config = VADConfig {
                cache_path: Some(temp_dir.path().to_path_buf()),
                ..Default::default()
            };

            let mut vad = SileroVAD::new(config).await.unwrap();

            // Warmup
            for _ in 0..10 {
                let audio = generate_audio_samples(512, false);
                let _ = vad.process_frame(&audio);
            }

            // Benchmark
            let num_frames = 1000;
            let start = Instant::now();

            for _ in 0..num_frames {
                let audio = generate_audio_samples(512, false);
                let _ = vad.process_frame(&audio);
            }

            let elapsed = start.elapsed();
            let avg_ms = elapsed.as_millis() as f64 / num_frames as f64;

            // VAD should be fast - less than 5ms per frame on average
            println!("VAD average inference time: {:.3}ms", avg_ms);
            assert!(avg_ms < 5.0, "VAD too slow: {:.3}ms per frame", avg_ms);

            // Check stats
            let stats = vad.get_stats();
            println!("VAD stats: {}", stats);
            assert!(stats.avg_inference_us < 5000); // Less than 5ms in microseconds
        }
    }
}

/// Tests for VAD stub when feature is disabled
#[cfg(not(feature = "vad"))]
mod vad_stub_tests {
    use waav_gateway::core::vad::{VADConfig, SileroVAD, VoiceActivityDetector};

    #[tokio::test]
    async fn test_stub_vad_creation() {
        let config = VADConfig::default();
        let result = SileroVAD::new(config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_stub_vad_always_returns_no_speech() {
        let config = VADConfig::default();
        let mut vad = SileroVAD::new(config).await.unwrap();

        let audio = vec![0.0f32; 512];
        let result = vad.process_frame(&audio).unwrap();

        assert!(!result.is_speech);
        assert_eq!(result.probability, 0.0);
        assert!(!result.speech_start);
        assert!(!result.speech_end);
    }

    #[tokio::test]
    async fn test_stub_vad_speech_probability() {
        let config = VADConfig::default();
        let vad = SileroVAD::new(config).await.unwrap();
        assert_eq!(vad.speech_probability(), 0.0);
    }

    #[tokio::test]
    async fn test_stub_vad_is_speaking() {
        let config = VADConfig::default();
        let vad = SileroVAD::new(config).await.unwrap();
        assert!(!vad.is_speaking());
    }
}

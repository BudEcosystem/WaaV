//! Performance benchmarks for WaaV Gateway
//!
//! Run with: cargo bench
//! Or for specific benchmarks: cargo bench -- <filter>

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::Duration;
use waav_gateway::core::cache::{CacheBackend, MemoryCacheBackend, XxHasher, KeyHasher};
use waav_gateway::handlers::ws::messages::{IncomingMessage, OutgoingMessage, MAX_SPEAK_TEXT_SIZE};
use waav_gateway::utils::phone_validation::validate_phone_number;

/// Benchmark message parsing performance
fn bench_message_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_parsing");
    group.measurement_time(Duration::from_secs(5));

    // Small config message
    let small_config = r#"{"type":"config","stream_id":"test-123","audio":true}"#;

    // Medium speak message
    let medium_speak = format!(
        r#"{{"type":"speak","text":"{}","flush":true}}"#,
        "Hello, this is a test message for TTS synthesis. ".repeat(10)
    );

    // Large speak message (approaching limit)
    let large_speak = format!(
        r#"{{"type":"speak","text":"{}","flush":true,"allow_interruption":false}}"#,
        "a".repeat(50_000)
    );

    // SendMessage with JSON debug data
    let send_message = r#"{"type":"send_message","message":"Hello world","role":"user","topic":"chat","debug":{"timestamp":1234567890,"metadata":{"key":"value"}}}"#;

    // SIP transfer message
    let sip_transfer = r#"{"type":"sip_transfer","transfer_to":"+1234567890"}"#;

    group.throughput(Throughput::Bytes(small_config.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("small_config", small_config.len()),
        &small_config,
        |b, msg| {
            b.iter(|| {
                let _: Result<IncomingMessage, _> = serde_json::from_str(black_box(msg));
            });
        },
    );

    group.throughput(Throughput::Bytes(medium_speak.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("medium_speak", medium_speak.len()),
        &medium_speak,
        |b, msg| {
            b.iter(|| {
                let _: Result<IncomingMessage, _> = serde_json::from_str(black_box(msg));
            });
        },
    );

    group.throughput(Throughput::Bytes(large_speak.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("large_speak", large_speak.len()),
        &large_speak,
        |b, msg| {
            b.iter(|| {
                let _: Result<IncomingMessage, _> = serde_json::from_str(black_box(msg));
            });
        },
    );

    group.throughput(Throughput::Bytes(send_message.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("send_message", send_message.len()),
        &send_message,
        |b, msg| {
            b.iter(|| {
                let _: Result<IncomingMessage, _> = serde_json::from_str(black_box(msg));
            });
        },
    );

    group.throughput(Throughput::Bytes(sip_transfer.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("sip_transfer", sip_transfer.len()),
        &sip_transfer,
        |b, msg| {
            b.iter(|| {
                let _: Result<IncomingMessage, _> = serde_json::from_str(black_box(msg));
            });
        },
    );

    group.finish();
}

/// Benchmark message validation performance
fn bench_message_validation(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_validation");
    group.measurement_time(Duration::from_secs(5));

    // Valid speak message
    let valid_speak = IncomingMessage::Speak {
        text: "Hello, world!".to_string(),
        flush: Some(true),
        allow_interruption: Some(false),
    };

    // Large but valid speak message
    let large_speak = IncomingMessage::Speak {
        text: "a".repeat(MAX_SPEAK_TEXT_SIZE - 100),
        flush: Some(true),
        allow_interruption: Some(true),
    };

    // Valid send message
    let valid_send = IncomingMessage::SendMessage {
        message: "Test message".to_string(),
        role: "user".to_string(),
        topic: Some("chat".to_string()),
        debug: None,
    };

    // Valid SIP transfer
    let valid_sip = IncomingMessage::SIPTransfer {
        transfer_to: "+1234567890".to_string(),
    };

    group.bench_function("valid_speak_small", |b| {
        b.iter(|| {
            black_box(&valid_speak).validate_size()
        });
    });

    group.bench_function("valid_speak_large", |b| {
        b.iter(|| {
            black_box(&large_speak).validate_size()
        });
    });

    group.bench_function("valid_send_message", |b| {
        b.iter(|| {
            black_box(&valid_send).validate_size()
        });
    });

    group.bench_function("valid_sip_transfer", |b| {
        b.iter(|| {
            black_box(&valid_sip).validate_size()
        });
    });

    group.finish();
}

/// Benchmark outgoing message serialization
fn bench_message_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_serialization");
    group.measurement_time(Duration::from_secs(5));

    // Ready message
    let ready_msg = OutgoingMessage::Ready {
        stream_id: "test-stream-123".to_string(),
        livekit_room_name: Some("test-room".to_string()),
        livekit_url: Some("ws://localhost:7880".to_string()),
        waav_participant_identity: Some("waav-ai".to_string()),
        waav_participant_name: Some("WaaV AI".to_string()),
    };

    // STT result
    let stt_result = OutgoingMessage::STTResult {
        transcript: "This is the transcribed text from the audio input".to_string(),
        is_final: true,
        is_speech_final: true,
        confidence: 0.95,
    };

    // Error message
    let error_msg = OutgoingMessage::Error {
        message: "Connection failed: timeout after 30 seconds".to_string(),
    };

    group.bench_function("ready_message", |b| {
        b.iter(|| {
            serde_json::to_string(black_box(&ready_msg))
        });
    });

    group.bench_function("stt_result", |b| {
        b.iter(|| {
            serde_json::to_string(black_box(&stt_result))
        });
    });

    group.bench_function("error_message", |b| {
        b.iter(|| {
            serde_json::to_string(black_box(&error_msg))
        });
    });

    group.finish();
}

/// Benchmark phone number validation
fn bench_phone_validation(c: &mut Criterion) {
    let mut group = c.benchmark_group("phone_validation");
    group.measurement_time(Duration::from_secs(5));

    // Various phone number formats
    let international = "+1234567890";
    let international_long = "+14155551234";
    let national = "4155551234";
    let with_spaces = "+1 415 555 1234";
    let with_dashes = "+1-415-555-1234";
    let extension = "1234";
    let invalid = "abc123";
    let empty = "";

    group.bench_function("international_simple", |b| {
        b.iter(|| {
            validate_phone_number(black_box(international))
        });
    });

    group.bench_function("international_long", |b| {
        b.iter(|| {
            validate_phone_number(black_box(international_long))
        });
    });

    group.bench_function("national", |b| {
        b.iter(|| {
            validate_phone_number(black_box(national))
        });
    });

    group.bench_function("with_spaces", |b| {
        b.iter(|| {
            validate_phone_number(black_box(with_spaces))
        });
    });

    group.bench_function("with_dashes", |b| {
        b.iter(|| {
            validate_phone_number(black_box(with_dashes))
        });
    });

    group.bench_function("extension", |b| {
        b.iter(|| {
            validate_phone_number(black_box(extension))
        });
    });

    group.bench_function("invalid", |b| {
        b.iter(|| {
            validate_phone_number(black_box(invalid))
        });
    });

    group.bench_function("empty", |b| {
        b.iter(|| {
            validate_phone_number(black_box(empty))
        });
    });

    group.finish();
}

/// Benchmark cache operations
fn bench_cache_operations(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("cache_operations");
    group.measurement_time(Duration::from_secs(5));

    // Create cache with memory backend
    let cache = MemoryCacheBackend::new(
        100_000,            // max entries
        Some(100_000_000),  // max size 100MB
        Some(Duration::from_secs(3600)), // 1 hour TTL
    );

    // Prepare test data
    let small_data = Bytes::from(vec![0u8; 100]);
    let medium_data = Bytes::from(vec![0u8; 10_000]);
    let large_data = Bytes::from(vec![0u8; 100_000]);

    // Insert benchmarks
    group.throughput(Throughput::Bytes(100));
    group.bench_function("insert_small_100b", |b| {
        let small = small_data.clone();
        b.to_async(&rt).iter(|| async {
            let _ = cache.set("key-small", black_box(small.clone()), None).await;
        });
    });

    group.throughput(Throughput::Bytes(10_000));
    group.bench_function("insert_medium_10kb", |b| {
        let medium = medium_data.clone();
        b.to_async(&rt).iter(|| async {
            let _ = cache.set("key-medium", black_box(medium.clone()), None).await;
        });
    });

    group.throughput(Throughput::Bytes(100_000));
    group.bench_function("insert_large_100kb", |b| {
        let large = large_data.clone();
        b.to_async(&rt).iter(|| async {
            let _ = cache.set("key-large", black_box(large.clone()), None).await;
        });
    });

    // Pre-populate cache for get benchmarks
    rt.block_on(async {
        let _ = cache.set("get-key-small", small_data.clone(), None).await;
        let _ = cache.set("get-key-medium", medium_data.clone(), None).await;
        let _ = cache.set("get-key-large", large_data.clone(), None).await;
    });

    // Get benchmarks
    group.bench_function("get_small_100b", |b| {
        b.to_async(&rt).iter(|| async {
            cache.get(black_box("get-key-small")).await
        });
    });

    group.bench_function("get_medium_10kb", |b| {
        b.to_async(&rt).iter(|| async {
            cache.get(black_box("get-key-medium")).await
        });
    });

    group.bench_function("get_large_100kb", |b| {
        b.to_async(&rt).iter(|| async {
            cache.get(black_box("get-key-large")).await
        });
    });

    // Miss benchmark
    group.bench_function("get_miss", |b| {
        b.to_async(&rt).iter(|| async {
            cache.get(black_box("nonexistent-key")).await
        });
    });

    group.finish();
}

/// Benchmark key hashing for cache
fn bench_cache_hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_hashing");
    group.measurement_time(Duration::from_secs(5));

    let hasher = XxHasher::new("cache");

    let short_key = "key";
    let medium_key = "provider:deepgram:voice:aura-asteria-en:format:pcm16:rate:24000";
    let long_key = "x".repeat(1000);

    group.bench_function("hash_short_key", |b| {
        b.iter(|| {
            hasher.hash(black_box(short_key))
        });
    });

    group.bench_function("hash_medium_key", |b| {
        b.iter(|| {
            hasher.hash(black_box(medium_key))
        });
    });

    group.bench_function("hash_long_key", |b| {
        b.iter(|| {
            hasher.hash(black_box(&long_key))
        });
    });

    group.finish();
}

/// Benchmark audio frame size validation
fn bench_audio_frame_validation(c: &mut Criterion) {
    use waav_gateway::handlers::ws::audio_handler::MAX_AUDIO_FRAME_SIZE;

    let mut group = c.benchmark_group("audio_frame_validation");
    group.measurement_time(Duration::from_secs(5));

    // Various audio frame sizes
    let small_frame = vec![0u8; 320]; // 10ms at 16kHz mono 16-bit
    let medium_frame = vec![0u8; 3200]; // 100ms at 16kHz mono 16-bit
    let large_frame = vec![0u8; 32000]; // 1s at 16kHz mono 16-bit
    let huge_frame = vec![0u8; 960000]; // 30s at 16kHz mono 16-bit

    group.bench_function("validate_small_320b", |b| {
        b.iter(|| {
            let len = black_box(&small_frame).len();
            len <= MAX_AUDIO_FRAME_SIZE
        });
    });

    group.bench_function("validate_medium_3kb", |b| {
        b.iter(|| {
            let len = black_box(&medium_frame).len();
            len <= MAX_AUDIO_FRAME_SIZE
        });
    });

    group.bench_function("validate_large_32kb", |b| {
        b.iter(|| {
            let len = black_box(&large_frame).len();
            len <= MAX_AUDIO_FRAME_SIZE
        });
    });

    group.bench_function("validate_huge_960kb", |b| {
        b.iter(|| {
            let len = black_box(&huge_frame).len();
            len <= MAX_AUDIO_FRAME_SIZE
        });
    });

    group.finish();
}

/// Benchmark JSON processing throughput for realistic workloads
fn bench_json_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_throughput");
    group.measurement_time(Duration::from_secs(10));

    // Simulate a batch of typical messages
    let messages: Vec<String> = (0..100).map(|i| {
        if i % 3 == 0 {
            format!(r#"{{"type":"speak","text":"Message number {} with some content","flush":true}}"#, i)
        } else if i % 3 == 1 {
            format!(r#"{{"type":"send_message","message":"Chat message {}","role":"user","topic":"chat"}}"#, i)
        } else {
            r#"{"type":"clear"}"#.to_string()
        }
    }).collect();

    let total_bytes: u64 = messages.iter().map(|m| m.len() as u64).sum();

    group.throughput(Throughput::Bytes(total_bytes));
    group.bench_function("batch_100_messages", |b| {
        b.iter(|| {
            for msg in black_box(&messages) {
                let _: Result<IncomingMessage, _> = serde_json::from_str(msg);
            }
        });
    });

    group.throughput(Throughput::Elements(messages.len() as u64));
    group.bench_function("batch_100_messages_count", |b| {
        b.iter(|| {
            for msg in black_box(&messages) {
                let _: Result<IncomingMessage, _> = serde_json::from_str(msg);
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_message_parsing,
    bench_message_validation,
    bench_message_serialization,
    bench_phone_validation,
    bench_cache_operations,
    bench_cache_hashing,
    bench_audio_frame_validation,
    bench_json_throughput,
);
criterion_main!(benches);

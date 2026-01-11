//! Breaking Point Test - Find the Absolute Limits of the WaaV Gateway
//!
//! This test escalates load until the gateway fails, recording:
//! - Maximum sustainable RPS
//! - Maximum concurrent connections
//! - Breaking point VUs (where error rate exceeds threshold)
//! - Failure modes (timeouts, connection refused, OOM, etc.)
//!
//! Run with: cargo test --test breaking_point_test --release -- --nocapture

mod mock_providers;

use mock_providers::{
    http_mock::{spawn_http_mock, HttpMockState},
    websocket_mock::{spawn_stt_websocket_mock, spawn_tts_websocket_mock, WebSocketMockState},
    ChaosConfig, LatencyProfile,
};

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, Semaphore};

// ============================================================================
// BREAKING POINT CONFIGURATION
// ============================================================================

/// Error rate threshold to consider as "breaking point"
const ERROR_THRESHOLD_PERCENT: f64 = 5.0;

/// Maximum VUs to test (safety limit)
const MAX_VUS: u32 = 50_000;

/// VU increment per iteration
const VU_INCREMENT: u32 = 500;

/// Starting VU count
const STARTING_VUS: u32 = 1000;

/// Duration per iteration
const ITERATION_DURATION_SECS: u64 = 30;

/// Cool-down between iterations
const COOLDOWN_SECS: u64 = 10;

/// Request timeout
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// ============================================================================
// BREAKING POINT RESULT
// ============================================================================

#[derive(Clone, Debug, serde::Serialize)]
pub struct BreakingPointResult {
    pub vus: u32,
    pub duration_secs: f64,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub timeout_requests: u64,
    pub connection_refused: u64,
    pub error_rate: f64,
    pub rps: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub timestamp: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct BreakingPointSummary {
    pub breaking_point_vus: Option<u32>,
    pub max_stable_vus: u32,
    pub max_rps: f64,
    pub failure_mode: String,
    pub iterations: Vec<BreakingPointResult>,
    pub hardware_info: HardwareInfo,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct HardwareInfo {
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub total_ram_gb: f32,
    pub os: String,
}

// ============================================================================
// SIMPLE LATENCY TRACKER
// ============================================================================

struct LatencyTracker {
    samples: Mutex<Vec<u64>>,
}

impl LatencyTracker {
    fn new() -> Self {
        Self {
            samples: Mutex::new(Vec::with_capacity(100_000)),
        }
    }

    async fn record(&self, latency_us: u64) {
        self.samples.lock().await.push(latency_us);
    }

    async fn percentile(&self, p: f64) -> u64 {
        let mut samples = self.samples.lock().await.clone();
        if samples.is_empty() {
            return 0;
        }
        samples.sort_unstable();
        let idx = ((samples.len() as f64) * p / 100.0).ceil() as usize;
        samples.get(idx.saturating_sub(1)).copied().unwrap_or(0)
    }

    async fn p50(&self) -> u64 {
        self.percentile(50.0).await
    }
    async fn p90(&self) -> u64 {
        self.percentile(90.0).await
    }
    async fn p99(&self) -> u64 {
        self.percentile(99.0).await
    }
    async fn max(&self) -> u64 {
        self.samples.lock().await.iter().max().copied().unwrap_or(0)
    }

    async fn reset(&self) {
        self.samples.lock().await.clear();
    }
}

// ============================================================================
// ITERATION STATS
// ============================================================================

struct IterationStats {
    total_requests: AtomicU64,
    successful_requests: AtomicU64,
    failed_requests: AtomicU64,
    timeout_requests: AtomicU64,
    connection_refused: AtomicU64,
    latency: LatencyTracker,
    start_time: Instant,
}

impl IterationStats {
    fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            timeout_requests: AtomicU64::new(0),
            connection_refused: AtomicU64::new(0),
            latency: LatencyTracker::new(),
            start_time: Instant::now(),
        }
    }

    async fn record_success(&self, latency_us: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.successful_requests.fetch_add(1, Ordering::Relaxed);
        self.latency.record(latency_us).await;
    }

    async fn record_failure(&self, error: &str, latency_us: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
        self.latency.record(latency_us).await;

        if error.contains("connection refused") || error.contains("Connection refused") {
            self.connection_refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_timeout(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.timeout_requests.fetch_add(1, Ordering::Relaxed);
    }

    fn error_rate(&self) -> f64 {
        let total = self.total_requests.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let failed = self.failed_requests.load(Ordering::Relaxed)
            + self.timeout_requests.load(Ordering::Relaxed);
        (failed as f64 / total as f64) * 100.0
    }

    fn rps(&self) -> f64 {
        let total = self.total_requests.load(Ordering::Relaxed);
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            total as f64 / elapsed
        } else {
            0.0
        }
    }

    async fn to_result(&self, vus: u32) -> BreakingPointResult {
        BreakingPointResult {
            vus,
            duration_secs: self.start_time.elapsed().as_secs_f64(),
            total_requests: self.total_requests.load(Ordering::Relaxed),
            successful_requests: self.successful_requests.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            timeout_requests: self.timeout_requests.load(Ordering::Relaxed),
            connection_refused: self.connection_refused.load(Ordering::Relaxed),
            error_rate: self.error_rate(),
            rps: self.rps(),
            p50_ms: self.latency.p50().await as f64 / 1000.0,
            p90_ms: self.latency.p90().await as f64 / 1000.0,
            p99_ms: self.latency.p99().await as f64 / 1000.0,
            max_ms: self.latency.max().await as f64 / 1000.0,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    async fn reset(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.successful_requests.store(0, Ordering::Relaxed);
        self.failed_requests.store(0, Ordering::Relaxed);
        self.timeout_requests.store(0, Ordering::Relaxed);
        self.connection_refused.store(0, Ordering::Relaxed);
        self.latency.reset().await;
    }
}

// ============================================================================
// RESULT SAVER
// ============================================================================

struct BreakingPointSaver {
    output_dir: PathBuf,
    log_file: Mutex<BufWriter<File>>,
}

impl BreakingPointSaver {
    fn new() -> std::io::Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let output_dir = PathBuf::from(format!("/tmp/waav_breaking_point_{}", timestamp));
        std::fs::create_dir_all(&output_dir)?;

        let log_file = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(output_dir.join("iterations.jsonl"))?,
        );

        println!("Results will be saved to: {:?}", output_dir);

        Ok(Self {
            output_dir,
            log_file: Mutex::new(log_file),
        })
    }

    async fn log_iteration(&self, result: &BreakingPointResult) {
        if let Ok(json) = serde_json::to_string(result) {
            let mut file = self.log_file.lock().await;
            let _ = writeln!(file, "{}", json);
            let _ = file.flush();
        }
    }

    async fn write_summary(&self, summary: &BreakingPointSummary) {
        let path = self.output_dir.join("summary.json");
        if let Ok(json) = serde_json::to_string_pretty(summary) {
            if let Ok(mut file) = File::create(&path) {
                let _ = file.write_all(json.as_bytes());
            }
        }

        // Also write human-readable report
        let report_path = self.output_dir.join("report.txt");
        if let Ok(mut file) = File::create(&report_path) {
            let report = generate_report(summary);
            let _ = file.write_all(report.as_bytes());
        }
    }
}

fn generate_report(summary: &BreakingPointSummary) -> String {
    let mut report = String::new();

    report.push_str("\n");
    report.push_str("╔══════════════════════════════════════════════════════════════════════════╗\n");
    report.push_str("║                    WAAV GATEWAY BREAKING POINT REPORT                    ║\n");
    report.push_str("╠══════════════════════════════════════════════════════════════════════════╣\n\n");

    report.push_str("HARDWARE:\n");
    report.push_str(&format!("  CPU: {} ({} cores)\n", summary.hardware_info.cpu_model, summary.hardware_info.cpu_cores));
    report.push_str(&format!("  RAM: {:.1} GB\n", summary.hardware_info.total_ram_gb));
    report.push_str(&format!("  OS: {}\n\n", summary.hardware_info.os));

    report.push_str("RESULTS:\n");
    if let Some(bp) = summary.breaking_point_vus {
        report.push_str(&format!("  Breaking Point: {} VUs\n", bp));
    } else {
        report.push_str("  Breaking Point: NOT REACHED (max VUs tested without failure)\n");
    }
    report.push_str(&format!("  Max Stable VUs: {}\n", summary.max_stable_vus));
    report.push_str(&format!("  Max RPS: {:.0}\n", summary.max_rps));
    report.push_str(&format!("  Failure Mode: {}\n\n", summary.failure_mode));

    report.push_str("ITERATIONS:\n");
    report.push_str("┌─────────┬───────────┬───────────┬───────────┬───────────┬───────────┐\n");
    report.push_str("│   VUs   │    RPS    │  Error %  │   P50 ms  │   P99 ms  │  Timeouts │\n");
    report.push_str("├─────────┼───────────┼───────────┼───────────┼───────────┼───────────┤\n");

    for iter in &summary.iterations {
        report.push_str(&format!(
            "│ {:>7} │ {:>9.0} │ {:>8.2}% │ {:>9.2} │ {:>9.2} │ {:>9} │\n",
            iter.vus,
            iter.rps,
            iter.error_rate,
            iter.p50_ms,
            iter.p99_ms,
            iter.timeout_requests
        ));
    }

    report.push_str("└─────────┴───────────┴───────────┴───────────┴───────────┴───────────┘\n\n");

    report.push_str("╚══════════════════════════════════════════════════════════════════════════╝\n");

    report
}

// ============================================================================
// HTTP CLIENT REQUESTS
// ============================================================================

async fn make_request(
    client: &reqwest::Client,
    base_url: &str,
    stats: &IterationStats,
) {
    let start = Instant::now();
    let url = format!("{}/", base_url);

    let result = tokio::time::timeout(REQUEST_TIMEOUT, client.get(&url).send()).await;

    let latency_us = start.elapsed().as_micros() as u64;

    match result {
        Ok(Ok(response)) => {
            if response.status().is_success() {
                stats.record_success(latency_us).await;
            } else {
                stats
                    .record_failure(&format!("HTTP {}", response.status()), latency_us)
                    .await;
            }
        }
        Ok(Err(e)) => {
            stats.record_failure(&e.to_string(), latency_us).await;
        }
        Err(_) => {
            stats.record_timeout();
        }
    }
}

// ============================================================================
// RUN ITERATION
// ============================================================================

async fn run_iteration(
    vus: u32,
    duration: Duration,
    base_url: &str,
    semaphore: Arc<Semaphore>,
) -> BreakingPointResult {
    let stats = Arc::new(IterationStats::new());

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(vus as usize)
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");

    let client = Arc::new(client);
    let running = Arc::new(AtomicBool::new(true));

    // Spawn workers
    let mut handles = Vec::with_capacity(vus as usize);

    for _ in 0..vus {
        let client = client.clone();
        let stats = stats.clone();
        let semaphore = semaphore.clone();
        let running = running.clone();
        let base_url = base_url.to_string();

        let handle = tokio::spawn(async move {
            while running.load(Ordering::Relaxed) {
                let _permit = semaphore.acquire().await.unwrap();
                make_request(&client, &base_url, &stats).await;

                // Small delay at high VU counts
                if vus >= 2000 {
                    tokio::time::sleep(Duration::from_micros(50)).await;
                }
            }
        });

        handles.push(handle);
    }

    // Run for duration
    tokio::time::sleep(duration).await;
    running.store(false, Ordering::Relaxed);

    // Wait for workers
    for handle in handles {
        let _ = handle.await;
    }

    stats.to_result(vus).await
}

// ============================================================================
// GET HARDWARE INFO
// ============================================================================

fn get_hardware_info() -> HardwareInfo {
    let cpu_model = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("Unknown").trim().to_string())
        })
        .unwrap_or_else(|| "Unknown".to_string());

    let cpu_cores = std::thread::available_parallelism()
        .map(|p| p.get() as u32)
        .unwrap_or(1);

    let total_ram_gb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal"))
                .and_then(|l| {
                    l.split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse::<f32>().ok())
                })
        })
        .map(|kb| kb / 1024.0 / 1024.0)
        .unwrap_or(0.0);

    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("PRETTY_NAME"))
                .map(|l| {
                    l.split('=')
                        .nth(1)
                        .unwrap_or("Unknown")
                        .trim_matches('"')
                        .to_string()
                })
        })
        .unwrap_or_else(|| "Unknown".to_string());

    HardwareInfo {
        cpu_model,
        cpu_cores,
        total_ram_gb,
        os,
    }
}

// ============================================================================
// MAIN TEST
// ============================================================================

/// Find the breaking point of the gateway by escalating load
#[tokio::test]
async fn test_find_breaking_point() {
    println!("\n{}", "=".repeat(70));
    println!("WaaV Gateway Breaking Point Test");
    println!("{}\n", "=".repeat(70));

    let base_url =
        std::env::var("GATEWAY_URL").unwrap_or_else(|_| "http://127.0.0.1:3001".to_string());

    // Initialize result saver
    let saver = Arc::new(BreakingPointSaver::new().expect("Failed to create result saver"));

    // Check gateway
    let client = reqwest::Client::new();
    match client.get(format!("{}/", base_url)).send().await {
        Ok(response) if response.status().is_success() => {
            println!("Gateway is running at {}", base_url);
        }
        _ => {
            println!("SKIPPING: Gateway not running at {}", base_url);
            println!("Start gateway with: cargo run --release");
            return;
        }
    }

    let hardware_info = get_hardware_info();
    println!("Hardware: {} ({} cores), {:.1} GB RAM",
             hardware_info.cpu_model, hardware_info.cpu_cores, hardware_info.total_ram_gb);

    let semaphore = Arc::new(Semaphore::new(100_000));

    let mut current_vus = STARTING_VUS;
    let mut iterations = Vec::new();
    let mut breaking_point_vus = None;
    let mut max_stable_vus = 0u32;
    let mut max_rps = 0.0f64;
    let mut failure_mode = "None".to_string();

    println!("\nStarting escalation test...");
    println!("  Error threshold: {}%", ERROR_THRESHOLD_PERCENT);
    println!("  VU increment: {}", VU_INCREMENT);
    println!("  Iteration duration: {}s", ITERATION_DURATION_SECS);
    println!("");

    while current_vus <= MAX_VUS {
        println!(
            "Testing {} VUs for {} seconds...",
            current_vus, ITERATION_DURATION_SECS
        );

        let result = run_iteration(
            current_vus,
            Duration::from_secs(ITERATION_DURATION_SECS),
            &base_url,
            semaphore.clone(),
        )
        .await;

        // Save immediately
        saver.log_iteration(&result).await;
        iterations.push(result.clone());

        // Print summary
        println!(
            "  RPS: {:.0} | Error Rate: {:.2}% | P99: {:.2}ms | Timeouts: {}",
            result.rps, result.error_rate, result.p99_ms, result.timeout_requests
        );

        // Track max RPS and stable VUs
        if result.error_rate < ERROR_THRESHOLD_PERCENT {
            max_stable_vus = current_vus;
            if result.rps > max_rps {
                max_rps = result.rps;
            }
        }

        // Check for breaking point
        if result.error_rate >= ERROR_THRESHOLD_PERCENT {
            breaking_point_vus = Some(current_vus);

            // Determine failure mode
            if result.timeout_requests > result.failed_requests {
                failure_mode = "TIMEOUT".to_string();
            } else if result.connection_refused > 0 {
                failure_mode = "CONNECTION_REFUSED".to_string();
            } else {
                failure_mode = "HTTP_ERRORS".to_string();
            }

            println!(
                "\n🔴 BREAKING POINT FOUND: {} VUs (Error Rate: {:.2}%)",
                current_vus, result.error_rate
            );
            println!("   Failure Mode: {}", failure_mode);
            break;
        }

        // Cool down
        println!("  [Cool-down: {}s]", COOLDOWN_SECS);
        tokio::time::sleep(Duration::from_secs(COOLDOWN_SECS)).await;

        current_vus += VU_INCREMENT;
    }

    if breaking_point_vus.is_none() {
        println!(
            "\n⚠️  Max VUs ({}) reached without hitting error threshold",
            MAX_VUS
        );
    }

    // Generate summary
    let summary = BreakingPointSummary {
        breaking_point_vus,
        max_stable_vus,
        max_rps,
        failure_mode,
        iterations,
        hardware_info,
    };

    // Save summary
    saver.write_summary(&summary).await;

    // Print report
    let report = generate_report(&summary);
    println!("{}", report);

    println!("Results saved to: {:?}", saver.output_dir);
}

/// Quick breaking point test (shorter iterations)
#[tokio::test]
async fn test_quick_breaking_point() {
    println!("\n{}", "=".repeat(70));
    println!("WaaV Gateway Quick Breaking Point Test (10s iterations)");
    println!("{}\n", "=".repeat(70));

    let base_url =
        std::env::var("GATEWAY_URL").unwrap_or_else(|_| "http://127.0.0.1:3001".to_string());

    let client = reqwest::Client::new();
    match client.get(format!("{}/", base_url)).send().await {
        Ok(response) if response.status().is_success() => {
            println!("Gateway is running at {}", base_url);
        }
        _ => {
            println!("SKIPPING: Gateway not running at {}", base_url);
            return;
        }
    }

    let semaphore = Arc::new(Semaphore::new(50_000));

    // Quick test with fewer VUs and shorter duration
    let quick_stages: &[(u32, u64)] = &[
        (100, 10),
        (500, 10),
        (1000, 10),
        (2000, 10),
        (3000, 10),
        (5000, 10),
    ];

    println!("Running quick escalation test...\n");

    for (vus, duration_secs) in quick_stages {
        println!("Testing {} VUs for {}s...", vus, duration_secs);

        let result = run_iteration(
            *vus,
            Duration::from_secs(*duration_secs),
            &base_url,
            semaphore.clone(),
        )
        .await;

        println!(
            "  RPS: {:.0} | Error: {:.2}% | P99: {:.2}ms",
            result.rps, result.error_rate, result.p99_ms
        );

        if result.error_rate >= ERROR_THRESHOLD_PERCENT {
            println!("\n🔴 Breaking at {} VUs", vus);
            return;
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    println!("\n✅ All stages passed without breaking");
}

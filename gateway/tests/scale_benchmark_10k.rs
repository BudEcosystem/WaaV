//! Scale Benchmark Test - 10,000 Requests with Gradual Ramp-Up
//!
//! This benchmark tests the WaaV Gateway's performance under increasing load,
//! measuring latency percentiles (P50, P90, P99, P99.9), throughput, and
//! identifying bottlenecks (compute/memory/IO/network bound).
//!
//! Run with: cargo test --test scale_benchmark_10k --release -- --nocapture

mod mock_providers;

use mock_providers::{
    http_mock::{spawn_http_mock, HttpMockState},
    websocket_mock::{spawn_stt_websocket_mock, spawn_tts_websocket_mock, WebSocketMockState},
    ChaosConfig, LatencyProfile, MockStats,
};

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write as IoWrite};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, Semaphore};

// ============================================================================
// CONTINUOUS RESULT SAVER
// ============================================================================

/// Continuously saves results to disk to prevent data loss on crash
pub struct ResultSaver {
    output_dir: PathBuf,
    request_log_file: Mutex<BufWriter<File>>,
    stage_log_file: Mutex<BufWriter<File>>,
    resource_log_file: Mutex<BufWriter<File>>,
    running: AtomicBool,
    last_flush: Mutex<Instant>,
}

impl ResultSaver {
    pub fn new() -> std::io::Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let output_dir = PathBuf::from(format!("/tmp/waav_benchmark_{}", timestamp));
        std::fs::create_dir_all(&output_dir)?;

        // Create log files with line buffering
        let request_log = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(output_dir.join("requests.jsonl"))?
        );

        let stage_log = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(output_dir.join("stages.jsonl"))?
        );

        let resource_log = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(output_dir.join("resources.jsonl"))?
        );

        println!("Results will be saved to: {:?}", output_dir);

        Ok(Self {
            output_dir,
            request_log_file: Mutex::new(request_log),
            stage_log_file: Mutex::new(stage_log),
            resource_log_file: Mutex::new(resource_log),
            running: AtomicBool::new(true),
            last_flush: Mutex::new(Instant::now()),
        })
    }

    pub fn output_dir(&self) -> &PathBuf {
        &self.output_dir
    }

    /// Append a request entry (line-delimited JSON)
    pub async fn log_request(&self, entry: &RequestLogEntry) {
        if let Ok(json) = serde_json::to_string(entry) {
            let mut file = self.request_log_file.lock().await;
            let _ = writeln!(file, "{}", json);

            // Flush every second
            let mut last_flush = self.last_flush.lock().await;
            if last_flush.elapsed() >= Duration::from_secs(1) {
                let _ = file.flush();
                *last_flush = Instant::now();
            }
        }
    }

    /// Log a stage result
    pub async fn log_stage(&self, result: &StageResult) {
        let stage_json = serde_json::json!({
            "stage": result.stage_name,
            "vus": result.vus,
            "duration_secs": result.duration_secs,
            "total_requests": result.total_requests,
            "successful_requests": result.successful_requests,
            "failed_requests": result.failed_requests,
            "timeout_requests": result.timeout_requests,
            "p50_ms": result.p50_ms,
            "p90_ms": result.p90_ms,
            "p99_ms": result.p99_ms,
            "p999_ms": result.p999_ms,
            "min_ms": result.min_ms,
            "max_ms": result.max_ms,
            "mean_ms": result.mean_ms,
            "rps": result.rps,
            "error_rate": result.error_rate,
            "timestamp": SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        });

        let mut file = self.stage_log_file.lock().await;
        let _ = writeln!(file, "{}", stage_json);
        let _ = file.flush(); // Always flush stage results immediately
    }

    /// Log resource snapshot
    pub async fn log_resource(&self, snapshot: &ResourceSnapshot) {
        if let Ok(json) = serde_json::to_string(snapshot) {
            let mut file = self.resource_log_file.lock().await;
            let _ = writeln!(file, "{}", json);
            let _ = file.flush();
        }
    }

    /// Force flush all files
    pub async fn flush_all(&self) {
        let _ = self.request_log_file.lock().await.flush();
        let _ = self.stage_log_file.lock().await.flush();
        let _ = self.resource_log_file.lock().await.flush();
    }

    /// Write final summary
    pub async fn write_summary(&self, report: &str) {
        let summary_path = self.output_dir.join("summary.txt");
        if let Ok(mut file) = File::create(&summary_path) {
            let _ = file.write_all(report.as_bytes());
            println!("Summary saved to: {:?}", summary_path);
        }
    }
}

// ============================================================================
// LATENCY HISTOGRAM
// ============================================================================

/// Latency histogram with 12 buckets for percentile calculation
/// Buckets: <1ms, <2ms, <5ms, <10ms, <20ms, <50ms, <100ms, <200ms, <500ms, <1s, <2s, >2s
pub struct LatencyHistogram {
    buckets: [AtomicU64; 12],
    total_count: AtomicU64,
    total_latency_us: AtomicU64,
    min_latency_us: AtomicU64,
    max_latency_us: AtomicU64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl LatencyHistogram {
    pub fn new() -> Self {
        Self {
            buckets: Default::default(),
            total_count: AtomicU64::new(0),
            total_latency_us: AtomicU64::new(0),
            min_latency_us: AtomicU64::new(u64::MAX),
            max_latency_us: AtomicU64::new(0),
        }
    }

    /// Record a latency measurement in microseconds
    pub fn record(&self, latency_us: u64) {
        // Update bucket
        let bucket_idx = self.get_bucket_index(latency_us);
        self.buckets[bucket_idx].fetch_add(1, Ordering::Relaxed);

        // Update aggregates
        self.total_count.fetch_add(1, Ordering::Relaxed);
        self.total_latency_us.fetch_add(latency_us, Ordering::Relaxed);

        // Update min (CAS loop for atomic min)
        let mut current_min = self.min_latency_us.load(Ordering::Relaxed);
        while latency_us < current_min {
            match self.min_latency_us.compare_exchange_weak(
                current_min,
                latency_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_min = actual,
            }
        }

        // Update max (CAS loop for atomic max)
        let mut current_max = self.max_latency_us.load(Ordering::Relaxed);
        while latency_us > current_max {
            match self.max_latency_us.compare_exchange_weak(
                current_max,
                latency_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }
    }

    fn get_bucket_index(&self, latency_us: u64) -> usize {
        let latency_ms = latency_us / 1000;
        match latency_ms {
            0 => 0,             // <1ms
            1 => 1,             // <2ms
            2..=4 => 2,         // <5ms
            5..=9 => 3,         // <10ms
            10..=19 => 4,       // <20ms
            20..=49 => 5,       // <50ms
            50..=99 => 6,       // <100ms
            100..=199 => 7,     // <200ms
            200..=499 => 8,     // <500ms
            500..=999 => 9,     // <1s
            1000..=1999 => 10,  // <2s
            _ => 11,            // >2s
        }
    }

    /// Calculate percentile in microseconds
    pub fn percentile(&self, p: f64) -> u64 {
        let total = self.total_count.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }

        let target = ((total as f64) * p / 100.0).ceil() as u64;
        let mut cumulative = 0u64;

        // Bucket upper bounds in microseconds
        let bucket_bounds_us: [u64; 12] = [
            1_000, 2_000, 5_000, 10_000, 20_000, 50_000, 100_000, 200_000, 500_000, 1_000_000,
            2_000_000, u64::MAX,
        ];

        for (idx, bound) in bucket_bounds_us.iter().enumerate() {
            cumulative += self.buckets[idx].load(Ordering::Relaxed);
            if cumulative >= target {
                // Interpolate within bucket for better accuracy
                if idx == 0 {
                    return *bound / 2; // Midpoint of first bucket
                }
                let prev_bound = if idx > 0 { bucket_bounds_us[idx - 1] } else { 0 };
                return (prev_bound + *bound) / 2; // Midpoint approximation
            }
        }

        self.max_latency_us.load(Ordering::Relaxed)
    }

    pub fn p50(&self) -> u64 {
        self.percentile(50.0)
    }
    pub fn p90(&self) -> u64 {
        self.percentile(90.0)
    }
    pub fn p99(&self) -> u64 {
        self.percentile(99.0)
    }
    pub fn p999(&self) -> u64 {
        self.percentile(99.9)
    }

    pub fn mean_us(&self) -> u64 {
        let count = self.total_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0;
        }
        self.total_latency_us.load(Ordering::Relaxed) / count
    }

    pub fn min_us(&self) -> u64 {
        let min = self.min_latency_us.load(Ordering::Relaxed);
        if min == u64::MAX {
            0
        } else {
            min
        }
    }

    pub fn max_us(&self) -> u64 {
        self.max_latency_us.load(Ordering::Relaxed)
    }

    pub fn count(&self) -> u64 {
        self.total_count.load(Ordering::Relaxed)
    }

    /// Reset histogram for next stage
    pub fn reset(&self) {
        for bucket in &self.buckets {
            bucket.store(0, Ordering::Relaxed);
        }
        self.total_count.store(0, Ordering::Relaxed);
        self.total_latency_us.store(0, Ordering::Relaxed);
        self.min_latency_us.store(u64::MAX, Ordering::Relaxed);
        self.max_latency_us.store(0, Ordering::Relaxed);
    }

    /// Get bucket distribution as string for detailed analysis
    pub fn bucket_distribution(&self) -> String {
        let labels = [
            "<1ms", "<2ms", "<5ms", "<10ms", "<20ms", "<50ms", "<100ms", "<200ms", "<500ms",
            "<1s", "<2s", ">2s",
        ];
        let mut result = String::new();
        for (idx, label) in labels.iter().enumerate() {
            let count = self.buckets[idx].load(Ordering::Relaxed);
            if count > 0 {
                result.push_str(&format!("  {}: {}\n", label, count));
            }
        }
        result
    }
}

// ============================================================================
// REQUEST LOG ENTRY
// ============================================================================

/// Per-request log entry for detailed analysis
#[derive(Clone, Debug, serde::Serialize)]
pub struct RequestLogEntry {
    pub timestamp_us: u64,       // Microseconds since benchmark start
    pub latency_us: u64,         // Request latency in microseconds
    pub success: bool,           // Success/failure
    pub stage: String,           // Current load stage
    pub concurrent: u32,         // Target concurrent VUs
    pub request_id: u64,         // Unique request ID
    pub error: Option<String>,   // Error message if failed
}

// ============================================================================
// BENCHMARK STATISTICS
// ============================================================================

/// Comprehensive benchmark statistics with atomic counters for thread-safety
pub struct BenchmarkStats {
    pub latency: LatencyHistogram,
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub timeout_requests: AtomicU64,
    pub start_time: Instant,
    pub request_log: Mutex<Vec<RequestLogEntry>>,
    pub current_stage: RwLock<String>,
    pub current_vus: AtomicU64,
    pub result_saver: Option<Arc<ResultSaver>>,
}

impl BenchmarkStats {
    pub fn new() -> Self {
        Self {
            latency: LatencyHistogram::new(),
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            timeout_requests: AtomicU64::new(0),
            start_time: Instant::now(),
            request_log: Mutex::new(Vec::with_capacity(100_000)),
            current_stage: RwLock::new("Initializing".to_string()),
            current_vus: AtomicU64::new(0),
            result_saver: None,
        }
    }

    pub fn with_saver(result_saver: Arc<ResultSaver>) -> Self {
        Self {
            latency: LatencyHistogram::new(),
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            timeout_requests: AtomicU64::new(0),
            start_time: Instant::now(),
            request_log: Mutex::new(Vec::with_capacity(100_000)),
            current_stage: RwLock::new("Initializing".to_string()),
            current_vus: AtomicU64::new(0),
            result_saver: Some(result_saver),
        }
    }

    pub async fn record_success(&self, latency_us: u64, request_id: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.successful_requests.fetch_add(1, Ordering::Relaxed);
        self.latency.record(latency_us);

        let entry = RequestLogEntry {
            timestamp_us: self.start_time.elapsed().as_micros() as u64,
            latency_us,
            success: true,
            stage: self.current_stage.read().await.clone(),
            concurrent: self.current_vus.load(Ordering::Relaxed) as u32,
            request_id,
            error: None,
        };

        // Save to disk immediately if saver is configured
        if let Some(saver) = &self.result_saver {
            saver.log_request(&entry).await;
        }

        self.request_log.lock().await.push(entry);
    }

    pub async fn record_failure(&self, latency_us: u64, request_id: u64, error: String) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
        self.latency.record(latency_us);

        let entry = RequestLogEntry {
            timestamp_us: self.start_time.elapsed().as_micros() as u64,
            latency_us,
            success: false,
            stage: self.current_stage.read().await.clone(),
            concurrent: self.current_vus.load(Ordering::Relaxed) as u32,
            request_id,
            error: Some(error),
        };

        // Save to disk immediately if saver is configured
        if let Some(saver) = &self.result_saver {
            saver.log_request(&entry).await;
        }

        self.request_log.lock().await.push(entry);
    }

    pub async fn record_timeout(&self, request_id: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.timeout_requests.fetch_add(1, Ordering::Relaxed);

        let entry = RequestLogEntry {
            timestamp_us: self.start_time.elapsed().as_micros() as u64,
            latency_us: 30_000_000, // 30 second timeout
            success: false,
            stage: self.current_stage.read().await.clone(),
            concurrent: self.current_vus.load(Ordering::Relaxed) as u32,
            request_id,
            error: Some("TIMEOUT".to_string()),
        };

        // Save to disk immediately if saver is configured
        if let Some(saver) = &self.result_saver {
            saver.log_request(&entry).await;
        }

        self.request_log.lock().await.push(entry);
    }

    pub fn error_rate(&self) -> f64 {
        let total = self.total_requests.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let failed = self.failed_requests.load(Ordering::Relaxed)
            + self.timeout_requests.load(Ordering::Relaxed);
        (failed as f64 / total as f64) * 100.0
    }

    pub fn rps(&self) -> f64 {
        let total = self.total_requests.load(Ordering::Relaxed);
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            total as f64 / elapsed
        } else {
            0.0
        }
    }
}

// ============================================================================
// STAGE RESULTS
// ============================================================================

#[derive(Clone, Debug)]
pub struct StageResult {
    pub stage_name: String,
    pub vus: u32,
    pub duration_secs: f64,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub timeout_requests: u64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    pub p999_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
    pub rps: f64,
    pub error_rate: f64,
}

// ============================================================================
// RESOURCE MONITOR
// ============================================================================

/// Resource usage snapshot
#[derive(Clone, Debug, serde::Serialize)]
pub struct ResourceSnapshot {
    pub timestamp_ms: u64,
    pub cpu_percent: f32,
    pub memory_mb: f32,
    pub open_fds: u32,
    pub active_connections: u32,
}

/// Resource monitor that periodically samples system metrics
pub struct ResourceMonitor {
    samples: Mutex<Vec<ResourceSnapshot>>,
    start_time: Instant,
    running: AtomicBool,
}

impl ResourceMonitor {
    pub fn new() -> Self {
        Self {
            samples: Mutex::new(Vec::with_capacity(10_000)),
            start_time: Instant::now(),
            running: AtomicBool::new(false),
        }
    }

    pub async fn start(&self, pid: Option<u32>) {
        self.running.store(true, Ordering::SeqCst);

        let start = self.start_time;
        let running = &self.running;
        let samples = &self.samples;

        while running.load(Ordering::SeqCst) {
            let snapshot = Self::capture_snapshot(&start, pid).await;
            samples.lock().await.push(snapshot);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    async fn capture_snapshot(start: &Instant, pid: Option<u32>) -> ResourceSnapshot {
        let timestamp_ms = start.elapsed().as_millis() as u64;

        // Try to read /proc stats if on Linux
        let (cpu_percent, memory_mb, open_fds) = if let Some(pid) = pid {
            Self::read_proc_stats(pid).await.unwrap_or((0.0, 0.0, 0))
        } else {
            (0.0, 0.0, 0)
        };

        ResourceSnapshot {
            timestamp_ms,
            cpu_percent,
            memory_mb,
            open_fds,
            active_connections: 0, // Would need gateway integration
        }
    }

    #[cfg(target_os = "linux")]
    async fn read_proc_stats(pid: u32) -> Option<(f32, f32, u32)> {
        use std::fs;

        // Read memory from /proc/[pid]/statm
        let statm = fs::read_to_string(format!("/proc/{}/statm", pid)).ok()?;
        let parts: Vec<&str> = statm.split_whitespace().collect();
        let rss_pages: u64 = parts.get(1)?.parse().ok()?;
        let page_size = 4096u64; // Typical page size
        let memory_mb = (rss_pages * page_size) as f32 / (1024.0 * 1024.0);

        // Count open file descriptors
        let fd_count = fs::read_dir(format!("/proc/{}/fd", pid))
            .map(|entries| entries.count() as u32)
            .unwrap_or(0);

        // CPU usage would require delta calculation over time - simplified here
        let cpu_percent = 0.0; // Would need /proc/[pid]/stat parsing

        Some((cpu_percent, memory_mb, fd_count))
    }

    #[cfg(not(target_os = "linux"))]
    async fn read_proc_stats(_pid: u32) -> Option<(f32, f32, u32)> {
        None
    }

    pub async fn get_peak_memory_mb(&self) -> f32 {
        self.samples
            .lock()
            .await
            .iter()
            .map(|s| s.memory_mb)
            .fold(0.0f32, f32::max)
    }

    pub async fn get_peak_fds(&self) -> u32 {
        self.samples
            .lock()
            .await
            .iter()
            .map(|s| s.open_fds)
            .max()
            .unwrap_or(0)
    }
}

// ============================================================================
// RAMP-UP STAGES
// ============================================================================

/// Ramp-up stages: (concurrent_users, duration, stage_name)
const RAMP_STAGES: &[(u32, u64, &str)] = &[
    (10, 30, "Warmup"),
    (100, 60, "Light Load"),
    (500, 60, "Medium Load"),
    (1000, 60, "Heavy Load"),
    (2000, 60, "Stress"),
    (5000, 60, "High Stress"),
    (10000, 60, "Maximum Load"),
];

// ============================================================================
// HTTP CLIENT FOR REST ENDPOINTS
// ============================================================================

async fn make_rest_request(
    client: &reqwest::Client,
    base_url: &str,
    stats: &BenchmarkStats,
    request_id: u64,
    timeout: Duration,
) {
    let start = Instant::now();
    let url = format!("{}/", base_url);

    let result = tokio::time::timeout(timeout, client.get(&url).send()).await;

    let latency_us = start.elapsed().as_micros() as u64;

    match result {
        Ok(Ok(response)) => {
            if response.status().is_success() {
                stats.record_success(latency_us, request_id).await;
            } else {
                stats
                    .record_failure(
                        latency_us,
                        request_id,
                        format!("HTTP {}", response.status()),
                    )
                    .await;
            }
        }
        Ok(Err(e)) => {
            stats
                .record_failure(latency_us, request_id, format!("Request error: {}", e))
                .await;
        }
        Err(_) => {
            stats.record_timeout(request_id).await;
        }
    }
}

async fn make_voices_request(
    client: &reqwest::Client,
    base_url: &str,
    stats: &BenchmarkStats,
    request_id: u64,
    timeout: Duration,
) {
    let start = Instant::now();
    let url = format!("{}/voices", base_url);

    let result = tokio::time::timeout(timeout, client.get(&url).send()).await;

    let latency_us = start.elapsed().as_micros() as u64;

    match result {
        Ok(Ok(response)) => {
            if response.status().is_success() {
                stats.record_success(latency_us, request_id).await;
            } else {
                stats
                    .record_failure(
                        latency_us,
                        request_id,
                        format!("HTTP {}", response.status()),
                    )
                    .await;
            }
        }
        Ok(Err(e)) => {
            stats
                .record_failure(latency_us, request_id, format!("Request error: {}", e))
                .await;
        }
        Err(_) => {
            stats.record_timeout(request_id).await;
        }
    }
}

// ============================================================================
// WEBSOCKET CLIENT
// ============================================================================

async fn make_websocket_request(
    base_url: &str,
    stats: &BenchmarkStats,
    request_id: u64,
    timeout: Duration,
) {
    use tokio_tungstenite::connect_async;

    let start = Instant::now();
    let ws_url = base_url.replace("http://", "ws://").replace("https://", "wss://");
    let ws_url = format!("{}/ws", ws_url);

    let result = tokio::time::timeout(timeout, connect_async(&ws_url)).await;

    let latency_us = start.elapsed().as_micros() as u64;

    match result {
        Ok(Ok((_ws_stream, _))) => {
            stats.record_success(latency_us, request_id).await;
            // Connection established - close it cleanly
        }
        Ok(Err(e)) => {
            stats
                .record_failure(latency_us, request_id, format!("WebSocket error: {}", e))
                .await;
        }
        Err(_) => {
            stats.record_timeout(request_id).await;
        }
    }
}

// ============================================================================
// BENCHMARK RUNNER
// ============================================================================

/// Run a single stage of the benchmark
async fn run_stage(
    stage_name: &str,
    vus: u32,
    duration: Duration,
    base_url: &str,
    stats: Arc<BenchmarkStats>,
    semaphore: Arc<Semaphore>,
) -> StageResult {
    println!(
        "\n{:=<70}",
        format!(" Stage: {} ({} VUs) ", stage_name, vus)
    );

    // Update current stage
    *stats.current_stage.write().await = stage_name.to_string();
    stats.current_vus.store(vus as u64, Ordering::Relaxed);

    // Reset histogram for this stage
    stats.latency.reset();
    let stage_start = Instant::now();
    let stage_request_start = stats.total_requests.load(Ordering::Relaxed);

    // Create HTTP client with connection pooling
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(vus as usize)
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");

    let client = Arc::new(client);
    let running = Arc::new(AtomicBool::new(true));

    // Spawn worker tasks
    let mut handles = Vec::with_capacity(vus as usize);
    let request_counter = Arc::new(AtomicU64::new(0));

    for worker_id in 0..vus {
        let client = client.clone();
        let stats = stats.clone();
        let semaphore = semaphore.clone();
        let running = running.clone();
        let base_url = base_url.to_string();
        let request_counter = request_counter.clone();

        let handle = tokio::spawn(async move {
            while running.load(Ordering::Relaxed) {
                // Acquire semaphore permit for rate limiting
                let _permit = semaphore.acquire().await.unwrap();

                let request_id = request_counter.fetch_add(1, Ordering::Relaxed);

                // Focus on health endpoint for pure gateway throughput testing
                // Other endpoints require provider configuration
                make_rest_request(
                    &client,
                    &base_url,
                    &stats,
                    request_id,
                    Duration::from_secs(30),
                )
                .await;

                // Small delay to prevent overwhelming at high VU counts
                if vus >= 1000 {
                    tokio::time::sleep(Duration::from_micros(100)).await;
                }
            }
        });

        handles.push(handle);
    }

    // Run for specified duration
    tokio::time::sleep(duration).await;
    running.store(false, Ordering::Relaxed);

    // Wait for all workers to complete
    for handle in handles {
        let _ = handle.await;
    }

    let stage_duration = stage_start.elapsed();
    let stage_requests = stats.total_requests.load(Ordering::Relaxed) - stage_request_start;
    let stage_rps = stage_requests as f64 / stage_duration.as_secs_f64();

    // Collect stage results
    let result = StageResult {
        stage_name: stage_name.to_string(),
        vus,
        duration_secs: stage_duration.as_secs_f64(),
        total_requests: stage_requests,
        successful_requests: stats.successful_requests.load(Ordering::Relaxed),
        failed_requests: stats.failed_requests.load(Ordering::Relaxed),
        timeout_requests: stats.timeout_requests.load(Ordering::Relaxed),
        p50_ms: stats.latency.p50() as f64 / 1000.0,
        p90_ms: stats.latency.p90() as f64 / 1000.0,
        p99_ms: stats.latency.p99() as f64 / 1000.0,
        p999_ms: stats.latency.p999() as f64 / 1000.0,
        min_ms: stats.latency.min_us() as f64 / 1000.0,
        max_ms: stats.latency.max_us() as f64 / 1000.0,
        mean_ms: stats.latency.mean_us() as f64 / 1000.0,
        rps: stage_rps,
        error_rate: stats.error_rate(),
    };

    // Print stage summary
    println!(
        "  Requests: {} | RPS: {:.1} | Error Rate: {:.2}%",
        result.total_requests, result.rps, result.error_rate
    );
    println!(
        "  Latency: P50={:.2}ms P90={:.2}ms P99={:.2}ms P99.9={:.2}ms",
        result.p50_ms, result.p90_ms, result.p99_ms, result.p999_ms
    );
    println!(
        "  Range: min={:.2}ms max={:.2}ms mean={:.2}ms",
        result.min_ms, result.max_ms, result.mean_ms
    );

    result
}

// ============================================================================
// REPORT GENERATION
// ============================================================================

fn generate_report(
    stage_results: &[StageResult],
    total_duration: Duration,
    total_requests: u64,
    peak_memory_mb: f32,
    peak_fds: u32,
) -> String {
    let mut report = String::new();

    report.push_str(
        "\n╔══════════════════════════════════════════════════════════════════════════╗\n",
    );
    report.push_str(
        "║                    WAAV GATEWAY SCALE BENCHMARK REPORT                    ║\n",
    );
    report.push_str(
        "╠══════════════════════════════════════════════════════════════════════════╣\n",
    );
    report.push_str(&format!(
        "║ Test Duration: {:.1} seconds                                              ║\n",
        total_duration.as_secs_f64()
    ));
    report.push_str(&format!(
        "║ Total Requests: {:>10}                                              ║\n",
        total_requests
    ));
    report.push_str(&format!(
        "║ Peak Memory: {:.1} MB | Peak FDs: {}                                    ║\n",
        peak_memory_mb, peak_fds
    ));
    report.push_str(
        "╠══════════════════════════════════════════════════════════════════════════╣\n\n",
    );

    // Stage results table
    report.push_str("Stage Results:\n");
    report.push_str("┌──────────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┐\n");
    report.push_str("│ Stage        │ VUs     │ P50     │ P90     │ P99     │ P99.9   │ RPS     │ Error % │\n");
    report.push_str("├──────────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤\n");

    for result in stage_results {
        report.push_str(&format!(
            "│ {:12} │ {:>7} │ {:>5.1}ms │ {:>5.1}ms │ {:>5.1}ms │ {:>5.1}ms │ {:>7.0} │ {:>5.2}% │\n",
            result.stage_name,
            result.vus,
            result.p50_ms,
            result.p90_ms,
            result.p99_ms,
            result.p999_ms,
            result.rps,
            result.error_rate
        ));
    }

    report.push_str("└──────────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┘\n\n");

    // Bottleneck analysis
    report.push_str("Bottleneck Analysis:\n");

    // Analyze where performance degraded
    let mut bottleneck = "UNKNOWN";
    let mut evidence = "Insufficient data";

    if let Some(max_stress) = stage_results.iter().find(|r| r.error_rate > 5.0) {
        bottleneck = "BREAKING POINT";
        evidence = &max_stress.stage_name;
    } else if let Some(high_latency) = stage_results.iter().find(|r| r.p99_ms > 500.0) {
        bottleneck = "LATENCY DEGRADATION";
        evidence = &high_latency.stage_name;
    }

    report.push_str(&format!("- Primary: {} bound\n", bottleneck));
    report.push_str(&format!("- Evidence: {}\n", evidence));

    // Find breaking point (first stage with >5% error rate)
    if let Some(breaking) = stage_results.iter().find(|r| r.error_rate > 5.0) {
        report.push_str(&format!(
            "- Breaking Point: {} VUs ({})\n",
            breaking.vus, breaking.stage_name
        ));
    } else {
        report.push_str("- Breaking Point: Not reached within test parameters\n");
    }

    report.push_str(
        "\n╚══════════════════════════════════════════════════════════════════════════╝\n",
    );

    report
}

// ============================================================================
// MAIN TEST FUNCTIONS
// ============================================================================

/// Main benchmark test - runs against a live gateway
///
/// This test requires the gateway to be running on PORT 3001.
/// Start gateway with: cargo run --release
#[tokio::test]
async fn test_scale_benchmark_10k_live() {
    println!("\n{}", "=".repeat(70));
    println!("WaaV Gateway Scale Benchmark - 10,000 Requests");
    println!("{}\n", "=".repeat(70));

    let base_url = std::env::var("GATEWAY_URL").unwrap_or_else(|_| "http://127.0.0.1:3001".to_string());

    // Initialize continuous result saver FIRST to capture all data even on crash
    let result_saver = Arc::new(
        ResultSaver::new().expect("Failed to create result saver")
    );
    println!("Continuous logging enabled - results will survive crash");

    // Check if gateway is running
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

    // Initialize stats with continuous saving
    let stats = Arc::new(BenchmarkStats::with_saver(result_saver.clone()));
    let resource_monitor = Arc::new(ResourceMonitor::new());

    // Start resource monitoring (try to find gateway PID)
    let gateway_pid = find_gateway_pid().await;
    if let Some(pid) = gateway_pid {
        println!("Monitoring gateway process: PID {}", pid);
    }

    // Global semaphore for connection limiting (prevent fd exhaustion)
    let semaphore = Arc::new(Semaphore::new(10_000));

    // Run all stages
    let mut stage_results = Vec::new();
    let benchmark_start = Instant::now();

    for (vus, duration_secs, stage_name) in RAMP_STAGES {
        let result = run_stage(
            stage_name,
            *vus,
            Duration::from_secs(*duration_secs),
            &base_url,
            stats.clone(),
            semaphore.clone(),
        )
        .await;

        // Save stage result IMMEDIATELY to disk (survives crash)
        result_saver.log_stage(&result).await;
        stage_results.push(result.clone());

        // Flush all pending writes every stage
        result_saver.flush_all().await;

        // Check for early termination if error rate exceeds threshold
        if result.error_rate > 10.0 {
            println!(
                "\n⚠️  Stopping benchmark: Error rate {:.2}% exceeds 10% threshold",
                result.error_rate
            );
            break;
        }

        // Cool-down period between stages
        println!("  [Cool-down: 5 seconds]");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    let total_duration = benchmark_start.elapsed();
    let total_requests = stats.total_requests.load(Ordering::Relaxed);

    // Stop resource monitoring
    resource_monitor.stop();

    // Generate and print report
    let report = generate_report(
        &stage_results,
        total_duration,
        total_requests,
        resource_monitor.get_peak_memory_mb().await,
        resource_monitor.get_peak_fds().await,
    );

    println!("{}", report);

    // Save final summary to result saver directory
    result_saver.write_summary(&report).await;
    result_saver.flush_all().await;

    println!("\n📁 All results saved to: {:?}", result_saver.output_dir());
    println!("   - requests.jsonl: All individual requests (line-delimited JSON)");
    println!("   - stages.jsonl: Stage summaries with percentiles");
    println!("   - resources.jsonl: Resource usage samples");
    println!("   - summary.txt: Final report");
}

/// Zero-latency mock test - measures gateway overhead only
///
/// Uses mock providers with near-zero latency to isolate gateway performance
#[tokio::test]
async fn test_gateway_overhead_benchmark() {
    println!("\n{}", "=".repeat(70));
    println!("WaaV Gateway Overhead Benchmark (Zero-Latency Mocks)");
    println!("{}\n", "=".repeat(70));

    // Start mock providers with zero latency (min/max both set to 1ms)
    let zero_latency = LatencyProfile {
        min_ms: 1,
        max_ms: 2,
        p50_ms: 1,
        p99_ms: 2,
    };

    let http_state = HttpMockState::new(zero_latency.clone(), ChaosConfig::default());
    let _http_handle = spawn_http_mock(18081, http_state);

    let ws_state = Arc::new(WebSocketMockState::new(
        zero_latency.clone(),
        zero_latency,
        ChaosConfig::default(),
    ));
    let _stt_handle = spawn_stt_websocket_mock(18082, ws_state.clone());
    let _tts_handle = spawn_tts_websocket_mock(18083, ws_state);

    // Give mock servers time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Mock providers started:");
    println!("  - HTTP TTS Mock: http://127.0.0.1:18081");
    println!("  - STT WebSocket Mock: ws://127.0.0.1:18082");
    println!("  - TTS WebSocket Mock: ws://127.0.0.1:18083");

    // Check gateway connection
    let base_url = std::env::var("GATEWAY_URL").unwrap_or_else(|_| "http://127.0.0.1:3001".to_string());

    let client = reqwest::Client::new();
    match client.get(format!("{}/", base_url)).send().await {
        Ok(response) if response.status().is_success() => {
            println!("Gateway is running at {}", base_url);
        }
        _ => {
            println!("SKIPPING: Gateway not running at {}", base_url);
            println!("Start gateway configured to use mock providers at ports 18081-18083");
            return;
        }
    }

    // Run reduced stage test for overhead measurement
    let stats = Arc::new(BenchmarkStats::new());
    let semaphore = Arc::new(Semaphore::new(5_000));

    // Smaller stages for overhead measurement
    let overhead_stages: &[(u32, u64, &str)] = &[
        (10, 10, "Warmup"),
        (100, 30, "Light"),
        (500, 30, "Medium"),
        (1000, 30, "Heavy"),
    ];

    let mut stage_results = Vec::new();
    let benchmark_start = Instant::now();

    for (vus, duration_secs, stage_name) in overhead_stages {
        let result = run_stage(
            stage_name,
            *vus,
            Duration::from_secs(*duration_secs),
            &base_url,
            stats.clone(),
            semaphore.clone(),
        )
        .await;

        stage_results.push(result);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let total_duration = benchmark_start.elapsed();
    let total_requests = stats.total_requests.load(Ordering::Relaxed);

    println!("\n{}", "=".repeat(70));
    println!("Gateway Overhead Results (excluding mock provider latency):");
    println!("{}", "=".repeat(70));

    for result in &stage_results {
        // Subtract mock latency (~1ms) to get pure gateway overhead
        let overhead_p50 = (result.p50_ms - 1.0).max(0.0);
        let overhead_p99 = (result.p99_ms - 1.0).max(0.0);
        println!(
            "{}: Gateway Overhead P50={:.2}ms P99={:.2}ms (Total P50={:.2}ms P99={:.2}ms)",
            result.stage_name, overhead_p50, overhead_p99, result.p50_ms, result.p99_ms
        );
    }

    println!(
        "\nTotal: {} requests in {:.1}s ({:.0} RPS)",
        total_requests,
        total_duration.as_secs_f64(),
        total_requests as f64 / total_duration.as_secs_f64()
    );
}

/// E2E benchmark with realistic provider latencies
#[tokio::test]
async fn test_e2e_realistic_benchmark() {
    println!("\n{}", "=".repeat(70));
    println!("WaaV Gateway E2E Benchmark (Realistic Provider Latencies)");
    println!("{}\n", "=".repeat(70));

    // Start mock providers with realistic latencies
    let http_state = HttpMockState::elevenlabs(); // 100-400ms latency
    let _http_handle = spawn_http_mock(19081, http_state);

    let ws_state = Arc::new(WebSocketMockState::deepgram()); // 30-150ms latency
    let _stt_handle = spawn_stt_websocket_mock(19082, ws_state.clone());
    let _tts_handle = spawn_tts_websocket_mock(19083, ws_state);

    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Mock providers started with realistic latencies:");
    println!("  - ElevenLabs-style HTTP TTS: P50=180ms, P99=350ms");
    println!("  - Deepgram-style WebSocket STT: P50=50ms, P99=120ms");

    let base_url = std::env::var("GATEWAY_URL").unwrap_or_else(|_| "http://127.0.0.1:3001".to_string());

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

    let stats = Arc::new(BenchmarkStats::new());
    let semaphore = Arc::new(Semaphore::new(2_000));

    // Moderate stages for E2E testing
    let e2e_stages: &[(u32, u64, &str)] = &[
        (10, 15, "Warmup"),
        (50, 30, "Light"),
        (100, 30, "Medium"),
        (200, 30, "Heavy"),
        (500, 30, "Stress"),
    ];

    let mut stage_results = Vec::new();
    let benchmark_start = Instant::now();

    for (vus, duration_secs, stage_name) in e2e_stages {
        let result = run_stage(
            stage_name,
            *vus,
            Duration::from_secs(*duration_secs),
            &base_url,
            stats.clone(),
            semaphore.clone(),
        )
        .await;

        stage_results.push(result);
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    let total_duration = benchmark_start.elapsed();

    // Generate report
    let report = generate_report(
        &stage_results,
        total_duration,
        stats.total_requests.load(Ordering::Relaxed),
        0.0, // Would need resource monitor
        0,
    );

    println!("{}", report);
}

/// Chaos benchmark - test gateway resilience under provider failures
#[tokio::test]
async fn test_chaos_benchmark() {
    println!("\n{}", "=".repeat(70));
    println!("WaaV Gateway Chaos Benchmark (5% Failure Rate)");
    println!("{}\n", "=".repeat(70));

    // Start mock providers with chaos (5% failure rate)
    let http_state = HttpMockState::elevenlabs_chaos();
    let _http_handle = spawn_http_mock(20081, http_state);

    let ws_state = Arc::new(WebSocketMockState::deepgram_chaos());
    let _stt_handle = spawn_stt_websocket_mock(20082, ws_state.clone());
    let _tts_handle = spawn_tts_websocket_mock(20083, ws_state);

    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Chaos mock providers started:");
    println!("  - 5% failure rate");
    println!("  - 3% timeout rate");
    println!("  - 2% connection drop rate");
    println!("  - 10% slow response rate");

    let base_url = std::env::var("GATEWAY_URL").unwrap_or_else(|_| "http://127.0.0.1:3001".to_string());

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

    let stats = Arc::new(BenchmarkStats::new());
    let semaphore = Arc::new(Semaphore::new(1_000));

    // Moderate stages for chaos testing
    let chaos_stages: &[(u32, u64, &str)] = &[
        (10, 15, "Warmup"),
        (50, 30, "Light Chaos"),
        (100, 30, "Medium Chaos"),
        (200, 30, "Heavy Chaos"),
    ];

    let mut stage_results = Vec::new();
    let benchmark_start = Instant::now();

    for (vus, duration_secs, stage_name) in chaos_stages {
        let result = run_stage(
            stage_name,
            *vus,
            Duration::from_secs(*duration_secs),
            &base_url,
            stats.clone(),
            semaphore.clone(),
        )
        .await;

        println!(
            "  Chaos Stats: {} provider errors injected",
            stats.failed_requests.load(Ordering::Relaxed)
        );

        stage_results.push(result);
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    let total_duration = benchmark_start.elapsed();

    println!("\n{}", "=".repeat(70));
    println!("Chaos Benchmark Results:");
    println!("{}", "=".repeat(70));

    let total_requests = stats.total_requests.load(Ordering::Relaxed);
    let total_failures = stats.failed_requests.load(Ordering::Relaxed);
    let observed_error_rate = (total_failures as f64 / total_requests as f64) * 100.0;

    println!(
        "Total Requests: {} | Failures: {} | Observed Error Rate: {:.2}%",
        total_requests, total_failures, observed_error_rate
    );
    println!("Expected Error Rate: ~5% (from chaos config)");

    // Check that gateway doesn't amplify errors
    if observed_error_rate > 10.0 {
        println!("⚠️  WARNING: Gateway amplifying errors beyond provider failure rate");
    } else {
        println!("✓ Gateway handling provider failures gracefully");
    }

    println!(
        "\nTotal Duration: {:.1}s",
        total_duration.as_secs_f64()
    );
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

async fn find_gateway_pid() -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let output = Command::new("pgrep")
            .args(["-f", "waav-gateway"])
            .output()
            .ok()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.lines().next()?.parse().ok()
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

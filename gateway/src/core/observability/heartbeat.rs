//! Pipeline liveness heartbeat (PIPECAT_FIX_PLAN D-G10).
//!
//! Pipecat injects a `HeartbeatFrame` through the pipeline every
//! `heartbeats_period_secs` and WARNS (never kills) if it doesn't traverse
//! within a timeout — a liveness probe distinct from per-turn timing.
//!
//! WaaV is callback-driven (STT result → orchestrator → LLM → TTS), not a
//! frame pipeline, so there is no sentinel frame that visibly traverses
//! every stage. The faithful adaptation: a per-session task that periodically
//! runs a cheap LIVENESS PROBE — "are the audio providers responsive?" —
//! bounded by a timeout. On success it records the probe round-trip
//! (`waav_pipeline_heartbeat_ms`); on timeout/not-ready it WARNS and counts a
//! miss (`waav_pipeline_heartbeat_misses_total`). It never tears anything
//! down — purely observability. Config-gated, OFF by default.
//!
//! The probe is supplied by the caller as an async closure so the heartbeat
//! stays decoupled from the VoiceManager (and trivially testable); the
//! production wiring probes `VoiceManager::is_ready()`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// A liveness probe: resolves `true` when the pipeline is responsive.
pub type LivenessProbe = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// Per-session heartbeat configuration.
#[derive(Clone, Copy, Debug)]
pub struct HeartbeatConfig {
    /// How often to probe. `0` = disabled (the default).
    pub period: Duration,
    /// Probe must complete (and report ready) within this budget.
    pub timeout: Duration,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            period: Duration::ZERO,
            timeout: Duration::from_secs(5),
        }
    }
}

impl HeartbeatConfig {
    pub fn enabled(&self) -> bool {
        !self.period.is_zero()
    }
}

/// Owns the heartbeat task; aborts it on drop (audited like any session task).
pub struct HeartbeatMonitor {
    handle: Option<JoinHandle<()>>,
}

impl HeartbeatMonitor {
    /// Spawn the heartbeat loop. Returns a no-op monitor when disabled (zero
    /// period) — no task is spawned.
    pub fn spawn(config: HeartbeatConfig, session_id: String, probe: LivenessProbe) -> Self {
        if !config.enabled() {
            return Self { handle: None };
        }
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(config.period);
            // The first tick fires immediately; skip it so we don't probe at t0.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let started = std::time::Instant::now();
                match tokio::time::timeout(config.timeout, probe()).await {
                    Ok(true) => {
                        let ms = started.elapsed().as_secs_f64() * 1000.0;
                        debug!(session = %session_id, heartbeat_ms = ms, "pipeline heartbeat ok");
                        crate::core::metrics::bridge::observe_pipeline_heartbeat_ms(ms);
                    }
                    Ok(false) => {
                        warn!(session = %session_id, "pipeline heartbeat MISS: a stage is not ready");
                        crate::core::metrics::bridge::record_pipeline_heartbeat_miss();
                    }
                    Err(_) => {
                        warn!(
                            session = %session_id,
                            timeout_ms = config.timeout.as_millis(),
                            "pipeline heartbeat MISS: probe timed out (wedged stage?)"
                        );
                        crate::core::metrics::bridge::record_pipeline_heartbeat_miss();
                    }
                }
            }
        });
        Self {
            handle: Some(handle),
        }
    }

    /// Whether a heartbeat task is running (test/observability).
    pub fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for HeartbeatMonitor {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn heartbeat_off_by_default() {
        let cfg = HeartbeatConfig::default();
        assert!(!cfg.enabled());
        let probed = Arc::new(AtomicUsize::new(0));
        let p = Arc::clone(&probed);
        let probe: LivenessProbe = Arc::new(move || {
            let p = Arc::clone(&p);
            Box::pin(async move {
                p.fetch_add(1, Ordering::SeqCst);
                true
            })
        });
        let mon = HeartbeatMonitor::spawn(cfg, "s".into(), probe);
        assert!(!mon.is_running(), "disabled ⇒ no task spawned");
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(probed.load(Ordering::SeqCst), 0, "disabled ⇒ never probes");
    }

    #[tokio::test]
    async fn heartbeat_traverses_and_reports() {
        let probed = Arc::new(AtomicUsize::new(0));
        let p = Arc::clone(&probed);
        let probe: LivenessProbe = Arc::new(move || {
            let p = Arc::clone(&p);
            Box::pin(async move {
                p.fetch_add(1, Ordering::SeqCst);
                true
            })
        });
        let cfg = HeartbeatConfig {
            period: Duration::from_millis(20),
            timeout: Duration::from_secs(1),
        };
        let mon = HeartbeatMonitor::spawn(cfg, "s".into(), probe);
        assert!(mon.is_running());
        tokio::time::sleep(Duration::from_millis(90)).await;
        assert!(
            probed.load(Ordering::SeqCst) >= 2,
            "the probe runs each period"
        );
        drop(mon);
    }

    #[tokio::test]
    async fn wedged_stage_triggers_miss_and_never_kills() {
        // A probe that never returns must time out (miss), and the monitor
        // keeps running (warn, never kill).
        let probe: LivenessProbe = Arc::new(|| {
            Box::pin(async move {
                std::future::pending::<()>().await;
                true
            })
        });
        let cfg = HeartbeatConfig {
            period: Duration::from_millis(20),
            timeout: Duration::from_millis(15),
        };
        let mon = HeartbeatMonitor::spawn(cfg, "s".into(), probe);
        tokio::time::sleep(Duration::from_millis(90)).await;
        assert!(
            mon.is_running(),
            "a wedged stage warns but never kills the monitor"
        );
    }
}

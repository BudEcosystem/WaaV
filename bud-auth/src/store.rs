//! The control-plane store abstraction.
//!
//! Everything that reads Bud state goes through this trait so the hydration, keyspace and
//! authorization logic is testable without a live Redis — including the failure modes that
//! matter most (a dropped connection, a transient error, a corrupt value), which are precisely
//! the ones a live-server test is worst at reproducing on demand.

use std::collections::HashMap;

/// Why a store read did not return a value.
///
/// The distinction is load-bearing: **absent** is authoritative and may be cached, while
/// **unavailable** is transient and must never be. Collapsing the two turns a momentary wobble
/// into a cached denial — the "unavailable is not absent" rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The backend could not be reached, or failed. Retryable; never cache a conclusion.
    Unavailable(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "store unavailable: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Read access to the Bud control plane.
#[async_trait::async_trait]
pub trait ControlPlaneStore: Send + Sync {
    /// One key. `Ok(None)` means authoritatively absent.
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError>;

    /// Every key matching a glob, with its value. Used only by hydration, never per request.
    async fn scan(&self, pattern: &str) -> Result<HashMap<String, String>, StoreError>;
}

/// An in-memory store for tests and for WaaV's standalone mode.
#[derive(Default)]
pub struct MemoryStore {
    data: std::sync::Mutex<HashMap<String, String>>,
    /// When set, every call fails — used to exercise the transient-failure paths.
    down: std::sync::atomic::AtomicBool,
    pub gets: std::sync::atomic::AtomicUsize,
    pub scans: std::sync::atomic::AtomicUsize,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, key: &str, value: &str) {
        #[expect(clippy::unwrap_used, reason = "test-only store")]
        self.data.lock().unwrap().insert(key.into(), value.into());
    }

    pub fn remove(&self, key: &str) {
        #[expect(clippy::unwrap_used, reason = "test-only store")]
        self.data.lock().unwrap().remove(key);
    }

    /// Simulate a connection loss. Reads fail with `Unavailable` until restored.
    pub fn set_down(&self, down: bool) {
        self.down.store(down, std::sync::atomic::Ordering::SeqCst);
    }

    fn is_down(&self) -> bool {
        self.down.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ControlPlaneStore for MemoryStore {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.gets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.is_down() {
            return Err(StoreError::Unavailable("connection down".into()));
        }
        #[expect(clippy::unwrap_used, reason = "test-only store")]
        Ok(self.data.lock().unwrap().get(key).cloned())
    }

    async fn scan(&self, pattern: &str) -> Result<HashMap<String, String>, StoreError> {
        self.scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.is_down() {
            return Err(StoreError::Unavailable("connection down".into()));
        }
        let prefix = pattern.trim_end_matches('*');
        #[expect(clippy::unwrap_used, reason = "test-only store")]
        let data = self.data.lock().unwrap();
        Ok(data
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

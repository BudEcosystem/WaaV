//! DAG execution context
//!
//! This module provides the execution context that is passed through DAG nodes
//! during execution. It contains authentication information, timing data,
//! node results, and cancellation support.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Context passed through DAG execution
///
/// This context is cloned for each branch during Split operations but shares
/// the same cancellation token for coordinated cancellation.
#[derive(Clone)]
pub struct DAGContext {
    /// Stream ID for this connection (from WebSocket)
    pub stream_id: String,

    /// Authenticated API key (bearer token)
    pub api_key: Option<String>,

    /// API key ID (tenant identifier from auth)
    pub api_key_id: Option<String>,

    /// Custom metadata for routing and processing
    pub metadata: HashMap<String, String>,

    /// Current node results (for condition evaluation)
    /// Key: node_id, Value: result from that node
    node_results: HashMap<String, Arc<dyn Any + Send + Sync>>,

    /// External resources (e.g., LiveKitClient, VoiceManager)
    /// Shared across branches using Arc for thread-safe access
    external_resources: Arc<HashMap<String, Arc<dyn Any + Send + Sync>>>,

    /// Timing information for performance tracking
    pub timing: DAGTiming,

    /// Cancellation token for graceful shutdown
    pub cancel_token: CancellationToken,

    /// Maximum execution time allowed (optional)
    pub deadline: Option<Instant>,
}

/// Resource keys for external resources stored in DAGContext
pub mod resource_keys {
    /// Key for LiveKitClient in external_resources
    pub const LIVEKIT_CLIENT: &str = "livekit_client";
    /// Key for VoiceManager in external_resources
    pub const VOICE_MANAGER: &str = "voice_manager";
    /// Key prefix for realtime providers (format: "realtime_provider:{provider_name}")
    pub const REALTIME_PROVIDER_PREFIX: &str = "realtime_provider:";
}

impl DAGContext {
    /// Create a new DAG context with minimal configuration
    pub fn new(stream_id: impl Into<String>) -> Self {
        Self {
            stream_id: stream_id.into(),
            api_key: None,
            api_key_id: None,
            metadata: HashMap::new(),
            node_results: HashMap::new(),
            external_resources: Arc::new(HashMap::new()),
            timing: DAGTiming::new(),
            cancel_token: CancellationToken::new(),
            deadline: None,
        }
    }

    /// Create context with API key authentication
    pub fn with_auth(
        stream_id: impl Into<String>,
        api_key: Option<String>,
        api_key_id: Option<String>,
    ) -> Self {
        Self {
            stream_id: stream_id.into(),
            api_key,
            api_key_id,
            metadata: HashMap::new(),
            node_results: HashMap::new(),
            external_resources: Arc::new(HashMap::new()),
            timing: DAGTiming::new(),
            cancel_token: CancellationToken::new(),
            deadline: None,
        }
    }

    /// Create context with external resources
    pub fn with_resources(
        stream_id: impl Into<String>,
        resources: HashMap<String, Arc<dyn Any + Send + Sync>>,
    ) -> Self {
        Self {
            stream_id: stream_id.into(),
            api_key: None,
            api_key_id: None,
            metadata: HashMap::new(),
            node_results: HashMap::new(),
            external_resources: Arc::new(resources),
            timing: DAGTiming::new(),
            cancel_token: CancellationToken::new(),
            deadline: None,
        }
    }

    /// Add external resources to context
    pub fn with_external_resources(
        mut self,
        resources: HashMap<String, Arc<dyn Any + Send + Sync>>,
    ) -> Self {
        self.external_resources = Arc::new(resources);
        self
    }

    /// Set a single external resource
    pub fn set_resource<T: Any + Send + Sync>(
        &mut self,
        key: impl Into<String>,
        resource: Arc<T>,
    ) {
        // We need to get mutable access to the resources
        // Since Arc is shared, we need to clone and replace
        let mut resources = (*self.external_resources).clone();
        resources.insert(key.into(), resource as Arc<dyn Any + Send + Sync>);
        self.external_resources = Arc::new(resources);
    }

    /// Get an external resource by key
    pub fn get_resource(&self, key: &str) -> Option<&Arc<dyn Any + Send + Sync>> {
        self.external_resources.get(key)
    }

    /// Get a typed external resource
    pub fn get_resource_as<T: 'static + Send + Sync>(&self, key: &str) -> Option<Arc<T>> {
        self.external_resources.get(key).and_then(|arc| {
            // Try to downcast the Arc<dyn Any> to Arc<T>
            arc.clone().downcast::<T>().ok()
        })
    }

    /// Check if an external resource exists
    pub fn has_resource(&self, key: &str) -> bool {
        self.external_resources.contains_key(key)
    }

    /// Set the execution deadline
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Set deadline from duration
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.deadline = Some(Instant::now() + timeout);
        self
    }

    /// Set custom cancellation token
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
        self
    }

    /// Add metadata to context
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Store a node's result in the context
    pub fn set_node_result<T: Any + Send + Sync>(&mut self, node_id: impl Into<String>, result: T) {
        self.node_results.insert(node_id.into(), Arc::new(result));
    }

    /// Store a node's result as Arc (avoids extra allocation)
    pub fn set_node_result_arc(
        &mut self,
        node_id: impl Into<String>,
        result: Arc<dyn Any + Send + Sync>,
    ) {
        self.node_results.insert(node_id.into(), result);
    }

    /// Get a node's result by ID
    pub fn get_node_result(&self, node_id: &str) -> Option<&Arc<dyn Any + Send + Sync>> {
        self.node_results.get(node_id)
    }

    /// Get a typed node result
    pub fn get_node_result_as<T: 'static>(&self, node_id: &str) -> Option<&T> {
        self.node_results
            .get(node_id)
            .and_then(|arc| arc.downcast_ref::<T>())
    }

    /// Check if a node's result exists
    pub fn has_node_result(&self, node_id: &str) -> bool {
        self.node_results.contains_key(node_id)
    }

    /// Clear all node results
    pub fn clear_results(&mut self) {
        self.node_results.clear();
    }

    /// Get all node result keys
    pub fn result_keys(&self) -> impl Iterator<Item = &String> {
        self.node_results.keys()
    }

    /// Check if execution has been cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Check if deadline has passed
    pub fn is_deadline_exceeded(&self) -> bool {
        self.deadline.map(|d| Instant::now() > d).unwrap_or(false)
    }

    /// Check if execution should continue
    pub fn should_continue(&self) -> bool {
        !self.is_cancelled() && !self.is_deadline_exceeded()
    }

    /// Get remaining time until deadline (if set)
    pub fn remaining_time(&self) -> Option<Duration> {
        self.deadline.and_then(|d| d.checked_duration_since(Instant::now()))
    }

    /// Record node execution start
    pub fn record_node_start(&mut self, node_id: &str) {
        self.timing.node_starts.insert(node_id.to_string(), Instant::now());
    }

    /// Record node execution end
    pub fn record_node_end(&mut self, node_id: &str) {
        if let Some(start) = self.timing.node_starts.get(node_id) {
            let duration = start.elapsed();
            self.timing.node_durations.insert(node_id.to_string(), duration);
        }
    }

    /// Clone for branch execution (shares cancel token and external resources, clears results)
    pub fn clone_for_branch(&self) -> Self {
        Self {
            stream_id: self.stream_id.clone(),
            api_key: self.api_key.clone(),
            api_key_id: self.api_key_id.clone(),
            metadata: self.metadata.clone(),
            node_results: HashMap::new(), // Fresh results for branch
            external_resources: self.external_resources.clone(), // Share external resources
            timing: DAGTiming::new(),
            cancel_token: self.cancel_token.clone(), // Share cancellation
            deadline: self.deadline,
        }
    }

    /// Get external resources map (for debugging or iteration)
    pub fn external_resources(&self) -> &HashMap<String, Arc<dyn Any + Send + Sync>> {
        &self.external_resources
    }
}

impl std::fmt::Debug for DAGContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DAGContext")
            .field("stream_id", &self.stream_id)
            .field("api_key_id", &self.api_key_id)
            .field("metadata", &self.metadata)
            .field("node_results", &self.node_results.keys().collect::<Vec<_>>())
            .field("external_resources", &self.external_resources.keys().collect::<Vec<_>>())
            .field("is_cancelled", &self.is_cancelled())
            .field("deadline", &self.deadline)
            .finish()
    }
}

/// Timing information for DAG execution
#[derive(Clone, Debug)]
pub struct DAGTiming {
    /// When execution started
    pub start_time: Instant,

    /// Node start times
    pub(crate) node_starts: HashMap<String, Instant>,

    /// Node execution durations
    pub node_durations: HashMap<String, Duration>,

    /// Total execution count (for statistics)
    pub execution_count: u64,
}

impl DAGTiming {
    /// Create new timing tracker
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            node_starts: HashMap::new(),
            node_durations: HashMap::new(),
            execution_count: 0,
        }
    }

    /// Reset timing for new execution
    pub fn reset(&mut self) {
        self.start_time = Instant::now();
        self.node_starts.clear();
        self.node_durations.clear();
        self.execution_count += 1;
    }

    /// Get total execution duration
    pub fn total_duration(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get node execution duration
    pub fn get_node_duration(&self, node_id: &str) -> Option<Duration> {
        self.node_durations.get(node_id).copied()
    }

    /// Get all node durations sorted by duration (descending)
    pub fn sorted_durations(&self) -> Vec<(&String, Duration)> {
        let mut durations: Vec<_> = self.node_durations.iter().map(|(k, v)| (k, *v)).collect();
        durations.sort_by(|a, b| b.1.cmp(&a.1));
        durations
    }

    /// Get the slowest node
    pub fn slowest_node(&self) -> Option<(&String, Duration)> {
        self.node_durations
            .iter()
            .max_by_key(|(_, v)| *v)
            .map(|(k, v)| (k, *v))
    }

    /// Calculate average node duration
    pub fn average_duration(&self) -> Option<Duration> {
        if self.node_durations.is_empty() {
            return None;
        }
        let total: Duration = self.node_durations.values().sum();
        Some(total / self.node_durations.len() as u32)
    }
}

impl Default for DAGTiming {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn test_context_creation() {
        let ctx = DAGContext::new("stream-123");
        assert_eq!(ctx.stream_id, "stream-123");
        assert!(ctx.api_key.is_none());
        assert!(!ctx.is_cancelled());
    }

    #[test]
    fn test_context_with_auth() {
        let ctx = DAGContext::with_auth(
            "stream-123",
            Some("secret".to_string()),
            Some("tenant-1".to_string()),
        );
        assert_eq!(ctx.api_key.as_deref(), Some("secret"));
        assert_eq!(ctx.api_key_id.as_deref(), Some("tenant-1"));
    }

    #[test]
    fn test_node_results() {
        let mut ctx = DAGContext::new("stream-123");

        ctx.set_node_result("stt_node", "Hello World".to_string());
        assert!(ctx.has_node_result("stt_node"));

        let result = ctx.get_node_result_as::<String>("stt_node");
        assert_eq!(result, Some(&"Hello World".to_string()));
    }

    #[test]
    fn test_deadline() {
        let ctx = DAGContext::new("stream-123")
            .with_timeout(Duration::from_millis(10));

        assert!(!ctx.is_deadline_exceeded());
        sleep(Duration::from_millis(20));
        assert!(ctx.is_deadline_exceeded());
        assert!(!ctx.should_continue());
    }

    #[test]
    fn test_cancellation() {
        let ctx = DAGContext::new("stream-123");
        assert!(!ctx.is_cancelled());
        assert!(ctx.should_continue());

        ctx.cancel_token.cancel();
        assert!(ctx.is_cancelled());
        assert!(!ctx.should_continue());
    }

    #[test]
    fn test_timing() {
        let mut timing = DAGTiming::new();

        // Simulate node execution
        timing.node_starts.insert("node1".to_string(), Instant::now());
        sleep(Duration::from_millis(10));
        timing.node_durations.insert("node1".to_string(), Duration::from_millis(10));

        timing.node_starts.insert("node2".to_string(), Instant::now());
        sleep(Duration::from_millis(5));
        timing.node_durations.insert("node2".to_string(), Duration::from_millis(5));

        let (slowest_id, _) = timing.slowest_node().unwrap();
        assert_eq!(slowest_id, "node1");
    }

    #[test]
    fn test_clone_for_branch() {
        let mut ctx = DAGContext::new("stream-123")
            .with_metadata("key1", "value1");
        ctx.set_node_result("node1", 42i32);

        let branch_ctx = ctx.clone_for_branch();

        // Should share these
        assert_eq!(branch_ctx.stream_id, ctx.stream_id);
        assert_eq!(branch_ctx.metadata.get("key1"), Some(&"value1".to_string()));

        // Should have fresh results
        assert!(!branch_ctx.has_node_result("node1"));

        // Cancellation should be shared
        ctx.cancel_token.cancel();
        assert!(branch_ctx.is_cancelled());
    }
}

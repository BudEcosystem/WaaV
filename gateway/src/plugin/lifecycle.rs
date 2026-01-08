//! Plugin Lifecycle Management
//!
//! This module defines the plugin lifecycle states and transitions.
//! Plugins go through a defined lifecycle from discovery to shutdown.
//!
//! # Lifecycle State Machine
//!
//! ```text
//!     +-------------+
//!     | Discovered  |  (inventory::collect!)
//!     +------+------+
//!            |
//!            v
//!     +------+------+
//!     | Registered  |  (dependencies checked)
//!     +------+------+
//!            |
//!            v
//!     +------+------+
//!     | Initializing|  (init() called)
//!     +------+------+
//!            |
//!     +------+------+
//!     |             |
//!     v             v
//! +---+---+    +----+----+
//! | Ready |    |  Failed |
//! +---+---+    +---------+
//!     |
//!     v
//! +---+---+
//! | Running |  (start() called)
//! +---+---+
//!     |
//!     v
//! +---+---+
//! | Stopping |  (shutdown() called)
//! +---+---+
//!     |
//!     v
//! +---+---+
//! | Stopped |
//! +---------+
//! ```

use async_trait::async_trait;
use std::time::Instant;

use super::isolation::PluginError;
use super::metadata::PluginManifest;
use crate::config::ServerConfig;
use std::sync::Arc;

/// Plugin lifecycle state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    /// Plugin has been discovered but not yet registered
    Discovered,

    /// Plugin has been registered and dependencies verified
    Registered,

    /// Plugin is being initialized
    Initializing,

    /// Plugin is ready but not yet started
    Ready,

    /// Plugin is running and processing requests
    Running,

    /// Plugin is being stopped
    Stopping,

    /// Plugin has been stopped
    Stopped,

    /// Plugin failed to initialize or crashed
    Failed,
}

impl std::fmt::Display for PluginState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginState::Discovered => write!(f, "discovered"),
            PluginState::Registered => write!(f, "registered"),
            PluginState::Initializing => write!(f, "initializing"),
            PluginState::Ready => write!(f, "ready"),
            PluginState::Running => write!(f, "running"),
            PluginState::Stopping => write!(f, "stopping"),
            PluginState::Stopped => write!(f, "stopped"),
            PluginState::Failed => write!(f, "failed"),
        }
    }
}

impl PluginState {
    /// Check if the plugin is in a healthy state
    pub fn is_healthy(&self) -> bool {
        matches!(self, PluginState::Ready | PluginState::Running)
    }

    /// Check if the plugin can accept requests
    pub fn can_process(&self) -> bool {
        matches!(self, PluginState::Running)
    }

    /// Check if the plugin can be started
    pub fn can_start(&self) -> bool {
        matches!(self, PluginState::Ready)
    }

    /// Check if the plugin can be stopped
    pub fn can_stop(&self) -> bool {
        matches!(self, PluginState::Running)
    }
}

/// Plugin health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PluginHealth {
    /// Plugin is functioning normally
    Healthy,

    /// Plugin is experiencing degraded performance
    Degraded,

    /// Plugin is not functioning correctly
    Unhealthy,

    /// Plugin health is unknown
    #[default]
    Unknown,
}

impl std::fmt::Display for PluginHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginHealth::Healthy => write!(f, "healthy"),
            PluginHealth::Degraded => write!(f, "degraded"),
            PluginHealth::Unhealthy => write!(f, "unhealthy"),
            PluginHealth::Unknown => write!(f, "unknown"),
        }
    }
}

/// Context provided to plugins during lifecycle operations
#[derive(Clone)]
pub struct PluginContext {
    /// Gateway configuration (read-only)
    pub gateway_config: Arc<ServerConfig>,

    /// Plugin-specific configuration (from plugins section in config)
    pub config: serde_json::Value,

    /// Plugin ID
    pub plugin_id: String,
}

impl PluginContext {
    /// Create a new plugin context
    pub fn new(
        gateway_config: Arc<ServerConfig>,
        plugin_id: impl Into<String>,
        config: serde_json::Value,
    ) -> Self {
        Self {
            gateway_config,
            plugin_id: plugin_id.into(),
            config,
        }
    }

    /// Create a context with default (empty) plugin config
    pub fn with_default_config(
        gateway_config: Arc<ServerConfig>,
        plugin_id: impl Into<String>,
    ) -> Self {
        Self::new(gateway_config, plugin_id, serde_json::Value::Null)
    }
}

/// Plugin lifecycle trait
///
/// Plugins can optionally implement this trait to participate in
/// the gateway's lifecycle management. This is useful for plugins
/// that need to initialize resources, connect to external services,
/// or perform cleanup on shutdown.
#[async_trait]
pub trait PluginLifecycle: Send + Sync {
    /// Returns the plugin manifest
    fn manifest(&self) -> &PluginManifest;

    /// Plugin initialization
    ///
    /// Called once when the plugin is loaded. Use this to:
    /// - Parse configuration
    /// - Validate settings
    /// - Initialize resources
    ///
    /// Return an error to prevent the plugin from starting.
    async fn init(&mut self, ctx: &PluginContext) -> Result<(), PluginError> {
        let _ = ctx;
        Ok(())
    }

    /// Plugin startup
    ///
    /// Called when the gateway starts serving requests. Use this to:
    /// - Connect to external services
    /// - Start background tasks
    /// - Register event handlers
    async fn start(&mut self, ctx: &PluginContext) -> Result<(), PluginError> {
        let _ = ctx;
        Ok(())
    }

    /// Plugin shutdown
    ///
    /// Called during graceful shutdown. Use this to:
    /// - Close connections
    /// - Flush buffers
    /// - Cancel background tasks
    async fn shutdown(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    /// Health check
    ///
    /// Called periodically to check plugin health. Return the current
    /// health status. Default implementation returns Healthy.
    async fn health(&self) -> PluginHealth {
        PluginHealth::Healthy
    }

    /// Get configuration schema (JSON Schema)
    ///
    /// Return a JSON Schema describing the plugin's configuration
    /// for validation and documentation.
    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}

/// Plugin entry in the registry with lifecycle state
#[derive(Debug)]
pub struct PluginEntry {
    /// Current plugin state
    pub state: PluginState,

    /// Time when the plugin was loaded
    pub loaded_at: Instant,

    /// Time when the plugin was last active
    pub last_active: Instant,

    /// Number of times the plugin has been called
    pub call_count: u64,

    /// Number of errors encountered
    pub error_count: u64,

    /// Last error message (if any)
    pub last_error: Option<String>,
}

impl PluginEntry {
    /// Create a new plugin entry
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            state: PluginState::Discovered,
            loaded_at: now,
            last_active: now,
            call_count: 0,
            error_count: 0,
            last_error: None,
        }
    }

    /// Record a successful call
    pub fn record_success(&mut self) {
        self.last_active = Instant::now();
        self.call_count += 1;
    }

    /// Record an error
    pub fn record_error(&mut self, error: impl Into<String>) {
        self.last_active = Instant::now();
        self.error_count += 1;
        self.last_error = Some(error.into());
    }

    /// Transition to a new state
    pub fn transition(&mut self, new_state: PluginState) {
        tracing::debug!(
            from = %self.state,
            to = %new_state,
            "Plugin state transition"
        );
        self.state = new_state;
    }

    /// Get uptime since loading
    pub fn uptime(&self) -> std::time::Duration {
        self.loaded_at.elapsed()
    }

    /// Get time since last activity
    pub fn idle_time(&self) -> std::time::Duration {
        self.last_active.elapsed()
    }
}

impl Default for PluginEntry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_state_display() {
        assert_eq!(format!("{}", PluginState::Running), "running");
        assert_eq!(format!("{}", PluginState::Failed), "failed");
    }

    #[test]
    fn test_plugin_state_transitions() {
        assert!(PluginState::Ready.can_start());
        assert!(!PluginState::Running.can_start());
        assert!(PluginState::Running.can_stop());
        assert!(!PluginState::Stopped.can_stop());
    }

    #[test]
    fn test_plugin_health_display() {
        assert_eq!(format!("{}", PluginHealth::Healthy), "healthy");
        assert_eq!(format!("{}", PluginHealth::Degraded), "degraded");
    }

    #[test]
    fn test_plugin_entry() {
        let mut entry = PluginEntry::new();
        assert_eq!(entry.state, PluginState::Discovered);
        assert_eq!(entry.call_count, 0);

        entry.record_success();
        assert_eq!(entry.call_count, 1);

        entry.record_error("test error");
        assert_eq!(entry.error_count, 1);
        assert!(entry.last_error.as_ref().unwrap().contains("test error"));

        entry.transition(PluginState::Running);
        assert_eq!(entry.state, PluginState::Running);
    }
}

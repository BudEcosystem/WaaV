//! WebSocket connection state management
//!
//! This module handles the state management for WebSocket connections,
//! optimized for low latency with appropriate use of RwLock and atomic types.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::RwLock;

use crate::{
    auth::Auth,
    core::voice_manager::VoiceManager,
    livekit::{LiveKitClient, operations::OperationQueue},
};

#[cfg(feature = "dag-routing")]
use crate::dag::{compiler::CompiledDAG, context::DAGContext, executor::DAGExecutor};

/// WebSocket connection state optimized for low latency
///
/// Uses RwLock for state that changes rarely but is read frequently:
/// - AtomicBool for audio_enabled flag - no locks needed for hot path
/// - RwLock for managers - fast reads, rare writes (only during config)
/// - Optimized for the common case: many reads, few writes
pub struct ConnectionState {
    pub voice_manager: Option<Arc<VoiceManager>>,
    /// D-G10: per-session pipeline liveness heartbeat (env-gated, off by
    /// default). Held here so it is aborted when the connection drops.
    pub heartbeat: Option<crate::core::observability::HeartbeatMonitor>,
    /// D-G4: registry of session-lifetime tasks (sender, LiveKit forwarder, DAG
    /// drain). Audited at teardown — any still running is warned + aborted.
    pub task_tracker: Arc<crate::core::observability::SessionTaskTracker>,
    pub livekit_client: Option<Arc<RwLock<LiveKitClient>>>,
    /// Operation queue for non-blocking LiveKit operations
    pub livekit_operation_queue: Option<OperationQueue>,
    /// Whether audio processing (STT/TTS) is enabled for this connection
    pub audio_enabled: AtomicBool,
    /// Unique identifier for this WebSocket session
    pub stream_id: Option<String>,
    /// LiveKit room name for cleanup operations
    pub livekit_room_name: Option<String>,
    /// Local LiveKit participant identity (WaaV agent)
    pub livekit_local_identity: Option<String>,
    /// Recording egress ID for cleanup operations
    pub recording_egress_id: Option<String>,
    /// Auth context for this connection (used for room name normalization)
    pub auth: Auth,

    /// D8 uplink opus decoder (feature `opus-codec`): `Some` only when the session negotiated
    /// `stt_config.audio_in_codec = opus`. Each client WS binary frame is one opus packet decoded
    /// to PCM16 (at the negotiated rate) BEFORE STT. `tokio::sync::Mutex` for `&mut` access under
    /// the connection read-lock on the audio hot path.
    #[cfg(feature = "opus-codec")]
    pub opus_decoder:
        Option<tokio::sync::Mutex<crate::core::audio::opus_codec::OpusStreamDecoder>>,

    // DAG routing state (feature-gated)
    /// Compiled DAG for this connection
    #[cfg(feature = "dag-routing")]
    pub compiled_dag: Option<Arc<CompiledDAG>>,
    /// DAG executor instance
    #[cfg(feature = "dag-routing")]
    pub dag_executor: Option<Arc<DAGExecutor>>,
    /// DAG execution context (holds auth, timing, node results)
    #[cfg(feature = "dag-routing")]
    pub dag_context: Option<DAGContext>,
    /// Whether DAG routing is enabled for this connection
    #[cfg(feature = "dag-routing")]
    pub dag_enabled: AtomicBool,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionState {
    pub fn new() -> Self {
        Self {
            voice_manager: None,
            heartbeat: None,
            task_tracker: Arc::new(crate::core::observability::SessionTaskTracker::new()),
            livekit_client: None,
            livekit_operation_queue: None,
            audio_enabled: AtomicBool::new(false),
            stream_id: None,
            livekit_room_name: None,
            livekit_local_identity: None,
            recording_egress_id: None,
            auth: Auth::empty(),
            #[cfg(feature = "opus-codec")]
            opus_decoder: None,
            #[cfg(feature = "dag-routing")]
            compiled_dag: None,
            #[cfg(feature = "dag-routing")]
            dag_executor: None,
            #[cfg(feature = "dag-routing")]
            dag_context: None,
            #[cfg(feature = "dag-routing")]
            dag_enabled: AtomicBool::new(false),
        }
    }

    /// Create a new ConnectionState with the given Auth context
    pub fn with_auth(auth: Auth) -> Self {
        Self {
            voice_manager: None,
            heartbeat: None,
            task_tracker: Arc::new(crate::core::observability::SessionTaskTracker::new()),
            livekit_client: None,
            livekit_operation_queue: None,
            audio_enabled: AtomicBool::new(false),
            stream_id: None,
            livekit_room_name: None,
            livekit_local_identity: None,
            recording_egress_id: None,
            auth,
            #[cfg(feature = "opus-codec")]
            opus_decoder: None,
            #[cfg(feature = "dag-routing")]
            compiled_dag: None,
            #[cfg(feature = "dag-routing")]
            dag_executor: None,
            #[cfg(feature = "dag-routing")]
            dag_context: None,
            #[cfg(feature = "dag-routing")]
            dag_enabled: AtomicBool::new(false),
        }
    }

    /// Check if audio processing is enabled
    pub fn is_audio_enabled(&self) -> bool {
        self.audio_enabled.load(Ordering::Relaxed)
    }

    /// Set audio enabled state
    pub fn set_audio_enabled(&self, enabled: bool) {
        self.audio_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Check if DAG routing is enabled
    #[cfg(feature = "dag-routing")]
    pub fn is_dag_enabled(&self) -> bool {
        self.dag_enabled.load(Ordering::Relaxed)
    }

    /// Set DAG enabled state
    #[cfg(feature = "dag-routing")]
    pub fn set_dag_enabled(&self, enabled: bool) {
        self.dag_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Check if DAG routing is enabled (stub when feature disabled)
    #[cfg(not(feature = "dag-routing"))]
    pub fn is_dag_enabled(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_state_new() {
        let state = ConnectionState::new();
        assert!(state.stream_id.is_none());
        assert!(state.voice_manager.is_none());
        assert!(state.livekit_client.is_none());
    }

    #[test]
    fn test_connection_state_stream_id_storage() {
        let mut state = ConnectionState::new();
        assert!(state.stream_id.is_none());

        state.stream_id = Some("test-stream-123".to_string());
        assert_eq!(state.stream_id, Some("test-stream-123".to_string()));
    }

    #[tokio::test]
    async fn task_tracker_is_wired_and_cancels_session_tasks() {
        // D-G4: every ConnectionState owns a task tracker; a recv/sleep-blocked
        // session loop registered on it is cancelled at teardown — and, because
        // it cancels cleanly, is NOT counted as dangling. (The actual-cancel and
        // dangling-detection behaviours are proven in task_tracker's own tests.)
        let state = ConnectionState::new();
        let handle = tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
        state.task_tracker.track("forever", handle);
        assert_eq!(state.task_tracker.tracked_count(), 1);
        assert_eq!(
            state
                .task_tracker
                .abort_and_audit(std::time::Duration::from_millis(100))
                .await,
            0,
            "a cancellable session loop is stopped without being flagged dangling"
        );
        assert_eq!(state.task_tracker.tracked_count(), 0, "audit drains the tracker");
    }

    #[test]
    fn test_connection_state_stream_id_with_uuid() {
        let mut state = ConnectionState::new();

        let uuid = uuid::Uuid::new_v4().to_string();
        state.stream_id = Some(uuid.clone());

        assert_eq!(state.stream_id, Some(uuid));
        // Verify UUID format (36 chars: 8-4-4-4-12)
        assert_eq!(state.stream_id.as_ref().unwrap().len(), 36);
    }
}

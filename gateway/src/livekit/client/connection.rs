// //! Connection lifecycle management for `LiveKitClient`.
//!
//! Provides the asynchronous connect/disconnect flows and lightweight
//! helpers for querying connection-related state.

use std::sync::atomic::Ordering;
use std::time::Duration;

use livekit::prelude::{DataPacketKind, Room, RoomOptions};
use tracing::{debug, error, info, warn};

use super::{LiveKitClient, LiveKitOperation, RELIABLE_BUFFER_THRESHOLD_BYTES};
use crate::AppError;
use crate::core::observability::{abort_and_await_task, await_task_shutdown};

const LIVEKIT_SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

impl LiveKitClient {
    /// Connect to the LiveKit room and bootstrap background workers.
    pub async fn connect(&mut self) -> Result<(), AppError> {
        Self::validate_audio_config(&self.config)?;
        info!("Connecting to LiveKit room with URL: {}", self.config.url);

        match Room::connect(&self.config.url, &self.config.token, RoomOptions::default()).await {
            Ok((room, room_events)) => {
                *self.room.lock().await = Some(room);
                self.room_events = Some(room_events);
                *self.is_connected.lock().await = true;
                self.is_connected_atomic.store(true, Ordering::Release);

                info!("Successfully connected to LiveKit room");

                self.configure_data_channel_threshold().await;

                self.setup_audio_publishing().await?;
                self.start_event_handler().await?;
                self.start_operation_worker().await;

                info!("LiveKit client initialization sequence completed");
                Ok(())
            }
            Err(e) => {
                error!("Failed to connect to LiveKit room: {:?}", e);
                *self.is_connected.lock().await = false;
                self.is_connected_atomic.store(false, Ordering::Release);
                Err(AppError::InternalServerError(format!(
                    "Failed to connect to LiveKit room: {e:?}"
                )))
            }
        }
    }

    /// Check whether the client is connected using atomic flag for fast access.
    pub fn is_connected(&self) -> bool {
        self.is_connected_atomic.load(Ordering::Acquire)
    }

    /// Determine if an audio source is currently active and ready for streaming.
    pub fn has_audio_source(&self) -> bool {
        self.has_audio_source_atomic.load(Ordering::Acquire)
    }

    /// Disconnect from the room and clean up running tasks.
    pub async fn disconnect(&mut self) -> Result<(), AppError> {
        info!("Disconnecting from LiveKit room");

        // Signal disconnect early
        *self.is_connected.lock().await = false;
        self.is_connected_atomic.store(false, Ordering::Release);

        // Request graceful shutdown if worker exists
        if let Some(queue) = &self.operation_queue {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            let _ = queue
                .queue(LiveKitOperation::Shutdown {
                    ack_tx: Some(ack_tx),
                })
                .await;
            // Wait for acknowledgement with timeout
            let _ = tokio::time::timeout(LIVEKIT_SHUTDOWN_GRACE, ack_rx).await;
        }

        // Abort and observe event handler task if running
        let event_handler = self
            .event_handler_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = event_handler {
            let _ = abort_and_await_task("livekit.event_handler", handle).await;
        }

        // Abort active streams without holding the registry lock across awaits.
        let stream_handles: Vec<_> = {
            let mut streams = self.active_streams.lock().await;
            streams.drain().collect()
        };
        for (key, handle) in stream_handles {
            let _ = abort_and_await_task(format!("livekit.audio_stream.{key}"), handle).await;
        }

        // Close the room
        if let Some(room) = self.room.lock().await.take() {
            let _ = room.close().await;
        }

        // Wait for worker to finish
        if let Some(handle) = self.operation_worker_handle.take() {
            // Worker should exit gracefully after receiving Shutdown
            let _ = await_task_shutdown("livekit.operation_worker", handle, LIVEKIT_SHUTDOWN_GRACE)
                .await;
        }

        // Clear the operation queue
        self.operation_queue = None;

        // Clean up resources
        *self.audio_source.lock().await = None;
        self.local_audio_track = None;
        *self.local_track_publication.lock().await = None;
        self.has_audio_source_atomic.store(false, Ordering::Release);

        *self.room.lock().await = None;
        self.room_events = None;

        info!("Successfully disconnected from LiveKit room");
        Ok(())
    }

    async fn configure_data_channel_threshold(&self) {
        let room_guard = self.room.lock().await;
        if let Some(room_ref) = room_guard.as_ref() {
            let participant = room_ref.local_participant();
            if let Err(e) = participant.set_data_channel_buffered_amount_low_threshold(
                RELIABLE_BUFFER_THRESHOLD_BYTES,
                DataPacketKind::Reliable,
            ) {
                warn!(
                    "Failed to set data channel buffered amount threshold: {:?}",
                    e
                );
            } else {
                debug!(
                    "Set reliable data channel buffered amount threshold to {} bytes",
                    RELIABLE_BUFFER_THRESHOLD_BYTES
                );
            }
        }
    }
}

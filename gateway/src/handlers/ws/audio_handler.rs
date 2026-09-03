//! Audio processing handler for WebSocket connections
//!
//! This module handles all audio-related operations including:
//! - Processing incoming audio data from clients
//! - Routing audio through STT (Speech-to-Text) providers
//! - Managing TTS (Text-to-Speech) synthesis requests
//! - Handling audio clear/interruption commands

use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};

use crate::core::voice_manager::VoiceManager;

use super::{
    messages::{MessageClass, MessageRoute, OutgoingMessage, send_with_policy},
    state::ConnectionState,
};

/// Maximum allowed size for a single audio frame (5 MB)
/// This is more restrictive than the WebSocket frame limit to prevent
/// memory exhaustion from oversized audio buffers.
/// Typical audio frame at 16kHz/16-bit mono for 1 second = 32KB
/// Even 10 minutes of uncompressed audio = ~19MB, so 5MB is generous for buffered streaming
pub const MAX_AUDIO_FRAME_SIZE: usize = 5 * 1024 * 1024;

async fn send_error(message_tx: &mpsc::Sender<MessageRoute>, message: impl Into<String>) {
    send_with_policy(
        message_tx,
        MessageRoute::Outgoing(OutgoingMessage::Error {
            message: message.into(),
        }),
        MessageClass::Critical,
    )
    .await;
}

/// Handle incoming audio data with zero-copy optimizations
///
/// Processes raw audio data received from WebSocket clients and forwards it
/// to the configured STT provider for transcription.
///
/// # Arguments
/// * `audio_data` - Raw audio bytes received from the client
/// * `state` - Connection state containing voice manager and configuration
/// * `message_tx` - Channel for sending response messages back to the client
///
/// # Returns
/// * `bool` - true to continue processing, false to terminate connection
///
/// # Performance Notes
/// - Uses read-only lock for fast state access
/// - Zero-copy data passing where possible
/// - Marked inline for hot path optimization
#[inline(always)]
pub async fn handle_audio_message(
    audio_data: Bytes,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    let audio_len = audio_data.len();
    debug!("Processing audio data: {} bytes", audio_len);

    // Check audio frame size limit early to prevent resource exhaustion
    if audio_len > MAX_AUDIO_FRAME_SIZE {
        warn!(
            "Audio frame too large: {} bytes (max: {} bytes)",
            audio_len, MAX_AUDIO_FRAME_SIZE
        );
        send_error(
            message_tx,
            format!(
                "Audio frame too large: {} bytes (max: {} bytes)",
                audio_len, MAX_AUDIO_FRAME_SIZE
            ),
        )
        .await;
        return true;
    }

    // Fast path: read lock to check state, get the voice manager, and (D8) opus-decode the frame.
    let (voice_manager, audio_data) = {
        let state_guard = state.read().await;

        // Check if audio processing is enabled (atomic read, no lock overhead)
        if !state_guard.is_audio_enabled() {
            send_error(
                message_tx,
                "Audio processing is disabled. Send config message with audio=true first.",
            )
            .await;
            return true;
        }

        let voice_manager = match &state_guard.voice_manager {
            Some(vm) => vm.clone(),
            None => {
                send_error(
                    message_tx,
                    "Voice manager not configured. Send config message with audio=true first.",
                )
                .await;
                return true;
            }
        };

        // D8: when the session negotiated opus uplink, each WS binary frame is one opus packet —
        // decode it to PCM16 (at the negotiated rate) before STT. A bad packet is logged + dropped,
        // never tearing down the stream. linear16 sessions (the common case) pass straight through.
        #[cfg(feature = "opus-codec")]
        let audio_data = match &state_guard.opus_decoder {
            Some(decoder) => match decoder.lock().await.decode_packet(&audio_data) {
                Ok(pcm) => Bytes::from(pcm),
                Err(e) => {
                    warn!("opus uplink decode failed: {e}; dropping frame");
                    return true;
                }
            },
            None => audio_data,
        };

        (voice_manager, audio_data)
    };

    // Send the (decoded) PCM audio to the STT provider. Bytes gives O(1) clones.
    if let Err(e) = voice_manager.receive_audio(audio_data).await {
        error!("Failed to process audio: {}", e);
        send_error(message_tx, format!("Failed to process audio: {e}")).await;
    }

    true
}

/// Handle text-to-speech synthesis request
///
/// Processes speak commands to synthesize text into audio using the configured
/// TTS provider. Supports queuing, flushing, and interruption control.
///
/// # Arguments
/// * `text` - Text to synthesize into speech
/// * `flush` - Whether to clear the TTS queue before speaking (default: true)
/// * `allow_interruption` - Whether this audio can be interrupted (default: true)
/// * `state` - Connection state containing voice manager
/// * `message_tx` - Channel for sending response messages
///
/// # Returns
/// * `bool` - true to continue processing, false to terminate connection
pub async fn handle_speak_message(
    text: String,
    flush: Option<bool>,
    allow_interruption: Option<bool>,
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    // Default flush to true for backward compatibility
    let should_flush = flush.unwrap_or(true);
    // Default allow_interruption to true for backward compatibility
    let allow_interruption = allow_interruption.unwrap_or(true);

    debug!(
        "Processing speak command: {} chars (flush: {}, allow_interruption: {})",
        text.len(),
        should_flush,
        allow_interruption
    );

    // Fast path: read lock to check state and get voice manager
    let voice_manager = match get_voice_manager_if_audio_enabled(state, message_tx).await {
        Some(vm) => vm,
        None => return true,
    };

    info!(
        "Speaking text (flush: {}, allow_interruption: {}): {}",
        should_flush, allow_interruption, text
    );

    // Send text to TTS provider with flush and allow_interruption parameters
    if let Err(e) = voice_manager
        .speak_with_interruption(&text, should_flush, allow_interruption)
        .await
    {
        error!("Failed to synthesize speech: {}", e);
        send_error(message_tx, format!("Failed to synthesize speech: {e}")).await;
    } else {
        debug!(
            "Speech synthesis started for: {} chars (flush: {}, allow_interruption: {})",
            text.len(),
            should_flush,
            allow_interruption
        );
    }

    true
}

/// Handle audio clear/interruption command
///
/// Clears the TTS queue and any pending audio. Respects non-interruptible
/// audio playback settings.
///
/// # Arguments
/// * `state` - Connection state containing voice and LiveKit managers
/// * `message_tx` - Channel for sending response messages
///
/// # Returns
/// * `bool` - true to continue processing, false to terminate connection
pub async fn handle_clear_message(
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    debug!("Processing clear command");

    // Fast path: read lock to get both managers
    let (voice_manager, livekit_client) = {
        let state_guard = state.read().await;

        // Check if audio processing is enabled for voice manager operations
        let vm = if state_guard.is_audio_enabled() {
            match &state_guard.voice_manager {
                Some(vm) => Some(vm.clone()),
                None => {
                    send_error(
                        message_tx,
                        "Voice manager not configured. Send config message with audio=true first.",
                    )
                    .await;
                    return true;
                }
            }
        } else {
            // Audio is disabled, so voice manager operations are not available
            None
        };

        let lk = state_guard.livekit_client.clone();
        (vm, lk)
    };

    // Check if we're in a non-interruptible state
    let is_blocked = if let Some(ref vm) = voice_manager {
        vm.is_interruption_blocked().await
    } else {
        false
    };

    if is_blocked {
        debug!("Clear command ignored - currently in non-interruptible audio playback");
        return true;
    }

    // Clear TTS provider and audio buffers (only if audio is enabled)
    // Note: The VoiceManager's clear_tts() will automatically call the audio_clear_callback
    // which clears the LiveKit audio buffer, so we don't need to do it separately
    if let Some(vm) = voice_manager {
        if let Err(e) = vm.clear_tts().await {
            error!("Failed to clear TTS provider: {}", e);
            send_error(message_tx, format!("Failed to clear TTS provider: {e}")).await;
        } else {
            debug!("Successfully cleared TTS and audio buffers");
        }
    } else {
        debug!("Audio processing disabled - skipping TTS provider clear");

        // If audio is disabled but LiveKit is configured, still clear LiveKit audio
        if let Some(livekit_manager) = livekit_client {
            // Use write() to wait for the lock - clear operation is important
            let client = livekit_manager.write().await;
            match client.clear_audio().await {
                Ok(()) => {
                    debug!("Successfully cleared LiveKit audio buffer (audio disabled mode)");
                }
                Err(e) => {
                    error!("Failed to clear LiveKit audio buffer: {}", e);
                }
            }
        }
    }

    debug!("Clear command completed");
    true
}

/// Handle audio end signal from client
///
/// This signals that the client has finished sending audio. The handler
/// triggers STT finalization which sends a CloseStream message to the
/// provider, causing it to finalize pending transcripts and send
/// `speech_final=true`.
///
/// # Arguments
/// * `state` - Connection state containing voice manager
/// * `message_tx` - Channel for sending response messages
///
/// # Returns
/// * `bool` - true to continue processing, false to terminate connection
pub async fn handle_audio_end(
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> bool {
    info!("Processing audio_end signal - finalizing STT stream");

    // Get voice manager if audio is enabled
    let voice_manager = match get_voice_manager_if_audio_enabled(state, message_tx).await {
        Some(vm) => vm,
        None => return true,
    };

    // Finalize the STT stream
    if let Err(e) = voice_manager.finalize_stt().await {
        error!("Failed to finalize STT stream: {}", e);
        send_error(message_tx, format!("Failed to finalize STT stream: {e}")).await;
    } else {
        debug!("STT stream finalized successfully");
    }

    true
}

/// Helper function to get voice manager if audio is enabled
///
/// Checks if audio processing is enabled and returns the voice manager if available.
/// Sends appropriate error messages if audio is disabled or voice manager is not configured.
///
/// # Arguments
/// * `state` - Connection state to check
/// * `message_tx` - Channel for sending error messages
///
/// # Returns
/// * `Option<Arc<VoiceManager>>` - Voice manager if available, None otherwise
async fn get_voice_manager_if_audio_enabled(
    state: &Arc<RwLock<ConnectionState>>,
    message_tx: &mpsc::Sender<MessageRoute>,
) -> Option<Arc<VoiceManager>> {
    let state_guard = state.read().await;

    // Check if audio processing is enabled (atomic read, no lock overhead)
    if !state_guard.is_audio_enabled() {
        send_error(
            message_tx,
            "Audio processing is disabled. Send config message with audio=true first.",
        )
        .await;
        return None;
    }

    match &state_guard.voice_manager {
        Some(vm) => Some(vm.clone()),
        None => {
            send_error(
                message_tx,
                "Voice manager not configured. Send config message with audio=true first.",
            )
            .await;
            None
        }
    }
}

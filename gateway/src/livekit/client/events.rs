//! Room event handling for `LiveKitClient`.
//!
//! Spawns background tasks that react to LiveKit room updates and forward
//! audio or data payloads through registered callbacks.

use std::sync::Arc;

use futures::StreamExt;
use livekit::participant::ParticipantKind;
use livekit::prelude::{RemoteTrack, RoomEvent};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::{
    AudioCallback, DataCallback, DataMessage, LiveKitClient, LiveKitConfig,
    ParticipantDisconnectCallback, ParticipantDisconnectEvent,
};
use crate::AppError;
use crate::core::observability::{abort_and_await_task, await_task_shutdown};
#[cfg(feature = "noise-filter")]
use crate::utils::noise_filter::reduce_noise_async;

impl LiveKitClient {
    pub(super) async fn start_event_handler(&mut self) -> Result<(), AppError> {
        if let Some(mut room_events) = self.room_events.take() {
            let audio_callback = self.audio_callback.clone();
            let data_callback = self.data_callback.clone();
            let participant_disconnect_callback = self.participant_disconnect_callback.clone();
            let active_streams = Arc::clone(&self.active_streams);
            let is_connected = Arc::clone(&self.is_connected);
            let config = self.config.clone();

            let handle = tokio::spawn(async move {
                while let Some(event) = room_events.recv().await {
                    if let Err(e) = LiveKitClient::handle_room_event(
                        event,
                        &audio_callback,
                        &data_callback,
                        &participant_disconnect_callback,
                        &active_streams,
                        &is_connected,
                        &config,
                    )
                    .await
                    {
                        error!("Error handling room event: {:?}", e);
                    }
                }
                info!("Room event handler finished");
            });

            // Store the handle for cleanup during disconnect/drop.
            let old = self
                .event_handler_handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .replace(handle);
            if let Some(old) = old {
                let _ = abort_and_await_task("livekit.event_handler.replace", old).await;
            }
        }

        Ok(())
    }

    pub(super) async fn restart_event_handler(
        mut room_events: tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
        audio_callback: &Option<AudioCallback>,
        data_callback: &Option<DataCallback>,
        participant_disconnect_callback: &Option<ParticipantDisconnectCallback>,
        active_streams: &Arc<Mutex<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>>,
        event_handler_handle: &Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
        is_connected: &Arc<Mutex<bool>>,
        config: &LiveKitConfig,
    ) {
        let audio_callback = audio_callback.clone();
        let data_callback = data_callback.clone();
        let participant_disconnect_callback = participant_disconnect_callback.clone();
        let active_streams = Arc::clone(active_streams);
        let is_connected = Arc::clone(is_connected);
        let config = config.clone();

        let handle = tokio::spawn(async move {
            info!("Restarting room event handler after reconnect");
            while let Some(event) = room_events.recv().await {
                if let Err(e) = LiveKitClient::handle_room_event(
                    event,
                    &audio_callback,
                    &data_callback,
                    &participant_disconnect_callback,
                    &active_streams,
                    &is_connected,
                    &config,
                )
                .await
                {
                    error!("Error handling room event: {:?}", e);
                }
            }
            info!("Restarted room event handler finished");
        });

        let old = event_handler_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(handle);
        if let Some(old) = old {
            let _ = abort_and_await_task("livekit.event_handler.reconnect_replace", old).await;
        }
    }

    /// One frame of a participant's audio to the session's audio callback, through the noise
    /// filter when it is on.
    #[cfg_attr(not(feature = "noise-filter"), allow(unused_variables))]
    async fn deliver_frame(
        audio_buffer: Vec<u8>,
        enable_noise_filter: bool,
        sample_rate: u32,
        pool: &Arc<Mutex<Vec<Vec<u8>>>>,
        callback: &AudioCallback,
    ) {
        #[cfg(feature = "noise-filter")]
        if enable_noise_filter {
            // For noise filtering, we need to clone since the filter may modify the buffer
            match reduce_noise_async(audio_buffer.clone().into(), sample_rate).await {
                Ok(filtered_audio) => {
                    LiveKitClient::return_buffer_to_pool(pool, audio_buffer);
                    callback(filtered_audio);
                }
                Err(e) => {
                    error!("Error reducing noise: {:?}", e);
                    callback(audio_buffer);
                }
            }
            return;
        }
        callback(audio_buffer);
    }

    async fn handle_room_event(
        event: RoomEvent,
        audio_callback: &Option<AudioCallback>,
        data_callback: &Option<DataCallback>,
        participant_disconnect_callback: &Option<ParticipantDisconnectCallback>,
        active_streams: &Arc<Mutex<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>>,
        is_connected: &Arc<Mutex<bool>>,
        config: &LiveKitConfig,
    ) -> Result<(), AppError> {
        match event {
            RoomEvent::TrackSubscribed {
                track,
                publication,
                participant,
            } => {
                info!(
                    "Track subscribed - Track: {:?}, Participant: {:?}",
                    publication.sid(),
                    participant.identity()
                );

                // Check if we should process this participant's audio
                if !config.listen_participants.is_empty()
                    && !config
                        .listen_participants
                        .contains(&participant.identity().to_string())
                {
                    debug!(
                        "Ignoring audio track from participant {} (not in listen_participants list)",
                        participant.identity()
                    );
                    return Ok(());
                }

                match track {
                    RemoteTrack::Audio(audio_track) => {
                        debug!(
                            "Received audio track from participant: {}",
                            participant.identity()
                        );

                        // Interaction mode hears one caller: the first whose audio arrives.
                        let phone = participant.kind() == ParticipantKind::Sip;
                        if let Some(tap) = &config.caller_tap
                            && !tap.admits(participant.identity().as_str(), phone)
                        {
                            debug!(
                                "Ignoring audio track from participant {} (not the session's caller)",
                                participant.identity()
                            );
                            return Ok(());
                        }
                        let caller_tap = config.caller_tap.clone();

                        if let Some(callback) = audio_callback {
                            let callback_clone = Arc::clone(callback);
                            let rtc_track = audio_track.rtc_track();
                            let mut audio_stream = NativeAudioStream::new(
                                rtc_track,
                                // INGRESS rate: what STT/VAD expect — never
                                // the egress/publish rate.
                                config.ingress_sample_rate as i32,
                                config.channels as i32,
                            );

                            let enable_noise_filter = config.enable_noise_filter;
                            let sample_rate = config.ingress_sample_rate;
                            let participant_id = participant.identity().to_string();

                            // Create a local buffer pool for this audio stream
                            let stream_buffer_pool = Arc::new(Mutex::new(
                                (0..4).map(|_| Vec::with_capacity(4800)).collect::<Vec<_>>(),
                            ));

                            let handle = tokio::spawn(async move {
                                info!(
                                    "Starting audio stream processing for participant: {} (noise_filter: {})",
                                    participant_id, enable_noise_filter
                                );

                                #[cfg(not(feature = "noise-filter"))]
                                if enable_noise_filter {
                                    warn!(
                                        "Noise filter requested for participant {} but the 'noise-filter' feature is disabled; forwarding raw audio",
                                        participant_id
                                    );
                                }

                                // Interaction mode: the caller's frames reach its tap as they
                                // arrive, ahead of the noise filter. The filter works on a
                                // queue of its own, so when it is slow (its workers start
                                // with the first frame) the tap does not wait for it. The
                                // queue holds 64 frames: beyond that a frame is dropped, as
                                // the stream drops frames nobody reads, so speech-to-text
                                // never falls further behind the call.
                                let filtered = caller_tap.map(|tap| {
                                    let (queue, mut queued) =
                                        tokio::sync::mpsc::channel::<Vec<u8>>(64);
                                    let (callback, pool) =
                                        (callback_clone.clone(), stream_buffer_pool.clone());
                                    tokio::spawn(async move {
                                        while let Some(audio_buffer) = queued.recv().await {
                                            LiveKitClient::deliver_frame(
                                                audio_buffer,
                                                enable_noise_filter,
                                                sample_rate,
                                                &pool,
                                                &callback,
                                            )
                                            .await;
                                        }
                                    });
                                    (tap, queue)
                                });

                                while let Some(audio_frame) = audio_stream.next().await {
                                    debug!(
                                        "Received audio frame: {} samples",
                                        audio_frame.data.len()
                                    );

                                    let audio_buffer = LiveKitClient::convert_frame_to_audio(
                                        &audio_frame,
                                        Some(&stream_buffer_pool),
                                    );
                                    match &filtered {
                                        Some((tap, queue)) => {
                                            tap.audio(&audio_buffer);
                                            if let Err(
                                                tokio::sync::mpsc::error::TrySendError::Full(_),
                                            ) = queue.try_send(audio_buffer)
                                            {
                                                crate::core::metrics::bridge::record_degraded(
                                                    "livekit",
                                                    "noise_filter_behind",
                                                );
                                            }
                                        }
                                        None => {
                                            LiveKitClient::deliver_frame(
                                                audio_buffer,
                                                enable_noise_filter,
                                                sample_rate,
                                                &stream_buffer_pool,
                                                &callback_clone,
                                            )
                                            .await;
                                        }
                                    }
                                }

                                info!("Audio stream ended for participant: {}", participant_id);
                            });

                            // E-G3: exactly ONE producer per participant —
                            // a resubscribe (mute/unmute re-drives
                            // subscription) aborts the old stream task
                            // BEFORE registering the new one (Pipecat
                            // close-old-before-register).
                            // Key by (participant, TRACK sid) — a participant
                            // can publish more than one audio track (mic +
                            // screenshare-audio); keying on identity alone
                            // aborted the first track's task on the second
                            // SUBSCRIBE (review wc71hewlx #6). The same key
                            // still aborts a genuine RESUBSCRIBE of the SAME
                            // track.
                            let key = format!("{}::{}", participant.identity(), publication.sid());
                            let mut finished = Vec::new();
                            let old = {
                                let mut streams = active_streams.lock().await;
                                let old = streams.insert(key.clone(), handle);
                                let mut retained =
                                    std::collections::HashMap::with_capacity(streams.len());
                                for (stream_key, stream_handle) in streams.drain() {
                                    if stream_handle.is_finished() {
                                        finished.push((stream_key, stream_handle));
                                    } else {
                                        retained.insert(stream_key, stream_handle);
                                    }
                                }
                                *streams = retained;
                                old
                            };
                            if let Some(old) = old {
                                if !old.is_finished() {
                                    info!(
                                        track = %key,
                                        "resubscribe: aborting the previous audio stream task"
                                    );
                                }
                                let _ = abort_and_await_task(
                                    format!("livekit.audio_stream.resubscribe.{key}"),
                                    old,
                                )
                                .await;
                            }
                            for (stream_key, stream_handle) in finished {
                                let _ = await_task_shutdown(
                                    format!("livekit.audio_stream.finished.{stream_key}"),
                                    stream_handle,
                                    std::time::Duration::from_millis(10),
                                )
                                .await;
                            }
                        } else {
                            warn!("No audio callback set, ignoring audio track");
                        }
                    }
                    RemoteTrack::Video(_) => {
                        debug!(
                            "Received video track (ignoring): {}",
                            participant.identity()
                        );
                    }
                }
            }
            RoomEvent::TrackUnsubscribed {
                publication,
                participant,
                ..
            } => {
                info!(
                    "Track unsubscribed - Track: {:?}, Participant: {:?}",
                    publication.sid(),
                    participant.identity()
                );
                // E-G3: stop ONLY this track's stream task (composite key —
                // review wc71hewlx #6) — relying on the SDK to close the
                // underlying stream leaked tasks under resubscribe storms.
                let key = format!("{}::{}", participant.identity(), publication.sid());
                if let Some(handle) = active_streams.lock().await.remove(&key) {
                    let _ = abort_and_await_task(
                        format!("livekit.audio_stream.unsubscribe.{key}"),
                        handle,
                    )
                    .await;
                }
            }
            RoomEvent::ParticipantConnected(participant) => {
                info!("Participant connected: {}", participant.identity());
            }
            RoomEvent::SipDTMFReceived {
                code,
                digit,
                participant,
            } => {
                if let Some(tap) = &config.caller_tap {
                    let identity = participant.as_ref().map(|p| p.identity().to_string());
                    tap.key(identity.as_deref(), code, digit.as_deref());
                }
            }
            RoomEvent::ParticipantDisconnected(participant) => {
                info!("Participant disconnected: {}", participant.identity());
                if let Some(tap) = &config.caller_tap
                    && tap.is_other(participant.identity().as_str())
                {
                    // The session goes on with its caller.
                    return Ok(());
                }

                if let Some(callback) = participant_disconnect_callback {
                    let event = ParticipantDisconnectEvent {
                        participant_identity: participant.identity().to_string(),
                        participant_name: Some(participant.name().to_string()),
                        room_name: config.room_name.clone(),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    };

                    callback(event);
                }
            }
            RoomEvent::Disconnected { reason } => {
                warn!("Room disconnected with reason: {:?}", reason);
                *is_connected.lock().await = false;
            }
            RoomEvent::DataReceived {
                payload,
                topic,
                participant,
                ..
            } => {
                let participant_identity = participant
                    .as_ref()
                    .map(|p| p.identity().to_string())
                    .unwrap_or_else(|| "server".to_string());

                debug!(
                    "Data received from participant: {}, {} bytes",
                    participant_identity,
                    payload.len()
                );

                // Check if we should process this participant's data messages
                if !config.listen_participants.is_empty()
                    && participant.is_some()
                    && !config.listen_participants.contains(&participant_identity)
                {
                    debug!(
                        "Ignoring data message from participant {} (not in listen_participants list)",
                        participant_identity
                    );
                    return Ok(());
                }

                if let Some(callback) = data_callback {
                    let data_message = DataMessage {
                        participant_identity,
                        data: payload.to_vec(),
                        topic,
                        room_name: config.room_name.clone(),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    };

                    callback(data_message);
                } else {
                    debug!("No data callback set, ignoring data message");
                }
            }
            _ => {
                debug!("Received other room event: {:?}", event);
            }
        }

        Ok(())
    }
}

use std::fmt;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// LiveKit configuration for connecting to a room
#[derive(Debug, Clone)]
pub struct LiveKitConfig {
    pub url: String,
    pub token: String,
    /// Room name (extracted from token or provided separately)
    pub room_name: String,
    /// Sample rate for audio PUBLISHING (egress: bot audio to the room).
    /// With `client_playback_rate` set this is the client rate (the bytes
    /// fed to the track are resampled to it).
    pub sample_rate: u32,
    /// Sample rate remote-participant (user) audio is DELIVERED at
    /// (ingress: room mic -> STT/VAD). MUST match what the voice pipeline
    /// declares downstream — the STT provider's configured rate — or
    /// transcription receives time-stretched audio (review wf_85659e16 #1:
    /// a single shared rate let the egress-only `client_playback_rate`
    /// silently re-rate the user's microphone).
    pub ingress_sample_rate: u32,
    /// Number of audio channels for publishing (typically 1 for mono)
    pub channels: u16,
    /// Enable noise filtering on incoming audio (default: enabled when the `noise-filter`
    /// feature is compiled in). Set to `false` to reduce latency when filtering is
    /// available.
    pub enable_noise_filter: bool,
    /// List of participant identities to listen to for audio tracks and data messages.
    ///
    /// If empty, all participants' audio and data will be processed.
    /// If populated, only participants in this list will be processed.
    pub listen_participants: Vec<String>,
    /// Interaction mode: the session hears one caller (see [`CallerTap`]). `None` on every
    /// other session.
    pub caller_tap: Option<CallerTap>,
}

/// Interaction mode's view of a room: one caller. The first participant whose audio track is
/// subscribed (among `listen_participants`, when the session lists any) is the caller for the
/// rest of the session. Nobody else's audio is heard, and nobody else's leaving ends the
/// session.
#[derive(Clone)]
pub struct CallerTap {
    audio: Arc<dyn Fn(&[u8]) + Send + Sync>,
    found: Arc<dyn Fn(bool) + Send + Sync>,
    /// The keys the caller presses on a phone (SIP DTMF).
    keys: Option<Arc<dyn Fn(char) + Send + Sync>>,
    identity: Arc<Mutex<Option<String>>>,
}

impl CallerTap {
    /// `audio` gets the caller's frames as they arrive, before the noise filter. `found` is
    /// called once, when the caller is known, with whether it is a phone (SIP) participant.
    pub fn new(
        audio: impl Fn(&[u8]) + Send + Sync + 'static,
        found: impl Fn(bool) + Send + Sync + 'static,
    ) -> Self {
        Self {
            audio: Arc::new(audio),
            found: Arc::new(found),
            keys: None,
            identity: Arc::default(),
        }
    }

    /// `keys` gets each key the caller presses (`0`-`9`, `*`, `#`); nobody else's.
    pub fn on_keys(mut self, keys: impl Fn(char) + Send + Sync + 'static) -> Self {
        self.keys = Some(Arc::new(keys));
        self
    }

    /// A SIP DTMF event of the room: passed on when it is the caller's. LiveKit gives the
    /// key as `digit`, or only as its `code` (`0`-`9`, then 10 for `*` and 11 for `#`).
    pub(crate) fn key(&self, identity: Option<&str>, code: u32, digit: Option<&str>) {
        let caller = self.caller();
        let Some(keys) = self
            .keys
            .as_ref()
            .filter(|_| identity.is_some() && caller.as_deref() == identity)
        else {
            return;
        };
        let key = digit.and_then(|d| d.trim().chars().next()).or(match code {
            0..=9 => char::from_digit(code, 10),
            10 => Some('*'),
            11 => Some('#'),
            _ => None,
        });
        if let Some(key) = key.filter(|k| k.is_ascii_digit() || *k == '*' || *k == '#') {
            drop(caller);
            keys(key);
        }
    }

    fn caller(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.identity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether `identity` is the caller. The first participant asked about becomes it.
    pub(crate) fn admits(&self, identity: &str, phone: bool) -> bool {
        let mut caller = self.caller();
        match caller.as_deref() {
            Some(caller) => caller == identity,
            None => {
                *caller = Some(identity.to_owned());
                (self.found)(phone);
                true
            }
        }
    }

    /// Whether the session has a caller and `identity` is somebody else.
    pub(crate) fn is_other(&self, identity: &str) -> bool {
        self.caller()
            .as_deref()
            .is_some_and(|caller| caller != identity)
    }

    pub(crate) fn audio(&self, frame: &[u8]) {
        (self.audio)(frame);
    }
}

impl fmt::Debug for CallerTap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallerTap")
            .field("caller", &*self.caller())
            .finish()
    }
}

/// LiveKit connection status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

/// Audio frame information received from LiveKit
#[derive(Debug, Clone)]
pub struct AudioFrameInfo {
    pub data: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u8,
    pub participant_identity: String,
    pub timestamp: u64,
}

/// LiveKit room information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomInfo {
    pub name: String,
    pub participants_count: usize,
    pub audio_tracks_count: usize,
    pub video_tracks_count: usize,
}

/// Participant information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub identity: String,
    pub name: Option<String>,
    pub is_local: bool,
    pub audio_tracks: Vec<String>,
    pub video_tracks: Vec<String>,
}

/// LiveKit error types specific to this implementation
#[derive(Debug, thiserror::Error)]
pub enum LiveKitError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Not connected to room")]
    NotConnected,

    #[error("Audio callback not set")]
    AudioCallbackNotSet,

    #[error("Room event error: {0}")]
    RoomEventError(String),

    #[error("Audio stream error: {0}")]
    AudioStreamError(String),

    #[error("SIP transfer request timeout (transfer likely succeeded)")]
    SIPTransferRequestTimeout,
}

//! Gnani.ai STT client: Prisma realtime streaming over WebSocket.
//!
//! ```text
//! send_audio → FrameChunker (exact 1,024-byte frames) → binary frames ─┐
//! end_turn / disconnect → {"type":"commit"} (text frame) ──────────────┤→ wss://api.vachana.ai/stt/v3/stream
//! on_result ← STTResult (final) ← {"type":"transcript"} ←──────────────┘
//! ```
//!
//! Every setting rides on the WebSocket upgrade headers, so a reconnect restores the featured
//! session by dialing again; the connection is ready when Gnani sends `connected`.

use bytes::Bytes;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config, tungstenite::Message,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::core::resilience::connect::{WS_CONNECT_TIMEOUT, with_timeout};
use crate::core::stt::base::{BaseSTT, STTConfig, STTError, STTErrorCallback, STTResultCallback};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

use super::config::GnaniSTTConfig;
use super::messages::{COMMIT_MESSAGE, FrameChunker, GnaniServerMessage, GnaniTranscript};

/// Per-message idle timeout. Gnani speaks only when a segment closes, and closes the session
/// itself after 60 s without speech (`x-idle-timeout-sec`), so this only catches a dead socket.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(90);

/// How long `connect` waits for Gnani's `connected` message.
const CONNECTED_TIMEOUT: Duration = Duration::from_secs(10);

/// On disconnect with an utterance open: how long to wait for the transcript the commit asks for.
const COMMIT_GRACE: Duration = Duration::from_millis(1000);

/// Gnani's default `x-min-silence-ms`: the silence after which it closes a segment by itself.
const GNANI_MIN_SILENCE_MS: u32 = 500;

type GnaniWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// What the client queues for the socket, in order.
#[derive(Debug)]
enum Outbound {
    Audio(Bytes),
    Commit,
}

/// The outbound side shared across reconnect attempts: the single-consumer queue and the frame
/// remainder, so neither queued audio nor a partial frame is lost to a reconnect.
struct OutboundPipe {
    rx: mpsc::Receiver<Outbound>,
    chunker: FrameChunker,
}

/// A [`WsTransport`] over one Gnani WebSocket. [`restore_session`](WsTransport::restore_session)
/// has nothing to re-send (the featured session is in the upgrade headers); [`run`] signals the
/// waiting `connect` on Gnani's `connected` message.
struct GnaniTransport {
    ws_sink: SplitSink<GnaniWs, Message>,
    ws_stream: SplitStream<GnaniWs>,
    outbound: Arc<Mutex<OutboundPipe>>,
    shutdown_token: CancellationToken,
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    on_result: Arc<RwLock<Option<STTResultCallback>>>,
    on_error: Arc<RwLock<Option<STTErrorCallback>>>,
    /// The last error, for `connect` to report when the session never became ready.
    last_error: Arc<std::sync::Mutex<Option<String>>>,
    sample_rate: u32,
    /// Audio was sent since the last transcript: there is an utterance a commit would close.
    utterance_open: bool,
}

impl GnaniTransport {
    async fn deliver(&mut self, transcript: &GnaniTranscript) {
        self.utterance_open = false;
        let result = transcript.to_stt_result();
        if result.transcript.is_empty() {
            return;
        }
        if let Some(callback) = self.on_result.read().await.as_ref() {
            callback(result).await;
        }
    }

    async fn report(&self, error: STTError) {
        *self.last_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(error.to_string());
        if let Some(callback) = self.on_error.read().await.as_ref() {
            callback(error).await;
        }
    }

    async fn send_frames(&mut self, frames: Vec<Bytes>) -> Result<(), String> {
        for frame in frames {
            self.ws_sink
                .send(Message::Binary(frame))
                .await
                .map_err(|e| e.to_string())?;
            self.utterance_open = true;
        }
        Ok(())
    }

    /// Handle one text frame.
    async fn on_text(&mut self, text: &str) {
        trace!("Gnani message: {}", text);
        match GnaniServerMessage::parse(text) {
            Ok(GnaniServerMessage::Connected {
                config, request_id, ..
            }) => {
                if let Some(rate) = config.and_then(|c| c.sample_rate)
                    && rate != self.sample_rate
                {
                    warn!(
                        sent = self.sample_rate,
                        active = rate,
                        "Gnani echoed a different sample rate"
                    );
                }
                debug!(?request_id, "Gnani STT session ready");
                if let Some(tx) = self.connected_tx.lock().await.take() {
                    let _ = tx.send(());
                }
            }
            Ok(GnaniServerMessage::Transcript(transcript)) => self.deliver(&transcript).await,
            Ok(GnaniServerMessage::Error { message }) => {
                // An error on one utterance leaves the session open; a fatal one is followed by
                // a close, whose code decides whether to reconnect.
                error!("Gnani STT error: {}", message);
                self.report(STTError::ProviderError(message)).await;
            }
            Ok(GnaniServerMessage::Processing { .. })
            | Ok(GnaniServerMessage::SpeechStart { .. })
            | Ok(GnaniServerMessage::SettingsUpdated { .. }) => {}
            Ok(GnaniServerMessage::Unknown) => debug!("Unknown Gnani message: {}", text),
            Err(e) => warn!("Unparseable Gnani message ({e}): {}", text),
        }
    }

    /// Intentional close: send what is queued, commit an open utterance and wait briefly for its
    /// transcript, then close the socket.
    async fn finish(&mut self) -> ReconnectOutcome {
        let pending: Vec<Outbound> = {
            let mut pipe = self.outbound.lock().await;
            let mut out = Vec::new();
            while let Ok(item) = pipe.rx.try_recv() {
                match item {
                    Outbound::Audio(audio) => {
                        out.extend(pipe.chunker.push(&audio).into_iter().map(Outbound::Audio))
                    }
                    Outbound::Commit => out.push(Outbound::Commit),
                }
            }
            out
        };
        for item in pending {
            let sent = match item {
                Outbound::Audio(frame) => self.send_frames(vec![frame]).await,
                Outbound::Commit => self.send_commit().await,
            };
            if sent.is_err() {
                return ReconnectOutcome::Completed;
            }
        }

        if self.utterance_open && self.send_commit().await.is_ok() {
            let deadline = tokio::time::Instant::now() + COMMIT_GRACE;
            while self.utterance_open {
                match tokio::time::timeout_at(deadline, self.ws_stream.next()).await {
                    Ok(Some(Ok(Message::Text(text)))) => {
                        self.on_text(&text).await;
                    }
                    Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) | Ok(None) | Err(_) => {
                        break;
                    }
                    Ok(Some(Ok(_))) => {}
                }
            }
        }
        let _ = self.ws_sink.close().await;
        ReconnectOutcome::Completed
    }

    async fn send_commit(&mut self) -> Result<(), String> {
        self.ws_sink
            .send(Message::Text(COMMIT_MESSAGE.into()))
            .await
            .map_err(|e| e.to_string())
    }
}

#[async_trait::async_trait]
impl WsTransport for GnaniTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // The featured session is in the upgrade headers: a fresh dial restored it. `connect` is
        // unblocked by Gnani's `connected` message in `run`.
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        let outbound = Arc::clone(&self.outbound);
        let mut pipe = outbound.lock().await;
        let shutdown_token = self.shutdown_token.clone();
        loop {
            if shutdown_token.is_cancelled() {
                drop(pipe);
                return self.finish().await;
            }

            tokio::select! {
                biased;

                _ = shutdown_token.cancelled() => {
                    info!("Received shutdown signal for Gnani STT");
                    drop(pipe);
                    return self.finish().await;
                }

                item = pipe.rx.recv() => {
                    let sent = match item {
                        Some(Outbound::Audio(audio)) => {
                            let frames = pipe.chunker.push(&audio);
                            self.send_frames(frames).await
                        }
                        Some(Outbound::Commit) => self.send_commit().await,
                        None => {
                            // Every sender is gone (the client was dropped).
                            drop(pipe);
                            return self.finish().await;
                        }
                    };
                    if let Err(e) = sent {
                        self.report(STTError::NetworkError(format!(
                            "Failed to send to Gnani: {e}"
                        )))
                        .await;
                        return ReconnectOutcome::Reconnectable(StreamError::new("send failed"));
                    }
                }

                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(Message::Text(text)))) => self.on_text(&text).await,
                        Ok(Some(Ok(Message::Close(frame)))) => {
                            info!("Gnani WebSocket closed by server: {:?}", frame);
                            // 1008: Gnani refused a handshake header (language, format) or the
                            // substitution map — the session's to fix, so not retried. Anything
                            // else (the 15-minute cap, the idle close) reconnects.
                            if frame.as_ref().is_some_and(|f| f.code == CloseCode::Policy) {
                                let reason = frame
                                    .map(|f| f.reason.as_str().to_string())
                                    .filter(|r| !r.is_empty())
                                    .unwrap_or_else(|| "policy violation".to_string());
                                let mut last = self.last_error.lock().unwrap_or_else(|p| p.into_inner());
                                if last.is_none() {
                                    *last = Some(format!("Gnani refused the session: {reason}"));
                                }
                                return ReconnectOutcome::Refused(StreamError::new(reason));
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("server close"));
                        }
                        Ok(Some(Ok(_))) => {
                            // Binary, ping/pong (answered by tungstenite), raw frames.
                        }
                        Ok(Some(Err(e))) => {
                            self.report(STTError::NetworkError(format!(
                                "Gnani WebSocket error: {e}"
                            )))
                            .await;
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("Gnani WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            self.report(STTError::NetworkError(format!(
                                "Gnani WebSocket idle timeout - no message for {}s",
                                WS_MESSAGE_TIMEOUT.as_secs()
                            )))
                            .await;
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }
            }
        }
    }
}

/// Gnani Speech-to-Text client (Prisma realtime streaming).
pub struct GnaniSTT {
    /// Provider-specific configuration
    config: Option<GnaniSTTConfig>,
    /// Ready: Gnani sent `connected` and the supervisor is running.
    is_ready: Arc<AtomicBool>,
    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before cancelling the shutdown token, so a client close
    /// racing a server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,
    on_result: Arc<RwLock<Option<STTResultCallback>>>,
    on_error: Arc<RwLock<Option<STTErrorCallback>>>,
    /// Audio and commits for the supervised transport (bounded for backpressure).
    outbound_tx: Option<mpsc::Sender<Outbound>>,
    shutdown_token: Option<CancellationToken>,
    /// The reconnect supervisor task.
    connection_handle: Option<tokio::task::JoinHandle<()>>,
    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`] supervisor. `None` before `set_resilience` (a direct
    /// unit-test construction) → the supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl Default for GnaniSTT {
    fn default() -> Self {
        Self {
            config: None,
            is_ready: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            on_result: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            outbound_tx: None,
            shutdown_token: None,
            connection_handle: None,
            resilience: None,
        }
    }
}

impl GnaniSTT {
    /// W1 keystone — construct from the standardized config (`format` extra, endpoint override).
    /// Credentials and languages are validated at `connect()`, as with `BaseSTT::new`.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let gnani_config =
            GnaniSTTConfig::from_standard(std).map_err(STTError::ConfigurationError)?;
        let mut stt = Self::default();
        stt.config = Some(gnani_config);
        Ok(stt)
    }

    /// Create a new Gnani STT instance
    pub fn create(config: STTConfig) -> Result<Self, STTError> {
        let gnani_config =
            GnaniSTTConfig::from_base(config).map_err(STTError::ConfigurationError)?;
        let mut stt = Self::default();
        stt.config = Some(gnani_config);
        Ok(stt)
    }

    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `GnaniSTT` built from
    /// the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
}

/// Dial Gnani with the session's configuration on the upgrade request.
async fn dial(url: &str, headers: &[(&'static str, String)]) -> Result<GnaniWs, StreamError> {
    // `into_client_request` derives the mandatory WebSocket handshake headers from the URL
    // (repo convention); Gnani's configuration rides on top.
    let mut request = url
        .into_client_request()
        .map_err(|e| StreamError::new(format!("Failed to create WebSocket request: {e}")))?;
    for (name, value) in headers {
        let value = HeaderValue::from_str(value)
            .map_err(|e| StreamError::new(format!("Invalid {name} header: {e}")))?;
        request
            .headers_mut()
            .insert(HeaderName::from_static(name), value);
    }
    // No Nagle: the commit frame leaves at once.
    let (ws, response) = with_timeout(
        WS_CONNECT_TIMEOUT,
        connect_async_with_config(request, None, true),
    )
    .await
    .map_err(|_| {
        StreamError::new(format!(
            "connect to Gnani timed out after {}s",
            WS_CONNECT_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|e| StreamError::new(format!("Failed to connect to Gnani: {e}")))?;
    debug!("Gnani WebSocket connected: {:?}", response.status());
    Ok(ws)
}

#[async_trait::async_trait]
impl BaseSTT for GnaniSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        GnaniSTT::create(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.is_ready() {
            return Ok(());
        }
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| STTError::ConfigurationError("No configuration set".to_string()))?;
        config.validate().map_err(|e| {
            if config.api_key.is_empty() {
                STTError::AuthenticationFailed(e)
            } else {
                STTError::ConfigurationError(e)
            }
        })?;
        let url = config.ws_url();
        let headers = config
            .upgrade_headers()
            .map_err(STTError::ConfigurationError)?;
        let sample_rate = config.base.sample_rate;
        info!(
            language = %config.base.language,
            model = %config.model.as_str(),
            sample_rate,
            "Connecting to Gnani STT"
        );

        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        let (outbound_tx, outbound_rx) = mpsc::channel::<Outbound>(64);
        let shutdown_token = CancellationToken::new();
        let (connected_tx, connected_rx) = oneshot::channel::<()>();
        self.outbound_tx = Some(outbound_tx);
        self.shutdown_token = Some(shutdown_token.clone());

        let outbound = Arc::new(Mutex::new(OutboundPipe {
            rx: outbound_rx,
            chunker: FrameChunker::new(),
        }));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));
        let last_error: Arc<std::sync::Mutex<Option<String>>> = Arc::default();
        let on_result = self.on_result.clone();
        let on_error = self.on_error.clone();

        // Storm control + provider breaker: the generic supervisor with the shared process-global
        // handles (W-D1/W-D2); its own per-session default when none were injected.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("gnani", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => ReconnectableStream::new(ReconnectableStreamConfig::new("gnani", reconnection)),
        }
        .with_disconnect_flag(disconnect_flag);

        let is_ready = Arc::clone(&self.is_ready);
        let task_last_error = Arc::clone(&last_error);
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let url = url.clone();
                    let headers = headers.clone();
                    let outbound = Arc::clone(&outbound);
                    let shutdown_token = shutdown_token.clone();
                    let connected_tx = Arc::clone(&connected_tx);
                    let on_result = on_result.clone();
                    let on_error = on_error.clone();
                    let last_error = Arc::clone(&task_last_error);
                    async move {
                        let ws = match dial(&url, &headers).await {
                            Ok(ws) => ws,
                            Err(e) => {
                                *last_error.lock().unwrap_or_else(|p| p.into_inner()) =
                                    Some(e.0.clone());
                                return Err(e);
                            }
                        };
                        let (ws_sink, ws_stream) = ws.split();
                        Ok(GnaniTransport {
                            ws_sink,
                            ws_stream,
                            outbound,
                            shutdown_token,
                            connected_tx,
                            on_result,
                            on_error,
                            last_error,
                            sample_rate,
                            utterance_open: false,
                        })
                    }
                })
                .await;
            info!("Gnani STT connection closed (supervisor exit: {exit:?})");
            is_ready.store(false, Ordering::SeqCst);
        });
        self.connection_handle = Some(connection_handle);

        let failure = |what: &str| {
            let detail = last_error.lock().unwrap_or_else(|p| p.into_inner()).clone();
            match detail {
                Some(d) if d.contains("401") || d.contains("403") => {
                    STTError::AuthenticationFailed(format!("{what}: {d}"))
                }
                Some(d) => STTError::ConnectionFailed(format!("{what}: {d}")),
                None => STTError::ConnectionFailed(what.to_string()),
            }
        };
        match timeout(CONNECTED_TIMEOUT, connected_rx).await {
            Ok(Ok(())) => {
                self.is_ready.store(true, Ordering::SeqCst);
                info!("Connected to Gnani STT");
                Ok(())
            }
            Ok(Err(_)) => {
                let err = failure("Gnani closed the session before it was ready");
                self.disconnect().await?;
                Err(err)
            }
            Err(_) => {
                let err = failure("Timed out waiting for Gnani's connected message");
                self.disconnect().await?;
                Err(err)
            }
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE cancelling the shutdown token so the supervisor sees it even
        // if the transport's run() just reported a reconnectable drop.
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        self.is_ready.store(false, Ordering::SeqCst);
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }
        if let Some(handle) = self.connection_handle.take() {
            crate::core::observability::await_task_shutdown(
                "gnani-stt-connection",
                handle,
                Duration::from_secs(5),
            )
            .await;
        }
        self.outbound_tx = None;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_ready.load(Ordering::SeqCst) && self.outbound_tx.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }
        let sender = self
            .outbound_tx
            .as_ref()
            .ok_or_else(|| STTError::ConnectionFailed("Not connected".to_string()))?;
        sender
            .send(Outbound::Audio(audio_data))
            .await
            .map_err(|_| STTError::ConnectionFailed("Not connected".to_string()))
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.on_result.write().await = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.on_error.write().await = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.config.as_ref().map(|c| &c.base)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        let mut gnani_config =
            GnaniSTTConfig::from_base(config).map_err(STTError::ConfigurationError)?;
        // Keep what only the standardized config sets.
        if let Some(old) = &self.config {
            gnani_config.format = old.format;
            gnani_config.endpoint_override = old.endpoint_override.clone();
        }
        self.config = Some(gnani_config);
        if self.is_ready() {
            self.disconnect().await?;
            self.connect().await?;
        }
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Gnani.ai Vachana STT - Indic speech-to-text, Prisma realtime streaming over WebSocket"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Every Gnani session trips the same breaker and shares the one process-wide reconnect
        // cap (W-D2).
        self.resilience = Some(resilience);
    }

    /// Gnani closes a segment after 500 ms of silence (its default `x-min-silence-ms`).
    fn end_turn_after_ms(&self) -> Option<u32> {
        Some(GNANI_MIN_SILENCE_MS)
    }

    /// `{"type":"commit"}`: Gnani closes the open utterance now and sends its transcript.
    fn end_turn(&mut self) -> bool {
        self.is_ready()
            && self
                .outbound_tx
                .as_ref()
                .is_some_and(|tx| tx.try_send(Outbound::Commit).is_ok())
    }
}

impl Drop for GnaniSTT {
    fn drop(&mut self) {
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }
        if let Some(handle) = self.connection_handle.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::STTResult;
    use crate::core::stt::gnani::messages::GNANI_STT_FRAME_BYTES;

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "gnani".to_string(),
            api_key: String::new(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: "default".to_string(),
        }
    }

    #[test]
    fn test_gnani_stt_creation() {
        assert!(GnaniSTT::create(create_test_config()).is_ok());
    }

    #[test]
    fn new_standard_carries_format_and_model() {
        use crate::core::stt::standard::StandardSTTConfig;
        let mut std = StandardSTTConfig::from_base(STTConfig {
            api_key: "key-1".into(),
            model: "prisma-v3.0".into(),
            ..create_test_config()
        });
        std.extras
            .0
            .insert("format".into(), serde_json::json!("transcribe"));
        let stt = GnaniSTT::new_standard(&std).expect("new_standard should succeed");
        let cfg = stt.config.as_ref().expect("config should be set");
        assert_eq!(cfg.api_key, "key-1");
        assert_eq!(cfg.model.as_str(), "prisma-v3.0");
        assert_eq!(cfg.format.as_str(), "transcribe");
    }

    #[test]
    fn test_gnani_stt_not_connected_initially() {
        let stt = GnaniSTT::create(create_test_config()).unwrap();
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_gnani_stt_send_audio_requires_connection() {
        let mut stt = GnaniSTT::create(create_test_config()).unwrap();
        match stt.send_audio(Bytes::from_static(b"test")).await {
            Err(STTError::ConnectionFailed(msg)) => assert!(msg.contains("Not connected")),
            other => panic!("Expected ConnectionFailed error, got {other:?}"),
        }
    }

    #[test]
    fn test_gnani_stt_provider_info() {
        let stt = GnaniSTT::create(create_test_config()).unwrap();
        assert!(stt.get_provider_info().contains("Gnani"));
        assert!(stt.get_provider_info().contains("WebSocket"));
    }

    #[tokio::test]
    async fn test_gnani_stt_connect_requires_a_key() {
        if std::env::var_os("GNANI_TOKEN").is_some() {
            return; // a configured key would be used
        }
        let mut stt = GnaniSTT::create(create_test_config()).unwrap();
        assert!(matches!(
            stt.connect().await,
            Err(STTError::AuthenticationFailed(_))
        ));
    }

    #[tokio::test]
    async fn test_gnani_stt_connect_refuses_urdu_before_dialing() {
        let mut stt = GnaniSTT::create(STTConfig {
            api_key: "key".into(),
            language: "ur-IN".into(),
            ..create_test_config()
        })
        .unwrap();
        match stt.connect().await {
            Err(STTError::ConfigurationError(msg)) => assert!(msg.contains("Urdu"), "{msg}"),
            other => panic!("Expected ConfigurationError, got {other:?}"),
        }
    }

    #[test]
    fn end_turn_is_false_when_not_connected() {
        let mut stt = GnaniSTT::create(create_test_config()).unwrap();
        assert!(!stt.end_turn());
        assert_eq!(stt.end_turn_after_ms(), Some(500));
    }

    #[tokio::test]
    async fn test_gnani_stt_disconnect_when_not_connected() {
        let mut stt = GnaniSTT::create(create_test_config()).unwrap();
        assert!(stt.disconnect().await.is_ok());
    }

    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let mut stt = GnaniSTT::create(create_test_config()).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    /// What the mock Gnani server saw.
    #[derive(Debug, PartialEq)]
    enum Seen {
        Binary(usize),
        Text(String),
    }

    /// A local Gnani: records the upgrade headers, sends `connected`, records every frame, and
    /// answers each `commit` with the next transcript.
    async fn spawn_mock_gnani(
        transcripts: Vec<&'static str>,
    ) -> (
        u16,
        oneshot::Receiver<Vec<(String, String)>>,
        mpsc::UnboundedReceiver<Seen>,
    ) {
        use tokio::net::TcpListener;
        use tokio_tungstenite::accept_hdr_async;
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (headers_tx, headers_rx) = oneshot::channel();
        let (seen_tx, seen_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept mock client");
            let mut headers_tx = Some(headers_tx);
            let callback = |req: &Request, resp: Response| {
                let headers = req
                    .headers()
                    .iter()
                    .map(|(n, v)| (n.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect::<Vec<_>>();
                if let Some(tx) = headers_tx.take() {
                    let _ = tx.send(headers);
                }
                Ok(resp)
            };
            let ws = accept_hdr_async(stream, callback)
                .await
                .expect("accept websocket");
            let (mut write, mut read) = ws.split();
            write
                .send(Message::Text(
                    r#"{"type":"connected","message":"STT service ready — VAD service connected","config":{"sample_rate":16000,"chunk_size":512}}"#.into(),
                ))
                .await
                .unwrap();
            let mut transcripts = transcripts.into_iter();
            let mut segment = 0;
            while let Some(Ok(frame)) = read.next().await {
                match frame {
                    Message::Binary(data) => {
                        let _ = seen_tx.send(Seen::Binary(data.len()));
                    }
                    Message::Text(text) => {
                        let _ = seen_tx.send(Seen::Text(text.to_string()));
                        if text.contains("commit")
                            && let Some(t) = transcripts.next()
                        {
                            segment += 1;
                            let _ = write
                                .send(Message::Text(
                                    r#"{"type":"processing","timestamp":"t"}"#.into(),
                                ))
                                .await;
                            let _ = write
                                .send(Message::Text(
                                    format!(
                                        r#"{{"type":"transcript","text":"{t}","audio_duration_ms":1200,"segment_id":"mock-seg-{segment}","segment_index":{segment},"latency":80}}"#
                                    )
                                    .into(),
                                ))
                                .await;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        });
        (port, headers_rx, seen_rx)
    }

    async fn next_seen(rx: &mut mpsc::UnboundedReceiver<Seen>) -> Seen {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("the mock should see a frame")
            .expect("the mock is running")
    }

    /// End to end against a local Gnani: the upgrade carries the session, audio arrives as exact
    /// 1,024-byte binary frames with the remainder held back, `end_turn` and `disconnect` send
    /// `commit`, and each `transcript` reaches the result callback as a final result.
    #[tokio::test(flavor = "current_thread")]
    // The env-lock guard spans connect(): validate() reads WAAV_ALLOW_LOOPBACK_ENDPOINTS there.
    #[allow(clippy::await_holding_lock)]
    async fn streams_to_a_local_gnani_end_to_end() {
        use crate::core::stt::standard::StandardSTTConfig;

        let (port, headers_rx, mut seen) = spawn_mock_gnani(vec!["hello world", "goodbye"]).await;
        let std_cfg = StandardSTTConfig::from_base(STTConfig {
            api_key: "test-gnani-key".into(),
            language: "pa-guru-IN".into(),
            model: "prisma-v3.0".into(),
            ..create_test_config()
        })
        .with_endpoint_override(format!("ws://127.0.0.1:{port}"));

        let (result_tx, mut results) = mpsc::unbounded_channel::<STTResult>();
        let mut stt = {
            let _guard = crate::core::net::ssrf_env_lock();
            let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
            // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
            unsafe { std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1") };

            let mut stt = GnaniSTT::new_standard(&std_cfg).unwrap();
            stt.on_result(Arc::new(move |r: STTResult| {
                let tx = result_tx.clone();
                Box::pin(async move {
                    let _ = tx.send(r);
                })
            }))
            .await
            .unwrap();
            let connected = stt.connect().await;

            // SAFETY: restore the process env before releasing the test env lock.
            unsafe {
                match previous {
                    Some(previous) => std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous),
                    None => std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS"),
                }
            }
            connected.expect("connect to the mock Gnani");
            stt
        };
        assert!(stt.is_ready());

        let headers = headers_rx.await.unwrap();
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(header("x-api-key-id").as_deref(), Some("test-gnani-key"));
        assert_eq!(header("language_code").as_deref(), Some("pa-IN"));
        assert_eq!(header("model").as_deref(), Some("prisma-v3.0"));
        assert_eq!(header("x-sample-rate").as_deref(), Some("16000"));
        assert_eq!(header("format").as_deref(), Some("verbatim"));

        // 5 × 700 = 3,500 bytes: three whole frames, 428 bytes held back.
        for _ in 0..5 {
            stt.send_audio(Bytes::from(vec![0u8; 700])).await.unwrap();
        }
        for _ in 0..3 {
            assert_eq!(
                next_seen(&mut seen).await,
                Seen::Binary(GNANI_STT_FRAME_BYTES)
            );
        }

        assert!(stt.end_turn(), "end_turn must queue a commit");
        assert_eq!(
            next_seen(&mut seen).await,
            Seen::Text(COMMIT_MESSAGE.to_string())
        );
        let first = tokio::time::timeout(Duration::from_secs(5), results.recv())
            .await
            .expect("a transcript should arrive")
            .unwrap();
        assert_eq!(first.transcript, "hello world");
        assert!(first.is_final && first.is_speech_final);
        assert_eq!(first.vendor_request_id.as_deref(), Some("mock-seg-1"));

        // 428 held back + 700 = one more frame. Disconnect commits the open utterance and waits
        // for its transcript.
        stt.send_audio(Bytes::from(vec![0u8; 700])).await.unwrap();
        assert_eq!(
            next_seen(&mut seen).await,
            Seen::Binary(GNANI_STT_FRAME_BYTES)
        );
        stt.disconnect().await.unwrap();
        assert_eq!(
            next_seen(&mut seen).await,
            Seen::Text(COMMIT_MESSAGE.to_string())
        );
        let second = results
            .try_recv()
            .expect("disconnect waits for the commit's transcript");
        assert_eq!(second.transcript, "goodbye");
        assert!(second.is_final);
        assert!(!stt.is_ready());
    }

    /// A 1008 close (Gnani refusing a handshake header) is not retried, and connect reports it.
    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn a_refused_session_fails_connect_with_gnanis_message() {
        use crate::core::stt::standard::StandardSTTConfig;
        use tokio::net::TcpListener;
        use tokio_tungstenite::accept_async;
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&accepts);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut ws = accept_async(stream).await.unwrap();
                let _ = ws
                    .send(Message::Text(
                        r#"{"type":"error","message":"invalid language_code"}"#.into(),
                    ))
                    .await;
                let _ = ws
                    .send(Message::Close(Some(CloseFrame {
                        code: CloseCode::Policy,
                        reason: "invalid language_code".into(),
                    })))
                    .await;
            }
        });

        let std_cfg = StandardSTTConfig::from_base(STTConfig {
            api_key: "k".into(),
            ..create_test_config()
        })
        .with_endpoint_override(format!("ws://127.0.0.1:{port}"));
        let _guard = crate::core::net::ssrf_env_lock();
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1") };
        let mut stt = GnaniSTT::new_standard(&std_cfg).unwrap();
        let result = stt.connect().await;
        // SAFETY: restore the process env before releasing the test env lock.
        unsafe {
            match previous {
                Some(previous) => std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous),
                None => std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS"),
            }
        }
        match result {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("invalid language_code"), "{msg}")
            }
            other => panic!("Expected ConnectionFailed, got {other:?}"),
        }
        assert_eq!(
            accepts.load(Ordering::SeqCst),
            1,
            "a refusal is not retried"
        );
    }
}

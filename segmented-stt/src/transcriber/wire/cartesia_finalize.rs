//! `cartesia_manual_finalize`: Cartesia's streaming socket, on which the gateway's detector ends
//! each utterance with `finalize` (the commit transport for Cartesia, Release 4, addendum B8).
//!
//! Cartesia's own client lets the vendor end utterances, and it ends one more than once; here the
//! gateway decides. One socket per session: each call sends one utterance as 16 kHz PCM, then the
//! text command `finalize`, and returns the final transcripts Cartesia sends before `flush_done`.
//! The language is part of the socket's address, so a pinned language (the vote) opens a new
//! socket. Any error or a missed deadline closes the socket.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::redact;
use crate::transcriber::{
    RequestPhase, RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo, TranscriberKind,
};
use crate::types::ErrorClass;

pub const DEFAULT_BASE: &str = "wss://api.cartesia.ai";
/// The API version Cartesia's streaming client is tested with.
pub const API_VERSION: &str = "2025-04-16";
/// Audio per binary frame: 100 ms at 16 kHz.
const FRAME_SAMPLES: usize = 1_600;

#[derive(Clone)]
pub struct CartesiaFinalizeConfig {
    /// `wss://api.cartesia.ai`, or a deployment's own address.
    pub base: String,
    pub api_key: String,
    pub model: String,
    pub version: String,
    pub connect_timeout: Duration,
}

impl std::fmt::Debug for CartesiaFinalizeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CartesiaFinalizeConfig")
            .field("base", &self.base)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl CartesiaFinalizeConfig {
    pub fn new(api_key: &str, model: &str) -> Self {
        Self {
            base: DEFAULT_BASE.into(),
            api_key: api_key.into(),
            model: if model.trim().is_empty() {
                "ink-whisper".into()
            } else {
                model.trim().into()
            },
            version: API_VERSION.into(),
            connect_timeout: Duration::from_millis(3_000),
        }
    }

    /// The socket address for one language. The key travels in the query, as Cartesia documents
    /// for sockets; it never appears in an error.
    fn url(&self, language: Option<&str>) -> String {
        let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
        let mut u = format!(
            "{}/stt/websocket?api_key={}&model={}&encoding=pcm_s16le&sample_rate=16000&cartesia_version={}",
            self.base.trim_end_matches('/'),
            enc(&self.api_key),
            enc(&self.model),
            enc(&self.version),
        );
        if let Some(l) = language {
            u.push_str("&language=");
            u.push_str(&enc(l));
        }
        u
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Open {
    ws: Socket,
    language: Option<String>,
}

pub struct CartesiaFinalizeTranscriber {
    cfg: CartesiaFinalizeConfig,
    info: TranscriberInfo,
    socket: tokio::sync::Mutex<Option<Open>>,
}

impl CartesiaFinalizeTranscriber {
    pub fn new(cfg: CartesiaFinalizeConfig) -> Result<Self, String> {
        let u = url::Url::parse(&cfg.base).map_err(|e| format!("invalid Cartesia address: {e}"))?;
        if !matches!(u.scheme(), "ws" | "wss") {
            return Err("the Cartesia socket address must be ws:// or wss://".into());
        }
        let host_key = crate::transcriber::http::host_key(
            &cfg.base
                .replacen("wss://", "https://", 1)
                .replacen("ws://", "http://", 1),
        );
        let mut info = TranscriberInfo::file("cartesia_manual_finalize", &host_key, &cfg.model);
        info.kind = TranscriberKind::Commit;
        info.droppable_fields = vec!["language".into()];
        Ok(Self {
            cfg,
            info,
            socket: tokio::sync::Mutex::new(None),
        })
    }

    async fn connect(&self, language: Option<&str>) -> Result<Socket, SegmentError> {
        let url = self.cfg.url(language);
        let secret = Some(self.cfg.api_key.as_str());
        match tokio::time::timeout(
            self.cfg.connect_timeout,
            tokio_tungstenite::connect_async(url.as_str()),
        )
        .await
        {
            Err(_) => Err(SegmentError::new(
                ErrorClass::Network,
                "the Cartesia socket did not open in time",
            )
            .with_phase(RequestPhase::BeforeSend)),
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(resp))) => {
                let body = resp
                    .body()
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).to_string())
                    .unwrap_or_default();
                Err(SegmentError::from_status(
                    resp.status().as_u16(),
                    redact(&format!("Cartesia refused the socket: {body}"), secret),
                ))
            }
            Ok(Err(e)) => Err(SegmentError::new(
                ErrorClass::Network,
                redact(&format!("Cartesia socket: {e}"), secret),
            )
            .with_phase(RequestPhase::BeforeSend)),
            Ok(Ok((ws, _))) => Ok(ws),
        }
    }

    async fn run(
        &self,
        open: &mut Open,
        audio: &SegmentAudio,
        progress: &RequestProgress,
    ) -> Result<SegmentTranscript, SegmentError> {
        progress.mark_sent();
        for chunk in audio.pcm.chunks(FRAME_SAMPLES) {
            let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
            open.ws
                .send(Message::Binary(bytes.into()))
                .await
                .map_err(|e| {
                    SegmentError::new(ErrorClass::Network, format!("Cartesia socket: {e}"))
                })?;
        }
        open.ws
            .send(Message::Text("finalize".into()))
            .await
            .map_err(|e| SegmentError::new(ErrorClass::Network, format!("Cartesia socket: {e}")))?;
        let mut finals: Vec<String> = Vec::new();
        let mut language = None;
        loop {
            let text = match open.ws.next().await {
                Some(Ok(Message::Text(t))) => t,
                Some(Ok(Message::Close(_))) | None => {
                    return Err(SegmentError::new(
                        ErrorClass::Network,
                        "the Cartesia socket closed",
                    )
                    .with_phase(phase(progress)));
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => {
                    return Err(SegmentError::new(
                        ErrorClass::Network,
                        format!("Cartesia socket: {e}"),
                    )
                    .with_phase(phase(progress)));
                }
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            match v.get("type").and_then(Value::as_str).unwrap_or_default() {
                "transcript" => {
                    progress.mark_headers();
                    if v.get("is_final").and_then(Value::as_bool) == Some(true) {
                        let t = v
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .trim();
                        if !t.is_empty() {
                            finals.push(t.to_string());
                        }
                        if let Some(l) = v.get("language").and_then(Value::as_str) {
                            language = Some(l.to_string());
                        }
                    }
                }
                "flush_done" => {
                    progress.mark_headers();
                    return Ok(SegmentTranscript {
                        text: finals.join(" "),
                        detected_language: language,
                        ..Default::default()
                    });
                }
                "error" => {
                    let message = v
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Cartesia reported an error");
                    let lower = message.to_ascii_lowercase();
                    let class = if lower.contains("api key") || lower.contains("unauthor") {
                        ErrorClass::Auth
                    } else if lower.contains("rate") || lower.contains("concurren") {
                        ErrorClass::RateLimited
                    } else if lower.contains("model") {
                        ErrorClass::ModelNotServed
                    } else {
                        ErrorClass::Vendor
                    };
                    return Err(
                        SegmentError::new(class, redact(message, Some(&self.cfg.api_key)))
                            .with_phase(phase(progress)),
                    );
                }
                _ => {}
            }
        }
    }
}

fn phase(progress: &RequestProgress) -> RequestPhase {
    if progress.headers_received() {
        RequestPhase::Headers
    } else {
        RequestPhase::Sent
    }
}

fn primary(lang: &str) -> String {
    let l = lang.trim();
    l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase()
}

#[async_trait::async_trait]
impl SegmentTranscriber for CartesiaFinalizeTranscriber {
    fn info(&self) -> &TranscriberInfo {
        &self.info
    }

    async fn transcribe(
        &self,
        audio: &SegmentAudio,
        ctx: &SegmentContext,
        timeout: Duration,
        progress: &RequestProgress,
    ) -> Result<SegmentTranscript, SegmentError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let language = ctx
            .language
            .as_deref()
            .filter(|_| !(ctx.minimal || ctx.omit_fields.iter().any(|f| f == "language")))
            .map(primary);
        let mut guard = tokio::time::timeout_at(deadline, self.socket.lock())
            .await
            .map_err(|_| {
                SegmentError::new(ErrorClass::Timeout, "the Cartesia socket was busy")
                    .with_phase(RequestPhase::BeforeSend)
            })?;
        if guard.as_ref().is_some_and(|o| o.language != language)
            && let Some(mut o) = guard.take()
        {
            let _ = o.ws.close(None).await;
        }
        if guard.is_none() {
            let ws = tokio::time::timeout_at(deadline, self.connect(language.as_deref()))
                .await
                .map_err(|_| {
                    SegmentError::new(
                        ErrorClass::Timeout,
                        "the Cartesia socket did not open in time",
                    )
                    .with_phase(RequestPhase::BeforeSend)
                })??;
            *guard = Some(Open {
                ws,
                language: language.clone(),
            });
        }
        let open = guard.as_mut().expect("opened above");
        let result = match tokio::time::timeout_at(deadline, self.run(open, audio, progress)).await
        {
            Ok(r) => r,
            Err(_) => Err(SegmentError::new(
                ErrorClass::Timeout,
                "no transcript before the deadline",
            )
            .with_phase(phase(progress))),
        };
        if result.is_err()
            && let Some(mut o) = guard.take()
        {
            let _ = o.ws.close(None).await;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::RawQuery;
    use axum::extract::ws::{Message as AxMessage, WebSocket, WebSocketUpgrade};
    use parking_lot::Mutex;

    #[derive(Default)]
    struct Seen {
        queries: Mutex<Vec<String>>,
        bytes_per_finalize: Mutex<Vec<usize>>,
        connections: AtomicUsize,
    }

    async fn serve(seen: Arc<Seen>, fail: bool, ws: WebSocket) {
        let (mut tx, mut rx) = ws.split();
        let mut bytes = 0usize;
        let mut n = 0;
        while let Some(Ok(m)) = rx.next().await {
            match m {
                AxMessage::Binary(b) => bytes += b.len(),
                AxMessage::Text(t) if t.as_str() == "finalize" => {
                    seen.bytes_per_finalize.lock().push(bytes);
                    bytes = 0;
                    n += 1;
                    if fail {
                        let _ = tx
                            .send(AxMessage::Text(
                                r#"{"type":"error","message":"Invalid API key"}"#.into(),
                            ))
                            .await;
                        continue;
                    }
                    for m in [
                        format!(r#"{{"type":"transcript","text":"part {n}","is_final":false}}"#),
                        format!(
                            r#"{{"type":"transcript","text":"hello","is_final":true,"language":"en"}}"#
                        ),
                        format!(r#"{{"type":"transcript","text":"world {n}","is_final":true}}"#),
                        r#"{"type":"flush_done"}"#.to_string(),
                    ] {
                        let _ = tx.send(AxMessage::Text(m.into())).await;
                    }
                }
                _ => {}
            }
        }
    }

    async fn mock(fail: bool) -> (String, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        let s = Arc::clone(&seen);
        let app = axum::Router::new().route(
            "/stt/websocket",
            axum::routing::get(move |ws: WebSocketUpgrade, RawQuery(q): RawQuery| {
                let s = Arc::clone(&s);
                async move {
                    s.connections.fetch_add(1, Ordering::SeqCst);
                    s.queries.lock().push(q.unwrap_or_default());
                    ws.on_upgrade(move |socket| serve(s, fail, socket))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("ws://{addr}"), seen)
    }

    fn transcriber(base: &str) -> CartesiaFinalizeTranscriber {
        let mut c = CartesiaFinalizeConfig::new(super::super::testkit::SECRET, "ink-whisper");
        c.base = base.into();
        CartesiaFinalizeTranscriber::new(c).unwrap()
    }

    fn ctx(lang: Option<&str>) -> SegmentContext {
        SegmentContext {
            language: lang.map(str::to_string),
            ..Default::default()
        }
    }

    fn audio(ms: usize) -> SegmentAudio {
        SegmentAudio::new(vec![100; 16 * ms])
    }

    #[tokio::test]
    async fn each_utterance_is_finalized_and_its_finals_returned_on_one_socket() {
        let (base, seen) = mock(false).await;
        let t = transcriber(&base);
        assert_eq!(t.info().kind, TranscriberKind::Commit);
        let p = RequestProgress::new();
        let a = t
            .transcribe(
                &audio(1000),
                &ctx(Some("en-US")),
                Duration::from_secs(5),
                &p,
            )
            .await
            .unwrap();
        assert_eq!(a.text, "hello world 1", "finals only, in order");
        assert_eq!(a.detected_language.as_deref(), Some("en"));
        let b = t
            .transcribe(
                &audio(500),
                &ctx(Some("en")),
                Duration::from_secs(5),
                &RequestProgress::new(),
            )
            .await
            .unwrap();
        assert_eq!(b.text, "hello world 2");
        assert_eq!(seen.connections.load(Ordering::SeqCst), 1);
        assert_eq!(
            *seen.bytes_per_finalize.lock(),
            vec![32_000, 16_000],
            "16 kHz PCM as sent"
        );
        let q = seen.queries.lock()[0].clone();
        for part in [
            "model=ink-whisper",
            "encoding=pcm_s16le",
            "sample_rate=16000",
            "cartesia_version=2025-04-16",
            "language=en",
        ] {
            assert!(q.contains(part), "{q}");
        }
    }

    #[tokio::test]
    async fn a_new_language_opens_a_new_socket() {
        let (base, seen) = mock(false).await;
        let t = transcriber(&base);
        t.transcribe(
            &audio(200),
            &ctx(None),
            Duration::from_secs(5),
            &RequestProgress::new(),
        )
        .await
        .unwrap();
        t.transcribe(
            &audio(200),
            &ctx(Some("de")),
            Duration::from_secs(5),
            &RequestProgress::new(),
        )
        .await
        .unwrap();
        let q = seen.queries.lock().clone();
        assert_eq!(q.len(), 2);
        assert!(!q[0].contains("language="));
        assert!(q[1].contains("language=de"));
    }

    #[tokio::test]
    async fn a_refused_key_is_auth_and_never_echoed() {
        let (base, seen) = mock(true).await;
        let t = transcriber(&base);
        let e = t
            .transcribe(
                &audio(200),
                &ctx(None),
                Duration::from_secs(5),
                &RequestProgress::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(e.class, ErrorClass::Auth);
        assert!(!e.message.contains(super::super::testkit::SECRET));
        t.transcribe(
            &audio(200),
            &ctx(None),
            Duration::from_secs(5),
            &RequestProgress::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            seen.connections.load(Ordering::SeqCst),
            2,
            "an error closes the socket"
        );
    }

    #[test]
    fn the_debug_output_never_prints_the_key() {
        let c = CartesiaFinalizeConfig::new("sk-secret-123456789", "ink-whisper");
        assert!(!format!("{c:?}").contains("sk-secret"));
    }
}

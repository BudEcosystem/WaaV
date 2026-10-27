//! `openai_realtime_transcription`: OpenAI's realtime transcription socket, on which the
//! gateway's detector commits each utterance (the *commit transport*, Release 4).
//!
//! The only way to reach OpenAI's and Azure OpenAI's live-only models on a call
//! (`gpt-live-transcribe`, `gpt-realtime-whisper`), and the low-latency route to `gpt-transcribe`.
//! To the attempt loop it is an ordinary [`SegmentTranscriber`] of kind
//! [`TranscriberKind::Commit`]: one socket per session, opened on first use and kept; each call
//! appends one utterance (resampled to the 24 kHz the socket takes), commits it, and returns the
//! `conversation.item.input_audio_transcription.completed` text for that item. Calls are serialised
//! on the socket, and the attempt loop never sends a second request on it.
//!
//! The session is configured with turn detection off, so the vendor never ends an utterance itself.
//! It is reconfigured (`session.update`) only when the model, the language or the context changes,
//! which is how the session language vote reaches it. Any error closes the socket; the next call
//! opens a new one, so a late answer for an abandoned item can never be read as the next one's.

use std::time::Duration;

use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::openai_compat::LanguageDialect;
use super::{Auth, redact};
use crate::transcriber::{
    RequestPhase, RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo, TranscriberKind,
};
use crate::types::ErrorClass;

/// The socket's audio rate.
pub const SOCKET_RATE: u32 = 24_000;
/// Audio per `input_audio_buffer.append`: 0.5 s at 24 kHz, about 32 KB of base64.
const APPEND_SAMPLES: usize = 12_000;

#[derive(Debug, Clone)]
pub struct OpenAiRealtimeConfig {
    /// `wss://…/v1/realtime?intent=transcription`.
    pub url: String,
    pub auth: Auth,
    pub model: String,
    pub language: LanguageDialect,
    pub send_prompt: bool,
    pub send_keywords: bool,
    pub connect_timeout: Duration,
}

impl OpenAiRealtimeConfig {
    pub fn new(url: &str, auth: Auth, model: &str) -> Self {
        Self {
            url: url.to_string(),
            auth,
            model: model.to_string(),
            language: LanguageDialect::single("language"),
            send_prompt: false,
            send_keywords: false,
            connect_timeout: Duration::from_millis(3_000),
        }
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Open {
    ws: Socket,
    /// The `session.update` last sent, to send it again only on a change.
    configured: Option<Value>,
}

pub struct OpenAiRealtimeTranscriber {
    cfg: OpenAiRealtimeConfig,
    info: TranscriberInfo,
    socket: tokio::sync::Mutex<Option<Open>>,
}

impl OpenAiRealtimeTranscriber {
    pub fn new(cfg: OpenAiRealtimeConfig) -> Result<Self, String> {
        let url =
            url::Url::parse(&cfg.url).map_err(|e| format!("invalid realtime address: {e}"))?;
        if !matches!(url.scheme(), "ws" | "wss") {
            return Err("the realtime transcription address must be ws:// or wss://".into());
        }
        let host_key = crate::transcriber::http::host_key(
            &cfg.url
                .replacen("wss://", "https://", 1)
                .replacen("ws://", "http://", 1),
        );
        let mut info =
            TranscriberInfo::file("openai_realtime_transcription", &host_key, &cfg.model);
        info.kind = TranscriberKind::Commit;
        info.droppable_fields = vec!["prompt".into(), "keywords".into(), "language".into()];
        Ok(Self {
            cfg,
            info,
            socket: tokio::sync::Mutex::new(None),
        })
    }

    /// The `session.update` this call needs.
    fn session_update(&self, ctx: &SegmentContext) -> Value {
        let mut transcription = serde_json::Map::new();
        transcription.insert("model".into(), json!(self.cfg.model));
        let omitted = |f: &str| ctx.minimal || ctx.omit_fields.iter().any(|o| o == f);
        if let Some(param) = self
            .cfg
            .language
            .param
            .as_deref()
            .filter(|_| !omitted("language"))
        {
            let langs: Vec<String> = match ctx.language.as_deref() {
                Some(l) => vec![primary(l)],
                None => ctx.candidate_languages.iter().map(|l| primary(l)).collect(),
            };
            if !langs.is_empty() {
                if self.cfg.language.list {
                    transcription.insert(param.into(), json!(langs));
                } else if langs.len() == 1 {
                    transcription.insert(param.into(), json!(langs[0]));
                }
            }
        }
        if self.cfg.send_prompt
            && !omitted("prompt")
            && let Some(p) = ctx.prompt.as_deref().filter(|p| !p.trim().is_empty())
        {
            transcription.insert("prompt".into(), json!(p));
        }
        if self.cfg.send_keywords && !omitted("keywords") && !ctx.keywords.is_empty() {
            transcription.insert("keywords".into(), json!(ctx.keywords));
        }
        json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "audio": {"input": {
                    "format": {"type": "audio/pcm", "rate": SOCKET_RATE},
                    "transcription": Value::Object(transcription),
                    "turn_detection": Value::Null,
                }},
            },
        })
    }

    async fn connect(&self) -> Result<Socket, SegmentError> {
        let mut req = self.cfg.url.as_str().into_client_request().map_err(|e| {
            SegmentError::new(ErrorClass::BadRequest, format!("realtime address: {e}"))
        })?;
        let header = match &self.cfg.auth {
            Auth::Bearer(s) if !s.is_empty() => Some(("authorization", format!("Bearer {s}"))),
            Auth::AzureApiKey(s) => Some((crate::vendor::azure_openai::API_KEY_HEADER, s.clone())),
            Auth::Header {
                name,
                scheme,
                secret,
            } if !secret.is_empty() => Some((
                *name,
                match scheme {
                    Some(w) => format!("{w} {secret}"),
                    None => secret.clone(),
                },
            )),
            _ => None,
        };
        if let Some((name, value)) = header {
            let mut v = http::HeaderValue::from_str(&value).map_err(|_| {
                SegmentError::new(
                    ErrorClass::Auth,
                    "the credential cannot be sent in a header",
                )
            })?;
            v.set_sensitive(true);
            req.headers_mut().insert(name, v);
        }
        let secret = self.cfg.auth.secret().map(str::to_string);
        let connect = tokio_tungstenite::connect_async(req);
        match tokio::time::timeout(self.cfg.connect_timeout, connect).await {
            Err(_) => Err(SegmentError::new(
                ErrorClass::Network,
                "the realtime socket did not open in time",
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
                    redact(
                        &format!("realtime handshake refused: {body}"),
                        secret.as_deref(),
                    ),
                ))
            }
            Ok(Err(e)) => Err(SegmentError::new(
                ErrorClass::Network,
                redact(&format!("realtime socket: {e}"), secret.as_deref()),
            )
            .with_phase(RequestPhase::BeforeSend)),
            Ok(Ok((ws, _))) => Ok(ws),
        }
    }

    async fn run(
        &self,
        open: &mut Open,
        audio: &SegmentAudio,
        ctx: &SegmentContext,
        progress: &RequestProgress,
    ) -> Result<SegmentTranscript, SegmentError> {
        let update = self.session_update(ctx);
        if open.configured.as_ref() != Some(&update) {
            send(&mut open.ws, &update).await?;
            open.configured = Some(update);
        }
        let pcm = upsample_16k_to_24k(&audio.pcm);
        progress.mark_sent();
        for chunk in pcm.chunks(APPEND_SAMPLES) {
            let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            send(
                &mut open.ws,
                &json!({"type": "input_audio_buffer.append", "audio": b64}),
            )
            .await?;
        }
        send(&mut open.ws, &json!({"type": "input_audio_buffer.commit"})).await?;

        let mut item: Option<String> = None;
        let mut deltas = String::new();
        loop {
            let msg = match open.ws.next().await {
                Some(Ok(Message::Text(t))) => t,
                Some(Ok(Message::Close(_))) | None => {
                    return Err(SegmentError::new(
                        ErrorClass::Network,
                        "the realtime socket closed",
                    )
                    .with_phase(phase(progress)));
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => {
                    return Err(SegmentError::new(
                        ErrorClass::Network,
                        format!("realtime socket: {e}"),
                    )
                    .with_phase(phase(progress)));
                }
            };
            let v: Value = match serde_json::from_str(&msg) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
            let item_of = || v.get("item_id").and_then(Value::as_str).map(str::to_string);
            match kind {
                "input_audio_buffer.committed" => {
                    progress.mark_headers();
                    item = item_of();
                }
                "conversation.item.input_audio_transcription.delta"
                    if item.is_some() && item_of() == item =>
                {
                    deltas.push_str(v.get("delta").and_then(Value::as_str).unwrap_or_default());
                }
                "conversation.item.input_audio_transcription.completed"
                    if item.is_some() && item_of() == item =>
                {
                    let text = v
                        .get("transcript")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or(deltas);
                    let billed_ms = v
                        .pointer("/usage/seconds")
                        .and_then(Value::as_f64)
                        .map(|s| (s * 1000.0) as u32);
                    return Ok(SegmentTranscript {
                        text: text.trim().to_string(),
                        vendor_request_id: item,
                        billed_ms,
                        ..Default::default()
                    });
                }
                "conversation.item.input_audio_transcription.failed"
                    if item.is_some() && item_of() == item =>
                {
                    let e = v.get("error").cloned().unwrap_or(Value::Null);
                    return Err(vendor_error(&e).with_phase(RequestPhase::Headers));
                }
                "error" => {
                    let e = v.get("error").cloned().unwrap_or(Value::Null);
                    let code = e.get("code").and_then(Value::as_str).unwrap_or_default();
                    // Nothing to transcribe: the segment had no audio the vendor kept.
                    if code == "input_audio_buffer_commit_empty" {
                        return Ok(SegmentTranscript::default());
                    }
                    return Err(vendor_error(&e).with_phase(phase(progress)));
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

async fn send(ws: &mut Socket, v: &Value) -> Result<(), SegmentError> {
    ws.send(Message::Text(v.to_string().into()))
        .await
        .map_err(|e| SegmentError::new(ErrorClass::Network, format!("realtime socket: {e}")))
}

/// An `error` object from the socket, classified like an HTTP status.
fn vendor_error(e: &Value) -> SegmentError {
    let code = e.get("code").and_then(Value::as_str).unwrap_or_default();
    let message = e
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the realtime transcription failed")
        .to_string();
    let class = match code {
        "invalid_api_key" | "unauthorized" | "insufficient_quota_auth" => ErrorClass::Auth,
        "rate_limit_exceeded" | "session_rate_limited" => ErrorClass::RateLimited,
        "model_not_found" | "invalid_model" => ErrorClass::ModelNotServed,
        c if c.starts_with("invalid") => ErrorClass::BadRequest,
        _ => ErrorClass::Vendor,
    };
    let mut err = SegmentError::new(class, format!("{code}: {message}"));
    if class == ErrorClass::BadRequest
        && let Some(param) = e.get("param").and_then(Value::as_str)
    {
        let field = param.rsplit('.').next().unwrap_or(param);
        if matches!(field, "prompt" | "keywords" | "language" | "languages") {
            err.refused_field = Some(if field == "languages" {
                "language".into()
            } else {
                field.into()
            });
        }
    }
    err
}

fn primary(lang: &str) -> String {
    let l = lang.trim();
    l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase()
}

/// 16 kHz to the socket's 24 kHz (3:2), by linear interpolation between neighbouring samples.
pub fn upsample_16k_to_24k(pcm: &[i16]) -> Vec<i16> {
    if pcm.is_empty() {
        return Vec::new();
    }
    let out_len = pcm.len() * 3 / 2;
    (0..out_len)
        .map(|i| {
            let pos = i as f32 * 2.0 / 3.0;
            let a = pos.floor() as usize;
            let frac = pos - a as f32;
            let x0 = pcm[a.min(pcm.len() - 1)] as f32;
            let x1 = pcm[(a + 1).min(pcm.len() - 1)] as f32;
            (x0 + (x1 - x0) * frac).round() as i16
        })
        .collect()
}

#[async_trait::async_trait]
impl SegmentTranscriber for OpenAiRealtimeTranscriber {
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
        let mut guard = match tokio::time::timeout_at(deadline, self.socket.lock()).await {
            Ok(g) => g,
            Err(_) => {
                return Err(
                    SegmentError::new(ErrorClass::Timeout, "the realtime socket was busy")
                        .with_phase(RequestPhase::BeforeSend),
                );
            }
        };
        if guard.is_none() {
            let ws = match tokio::time::timeout_at(deadline, self.connect()).await {
                Ok(r) => r?,
                Err(_) => {
                    return Err(SegmentError::new(
                        ErrorClass::Timeout,
                        "the realtime socket did not open in time",
                    )
                    .with_phase(RequestPhase::BeforeSend));
                }
            };
            *guard = Some(Open {
                ws,
                configured: None,
            });
        }
        let open = guard.as_mut().expect("opened above");
        let result =
            match tokio::time::timeout_at(deadline, self.run(open, audio, ctx, progress)).await {
                Ok(r) => r,
                Err(_) => Err(SegmentError::new(
                    ErrorClass::Timeout,
                    "no transcript before the deadline",
                )
                .with_phase(phase(progress))),
            };
        if result.is_err() {
            // A fresh socket for the next call: an abandoned item must never answer for it.
            if let Some(mut o) = guard.take() {
                let _ = o.ws.close(None).await;
            }
        }
        result
    }

    async fn prewarm(&self, _connections: usize) {
        let mut guard = self.socket.lock().await;
        if guard.is_none()
            && let Ok(ws) = self.connect().await
        {
            *guard = Some(Open {
                ws,
                configured: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::ws::{Message as AxMessage, WebSocket, WebSocketUpgrade};
    use axum::http::HeaderMap;
    use parking_lot::Mutex;

    /// What the mock socket saw, per connection.
    #[derive(Default)]
    struct Seen {
        connections: AtomicUsize,
        auth: Mutex<Vec<String>>,
        updates: Mutex<Vec<Value>>,
        appended_samples: Mutex<Vec<usize>>,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Script {
        Answer,
        RateLimited,
        Silent,
        Empty,
    }

    async fn serve(seen: Arc<Seen>, script: Arc<Mutex<Vec<Script>>>, ws: WebSocket) {
        let (mut tx, mut rx) = ws.split();
        let _ = tx
            .send(AxMessage::Text(
                json!({"type": "session.created", "session": {"type": "transcription"}})
                    .to_string()
                    .into(),
            ))
            .await;
        let mut samples = 0usize;
        let mut n = 0usize;
        while let Some(Ok(AxMessage::Text(t))) = rx.next().await {
            let v: Value = serde_json::from_str(&t).unwrap();
            match v["type"].as_str().unwrap() {
                "session.update" => seen.updates.lock().push(v),
                "input_audio_buffer.append" => {
                    let b = base64::engine::general_purpose::STANDARD
                        .decode(v["audio"].as_str().unwrap())
                        .unwrap();
                    samples += b.len() / 2;
                }
                "input_audio_buffer.commit" => {
                    seen.appended_samples.lock().push(samples);
                    samples = 0;
                    n += 1;
                    let step = script.lock().first().copied().unwrap_or(Script::Answer);
                    if !script.lock().is_empty() {
                        script.lock().remove(0);
                    }
                    let item = format!("item_{n}");
                    match step {
                        Script::Answer => {
                            for m in [
                                json!({"type": "input_audio_buffer.committed", "item_id": item}),
                                json!({"type": "conversation.item.input_audio_transcription.delta", "item_id": item, "delta": "hello "}),
                                json!({"type": "conversation.item.input_audio_transcription.completed", "item_id": item,
                                       "transcript": format!("hello {n}"), "usage": {"type": "duration", "seconds": 1.5}}),
                            ] {
                                let _ = tx.send(AxMessage::Text(m.to_string().into())).await;
                            }
                        }
                        Script::RateLimited => {
                            let m = json!({"type": "error", "error": {"type": "invalid_request_error", "code": "rate_limit_exceeded", "message": "slow down"}});
                            let _ = tx.send(AxMessage::Text(m.to_string().into())).await;
                        }
                        Script::Empty => {
                            let m = json!({"type": "error", "error": {"code": "input_audio_buffer_commit_empty", "message": "empty"}});
                            let _ = tx.send(AxMessage::Text(m.to_string().into())).await;
                        }
                        Script::Silent => {}
                    }
                }
                _ => {}
            }
        }
    }

    async fn mock(script: Vec<Script>, refuse_auth: bool) -> (String, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        let script = Arc::new(Mutex::new(script));
        let s = Arc::clone(&seen);
        let app = axum::Router::new().route(
            "/v1/realtime",
            axum::routing::get(move |ws: WebSocketUpgrade, headers: HeaderMap| {
                let s = Arc::clone(&s);
                let script = Arc::clone(&script);
                async move {
                    s.connections.fetch_add(1, Ordering::SeqCst);
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    s.auth.lock().push(auth);
                    if refuse_auth {
                        return axum::http::StatusCode::UNAUTHORIZED.into_response();
                    }
                    ws.on_upgrade(move |socket| serve(s, script, socket))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            format!("ws://{addr}/v1/realtime?intent=transcription"),
            seen,
        )
    }

    use axum::response::IntoResponse;

    fn transcriber(url: &str, list: bool) -> OpenAiRealtimeTranscriber {
        let mut cfg = OpenAiRealtimeConfig::new(
            url,
            Auth::Bearer(super::super::testkit::SECRET.into()),
            "gpt-live-transcribe",
        );
        cfg.language = if list {
            LanguageDialect::list("languages")
        } else {
            LanguageDialect::single("language")
        };
        cfg.send_keywords = true;
        OpenAiRealtimeTranscriber::new(cfg).unwrap()
    }

    fn ctx(lang: Option<&str>) -> SegmentContext {
        SegmentContext {
            language: lang.map(str::to_string),
            keywords: vec!["Bud".into()],
            ..Default::default()
        }
    }

    fn audio(ms: usize) -> SegmentAudio {
        SegmentAudio::new((0..16 * ms).map(|i| ((i % 40) as i16 - 20) * 300).collect())
    }

    #[tokio::test]
    async fn utterances_are_committed_on_one_socket_configured_once() {
        let (url, seen) = mock(vec![], false).await;
        let t = transcriber(&url, true);
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
        assert_eq!(a.text, "hello 1");
        assert_eq!(a.billed_ms, Some(1500));
        assert!(p.headers_received(), "committed is the vendor's acceptance");
        let b = t
            .transcribe(
                &audio(500),
                &ctx(Some("en-US")),
                Duration::from_secs(5),
                &RequestProgress::new(),
            )
            .await
            .unwrap();
        assert_eq!(b.text, "hello 2");
        assert_eq!(
            seen.connections.load(Ordering::SeqCst),
            1,
            "one socket per session"
        );
        let updates = seen.updates.lock().clone();
        assert_eq!(updates.len(), 1, "configured once");
        let tr = &updates[0]["session"]["audio"]["input"]["transcription"];
        assert_eq!(tr["model"], "gpt-live-transcribe");
        assert_eq!(tr["languages"], json!(["en"]));
        assert_eq!(tr["keywords"], json!(["Bud"]));
        assert_eq!(
            updates[0]["session"]["audio"]["input"]["turn_detection"],
            Value::Null
        );
        assert_eq!(
            updates[0]["session"]["audio"]["input"]["format"]["rate"],
            24_000
        );
        assert_eq!(
            *seen.appended_samples.lock(),
            vec![24_000, 12_000],
            "16 kHz audio at 24 kHz"
        );
        assert!(seen.auth.lock()[0].starts_with("Bearer "));
    }

    #[tokio::test]
    async fn a_pinned_language_reconfigures_the_socket() {
        let (url, seen) = mock(vec![], false).await;
        let t = transcriber(&url, false);
        t.transcribe(
            &audio(300),
            &ctx(None),
            Duration::from_secs(5),
            &RequestProgress::new(),
        )
        .await
        .unwrap();
        t.transcribe(
            &audio(300),
            &ctx(Some("de")),
            Duration::from_secs(5),
            &RequestProgress::new(),
        )
        .await
        .unwrap();
        let updates = seen.updates.lock().clone();
        assert_eq!(updates.len(), 2);
        assert!(
            updates[0]["session"]["audio"]["input"]["transcription"]
                .get("language")
                .is_none()
        );
        assert_eq!(
            updates[1]["session"]["audio"]["input"]["transcription"]["language"],
            "de"
        );
    }

    #[tokio::test]
    async fn an_error_closes_the_socket_and_the_next_call_opens_a_fresh_one() {
        let (url, seen) = mock(vec![Script::RateLimited, Script::Answer], false).await;
        let t = transcriber(&url, true);
        let e = t
            .transcribe(
                &audio(300),
                &ctx(Some("en")),
                Duration::from_secs(5),
                &RequestProgress::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(e.class, ErrorClass::RateLimited);
        let ok = t
            .transcribe(
                &audio(300),
                &ctx(Some("en")),
                Duration::from_secs(5),
                &RequestProgress::new(),
            )
            .await
            .unwrap();
        assert_eq!(ok.text, "hello 1", "a new socket counts its own items");
        assert_eq!(seen.connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn no_answer_by_the_deadline_is_a_timeout_and_a_fresh_socket() {
        let (url, seen) = mock(vec![Script::Silent, Script::Answer], false).await;
        let t = transcriber(&url, true);
        let e = t
            .transcribe(
                &audio(300),
                &ctx(None),
                Duration::from_millis(300),
                &RequestProgress::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(e.class, ErrorClass::Timeout);
        t.transcribe(
            &audio(300),
            &ctx(None),
            Duration::from_secs(5),
            &RequestProgress::new(),
        )
        .await
        .unwrap();
        assert_eq!(seen.connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_empty_commit_is_an_empty_transcript() {
        let (url, _) = mock(vec![Script::Empty], false).await;
        let t = transcriber(&url, true);
        let r = t
            .transcribe(
                &audio(100),
                &ctx(None),
                Duration::from_secs(5),
                &RequestProgress::new(),
            )
            .await
            .unwrap();
        assert!(r.text.is_empty());
    }

    #[tokio::test]
    async fn a_refused_credential_is_auth_and_never_echoes_the_key() {
        let (url, _) = mock(vec![], true).await;
        let t = transcriber(&url, true);
        let e = t
            .transcribe(
                &audio(100),
                &ctx(None),
                Duration::from_secs(5),
                &RequestProgress::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(e.class, ErrorClass::Auth);
        assert!(!e.message.contains(super::super::testkit::SECRET));
    }

    #[test]
    fn a_non_socket_address_is_refused() {
        let cfg = OpenAiRealtimeConfig::new("https://api.openai.com/v1/realtime", Auth::None, "m");
        assert!(OpenAiRealtimeTranscriber::new(cfg).is_err());
    }

    #[test]
    fn upsampling_keeps_the_waveform_and_the_length_ratio() {
        let pcm: Vec<i16> = (0..160).map(|i| (i * 100) as i16).collect();
        let out = upsample_16k_to_24k(&pcm);
        assert_eq!(out.len(), 240);
        assert_eq!(out[0], 0);
        assert_eq!(
            out[3], pcm[2],
            "every third output sample is an input sample"
        );
        assert!(out.windows(2).all(|w| w[1] >= w[0]), "a ramp stays a ramp");
    }
}

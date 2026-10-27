//! A mock vendor on a loopback socket, and the error contract every family must meet.
//!
//! The mock records each request as the vendor would see it (method, path, query, headers, and
//! the multipart parts or raw body) and answers from a script. The contract functions run one
//! family through the shared rules: status classes, `Retry-After`, a named refused field, the
//! request's own time limit, a dead host, and the request progress marks.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequest, Multipart, Request};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use parking_lot::Mutex;

use crate::transcriber::http::{HttpSettings, UploadClients};
use crate::transcriber::{
    RequestPhase, RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript,
};
use crate::types::ErrorClass;

/// The credential every harness uses; the contract checks no error message carries it.
pub const SECRET: &str = "sk-test-SECRET-0123456789";

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }

    pub fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())],
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Part {
    pub name: String,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: http::HeaderMap,
    pub parts: Vec<Part>,
    pub body: Bytes,
    pub peer: SocketAddr,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    pub fn part(&self, name: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.name == name)
    }

    /// Every value of a text part, in order.
    pub fn texts(&self, name: &str) -> Vec<String> {
        self.parts
            .iter()
            .filter(|p| p.name == name)
            .map(|p| String::from_utf8_lossy(&p.data).into_owned())
            .collect()
    }

    pub fn text(&self, name: &str) -> Option<String> {
        self.texts(name).into_iter().next()
    }

    pub fn part_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for p in &self.parts {
            if !names.contains(&p.name) {
                names.push(p.name.clone());
            }
        }
        names
    }

    pub fn query_values(&self, name: &str) -> Vec<String> {
        self.query
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .collect()
    }

    pub fn query_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for (k, _) in &self.query {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
        names
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("the request body is JSON")
    }
}

#[derive(Default)]
struct State {
    requests: Mutex<Vec<Recorded>>,
    queue: Mutex<VecDeque<Reply>>,
    default: Mutex<Option<Reply>>,
}

pub struct MockVendor {
    pub base: String,
    state: Arc<State>,
}

impl MockVendor {
    pub async fn start(default: Reply) -> Self {
        let state = Arc::new(State::default());
        *state.default.lock() = Some(default);
        let shared = state.clone();
        let app = axum::Router::new().fallback(
            move |ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request| {
                let state = shared.clone();
                async move { handle(state, peer, req).await }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            base: format!("http://{addr}"),
            state,
        }
    }

    /// Answers the next request with `reply`, before falling back to the default.
    pub fn push(&self, reply: Reply) {
        self.state.queue.lock().push_back(reply);
    }

    pub fn set_default(&self, reply: Reply) {
        *self.state.default.lock() = Some(reply);
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.state.requests.lock().clone()
    }

    pub fn last(&self) -> Recorded {
        self.requests()
            .pop()
            .expect("the vendor received a request")
    }
}

async fn handle(state: Arc<State>, peer: SocketAddr, req: Request) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req
        .uri()
        .query()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();
    let headers = req.headers().clone();
    let is_multipart = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("multipart/form-data"));
    let mut parts = Vec::new();
    let mut body = Bytes::new();
    if is_multipart {
        let mut mp = Multipart::from_request(req, &()).await.unwrap();
        while let Some(field) = mp.next_field().await.unwrap() {
            let name = field.name().unwrap_or_default().to_string();
            let file_name = field.file_name().map(str::to_string);
            let content_type = field.content_type().map(str::to_string);
            let data = field.bytes().await.unwrap();
            parts.push(Part {
                name,
                file_name,
                content_type,
                data,
            });
        }
    } else {
        body = axum::body::to_bytes(req.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap();
    }
    state.requests.lock().push(Recorded {
        method,
        path,
        query,
        headers,
        parts,
        body,
        peer,
    });
    let reply = state
        .queue
        .lock()
        .pop_front()
        .or_else(|| state.default.lock().clone())
        .unwrap();
    if !reply.delay.is_zero() {
        tokio::time::sleep(reply.delay).await;
    }
    let mut resp = (
        http::StatusCode::from_u16(reply.status).unwrap(),
        reply.body,
    )
        .into_response();
    for (k, v) in reply.headers {
        resp.headers_mut().insert(
            http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    resp
}

/// A base URL where nothing listens.
pub fn dead_base() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

pub fn client() -> reqwest::Client {
    UploadClients::new(&HttpSettings::default())
        .unwrap()
        .client(false)
        .clone()
}

/// Half a second of a quiet ramp, so the WAV is not all zeros.
pub fn audio() -> SegmentAudio {
    SegmentAudio::new((0..8000).map(|i| ((i % 200) as i16 - 100) * 50).collect())
}

pub async fn run(
    t: &dyn SegmentTranscriber,
    ctx: &SegmentContext,
) -> (
    Result<SegmentTranscript, SegmentError>,
    Arc<RequestProgress>,
) {
    let progress = RequestProgress::new();
    let out = t
        .transcribe(&audio(), ctx, Duration::from_secs(5), &progress)
        .await;
    (out, progress)
}

/// Builds the family's transcriber against a base URL (a mock's, or a dead one).
pub type Build = fn(&str) -> Arc<dyn SegmentTranscriber>;

/// One family's canned answers for the contract.
pub struct Harness {
    pub build: Build,
    pub ctx: SegmentContext,
    pub success: Reply,
    /// A 401 whose body echoes the key, as some vendors do.
    pub unauthorized: Reply,
    pub model_missing: Reply,
    /// A 429 with `Retry-After: 2`.
    pub rate_limited: Reply,
    pub unavailable: Reply,
    /// A refusal naming an optional field the harness context makes the family send.
    pub refused: Reply,
    pub refused_field: &'static str,
}

async fn failure(h: &Harness, ctx: &SegmentContext, reply: Reply) -> SegmentError {
    let vendor = MockVendor::start(reply).await;
    let t = (h.build)(&vendor.base);
    let (out, _) = run(t.as_ref(), ctx).await;
    let err = out.expect_err("the vendor refused");
    assert!(
        !err.message.contains(SECRET),
        "the key leaked: {}",
        err.message
    );
    err
}

pub async fn unauthorized_is_auth(h: Harness) {
    let err = failure(&h, &h.ctx, h.unauthorized.clone()).await;
    assert_eq!(err.class, ErrorClass::Auth, "{err}");
    assert_eq!(err.phase, RequestPhase::Headers);
    assert!(!err.is_fast_retryable());
    assert!(!err.counts_for_breaker());
}

pub async fn missing_model_is_not_served(h: Harness) {
    // No language and no vocabulary, so a refusal can only be about the model.
    let err = failure(&h, &SegmentContext::default(), h.model_missing.clone()).await;
    assert_eq!(err.class, ErrorClass::ModelNotServed, "{err}");
    assert!(!err.is_fast_retryable());
}

pub async fn rate_limit_carries_retry_after(h: Harness) {
    let err = failure(&h, &h.ctx, h.rate_limited.clone()).await;
    assert_eq!(err.class, ErrorClass::RateLimited, "{err}");
    assert_eq!(err.retry_after, Some(Duration::from_secs(2)));
    assert!(!err.counts_for_breaker());
}

pub async fn unavailable_is_vendor_and_fast_retryable(h: Harness) {
    let err = failure(&h, &h.ctx, h.unavailable.clone()).await;
    assert_eq!(err.class, ErrorClass::Vendor, "{err}");
    assert_eq!(err.status, Some(503));
    assert!(err.is_fast_retryable());
    assert!(err.counts_for_breaker());
}

pub async fn refusal_names_the_field(h: Harness) {
    let err = failure(&h, &h.ctx, h.refused.clone()).await;
    assert_eq!(err.class, ErrorClass::BadRequest, "{err}");
    assert_eq!(err.refused_field.as_deref(), Some(h.refused_field));
    let t = (h.build)("http://127.0.0.1:9");
    assert!(
        t.info()
            .droppable_fields
            .iter()
            .any(|f| f == h.refused_field),
        "a field the vendor may refuse is declared droppable"
    );
}

pub async fn the_request_limit_is_applied(h: Harness) {
    let vendor = MockVendor::start(h.success.clone().delayed(Duration::from_secs(3))).await;
    let t = (h.build)(&vendor.base);
    let progress = RequestProgress::new();
    let started = Instant::now();
    let err = t
        .transcribe(&audio(), &h.ctx, Duration::from_millis(300), &progress)
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(err.class, ErrorClass::Timeout, "{err}");
    assert_eq!(err.phase, RequestPhase::Sent);
    assert!(!progress.headers_received());
}

pub async fn a_dead_host_is_network_before_send(h: Harness) {
    let t = (h.build)(&dead_base());
    let (out, progress) = run(t.as_ref(), &h.ctx).await;
    let err = out.unwrap_err();
    assert_eq!(err.class, ErrorClass::Network, "{err}");
    assert_eq!(err.phase, RequestPhase::BeforeSend);
    assert!(err.is_fast_retryable());
    assert!(!progress.headers_received());
}

pub async fn progress_marks_sent_and_headers(h: Harness) {
    let vendor = MockVendor::start(h.success.clone().delayed(Duration::from_millis(30))).await;
    let t = (h.build)(&vendor.base);
    let (out, progress) = run(t.as_ref(), &h.ctx).await;
    out.unwrap();
    assert!(progress.headers_received());
    let ttfh = progress
        .time_to_headers()
        .expect("sent and headers both marked");
    assert!(ttfh >= Duration::from_millis(25), "{ttfh:?}");
}

/// Two uploads in flight on one transcriber overlap instead of queueing.
pub async fn two_calls_overlap(h: Harness) {
    let vendor = MockVendor::start(h.success.clone().delayed(Duration::from_millis(300))).await;
    let t = (h.build)(&vendor.base);
    let started = Instant::now();
    let (a, b) = tokio::join!(run(t.as_ref(), &h.ctx), run(t.as_ref(), &h.ctx));
    a.0.unwrap();
    b.0.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(550),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(vendor.requests().len(), 2);
}

pub fn info_is_a_file_target(t: &dyn SegmentTranscriber, adapter: &str, base: &str) {
    let info = t.info();
    assert_eq!(info.adapter, adapter);
    assert_eq!(info.kind, crate::transcriber::TranscriberKind::File);
    assert_eq!(info.host_key, crate::transcriber::http::host_key(base));
}

/// Stamps the contract tests into a family's test module. `$harness` is a `fn() -> Harness`.
macro_rules! contract_tests {
    ($harness:path) => {
        mod contract {
            use crate::transcriber::wire::testkit as kit;

            #[tokio::test]
            async fn unauthorized_is_auth() {
                kit::unauthorized_is_auth($harness()).await
            }

            #[tokio::test]
            async fn missing_model_is_not_served() {
                kit::missing_model_is_not_served($harness()).await
            }

            #[tokio::test]
            async fn rate_limit_carries_retry_after() {
                kit::rate_limit_carries_retry_after($harness()).await
            }

            #[tokio::test]
            async fn unavailable_is_vendor_and_fast_retryable() {
                kit::unavailable_is_vendor_and_fast_retryable($harness()).await
            }

            #[tokio::test]
            async fn refusal_names_the_field() {
                kit::refusal_names_the_field($harness()).await
            }

            #[tokio::test]
            async fn the_request_limit_is_applied() {
                kit::the_request_limit_is_applied($harness()).await
            }

            #[tokio::test]
            async fn a_dead_host_is_network_before_send() {
                kit::a_dead_host_is_network_before_send($harness()).await
            }

            #[tokio::test]
            async fn progress_marks_sent_and_headers() {
                kit::progress_marks_sent_and_headers($harness()).await
            }

            #[tokio::test]
            async fn two_calls_overlap() {
                kit::two_calls_overlap($harness()).await
            }
        }
    };
}
pub(crate) use contract_tests;

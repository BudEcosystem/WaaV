//! W-C1 observability: request-id correlation propagates (extreme-TDD RED→GREEN, E13).
//!
//! RED (before this PR): there was no request-id middleware, no `RequestId` extension, and no
//! `x-request-id` echo — so `middleware::request_id_middleware` / `middleware::RequestId` did not
//! exist and this test could not compile.
//!
//! GREEN: the middleware reads/mints `x-request-id` (or derives from `traceparent`), stashes a
//! `RequestId` extension for handlers (so they can forward it to outbound provider requests),
//! echoes it on the response, and runs the handler inside a tracing span carrying `request_id`.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{
    Extension, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
};
use futures::FutureExt;
use tokio::task::JoinHandle;
use tower::util::ServiceExt;

use waav_gateway::middleware::{RequestId, request_id_middleware};

struct TestServer {
    handle: JoinHandle<()>,
    panicked: Arc<AtomicBool>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if !self.handle.is_finished() {
            self.handle.abort();
        }
        if self.panicked.load(Ordering::SeqCst) {
            if std::thread::panicking() {
                eprintln!("request-id mock provider server panicked");
            } else {
                panic!("request-id mock provider server panicked");
            }
        }
    }
}

fn spawn_test_server<F>(future: F) -> TestServer
where
    F: Future<Output = ()> + Send + 'static,
{
    let panicked = Arc::new(AtomicBool::new(false));
    let panicked_in_task = Arc::clone(&panicked);
    let handle = tokio::spawn(async move {
        if AssertUnwindSafe(future).catch_unwind().await.is_err() {
            panicked_in_task.store(true, Ordering::SeqCst);
        }
    });
    TestServer { handle, panicked }
}

/// A handler standing in for a real provider-calling handler: it reads the propagated
/// `RequestId` and forwards it on an *outbound* request to a local mock, mirroring how WaaV
/// attaches `x-request-id` to provider requests.
async fn forwarding_handler(
    State(mock_url): State<Arc<String>>,
    Extension(request_id): Extension<RequestId>,
) -> impl IntoResponse {
    // Forward the correlation id on the outbound provider request header.
    let client = reqwest::Client::new();
    let _ = client
        .get(mock_url.as_str())
        .header("x-request-id", request_id.as_str())
        .send()
        .await;
    StatusCode::OK
}

#[tokio::test]
async fn request_id_propagates() {
    use axum::routing::get as axget;
    use tokio::net::TcpListener;

    // Mock "provider" that records the inbound x-request-id it receives.
    static SEEN: AtomicBool = AtomicBool::new(false);
    let seen_value: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let seen_for_handler = seen_value.clone();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mock = Router::new()
        .route(
            "/provider",
            axget(move |headers: axum::http::HeaderMap| {
                let seen = seen_for_handler.clone();
                async move {
                    if let Some(id) = headers.get("x-request-id").and_then(|v| v.to_str().ok()) {
                        *seen.lock().unwrap() = Some(id.to_string());
                        SEEN.store(true, Ordering::SeqCst);
                    }
                    StatusCode::OK
                }
            }),
        )
        .with_state(());
    let _server = spawn_test_server(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    let mock_url = Arc::new(format!("http://{addr}/provider"));
    let app = Router::new()
        .route("/call", get(forwarding_handler))
        .with_state(mock_url)
        .layer(axum::middleware::from_fn(request_id_middleware));

    // --- Case 1: inbound x-request-id is honored end-to-end -------------------------------
    let req = Request::builder()
        .uri("/call")
        .header("x-request-id", "corr-abc-123")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The response echoes the same correlation id.
    let echoed = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        echoed.as_deref(),
        Some("corr-abc-123"),
        "the response must echo the inbound x-request-id"
    );

    // The same id was forwarded on the outbound provider request.
    // (Give the spawned outbound request a moment to land.)
    for _ in 0..50 {
        if SEEN.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        seen_value.lock().unwrap().clone(),
        Some("corr-abc-123".to_string()),
        "the correlation id must be forwarded to the outbound provider request"
    );

    // --- Case 2: absent inbound id is minted and echoed ----------------------------------
    let req = Request::builder().uri("/call").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let minted = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .expect("a request-id must be minted when none is supplied");
    assert_eq!(
        minted.len(),
        36,
        "a minted id is a UUIDv4 string (36 chars): {minted}"
    );
}

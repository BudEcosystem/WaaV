//! An in-process stand-in for budgateway's `/v1/responses` and `/v1/responses/{id}/cancel`: it
//! answers each request with the next scripted reply and records what it was sent. Shared by
//! the voice-agent tests (`#[path]`-included).

#![allow(dead_code)]

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use bytes::Bytes;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[derive(Clone)]
pub enum Reply {
    /// SSE frames, with an optional pause (ms) before each.
    Sse(Vec<(u64, Value)>),
    Status(u16, Value),
    /// A turn that holds a call for the caller's approval (spec 029 §7.3): the frames up to
    /// the hold, then the stream stays open until a vote arrives, and goes on with `approved`
    /// or `declined`.
    Held {
        before: Vec<(u64, Value)>,
        approved: Vec<(u64, Value)>,
        declined: Vec<(u64, Value)>,
    },
}

#[derive(Clone, Default)]
pub struct Gateway {
    pub replies: Arc<Mutex<VecDeque<Reply>>>,
    pub bodies: Arc<Mutex<Vec<Value>>>,
    pub auth: Arc<Mutex<Vec<String>>>,
    pub cancels: Arc<Mutex<Vec<(String, String)>>>,
    /// Each cancel's `traceparent` header, in order.
    pub cancel_traceparents: Arc<Mutex<Vec<Option<String>>>>,
    /// Each vote (`mcp_approval_response`) with its `authorization` header, in order.
    pub votes: Arc<Mutex<Vec<(Value, String)>>>,
    /// The held turn waiting for its vote.
    held: Arc<Mutex<Option<tokio::sync::oneshot::Sender<bool>>>>,
}

fn sse_chunk(frame: &Value) -> Bytes {
    let kind = frame["type"].as_str().unwrap_or("x").to_string();
    Bytes::from(format!("event: {kind}\ndata: {frame}\n\n"))
}

type Chunks = futures::channel::mpsc::UnboundedSender<Result<Bytes, Infallible>>;

async fn send_frames(tx: &Chunks, frames: Vec<(u64, Value)>) {
    for (pause, frame) in frames {
        if pause > 0 {
            tokio::time::sleep(Duration::from_millis(pause)).await;
        }
        let _ = tx.unbounded_send(Ok(sse_chunk(&frame)));
    }
}

async fn responses(
    State(g): State<Gateway>,
    headers: HeaderMap,
    body: axum::Json<Value>,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    // A vote is a request of its own: it releases the held turn and takes no reply.
    if body["input"][0]["type"] == "mcp_approval_response" {
        let approve = body["input"][0]["approve"] == true;
        g.votes.lock().push((body.0, auth));
        if let Some(held) = g.held.lock().take() {
            let _ = held.send(approve);
        }
        return axum::Json(json!({"id": "resp_vote", "status": "completed"})).into_response();
    }
    g.bodies.lock().push(body.0);
    g.auth.lock().push(auth);
    match g.replies.lock().pop_front() {
        Some(Reply::Held {
            before,
            approved,
            declined,
        }) => {
            let (tx, rx) = futures::channel::mpsc::unbounded();
            let (vote, voted) = tokio::sync::oneshot::channel();
            *g.held.lock() = Some(vote);
            tokio::spawn(async move {
                send_frames(&tx, before).await;
                let _ = tx.unbounded_send(Ok(Bytes::from_static(b": keep-alive\n\n")));
                let Ok(approve) = voted.await else { return };
                send_frames(&tx, if approve { approved } else { declined }).await;
            });
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(rx))
                .unwrap()
        }
        Some(Reply::Status(code, body)) => {
            (StatusCode::from_u16(code).unwrap(), axum::Json(body)).into_response()
        }
        Some(Reply::Sse(frames)) => {
            let stream = futures::stream::unfold(frames.into_iter(), |mut it| async move {
                let (pause, frame) = it.next()?;
                if pause > 0 {
                    tokio::time::sleep(Duration::from_millis(pause)).await;
                }
                let kind = frame["type"].as_str().unwrap_or("x").to_string();
                let chunk = format!("event: {kind}\ndata: {frame}\n\n");
                Some((Ok::<_, Infallible>(Bytes::from(chunk)), it))
            });
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(stream))
                .unwrap()
        }
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn cancel(
    State(g): State<Gateway>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> StatusCode {
    let model = headers
        .get("x-model-name")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    g.cancels.lock().push((id, model));
    g.cancel_traceparents.lock().push(
        headers
            .get("traceparent")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
    );
    StatusCode::OK
}

pub async fn gateway() -> (String, Gateway) {
    let g = Gateway::default();
    let app = Router::new()
        .route("/v1/responses", post(responses))
        .route("/v1/responses/{id}/cancel", post(cancel))
        .with_state(g.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/v1"), g)
}

pub fn created(id: &str) -> Value {
    json!({"type": "response.created", "response": {"id": id, "status": "in_progress", "conversation": {"id": "conv_voice_s"}}})
}
pub fn delta(t: &str) -> Value {
    json!({"type": "response.output_text.delta", "item_id": "msg_1", "delta": t})
}
pub fn completed() -> Value {
    json!({"type": "response.completed", "response": {"status": "completed", "output": [], "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}}})
}

/// The hold of `name` (an `mcp_approval_request`), `expires_in_s` from now on the wall clock.
pub fn hold(id: &str, name: &str, arguments: Value, expires_in_s: u64) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    json!({"type": "response.output_item.added", "output_index": 0, "sequence_number": 1,
           "item": {"type": "mcp_approval_request", "id": id, "name": name,
                    "arguments": arguments.to_string(), "server_label": "orders",
                    "bud_expires_at": now + expires_in_s}})
}

/// A tool item of the run starting (`done: false`) or finishing.
pub fn tool(item_id: &str, name: &str, done: bool) -> Value {
    let kind = if done {
        "response.output_item.done"
    } else {
        "response.output_item.added"
    };
    json!({"type": kind, "item": {"id": item_id, "type": "mcp_call", "name": name, "status": if done { "completed" } else { "in_progress" }}})
}

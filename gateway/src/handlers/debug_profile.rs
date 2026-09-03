//! Live latency-profile debug surface.
//!
//! Two operator endpoints over the process-wide [`LatencyProfiler`] hub:
//!
//! - `GET /debug/profile` — JSON snapshot: rolling p50/p90/p99 per stage, the
//!   headline end-of-speech→first-audio window, bottleneck histogram, recent
//!   slow turns, and the `realtime_blockers` block.
//! - `GET /debug/profile/stream` — SSE: one `turn` event per completed turn
//!   (sampled per `WAAV_DEBUG_PROFILE_SAMPLE_N`). A lagging client receives a
//!   `lagged` event instead of killing the stream.
//!
//! Both are mounted ONLY when `WAAV_DEBUG_PROFILE=1` and sit behind the same
//! `auth_middleware` as the protected API (double lock — never public). The
//! SSE holds a [`SubscriberGuard`] in its stream state so the zero-cost
//! broadcast gate re-arms when the client disconnects.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

/// `GET /debug/profile` — rolling aggregate snapshot (JSON).
pub async fn profile_snapshot(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.core_state.profiler.snapshot())
}

/// `GET /debug/profile/stream` — live per-turn SSE feed.
pub async fn profile_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let hub = state.core_state.profiler.clone();
    // Order matters: subscribe first so no turn between subscribe and guard is
    // dropped, then arm the gate (the hub only broadcasts while subscribers>0).
    let rx = hub.subscribe();
    let guard = hub.subscriber_guard();

    let stream = futures::stream::unfold((rx, guard), |(mut rx, guard)| async move {
        match rx.recv().await {
            Ok(trace) => {
                let event = Event::default()
                    .event("turn")
                    .json_data(trace.summary())
                    .unwrap_or_else(|_| Event::default().event("turn").data("{}"));
                Some((Ok(event), (rx, guard)))
            }
            // Slow client fell behind the ring: tell it, keep streaming.
            Err(RecvError::Lagged(missed)) => Some((
                Ok(Event::default().event("lagged").data(missed.to_string())),
                (rx, guard),
            )),
            Err(RecvError::Closed) => None,
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

# Realtime S2S resilience validation (credential-free)

The realtime/S2S scaffold's **reconnect supervisor, conversation replay, and
quick-failure cutoff** (`src/core/realtime/scaffold/session.rs`) are the riskiest
code in the subsystem. These two harnesses validate them **end-to-end through the
real `waav-gateway` binary**, with **no vendor keys** — by pointing
`<PROVIDER>_REALTIME_URL` at a local mock upstream that misbehaves on purpose.

Both self-manage a gateway instance + a mock on spare ports, use dummy API keys,
and write gateway logs to the system temp dir (never into the repo).

## Prerequisites

```bash
cd gateway
export CUDA_HOME=/tmp/nocuda ORT_DYLIB_PATH=/home/bud/.local/ortlib/libonnxruntime.so
cargo build --bin waav-gateway --features dag-routing,turn-detect,noise-filter,openapi
python3 -m pip install websockets   # only dependency
```

## 1. Reconnect + conversation-replay — `scripts/realtime_resilience_reconnect_replay.py`

A mock runs one normal turn (so the gateway logs a conversation turn), holds the
connection stable past `MIN_STABLE_CONNECTION`, then drops the WS. Asserts the
scaffold:

- **reconnects** (mock sees a 2nd connection within the backoff window),
- **re-sends the session config** on reconnect,
- **replays the `conversation_log` verbatim** (openai re-emits each logged turn as
  `conversation.item.create`; deepgram re-sends `Settings` — its replay surface),
- is **usable after reconnect** (a fresh turn round-trips),
- and that the **quick-failure cutoff bounds the storm** (a mock that drops every
  dial → the scaffold stops at exactly `MAX_CONSECUTIVE_QUICK_FAILURES`, no loop).

```bash
python3 scripts/realtime_resilience_reconnect_replay.py     # validates openai + deepgram replay surfaces
```

## 2. Dead-session client-surface guard — `scripts/realtime_resilience_quickfail_surface.py`

Regression guard for the handler `on_reconnection` fix (commit `185f0cb`). A mock
drops every dial, forcing the quick-failure cutoff; a client then keeps streaming
audio. Asserts the client **is surfaced a terminal `connection_lost` error and/or a
WS close** — never left streaming into a silently-dead session (the original bug
held it open indefinitely, since the handler reset its idle timer on every inbound
frame). **Exits 0 on PASS, non-zero if the indefinite-hold bug regresses** — usable
in CI/ops.

```bash
python3 scripts/realtime_resilience_quickfail_surface.py && echo PASS || echo REGRESSED
```

## Hermetic companions (always run, no gateway process)

The same invariants are guarded at the unit layer in
`src/core/realtime/scaffold/mock.rs` (run with the standard `cargo test --lib`):

- `quick_failure_cutoff_stops_the_storm` — the local per-session storm bound.
- `handshake_then_immediate_close_trips_shared_breaker_fatal` — the
  handshake-then-drop signature feeds the **shared per-provider circuit breaker**'s
  FATAL fast-trip (cross-session protection), via
  `CircuitBreaker::record_connection_closed`.
- `barge_in_truncates_at_received_duration_and_clears_playback`,
  `server_interruption_emits_cancel_then_truncate` — barge-in/truncate bounds.

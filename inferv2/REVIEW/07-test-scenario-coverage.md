# WaaV Infer — Test / Scenario Coverage Audit (Phase A, hands-on)

Method: keyword match on test-fn NAMES across all `crates/**/*.rs` (incl. `#[cfg(test)]` mods in `src`).
824 fns under `#[test]`/`#[tokio::test]`. A scenario MAY be covered under a different name — Phase D
validates the thin/zero areas empirically with live runs, so treat this as a prioritized hit-list, not proof.

## Strong coverage (no action)
`concurrent` 81 · `reject` 47 · `cancel` 28 · `empty` 27 · `barge` 20 · `shed` 17 · `drain` 17 ·
`race` 11 · `leak` 9 · `reconnect` 8 · `backpressure` 7 · `crash` 6 · `hang` 6 · `migration` 6 ·
`stale` 6 · `poison` 5 · `malformed` 5 · `timeout` 4 · `disconnect` 4 · `failover` 3.

## GAPS — zero direct test coverage (Phase D will exercise live; Phase E may add regression gates)
- **`chaos` / fault-injection = 0** — no test injects a transient GPU/CUDA fault, a mid-stream backend
  error, an OOM-recovery, or random partial failures. Enterprise chaos resilience is unproven by tests.
- **`fairness` / `starvation` = 0** — nothing proves a long/aggressive stream can't starve others, or that
  the cohort scheduler is fair under saturation. Critical for multi-tenant SLAs.
- **`oversized` = 0** — no test for an oversized / adversarially-huge input frame or utterance.
- **`rollout` = 0** — `scheduler/rollout.rs` exists but has no test named for it → suspected untested /
  possible shelf-ware (Phase A scheduler agent confirming).

## THIN coverage (1–2 tests — under-proven for "chaotic real-world")
- **`spike` = 1**, **`unbounded` = 1** — only the single #9 stress test guards the whole overload surface.
- **`deadlock` = 2** — only 2 deadlock guards despite the ORT first-touch deadlock we just found (a class,
  not a one-off). Need broader no-hang gates across the live concurrent paths.
- **`oom` = 2**, **`slow` (slow-consumer) = 2**, **`shutdown` = 2**, **`timeout` = 4** — light for the
  unified-memory + slow-consumer + graceful-drain risks that bit us this session.

## Phase D plan (live validation of the above)
1. Overload spike at 10×/100× MAX_ADMIT — confirm bounded queue, 429 shed, flat memory (extend #9).
2. Fairness: 1 long + N short concurrent streams — measure per-stream TTFA/latency; no starvation.
3. Slow / disconnecting consumer mid-stream — confirm bounded backpressure + slot/VRAM freed, no leak.
4. Sustained load (minutes) — memory stability (no slow leak), RTF stability, no degradation.
5. Oversized / zero-length / malformed inputs — typed reject, no panic/crash.
6. Chaos: kill the torch sidecar mid-request; force a CUDA OOM via tiny WAAV_ORT_GPU_MEM_LIMIT_BYTES —
   confirm clean error + worker survival (not crash/hang).
7. Barge-in storm — repeated rapid cancels, confirm only-own-slot cancel + no state corruption.

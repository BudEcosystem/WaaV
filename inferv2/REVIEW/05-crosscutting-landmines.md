# 05 — Cross-Cutting Enterprise-Readiness Landmine Sweep (all 13 crates)

**Scope:** static read-only sweep (rg + Read, NO build/run) across all 13 crates of `/home/bud/ditto/waav/waav-infer/crates/`, ~93K LoC (~50% test). Bar: *a single worker must not crash, hang, leak, or silently corrupt under chaotic real-world load.* Every finding is classified **live-path** vs **`#[cfg(test)]` shelf-ware** — only non-test, live-path instances are landmines. Confirmed bugs are separated from style.

---

## SEVERITY TALLY (confirmed, live-path)

| Severity | Count | One-liner |
|---|---|---|
| **CRITICAL** | **1** | Torch-sidecar out-of-band death-scan (`SidecarHeartbeat::check_at`) has no production poller — an idle-wedged/zombie sidecar between requests is tracked but never reaped (per-request bounded-read still protects in-flight calls, so blast radius is the idle-zombie window). |
| **HIGH** | **3** | (H1) Poison-pill / dead-letter / crash-loop quarantine subsystem fully built+exported but unwired (a repeatedly-crashing input is undefended). (H2) Datagram jitter-buffer `u64` seq overflow + the whole module is unwired shelf-ware. (H3) `RecycleGate.states` / crash+source maps have no eviction *and* no production caller — latent unbounded leak the instant anyone wires them. |
| **MEDIUM** | **6** | Unbounded coalescer job queue (bounded only indirectly by a semaphore); `SharedCreditPool` 5× poison-cascade `.expect`; encdec `slice_rows` OOB (no `ai<b` guard); diarize segmentation slice trusts claimed-frame-count vs data-len; qwen3/funasr silent-zero embedding on OOB token; arstep `u64→i32` ring-phase silent audio corruption on a multi-year slot. |
| **LOW / style** | many | Infallible-by-construction `.expect`s on hot paths, dead unbounded-channel methods, `CpuTier` sysfs re-sniff per load, kokoro silent-partial-audio swallow, etc. (listed per crate). |

**Headline.** The *running* hot path (ingress → scheduler → backend/ORT → core model → egress) is unusually disciplined: typed `Result<_, InferError>` everywhere, poison-recovering locks, saturating/checked arithmetic, `chunks_exact` instead of slice-index on client bytes, bounded decode loops, models locked *inside* `spawn_blocking`. **Zero client-reachable panic was found on the server front door.** The real enterprise risk is **architectural, not hot-loop**: several resilience/quarantine subsystems and an entire gateway-integration layer are *built, tested, and exported but never wired to a live path* — so the defenses cannot fire when needed, and the test suite looks far healthier than the live integration actually is.

---

# PART 1 — Cross-Cutting Landmine Sweep

## Per-crate landmine COUNTS

`unwrap`/`expect`/`panic` columns show **live (non-test) / test-shelf-ware**. "RawTotal" is the naive grep (for reference — it is dominated by tests). `unsafe` = real `unsafe {}` blocks (most crates are `#![forbid(unsafe_code)]`; raw grep matched the forbid line + doc prose, see note).

| Crate | LoC | unwrap (live/test) | expect (live/test) | panic (live/test) | real `unsafe` blocks | blocking-in-async | unbounded (live) | real-gap markers |
|---|---:|---|---|---|---:|---|---|---:|
| **waav-infer-runtime** | 24.1K | **0** / 228 | ~6 / ~234 (all infallible-by-construction) | 0 / 22 | **0** ("unsafe" hits = CUDA-graph domain term) | 0 (pure-sync: dedicated OS threads + `spawn_blocking`, no `.await`) | 0 wired (datagram overflow is unwired) | 4 unwired subsystems |
| **waav-infer-scheduler** | 15.0K | 0 / ~180 | ~2 / ~112 (both infallible) | 0 / 31 | **0** (`#![forbid]`) | 0 (1 live Mutex, poison-safe, never across await) | 0 active (1 latent gated behind unwired code) | 0 |
| **waav-infer-core** | 13.4K | ~0 / ~142 | ~38 live / ~167 test (most safe-by-loop-invariant) | 0 / 8 | **0** | 0 (fully sync; all `std::fs` is load-time) | 0 (every decode loop has a hard token cap) | 0 real |
| **waav-infer-server** | 9.8K | **0 client-reachable** / 49 | 5 boot/spawn-time / 75 | 0 / 16 | **0** | **0** (all 7 std Mutex inside `spawn_blocking`/dedicated thread; 1 live `thread::sleep` on dedicated mux thread) | 2 (coalescer job queues, see M1) | 0 real |
| **waav-infer-dag** | 7.5K | **3** (cloud.rs) / ~290 | 0 / (in test) | 0 / 0 | **0** (`#![forbid]`; 6 "unsafe" = doc prose) | N/A — crate has **zero async** (sync thread-per-stage) | 0 | 0 (the "39 markers" do not exist) |
| **waav-infer-provider** | 4.7K | **0** / 149 | **0** / 38 | **0** / 46 | **0** (`#![forbid]`; 2 "unsafe" = forbid line + doc) | N/A — zero async | 0 | 2 documented gaps (streaming-TTS no-op) |
| **waav-infer-backend-api** | 3.9K | 0 / ~16 | **5** poison-`.expect` / 17 | 0 / 2 | **0** (`#![forbid]`) | N/A — zero async (sync Condvar) | 0 | 0 real |
| **waav-infer-features** | 3.5K | **0** / 139 | 0 / 15 | 0 / 8 | **0** (`#![forbid]`) | N/A | 0 today (1 latent if `EncoderCache` ever wired) | 0 real |
| **waav-infer-components** | 2.7K | **0** / 5 | **6** infallible / (test) | 0 / 0 | **0** (`#![forbid]`) | 0 (all 6 `std::fs` are load-time, sync `Model::load`) | 0 | 0 |
| **waav-infer-backend-ort** | 2.1K | **0** / 19 | **0** / 45 | 0 / 4 | **1** (libloading dlopen, documented + correct) | N/A — zero async (caller must `spawn_blocking`) | 0 | 0 real |
| **waav-gateway-provider-api** | 1.4K | **2** infallible / 11 | 0 / 14 | 0 / 3 | **0** (`#![forbid]`) | N/A | 0 | 0 |
| **waav-infer-router** | 1.3K | **0** / 0 | 0 / 2 | 0 / 1 | **0** (`#![forbid]`) | N/A | 0 | 0 (whole crate unwired) |
| **waav-infer-protocol** | 1.2K | **2** infallible / 44 | 0 / 1 | 0 / 1 | **0** (`#![forbid]`) | N/A | 0 | 0 |

> **`unsafe` note:** the only real `unsafe {}` in the entire 13-crate tree is **one** block — `waav-infer-backend-ort/src/lib.rs:57` (`libloading::Library::new`, a deliberate `dlopen`-and-immediately-drop to convert ORT's re-entrant-`Once` load-deadlock into a typed `Err`; SAFETY comment present, no ptr deref/transmute/aliasing). **Every other crate is `#![forbid(unsafe_code)]`.** All the "unsafe=3/6/2/1" raw counts were the forbid-attribute line itself plus doc-comment prose ("we use `Arc<Mutex>` *instead of* unsafe"). **There is no transmute, no `from_raw_parts`, no manual `Send`/`Sync`, no dtype-byte reinterpretation anywhere** — the tensor seam is fully type-dispatched (`TensorData` enum → typed `ort::Tensor<T>`), so a wrong-dtype extraction returns `Err`, never UB.

### Markers (debt/gaps)
Raw grep flagged ~445 markers tree-wide. After reading them: **the overwhelming majority are doc-prose** ("not yet on the M1 path", "for now we…", roadmap references to `INFER_BUILD_TODO.md`) describing *implemented* behavior or *intentional* M1 scope, **not** broken live code. Confirmed **zero** `unimplemented!`/`todo!`/`unreachable!` on any live path; **zero** stub handler bodies; **zero** "for now" wrong-behavior on a live request path. The genuine production gaps are the *unwired subsystems* tracked as CRIT/HIGH below, plus the provider's documented streaming-TTS no-op (P-prov-1).

---

## Confirmed dangerous live-path findings (severity-ranked)

### CRITICAL

**C1 — Torch-sidecar out-of-band death-scan never polled in production.**
`waav-infer-runtime/src/watchdog.rs:2835` `SidecarHeartbeat::check_at` (the app-heartbeat-window death scan). `register`/`record_heartbeat`/`deregister` **are** wired (`waav-infer-server/src/torch_sidecar.rs:170/272/301`), but **every caller of `check_at` is inside `#[cfg(test)]`** (boundary `torch_sidecar.rs:487`; callers at 576/624/650/667). No production thread/interval polls it.
*Nuance (verified):* the per-request path IS protected by a separate **bounded `read_frame_bounded` reaper** (torch_sidecar.rs ~233) — a response that doesn't arrive within the request deadline trips a `kill()`, so an in-flight inference against a wedged sidecar does NOT hang the worker. The unprotected case is the **idle/zombie sidecar between requests** (the 15 s app-heartbeat window the scan exists to enforce) — it is tracked in the ledger but never declared dead, so a wedged-but-idle sidecar is never recycled until the next request happens to hit the per-request reaper. **Fix:** install an out-of-band `tokio::time::interval` poller calling `check_at` (mirror the already-correct `spawn_watchdog`→`FrameWatchdog::check_at` at `waav-infer-server/src/lib.rs:342`).

### HIGH

**H1 — Poison-pill / dead-letter / crash-loop quarantine is fully-built shelf-ware (undefended live).**
`waav-infer-runtime/src/watchdog.rs`: `InputFirewall::admit` (1430), `DeadLetterSink::record_dead_letter` (1820), `SourceRateLimiter::capture` (1674), `CrashRecord::record_crash` (1463). Tree-wide `rg`: these appear outside watchdog.rs **only** as `lib.rs` re-exports; `record_crash`/`record_dead_letter`/`request_recycle` have **zero callers anywhere**. The catalog-J17 defense against a repeatedly-crashing input is coded, tested, exported — and **never connected to the admission queue**. A real crash-loop input is undefended despite the machinery existing.

**H2 — Datagram jitter-buffer `u64` sequence overflow (and the whole module is unwired).**
`waav-infer-runtime/src/datagram.rs:288` `if seq > cursor + self.depth as u64` and `:292` `let new_cursor = seq - self.depth as u64`, where `seq`/`cursor` are peer-controlled `MediaSeq(u64)` (J18 on-wire seq, monotonic, never reset down). Near `u64::MAX`: debug-panic or release-wrap that **defeats the very horizon test this function exists to enforce** (a frame admitted past the bound → jitter buffer no longer bounded). Held at HIGH (not CRIT) because the **entire `DatagramJitterBuffer` is unwired** — every `ingest` caller is in `#[cfg(test)]` (boundary `datagram.rs:387`); only a `lib.rs:36` re-export exists. **Fix before it ships:** `cursor.saturating_add(depth)` / `seq.checked_sub(cursor)`. (Companion `MediaSeq::next` datagram.rs:110 `self.0 + 1` unchecked — Low.)

**H3 — Quarantine/recycle maps have no eviction AND no production caller (latent unbounded leak).**
`waav-infer-runtime/src/watchdog.rs`: `RecycleGate.states` (BTreeMap field ~1961) is inserted via `entry().or_default()` (2024/2028) keyed by `ChannelId` and **the type has no `remove`/`retain`/`clear` at all** — one entry per distinct channel, never freed. Independently, the CrashRecord (insert 1463), DeadLetter (1690), SourceRecord (1822) maps are pruned only by `clear_channel`/`clear_source` which are **test-only callers**. All masked today because nothing drives them (H1), but they become per-channel/per-input-signature memory leaks the instant anyone wires the quarantine path. **Trap:** an integrator wiring `InputFirewall` without also wiring `clear_channel`/`clear_source` + a `RecycleGate.states` eviction trades an unwired defense for a live leak. **Fix:** wire the clears into the slot-recycle (F3) path *at the same time* as the firewall.

### MEDIUM

**M1 — Unbounded coalescer job queue (bounded only indirectly).**
`waav-infer-server/src/tts_coalescer.rs:51/70` and `stt_coalescer.rs:46/65`: `mpsc::unbounded_channel` for submit jobs. The *cohort* is bounded (`MAX_BATCH=24` drains/forward) but the *queue feeding it* is not — `submit` does `tx.send(...)` with no `try_send`/capacity check. **Why only MED:** in normal flow the admission `Semaphore` (`max_concurrency=4`) is acquired before `synthesize`/`transcribe`, so at most N callers are in `submit().await` → backlog is *accidentally* bounded. But the bound is indirect with no defense-in-depth; the asymmetry with the *already-hardened* `codec_ar_batcher` (which uses bounded `channel(256)` + typed-429) is itself a smell. **Fix:** bounded `channel(N)` + `try_send`→429, mirroring the codec path.

**M2 — `SharedCreditPool` 5× poison-cascade `.expect`.**
`waav-infer-backend-api/src/lib.rs:1217/1234/1252/1273/1287` `.expect("credit pool mutex poisoned")` on the relay back-pressure path. If any holder panics, the lock poisons and **every subsequent credit op panics**, cascading one failure into a crash of all hand-offs on that edge. Critical sections are tiny (int inc/dec) so poisoning is unlikely, but the posture is inconsistent with this crate's own deadlock test which uses `unwrap_or_else(|e| e.into_inner())` to *survive* poison. **Currently dead-code (0 external callers)** so live blast radius is nil until wired. **Fix:** `unwrap_or_else(|e| e.into_inner())` at all 5.

**M3 — `slice_rows` OOB: no `ai < b` bound check.**
`waav-infer-core/src/stt/encdec.rs:563,573`. `v.len() >= b*row_elems` is checked, but the loop does `v[ai*row_elems..(ai+1)*row_elems]` for each `ai` in `active`/`survivors` with **no guarantee `ai < b`**. A cohort-bookkeeping bug (survivor index ≥ batch size, e.g. `compact_kv_positions:587`) → panic. Defensive gap on the batched-decode path. **Fix:** `if ai >= b { return Err(...) }`.

**M4 — Diarize segmentation slice trusts claimed frame-count vs data length.**
`waav-infer-core/src/diarize.rs:209`. `frames = y.shape.get(1).unwrap_or(&0)` then `data[f*NUM_CLASSES..(f+1)*NUM_CLASSES]` with no `data.len() >= frames*NUM_CLASSES` guard. A graph whose declared `shape[1]` exceeds its data length → panic. **Fix:** length guard before the slice.

**M5 — Silent-zero embedding on out-of-range token (silent wrong transcript).**
`waav-infer-core/src/stt/qwen3_asr.rs:109-122` and `stt/funasr_nano.rs` `embed_row(t)`: an out-of-table token id emits `0.0` for the out-of-range dims → all-zero embedding → garbage next step → **silent wrong transcript, no crash**. Tokens normally come from a bounded argmax, but a corrupt-logits path degrades silently. **Fix:** assert `t < vocab`.

**M6 — `arstep` ring-phase `u64→i32` truncation → silent audio corruption.**
`waav-infer-runtime/src/arstep.rs:387,396` `(self.offset as i32 + self.delays[k]).rem_euclid(self.ct as i32)`. `offset: u64` increments once per AR stride for the slot's whole lifetime; past ~2.1B strides the `u64→i32` cast goes negative → wrong ring cell → silently glitched audio (`rem_euclid` keeps it in range, no panic). Only on a multi-year never-recycled slot, but a silent-corruption cast in otherwise `u64`-clean code. **Fix:** take the modulo in `u64` before `as i32`.

### LOW / style (confirmed, non-urgent)
- `waav-infer-core/src/tts/kokoro.rs:244` — `unwrap_or_default()` swallows a missing/wrong-dtype `waveform`: on a multi-segment utterance one bad segment silently contributes *silence* while others succeed, the `is_empty()` guard never fires → **silent partial audio**. Return `Err` instead.
- `waav-infer-server/src/engine.rs:1060` `serve_codec_ar_stream` — a `pub` method carrying an **unbounded channel** (engine.rs:1066) that is now **test-only callers** (live path uses the bounded `CodecArBatcher`). Dead code inviting a future regression. `#[cfg(test)]`-gate or delete. Same for `serve_codec_ar_streams`/`..._guarded`.
- `waav-infer-server/src/codec_ar_batcher.rs:342` `.expect("spawn codec-ar shared loop thread")` — the one spawn-time panic reachable under OS thread exhaustion (poisons the `OnceCell`). Prefer typed `InferError`.
- `waav-infer-dag/src/cloud.rs:221/237/245` — `.lock().unwrap()`/`.wait_timeout(...).unwrap()` panic on a *poisoned* mutex (sibling `backpressure.rs` recovers via `into_inner()`). Only on a stage-thread panic; unwired today.
- `waav-infer-backend-ort/src/cpu_tier.rs:59/69/81` — `CpuTier::detect()` re-walks sysfs (3 blocking `std::fs` reads) on *every* CPU-EP model load, no caching (load-time, graceful fallback). Add a `OnceLock`.
- `waav-infer-components` 6× `.expect("rfft"/"irfft")` (mel.rs:97, stft.rs:89/117, nemo_mel.rs:106, kaldi_fbank.rs:99) — infallible-by-construction (buffers are `make_*_vec()`-sized) but run per-frame on every model; prefer `?` for defense-in-depth.
- `waav-infer-core/src/stt/parakeet.rs:206` — `argmax(&logits[vocab_size..])` on a possibly-empty duration slice → `skip=0` forced → silent-wrong (no hang). Minor.
- `waav-infer-core/src/diarize.rs:329-388` — agglomerative clustering is ~O(n⁴) in embedding count with no time budget → adversarially long multi-speaker audio = CPU slow-loris (still terminates). Consider an embedding-count ceiling.

### Explicitly checked and CLEARED (so they are not mistaken for gaps)
- **`FrameWatchdog::check_at` (session stall) IS wired** — `waav-infer-server/src/lib.rs:342` `spawn_watchdog` runs a real 1 s `tokio::time::interval` poller that sheds silently-hung sessions. The #1 silent-hang defense fires. (Contrast C1's *sidecar* scan.)
- **Lock poisoning, hot path: clean.** Runtime/scheduler/server model locks use `.lock().unwrap_or_else(|p| p.into_inner())` (poison-recovering) and live inside `spawn_blocking`/dedicated OS threads — never across `.await`. A panicked request never bricks the model.
- **Blocking-in-async: none on the server front door.** All 7 server std Mutex are inside `spawn_blocking`; the 1 live `thread::sleep` (codec_ar_batcher.rs:371, 2 ms park) is on the dedicated `codec-ar-mux` OS thread, not a tokio worker; all `std::fs` is startup/model-load; Symphonia decode is correctly `spawn_blocking`'d (lib.rs:947).
- **Client-input panic surface: zero.** `ws.rs`/`ingress.rs`/`control.rs` parse client data via `serde_json::from_str`→typed error and `chunks_exact` (never slice-index); `ingress.rs:52` validates sample-rate `4000..=192000` (defends the rate-0 → infinite-resample/huge-alloc trap); `ws.rs:186` caps per-session audio; WS session count, body size, text bytes all capped.
- **Decode loops: all bounded.** Every AR/CTC/transducer/CFM loop in `-core` has a hard token/length cap + EOS break (verified ledger: chatterbox 1000, voxtral/qwen3/funasr `MAX_NEW_TOKENS`, canary 1024, cohere 448, etc.). No unbounded generation/hang.
- **KV/prefix cache: capped, no panic-on-exhaustion** (`paged_kv` evicts via `while resident > max { pop_front }` with anti-spin break; `prefix_cache` evicts before insert). **Scheduler** is bounds-checked throughout (`.get()`/`.get_mut()`, saturating/checked arithmetic, every divisor validated `>0` at construction, reconnect storm is a fixed 4-scalar token bucket with **no per-client map** → a reconnect storm cannot grow memory).

---

# PART 2 — Small-Crate Review (purpose | integrated? | issues)

> **Definitive integration fact (from the Cargo.toml dependency graph + non-test symbol grep):** `waav-infer-server` depends on `{backend-api, backend-ort, components, core, dag, protocol, runtime, scheduler}`. It does **NOT** depend on `provider`, `gateway-provider-api`, or `router`. The server serves STT/TTS **directly** through `waav_infer_core::model::{SttModel, TtsModel}` in `cascade.rs`/`ws.rs`/coalescers. The entire **gateway-integration cluster (`waav-gateway-provider-api` ← `waav-infer-provider`, and `waav-infer-router`) is compiled-but-unwired scaffolding** awaiting the external WaaV gateway seam — exercised only by its own tests.

### waav-infer-dag
- **Purpose:** Synchronous (no async/tokio) thread-per-stage blocking pipeline for heterogeneous cascades (STT→LLM→TTS, distinct `Paradigm` per stage over reused `BackpressureChannel`+`Condvar` edges), plus join/route/aggregate/barge-in/reset primitives.
- **Integrated?** **PARTIAL — CLI/test only.** `CascadeDag::spawn`/`run_cascade` is reached from the **CLI binary** (`waav-infer-server/src/bin/waav_infer.rs:461`, `run-dag` subcommand) and a live test (`cascade_live.rs`), NOT from any HTTP/WS request handler (`ws.rs`/`engine.rs`/`ingress.rs` have zero `CascadeDag` use — only doc-comments). Its `SpanAggregator` wraps `features::stable_span` (the one real cross-crate `features` consumer). The L2 primitives (join/route/aggregate/final_gate/final_prop/reset/terminal/channel_drop) are pub with **no non-test caller** — scaffolding ahead of wiring.
- **Issues:** Code quality is high and defensive (typed errors, poison→`Internal` mapping, `Drop` requests cooperative stop, empty-barrier rejected at construction). **Confirmed:** 3 LOW poison-handling nits in `cloud.rs:221/237/245` (use `into_inner()` like its sibling). The crate's analyzer flags (6 unsafe / 39 markers / 4 sleeps) are **all false positives** (`#![forbid(unsafe_code)]`; sleeps test-only; markers don't exist). **No crash/hang/leak landmine.** Risk = not-yet-wired to request handlers.

### waav-infer-provider
- **Purpose:** The `"waav-infer"` gateway provider adapter — implements gateway `BaseSTT`/`BaseTTS` (and native-S2S/WS-v1 framing, barge-in cancel, in-proc edge bootstrap) by driving the `-core` `SttModel`/`TtsModel` seam.
- **Integrated?** **NO — pure shelf-ware (tests only).** Zero `use waav_infer_provider` anywhere; no `Cargo.toml` depends on it (only the workspace glob compiles it). Every non-test construction of the adapters is `#[cfg(test)]`. `full_duplex_bench` is **misnamed** — no `benches/` dir, no `[[bench]]`; it's a `pub mod` driven only by unit tests.
- **Issues:** 72% test; **0 live** unwrap/expect/panic/unsafe; no blocking, no unbounded growth, no swallowed sends. **Documented functional gaps (real if a streaming model is registered):** `tts.rs:107-123` `speak_with_context` drops the context id and `clear_context` is a no-op ack (in-proc one-shot seam; streaming/AR mapping deferred); `stt.rs:124/131-133` `speed` no-op + `ttfs_p99_ms: None`. **Latent:** `barge_in.rs:125` calls `DagBargeIn::cancel` synchronously — must be confirmed non-blocking before a future async gateway drives it.

### waav-gateway-provider-api
- **Purpose:** The gateway-side provider contract — object-safe `BaseSTT`/`BaseTTS` traits, egress types (`STTResult`/`AudioData`), configs (`STTConfig`/`TTSConfig`/`Endpoint`), `AdmissionSurface`/`AdmissionReason`, `ProviderCapabilities`/`PrefixFingerprint`, `PluginConstructor`. Depends only on `-protocol` (engine-free, breaks the cycle).
- **Integrated?** **Wired ONLY to `provider` + `router`, both of which are themselves unwired — so server-unreachable.** A self-consistent, well-tested, server-disconnected sub-graph.
- **Issues:** Strongest code in the cluster — over-admission is *unrepresentable* (`total_slots: NonZeroU32`, `used` clamped to `total`, decision computed-not-stored, `try_admit` pure). 2 live `unwrap` (caps.rs:48-49 nibble→hex, infallible). Dead-ish forward seams: `PluginConstructor`/`Endpoint::{Unix,WsRemote}` have no live consumer. **No landmine.**

### waav-infer-router
- **Purpose:** Prefix-affinity / fleet-failover placement engine (`Router::route`, herd-spread, cross-tier failover) over `gateway-provider-api`.
- **Integrated?** **SHELF-WARE — ZERO dependents.** No crate's `Cargo.toml` depends on it; no non-test code references `waav_infer_router`/`RouterEndpoint`/`Router` anywhere. Designed as the gateway seam (`impl From<PrefixFingerprint> for PrefixKey`) but the gateway/provider never imports it. **The single biggest unwired surface:** a core scheduling/failover decision engine nothing calls.
- **Issues:** Code is correct + defensive even though dead (`f64::total_cmp` not `partial_cmp` → no NaN panic; empty-fleet → `NoHealthyReplica` not a phantom worker; no indexing/div). **No bug** — just unwired.

### waav-infer-features
- **Purpose:** Streaming-ASR feature primitives — encoder cache, contextual biasing, stable-span/local-agreement, ASR post, transport egress, text/SSML frontend.
- **Integrated?** **PARTIAL — 2 of 7 modules wired, 5 dead.** WIRED: `bias::{BiasContext,prefix_extra_key}` (via `provider/fingerprint.rs` — itself unwired — and conceptually mirrored in `server/calib.rs`'s prefix hash), `stable_span` (via `dag/aggregate.rs`). **DEAD (no non-test consumer):** `transport_egress` (despite being the "WIRED egress" in the prompt — the live server emits terminals via its **own** `dag/terminal.rs`+egress FSM, NOT this), `text_frontend`, `asr_post`, `feature_stage`, and `features::streaming_encoder_cache`.
- **Issues:** 0 live panic/unsafe. **Maintenance trap:** `features::StreamingEncoderCache` (BTreeMap open-set) and `runtime::prefix_cache::StreamingEncoderCache` (ring-buffer tier-3, the *live* one) are **two different types with the same name** — rename one. **Latent:** if `EncoderCache::thread_chunk` (streaming_encoder_cache.rs:79) is ever wired, it grows `cache_last_channel/time` unboundedly within a turn (the M3-ENC-T1 cap doesn't exist yet).

### waav-infer-components
- **Purpose:** Shared per-frame DSP/text primitives used by *every* model — mel/STFT/kaldi-fbank/nemo-mel/resample/CTC/G2P/tokenizer/unicode.
- **Integrated?** **FULLY WIRED & HOT** (depended on by core/server/features). The live per-frame layer.
- **Issues:** 0 live unwrap, 0 unsafe. The 6 live `.expect("rfft")` are infallible-by-construction (LOW, per-frame, prefer `?`). **Short/empty/huge-audio robustness verified SAFE:** `mel` always pads to fixed `N_SAMPLES` before indexing; `kaldi_fbank`/`nemo_mel`/`ctc`/`resample` all have empty + length + div-by-zero guards (`vocab==0→empty`, `out_len.max(2)` divisor, `cutoff>=nyq` guard). All `std::fs` is load-time sync `Model::load`. **No bug.**

### waav-infer-protocol
- **Purpose:** The wire spine — serde message/error/WS-frame/trace types. Depended on by 8 crates.
- **Integrated?** **FULLY WIRED.** Pure serde data types.
- **Issues:** 2 live `unwrap` (trace.rs:154-155 nibble→hex, infallible). **Malformed-client safety verified CORRECT:** `trace::parse` validates field count + exact widths *before* `hex_into` indexes (so a bad `traceparent` → typed `BadConfig`, never panic); all frames are `#[serde(tag)]` (unknown frame → serde `Err` at call site). `tts.rs:49` div-by-zero guarded. **No bug.**

### waav-infer-backend-api
- **Purpose:** The GPU-free tensor/inference seam abstraction (`StaticGraph` trait, `TensorData`/`ElemType`, `IoBinding`, `BackendError`, EP types) + a large M4.x stage-placer/relay/shm policy block.
- **Integrated?** **PARTIAL.** Core seam (`StaticGraph`, `TensorData`, `IoBinding`, `BackendError`, `EpKind`/`ActiveEp`/`EpCaps`, `SM12X_FORBIDDEN`) used across ~45 files — fully live. But **~900 lines of pub policy API have ZERO callers** (`StagePlacer`/`StageSpec`/`Placement`/`place`, `RelayPlan`/`relay_for`/`ZeroCopyBuffer`/`DoubleBuffer`/`CreditAllocator`/`SharedCreditPool`, `sniff_*`/`PayloadGraph`, `ShmReaper`/`ShmSegment`, `run_or_degrade`/`AttentionBackend`/`flashinfer_allowed`, `RepeatNgramGuard`, `recycle_decision`, …). `runtime/accel.rs` even re-derives the FlashInfer veto from `SM12X_FORBIDDEN` directly rather than calling `flashinfer_allowed`.
- **Issues:** **M2** (`SharedCreditPool` poison-cascade `.expect`, dead until wired). `#![forbid(unsafe_code)]`, no transmute. The dead block is tested only in isolation — its bit-identity/placement contracts are unproven against real callers. Feature-gate or remove.

### waav-infer-backend-ort
- **Purpose:** The ONNX-Runtime backend — session creation, EP (CUDA/CPU) selection, the per-request `session.run()`.
- **Integrated?** **FULLY WIRED & HOT** (the actual inference call for every ORT model).
- **Issues:** **The one real `unsafe` in the tree** (lib.rs:57 dlopen-preflight) is correct + documented. `session.run`/`run_bound` propagate every error via `?` (OOM/bad-shape → typed `Err`, never panic); the stateful `run_bound` epoch-rebind is verified correct end-to-end (supertonic per-utterance `wrapping_add`, encdec `AtomicU64`). 0 live unwrap/expect. **Design note (ops-visible):** EP-registration failure **degrades to CPU silently-to-caller** (telemetry-only, `waav_degraded_total` + `warn!`) — intentional P-6 policy, but a CUDA box that lost its driver serves on CPU with only a metric to show it. **Caller contract:** `StaticGraph::run` is sync + compute-bound and the trait can't enforce `spawn_blocking` — callers must (server does, via coalescers/spawn_blocking).

---

## Recommended fix order
1. **C1** — install an out-of-band poller for `SidecarHeartbeat::check_at` (idle-zombie sidecar reaping).
2. **H1 + H3 together** — wire `InputFirewall`/`DeadLetterSink`/`SourceRateLimiter` into admission **and** wire `clear_channel`/`clear_source` + a `RecycleGate.states` eviction into slot-recycle in the *same* change (or you trade an unwired defense for a live leak).
3. **M1** — bound the coalescer job queues (`channel(N)` + try_send→429), matching the codec batcher.
4. **H2** — `saturating_add`/`checked_sub` the datagram seq math before that module is integrated.
5. **M2–M6** — poison-tolerant `SharedCreditPool`; `ai<b` guard in encdec `slice_rows`; data-len guard in diarize; `t<vocab` assert in qwen3/funasr embed; `u64` modulo in arstep.
6. **Hygiene** — `#[cfg(test)]`-gate or delete the dead unbounded-channel engine methods; rename the duplicate `StreamingEncoderCache`; decide the fate of the ~900-line dead backend-api policy block and the unwired gateway cluster (`router`/`provider`/`gateway-provider-api` + the 5 dead `features` modules).

**Key files:** `waav-infer-runtime/src/{watchdog.rs, datagram.rs, arstep.rs}`; `waav-infer-server/src/{lib.rs, torch_sidecar.rs, tts_coalescer.rs, stt_coalescer.rs, engine.rs, codec_ar_batcher.rs}`; `waav-infer-backend-api/src/lib.rs`; `waav-infer-core/src/{stt/encdec.rs, diarize.rs, stt/qwen3_asr.rs, tts/kokoro.rs}`; `waav-infer-backend-ort/src/{lib.rs, cpu_tier.rs}`.

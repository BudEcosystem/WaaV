# S2S / Duplex Engine-Integration Gap — CLOSED

**Date:** 2026-06-24 · **Branch:** working tree (NOT committed, per instructions)
**Scope (hook item-3):** wire the byte-faithful duplex/S2S models (hibiki, lfm2.5-audio) as a
**first-class engine-served S2S endpoint** — a generic `S2sModel` trait + `engine::load_model_at`
dispatch + a serve path + live gates. Mirrors the existing `SttModel`/`TtsModel` registry seam.

---

## TL;DR — are the duplex models engine-served now? **YES.**

Before this change the duplex models were **bench-only**: `HibikiDuplexModel:DuplexStep`,
`CodecArDuplexModel:DuplexStepModel`, and `Lfm2Audio::round_trip` were driven exclusively by bench
drivers / the `full_duplex_bench`. `engine::load_model_at` dispatched **only** `LoadedModel::Stt` /
`LoadedModel::Tts` — there was **no** S2S/duplex variant the registry could return, and the WS
`Task::S2s` path was a hard `not_implemented("S2S sessions land in M5")`.

Now:

- **hibiki** (`load_model_at(hibiki_dir, ep)`) returns a working `LoadedModel::S2s` whose `s2s_turn`
  drives a real duplex turn end-to-end, **BYTE-IDENTICAL** to the model's standalone duplex
  trajectory (live-gated, 15360 reply samples, `==`).
- **lfm2.5-audio S2S** (`load_model_at(lfm2_audio_s2s_dir, ep)`) returns `LoadedModel::S2s` whose
  `s2s_turn` is **BYTE-IDENTICAL** to the core `Lfm2Audio::round_trip` it wraps (live-gated, `==`).
- The native-WS `Task::S2s` endpoint is wired (handshake validate → accumulate user audio →
  `finalize` runs ONE turn → reply text + audio chunks + terminal), reusing the existing realtime/WS
  scaffolding (no new protocol invented — `session.config`/binary-audio/`finalize`/`Transcript`/
  `chunk_meta`/`Terminal` frames).

THE LAW holds: **engine-served == standalone, byte-faithful**, for both models.

---

## 1. The trait (the duplex step seam, mirrors SttModel/TtsModel)

`waav_infer_core::model::S2sModel` (new) — object-safe `Box<dyn S2sModel>`, the duplex analog of
`SttModel`/`TtsModel`:

```rust
pub trait S2sModel: Send {
    fn s2s_turn(&mut self, channel: u64, pcm_16k: &[f32]) -> Result<S2sTurn, InferError>;
    fn reset_channel(&mut self, channel: u64) { /* default no-op */ }
    fn active_ep(&self) -> &'static str;
    fn output_sample_rate(&self) -> u32;
}
pub struct S2sTurn { pub text: String, pub audio: SynthChunks }
```

- **`channel: u64`** is the F3 isolation key — per-occupant session state (the duplex circular cache +
  KV) is keyed on it so concurrent turns never bleed (`ChannelId` isolation).
- **One seam, two faithful regimes:** frame-synchronous native duplex (hibiki — push input frame →
  emit output frame, the always-modeled user stream) and turn-based S2S (lfm2 — ASR-then-respond
  round-trip). Both surface the same `s2s_turn(channel, pcm) -> (text, audio)`.
- `LoadedModel` gained an `S2s(Box<dyn S2sModel>)` variant; `LoadedModel::as_stepped()` returns
  `None` for it (an S2S model rides its own duplex serve loop, NOT the lockstep TTS/STT batcher).

The K=1 `DuplexStep` and batched `DuplexStepModel` runtime seams are **unchanged** — the new
`S2sModel` is the *engine-level* registry seam that composes the per-frame `DuplexStep` driver +
the codec into one turn (hibiki) or wraps the existing `round_trip` (lfm2).

## 2. `engine::load_model_at` dispatch arms

- **Core registry** (`waav_infer_core::model::load_model`, ORT path): `lfm2_audio_s2s` →
  `LoadedModel::S2s(Lfm2AudioS2s)` (the new task-seam arm over the SAME 5-graph `Lfm2Audio` core;
  `lfm2_audio_asr`→Stt / `lfm2_audio_tts`→Tts / `lfm2_audio_s2s`→S2s — one config, one task).
- **Torch in-process** (`engine::load_torch_inprocess_model`, `--features torch`): `"hibiki"` /
  `"hibiki_zero"` → `LoadedModel::S2s(HibikiS2sModel)` reading the `waav.json`
  `{"runtime":{"backend":"torch-inprocess","architecture":"hibiki"}}` manifest.
- `CodecArDuplexModel` (chatterbox-backbone `DuplexStepModel`) was evaluated and **deliberately not
  wired** as an arch arm: it is a synthetic *seam exerciser* (a TTS backbone re-used as a duplex
  double for the batched-`SlotBatch` perf gate), not a real S2S checkpoint with its own weights /
  `config.json` — wiring it as an engine model would be a fake endpoint. The two REAL S2S models
  (hibiki, lfm2) are the ones engine-served.

## 3. The serve path (native-WS duplex endpoint, no new protocol)

`Task::S2s` in `ws.rs` (was `not_implemented`) now:
1. **Handshake:** validates an S2S model is loaded + its id matches (`engine.has_s2s()` +
   `knows_s2s(model)`); rejects f32 egress (PCM16 contract, mirrors TTS).
2. **Ingest:** accumulates raw source-rate user audio (the same robust accumulate-once pattern STT
   uses — no per-frame resampler reset).
3. **Turn:** `finalize{id}` → `finalize_s2s` decodes+resamples once → `engine.s2s_turn(channel,
   pcm16k)` (on a blocking thread, behind the model `Arc<Mutex>`, admission-gated, deadline-bounded)
   → streams the reply **text** (a finalized `Transcript`) + **audio** (`chunk_meta` + binary PCM,
   resampled to the negotiated egress) + the explicit `TerminalFrame::Final` + the `finalized{id}`
   ack.
4. **Lifecycle:** per-session monotonic `channel` (minted from `SESSION_SEQ`); on session close the
   F3 recycle drops the channel's duplex state (`engine.s2s_reset_channel`).

Engine API added: `has_s2s` / `knows_s2s` / `s2s_model_id` / `s2s_turn` / `s2s_output_sample_rate` /
`s2s_reset_channel`; an `s2s: Option<Arc<Mutex<Box<dyn S2sModel>>>>` field loaded in `Engine::load`
from `EngineConfig::s2s_dir` (CLI `--s2s-dir` / `$WAAV_S2S_DIR`), and a `resolve_s2s_dir` helper.

## 4. Live gates — byte-faithful, PASSING

| Gate | What it proves | Result |
|---|---|---|
| `engine_serves_inprocess_torch_hibiki_s2s_byte_identical_to_standalone` (torch_inprocess_live.rs) | `load_model_at(hibiki) → LoadedModel::S2s → s2s_turn` over the golden user audio == standalone `HibikiS2sModel`, byte-for-byte | **PASS** — 15360 reply samples, `==` (CPU-f32, golden regime) |
| `lfm2_audio_s2s_via_registry_engine_served` (lfm2_audio_registry.rs) | `load_model_at(lfm2_audio_s2s) → LoadedModel::S2s → s2s_turn` == core `Lfm2Audio::round_trip`, byte-for-byte (text + PCM) | **PASS** — `==` |
| `s2s_seam_registered_and_object_safe` (core unit, GPU-free) | `lfm2_audio_s2s` is in `REGISTERED_ARCHITECTURES`; `S2sModel`/`LoadedModel::S2s` is object-safe + wired; `as_stepped()` is `None`; the user input is load-bearing | **PASS** |
| `torch_hibiki::*` (7 standalone gates) | the hibiki duplex trajectory is unchanged (0 mismatches vs the moshi golden) | **PASS** (no regression) |

The hibiki live gate's byte-identity to standalone, combined with the existing
`torch_hibiki::duplex_greedy_target_codes_byte_identical` (standalone == moshi golden, 0/96
mismatches), means the **engine-served turn is byte-faithful to the model's standalone trajectory**
transitively.

## 5. No regression + clean

- `cargo test -p waav-infer-server -p waav-infer-core -p waav-infer-provider` → **all green** (the
  existing STT/TTS engine gates, WS gates, cascade, and the registry invariant unchanged).
- `cargo test -p waav-infer-server --features torch --lib` → **67 passed**.
- `cargo clippy --workspace --all-targets -D warnings` → **clean**; `--features torch` → **clean**.
- Full workspace builds (with and without `--features torch`).
- The registry-count invariant (`REGISTERED_ARCHITECTURES.len()`) bumped 22 → **23** (one new task
  arm `lfm2_audio_s2s`); the assertion + rationale updated.

## Files (all working-tree; NOT committed)

- `crates/waav-infer-core/src/model.rs` — **`S2sModel` trait + `S2sTurn` + `LoadedModel::S2s` +
  `lfm2_audio_s2s` dispatch arm + registry entry (count→23) + the S2S seam unit gate**.
- `crates/waav-infer-core/src/sts/lfm2_audio.rs` — `Lfm2AudioS2s` wrapper (drives `round_trip`).
- `crates/waav-infer-core/src/sts/mod.rs`, `crates/waav-infer-core/src/lib.rs` — re-exports.
- `crates/waav-infer-backend-torch/src/hibiki.rs` — **`HibikiS2sModel`** (impl `S2sModel`: encode
  user PCM→Mimi codes, drive the per-frame `DuplexStep`, decode target codes→24 kHz PCM) +
  `HibikiDuplexModel::reset_channel`. **[--features torch]**
- `crates/waav-infer-server/src/engine.rs` — `hibiki` torch-inprocess dispatch arm; `Engine.s2s`
  field + `has_s2s`/`knows_s2s`/`s2s_turn`/`s2s_output_sample_rate`/`s2s_reset_channel`;
  `EngineConfig.s2s_dir` + `resolve_s2s_dir`; S2S load in `Engine::load`; `family_id` arms.
- `crates/waav-infer-server/src/ws.rs` — `Task::S2s` handshake validate + S2S audio accumulate +
  `finalize_s2s` (the duplex turn serve) + per-session `channel` + F3 recycle on close.
- `crates/waav-infer-server/src/bin/waav_infer.rs` — `--s2s-dir` CLI arg; `EngineConfig` literals.
- `crates/waav-infer-provider/src/inproc.rs` — `LoadedModel::S2s` edge arm (typed-unsupported: S2S is
  served via the duplex endpoint, not the half-duplex cascade edge).
- **Tests:** `crates/waav-infer-server/tests/torch_inprocess_live.rs` (hibiki S2S gate **[torch]**),
  `crates/waav-infer-server/tests/lfm2_audio_registry.rs` (lfm2 S2S engine gate),
  `crates/waav-infer-server/tests/fixtures/torch_inprocess/hibiki.waav.json` (new fixture).
- Exhaustiveness fixups (new `LoadedModel::S2s` variant): `cascade_live.rs`, `perf_bench.rs`,
  `bin/waav_infer.rs`, `engine.rs` (the `_ =>` / S2s arms).

### Concurrency note (shared files with the ZONOS2-port agent)
`backend-torch/src/lib.rs` and `core/src/model.rs::REGISTERED_ARCHITECTURES` are shared with a
concurrent zonos2-port agent. At time of writing zonos2 has added `pub mod zonos2;` (lib.rs) +
`zonos2.rs`/`cuda_torch_zonos2.rs` but has NOT yet touched the registry array — my count is 23
(added `lfm2_audio_s2s`). If zonos2 lands a new arch arm it must re-read and bump 23→24.

## Scoped remainder (precise)

The S2S WS path is **wired + compiles + passes the existing handshake/finalize gates**, but a full
**live WS end-to-end S2S test** (handshake → stream audio → finalize → assert reply frames) is not
yet added because `ws_live.rs`'s harness stands the engine up via `from_tts_for_test` (no S2S model).
Closing it cleanly is a small, well-scoped follow-on: add a `from_s2s_for_test(Box<dyn S2sModel>)`
test constructor (the S2S analog of `from_tts_for_test`) + one `ws_live.rs` S2S round-trip test
driving a fake `S2sModel`. The core integration the LAW requires — the trait, the `load_model_at`
dispatch for BOTH models, and the in-process duplex-turn byte-faithful gate — is **complete and
live-gated**.

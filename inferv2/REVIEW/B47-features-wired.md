# B47 — `waav-infer-features` text frontend WIRED into the live serve TEXT path

**Goal (item-3 "wire" half).** The `waav-infer-features` crate (SSML / text-normalization frontend
edges) was built + tested but had **0 live serve callers** (per `SHELFWARE_DISPOSITION_ITEM3.md`:
"features … pending wire into the serve TEXT path"). This change folds the crate's `TextFrontend`
into the TTS serve text path on **both** surfaces, so it is no longer shelfware: it now runs on every
REST `/v1/audio/speech` request and every native-WS `speak` frame BEFORE the text reaches the model
`synthesize` (or the codec-AR batcher).

It is **additive / opt-in-safe**: plain text passes through **byte-identical** under the default
policy, so every existing model gate's synthesize input (hence its audio) is unchanged.

---

## Where the frontend now applies on the serve path

The wrapper `crates/waav-infer-server/src/text_frontend.rs::normalize_tts_text(raw, policy, lang)`
calls `waav_infer_features::TextFrontend::process` and returns the normalized plain text (or the
feature crate's typed `InferError`). Both serve paths call it immediately after the cheap
empty/length/voice/format validation and **before** admission — so it precedes every downstream
synthesize route, and a typed reject costs no admission permit.

### REST `/v1/audio/speech` (`crates/waav-infer-server/src/lib.rs`)
- **`lib.rs:861`** — `let input = normalize_tts_text(&req.input, s.limits.text_norm, tts_lang(&s.engine))?`
  (returns the typed `InferError` envelope on malformed SSML).
- The normalized `input` then feeds **both** downstream TTS routes (it replaced `req.input` at each):
  - codec-AR streaming batcher: **`lib.rs:885`** `batcher.submit(input, …)`
  - one-shot whole-utterance: **`lib.rs:926`** `s.engine.synthesize(input, voice, speed)`

### Native-WS `speak` (`crates/waav-infer-server/src/ws.rs`)
- **`ws.rs:301`** — `let text = normalize_tts_text(&text, s.limits().text_norm, &lang)?`
  (typed error surfaced on the WS `error` frame via `send_err`, never a panic).
- The normalized `text` then feeds **both** downstream TTS routes (replaced the raw `text` at each):
  - codec-AR streaming batcher: **`ws.rs:328`** `batcher.submit(text, …)`
  - one-shot whole-utterance: **`ws.rs:408`** `s.engine.synthesize(text, …)`

Language tag for the frontend: REST uses `tts_lang(engine)` (the TTS model's first declared language,
else BCP-47 `"und"`); WS uses `ws_speak_lang` (the session's negotiated/updated `cfg.language` if set,
else the engine's). The SSML-core edge wired here is locale-**independent** (tag→marker mapping +
whitespace/entity normalization are language-agnostic); the language only seeds the additive locale-TN
follow-on.

### Capability choice (why audio is unchanged for SSML too)
The coarse `TtsModel::synthesize(text, voice, speed)` seam is a **flat text string** — it cannot carry
`<break>`/`<prosody>` markers. So the frontend runs with `SsmlCapability::default()` (all capabilities
off): every SSML tag **degrades to plain text** (markup stripped, content kept, TTS-113), the model
receives the deterministic plain word stream, and **no tag literal ever reaches the model**. Wiring the
per-model pause/prosody markers into a *structured* synthesize call is an additive follow-on that needs
a seam change this bounded wire deliberately does not make.

---

## The config flag (opinionated normalization is opt-in)

New `ServerLimits.text_norm: TextNormPolicy` (`lib.rs:81`, default `SsmlOnly` at `lib.rs:102`;
re-exported at `lib.rs:43`). Three modes:

| Policy | Plain text (no `<`) | SSML markup | Malformed SSML |
|---|---|---|---|
| **`SsmlOnly`** (default) | **passed through byte-identical** (frontend not invoked) | parsed → normalized plain stream | typed `InferError` |
| `Full` (opt-in) | whitespace-collapsed + entity-decoded (feature `NormText` canon) | parsed → normalized plain stream | typed `InferError` |
| `Off` (escape hatch) | passed through unchanged | passed through unchanged (a tag literal would reach the model) | passed through |

The default skips the frontend entirely for markup-free input (`has_markup` = "contains `<`"), which is
what makes plain text **byte-identical to today**. Markup always routes through the frontend regardless
of policy (so a tag literal never reaches the model and malformed SSML is always typed).

---

## Pass-through-unchanged proof (the additive invariant)

Two layers prove it:

1. **Unit (`text_frontend::tests::ssml_only_passes_plain_text_through_byte_identical`)** — under
   `SsmlOnly`, `normalize_tts_text` returns the **exact input bytes** for a battery of plain inputs
   including non-canonical whitespace that `Full` *would* collapse (`"Hello   world"`,
   `"  leading and trailing  "`, `"tabs\tand\nnewlines"`) and a non-markup ampersand (`"Tom & Jerry"`).
   `off_policy_passes_everything_through_unchanged` proves `Off` is a total pass-through (even SSML).

2. **Serve-level (`tests/features_text_frontend.rs::b47a_plain_text_reaches_synthesize_byte_identical`)**
   — a **recording** TTS fake captures the exact `&str` that crosses the `synthesize` seam, driven
   through the REAL axum router (`build_router` → `/v1/audio/speech` → `tower::oneshot`). For each plain
   input the captured synthesize argument is asserted **byte-identical** to the request `input`. This is
   the end-to-end proof that no existing model gate's synthesize input (hence its audio) changes.

(The feature crate's own gate-tests prove `process` is deterministic across repeated calls — no
hash/iteration-order nondeterminism — so the byte-identity is stable.)

---

## SSML / normalization + typed-error tests

**Serve-level integration (`crates/waav-infer-server/tests/features_text_frontend.rs`, 4 tests):**
- `b47a_plain_text_reaches_synthesize_byte_identical` — (a) pass-through, above.
- `b47b_ssml_input_is_normalized_before_synthesize` — (b) SSML `input`
  `<speak>Hello <prosody rate="slow">brave</prosody> &amp; new world</speak>` reaches `synthesize` as
  `"Hello brave & new world"`: tags stripped, content in order, whitespace collapsed, `&amp;` decoded,
  **no `<` / `&amp;` reaches the model**.
- `b47b_full_policy_normalizes_plain_text_before_synthesize` — (b') the opt-in `Full` policy collapses
  `"  the   quick\tbrown\nfox  "` → `"the quick brown fox"` before the seam (proves the opinionated mode
  is reachable end-to-end).
- `b47c_malformed_ssml_is_typed_400_and_skips_synthesize` — (c) `"hello <break world"` (unterminated
  `<`) → HTTP **400** with the typed `{"error":{"code":"bad_config","retriable":false,…}}` envelope, AND
  the recording fake confirms `synthesize` was **NEVER invoked** (rejected at the edge — it can never
  reach/poison the model).

**Unit (`crates/waav-infer-server/src/text_frontend.rs::tests`, 5 tests)** — the same three behaviors at
the helper boundary plus the `Off`/`Full` policy matrix:
`ssml_only_passes_plain_text_through_byte_identical`, `off_policy_passes_everything_through_unchanged`,
`ssml_markup_is_normalized_to_a_plain_word_stream`, `full_policy_normalizes_plain_text`,
`malformed_ssml_yields_the_typed_infer_error` (asserts `ErrorCode::BadConfig` + `!retriable`, surfaced
verbatim from the feature crate — never a panic).

The typed error is the feature crate's own taxonomy: `TextFrontend::process` returns
`InferError::bad_config(...)`, which `normalize_tts_text` propagates with `?`; the REST handler renders
it via the existing `err()` envelope (400, `bad_config`) and the WS handler via `send_err()` (the
`error` frame) — no new error type, full reuse of the `-protocol` `InferError`.

---

## Files changed (ONLY `crates/waav-infer-server/` + a server test)

| File | Change |
|---|---|
| `crates/waav-infer-server/Cargo.toml` | add `waav-infer-features.workspace = true` (already a workspace dep; pure-logic, no backend deps) |
| `crates/waav-infer-server/src/text_frontend.rs` | **NEW** — `TextNormPolicy` enum + `normalize_tts_text()` wrapper over `waav_infer_features::TextFrontend` (+ 5 unit tests) |
| `crates/waav-infer-server/src/lib.rs` | `pub mod text_frontend;` + `pub use TextNormPolicy`; `ServerLimits.text_norm` field (default `SsmlOnly`); apply at REST `speech` (line 861) feeding submit/synthesize; `tts_lang()` helper |
| `crates/waav-infer-server/src/ws.rs` | apply at WS `speak` (line 301) feeding submit/synthesize; `ws_speak_lang()` helper |
| `crates/waav-infer-server/tests/features_text_frontend.rs` | **NEW** — 4 serve-level HTTP integration tests (recording TTS → `build_router` → `tower::oneshot`) proving pass-through / normalization / typed-error |

No backend-torch model crate, scheduler, or runtime touched. No `git commit`.

---

## Law — bit-faithful + green (verified)

`source gb10-env.sh`, then:
- `cargo test -p waav-infer-features -p waav-infer-server --lib` → **features 54/54, server 66/66**
  (0 failed; the 4 ignored are pre-existing live-GPU tests, unrelated).
- `cargo test -p waav-infer-server --test features_text_frontend` → **4/4** (the new serve-level gates).
- `cargo test -p waav-infer-server --test oversized_input` → **4/4** (neighbor regression check: clean).
- `cargo clippy -p waav-infer-server --all-targets -- -D warnings` → **clean** (default, no-feature build).
- `cargo clippy -p waav-infer-server --all-targets --features torch -- -D warnings` → **clean** (the
  shared lib.rs/ws.rs edits compile under the torch feature too).
- `cargo clippy -p waav-infer-features --all-targets -- -D warnings` → **clean**.

The wiring is not feature-gated (it has no backend dependency), so the ONNX-only default build carries
it with zero extra link cost; both feature configs were clippy-verified regardless.

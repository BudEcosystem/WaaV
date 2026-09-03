# B20 — Wiring the poison-pill Quarantine's 3 call-sites into the LIVE codec-AR serve/admission path

**Status: DONE.** The poison-pill quarantine now **FIRES on real traffic.** B14 built + tested the machinery
(`InputFirewall` + `DeadLetterSink` + `SourceRateLimiter`, bundled into `Quarantine`) and exposed stable
hooks on `AppState`/`ProdSpine`, but the actual invocations lived in the codec-AR serve/admission path it
was forbidden to touch — so nothing reported crashes and nothing was quarantined. This wires the three
call-sites into `CodecArBatcher` (the seam every live WS + REST codec-AR stream flows through), so a
crash-loop input is now dead-lettered and **refused on replay through the REAL admission path.**

Gates green:
- `source gb10-env.sh && cargo test -p waav-infer-server --lib` → **61 passed; 0 failed; 4 ignored**
  (the 4 ignored are pre-existing live-GPU-gated, unrelated).
- `cargo clippy -p waav-infer-server --all-targets -- -D warnings` → **clean** (lib + tests + the chaos
  integration test).
- `cargo test -p waav-infer-server --test chaos_concurrency` → **3 passed; 0 failed** (concurrency +
  bit-identity + the "one slot's backend fault does not poison the others" gate all still hold).

Touched ONLY the allowed set: `crates/waav-infer-server/{codec_ar_batcher.rs, lib.rs}` +
`crates/waav-infer-server/tests/chaos_concurrency.rs` (a mechanical 5→6-arg call-site fix for the new
`CodecArBatcher::new` signature). **Did NOT touch** the candle/backend-torch crates, the scheduler crate,
or `runtime/watchdog.rs` (the machinery — left exactly as B14 shipped it). No `git commit`.

---

## Where the codec-AR stream flows (the trace that located the seam)

```
ws.rs::speak / lib.rs::speech                      ← per-request handler (WS native + REST OpenAI-compat)
  └─ batcher.submit(text, deadline)                ← the ONE dispatch point for a codec-AR stream
       ├─ [admission gate] try_admit (GATE #9)     ← bounded concurrency + VRAM + deadline
       └─ tok_tx.try_send(Submission{…})           ← onto the bounded submission channel
            └─ forwarder task (spawn_loop)          ← builds the MuxAdmit + its egress sink closure
                 └─ std_tx.send(MuxAdmit{ sink })   ← onto the runtime's std::sync::mpsc admission channel
                      └─ serve_codec_ar_multiplexed_bounded(…)   ← RUNTIME CRATE (forbidden) — the shared
                           · mints ChannelId per slot (mint closure)  lockstep loop: prefill/step_batch/
                           · decode_audio_stream → Err ⇒ Terminal::Error  decode/reset_slot, F3 recycle
                           · sink(EgressEvent::Delta|End)            ← calls back into the server-crate sink
```

**The decisive finding:** the runtime loop (where the `ChannelId` is minted, where decode crashes, where the
F3 slot-recycle happens) is the *forbidden* `runtime/serve.rs`. But every one of those events is **observable
in the server crate** through the seam the server crate owns — the `MuxAdmit.sink` closure built in
`codec_ar_batcher.rs::spawn_loop`, and the `submit` entry point. A decode-killing input
(`decode_audio_stream`/`step` fault, or the H1 NaN-reject) closes its stream on a **`TerminalFrame::Error`**
that flows out through that sink as `EgressEvent::End(Terminal::Error(e))`. So **all three call-sites fit
inside `CodecArBatcher` with the `Quarantine` handle threaded in — zero runtime-crate edits.** This is the
exact seam B14 pointed at ("the call-site lives in the codec-AR serve loop / admission path").

---

## The 3 call-sites added

The `Quarantine` is threaded into the batcher: `ProdSpine::new` already holds it; `AppState::new` now passes
`spine.quarantine.clone()` into `CodecArBatcher::new` (**`lib.rs:228`**), so the batcher reports/admits/clears
against the **same `Arc` ledgers** the out-of-band poller (`spawn_watchdog`) ages out — a dead-letter the
serve path records is the dead-letter `admit` refuses and the poller evicts. It rides into each stream's sink
on the `Submission` struct (new fields `channel`, `signature`, `quarantine`).

### 1. Admit-check before dispatch — `codec_ar_batcher.rs:221` (inside `submit`, line 212 marker)
```rust
let (channel, signature) = Self::ingress_fingerprint(&utterance);
if let Err(e) = self.quarantine.admit(channel, signature) {
    metrics::counter!("waav_infer_quarantine_refused_total", "code" => e.code.label()).increment(1);
    return Err(e);
}
```
First thing `submit` does — **before** the GATE #9 capacity `try_admit` (a poison-pill must not even consume
a concurrency slot). A dead-lettered crash-loop input is refused with the firewall's typed `BadConfig`
(**not** retriable — the gateway must not replay a poison-pill); a source flooding distinct pills is refused
with a retriable `AdmissionRejected` (429). On a typed refusal the stream is **shed with the returned error**
and never dispatched.

### 2. Report a decode crash — `codec_ar_batcher.rs:345` (in the sink's `EgressEvent::End` branch, marker 332)
```rust
EgressEvent::End(t) => {
    let wire = t.to_wire();
    if let TerminalFrame::Error(e) = &wire && is_decode_fault(e.code) {
        let _ = quarantine.report_decode_crash(channel, signature);
        metrics::counter!("waav_infer_decode_crash_reported_total").increment(1);
    }
    …
}
```
This is the point in the server crate where the serve loop's typed `Error` terminal lands — i.e. the point
the loop already tore the slot down on a model fault. Reported **only for a genuine decode fault**
(`is_decode_fault` = `Internal | BadConfig` — the H1 NaN-reject / model `step` / `decode_audio_stream`
failure), never for an infra terminal (`AdmissionRejected`, `SlowConsumer`, `StallTimeout`, `Draining`,
`Backpressure`, `NotImplemented`), so a healthy input shed under load is never falsely quarantined. The 1st
such crash is absorbed (replay budget = 1); the 2nd identical one dead-letters the input so call-site 1
refuses it.

### 3. Per-slot clear on recycle — `codec_ar_batcher.rs:358` (same `End` branch, marker 348)
```rust
if !matches!(&wire, TerminalFrame::Error(e) if is_decode_fault(e.code)) {
    quarantine.clear_channel(channel);
}
```
The terminal `End` event is the moment the runtime loop recycles the slot (F3: it resets + frees the slot
right after the sink lands the terminal). On a **clean** finish (`Final`/`Cancelled`, or an infra `Error`),
`clear_channel(channel)` resets that channel's quarantine so a recycled id starts fresh. **Deliberately
skipped for a just-dead-lettered decode-fault `Error`** so the poison-pill's quarantine *survives* to refuse
the replay — the poller's TTL eviction (`Quarantine::evict_expired`, already driven by `spawn_watchdog`) is
the time-based backstop that bounds the maps regardless. (`AppState::clear_channel_quarantine` →
`Quarantine::clear_channel` is the same hook; the `RecycleGate::clear_channel` H3-bound leg is already
self-driving via its hard cap per B14, so no separate recycle hook is needed for the bound.)

> The `AppState` hooks B14 exposed (`report_decode_crash` / `admit_input` / `clear_channel_quarantine`,
> `lib.rs:277/298/…`) remain as the public delegating surface; the live serve path reaches the **same shared
> `Quarantine`** directly via the batcher's clone (one fewer `&AppState` hop on the hot path, identical
> ledgers).

---

## How the signature (and channel) are derived at ingress — `ingress_fingerprint`, `codec_ar_batcher.rs:183`

```rust
fn ingress_fingerprint(utterance: &str) -> (ChannelId, InputSignature) {
    let digest = fnv1a64(utterance.as_bytes());                       // §13.6 content fingerprint
    (ChannelId::from_monotonic(digest), InputSignature::for_test(digest))
}
```

- **`signature`** = an **FNV-1a 64-bit digest of the full request content** (the utterance bytes already in
  hand at this seam) — the §13.6 ingress poison-pill fingerprint. The firewall treats it as opaque; the only
  contract it needs is **two replays of the same crash-relevant input collide, two distinct inputs do not** —
  which a content hash gives exactly.
- **`channel`** = the source bucket the firewall + restart-rate-limiter key on. At the M1 codec-AR seam **no
  per-tenant identity is threaded down to the batcher**, so the request's own content digest is the stable
  source key: a replay of the same poison content recurs on the same `(channel, signature)` pair (the firewall
  counts it → trips on the 2nd crash). This is the keying B14's `Quarantine` unit test itself uses (a fixed
  `chan` across the two replays). When a real tenant/source identity is later threaded through `submit`,
  `channel` should switch to it (so the per-source rate-limiter also sees one source minting *distinct*
  pills); the firewall leg — the one the required test exercises — is unchanged either way.
- **FNV-1a, not `DefaultHasher`** (`fnv1a64`, `codec_ar_batcher.rs:486`): a `DefaultHasher` is randomized
  per process, so a replayed pill would not collide after a restart; FNV-1a is stable across restarts +
  machines and has no hash-DoS surface on the tenant-controlled id. Pure, cheap, **control-plane** — it never
  touches an admitted stream's numerics.

---

## The real-path crash-loop test (proves it FIRES through the actual admission path)

`crash_loop_input_is_quarantined_through_real_admission_and_refused_on_replay`
(`codec_ar_batcher.rs:1397`, `#[tokio::test(multi_thread)]`). It drives the **REAL `CodecArBatcher::submit`
path** — not the `Quarantine` unit — with `PoisonDecodeTts`, a codec-AR model double whose
`decode_audio` returns `Err(Internal)` for any input containing `POISON` (the way a malformed input kills
`decode_audio_stream`), and is otherwise bit-identical to `CohortRecordingTts`:

1. **1st `submit(poison)`** → admitted → the shared lockstep loop crashes its decode → lands
   `Terminal::Error(Internal)` (asserted; **no committed audio**, never a hang). Call-site 2 reports it; not
   yet quarantined (replay budget).
2. **2nd `submit(poison)`** → still admits once → crashes again → the **2nd identical crash dead-letters it**
   (`quarantine.is_quarantined(chan, sig)` now true — the report call-site fired twice on the real path).
3. **3rd `submit(poison)`** → **refused at call-site 1** with a typed `BadConfig`, `retriable == false` —
   never re-entering the loop to re-crash it. *This is the firewall firing on REAL admission.*
4. **A benign `submit("hello world")`** → still admits, finishes `Final`, and its PCM is **byte-equal to its
   single-stream solo reference** (`poison_solo_pcm`) — proving the wiring is per-`(channel,signature)`,
   control-plane only, and leaves an admitted stream token-for-token unperturbed.

Two supporting tests:
- `recycle_clears_channel_and_infra_terminal_is_not_a_decode_crash` (`:1463`) — a clean `Final` turn fires
  call-site 3 (`clear_channel`) and leaves **nothing** quarantined (the per-slot clear is a clean reset, no
  leak); and asserts `is_decode_fault` rejects every infra code (no false quarantine of a healthy input shed
  under load).
- `ingress_fingerprint_is_stable_and_discriminates_distinct_inputs` (`:1505`) — same content ⇒ same
  `(channel, signature)` (replays collide → the 2nd crash trips); distinct content ⇒ distinct fingerprint (no
  false-collision quarantine); FNV-1a is deterministic.

---

## Bit-faithful + hot-path-safe (the LAW)

Admission/quarantine is **control-plane only — who is admitted/shed, never numerics**:
- **Call-site 1** runs **before any dispatch**; it either returns early (shed) or falls straight through with
  zero effect on a non-quarantined stream.
- **Call-sites 2 + 3** run in the sink's `End` branch as **pure side-effects on the quarantine ledger**,
  *after* `wire = t.to_wire()` is computed exactly as before. The returned `StreamItem::Terminal(wire)` is
  byte-identical to the pre-change value; the audio `Delta` branches are **completely untouched** — every PCM
  sample flows through identically.
- The existing NaN-reject (H1) / frame-progress watchdog (J16) / bounded-stride / GATE #9 / F2-non-blocking
  guards are **unchanged** — quarantine sits *beside* them, it does not replace or alter any of them.

Evidence: the bit-identity / concurrency gates (`live_concurrent_…bit_identical`, the GATE #9 stress test,
and the chaos `…does_not_poison_the_others` / `wedged_consumer…f2` / `>4 concurrent` tests) all pass
unchanged. New metrics: `waav_infer_decode_crash_reported_total`, `waav_infer_quarantine_refused_total{code}`
(alongside B14's `waav_infer_quarantine_evicted_total`).

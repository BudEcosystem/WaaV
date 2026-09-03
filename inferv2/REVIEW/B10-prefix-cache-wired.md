# B10 — RadixPrefixCache wired into the LIVE codec-AR path (Track 1a)

**Scope:** wire the shelf-ware `RadixPrefixCache` (Track 1a) into the chatterbox codec-AR prefill path so a
returning/cloned voice reuses its already-computed conditioning-prefill KV instead of recomputing it — **bit-faithful
(byte-identical to a no-cache prefill) and tenant-isolated.**

**Files touched (all absolute):**
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-core/src/tts/chatterbox.rs` — the wiring + KV⇄PrefillState
  serialization + the radix key + the new tests (deterministic + live).
- `/home/bud/ditto/waav/waav-infer/ci/heavy_live_tests.sh` — registered the new live gate (process-isolated).
- `RadixPrefixCache` itself (`crates/waav-infer-runtime/src/prefix_cache.rs`) — **untouched** (used via its public
  API, kept backend-free in `-runtime`). The concurrently-built candle backend crate — **untouched.**

---

## Headline answer

**Is prefix-cache LIVE on the codec-AR path?** — **Wired end-to-end and proven bit-faithful on live CUDA, but NOT
armed by default**, because on the chatterbox ONNX export the reuse is **measured TTFA-NEGATIVE** (a hit is *slower*
than a cold prefill). The wiring is complete, the bit-identity law holds (proven twice on real GB10 weights), tenant
isolation holds — but the ~7× TTFA *win* does not materialize on this graph and needs a deferred device-resident-KV
re-export. This is the exact seam gap the task asked to report honestly rather than ship a brittle/regressing default.

| dimension | result |
|---|---|
| Wired at the prefill boundary | YES — `LmDecoder::prefill_slot_cached`, reached via `ChatterboxTts::prefill_text` / `prefill_text_for(tenant)` and `ChatterboxArStep::prefill` / `prefill_for(tenant)` |
| Bit-faithful (byte-identical to no-cache) | **YES — proven on live CUDA: 234 body codes IDENTICAL, 3/3 runs** (frame-replay and multi-token-suffix variants) + deterministic gates |
| Tenant-isolated (no cross-user side channel) | YES — `prefix_cache_tenant_isolation` (deterministic, KV-value-sensitive fake catches a leak as diverged codes) |
| Measured TTFA on a prefix-hit | **warm SLOWER than cold (0.24–0.77×)** on GB10 — net-negative on the host-KV export; reported, not asserted |
| Armed in production | **NO** (would regress TTFA); available behind `enable_prefix_cache()` for the future device-KV path |
| Default lib suite | green: **65 passed, 0 failed, 8 ignored** (baseline was 61/0/7; +4 deterministic, +1 live ignored) |
| Live gate (process-isolated) | green: **1 passed** — bit-identity asserted, TTFA reported |
| Clippy | clean |

---

## What was wired (the seam)

The cache is one `RadixPrefixCache` per `LmDecoder` (one per loaded model), shared across slots, behind the existing
seam — `-core` calls the `-runtime` cache, never the backend. Field: `LmDecoder.prefix_cache: Option<RadixPrefixCache>`
(armed by `enable_prefix_cache()`); `voice_salt: i32` (a content fingerprint of the voice conditioning).

**The radix key** = `[cond_marker × cond_len] ++ text_ids` (`LmDecoder::prefix_key`). One key token ⇔ one KV frame, so
a longest-prefix match slices the KV frame-aligned. The cond prefix is `cond_len` rows of cached `speech_encoder`
`audio_features` (no natural token ids), so each cond frame gets a synthetic marker `COND_MARKER_BASE + salt*4096 + f`
salted by an FNV-1a fingerprint of the voice's `speaker_embeddings` + cond shape — so the **same voice shares its whole
cond prefix** (+ any shared text head) while a **different voice keys disjointly** (diverges at frame 0). Markers live
at `1<<24`, far above the real text vocab (no collision with a genuine text id).

**Cache hidden_dim** = the per-frame split-KV width across all layers: `n_layers × 2 × kv_heads × head_dim` f32s.

**Serialize/deserialize** (`prefill_state_from_kv` / `kv_from_prefill_state`): the chatterbox KV is layer-major
`past_key_values.{i}.{key,value}`, each `[1, kv_heads, seq, head_dim]` — so a frame is *strided* across heads (seq is
the 3rd axis). The helpers gather each frame's slice into a contiguous row-major `PrefillState` row and scatter back to
the `[head, seq, head_dim]` layout the LM graph emitted — **byte-exact** (proven by `prefix_cache_kv_roundtrip_is_byte_exact`
on distinct-valued strided KV). The re-feed dtype is graph-driven via the existing `feed_float` (f32 today, widened to
f16 only under a future f16 export — the B7 law, one definition).

**`prefill_slot_cached`** (the boundary):
1. build the key + `match_for(tenant, key)` → longest cached shared prefix (the EXACT prior-prefill bytes).
2. `reuse = min(matched_tokens, prefill_len - 1)` (always replay ≥1 frame so the forward produces the first-token logits).
3. **cold (`reuse==0`):** the efficient *single multi-token* `prefill_slot` (byte-for-byte the existing path — **no
   regression on the first request**), then serialize its full prefix KV and `insert_for`.
4. **warm (`reuse>0`):** deserialize the reused prefix KV → `past`, prefill the unmatched suffix in **one multi-token
   forward** with that past (GQA chunked-prefill: `seqlens_k = ReduceSum(mask)-1` over the LEFT-justified mask positions
   the suffix tokens at `reuse..prefill_len`), then re-cache the now-complete full prefix.
5. cache insert failure is logged and ignored — a pure perf accelerator never fails a request.

The slot's staged AR state (`generated`, `next_pos`, `attn_len`, `embed_pos_next`, grown KV) is **identical geometry to
`prefill_slot`**, so the reuse is invisible to the decode loop downstream.

---

## The bit-faithful proof (the non-negotiable law)

The bit-faithfulness rests on three independently-proven properties:

1. **The KV bytes are stored/returned verbatim** — `prefix_cache_kv_roundtrip_is_byte_exact` proves the strided
   `[head, seq, head_dim]` gather/scatter is byte-exact for any `matched ≤ seq`. The cache itself
   (`RadixPrefixCache`) guarantees it returns the exact bytes a prior full prefill stored (its own module law).
2. **The replay GEOMETRY equals a full prefill** — `prefix_cache_reuse_is_bit_identical_to_full_prefill` (deterministic,
   plain geometry-driven fake): prefill V+"AB", then V+"AC" sharing `[cond(V),A]` on an armed model → the cached "AC"
   codes are byte-identical to a no-cache "AC" run, and the suffix diverges correctly (V+AB ≠ V+AC first codes).
3. **The causal-attention equivalence holds on REAL weights** — `live_prefix_cache_reuse_bit_identical_to_full_prefill`
   (GB10, CUDA, real ResembleAI weights): a warm (reused-shared-head) prefill's emitted codes are **byte-identical
   token-for-token to a cold full prefill — 234 codes IDENTICAL** (a long shared head + a multi-token divergent tail,
   so deep AR compounding bites). This certifies the property the fake can't model: that {reuse cached prefix KV + a
   multi-token-suffix-with-past forward} == {a single multi-token full prefill}. Run TWICE (frame-replay variant and
   multi-token-suffix variant) — **both PASS bit-identity**, so the GQA chunked-prefill-with-non-empty-past is correct.

The existing bit-identity gates stay green (`codec_ar_emitted_codes_identical_to_edge_path`,
`batched_forward_codes_identical_to_per_slot`, `ragged_batched_forward_codes_identical_to_per_slot`,
`codec_ar_run_ar_compounding_identical`, the D2H `zero_d2h_sync_during_decode` family, the streaming/barge-in gates),
and the live CUDA bit-identity gates (`live_ragged_batched_forward_bit_identical_and_scales` etc.) are unchanged.

---

## The tenant-isolation proof (the side-channel boundary)

`prefix_cache_tenant_isolation` (deterministic, KV-value-sensitive fake): tenant A warms `(V, ids)`; tenant B
prefilling the IDENTICAL `(V, ids)`:
- the cache lookup for B **misses A's entry** (`match_for(B, key).matched_tokens() == 0`) — the strongest check, at the
  cache level;
- B's actual run produces codes byte-identical to an isolated no-cache run (isolation ≠ corruption);
- after B's run, A and B each hold their OWN cached full prefix in disjoint namespaces.

The KV-value-sensitive fake LM folds a checksum of the fed past-KV into its codes, so a *broken* isolation (B fed A's KV
bytes) would surface as DIVERGED codes, not just a wrong count — the contamination is detectable as wrong bytes.

---

## The measured TTFA finding (the honest seam gap)

`live_prefix_cache_reuse_bit_identical_to_full_prefill` measures the prefill (first-audio) wall time, warm (cache-hit)
vs cold (full prefill), on a long shared head:

| variant of the warm path | bit-identity | TTFA (warm/cold) |
|---|---|---|
| frame-by-frame suffix replay (rejected) | PASS (234 codes) | **0.24×** (warm 869.89 ms vs cold 210.63 ms) |
| single multi-token suffix forward (shipped) | PASS (234 codes) | **0.77×** (warm 231.21 ms vs cold 179.11 ms) |
| single multi-token suffix forward (re-run) | PASS (234 codes) | **0.35×** (warm 243.51 ms vs cold 84.16 ms) |

(The absolute cold ms is JIT/warmup-noisy run-to-run — 84–210 ms; the **warm-slower direction is stable and
reproducible across all 3 runs**, and bit-identity is 234/234 codes every run.)

**The lever does not materialize on the chatterbox ONNX export — and this is architectural, not a wiring bug.** The
`language_model.onnx` threads the KV through HOST every forward (host `past_key_values.*` inputs / `present.*` outputs;
there is no device-handle KV I/O). So a prefix-cache hit must FEED the reused prefix KV to host (an `O(prefix_len)`
transfer) and reconstruct it from the cache on the suffix forward — and that host-KV transfer + reconstruction cost
**exceeds the prefix attention compute it saves**. The warm path saves the prefix COMPUTE but still pays the prefix
host-KV transfer (in + out). The frame-by-frame variant was 0.24× because it re-streamed the whole reused KV *per suffix
frame*; the multi-token-suffix variant streams it once → 0.77×, still net-negative.

This is the *same* host-KV-feedback property the codebase already measured for `run_bound` on this exact graph
(`perf_lever_run_bound_vs_run_on_real_chatterbox_lm_cuda` = 0.77× — strikingly the identical ratio), and the same reason
the batching ceiling is ~1.8×@16 not 55×@64. **The ~7× TTFA lever is a property of a DEVICE-RESIDENT-KV export** (the
reused prefix KV stays on-device across the boundary and is not re-streamed) — the same deferred graph re-export the
batching ceiling needs (device-handle KV I/O / retiring the host re-stream).

**Decision:** ship the complete, bit-faithful, tenant-isolated wiring, but **do NOT arm it by default** on this export
(arming would regress production TTFA). It is available behind `enable_prefix_cache()` for tests/benches and for a
future device-KV path. The live gate asserts the **bit-identity law** (which MUST hold) and **reports** (does not assert)
the TTFA direction (a known architectural property of the export). `prefill_text` is byte-for-byte the plain
`prefill_slot` until armed — zero production behaviour change.

---

## What's wired vs the seam gap

**Wired + verified (LIVE-capable, bit-faithful, tenant-isolated):**
- `RadixPrefixCache` owned per-model, shared across slots, in `-runtime` behind the seam.
- The radix key (cond-prefix markers salted per voice + text ids), the byte-exact KV⇄PrefillState serialization, the
  reuse + suffix-prefill at the prefill boundary, the LRU bound, cache-insert-never-fails-the-request.
- Bit-identity proven on live CUDA (234 codes, twice) + deterministic geometry/round-trip/isolation gates.
- Tenant isolation at the cache level + as diverged codes.
- The seam to thread a per-user `TenantId` to the cache (`prefill_text_for` / `prefill_for`).

**The seam gap (honest):**
1. **TTFA win missing on the host-KV export** — the #1 finding. Bit-faithful but net-negative (0.77×); needs a
   device-resident-KV `language_model` re-export to realize the ~7× lever. Until then the cache is unarmed by default.
2. **No `TenantId` in the `ArStepModel::prefill(slot, text)` trait** — the lockstep serve path (`serve.rs` calls
   `model.prefill(slot, text)`) carries no tenant, so production reuse runs under the reserved single-tenant
   `TenantId::DEFAULT` domain. Multi-tenant isolation is available via the explicit `prefill_text_for(slot, tenant,
   text)` / `prefill_for(...)` entry points, but threading the request's tenant through the trait + scheduler is a
   follow-up (changing the trait signature has a wider blast radius; deferred deliberately).

---

## Reproduce

```bash
source /home/bud/ditto/waav/waav-infer/gb10-env.sh
# deterministic (default pass): bit-identity geometry + byte-exact KV round-trip + tenant isolation + work-saving
timeout -k 30 600 cargo test -p waav-infer-core --lib            # 65 passed, 0 failed, 8 ignored
cargo clippy -p waav-infer-core --lib --tests                    # clean
# the live CUDA bit-identity + TTFA measurement (process-isolated; needs the cached model):
export CHATTERBOX_DIR="$HOME/.cache/waav-models/chatterbox-onnx"
cargo test -p waav-infer-core --lib \
  tts::chatterbox::tests::live_prefix_cache_reuse_bit_identical_to_full_prefill \
  -- --ignored --exact --nocapture --test-threads=1
#   bit-identity (warm reuse == cold full prefill): PASS — 234 body codes identical
#   TTFA prefill wall: COLD ~179 ms vs WARM(cache-hit) ~231 ms → ~0.77x (host-KV export; ≤1× expected)
```

**Not committed** (per instruction).

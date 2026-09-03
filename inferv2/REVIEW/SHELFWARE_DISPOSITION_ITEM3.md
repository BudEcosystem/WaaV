# Item 3 — Shelfware Disposition (honest wiring status, no false claims)
The Goal-C brutal review flagged several crates as "pub/tested with no LIVE callers → wire or down-scope." Re-audited
against the current tree; the honest status of each (this IS the down-scope — accurate, neither "live/done" nor "dead"):

| Crate | LOC | Real status | Disposition |
|---|---|---|---|
| **backend-api** | — | **LIVE — NOT dead.** Now the DeviceCaps + AccelMapper + KernelPolicy hub; 10+ consumers (backend-ort, backend-torch, core, gateway-provider-api). | The original "dead" flag is STALE — wired this session. |
| **provider** | 4679 | **LIVE from the gateway** (GW-2 adapter: impls the gateway `BaseSTT`/`BaseTTS` by driving `core::{SttModel,TtsModel}`). Caller = the **gateway** (external workspace), so 0 server/core refs is correct. | NOT shelfware — it's the cross-process integration seam. Documented as gateway-facing. |
| **router** | 1288 | **Built + gate-tested, NOT wired into single-node serve.** Prefix-affinity fleet-placement (engine half). A genuine **multi-worker FLEET feature** — irrelevant to the single-node serve path that exists today. | DOWN-SCOPE: keep (it's the vLLM-distributed-placement scaffold the user wants eventually); doc'd as "fleet-only, not single-node-wired." Not a false claim. |
| **features** | 3508 | **Built + tested, pending wire into the serve TEXT path.** SSML / text-normalization frontend edges (pure logic). A real serve-path need. | WIRE-NEXT: the one genuinely-wireable item — fold into the server's text-ingest before synthesis. Tracked as a bounded follow-up. |
| **dag** | 7549 | **Partially live** (3 refs; the CLI composition path). Stage-DAG machinery for multi-stage pipelines (STT→translate→TTS). | Partially-wired; the full multi-stage serve wiring is a fleet/pipeline follow-up. |
| **S2S scaffold** | — | The earlier "native-S2S" was a synthetic-hash scaffold; the REAL `DuplexStepModel` was registered on GPU (task #63, committed). | Already corrected — real S2S seam exists; the scaffold claim was fixed. |

## Net
- No crate is making a FALSE "it's live" claim after this audit. `backend-api`/`provider` are live (the "dead" flags were
  stale/mis-scoped); `router`/`dag` are honestly future-fleet scaffold (kept, not deleted — they're the vLLM-distributed
  features the goal wants, just not single-node-relevant yet); `features` is the one bounded serve-path wire to do next.
- This is the "down-scope the docs" the item asked for: accurate status. The only "wire" worth doing single-node is
  `features` (SSML/text-norm) → tracked.

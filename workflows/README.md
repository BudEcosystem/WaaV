# WaaV implementation workflows

Multi-agent workflow scripts that drive WaaV to production-readiness. They are the
executable counterpart of `PRODUCTION_PLAN.md` and are grounded in `BRUTAL_REVIEW.md`.

Run from a Claude Code session with:

```
Workflow({ scriptPath: "/home/bud/ditto/waav/WaaV/workflows/<file>.js" })
```

| Order | Workflow | What it does | Prerequisite |
|---|---|---|---|
| 01 | `01-standardized-config.js` | Replaces the flat STT/TTS config with the standardized capability-rich config (the **S1 keystone**) and migrates all ~60 providers to it, TDD-first, worktree-isolated per batch. | `cargo check` green; design accepted in `PRODUCTION_PLAN.md` |
| 02 | `02-fix-broken-providers.js` | TDD-fixes the broken/crippled integrations (Azure framing, Cartesia version, Tencent signature, Tinkoff auth, TTS codec mismatch, Deepgram keyterm, Hume wiring, universal reconnection), each verified against current provider docs, then adversarially re-verified. | 01 landed |
| 03 | `03-validate-loop.js` | Loops build → clippy → unit → mock-integration → server-startup → local neural accuracy until the **no-credentials gate** is green; emits secret-gated CI for real-provider e2e. | any time |

## Execution order & loop

```
01 standardized-config  ->  02 fix-broken-providers  ->  03 validate-loop  (repeat 03 until green)
```

## What CAN and CANNOT be validated in a keyless environment

- **CAN (no keys):** compile (all feature flags), clippy, unit + integration tests against the
  in-repo mock providers (`tests/mock_providers`), server boot + health, DAG with mock providers,
  and **local neural accuracy** — Silero VAD / Smart-Turn / Turn-Detect run ONNX models locally.
- **CANNOT (needs secrets, runs only in CI):** real third-party transcription/synthesis against
  Deepgram/OpenAI/ElevenLabs/Azure/etc., and LiveKit/SIP against a live server. These are wired as
  `#[ignore]` tests (`tests/real_provider_tests.rs`, `tests/test_all_providers.rs`) and a
  secret-gated CI job. **Do not claim these are validated locally — they are not.**

## Conventions enforced by every workflow

- Strict TDD: the failing test is written first and must fail on the old behavior.
- No stubs/mocks/hardcoding in production paths; no `#[ignore]` to hide a real failure.
- Agents run real `cargo` commands and report actual output — never fabricated pass/fail.
- Adversarial verification before a fix is considered done.

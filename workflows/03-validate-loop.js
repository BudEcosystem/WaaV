/**
 * WaaV implementation workflow 03 — VALIDATION LOOP.
 *
 * Drives the system to green and validates everything that can be validated WITHOUT
 * paid-provider credentials: build (all feature flags), clippy, unit + integration +
 * contract tests (mock harness), server startup, DAG-with-mocks e2e, and local neural
 * accuracy (Silero VAD / Smart-Turn / Turn-Detect run ONNX locally — no API key).
 *
 * Real third-party-provider e2e is emitted as a CI job gated on secrets (it cannot run
 * here: no DEEPGRAM/OPENAI/... keys in this environment).
 *
 * Loops until the no-credentials gate is fully green or budget is exhausted.
 *
 * Invoke: Workflow({ scriptPath: ".../workflows/03-validate-loop.js" })
 */
export const meta = {
  name: 'waav-validate-loop',
  description: 'Build/clippy/test/accuracy validation loop for everything runnable without paid-provider keys; emit secret-gated CI for the rest',
  phases: [
    { title: 'Gate', detail: 'build + clippy + unit/integration/contract + startup + local accuracy' },
    { title: 'Triage', detail: 'fix failures, loop' },
  ],
}

const GW = '/home/bud/ditto/waav/WaaV/gateway'
const COMMON = `Repo: ${GW}. Run real commands and report ACTUAL output (never fabricate pass/fail).`

const GATE_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['build', 'clippy', 'unit', 'integration_mock', 'server_startup', 'local_accuracy', 'failures', 'green'],
  properties: {
    build: { type: 'string', description: 'cargo check result for default AND --features turn-ensemble,dag-routing,noise-filter' },
    clippy: { type: 'string' },
    unit: { type: 'string', description: 'cargo test --lib result' },
    integration_mock: { type: 'string', description: 'cargo test against tests/mock_providers + e2e_mock_tests result' },
    server_startup: { type: 'string', description: 'does the binary boot + answer GET / health, with a minimal config' },
    local_accuracy: { type: 'string', description: 'Silero VAD / Smart-Turn / Turn-Detect: do models download+load and produce sane outputs on a known clip; report metrics if datasets present' },
    failures: { type: 'array', items: { type: 'string' }, description: 'each failure with file:line + root cause + fix hint' },
    green: { type: 'boolean', description: 'true ONLY if build+clippy+unit+integration_mock+server_startup all pass' },
  },
}

const MAX_ROUNDS = budget.total ? 6 : 3
let round = 0
let lastGate = null
while (round < MAX_ROUNDS) {
  round++
  phase('Gate')
  lastGate = await agent(
    `${COMMON}\n\nRound ${round}. Run the no-credentials validation gate and report each leg with real command output:\n` +
    `1) cargo check (default) and cargo check --features turn-ensemble,dag-routing,noise-filter\n` +
    `2) cargo clippy --all-targets -- -D warnings (summarize)\n` +
    `3) cargo test --lib (unit)\n` +
    `4) cargo test --test e2e_mock_tests --test load_test_with_mocks (mock integration)\n` +
    `5) boot the release/debug binary with a minimal config and curl GET / for {"status":"ok"}\n` +
    `6) build with --features turn-ensemble; exercise Silero VAD + Smart-Turn + Turn-Detect on tests/live_testing/audio (or a generated clip): confirm models download, sessions load, outputs are in-range; report any accuracy metrics available.\n` +
    `Set green=true only if legs 1-5 pass.`,
    { label: `gate:r${round}`, phase: 'Gate', schema: GATE_SCHEMA })

  if (lastGate.green) { log(`Gate GREEN on round ${round}`); break }
  if (!lastGate.failures || !lastGate.failures.length) { log('No actionable failures parsed; stopping'); break }

  phase('Triage')
  await parallel(lastGate.failures.slice(0, 8).map((f, i) => () =>
    agent(`${COMMON}\n\nFix this validation failure (strict TDD; no masking, no #[ignore] to hide it):\n${f}\n\n` +
      `Implement the real fix, then re-run the specific failing command to confirm. Report file:line + confirmation output.`,
      { label: `triage:r${round}:${i}`, phase: 'Triage', isolation: 'worktree' })))
}

return { rounds: round, finalGate: lastGate, green: lastGate ? lastGate.green : false }

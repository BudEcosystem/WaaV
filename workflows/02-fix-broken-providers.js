/**
 * WaaV implementation workflow 02 — FIX BROKEN / CRIPPLED INTEGRATIONS.
 *
 * Each defect is fixed as an independent TDD pipeline: write a failing test that
 * encodes the CORRECT provider protocol (verified against current docs), then fix
 * the code until it passes, then a skeptic verifies the fix is real (not test-rigged).
 *
 * Run AFTER workflow 01 (standardized config) lands, since several fixes also need
 * the richer config to expose the corrected behavior.
 *
 * Invoke: Workflow({ scriptPath: ".../workflows/02-fix-broken-providers.js" })
 */
export const meta = {
  name: 'waav-fix-broken-providers',
  description: 'TDD-fix the broken/crippled provider integrations (Azure, Cartesia, Tencent, Tinkoff, TTS codec mismatch, Deepgram keyterm, Hume, reconnection)',
  phases: [
    { title: 'Fix', detail: 'per-defect: failing test -> fix -> cargo check' },
    { title: 'Adversarial-verify', detail: 'skeptic confirms each fix is real' },
  ],
}

const GW = '/home/bud/ditto/waav/WaaV/gateway'
const REVIEW = '/home/bud/ditto/waav/WaaV/BRUTAL_REVIEW.md'
const COMMON = `Repo: ${GW}. Findings: ${REVIEW}. Strict TDD; verify the corrected protocol against CURRENT provider docs ` +
  `(web search allowed) before writing the test; no hardcoding; run \`cargo check\` before done; cite file:line.`

// Each defect: a precise, independently-verifiable fix spec.
const DEFECTS = [
  { id: 'azure-stt-framing', spec: 'Azure STT WS sends bare binary + never sends speech.config. Implement the USP framing: a speech.config text message after handshake, and per-audio binary frames prefixed with a 2-byte big-endian header length + CRLF-delimited headers (Path:audio, X-RequestId, X-Timestamp, Content-Type). Test: assert the first text frame is speech.config and an audio frame begins with the header block.' },
  { id: 'cartesia-stt-version', spec: 'Cartesia STT from_base never sets cartesia_version, so the required version query param is omitted and the WS is rejected. Thread cartesia_version (default to the current API version) through from_base. Test: assert the connect URL/headers carry the version.' },
  { id: 'tencent-stt-signature', spec: 'Tencent ASR HMAC string-to-sign omits the host+path prefix. Prepend "asr.cloud.tencent.com/asr/v2/<appid>?" before signing. Test: known-input signature vector matches the documented expected signature.' },
  { id: 'tinkoff-auth', spec: 'Tinkoff uses raw x-api-key/x-secret-key + hardcoded JWT test claims + non-base64-decoded secret. Build a real HS256 JWT (aud per service, real iss/sub from config, exp) signed with the base64-DECODED secret, sent as authorization: Bearer. Test: decode the produced JWT and assert claims + that the signing key was base64-decoded.' },
  { id: 'tts-codec-mismatch', spec: 'wellsaid/speechify/unrealspeech emit MP3/WAV but base_config.audio_format stays linear16, so the generic chunker mislabels/mischunks. Reconcile audio_format with each provider real output codec before the chunker; strip WAV headers. Test: feed a known MP3/WAV body and assert emitted AudioData.format matches the real codec and PCM path is not used for compressed audio.' },
  { id: 'deepgram-keyterm', spec: 'Deepgram default model nova-3 needs keyterm= not keywords=. Send keyterm for nova-3 (keywords for nova-2/earlier). Test: with model=nova-3 assert the query uses keyterm; with nova-2 assert keywords.' },
  { id: 'hume-wiring', spec: 'Hume description + emotion_config never reach the factory path. Wire the standardized config (instructions/emotion) into the EVI/Octave request. Test: assert the description/acting-instructions field is present in the outbound request when set.' },
  { id: 'reconnection', spec: 'No streaming STT/TTS provider nor Hume EVI reconnects. Add a shared reconnect-with-exponential-backoff helper (cap, jitter, max attempts) and use it in the event loops; re-send config/start frames on reconnect. Test: a mock server that drops once triggers exactly one reconnect and resumes.' },
]

phase('Fix')
const fixed = await pipeline(
  DEFECTS,
  (d) => agent(
    `${COMMON}\n\nDEFECT ${d.id}:\n${d.spec}\n\nWrite the failing test first, verify the correct protocol against current docs, ` +
    `then implement the fix, then \`cargo check\`. Return: test name + file:line, the fix diff summary, and the doc source you verified against.`,
    { label: `fix:${d.id}`, phase: 'Fix', isolation: 'worktree' }),
)

phase('Adversarial-verify')
const verdicts = await parallel(fixed.filter(Boolean).map((f, i) => () =>
  agent(
    `${COMMON}\n\nAdversarially verify this claimed fix is REAL and not rigged to pass a weak test. Re-read the code and the test, ` +
    `confirm the test would FAIL on the old behavior and the protocol matches current docs. Defect/fix report:\n${typeof f === 'string' ? f : JSON.stringify(f)}\n\n` +
    `Return a verdict: { real: bool, reason, residual_gaps }.`,
    { label: `verify:${DEFECTS[i] ? DEFECTS[i].id : i}`, phase: 'Adversarial-verify',
      schema: { type: 'object', additionalProperties: false, required: ['real', 'reason', 'residual_gaps'],
        properties: { real: { type: 'boolean' }, reason: { type: 'string' }, residual_gaps: { type: 'string' } } } })))

return { fixed, verdicts }

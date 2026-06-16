// Live P0 validation against a real gateway, using the BUILT SDK (dist).
// Usage: WAAV_GATEWAY_URL=ws://127.0.0.1:3081 node tests/live/live-p0.mjs
//
// Asserts:
//  (1) the SDK's own deserializer surfaces a NON-EMPTY transcript from the
//      exact gateway stt_result wire,
//  (2) a real /ws ready handshake yields a non-empty streamId + protocol_version '1.0'
//      (via the SDK's WebSocketSession + deserializer, not raw msg access),
//  (3) clear() and audio_end are ACCEPTED by the gateway (no parse-error / no
//      socket close after sending them).
import { WebSocket } from 'ws';
import {
  WebSocketSession,
  deserializeMessage,
  serializeMessage,
  createClearMessage,
  createAudioEndMessage,
} from '../../dist/index.mjs';

const URL = process.env.WAAV_GATEWAY_URL ?? 'ws://127.0.0.1:3081';
const WS_URL = URL.endsWith('/ws') ? URL : `${URL.replace(/\/$/, '')}/ws`;

let failures = 0;
function ok(name, cond, detail) {
  const status = cond ? 'PASS' : 'FAIL';
  if (!cond) failures++;
  console.log(`[${status}] ${name}${detail ? ' :: ' + detail : ''}`);
}

// (1) Deserializer round-trip through the SDK's OWN deserializer.
{
  const wire = JSON.stringify({
    type: 'stt_result',
    transcript: 'hello world',
    is_final: true,
    is_speech_final: true,
    confidence: 0.97,
  });
  const msg = deserializeMessage(wire);
  ok('deserializer surfaces non-empty transcript', msg.transcript === 'hello world', `got=${JSON.stringify(msg.transcript)}`);
  ok('serializer clear() => {type:clear}', serializeMessage(createClearMessage()) === '{"type":"clear"}');
  ok('serializer audio_end => {type:audio_end}', serializeMessage(createAudioEndMessage()) === '{"type":"audio_end"}');
}

// (2)+(3) Live handshake + control-op acceptance via the SDK session.
const session = new WebSocketSession({
  url: WS_URL,
  WebSocket,
  autoConfig: false,
  reconnect: false,
  connectionTimeout: 8000,
});

let readyEvent = null;
let sawError = null;
let closedEarly = null;
const errors = [];
session.on('ready', (e) => { readyEvent = e; });
session.on('error', (e) => { sawError = e; errors.push(e); });
session.on('close', (e) => { closedEarly = e; });

try {
  await session.connect();
  // The gateway emits `ready` only after a config message (handlers/ws/
  // config_handler.rs). audio:false means no provider key is needed.
  session.send({ type: 'config', audio: false });
  const ready = await session.waitForReady(8000);
  readyEvent = ready;
  ok('ws connect + ready', true);
} catch (err) {
  ok('ws connect + ready', false, String(err && err.message || err));
}

ok('ready streamId is non-empty', !!(readyEvent && readyEvent.streamId), `streamId=${readyEvent?.streamId}`);
ok('ready protocolVersion is 1.0', readyEvent?.protocolVersion === '1.0', `protocolVersion=${readyEvent?.protocolVersion}`);
ok('getSessionId() non-empty', !!session.getSessionId(), `sessionId=${session.getSessionId()}`);
ok('no protocol mismatch error on ready', !(sawError && sawError.code === 'PROTOCOL_VERSION_MISMATCH'), sawError ? sawError.code : 'none');

// Send the P0 control ops and confirm the socket stays open (a parse-error
// would either error or close it).
if (session.isConnected()) {
  // clear() (barge-in) is valid regardless of audio state — expect NO error.
  const errBeforeClear = errors.length;
  session.clear();
  await new Promise((r) => setTimeout(r, 600));
  ok('clear() accepted (no error, socket open)', session.isConnected() && errors.length === errBeforeClear,
    errors.slice(errBeforeClear).map((e) => e.message).join('; ') || 'clean');

  // audio_end() is a RECOGNIZED op (IncomingMessage::AudioEnd). On this
  // audio-disabled session the gateway answers with the typed "Audio
  // processing is disabled" message — which itself proves the op parsed (an
  // unknown op like the old `flush` would fail to deserialize instead).
  const errBeforeEnd = errors.length;
  session.audioEnd();
  await new Promise((r) => setTimeout(r, 800));
  const endErr = errors.slice(errBeforeEnd)[0];
  ok('audio_end() is a recognized op (parsed, not a parse-error)',
    session.isConnected() && (!endErr || /audio processing is disabled/i.test(endErr.message)),
    endErr ? endErr.message : 'accepted');
}

await session.disconnect();

console.log(`\nRESULT: ${failures === 0 ? 'ALL PASS' : failures + ' FAILURE(S)'}`);
process.exit(failures === 0 ? 0 : 1);

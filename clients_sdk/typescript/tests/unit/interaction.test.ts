/**
 * Interaction mode (protocol 1.1) on `/ws`: config, ready echo, typed events, client messages
 * and the task list.
 *
 * The event frames are the gateway's own wire examples (gateway/src/core/interaction/events.rs
 * tests), so a renamed field on either side fails here.
 */
import { describe, it, expect, vi } from 'vitest';
import { WebSocketSession } from '../../src/ws/session.js';
import { createConfigMessage, serializeMessage } from '../../src/ws/messages.js';
import { buildAgentSessionConfig, AgentSession } from '../../src/pipelines/agent.js';
import { RestClient } from '../../src/rest/client.js';
import { isInteractionEvent, INTERACTION_EVENT_TYPES } from '../../src/types/interaction.js';
import type { InteractionEvent, InteractionConfig } from '../../src/types/interaction.js';
import type { ReadyEvent } from '../../src/ws/events.js';

/** Browser-WebSocket-shaped mock: records what is sent, lets a test push frames in. */
class MockWebSocket {
  static OPEN = 1;
  OPEN = 1;
  readyState = 0;
  binaryType = 'arraybuffer';
  onopen: (() => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  sent: unknown[] = [];
  constructor(public url: string) {
    setTimeout(() => {
      this.readyState = 1;
      this.onopen?.();
    }, 0);
  }
  send(data: unknown): void {
    this.sent.push(data);
  }
  close(code = 1000, reason = ''): void {
    this.readyState = 3;
    this.onclose?.({ code, reason });
  }
  emit(obj: unknown): void {
    this.onmessage?.({ data: JSON.stringify(obj) });
  }
}

function makeSession(extra: { interaction?: boolean | InteractionConfig; autoConfig?: boolean } = {}) {
  let ws!: MockWebSocket;
  const Impl = class extends MockWebSocket {
    constructor(url: string) {
      super(url);
      ws = this;
    }
  };
  const session = new WebSocketSession({
    url: 'ws://127.0.0.1:3009/ws',
    WebSocket: Impl as unknown as typeof WebSocket,
    autoConfig: extra.autoConfig ?? false,
    reconnect: false,
    liveness: false,
    keepalive: false,
    interaction: extra.interaction,
  });
  const sentJson = () =>
    ws.sent.filter((d): d is string => typeof d === 'string').map((d) => JSON.parse(d) as Record<string, unknown>);
  return { session, getWs: () => ws, sentJson };
}

// One frame per event, as the gateway sends it.
const WIRE: Record<string, Record<string, unknown>> = {
  task_created: { type: 'task_created', task_id: 'task_5b1e9c0d', task_ref: 't1', revision: 1, state: 'working', cancelling: false, title: 'your table booking', kind: 'action', side_effects: true, speculative: true, origin: 'front', background: 'agent:support', t_ms: 4180 },
  task_progress: { type: 'task_progress', task_id: 'task_5b1e9c0d', task_ref: 't1', revision: 2, state: 'working', cancelling: false, seq: 4, text: null, fraction: null, spoken: false, effects: null, answer_stopped: { reason: 'revised', released_sentences: 1 }, t_ms: 8350 },
  task_question: { type: 'task_question', task_id: 'task_5b1e9c0d', task_ref: 't1', state: 'input_required', cancelling: false, request_id: 'q_task_5b1e9c0d_2', kind: 'choose', prompt: 'Would you like 7 pm or 8 pm?', options: ['7 pm', '8 pm'], schema: null, sensitive: false, irreversible: false, channels: ['voice', 'client'], expires_at_ms: 61000, spoken: true, heard: 'pending', t_ms: 31000 },
  task_answer_received: { type: 'task_answer_received', task_id: 'task_5b1e9c0d', task_ref: 't1', state: 'working', cancelling: false, request_id: 'q_task_5b1e9c0d_1', outcome: 'decline', reason: null, correction: 'make it Tuesday', by: 'voice', corroborated: false, value: null, value_redacted: false, t_ms: 12400 },
  task_result: { type: 'task_result', task_id: 'task_5b1e9c0d', task_ref: 't1', revision: 1, state: 'completed', status: 'completed', cancelling: false, summary: 'Booked: table for 4, Saturday 19:00, confirmation R-5512', effects: ["table booking at Rosa's"], spoken: 'full', streamed: true, superseded: false, t_ms: 13900 },
  task_output: { type: 'task_output', task_id: 'task_9f2c', task_ref: 't2', revision: 1, state: 'completed', cancelling: false, text: 'Booked AI 101 on Friday at 9:40.', data: { flight: 'AI 101' }, artifacts: [{ url: 'https://air.example.com/t/AI101' }], effects: 'none_committed', t_ms: 21400, parent_id: 'task_5b1e', agent: 'browser', trace_id: '4bf92f3577b34da6a3ce929d0e0e4736' },
  task_cancelled: { type: 'task_cancelled', task_id: 'task_77aa01fe', task_ref: 't2', revision: 1, state: 'cancelled', cancelling: false, by: 'client', reason: 'client_cancelled', effects: 'none_committed', t_ms: 15200 },
  floor_state: { type: 'floor_state', holder: 'none', lane: null, reason: 'stream_gap', t_ms: 13240 },
  audio_segment: { type: 'audio_segment', phase: 'start', utterance_id: 'u_41', lane: 'front', kind: 'ack', task_id: null, t_ms: 12100 },
  agent_audio_stopped: { type: 'agent_audio_stopped', utterance_ids: ['u_44', 'u_45'], reason: 'policy', delivered_ms: [600, 0], t_ms: 20400 },
  interaction_updated: { type: 'interaction_updated', applied: ['front.caller.verbosity'], refused: [{ path: 'regime', reason: 'not_mid_session' }] },
  assistant_transcript: { type: 'assistant_transcript', turn_index: 3, delta: "Booked: a table for 4 at Rosa's on Saturday at 7 pm.", source: 'background', kind: 'relay', verbatim: true, task_id: 'task_5b1e9c0d', utterance_id: 'u_42', sentence: { attempt: 0, seq: 0 }, heard: 'full' },
  session_failed: { type: 'session_failed', service: 'background', reason: 'no answer for 30 s after the first stall line', pending: [{ task_id: 'task_5b1e9c0d', task_ref: 't1', title: 'what time does the store open', state: 'working' }], follow_up: false, t_ms: 40210 },
  unverified_claim: { type: 'unverified_claim', kind: 'completion', rule: 'CompletionClaim', text: 'Your table is booked.', corrected: true, output_stopped: false, t_ms: 9000 },
};

const ECHO = {
  enabled: true,
  protocol_version: '1.1',
  regime: 'cascade',
  front: { persona: 'default' },
  tasks: { backend: 'llm', speculative_start: 'auto', tools: ['book_table'], reconnect_grace_s: 60, push: false },
  policy: { ack_due_ms: 300 },
  duplex: { barge_in: true },
  events: { floor: true, segments: true },
  hitl: { channels: ['voice', 'client'] },
  degraded: [],
};

describe('interaction config on the wire', () => {
  const wire = (interaction?: boolean | InteractionConfig) =>
    JSON.parse(serializeMessage(createConfigMessage(undefined, undefined, undefined, undefined, { interaction }))) as Record<string, unknown>;

  it('sends true, false and an object as given', () => {
    expect(wire(true).interaction).toBe(true);
    expect(wire(false).interaction).toBe(false);
    expect(wire({ policy: { ack_due_ms: 250 } }).interaction).toEqual({ policy: { ack_due_ms: 250 } });
    expect('interaction' in wire(undefined)).toBe(false);
  });

  it('keeps snake_case keys and a tool schema as they are', () => {
    const block: InteractionConfig = {
      tasks: {
        tools: [{
          name: 'book_table', url: 'https://t.example/book', side_effects: true,
          parameters: { type: 'object', properties: { party_size: { type: 'integer' } }, required: ['party_size'] },
        }],
      },
      hitl: { channels: ['voice', 'client'] },
    };
    expect(wire(block).interaction).toEqual(block);
  });

  it('the session sends it in its config, and bud.agent passes it on', async () => {
    const { session, sentJson } = makeSession({ interaction: true, autoConfig: true });
    await session.connect();
    expect(sentJson()[0]).toEqual({ type: 'config', interaction: true });
    await session.disconnect();

    const cfg = buildAgentSessionConfig({ llm: { baseUrl: 'http://llm/v1', model: 'm' }, interaction: { events: { floor: false } } }, 'ws://x/ws');
    expect(cfg.interaction).toEqual({ events: { floor: false } });
    expect(buildAgentSessionConfig({ llm: { baseUrl: 'b', model: 'm' } }, 'ws://x/ws').interaction).toBeUndefined();
  });
});

describe('ready on an interaction session', () => {
  it('reads protocol 1.1 and the echo, with no drift warning', async () => {
    const { session, getWs } = makeSession();
    const ready: ReadyEvent[] = [];
    const warnings: string[] = [];
    const errors: string[] = [];
    session.on('ready', (e) => ready.push(e));
    session.on('configWarning', (w) => warnings.push(w.code));
    session.on('error', (e) => errors.push(e.code));
    await session.connect();
    getWs().emit({ type: 'ready', protocol_version: '1.1', stream_id: 'call-1', interaction: ECHO });
    expect(ready[0]?.protocolVersion).toBe('1.1');
    expect(ready[0]?.interaction).toEqual(ECHO);
    expect(session.getInteraction()?.tasks.tools).toEqual(['book_table']);
    expect(warnings).toEqual([]);
    expect(errors).toEqual([]);
    await session.disconnect();
  });

  it('a plain session has no echo', async () => {
    const { session, getWs } = makeSession();
    await session.connect();
    getWs().emit({ type: 'ready', protocol_version: '1.0', stream_id: 's' });
    expect(session.getInteraction()).toBeUndefined();
    await session.disconnect();
  });
});

describe('interaction events', () => {
  it('knows every event type of the gateway', () => {
    expect([...INTERACTION_EVENT_TYPES].sort()).toEqual(Object.keys(WIRE).sort());
  });

  it('emits each frame once, unchanged, on `interaction`', async () => {
    const { session, getWs } = makeSession();
    const seen: InteractionEvent[] = [];
    session.on('interaction', (e) => seen.push(e));
    await session.connect();
    for (const frame of Object.values(WIRE)) getWs().emit(frame);
    // A voice agent's line without the mode (no `source`) and other frames are not events.
    getWs().emit({ type: 'assistant_transcript', turn_index: 1, delta: 'Hi' });
    getWs().emit({ type: 'tts_playback_complete', timestamp: 1 });
    expect(seen).toEqual(Object.values(WIRE));
    await session.disconnect();
  });

  it('narrows by type', () => {
    const event = WIRE.task_question as unknown as InteractionEvent;
    if (event.type !== 'task_question') throw new Error('not a question');
    expect(event.request_id).toBe('q_task_5b1e9c0d_2');
    expect(event.options).toEqual(['7 pm', '8 pm']);
    expect(isInteractionEvent({ type: 'assistant_transcript', delta: 'x' })).toBe(false);
    expect(isInteractionEvent({ type: 'assistant_transcript', delta: 'x', source: 'front' })).toBe(true);
    expect(isInteractionEvent({ type: 'stt_result' })).toBe(false);
    const failed = WIRE.session_failed as unknown as InteractionEvent;
    if (failed.type !== 'session_failed') throw new Error('not a failure');
    expect([failed.service, failed.follow_up, failed.pending[0].state]).toEqual(['background', false, 'working']);
    const updated = { type: 'interaction_updated', applied: [], refused: [], failure: { line: 'stall-1', kind: 'stall', service: 'background', text: 'Bear with me.' } } as unknown as InteractionEvent;
    if (updated.type !== 'interaction_updated') throw new Error('not an update');
    expect(updated.failure?.kind).toBe('stall');
    expect(isInteractionEvent({ type: 'task_output', task_id: 't', text: 'Done.', data: null, artifacts: [] })).toBe(true);
  });
});

describe('interaction client messages', () => {
  it('sends the gateway wire of each', async () => {
    const { session, sentJson } = makeSession();
    await session.connect();
    session.answerTask('q1');
    session.answerTask('q2', 'accept', { value: '8 pm' });
    session.answerTask('q3', 'decline', { correction: 'make it four' });
    session.answerTask('q4', 'cancel');
    session.cancelTask('task_1');
    session.markTaskSeen('task_2');
    session.updateInteraction({ policy: { hold_after_silence_ms: 5000 } });
    session.reportPlayback('u_41', 1320.4);
    expect(sentJson()).toEqual([
      { type: 'task_answer', request_id: 'q1', outcome: 'accept' },
      { type: 'task_answer', request_id: 'q2', outcome: 'accept', value: '8 pm' },
      { type: 'task_answer', request_id: 'q3', outcome: 'decline', correction: 'make it four' },
      { type: 'task_answer', request_id: 'q4', outcome: 'cancel' },
      { type: 'task_cancel', task_id: 'task_1' },
      { type: 'task_seen', task_id: 'task_2' },
      { type: 'interaction_update', patch: { policy: { hold_after_silence_ms: 5000 } } },
      { type: 'playback_position', utterance_id: 'u_41', played_ms: 1320 },
    ]);
    await session.disconnect();
  });

  it('AgentSession delegates', () => {
    const agent = new AgentSession({ llm: { baseUrl: 'b', model: 'm' }, interaction: true }, 'ws://x/ws');
    const inner = agent.getSession();
    const send = vi.spyOn(inner, 'send').mockImplementation(() => {});
    agent.answerTask('q1', 'accept', { value: { party: 2 } });
    agent.cancelTask('task_1');
    agent.updateInteraction({ events: { floor: false } });
    agent.reportPlayback('u_1', 250);
    expect(send.mock.calls.map(([m]) => (m as { type: string }).type)).toEqual([
      'task_answer', 'task_cancel', 'interaction_update', 'playback_position',
    ]);
    expect(agent.getInteraction()).toBeUndefined();
  });
});

describe('task list (GET /v1/tasks)', () => {
  it('lists and gets tasks with their wire fields', async () => {
    const urls: string[] = [];
    const listed = { tasks: [{ id: 'task_1', task_ref: 't1', revision: 1, title: 'your table booking', state: 'input_required', cancelling: false, effects: 'none_committed', open_question: 'q_task_1_1', side_effects: true, started_at: 4180 }] };
    const one = { ...listed.tasks[0], state: 'completed', effects: ['booked a table for 2'], open_question: null, stream_id: 'call 1', result: { state: 'completed', summary: 'Booked.', effects: ['booked a table for 2'] } };
    const bodies = [listed, {}, one];
    const fetchFn = vi.fn(async (url: RequestInfo | URL) => {
      urls.push(String(url));
      return new Response(JSON.stringify(bodies.shift()), { status: 200, headers: { 'content-type': 'application/json' } });
    }) as unknown as typeof fetch;
    const client = new RestClient({ baseUrl: 'http://gw', fetch: fetchFn });
    const tasks = await client.listTasks('call 1');
    expect(tasks.map((t) => [t.id, t.state, t.open_question])).toEqual([['task_1', 'input_required', 'q_task_1_1']]);
    expect(await client.listTasks()).toEqual([]);
    const task = await client.getTask('task_1', 'call 1');
    expect(task.result?.summary).toBe('Booked.');
    expect(task.stream_id).toBe('call 1');
    expect(urls).toEqual([
      'http://gw/v1/tasks?stream_id=call%201',
      'http://gw/v1/tasks',
      'http://gw/v1/tasks/task_1?stream_id=call%201',
    ]);
  });
});

/**
 * TS↔Python REST-surface parity: sipTransfer, LiveKit participant management,
 * DAG templates/validation, Prometheus metrics, and live language capabilities.
 *
 * Every wire field asserted here is verified against the gateway handler
 * structs (the authority):
 * - SIPTransferRequest/Response       — gateway/src/handlers/sip/transfer.rs
 * - Remove/MuteParticipantRequest     — gateway/src/handlers/livekit/participants.rs
 * - ListTemplates/ValidateDAG         — gateway/src/handlers/dag.rs
 * - LanguageCapabilitiesResponse      — gateway/src/handlers/capabilities.rs
 */
import { describe, it, expect, vi } from 'vitest';
import { RestClient } from '../../src/rest/client.js';

/** Recorded call for wire-shape assertions. */
interface RecordedCall {
  url: string;
  method?: string;
  body?: unknown;
}

/** Build a RestClient whose fetch records method/path/body and returns `response`. */
function mockClient(response: () => Response): { client: RestClient; calls: RecordedCall[] } {
  const calls: RecordedCall[] = [];
  const fetchFn = vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => {
    calls.push({
      url: String(url),
      method: init?.method,
      body: typeof init?.body === 'string' ? JSON.parse(init.body) : undefined,
    });
    return response();
  }) as unknown as typeof fetch;
  return { client: new RestClient({ baseUrl: 'http://gw', fetch: fetchFn }), calls };
}

const json = (payload: unknown) =>
  new Response(JSON.stringify(payload), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });

describe('sipTransfer (POST /sip/transfer)', () => {
  it('sends the gateway SIPTransferRequest field names and maps the response', async () => {
    const { client, calls } = mockClient(() =>
      json({
        status: 'completed',
        room_name: 'proj_call-room-123',
        participant_identity: 'sip_participant_456',
        transfer_to: 'tel:+15551234567',
      }),
    );

    const result = await client.sipTransfer({
      roomName: 'call-room-123',
      participantIdentity: 'sip_participant_456',
      transferTo: '+15551234567',
    });

    expect(calls).toHaveLength(1);
    expect(calls[0]?.method).toBe('POST');
    expect(calls[0]?.url).toBe('http://gw/sip/transfer');
    // Gateway struct requires exactly these three snake_case fields (NO stream_id).
    expect(calls[0]?.body).toEqual({
      room_name: 'call-room-123',
      participant_identity: 'sip_participant_456',
      transfer_to: '+15551234567',
    });

    expect(result).toEqual({
      status: 'completed',
      roomName: 'proj_call-room-123',
      participantIdentity: 'sip_participant_456',
      transferTo: 'tel:+15551234567',
    });
  });
});

describe('removeLiveKitParticipant (DELETE /livekit/participant)', () => {
  it('sends room_name + participant_identity in a DELETE body and maps the response', async () => {
    const { client, calls } = mockClient(() =>
      json({
        status: 'removed',
        room_name: 'proj_room-1',
        participant_identity: 'user-alice',
      }),
    );

    const result = await client.removeLiveKitParticipant('room-1', 'user-alice');

    expect(calls).toHaveLength(1);
    expect(calls[0]?.method).toBe('DELETE');
    expect(calls[0]?.url).toBe('http://gw/livekit/participant');
    expect(calls[0]?.body).toEqual({
      room_name: 'room-1',
      participant_identity: 'user-alice',
    });

    expect(result).toEqual({
      status: 'removed',
      roomName: 'proj_room-1',
      participantIdentity: 'user-alice',
    });
  });
});

describe('muteLiveKitParticipant (POST /livekit/participant/mute)', () => {
  it('sends all four MuteParticipantRequest fields (muted defaults to true)', async () => {
    const { client, calls } = mockClient(() =>
      json({
        room_name: 'proj_room-1',
        participant_identity: 'user-alice',
        track_sid: 'TR_abc123',
        muted: true,
      }),
    );

    const result = await client.muteLiveKitParticipant('room-1', 'user-alice', 'TR_abc123');

    expect(calls[0]?.method).toBe('POST');
    expect(calls[0]?.url).toBe('http://gw/livekit/participant/mute');
    expect(calls[0]?.body).toEqual({
      room_name: 'room-1',
      participant_identity: 'user-alice',
      track_sid: 'TR_abc123',
      muted: true,
    });

    expect(result).toEqual({
      roomName: 'proj_room-1',
      participantIdentity: 'user-alice',
      trackSid: 'TR_abc123',
      muted: true,
    });
  });

  it('supports unmuting (muted: false is sent on the wire, not dropped)', async () => {
    const { client, calls } = mockClient(() =>
      json({
        room_name: 'proj_room-1',
        participant_identity: 'user-alice',
        track_sid: 'TR_abc123',
        muted: false,
      }),
    );

    const result = await client.muteLiveKitParticipant('room-1', 'user-alice', 'TR_abc123', false);

    expect((calls[0]?.body as Record<string, unknown>).muted).toBe(false);
    expect(result.muted).toBe(false);
  });
});

describe('DAG template methods (handlers/dag.rs)', () => {
  it('listDAGTemplates GETs /dag/templates and returns the wire shape verbatim', async () => {
    const payload = {
      templates: [
        { name: 'voice-assistant', version: '1.0', description: 'Voice Assistant' },
        { name: 'simple-stt', version: '1.0', description: null },
      ],
      count: 2,
    };
    const { client, calls } = mockClient(() => json(payload));

    const result = await client.listDAGTemplates();

    expect(calls[0]?.method).toBe('GET');
    expect(calls[0]?.url).toBe('http://gw/dag/templates');
    expect(calls[0]?.body).toBeUndefined();
    expect(result).toEqual(payload);
  });

  it('getDAGTemplate GETs /dag/templates/{name} (URL-encoded)', async () => {
    const payload = {
      name: 'voice assistant',
      template: { id: 'va', entry_node: 'in', exit_nodes: ['out'] },
    };
    const { client, calls } = mockClient(() => json(payload));

    const result = await client.getDAGTemplate('voice assistant');

    expect(calls[0]?.method).toBe('GET');
    expect(calls[0]?.url).toBe('http://gw/dag/templates/voice%20assistant');
    expect(result).toEqual(payload);
  });

  it('validateDAG POSTs {dag: definition} verbatim and maps node_count/edge_count', async () => {
    const { client, calls } = mockClient(() =>
      json({ valid: true, errors: [], warnings: [], node_count: 3, edge_count: 2 }),
    );

    const definition = {
      id: 'd1',
      name: 'D1',
      version: '1.0',
      nodes: [{ id: 'a', type: 'audio_input' }],
      edges: [],
      entry_node: 'a',
      exit_nodes: ['a'],
    };
    const result = await client.validateDAG(definition);

    expect(calls[0]?.method).toBe('POST');
    expect(calls[0]?.url).toBe('http://gw/dag/validate');
    // Gateway ValidateDAGRequest is an envelope: { dag: <definition passthrough> }.
    expect(calls[0]?.body).toEqual({ dag: definition });

    expect(result).toEqual({
      valid: true,
      errors: [],
      warnings: [],
      nodeCount: 3,
      edgeCount: 2,
    });
  });

  it('validateDAG surfaces server-side parse failures as valid:false', async () => {
    const { client } = mockClient(() =>
      json({
        valid: false,
        errors: ['Failed to parse DAG definition: missing field `entry_node`'],
        warnings: [],
        node_count: 0,
        edge_count: 0,
      }),
    );

    const result = await client.validateDAG({ id: 'broken' });
    expect(result.valid).toBe(false);
    expect(result.errors[0]).toContain('entry_node');
    expect(result.nodeCount).toBe(0);
  });
});

describe('getMetrics (GET /metrics)', () => {
  it('returns the Prometheus text exposition as a string', async () => {
    const exposition =
      '# HELP waav_turns_total Total turns\n# TYPE waav_turns_total counter\nwaav_turns_total 42\n';
    const { client, calls } = mockClient(
      () =>
        new Response(exposition, {
          status: 200,
          headers: { 'content-type': 'text/plain; version=0.0.4' },
        }),
    );

    const result = await client.getMetrics();

    expect(calls[0]?.method).toBe('GET');
    expect(calls[0]?.url).toBe('http://gw/metrics');
    expect(typeof result).toBe('string');
    expect(result).toBe(exposition);
  });
});

describe('getLanguageCapabilities (GET /capabilities/languages)', () => {
  const wirePayload = {
    canonical_languages: [
      { bcp47: 'en-US', lang_subtag: 'en', iso639_1: 'en', region: 'US' },
      { bcp47: 'cmn-CN', lang_subtag: 'cmn', iso639_1: 'zh', region: 'CN' },
    ],
    providers: [
      {
        provider: 'deepgram',
        notation: 'bcp47',
        supports_auto: true,
        example_cmn_cn: 'zh-CN',
        example_en_us: 'en-US',
      },
      {
        provider: 'elevenlabs',
        notation: 'iso6391',
        supports_auto: true,
        example_cmn_cn: 'zh',
        example_en_us: 'en',
      },
      {
        provider: 'hume',
        notation: 'none',
        supports_auto: true,
        example_cmn_cn: null,
        example_en_us: null,
      },
    ],
    canonical_count: 2,
  };

  it('GETs with NO query params and maps snake_case to camelCase', async () => {
    const { client, calls } = mockClient(() => json(wirePayload));

    const result = await client.getLanguageCapabilities();

    expect(calls[0]?.method).toBe('GET');
    // The gateway handler takes no Query extractor — the URL must be bare.
    expect(calls[0]?.url).toBe('http://gw/capabilities/languages');

    expect(result.canonicalCount).toBe(2);
    expect(result.canonicalLanguages).toEqual([
      { bcp47: 'en-US', langSubtag: 'en', iso6391: 'en', region: 'US' },
      { bcp47: 'cmn-CN', langSubtag: 'cmn', iso6391: 'zh', region: 'CN' },
    ]);
    expect(result.providers).toHaveLength(3);
    expect(result.providers[0]).toEqual({
      provider: 'deepgram',
      notation: 'bcp47',
      supportsAuto: true,
      exampleCmnCn: 'zh-CN',
      exampleEnUs: 'en-US',
    });
    // Option<String> None rows arrive as null and stay null (not undefined).
    expect(result.providers[2]?.exampleCmnCn).toBeNull();
  });

  it('filters providers CLIENT-side when a provider is given (URL stays bare)', async () => {
    const { client, calls } = mockClient(() => json(wirePayload));

    const result = await client.getLanguageCapabilities('elevenlabs');

    expect(calls[0]?.url).toBe('http://gw/capabilities/languages');
    expect(result.providers).toHaveLength(1);
    expect(result.providers[0]?.provider).toBe('elevenlabs');
    // The canonical value space is always returned in full.
    expect(result.canonicalLanguages).toHaveLength(2);
    expect(result.canonicalCount).toBe(2);
  });

  it('returns an empty providers list for an unknown provider filter', async () => {
    const { client } = mockClient(() => json(wirePayload));
    const result = await client.getLanguageCapabilities('nope');
    expect(result.providers).toEqual([]);
  });
});

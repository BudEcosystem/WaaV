import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  STT_PROVIDERS,
  TTS_PROVIDERS,
  REALTIME_PROVIDERS,
  isValidSTTProvider,
  isValidTTSProvider,
  isValidRealtimeProvider,
  getProviderCapabilities,
} from '../../src/types/providers';

// =============================================================================
// Provider-enum DRIFT GUARD
//
// The SDK provider enums must cover the gateway's canonical provider fleet so a
// provider that exists server-side can never be unreachable from the SDK. The
// de-duplicated source of truth is the `Builtin{STT,TTS,Realtime}Provider` enum
// variants in gateway/src/plugin/dispatch.rs (the PHF maps carry aliases too;
// the enum variants are the minimal canonical set). We parse those variant
// names, normalize them, and assert SDK ⊇ gateway. A new gateway variant with
// no SDK entry FAILS this test.
// =============================================================================

const DISPATCH_RS = resolve(__dirname, '../../../../gateway/src/plugin/dispatch.rs');

/** Normalize a provider identifier for cross-naming comparison. */
function norm(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]/g, '');
}

/**
 * Known canonical renames where the SDK's canonical name differs from the
 * gateway enum variant spelling (the variant is an alias of the canonical).
 * STT/TTS canonicalise Azure to `microsoft-azure`; the realtime fleet keeps the
 * bare `azure`, so the rename is domain-scoped.
 */
const STT_TTS_RENAME: Record<string, string> = {
  // Gateway enum variant `Azure` → canonical SDK name `microsoft-azure`.
  azure: 'microsoftazure',
};

function canon(variant: string, rename: Record<string, string> = {}): string {
  const n = norm(variant);
  return rename[n] ?? n;
}

/**
 * Extract the PascalCase variant names of a `pub enum <name> { ... }` block
 * from the dispatch.rs source (4-space-indented identifiers, ignoring derives,
 * doc-comments and attributes).
 */
function extractEnumVariants(source: string, enumName: string): string[] {
  const start = source.indexOf(`pub enum ${enumName} {`);
  if (start === -1) throw new Error(`enum ${enumName} not found in dispatch.rs`);
  const open = source.indexOf('{', start);
  // Walk to the matching closing brace.
  let depth = 0;
  let end = open;
  for (let i = open; i < source.length; i++) {
    if (source[i] === '{') depth++;
    else if (source[i] === '}') {
      depth--;
      if (depth === 0) { end = i; break; }
    }
  }
  const body = source.slice(open + 1, end);
  const variants: string[] = [];
  for (const line of body.split('\n')) {
    // Variant lines look like `    OpenAI = 0,` or `    Deepgram,` — a 4-space
    // indented PascalCase ident, an optional `= <discriminant>`, then a comma.
    // Doc-comments (`/// ...`) and attributes (`#[...]`) are skipped.
    const m = line.match(/^\s{4}([A-Z][A-Za-z0-9]*)\s*(?:=\s*[^,]+)?,\s*$/);
    if (m) variants.push(m[1]!);
  }
  return variants;
}

describe('provider-enum drift guard (SDK ⊇ gateway dispatch.rs)', () => {
  const source = readFileSync(DISPATCH_RS, 'utf8');

  it('SDK STT_PROVIDERS covers every gateway BuiltinSTTProvider variant', () => {
    const variants = extractEnumVariants(source, 'BuiltinSTTProvider');
    expect(variants.length).toBeGreaterThan(0);
    const sdk = new Set(STT_PROVIDERS.map(norm));
    const missing = variants.filter((v) => !sdk.has(canon(v, STT_TTS_RENAME)));
    expect(missing, `gateway STT providers missing from SDK STT_PROVIDERS: ${missing.join(', ')}`).toEqual([]);
  });

  it('SDK TTS_PROVIDERS covers every gateway BuiltinTTSProvider variant', () => {
    const variants = extractEnumVariants(source, 'BuiltinTTSProvider');
    expect(variants.length).toBeGreaterThan(0);
    const sdk = new Set(TTS_PROVIDERS.map(norm));
    const missing = variants.filter((v) => !sdk.has(canon(v, STT_TTS_RENAME)));
    expect(missing, `gateway TTS providers missing from SDK TTS_PROVIDERS: ${missing.join(', ')}`).toEqual([]);
  });

  it('SDK REALTIME_PROVIDERS covers every gateway BuiltinRealtimeProvider variant', () => {
    const variants = extractEnumVariants(source, 'BuiltinRealtimeProvider');
    expect(variants.length).toBe(12); // 12 realtime/S2S providers
    const sdk = new Set(REALTIME_PROVIDERS.map(norm));
    const missing = variants.filter((v) => !sdk.has(canon(v)));
    expect(missing, `gateway realtime providers missing from SDK REALTIME_PROVIDERS: ${missing.join(', ')}`).toEqual([]);
  });
});

describe('STT Provider Types (full fleet)', () => {
  it('has the full canonical STT fleet (32 providers)', () => {
    expect(STT_PROVIDERS).toHaveLength(32);
  });

  it('includes the common providers under their canonical names', () => {
    for (const p of ['deepgram', 'google', 'microsoft-azure', 'cartesia', 'assemblyai', 'openai', 'sarvam', 'speechmatics']) {
      expect(STT_PROVIDERS).toContain(p);
    }
  });

  it('validates correct STT providers', () => {
    expect(isValidSTTProvider('deepgram')).toBe(true);
    expect(isValidSTTProvider('microsoft-azure')).toBe(true);
    expect(isValidSTTProvider('openai')).toBe(true); // STT now supports openai
  });

  it('rejects invalid STT providers', () => {
    expect(isValidSTTProvider('invalid')).toBe(false);
    expect(isValidSTTProvider('')).toBe(false);
    // Old narrowed names that no longer exist as canonical:
    expect(isValidSTTProvider('azure')).toBe(false);
    expect(isValidSTTProvider('openai-whisper')).toBe(false);
  });
});

describe('TTS Provider Types (full fleet)', () => {
  it('has the full canonical TTS fleet (37 providers)', () => {
    expect(TTS_PROVIDERS).toHaveLength(37);
  });

  it('includes the common providers', () => {
    for (const p of ['deepgram', 'elevenlabs', 'google', 'microsoft-azure', 'cartesia', 'openai', 'hume', 'lmnt', 'playht']) {
      expect(TTS_PROVIDERS).toContain(p);
    }
  });

  it('marks providers that support emotion (catalogued subset)', () => {
    expect(getProviderCapabilities('elevenlabs', 'tts')?.supportsEmotion).toBe(true);
    expect(getProviderCapabilities('hume', 'tts')?.supportsEmotion).toBe(true);
    expect(getProviderCapabilities('microsoft-azure', 'tts')?.supportsEmotion).toBe(true);
    expect(getProviderCapabilities('deepgram', 'tts')?.supportsEmotion).toBe(false);
  });

  it('returns undefined (not throw) for a valid-but-uncatalogued provider', () => {
    expect(isValidTTSProvider('murf')).toBe(true);
    expect(getProviderCapabilities('murf', 'tts')).toBeUndefined();
  });
});

describe('Realtime Provider Types (gateway /realtime fleet)', () => {
  it('has the full realtime/S2S fleet (12 providers)', () => {
    expect(REALTIME_PROVIDERS).toHaveLength(12);
  });

  it('uses gateway canonical names, not OpenAI-native names', () => {
    expect(isValidRealtimeProvider('openai')).toBe(true);
    expect(isValidRealtimeProvider('hume')).toBe(true);
    expect(isValidRealtimeProvider('gemini')).toBe(true);
    expect(isValidRealtimeProvider('nova_sonic')).toBe(true);
    // Old narrowed names are gone.
    expect(isValidRealtimeProvider('openai-realtime')).toBe(false);
    expect(isValidRealtimeProvider('hume-evi')).toBe(false);
  });

  it('returns capabilities for the catalogued realtime providers', () => {
    const openai = getProviderCapabilities('openai', 'realtime');
    expect(openai?.streaming).toBe(true);
    expect(openai?.supportsInterruption).toBe(true);
  });
});

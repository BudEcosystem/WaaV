/**
 * Canonical Language Value Space (P2 unified-language system).
 *
 * WaaV's gateway maps ONE canonical, region-qualified BCP-47 language token to EACH provider's
 * native notation internally — so the same `language` string works on every provider (switch
 * models without client edits). A developer passes one of these canonical tokens (or a bare
 * ISO-639-1 shorthand like `"en"`/`"hi"`, or the reversed/underscore/name spellings the gateway
 * folds: `"us-en"`, `"en_US"`, `"english"`), and the gateway resolves + renders it.
 *
 * This list mirrors the gateway's `CanonicalLanguage::all()`
 * (`gateway/src/core/lang/types.rs`) 1:1. It is REFERENCE/discovery only — the wire `language`
 * field stays a free `string` (an unrecognized token is forwarded verbatim and surfaced as a
 * `config_warning`, never a hard error). For the authoritative, always-current per-provider
 * native-notation matrix, call the gateway's `GET /capabilities/languages`.
 *
 * Chinese uses `cmn`/`yue` (ISO-639-3), NOT `zh`, because the canonical must disambiguate Mandarin
 * from Cantonese (the providers do: Google STT `cmn-Hans-CN` vs `yue-Hant-HK`). `"auto"` is the
 * detection token.
 */
export const CANONICAL_LANGUAGES = [
  'en-US', 'en-GB', 'en-IN', 'en-AU', 'en-ZA', 'en-NZ',
  'es-ES', 'es-MX', 'es-US',
  'fr-FR', 'fr-CA',
  'pt-BR', 'pt-PT',
  'de-DE', 'it-IT', 'nl-NL', 'ru-RU', 'tr-TR', 'pl-PL', 'sv-SE', 'nb-NO',
  'da-DK', 'fi-FI', 'uk-UA', 'cs-CZ', 'el-GR', 'ro-RO', 'hu-HU',
  'ja-JP', 'ko-KR', 'cmn-CN', 'cmn-TW', 'yue-HK',
  'hi-IN', 'bn-IN', 'ta-IN', 'te-IN', 'gu-IN', 'kn-IN', 'ml-IN', 'mr-IN',
  'pa-IN', 'or-IN',
  'ar-SA', 'vi-VN', 'th-TH', 'id-ID', 'ms-MY', 'he-IL',
] as const;

/**
 * A canonical region-qualified BCP-47 language token WaaV accepts (the literal union of
 * {@link CANONICAL_LANGUAGES}). The wire field accepts any `string`; this type is a convenience for
 * autocomplete/validation.
 */
export type CanonicalLanguage = (typeof CANONICAL_LANGUAGES)[number];

/**
 * Whether `value` is one of the canonical language tokens. NOTE: a `false` here does NOT mean the
 * gateway will reject the value — bare ISO-639-1 shorthands (`"en"`), name spellings (`"english"`),
 * and vendor quirks still resolve gateway-side, and any unrecognized token is forwarded verbatim
 * with a `config_warning`. Use this only for canonical-set membership checks.
 */
export function isCanonicalLanguage(value: string): value is CanonicalLanguage {
  return (CANONICAL_LANGUAGES as readonly string[]).includes(value);
}

/**
 * How a provider spells languages natively. Mirrors the gateway language support matrix
 * (`gateway/src/core/lang/mod.rs::language_support_matrix`): `bcp47` = region-qualified BCP-47
 * verbatim; `iso6391` = ISO-639-1 downgrade (region dropped); `underscore` = `lc_RR`; `model-id` =
 * the language IS a numeric model id; `composite` = a fused token; `none` = no language parameter
 * (inferred from text/voice/description).
 */
export type LanguageNotation = 'bcp47' | 'iso6391' | 'underscore' | 'model-id' | 'composite' | 'none';

/** The canonical language capability summary for a provider. */
export interface LanguageCapabilities {
  provider: string;
  notation: LanguageNotation;
  autoDetect: boolean;
  /** The full canonical value space accepted for EVERY provider (the gateway maps it). */
  canonicalLanguages: readonly CanonicalLanguage[];
}

// =============================================================================
// Live capability surface (`GET /capabilities/languages`, gateway
// handlers/capabilities.rs) — the always-current server-side matrix, as opposed
// to the static offline mirror below.
// =============================================================================

/**
 * One canonical language entry from the live endpoint (gateway
 * `CanonicalLanguageInfo`): the BCP-47 token a developer passes + its
 * decomposed subtags.
 */
export interface CanonicalLanguageInfo {
  /** The canonical region-qualified BCP-47 token (`en-US`, `cmn-CN`, `or-IN`). */
  bcp47: string;
  /** The bare language subtag (`en`, `cmn`, `yue`) (wire: `lang_subtag`). */
  langSubtag: string;
  /** The ISO-639-1 downgrade form (`en`, `zh`, `yue`) (wire: `iso639_1`). */
  iso6391: string;
  /** The UPPERCASE region subtag (`US`, `CN`, `IN`). */
  region: string;
}

/**
 * One per-provider row of the live matrix (gateway `LanguageSupportRow`,
 * core/lang/mod.rs): how the provider spells languages natively.
 */
export interface ProviderLanguageSupport {
  /** Provider id (e.g. `"deepgram"`). */
  provider: string;
  /** Native notation kind (`bcp47` / `iso6391` / `underscore` / `none`). */
  notation: LanguageNotation;
  /** Whether the provider honors the canonical `auto` natively (wire: `supports_auto`). */
  supportsAuto: boolean;
  /** Canonical `cmn-CN` rendered natively; null if the provider takes no language param (wire: `example_cmn_cn`). */
  exampleCmnCn: string | null;
  /** Canonical `en-US` rendered natively; null if no language param (wire: `example_en_us`). */
  exampleEnUs: string | null;
}

/**
 * The `GET /capabilities/languages` response (gateway
 * `LanguageCapabilitiesResponse`, handlers/capabilities.rs): the canonical
 * value space + the per-provider native-notation matrix.
 */
export interface LanguageCapabilitiesResponse {
  /** Every canonical language a developer may pass (wire: `canonical_languages`). */
  canonicalLanguages: CanonicalLanguageInfo[];
  /** The per-provider native-notation matrix (wire: `providers`). */
  providers: ProviderLanguageSupport[];
  /** Count of canonical languages (wire: `canonical_count`). */
  canonicalCount: number;
}

// Per-provider native-notation summary, mirroring the gateway language support matrix. This is the
// CANONICAL contract (how a provider spells languages + whether it auto-detects), NOT a stale
// per-provider language whitelist — passing any canonical token is supported for every provider
// (unsupported pairings degrade with a `config_warning`, never a hard error).
const LANGUAGE_NOTATION: Record<string, [LanguageNotation, boolean]> = {
  // IDENTITY-BCP47 (native == canonical BCP-47, region preserved)
  deepgram: ['bcp47', true],
  'microsoft-azure': ['bcp47', true],
  azure: ['bcp47', true],
  'aws-transcribe': ['bcp47', true],
  'aws-polly': ['bcp47', false],
  sarvam: ['bcp47', true],
  yandex: ['bcp47', true],
  google: ['bcp47', true],
  gemini: ['bcp47', true],
  nova_sonic: ['bcp47', true],
  tinkoff: ['bcp47', false],
  // DOWNGRADE-TO-ISO639-1
  elevenlabs: ['iso6391', true],
  openai: ['iso6391', true],
  cartesia: ['iso6391', false],
  assemblyai: ['iso6391', true],
  speechmatics: ['iso6391', true],
  groq: ['iso6391', true],
  gladia: ['iso6391', true],
  'fpt-ai': ['iso6391', true],
  reverie: ['iso6391', false],
  // UNDERSCORE
  iflytek: ['underscore', false],
  // ENUM / NUMERIC / COMPOSITE
  baidu: ['model-id', false],
  tencent: ['composite', false],
  // SPECIAL (no language parameter)
  hume: ['none', true],
};

/**
 * Canonical language capabilities for a provider (P2 standardization) — the STATIC, offline
 * lookup.
 *
 * For the authoritative, always-current server-side matrix call
 * `RestClient.getLanguageCapabilities()` (`GET /capabilities/languages`); this local table is the
 * connection-free mirror. It tells a developer (a) that the FULL canonical value space
 * ({@link CANONICAL_LANGUAGES}) is accepted for every provider — the gateway maps it — and (b) how
 * the provider spells languages natively (`notation`) + whether it `autoDetect`s. It intentionally
 * does NOT return a stale per-provider language whitelist (the thing the SDK used to hardcode and
 * let drift); passing any canonical token is supported. An uncatalogued provider gets the gateway's
 * safe `bcp47` default and is never blocked.
 */
export function languageCapabilities(provider: string): LanguageCapabilities {
  const [notation, autoDetect] = LANGUAGE_NOTATION[provider] ?? ['bcp47', false];
  return {
    provider,
    notation,
    autoDetect,
    // The full canonical value space is accepted for every provider; the gateway renders it to the
    // provider's native notation server-side.
    canonicalLanguages: CANONICAL_LANGUAGES,
  };
}

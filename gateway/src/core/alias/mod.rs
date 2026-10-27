//! Alias Registry (SDK standardization plan, Phase P3 — proxy / placeholder model names).
//!
//! An *alias* is a logical, server-defined name (e.g. `support-bot`) that maps to a
//! FULL resolved configuration bundle: a `{stt provider+model+language, tts
//! provider+voice+emotion, llm/conversation settings}` OR a whole `dag_template`. A
//! connecting client NAMES an alias on its `config` message (`alias: "support-bot"`)
//! and the gateway resolves it — BEFORE provider construction — by splicing the
//! resolved fragments into the session config. Ops can then RE-POINT the alias (swap
//! providers, A/B, kill-switch, cost-tier) by editing server config only, with ZERO
//! client redeploy. A single `model` string can't carry a `{stt,tts,llm}` bundle; the
//! alias can.
//!
//! This mirrors [`crate::dag::templates`] exactly — the same named-whole-DAG
//! indirection primitive WaaV already has — but at single-provider / agent
//! granularity:
//! - [`AliasRegistry`] backed by a [`DashMap`] for O(1) concurrent, case-insensitive
//!   lookup;
//! - a process-wide singleton via [`global_aliases`] / [`OnceLock`];
//! - [`initialize_aliases`] called once at startup from server config.
//!
//! # Security (SSRF)
//!
//! Alias DEFINITIONS are SERVER-CONFIG-ONLY (the `aliases:` section of `config.yaml`),
//! exactly like [`crate::config::ServerConfig::realtime_endpoint_overrides`]. The
//! client message carries only an alias NAME (a `String`), never an alias definition,
//! so a client can never inject a backend URL or credential through an alias. The
//! resolved bundle's `llm.base_url` (if any) still flows through the SAME SSRF
//! validation as a client-supplied `conversation_config.base_url` (it is spliced into
//! that field and validated downstream by the conversation orchestrator) — the alias
//! is just a trusted source of defaults, not a bypass.
//!
//! # Composition with the canonical layer (P2)
//!
//! An alias resolves to `provider + model + language + voice + emotion`; the canonical
//! language/emotion mappers then run against WHATEVER it resolved to. So re-pointing an
//! alias's TTS provider keeps a client's `language="en-US"` / `emotion="friendly"`
//! working automatically — the alias composes with, and does not replace, the canonical
//! mappers.

use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::OnceLock;
use tracing::{debug, info, warn};

use crate::handlers::ws::config::{
    ConversationWebSocketConfig, DAGWebSocketConfig, STTWebSocketConfig, TTSWebSocketConfig,
};

/// The kind of session an alias is intended for. Advisory only (the splice merges
/// whatever blocks are present); echoed back in the ready-ack so a developer sees what
/// the alias was meant to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum AliasKind {
    /// Speech-to-text only.
    Stt,
    /// Text-to-speech only.
    Tts,
    /// Speech-to-speech / `/realtime`.
    Realtime,
    /// Full STT→LLM→TTS conversation loop.
    Agent,
    /// A named DAG template.
    Dag,
}

/// Sparse STT defaults carried by an alias (the `[aliases.x.stt]` block).
///
/// Every field is optional: an alias supplies only the knobs it wants to fix; a client
/// field always overrides. Mirrors the public, provider-agnostic shape of
/// [`STTWebSocketConfig`].
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AliasStt {
    /// STT provider name (e.g. `deepgram`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Provider model (e.g. `nova-3`). Empty/None lets the provider pick its default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Canonical language token (e.g. `en-US`); resolved by the P2 language mapper.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The leg's own server (an OpenAI-compatible base such as `http://127.0.0.1:8080/v1`),
    /// instead of the process-wide `OPENAI_BASE_URL`. SERVER-CONFIG-ONLY: never echoed and never
    /// settable by a client; validated at startup with the egress rules (`validate_aliases`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Operator credential (`core::credentials`) the leg sends to `base_url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

/// Sparse TTS defaults carried by an alias (the `[aliases.x.tts]` block).
///
/// Mirrors the public, provider-agnostic shape of [`TTSWebSocketConfig`]. `emotion` is
/// the canonical emotion token (resolved by the emotion mapper downstream).
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AliasTts {
    /// TTS provider name (e.g. `elevenlabs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Provider voice id / name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Provider model (e.g. `eleven_turbo_v2`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Canonical emotion token (e.g. `friendly`); resolved by the emotion mapper.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<crate::core::emotion::Emotion>,
    /// The leg's own OpenAI-compatible speech server; see [`AliasStt::base_url`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Operator credential the leg sends to `base_url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    /// The synthesis request's timeout, seconds, when the client names none (the default is
    /// 60 s; a model loading on demand can take longer). Clamped like a client's value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_timeout_secs: Option<u64>,
}

/// Sparse LLM / conversation-loop defaults carried by an alias (the `[aliases.x.llm]`
/// block). Maps onto the public knobs of [`ConversationWebSocketConfig`].
///
/// SECURITY: `base_url` here is TRUSTED server config (it can only be set in
/// `config.yaml`), but when spliced it lands in `conversation_config.base_url` and is
/// validated by the SAME SSRF guard the conversation orchestrator applies to a
/// client-supplied base url — no bypass.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AliasLlm {
    /// OpenAI-compatible base URL for the LLM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Model identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Reasoning effort dial (`minimal|low|medium|high`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<crate::core::llm::ReasoningEffort>,
    /// Two-tier reasoning model (escalation tier).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_model: Option<String>,
    /// LLM vendor wire format (`openai|anthropic|gemini`).
    ///
    /// `value_type = Option<String>` for the OpenAPI schema because `AdapterKind` does
    /// not derive `ToSchema` (same approach `ConversationWebSocketConfig` uses).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, example = "openai"))]
    pub provider_kind: Option<crate::core::llm::AdapterKind>,
    /// Operator credential (`core::credentials`) the LLM leg uses when the client sends no key.
    /// The NAME only — the secret stays in the credential's env var / file, bound to its hosts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

/// One alias definition (one `[aliases.<name>]` table in `config.yaml`).
///
/// SERVER-CONFIG-ONLY. Deserialized from the `aliases:` YAML section at startup;
/// `deny_unknown_fields` makes a typo in an alias definition fail loudly at boot rather
/// than silently misconfigure a production session.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AliasDefinition {
    /// Intended session kind (advisory; echoed in the ready-ack).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<AliasKind>,
    /// STT defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt: Option<AliasStt>,
    /// TTS defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tts: Option<AliasTts>,
    /// LLM / conversation-loop defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<AliasLlm>,
    /// A named DAG template to route through (mutually-exclusive intent with the
    /// stt/tts/llm blocks; resolved at the SAME registry layer as
    /// `dag_config.template`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dag_template: Option<String>,
    /// Interaction mode for the sessions of this alias: the `interaction` block a session
    /// could send itself. A session's own block (or `false`) wins. Its tools are the
    /// operator's, like those of the server's default block.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub interaction: Option<serde_json::Value>,
    /// Whether the OpenAI-compatible routes may resolve this alias as a `model`
    /// (`POST /v1/audio/speech` → its TTS leg, `/v1/audio/transcriptions` → its STT leg). Off by
    /// default: only aliases the operator marks are reachable that way.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub openai_compatible: bool,
}

/// A leg's own server, from an alias: where the leg goes and which operator credential it sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasLegEndpoint {
    pub provider: String,
    pub base_url: String,
    pub credential_ref: Option<String>,
}

impl AliasLegEndpoint {
    /// Whether this endpoint governs a leg whose (final) provider is `provider`: a client that
    /// chose another provider does not get its traffic sent to the alias's server.
    pub fn applies_to(&self, provider: &str) -> bool {
        self.provider.trim().eq_ignore_ascii_case(provider.trim())
    }

    /// The base an OpenAI-shaped server is called at, with its version segment
    /// (`http://h:8080` and `http://h:8080/v1` both become `http://h:8080/v1`).
    pub fn openai_api_base(&self) -> String {
        waav_segmented_stt::vendor::openai::join_api_path(&self.base_url, "/v1")
    }
}

impl AliasDefinition {
    /// The STT leg's own server, when the alias names one.
    pub fn stt_endpoint(&self) -> Option<AliasLegEndpoint> {
        let leg = self.stt.as_ref()?;
        Some(AliasLegEndpoint {
            provider: leg.provider.clone()?,
            base_url: leg.base_url.clone()?,
            credential_ref: leg.credential_ref.clone(),
        })
    }

    /// The TTS leg's own server, when the alias names one.
    pub fn tts_endpoint(&self) -> Option<AliasLegEndpoint> {
        let leg = self.tts.as_ref()?;
        Some(AliasLegEndpoint {
            provider: leg.provider.clone()?,
            base_url: leg.base_url.clone()?,
            credential_ref: leg.credential_ref.clone(),
        })
    }
}

/// An alias's leg endpoints for one session (each `None` when the alias names no server).
#[derive(Debug, Clone, Default)]
pub struct AliasLegs {
    pub stt: Option<AliasLegEndpoint>,
    pub tts: Option<AliasLegEndpoint>,
}

impl AliasLegs {
    pub fn of(def: &AliasDefinition) -> Self {
        Self {
            stt: def.stt_endpoint(),
            tts: def.tts_endpoint(),
        }
    }
}

/// The key an alias leg sends to its own server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AliasLegKey {
    /// Empty when the server gets none (a keyless local server).
    pub key: String,
    /// The operator holds a key for the provider but it may not go to this server: the leg runs
    /// keyless, and the client is told (`config_warning`), so a 401 has an explanation.
    pub withheld: bool,
}

/// The key decision for an alias leg, the one rule every route applies: the client's own key;
/// else the alias's `credential_ref`, bound to the leg's server (a credential bound elsewhere is
/// an error, never a silent fallback); else the operator's key for the provider only where the
/// operator approved that server (`core::client_destination::operator_approved`); else none.
pub fn alias_leg_key(
    endpoint: &AliasLegEndpoint,
    client_key: Option<&str>,
    operator_key: impl FnOnce() -> Option<String>,
) -> Result<AliasLegKey, String> {
    if let Some(key) = client_key.map(str::trim).filter(|k| !k.is_empty()) {
        return Ok(AliasLegKey {
            key: key.to_string(),
            withheld: false,
        });
    }
    if let Some(name) = endpoint.credential_ref.as_deref() {
        let (bound, _) = crate::core::credentials::global_credentials()
            .bind(name, &endpoint.base_url)
            .map_err(|e| format!("alias leg ({}): {e}", endpoint.provider))?;
        let key = bound.current().ok_or_else(|| {
            format!(
                "alias leg ({}): credential '{name}' is declared but holds no value",
                endpoint.provider
            )
        })?;
        return Ok(AliasLegKey {
            key,
            withheld: false,
        });
    }
    let operator = operator_key().filter(|k| !k.trim().is_empty());
    if crate::core::client_destination::operator_approved(&endpoint.provider, &endpoint.base_url) {
        return Ok(AliasLegKey {
            key: operator.unwrap_or_default(),
            withheld: false,
        });
    }
    Ok(AliasLegKey {
        key: String::new(),
        withheld: operator.is_some(),
    })
}

/// Startup checks for the server's aliases: a leg's `base_url` needs the leg's `provider` and
/// must pass the egress rules (an http(s) URL, and a private target only where the operator
/// allows it), so a mistake stops the server instead of failing every session; an alias marked
/// `openai_compatible` needs an STT or TTS leg.
pub fn validate_aliases(config: &AliasConfig) -> Result<(), String> {
    for (name, def) in &config.aliases {
        for (leg, provider, base) in [
            (
                "stt",
                def.stt.as_ref().and_then(|l| l.provider.as_deref()),
                def.stt.as_ref().and_then(|l| l.base_url.as_deref()),
            ),
            (
                "tts",
                def.tts.as_ref().and_then(|l| l.provider.as_deref()),
                def.tts.as_ref().and_then(|l| l.base_url.as_deref()),
            ),
        ] {
            let Some(base) = base else { continue };
            if provider.is_none_or(|p| p.trim().is_empty()) {
                return Err(format!(
                    "alias '{name}': {leg}.base_url needs {leg}.provider (the provider that speaks to it)"
                ));
            }
            crate::core::net::validate_url_for_ssrf(base, crate::core::net::HTTP_WS_URL_SCHEMES)
                .map_err(|e| {
                    format!(
                        "alias '{name}': {leg}.base_url {base:?} is not allowed: {e}. A private \
                     address needs WAAV_PRIVATE_EGRESS_ALLOWLIST (host:port)"
                    )
                })?;
        }
        // The OpenAI-compatible routes run a leg's own server through the self-hosted vendor,
        // which speaks OpenAI's API; another provider's base would be ignored there, silently.
        if def.openai_compatible {
            for (leg, endpoint) in [("stt", def.stt_endpoint()), ("tts", def.tts_endpoint())] {
                if let Some(endpoint) = endpoint
                    && !endpoint.provider.trim().eq_ignore_ascii_case("openai")
                {
                    return Err(format!(
                        "alias '{name}': openai_compatible serves a leg's base_url as an \
                         OpenAI-compatible server, so {leg}.provider must be openai (got {:?})",
                        endpoint.provider
                    ));
                }
            }
        }
        if def.openai_compatible && def.stt.is_none() && def.tts.is_none() {
            return Err(format!(
                "alias '{name}': openai_compatible needs an stt or tts leg to serve"
            ));
        }
    }
    Ok(())
}

/// Configuration for the alias registry, assembled at server-config load.
///
/// Mirrors [`crate::dag::templates::TemplatesConfig`]: a name→definition map the
/// startup hook registers.
#[derive(Debug, Clone, Default)]
pub struct AliasConfig {
    /// Inline alias definitions (name → definition).
    pub aliases: HashMap<String, AliasDefinition>,
}

/// Global alias registry singleton.
static GLOBAL_ALIASES: OnceLock<AliasRegistry> = OnceLock::new();

/// Get the global alias registry.
///
/// Lazily initialized empty on first access (a deployment with no `aliases:` section
/// simply has an empty registry, and every `alias` lookup misses → `config_warning`).
pub fn global_aliases() -> &'static AliasRegistry {
    GLOBAL_ALIASES.get_or_init(AliasRegistry::new)
}

/// Initialize the global alias registry from server configuration.
///
/// Called ONCE during server startup (mirrors
/// [`crate::dag::templates::initialize_templates`]). Returns the number of aliases
/// registered.
pub fn initialize_aliases(config: &AliasConfig) -> usize {
    let registry = global_aliases();
    let mut count = 0;
    for (name, def) in &config.aliases {
        registry.register(name.clone(), def.clone());
        info!(name = %name, kind = ?def.kind, "Registered config alias");
        count += 1;
    }
    info!(total = %count, "Alias registry initialized");
    count
}

/// Thread-safe, case-insensitive registry of alias definitions.
///
/// Mirrors [`crate::dag::templates::DAGTemplateRegistry`].
pub struct AliasRegistry {
    /// Definitions indexed by lowercased name.
    aliases: DashMap<String, AliasDefinition>,
    /// Original names for display.
    original_names: DashMap<String, String>,
}

impl AliasRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            aliases: DashMap::new(),
            original_names: DashMap::new(),
        }
    }

    /// Register an alias under the given name (stored case-insensitively).
    pub fn register(&self, name: impl Into<String>, def: AliasDefinition) {
        let original = name.into();
        let key = original.to_lowercase();
        debug!(name = %original, "Registering alias");
        self.aliases.insert(key.clone(), def);
        self.original_names.insert(key, original);
    }

    /// Resolve an alias by name (case-insensitive). `None` when unknown.
    pub fn resolve(&self, name: &str) -> Option<AliasDefinition> {
        let key = name.to_lowercase();
        self.aliases.get(&key).map(|e| e.value().clone())
    }

    /// Whether an alias with this name exists (case-insensitive).
    pub fn contains(&self, name: &str) -> bool {
        self.aliases.contains_key(&name.to_lowercase())
    }

    /// All registered alias names (original casing).
    pub fn names(&self) -> Vec<String> {
        self.original_names
            .iter()
            .map(|e| e.value().clone())
            .collect()
    }

    /// Number of registered aliases.
    pub fn len(&self) -> usize {
        self.aliases.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }

    /// Remove an alias (case-insensitive). Returns the removed definition.
    pub fn remove(&self, name: &str) -> Option<AliasDefinition> {
        let key = name.to_lowercase();
        self.original_names.remove(&key);
        self.aliases.remove(&key).map(|(_, v)| v)
    }

    /// Clear all aliases.
    pub fn clear(&self) {
        self.aliases.clear();
        self.original_names.clear();
    }
}

impl Default for AliasRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ────────────────────────────────────────────────────────────────────────────────────
// Splice: merge a resolved alias into the session's WS config fragments.
//
// Contract (the plan's rule #3): the alias supplies DEFAULTS; an explicit client field
// ALWAYS wins. So we only fill a destination field when the alias has a value AND the
// client did not already provide one. When the client omitted a whole sub-config, the
// alias MATERIALIZES it from its own defaults (e.g. `{alias:"support-bot"}` with no
// `stt_config` produces a full STT config from the alias).
// ────────────────────────────────────────────────────────────────────────────────────

/// The concrete config an alias splice produced, returned so the caller can ECHO it in
/// the ready-ack (a developer SEES what `support-bot` became) and so SDK capability
/// negotiation runs against the resolved providers.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ResolvedAliasEcho {
    /// The alias name that was resolved.
    pub name: String,
    /// The alias kind (if declared).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<AliasKind>,
    /// Resolved STT provider/model/language (post-merge, client-wins).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt: Option<AliasStt>,
    /// Resolved TTS provider/voice/model/emotion (post-merge, client-wins).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tts: Option<AliasTts>,
    /// Resolved LLM provider/model (post-merge, client-wins). The base URL is echoed so
    /// a developer can confirm the upstream; no secret is included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<AliasLlm>,
    /// Resolved DAG template name, if the alias routes through one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dag_template: Option<String>,
}

/// A fresh STT config carrying only the universal audio defaults (16 kHz mono PCM,
/// punctuation on). The provider/model/language are left EMPTY so the alias (and then
/// the client) fills them; an empty `provider` would fail validation, which is exactly
/// the desired loud failure when neither the alias nor the client supplies one.
///
/// Note: [`STTWebSocketConfig`] does not derive `Default` (its `sample_rate`/`channels`
/// are required on the wire and would default to a broken `0`), so the alias path
/// materializes it explicitly — mirroring the `default_tts_config` precedent in
/// `config_handler`.
fn default_alias_stt() -> STTWebSocketConfig {
    STTWebSocketConfig {
        provider: String::new(),
        language: String::new(),
        sample_rate: 16_000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: String::new(),
        api_key: None,
        features: Default::default(),
        extras: Default::default(),
        translation: None,
        turn_detection: None,
        transcription_mode: None,
        audio_in_codec: None,
        noise_suppression: None,
    }
}

/// A fresh TTS config carrying only the universal audio default (24 kHz, matching the
/// `default_tts_config` precedent in `config_handler`). Provider/voice/model left EMPTY
/// for the alias/client to fill.
fn default_alias_tts() -> TTSWebSocketConfig {
    TTSWebSocketConfig {
        provider: String::new(),
        voice_id: None,
        speaking_rate: None,
        audio_format: None,
        sample_rate: Some(24_000),
        client_playback_rate: None,
        audio_out_chunk_ms: None,
        connection_timeout: None,
        request_timeout: None,
        model: String::new(),
        pronunciations: Vec::new(),
        api_key: None,
        emotion: None,
        emotion_intensity: None,
        delivery_style: None,
        emotion_description: None,
        voice_descriptor: None,
        features: Default::default(),
        extras: Default::default(),
        audio_out_codec: None,
    }
}

fn fallback_alias_conversation() -> ConversationWebSocketConfig {
    ConversationWebSocketConfig {
        base_url: String::new(),
        model: String::new(),
        system_prompt: None,
        api_key: None,
        temperature: None,
        max_tokens: None,
        streaming: true,
        max_history: 20,
        allow_interruption: true,
        eager_eot: false,
        provider_kind: None,
        barge_in_min_words: None,
        summarize_target_tokens: 0,
        mute_strategy: None,
        strip_markdown: true,
        user_idle_timeout_ms: 0,
        reasoning_effort: None,
        latency_filler: Default::default(),
        latency_filler_after_ms: None,
        latency_filler_phrases: Vec::new(),
        reasoning_model: None,
        reasoning_base_url: None,
        reasoning_api_key: None,
        credential_ref: None,
        reasoning_credential_ref: None,
        reasoning_provider_kind: None,
        reasoning_route: Default::default(),
        reasoning_budget_ms: None,
        degradation_message: None,
        max_llm_calls_per_turn: None,
        max_reasoning_tokens: None,
        llm_timeout_ms: None,
    }
}

fn alias_conversation_from_wire_defaults(value: serde_json::Value) -> ConversationWebSocketConfig {
    match serde_json::from_value(value) {
        Ok(config) => config,
        Err(err) => {
            warn!(
                error = %err,
                "failed to materialize alias conversation wire defaults; using inert empty conversation defaults"
            );
            fallback_alias_conversation()
        }
    }
}

/// A fresh conversation config carrying the EXACT serde defaults a client would get by
/// sending `{"base_url":"","model":""}` (streaming on, allow_interruption on,
/// strip_markdown on, max_history default, ...). Built by round-tripping through serde
/// on the normal path so it tracks wire defaults, with a non-panicking fallback for
/// future schema drift. `base_url`/`model` are left empty for the alias (then the
/// client) to fill.
fn default_alias_conversation() -> ConversationWebSocketConfig {
    alias_conversation_from_wire_defaults(serde_json::json!({ "base_url": "", "model": "" }))
}

/// Splice a resolved alias definition into the four optional WS config fragments,
/// client-wins, and return what the merged config concretely became (for the ready-ack
/// echo).
///
/// `stt`/`tts`/`conversation`/`dag` are the client-supplied configs (each `None` when
/// the client omitted that block). They are mutated in place: an alias default fills a
/// field only when both (a) the alias has a value and (b) the client left it at its
/// type default / `None`.
pub fn splice_alias(
    name: &str,
    def: &AliasDefinition,
    stt: &mut Option<STTWebSocketConfig>,
    tts: &mut Option<TTSWebSocketConfig>,
    conversation: &mut Option<ConversationWebSocketConfig>,
    dag: &mut Option<DAGWebSocketConfig>,
) -> ResolvedAliasEcho {
    // ── STT ──────────────────────────────────────────────────────────────────────
    if let Some(a) = &def.stt {
        let cfg = stt.get_or_insert_with(default_alias_stt);
        if let Some(p) = &a.provider
            && cfg.provider.is_empty()
        {
            cfg.provider = p.clone();
        }
        if let Some(m) = &a.model
            && cfg.model.is_empty()
        {
            cfg.model = m.clone();
        }
        if let Some(l) = &a.language
            && cfg.language.is_empty()
        {
            cfg.language = l.clone();
        }
    }

    // ── TTS ──────────────────────────────────────────────────────────────────────
    if let Some(a) = &def.tts {
        let cfg = tts.get_or_insert_with(default_alias_tts);
        if let Some(p) = &a.provider
            && cfg.provider.is_empty()
        {
            cfg.provider = p.clone();
        }
        if let Some(v) = &a.voice
            && cfg.voice_id.is_none()
        {
            cfg.voice_id = Some(v.clone());
        }
        if let Some(m) = &a.model
            && cfg.model.is_empty()
        {
            cfg.model = m.clone();
        }
        if let Some(e) = a.emotion
            && cfg.emotion.is_none()
        {
            cfg.emotion = Some(e);
        }
        if let Some(secs) = a.request_timeout_secs
            && cfg.request_timeout.is_none()
        {
            cfg.request_timeout = Some(secs);
        }
    }

    // ── LLM / conversation loop ───────────────────────────────────────────────────
    if let Some(a) = &def.llm {
        // base_url + model are REQUIRED on ConversationWebSocketConfig, so the
        // materialize-from-alias path needs both. If the client omitted the block and
        // the alias lacks a base_url/model, we still create it from what we have (the
        // orchestrator validates required fields and emits a clear error) — but the
        // common case is the alias supplies both.
        let cfg = conversation.get_or_insert_with(default_alias_conversation);
        if let Some(u) = &a.base_url
            && cfg.base_url.is_empty()
        {
            cfg.base_url = u.clone();
        }
        if let Some(m) = &a.model
            && cfg.model.is_empty()
        {
            cfg.model = m.clone();
        }
        if let Some(s) = &a.system_prompt
            && cfg.system_prompt.is_none()
        {
            cfg.system_prompt = Some(s.clone());
        }
        if let Some(re) = a.reasoning_effort
            && cfg.reasoning_effort.is_none()
        {
            cfg.reasoning_effort = Some(re);
        }
        if let Some(rm) = &a.reasoning_model
            && cfg.reasoning_model.is_none()
        {
            cfg.reasoning_model = Some(rm.clone());
        }
        if let Some(pk) = a.provider_kind
            && cfg.provider_kind.is_none()
        {
            cfg.provider_kind = Some(pk);
        }
        // The alias names the operator's credential; a client key (or ref) of its own wins.
        if let Some(r) = &a.credential_ref
            && cfg.api_key.is_none()
            && cfg.credential_ref.is_none()
        {
            cfg.credential_ref = Some(r.clone());
        }
    }

    // ── DAG template ──────────────────────────────────────────────────────────────
    if let Some(tmpl) = &def.dag_template {
        let cfg = dag.get_or_insert_with(|| DAGWebSocketConfig {
            template: None,
            definition: None,
            enable_metrics: false,
            timeout_ms: None,
        });
        // Only fill the template when the client didn't pin one AND didn't supply an
        // inline definition (an inline definition is an explicit client override of the
        // whole DAG).
        if cfg.template.is_none() && cfg.definition.is_none() {
            cfg.template = Some(tmpl.clone());
        }
    }

    build_echo(name, def, stt, tts, conversation, dag)
}

/// Build the post-merge echo from the (now-spliced) configs, so the developer sees the
/// CONCRETE resolved providers (not the alias's raw defaults).
fn build_echo(
    name: &str,
    def: &AliasDefinition,
    stt: &Option<STTWebSocketConfig>,
    tts: &Option<TTSWebSocketConfig>,
    conversation: &Option<ConversationWebSocketConfig>,
    dag: &Option<DAGWebSocketConfig>,
) -> ResolvedAliasEcho {
    // Echo a sub-block only when the alias declared an intent for it (so an alias that
    // only fixes TTS doesn't suddenly echo a client's unrelated STT). The values come
    // from the merged config — the concrete truth.
    let echo_stt = def.stt.as_ref().and(stt.as_ref()).map(|c| AliasStt {
        provider: (!c.provider.is_empty()).then(|| c.provider.clone()),
        model: (!c.model.is_empty()).then(|| c.model.clone()),
        language: (!c.language.is_empty()).then(|| c.language.clone()),
        // The leg's server stays server-side: never echoed.
        base_url: None,
        credential_ref: None,
    });
    let echo_tts = def.tts.as_ref().and(tts.as_ref()).map(|c| AliasTts {
        provider: (!c.provider.is_empty()).then(|| c.provider.clone()),
        voice: c.voice_id.clone(),
        model: (!c.model.is_empty()).then(|| c.model.clone()),
        emotion: c.emotion,
        base_url: None,
        credential_ref: None,
        request_timeout_secs: None,
    });
    let echo_llm = def
        .llm
        .as_ref()
        .and(conversation.as_ref())
        .map(|c| AliasLlm {
            base_url: (!c.base_url.is_empty()).then(|| c.base_url.clone()),
            model: (!c.model.is_empty()).then(|| c.model.clone()),
            // The system prompt can be large/sensitive; report only presence-bearing
            // identity fields in the echo, not the full prompt text.
            system_prompt: None,
            reasoning_effort: c.reasoning_effort,
            reasoning_model: c.reasoning_model.clone(),
            provider_kind: c.provider_kind,
            // The credential's NAME is not a secret, but the echo stays minimal.
            credential_ref: None,
        });
    let echo_dag = def
        .dag_template
        .clone()
        .or_else(|| dag.as_ref().and_then(|d| d.template.clone()));

    ResolvedAliasEcho {
        name: name.to_string(),
        kind: def.kind,
        stt: echo_stt,
        tts: echo_tts,
        llm: echo_llm,
        dag_template: echo_dag,
    }
}

/// Splice a resolved alias into the `/realtime` (S2S) session fields, client-wins.
///
/// The realtime path is a single provider-agnostic session (not the cascade's separate
/// STT/TTS structs), so the alias maps:
/// - `realtime`/`agent` kind or an `stt.provider` → the realtime `provider`;
/// - `tts.voice` → the realtime `voice`; `tts.model`/`stt.model`/`llm.model` → `model`;
/// - `llm.system_prompt` → `instructions`.
///
/// Each destination fills only when the client left it `None`. Returns the echo for the
/// session ack (best-effort; the realtime ack carries the resolved provider/model/voice).
pub fn splice_alias_realtime(
    name: &str,
    def: &AliasDefinition,
    provider: &mut Option<String>,
    model: &mut Option<String>,
    voice: &mut Option<String>,
    instructions: &mut Option<String>,
) -> ResolvedAliasEcho {
    // Provider: prefer an explicit stt/tts provider in the alias (the realtime vendor),
    // falling back to nothing (the handler then uses its own default provider).
    let alias_provider = def
        .stt
        .as_ref()
        .and_then(|s| s.provider.clone())
        .or_else(|| def.tts.as_ref().and_then(|t| t.provider.clone()));
    if provider.is_none()
        && let Some(p) = alias_provider
    {
        *provider = Some(p);
    }

    // Model: tts.model → stt.model → llm.model (first the alias declares).
    let alias_model = def
        .tts
        .as_ref()
        .and_then(|t| t.model.clone())
        .or_else(|| def.stt.as_ref().and_then(|s| s.model.clone()))
        .or_else(|| def.llm.as_ref().and_then(|l| l.model.clone()));
    if model.is_none()
        && let Some(m) = alias_model
    {
        *model = Some(m);
    }

    if voice.is_none()
        && let Some(v) = def.tts.as_ref().and_then(|t| t.voice.clone())
    {
        *voice = Some(v);
    }

    if instructions.is_none()
        && let Some(s) = def.llm.as_ref().and_then(|l| l.system_prompt.clone())
    {
        *instructions = Some(s);
    }

    ResolvedAliasEcho {
        name: name.to_string(),
        kind: def.kind,
        stt: None,
        tts: Some(AliasTts {
            provider: provider.clone(),
            voice: voice.clone(),
            model: model.clone(),
            emotion: def.tts.as_ref().and_then(|t| t.emotion),
            ..Default::default()
        }),
        llm: def.llm.as_ref().map(|l| AliasLlm {
            base_url: None,
            model: model.clone(),
            system_prompt: None,
            reasoning_effort: l.reasoning_effort,
            reasoning_model: l.reasoning_model.clone(),
            provider_kind: l.provider_kind,
            credential_ref: None,
        }),
        dag_template: None,
    }
}

/// Emit a non-fatal `config_warning`-shaped advisory description for an UNKNOWN alias.
///
/// Per the plan (rule #3) an unknown alias is NOT a hard fail — the session proceeds
/// with the client's own config (or fails later on missing provider, exactly as if no
/// alias were named). The caller turns this into an
/// [`crate::handlers::ws::messages::OutgoingMessage::ConfigWarning`].
pub fn unknown_alias_message(name: &str) -> (String, String) {
    warn!(alias = %name, "Client named an unknown alias; proceeding with client config");
    (
        "alias_unknown".to_string(),
        format!(
            "Alias '{name}' is not defined in server config (aliases:). Proceeding with the \
             client-supplied config; define the alias server-side or send an explicit \
             stt_config/tts_config/conversation_config."
        ),
    )
}

#[cfg(test)]
mod tests;

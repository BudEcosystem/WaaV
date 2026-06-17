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
        turn_detection: None,
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
    }
}

/// A fresh conversation config carrying the EXACT serde defaults a client would get by
/// sending `{"base_url":"","model":""}` (streaming on, allow_interruption on,
/// strip_markdown on, max_history default, …). Built by round-tripping through serde so
/// it can never drift from the wire defaults, rather than re-listing ~20 fields here.
/// `base_url`/`model` are left empty for the alias (then the client) to fill.
fn default_alias_conversation() -> ConversationWebSocketConfig {
    serde_json::from_value(serde_json::json!({ "base_url": "", "model": "" }))
        .expect("empty base_url/model conversation config is always valid")
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
    });
    let echo_tts = def.tts.as_ref().and(tts.as_ref()).map(|c| AliasTts {
        provider: (!c.provider.is_empty()).then(|| c.provider.clone()),
        voice: c.voice_id.clone(),
        model: (!c.model.is_empty()).then(|| c.model.clone()),
        emotion: c.emotion,
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
        }),
        llm: def.llm.as_ref().map(|l| AliasLlm {
            base_url: None,
            model: model.clone(),
            system_prompt: None,
            reasoning_effort: l.reasoning_effort,
            reasoning_model: l.reasoning_model.clone(),
            provider_kind: l.provider_kind,
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

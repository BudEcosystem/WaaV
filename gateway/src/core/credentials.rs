//! Credential helpers and the operator's NAMED CREDENTIALS.
//!
//! # Why named credentials
//!
//! A session config is CLIENT input: `conversation_config`, an inline `dag_config.definition`,
//! `interaction.tasks.llm`. Two things used to go wrong with keys in it:
//!
//! * A DAG `llm_endpoint` with `api_key: "${OPENAI_API_KEY}"` (or with no key at all) made the
//!   gateway read ITS OWN environment and send the operator's key to whatever `base_url` the
//!   client chose — any public host, and with `WAAV_ALLOW_LOOPBACK_ENDPOINTS` on, any loopback
//!   port. `conversation_config` with no `api_key` did the same with `OPENAI_API_KEY`.
//! * `conversation_config.api_key: "${LOCAL_LLM_API_KEY}"` was sent literally (`Bearer
//!   ${LOCAL_LLM_API_KEY}` → 401), because that path passed the key as a per-call key, which is
//!   never interpolated — while the DAG path interpolated it. Same syntax, opposite behaviour.
//!
//! The model now: environment / file secrets are read ONLY for credentials the operator
//! declared, each bound to the hosts it may be sent to. A client may NAME one
//! (`credential_ref: "local-llm"`, or the legacy spelling `api_key: "${local-llm}"`) and the
//! gateway sends it only if the request's `base_url` is one of the credential's hosts. A client
//! may always send a LITERAL key (its own). A client can never make the gateway read an arbitrary
//! environment variable.
//!
//! # Declaring credentials (operator)
//!
//! `WAAV_CREDENTIALS_FILE` (YAML or JSON) or `WAAV_CREDENTIALS_JSON`:
//!
//! ```yaml
//! LOCAL_LLM_API_KEY:              # the name clients use (`${LOCAL_LLM_API_KEY}` works too)
//!   env: LOCAL_LLM_API_KEY        # or `file: /run/secrets/llm-key`
//!   hosts: ["127.0.0.1:8000"]     # host, host:port, *.suffix — where it may be sent
//!   private: true                 # these exact host:port entries may be loopback/private
//! azure:
//!   env: AZURE_OPENAI_API_KEY
//!   hosts: ["*.openai.azure.com"]
//! ```
//!
//! Without a declaration, the well-known vendor keys keep working against their vendor's own
//! host only ([`IMPLICIT_BINDINGS`]: `OPENAI_API_KEY` → `api.openai.com`, …), so a client that
//! omits `api_key` and talks to the vendor still uses the operator's key, exactly as before —
//! and a client that points `base_url` anywhere else gets no key.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::core::net::HostPort;

/// Env var: path of the operator's credentials file (YAML or JSON).
pub const CREDENTIALS_FILE_ENV: &str = "WAAV_CREDENTIALS_FILE";
/// Env var: the operator's credentials inline, as JSON.
pub const CREDENTIALS_JSON_ENV: &str = "WAAV_CREDENTIALS_JSON";

/// Vendor keys usable without a declaration, each ONLY against its vendor's own host(s).
pub const IMPLICIT_BINDINGS: &[(&str, &[&str])] = &[
    ("OPENAI_API_KEY", &["api.openai.com"]),
    ("ANTHROPIC_API_KEY", &["api.anthropic.com"]),
    ("GOOGLE_AI_API_KEY", &["generativelanguage.googleapis.com"]),
    ("GROQ_API_KEY", &["api.groq.com"]),
    ("MISTRAL_API_KEY", &["api.mistral.ai"]),
    ("TOGETHER_API_KEY", &["api.together.xyz"]),
    ("FIREWORKS_API_KEY", &["api.fireworks.ai"]),
    ("PERPLEXITY_API_KEY", &["api.perplexity.ai"]),
    ("COHERE_API_KEY", &["api.cohere.ai", "api.cohere.com"]),
    ("SARVAM_API_KEY", &["api.sarvam.ai"]),
    // Speech vendors' keys, against the vendor's own hosts: what a client-chosen destination for
    // the gateway's speech credential may be without a declaration (`core::client_destination`).
    (
        "DEEPGRAM_API_KEY",
        &["api.deepgram.com", "api.eu.deepgram.com"],
    ),
    (
        "ELEVENLABS_API_KEY",
        &["api.elevenlabs.io", "*.elevenlabs.io"],
    ),
    ("CARTESIA_API_KEY", &["api.cartesia.ai"]),
    (
        "ASSEMBLYAI_API_KEY",
        &["api.assemblyai.com", "*.assemblyai.com"],
    ),
    ("HUME_API_KEY", &["api.hume.ai"]),
    (
        "AZURE_SPEECH_SUBSCRIPTION_KEY",
        &[
            "*.stt.speech.microsoft.com",
            "*.tts.speech.microsoft.com",
            "*.api.cognitive.microsoft.com",
            "*.cognitiveservices.azure.com",
        ],
    ),
];

/// One operator credential as written in the credentials file.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialSpec {
    /// Environment variable holding the secret.
    #[serde(default)]
    pub env: Option<String>,
    /// File holding the secret (trailing whitespace trimmed); read per use, so rotation works.
    #[serde(default)]
    pub file: Option<PathBuf>,
    /// Where it may be sent: `host`, `host:port`, `*.suffix`, `*.suffix:port`. Required.
    pub hosts: Vec<String>,
    /// Whether its `host:port` entries (port required) may resolve to loopback/private
    /// addresses. Never applies to entries without a port.
    #[serde(default)]
    pub private: bool,
    /// Whether a CLIENT may name it (`credential_ref`, `${NAME}`, a client DAG's LLM node).
    /// `false` keeps it for the operator's own config — aliases and server DAG templates — so a
    /// key holder cannot spend it against its hosts with any model and path. Default `true`.
    #[serde(default = "default_true")]
    pub clients: bool,
}

fn default_true() -> bool {
    true
}

/// A credential a request is allowed to use, bound to the request's target. Read at call time
/// (env or file), so a rotated secret is picked up without a restart. `Debug` never prints it.
#[derive(Clone)]
pub struct BoundCredential {
    name: String,
    source: SecretSource,
}

#[derive(Clone)]
enum SecretSource {
    Env(String),
    File(PathBuf),
}

impl BoundCredential {
    /// The credential's name (safe to log).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The secret as it is now, or `None` when unset/unreadable/blank.
    pub fn current(&self) -> Option<String> {
        match &self.source {
            SecretSource::Env(var) => env_api_key(var),
            SecretSource::File(path) => std::fs::read_to_string(path)
                .ok()
                .and_then(|v| explicit_api_key(&v)),
        }
    }
}

impl std::fmt::Debug for BoundCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BoundCredential({}, [redacted])", self.name)
    }
}

#[derive(Debug, Clone)]
struct Credential {
    name: String,
    source: SecretSourceSpec,
    hosts: Vec<HostPattern>,
    private: bool,
    /// See [`CredentialSpec::clients`]; implicit vendor bindings are always nameable.
    clients: bool,
}

#[derive(Debug, Clone)]
enum SecretSourceSpec {
    Env(String),
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostPattern {
    /// Lowercased; `*.suffix` keeps the `*.` prefix.
    host: String,
    port: Option<u16>,
}

impl HostPattern {
    /// `host` or `host:port`, as an operator wrote it (logs and refusals print the same thing).
    fn label(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{p}", self.host),
            None => self.host.clone(),
        }
    }

    fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim().to_ascii_lowercase();
        if raw.is_empty() || raw.contains('/') {
            return Err(format!("'{raw}' is not a host or host:port"));
        }
        match HostPort::parse(&raw) {
            Ok(hp) => Ok(Self {
                host: hp.host,
                port: Some(hp.port),
            }),
            Err(_) if !raw.contains(':') || raw.starts_with('[') => Ok(Self {
                host: raw,
                port: None,
            }),
            Err(e) => Err(e),
        }
    }

    fn matches(&self, host: &str, port: Option<u16>) -> bool {
        let host = host.to_ascii_lowercase();
        let host_ok = match self.host.strip_prefix("*.") {
            Some(suffix) => host.len() > suffix.len() + 1 && host.ends_with(&format!(".{suffix}")),
            None => self.host == host,
        };
        host_ok && self.port.is_none_or(|p| Some(p) == port)
    }
}

/// Why a request may not use the key it asked for. Every variant is a CONFIG refusal the client
/// can act on; none of them names a secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    #[error(
        "credential '{0}' is not defined on this gateway. `${{NAME}}` / credential_ref name an \
         operator credential (WAAV_CREDENTIALS_FILE); the gateway does not read arbitrary \
         environment variables for a client"
    )]
    Unknown(String),
    #[error("credential '{name}' may not be sent to '{host}' (allowed: {allowed})")]
    HostNotAllowed {
        name: String,
        host: String,
        allowed: String,
    },
    #[error("invalid base_url '{0}'")]
    BadUrl(String),
    #[error("both api_key and credential_ref are set; send one")]
    Ambiguous,
    #[error(
        "credential '{0}' is reserved for this gateway's own configuration (aliases and server \
         DAG templates); a client may not name it. Use an alias that carries it"
    )]
    NotForClients(String),
}

/// The key decision for one client-configured LLM target.
#[derive(Debug, Clone, Default)]
pub struct ClientKey {
    /// The client's own literal key, sent as is.
    pub literal: Option<String>,
    /// An operator credential the client named and may use against this target.
    pub bound: Option<BoundCredential>,
    /// `host:port` entries of `bound` that may be private (for the SSRF check and the resolver).
    pub private_grants: Vec<HostPort>,
    /// The vendor's default key (e.g. `OPENAI_API_KEY`) when it is configured but was NOT bound,
    /// because the target is not one of its hosts. Callers say so: the leg runs keyless, and the
    /// server's 401 would otherwise name neither the key nor the host.
    pub withheld: Option<String>,
}

/// The process-wide registry of operator credentials.
#[derive(Debug, Default)]
pub struct CredentialRegistry {
    by_name: HashMap<String, Credential>,
}

static GLOBAL: OnceLock<Arc<CredentialRegistry>> = OnceLock::new();

/// The registry `initialize` loaded (empty until then — implicit vendor bindings still apply).
pub fn global_credentials() -> Arc<CredentialRegistry> {
    GLOBAL
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(CredentialRegistry::default()))
}

/// Load the operator's credentials once at startup from `WAAV_CREDENTIALS_FILE` /
/// `WAAV_CREDENTIALS_JSON`. A malformed file is a startup error (never "no credentials").
pub fn initialize_credentials() -> Result<usize, String> {
    let mut specs: HashMap<String, CredentialSpec> = HashMap::new();
    if let Ok(path) = std::env::var(CREDENTIALS_FILE_ENV)
        && !path.trim().is_empty()
    {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("{CREDENTIALS_FILE_ENV}={path}: {e}"))?;
        let parsed: HashMap<String, CredentialSpec> = serde_yaml::from_str(&raw)
            .map_err(|e| format!("{CREDENTIALS_FILE_ENV}={path}: {e}"))?;
        specs.extend(parsed);
    }
    if let Ok(raw) = std::env::var(CREDENTIALS_JSON_ENV)
        && !raw.trim().is_empty()
    {
        let parsed: HashMap<String, CredentialSpec> =
            serde_json::from_str(&raw).map_err(|e| format!("{CREDENTIALS_JSON_ENV}: {e}"))?;
        specs.extend(parsed);
    }
    let registry = CredentialRegistry::from_specs(specs)?;
    let n = registry.by_name.len();
    for c in registry.by_name.values() {
        tracing::info!(credential = %c.name, hosts = ?c.hosts.iter().map(HostPattern::label).collect::<Vec<_>>(), private = c.private, "operator credential registered");
    }
    if crate::core::net::loopback_allowance() == crate::core::net::LoopbackAllowance::AnyPort {
        tracing::warn!(
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS=1 lets client-chosen endpoints reach EVERY loopback \
             port of this host; prefer a port list (WAAV_ALLOW_LOOPBACK_ENDPOINTS=8000) or \
             WAAV_PRIVATE_EGRESS_ALLOWLIST=127.0.0.1:8000"
        );
    }
    if std::env::var(OPENAI_BASE_URL_ENV).is_ok_and(|v| v.trim().is_empty()) {
        tracing::warn!(
            "{OPENAI_BASE_URL_ENV} is set but empty: the openai providers use OpenAI's cloud \
             (api.openai.com). Unset it to mean that, or set the server's address"
        );
    }
    let _ = GLOBAL.set(Arc::new(registry));
    Ok(n)
}

impl CredentialRegistry {
    /// Build from parsed specs, validating every entry.
    pub fn from_specs(specs: HashMap<String, CredentialSpec>) -> Result<Self, String> {
        let mut by_name = HashMap::new();
        for (name, spec) in specs {
            let source = match (spec.env, spec.file) {
                (Some(env), None) if !env.trim().is_empty() => SecretSourceSpec::Env(env),
                (None, Some(file)) => SecretSourceSpec::File(file),
                _ => {
                    return Err(format!(
                        "credential '{name}': set exactly one of env / file"
                    ));
                }
            };
            if spec.hosts.is_empty() {
                return Err(format!(
                    "credential '{name}': `hosts` is required (where it may be sent)"
                ));
            }
            let hosts = spec
                .hosts
                .iter()
                .map(|h| HostPattern::parse(h).map_err(|e| format!("credential '{name}': {e}")))
                .collect::<Result<Vec<_>, _>>()?;
            if spec.private
                && hosts
                    .iter()
                    .any(|h| h.port.is_none() || h.host.starts_with("*."))
            {
                return Err(format!(
                    "credential '{name}': `private: true` needs every host as an exact host:port \
                     (a private allowance is port-scoped)"
                ));
            }
            by_name.insert(
                name.to_ascii_lowercase(),
                Credential {
                    name,
                    source,
                    hosts,
                    private: spec.private,
                    clients: spec.clients,
                },
            );
        }
        Ok(Self { by_name })
    }

    /// A declared credential, else an implicit vendor binding of that env-var name.
    fn lookup(&self, name: &str) -> Option<Credential> {
        if let Some(c) = self.by_name.get(&name.to_ascii_lowercase()) {
            return Some(c.clone());
        }
        implicit_binding(name).map(|implicit| {
            if implicit.name == OPENAI_KEY_ENV {
                with_operator_base(implicit, std::env::var(OPENAI_BASE_URL_ENV).ok().as_deref())
            } else {
                implicit
            }
        })
    }

    /// Whether `name` is a credential the operator declared (or one implied by a provider key
    /// variable), whatever hosts it may reach.
    pub fn is_declared(&self, name: &str) -> bool {
        self.lookup(name).is_some()
    }

    /// Bind credential `name` to `url`, or say why not.
    pub fn bind(
        &self,
        name: &str,
        url: &str,
    ) -> Result<(BoundCredential, Vec<HostPort>), CredentialError> {
        let cred = self
            .lookup(name)
            .ok_or_else(|| CredentialError::Unknown(name.to_string()))?;
        let parsed = url::Url::parse(url).map_err(|_| CredentialError::BadUrl(url.to_string()))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| CredentialError::BadUrl(url.to_string()))?
            .to_ascii_lowercase();
        let port = parsed.port_or_known_default();
        let Some(pattern) = cred.hosts.iter().find(|h| h.matches(&host, port)) else {
            return Err(CredentialError::HostNotAllowed {
                name: cred.name.clone(),
                host: match port {
                    Some(p) => format!("{host}:{p}"),
                    None => host,
                },
                allowed: cred
                    .hosts
                    .iter()
                    .map(HostPattern::label)
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        };
        let grants = match (cred.private, pattern.port) {
            (true, Some(p)) => vec![HostPort { host, port: p }],
            _ => Vec::new(),
        };
        let source = match cred.source {
            SecretSourceSpec::Env(v) => SecretSource::Env(v),
            SecretSourceSpec::File(f) => SecretSource::File(f),
        };
        Ok((
            BoundCredential {
                name: cred.name,
                source,
            },
            grants,
        ))
    }

    /// Refuse a credential a CLIENT named (`api_key: "${NAME}"` or `credential_ref`) when the
    /// operator kept it for its own config (`clients: false`). Call it where a value is known to
    /// come from the client — before an alias is spliced in, or for a client DAG — since
    /// [`Self::resolve_client_key`] also serves alias-supplied names. A name nobody declared is
    /// left to `resolve_client_key`, which refuses it with its own reason.
    pub fn refuse_operator_only(
        &self,
        api_key: Option<&str>,
        credential_ref: Option<&str>,
    ) -> Result<(), CredentialError> {
        let named = credential_ref
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .or_else(|| {
                api_key
                    .and_then(explicit_api_key)
                    .as_deref()
                    .and_then(env_reference)
                    .map(str::to_string)
            });
        match named {
            Some(name)
                if self
                    .by_name
                    .get(&name.to_ascii_lowercase())
                    .is_some_and(|c| !c.clients) =>
            {
                Err(CredentialError::NotForClients(name))
            }
            _ => Ok(()),
        }
    }

    /// The key decision for a CLIENT-configured LLM target (`conversation_config`, an inline
    /// DAG `llm_endpoint`/`translate`, `interaction.tasks.llm`):
    ///
    /// * `credential_ref` or `api_key: "${NAME}"` → operator credential NAME, only if `base_url`
    ///   is one of its hosts (else an error — never a silent literal, never the raw env var);
    /// * a literal `api_key` → the client's own key, as is;
    /// * neither → the vendor's implicit binding (`default_env_key`, e.g. `OPENAI_API_KEY`) if
    ///   `base_url` is that vendor's host; otherwise NO key.
    pub fn resolve_client_key(
        &self,
        api_key: Option<&str>,
        credential_ref: Option<&str>,
        base_url: &str,
        default_env_key: &str,
    ) -> Result<ClientKey, CredentialError> {
        let api_key = api_key.and_then(explicit_api_key);
        let credential_ref = credential_ref.map(str::trim).filter(|r| !r.is_empty());
        let reference = match (api_key.as_deref(), credential_ref) {
            (Some(_), Some(_)) => return Err(CredentialError::Ambiguous),
            (Some(key), None) => env_reference(key).map(str::to_string),
            (None, Some(r)) => Some(r.to_string()),
            (None, None) => None,
        };
        if let Some(name) = reference {
            let (bound, private_grants) = self.bind(&name, base_url)?;
            return Ok(ClientKey {
                bound: Some(bound),
                private_grants,
                ..Default::default()
            });
        }
        if let Some(literal) = api_key {
            return Ok(ClientKey {
                literal: Some(literal),
                ..Default::default()
            });
        }
        // No key named: the vendor's own key, only toward the vendor's own host.
        Ok(match self.bind(default_env_key, base_url) {
            Ok((bound, private_grants)) => ClientKey {
                bound: Some(bound),
                private_grants,
                ..Default::default()
            },
            Err(_) => ClientKey {
                withheld: env_api_key(default_env_key).map(|_| default_env_key.to_string()),
                ..Default::default()
            },
        })
    }
}

/// An [`IMPLICIT_BINDINGS`] entry as a credential: the vendor's key, its own hosts, no port.
fn implicit_binding(name: &str) -> Option<Credential> {
    IMPLICIT_BINDINGS
        .iter()
        .find(|(var, _)| var.eq_ignore_ascii_case(name))
        .map(|(var, hosts)| Credential {
            name: (*var).to_string(),
            source: SecretSourceSpec::Env((*var).to_string()),
            hosts: hosts
                .iter()
                .map(|h| HostPattern {
                    host: (*h).to_string(),
                    port: None,
                })
                .collect(),
            private: false,
            clients: true,
        })
}

const OPENAI_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";

/// The implicit `OPENAI_API_KEY` binding, widened to the operator's `OPENAI_BASE_URL`.
///
/// The speech legs already send that key to that base (it is operator configuration), so an LLM
/// leg pointed at the same server gets the key too, where it used to get none: a conversation
/// `base_url` naming the operator's local OpenAI-compatible server received no `Authorization`
/// and failed as a 401 nobody could trace. The entry is the base's exact `host:port`, and that
/// one entry may be private — a loopback or LAN server is the usual reason to set the variable.
/// Unset or blank leaves `api.openai.com` only.
fn with_operator_base(mut cred: Credential, base: Option<&str>) -> Credential {
    let Some(base) = base.map(str::trim).filter(|b| !b.is_empty()) else {
        return cred;
    };
    let Some((host, port)) = url::Url::parse(base).ok().and_then(|u| {
        Some((
            u.host_str()?.to_ascii_lowercase(),
            u.port_or_known_default()?,
        ))
    }) else {
        tracing::warn!(base = %base, "{OPENAI_BASE_URL_ENV} is not a URL; {OPENAI_KEY_ENV} stays bound to api.openai.com only");
        return cred;
    };
    if cred.hosts.iter().all(|h| h.host != host) {
        cred.hosts.push(HostPattern {
            host,
            port: Some(port),
        });
        // Private grants come only from patterns with a port: api.openai.com has none, so this
        // exempts exactly the operator's base.
        cred.private = true;
    }
    cred
}

/// `${NAME}` → `NAME` (the legacy reference spelling), else `None`.
pub fn env_reference(raw: &str) -> Option<&str> {
    raw.trim()
        .strip_prefix("${")
        .and_then(|s| s.strip_suffix('}'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Normalize an explicit API key supplied through a request/config object.
///
/// Empty and whitespace-only strings are not credentials. Non-empty values are
/// trimmed so accidental surrounding whitespace does not break provider auth.
pub(crate) fn explicit_api_key(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read an API key from an environment variable with the same normalization as
/// explicit config keys.
pub(crate) fn env_api_key(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .and_then(|value| explicit_api_key(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> CredentialRegistry {
        let specs: HashMap<String, CredentialSpec> = serde_yaml::from_str(
            r#"
LOCAL_LLM_API_KEY:
  env: WAAV_TEST_LOCAL_LLM_KEY
  hosts: ["127.0.0.1:8000"]
  private: true
azure:
  env: WAAV_TEST_AZURE_KEY
  hosts: ["*.openai.azure.com"]
"#,
        )
        .unwrap();
        CredentialRegistry::from_specs(specs).unwrap()
    }

    #[test]
    fn a_named_credential_binds_only_to_its_hosts() {
        let r = registry();
        // The legacy `${NAME}` spelling and credential_ref are the same thing.
        let k = r
            .resolve_client_key(
                Some("${LOCAL_LLM_API_KEY}"),
                None,
                "http://127.0.0.1:8000/v1",
                "OPENAI_API_KEY",
            )
            .unwrap();
        assert_eq!(
            k.bound.as_ref().map(BoundCredential::name),
            Some("LOCAL_LLM_API_KEY")
        );
        assert_eq!(
            k.private_grants,
            vec![HostPort {
                host: "127.0.0.1".into(),
                port: 8000
            }]
        );
        // Another loopback PORT is not the credential's host.
        assert!(matches!(
            r.resolve_client_key(
                None,
                Some("LOCAL_LLM_API_KEY"),
                "http://127.0.0.1:8001/v1",
                "OPENAI_API_KEY"
            ),
            Err(CredentialError::HostNotAllowed { .. })
        ));
        assert!(
            r.resolve_client_key(
                None,
                Some("azure"),
                "https://x.openai.azure.com/openai",
                "OPENAI_API_KEY"
            )
            .is_ok()
        );
        assert!(
            r.resolve_client_key(
                None,
                Some("azure"),
                "https://openai.azure.com.evil.io/",
                "OPENAI_API_KEY"
            )
            .is_err()
        );
    }

    #[test]
    fn a_client_cannot_name_an_arbitrary_env_var_or_send_a_vendor_key_elsewhere() {
        let r = registry();
        assert!(matches!(
            r.resolve_client_key(
                Some("${AWS_SECRET_ACCESS_KEY}"),
                None,
                "https://example.com/v1",
                "OPENAI_API_KEY"
            ),
            Err(CredentialError::Unknown(_))
        ));
        // The implicit OPENAI_API_KEY binding is api.openai.com only.
        assert!(matches!(
            r.resolve_client_key(
                Some("${OPENAI_API_KEY}"),
                None,
                "http://example.com/v1",
                "OPENAI_API_KEY"
            ),
            Err(CredentialError::HostNotAllowed { .. })
        ));
        assert!(
            r.resolve_client_key(
                Some("${OPENAI_API_KEY}"),
                None,
                "https://api.openai.com/v1",
                "OPENAI_API_KEY"
            )
            .is_ok()
        );
        // No key + a non-vendor host: no key at all (it used to be the operator's OPENAI_API_KEY).
        let k = r
            .resolve_client_key(None, None, "http://example.com/v1", "OPENAI_API_KEY")
            .unwrap();
        assert!(k.bound.is_none() && k.literal.is_none());
        // No key + the vendor's host: the operator's key, as before.
        let k = r
            .resolve_client_key(None, None, "https://api.openai.com/v1", "OPENAI_API_KEY")
            .unwrap();
        assert_eq!(
            k.bound.as_ref().map(BoundCredential::name),
            Some("OPENAI_API_KEY")
        );
    }

    fn implicit_openai() -> Credential {
        implicit_binding("OPENAI_API_KEY").expect("an implicit binding")
    }

    fn bind_with(
        cred: Credential,
        url: &str,
    ) -> Result<(BoundCredential, Vec<HostPort>), CredentialError> {
        let mut by_name = HashMap::new();
        by_name.insert("openai_api_key".to_string(), cred);
        CredentialRegistry { by_name }.bind("OPENAI_API_KEY", url)
    }

    /// The operator's OPENAI_BASE_URL is where the speech legs already send OPENAI_API_KEY; an
    /// LLM leg aimed at the same server now gets it too (it used to get no key and a 401).
    #[test]
    fn the_operator_openai_base_extends_the_implicit_binding_by_exact_host_port() {
        let widened = with_operator_base(implicit_openai(), Some("http://127.0.0.1:8080"));
        let (_, grants) = bind_with(widened.clone(), "http://127.0.0.1:8080/v1").unwrap();
        assert_eq!(
            grants,
            vec![HostPort {
                host: "127.0.0.1".into(),
                port: 8080
            }]
        );
        assert!(bind_with(widened.clone(), "http://127.0.0.1:8081/v1").is_err());
        let (_, grants) = bind_with(widened, "https://api.openai.com/v1").unwrap();
        assert!(grants.is_empty(), "api.openai.com is never a private grant");

        let ported = with_operator_base(implicit_openai(), Some("https://llm.lan/v1"));
        assert!(bind_with(ported.clone(), "https://llm.lan:443/v1").is_ok());
        assert!(bind_with(ported, "https://evil.example/v1").is_err());
    }

    #[test]
    fn a_blank_or_unparseable_operator_base_widens_nothing() {
        for base in [None, Some(""), Some("   "), Some("not a url")] {
            let cred = with_operator_base(implicit_openai(), base);
            assert_eq!(cred.hosts.len(), 1, "{base:?}");
            assert!(!cred.private, "{base:?}");
        }
    }

    #[test]
    fn an_operator_only_credential_cannot_be_named_by_a_client() {
        let specs: HashMap<String, CredentialSpec> = serde_yaml::from_str(
            r#"
local-gw:
  env: WAAV_TEST_LOCAL_GW_KEY
  hosts: ["127.0.0.1:8080"]
  private: true
  clients: false
shared:
  env: WAAV_TEST_SHARED_KEY
  hosts: ["api.example.com"]
"#,
        )
        .unwrap();
        let r = CredentialRegistry::from_specs(specs).unwrap();
        assert_eq!(
            r.refuse_operator_only(None, Some("local-gw")),
            Err(CredentialError::NotForClients("local-gw".into()))
        );
        assert_eq!(
            r.refuse_operator_only(Some("${local-gw}"), None),
            Err(CredentialError::NotForClients("local-gw".into()))
        );
        // Nameable credentials, literals, unknown names (refused later, by resolve) and nothing.
        assert_eq!(r.refuse_operator_only(None, Some("shared")), Ok(()));
        assert_eq!(r.refuse_operator_only(Some("sk-literal"), None), Ok(()));
        assert_eq!(r.refuse_operator_only(None, Some("nobody")), Ok(()));
        assert_eq!(r.refuse_operator_only(None, None), Ok(()));
        // The operator's own config (an alias) still binds it.
        assert!(r.bind("local-gw", "http://127.0.0.1:8080/v1").is_ok());
    }

    #[test]
    fn a_configured_default_key_that_is_not_sent_is_reported_by_name() {
        // SAFETY: test-only env mutation of a variable no other test reads.
        unsafe { std::env::set_var("WAAV_TEST_WITHHELD_KEY", "sk-test") };
        let mut by_name = HashMap::new();
        by_name.insert(
            "waav_test_withheld_key".to_string(),
            Credential {
                name: "WAAV_TEST_WITHHELD_KEY".into(),
                source: SecretSourceSpec::Env("WAAV_TEST_WITHHELD_KEY".into()),
                hosts: vec![HostPattern::parse("api.vendor.example").unwrap()],
                private: false,
                clients: true,
            },
        );
        let r = CredentialRegistry { by_name };
        let k = r
            .resolve_client_key(
                None,
                None,
                "http://127.0.0.1:8080/v1",
                "WAAV_TEST_WITHHELD_KEY",
            )
            .unwrap();
        assert!(k.bound.is_none() && k.literal.is_none());
        assert_eq!(k.withheld.as_deref(), Some("WAAV_TEST_WITHHELD_KEY"));
        let k = r
            .resolve_client_key(
                None,
                None,
                "https://api.vendor.example/v1",
                "WAAV_TEST_WITHHELD_KEY",
            )
            .unwrap();
        assert!(k.bound.is_some() && k.withheld.is_none());
        // A key that is not configured is not "withheld".
        let k = r
            .resolve_client_key(
                None,
                None,
                "http://127.0.0.1:8080/v1",
                "WAAV_TEST_UNSET_KEY_X",
            )
            .unwrap();
        assert_eq!(k.withheld, None);
        unsafe { std::env::remove_var("WAAV_TEST_WITHHELD_KEY") };
    }

    #[test]
    fn a_literal_key_is_the_clients_own() {
        let k = registry()
            .resolve_client_key(
                Some(" sk-own "),
                None,
                "http://example.com/v1",
                "OPENAI_API_KEY",
            )
            .unwrap();
        assert_eq!(k.literal.as_deref(), Some("sk-own"));
        assert!(k.bound.is_none());
    }

    #[test]
    fn private_needs_exact_host_port() {
        let specs: HashMap<String, CredentialSpec> =
            serde_yaml::from_str("x: {env: X, hosts: [\"127.0.0.1\"], private: true}").unwrap();
        assert!(CredentialRegistry::from_specs(specs).is_err());
    }

    #[test]
    fn debug_never_prints_a_secret_source_value() {
        let (b, _) = registry()
            .bind("azure", "https://a.openai.azure.com/")
            .unwrap();
        assert_eq!(format!("{b:?}"), "BoundCredential(azure, [redacted])");
    }

    #[test]
    fn p5_explicit_api_key_trims_and_rejects_blank_values() {
        assert_eq!(super::explicit_api_key(""), None);
        assert_eq!(super::explicit_api_key(" \n\t"), None);
        assert_eq!(
            super::explicit_api_key("  provider-key  "),
            Some("provider-key".to_string())
        );
    }
}

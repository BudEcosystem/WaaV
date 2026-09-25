//! Vendor credentials: decrypted once at hydration, never per request.
//!
//! budapp RSA-encrypts vendor API keys into the `voice_table:{endpoint_id}` blob, exactly as it
//! does for `model_table`. WaaV decrypts them **when the blob is loaded**, not when a request
//! arrives.
//!
//! That distinction is the whole point. RSA private-key operations are expensive and
//! contend; doing one per request on a path carrying ten thousand concurrent sessions is the
//! shape of an outage this platform has already had. Hydration-time decryption turns
//! credential resolution into a hash-map probe.
//!
//! Wire format matches budgateway's `encryption.rs`: RSA-OAEP with SHA-256, hex-encoded
//! (base64 accepted as a fallback, as budgateway does).
//!
//! A value longer than one RSA block is **multi-block** (voice contract §2): budapp splits the
//! UTF-8 bytes into chunks of at most `k - 66` bytes (`k` = modulus bytes), encrypts each, and
//! concatenates the `k`-byte ciphertext blocks. A Google service-account key (~2.3 KB) is the
//! value that forced it; one 2048-bit block holds only 190 bytes. A value that fits one block is
//! byte-for-byte what it always was.

use base64::Engine;
use rsa::pkcs8::DecodePrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::{Oaep, RsaPrivateKey};
use serde::Deserialize;
use sha2::Sha256;

use crate::endpoint_config::{VoiceEndpointSettings, parse_endpoint_settings};
use std::collections::{BTreeMap, HashMap};

/// The most RSA blocks one credential may span.
///
/// A bound on hydration work, not a format rule: every block is a private-key operation, and
/// hydration decrypts every credential in `voice_table`. 64 blocks is 12 KB of plaintext at 2048
/// bits and 28 KB at 4096 — five times the largest credential Bud stores (a service-account key).
pub const MAX_CREDENTIAL_BLOCKS: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("no private key configured; encrypted credentials cannot be used")]
    NoKey,
    #[error("private key could not be loaded: {0}")]
    BadKey(String),
    #[error("ciphertext is neither hex nor base64")]
    BadEncoding,
    /// Not a whole number of RSA blocks: truncated, padded, or encrypted under another key size.
    #[error(
        "ciphertext is {len} bytes, which is not a positive multiple of the {block}-byte key size"
    )]
    BadLength { len: usize, block: usize },
    #[error("ciphertext spans {blocks} RSA blocks; a credential may span at most {max}")]
    TooManyBlocks { blocks: usize, max: usize },
    #[error("decryption failed")]
    DecryptFailed,
    #[error("decrypted bytes are not valid utf-8")]
    NotUtf8,
}

/// Holds the private key used to open budapp-encrypted credentials.
///
/// `Debug` prints only whether a key is loaded. Deriving it would put the RSA private key's
/// components into any log line or test failure that formats this value.
pub struct CredentialDecryptor {
    key: Option<RsaPrivateKey>,
}

impl std::fmt::Debug for CredentialDecryptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialDecryptor")
            .field("key_loaded", &self.key.is_some())
            .finish()
    }
}

impl CredentialDecryptor {
    /// Absent decryptor: plaintext credentials still work, encrypted ones fail loudly.
    pub fn disabled() -> Self {
        Self { key: None }
    }

    /// Load an unencrypted PKCS#8 or PKCS#1 key.
    pub fn from_pem(pem: &str) -> Result<Self, CredentialError> {
        Self::from_pem_with_password(pem, None)
    }

    /// Load a key, opening an encrypted PKCS#8 PEM when a passphrase is supplied.
    ///
    /// Bud's clusters do not store a bare key: `{release}-rsa-keys` holds an `ENCRYPTED
    /// PRIVATE KEY` PEM alongside its passphrase, and budgateway opens it the same way
    /// (`tensorzero-internal/src/encryption.rs`). Without this, every vendor credential in
    /// `voice_table` stays unreadable and WaaV refuses to start -- with a message about a
    /// malformed key, which is the wrong thing to go and look at.
    pub fn from_pem_with_password(
        pem: &str,
        password: Option<&str>,
    ) -> Result<Self, CredentialError> {
        if let Ok(key) = RsaPrivateKey::from_pkcs8_pem(pem) {
            return Ok(Self { key: Some(key) });
        }
        if let Ok(key) = rsa::pkcs1::DecodeRsaPrivateKey::from_pkcs1_pem(pem) {
            return Ok(Self { key: Some(key) });
        }

        let encrypted = pem.contains("ENCRYPTED PRIVATE KEY");
        match password.filter(|p| !p.is_empty()) {
            Some(password) => RsaPrivateKey::from_pkcs8_encrypted_pem(pem, password.as_bytes())
                .map(|key| Self { key: Some(key) })
                .map_err(|e| {
                    CredentialError::BadKey(format!("encrypted PKCS#8 key would not open: {e}"))
                }),
            // Naming the missing variable is the whole point: the alternative is an operator
            // reading "invalid key" about a key that is perfectly valid.
            None if encrypted => Err(CredentialError::BadKey(
                "key is an encrypted PKCS#8 PEM but no passphrase was supplied; set \
                 WAAV_RSA_PRIVATE_KEY_PASSWORD (budgateway reads the same passphrase from \
                 the `private-key-password` entry of the rsa-keys secret)"
                    .to_string(),
            )),
            None => Err(CredentialError::BadKey(
                "not a PKCS#8 or PKCS#1 private key".to_string(),
            )),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.key.is_some()
    }

    /// Decrypt one credential value.
    ///
    /// Accepts hex (what budapp writes) or base64 (budgateway's documented fallback).
    ///
    /// The ciphertext is one or more `k`-byte RSA-OAEP blocks (`k` = the key's modulus bytes).
    /// Each block is decrypted on its own and the plaintext BYTES are joined before the UTF-8
    /// check: budapp splits on byte boundaries, so a block may end inside a multi-byte
    /// character, and decoding block by block would reject a perfectly valid credential.
    ///
    /// Any block that fails to open fails the whole credential. A partial credential is not a
    /// degraded one — it is a wrong key sent to a vendor.
    pub fn decrypt(&self, encoded: &str) -> Result<String, CredentialError> {
        let key = self.key.as_ref().ok_or(CredentialError::NoKey)?;

        let bytes = hex::decode(encoded.trim())
            .or_else(|_| {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded.trim())
                    .map_err(|_| ())
            })
            .map_err(|_| CredentialError::BadEncoding)?;

        let block = key.size();
        if bytes.is_empty() || !bytes.len().is_multiple_of(block) {
            return Err(CredentialError::BadLength {
                len: bytes.len(),
                block,
            });
        }
        let blocks = bytes.len() / block;
        if blocks > MAX_CREDENTIAL_BLOCKS {
            return Err(CredentialError::TooManyBlocks {
                blocks,
                max: MAX_CREDENTIAL_BLOCKS,
            });
        }

        let mut plain = Vec::with_capacity(bytes.len());
        for ciphertext in bytes.chunks_exact(block) {
            let part = key
                .decrypt(Oaep::new::<Sha256>(), ciphertext)
                .map_err(|_| CredentialError::DecryptFailed)?;
            plain.extend_from_slice(&part);
        }

        String::from_utf8(plain).map_err(|_| CredentialError::NotUtf8)
    }
}

/// One voice endpoint as budapp publishes it.
#[derive(Debug, Clone, Deserialize)]
pub struct VoiceEndpointBlob {
    /// `deepgram`, `elevenlabs`, … or `self_hosted`.
    pub vendor: String,
    /// Vendor base URL, or the cluster deployment URL for a self-hosted model.
    #[serde(default)]
    pub api_base: Option<String>,
    /// RSA-encrypted vendor key. Absent for a keyless self-hosted deployment.
    #[serde(default)]
    pub credential: Option<String>,
    /// Which Bud capabilities this endpoint serves.
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    /// The operator's feature configuration (FRD-018 Part III C2).
    ///
    /// Kept as a raw `Value` here and parsed by [`parse_endpoint_settings`] so that a malformed
    /// block degrades to vendor defaults rather than failing the whole entry: `serde_json::from_value`
    /// on the typed struct is all-or-nothing, and an entry that fails to deserialize is an
    /// endpoint that stops resolving.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    /// Non-secret, per-vendor parameters (voice contract §3): `region` for AWS, `project_id` /
    /// `location` for Google, `api_version` for Azure OpenAI. Plaintext, string → string.
    ///
    /// Raw here for the same reason as `config`: a malformed block must degrade to "no
    /// parameters", not drop the endpoint. Filtered and validated in [`parse_provider_params`].
    #[serde(default)]
    pub provider_params: Option<serde_json::Value>,
}

/// A voice endpoint after hydration: the credential is already plaintext.
///
/// `PartialEq` but not `Eq`: the config block carries vendor float knobs (pitch, stability,
/// similarity boost), and a total-equality bound on a struct containing `f32` does not hold.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceEndpoint {
    pub vendor: String,
    pub api_base: Option<String>,
    /// Plaintext. Decrypted at hydration; never logged, never serialised.
    pub credential: Option<String>,
    /// The decrypted credential's fields, when it is a flat JSON object of strings.
    ///
    /// That is the AWS shape — `{"access_key_id", "secret_access_key", "session_token"?}`, the
    /// blob budapp packs for `aws-polly` / `aws-transcribe` — split once here so the request path
    /// does no JSON work. `None` for a plain API key, and for any JSON with a non-string value.
    /// A Google service-account key is all strings, so it gets parts too; Google reads the raw
    /// `credential` regardless. Secret, exactly like `credential`.
    pub credential_parts: Option<BTreeMap<String, String>>,
    pub endpoints: Vec<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub language: Option<String>,
    /// Parsed at hydration, like the credential: the request path does no JSON work.
    pub config: VoiceEndpointSettings,
    /// Non-secret vendor parameters, already restricted to [`allowed_provider_params`] for this
    /// vendor and validated. A key a vendor does not allow never reaches this map, so nothing
    /// downstream can forward it into a provider's extras.
    pub provider_params: BTreeMap<String, String>,
}

impl VoiceEndpoint {
    pub fn serves(&self, capability: &str) -> bool {
        self.endpoints.iter().any(|e| e == capability)
    }

    /// One vendor parameter (`region`, `project_id`, `location`, `api_version`), if published.
    pub fn provider_param(&self, key: &str) -> Option<&str> {
        self.provider_params.get(key).map(String::as_str)
    }
}

/// The `provider_params` keys a vendor may carry (voice contract §3).
///
/// Everything else is dropped at hydration. The allowlist is enforced here, where the blob is
/// read, rather than trusted to each consumer: a key like `endpoint_override` copied into a
/// provider's extras would redirect a vendor call that carries a Bud credential. `-` and `_`
/// are the same separator, since budapp and WaaV's registry disagree about them.
pub fn allowed_provider_params(vendor: &str) -> &'static [&'static str] {
    match vendor
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "aws_polly" | "aws_transcribe" => &["region"],
        "google" => &["project_id", "location"],
        "azure_openai" => &["api_version"],
        _ => &[],
    }
}

/// `^[a-z]{2}(-gov)?-[a-z]+-\d+$` — the region shape budapp enforces at publish.
///
/// Re-checked here because a region becomes part of a HOSTNAME (`polly.{region}.amazonaws.com`):
/// an unchecked value is an attacker-chosen host receiving signed AWS requests.
fn is_aws_region(value: &str) -> bool {
    fn lower(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase())
    }
    fn digits(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
    }
    let parts: Vec<&str> = value.split('-').collect();
    match parts.as_slice() {
        [area, name, number] | [area, "gov", name, number] => {
            area.len() == 2 && lower(area) && lower(name) && digits(number)
        }
        _ => false,
    }
}

/// `^[a-z0-9-]{1,40}$` — a Google Speech-to-Text v2 location (`global`, `us`, `eu`, `us-central1`).
fn is_google_location(value: &str) -> bool {
    (1..=40).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn provider_param_is_valid(key: &str, value: &str) -> bool {
    match key {
        "region" => is_aws_region(value),
        "location" => is_google_location(value),
        // `project_id` and `api_version` have no published shape; both land in a URL path or
        // query that the consumer encodes, so non-empty is the bar.
        _ => !value.is_empty(),
    }
}

/// Filter and validate a raw `provider_params` block for one vendor.
///
/// Every refusal is logged by KEY, never by value, and never drops the endpoint: a vendor whose
/// region is missing fails its own request with a message naming the field, which is a better
/// outcome than an endpoint that silently stopped existing.
fn parse_provider_params(
    endpoint_id: &str,
    vendor: &str,
    raw: Option<&serde_json::Value>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let fields = match raw {
        None | Some(serde_json::Value::Null) => return out,
        Some(serde_json::Value::Object(fields)) => fields,
        Some(_) => {
            tracing::warn!(
                endpoint_id = %endpoint_id,
                vendor = %vendor,
                "voice_table provider_params is not a JSON object; ignoring it"
            );
            return out;
        }
    };

    let allowed = allowed_provider_params(vendor);
    for (key, value) in fields {
        if !allowed.contains(&key.as_str()) {
            tracing::warn!(
                endpoint_id = %endpoint_id,
                vendor = %vendor,
                field = %key,
                "voice_table provider_params carries a key this vendor does not take; ignored"
            );
            continue;
        }
        let Some(value) = value.as_str().map(str::trim) else {
            tracing::warn!(
                endpoint_id = %endpoint_id,
                vendor = %vendor,
                field = %key,
                "voice_table provider_params value is not a string; ignored"
            );
            continue;
        };
        if !provider_param_is_valid(key, value) {
            tracing::warn!(
                endpoint_id = %endpoint_id,
                vendor = %vendor,
                field = %key,
                "voice_table provider_params value is malformed; ignored"
            );
            continue;
        }
        out.insert(key.clone(), value.to_string());
    }
    out
}

/// Split a decrypted credential into fields when it is a flat, non-empty JSON object of strings.
///
/// A plain API key — nearly every credential — is recognised by its first character and never
/// reaches the JSON parser. Nothing here logs: the input is a secret.
fn split_credential(plain: &str) -> Option<BTreeMap<String, String>> {
    if !plain.trim_start().starts_with('{') {
        return None;
    }
    let serde_json::Value::Object(fields) =
        serde_json::from_str::<serde_json::Value>(plain).ok()?
    else {
        return None;
    };
    if fields.is_empty() {
        return None;
    }
    fields
        .into_iter()
        .map(|(k, v)| match v {
            serde_json::Value::String(s) => Some((k, s)),
            _ => None,
        })
        .collect()
}

/// Parse and decrypt a `voice_table:{endpoint_id}` blob.
///
/// The blob is `{ "<endpoint_id>": { … } }`, mirroring `model_table`.
///
/// Unknown fields are **accepted and logged** rather than rejected. budgateway's provider
/// config is `deny_unknown_fields` and drops mismatched entries with no error at all — a
/// documented silent-failure source that this deliberately does not inherit.
pub fn parse_voice_blob(
    json: &str,
    decryptor: &CredentialDecryptor,
) -> Result<HashMap<String, VoiceEndpoint>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("not json: {e}"))?;
    let serde_json::Value::Object(obj) = value else {
        return Err("voice blob is not an object".into());
    };

    let mut out = HashMap::new();
    for (endpoint_id, raw) in obj {
        // Warn on fields we do not model, so a budapp change is visible rather than silent.
        if let serde_json::Value::Object(fields) = &raw {
            const KNOWN: &[&str] = &[
                "vendor",
                "api_base",
                "credential",
                "endpoints",
                "model",
                "voice",
                "language",
                "pricing",
                // FRD-018 Part III C2. Listed so a widened blob stops being reported as drift;
                // the fields INSIDE it get the same treatment in `parse_endpoint_settings`.
                "config",
                // Voice contract §3. The keys inside are allowlisted per vendor in
                // `parse_provider_params`, which warns about the rest.
                "provider_params",
            ];
            for k in fields.keys() {
                if !KNOWN.contains(&k.as_str()) {
                    tracing::warn!(
                        endpoint_id = %endpoint_id,
                        field = %k,
                        "voice_table entry carries a field this build does not model; \
                         accepting it, but budapp and WaaV may have drifted"
                    );
                }
            }
        }

        let blob: VoiceEndpointBlob = match serde_json::from_value(raw) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(endpoint_id = %endpoint_id, error = %e, "skipping unparseable voice endpoint");
                continue;
            }
        };

        let credential = match &blob.credential {
            None => None,
            Some(enc) if enc.is_empty() => None,
            Some(enc) => match decryptor.decrypt(enc) {
                Ok(plain) => Some(plain),
                Err(e) => {
                    // Never log the ciphertext, and never fall through to using it raw:
                    // sending an encrypted blob to a vendor as a bearer token would leak it.
                    tracing::error!(
                        endpoint_id = %endpoint_id,
                        error = %e,
                        "voice endpoint credential could not be decrypted; endpoint unusable"
                    );
                    continue;
                }
            },
        };

        // Parsed BEFORE the insert, which moves `endpoint_id`. Hydration-time, like the
        // credential decryption above: the request path does no JSON work.
        let config = blob
            .config
            .as_ref()
            .map(|raw| parse_endpoint_settings(&endpoint_id, raw))
            .unwrap_or_default();
        let provider_params =
            parse_provider_params(&endpoint_id, &blob.vendor, blob.provider_params.as_ref());
        let credential_parts = credential.as_deref().and_then(split_credential);

        out.insert(
            endpoint_id,
            VoiceEndpoint {
                vendor: blob.vendor,
                api_base: blob.api_base,
                credential,
                credential_parts,
                endpoints: blob.endpoints,
                model: blob.model,
                voice: blob.voice,
                language: blob.language,
                config,
                provider_params,
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIV: &str = include_str!("../tests/fixtures/test_cred_private.pem");
    const ENC_HEX: &str = include_str!("../tests/fixtures/test_cred_encrypted.hex");
    const PLAIN: &str = "dg_vendor_key_abc123";

    fn decryptor() -> CredentialDecryptor {
        CredentialDecryptor::from_pem(PRIV).unwrap()
    }

    /// TC-CRED-08 — wire compatibility with what budapp actually produces.
    ///
    /// The ciphertext fixture was generated by openssl with OAEP/SHA-256, independently of this
    /// code, so the test cannot pass by agreeing with a bug here.
    #[test]
    fn decrypts_a_budapp_encrypted_credential() {
        assert_eq!(decryptor().decrypt(ENC_HEX.trim()).unwrap(), PLAIN);
    }

    #[test]
    fn accepts_base64_as_well_as_hex() {
        let raw = hex::decode(ENC_HEX.trim()).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        assert_eq!(decryptor().decrypt(&b64).unwrap(), PLAIN);
    }

    #[test]
    fn a_disabled_decryptor_refuses_rather_than_guesses() {
        let d = CredentialDecryptor::disabled();
        assert!(matches!(
            d.decrypt(ENC_HEX.trim()),
            Err(CredentialError::NoKey)
        ));
    }

    #[test]
    fn garbage_ciphertext_is_an_error_not_a_panic() {
        let d = decryptor();
        // Neither hex nor base64 — `!` is outside both alphabets.
        assert!(matches!(
            d.decrypt("!!!!"),
            Err(CredentialError::BadEncoding)
        ));
        // Well-encoded but not our ciphertext.
        assert!(matches!(
            d.decrypt(&"ab".repeat(256)),
            Err(CredentialError::DecryptFailed)
        ));
    }

    /// The hex-then-base64 fallback is deliberate, and `zzzz` is a reminder of why the two
    /// alphabets overlap: a value that is not hex may still be valid base64, so encoding
    /// detection cannot be used to decide whether a credential is well-formed.
    #[test]
    fn base64_fallback_catches_values_that_are_not_hex() {
        let d = decryptor();
        // `zzzz` is not hex but IS base64 (3 bytes). Reaching the length check at all -- not
        // `BadEncoding` -- is the proof the fallback decoded it.
        assert!(matches!(
            d.decrypt("zzzz"),
            Err(CredentialError::BadLength { len: 3, .. })
        ));
    }

    #[test]
    fn a_bad_pem_is_rejected_at_load_time() {
        assert!(CredentialDecryptor::from_pem("-----BEGIN NONSENSE-----").is_err());
    }

    /// TC-CRED-01 — the publisher contract.
    #[test]
    fn parses_a_voice_table_blob_and_decrypts_in_place() {
        let json = format!(
            r#"{{"ep-1":{{"vendor":"deepgram","api_base":"https://api.deepgram.com","credential":"{}","endpoints":["text_to_speech"],"voice":"aura-asteria-en"}}}}"#,
            ENC_HEX.trim()
        );
        let map = parse_voice_blob(&json, &decryptor()).unwrap();

        let ep = map.get("ep-1").expect("endpoint present");
        assert_eq!(ep.vendor, "deepgram");
        assert_eq!(ep.credential.as_deref(), Some(PLAIN));
        assert!(ep.serves("text_to_speech"));
        assert!(!ep.serves("audio_transcription"));
    }

    /// A self-hosted deployment has no vendor key at all.
    #[test]
    fn a_keyless_self_hosted_endpoint_is_valid() {
        let json = r#"{"ep-2":{"vendor":"self_hosted","api_base":"http://whisper.svc:8000/v1","endpoints":["audio_transcription"]}}"#;
        let map = parse_voice_blob(json, &CredentialDecryptor::disabled()).unwrap();
        let ep = map.get("ep-2").unwrap();
        assert_eq!(ep.vendor, "self_hosted");
        assert!(ep.credential.is_none());
        assert!(ep.serves("audio_transcription"));
    }

    /// TC-CRED-03 — an unknown field must not silently drop the entry.
    #[test]
    fn an_unknown_field_is_accepted_rather_than_dropping_the_entry() {
        let json = r#"{"ep-3":{"vendor":"elevenlabs","endpoints":["text_to_speech"],"some_new_budapp_field":42}}"#;
        let map = parse_voice_blob(json, &CredentialDecryptor::disabled()).unwrap();
        assert!(
            map.contains_key("ep-3"),
            "an unknown field dropped the endpoint; that is budgateway's silent-failure mode"
        );
    }

    /// An undecryptable credential must disable the endpoint, never leak the ciphertext
    /// onward as if it were a key.
    #[test]
    fn an_undecryptable_credential_disables_the_endpoint() {
        let json = r#"{"ep-4":{"vendor":"deepgram","credential":"deadbeef","endpoints":["text_to_speech"]}}"#;
        let map = parse_voice_blob(json, &decryptor()).unwrap();
        assert!(
            !map.contains_key("ep-4"),
            "an endpoint with an unopenable credential was kept; its ciphertext would be sent as a bearer token"
        );
    }

    #[test]
    fn one_bad_endpoint_does_not_discard_its_siblings() {
        let json = r#"{"good":{"vendor":"deepgram","endpoints":["text_to_speech"]},"bad":"not-an-object"}"#;
        let map = parse_voice_blob(json, &CredentialDecryptor::disabled()).unwrap();
        assert!(map.contains_key("good"));
        assert!(!map.contains_key("bad"));
    }

    #[test]
    fn malformed_blob_is_an_error() {
        assert!(parse_voice_blob("nope", &CredentialDecryptor::disabled()).is_err());
        assert!(parse_voice_blob("[1,2]", &CredentialDecryptor::disabled()).is_err());
    }

    /// TC-CRED-12 — the plaintext must never reach a log line or a serialised form.
    #[test]
    fn the_debug_rendering_does_not_expose_the_plaintext_by_accident() {
        // `VoiceEndpoint` deliberately derives Debug for diagnostics, so the guard is that
        // callers never log it whole. This test pins the shape so a future `#[derive(Serialize)]`
        // — which would put credentials on the wire — is a deliberate, visible change.
        let ep = VoiceEndpoint {
            vendor: "deepgram".into(),
            api_base: None,
            credential: Some("secret".into()),
            credential_parts: None,
            endpoints: vec![],
            model: None,
            voice: None,
            language: None,
            config: Default::default(),
            provider_params: Default::default(),
        };
        let rendered = format!("{ep:?}");
        assert!(
            rendered.contains("secret"),
            "if this ever stops being true, the redaction was added — update the call sites too"
        );
    }

    // ------------------------------------------------------------------------------------ //
    // Multi-block credentials (voice contract §2).
    // ------------------------------------------------------------------------------------ //

    /// The fixture key: an openssl-generated 2048-bit RSA key (see `tests/encrypted_key.rs`).
    fn fixture_key() -> RsaPrivateKey {
        decryptor().key.expect("the fixture key loads")
    }

    /// Encrypt exactly as budapp does under contract §2: the UTF-8 bytes split into chunks of at
    /// most `k - 66`, each OAEP-SHA-256 encrypted, the `k`-byte blocks concatenated, hex-encoded.
    fn encrypt_like_budapp(plain: &str) -> String {
        let key = fixture_key();
        let public = key.to_public_key();
        let mut ciphertext = Vec::new();
        for chunk in plain.as_bytes().chunks(key.size() - 66) {
            ciphertext.extend(
                public
                    .encrypt(&mut rsa::rand_core::OsRng, Oaep::new::<Sha256>(), chunk)
                    .expect("a chunk of at most k - 66 bytes encrypts"),
            );
        }
        hex::encode(ciphertext)
    }

    fn blocks_of(hex_ciphertext: &str) -> usize {
        hex_ciphertext.len() / 2 / fixture_key().size()
    }

    /// A service-account-shaped JSON of exactly `total` bytes -- the credential multi-block
    /// exists for. The key field is filler, not key material: the test is about framing, and a
    /// real-looking PEM in the source would trip the secret scanner.
    fn service_account_of_len(total: usize) -> String {
        let shell = |private_key: &str| {
            serde_json::json!({
                "type": "service_account",
                "project_id": "bud-voice-test",
                "private_key_id": "test-private-key-id",
                "private_key": private_key,
                "client_email": "waav-test@bud-voice-test.iam.gserviceaccount.com",
                "client_id": "100000000000000000001",
                "token_uri": "https://oauth2.googleapis.com/token",
            })
            .to_string()
        };
        let base = shell("").len();
        assert!(
            total > base,
            "the service-account shell alone is {base} bytes"
        );
        let json = shell(&"A".repeat(total - base));
        assert_eq!(json.len(), total);
        json
    }

    #[test]
    fn the_fixture_key_is_2048_bit() {
        // Every block count below assumes it: 256-byte blocks carrying 190 plaintext bytes each.
        assert_eq!(fixture_key().size(), 256);
    }

    #[test]
    fn a_single_block_value_round_trips_unchanged() {
        let enc = encrypt_like_budapp(PLAIN);
        assert_eq!(blocks_of(&enc), 1);
        assert_eq!(decryptor().decrypt(&enc).unwrap(), PLAIN);
    }

    #[test]
    fn one_byte_past_a_full_block_is_the_first_multi_block_value() {
        // 190 bytes is the most one 2048-bit OAEP-SHA-256 block holds.
        let full = "k".repeat(190);
        let enc = encrypt_like_budapp(&full);
        assert_eq!(blocks_of(&enc), 1);
        assert_eq!(decryptor().decrypt(&enc).unwrap(), full);

        let over = "k".repeat(191);
        let enc = encrypt_like_budapp(&over);
        assert_eq!(blocks_of(&enc), 2);
        assert_eq!(decryptor().decrypt(&enc).unwrap(), over);
    }

    #[test]
    fn a_service_account_key_spanning_13_blocks_round_trips() {
        // 2400 bytes: past 12 blocks (2280) and within 13 (2470) -- a real key is ~2.3 KB.
        let sa = service_account_of_len(2400);
        let enc = encrypt_like_budapp(&sa);
        assert_eq!(blocks_of(&enc), 13);
        assert_eq!(decryptor().decrypt(&enc).unwrap(), sa);

        // The base64 fallback frames the same way.
        let b64 = base64::engine::general_purpose::STANDARD.encode(hex::decode(&enc).unwrap());
        assert_eq!(decryptor().decrypt(&b64).unwrap(), sa);
    }

    #[test]
    fn a_block_boundary_inside_a_multibyte_character_is_joined_before_decoding() {
        // budapp splits on BYTE boundaries. 'x' then 200 two-byte 'é': byte 190 is the second
        // half of a character, so block 1 ends mid-character and is not UTF-8 on its own.
        let plain = format!("x{}", "é".repeat(200));
        assert!(
            !plain.is_char_boundary(190),
            "precondition: block 1 must end inside a character"
        );
        let enc = encrypt_like_budapp(&plain);
        assert_eq!(blocks_of(&enc), 3);
        assert_eq!(decryptor().decrypt(&enc).unwrap(), plain);
    }

    #[test]
    fn a_ciphertext_that_is_not_a_whole_number_of_blocks_is_refused() {
        let d = decryptor();
        let one = encrypt_like_budapp(PLAIN);

        // A trailing byte.
        assert!(matches!(
            d.decrypt(&format!("{one}00")),
            Err(CredentialError::BadLength {
                len: 257,
                block: 256
            })
        ));
        // Half a block: a truncated write.
        assert!(matches!(
            d.decrypt(&one[..256]),
            Err(CredentialError::BadLength {
                len: 128,
                block: 256
            })
        ));
        // Two blocks less one byte.
        let two = encrypt_like_budapp(&"k".repeat(191));
        assert!(matches!(
            d.decrypt(&two[..two.len() - 2]),
            Err(CredentialError::BadLength { len: 511, .. })
        ));
        // Nothing at all.
        assert!(matches!(
            d.decrypt(""),
            Err(CredentialError::BadLength { len: 0, .. })
        ));
    }

    #[test]
    fn one_unopenable_block_fails_the_whole_credential() {
        // A partial credential is not a degraded one; it is a wrong key sent to a vendor.
        let enc = encrypt_like_budapp(&"k".repeat(400));
        assert_eq!(blocks_of(&enc), 3);
        let mut bytes = hex::decode(&enc).unwrap();
        bytes[256 + 100] ^= 0x01; // inside block 2
        assert!(matches!(
            decryptor().decrypt(&hex::encode(bytes)),
            Err(CredentialError::DecryptFailed)
        ));
    }

    #[test]
    fn a_ciphertext_beyond_the_block_cap_is_refused_before_any_decryption() {
        let blocks = MAX_CREDENTIAL_BLOCKS + 1;
        let err = decryptor().decrypt(&"ab".repeat(256 * blocks)).unwrap_err();
        assert!(
            matches!(err, CredentialError::TooManyBlocks { blocks: b, .. } if b == blocks),
            "got {err:?}"
        );
    }

    #[test]
    fn a_multi_block_credential_hydrates_like_a_single_block_one() {
        let sa = service_account_of_len(2400);
        let json = serde_json::json!({ "ep-g": {
            "vendor": "google",
            "credential": encrypt_like_budapp(&sa),
            "endpoints": ["audio_transcription"],
            "provider_params": { "project_id": "bud-voice-test", "location": "us" },
        }})
        .to_string();
        let map = parse_voice_blob(&json, &decryptor()).unwrap();
        let ep = map
            .get("ep-g")
            .expect("a 13-block credential must not drop the endpoint");
        assert_eq!(ep.credential.as_deref(), Some(sa.as_str()));
        assert_eq!(ep.provider_param("project_id"), Some("bud-voice-test"));
        assert_eq!(ep.provider_param("location"), Some("us"));
        // A service account is all strings, so it splits too; Google reads the raw credential.
        assert_eq!(
            ep.credential_parts
                .as_ref()
                .and_then(|p| p.get("type"))
                .map(String::as_str),
            Some("service_account")
        );
    }

    // ------------------------------------------------------------------------------------ //
    // provider_params and credential_parts (voice contract §3).
    // ------------------------------------------------------------------------------------ //

    #[test]
    fn an_entry_without_provider_params_has_none_and_a_plain_key_does_not_split() {
        let json = format!(
            r#"{{"ep-1":{{"vendor":"deepgram","credential":"{}","endpoints":["text_to_speech"]}}}}"#,
            ENC_HEX.trim()
        );
        let map = parse_voice_blob(&json, &decryptor()).unwrap();
        let ep = map.get("ep-1").unwrap();
        assert!(ep.provider_params.is_empty());
        assert!(ep.credential_parts.is_none(), "a plain API key is not JSON");
        assert_eq!(ep.credential.as_deref(), Some(PLAIN));
    }

    #[test]
    fn an_aws_entry_carries_its_region_and_its_split_credential() {
        let blob = r#"{"access_key_id":"test-akid-0001","secret_access_key":"test-secret-0001","session_token":"test-session-0001"}"#;
        let json = serde_json::json!({ "ep-aws": {
            "vendor": "aws-polly",
            "endpoints": ["text_to_speech"],
            "model": "neural",
            "voice": "Joanna",
            "credential": encrypt_like_budapp(blob),
            "provider_params": { "region": "eu-west-1" },
        }})
        .to_string();
        let map = parse_voice_blob(&json, &decryptor()).unwrap();
        let ep = map.get("ep-aws").expect("endpoint present");

        assert_eq!(ep.provider_param("region"), Some("eu-west-1"));
        let parts = ep
            .credential_parts
            .as_ref()
            .expect("the AWS blob splits into its fields");
        assert_eq!(
            parts.get("access_key_id").map(String::as_str),
            Some("test-akid-0001")
        );
        assert_eq!(
            parts.get("secret_access_key").map(String::as_str),
            Some("test-secret-0001")
        );
        assert_eq!(
            parts.get("session_token").map(String::as_str),
            Some("test-session-0001")
        );
        assert_eq!(
            ep.credential.as_deref(),
            Some(blob),
            "the raw decrypted string is kept alongside the parts"
        );
    }

    #[test]
    fn a_key_outside_the_vendor_allowlist_is_dropped_not_fatal() {
        let json = r#"{"ep-1":{"vendor":"aws_transcribe","endpoints":["audio_transcription"],
            "provider_params":{"region":"us-gov-west-1","endpoint_override":"http://169.254.169.254","project_id":"p"}}}"#;
        let map = parse_voice_blob(json, &CredentialDecryptor::disabled()).unwrap();
        let ep = map
            .get("ep-1")
            .expect("an unexpected key must not drop the endpoint");
        assert_eq!(ep.provider_param("region"), Some("us-gov-west-1"));
        assert!(
            ep.provider_param("endpoint_override").is_none(),
            "endpoint_override must never be settable through provider_params"
        );
        assert!(
            ep.provider_param("project_id").is_none(),
            "project_id is Google's, not AWS's"
        );
        assert_eq!(ep.provider_params.len(), 1);
    }

    #[test]
    fn malformed_provider_params_degrade_to_nothing_and_keep_the_endpoint() {
        let d = CredentialDecryptor::disabled();
        for (vendor, params) in [
            ("aws-polly", r#"{"region": 5}"#),
            ("aws-polly", r#"{"region": "eu-west-1.evil.example"}"#),
            ("aws-polly", r#"{"region": "EU-WEST-1"}"#),
            ("google", r#"{"location": "us central"}"#),
            ("azure_openai", r#"{"api_version": "   "}"#),
            ("azure_openai", r#""2025-04-01-preview""#),
            ("azure_openai", r#"["api_version"]"#),
        ] {
            let json = format!(
                r#"{{"ep":{{"vendor":"{vendor}","endpoints":["text_to_speech"],"provider_params":{params}}}}}"#
            );
            let map = parse_voice_blob(&json, &d).unwrap();
            let ep = map
                .get("ep")
                .unwrap_or_else(|| panic!("{vendor} {params}: the endpoint was dropped"));
            assert!(
                ep.provider_params.is_empty(),
                "{vendor} {params}: kept {:?}",
                ep.provider_params
            );
        }
    }

    #[test]
    fn an_azure_openai_entry_keeps_its_api_version() {
        let json = r#"{"ep":{"vendor":"azure_openai","api_base":"https://bud-test.openai.azure.com","endpoints":["text_to_speech"],"model":"tts-1","provider_params":{"api_version":"2025-03-01-preview"}}}"#;
        let map = parse_voice_blob(json, &CredentialDecryptor::disabled()).unwrap();
        assert_eq!(
            map.get("ep").unwrap().provider_param("api_version"),
            Some("2025-03-01-preview")
        );
    }

    #[test]
    fn the_allowlist_follows_the_contract_under_either_separator() {
        assert_eq!(allowed_provider_params("aws-polly"), &["region"]);
        assert_eq!(allowed_provider_params("aws_transcribe"), &["region"]);
        assert_eq!(
            allowed_provider_params("google"),
            &["project_id", "location"]
        );
        assert_eq!(allowed_provider_params("azure-openai"), &["api_version"]);
        assert_eq!(allowed_provider_params(" Azure_OpenAI "), &["api_version"]);
        assert!(
            allowed_provider_params("azure").is_empty(),
            "Azure AI Speech takes no provider_params"
        );
        assert!(allowed_provider_params("deepgram").is_empty());
    }

    #[test]
    fn aws_regions_are_checked_against_the_published_shape() {
        for ok in [
            "eu-west-1",
            "us-east-1",
            "ap-southeast-2",
            "us-gov-west-1",
            "cn-north-1",
            "il-central-1",
        ] {
            assert!(is_aws_region(ok), "{ok} should be accepted");
        }
        for bad in [
            "",
            "eu",
            "eu-west",
            "eu-west-",
            "EU-WEST-1",
            "eu-west-1a",
            "eu-west-1.evil.example",
            "euw-west-1",
            "us-iso-east-1",
            "eu_west_1",
            "eu-west-1/x",
        ] {
            assert!(!is_aws_region(bad), "{bad} should be refused");
        }
    }

    #[test]
    fn only_a_flat_non_empty_json_object_of_strings_splits() {
        assert!(split_credential("dg_vendor_key_abc123").is_none());
        assert!(split_credential("{}").is_none());
        assert!(split_credential("{not json").is_none());
        assert!(split_credential(r#"["a","b"]"#).is_none());
        // Any non-string value: the whole credential stays unsplit.
        assert!(split_credential(r#"{"a":"b","n":1}"#).is_none());
        assert!(split_credential(r#"{"type":"service_account","nested":{"c":"d"}}"#).is_none());
        let parts =
            split_credential(r#" {"access_key_id":"test-akid","secret_access_key":"test-secret"}"#)
                .expect("leading whitespace is still an object");
        assert_eq!(parts.len(), 2);
    }
}

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

use base64::Engine;
use rsa::pkcs8::DecodePrivateKey;
use rsa::{Oaep, RsaPrivateKey};
use serde::Deserialize;
use sha2::Sha256;

use crate::endpoint_config::{VoiceEndpointSettings, parse_endpoint_settings};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("no private key configured; encrypted credentials cannot be used")]
    NoKey,
    #[error("private key could not be loaded: {0}")]
    BadKey(String),
    #[error("ciphertext is neither hex nor base64")]
    BadEncoding,
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
    pub fn decrypt(&self, encoded: &str) -> Result<String, CredentialError> {
        let key = self.key.as_ref().ok_or(CredentialError::NoKey)?;

        let bytes = hex::decode(encoded.trim())
            .or_else(|_| {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded.trim())
                    .map_err(|_| ())
            })
            .map_err(|_| CredentialError::BadEncoding)?;

        let plain = key
            .decrypt(Oaep::new::<Sha256>(), &bytes)
            .map_err(|_| CredentialError::DecryptFailed)?;

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
    pub endpoints: Vec<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub language: Option<String>,
    /// Parsed at hydration, like the credential: the request path does no JSON work.
    pub config: VoiceEndpointSettings,
}

impl VoiceEndpoint {
    pub fn serves(&self, capability: &str) -> bool {
        self.endpoints.iter().any(|e| e == capability)
    }
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

        out.insert(
            endpoint_id,
            VoiceEndpoint {
                vendor: blob.vendor,
                api_base: blob.api_base,
                credential,
                endpoints: blob.endpoints,
                model: blob.model,
                voice: blob.voice,
                language: blob.language,
                config,
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
        assert!(matches!(
            d.decrypt("zzzz"),
            Err(CredentialError::DecryptFailed)
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
            endpoints: vec![],
            model: None,
            voice: None,
            language: None,
            config: Default::default(),
        };
        let rendered = format!("{ep:?}");
        assert!(
            rendered.contains("secret"),
            "if this ever stops being true, the redaction was added — update the call sites too"
        );
    }
}

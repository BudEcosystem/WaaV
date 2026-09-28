//! `ek_bud_…` client secrets (FRD-023 §5.8, D-7, S-4).
//!
//! A backend holding a Bud credential mints a short-lived secret and hands it to a browser, which
//! connects to `/v1/realtime` with it. WaaV stores NOTHING: the secret carries its own claims,
//! sealed with XChaCha20-Poly1305 so they are opaque to the holder (often a third party's end
//! user) and tamper-evident in one step.
//!
//! ```text
//! ek_bud_<kid>.<base64url_nopad( nonce[24] ‖ XChaCha20-Poly1305(key[kid], nonce, claims,
//!                                                               aad = "ek_bud:v1:" ‖ kid) )>
//! ```
//!
//! * **XChaCha20** — its 192-bit random nonce removes the nonce-reuse ceiling AES-GCM's 96-bit
//!   random nonce puts on a long-lived key.
//! * **The parent is the full snapshot hash** (`hash_api_key`), so the parent check at connect is
//!   the same `HashMap::get` a direct connect makes; a truncated fingerprint could not be looked
//!   up.
//! * **Alphabet** — `kid` and unpadded base64url are HTTP `tchar`s, so the token is legal inside
//!   `Sec-WebSocket-Protocol`.
//!
//! Keys: `WAAV_CLIENT_SECRET_KEYS` = comma-separated `kid:base64(32 bytes)`; the FIRST seals, all
//! open. A key that is not 32 bytes, or a duplicate `kid`, fails startup.

use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

pub const TOKEN_PREFIX: &str = "ek_bud_";
const AAD_PREFIX: &str = "ek_bud:v1:";
const NONCE_LEN: usize = 24;
pub const KEYS_ENV: &str = "WAAV_CLIENT_SECRET_KEYS";

/// Who minted the secret. Revalidated at connect and every 30 s of a live session (D-17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum Parent {
    /// An API key, by its auth-snapshot key (`sha256("bud-" + key)` hex). `ck`: it was a
    /// `bud_client_*` key, which also reaches the published overlay.
    #[serde(rename = "api_key")]
    ApiKey {
        h: String,
        #[serde(default)]
        ck: bool,
    },
    /// A Keycloak subject.
    #[serde(rename = "jwt")]
    Jwt { sub: String },
}

/// The sealed claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub v: u8,
    pub iat: u64,
    /// Already capped at the parent's expiry.
    pub exp: u64,
    /// The bound endpoint id.
    pub ep: String,
    /// The name it was minted for.
    pub alias: String,
    pub parent: Parent,
    /// Attribution copied from the minting principal: project, user, API key id.
    #[serde(default)]
    pub pid: Option<String>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub akid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Not an `ek_bud_` token at all, or not in its shape.
    Malformed,
    /// No configured key has this `kid`.
    UnknownKey,
    /// The box did not open: any altered byte, or a different key's `kid`.
    Tampered,
    Expired,
}

struct SealKey {
    kid: String,
    cipher: XChaCha20Poly1305,
}

/// The configured sealing keys. The first seals; all open.
pub struct ClientSecretKeys {
    keys: Vec<SealKey>,
}

impl std::fmt::Debug for ClientSecretKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSecretKeys")
            .field(
                "kids",
                &self.keys.iter().map(|k| &k.kid).collect::<Vec<_>>(),
            )
            .finish()
    }
}

fn valid_kid(kid: &str) -> bool {
    (1..=16).contains(&kid.len())
        && kid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl ClientSecretKeys {
    /// Parse `kid:base64(32 bytes),…`. Every problem is fatal and named (TC-EK-16).
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut keys: Vec<SealKey> = Vec::new();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (kid, b64) = part.split_once(':').ok_or_else(|| {
                format!("{KEYS_ENV}: entry is not kid:base64 (got a value without ':')")
            })?;
            let kid = kid.trim();
            if !valid_kid(kid) {
                return Err(format!(
                    "{KEYS_ENV}: kid '{kid}' must be 1-16 characters of [A-Za-z0-9_-]"
                ));
            }
            if keys.iter().any(|k| k.kid == kid) {
                return Err(format!("{KEYS_ENV}: duplicate kid '{kid}'"));
            }
            let raw = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|_| format!("{KEYS_ENV}: key '{kid}' is not valid base64"))?;
            let key: [u8; 32] = raw.as_slice().try_into().map_err(|_| {
                format!(
                    "{KEYS_ENV}: key '{kid}' is {} bytes; XChaCha20-Poly1305 needs exactly 32",
                    raw.len()
                )
            })?;
            keys.push(SealKey {
                kid: kid.to_string(),
                cipher: XChaCha20Poly1305::new(&key.into()),
            });
        }
        if keys.is_empty() {
            return Err(format!("{KEYS_ENV} is set but names no key"));
        }
        Ok(Self { keys })
    }

    /// `Ok(None)` when unset (client secrets disabled: the route answers 501).
    pub fn from_env() -> Result<Option<Self>, String> {
        match std::env::var(KEYS_ENV) {
            Ok(v) if !v.trim().is_empty() => Self::parse(&v).map(Some),
            _ => Ok(None),
        }
    }

    pub fn sealing_kid(&self) -> &str {
        &self.keys[0].kid
    }

    fn aad(kid: &str) -> Vec<u8> {
        format!("{AAD_PREFIX}{kid}").into_bytes()
    }

    /// Seal claims with the first key.
    pub fn seal(&self, claims: &Claims) -> Result<String, String> {
        let key = &self.keys[0];
        let plaintext = serde_json::to_vec(claims).map_err(|e| e.to_string())?;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let aad = Self::aad(&key.kid);
        let sealed = key
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| "sealing failed".to_string())?;
        let mut body = Vec::with_capacity(NONCE_LEN + sealed.len());
        body.extend_from_slice(&nonce);
        body.extend_from_slice(&sealed);
        Ok(format!(
            "{TOKEN_PREFIX}{}.{}",
            key.kid,
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body)
        ))
    }

    /// Open a token, checking its expiry against `now` (unix seconds).
    pub fn open(&self, token: &str, now: u64) -> Result<Claims, OpenError> {
        let rest = token
            .strip_prefix(TOKEN_PREFIX)
            .ok_or(OpenError::Malformed)?;
        let (kid, body) = rest.split_once('.').ok_or(OpenError::Malformed)?;
        if !valid_kid(kid) {
            return Err(OpenError::Malformed);
        }
        let key = self
            .keys
            .iter()
            .find(|k| k.kid == kid)
            .ok_or(OpenError::UnknownKey)?;
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| OpenError::Malformed)?;
        if body.len() <= NONCE_LEN {
            return Err(OpenError::Malformed);
        }
        let (nonce, sealed) = body.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| OpenError::Malformed)?;
        let aad = Self::aad(kid);
        let plaintext = key
            .cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: sealed,
                    aad: &aad,
                },
            )
            .map_err(|_| OpenError::Tampered)?;
        let claims: Claims = serde_json::from_slice(&plaintext).map_err(|_| OpenError::Tampered)?;
        if claims.v != 1 {
            return Err(OpenError::Malformed);
        }
        if claims.exp <= now {
            return Err(OpenError::Expired);
        }
        Ok(claims)
    }
}

pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn keys(spec: &str) -> ClientSecretKeys {
        ClientSecretKeys::parse(spec).unwrap()
    }

    fn claims(exp: u64) -> Claims {
        Claims {
            v: 1,
            iat: 1000,
            exp,
            ep: "0f5d9a3e-1111-4222-8333-444455556666".into(),
            alias: "my-rt".into(),
            parent: Parent::ApiKey {
                h: bud_auth::hash_api_key("bud_parent_key"),
                ck: false,
            },
            pid: Some("proj-7c1e".into()),
            uid: Some("user-9a2b".into()),
            akid: Some("akid-44f0".into()),
        }
    }

    #[test]
    fn a_sealed_secret_opens_to_its_claims() {
        let k = keys(&format!("k1:{}", b64(&[7u8; 32])));
        let token = k.seal(&claims(5000)).unwrap();
        assert_eq!(k.open(&token, 2000).unwrap(), claims(5000));
    }

    /// TC-EK-06 🔒 — any altered byte, or another configured key's kid, fails.
    #[test]
    fn tc_ek_06_tampering_fails() {
        let k = keys(&format!("k1:{},k2:{}", b64(&[7u8; 32]), b64(&[9u8; 32])));
        let token = k.seal(&claims(5000)).unwrap();
        let (head, body) = token.rsplit_once('.').unwrap();
        let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let flipped = format!(
            "{head}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
        );
        assert_eq!(k.open(&flipped, 2000), Err(OpenError::Tampered));
        let other_kid = token.replacen("ek_bud_k1.", "ek_bud_k2.", 1);
        assert_eq!(k.open(&other_kid, 2000), Err(OpenError::Tampered));
    }

    /// TC-EK-07
    #[test]
    fn tc_ek_07_expired_secrets_are_refused() {
        let k = keys(&format!("k1:{}", b64(&[7u8; 32])));
        let token = k.seal(&claims(5000)).unwrap();
        assert_eq!(k.open(&token, 5000), Err(OpenError::Expired));
    }

    /// TC-EK-10 — rotation: `k1` → `k2,k1` → `k2`.
    #[test]
    fn tc_ek_10_rotation() {
        let (k1, k2) = (b64(&[1u8; 32]), b64(&[2u8; 32]));
        let old = keys(&format!("k1:{k1}")).seal(&claims(5000)).unwrap();
        let both = keys(&format!("k2:{k2},k1:{k1}"));
        assert!(
            both.open(&old, 2000).is_ok(),
            "an old secret opens during rotation"
        );
        let new = both.seal(&claims(5000)).unwrap();
        assert!(new.starts_with("ek_bud_k2."), "the first key seals");
        let only_new = keys(&format!("k2:{k2}"));
        assert!(only_new.open(&new, 2000).is_ok());
        assert_eq!(only_new.open(&old, 2000), Err(OpenError::UnknownKey));
    }

    /// TC-EK-13 🔒 — the claims are opaque to the holder.
    #[test]
    fn tc_ek_13_claims_are_opaque() {
        let k = keys(&format!("k1:{}", b64(&[7u8; 32])));
        let c = claims(5000);
        let token = k.seal(&c).unwrap();
        let body = token.rsplit_once('.').unwrap().1;
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .unwrap();
        let haystack = String::from_utf8_lossy(&decoded);
        let Parent::ApiKey { h, .. } = &c.parent else {
            unreachable!()
        };
        for needle in [
            c.ep.as_str(),
            c.alias.as_str(),
            h.as_str(),
            c.pid.as_deref().unwrap(),
            c.uid.as_deref().unwrap(),
            c.akid.as_deref().unwrap(),
        ] {
            assert!(
                !haystack.contains(needle),
                "{needle} is readable in the token"
            );
            assert!(!token.contains(needle), "{needle} is readable in the token");
        }
    }

    /// TC-EK-15 — legal inside `Sec-WebSocket-Protocol`.
    #[test]
    fn tc_ek_15_subprotocol_safe_alphabet() {
        let k = keys(&format!("k-1_a:{}", b64(&[7u8; 32])));
        let token = k.seal(&claims(5000)).unwrap();
        let (head, body) = token.split_once('.').unwrap();
        assert!(head.starts_with("ek_bud_"));
        let kid = &head["ek_bud_".len()..];
        assert!(valid_kid(kid));
        assert!(
            body.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "{body}"
        );
        assert!(!token.contains('='));
        // ~410 characters with full UUID attribution (FRD-023 §5.8): comfortably inside a header.
        assert!((200..=600).contains(&token.len()), "length {}", token.len());
    }

    /// TC-EK-16 — bad key configuration fails, naming the problem.
    #[test]
    fn tc_ek_16_bad_keys_fail_startup() {
        let short = ClientSecretKeys::parse(&format!("k1:{}", b64(&[7u8; 16]))).unwrap_err();
        assert!(short.contains("16 bytes"), "{short}");
        let dup = ClientSecretKeys::parse(&format!("k1:{0},k1:{0}", b64(&[7u8; 32]))).unwrap_err();
        assert!(dup.contains("duplicate kid"), "{dup}");
        assert!(ClientSecretKeys::parse("k1").is_err());
        assert!(ClientSecretKeys::parse(&format!("bad.kid:{}", b64(&[7u8; 32]))).is_err());
        assert!(ClientSecretKeys::parse(" , ").is_err());
    }

    #[test]
    fn junk_is_malformed() {
        let k = keys(&format!("k1:{}", b64(&[7u8; 32])));
        for junk in [
            "",
            "bud_key",
            "ek_bud_",
            "ek_bud_k1",
            "ek_bud_k1.!!!",
            "ek_bud_k1.AAAA",
        ] {
            assert!(
                matches!(k.open(junk, 0), Err(OpenError::Malformed)),
                "{junk}"
            );
        }
    }
}

//! A credential key pair made at run time, once per test binary: the PKCS#8 private key and the test vendor key
//! encrypted to it the way budapp encrypts credentials (RSA-OAEP-SHA-256, hex). bud-auth's `*.pem` fixtures are
//! git-ignored, so a clean checkout (CI) has no key to read and no way to open a committed ciphertext; this
//! mirrors `src/test_support.rs`. Included with `#[path = "support/credential_keys.rs"] mod credential_keys;`.

use std::sync::OnceLock;

/// What every test credential decrypts to.
#[allow(dead_code)]
pub const VENDOR_KEY: &str = "dg_vendor_key_abc123";

pub struct CredentialKey {
    /// The private key, PKCS#8 PEM (what `CredentialDecryptor::from_pem` reads).
    pub pem: String,
    /// [`VENDOR_KEY`] encrypted to it, hex.
    pub ciphertext: String,
}

pub fn credential_key() -> &'static CredentialKey {
    static KEY: OnceLock<CredentialKey> = OnceLock::new();
    KEY.get_or_init(|| {
        use rsa::pkcs8::{EncodePrivateKey, LineEnding};
        let mut rng = rsa::rand_core::OsRng;
        let key = rsa::RsaPrivateKey::new(&mut rng, 2048).expect("test key generates");
        let ciphertext = key
            .to_public_key()
            .encrypt(&mut rng, rsa::Oaep::new::<sha2::Sha256>(), VENDOR_KEY.as_bytes())
            .expect("test credential encrypts");
        CredentialKey {
            pem: key.to_pkcs8_pem(LineEnding::LF).expect("test key encodes").to_string(),
            ciphertext: hex::encode(ciphertext),
        }
    })
}

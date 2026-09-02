//! The RSA key Bud's clusters actually ship is an ENCRYPTED PKCS#8 PEM.
//!
//! `{release}-rsa-keys` holds `rsa-private-key.pem` beginning `-----BEGIN ENCRYPTED PRIVATE
//! KEY-----`, with the passphrase in the same secret under `private-key-password`;
//! budgateway opens it that way (`tensorzero-internal/src/encryption.rs`). A decryptor that
//! only understands an unencrypted PEM does not degrade — `BudMode::start` returns Err and
//! the pod refuses to start, reporting a malformed key about a key that is entirely valid.
//!
//! The fixtures were produced by openssl, independently of this crate:
//!
//! ```text
//! openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out plain.pem
//! openssl pkcs8 -topk8 -in plain.pem -out encrypted-pkcs8.pem -passout pass:waav-test-pw
//! openssl rsa -in plain.pem -pubout -out pub.pem
//! printf 'dg_live_secret_key' | openssl pkeyutl -encrypt -pubin -inkey pub.pem \
//!   -pkeyopt rsa_padding_mode:oaep -pkeyopt rsa_oaep_md:sha256 | xxd -p | tr -d '\n'
//! ```
//!
//! so the test cannot pass by agreeing with a mistake in this crate's own encryption.

use bud_auth::CredentialDecryptor;

const ENCRYPTED_PEM: &str = include_str!("fixtures/encrypted-pkcs8.pem");
const CIPHERTEXT_HEX: &str = include_str!("fixtures/encrypted-pkcs8.ciphertext.hex");
const PASSWORD: &str = "waav-test-pw";
const PLAINTEXT: &str = "dg_live_secret_key";

#[test]
fn opens_the_encrypted_key_bud_clusters_ship() {
    let decryptor = CredentialDecryptor::from_pem_with_password(ENCRYPTED_PEM, Some(PASSWORD))
        .expect("an encrypted PKCS#8 PEM with its passphrase must load");
    assert!(decryptor.is_enabled());
    assert_eq!(
        decryptor.decrypt(CIPHERTEXT_HEX.trim()).unwrap(),
        PLAINTEXT,
        "the key loaded but does not decrypt what budapp encrypted with its public half"
    );
}

#[test]
fn refuses_an_encrypted_key_with_no_passphrase_and_says_which_variable_is_missing() {
    // The message is the deliverable. "invalid key" about a valid key sends an operator to
    // regenerate the secret instead of setting one environment variable.
    let err = CredentialDecryptor::from_pem_with_password(ENCRYPTED_PEM, None)
        .expect_err("an encrypted key with no passphrase cannot load");
    let message = err.to_string();
    assert!(
        message.contains("WAAV_RSA_PRIVATE_KEY_PASSWORD"),
        "the refusal must name the missing variable, got: {message}"
    );
}

#[test]
fn refuses_an_encrypted_key_with_the_wrong_passphrase() {
    assert!(CredentialDecryptor::from_pem_with_password(ENCRYPTED_PEM, Some("wrong")).is_err());
}

#[test]
fn an_empty_passphrase_reads_as_absent_rather_than_as_a_passphrase() {
    // "" is the shape an unset value takes in a Helm template, and trying it as a real
    // passphrase would report a wrong-password failure for a variable nobody set.
    let err = CredentialDecryptor::from_pem_with_password(ENCRYPTED_PEM, Some(""))
        .expect_err("empty is not a passphrase");
    assert!(err.to_string().contains("WAAV_RSA_PRIVATE_KEY_PASSWORD"));
}

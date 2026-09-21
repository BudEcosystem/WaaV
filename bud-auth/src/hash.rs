//! Credential hashing, byte-compatible with budgateway.
//!
//! WaaV and budgateway read the SAME Redis namespace (`api_key:{hash}`), written by budapp.
//! The hash function is therefore a cross-service contract, not an implementation detail: if
//! it drifts, WaaV reads a namespace nobody writes and every request 401s while the key is
//! plainly present in Redis.
//!
//! Reference implementation: `tensorzero-internal/src/auth.rs::hash_api_key`.

use sha2::{Digest, Sha256};

/// The prefix budapp mixes in before hashing. Not a salt in any cryptographic sense —
/// it is a namespace marker, and it is part of the wire contract.
const HASH_PREFIX: &[u8] = b"bud-";

/// `sha256("bud-" + api_key)`, lowercase hex.
///
/// Must stay identical to budgateway's `hash_api_key`. `TC-AUTH-01` pins it against
/// constants computed independently of this code.
pub fn hash_api_key(api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(HASH_PREFIX);
    hasher.update(api_key.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden values computed OUTSIDE this crate (python hashlib) so the test cannot pass by
    // agreeing with a bug in `hash_api_key`. These are the exact digests budgateway produces.
    const GOLDEN: &[(&str, &str)] = &[
        (
            "bud_test",
            "fcb89c0daaf6545dfff7cd9e8836c26eed4ddc4d1abc4cd034adba7c2174f186",
        ),
        (
            "bud_test_key_123",
            "c18e372389d5bd8bfb7bbec20013a9459094dd1cbe737defb9759349b8ff82d7",
        ),
        (
            "budserve_abc",
            "f42b0a6f886910afcbd8f721fc6b6e196a139e482de1a6ad6ac714c0df461b3c",
        ),
    ];

    /// TC-AUTH-01 — the cross-service contract.
    #[test]
    fn matches_budgateway_digests() {
        for (key, expected) in GOLDEN {
            assert_eq!(
                hash_api_key(key),
                *expected,
                "hash drifted from budgateway for {key:?}; WaaV would read a namespace nobody writes"
            );
        }
    }

    #[test]
    fn is_lowercase_hex_of_fixed_width() {
        let h = hash_api_key("anything");
        assert_eq!(h.len(), 64);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    /// The prefix must actually participate; hashing the bare key would still produce a
    /// well-formed digest, so shape checks alone cannot catch its removal.
    #[test]
    fn prefix_participates() {
        let with_prefix = hash_api_key("k");
        let bare = {
            let mut h = Sha256::new();
            h.update(b"k");
            format!("{:x}", h.finalize())
        };
        assert_ne!(with_prefix, bare);
    }

    #[test]
    fn distinct_keys_distinct_hashes() {
        assert_ne!(hash_api_key("a"), hash_api_key("b"));
    }

    #[test]
    fn empty_key_is_hashable() {
        // budapp will never mint one, but a client can send `Authorization: Bearer `.
        // It must hash rather than panic; the lookup then misses like any unknown key.
        assert_eq!(hash_api_key("").len(), 64);
    }
}

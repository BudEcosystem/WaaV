//! Every map key a provider reads that could shape WHERE a request goes is reviewed here.
//!
//! A standalone leg without a client key carries the operator's credential, so a client value
//! that picks a destination must pass `core::client_destination` (the guarded URL keys) or be
//! confined by the provider (a typed region, a validated location, a vendor-side callback). This
//! test scans the non-test code of every STT, TTS and realtime provider for the keys it reads
//! (`.get("…")`, `.contains_key("…")`, `.remove("…")`, `helper(map, "…")`) and fails on a
//! destination-like key nobody has classified, so a new provider, or a new extras key, is
//! reviewed before it ships. Response-parsing keys that merely look URL-like are listed too.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Reviewed destination-like keys, each with the reason it is safe.
const REVIEWED: &[(&str, &str)] = &[
    // Guarded by core::client_destination::URL_KEYS (operator approval or the client's own key).
    ("endpoint_override", "guarded URL key"),
    ("custom_endpoint", "guarded URL key"),
    ("custom_callback_url", "guarded URL key (Bhashini dials it)"),
    ("base_url", "guarded URL key"),
    (
        "api_base",
        "guarded URL key (Azure confines it to Microsoft hosts itself)",
    ),
    (
        "use_https",
        "guarded: use_https=false with the operator key is refused",
    ),
    // Confined by the provider.
    (
        "region",
        "parsed into a typed region (AWS, Azure, Alibaba, Huawei, Murf, segmented); Tencent sends it only as the X-TC-Region header",
    ),
    (
        "location",
        "Google: ^[a-z0-9-]{1,40}$ before it is part of the hostname",
    ),
    (
        "instance_id",
        "IBM: net::validate_path_segment (one plain path segment)",
    ),
    (
        "use_intl_endpoint",
        "Tencent: a bool choosing between two fixed hosts",
    ),
    (
        "domain",
        "AssemblyAI query value (encoded), Speechmatics language pack in the body",
    ),
    (
        "domain_personalization",
        "iFlytek request parameter, not a host",
    ),
    ("base_model_version", "IBM recognize parameter, not a host"),
    // Called by the VENDOR, never dialed by WaaV (no WaaV credential goes there).
    (
        "callback",
        "Deepgram async callback, sent to Deepgram as a parameter",
    ),
    ("callback_method", "Deepgram async callback method"),
    (
        "callback_url",
        "FPT async callback, sent to FPT as a header",
    ),
    ("url", "Groq: an audio URL Groq fetches"),
    // Credentials, not destinations (a client sending them brings its own key).
    ("aws_access_key_id", "client credential"),
    ("aws_secret_access_key", "client credential"),
    ("aws_session_token", "client credential"),
    // Read from vendor RESPONSES, not from client extras.
    ("upload_url", "AssemblyAI upload response"),
    ("audio_base_64", "ElevenLabs realtime response field"),
    ("serverContent", "Gemini Live response field"),
];

const DESTINATION_LIKE: &str = r"(?i)url|endpoint|host|base|domain|server|ws|socket|callback|webhook|region|uri|addr|origin|proxy|location|cluster|instance|deployment|path|https|tls|scheme|port|resource";

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && !path.to_string_lossy().contains("test")
        {
            out.push(path);
        }
    }
}

/// `provider module -> keys read`, over src/core/{stt,tts,realtime}.
fn inventory() -> BTreeMap<String, BTreeSet<String>> {
    let key = regex::Regex::new(
        r#"(?:\.get|\.contains_key|\.remove|\.get_mut)\(\s*"([A-Za-z][A-Za-z0-9_]*)"|[A-Za-z_][A-Za-z0-9_]*\(\s*&?[A-Za-z_][A-Za-z0-9_.()]*\s*,\s*"([A-Za-z][A-Za-z0-9_]*)"\s*\)"#,
    )
    .unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core");
    let mut out = BTreeMap::new();
    for kind in ["stt", "tts", "realtime"] {
        for entry in std::fs::read_dir(root.join(kind)).unwrap().flatten() {
            let path = entry.path();
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let mut files = Vec::new();
            if path.is_dir() {
                rust_files(&path, &mut files);
            } else if name != "mod"
                && !name.contains("test")
                && path.extension().is_some_and(|e| e == "rs")
            {
                files.push(path.clone());
            }
            let mut keys = BTreeSet::new();
            for file in files {
                let src = std::fs::read_to_string(&file).unwrap();
                let code = src.split("#[cfg(test)]").next().unwrap_or("");
                for c in key.captures_iter(code) {
                    keys.insert(c.get(1).or(c.get(2)).unwrap().as_str().to_string());
                }
            }
            if !keys.is_empty() {
                out.insert(format!("{kind}/{name}"), keys);
            }
        }
    }
    out
}

#[test]
fn every_destination_like_key_a_provider_reads_is_reviewed() {
    let like = regex::Regex::new(DESTINATION_LIKE).unwrap();
    let reviewed: BTreeSet<&str> = REVIEWED.iter().map(|(k, _)| *k).collect();
    let inventory = inventory();
    assert!(
        inventory.len() >= 60,
        "the scan found only {} provider modules; did the layout change?",
        inventory.len()
    );
    let mut unreviewed: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (module, keys) in &inventory {
        for k in keys.iter().filter(|k| like.is_match(k)) {
            if !reviewed.contains(k.as_str()) {
                unreviewed.entry(k).or_default().push(module);
            }
        }
    }
    assert!(
        unreviewed.is_empty(),
        "provider keys that may pick a destination and nobody reviewed: {unreviewed:?}. \
         Guard them (core::client_destination::URL_KEYS), confine them in the provider, or \
         classify them in REVIEWED with the reason they are safe."
    );
}

#[test]
fn the_guarded_keys_are_exactly_the_client_destination_url_keys() {
    let guarded: BTreeSet<&str> = REVIEWED
        .iter()
        .filter(|(_, why)| why.starts_with("guarded URL key"))
        .map(|(k, _)| *k)
        .collect();
    let url_keys: BTreeSet<&str> = waav_gateway::core::client_destination::URL_KEYS
        .iter()
        .copied()
        .collect();
    assert_eq!(guarded, url_keys);
}

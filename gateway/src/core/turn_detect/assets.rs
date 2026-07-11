use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::fs;
use tracing::{error, info, warn};

use crate::core::model_integrity::skip_model_hash_check_from_env;
use crate::core::turn_detect::config::TurnDetectorConfig;

const MODEL_FILENAME: &str = "model_quantized.onnx";
const TOKENIZER_FILENAME: &str = "tokenizer.json";
const TURN_DETECT_ARTIFACT_URL_SCHEMES: &[&str] = &["http", "https"];

/// Download all turn detector artifacts (model + tokenizer) if not already cached.
pub async fn download_assets(config: &TurnDetectorConfig) -> Result<()> {
    let model_path = download_model(config).await?;
    info!("Turn detector model ready at: {:?}", model_path);

    let tokenizer_path = download_tokenizer(config).await?;
    info!("Turn detector tokenizer ready at: {:?}", tokenizer_path);

    Ok(())
}

/// Ensure the model exists locally, downloading it when missing.
pub async fn download_model(config: &TurnDetectorConfig) -> Result<PathBuf> {
    if let Some(model_path) = &config.model_path {
        if model_path.exists() {
            return Ok(model_path.clone());
        }

        error!(
            "Configured turn detector model path {:?} is missing or unreadable",
            model_path
        );
        anyhow::bail!(
            "Configured turn detector model path {:?} does not exist",
            model_path
        );
    }

    let cache_dir = config.get_cache_dir()?;
    fs::create_dir_all(&cache_dir).await?;
    let model_path = cache_dir.join(MODEL_FILENAME);

    if model_path.exists() {
        info!("Using cached turn detector model at: {:?}", model_path);
        return Ok(model_path);
    }

    let model_url = config
        .model_url
        .as_deref()
        .context("No model URL specified and model not found locally")?;
    let model_url = validate_turn_detector_artifact_url("model_url", model_url)?;

    info!("Downloading turn detector model from: {}", model_url);
    download_file(model_url, &model_path).await?;

    Ok(model_path)
}

/// Ensure the tokenizer exists locally, downloading it when missing.
pub async fn download_tokenizer(config: &TurnDetectorConfig) -> Result<PathBuf> {
    if let Some(path) = &config.tokenizer_path {
        if path.exists() {
            return Ok(path.clone());
        }

        anyhow::bail!(
            "Configured turn detector tokenizer path {:?} does not exist",
            path
        );
    }

    let cache_dir = config.get_cache_dir()?;
    fs::create_dir_all(&cache_dir).await?;
    let tokenizer_path = cache_dir.join(TOKENIZER_FILENAME);

    if tokenizer_path.exists() {
        info!(
            "Using cached turn detector tokenizer at: {:?}",
            tokenizer_path
        );
        return Ok(tokenizer_path);
    }

    let tokenizer_url = config
        .tokenizer_url
        .as_deref()
        .context("No tokenizer URL specified and tokenizer not found locally")?;
    let tokenizer_url = validate_turn_detector_artifact_url("tokenizer_url", tokenizer_url)?;

    info!(
        "Downloading turn detector tokenizer from: {}",
        tokenizer_url
    );
    download_file(tokenizer_url, &tokenizer_path).await?;

    Ok(tokenizer_path)
}

/// Resolve the expected on-disk location of the model without downloading it.
pub fn model_path(config: &TurnDetectorConfig) -> Result<PathBuf> {
    if let Some(model_path) = &config.model_path {
        if model_path.exists() {
            return Ok(model_path.clone());
        }

        anyhow::bail!(
            "Turn detector model not found at configured path {:?}. Run `waav-gateway init` first.",
            model_path
        );
    }

    let cache_dir = config.get_cache_dir()?;
    let model_path = cache_dir.join(MODEL_FILENAME);

    if model_path.exists() {
        Ok(model_path)
    } else {
        error!(
            "Turn detector model expected at {:?} but not found. Ensure `waav-gateway init` populated the cache.",
            model_path
        );
        anyhow::bail!(
            "Turn detector model missing at {:?}. Run `waav-gateway init` before starting the server.",
            model_path
        );
    }
}

/// Resolve the expected on-disk location of the tokenizer without downloading it.
pub fn tokenizer_path(config: &TurnDetectorConfig) -> Result<PathBuf> {
    if let Some(path) = &config.tokenizer_path {
        if path.exists() {
            return Ok(path.clone());
        }

        error!(
            "Configured turn detector tokenizer path {:?} is missing or unreadable",
            path
        );
        anyhow::bail!(
            "Turn detector tokenizer not found at configured path {:?}. Run `waav-gateway init` first.",
            path
        );
    }

    let cache_dir = config.get_cache_dir()?;
    let tokenizer_path = cache_dir.join(TOKENIZER_FILENAME);

    if tokenizer_path.exists() {
        Ok(tokenizer_path)
    } else {
        error!(
            "Turn detector tokenizer expected at {:?} but not found. Ensure `waav-gateway init` populated the cache.",
            tokenizer_path
        );
        anyhow::bail!(
            "Turn detector tokenizer missing at {:?}. Run `waav-gateway init` before starting the server.",
            tokenizer_path
        );
    }
}

fn validate_turn_detector_artifact_url<'a>(label: &str, url: &'a str) -> Result<&'a str> {
    let url = url.trim();
    if url.is_empty() {
        anyhow::bail!("{label} rejected (SSRF protection): empty URL");
    }
    crate::core::net::validate_url_for_ssrf(url, TURN_DETECT_ARTIFACT_URL_SCHEMES)
        .map_err(|msg| anyhow::anyhow!("{label} rejected (SSRF protection): {msg}"))?;
    Ok(url)
}

async fn download_file(url: &str, path: &Path) -> Result<()> {
    let url = validate_turn_detector_artifact_url("artifact URL", url)?;
    let client = crate::core::net::ssrf_protected_client(TURN_DETECT_ARTIFACT_URL_SCHEMES)
        .context("Failed to create SSRF-protected download client")?;
    let response = client
        .get(url)
        .send()
        .await
        .context("Failed to download turn detector artifact")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Failed to download turn detector artifact: HTTP {}",
            response.status()
        );
    }

    let bytes = response.bytes().await?;

    if let Some(expected_hash) = get_expected_hash(url) {
        verify_hash(&bytes, &expected_hash)?;
    }

    fs::write(path, bytes).await?;
    info!("Downloaded turn detector artifact to: {:?}", path);

    Ok(())
}

fn get_expected_hash(url: &str) -> Option<String> {
    // Pinned SHA-256 of the livekit/turn-detector artifacts (supply-chain integrity, plan E7).
    // Previously "expected_hash_here", so verification never matched and was warn-only — a
    // 165 MB model was downloaded over the network and executed with NO integrity check.
    if url.contains(MODEL_FILENAME) {
        Some("4e685767c3643b0363c9f826a98325683f29e9c7d550162c8e8740ba33aa31aa".to_string())
    } else if url.contains(TOKENIZER_FILENAME) {
        Some("c8219a662de786c94771323c3500377970f5eaa3afbeaef9390c9a51db9f7884".to_string())
    } else {
        None
    }
}

fn verify_hash(data: &[u8], expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = format!("{:x}", hasher.finalize());
    let skip_hash_check = skip_model_hash_check_from_env()?;

    if actual != expected {
        // Fail CLOSED: an unexpected artifact may be tampered or corrupt and must not be
        // executed. Operators intentionally adopting a newer pinned model can override with
        // WAAV_SKIP_MODEL_HASH_CHECK=1 after verifying the source.
        if skip_hash_check {
            warn!(
                "Turn detector artifact hash mismatch IGNORED (WAAV_SKIP_MODEL_HASH_CHECK) - expected: {}, actual: {}",
                expected, actual
            );
            return Ok(());
        }
        anyhow::bail!(
            "Turn detector artifact hash mismatch - expected: {expected}, actual: {actual}. \
             Refusing to load a potentially-tampered model; set WAAV_SKIP_MODEL_HASH_CHECK=1 to override."
        );
    }

    Ok(())
}

#[cfg(test)]
mod hash_tests {
    use super::*;
    use crate::core::model_integrity::WAAV_SKIP_MODEL_HASH_CHECK_ENV;
    use serial_test::serial;

    struct EnvGuard(Option<String>);

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.0.as_deref() {
                Some(value) => unsafe { std::env::set_var(WAAV_SKIP_MODEL_HASH_CHECK_ENV, value) },
                None => unsafe { std::env::remove_var(WAAV_SKIP_MODEL_HASH_CHECK_ENV) },
            }
        }
    }

    fn set_hash_skip_env(value: Option<&str>) -> EnvGuard {
        let previous = std::env::var(WAAV_SKIP_MODEL_HASH_CHECK_ENV).ok();
        match value {
            Some(value) => unsafe { std::env::set_var(WAAV_SKIP_MODEL_HASH_CHECK_ENV, value) },
            None => unsafe { std::env::remove_var(WAAV_SKIP_MODEL_HASH_CHECK_ENV) },
        }
        EnvGuard(previous)
    }

    #[test]
    #[serial]
    fn verify_hash_rejects_mismatch_by_default() {
        let _guard = set_hash_skip_env(None);

        // SHA-256 of "hello" is well-known; verifying it against a wrong expected hash fails.
        let err = verify_hash(b"hello", "deadbeef");
        assert!(err.is_err(), "hash mismatch must fail closed");
    }

    #[test]
    #[serial]
    fn verify_hash_accepts_match() {
        let _guard = set_hash_skip_env(None);

        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"; // sha256("hello")
        assert!(verify_hash(b"hello", expected).is_ok());
    }

    #[test]
    #[serial]
    fn verify_hash_explicit_false_does_not_bypass_mismatch() {
        let _guard = set_hash_skip_env(Some("0"));

        let err = verify_hash(b"hello", "deadbeef").unwrap_err();
        assert!(
            err.to_string().contains("hash mismatch"),
            "explicit false must keep hash verification enforced: {err}"
        );
    }

    #[test]
    #[serial]
    fn verify_hash_explicit_true_bypasses_mismatch() {
        let _guard = set_hash_skip_env(Some("yes"));

        assert!(verify_hash(b"hello", "deadbeef").is_ok());
    }

    #[test]
    #[serial]
    fn verify_hash_rejects_malformed_skip_env() {
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"; // sha256("hello")
        let _guard = set_hash_skip_env(Some("off"));

        let err = verify_hash(b"hello", expected).unwrap_err();
        assert!(
            err.to_string().contains(WAAV_SKIP_MODEL_HASH_CHECK_ENV),
            "malformed explicit override must be rejected: {err}"
        );
    }

    #[test]
    fn expected_hashes_are_pinned_not_placeholder() {
        let h = get_expected_hash("https://x/model_quantized.onnx").unwrap();
        assert_eq!(h.len(), 64);
        assert_ne!(h, "expected_hash_here");
        assert!(get_expected_hash("https://x/tokenizer.json").is_some());
    }

    #[test]
    fn artifact_urls_are_ssrf_checked() {
        let _env = crate::core::net::ssrf_env_lock();
        assert_eq!(
            validate_turn_detector_artifact_url(
                "model_url",
                " https://models.example.com/model_quantized.onnx "
            )
            .unwrap(),
            "https://models.example.com/model_quantized.onnx"
        );

        let err = validate_turn_detector_artifact_url(
            "model_url",
            "http://127.0.0.1:9000/model_quantized.onnx",
        )
        .unwrap_err();
        assert!(err.to_string().contains("SSRF protection"), "{err}");

        let err =
            validate_turn_detector_artifact_url("tokenizer_url", "file:///tmp/tokenizer.json")
                .unwrap_err();
        assert!(err.to_string().contains("not allowed"), "{err}");
    }
}

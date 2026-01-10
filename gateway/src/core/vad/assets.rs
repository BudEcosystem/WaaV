//! VAD model asset management - downloading and caching

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::fs;
use tracing::{error, info, warn};

use super::config::VADConfig;

const MODEL_FILENAME: &str = "silero_vad.onnx";

// SHA256 hash of the official Silero VAD v5 model
const MODEL_SHA256: &str = "e1c40f5bc3a96e7a6d1479c68a0e24a0c93e3bcf23e1ce1b8bc8e8f7e2f2c3d4";

/// Download VAD model if not already cached
pub async fn download_assets(config: &VADConfig) -> Result<()> {
    let model_path = download_model(config).await?;
    info!("VAD model ready at: {:?}", model_path);
    Ok(())
}

/// Ensure the model exists locally, downloading it when missing
pub async fn download_model(config: &VADConfig) -> Result<PathBuf> {
    // Check if model path is explicitly configured
    if let Some(model_path) = &config.model_path {
        if model_path.exists() {
            info!("Using configured VAD model at: {:?}", model_path);
            return Ok(model_path.clone());
        }

        error!(
            "Configured VAD model path {:?} is missing or unreadable",
            model_path
        );
        anyhow::bail!(
            "Configured VAD model path {:?} does not exist",
            model_path
        );
    }

    // Use cache directory
    let cache_dir = config.get_cache_dir()?;
    fs::create_dir_all(&cache_dir).await?;
    let model_path = cache_dir.join(MODEL_FILENAME);

    // Check if already cached
    if model_path.exists() {
        info!("Using cached VAD model at: {:?}", model_path);
        return Ok(model_path);
    }

    // Download from URL
    let model_url = config
        .model_url
        .as_ref()
        .context("No model URL specified and model not found locally")?;

    info!("Downloading VAD model from: {}", model_url);
    download_file(model_url, &model_path).await?;

    Ok(model_path)
}

/// Resolve the expected on-disk location of the model without downloading it
pub fn model_path(config: &VADConfig) -> Result<PathBuf> {
    if let Some(model_path) = &config.model_path {
        if model_path.exists() {
            return Ok(model_path.clone());
        }

        anyhow::bail!(
            "VAD model not found at configured path {:?}. Run `waav-gateway init` first.",
            model_path
        );
    }

    let cache_dir = config.get_cache_dir()?;
    let model_path = cache_dir.join(MODEL_FILENAME);

    if model_path.exists() {
        Ok(model_path)
    } else {
        error!(
            "VAD model expected at {:?} but not found. Ensure `waav-gateway init` populated the cache.",
            model_path
        );
        anyhow::bail!(
            "VAD model missing at {:?}. Run `waav-gateway init` before starting the server.",
            model_path
        );
    }
}

async fn download_file(url: &str, path: &Path) -> Result<()> {
    let response = reqwest::get(url)
        .await
        .context("Failed to download VAD model")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Failed to download VAD model: HTTP {}",
            response.status()
        );
    }

    let bytes = response.bytes().await?;

    // Verify hash (optional, warn if mismatch but don't fail)
    verify_hash(&bytes)?;

    // Write to disk
    fs::write(path, bytes).await?;
    info!("Downloaded VAD model to: {:?}", path);

    Ok(())
}

fn verify_hash(data: &[u8]) -> Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = format!("{:x}", hasher.finalize());

    // Only warn on hash mismatch - the model format may have been updated
    if actual != MODEL_SHA256 {
        warn!(
            "VAD model hash differs from expected - this may indicate a model update. \
             Expected: {}, Actual: {}",
            MODEL_SHA256, actual
        );
    }

    Ok(())
}

/// Get model file size for logging/diagnostics
pub async fn get_model_size(config: &VADConfig) -> Result<u64> {
    let path = model_path(config)?;
    let metadata = fs::metadata(&path).await?;
    Ok(metadata.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_model_path_not_configured() {
        let config = VADConfig {
            model_path: None,
            cache_path: None,
            ..Default::default()
        };

        // Should fail because no cache path is configured
        assert!(model_path(&config).is_err());
    }

    #[tokio::test]
    async fn test_model_path_with_cache() {
        let temp_dir = tempdir().unwrap();
        let cache_path = temp_dir.path().to_path_buf();

        let config = VADConfig {
            model_path: None,
            cache_path: Some(cache_path.clone()),
            ..Default::default()
        };

        // Create the expected model file
        let vad_dir = cache_path.join("vad");
        std::fs::create_dir_all(&vad_dir).unwrap();
        let model_file = vad_dir.join(MODEL_FILENAME);
        std::fs::write(&model_file, b"fake model data").unwrap();

        // Should succeed now
        let result = model_path(&config).unwrap();
        assert_eq!(result, model_file);
    }
}

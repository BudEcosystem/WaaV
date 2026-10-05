//! Initialization helpers for preparing runtime assets before starting the
//! WaaV Gateway server.
//!
//! This module hosts the logic that powers the `waav-gateway init` CLI command. The
//! command downloads and caches the turn detection model and tokenizer, and the Silero and
//! SmartTurn models, so that regular server startups do not have to perform network fetches.
//!
//! Typical usage from the CLI:
//!
//! ```text
//! $ CACHE_PATH=/app/cache waav-gateway init
//! ```
//!
//! If you prefer to invoke the initialization routine programmatically, call
//! [`run`] inside an async context:
//!
//! ```rust,no_run
//! use waav_gateway::init;
//!
//! let runtime = tokio::runtime::Runtime::new().unwrap();
//! runtime.block_on(async {
//!     init::run().await.expect("failed to download assets");
//! });
//! ```

#[cfg(feature = "turn-detect")]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(any(
    feature = "turn-detect",
    feature = "silero-vad",
    feature = "smart-turn"
)))]
use anyhow::anyhow;

#[cfg(feature = "turn-detect")]
use crate::config::ServerConfig;
#[cfg(feature = "turn-detect")]
use crate::core::turn_detect::{TurnDetectorConfig, assets};

/// Download and prepare every model this build uses: the turn detector's model and tokenizer,
/// and the Silero speech detector and SmartTurn end-of-turn model that segmented speech-to-text
/// runs on every live call on a file-only model. Silero and SmartTurn go to `$CACHE_PATH/models`
/// (see [`crate::core::model_dir`]), where a replica finds them without a network.
#[cfg(any(
    feature = "turn-detect",
    feature = "silero-vad",
    feature = "smart-turn"
))]
pub async fn run() -> Result<()> {
    #[cfg(feature = "turn-detect")]
    {
        let config = ServerConfig::from_env().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let cache_path = config
            .cache_path
            .as_ref()
            .context("CACHE_PATH environment variable must be set to run `waav-gateway init`")?
            .clone();

        let turn_config = TurnDetectorConfig {
            cache_path: Some(cache_path.clone()),
            ..Default::default()
        };

        tracing::info!(
            "Preparing turn detector assets using cache path: {:?}",
            cache_path
        );
        assets::download_assets(&turn_config).await?;

        tracing::info!("Turn detector assets downloaded successfully");
    }

    #[cfg(feature = "silero-vad")]
    {
        let dir = crate::core::model_dir::model_cache_dir();
        tracing::info!("Preparing the Silero speech detector in {:?}", dir);
        crate::core::silero_vad::SileroVAD::new(crate::core::silero_vad::SileroVADConfig::default())
            .await
            .map_err(|e| anyhow::anyhow!("Silero speech detector: {e}"))?;
    }

    #[cfg(feature = "smart-turn")]
    {
        tracing::info!("Preparing the SmartTurn end-of-turn model");
        crate::core::smart_turn::SmartTurnDetector::new(
            crate::core::smart_turn::SmartTurnDetectorConfig::default(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("SmartTurn end-of-turn model: {e}"))?;
    }

    Ok(())
}

#[cfg(not(any(
    feature = "turn-detect",
    feature = "silero-vad",
    feature = "smart-turn"
)))]
pub async fn run() -> Result<()> {
    Err(anyhow!(
        "`waav-gateway init` downloads model assets, and this build has none. \
         Rebuild with `--features turn-ensemble` (turn detector, Silero and SmartTurn)."
    ))
}

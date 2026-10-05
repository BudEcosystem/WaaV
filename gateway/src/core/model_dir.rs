//! Where the gateway keeps the ONNX models it downloads (Silero, SmartTurn).
//!
//! With `CACHE_PATH` set (the production image sets `/app/cache`), models live in
//! `$CACHE_PATH/models`, which `waav-gateway init` fills at image build time, so a replica never
//! downloads at start-up and an air-gapped one still has its speech detector. Without it they go to
//! the user's cache directory, as before.

use std::path::{Path, PathBuf};

/// The directory a model is downloaded to.
pub fn model_cache_dir() -> PathBuf {
    model_cache_dir_for(std::env::var_os("CACHE_PATH").as_deref().map(Path::new))
}

/// The places a model file is looked for, in order: the working directory's `models/` and `./`,
/// then `$CACHE_PATH/models`, then the user's cache directory.
pub fn model_candidates(file: &str) -> Vec<PathBuf> {
    model_candidates_for(
        file,
        std::env::var_os("CACHE_PATH").as_deref().map(Path::new),
    )
}

fn user_cache_models() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("waav")
        .join("models")
}

fn model_cache_dir_for(cache_path: Option<&Path>) -> PathBuf {
    match cache_path.filter(|p| !p.as_os_str().is_empty()) {
        Some(p) => p.join("models"),
        None => user_cache_models(),
    }
}

fn model_candidates_for(file: &str, cache_path: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![
        PathBuf::from("models").join(file),
        PathBuf::from(".").join(file),
    ];
    if let Some(p) = cache_path.filter(|p| !p.as_os_str().is_empty()) {
        out.push(p.join("models").join(file));
    }
    out.push(user_cache_models().join(file));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_path_holds_the_models_and_is_searched_before_the_user_cache() {
        let c = Path::new("/app/cache");
        assert_eq!(
            model_cache_dir_for(Some(c)),
            PathBuf::from("/app/cache/models")
        );
        let found = model_candidates_for("silero_vad.onnx", Some(c));
        let baked = found
            .iter()
            .position(|p| p == Path::new("/app/cache/models/silero_vad.onnx"));
        let user = found
            .iter()
            .position(|p| p == &user_cache_models().join("silero_vad.onnx"));
        assert!(baked.is_some() && baked < user, "{found:?}");
    }

    #[test]
    fn without_a_cache_path_models_go_to_the_user_cache() {
        assert_eq!(model_cache_dir_for(None), user_cache_models());
        assert_eq!(
            model_cache_dir_for(Some(Path::new(""))),
            user_cache_models()
        );
        assert_eq!(model_candidates_for("x.onnx", None).len(), 3);
    }
}

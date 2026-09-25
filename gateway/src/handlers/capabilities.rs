//! Capability discovery handlers (P2): expose the unified-language support matrix so an SDK /
//! operator can see, without trial-and-error, which canonical language tokens exist and how each
//! provider renders them natively (the Chinese fork, the ElevenLabs downgrade, Baidu's numeric
//! dev_pid, …). Mirrors the existing simple GET handlers in [`super::dag`].

use axum::{Json, extract::State, response::IntoResponse};
use serde::Serialize;
use std::sync::Arc;

use crate::core::lang::{CanonicalLanguage, LanguageSupportRow, language_support_matrix};
use crate::state::AppState;

/// One canonical language entry: the BCP-47 token a developer passes + its decomposed subtags.
#[derive(Debug, Clone, Serialize)]
pub struct CanonicalLanguageInfo {
    /// The canonical region-qualified BCP-47 token (`en-US`, `cmn-CN`, `or-IN`).
    pub bcp47: &'static str,
    /// The bare language subtag (`en`, `cmn`, `yue`).
    pub lang_subtag: &'static str,
    /// The ISO-639-1 downgrade form (`en`, `zh`, `yue`).
    pub iso639_1: &'static str,
    /// The UPPERCASE region subtag (`US`, `CN`, `IN`).
    pub region: &'static str,
}

/// The `/capabilities/languages` response: the canonical value space + the per-provider matrix.
#[derive(Debug, Clone, Serialize)]
pub struct LanguageCapabilitiesResponse {
    /// Every canonical language a developer may pass (the `language` value space).
    pub canonical_languages: Vec<CanonicalLanguageInfo>,
    /// The per-provider native-notation matrix (how each provider renders `en-US` / `cmn-CN`).
    pub providers: Vec<LanguageSupportRow>,
    /// Models whose language support is NARROWER than their vendor's.
    ///
    /// Deepgram's specialised models — medical, pharma, phonecall and the rest — are English-only
    /// while `nova-3` itself speaks fifty languages, so a per-vendor answer is not enough to tell
    /// a caller what a given deployment will accept. A model absent from this list carries no
    /// model-level restriction; the provider row above applies.
    pub model_restrictions: &'static [crate::core::lang::ModelLanguageRow],
    /// Count of canonical languages (convenience for clients).
    pub canonical_count: usize,
}

/// `GET /capabilities/languages` — the unified-language discovery surface.
pub async fn list_language_capabilities(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let canonical_languages: Vec<CanonicalLanguageInfo> = CanonicalLanguage::all()
        .iter()
        .map(|c| CanonicalLanguageInfo {
            bcp47: c.as_bcp47(),
            lang_subtag: c.lang_subtag(),
            iso639_1: c.iso639_1(),
            region: c.region(),
        })
        .collect();
    let canonical_count = canonical_languages.len();
    Json(LanguageCapabilitiesResponse {
        canonical_languages,
        providers: language_support_matrix(),
        model_restrictions: crate::core::lang::MODEL_LANGUAGE_SUPPORT,
        canonical_count,
    })
}

/// `GET /capabilities/features` — which providers honour which canonical features.
///
/// The discovery surface for FRD-018 M1. `/capabilities/languages` is the precedent and the
/// shape this copies: a canonical value space plus a per-provider matrix, so an SDK, budadmin or
/// an operator can see what a knob will actually do on a given vendor without trial and error.
///
/// The matrix is derived from the mappings themselves rather than declared beside them — see
/// `core::capabilities` for why, and for the guard that keeps the two from drifting.
pub async fn list_feature_capabilities(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(crate::core::capabilities::feature_capabilities())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_info_decomposes_subtags() {
        let infos: Vec<_> = CanonicalLanguage::all()
            .iter()
            .map(|c| CanonicalLanguageInfo {
                bcp47: c.as_bcp47(),
                lang_subtag: c.lang_subtag(),
                iso639_1: c.iso639_1(),
                region: c.region(),
            })
            .collect();
        // The Chinese entry shows the cmn subtag with the zh downgrade — the headline distinction.
        let cmn = infos.iter().find(|i| i.bcp47 == "cmn-CN").unwrap();
        assert_eq!(cmn.lang_subtag, "cmn");
        assert_eq!(cmn.iso639_1, "zh");
        assert_eq!(cmn.region, "CN");
        // No Auto in the value space (it is a behavior, not a passable locale here).
        assert!(infos.iter().all(|i| i.bcp47 != "auto"));
    }
}

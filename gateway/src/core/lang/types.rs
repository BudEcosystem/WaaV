//! Canonical language types for the unified language system (P2).
//!
//! A developer passes ONE canonical language token — a region-qualified BCP-47
//! code such as `en-US`, `es-ES`, `hi-IN`, or `cmn-CN` (Mandarin) — and the
//! gateway maps it to each provider's native notation internally. This is the
//! language analogue of the proven [`crate::core::emotion`] system: a canonical
//! value space ([`CanonicalLanguage`]) plus a per-provider mapper registry plus
//! graceful-degrade warnings.
//!
//! # Why region-qualified BCP-47 (richer than the infer engine's canonical)
//!
//! The infer engine's `standardize` chassis canonicalizes to a *language-only*
//! token. The gateway needs **region** because it matters acoustically and for
//! TTS voice selection: `en-US` != `en-GB` != `en-IN` != `en-AU`. So the
//! canonical scheme is region-qualified BCP-47 (`ll-RR`: lowercase language
//! subtag + UPPERCASE region), accepting bare ISO-639-1 (`en`, `es`, `hi`) as
//! input *shorthand* that resolves to a default region.
//!
//! Chinese is the one place where the language subtag itself carries
//! information BCP-47 `zh-*` cannot: `zh-CN` cannot distinguish Mandarin from
//! Cantonese, but the downstream providers DO (Google STT `cmn-Hans-CN` vs
//! `yue-Hant-HK`; AWS `cmn-CN` vs no-`yue`; Speechmatics `cmn` vs `yue`). So we
//! canonicalize on the ISO-639-3 macrolanguage-resolving subtags `cmn`
//! (Mandarin) and `yue` (Cantonese) rather than the `zh` macrolanguage —
//! preserving the information the per-provider mappers need. Aliases fold both
//! `zh-*` and `cmn-*`/`yue-*` spellings onto these canonical variants.
//!
//! Variant identifiers follow Rust CamelCase with the region as a trailing word
//! (`EnUs`, `CmnCn`, `YueHk`); the `#[serde]` rename pins the `ll-RR` wire form.

use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Canonical Language Enum
// =============================================================================

/// A canonical, provider-agnostic language token.
///
/// Each variant is a curated region-qualified BCP-47 locale (or [`Auto`] for
/// detection). A developer passes one of these (or any alias spelling — see
/// [`super::resolve`]) and the per-provider mapper renders the correct native
/// notation. Carries [`as_bcp47`](Self::as_bcp47) (`"en-US"`),
/// [`iso639_1`](Self::iso639_1) (`"en"`), [`region`](Self::region) (`"US"`),
/// and [`lang_subtag`](Self::lang_subtag) (`"en"`/`"cmn"`/`"yue"`).
///
/// [`Auto`]: Self::Auto
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub enum CanonicalLanguage {
    /// Automatic language detection (or multilingual code-switching). Maps to
    /// each provider's detection mechanism, or warn-and-default where none
    /// exists. This is the default so an omitted/empty `language` is "auto".
    #[default]
    #[serde(rename = "auto")]
    Auto,

    // ---- English (region matters: do NOT collapse) --------------------------
    /// English (United States).
    #[serde(rename = "en-US")]
    EnUs,
    /// English (United Kingdom).
    #[serde(rename = "en-GB")]
    EnGb,
    /// English (India).
    #[serde(rename = "en-IN")]
    EnIn,
    /// English (Australia).
    #[serde(rename = "en-AU")]
    EnAu,
    /// English (South Africa).
    #[serde(rename = "en-ZA")]
    EnZa,
    /// English (New Zealand).
    #[serde(rename = "en-NZ")]
    EnNz,

    // ---- Spanish ------------------------------------------------------------
    /// Spanish (Spain).
    #[serde(rename = "es-ES")]
    EsEs,
    /// Spanish (Mexico).
    #[serde(rename = "es-MX")]
    EsMx,
    /// Spanish (United States).
    #[serde(rename = "es-US")]
    EsUs,

    // ---- French -------------------------------------------------------------
    /// French (France).
    #[serde(rename = "fr-FR")]
    FrFr,
    /// French (Canada).
    #[serde(rename = "fr-CA")]
    FrCa,

    // ---- Portuguese ---------------------------------------------------------
    /// Portuguese (Brazil).
    #[serde(rename = "pt-BR")]
    PtBr,
    /// Portuguese (Portugal).
    #[serde(rename = "pt-PT")]
    PtPt,

    // ---- Other European -----------------------------------------------------
    /// German (Germany).
    #[serde(rename = "de-DE")]
    DeDe,
    /// Italian (Italy).
    #[serde(rename = "it-IT")]
    ItIt,
    /// Dutch (Netherlands).
    #[serde(rename = "nl-NL")]
    NlNl,
    /// Russian (Russia).
    #[serde(rename = "ru-RU")]
    RuRu,
    /// Turkish (Turkey).
    #[serde(rename = "tr-TR")]
    TrTr,
    /// Polish (Poland).
    #[serde(rename = "pl-PL")]
    PlPl,
    /// Swedish (Sweden).
    #[serde(rename = "sv-SE")]
    SvSe,
    /// Norwegian Bokmål (Norway). Canonical uses `nb`; folds `no`/`nn`.
    #[serde(rename = "nb-NO")]
    NbNo,
    /// Danish (Denmark).
    #[serde(rename = "da-DK")]
    DaDk,
    /// Finnish (Finland).
    #[serde(rename = "fi-FI")]
    FiFi,
    /// Ukrainian (Ukraine).
    #[serde(rename = "uk-UA")]
    UkUa,
    /// Czech (Czechia).
    #[serde(rename = "cs-CZ")]
    CsCz,
    /// Greek (Greece).
    #[serde(rename = "el-GR")]
    ElGr,
    /// Romanian (Romania).
    #[serde(rename = "ro-RO")]
    RoRo,
    /// Hungarian (Hungary).
    #[serde(rename = "hu-HU")]
    HuHu,

    // ---- East Asian ---------------------------------------------------------
    /// Japanese (Japan).
    #[serde(rename = "ja-JP")]
    JaJp,
    /// Korean (Korea).
    #[serde(rename = "ko-KR")]
    KoKr,
    /// Mandarin Chinese, Simplified, China. Canonical uses `cmn` (ISO-639-3),
    /// since `zh` cannot disambiguate Mandarin from Cantonese.
    #[serde(rename = "cmn-CN")]
    CmnCn,
    /// Mandarin Chinese, Traditional, Taiwan.
    #[serde(rename = "cmn-TW")]
    CmnTw,
    /// Cantonese, Hong Kong.
    #[serde(rename = "yue-HK")]
    YueHk,

    // ---- Indian (Sarvam/Reverie/Bhashini families) --------------------------
    /// Hindi (India).
    #[serde(rename = "hi-IN")]
    HiIn,
    /// Bengali (India).
    #[serde(rename = "bn-IN")]
    BnIn,
    /// Tamil (India).
    #[serde(rename = "ta-IN")]
    TaIn,
    /// Telugu (India).
    #[serde(rename = "te-IN")]
    TeIn,
    /// Gujarati (India).
    #[serde(rename = "gu-IN")]
    GuIn,
    /// Kannada (India).
    #[serde(rename = "kn-IN")]
    KnIn,
    /// Malayalam (India).
    #[serde(rename = "ml-IN")]
    MlIn,
    /// Marathi (India).
    #[serde(rename = "mr-IN")]
    MrIn,
    /// Punjabi (India).
    #[serde(rename = "pa-IN")]
    PaIn,
    /// Odia (India). Canonical uses the *correct* ISO `or`; Sarvam's nonstandard
    /// `od-IN` is an OUTPUT-side override (see the Sarvam mapper).
    #[serde(rename = "or-IN")]
    OrIn,

    // ---- Middle East / SE Asia / other --------------------------------------
    /// Arabic (Saudi Arabia / Modern Standard). Aliases `arb`/`ara`/`arabic`.
    #[serde(rename = "ar-SA")]
    ArSa,
    /// Vietnamese (Vietnam).
    #[serde(rename = "vi-VN")]
    ViVn,
    /// Thai (Thailand).
    #[serde(rename = "th-TH")]
    ThTh,
    /// Indonesian (Indonesia).
    #[serde(rename = "id-ID")]
    IdId,
    /// Malay (Malaysia).
    #[serde(rename = "ms-MY")]
    MsMy,
    /// Hebrew (Israel).
    #[serde(rename = "he-IL")]
    HeIl,
}

impl CanonicalLanguage {
    /// All non-[`Auto`](Self::Auto) canonical locales, for capability listings
    /// and the SDK value-space mirror.
    pub const fn all() -> &'static [CanonicalLanguage] {
        use CanonicalLanguage::*;
        &[
            EnUs, EnGb, EnIn, EnAu, EnZa, EnNz, EsEs, EsMx, EsUs, FrFr, FrCa, PtBr, PtPt, DeDe,
            ItIt, NlNl, RuRu, TrTr, PlPl, SvSe, NbNo, DaDk, FiFi, UkUa, CsCz, ElGr, RoRo, HuHu,
            JaJp, KoKr, CmnCn, CmnTw, YueHk, HiIn, BnIn, TaIn, TeIn, GuIn, KnIn, MlIn, MrIn, PaIn,
            OrIn, ArSa, ViVn, ThTh, IdId, MsMy, HeIl,
        ]
    }

    /// The region-qualified BCP-47 string (`"en-US"`, `"cmn-CN"`). For
    /// [`Auto`](Self::Auto) this is the sentinel `"auto"`.
    pub const fn as_bcp47(&self) -> &'static str {
        use CanonicalLanguage::*;
        match self {
            Auto => "auto",
            EnUs => "en-US",
            EnGb => "en-GB",
            EnIn => "en-IN",
            EnAu => "en-AU",
            EnZa => "en-ZA",
            EnNz => "en-NZ",
            EsEs => "es-ES",
            EsMx => "es-MX",
            EsUs => "es-US",
            FrFr => "fr-FR",
            FrCa => "fr-CA",
            PtBr => "pt-BR",
            PtPt => "pt-PT",
            DeDe => "de-DE",
            ItIt => "it-IT",
            NlNl => "nl-NL",
            RuRu => "ru-RU",
            TrTr => "tr-TR",
            PlPl => "pl-PL",
            SvSe => "sv-SE",
            NbNo => "nb-NO",
            DaDk => "da-DK",
            FiFi => "fi-FI",
            UkUa => "uk-UA",
            CsCz => "cs-CZ",
            ElGr => "el-GR",
            RoRo => "ro-RO",
            HuHu => "hu-HU",
            JaJp => "ja-JP",
            KoKr => "ko-KR",
            CmnCn => "cmn-CN",
            CmnTw => "cmn-TW",
            YueHk => "yue-HK",
            HiIn => "hi-IN",
            BnIn => "bn-IN",
            TaIn => "ta-IN",
            TeIn => "te-IN",
            GuIn => "gu-IN",
            KnIn => "kn-IN",
            MlIn => "ml-IN",
            MrIn => "mr-IN",
            PaIn => "pa-IN",
            OrIn => "or-IN",
            ArSa => "ar-SA",
            ViVn => "vi-VN",
            ThTh => "th-TH",
            IdId => "id-ID",
            MsMy => "ms-MY",
            HeIl => "he-IL",
        }
    }

    /// The ISO-639-1 two-letter language code (`"en"`, `"es"`), region dropped.
    /// For Chinese this is the BCP-47/ISO-639-1 macrolanguage `"zh"` for the
    /// downgrade group's convenience (`cmn-CN`/`cmn-TW` → `"zh"`); Cantonese
    /// keeps `"yue"` (no ISO-639-1 macrolanguage maps it). For [`Auto`](Self::Auto)
    /// this is the empty string.
    pub const fn iso639_1(&self) -> &'static str {
        use CanonicalLanguage::*;
        match self {
            Auto => "",
            EnUs | EnGb | EnIn | EnAu | EnZa | EnNz => "en",
            EsEs | EsMx | EsUs => "es",
            FrFr | FrCa => "fr",
            PtBr | PtPt => "pt",
            DeDe => "de",
            ItIt => "it",
            NlNl => "nl",
            RuRu => "ru",
            TrTr => "tr",
            PlPl => "pl",
            SvSe => "sv",
            NbNo => "nb",
            DaDk => "da",
            FiFi => "fi",
            UkUa => "uk",
            CsCz => "cs",
            ElGr => "el",
            RoRo => "ro",
            HuHu => "hu",
            JaJp => "ja",
            KoKr => "ko",
            // The macrolanguage code for the downgrade group (ElevenLabs/OpenAI/
            // Cartesia use `zh`). Cantonese has no ISO-639-1 form → `yue`.
            CmnCn | CmnTw => "zh",
            YueHk => "yue",
            HiIn => "hi",
            BnIn => "bn",
            TaIn => "ta",
            TeIn => "te",
            GuIn => "gu",
            KnIn => "kn",
            MlIn => "ml",
            MrIn => "mr",
            PaIn => "pa",
            OrIn => "or",
            ArSa => "ar",
            ViVn => "vi",
            ThTh => "th",
            IdId => "id",
            MsMy => "ms",
            HeIl => "he",
        }
    }

    /// The UPPERCASE region subtag (`"US"`, `"CN"`). Empty for [`Auto`](Self::Auto).
    pub const fn region(&self) -> &'static str {
        use CanonicalLanguage::*;
        match self {
            Auto => "",
            EnUs | EsUs => "US",
            EnGb => "GB",
            EnIn | HiIn | BnIn | TaIn | TeIn | GuIn | KnIn | MlIn | MrIn | PaIn | OrIn => "IN",
            EnAu => "AU",
            EnZa => "ZA",
            EnNz => "NZ",
            EsEs => "ES",
            EsMx => "MX",
            FrFr => "FR",
            FrCa => "CA",
            PtBr => "BR",
            PtPt => "PT",
            DeDe => "DE",
            ItIt => "IT",
            NlNl => "NL",
            RuRu => "RU",
            TrTr => "TR",
            PlPl => "PL",
            SvSe => "SE",
            NbNo => "NO",
            DaDk => "DK",
            FiFi => "FI",
            UkUa => "UA",
            CsCz => "CZ",
            ElGr => "GR",
            RoRo => "RO",
            HuHu => "HU",
            JaJp => "JP",
            KoKr => "KR",
            CmnCn => "CN",
            CmnTw => "TW",
            YueHk => "HK",
            ArSa => "SA",
            ViVn => "VN",
            ThTh => "TH",
            IdId => "ID",
            MsMy => "MY",
            HeIl => "IL",
        }
    }

    /// The language subtag — `"en"`, but `"cmn"`/`"yue"` for Chinese (NOT the
    /// `zh` macrolanguage). This is the disambiguating subtag the BCP-47-script
    /// and ISO-639-3 mappers (Google STT, Speechmatics) need.
    pub const fn lang_subtag(&self) -> &'static str {
        use CanonicalLanguage::*;
        match self {
            CmnCn | CmnTw => "cmn",
            YueHk => "yue",
            // For every other variant the language subtag IS the ISO-639-1 code.
            other => other.iso639_1(),
        }
    }

    /// Whether this is the [`Auto`](Self::Auto) detection sentinel.
    #[inline]
    pub const fn is_auto(&self) -> bool {
        matches!(self, CanonicalLanguage::Auto)
    }
}

impl fmt::Display for CanonicalLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_bcp47())
    }
}

// =============================================================================
// Alias Table
// =============================================================================

/// Alias spelling → canonical variant. Looked up after normalization
/// (lowercase; `_`/space → `-`) by [`super::resolve`].
///
/// Two alias classes are folded here:
/// * (A) bare-ISO / language-name / ISO-639-2/3 shorthands → default-region
///   canonical (`"english"`/`"eng"`/`"en"` → `en-US`, `"spanish"` → `es-ES`);
/// * (B) region-qualified + vendor-quirk spellings → exact canonical
///   (`"us-en"` → `en-US`, `"od-IN"` → `or-IN`, `"cmn-Hans-CN"` → `cmn-CN`).
///
/// The canonical region-qualified spellings (`"en-us"`, `"es-mx"`) map to
/// themselves (identity) so an already-canonical token always resolves. Native
/// provider codes a client might echo back (Google's `cmn-Hans-CN`, Sarvam's
/// `od-IN`) are included so they resolve too — the infer chassis's
/// "exact-canonical-always-resolves" rule.
///
/// Keys MUST already be normalized (lowercase, `-`-separated) since the resolver
/// normalizes the *input* before matching.
pub const LANG_ALIASES: &[(&str, CanonicalLanguage)] = {
    use CanonicalLanguage::*;
    &[
        // ---- Auto / detection sentinels -------------------------------------
        ("auto", Auto),
        ("detect", Auto),
        ("any", Auto),
        ("multilingual", Auto),
        ("multi", Auto),
        // ---- English --------------------------------------------------------
        ("en", EnUs),
        ("en-us", EnUs),
        ("us-en", EnUs), // the live-proven reversed-order bug — fold explicitly
        ("eng", EnUs),
        ("english", EnUs),
        ("en-gb", EnGb),
        ("british", EnGb),
        ("uk-en", EnGb),
        ("en-in", EnIn),
        ("en-au", EnAu),
        ("en-za", EnZa),
        ("en-nz", EnNz),
        // ---- Spanish --------------------------------------------------------
        ("es", EsEs),
        ("es-es", EsEs),
        ("spa", EsEs),
        ("spanish", EsEs),
        ("castilian", EsEs),
        ("es-mx", EsMx),
        ("es-us", EsUs),
        // ---- French ---------------------------------------------------------
        ("fr", FrFr),
        ("fr-fr", FrFr),
        ("fra", FrFr),
        ("fre", FrFr),
        ("french", FrFr),
        ("fr-ca", FrCa),
        // ---- Portuguese -----------------------------------------------------
        ("pt", PtBr),
        ("pt-br", PtBr),
        ("por", PtBr),
        ("portuguese", PtBr),
        ("pt-pt", PtPt),
        // ---- German ---------------------------------------------------------
        ("de", DeDe),
        ("de-de", DeDe),
        ("deu", DeDe),
        ("ger", DeDe),
        ("german", DeDe),
        // ---- Italian --------------------------------------------------------
        ("it", ItIt),
        ("it-it", ItIt),
        ("ita", ItIt),
        ("italian", ItIt),
        // ---- Dutch ----------------------------------------------------------
        ("nl", NlNl),
        ("nl-nl", NlNl),
        ("nld", NlNl),
        ("dut", NlNl),
        ("dutch", NlNl),
        // ---- Russian --------------------------------------------------------
        ("ru", RuRu),
        ("ru-ru", RuRu),
        ("rus", RuRu),
        ("russian", RuRu),
        // ---- Turkish --------------------------------------------------------
        ("tr", TrTr),
        ("tr-tr", TrTr),
        ("tur", TrTr),
        ("turkish", TrTr),
        // ---- Polish ---------------------------------------------------------
        ("pl", PlPl),
        ("pl-pl", PlPl),
        ("pol", PlPl),
        ("polish", PlPl),
        // ---- Swedish --------------------------------------------------------
        ("sv", SvSe),
        ("sv-se", SvSe),
        ("swe", SvSe),
        ("swedish", SvSe),
        // ---- Norwegian (nb<->no fold; Bokmål default) -----------------------
        ("nb", NbNo),
        ("no", NbNo),
        ("nn", NbNo),
        ("nb-no", NbNo),
        ("no-no", NbNo),
        ("nob", NbNo),
        ("nor", NbNo),
        ("norwegian", NbNo),
        // ---- Danish ---------------------------------------------------------
        ("da", DaDk),
        ("da-dk", DaDk),
        ("dan", DaDk),
        ("danish", DaDk),
        // ---- Finnish --------------------------------------------------------
        ("fi", FiFi),
        ("fi-fi", FiFi),
        ("fin", FiFi),
        ("finnish", FiFi),
        // ---- Ukrainian ------------------------------------------------------
        ("uk", UkUa),
        ("uk-ua", UkUa),
        ("ukr", UkUa),
        ("ukrainian", UkUa),
        // ---- Czech ----------------------------------------------------------
        ("cs", CsCz),
        ("cs-cz", CsCz),
        ("cze", CsCz),
        ("ces", CsCz),
        ("czech", CsCz),
        // ---- Greek ----------------------------------------------------------
        ("el", ElGr),
        ("el-gr", ElGr),
        ("ell", ElGr),
        ("gre", ElGr),
        ("greek", ElGr),
        // ---- Romanian -------------------------------------------------------
        ("ro", RoRo),
        ("ro-ro", RoRo),
        ("ron", RoRo),
        ("rum", RoRo),
        ("romanian", RoRo),
        // ---- Hungarian ------------------------------------------------------
        ("hu", HuHu),
        ("hu-hu", HuHu),
        ("hun", HuHu),
        ("hungarian", HuHu),
        // ---- Japanese -------------------------------------------------------
        ("ja", JaJp),
        ("ja-jp", JaJp),
        ("jpn", JaJp),
        ("japanese", JaJp),
        // ---- Korean ---------------------------------------------------------
        ("ko", KoKr),
        ("ko-kr", KoKr),
        ("kor", KoKr),
        ("korean", KoKr),
        // ---- Mandarin (zh<->cmn fold; Simplified/China default) -------------
        ("zh", CmnCn),
        ("zh-cn", CmnCn),
        ("zh-hans", CmnCn),
        ("zh-hans-cn", CmnCn),
        ("zh-hk", CmnCn), // ambiguous -> default Mandarin (Cantonese must say yue)
        ("cmn", CmnCn),
        ("cmn-cn", CmnCn),
        ("cmn-hans", CmnCn),
        ("cmn-hans-cn", CmnCn), // Google STT native echo
        ("mandarin", CmnCn),
        ("chinese", CmnCn),
        ("zho", CmnCn),
        ("chi", CmnCn),
        // ---- Mandarin Traditional / Taiwan ----------------------------------
        ("zh-tw", CmnTw),
        ("zh-hant", CmnTw),
        ("zh-hant-tw", CmnTw),
        ("cmn-tw", CmnTw),
        ("cmn-hant", CmnTw),
        ("cmn-hant-tw", CmnTw),
        // ---- Cantonese (yue<->zh-HK fold) -----------------------------------
        ("yue", YueHk),
        ("yue-hk", YueHk),
        ("yue-hant-hk", YueHk), // Google STT native echo
        ("cantonese", YueHk),
        ("zh-yue", YueHk),
        ("zh-hant-hk", YueHk),
        // ---- Hindi ----------------------------------------------------------
        ("hi", HiIn),
        ("hi-in", HiIn),
        ("hin", HiIn),
        ("hindi", HiIn),
        // ---- Bengali --------------------------------------------------------
        ("bn", BnIn),
        ("bn-in", BnIn),
        ("ben", BnIn),
        ("bengali", BnIn),
        // ---- Tamil ----------------------------------------------------------
        ("ta", TaIn),
        ("ta-in", TaIn),
        ("tam", TaIn),
        ("tamil", TaIn),
        // ---- Telugu ---------------------------------------------------------
        ("te", TeIn),
        ("te-in", TeIn),
        ("tel", TeIn),
        ("telugu", TeIn),
        // ---- Gujarati -------------------------------------------------------
        ("gu", GuIn),
        ("gu-in", GuIn),
        ("guj", GuIn),
        ("gujarati", GuIn),
        // ---- Kannada --------------------------------------------------------
        ("kn", KnIn),
        ("kn-in", KnIn),
        ("kan", KnIn),
        ("kannada", KnIn),
        // ---- Malayalam ------------------------------------------------------
        ("ml", MlIn),
        ("ml-in", MlIn),
        ("mal", MlIn),
        ("malayalam", MlIn),
        // ---- Marathi --------------------------------------------------------
        ("mr", MrIn),
        ("mr-in", MrIn),
        ("mar", MrIn),
        ("marathi", MrIn),
        // ---- Punjabi --------------------------------------------------------
        ("pa", PaIn),
        ("pa-in", PaIn),
        ("pan", PaIn),
        ("punjabi", PaIn),
        ("panjabi", PaIn),
        // ---- Odia (canonical `or`; Sarvam's `od-IN` folds IN here) ----------
        ("or", OrIn),
        ("or-in", OrIn),
        ("ori", OrIn),
        ("oriya", OrIn),
        ("odia", OrIn),
        ("od", OrIn),
        ("od-in", OrIn), // Sarvam nonstandard spelling -> canonical or-IN
        // ---- Arabic (arb/ara folds) -----------------------------------------
        ("ar", ArSa),
        ("ar-sa", ArSa),
        ("arb", ArSa),
        ("ara", ArSa),
        ("arabic", ArSa),
        // ---- Vietnamese -----------------------------------------------------
        ("vi", ViVn),
        ("vi-vn", ViVn),
        ("vie", ViVn),
        ("vietnamese", ViVn),
        // ---- Thai -----------------------------------------------------------
        ("th", ThTh),
        ("th-th", ThTh),
        ("tha", ThTh),
        ("thai", ThTh),
        // ---- Indonesian -----------------------------------------------------
        ("id", IdId),
        ("id-id", IdId),
        ("ind", IdId),
        ("indonesian", IdId),
        // ---- Malay ----------------------------------------------------------
        ("ms", MsMy),
        ("ms-my", MsMy),
        ("msa", MsMy),
        ("may", MsMy),
        ("malay", MsMy),
        // ---- Hebrew ---------------------------------------------------------
        ("he", HeIl),
        ("he-il", HeIl),
        ("heb", HeIl),
        ("hebrew", HeIl),
        ("iw", HeIl), // legacy ISO code for Hebrew
    ]
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_bcp47_is_region_qualified() {
        assert_eq!(CanonicalLanguage::EnUs.as_bcp47(), "en-US");
        assert_eq!(CanonicalLanguage::CmnCn.as_bcp47(), "cmn-CN");
        assert_eq!(CanonicalLanguage::OrIn.as_bcp47(), "or-IN");
        assert_eq!(CanonicalLanguage::Auto.as_bcp47(), "auto");
    }

    #[test]
    fn iso639_1_drops_region() {
        assert_eq!(CanonicalLanguage::EnUs.iso639_1(), "en");
        assert_eq!(CanonicalLanguage::EnGb.iso639_1(), "en");
        assert_eq!(CanonicalLanguage::EsMx.iso639_1(), "es");
        // Mandarin downgrades to the macrolanguage `zh`; Cantonese stays `yue`.
        assert_eq!(CanonicalLanguage::CmnCn.iso639_1(), "zh");
        assert_eq!(CanonicalLanguage::YueHk.iso639_1(), "yue");
        assert_eq!(CanonicalLanguage::Auto.iso639_1(), "");
    }

    #[test]
    fn region_and_lang_subtag() {
        assert_eq!(CanonicalLanguage::EnUs.region(), "US");
        assert_eq!(CanonicalLanguage::FrCa.region(), "CA");
        assert_eq!(CanonicalLanguage::CmnTw.region(), "TW");
        assert_eq!(CanonicalLanguage::CmnCn.lang_subtag(), "cmn");
        assert_eq!(CanonicalLanguage::YueHk.lang_subtag(), "yue");
        assert_eq!(CanonicalLanguage::EnUs.lang_subtag(), "en");
    }

    #[test]
    fn bcp47_region_segment_matches_region_accessor() {
        // For every non-Auto variant, region() must equal the segment after '-'.
        for &c in CanonicalLanguage::all() {
            let bcp = c.as_bcp47();
            let region_from_bcp = bcp.split_once('-').map(|(_, r)| r).unwrap_or("");
            assert_eq!(
                c.region(),
                region_from_bcp,
                "region() disagrees with as_bcp47() for {c:?}"
            );
        }
    }

    #[test]
    fn serde_uses_bcp47_spelling() {
        let json = serde_json::to_string(&CanonicalLanguage::EnUs).unwrap();
        assert_eq!(json, "\"en-US\"");
        let back: CanonicalLanguage = serde_json::from_str("\"cmn-CN\"").unwrap();
        assert_eq!(back, CanonicalLanguage::CmnCn);
    }

    #[test]
    fn alias_table_keys_are_prenormalized() {
        // Every key must already be lowercase and '-'-separated (the resolver
        // normalizes the *input*, not the table).
        for (k, _) in LANG_ALIASES {
            assert_eq!(*k, k.to_lowercase(), "alias key not lowercase: {k}");
            assert!(!k.contains('_'), "alias key contains '_': {k}");
            assert!(!k.contains(' '), "alias key contains space: {k}");
        }
    }

    #[test]
    fn alias_table_has_no_duplicate_keys() {
        let mut seen = std::collections::HashSet::new();
        for (k, _) in LANG_ALIASES {
            assert!(seen.insert(*k), "duplicate alias key: {k}");
        }
    }

    #[test]
    fn every_canonical_has_a_self_alias() {
        // The canonical lowercased bcp47 must resolve back to itself, so an
        // already-canonical token is always recognized (identity rule).
        for &c in CanonicalLanguage::all() {
            let key = c.as_bcp47().to_lowercase();
            let found = LANG_ALIASES.iter().find(|(k, _)| *k == key);
            assert!(found.is_some(), "no self-alias for {} ({key})", c.as_bcp47());
            assert_eq!(found.unwrap().1, c, "self-alias maps elsewhere for {key}");
        }
    }
}

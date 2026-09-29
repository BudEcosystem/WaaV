//! One spelling per language for `bud.voice.detected_language` (FRD-021 §6.1, Phase 5).
//!
//! Vendors answer "what language was this" in their own notation: Deepgram `en`, Google and Azure
//! `en-US`, ElevenLabs `eng` (ISO 639-3), Whisper-style backends `english`. Recorded verbatim, one
//! language became four rows in every "by language" breakdown. The value recorded is the lowercase
//! BCP-47 PRIMARY language subtag — the ISO 639-1 two-letter code wherever one exists — so all four
//! read `en`.
//!
//! Deliberately a closed table rather than a guess: a code or name that is not listed keeps its own
//! primary subtag (`en-ZZ` → `en`, `yue-HK` → `yue`), so an unknown language is still counted, under
//! the vendor's own code, and never folded into another.

/// ISO 639-1 codes, each with the ISO 639-2/T, 639-2/B and 639-3 codes, deprecated two-letter codes
/// and English names vendors are known to answer with. A few languages without a 639-1 code are
/// listed under their 639-3 code, so that their English name still resolves to it.
const LANGUAGES: &[(&str, &[&str])] = &[
    ("af", &["afr", "afrikaans"]),
    ("am", &["amh", "amharic"]),
    ("ar", &["ara", "arabic"]),
    ("as", &["asm", "assamese"]),
    ("az", &["aze", "azerbaijani"]),
    ("ba", &["bak", "bashkir"]),
    ("be", &["bel", "belarusian"]),
    ("bg", &["bul", "bulgarian"]),
    ("bn", &["ben", "bengali", "bangla"]),
    ("bo", &["bod", "tib", "tibetan"]),
    ("br", &["bre", "breton"]),
    ("bs", &["bos", "bosnian"]),
    ("ca", &["cat", "catalan", "valencian"]),
    ("cs", &["ces", "cze", "czech"]),
    ("cy", &["cym", "wel", "welsh"]),
    ("da", &["dan", "danish"]),
    ("de", &["deu", "ger", "german"]),
    ("el", &["ell", "gre", "greek"]),
    ("en", &["eng", "english"]),
    ("eo", &["epo", "esperanto"]),
    ("es", &["spa", "spanish", "castilian"]),
    ("et", &["est", "estonian"]),
    ("eu", &["eus", "baq", "basque"]),
    ("fa", &["fas", "per", "persian", "farsi"]),
    ("fi", &["fin", "finnish"]),
    ("fo", &["fao", "faroese"]),
    ("fr", &["fra", "fre", "french"]),
    ("fy", &["fry", "frisian", "western frisian"]),
    ("ga", &["gle", "irish"]),
    ("gd", &["gla", "gaelic", "scottish gaelic"]),
    ("gl", &["glg", "galician"]),
    ("gu", &["guj", "gujarati"]),
    ("ha", &["hau", "hausa"]),
    ("he", &["heb", "hebrew", "iw"]),
    ("hi", &["hin", "hindi"]),
    ("hr", &["hrv", "croatian"]),
    ("ht", &["hat", "haitian", "haitian creole"]),
    ("hu", &["hun", "hungarian"]),
    ("hy", &["hye", "arm", "armenian"]),
    ("id", &["ind", "indonesian", "in"]),
    ("ig", &["ibo", "igbo"]),
    ("is", &["isl", "ice", "icelandic"]),
    ("it", &["ita", "italian"]),
    ("ja", &["jpn", "japanese"]),
    ("jv", &["jav", "javanese", "jw"]),
    ("ka", &["kat", "geo", "georgian"]),
    ("kk", &["kaz", "kazakh"]),
    ("km", &["khm", "khmer", "central khmer"]),
    ("kn", &["kan", "kannada"]),
    ("ko", &["kor", "korean"]),
    ("ku", &["kur", "kurdish"]),
    ("ky", &["kir", "kyrgyz", "kirghiz"]),
    ("la", &["lat", "latin"]),
    ("lb", &["ltz", "luxembourgish", "letzeburgesch"]),
    ("lg", &["lug", "ganda", "luganda"]),
    ("ln", &["lin", "lingala"]),
    ("lo", &["lao"]),
    ("lt", &["lit", "lithuanian"]),
    ("lv", &["lav", "latvian"]),
    ("mg", &["mlg", "malagasy"]),
    ("mi", &["mri", "mao", "maori"]),
    ("mk", &["mkd", "mac", "macedonian"]),
    ("ml", &["mal", "malayalam"]),
    ("mn", &["mon", "mongolian"]),
    ("mr", &["mar", "marathi"]),
    ("ms", &["msa", "may", "zsm", "malay"]),
    ("mt", &["mlt", "maltese"]),
    ("my", &["mya", "bur", "burmese", "myanmar"]),
    (
        "nb",
        &[
            "nob",
            "bokmal",
            "bokmål",
            "norwegian bokmal",
            "norwegian bokmål",
        ],
    ),
    ("ne", &["nep", "npi", "nepali"]),
    ("nl", &["nld", "dut", "dutch", "flemish"]),
    ("nn", &["nno", "nynorsk", "norwegian nynorsk"]),
    ("no", &["nor", "norwegian"]),
    ("ny", &["nya", "chichewa", "nyanja"]),
    ("oc", &["oci", "occitan"]),
    ("om", &["orm", "oromo"]),
    ("or", &["ori", "ory", "odia", "oriya"]),
    ("pa", &["pan", "punjabi", "panjabi"]),
    ("pl", &["pol", "polish"]),
    ("ps", &["pus", "pbt", "pashto", "pushto"]),
    ("pt", &["por", "portuguese"]),
    ("ro", &["ron", "rum", "romanian", "moldavian", "moldovan"]),
    ("ru", &["rus", "russian"]),
    ("rw", &["kin", "kinyarwanda"]),
    ("sa", &["san", "sanskrit"]),
    ("sd", &["snd", "sindhi"]),
    ("si", &["sin", "sinhala", "sinhalese"]),
    ("sk", &["slk", "slo", "slovak"]),
    ("sl", &["slv", "slovenian", "slovene"]),
    ("sn", &["sna", "shona"]),
    ("so", &["som", "somali"]),
    ("sq", &["sqi", "alb", "albanian"]),
    ("sr", &["srp", "serbian"]),
    ("su", &["sun", "sundanese"]),
    ("sv", &["swe", "swedish"]),
    ("sw", &["swa", "swh", "swahili"]),
    ("ta", &["tam", "tamil"]),
    ("te", &["tel", "telugu"]),
    ("tg", &["tgk", "tajik"]),
    ("th", &["tha", "thai"]),
    ("ti", &["tir", "tigrinya"]),
    ("tk", &["tuk", "turkmen"]),
    ("tl", &["tgl", "tagalog"]),
    ("tr", &["tur", "turkish"]),
    ("tt", &["tat", "tatar"]),
    ("ug", &["uig", "uyghur", "uighur"]),
    ("uk", &["ukr", "ukrainian"]),
    ("ur", &["urd", "urdu"]),
    ("uz", &["uzb", "uzbek"]),
    ("vi", &["vie", "vietnamese"]),
    ("xh", &["xho", "xhosa"]),
    ("yi", &["yid", "yiddish", "ji"]),
    ("yo", &["yor", "yoruba"]),
    // Mandarin (`cmn`, Google's `cmn-Hans-CN`) is counted as Chinese, as every vendor that answers
    // in two letters already does.
    ("zh", &["zho", "chi", "cmn", "chinese", "mandarin"]),
    ("zu", &["zul", "zulu"]),
    // No ISO 639-1 code: the 639-3 code is the primary subtag, and the name maps to it.
    ("yue", &["cantonese"]),
    ("haw", &["hawaiian"]),
    ("fil", &["filipino"]),
    ("ceb", &["cebuano"]),
];

/// Values that say no language was determined — recorded as absent, not as a language.
const UNDETERMINED: &[&str] = &["und", "unknown", "none", "null", "n/a"];

fn lookup(token: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(code, aliases)| *code == token || aliases.contains(&token))
        .map(|(code, _)| *code)
}

/// The primary language subtag of a vendor's detected-language value, lowercase: `en` for `en`,
/// `eng`, `English` and `en-US` alike. `None` for an absent, empty or undetermined value.
pub fn normalize_language(raw: Option<&str>) -> Option<String> {
    let lower = raw?.trim().to_lowercase();
    // A vendor's field, bounded before it becomes an attribute.
    if lower.is_empty() || lower.len() > 64 || lower.chars().any(char::is_control) {
        return None;
    }
    if UNDETERMINED.contains(&lower.as_str()) {
        return None;
    }
    // A whole name first: `haitian creole`, `norwegian nynorsk`.
    if let Some(code) = lookup(&lower) {
        return Some(code.to_string());
    }
    // Then the primary subtag of a tag: `en-US`, `en_us`, `cmn-Hans-CN`, `pt-BR.UTF-8`.
    let primary = lower
        .split(['-', '_', '.', '@'])
        .next()
        .unwrap_or_default()
        .trim();
    if primary.is_empty() || UNDETERMINED.contains(&primary) {
        return None;
    }
    if let Some(code) = lookup(primary) {
        return Some(code.to_string());
    }
    // Unlisted: a language subtag (two or three letters, as every assigned one is) keeps its own
    // spelling; anything else — a name this table does not know — is kept whole rather than cut
    // at a hyphen into something that is not a language.
    if (2..=3).contains(&primary.len()) && primary.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(primary.to_string())
    } else {
        Some(lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(raw: &str) -> Option<String> {
        normalize_language(Some(raw))
    }

    #[test]
    fn every_vendor_spelling_of_english_is_en() {
        for raw in [
            "en", "EN", "eng", "english", "English", " en-US ", "en_us", "en-GB",
        ] {
            assert_eq!(norm(raw).as_deref(), Some("en"), "{raw}");
        }
    }

    #[test]
    fn three_letter_codes_and_names_map_to_their_two_letter_code() {
        for (raw, want) in [
            ("spa", "es"),
            ("fra", "fr"),
            ("fre", "fr"),
            ("deu", "de"),
            ("ger", "de"),
            ("hin", "hi"),
            ("ara", "ar"),
            ("zho", "zh"),
            ("chi", "zh"),
            ("jpn", "ja"),
            ("kor", "ko"),
            ("por", "pt"),
            ("rus", "ru"),
            ("ita", "it"),
            ("nld", "nl"),
            ("dut", "nl"),
            ("tur", "tr"),
            ("pol", "pl"),
            ("swe", "sv"),
            ("ukr", "uk"),
            ("vie", "vi"),
            ("tha", "th"),
            ("ind", "id"),
            ("msa", "ms"),
            ("may", "ms"),
            ("ben", "bn"),
            ("tam", "ta"),
            ("tel", "te"),
            ("urd", "ur"),
            ("mar", "mr"),
            ("guj", "gu"),
            ("kan", "kn"),
            ("mal", "ml"),
            ("pan", "pa"),
            ("heb", "he"),
            ("ell", "el"),
            ("gre", "el"),
            ("ces", "cs"),
            ("cze", "cs"),
            ("ron", "ro"),
            ("rum", "ro"),
            ("hun", "hu"),
            ("fin", "fi"),
            ("dan", "da"),
            ("nor", "no"),
            ("srp", "sr"),
            ("hrv", "hr"),
            ("bos", "bs"),
            ("spanish", "es"),
            ("French", "fr"),
            ("mandarin", "zh"),
            ("haitian creole", "ht"),
            ("Norwegian Nynorsk", "nn"),
            ("cmn-Hans-CN", "zh"),
            ("pt-BR", "pt"),
            ("zh_TW", "zh"),
            ("iw", "he"),
            ("hi-IN", "hi"),
            ("cantonese", "yue"),
        ] {
            assert_eq!(norm(raw).as_deref(), Some(want), "{raw}");
        }
    }

    #[test]
    fn an_unlisted_language_keeps_its_own_region_stripped_code() {
        assert_eq!(norm("yue-HK").as_deref(), Some("yue"));
        assert_eq!(norm("ab").as_deref(), Some("ab"));
        assert_eq!(norm("xx-ZZ").as_deref(), Some("xx"));
        // A name the table does not know is kept whole, never cut into a fake subtag.
        assert_eq!(norm("serbo-croatian").as_deref(), Some("serbo-croatian"));
    }

    #[test]
    fn absent_empty_and_undetermined_are_none() {
        assert_eq!(normalize_language(None), None);
        for raw in ["", "   ", "und", "UND", "unknown", "und-US"] {
            assert_eq!(norm(raw), None, "{raw:?}");
        }
        assert_eq!(norm(&"x".repeat(65)), None);
        assert_eq!(norm("en\nforged"), None);
    }

    #[test]
    fn the_table_is_consistent() {
        let mut codes: Vec<&str> = LANGUAGES.iter().map(|(c, _)| *c).collect();
        codes.sort_unstable();
        let n = codes.len();
        codes.dedup();
        assert_eq!(n, codes.len(), "a code is listed twice");
        let mut aliases: Vec<&str> = LANGUAGES
            .iter()
            .flat_map(|(_, a)| a.iter().copied())
            .collect();
        aliases.sort_unstable();
        let n = aliases.len();
        aliases.dedup();
        assert_eq!(n, aliases.len(), "an alias is listed under two codes");
        for (code, list) in LANGUAGES {
            assert_eq!(*code, code.to_lowercase());
            for a in *list {
                assert_eq!(*a, a.to_lowercase(), "{a}");
                assert!(!codes.contains(a), "{a} is both a code and an alias");
            }
        }
    }
}

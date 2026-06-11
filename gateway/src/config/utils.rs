/// True when a configured credential value is a PLACEHOLDER, not a real secret.
///
/// The shipped `config.yaml` uses values like `"your-deepgram-api-key"`; before
/// this check they were returned verbatim by `get_api_key` and sent to the
/// provider (observed live: Deepgram 401 with the literal placeholder string).
/// A placeholder is treated as UNSET so the documented `# ENV:` fallback
/// actually applies. Scope: credentials only — never general config strings.
pub(crate) fn is_placeholder_credential(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return true;
    }
    let lower = v.to_ascii_lowercase();
    lower.starts_with("your-")
        || lower.starts_with("your_")
        || matches!(lower.as_str(), "changeme" | "change-me" | "change_me" | "placeholder" | "todo")
        || (lower.len() >= 3 && lower.bytes().all(|b| b == b'x'))
}

/// Parse a boolean value from a string, supporting multiple formats
///
/// Accepts: "true", "false", "1", "0", "yes", "no" (case insensitive)
pub fn parse_bool(s: &str) -> Option<bool> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bool_true_variants() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("TRUE"), Some(true));
        assert_eq!(parse_bool("1"), Some(true));
        assert_eq!(parse_bool("yes"), Some(true));
        assert_eq!(parse_bool("YES"), Some(true));
        assert_eq!(parse_bool("Yes"), Some(true));
    }

    #[test]
    fn test_parse_bool_false_variants() {
        assert_eq!(parse_bool("false"), Some(false));
        assert_eq!(parse_bool("FALSE"), Some(false));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("no"), Some(false));
        assert_eq!(parse_bool("NO"), Some(false));
        assert_eq!(parse_bool("No"), Some(false));
    }

    #[test]
    fn test_parse_bool_invalid() {
        assert_eq!(parse_bool("invalid"), None);
        assert_eq!(parse_bool("2"), None);
        assert_eq!(parse_bool(""), None);
        assert_eq!(parse_bool("maybe"), None);
    }
}

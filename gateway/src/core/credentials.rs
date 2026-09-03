/// Normalize an explicit API key supplied through a request/config object.
///
/// Empty and whitespace-only strings are not credentials. Non-empty values are
/// trimmed so accidental surrounding whitespace does not break provider auth.
pub(crate) fn explicit_api_key(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read an API key from an environment variable with the same normalization as
/// explicit config keys.
pub(crate) fn env_api_key(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .and_then(|value| explicit_api_key(&value))
}

#[cfg(test)]
mod tests {
    #[test]
    fn p5_explicit_api_key_trims_and_rejects_blank_values() {
        assert_eq!(super::explicit_api_key(""), None);
        assert_eq!(super::explicit_api_key(" \n\t"), None);
        assert_eq!(
            super::explicit_api_key("  provider-key  "),
            Some("provider-key".to_string())
        );
    }
}

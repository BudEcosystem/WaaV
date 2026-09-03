use anyhow::{Result, anyhow};

use crate::config::utils::parse_bool;

pub(crate) const WAAV_SKIP_MODEL_HASH_CHECK_ENV: &str = "WAAV_SKIP_MODEL_HASH_CHECK";

pub(crate) fn skip_model_hash_check_from_env() -> Result<bool> {
    match std::env::var(WAAV_SKIP_MODEL_HASH_CHECK_ENV) {
        Ok(value) => parse_skip_model_hash_check(&value).ok_or_else(|| {
            anyhow!(
                "{WAAV_SKIP_MODEL_HASH_CHECK_ENV} must be a boolean (true/false, 1/0, yes/no), got {value:?}"
            )
        }),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(std::env::VarError::NotUnicode(_)) => Err(anyhow!(
            "{WAAV_SKIP_MODEL_HASH_CHECK_ENV} must be valid UTF-8 boolean text"
        )),
    }
}

fn parse_skip_model_hash_check(value: &str) -> Option<bool> {
    parse_bool(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvGuard;

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe { std::env::remove_var(WAAV_SKIP_MODEL_HASH_CHECK_ENV) };
        }
    }

    fn set_env(value: Option<&str>) -> EnvGuard {
        match value {
            Some(value) => unsafe { std::env::set_var(WAAV_SKIP_MODEL_HASH_CHECK_ENV, value) },
            None => unsafe { std::env::remove_var(WAAV_SKIP_MODEL_HASH_CHECK_ENV) },
        }
        EnvGuard
    }

    #[test]
    #[serial]
    fn unset_defaults_to_hash_check_enabled() {
        let _guard = set_env(None);

        assert!(!skip_model_hash_check_from_env().unwrap());
    }

    #[test]
    #[serial]
    fn explicit_true_values_skip_hash_check() {
        for value in ["1", "true", "yes", "TRUE"] {
            let _guard = set_env(Some(value));

            assert!(
                skip_model_hash_check_from_env().unwrap(),
                "{value:?} should skip hash checking"
            );
        }
    }

    #[test]
    #[serial]
    fn explicit_false_values_keep_hash_check_enabled() {
        for value in ["0", "false", "no", "FALSE"] {
            let _guard = set_env(Some(value));

            assert!(
                !skip_model_hash_check_from_env().unwrap(),
                "{value:?} should keep hash checking enabled"
            );
        }
    }

    #[test]
    #[serial]
    fn malformed_explicit_value_is_rejected() {
        let _guard = set_env(Some("off"));

        let err = skip_model_hash_check_from_env().unwrap_err();
        assert!(
            err.to_string().contains(WAAV_SKIP_MODEL_HASH_CHECK_ENV),
            "error should name the env var: {err}"
        );
    }
}

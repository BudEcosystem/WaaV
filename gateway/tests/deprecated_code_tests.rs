//! TDD tests for Fix 6: Remove deprecated Hume EVI v1/v2 support
//!
//! These tests verify that:
//! 1. Deprecated EVI v1/v2 versions are rejected with proper error messages
//! 2. V3 and V4-mini continue to work correctly
//! 3. Migration guidance is provided in error messages
//! 4. Deserialization of v1/v2 from JSON fails with helpful errors

use std::str::FromStr;

// =============================================================================
// Test Constants
// =============================================================================

/// Expected error message substring for deprecated versions (uses "sunset" terminology)
const DEPRECATED_ERROR_MSG: &str = "sunset";

/// Expected migration URL substring
const MIGRATION_URL_SUBSTRING: &str = "hume.ai";

// =============================================================================
// Mock Types for Testing (simulates the expected behavior after fix)
// =============================================================================

/// Error type for EVI configuration errors
#[derive(Debug, Clone, PartialEq)]
pub enum EVIConfigError {
    /// Version is deprecated and no longer supported
    DeprecatedVersion {
        version: String,
        message: String,
        migration_guide: String,
    },
    /// Unknown version number
    UnknownVersion(String),
    /// API key is required
    MissingApiKey,
    /// Invalid sample rate
    InvalidSampleRate,
    /// Invalid channel count
    InvalidChannels,
}

impl std::fmt::Display for EVIConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EVIConfigError::DeprecatedVersion {
                version,
                message,
                migration_guide,
            } => {
                write!(
                    f,
                    "EVI version {} is deprecated: {}. Migration guide: {}",
                    version, message, migration_guide
                )
            }
            EVIConfigError::UnknownVersion(v) => write!(f, "Unknown EVI version: {}", v),
            EVIConfigError::MissingApiKey => write!(f, "API key is required"),
            EVIConfigError::InvalidSampleRate => write!(f, "Sample rate must be greater than 0"),
            EVIConfigError::InvalidChannels => write!(f, "Channels must be greater than 0"),
        }
    }
}

impl std::error::Error for EVIConfigError {}

/// EVI Version enum (after fix - V1/V2 removed)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EVIVersionFixed {
    /// EVI version 3 (current, English only)
    #[default]
    V3,
    /// EVI version 4-mini (multilingual, lower latency)
    V4Mini,
}

impl EVIVersionFixed {
    /// Parse from legacy version number
    pub fn from_legacy(version: &str) -> Result<Self, EVIConfigError> {
        match version {
            "1" | "2" => Err(EVIConfigError::DeprecatedVersion {
                version: version.to_string(),
                message: "Hume EVI v1/v2 were sunset on August 30, 2025. Please migrate to v3 or v4-mini.".to_string(),
                migration_guide: "https://dev.hume.ai/docs/evi-version".to_string(),
            }),
            "3" => Ok(EVIVersionFixed::V3),
            "4-mini" => Ok(EVIVersionFixed::V4Mini),
            _ => Err(EVIConfigError::UnknownVersion(version.to_string())),
        }
    }

    /// Get the version string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            EVIVersionFixed::V3 => "3",
            EVIVersionFixed::V4Mini => "4-mini",
        }
    }
}

impl FromStr for EVIVersionFixed {
    type Err = EVIConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        EVIVersionFixed::from_legacy(s)
    }
}

impl std::fmt::Display for EVIVersionFixed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Simulated config that validates on construction
#[derive(Debug)]
pub struct HumeEVIConfigFixed {
    pub api_key: String,
    pub evi_version: EVIVersionFixed,
    pub sample_rate: u32,
    pub channels: u8,
}

impl HumeEVIConfigFixed {
    pub fn new(api_key: impl Into<String>) -> Result<Self, EVIConfigError> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(EVIConfigError::MissingApiKey);
        }
        Ok(Self {
            api_key,
            evi_version: EVIVersionFixed::default(),
            sample_rate: 44100,
            channels: 1,
        })
    }

    pub fn with_version(mut self, version: EVIVersionFixed) -> Self {
        self.evi_version = version;
        self
    }

    pub fn with_version_str(self, version: &str) -> Result<Self, EVIConfigError> {
        let evi_version = EVIVersionFixed::from_legacy(version)?;
        Ok(Self {
            evi_version,
            ..self
        })
    }

    pub fn validate(&self) -> Result<(), EVIConfigError> {
        if self.api_key.is_empty() {
            return Err(EVIConfigError::MissingApiKey);
        }
        if self.sample_rate == 0 {
            return Err(EVIConfigError::InvalidSampleRate);
        }
        if self.channels == 0 {
            return Err(EVIConfigError::InvalidChannels);
        }
        Ok(())
    }
}

// =============================================================================
// Test 6.1: Deprecated Version Rejection
// =============================================================================

/// Test that EVI V1 is rejected with proper error
#[test]
fn test_evi_v1_rejected_with_error() {
    let result = EVIVersionFixed::from_legacy("1");

    assert!(result.is_err(), "EVI V1 should be rejected");

    let err = result.unwrap_err();
    match err {
        EVIConfigError::DeprecatedVersion {
            version,
            message,
            migration_guide,
        } => {
            assert_eq!(version, "1");
            assert!(
                message.to_lowercase().contains(DEPRECATED_ERROR_MSG),
                "Error message should mention sunset: {}",
                message
            );
            assert!(
                migration_guide.contains(MIGRATION_URL_SUBSTRING),
                "Migration guide should contain Hume URL: {}",
                migration_guide
            );
        }
        _ => panic!("Expected DeprecatedVersion error, got: {:?}", err),
    }
}

/// Test that EVI V2 is rejected with proper error
#[test]
fn test_evi_v2_rejected_with_error() {
    let result = EVIVersionFixed::from_legacy("2");

    assert!(result.is_err(), "EVI V2 should be rejected");

    let err = result.unwrap_err();
    match err {
        EVIConfigError::DeprecatedVersion {
            version,
            message,
            migration_guide,
        } => {
            assert_eq!(version, "2");
            assert!(
                message.to_lowercase().contains(DEPRECATED_ERROR_MSG),
                "Error message should mention sunset: {}",
                message
            );
            assert!(
                migration_guide.contains(MIGRATION_URL_SUBSTRING),
                "Migration guide should contain Hume URL"
            );
        }
        _ => panic!("Expected DeprecatedVersion error, got: {:?}", err),
    }
}

/// Test that error message includes sunset date
#[test]
fn test_error_message_includes_sunset_date() {
    let result = EVIVersionFixed::from_legacy("1");
    let err = result.unwrap_err();

    let err_string = err.to_string();
    assert!(
        err_string.contains("2025") || err_string.contains("sunset"),
        "Error message should mention sunset date or sunset: {}",
        err_string
    );
}

// =============================================================================
// Test 6.2: Supported Versions Work Correctly
// =============================================================================

/// Test that EVI V3 is accepted
#[test]
fn test_evi_v3_accepted() {
    let result = EVIVersionFixed::from_legacy("3");

    assert!(result.is_ok(), "EVI V3 should be accepted");
    assert_eq!(result.unwrap(), EVIVersionFixed::V3);
}

/// Test that EVI V4-mini is accepted
#[test]
fn test_evi_v4_mini_accepted() {
    let result = EVIVersionFixed::from_legacy("4-mini");

    assert!(result.is_ok(), "EVI V4-mini should be accepted");
    assert_eq!(result.unwrap(), EVIVersionFixed::V4Mini);
}

/// Test V3 as_str returns correct value
#[test]
fn test_v3_as_str() {
    assert_eq!(EVIVersionFixed::V3.as_str(), "3");
}

/// Test V4-mini as_str returns correct value
#[test]
fn test_v4_mini_as_str() {
    assert_eq!(EVIVersionFixed::V4Mini.as_str(), "4-mini");
}

/// Test default version is V3
#[test]
fn test_default_version_is_v3() {
    assert_eq!(EVIVersionFixed::default(), EVIVersionFixed::V3);
}

// =============================================================================
// Test 6.3: Unknown Version Handling
// =============================================================================

/// Test that unknown versions are rejected with proper error
#[test]
fn test_unknown_version_rejected() {
    let result = EVIVersionFixed::from_legacy("5");

    assert!(result.is_err(), "Unknown version should be rejected");

    let err = result.unwrap_err();
    match err {
        EVIConfigError::UnknownVersion(v) => {
            assert_eq!(v, "5");
        }
        _ => panic!("Expected UnknownVersion error, got: {:?}", err),
    }
}

/// Test that invalid version strings are rejected
#[test]
fn test_invalid_version_string_rejected() {
    let invalid_versions = ["", "invalid", "v3", "V3", "3.0", "3-mini"];

    for version in invalid_versions {
        let result = EVIVersionFixed::from_legacy(version);
        assert!(result.is_err(), "Version '{}' should be rejected", version);
    }
}

// =============================================================================
// Test 6.4: Config Builder Rejects Deprecated Versions
// =============================================================================

/// Test that config builder rejects deprecated version string
#[test]
fn test_config_builder_rejects_deprecated_version() {
    let config = HumeEVIConfigFixed::new("test-key").unwrap();
    let result = config.with_version_str("1");

    assert!(
        result.is_err(),
        "Config builder should reject deprecated version"
    );

    let err = result.unwrap_err();
    assert!(matches!(err, EVIConfigError::DeprecatedVersion { .. }));
}

/// Test that config builder accepts V3
#[test]
fn test_config_builder_accepts_v3() {
    let config = HumeEVIConfigFixed::new("test-key").unwrap();
    let result = config.with_version_str("3");

    assert!(result.is_ok(), "Config builder should accept V3");
    assert_eq!(result.unwrap().evi_version, EVIVersionFixed::V3);
}

/// Test that config builder accepts V4-mini
#[test]
fn test_config_builder_accepts_v4_mini() {
    let config = HumeEVIConfigFixed::new("test-key").unwrap();
    let result = config.with_version_str("4-mini");

    assert!(result.is_ok(), "Config builder should accept V4-mini");
    assert_eq!(result.unwrap().evi_version, EVIVersionFixed::V4Mini);
}

// =============================================================================
// Test 6.5: FromStr Implementation
// =============================================================================

/// Test FromStr rejects deprecated versions
#[test]
fn test_from_str_rejects_deprecated() {
    let result: Result<EVIVersionFixed, _> = "1".parse();
    assert!(result.is_err(), "FromStr should reject V1");

    let result: Result<EVIVersionFixed, _> = "2".parse();
    assert!(result.is_err(), "FromStr should reject V2");
}

/// Test FromStr accepts supported versions
#[test]
fn test_from_str_accepts_supported() {
    let result: Result<EVIVersionFixed, _> = "3".parse();
    assert!(result.is_ok(), "FromStr should accept V3");
    assert_eq!(result.unwrap(), EVIVersionFixed::V3);

    let result: Result<EVIVersionFixed, _> = "4-mini".parse();
    assert!(result.is_ok(), "FromStr should accept V4-mini");
    assert_eq!(result.unwrap(), EVIVersionFixed::V4Mini);
}

// =============================================================================
// Test 6.6: Config Validation
// =============================================================================

/// Test that valid config passes validation
#[test]
fn test_valid_config_passes_validation() {
    let config = HumeEVIConfigFixed::new("test-key").unwrap();
    assert!(config.validate().is_ok());
}

/// Test that empty API key fails validation
#[test]
fn test_empty_api_key_fails() {
    let result = HumeEVIConfigFixed::new("");
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), EVIConfigError::MissingApiKey));
}

// =============================================================================
// Test 6.7: Display Implementation
// =============================================================================

/// Test that Display shows correct version strings
#[test]
fn test_display_implementation() {
    assert_eq!(format!("{}", EVIVersionFixed::V3), "3");
    assert_eq!(format!("{}", EVIVersionFixed::V4Mini), "4-mini");
}

/// Test that error Display includes all relevant information
#[test]
fn test_error_display_includes_details() {
    let err = EVIConfigError::DeprecatedVersion {
        version: "1".to_string(),
        message: "Test message".to_string(),
        migration_guide: "https://test.url".to_string(),
    };

    let display = format!("{}", err);
    assert!(display.contains("1"), "Should contain version");
    assert!(display.contains("Test message"), "Should contain message");
    assert!(
        display.contains("https://test.url"),
        "Should contain migration guide"
    );
}

// =============================================================================
// Test 6.8: Serde Behavior (JSON Deserialization)
// =============================================================================

/// Test that JSON with deprecated version fails to parse
#[test]
fn test_json_deprecated_version_fails() {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(deserialize_with = "deserialize_evi_version")]
        version: EVIVersionFixed,
    }

    fn deserialize_evi_version<'de, D>(deserializer: D) -> Result<EVIVersionFixed, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = serde::Deserialize::deserialize(deserializer)?;
        EVIVersionFixed::from_legacy(&s).map_err(serde::de::Error::custom)
    }

    let json = r#"{"version": "1"}"#;
    let result: Result<TestConfig, _> = serde_json::from_str(json);

    assert!(
        result.is_err(),
        "JSON with deprecated version should fail to parse"
    );

    let err = result.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("deprecated"),
        "Error should mention deprecation: {}",
        err
    );
}

/// Test that JSON with supported version parses correctly
#[test]
fn test_json_supported_version_succeeds() {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(deserialize_with = "deserialize_evi_version")]
        version: EVIVersionFixed,
    }

    fn deserialize_evi_version<'de, D>(deserializer: D) -> Result<EVIVersionFixed, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = serde::Deserialize::deserialize(deserializer)?;
        EVIVersionFixed::from_legacy(&s).map_err(serde::de::Error::custom)
    }

    let json = r#"{"version": "3"}"#;
    let result: Result<TestConfig, _> = serde_json::from_str(json);

    assert!(result.is_ok(), "JSON with V3 should parse successfully");
    assert_eq!(result.unwrap().version, EVIVersionFixed::V3);

    let json = r#"{"version": "4-mini"}"#;
    let result: Result<TestConfig, _> = serde_json::from_str(json);

    assert!(
        result.is_ok(),
        "JSON with V4-mini should parse successfully"
    );
    assert_eq!(result.unwrap().version, EVIVersionFixed::V4Mini);
}

// =============================================================================
// Test 6.9: Migration Path Tests
// =============================================================================

/// Test that migration from V1 suggests V3
#[test]
fn test_v1_migration_suggests_v3() {
    let result = EVIVersionFixed::from_legacy("1");
    let err = result.unwrap_err();

    let err_string = format!("{:?}", err);
    // Error should guide users to v3 or v4-mini
    assert!(
        err_string.contains("v3") || err_string.contains("V3") || err_string.contains("4-mini"),
        "Error should suggest migration target: {}",
        err_string
    );
}

/// Test that migration from V2 suggests V3
#[test]
fn test_v2_migration_suggests_v3() {
    let result = EVIVersionFixed::from_legacy("2");
    let err = result.unwrap_err();

    let err_string = format!("{:?}", err);
    // Error should guide users to v3 or v4-mini
    assert!(
        err_string.contains("v3") || err_string.contains("V3") || err_string.contains("4-mini"),
        "Error should suggest migration target: {}",
        err_string
    );
}

// =============================================================================
// Test 6.10: Edge Cases
// =============================================================================

/// Test case sensitivity (versions should be exact match)
#[test]
fn test_version_case_sensitivity() {
    // These should all fail - versions must be exact
    assert!(EVIVersionFixed::from_legacy("V3").is_err());
    assert!(EVIVersionFixed::from_legacy("V4-MINI").is_err());
    assert!(EVIVersionFixed::from_legacy("4-MINI").is_err());

    // These should succeed
    assert!(EVIVersionFixed::from_legacy("3").is_ok());
    assert!(EVIVersionFixed::from_legacy("4-mini").is_ok());
}

/// Test whitespace handling
#[test]
fn test_whitespace_handling() {
    // Versions with whitespace should fail
    assert!(EVIVersionFixed::from_legacy(" 3").is_err());
    assert!(EVIVersionFixed::from_legacy("3 ").is_err());
    assert!(EVIVersionFixed::from_legacy(" 3 ").is_err());
}

/// Test numeric-only rejection for V4
#[test]
fn test_v4_numeric_only_rejected() {
    // "4" alone should not work, must be "4-mini"
    let result = EVIVersionFixed::from_legacy("4");
    assert!(
        result.is_err(),
        "Version '4' alone should be rejected, must use '4-mini'"
    );
}

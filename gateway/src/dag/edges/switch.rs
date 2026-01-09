//! Switch pattern matching for edges
//!
//! Simple field-based routing without full expression evaluation.

use std::collections::HashMap;

use crate::dag::error::{DAGError, DAGResult};

/// Switch pattern matcher
///
/// Matches a field value against a set of cases and returns the target.
#[derive(Debug, Clone)]
pub struct SwitchMatcher {
    /// Field path to extract (dot-separated)
    field_segments: Vec<String>,
    /// Value -> target mapping
    cases: HashMap<String, String>,
    /// Default target if no case matches
    default: Option<String>,
}

impl SwitchMatcher {
    /// Create a new switch matcher
    pub fn new(field: impl Into<String>) -> Self {
        let field_str: String = field.into();
        Self {
            field_segments: field_str.split('.').map(String::from).collect(),
            cases: HashMap::new(),
            default: None,
        }
    }

    /// Add a case
    pub fn add_case(mut self, value: impl Into<String>, target: impl Into<String>) -> Self {
        self.cases.insert(value.into(), target.into());
        self
    }

    /// Set default target
    pub fn with_default(mut self, target: impl Into<String>) -> Self {
        self.default = Some(target.into());
        self
    }

    /// Match against JSON data
    pub fn match_value(&self, data: &serde_json::Value) -> DAGResult<Option<&str>> {
        // Extract field value
        let value = self.extract_field(data)?;
        let value_str = self.value_to_string(&value);

        // Match against cases
        if let Some(target) = self.cases.get(&value_str) {
            return Ok(Some(target.as_str()));
        }

        // Return default if available
        Ok(self.default.as_deref())
    }

    /// Check if a value matches any case
    pub fn has_match(&self, data: &serde_json::Value) -> bool {
        match self.match_value(data) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => false,
        }
    }

    /// Extract field from JSON data
    fn extract_field(&self, data: &serde_json::Value) -> DAGResult<serde_json::Value> {
        let mut current = data;

        for segment in &self.field_segments {
            current = current.get(segment).ok_or_else(|| DAGError::FieldExtractionError {
                field: self.field_segments.join("."),
                error: format!("Field '{}' not found", segment),
            })?;
        }

        Ok(current.clone())
    }

    /// Convert JSON value to string for matching
    fn value_to_string(&self, value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Null => "null".to_string(),
            _ => value.to_string(),
        }
    }

    /// Get the field path
    pub fn field(&self) -> String {
        self.field_segments.join(".")
    }

    /// Get all cases
    pub fn cases(&self) -> &HashMap<String, String> {
        &self.cases
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_switch_simple_match() {
        let matcher = SwitchMatcher::new("language")
            .add_case("en-US", "english_handler")
            .add_case("es-ES", "spanish_handler");

        let data = json!({ "language": "en-US" });
        let result = matcher.match_value(&data).unwrap();
        assert_eq!(result, Some("english_handler"));
    }

    #[test]
    fn test_switch_nested_field() {
        let matcher = SwitchMatcher::new("stt_result.language")
            .add_case("en-US", "english_handler");

        let data = json!({
            "stt_result": {
                "language": "en-US"
            }
        });

        let result = matcher.match_value(&data).unwrap();
        assert_eq!(result, Some("english_handler"));
    }

    #[test]
    fn test_switch_default() {
        let matcher = SwitchMatcher::new("language")
            .add_case("en-US", "english_handler")
            .with_default("default_handler");

        let data = json!({ "language": "fr-FR" });
        let result = matcher.match_value(&data).unwrap();
        assert_eq!(result, Some("default_handler"));
    }

    #[test]
    fn test_switch_no_match() {
        let matcher = SwitchMatcher::new("language")
            .add_case("en-US", "english_handler");

        let data = json!({ "language": "fr-FR" });
        let result = matcher.match_value(&data).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_switch_bool_value() {
        let matcher = SwitchMatcher::new("is_final")
            .add_case("true", "final_handler")
            .add_case("false", "interim_handler");

        let data = json!({ "is_final": true });
        let result = matcher.match_value(&data).unwrap();
        assert_eq!(result, Some("final_handler"));
    }

    #[test]
    fn test_switch_missing_field() {
        let matcher = SwitchMatcher::new("missing_field")
            .add_case("value", "handler");

        let data = json!({ "other_field": "value" });
        let result = matcher.match_value(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_match() {
        let matcher = SwitchMatcher::new("language")
            .add_case("en-US", "handler");

        let data1 = json!({ "language": "en-US" });
        assert!(matcher.has_match(&data1));

        let data2 = json!({ "language": "fr-FR" });
        assert!(!matcher.has_match(&data2));
    }
}

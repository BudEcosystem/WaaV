//! DAG Templates Registry
//!
//! This module provides a registry for storing and retrieving pre-defined DAG
//! templates. Templates can be loaded from:
//! - Inline configuration in YAML/JSON
//! - External files in a templates directory
//! - Programmatic registration
//!
//! # Usage
//!
//! ```ignore
//! use waav_gateway::dag::templates::{global_templates, load_templates_from_config};
//!
//! // Load templates from config
//! load_templates_from_config(&config)?;
//!
//! // Get a template by name
//! let dag = global_templates().get("voice-bot")?;
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use dashmap::DashMap;
use tracing::{debug, info, warn};

use super::definition::DAGDefinition;

/// Global templates registry singleton
static GLOBAL_TEMPLATES: OnceLock<DAGTemplateRegistry> = OnceLock::new();

/// Get the global DAG templates registry
///
/// The registry is lazily initialized on first access.
pub fn global_templates() -> &'static DAGTemplateRegistry {
    GLOBAL_TEMPLATES.get_or_init(DAGTemplateRegistry::new)
}

/// Initialize the global templates registry with templates from configuration
///
/// This should be called during server startup to load templates from:
/// - Inline templates in the configuration
/// - Template files from the templates directory
pub fn initialize_templates(config: &TemplatesConfig) -> Result<usize, TemplateError> {
    let registry = global_templates();
    let mut count = 0;

    // Load inline templates
    for (name, template) in &config.templates {
        registry.register(name.clone(), template.clone());
        info!(name = %name, "Registered inline DAG template");
        count += 1;
    }

    // Load templates from directory
    if let Some(dir) = &config.templates_dir {
        match registry.load_from_directory(dir) {
            Ok(loaded) => {
                info!(directory = %dir.display(), count = %loaded, "Loaded DAG templates from directory");
                count += loaded;
            }
            Err(e) => {
                if config.templates_dir_required {
                    return Err(e);
                }
                warn!(directory = %dir.display(), error = %e, "Failed to load templates from directory");
            }
        }
    }

    info!(total = %count, "DAG template registry initialized");
    Ok(count)
}

/// Configuration for DAG templates
#[derive(Debug, Clone, Default)]
pub struct TemplatesConfig {
    /// Inline template definitions (name -> definition)
    pub templates: HashMap<String, DAGDefinition>,
    /// Directory to load template files from
    pub templates_dir: Option<PathBuf>,
    /// Whether the templates directory is required (error if missing/unreadable)
    pub templates_dir_required: bool,
}

/// Error type for template operations
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("Template not found: {0}")]
    NotFound(String),

    #[error("Failed to read template file '{path}': {error}")]
    ReadError { path: String, error: String },

    #[error("Failed to parse template '{name}': {error}")]
    ParseError { name: String, error: String },

    #[error("Templates directory not found: {0}")]
    DirectoryNotFound(String),

    #[error("Invalid template: {0}")]
    InvalidTemplate(String),
}

/// Registry for DAG templates
///
/// Provides thread-safe storage and retrieval of DAG templates.
/// Uses DashMap for O(1) concurrent access.
pub struct DAGTemplateRegistry {
    /// Templates indexed by name (lowercase for case-insensitive lookup)
    templates: DashMap<String, DAGDefinition>,
    /// Original names for display purposes
    original_names: DashMap<String, String>,
}

impl DAGTemplateRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            templates: DashMap::new(),
            original_names: DashMap::new(),
        }
    }

    /// Register a template with the given name
    ///
    /// Names are stored case-insensitively for lookup.
    pub fn register(&self, name: impl Into<String>, template: DAGDefinition) {
        let original_name = name.into();
        let key = original_name.to_lowercase();

        debug!(name = %original_name, id = %template.id, "Registering DAG template");

        self.templates.insert(key.clone(), template);
        self.original_names.insert(key, original_name);
    }

    /// Get a template by name
    ///
    /// Lookup is case-insensitive.
    pub fn get(&self, name: &str) -> Option<DAGDefinition> {
        let key = name.to_lowercase();
        self.templates.get(&key).map(|entry| entry.value().clone())
    }

    /// Check if a template exists
    pub fn contains(&self, name: &str) -> bool {
        let key = name.to_lowercase();
        self.templates.contains_key(&key)
    }

    /// Remove a template
    pub fn remove(&self, name: &str) -> Option<DAGDefinition> {
        let key = name.to_lowercase();
        self.original_names.remove(&key);
        self.templates.remove(&key).map(|(_, v)| v)
    }

    /// Get all template names
    pub fn names(&self) -> Vec<String> {
        self.original_names
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get the number of registered templates
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Clear all templates
    pub fn clear(&self) {
        self.templates.clear();
        self.original_names.clear();
    }

    /// Load templates from a directory
    ///
    /// Loads all `.yaml`, `.yml`, and `.json` files from the directory.
    /// Template names are derived from filenames (without extension).
    pub fn load_from_directory(&self, dir: &Path) -> Result<usize, TemplateError> {
        if !dir.exists() {
            return Err(TemplateError::DirectoryNotFound(dir.display().to_string()));
        }

        if !dir.is_dir() {
            return Err(TemplateError::DirectoryNotFound(format!(
                "{} is not a directory",
                dir.display()
            )));
        }

        let mut count = 0;
        let entries = std::fs::read_dir(dir).map_err(|e| TemplateError::ReadError {
            path: dir.display().to_string(),
            error: e.to_string(),
        })?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "Failed to read directory entry");
                    continue;
                }
            };

            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // Check file extension
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(extension, "yaml" | "yml" | "json") {
                continue;
            }

            // Get template name from filename
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            match self.load_template_file(&path, &name) {
                Ok(()) => {
                    debug!(name = %name, path = %path.display(), "Loaded DAG template");
                    count += 1;
                }
                Err(e) => {
                    warn!(name = %name, path = %path.display(), error = %e, "Failed to load template");
                }
            }
        }

        Ok(count)
    }

    /// Load a single template file
    fn load_template_file(&self, path: &Path, name: &str) -> Result<(), TemplateError> {
        let content = std::fs::read_to_string(path).map_err(|e| TemplateError::ReadError {
            path: path.display().to_string(),
            error: e.to_string(),
        })?;

        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("yaml");

        let template: DAGDefinition = match extension {
            "json" => serde_json::from_str(&content).map_err(|e| TemplateError::ParseError {
                name: name.to_string(),
                error: e.to_string(),
            })?,
            _ => serde_yaml::from_str(&content).map_err(|e| TemplateError::ParseError {
                name: name.to_string(),
                error: e.to_string(),
            })?,
        };

        self.register(name.to_string(), template);
        Ok(())
    }

    /// Load a template from a YAML string
    pub fn load_from_yaml(&self, name: &str, yaml: &str) -> Result<(), TemplateError> {
        let template: DAGDefinition = serde_yaml::from_str(yaml).map_err(|e| {
            TemplateError::ParseError {
                name: name.to_string(),
                error: e.to_string(),
            }
        })?;
        self.register(name.to_string(), template);
        Ok(())
    }

    /// Load a template from a JSON string
    pub fn load_from_json(&self, name: &str, json: &str) -> Result<(), TemplateError> {
        let template: DAGDefinition = serde_json::from_str(json).map_err(|e| {
            TemplateError::ParseError {
                name: name.to_string(),
                error: e.to_string(),
            }
        })?;
        self.register(name.to_string(), template);
        Ok(())
    }
}

impl Default for DAGTemplateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_template(id: &str) -> DAGDefinition {
        DAGDefinition {
            id: id.to_string(),
            name: format!("Test Template {}", id),
            version: "1.0.0".to_string(),
            nodes: vec![],
            edges: vec![],
            entry_node: "input".to_string(),
            exit_nodes: vec!["output".to_string()],
            api_key_routes: HashMap::new(),
            config: Default::default(),
        }
    }

    #[test]
    fn test_register_and_get() {
        let registry = DAGTemplateRegistry::new();
        let template = create_test_template("test-1");

        registry.register("Voice-Bot", template.clone());

        // Case-insensitive lookup
        assert!(registry.contains("voice-bot"));
        assert!(registry.contains("Voice-Bot"));
        assert!(registry.contains("VOICE-BOT"));

        let retrieved = registry.get("voice-bot").unwrap();
        assert_eq!(retrieved.id, "test-1");
    }

    #[test]
    fn test_names() {
        let registry = DAGTemplateRegistry::new();
        registry.register("Voice-Bot", create_test_template("1"));
        registry.register("STT-Pipeline", create_test_template("2"));

        let names = registry.names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"Voice-Bot".to_string()));
        assert!(names.contains(&"STT-Pipeline".to_string()));
    }

    #[test]
    fn test_remove() {
        let registry = DAGTemplateRegistry::new();
        registry.register("test", create_test_template("1"));

        assert!(registry.contains("test"));
        let removed = registry.remove("TEST").unwrap();
        assert_eq!(removed.id, "1");
        assert!(!registry.contains("test"));
    }

    #[test]
    fn test_load_from_yaml() {
        let registry = DAGTemplateRegistry::new();
        let yaml = r#"
            id: yaml-test
            name: YAML Test Template
            version: "1.0.0"
            nodes: []
            edges: []
            entry_node: input
            exit_nodes:
              - output
        "#;

        registry.load_from_yaml("yaml-test", yaml).unwrap();
        let template = registry.get("yaml-test").unwrap();
        assert_eq!(template.id, "yaml-test");
        assert_eq!(template.name, "YAML Test Template");
    }

    #[test]
    fn test_load_from_json() {
        let registry = DAGTemplateRegistry::new();
        let json = r#"{
            "id": "json-test",
            "name": "JSON Test Template",
            "version": "1.0.0",
            "nodes": [],
            "edges": [],
            "entry_node": "input",
            "exit_nodes": ["output"]
        }"#;

        registry.load_from_json("json-test", json).unwrap();
        let template = registry.get("json-test").unwrap();
        assert_eq!(template.id, "json-test");
        assert_eq!(template.name, "JSON Test Template");
    }

    #[test]
    fn test_global_templates() {
        // Ensure global_templates() returns consistent reference
        let t1 = global_templates();
        let t2 = global_templates();
        assert!(std::ptr::eq(t1, t2));
    }
}

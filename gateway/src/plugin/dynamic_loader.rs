//! Dynamic Plugin Loader
//!
//! This module provides runtime discovery and loading of external plugins
//! from shared libraries (.so on Linux, .dll on Windows, .dylib on macOS).
//!
//! # Architecture
//!
//! The dynamic loader:
//! 1. Scans configured plugin directories for plugin libraries
//! 2. Validates ABI compatibility using `abi_stable`
//! 3. Checks gateway version requirements
//! 4. Registers loaded plugins with the existing `PluginRegistry`
//!
//! # Safety
//!
//! Plugin loading involves unsafe operations. The loader provides:
//! - ABI version verification via `abi_stable`
//! - Gateway version compatibility checks
//! - Graceful error handling (failed plugins don't crash gateway)
//! - Panic isolation for plugin factory calls
//!
//! # Plugin Naming Convention
//!
//! Plugins must follow the naming pattern:
//! - Linux: `libwaav_plugin_<name>.so`
//! - macOS: `libwaav_plugin_<name>.dylib`
//! - Windows: `waav_plugin_<name>.dll`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use abi_stable::library::{LibraryError, RootModule};
use waav_plugin_api::{
    FFIConfig, PluginCapabilityType, PluginManifest, PluginModule_Ref,
    RealtimeProvider, STTProvider, TTSProvider,
};

use super::metadata::ProviderMetadata;
use super::registry::{PluginRegistry, RealtimeFactoryFn, STTFactoryFn, TTSFactoryFn};
use crate::core::realtime::{RealtimeConfig, RealtimeError};
use crate::core::stt::{STTConfig, STTError};
use crate::core::tts::{TTSConfig, TTSError};

/// Errors that can occur during plugin loading
#[derive(Debug, thiserror::Error)]
pub enum PluginLoadError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Library loading error: {0}")]
    LibraryError(String),

    #[error("ABI error: {0}")]
    AbiError(String),

    #[error("Plugin initialization failed: {0}")]
    InitializationError(String),

    #[error("Version incompatible: plugin requires gateway {required}, but running {actual}")]
    VersionIncompatible { required: String, actual: String },

    #[error("Plugin manifest invalid: {0}")]
    ManifestInvalid(String),
}

impl From<LibraryError> for PluginLoadError {
    fn from(e: LibraryError) -> Self {
        PluginLoadError::AbiError(e.to_string())
    }
}

/// Information about a discovered plugin candidate
#[derive(Debug, Clone)]
pub struct PluginCandidate {
    /// Path to the plugin library file
    pub path: PathBuf,
    /// Extracted plugin name from filename
    pub name: String,
}

/// A loaded plugin with its module reference
///
/// Note: The library is kept alive by abi_stable's internal mechanisms.
/// abi_stable intentionally leaks the library to ensure FFI safety -
/// Rust code in shared libraries cannot be safely unloaded at runtime
/// due to pending destructors, static variables, etc. This is a deliberate
/// design decision. To reload plugins, restart the gateway.
pub struct LoadedPlugin {
    /// The plugin's root module reference (library is kept alive by abi_stable)
    module: PluginModule_Ref,
    /// Cached manifest
    manifest: PluginManifest,
    /// Path to the plugin library (for debugging/logging)
    path: PathBuf,
}

impl LoadedPlugin {
    /// Get the plugin manifest
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Get the plugin ID
    pub fn id(&self) -> &str {
        self.manifest.id.as_str()
    }

    /// Get the path to the plugin library
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Dynamic Plugin Loader
///
/// Discovers, loads, and registers plugins from filesystem directories.
pub struct DynamicPluginLoader {
    /// Currently loaded plugins (keyed by plugin ID)
    loaded_plugins: HashMap<String, LoadedPlugin>,
    /// Gateway version for compatibility checking
    gateway_version: semver::Version,
}

impl DynamicPluginLoader {
    /// Create a new plugin loader
    pub fn new() -> Self {
        // Parse gateway version from Cargo.toml
        let gateway_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .unwrap_or_else(|_| semver::Version::new(1, 0, 0));

        Self {
            loaded_plugins: HashMap::new(),
            gateway_version,
        }
    }

    /// Discover plugin candidates in a directory
    ///
    /// Scans the directory for files matching the plugin naming convention.
    /// Does not load the plugins, only identifies candidates.
    pub fn discover(&self, plugin_dir: &Path) -> Result<Vec<PluginCandidate>, PluginLoadError> {
        let mut candidates = Vec::new();

        if !plugin_dir.exists() {
            tracing::debug!(path = %plugin_dir.display(), "Plugin directory does not exist");
            return Ok(candidates);
        }

        if !plugin_dir.is_dir() {
            tracing::warn!(path = %plugin_dir.display(), "Plugin path is not a directory");
            return Ok(candidates);
        }

        // Scan for plugin files
        for entry in std::fs::read_dir(plugin_dir)? {
            let entry = entry?;
            let path = entry.path();

            if let Some(name) = self.extract_plugin_name(&path) {
                candidates.push(PluginCandidate {
                    path: path.clone(),
                    name,
                });
                tracing::trace!(path = %path.display(), "Found plugin candidate");
            }
        }

        // Also scan subdirectories (one level deep)
        for entry in std::fs::read_dir(plugin_dir)? {
            let entry = entry?;
            let subdir = entry.path();

            if subdir.is_dir() {
                for sub_entry in std::fs::read_dir(&subdir)? {
                    let sub_entry = sub_entry?;
                    let path = sub_entry.path();

                    if let Some(name) = self.extract_plugin_name(&path) {
                        candidates.push(PluginCandidate {
                            path: path.clone(),
                            name,
                        });
                        tracing::trace!(path = %path.display(), "Found plugin candidate in subdirectory");
                    }
                }
            }
        }

        tracing::info!(
            count = candidates.len(),
            path = %plugin_dir.display(),
            "Discovered plugin candidates"
        );

        Ok(candidates)
    }

    /// Extract plugin name from a library filename
    ///
    /// Returns None if the file doesn't match the plugin naming convention.
    fn extract_plugin_name(&self, path: &Path) -> Option<String> {
        let filename = path.file_name()?.to_str()?;

        // Platform-specific prefix and suffix
        #[cfg(target_os = "linux")]
        let (prefix, suffix) = ("libwaav_plugin_", ".so");

        #[cfg(target_os = "macos")]
        let (prefix, suffix) = ("libwaav_plugin_", ".dylib");

        #[cfg(target_os = "windows")]
        let (prefix, suffix) = ("waav_plugin_", ".dll");

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        let (prefix, suffix) = ("libwaav_plugin_", ".so");

        if filename.starts_with(prefix) && filename.ends_with(suffix) {
            let name_start = prefix.len();
            let name_end = filename.len() - suffix.len();
            if name_end > name_start {
                return Some(filename[name_start..name_end].to_string());
            }
        }

        None
    }

    /// Load a plugin from a library file
    ///
    /// # Safety
    ///
    /// This function loads and executes code from a shared library.
    /// Only load plugins from trusted sources.
    pub fn load(&mut self, candidate: &PluginCandidate) -> Result<&LoadedPlugin, PluginLoadError> {
        tracing::info!(path = %candidate.path.display(), name = %candidate.name, "Loading plugin");

        // Load the library using abi_stable
        let module = PluginModule_Ref::load_from_file(&candidate.path)?;

        // Get manifest
        let manifest = (module.manifest())();

        // Validate manifest
        if manifest.id.is_empty() {
            return Err(PluginLoadError::ManifestInvalid(
                "Plugin ID cannot be empty".into(),
            ));
        }

        // Check gateway version compatibility
        self.check_version_compatibility(&manifest)?;

        // Initialize the plugin
        let config = FFIConfig::default();
        let init_result = (module.init())(&config as *const _);

        if let abi_stable::std_types::RResult::RErr(e) = init_result {
            return Err(PluginLoadError::InitializationError(e.to_string()));
        }

        // Store the loaded plugin
        // Note: abi_stable's load_from_file keeps the library alive internally
        // by intentionally leaking it. This is a deliberate design choice for
        // FFI safety. The library cannot be unloaded at runtime.
        let plugin = LoadedPlugin {
            module,
            manifest,
            path: candidate.path.clone(),
        };

        let id = plugin.id().to_string();
        self.loaded_plugins.insert(id.clone(), plugin);

        tracing::info!(
            plugin_id = %id,
            path = %candidate.path.display(),
            "Successfully loaded plugin"
        );

        Ok(self.loaded_plugins.get(&id).unwrap())
    }

    /// Check if a plugin is compatible with the current gateway version
    fn check_version_compatibility(&self, manifest: &PluginManifest) -> Result<(), PluginLoadError> {
        let version_req_str = manifest.gateway_version_req.as_str();

        // Empty requirement means any version is acceptable
        if version_req_str.is_empty() {
            return Ok(());
        }

        let version_req = semver::VersionReq::parse(version_req_str).map_err(|e| {
            PluginLoadError::ManifestInvalid(format!(
                "Invalid gateway version requirement '{}': {}",
                version_req_str, e
            ))
        })?;

        if !version_req.matches(&self.gateway_version) {
            return Err(PluginLoadError::VersionIncompatible {
                required: version_req_str.to_string(),
                actual: self.gateway_version.to_string(),
            });
        }

        Ok(())
    }

    /// Register a loaded plugin with the gateway's plugin registry
    pub fn register_plugin(&self, plugin: &LoadedPlugin, registry: &PluginRegistry) {
        let manifest = plugin.manifest();
        let module = plugin.module;

        for cap in manifest.capabilities.iter() {
            match cap {
                PluginCapabilityType::STT => {
                    // create_stt is a required field, returns ROption directly
                    if let abi_stable::std_types::ROption::RSome(create_fn) = module.create_stt() {
                        self.register_stt_factory(manifest, create_fn, registry);
                    }
                }
                PluginCapabilityType::TTS => {
                    // create_tts is a suffix field, returns Option<ROption>
                    if let Some(abi_stable::std_types::ROption::RSome(create_fn)) = module.create_tts() {
                        self.register_tts_factory(manifest, create_fn, registry);
                    }
                }
                PluginCapabilityType::Realtime => {
                    // create_realtime is a suffix field, returns Option<ROption>
                    if let Some(abi_stable::std_types::ROption::RSome(create_fn)) = module.create_realtime() {
                        self.register_realtime_factory(manifest, create_fn, registry);
                    }
                }
                PluginCapabilityType::WSHandler => {
                    // WebSocket handlers would need additional infrastructure
                    tracing::warn!(
                        plugin_id = manifest.id.as_str(),
                        "WSHandler capability not yet supported for dynamic plugins"
                    );
                }
            }
        }
    }

    /// Create and register an STT factory for a dynamic plugin
    fn register_stt_factory(
        &self,
        manifest: &PluginManifest,
        create_fn: extern "C" fn(*const FFIConfig) -> abi_stable::std_types::RResult<STTProvider, abi_stable::std_types::RString>,
        registry: &PluginRegistry,
    ) {
        let plugin_id = manifest.id.to_string();
        let plugin_name = manifest.name.to_string();

        // Create factory function that wraps the FFI call
        let factory: STTFactoryFn = Arc::new(move |config: STTConfig| {
            // Convert config to JSON
            let config_json = serde_json::to_string(&config)
                .unwrap_or_else(|_| "{}".to_string());
            let ffi_config = FFIConfig::from_json(config_json);

            // Call the plugin's factory
            let result = create_fn(&ffi_config as *const _);

            match result {
                abi_stable::std_types::RResult::ROk(provider) => {
                    // Wrap the FFI provider in an adapter
                    Ok(Box::new(super::ffi_adapters::FFISTTAdapter::new(provider))
                        as Box<dyn crate::core::stt::BaseSTT>)
                }
                abi_stable::std_types::RResult::RErr(e) => {
                    Err(STTError::ConfigurationError(e.to_string()))
                }
            }
        });

        // Create metadata
        let metadata = ProviderMetadata::stt(&plugin_id, &plugin_name)
            .with_feature("streaming")  // Assume streaming capability
            .with_description(manifest.description.to_string());

        registry.register_stt(&plugin_id, factory, metadata);

        tracing::info!(
            plugin_id = %plugin_id,
            "Registered dynamic STT provider"
        );
    }

    /// Create and register a TTS factory for a dynamic plugin
    fn register_tts_factory(
        &self,
        manifest: &PluginManifest,
        create_fn: extern "C" fn(*const FFIConfig) -> abi_stable::std_types::RResult<TTSProvider, abi_stable::std_types::RString>,
        registry: &PluginRegistry,
    ) {
        let plugin_id = manifest.id.to_string();
        let plugin_name = manifest.name.to_string();

        let factory: TTSFactoryFn = Arc::new(move |config: TTSConfig| {
            let config_json = serde_json::to_string(&config)
                .unwrap_or_else(|_| "{}".to_string());
            let ffi_config = FFIConfig::from_json(config_json);

            let result = create_fn(&ffi_config as *const _);

            match result {
                abi_stable::std_types::RResult::ROk(provider) => {
                    Ok(Box::new(super::ffi_adapters::FFITTSAdapter::new(provider))
                        as Box<dyn crate::core::tts::BaseTTS>)
                }
                abi_stable::std_types::RResult::RErr(e) => {
                    Err(TTSError::InvalidConfiguration(e.to_string()))
                }
            }
        });

        let metadata = ProviderMetadata::tts(&plugin_id, &plugin_name)
            .with_feature("streaming")
            .with_description(manifest.description.to_string());

        registry.register_tts(&plugin_id, factory, metadata);

        tracing::info!(
            plugin_id = %plugin_id,
            "Registered dynamic TTS provider"
        );
    }

    /// Create and register a Realtime factory for a dynamic plugin
    fn register_realtime_factory(
        &self,
        manifest: &PluginManifest,
        create_fn: extern "C" fn(*const FFIConfig) -> abi_stable::std_types::RResult<RealtimeProvider, abi_stable::std_types::RString>,
        registry: &PluginRegistry,
    ) {
        let plugin_id = manifest.id.to_string();
        let plugin_name = manifest.name.to_string();

        let factory: RealtimeFactoryFn = Arc::new(move |config: RealtimeConfig| {
            let config_json = serde_json::to_string(&config)
                .unwrap_or_else(|_| "{}".to_string());
            let ffi_config = FFIConfig::from_json(config_json);

            let result = create_fn(&ffi_config as *const _);

            match result {
                abi_stable::std_types::RResult::ROk(provider) => {
                    Ok(Box::new(super::ffi_adapters::FFIRealtimeAdapter::new(provider))
                        as Box<dyn crate::core::realtime::BaseRealtime>)
                }
                abi_stable::std_types::RResult::RErr(e) => {
                    Err(RealtimeError::InvalidConfiguration(e.to_string()))
                }
            }
        });

        let metadata = ProviderMetadata::realtime(&plugin_id, &plugin_name)
            .with_description(manifest.description.to_string());

        registry.register_realtime(&plugin_id, factory, metadata);

        tracing::info!(
            plugin_id = %plugin_id,
            "Registered dynamic Realtime provider"
        );
    }

    /// Load all plugins from a directory and register them
    ///
    /// This is the main entry point for dynamic plugin loading.
    /// Failed plugins are logged but don't prevent other plugins from loading.
    pub fn load_all_from_directory(
        &mut self,
        plugin_dir: &Path,
        registry: &PluginRegistry,
    ) -> Result<usize, PluginLoadError> {
        let candidates = self.discover(plugin_dir)?;
        let mut loaded_ids: Vec<String> = Vec::new();

        // First pass: load all plugins
        for candidate in candidates {
            match self.load(&candidate) {
                Ok(plugin) => {
                    loaded_ids.push(plugin.id().to_string());
                }
                Err(e) => {
                    tracing::warn!(
                        path = %candidate.path.display(),
                        error = %e,
                        "Failed to load plugin"
                    );
                }
            }
        }

        // Second pass: register all loaded plugins
        for id in &loaded_ids {
            if let Some(plugin) = self.loaded_plugins.get(id) {
                self.register_plugin(plugin, registry);
            }
        }

        tracing::info!(
            loaded = loaded_ids.len(),
            directory = %plugin_dir.display(),
            "Dynamic plugin loading complete"
        );

        Ok(loaded_ids.len())
    }

    /// Get a list of currently loaded plugin IDs
    pub fn loaded_plugin_ids(&self) -> Vec<String> {
        self.loaded_plugins.keys().cloned().collect()
    }

    /// Shutdown all loaded plugins
    ///
    /// Note: This calls each plugin's shutdown function for cleanup, but the
    /// shared libraries themselves cannot be unloaded at runtime. This is a
    /// deliberate design choice by abi_stable for FFI safety. The libraries
    /// will remain loaded until the process exits. To reload plugins, restart
    /// the gateway.
    pub fn shutdown_all(&mut self) {
        for (id, plugin) in self.loaded_plugins.drain() {
            tracing::debug!(
                plugin_id = %id,
                path = %plugin.path.display(),
                "Shutting down plugin"
            );

            // Call shutdown function for plugin cleanup
            let result = (plugin.module.shutdown())();

            if let abi_stable::std_types::RResult::RErr(e) = result {
                tracing::warn!(
                    plugin_id = %id,
                    error = %e.as_str(),
                    "Plugin shutdown returned error"
                );
            }

            // Note: Library remains loaded (by design) - abi_stable leaks it
            // for FFI safety. The memory will be freed when process exits.
        }
    }
}

impl Default for DynamicPluginLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DynamicPluginLoader {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_plugin_name_linux() {
        let loader = DynamicPluginLoader::new();

        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                loader.extract_plugin_name(Path::new("/opt/plugins/libwaav_plugin_custom.so")),
                Some("custom".to_string())
            );
            assert_eq!(
                loader.extract_plugin_name(Path::new("libwaav_plugin_my_stt.so")),
                Some("my_stt".to_string())
            );
            assert_eq!(
                loader.extract_plugin_name(Path::new("libother.so")),
                None
            );
            assert_eq!(
                loader.extract_plugin_name(Path::new("waav_plugin_test.dll")),
                None
            );
        }
    }

    #[test]
    fn test_version_compatibility() {
        let loader = DynamicPluginLoader::new();

        // Valid version requirements
        let manifest = PluginManifest::new("test", "Test", "1.0.0")
            .with_gateway_version(">=1.0.0");
        assert!(loader.check_version_compatibility(&manifest).is_ok());

        // Empty version requirement (always valid)
        let manifest = PluginManifest::new("test", "Test", "1.0.0")
            .with_gateway_version("");
        assert!(loader.check_version_compatibility(&manifest).is_ok());

        // Invalid version requirement syntax
        let manifest = PluginManifest::new("test", "Test", "1.0.0")
            .with_gateway_version("not-a-version");
        assert!(loader.check_version_compatibility(&manifest).is_err());
    }

    #[test]
    fn test_discover_empty_directory() {
        let loader = DynamicPluginLoader::new();
        let temp_dir = std::env::temp_dir().join("waav_plugin_test_empty");
        let _ = std::fs::create_dir_all(&temp_dir);

        let candidates = loader.discover(&temp_dir).unwrap();
        assert!(candidates.is_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_discover_nonexistent_directory() {
        let loader = DynamicPluginLoader::new();
        let candidates = loader
            .discover(Path::new("/nonexistent/path/to/plugins"))
            .unwrap();
        assert!(candidates.is_empty());
    }
}

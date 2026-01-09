//! Plugin Registry
//!
//! This module provides the central registry for all plugins. Plugins are
//! registered at compile-time using the `inventory` crate and indexed by
//! their capabilities.
//!
//! # Architecture
//!
//! The registry uses DashMap for concurrent O(1) amortized provider lookup.
//! All providers (built-in and runtime-registered) are stored in the same maps
//! for simplicity and consistent performance.
//!
//! # Usage
//!
//! ```ignore
//! use waav_gateway::plugin::global_registry;
//!
//! // Create a provider using the registry
//! let stt = global_registry().create_stt("deepgram", config)?;
//! ```

use dashmap::DashMap;
use serde_json::Value;
use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use super::capabilities::{
    AudioProcessor, AudioProcessorCapability, AudioProcessorError, ProcessorMetadata,
    RealtimeCapability, STTCapability, TTSCapability, WSContext, WSError, WSResponse,
};
use super::dispatch::{resolve_realtime_provider, resolve_stt_provider, resolve_tts_provider};
use super::isolation::call_plugin_preserving_error;
use super::lifecycle::PluginEntry;
use super::metadata::ProviderMetadata;
use crate::core::realtime::{BaseRealtime, RealtimeConfig, RealtimeError, RealtimeResult};
use crate::core::stt::{BaseSTT, STTConfig, STTError};
use crate::core::tts::{BaseTTS, TTSConfig, TTSResult};

/// Factory function type for STT providers
pub type STTFactoryFn = Arc<dyn Fn(STTConfig) -> Result<Box<dyn BaseSTT>, STTError> + Send + Sync>;

/// Factory function type for TTS providers
pub type TTSFactoryFn = Arc<dyn Fn(TTSConfig) -> TTSResult<Box<dyn BaseTTS>> + Send + Sync>;

/// Factory function type for Realtime providers
pub type RealtimeFactoryFn =
    Arc<dyn Fn(RealtimeConfig) -> RealtimeResult<Box<dyn BaseRealtime>> + Send + Sync>;

/// Factory function type for Audio processors
pub type AudioProcessorFactoryFn =
    Arc<dyn Fn(serde_json::Value) -> Result<Box<dyn AudioProcessor>, AudioProcessorError> + Send + Sync>;

/// Handler function type for WebSocket messages
///
/// Takes a message payload and context, returns a future that resolves to
/// an optional response. Handlers can return:
/// - `Ok(Some(response))` to send a response back
/// - `Ok(None)` if no response is needed
/// - `Err(e)` if handling failed
pub type WSHandlerFn = Arc<
    dyn Fn(
            Value,
            WSContext,
        ) -> Pin<Box<dyn Future<Output = Result<Option<WSResponse>, WSError>> + Send>>
        + Send
        + Sync,
>;

/// Metadata function type for deferred metadata creation
pub type MetadataFn = fn() -> ProviderMetadata;

/// Factory function pointer type for STT providers (non-Arc version for PluginConstructor)
pub type STTFactoryPtr = fn(STTConfig) -> Result<Box<dyn BaseSTT>, STTError>;

/// Factory function pointer type for TTS providers (non-Arc version for PluginConstructor)
pub type TTSFactoryPtr = fn(TTSConfig) -> TTSResult<Box<dyn BaseTTS>>;

/// Factory function pointer type for Realtime providers (non-Arc version for PluginConstructor)
pub type RealtimeFactoryPtr = fn(RealtimeConfig) -> RealtimeResult<Box<dyn BaseRealtime>>;

/// Factory function pointer type for Audio processors (non-Arc version for PluginConstructor)
pub type AudioProcessorFactoryPtr = fn(serde_json::Value) -> Result<Box<dyn AudioProcessor>, AudioProcessorError>;

/// Plugin constructor for inventory-based registration
///
/// Uses function pointers to defer non-const operations (metadata creation)
/// until runtime, making it compatible with `inventory::submit!`.
pub struct PluginConstructor {
    /// Factory function to create the plugin capability
    pub create_stt: Option<STTFactoryPtr>,
    pub create_tts: Option<TTSFactoryPtr>,
    pub create_realtime: Option<RealtimeFactoryPtr>,
    pub create_audio_processor: Option<AudioProcessorFactoryPtr>,

    /// Provider metadata function (deferred creation for const compatibility)
    pub metadata_fn: MetadataFn,

    /// Processor metadata function (for audio processors)
    pub processor_metadata_fn: Option<fn() -> ProcessorMetadata>,

    /// Provider ID for lookup
    pub provider_id: &'static str,

    /// Aliases for this provider
    pub aliases: &'static [&'static str],
}

impl PluginConstructor {
    /// Create a new STT plugin constructor
    pub const fn stt(
        provider_id: &'static str,
        metadata_fn: MetadataFn,
        factory: fn(STTConfig) -> Result<Box<dyn BaseSTT>, STTError>,
    ) -> Self {
        Self {
            create_stt: Some(factory),
            create_tts: None,
            create_realtime: None,
            create_audio_processor: None,
            metadata_fn,
            processor_metadata_fn: None,
            provider_id,
            aliases: &[],
        }
    }

    /// Create a new TTS plugin constructor
    pub const fn tts(
        provider_id: &'static str,
        metadata_fn: MetadataFn,
        factory: fn(TTSConfig) -> TTSResult<Box<dyn BaseTTS>>,
    ) -> Self {
        Self {
            create_stt: None,
            create_tts: Some(factory),
            create_realtime: None,
            create_audio_processor: None,
            metadata_fn,
            processor_metadata_fn: None,
            provider_id,
            aliases: &[],
        }
    }

    /// Create a new Realtime plugin constructor
    pub const fn realtime(
        provider_id: &'static str,
        metadata_fn: MetadataFn,
        factory: fn(RealtimeConfig) -> RealtimeResult<Box<dyn BaseRealtime>>,
    ) -> Self {
        Self {
            create_stt: None,
            create_tts: None,
            create_realtime: Some(factory),
            create_audio_processor: None,
            metadata_fn,
            processor_metadata_fn: None,
            provider_id,
            aliases: &[],
        }
    }

    /// Create a new Audio Processor plugin constructor
    pub const fn audio_processor(
        processor_id: &'static str,
        metadata_fn: MetadataFn,
        processor_metadata_fn: fn() -> ProcessorMetadata,
        factory: fn(serde_json::Value) -> Result<Box<dyn AudioProcessor>, AudioProcessorError>,
    ) -> Self {
        Self {
            create_stt: None,
            create_tts: None,
            create_realtime: None,
            create_audio_processor: Some(factory),
            metadata_fn,
            processor_metadata_fn: Some(processor_metadata_fn),
            provider_id: processor_id,
            aliases: &[],
        }
    }

    /// Add aliases for this provider
    pub const fn with_aliases(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    /// Get the metadata (calls the deferred function)
    pub fn metadata(&self) -> ProviderMetadata {
        (self.metadata_fn)()
    }

    /// Get the processor metadata (calls the deferred function)
    pub fn processor_metadata(&self) -> Option<ProcessorMetadata> {
        self.processor_metadata_fn.map(|f| f())
    }
}

// Collect all registered plugins at link time
inventory::collect!(PluginConstructor);

/// Central plugin registry
///
/// The registry maintains indexes of all registered plugins and provides
/// methods to create provider instances by name.
pub struct PluginRegistry {
    /// STT provider factories indexed by provider ID
    stt_factories: DashMap<String, (STTFactoryFn, ProviderMetadata)>,

    /// TTS provider factories indexed by provider ID
    tts_factories: DashMap<String, (TTSFactoryFn, ProviderMetadata)>,

    /// Realtime provider factories indexed by provider ID
    realtime_factories: DashMap<String, (RealtimeFactoryFn, ProviderMetadata)>,

    /// Audio processor factories indexed by processor ID
    audio_processor_factories: DashMap<String, (AudioProcessorFactoryFn, ProcessorMetadata)>,

    /// WebSocket message handlers indexed by message type
    ws_handlers: DashMap<String, Vec<WSHandlerFn>>,

    /// Capability index: TypeId -> list of provider IDs
    capability_index: DashMap<TypeId, Vec<String>>,

    /// Plugin entries for lifecycle management
    plugin_entries: DashMap<String, PluginEntry>,
}

impl PluginRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            stt_factories: DashMap::new(),
            tts_factories: DashMap::new(),
            realtime_factories: DashMap::new(),
            audio_processor_factories: DashMap::new(),
            ws_handlers: DashMap::new(),
            capability_index: DashMap::new(),
            plugin_entries: DashMap::new(),
        }
    }

    /// Register an STT provider factory
    pub fn register_stt(
        &self,
        provider_id: &str,
        factory: STTFactoryFn,
        metadata: ProviderMetadata,
    ) {
        let id = provider_id.to_lowercase();

        // Register by primary name
        self.stt_factories
            .insert(id.clone(), (factory.clone(), metadata.clone()));

        // Register by aliases
        for alias in &metadata.aliases {
            self.stt_factories
                .insert(alias.to_lowercase(), (factory.clone(), metadata.clone()));
        }

        // Update capability index
        self.capability_index
            .entry(TypeId::of::<dyn STTCapability>())
            .or_default()
            .push(id.clone());

        // Create plugin entry
        self.plugin_entries.entry(id).or_default();

        tracing::debug!(
            provider_id = %provider_id,
            aliases = ?metadata.aliases,
            "Registered STT provider"
        );
    }

    /// Register a TTS provider factory
    pub fn register_tts(
        &self,
        provider_id: &str,
        factory: TTSFactoryFn,
        metadata: ProviderMetadata,
    ) {
        let id = provider_id.to_lowercase();

        self.tts_factories
            .insert(id.clone(), (factory.clone(), metadata.clone()));

        for alias in &metadata.aliases {
            self.tts_factories
                .insert(alias.to_lowercase(), (factory.clone(), metadata.clone()));
        }

        self.capability_index
            .entry(TypeId::of::<dyn TTSCapability>())
            .or_default()
            .push(id.clone());

        self.plugin_entries.entry(id).or_default();

        tracing::debug!(
            provider_id = %provider_id,
            aliases = ?metadata.aliases,
            "Registered TTS provider"
        );
    }

    /// Register a Realtime provider factory
    pub fn register_realtime(
        &self,
        provider_id: &str,
        factory: RealtimeFactoryFn,
        metadata: ProviderMetadata,
    ) {
        let id = provider_id.to_lowercase();

        self.realtime_factories
            .insert(id.clone(), (factory.clone(), metadata.clone()));

        for alias in &metadata.aliases {
            self.realtime_factories
                .insert(alias.to_lowercase(), (factory.clone(), metadata.clone()));
        }

        self.capability_index
            .entry(TypeId::of::<dyn RealtimeCapability>())
            .or_default()
            .push(id.clone());

        self.plugin_entries.entry(id).or_default();

        tracing::debug!(
            provider_id = %provider_id,
            aliases = ?metadata.aliases,
            "Registered Realtime provider"
        );
    }

    /// Register an Audio Processor factory
    pub fn register_audio_processor(
        &self,
        processor_id: &str,
        factory: AudioProcessorFactoryFn,
        metadata: ProcessorMetadata,
    ) {
        let id = processor_id.to_lowercase();

        self.audio_processor_factories
            .insert(id.clone(), (factory.clone(), metadata.clone()));

        self.capability_index
            .entry(TypeId::of::<dyn AudioProcessorCapability>())
            .or_default()
            .push(id.clone());

        self.plugin_entries.entry(id).or_default();

        tracing::debug!(
            processor_id = %processor_id,
            "Registered Audio Processor"
        );
    }

    /// Create an STT provider by name
    ///
    /// Looks up the provider factory and creates an instance with the given config.
    /// Uses PHF for O(1) guaranteed lookup of built-in providers with automatic
    /// alias resolution. Falls back to DashMap for runtime-registered providers.
    /// The call is wrapped in panic isolation to prevent plugin panics from
    /// crashing the gateway.
    pub fn create_stt(
        &self,
        provider: &str,
        config: STTConfig,
    ) -> Result<Box<dyn BaseSTT>, STTError> {
        // Use PHF for O(1) canonical name resolution (handles aliases + case insensitivity)
        // Falls back to lowercase for runtime-registered providers
        let id = resolve_stt_provider(provider)
            .map(|p| p.canonical_name().to_string())
            .unwrap_or_else(|| provider.to_lowercase());

        let factory_entry = self.stt_factories.get(&id).ok_or_else(|| {
            STTError::ConfigurationError(format!(
                "Unknown STT provider: '{}'. Available providers: {:?}",
                provider,
                self.get_stt_provider_names()
            ))
        })?;

        let factory = factory_entry.0.clone();
        drop(factory_entry); // Release lock before calling factory

        // Call with panic isolation, preserving original error type
        let result = call_plugin_preserving_error(
            std::panic::AssertUnwindSafe(|| factory(config)),
            |panic_msg| STTError::ProviderError(format!("Plugin panicked: {}", panic_msg)),
        );

        // Record success or failure AFTER the factory call
        if let Some(mut entry) = self.plugin_entries.get_mut(&id) {
            match &result {
                Ok(_) => entry.record_success(),
                Err(e) => entry.record_error(e.to_string()),
            }
        }

        result
    }

    /// Create a TTS provider by name
    ///
    /// Uses PHF for O(1) guaranteed lookup of built-in providers with automatic
    /// alias resolution. Falls back to DashMap for runtime-registered providers.
    pub fn create_tts(&self, provider: &str, config: TTSConfig) -> TTSResult<Box<dyn BaseTTS>> {
        // Use PHF for O(1) canonical name resolution (handles aliases + case insensitivity)
        let id = resolve_tts_provider(provider)
            .map(|p| p.canonical_name().to_string())
            .unwrap_or_else(|| provider.to_lowercase());

        let factory_entry = self.tts_factories.get(&id).ok_or_else(|| {
            crate::core::tts::TTSError::InvalidConfiguration(format!(
                "Unknown TTS provider: '{}'. Available providers: {:?}",
                provider,
                self.get_tts_provider_names()
            ))
        })?;

        let factory = factory_entry.0.clone();
        drop(factory_entry);

        let result = call_plugin_preserving_error(
            std::panic::AssertUnwindSafe(|| factory(config)),
            |panic_msg| {
                crate::core::tts::TTSError::ProviderError(format!("Plugin panicked: {}", panic_msg))
            },
        );

        // Record success or failure AFTER the factory call
        if let Some(mut entry) = self.plugin_entries.get_mut(&id) {
            match &result {
                Ok(_) => entry.record_success(),
                Err(e) => entry.record_error(e.to_string()),
            }
        }

        result
    }

    /// Create a Realtime provider by name
    ///
    /// Uses PHF for O(1) guaranteed lookup of built-in providers with automatic
    /// alias resolution. Falls back to DashMap for runtime-registered providers.
    pub fn create_realtime(
        &self,
        provider: &str,
        config: RealtimeConfig,
    ) -> RealtimeResult<Box<dyn BaseRealtime>> {
        // Use PHF for O(1) canonical name resolution (handles aliases + case insensitivity)
        let id = resolve_realtime_provider(provider)
            .map(|p| p.canonical_name().to_string())
            .unwrap_or_else(|| provider.to_lowercase());

        let factory_entry = self.realtime_factories.get(&id).ok_or_else(|| {
            RealtimeError::InvalidConfiguration(format!(
                "Unknown Realtime provider: '{}'. Available providers: {:?}",
                provider,
                self.get_realtime_provider_names()
            ))
        })?;

        let factory = factory_entry.0.clone();
        drop(factory_entry);

        let result = call_plugin_preserving_error(
            std::panic::AssertUnwindSafe(|| factory(config)),
            |panic_msg| RealtimeError::ProviderError(format!("Plugin panicked: {}", panic_msg)),
        );

        // Record success or failure AFTER the factory call
        if let Some(mut entry) = self.plugin_entries.get_mut(&id) {
            match &result {
                Ok(_) => entry.record_success(),
                Err(e) => entry.record_error(e.to_string()),
            }
        }

        result
    }

    /// Create an Audio Processor by ID
    ///
    /// Looks up the processor factory and creates an instance with the given config.
    pub fn create_audio_processor(
        &self,
        processor_id: &str,
        config: serde_json::Value,
    ) -> Result<Box<dyn AudioProcessor>, AudioProcessorError> {
        let id = processor_id.to_lowercase();

        let factory_entry = self.audio_processor_factories.get(&id).ok_or_else(|| {
            AudioProcessorError::ConfigurationError(format!(
                "Unknown Audio Processor: '{}'. Available processors: {:?}",
                processor_id,
                self.get_audio_processor_names()
            ))
        })?;

        let factory = factory_entry.0.clone();
        drop(factory_entry);

        let result = call_plugin_preserving_error(
            std::panic::AssertUnwindSafe(|| factory(config)),
            |panic_msg| {
                AudioProcessorError::InternalError(format!("Plugin panicked: {}", panic_msg))
            },
        );

        // Record success or failure AFTER the factory call
        if let Some(mut entry) = self.plugin_entries.get_mut(&id) {
            match &result {
                Ok(_) => entry.record_success(),
                Err(e) => entry.record_error(e.to_string()),
            }
        }

        result
    }

    /// Get all registered STT provider names (excluding aliases)
    pub fn get_stt_provider_names(&self) -> Vec<String> {
        self.capability_index
            .get(&TypeId::of::<dyn STTCapability>())
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Get all registered TTS provider names (excluding aliases)
    pub fn get_tts_provider_names(&self) -> Vec<String> {
        self.capability_index
            .get(&TypeId::of::<dyn TTSCapability>())
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Get all registered Realtime provider names (excluding aliases)
    pub fn get_realtime_provider_names(&self) -> Vec<String> {
        self.capability_index
            .get(&TypeId::of::<dyn RealtimeCapability>())
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Get all registered Audio Processor names
    pub fn get_audio_processor_names(&self) -> Vec<String> {
        self.capability_index
            .get(&TypeId::of::<dyn AudioProcessorCapability>())
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Get provider metadata by name
    pub fn get_stt_metadata(&self, provider: &str) -> Option<ProviderMetadata> {
        self.stt_factories
            .get(&provider.to_lowercase())
            .map(|entry| entry.1.clone())
    }

    /// Get provider metadata by name
    pub fn get_tts_metadata(&self, provider: &str) -> Option<ProviderMetadata> {
        self.tts_factories
            .get(&provider.to_lowercase())
            .map(|entry| entry.1.clone())
    }

    /// Get provider metadata by name
    pub fn get_realtime_metadata(&self, provider: &str) -> Option<ProviderMetadata> {
        self.realtime_factories
            .get(&provider.to_lowercase())
            .map(|entry| entry.1.clone())
    }

    /// Get audio processor metadata by ID
    pub fn get_audio_processor_metadata(&self, processor_id: &str) -> Option<ProcessorMetadata> {
        self.audio_processor_factories
            .get(&processor_id.to_lowercase())
            .map(|entry| entry.1.clone())
    }

    /// Check if an STT provider is registered
    ///
    /// Uses PHF for O(1) guaranteed check of built-in providers.
    pub fn has_stt_provider(&self, provider: &str) -> bool {
        // Fast path: check PHF for built-in providers
        if resolve_stt_provider(provider).is_some() {
            return true;
        }
        // Slow path: check DashMap for runtime-registered providers
        self.stt_factories.contains_key(&provider.to_lowercase())
    }

    /// Check if a TTS provider is registered
    ///
    /// Uses PHF for O(1) guaranteed check of built-in providers.
    pub fn has_tts_provider(&self, provider: &str) -> bool {
        if resolve_tts_provider(provider).is_some() {
            return true;
        }
        self.tts_factories.contains_key(&provider.to_lowercase())
    }

    /// Check if a Realtime provider is registered
    ///
    /// Uses PHF for O(1) guaranteed check of built-in providers.
    pub fn has_realtime_provider(&self, provider: &str) -> bool {
        if resolve_realtime_provider(provider).is_some() {
            return true;
        }
        self.realtime_factories
            .contains_key(&provider.to_lowercase())
    }

    /// Check if an Audio Processor is registered
    pub fn has_audio_processor(&self, processor_id: &str) -> bool {
        self.audio_processor_factories
            .contains_key(&processor_id.to_lowercase())
    }

    /// Get the number of registered STT providers
    pub fn stt_provider_count(&self) -> usize {
        self.get_stt_provider_names().len()
    }

    /// Get the number of registered TTS providers
    pub fn tts_provider_count(&self) -> usize {
        self.get_tts_provider_names().len()
    }

    /// Get the number of registered Realtime providers
    pub fn realtime_provider_count(&self) -> usize {
        self.get_realtime_provider_names().len()
    }

    /// Get the number of registered Audio Processors
    pub fn audio_processor_count(&self) -> usize {
        self.get_audio_processor_names().len()
    }

    /// Register a WebSocket message handler
    ///
    /// Multiple handlers can be registered for the same message type.
    /// They will all be called in registration order.
    pub fn register_ws_handler(&self, message_type: &str, handler: WSHandlerFn) {
        let msg_type = message_type.to_lowercase();
        self.ws_handlers
            .entry(msg_type.clone())
            .or_default()
            .push(handler);

        tracing::debug!(message_type = %message_type, "Registered WebSocket handler");
    }

    /// Get all handlers for a message type
    ///
    /// Returns an empty vector if no handlers are registered for this type.
    pub fn get_ws_handlers(&self, message_type: &str) -> Vec<WSHandlerFn> {
        self.ws_handlers
            .get(&message_type.to_lowercase())
            .map(|handlers| handlers.clone())
            .unwrap_or_default()
    }

    /// Check if any handlers are registered for a message type
    pub fn has_ws_handler(&self, message_type: &str) -> bool {
        self.ws_handlers
            .get(&message_type.to_lowercase())
            .is_some_and(|h| !h.is_empty())
    }

    /// Get all registered WS message types
    pub fn get_ws_message_types(&self) -> Vec<String> {
        self.ws_handlers
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Get the number of registered WS message types
    pub fn ws_handler_count(&self) -> usize {
        self.ws_handlers.len()
    }

    /// Get plugin health metrics for a specific provider
    ///
    /// Returns call count, error count, uptime, and idle time if the plugin exists.
    pub fn get_plugin_metrics(&self, provider_id: &str) -> Option<PluginMetrics> {
        let id = provider_id.to_lowercase();
        self.plugin_entries.get(&id).map(|entry| PluginMetrics {
            call_count: entry.call_count,
            error_count: entry.error_count,
            error_rate: if entry.call_count > 0 {
                entry.error_count as f64 / entry.call_count as f64
            } else {
                0.0
            },
            last_error: entry.last_error.clone(),
            uptime_seconds: entry.uptime().as_secs(),
            idle_seconds: entry.idle_time().as_secs(),
            state: entry.state.to_string(),
        })
    }

    /// Get all plugin metrics for monitoring
    pub fn get_all_plugin_metrics(&self) -> Vec<(String, PluginMetrics)> {
        self.plugin_entries
            .iter()
            .map(|entry| {
                let id = entry.key().clone();
                let metrics = PluginMetrics {
                    call_count: entry.call_count,
                    error_count: entry.error_count,
                    error_rate: if entry.call_count > 0 {
                        entry.error_count as f64 / entry.call_count as f64
                    } else {
                        0.0
                    },
                    last_error: entry.last_error.clone(),
                    uptime_seconds: entry.uptime().as_secs(),
                    idle_seconds: entry.idle_time().as_secs(),
                    state: entry.state.to_string(),
                };
                (id, metrics)
            })
            .collect()
    }
}

/// Plugin metrics for health reporting
#[derive(Debug, Clone)]
pub struct PluginMetrics {
    /// Number of times the plugin has been called
    pub call_count: u64,
    /// Number of errors encountered
    pub error_count: u64,
    /// Error rate (0.0 to 1.0)
    pub error_rate: f64,
    /// Last error message (if any)
    pub last_error: Option<String>,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Idle time in seconds
    pub idle_seconds: u64,
    /// Current plugin state
    pub state: String,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Global registry instance
static GLOBAL_REGISTRY: OnceLock<PluginRegistry> = OnceLock::new();

/// Get the global plugin registry
///
/// The registry is lazily initialized on first access and populated
/// with all plugins registered via `inventory::submit!`.
///
/// # Example
///
/// ```ignore
/// use waav_gateway::plugin::global_registry;
///
/// let stt = global_registry().create_stt("deepgram", config)?;
/// ```
pub fn global_registry() -> &'static PluginRegistry {
    GLOBAL_REGISTRY.get_or_init(|| {
        let registry = PluginRegistry::new();

        // Register all plugins discovered via inventory
        for constructor in inventory::iter::<PluginConstructor> {
            // Get metadata (deferred creation)
            let metadata = constructor.metadata();

            // Register STT factory if present
            if let Some(factory) = constructor.create_stt {
                let factory_arc: STTFactoryFn = Arc::new(factory);
                registry.register_stt(
                    constructor.provider_id,
                    factory_arc.clone(),
                    metadata.clone(),
                );

                // Also register aliases
                for alias in constructor.aliases {
                    registry.register_stt(alias, factory_arc.clone(), metadata.clone());
                }
            }

            // Register TTS factory if present
            if let Some(factory) = constructor.create_tts {
                let factory_arc: TTSFactoryFn = Arc::new(factory);
                registry.register_tts(
                    constructor.provider_id,
                    factory_arc.clone(),
                    metadata.clone(),
                );

                for alias in constructor.aliases {
                    registry.register_tts(alias, factory_arc.clone(), metadata.clone());
                }
            }

            // Register Realtime factory if present
            if let Some(factory) = constructor.create_realtime {
                let factory_arc: RealtimeFactoryFn = Arc::new(factory);
                registry.register_realtime(
                    constructor.provider_id,
                    factory_arc.clone(),
                    metadata.clone(),
                );

                for alias in constructor.aliases {
                    registry.register_realtime(alias, factory_arc.clone(), metadata.clone());
                }
            }

            // Register Audio Processor factory if present
            if let Some(factory) = constructor.create_audio_processor {
                let factory_arc: AudioProcessorFactoryFn = Arc::new(factory);
                let proc_metadata = constructor.processor_metadata().unwrap_or_else(|| {
                    ProcessorMetadata {
                        id: constructor.provider_id.to_string(),
                        name: constructor.provider_id.to_string(),
                        ..Default::default()
                    }
                });
                registry.register_audio_processor(
                    constructor.provider_id,
                    factory_arc.clone(),
                    proc_metadata.clone(),
                );

                for alias in constructor.aliases {
                    registry.register_audio_processor(alias, factory_arc.clone(), proc_metadata.clone());
                }
            }
        }

        tracing::info!(
            stt_count = registry.stt_provider_count(),
            tts_count = registry.tts_provider_count(),
            realtime_count = registry.realtime_provider_count(),
            audio_processor_count = registry.audio_processor_count(),
            "Plugin registry initialized"
        );

        registry
    })
}

/// Initialize the global registry
///
/// This function forces initialization of the registry. Call this during
/// application startup to ensure all plugins are registered before any
/// requests are processed.
pub fn init_registry() {
    let _ = global_registry();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new() {
        let registry = PluginRegistry::new();
        assert_eq!(registry.stt_provider_count(), 0);
        assert_eq!(registry.tts_provider_count(), 0);
        assert_eq!(registry.realtime_provider_count(), 0);
    }

    #[test]
    fn test_registry_register_stt() {
        let registry = PluginRegistry::new();

        let factory: STTFactoryFn =
            Arc::new(|_config| Err(STTError::ConfigurationError("test".to_string())));

        let metadata = ProviderMetadata::stt("test-stt", "Test STT Provider");

        registry.register_stt("test-stt", factory, metadata);

        assert!(registry.has_stt_provider("test-stt"));
        assert!(registry.has_stt_provider("TEST-STT")); // Case insensitive
        assert!(!registry.has_stt_provider("unknown"));
        assert_eq!(registry.stt_provider_count(), 1);
    }

    #[test]
    fn test_registry_provider_names() {
        let registry = PluginRegistry::new();

        let factory: STTFactoryFn =
            Arc::new(|_| Err(STTError::ConfigurationError("test".to_string())));

        registry.register_stt(
            "provider-a",
            factory.clone(),
            ProviderMetadata::stt("provider-a", "Provider A"),
        );
        registry.register_stt(
            "provider-b",
            factory,
            ProviderMetadata::stt("provider-b", "Provider B"),
        );

        let names = registry.get_stt_provider_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"provider-a".to_string()));
        assert!(names.contains(&"provider-b".to_string()));
    }

    #[test]
    fn test_registry_metadata() {
        let registry = PluginRegistry::new();

        let factory: STTFactoryFn =
            Arc::new(|_| Err(STTError::ConfigurationError("test".to_string())));

        let metadata = ProviderMetadata::stt("test", "Test Provider")
            .with_description("A test provider")
            .with_features(["streaming", "word-timestamps"]);

        registry.register_stt("test", factory, metadata);

        let retrieved = registry.get_stt_metadata("test").unwrap();
        assert_eq!(retrieved.name, "test");
        assert_eq!(retrieved.display_name, "Test Provider");
        assert!(retrieved.features.contains("streaming"));
    }

    #[test]
    fn test_registry_unknown_provider() {
        let registry = PluginRegistry::new();
        let config = STTConfig::default();

        let result = registry.create_stt("unknown", config);
        assert!(result.is_err());
    }

    #[test]
    fn test_registry_records_success_after_call() {
        let registry = PluginRegistry::new();

        // Create a factory that succeeds
        let success_factory: STTFactoryFn = Arc::new(|_config| {
            Err(STTError::ConfigurationError(
                "intentional error".to_string(),
            ))
        });

        let metadata = ProviderMetadata::stt("test-error", "Test Error Provider");
        registry.register_stt("test-error", success_factory, metadata);

        // Call should fail
        let config = STTConfig::default();
        let result = registry.create_stt("test-error", config);
        assert!(result.is_err());

        // Check that error was recorded (call_count should still increment for tracking)
        // but error_count should be 1
        let entry = registry.plugin_entries.get("test-error").unwrap();
        assert_eq!(
            entry.error_count, 1,
            "Error count should be 1 after failed call"
        );
        assert!(entry.last_error.is_some(), "Last error should be set");
        assert!(
            entry
                .last_error
                .as_ref()
                .unwrap()
                .contains("intentional error"),
            "Error message should be preserved"
        );
    }

    #[test]
    fn test_registry_phf_alias_resolution() {
        // Use the global registry which has real providers
        let registry = global_registry();

        // Test that aliases resolve to the same provider as canonical names
        // STT: "azure" should resolve to "microsoft-azure"
        assert!(registry.has_stt_provider("azure"));
        assert!(registry.has_stt_provider("microsoft-azure"));
        assert!(registry.has_stt_provider("AZURE")); // case insensitive

        // STT: "watson" should resolve to "ibm-watson"
        assert!(registry.has_stt_provider("watson"));
        assert!(registry.has_stt_provider("ibm-watson"));
        assert!(registry.has_stt_provider("ibm")); // Another alias

        // TTS: "polly" should resolve to "aws-polly"
        assert!(registry.has_tts_provider("polly"));
        assert!(registry.has_tts_provider("aws-polly"));
        assert!(registry.has_tts_provider("amazon-polly"));

        // TTS: "play.ht" should resolve to "playht"
        assert!(registry.has_tts_provider("play.ht"));
        assert!(registry.has_tts_provider("playht"));
        assert!(registry.has_tts_provider("play-ht"));

        // Realtime: "evi" should resolve to "hume"
        assert!(registry.has_realtime_provider("evi"));
        assert!(registry.has_realtime_provider("hume"));
        assert!(registry.has_realtime_provider("hume-evi"));
    }

    #[test]
    fn test_registry_records_success_on_success() {
        // Use the global registry which has real providers
        let registry = global_registry();

        // Get initial state
        let initial_call_count = registry
            .plugin_entries
            .get("deepgram")
            .map(|e| e.call_count)
            .unwrap_or(0);

        // This will fail because we don't have a valid API key,
        // but it tests that the recording happens after the call
        let config = STTConfig {
            api_key: "invalid_key".to_string(),
            ..Default::default()
        };
        let _ = registry.create_stt("deepgram", config);

        // Call count should have incremented
        let entry = registry.plugin_entries.get("deepgram").unwrap();
        assert!(
            entry.call_count > initial_call_count || entry.error_count > 0,
            "Either call_count or error_count should have incremented"
        );
    }
}

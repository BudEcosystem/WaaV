//! Plugin Registration Macros
//!
//! This module provides convenience macros for registering plugins with the
//! WaaV Gateway plugin system. These macros abstract the `inventory::submit!`
//! boilerplate and provide a more ergonomic API.
//!
//! # Example
//!
//! ```ignore
//! use waav_gateway::plugin::prelude::*;
//! use waav_gateway::register_stt_plugin;
//!
//! // Define your STT provider
//! fn create_my_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
//!     Ok(Box::new(MySTT::new(config)?))
//! }
//!
//! fn my_stt_metadata() -> ProviderMetadata {
//!     ProviderMetadata::stt("my-stt", "My Custom STT")
//!         .with_description("A custom STT provider")
//! }
//!
//! // Register the plugin
//! register_stt_plugin!("my-stt", my_stt_metadata, create_my_stt);
//! ```

/// Register an STT (Speech-to-Text) plugin with the gateway.
///
/// This macro simplifies the registration of STT providers by wrapping
/// the `inventory::submit!` call with the correct `PluginConstructor` type.
///
/// # Arguments
///
/// * `$id` - The provider identifier string (e.g., "my-stt")
/// * `$metadata_fn` - Function that returns `ProviderMetadata`
/// * `$factory_fn` - Factory function with signature `fn(STTConfig) -> Result<Box<dyn BaseSTT>, STTError>`
///
/// # Optional Arguments
///
/// * `aliases: [$alias1, $alias2, ...]` - Alternative names for the provider
///
/// # Example
///
/// ```ignore
/// register_stt_plugin!("my-stt", my_metadata, create_my_stt);
///
/// // With aliases
/// register_stt_plugin!("my-stt", my_metadata, create_my_stt, aliases: ["mystt", "my_stt"]);
/// ```
#[macro_export]
macro_rules! register_stt_plugin {
    ($id:expr, $metadata_fn:expr, $factory_fn:expr) => {
        ::inventory::submit! {
            $crate::plugin::registry::PluginConstructor::stt($id, $metadata_fn, $factory_fn)
        }
    };
    ($id:expr, $metadata_fn:expr, $factory_fn:expr, aliases: [$($alias:expr),* $(,)?]) => {
        ::inventory::submit! {
            $crate::plugin::registry::PluginConstructor::stt($id, $metadata_fn, $factory_fn)
                .with_aliases(&[$($alias),*])
        }
    };
}

/// Register a TTS (Text-to-Speech) plugin with the gateway.
///
/// This macro simplifies the registration of TTS providers by wrapping
/// the `inventory::submit!` call with the correct `PluginConstructor` type.
///
/// # Arguments
///
/// * `$id` - The provider identifier string (e.g., "my-tts")
/// * `$metadata_fn` - Function that returns `ProviderMetadata`
/// * `$factory_fn` - Factory function with signature `fn(TTSConfig) -> TTSResult<Box<dyn BaseTTS>>`
///
/// # Example
///
/// ```ignore
/// register_tts_plugin!("my-tts", my_metadata, create_my_tts);
/// ```
#[macro_export]
macro_rules! register_tts_plugin {
    ($id:expr, $metadata_fn:expr, $factory_fn:expr) => {
        ::inventory::submit! {
            $crate::plugin::registry::PluginConstructor::tts($id, $metadata_fn, $factory_fn)
        }
    };
    ($id:expr, $metadata_fn:expr, $factory_fn:expr, aliases: [$($alias:expr),* $(,)?]) => {
        ::inventory::submit! {
            $crate::plugin::registry::PluginConstructor::tts($id, $metadata_fn, $factory_fn)
                .with_aliases(&[$($alias),*])
        }
    };
}

/// Register a Realtime (Audio-to-Audio) plugin with the gateway.
///
/// This macro simplifies the registration of Realtime providers by wrapping
/// the `inventory::submit!` call with the correct `PluginConstructor` type.
///
/// # Arguments
///
/// * `$id` - The provider identifier string (e.g., "my-realtime")
/// * `$metadata_fn` - Function that returns `ProviderMetadata`
/// * `$factory_fn` - Factory function with signature `fn(RealtimeConfig) -> RealtimeResult<Box<dyn BaseRealtime>>`
///
/// # Example
///
/// ```ignore
/// register_realtime_plugin!("my-realtime", my_metadata, create_my_realtime);
/// ```
#[macro_export]
macro_rules! register_realtime_plugin {
    ($id:expr, $metadata_fn:expr, $factory_fn:expr) => {
        ::inventory::submit! {
            $crate::plugin::registry::PluginConstructor::realtime($id, $metadata_fn, $factory_fn)
        }
    };
    ($id:expr, $metadata_fn:expr, $factory_fn:expr, aliases: [$($alias:expr),* $(,)?]) => {
        ::inventory::submit! {
            $crate::plugin::registry::PluginConstructor::realtime($id, $metadata_fn, $factory_fn)
                .with_aliases(&[$($alias),*])
        }
    };
}

/// Register a WebSocket message handler plugin with the gateway.
///
/// This macro simplifies the registration of custom WebSocket message handlers.
///
/// # Arguments
///
/// * `$message_type` - The message type string this handler responds to
/// * `$handler_fn` - Handler function
///
/// # Example
///
/// ```ignore
/// async fn handle_my_message(
///     payload: serde_json::Value,
///     ctx: WSContext,
/// ) -> Result<Option<WSResponse>, WSError> {
///     // Handle the message
///     Ok(Some(WSResponse::Json(serde_json::json!({"status": "ok"}))))
/// }
///
/// register_ws_handler!("my_message", handle_my_message);
/// ```
#[macro_export]
macro_rules! register_ws_handler {
    ($message_type:expr, $handler_fn:expr) => {
        // Note: WS handlers are registered at runtime via registry.register_ws_handler()
        // This macro provides the call site but actual registration happens during init
        $crate::plugin::global_registry().register_ws_handler(
            $message_type,
            ::std::sync::Arc::new(|payload, ctx| Box::pin($handler_fn(payload, ctx))),
        );
    };
}

#[cfg(test)]
mod tests {
    use crate::core::stt::{BaseSTT, STTConfig, STTError};
    use crate::plugin::metadata::ProviderMetadata;

    // Test that the macro compiles correctly
    fn test_stt_metadata() -> ProviderMetadata {
        ProviderMetadata::stt("test-macro-stt", "Test Macro STT")
    }

    fn test_stt_factory(_config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
        Err(STTError::ConfigurationError("test".to_string()))
    }

    // Verify macro expansion compiles
    register_stt_plugin!("test-macro-stt", test_stt_metadata, test_stt_factory);

    #[test]
    fn test_macro_registered_provider() {
        use crate::plugin::global_registry;

        // The macro should have registered the provider
        let registry = global_registry();
        assert!(
            registry.has_stt_provider("test-macro-stt"),
            "Macro-registered provider should be in registry"
        );
    }
}

//! # WaaV Plugin API
//!
//! This crate provides the FFI-safe interface for creating WaaV Gateway plugins.
//! Third-party developers can use this crate to create STT, TTS, and Realtime
//! provider plugins that can be dynamically loaded by the gateway at runtime.
//!
//! # ABI Stability
//!
//! This crate uses `abi_stable` to ensure type layout compatibility across
//! different Rust compiler versions. All public types derive `StableAbi`.
//!
//! # Architecture
//!
//! Plugins export a root module with factory functions. The gateway calls these
//! factories to create provider instances. Each provider instance uses a vtable
//! pattern for method dispatch.
//!
//! # Safety
//!
//! All callback functions use raw pointers instead of references to ensure
//! `StableAbi` compatibility. Callers must ensure pointers are valid.
//!
//! # Example Plugin
//!
//! ```rust,ignore
//! use waav_plugin_api::*;
//! use abi_stable::export_root_module;
//!
//! // Define your provider state
//! struct MySTTState {
//!     api_key: String,
//!     connected: bool,
//! }
//!
//! // Implement the vtable functions
//! extern "C" fn my_connect(handle: *mut ProviderHandle) -> FFIResult {
//!     let handle = unsafe { &mut *handle };
//!     let state = unsafe { handle.as_mut::<MySTTState>() };
//!     state.connected = true;
//!     ffi_ok()
//! }
//!
//! // Export the root module
//! #[export_root_module]
//! fn get_module() -> PluginModule_Ref {
//!     PluginModule {
//!         manifest: get_manifest,
//!         init: plugin_init,
//!         shutdown: plugin_shutdown,
//!         create_stt: ROption::RSome(create_my_stt),
//!         create_tts: ROption::RNone,
//!         create_realtime: ROption::RNone,
//!     }.leak_into_prefix()
//! }
//! ```

#![allow(non_camel_case_types)]

use abi_stable::{
    declare_root_module_statics,
    library::RootModule,
    package_version_strings,
    std_types::{ROption, RResult, RString, RVec},
    StableAbi,
};

// =============================================================================
// Re-exports for plugin developers
// =============================================================================

pub use abi_stable;

// =============================================================================
// FFI Result Type
// =============================================================================

/// FFI-safe result type.
///
/// Uses `RResult` from `abi_stable` for ABI stability.
pub type FFIResult = RResult<(), RString>;

/// Helper to create success result.
pub fn ffi_ok() -> FFIResult {
    RResult::ROk(())
}

/// Helper to create error result.
pub fn ffi_err(msg: impl Into<RString>) -> FFIResult {
    RResult::RErr(msg.into())
}

// =============================================================================
// Plugin Manifest
// =============================================================================

/// Plugin manifest containing metadata about the plugin.
///
/// This is used by the gateway to identify and validate plugins before loading.
#[repr(C)]
#[derive(StableAbi, Clone, Debug)]
pub struct PluginManifest {
    /// Unique plugin identifier (e.g., "my-custom-stt")
    pub id: RString,

    /// Human-readable plugin name (e.g., "My Custom STT Provider")
    pub name: RString,

    /// Plugin version following semver (e.g., "1.0.0")
    pub version: RString,

    /// Required gateway version (semver range, e.g., ">=1.0.0, <2.0.0")
    pub gateway_version_req: RString,

    /// Plugin capabilities (STT, TTS, Realtime, etc.)
    pub capabilities: RVec<PluginCapabilityType>,

    /// Plugin author name
    pub author: RString,

    /// Plugin description
    pub description: RString,
}

impl PluginManifest {
    /// Create a new plugin manifest with required fields.
    pub fn new(id: impl Into<RString>, name: impl Into<RString>, version: impl Into<RString>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            gateway_version_req: ">=1.0.0".into(),
            capabilities: RVec::new(),
            author: RString::new(),
            description: RString::new(),
        }
    }

    /// Set the gateway version requirement.
    pub fn with_gateway_version(mut self, req: impl Into<RString>) -> Self {
        self.gateway_version_req = req.into();
        self
    }

    /// Add a capability.
    pub fn with_capability(mut self, cap: PluginCapabilityType) -> Self {
        self.capabilities.push(cap);
        self
    }

    /// Set the author.
    pub fn with_author(mut self, author: impl Into<RString>) -> Self {
        self.author = author.into();
        self
    }

    /// Set the description.
    pub fn with_description(mut self, desc: impl Into<RString>) -> Self {
        self.description = desc.into();
        self
    }
}

/// Plugin capability types.
#[repr(C)]
#[derive(StableAbi, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginCapabilityType {
    /// Speech-to-Text provider
    STT = 0,
    /// Text-to-Speech provider
    TTS = 1,
    /// Realtime audio-to-audio provider
    Realtime = 2,
    /// WebSocket message handler
    WSHandler = 3,
}

// =============================================================================
// Configuration Types (FFI-safe)
// =============================================================================

/// FFI-safe configuration passed as JSON string.
///
/// Plugins receive configuration as a JSON string and parse it internally.
/// This avoids complex struct layouts crossing FFI boundaries.
#[repr(C)]
#[derive(StableAbi, Clone, Debug, Default)]
pub struct FFIConfig {
    /// JSON configuration string
    pub json: RString,
}

impl FFIConfig {
    /// Create from JSON string.
    pub fn from_json(json: impl Into<RString>) -> Self {
        Self { json: json.into() }
    }

    /// Get as string slice.
    pub fn as_str(&self) -> &str {
        self.json.as_str()
    }
}

// =============================================================================
// Result Types (FFI-safe)
// =============================================================================

/// FFI-safe STT result.
#[repr(C)]
#[derive(StableAbi, Clone, Debug)]
pub struct FFISTTResult {
    /// Transcribed text
    pub transcript: RString,
    /// Whether this is a final result
    pub is_final: bool,
    /// Whether this marks end of speech
    pub is_speech_final: bool,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
}

impl FFISTTResult {
    /// Create a new STT result.
    pub fn new(
        transcript: impl Into<RString>,
        is_final: bool,
        is_speech_final: bool,
        confidence: f32,
    ) -> Self {
        Self {
            transcript: transcript.into(),
            is_final,
            is_speech_final,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }
}

/// FFI-safe audio data for TTS output.
#[repr(C)]
#[derive(StableAbi, Clone, Debug)]
pub struct FFIAudioData {
    /// Audio bytes
    pub data: RVec<u8>,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Audio format (e.g., "pcm", "wav")
    pub format: RString,
    /// Duration in milliseconds (0 if unknown)
    pub duration_ms: u32,
}

impl FFIAudioData {
    /// Create new audio data.
    pub fn new(data: impl Into<RVec<u8>>, sample_rate: u32, format: impl Into<RString>) -> Self {
        Self {
            data: data.into(),
            sample_rate,
            format: format.into(),
            duration_ms: 0,
        }
    }

    /// Set duration.
    pub fn with_duration(mut self, duration_ms: u32) -> Self {
        self.duration_ms = duration_ms;
        self
    }
}

/// FFI-safe realtime transcript result.
#[repr(C)]
#[derive(StableAbi, Clone, Debug)]
pub struct FFITranscriptResult {
    /// Transcribed text
    pub text: RString,
    /// Speaker role ("user" or "assistant")
    pub role: RString,
    /// Whether this is a final result
    pub is_final: bool,
}

impl FFITranscriptResult {
    /// Create a new transcript result.
    pub fn new(text: impl Into<RString>, role: impl Into<RString>, is_final: bool) -> Self {
        Self {
            text: text.into(),
            role: role.into(),
            is_final,
        }
    }
}

/// FFI-safe realtime audio data.
#[repr(C)]
#[derive(StableAbi, Clone, Debug)]
pub struct FFIRealtimeAudio {
    /// Raw audio bytes (PCM 16-bit, 24kHz, mono, little-endian)
    pub data: RVec<u8>,
    /// Sample rate in Hz
    pub sample_rate: u32,
}

impl FFIRealtimeAudio {
    /// Create new realtime audio.
    pub fn new(data: impl Into<RVec<u8>>, sample_rate: u32) -> Self {
        Self {
            data: data.into(),
            sample_rate,
        }
    }
}

// =============================================================================
// Opaque Provider Handle
// =============================================================================

/// Opaque handle to a provider instance.
///
/// This is a type-erased pointer to the plugin's internal state.
/// The plugin is responsible for managing the memory.
#[repr(C)]
#[derive(StableAbi)]
pub struct ProviderHandle {
    /// Pointer to provider state (managed by plugin)
    pub ptr: *mut (),
    /// Drop function to clean up the provider
    pub drop_fn: Option<extern "C" fn(*mut ())>,
}

impl ProviderHandle {
    /// Create a new handle from a boxed value.
    pub fn new<T>(value: T) -> Self {
        let boxed = Box::new(value);
        let ptr = Box::into_raw(boxed) as *mut ();

        extern "C" fn drop_impl<T>(ptr: *mut ()) {
            unsafe {
                let _ = Box::from_raw(ptr as *mut T);
            }
        }

        Self {
            ptr,
            drop_fn: Some(drop_impl::<T>),
        }
    }

    /// Create a null handle (for uninitialized state).
    pub fn null() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            drop_fn: None,
        }
    }

    /// Check if the handle is null.
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// Get a reference to the underlying value.
    ///
    /// # Safety
    /// The caller must ensure T matches the original type and ptr is valid.
    pub unsafe fn as_ref<T>(&self) -> &T {
        &*(self.ptr as *const T)
    }

    /// Get a mutable reference to the underlying value.
    ///
    /// # Safety
    /// The caller must ensure T matches the original type and ptr is valid.
    pub unsafe fn as_mut<T>(&mut self) -> &mut T {
        &mut *(self.ptr as *mut T)
    }
}

impl Drop for ProviderHandle {
    fn drop(&mut self) {
        if let Some(drop_fn) = self.drop_fn {
            if !self.ptr.is_null() {
                drop_fn(self.ptr);
            }
        }
    }
}

// Safety: ProviderHandle is Send + Sync because:
// 1. The plugin is responsible for making its state thread-safe
// 2. The gateway wraps providers in appropriate synchronization primitives
unsafe impl Send for ProviderHandle {}
unsafe impl Sync for ProviderHandle {}

// =============================================================================
// Callback Wrapper Types (required by abi_stable for nested function pointers)
// =============================================================================

/// Wrapper for STT result callback function.
#[repr(transparent)]
#[derive(StableAbi, Clone, Copy)]
pub struct STTResultCallbackFn {
    pub func: extern "C" fn(*const FFISTTResult, *mut ()),
}

/// Wrapper for error callback function.
#[repr(transparent)]
#[derive(StableAbi, Clone, Copy)]
pub struct ErrorCallbackFn {
    pub func: extern "C" fn(u32, *const RString, *mut ()),
}

/// Wrapper for TTS audio callback function.
#[repr(transparent)]
#[derive(StableAbi, Clone, Copy)]
pub struct TTSAudioCallbackFn {
    pub func: extern "C" fn(*const FFIAudioData, *mut ()),
}

/// Wrapper for completion callback function.
#[repr(transparent)]
#[derive(StableAbi, Clone, Copy)]
pub struct CompleteCallbackFn {
    pub func: extern "C" fn(*mut ()),
}

/// Wrapper for realtime transcript callback function.
#[repr(transparent)]
#[derive(StableAbi, Clone, Copy)]
pub struct RealtimeTranscriptCallbackFn {
    pub func: extern "C" fn(*const FFITranscriptResult, *mut ()),
}

/// Wrapper for realtime audio callback function.
#[repr(transparent)]
#[derive(StableAbi, Clone, Copy)]
pub struct RealtimeAudioCallbackFn {
    pub func: extern "C" fn(*const FFIRealtimeAudio, *mut ()),
}

// =============================================================================
// STT Provider VTable
// =============================================================================

/// VTable for STT provider operations.
///
/// All function pointers use raw pointers for StableAbi compatibility.
#[repr(C)]
#[derive(StableAbi, Clone)]
pub struct STTVTable {
    /// Connect to the STT service.
    /// `handle` points to a valid ProviderHandle.
    pub connect: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Disconnect from the STT service.
    pub disconnect: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Check if connected and ready.
    pub is_ready: extern "C" fn(handle: *const ProviderHandle) -> bool,

    /// Send audio data for transcription.
    /// `audio_data` points to audio bytes, `audio_len` is the byte count.
    pub send_audio: extern "C" fn(handle: *mut ProviderHandle, audio_data: *const u8, audio_len: usize) -> FFIResult,

    /// Set the result callback.
    /// `callback` is called with STT results; `user_data` is passed through.
    pub set_result_callback: extern "C" fn(
        handle: *mut ProviderHandle,
        callback: STTResultCallbackFn,
        user_data: *mut (),
    ),

    /// Set the error callback.
    /// `callback` is called with error code and message.
    pub set_error_callback: extern "C" fn(
        handle: *mut ProviderHandle,
        callback: ErrorCallbackFn,
        user_data: *mut (),
    ),

    /// Get provider info as JSON string.
    pub get_provider_info: extern "C" fn(handle: *const ProviderHandle) -> RString,
}

/// STT Provider instance with handle and vtable.
#[repr(C)]
#[derive(StableAbi)]
pub struct STTProvider {
    /// Provider state handle
    pub handle: ProviderHandle,
    /// VTable with method implementations
    pub vtable: STTVTable,
}

impl STTProvider {
    /// Connect to the STT service.
    pub fn connect(&mut self) -> FFIResult {
        (self.vtable.connect)(&mut self.handle)
    }

    /// Disconnect from the STT service.
    pub fn disconnect(&mut self) -> FFIResult {
        (self.vtable.disconnect)(&mut self.handle)
    }

    /// Check if ready.
    pub fn is_ready(&self) -> bool {
        (self.vtable.is_ready)(&self.handle)
    }

    /// Send audio data.
    pub fn send_audio(&mut self, audio: &[u8]) -> FFIResult {
        (self.vtable.send_audio)(&mut self.handle, audio.as_ptr(), audio.len())
    }

    /// Get provider info.
    pub fn get_provider_info(&self) -> RString {
        (self.vtable.get_provider_info)(&self.handle)
    }
}

// =============================================================================
// TTS Provider VTable
// =============================================================================

/// VTable for TTS provider operations.
#[repr(C)]
#[derive(StableAbi, Clone)]
pub struct TTSVTable {
    /// Connect to the TTS service.
    pub connect: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Disconnect from the TTS service.
    pub disconnect: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Check if connected and ready.
    pub is_ready: extern "C" fn(handle: *const ProviderHandle) -> bool,

    /// Send text for synthesis.
    /// `text` points to the text string; `flush` indicates whether to flush immediately.
    pub speak: extern "C" fn(handle: *mut ProviderHandle, text: *const RString, flush: bool) -> FFIResult,

    /// Clear queued text.
    pub clear: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Flush and process queued text.
    pub flush: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Set the audio callback.
    pub set_audio_callback: extern "C" fn(
        handle: *mut ProviderHandle,
        callback: TTSAudioCallbackFn,
        user_data: *mut (),
    ),

    /// Set the error callback.
    pub set_error_callback: extern "C" fn(
        handle: *mut ProviderHandle,
        callback: ErrorCallbackFn,
        user_data: *mut (),
    ),

    /// Set the completion callback.
    pub set_complete_callback: extern "C" fn(
        handle: *mut ProviderHandle,
        callback: CompleteCallbackFn,
        user_data: *mut (),
    ),

    /// Get provider info as JSON string.
    pub get_provider_info: extern "C" fn(handle: *const ProviderHandle) -> RString,
}

/// TTS Provider instance with handle and vtable.
#[repr(C)]
#[derive(StableAbi)]
pub struct TTSProvider {
    /// Provider state handle
    pub handle: ProviderHandle,
    /// VTable with method implementations
    pub vtable: TTSVTable,
}

impl TTSProvider {
    /// Connect to the TTS service.
    pub fn connect(&mut self) -> FFIResult {
        (self.vtable.connect)(&mut self.handle)
    }

    /// Disconnect from the TTS service.
    pub fn disconnect(&mut self) -> FFIResult {
        (self.vtable.disconnect)(&mut self.handle)
    }

    /// Check if ready.
    pub fn is_ready(&self) -> bool {
        (self.vtable.is_ready)(&self.handle)
    }

    /// Speak text.
    pub fn speak(&mut self, text: &RString, flush: bool) -> FFIResult {
        (self.vtable.speak)(&mut self.handle, text, flush)
    }

    /// Clear queued text.
    pub fn clear(&mut self) -> FFIResult {
        (self.vtable.clear)(&mut self.handle)
    }

    /// Flush queued text.
    pub fn flush(&mut self) -> FFIResult {
        (self.vtable.flush)(&mut self.handle)
    }

    /// Get provider info.
    pub fn get_provider_info(&self) -> RString {
        (self.vtable.get_provider_info)(&self.handle)
    }
}

// =============================================================================
// Realtime Provider VTable
// =============================================================================

/// VTable for Realtime provider operations.
#[repr(C)]
#[derive(StableAbi, Clone)]
pub struct RealtimeVTable {
    /// Connect to the realtime service.
    pub connect: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Disconnect from the realtime service.
    pub disconnect: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Check if connected and ready.
    pub is_ready: extern "C" fn(handle: *const ProviderHandle) -> bool,

    /// Send audio data.
    pub send_audio: extern "C" fn(handle: *mut ProviderHandle, audio_data: *const u8, audio_len: usize) -> FFIResult,

    /// Send text message.
    pub send_text: extern "C" fn(handle: *mut ProviderHandle, text: *const RString) -> FFIResult,

    /// Request the model to generate a response.
    pub create_response: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Cancel current response generation.
    pub cancel_response: extern "C" fn(handle: *mut ProviderHandle) -> FFIResult,

    /// Set the transcript callback.
    pub set_transcript_callback: extern "C" fn(
        handle: *mut ProviderHandle,
        callback: RealtimeTranscriptCallbackFn,
        user_data: *mut (),
    ),

    /// Set the audio callback.
    pub set_audio_callback: extern "C" fn(
        handle: *mut ProviderHandle,
        callback: RealtimeAudioCallbackFn,
        user_data: *mut (),
    ),

    /// Set the error callback.
    pub set_error_callback: extern "C" fn(
        handle: *mut ProviderHandle,
        callback: ErrorCallbackFn,
        user_data: *mut (),
    ),

    /// Get provider info as JSON string.
    pub get_provider_info: extern "C" fn(handle: *const ProviderHandle) -> RString,
}

/// Realtime Provider instance with handle and vtable.
#[repr(C)]
#[derive(StableAbi)]
pub struct RealtimeProvider {
    /// Provider state handle
    pub handle: ProviderHandle,
    /// VTable with method implementations
    pub vtable: RealtimeVTable,
}

impl RealtimeProvider {
    /// Connect to the realtime service.
    pub fn connect(&mut self) -> FFIResult {
        (self.vtable.connect)(&mut self.handle)
    }

    /// Disconnect from the realtime service.
    pub fn disconnect(&mut self) -> FFIResult {
        (self.vtable.disconnect)(&mut self.handle)
    }

    /// Check if ready.
    pub fn is_ready(&self) -> bool {
        (self.vtable.is_ready)(&self.handle)
    }

    /// Send audio data.
    pub fn send_audio(&mut self, audio: &[u8]) -> FFIResult {
        (self.vtable.send_audio)(&mut self.handle, audio.as_ptr(), audio.len())
    }

    /// Send text.
    pub fn send_text(&mut self, text: &RString) -> FFIResult {
        (self.vtable.send_text)(&mut self.handle, text)
    }

    /// Create response.
    pub fn create_response(&mut self) -> FFIResult {
        (self.vtable.create_response)(&mut self.handle)
    }

    /// Cancel response.
    pub fn cancel_response(&mut self) -> FFIResult {
        (self.vtable.cancel_response)(&mut self.handle)
    }

    /// Get provider info.
    pub fn get_provider_info(&self) -> RString {
        (self.vtable.get_provider_info)(&self.handle)
    }
}

// =============================================================================
// Plugin Module (Root Module)
// =============================================================================

/// Root module exported by plugins.
///
/// This struct defines the entry points that the gateway uses to interact with plugins.
/// Use the `#[export_root_module]` attribute to export this from your plugin.
#[repr(C)]
#[derive(StableAbi)]
#[sabi(kind(Prefix(prefix_ref = PluginModule_Ref)))]
pub struct PluginModule {
    /// Get the plugin manifest.
    pub manifest: extern "C" fn() -> PluginManifest,

    /// Initialize the plugin.
    ///
    /// Called once when the plugin is loaded.
    /// The config parameter contains plugin-specific configuration as JSON.
    pub init: extern "C" fn(config: *const FFIConfig) -> FFIResult,

    /// Shutdown the plugin.
    ///
    /// Called when the plugin is being unloaded.
    pub shutdown: extern "C" fn() -> FFIResult,

    /// Factory function for creating STT providers.
    ///
    /// Set to `ROption::RNone` if this plugin doesn't provide STT.
    #[sabi(last_prefix_field)]
    pub create_stt: ROption<extern "C" fn(*const FFIConfig) -> RResult<STTProvider, RString>>,

    /// Factory function for creating TTS providers.
    ///
    /// Set to `ROption::RNone` if this plugin doesn't provide TTS.
    pub create_tts: ROption<extern "C" fn(*const FFIConfig) -> RResult<TTSProvider, RString>>,

    /// Factory function for creating Realtime providers.
    ///
    /// Set to `ROption::RNone` if this plugin doesn't provide Realtime.
    pub create_realtime: ROption<extern "C" fn(*const FFIConfig) -> RResult<RealtimeProvider, RString>>,
}

impl RootModule for PluginModule_Ref {
    declare_root_module_statics! {PluginModule_Ref}

    const BASE_NAME: &'static str = "waav_plugin";
    const NAME: &'static str = "waav_plugin";
    const VERSION_STRINGS: abi_stable::sabi_types::VersionStrings = package_version_strings!();
}

// =============================================================================
// Error Codes
// =============================================================================

/// Standard error codes for plugin errors.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// No error
    Ok = 0,
    /// Connection failed
    ConnectionFailed = 1,
    /// Authentication failed
    AuthenticationFailed = 2,
    /// Configuration error
    ConfigurationError = 3,
    /// Provider error
    ProviderError = 4,
    /// Network error
    NetworkError = 5,
    /// Audio processing error
    AudioProcessingError = 6,
    /// Timeout error
    TimeoutError = 7,
    /// Internal error
    InternalError = 8,
    /// Not connected
    NotConnected = 9,
    /// Rate limited
    RateLimited = 10,
    /// Invalid input
    InvalidInput = 11,
}

impl ErrorCode {
    /// Convert error code to u32.
    pub fn as_u32(self) -> u32 {
        self as u32
    }

    /// Create from u32.
    pub fn from_u32(code: u32) -> Self {
        match code {
            0 => ErrorCode::Ok,
            1 => ErrorCode::ConnectionFailed,
            2 => ErrorCode::AuthenticationFailed,
            3 => ErrorCode::ConfigurationError,
            4 => ErrorCode::ProviderError,
            5 => ErrorCode::NetworkError,
            6 => ErrorCode::AudioProcessingError,
            7 => ErrorCode::TimeoutError,
            8 => ErrorCode::InternalError,
            9 => ErrorCode::NotConnected,
            10 => ErrorCode::RateLimited,
            _ => ErrorCode::InternalError,
        }
    }
}

// =============================================================================
// Callback Type Aliases (for convenience)
// =============================================================================

/// Type alias for STT result callback.
pub type STTResultCallback = extern "C" fn(*const FFISTTResult, *mut ());

/// Type alias for error callback.
pub type ErrorCallback = extern "C" fn(u32, *const RString, *mut ());

/// Type alias for TTS audio callback.
pub type TTSAudioCallback = extern "C" fn(*const FFIAudioData, *mut ());

/// Type alias for completion callback.
pub type CompleteCallback = extern "C" fn(*mut ());

/// Type alias for Realtime transcript callback.
pub type RealtimeTranscriptCallback = extern "C" fn(*const FFITranscriptResult, *mut ());

/// Type alias for Realtime audio callback.
pub type RealtimeAudioCallback = extern "C" fn(*const FFIRealtimeAudio, *mut ());

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_manifest_builder() {
        let manifest = PluginManifest::new("test-plugin", "Test Plugin", "1.0.0")
            .with_gateway_version(">=1.0.0, <2.0.0")
            .with_capability(PluginCapabilityType::STT)
            .with_capability(PluginCapabilityType::TTS)
            .with_author("Test Author")
            .with_description("A test plugin");

        assert_eq!(manifest.id.as_str(), "test-plugin");
        assert_eq!(manifest.name.as_str(), "Test Plugin");
        assert_eq!(manifest.version.as_str(), "1.0.0");
        assert_eq!(manifest.capabilities.len(), 2);
        assert_eq!(manifest.author.as_str(), "Test Author");
    }

    #[test]
    fn test_ffi_stt_result() {
        let result = FFISTTResult::new("Hello world", true, true, 0.95);
        assert_eq!(result.transcript.as_str(), "Hello world");
        assert!(result.is_final);
        assert!(result.is_speech_final);
        assert_eq!(result.confidence, 0.95);
    }

    #[test]
    fn test_ffi_stt_result_confidence_clamping() {
        let result = FFISTTResult::new("Test", true, false, 1.5);
        assert_eq!(result.confidence, 1.0);

        let result = FFISTTResult::new("Test", true, false, -0.5);
        assert_eq!(result.confidence, 0.0);
    }

    #[test]
    fn test_ffi_audio_data() {
        let data = vec![0u8; 1024];
        let audio = FFIAudioData::new(data, 24000, "pcm").with_duration(500);

        assert_eq!(audio.data.len(), 1024);
        assert_eq!(audio.sample_rate, 24000);
        assert_eq!(audio.format.as_str(), "pcm");
        assert_eq!(audio.duration_ms, 500);
    }

    #[test]
    fn test_error_code_conversion() {
        assert_eq!(ErrorCode::from_u32(0), ErrorCode::Ok);
        assert_eq!(ErrorCode::from_u32(1), ErrorCode::ConnectionFailed);
        assert_eq!(ErrorCode::from_u32(999), ErrorCode::InternalError);

        assert_eq!(ErrorCode::ConnectionFailed.as_u32(), 1);
    }

    #[test]
    fn test_ffi_config() {
        let config = FFIConfig::from_json(r#"{"api_key": "test"}"#);
        assert_eq!(config.as_str(), r#"{"api_key": "test"}"#);
    }

    #[test]
    fn test_provider_handle() {
        struct TestState {
            value: i32,
        }

        let handle = ProviderHandle::new(TestState { value: 42 });

        unsafe {
            let state = handle.as_ref::<TestState>();
            assert_eq!(state.value, 42);
        }

        // Handle drops and cleans up TestState
    }

    #[test]
    fn test_provider_handle_null() {
        let handle = ProviderHandle::null();
        assert!(handle.is_null());
    }

    #[test]
    fn test_ffi_result_helpers() {
        let ok = ffi_ok();
        assert!(matches!(ok, RResult::ROk(())));

        let err = ffi_err("test error");
        assert!(matches!(err, RResult::RErr(_)));
    }

    #[test]
    fn test_ffi_transcript_result() {
        let result = FFITranscriptResult::new("Hello", "user", true);
        assert_eq!(result.text.as_str(), "Hello");
        assert_eq!(result.role.as_str(), "user");
        assert!(result.is_final);
    }

    #[test]
    fn test_ffi_realtime_audio() {
        let data = vec![0u8; 512];
        let audio = FFIRealtimeAudio::new(data, 24000);
        assert_eq!(audio.data.len(), 512);
        assert_eq!(audio.sample_rate, 24000);
    }

    #[test]
    fn test_callback_wrapper_types() {
        // Verify callback wrapper types have correct size (same as raw function pointer)
        assert_eq!(
            std::mem::size_of::<STTResultCallbackFn>(),
            std::mem::size_of::<extern "C" fn(*const FFISTTResult, *mut ())>()
        );
        assert_eq!(
            std::mem::size_of::<ErrorCallbackFn>(),
            std::mem::size_of::<extern "C" fn(u32, *const RString, *mut ())>()
        );
    }
}

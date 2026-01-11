//! FFI Adapters
//!
//! This module provides adapter types that wrap FFI providers from the plugin API
//! and implement the gateway's native Rust traits (BaseSTT, BaseTTS, BaseRealtime).
//!
//! # Architecture
//!
//! Each adapter:
//! - Wraps an FFI provider type (STTProvider, TTSProvider, RealtimeProvider)
//! - Implements the corresponding Rust async trait
//! - Bridges callbacks between FFI (function pointers) and Rust (async closures)
//! - Handles thread synchronization for callbacks
//! - Properly manages callback memory lifecycle to prevent leaks
//!
//! # Memory Safety
//!
//! Callbacks are stored using `TypeErasedCallback` which preserves the original type's
//! drop function. When the adapter is dropped, each callback is freed using its correct
//! type, preventing heap corruption from size/alignment mismatches.

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use waav_plugin_api::{
    ErrorCallbackFn, FFISTTResult, FFIAudioData, FFITranscriptResult, FFIRealtimeAudio,
    RealtimeAudioCallbackFn, RealtimeProvider, RealtimeTranscriptCallbackFn,
    STTProvider, STTResultCallbackFn, TTSAudioCallbackFn, TTSProvider,
    CompleteCallbackFn,
};

// =============================================================================
// Type-Safe Callback Memory Management
// =============================================================================

/// Type-erased callback storage with proper cleanup.
///
/// This struct stores a raw pointer along with the correct drop function for
/// the original type. This ensures that when the callback is dropped, we free
/// the memory with the correct size and alignment, preventing heap corruption.
struct TypeErasedCallback {
    /// Raw pointer to the boxed callback
    ptr: *mut (),
    /// Drop function that knows the actual type
    drop_fn: unsafe fn(*mut ()),
}

impl TypeErasedCallback {
    /// Create a new type-erased callback from a value.
    ///
    /// The value is boxed and its pointer is stored along with a type-specific
    /// drop function that will correctly free the memory when dropped.
    fn new<T>(value: T) -> Self {
        let boxed = Box::new(value);
        let ptr = Box::into_raw(boxed) as *mut ();

        // This function is monomorphized for each T, so it knows the correct type
        unsafe fn drop_typed<T>(ptr: *mut ()) {
            if !ptr.is_null() {
                // SAFETY: ptr was created from Box::into_raw(Box<T>), so this is safe
                unsafe { let _ = Box::from_raw(ptr as *mut T); }
            }
        }

        Self {
            ptr,
            drop_fn: drop_typed::<T>,
        }
    }

    /// Get the raw pointer for use as FFI user_data
    fn as_ptr(&self) -> *mut () {
        self.ptr
    }
}

impl Drop for TypeErasedCallback {
    fn drop(&mut self) {
        // SAFETY: drop_fn was created with the correct type in new<T>()
        unsafe { (self.drop_fn)(self.ptr); }
    }
}

// SAFETY: The stored callback pointers point to Send+Sync types (required by async traits)
// and the drop function is safe to call from any thread.
unsafe impl Send for TypeErasedCallback {}
unsafe impl Sync for TypeErasedCallback {}

/// Manages callbacks with proper type-aware cleanup.
///
/// This replaces the broken `CallbackPointers` struct that incorrectly freed
/// memory using `Box<()>`. This new implementation preserves type information
/// through stored drop functions, ensuring correct memory deallocation.
struct CallbackStorage {
    /// Type-erased callbacks that will be properly freed on drop
    callbacks: Vec<TypeErasedCallback>,
}

impl CallbackStorage {
    fn new() -> Self {
        Self {
            callbacks: Vec::new(),
        }
    }

    /// Store a callback and return its raw pointer for FFI use.
    ///
    /// The callback will be properly freed when this `CallbackStorage` is dropped,
    /// using the correct type information to ensure proper memory deallocation.
    fn store<T: 'static>(&mut self, value: T) -> *mut () {
        let callback = TypeErasedCallback::new(value);
        let ptr = callback.as_ptr();
        self.callbacks.push(callback);
        ptr
    }

    /// Clear all stored callbacks, freeing their memory properly.
    #[allow(dead_code)]
    fn clear(&mut self) {
        self.callbacks.clear();
    }
}

use crate::core::realtime::{
    BaseRealtime, ConnectionState as RealtimeConnectionState, RealtimeAudioData, RealtimeConfig,
    RealtimeError, RealtimeResult, TranscriptResult, TranscriptRole,
    TranscriptCallback, AudioOutputCallback, RealtimeErrorCallback,
    FunctionCallCallback, SpeechEventCallback, ResponseDoneCallback, ReconnectionCallback,
};
use crate::core::stt::{BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback};
use crate::core::tts::{
    AudioCallback, AudioData, BaseTTS, ConnectionState as TTSConnectionState, TTSConfig, TTSError,
    TTSResult as TTSOpResult,
};

// =============================================================================
// STT Adapter
// =============================================================================

/// Adapter that wraps an FFI STT provider and implements BaseSTT.
pub struct FFISTTAdapter {
    /// The underlying FFI provider
    provider: Mutex<STTProvider>,
    /// Stored config for get_config
    config: Mutex<Option<STTConfig>>,
    /// Current connection state
    connected: Mutex<bool>,
    /// Type-safe callback storage with proper cleanup on drop
    callback_storage: Mutex<CallbackStorage>,
}

impl FFISTTAdapter {
    /// Create a new STT adapter wrapping the given FFI provider.
    pub fn new(provider: STTProvider) -> Self {
        Self {
            provider: Mutex::new(provider),
            config: Mutex::new(None),
            connected: Mutex::new(false),
            callback_storage: Mutex::new(CallbackStorage::new()),
        }
    }
}

#[async_trait]
impl BaseSTT for FFISTTAdapter {
    fn new(_config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        Err(STTError::ConfigurationError(
            "Use factory function to create FFI STT providers".into(),
        ))
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.connect()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => {
                *self.connected.lock().unwrap() = true;
                Ok(())
            }
            abi_stable::std_types::RResult::RErr(e) => {
                Err(STTError::ConnectionFailed(e.to_string()))
            }
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.disconnect()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => {
                *self.connected.lock().unwrap() = false;
                Ok(())
            }
            abi_stable::std_types::RResult::RErr(e) => {
                Err(STTError::ProviderError(e.to_string()))
            }
        }
    }

    fn is_ready(&self) -> bool {
        let provider = self.provider.lock().unwrap();
        provider.is_ready()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.send_audio(&audio_data)
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => Ok(()),
            abi_stable::std_types::RResult::RErr(e) => {
                Err(STTError::AudioProcessingError(e.to_string()))
            }
        }
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        // Store callback with type-safe cleanup
        let user_data = self.callback_storage.lock().unwrap().store(callback);

        extern "C" fn stt_result_callback(result: *const FFISTTResult, user_data: *mut ()) {
            if result.is_null() || user_data.is_null() {
                return;
            }

            unsafe {
                let ffi_result = &*result;
                let rust_result = STTResult {
                    transcript: ffi_result.transcript.to_string(),
                    is_final: ffi_result.is_final,
                    is_speech_final: ffi_result.is_speech_final,
                    confidence: ffi_result.confidence,
                };

                let callback = &*(user_data as *const STTResultCallback);
                let future = callback(rust_result);
                tokio::spawn(future);
            }
        }

        let callback_fn = STTResultCallbackFn {
            func: stt_result_callback,
        };

        let mut provider = self.provider.lock().unwrap();
        // Get the vtable function and call it with handle reference
        let set_callback = provider.vtable.set_result_callback;
        set_callback(&mut provider.handle, callback_fn, user_data);

        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        // Store callback with type-safe cleanup
        let user_data = self.callback_storage.lock().unwrap().store(callback);

        extern "C" fn stt_error_callback(
            error_code: u32,
            message: *const abi_stable::std_types::RString,
            user_data: *mut (),
        ) {
            if message.is_null() || user_data.is_null() {
                return;
            }

            unsafe {
                let msg = &*message;
                let error = match error_code {
                    1 => STTError::ConnectionFailed(msg.to_string()),
                    2 => STTError::AuthenticationFailed(msg.to_string()),
                    3 => STTError::ConfigurationError(msg.to_string()),
                    4 => STTError::ProviderError(msg.to_string()),
                    5 => STTError::NetworkError(msg.to_string()),
                    6 => STTError::AudioProcessingError(msg.to_string()),
                    _ => STTError::ProviderError(msg.to_string()),
                };

                let callback = &*(user_data as *const STTErrorCallback);
                let future = callback(error);
                tokio::spawn(future);
            }
        }

        let callback_fn = ErrorCallbackFn {
            func: stt_error_callback,
        };

        let mut provider = self.provider.lock().unwrap();
        let set_callback = provider.vtable.set_error_callback;
        set_callback(&mut provider.handle, callback_fn, user_data);

        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        None
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        *self.config.lock().unwrap() = Some(config);
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "dynamic-plugin-stt"
    }
}

unsafe impl Send for FFISTTAdapter {}
unsafe impl Sync for FFISTTAdapter {}

// =============================================================================
// TTS Adapter
// =============================================================================

/// Adapter that wraps an FFI TTS provider and implements BaseTTS.
pub struct FFITTSAdapter {
    provider: Mutex<TTSProvider>,
    connected: Mutex<bool>,
    /// Type-safe callback storage with proper cleanup on drop
    callback_storage: Mutex<CallbackStorage>,
}

impl FFITTSAdapter {
    /// Create a new TTS adapter wrapping the given FFI provider.
    pub fn new(provider: TTSProvider) -> Self {
        Self {
            provider: Mutex::new(provider),
            connected: Mutex::new(false),
            callback_storage: Mutex::new(CallbackStorage::new()),
        }
    }
}

#[async_trait]
impl BaseTTS for FFITTSAdapter {
    fn new(_config: TTSConfig) -> TTSOpResult<Self>
    where
        Self: Sized,
    {
        Err(TTSError::InternalError(
            "Use factory function to create FFI TTS providers".into(),
        ))
    }

    async fn connect(&mut self) -> TTSOpResult<()> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.connect()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => {
                *self.connected.lock().unwrap() = true;
                Ok(())
            }
            abi_stable::std_types::RResult::RErr(e) => {
                Err(TTSError::ConnectionFailed(e.to_string()))
            }
        }
    }

    async fn disconnect(&mut self) -> TTSOpResult<()> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.disconnect()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => {
                *self.connected.lock().unwrap() = false;
                Ok(())
            }
            abi_stable::std_types::RResult::RErr(e) => {
                Err(TTSError::ProviderError(e.to_string()))
            }
        }
    }

    fn is_ready(&self) -> bool {
        let provider = self.provider.lock().unwrap();
        provider.is_ready()
    }

    fn get_connection_state(&self) -> TTSConnectionState {
        if *self.connected.lock().unwrap() {
            TTSConnectionState::Connected
        } else {
            TTSConnectionState::Disconnected
        }
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSOpResult<()> {
        let text_rstring: abi_stable::std_types::RString = text.into();
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.speak(&text_rstring, flush)
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => Ok(()),
            abi_stable::std_types::RResult::RErr(e) => {
                Err(TTSError::AudioGenerationFailed(e.to_string()))
            }
        }
    }

    async fn clear(&mut self) -> TTSOpResult<()> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.clear()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => Ok(()),
            abi_stable::std_types::RResult::RErr(e) => {
                Err(TTSError::ProviderError(e.to_string()))
            }
        }
    }

    async fn flush(&self) -> TTSOpResult<()> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.flush()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => Ok(()),
            abi_stable::std_types::RResult::RErr(e) => {
                Err(TTSError::ProviderError(e.to_string()))
            }
        }
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSOpResult<()> {
        // Store callback with type-safe cleanup
        let user_data = self.callback_storage.lock().unwrap().store(callback);

        extern "C" fn tts_audio_callback(audio: *const FFIAudioData, user_data: *mut ()) {
            if audio.is_null() || user_data.is_null() {
                return;
            }

            unsafe {
                let ffi_audio = &*audio;
                let rust_audio = AudioData {
                    data: ffi_audio.data.to_vec(),
                    sample_rate: ffi_audio.sample_rate,
                    format: ffi_audio.format.to_string(),
                    duration_ms: if ffi_audio.duration_ms > 0 {
                        Some(ffi_audio.duration_ms)
                    } else {
                        None
                    },
                };

                let callback = &*(user_data as *const Arc<dyn AudioCallback>);
                let future = callback.on_audio(rust_audio);
                tokio::spawn(future);
            }
        }

        extern "C" fn tts_error_callback(
            _error_code: u32,
            message: *const abi_stable::std_types::RString,
            user_data: *mut (),
        ) {
            if message.is_null() || user_data.is_null() {
                return;
            }

            unsafe {
                let msg = &*message;
                let error = TTSError::ProviderError(msg.to_string());

                let callback = &*(user_data as *const Arc<dyn AudioCallback>);
                let future = callback.on_error(error);
                tokio::spawn(future);
            }
        }

        extern "C" fn tts_complete_callback(user_data: *mut ()) {
            if user_data.is_null() {
                return;
            }

            unsafe {
                let callback = &*(user_data as *const Arc<dyn AudioCallback>);
                let future = callback.on_complete();
                tokio::spawn(future);
            }
        }

        let audio_callback_fn = TTSAudioCallbackFn {
            func: tts_audio_callback,
        };
        let error_callback_fn = ErrorCallbackFn {
            func: tts_error_callback,
        };
        let complete_callback_fn = CompleteCallbackFn {
            func: tts_complete_callback,
        };

        let mut provider = self.provider.lock().unwrap();
        // Get function pointers before borrowing handle
        let set_audio = provider.vtable.set_audio_callback;
        let set_error = provider.vtable.set_error_callback;
        let set_complete = provider.vtable.set_complete_callback;

        set_audio(&mut provider.handle, audio_callback_fn, user_data);
        set_error(&mut provider.handle, error_callback_fn, user_data);
        set_complete(&mut provider.handle, complete_callback_fn, user_data);

        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        let info = {
            let provider = self.provider.lock().unwrap();
            provider.get_provider_info()
        };

        serde_json::from_str(info.as_str()).unwrap_or_else(|_| {
            serde_json::json!({
                "provider": "dynamic-plugin-tts",
                "type": "ffi"
            })
        })
    }
}

unsafe impl Send for FFITTSAdapter {}
unsafe impl Sync for FFITTSAdapter {}

// =============================================================================
// Realtime Adapter
// =============================================================================

/// Adapter that wraps an FFI Realtime provider and implements BaseRealtime.
pub struct FFIRealtimeAdapter {
    provider: Mutex<RealtimeProvider>,
    config: Mutex<Option<RealtimeConfig>>,
    connected: Mutex<bool>,
    /// Type-safe callback storage with proper cleanup on drop
    callback_storage: Mutex<CallbackStorage>,
}

impl FFIRealtimeAdapter {
    /// Create a new Realtime adapter wrapping the given FFI provider.
    pub fn new(provider: RealtimeProvider) -> Self {
        Self {
            provider: Mutex::new(provider),
            config: Mutex::new(None),
            connected: Mutex::new(false),
            callback_storage: Mutex::new(CallbackStorage::new()),
        }
    }
}

#[async_trait]
impl BaseRealtime for FFIRealtimeAdapter {
    fn new(_config: RealtimeConfig) -> RealtimeResult<Self>
    where
        Self: Sized,
    {
        Err(RealtimeError::InvalidConfiguration(
            "Use factory function to create FFI Realtime providers".into(),
        ))
    }

    async fn connect(&mut self) -> RealtimeResult<()> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.connect()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => {
                *self.connected.lock().unwrap() = true;
                Ok(())
            }
            abi_stable::std_types::RResult::RErr(e) => {
                Err(RealtimeError::ConnectionFailed(e.to_string()))
            }
        }
    }

    async fn disconnect(&mut self) -> RealtimeResult<()> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.disconnect()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => {
                *self.connected.lock().unwrap() = false;
                Ok(())
            }
            abi_stable::std_types::RResult::RErr(e) => {
                Err(RealtimeError::ProviderError(e.to_string()))
            }
        }
    }

    fn is_ready(&self) -> bool {
        let provider = self.provider.lock().unwrap();
        provider.is_ready()
    }

    fn get_connection_state(&self) -> RealtimeConnectionState {
        if *self.connected.lock().unwrap() {
            RealtimeConnectionState::Connected
        } else {
            RealtimeConnectionState::Disconnected
        }
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> RealtimeResult<()> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.send_audio(&audio_data)
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => Ok(()),
            abi_stable::std_types::RResult::RErr(e) => {
                Err(RealtimeError::ProviderError(e.to_string()))
            }
        }
    }

    async fn send_text(&mut self, text: &str) -> RealtimeResult<()> {
        let text_rstring: abi_stable::std_types::RString = text.into();
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.send_text(&text_rstring)
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => Ok(()),
            abi_stable::std_types::RResult::RErr(e) => {
                Err(RealtimeError::ProviderError(e.to_string()))
            }
        }
    }

    async fn create_response(&mut self) -> RealtimeResult<()> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.create_response()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => Ok(()),
            abi_stable::std_types::RResult::RErr(e) => {
                Err(RealtimeError::ProviderError(e.to_string()))
            }
        }
    }

    async fn cancel_response(&mut self) -> RealtimeResult<()> {
        let result = {
            let mut provider = self.provider.lock().unwrap();
            provider.cancel_response()
        };

        match result {
            abi_stable::std_types::RResult::ROk(()) => Ok(()),
            abi_stable::std_types::RResult::RErr(e) => {
                Err(RealtimeError::ProviderError(e.to_string()))
            }
        }
    }

    async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
        Ok(())
    }

    async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
        Ok(())
    }

    fn on_transcript(&mut self, callback: TranscriptCallback) -> RealtimeResult<()> {
        // Store callback with type-safe cleanup
        let user_data = self.callback_storage.lock().unwrap().store(callback);

        extern "C" fn realtime_transcript_callback(
            result: *const FFITranscriptResult,
            user_data: *mut (),
        ) {
            if result.is_null() || user_data.is_null() {
                return;
            }

            unsafe {
                let ffi_result = &*result;
                let role_str = ffi_result.role.as_str();
                let role = if role_str == "user" {
                    TranscriptRole::User
                } else {
                    TranscriptRole::Assistant
                };
                let rust_result = TranscriptResult {
                    text: ffi_result.text.to_string(),
                    role,
                    is_final: ffi_result.is_final,
                    item_id: None,
                };

                let callback = &*(user_data as *const TranscriptCallback);
                let future = callback(rust_result);
                tokio::spawn(future);
            }
        }

        let callback_fn = RealtimeTranscriptCallbackFn {
            func: realtime_transcript_callback,
        };

        let mut provider = self.provider.lock().unwrap();
        let set_callback = provider.vtable.set_transcript_callback;
        set_callback(&mut provider.handle, callback_fn, user_data);

        Ok(())
    }

    fn on_audio(&mut self, callback: AudioOutputCallback) -> RealtimeResult<()> {
        // Store callback with type-safe cleanup
        let user_data = self.callback_storage.lock().unwrap().store(callback);

        extern "C" fn realtime_audio_callback(audio: *const FFIRealtimeAudio, user_data: *mut ()) {
            if audio.is_null() || user_data.is_null() {
                return;
            }

            unsafe {
                let ffi_audio = &*audio;
                let rust_audio = RealtimeAudioData {
                    data: Bytes::from(ffi_audio.data.to_vec()),
                    sample_rate: ffi_audio.sample_rate,
                    item_id: None,
                    response_id: None,
                };

                let callback = &*(user_data as *const AudioOutputCallback);
                let future = callback(rust_audio);
                tokio::spawn(future);
            }
        }

        let callback_fn = RealtimeAudioCallbackFn {
            func: realtime_audio_callback,
        };

        let mut provider = self.provider.lock().unwrap();
        let set_callback = provider.vtable.set_audio_callback;
        set_callback(&mut provider.handle, callback_fn, user_data);

        Ok(())
    }

    fn on_error(&mut self, callback: RealtimeErrorCallback) -> RealtimeResult<()> {
        // Store callback with type-safe cleanup
        let user_data = self.callback_storage.lock().unwrap().store(callback);

        extern "C" fn realtime_error_callback(
            error_code: u32,
            message: *const abi_stable::std_types::RString,
            user_data: *mut (),
        ) {
            if message.is_null() || user_data.is_null() {
                return;
            }

            unsafe {
                let msg = &*message;
                let error = match error_code {
                    1 => RealtimeError::ConnectionFailed(msg.to_string()),
                    2 => RealtimeError::AuthenticationFailed(msg.to_string()),
                    3 => RealtimeError::InvalidConfiguration(msg.to_string()),
                    _ => RealtimeError::ProviderError(msg.to_string()),
                };

                let callback = &*(user_data as *const RealtimeErrorCallback);
                let future = callback(error);
                tokio::spawn(future);
            }
        }

        let callback_fn = ErrorCallbackFn {
            func: realtime_error_callback,
        };

        let mut provider = self.provider.lock().unwrap();
        let set_callback = provider.vtable.set_error_callback;
        set_callback(&mut provider.handle, callback_fn, user_data);

        Ok(())
    }

    fn on_function_call(&mut self, _callback: FunctionCallCallback) -> RealtimeResult<()> {
        // FFI plugins don't support function calls yet
        Ok(())
    }

    fn on_speech_event(&mut self, _callback: SpeechEventCallback) -> RealtimeResult<()> {
        // FFI plugins don't support speech events yet
        Ok(())
    }

    fn on_response_done(&mut self, _callback: ResponseDoneCallback) -> RealtimeResult<()> {
        // FFI plugins don't support response done callbacks yet
        Ok(())
    }

    fn on_reconnection(&mut self, _callback: ReconnectionCallback) -> RealtimeResult<()> {
        // FFI plugins don't support reconnection callbacks yet
        Ok(())
    }

    async fn update_session(&mut self, config: RealtimeConfig) -> RealtimeResult<()> {
        *self.config.lock().unwrap() = Some(config);
        Ok(())
    }

    async fn submit_function_result(&mut self, _call_id: &str, _result: &str) -> RealtimeResult<()> {
        // FFI plugins don't support function results yet
        Err(RealtimeError::ProviderError(
            "Function results not supported by FFI plugins".into(),
        ))
    }

    fn get_provider_info(&self) -> serde_json::Value {
        let info = {
            let provider = self.provider.lock().unwrap();
            provider.get_provider_info()
        };

        serde_json::from_str(info.as_str()).unwrap_or_else(|_| {
            serde_json::json!({
                "provider": "dynamic-plugin-realtime",
                "type": "ffi"
            })
        })
    }
}

unsafe impl Send for FFIRealtimeAdapter {}
unsafe impl Sync for FFIRealtimeAdapter {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_types_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FFISTTAdapter>();
        assert_send_sync::<FFITTSAdapter>();
        assert_send_sync::<FFIRealtimeAdapter>();
    }
}

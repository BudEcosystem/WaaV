//! Test Plugin for WaaV Gateway
//!
//! This plugin demonstrates how to create a dynamic STT plugin that can be
//! loaded at runtime by the gateway.
//!
//! # Building
//!
//! ```bash
//! cargo build --release
//! ```
//!
//! The resulting `.so`/`.dll`/`.dylib` file will be in `target/release/`.
//! Rename it to follow the naming convention:
//! - Linux: `libwaav_plugin_test.so`
//! - macOS: `libwaav_plugin_test.dylib`
//! - Windows: `waav_plugin_test.dll`
//!
//! # Installation
//!
//! 1. Create a plugin directory: `mkdir -p /opt/waav/plugins/test`
//! 2. Copy the plugin: `cp target/release/libwaav_plugin_test.so /opt/waav/plugins/test/`
//! 3. Configure the gateway:
//!    ```yaml
//!    plugins:
//!      enabled: true
//!      plugin_dir: /opt/waav/plugins
//!    ```
//! 4. Restart the gateway

use abi_stable::{
    export_root_module,
    prefix_type::PrefixTypeTrait,
    sabi_extern_fn,
    std_types::{ROption, RResult, RString},
};
use waav_plugin_api::{
    ErrorCallbackFn, FFIConfig, FFISTTResult, PluginCapabilityType, PluginManifest,
    PluginModule, PluginModule_Ref, ProviderHandle, STTProvider, STTResultCallbackFn,
    STTVTable, ffi_ok, ffi_err,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Plugin state stored in the ProviderHandle
struct TestSTTState {
    /// Whether the provider is connected
    connected: AtomicBool,
    /// Audio bytes received counter
    bytes_received: AtomicU64,
    /// Result callback
    result_callback: Option<(STTResultCallbackFn, *mut ())>,
    /// Error callback
    error_callback: Option<(ErrorCallbackFn, *mut ())>,
}

impl Default for TestSTTState {
    fn default() -> Self {
        Self {
            connected: AtomicBool::new(false),
            bytes_received: AtomicU64::new(0),
            result_callback: None,
            error_callback: None,
        }
    }
}

// FFI functions for the STT VTable

extern "C" fn test_stt_connect(handle: *mut ProviderHandle) -> RResult<(), RString> {
    if handle.is_null() {
        return ffi_err("Null handle");
    }
    unsafe {
        let handle = &mut *handle;
        if handle.is_null() {
            return ffi_err("Invalid handle state");
        }
        let state = handle.as_mut::<TestSTTState>();
        state.connected.store(true, Ordering::SeqCst);
    }
    ffi_ok()
}

extern "C" fn test_stt_disconnect(handle: *mut ProviderHandle) -> RResult<(), RString> {
    if handle.is_null() {
        return ffi_err("Null handle");
    }
    unsafe {
        let handle = &mut *handle;
        if handle.is_null() {
            return ffi_err("Invalid handle state");
        }
        let state = handle.as_mut::<TestSTTState>();
        state.connected.store(false, Ordering::SeqCst);
    }
    ffi_ok()
}

extern "C" fn test_stt_is_ready(handle: *const ProviderHandle) -> bool {
    if handle.is_null() {
        return false;
    }
    unsafe {
        let handle = &*handle;
        if handle.is_null() {
            return false;
        }
        let state = handle.as_ref::<TestSTTState>();
        state.connected.load(Ordering::SeqCst)
    }
}

extern "C" fn test_stt_send_audio(
    handle: *mut ProviderHandle,
    audio_data: *const u8,
    audio_len: usize,
) -> RResult<(), RString> {
    if handle.is_null() || audio_data.is_null() {
        return ffi_err("Null handle or audio data");
    }

    unsafe {
        let handle = &mut *handle;
        if handle.is_null() {
            return ffi_err("Invalid handle state");
        }

        let state = handle.as_mut::<TestSTTState>();

        // Update bytes received counter
        let prev = state.bytes_received.fetch_add(audio_len as u64, Ordering::SeqCst);
        let total = prev + audio_len as u64;

        // Generate a mock transcript every 16000 bytes (about 1 second of 16kHz audio)
        if total / 16000 > prev / 16000 {
            if let Some((callback_fn, user_data)) = &state.result_callback {
                let transcript = format!("Test transcript at {} bytes", total);
                let result = FFISTTResult {
                    transcript: transcript.into(),
                    is_final: false,
                    is_speech_final: false,
                    confidence: 0.95,
                };
                (callback_fn.func)(&result as *const _, *user_data);
            }
        }
    }

    ffi_ok()
}

extern "C" fn test_stt_set_result_callback(
    handle: *mut ProviderHandle,
    callback: STTResultCallbackFn,
    user_data: *mut (),
) {
    if handle.is_null() {
        return;
    }
    unsafe {
        let handle = &mut *handle;
        if !handle.is_null() {
            let state = handle.as_mut::<TestSTTState>();
            state.result_callback = Some((callback, user_data));
        }
    }
}

extern "C" fn test_stt_set_error_callback(
    handle: *mut ProviderHandle,
    callback: ErrorCallbackFn,
    user_data: *mut (),
) {
    if handle.is_null() {
        return;
    }
    unsafe {
        let handle = &mut *handle;
        if !handle.is_null() {
            let state = handle.as_mut::<TestSTTState>();
            state.error_callback = Some((callback, user_data));
        }
    }
}

extern "C" fn test_stt_get_provider_info(
    _handle: *const ProviderHandle,
) -> RString {
    r#"{"provider": "test-stt", "version": "1.0.0", "type": "dynamic"}"#.into()
}

/// Create the VTable for our STT provider
const TEST_STT_VTABLE: STTVTable = STTVTable {
    connect: test_stt_connect,
    disconnect: test_stt_disconnect,
    is_ready: test_stt_is_ready,
    send_audio: test_stt_send_audio,
    set_result_callback: test_stt_set_result_callback,
    set_error_callback: test_stt_set_error_callback,
    get_provider_info: test_stt_get_provider_info,
};

/// Factory function to create an STT provider
#[sabi_extern_fn]
fn create_stt(_config: *const FFIConfig) -> RResult<STTProvider, RString> {
    // Create state using ProviderHandle::new which sets up automatic cleanup
    let state = TestSTTState::default();
    let handle = ProviderHandle::new(state);

    RResult::ROk(STTProvider {
        handle,
        vtable: TEST_STT_VTABLE,
    })
}

/// Return the plugin manifest
#[sabi_extern_fn]
fn get_manifest() -> PluginManifest {
    PluginManifest::new("test-stt", "Test STT Plugin", "1.0.0")
        .with_gateway_version(">=1.0.0")
        .with_capability(PluginCapabilityType::STT)
        .with_author("WaaV Team")
        .with_description("A test STT plugin that generates mock transcripts")
}

/// Initialize the plugin
#[sabi_extern_fn]
fn init(_config: *const FFIConfig) -> RResult<(), RString> {
    // Plugin initialization (e.g., load models, initialize resources)
    ffi_ok()
}

/// Shutdown the plugin
#[sabi_extern_fn]
fn shutdown() -> RResult<(), RString> {
    // Plugin cleanup
    ffi_ok()
}

/// Export the root module that the gateway will load
#[export_root_module]
fn get_root_module() -> PluginModule_Ref {
    PluginModule {
        manifest: get_manifest,
        init,
        shutdown,
        create_stt: ROption::RSome(create_stt),
        create_tts: ROption::RNone,
        create_realtime: ROption::RNone,
    }
    .leak_into_prefix()
}

# WaaV Gateway Examples

This directory contains example plugins and integrations for the WaaV Gateway.

## Available Examples

### 1. Resemble AI TTS Plugin (`resemble-tts-plugin/`)

A production-ready TTS plugin that integrates with the [Resemble AI](https://www.resemble.ai/) voice synthesis API.

**Features:**
- HTTP streaming for low-latency audio delivery
- Multiple voice support
- Configurable sample rates and audio formats
- SSML support for prosody control
- Production-grade reliability:
  - Retry logic with exponential backoff
  - Circuit breaker for failure isolation
  - Request timeouts and size limits
  - Lock-free callback invocation

**Building:**

```bash
cd resemble-tts-plugin
cargo build --release
```

**Output:** `target/release/libwaav_plugin_resemble.so`

**Installation:**

1. Build the plugin
2. Copy to plugin directory:
   ```bash
   mkdir -p /opt/waav/plugins/resemble
   cp target/release/libwaav_plugin_resemble.so /opt/waav/plugins/resemble/
   ```
3. Configure gateway:
   ```yaml
   plugins:
     enabled: true
     plugin_dir: /opt/waav/plugins
   ```
4. Restart gateway

**Usage:**

```json
{
  "type": "config",
  "tts_config": {
    "provider": "resemble",
    "voice_id": "your-voice-uuid",
    "model": "chatterbox"
  }
}
```

---

### 2. Test Plugin (`test-plugin/`)

A minimal test plugin for verifying the dynamic plugin loading system.

**Purpose:**
- Verify plugin registration and discovery
- Test FFI boundary safety
- Benchmark plugin overhead

**Building:**

```bash
cd test-plugin
cargo build --release
```

---

## Creating Your Own Plugin

### Prerequisites

1. Rust 1.75+
2. `waav-plugin-api` crate (from `../plugin-api/`)

### Project Setup

Create a new Rust library project:

```bash
cargo new --lib my-awesome-plugin
cd my-awesome-plugin
```

Configure `Cargo.toml`:

```toml
[package]
name = "waav-plugin-myawesome"
version = "1.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]  # Dynamic library for runtime loading

[dependencies]
waav-plugin-api = { path = "../plugin-api" }
abi_stable = "0.11"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### Implementing a TTS Plugin

```rust
use abi_stable::{
    export_root_module,
    prefix_type::PrefixTypeTrait,
    sabi_extern_fn,
    std_types::{ROption, RResult, RString, RVec},
};
use waav_plugin_api::{
    CompleteCallbackFn, ErrorCallbackFn, FFIAudioData, FFIConfig,
    PluginCapabilityType, PluginManifest, PluginModule, PluginModule_Ref,
    ProviderHandle, TTSAudioCallbackFn, TTSProvider, TTSVTable,
    ffi_err, ffi_ok, ErrorCode,
};

// Export the plugin module
#[export_root_module]
fn get_root_module() -> PluginModule_Ref {
    PluginModule {
        get_manifest,
        create_tts_provider: ROption::RSome(create_tts_provider),
        create_stt_provider: ROption::RNone,
        create_realtime_provider: ROption::RNone,
    }
    .leak_into_prefix()
}

// Return plugin metadata
#[sabi_extern_fn]
fn get_manifest() -> PluginManifest {
    PluginManifest {
        name: RString::from("my-awesome-tts"),
        version: RString::from("1.0.0"),
        description: RString::from("My awesome TTS provider"),
        author: RString::from("Your Name"),
        capabilities: RVec::from(vec![PluginCapabilityType::TTS]),
        provider_ids: RVec::from(vec![RString::from("my-awesome")]),
    }
}

// Create TTS provider instance
#[sabi_extern_fn]
fn create_tts_provider(config: FFIConfig) -> RResult<TTSProvider, RString> {
    // Parse config JSON
    let config_str = config.json.as_str();
    let parsed: MyConfig = match serde_json::from_str(config_str) {
        Ok(c) => c,
        Err(e) => return ffi_err(format!("Invalid config: {}", e)),
    };

    // Create provider state
    let state = Box::new(MyTTSState::new(parsed));
    let handle = ProviderHandle::from_box(state);

    // Return provider with vtable
    ffi_ok(TTSProvider {
        handle,
        vtable: &MY_TTS_VTABLE,
    })
}

// Define TTS operations
static MY_TTS_VTABLE: TTSVTable = TTSVTable {
    connect,
    speak,
    flush,
    clear,
    disconnect,
    on_audio,
    on_complete,
    on_error,
    set_voice,
    drop_provider,
};

// Implement each vtable function...
```

### Implementing an STT Plugin

Similar structure, but implement `STTProvider` and `STTVTable`:

```rust
use waav_plugin_api::{
    STTProvider, STTVTable, STTResultCallbackFn,
};

#[sabi_extern_fn]
fn create_stt_provider(config: FFIConfig) -> RResult<STTProvider, RString> {
    // Create provider...
    ffi_ok(STTProvider {
        handle,
        vtable: &MY_STT_VTABLE,
    })
}

static MY_STT_VTABLE: STTVTable = STTVTable {
    connect,
    send_audio,
    end_utterance,
    disconnect,
    on_result,
    on_error,
    drop_provider,
};
```

### Best Practices

1. **Error Handling**: Always return descriptive errors via `ffi_err()`
2. **Memory Safety**: Use `ProviderHandle` for state management
3. **Thread Safety**: Use `Mutex`/`RwLock` for shared state
4. **Resource Cleanup**: Implement `drop_provider` properly
5. **Timeout Handling**: Set reasonable timeouts for network operations
6. **Circuit Breaker**: Implement circuit breaker for external APIs
7. **Logging**: Use `tracing` crate for observability

### Testing Your Plugin

```bash
# Build in release mode
cargo build --release

# Run tests
cargo test

# Check for FFI safety issues
cargo clippy -- -D warnings
```

---

## Plugin API Reference

See the [Plugin Architecture Documentation](../gateway/docs/plugins.md) for:
- Complete API reference
- Capability traits
- Registration macros
- Configuration options
- Troubleshooting guide

---

## Contributing

When contributing new examples:

1. Follow the existing project structure
2. Include comprehensive documentation
3. Add error handling and logging
4. Include a `README.md` in the example directory
5. Test with the gateway before submitting

---

## License

Apache-2.0

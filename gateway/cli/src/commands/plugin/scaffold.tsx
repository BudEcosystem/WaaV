/**
 * WaaV CLI Plugin Scaffold Command
 *
 * Generates plugin templates for creating custom WaaV providers.
 * Supports STT, TTS, and Realtime plugin types with Rust boilerplate.
 */

import React, { useState, useCallback } from 'react';
import { render, Text, Box, useInput, useApp } from 'ink';
import * as fs from 'fs';
import * as path from 'path';

import { logger } from '../../utils/logger.js';
import type { GlobalOptions } from '../../cli.js';
import {
  TextInput,
  Select,
  Confirm,
  Alert,
  Spinner,
  Card,
  type SelectItem,
} from '../../components/common/index.js';

/**
 * Plugin types
 */
type PluginType = 'stt' | 'tts' | 'realtime';

/**
 * Plugin mode (cloud API vs local inference)
 */
type PluginMode = 'cloud' | 'local';

/**
 * Plugin configuration
 */
interface PluginConfig {
  name: string;
  description: string;
  type: PluginType;
  mode: PluginMode;
  author: string;
  license: string;
  version: string;
}

/**
 * Template generators
 */
const templates = {
  /**
   * Generate Cargo.toml
   */
  cargoToml: (config: PluginConfig): string => `[package]
name = "waav-plugin-${config.name}"
version = "${config.version}"
edition = "2021"
authors = ["${config.author}"]
description = "${config.description}"
license = "${config.license}"
repository = "https://github.com/YOUR_USERNAME/waav-plugin-${config.name}"
keywords = ["waav", "${config.type}", "plugin", "voice", "audio"]
categories = ["multimedia::audio"]

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
waav-plugin-api = { version = "0.1", features = ["${config.type}"] }
tokio = { version = "1.35", features = ["full"] }
async-trait = "0.1"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "1.0"
tracing = "0.1"

${config.mode === 'cloud' ? `# HTTP client for cloud APIs
reqwest = { version = "0.11", features = ["json", "rustls-tls"] }` : `# Local inference dependencies
# Add your model inference dependencies here`}

[dev-dependencies]
tokio-test = "0.4"

[profile.release]
lto = true
codegen-units = 1
opt-level = 3
strip = true
`,

  /**
   * Generate lib.rs for STT plugin
   */
  sttLib: (config: PluginConfig): string => `//! WaaV ${config.name} STT Plugin
//!
//! ${config.description}

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use waav_plugin_api::{
    stt::{SttPlugin, SttConfig, SttResult, SttError},
    AudioChunk, PluginInfo, PluginCapabilities,
};

/// Plugin error types
#[derive(Error, Debug)]
pub enum ${toPascalCase(config.name)}Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Audio processing error: {0}")]
    AudioProcessing(String),

    ${config.mode === 'cloud' ? `#[error("API error: {0}")]
    Api(#[from] reqwest::Error),` : `#[error("Inference error: {0}")]
    Inference(String),`}

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<${toPascalCase(config.name)}Error> for SttError {
    fn from(err: ${toPascalCase(config.name)}Error) -> Self {
        SttError::PluginError(err.to_string())
    }
}

/// Plugin configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ${toPascalCase(config.name)}Config {
    ${config.mode === 'cloud' ? `/// API key for authentication
    pub api_key: String,

    /// API endpoint URL
    #[serde(default = "default_endpoint")]
    pub endpoint: String,` : `/// Model path for local inference
    pub model_path: String,

    /// Use GPU acceleration
    #[serde(default)]
    pub use_gpu: bool,`}

    /// Language code (e.g., "en-US")
    #[serde(default = "default_language")]
    pub language: String,

    /// Sample rate in Hz
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
}

${config.mode === 'cloud' ? `fn default_endpoint() -> String {
    "https://api.example.com/v1/stt".to_string()
}` : ''}

fn default_language() -> String {
    "en-US".to_string()
}

fn default_sample_rate() -> u32 {
    16000
}

/// ${config.name} STT Plugin
pub struct ${toPascalCase(config.name)}Stt {
    config: ${toPascalCase(config.name)}Config,
    ${config.mode === 'cloud' ? `client: reqwest::Client,` : `// Add model instance here`}
}

impl ${toPascalCase(config.name)}Stt {
    /// Create a new plugin instance
    pub fn new(config: ${toPascalCase(config.name)}Config) -> Result<Self, ${toPascalCase(config.name)}Error> {
        ${config.mode === 'cloud' ? `let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self { config, client })` : `// Initialize local model here
        Ok(Self { config })`}
    }
}

#[async_trait]
impl SttPlugin for ${toPascalCase(config.name)}Stt {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "${config.name}".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: "${config.description}".to_string(),
            author: "${config.author}".to_string(),
            capabilities: PluginCapabilities {
                streaming: ${config.mode === 'cloud'},
                batch: true,
                languages: vec!["en-US".to_string(), "es".to_string(), "fr".to_string()],
            },
        }
    }

    async fn configure(&mut self, config: SttConfig) -> Result<(), SttError> {
        if let Some(language) = config.language {
            self.config.language = language;
        }
        if let Some(sample_rate) = config.sample_rate {
            self.config.sample_rate = sample_rate;
        }
        Ok(())
    }

    async fn transcribe(&self, audio: AudioChunk) -> Result<SttResult, SttError> {
        tracing::debug!(
            "Transcribing {} bytes of audio at {}Hz",
            audio.data.len(),
            audio.sample_rate
        );

        ${config.mode === 'cloud' ? `// Send audio to cloud API
        let response = self.client
            .post(&self.config.endpoint)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "audio/wav")
            .body(audio.data)
            .send()
            .await?;

        // Parse response
        let result: ApiResponse = response.json().await?;

        Ok(SttResult {
            text: result.text,
            confidence: result.confidence,
            is_final: true,
            words: vec![],
        })` : `// Perform local inference
        // TODO: Implement local model inference

        Ok(SttResult {
            text: "TODO: Implement transcription".to_string(),
            confidence: 0.0,
            is_final: true,
            words: vec![],
        })`}
    }

    async fn start_stream(&mut self) -> Result<(), SttError> {
        tracing::info!("Starting ${config.name} STT stream");
        Ok(())
    }

    async fn process_stream_chunk(&mut self, audio: AudioChunk) -> Result<Option<SttResult>, SttError> {
        // Process streaming audio chunk
        // Return intermediate results
        Ok(None)
    }

    async fn end_stream(&mut self) -> Result<SttResult, SttError> {
        tracing::info!("Ending ${config.name} STT stream");
        Ok(SttResult {
            text: String::new(),
            confidence: 0.0,
            is_final: true,
            words: vec![],
        })
    }
}

${config.mode === 'cloud' ? `/// API response structure
#[derive(Debug, Deserialize)]
struct ApiResponse {
    text: String,
    confidence: f32,
}` : ''}

/// Plugin entry point - required for dynamic loading
#[no_mangle]
pub extern "C" fn waav_plugin_create(config_json: *const std::os::raw::c_char) -> *mut ${toPascalCase(config.name)}Stt {
    use std::ffi::CStr;

    let config_str = unsafe {
        if config_json.is_null() {
            return std::ptr::null_mut();
        }
        match CStr::from_ptr(config_json).to_str() {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        }
    };

    let config: ${toPascalCase(config.name)}Config = match serde_json::from_str(config_str) {
        Ok(c) => c,
        Err(_) => return std::ptr::null_mut(),
    };

    match ${toPascalCase(config.name)}Stt::new(config) {
        Ok(plugin) => Box::into_raw(Box::new(plugin)),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn waav_plugin_destroy(plugin: *mut ${toPascalCase(config.name)}Stt) {
    if !plugin.is_null() {
        unsafe {
            drop(Box::from_raw(plugin));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_plugin_info() {
        ${config.mode === 'cloud' ? `let config = ${toPascalCase(config.name)}Config {
            api_key: "test-key".to_string(),
            endpoint: "https://test.api.com".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
        };` : `let config = ${toPascalCase(config.name)}Config {
            model_path: "/tmp/model".to_string(),
            use_gpu: false,
            language: "en-US".to_string(),
            sample_rate: 16000,
        };`}

        let plugin = ${toPascalCase(config.name)}Stt::new(config).unwrap();
        let info = plugin.info();

        assert_eq!(info.name, "${config.name}");
        assert!(!info.description.is_empty());
    }
}
`,

  /**
   * Generate lib.rs for TTS plugin
   */
  ttsLib: (config: PluginConfig): string => `//! WaaV ${config.name} TTS Plugin
//!
//! ${config.description}

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use waav_plugin_api::{
    tts::{TtsPlugin, TtsConfig, TtsError, Voice},
    AudioChunk, PluginInfo, PluginCapabilities,
};

/// Plugin error types
#[derive(Error, Debug)]
pub enum ${toPascalCase(config.name)}Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Synthesis error: {0}")]
    Synthesis(String),

    ${config.mode === 'cloud' ? `#[error("API error: {0}")]
    Api(#[from] reqwest::Error),` : `#[error("Inference error: {0}")]
    Inference(String),`}

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<${toPascalCase(config.name)}Error> for TtsError {
    fn from(err: ${toPascalCase(config.name)}Error) -> Self {
        TtsError::PluginError(err.to_string())
    }
}

/// Plugin configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ${toPascalCase(config.name)}Config {
    ${config.mode === 'cloud' ? `/// API key for authentication
    pub api_key: String,

    /// API endpoint URL
    #[serde(default = "default_endpoint")]
    pub endpoint: String,` : `/// Model path for local inference
    pub model_path: String,

    /// Use GPU acceleration
    #[serde(default)]
    pub use_gpu: bool,`}

    /// Default voice ID
    #[serde(default = "default_voice")]
    pub voice_id: String,

    /// Sample rate in Hz
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
}

${config.mode === 'cloud' ? `fn default_endpoint() -> String {
    "https://api.example.com/v1/tts".to_string()
}` : ''}

fn default_voice() -> String {
    "default".to_string()
}

fn default_sample_rate() -> u32 {
    24000
}

/// ${config.name} TTS Plugin
pub struct ${toPascalCase(config.name)}Tts {
    config: ${toPascalCase(config.name)}Config,
    ${config.mode === 'cloud' ? `client: reqwest::Client,` : ``}
}

impl ${toPascalCase(config.name)}Tts {
    /// Create a new plugin instance
    pub fn new(config: ${toPascalCase(config.name)}Config) -> Result<Self, ${toPascalCase(config.name)}Error> {
        ${config.mode === 'cloud' ? `let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        Ok(Self { config, client })` : `// Initialize local model here
        Ok(Self { config })`}
    }
}

#[async_trait]
impl TtsPlugin for ${toPascalCase(config.name)}Tts {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "${config.name}".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: "${config.description}".to_string(),
            author: "${config.author}".to_string(),
            capabilities: PluginCapabilities {
                streaming: ${config.mode === 'cloud'},
                batch: true,
                languages: vec!["en-US".to_string(), "es".to_string(), "fr".to_string()],
            },
        }
    }

    async fn configure(&mut self, config: TtsConfig) -> Result<(), TtsError> {
        if let Some(voice_id) = config.voice_id {
            self.config.voice_id = voice_id;
        }
        if let Some(sample_rate) = config.sample_rate {
            self.config.sample_rate = sample_rate;
        }
        Ok(())
    }

    async fn list_voices(&self) -> Result<Vec<Voice>, TtsError> {
        // Return available voices
        Ok(vec![
            Voice {
                id: "default".to_string(),
                name: "Default Voice".to_string(),
                language: "en-US".to_string(),
                gender: Some("neutral".to_string()),
                preview_url: None,
            },
        ])
    }

    async fn synthesize(&self, text: &str) -> Result<AudioChunk, TtsError> {
        tracing::debug!("Synthesizing: {}", text);

        ${config.mode === 'cloud' ? `// Send text to cloud API
        let response = self.client
            .post(&self.config.endpoint)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&serde_json::json!({
                "text": text,
                "voice_id": self.config.voice_id,
            }))
            .send()
            .await?;

        let audio_data = response.bytes().await?.to_vec();

        Ok(AudioChunk {
            data: audio_data,
            sample_rate: self.config.sample_rate,
            channels: 1,
            timestamp_ms: 0,
        })` : `// Perform local synthesis
        // TODO: Implement local model synthesis

        Ok(AudioChunk {
            data: vec![],
            sample_rate: self.config.sample_rate,
            channels: 1,
            timestamp_ms: 0,
        })`}
    }

    async fn synthesize_stream(
        &self,
        text: &str,
        callback: Box<dyn FnMut(AudioChunk) + Send>,
    ) -> Result<(), TtsError> {
        // Streaming synthesis
        let audio = self.synthesize(text).await?;
        let mut callback = callback;
        callback(audio);
        Ok(())
    }
}

/// Plugin entry point - required for dynamic loading
#[no_mangle]
pub extern "C" fn waav_plugin_create(config_json: *const std::os::raw::c_char) -> *mut ${toPascalCase(config.name)}Tts {
    use std::ffi::CStr;

    let config_str = unsafe {
        if config_json.is_null() {
            return std::ptr::null_mut();
        }
        match CStr::from_ptr(config_json).to_str() {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        }
    };

    let config: ${toPascalCase(config.name)}Config = match serde_json::from_str(config_str) {
        Ok(c) => c,
        Err(_) => return std::ptr::null_mut(),
    };

    match ${toPascalCase(config.name)}Tts::new(config) {
        Ok(plugin) => Box::into_raw(Box::new(plugin)),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn waav_plugin_destroy(plugin: *mut ${toPascalCase(config.name)}Tts) {
    if !plugin.is_null() {
        unsafe {
            drop(Box::from_raw(plugin));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_plugin_info() {
        ${config.mode === 'cloud' ? `let config = ${toPascalCase(config.name)}Config {
            api_key: "test-key".to_string(),
            endpoint: "https://test.api.com".to_string(),
            voice_id: "default".to_string(),
            sample_rate: 24000,
        };` : `let config = ${toPascalCase(config.name)}Config {
            model_path: "/tmp/model".to_string(),
            use_gpu: false,
            voice_id: "default".to_string(),
            sample_rate: 24000,
        };`}

        let plugin = ${toPascalCase(config.name)}Tts::new(config).unwrap();
        let info = plugin.info();

        assert_eq!(info.name, "${config.name}");
    }

    #[tokio::test]
    async fn test_list_voices() {
        ${config.mode === 'cloud' ? `let config = ${toPascalCase(config.name)}Config {
            api_key: "test-key".to_string(),
            endpoint: "https://test.api.com".to_string(),
            voice_id: "default".to_string(),
            sample_rate: 24000,
        };` : `let config = ${toPascalCase(config.name)}Config {
            model_path: "/tmp/model".to_string(),
            use_gpu: false,
            voice_id: "default".to_string(),
            sample_rate: 24000,
        };`}

        let plugin = ${toPascalCase(config.name)}Tts::new(config).unwrap();
        let voices = plugin.list_voices().await.unwrap();

        assert!(!voices.is_empty());
    }
}
`,

  /**
   * Generate README.md
   */
  readme: (config: PluginConfig): string => `# waav-plugin-${config.name}

${config.description}

## Plugin Type

- **Type**: ${config.type.toUpperCase()}
- **Mode**: ${config.mode === 'cloud' ? 'Cloud API' : 'Local Inference'}

## Installation

\`\`\`bash
cargo build --release
\`\`\`

The compiled plugin will be at \`target/release/libwaav_plugin_${config.name.replace(/-/g, '_')}.so\` (Linux) or \`target/release/libwaav_plugin_${config.name.replace(/-/g, '_')}.dylib\` (macOS).

## Configuration

Add to your WaaV configuration:

\`\`\`yaml
plugins:
  ${config.name}:
    path: /path/to/libwaav_plugin_${config.name.replace(/-/g, '_')}.so
    config:
      ${config.mode === 'cloud' ? `api_key: YOUR_API_KEY
      endpoint: https://api.example.com/v1/${config.type}` : `model_path: /path/to/model
      use_gpu: true`}
      ${config.type === 'stt' ? 'language: en-US' : 'voice_id: default'}
\`\`\`

## Development

\`\`\`bash
# Build
cargo build

# Test
cargo test

# Build release
cargo build --release
\`\`\`

## License

${config.license}
`,

  /**
   * Generate GitHub Actions workflow
   */
  workflow: (config: PluginConfig): string => `name: Build

on:
  push:
    branches: [ main ]
  pull_request:
    branches: [ main ]
  release:
    types: [ created ]

env:
  CARGO_TERM_COLOR: always

jobs:
  build:
    strategy:
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            artifact: libwaav_plugin_${config.name.replace(/-/g, '_')}.so
          - os: macos-latest
            target: x86_64-apple-darwin
            artifact: libwaav_plugin_${config.name.replace(/-/g, '_')}.dylib
          - os: macos-latest
            target: aarch64-apple-darwin
            artifact: libwaav_plugin_${config.name.replace(/-/g, '_')}.dylib
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            artifact: waav_plugin_${config.name.replace(/-/g, '_')}.dll

    runs-on: \${{ matrix.os }}

    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-action@stable
        with:
          targets: \${{ matrix.target }}

      - name: Build
        run: cargo build --release --target \${{ matrix.target }}

      - name: Run tests
        run: cargo test --release --target \${{ matrix.target }}

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: waav-plugin-${config.name}-\${{ matrix.target }}
          path: target/\${{ matrix.target }}/release/\${{ matrix.artifact }}

  release:
    needs: build
    if: github.event_name == 'release'
    runs-on: ubuntu-latest
    steps:
      - name: Download all artifacts
        uses: actions/download-artifact@v4

      - name: Upload to release
        uses: softprops/action-gh-release@v1
        with:
          files: |
            waav-plugin-${config.name}-*/libwaav_plugin_${config.name.replace(/-/g, '_')}.*
            waav-plugin-${config.name}-*/waav_plugin_${config.name.replace(/-/g, '_')}.*
`,

  /**
   * Generate .gitignore
   */
  gitignore: (): string => `/target
Cargo.lock
*.swp
*.swo
.idea/
.vscode/
*.log
`,
};

/**
 * Convert to PascalCase
 */
function toPascalCase(str: string): string {
  return str
    .split(/[-_]/)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
    .join('');
}

/**
 * Plugin type items
 */
const pluginTypeItems: SelectItem[] = [
  { label: '🎤 STT (Speech-to-Text)', value: 'stt' },
  { label: '🔊 TTS (Text-to-Speech)', value: 'tts' },
  { label: '⚡ Realtime (Audio-to-Audio)', value: 'realtime' },
];

/**
 * Plugin mode items
 */
const pluginModeItems: SelectItem[] = [
  { label: '☁️ Cloud API', value: 'cloud', description: 'Provider with external API' },
  { label: '💻 Local Inference', value: 'local', description: 'Run model locally' },
];

/**
 * Interactive scaffold wizard
 */
interface ScaffoldWizardProps {
  options: GlobalOptions;
  initialName?: string;
  initialType?: PluginType;
  initialMode?: PluginMode;
}

const ScaffoldWizard: React.FC<ScaffoldWizardProps> = ({
  options,
  initialName,
  initialType,
  initialMode,
}) => {
  const { exit } = useApp();
  const [step, setStep] = useState<'name' | 'description' | 'type' | 'mode' | 'author' | 'confirm' | 'generating'>('name');
  const [config, setConfig] = useState<Partial<PluginConfig>>({
    name: initialName || '',
    type: initialType,
    mode: initialMode,
    version: '0.1.0',
    license: 'MIT',
  });
  const [error, setError] = useState<string | null>(null);

  const handleNameSubmit = useCallback((name: string) => {
    if (!name.trim()) {
      setError('Plugin name is required');
      return;
    }
    const sanitized = name.toLowerCase().replace(/[^a-z0-9-]/g, '-');
    setConfig((prev) => ({ ...prev, name: sanitized }));
    setError(null);
    setStep('description');
  }, []);

  const handleDescriptionSubmit = useCallback((description: string) => {
    setConfig((prev) => ({ ...prev, description: description || `WaaV ${config.name} plugin` }));
    setStep(config.type ? (config.mode ? 'author' : 'mode') : 'type');
  }, [config.name, config.type, config.mode]);

  const handleTypeSelect = useCallback((item: SelectItem) => {
    setConfig((prev) => ({ ...prev, type: item.value as PluginType }));
    setStep(config.mode ? 'author' : 'mode');
  }, [config.mode]);

  const handleModeSelect = useCallback((item: SelectItem) => {
    setConfig((prev) => ({ ...prev, mode: item.value as PluginMode }));
    setStep('author');
  }, []);

  const handleAuthorSubmit = useCallback((author: string) => {
    setConfig((prev) => ({ ...prev, author: author || 'Unknown Author' }));
    setStep('confirm');
  }, []);

  const handleConfirm = useCallback(async (confirmed: boolean) => {
    if (!confirmed) {
      exit();
      return;
    }

    setStep('generating');

    try {
      const fullConfig: PluginConfig = {
        name: config.name!,
        description: config.description!,
        type: config.type!,
        mode: config.mode!,
        author: config.author!,
        license: config.license || 'MIT',
        version: config.version || '0.1.0',
      };

      // Create directory
      const projectDir = path.join(process.cwd(), `waav-plugin-${fullConfig.name}`);

      if (fs.existsSync(projectDir)) {
        setError(`Directory already exists: ${projectDir}`);
        return;
      }

      fs.mkdirSync(projectDir, { recursive: true });
      fs.mkdirSync(path.join(projectDir, 'src'), { recursive: true });
      fs.mkdirSync(path.join(projectDir, 'tests'), { recursive: true });
      fs.mkdirSync(path.join(projectDir, '.github', 'workflows'), { recursive: true });

      // Generate files
      fs.writeFileSync(
        path.join(projectDir, 'Cargo.toml'),
        templates.cargoToml(fullConfig)
      );

      const libContent = fullConfig.type === 'stt'
        ? templates.sttLib(fullConfig)
        : templates.ttsLib(fullConfig);

      fs.writeFileSync(path.join(projectDir, 'src', 'lib.rs'), libContent);
      fs.writeFileSync(path.join(projectDir, 'README.md'), templates.readme(fullConfig));
      fs.writeFileSync(path.join(projectDir, '.gitignore'), templates.gitignore());
      fs.writeFileSync(
        path.join(projectDir, '.github', 'workflows', 'build.yml'),
        templates.workflow(fullConfig)
      );

      if (options.json) {
        console.log(JSON.stringify({
          success: true,
          path: projectDir,
          config: fullConfig,
        }));
      } else {
        logger.success(`Plugin scaffolded at: ${projectDir}`);
        logger.info(`Next steps:
  cd waav-plugin-${fullConfig.name}
  cargo build
  cargo test`);
      }

      exit();
    } catch (err) {
      setError(`Failed to create plugin: ${err instanceof Error ? err.message : 'Unknown error'}`);
    }
  }, [config, options.json, exit]);

  // Keyboard shortcuts
  useInput((_input, key) => {
    if (key.escape) {
      exit();
    }
  });

  const renderStep = () => {
    switch (step) {
      case 'name':
        return (
          <Box flexDirection="column">
            <Text bold color="cyan">Plugin Name</Text>
            <Text dimColor>Enter a unique name for your plugin (e.g., my-provider)</Text>
            <Box marginTop={1}>
              <TextInput
                value={config.name || ''}
                onChange={(value) => setConfig((prev) => ({ ...prev, name: value }))}
                onSubmit={handleNameSubmit}
                placeholder="my-custom-provider"
              />
            </Box>
          </Box>
        );

      case 'description':
        return (
          <Box flexDirection="column">
            <Text bold color="cyan">Description</Text>
            <Box marginTop={1}>
              <TextInput
                value={config.description || ''}
                onChange={(value) => setConfig((prev) => ({ ...prev, description: value }))}
                onSubmit={handleDescriptionSubmit}
                placeholder="A custom WaaV plugin for..."
              />
            </Box>
          </Box>
        );

      case 'type':
        return (
          <Box flexDirection="column">
            <Text bold color="cyan">Plugin Type</Text>
            <Box marginTop={1}>
              <Select items={pluginTypeItems} onSelect={handleTypeSelect} />
            </Box>
          </Box>
        );

      case 'mode':
        return (
          <Box flexDirection="column">
            <Text bold color="cyan">Plugin Mode</Text>
            <Text dimColor>Is this a cloud API or local inference plugin?</Text>
            <Box marginTop={1}>
              <Select items={pluginModeItems} onSelect={handleModeSelect} />
            </Box>
          </Box>
        );

      case 'author':
        return (
          <Box flexDirection="column">
            <Text bold color="cyan">Author</Text>
            <Box marginTop={1}>
              <TextInput
                value={config.author || ''}
                onChange={(value) => setConfig((prev) => ({ ...prev, author: value }))}
                onSubmit={handleAuthorSubmit}
                placeholder="Your Name <email@example.com>"
              />
            </Box>
          </Box>
        );

      case 'confirm':
        return (
          <Box flexDirection="column">
            <Text bold color="cyan">Review Configuration</Text>
            <Card title="Plugin Settings" variant="info">
              <Box flexDirection="column">
                <Text><Text bold>Name:</Text> waav-plugin-{config.name}</Text>
                <Text><Text bold>Type:</Text> {config.type?.toUpperCase()}</Text>
                <Text><Text bold>Mode:</Text> {config.mode === 'cloud' ? 'Cloud API' : 'Local Inference'}</Text>
                <Text><Text bold>Author:</Text> {config.author}</Text>
                <Text><Text bold>Description:</Text> {config.description}</Text>
              </Box>
            </Card>
            <Box marginTop={1}>
              <Confirm
                message="Create this plugin?"
                onConfirm={(confirmed) => handleConfirm(confirmed)}
              />
            </Box>
          </Box>
        );

      case 'generating':
        return (
          <Box>
            <Spinner />
            <Text> Generating plugin scaffold...</Text>
          </Box>
        );

      default:
        return null;
    }
  };

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box borderStyle="round" paddingX={2} paddingY={1} marginBottom={1}>
        <Text bold color="magenta">🔌 WaaV Plugin Scaffolder</Text>
      </Box>

      {/* Error */}
      {error && (
        <Box marginBottom={1}>
          <Alert type="error" message={error} />
        </Box>
      )}

      {/* Current step */}
      {renderStep()}

      {/* Footer */}
      <Box marginTop={2} borderStyle="single" borderTop paddingTop={1}>
        <Text dimColor>[ESC] Cancel</Text>
      </Box>
    </Box>
  );
};

/**
 * Main command function
 */
export async function pluginScaffoldCommand(
  name?: string,
  options: GlobalOptions & {
    type?: PluginType;
    local?: boolean;
  } = {}
): Promise<void> {
  const pluginType = options.type;
  const pluginMode: PluginMode | undefined = options.local ? 'local' : undefined;

  if (options.json && !name) {
    console.log(JSON.stringify({
      error: 'Plugin name is required in JSON mode',
    }));
    process.exit(1);
    return;
  }

  logger.info('Starting plugin scaffolder...');

  render(
    <ScaffoldWizard
      options={options}
      initialName={name}
      initialType={pluginType}
      initialMode={pluginMode}
    />
  );
}

export default pluginScaffoldCommand;

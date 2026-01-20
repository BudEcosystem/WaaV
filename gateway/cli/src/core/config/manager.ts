/**
 * WaaV CLI Configuration Manager
 *
 * Handles loading, saving, and managing CLI configuration
 */

import Conf from 'conf';
import fs from 'fs';
import path from 'path';
import os from 'os';
import yaml from 'js-yaml';
import {
  WaavConfigSchema,
  DEFAULT_CONFIG,
  CONFIG_FILE_NAME,
  CONFIG_DIR_NAME,
  type WaavConfig,
  type GatewayConfig,
  type AudioConfig,
  type VoiceModeConfig,
  type UIConfig,
  type ProviderConfig,
} from '../../types/config.js';
import { ConfigError, ErrorCode } from '../../utils/errors.js';
import { logger } from '../../utils/logger.js';
import { parseEnvVars, deepMerge, getNestedValue, setNestedValue } from '../../utils/helpers.js';

// Configuration schema version
const SCHEMA_VERSION = 1;

// Configuration store for non-YAML settings
const store = new Conf<{
  lastProfile: string;
  telemetry: boolean;
  firstRun: boolean;
}>({
  projectName: 'waav-cli',
  defaults: {
    lastProfile: 'default',
    telemetry: true,
    firstRun: true,
  },
});

/**
 * Configuration Manager Class
 */
export class ConfigManager {
  private config: WaavConfig;
  private configPath: string;
  private profile: string;
  private dirty = false;

  constructor() {
    this.config = DEFAULT_CONFIG;
    this.configPath = this.getDefaultConfigPath();
    this.profile = 'default';
  }

  /**
   * Get the default configuration file path
   */
  getDefaultConfigPath(): string {
    const home = os.homedir();
    return path.join(home, CONFIG_FILE_NAME);
  }

  /**
   * Get the configuration directory path
   */
  getConfigDir(): string {
    const home = os.homedir();
    return path.join(home, CONFIG_DIR_NAME);
  }

  /**
   * Initialize the configuration system
   */
  async initialize(options: { configPath?: string; profile?: string } = {}): Promise<void> {
    // Set custom config path if provided
    if (options.configPath) {
      this.configPath = path.resolve(options.configPath);
    }

    // Set profile
    this.profile = options.profile ?? store.get('lastProfile');

    // Ensure config directory exists
    const configDir = this.getConfigDir();
    if (!fs.existsSync(configDir)) {
      fs.mkdirSync(configDir, { recursive: true });
      logger.debug(`Created config directory: ${configDir}`);
    }

    // Load configuration
    await this.load();

    // Save last used profile
    store.set('lastProfile', this.profile);
  }

  /**
   * Load configuration from file
   */
  async load(): Promise<WaavConfig> {
    try {
      // Check if config file exists
      if (!fs.existsSync(this.configPath)) {
        logger.debug(`Config file not found: ${this.configPath}`);
        this.config = DEFAULT_CONFIG;
        return this.config;
      }

      // Read and parse YAML
      const content = fs.readFileSync(this.configPath, 'utf-8');
      const rawConfig = yaml.load(content) as Record<string, unknown>;

      // Apply environment variable substitution
      const processedConfig = this.processEnvVars(rawConfig);

      // Validate against schema
      const result = WaavConfigSchema.safeParse(processedConfig);

      if (!result.success) {
        const errors = result.error.errors.map(e => `${e.path.join('.')}: ${e.message}`);
        throw new ConfigError(`Invalid configuration:\n${errors.join('\n')}`, {
          code: ErrorCode.CONFIG_INVALID,
          details: { errors },
        });
      }

      this.config = result.data;

      // Apply profile-specific overrides
      if (this.profile !== 'default' && this.config.profiles[this.profile]) {
        const profileConfig = this.config.profiles[this.profile];
        if (profileConfig) {
          this.config = deepMerge(this.config, profileConfig as unknown as Partial<WaavConfig>);
        }
      }

      logger.debug(`Loaded config from: ${this.configPath}`);
      return this.config;
    } catch (error) {
      if (error instanceof ConfigError) {
        throw error;
      }

      if (error instanceof yaml.YAMLException) {
        throw new ConfigError(`Failed to parse configuration: ${error.message}`, {
          code: ErrorCode.CONFIG_PARSE_ERROR,
          cause: error,
        });
      }

      throw new ConfigError(`Failed to load configuration: ${error instanceof Error ? error.message : String(error)}`, {
        code: ErrorCode.CONFIG_NOT_FOUND,
        cause: error instanceof Error ? error : undefined,
      });
    }
  }

  /**
   * Save configuration to file
   */
  async save(): Promise<void> {
    try {
      // Ensure directory exists
      const dir = path.dirname(this.configPath);
      if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
      }

      // Convert to YAML with nice formatting
      const yamlContent = yaml.dump(this.config, {
        indent: 2,
        lineWidth: 120,
        noRefs: true,
        sortKeys: false,
        quotingType: '"',
      });

      // Add header comment
      const header = `# WaaV CLI Configuration
# Version: ${SCHEMA_VERSION}
# Documentation: https://waav.ai/docs/cli/config
#
# Environment variables can be used with \${VAR_NAME} syntax
# Profiles can override settings for different environments
#
`;

      fs.writeFileSync(this.configPath, header + yamlContent, 'utf-8');
      this.dirty = false;
      logger.debug(`Saved config to: ${this.configPath}`);
    } catch (error) {
      throw new ConfigError(`Failed to save configuration: ${error instanceof Error ? error.message : String(error)}`, {
        code: ErrorCode.CONFIG_WRITE_ERROR,
        cause: error instanceof Error ? error : undefined,
      });
    }
  }

  /**
   * Process environment variable substitution in config
   */
  private processEnvVars(obj: Record<string, unknown>): Record<string, unknown> {
    const process = (value: unknown): unknown => {
      if (typeof value === 'string') {
        return parseEnvVars(value);
      }
      if (Array.isArray(value)) {
        return value.map(process);
      }
      if (value !== null && typeof value === 'object') {
        const result: Record<string, unknown> = {};
        for (const [k, v] of Object.entries(value)) {
          result[k] = process(v);
        }
        return result;
      }
      return value;
    };

    return process(obj) as Record<string, unknown>;
  }

  /**
   * Get the current configuration
   */
  get(): WaavConfig {
    return this.config;
  }

  /**
   * Get a specific configuration value by path
   */
  getValue<T = unknown>(path: string): T | undefined {
    return getNestedValue(this.config as unknown as Record<string, unknown>, path) as T | undefined;
  }

  /**
   * Set a configuration value by path
   */
  setValue(path: string, value: unknown): void {
    setNestedValue(this.config as unknown as Record<string, unknown>, path, value);
    this.dirty = true;
  }

  /**
   * Alias for setValue - set a configuration value by path
   */
  set(path: string, value: unknown): void {
    this.setValue(path, value);
  }

  /**
   * Get gateway configuration
   */
  getGateway(): GatewayConfig {
    return this.config.gateway;
  }

  /**
   * Set gateway URL
   */
  setGatewayUrl(url: string): void {
    this.config.gateway.url = url;
    this.dirty = true;
  }

  /**
   * Get audio configuration
   */
  getAudio(): AudioConfig {
    return this.config.audio;
  }

  /**
   * Get voice mode configuration
   */
  getVoiceMode(): VoiceModeConfig {
    return this.config.voice;
  }

  /**
   * Get UI configuration
   */
  getUI(): UIConfig {
    return this.config.ui;
  }

  /**
   * Get provider configuration
   */
  getProvider(providerId: string): ProviderConfig | undefined {
    return this.config.providers[providerId];
  }

  /**
   * Set provider configuration
   */
  setProvider(providerId: string, config: ProviderConfig): void {
    this.config.providers[providerId] = config;
    this.dirty = true;
  }

  /**
   * Remove provider configuration
   */
  removeProvider(providerId: string): boolean {
    if (this.config.providers[providerId]) {
      delete this.config.providers[providerId];
      this.dirty = true;
      return true;
    }
    return false;
  }

  /**
   * Get all configured providers
   */
  getConfiguredProviders(): string[] {
    return Object.keys(this.config.providers);
  }

  /**
   * Get default STT provider
   */
  getDefaultSTT(): string {
    return this.config.defaults.stt_provider;
  }

  /**
   * Set default STT provider
   */
  setDefaultSTT(provider: string): void {
    this.config.defaults.stt_provider = provider;
    this.dirty = true;
  }

  /**
   * Get default TTS provider
   */
  getDefaultTTS(): string {
    return this.config.defaults.tts_provider;
  }

  /**
   * Set default TTS provider
   */
  setDefaultTTS(provider: string): void {
    this.config.defaults.tts_provider = provider;
    this.dirty = true;
  }

  /**
   * Get current profile name
   */
  getProfile(): string {
    return this.profile;
  }

  /**
   * Switch to a different profile
   */
  async switchProfile(profileName: string): Promise<void> {
    if (profileName !== 'default' && !this.config.profiles[profileName]) {
      throw new ConfigError(`Profile '${profileName}' does not exist`, {
        code: ErrorCode.CONFIG_INVALID,
        suggestion: `Available profiles: ${['default', ...Object.keys(this.config.profiles)].join(', ')}`,
      });
    }

    this.profile = profileName;
    store.set('lastProfile', profileName);
    await this.load(); // Reload with new profile
  }

  /**
   * List all available profiles
   */
  listProfiles(): string[] {
    return ['default', ...Object.keys(this.config.profiles)];
  }

  /**
   * Create a new profile
   */
  createProfile(name: string, config: Partial<WaavConfig> = {}): void {
    if (name === 'default') {
      throw new ConfigError('Cannot create a profile named "default"');
    }
    this.config.profiles[name] = config as Record<string, unknown>;
    this.dirty = true;
  }

  /**
   * Delete a profile
   */
  deleteProfile(name: string): boolean {
    if (name === 'default') {
      throw new ConfigError('Cannot delete the default profile');
    }
    if (this.config.profiles[name]) {
      delete this.config.profiles[name];
      this.dirty = true;
      return true;
    }
    return false;
  }

  /**
   * Check if configuration has unsaved changes
   */
  isDirty(): boolean {
    return this.dirty;
  }

  /**
   * Get the configuration file path
   */
  getConfigPath(): string {
    return this.configPath;
  }

  /**
   * Check if this is the first run
   */
  isFirstRun(): boolean {
    return store.get('firstRun');
  }

  /**
   * Mark first run as complete
   */
  completeFirstRun(): void {
    store.set('firstRun', false);
  }

  /**
   * Reset configuration to defaults
   */
  reset(): void {
    this.config = DEFAULT_CONFIG;
    this.dirty = true;
  }

  /**
   * Validate current configuration
   */
  validate(): { valid: boolean; errors: string[] } {
    const result = WaavConfigSchema.safeParse(this.config);

    if (result.success) {
      return { valid: true, errors: [] };
    }

    const errors = result.error.errors.map(e => `${e.path.join('.')}: ${e.message}`);
    return { valid: false, errors };
  }

  /**
   * Export configuration as YAML string
   */
  export(): string {
    return yaml.dump(this.config, {
      indent: 2,
      lineWidth: 120,
      noRefs: true,
    });
  }

  /**
   * Import configuration from YAML string
   */
  import(yamlContent: string): void {
    const rawConfig = yaml.load(yamlContent) as Record<string, unknown>;
    const result = WaavConfigSchema.safeParse(rawConfig);

    if (!result.success) {
      const errors = result.error.errors.map(e => `${e.path.join('.')}: ${e.message}`);
      throw new ConfigError(`Invalid configuration:\n${errors.join('\n')}`);
    }

    this.config = result.data;
    this.dirty = true;
  }
}

// Singleton instance
export const configManager = new ConfigManager();

// Export for testing
export { store };

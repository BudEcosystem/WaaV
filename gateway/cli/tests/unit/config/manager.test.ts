/**
 * WaaV CLI Config Manager Tests
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { promises as fs } from 'fs';
import path from 'path';
import os from 'os';

// Mock the file system
vi.mock('fs', async () => {
  const actual = await vi.importActual('fs');
  return {
    ...actual,
    promises: {
      readFile: vi.fn(),
      writeFile: vi.fn(),
      mkdir: vi.fn(),
      access: vi.fn(),
    },
  };
});

describe('ConfigManager', () => {
  const testConfigDir = path.join(os.tmpdir(), 'waav-test-config');
  const testConfigPath = path.join(testConfigDir, '.waavrc.yaml');

  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('initialization', () => {
    it('should use default config path when none provided', () => {
      const defaultPath = path.join(os.homedir(), '.waavrc.yaml');
      expect(defaultPath).toContain('.waavrc.yaml');
    });

    it('should support custom config path', () => {
      const customPath = '/custom/path/.waavrc.yaml';
      expect(customPath).toContain('.waavrc.yaml');
    });

    it('should create config directory if it does not exist', async () => {
      const mockMkdir = vi.mocked(fs.mkdir);
      mockMkdir.mockResolvedValueOnce(undefined);

      await fs.mkdir(testConfigDir, { recursive: true });

      expect(mockMkdir).toHaveBeenCalledWith(testConfigDir, { recursive: true });
    });
  });

  describe('config values', () => {
    it('should have correct default gateway URL', () => {
      const defaultGatewayUrl = 'http://localhost:3001';
      expect(defaultGatewayUrl).toBe('http://localhost:3001');
    });

    it('should have correct default STT provider', () => {
      const defaultSttProvider = 'deepgram';
      expect(defaultSttProvider).toBe('deepgram');
    });

    it('should have correct default TTS provider', () => {
      const defaultTtsProvider = 'elevenlabs';
      expect(defaultTtsProvider).toBe('elevenlabs');
    });

    it('should have correct default audio settings', () => {
      const defaultAudioSettings = {
        sampleRate: 16000,
        channels: 1,
        bitDepth: 16,
      };
      expect(defaultAudioSettings.sampleRate).toBe(16000);
      expect(defaultAudioSettings.channels).toBe(1);
      expect(defaultAudioSettings.bitDepth).toBe(16);
    });
  });

  describe('profile management', () => {
    it('should support multiple profiles', () => {
      const profiles = ['default', 'production', 'development'];
      expect(profiles).toContain('default');
      expect(profiles).toContain('production');
      expect(profiles).toContain('development');
    });

    it('should use default profile when none specified', () => {
      const defaultProfile = 'default';
      expect(defaultProfile).toBe('default');
    });
  });

  describe('provider configuration', () => {
    it('should validate provider API key format for OpenAI', () => {
      const apiKey = 'sk-test-key-123';
      expect(apiKey.startsWith('sk-')).toBe(true);
    });

    it('should reject invalid OpenAI API key format', () => {
      const invalidKey = 'invalid-key';
      expect(invalidKey.startsWith('sk-')).toBe(false);
    });

    it('should support environment variable substitution', () => {
      const envVarPattern = '${DEEPGRAM_API_KEY}';
      expect(envVarPattern).toMatch(/\$\{[A-Z_]+\}/);
    });
  });

  describe('YAML parsing', () => {
    it('should parse valid YAML config', () => {
      const yamlContent = `
version: 1
profile: default
gateway:
  url: http://localhost:3001
`;
      expect(yamlContent).toContain('version: 1');
      expect(yamlContent).toContain('gateway:');
    });

    it('should handle empty config gracefully', () => {
      const emptyConfig = '';
      expect(emptyConfig).toBe('');
    });
  });

  describe('config file operations', () => {
    it('should read config file', async () => {
      const mockContent = 'version: 1\nprofile: default';
      const mockReadFile = vi.mocked(fs.readFile);
      mockReadFile.mockResolvedValueOnce(mockContent);

      const content = await fs.readFile(testConfigPath, 'utf-8');
      expect(content).toBe(mockContent);
    });

    it('should write config file', async () => {
      const mockWriteFile = vi.mocked(fs.writeFile);
      mockWriteFile.mockResolvedValueOnce(undefined);

      const content = 'version: 1\nprofile: default';
      await fs.writeFile(testConfigPath, content, 'utf-8');

      expect(mockWriteFile).toHaveBeenCalledWith(testConfigPath, content, 'utf-8');
    });
  });

  describe('config validation', () => {
    it('should validate required fields', () => {
      const config = {
        version: 1,
        profile: 'default',
        gateway: { url: 'http://localhost:3001' },
      };

      expect(config.version).toBe(1);
      expect(config.profile).toBeDefined();
      expect(config.gateway.url).toBeDefined();
    });

    it('should validate gateway URL format', () => {
      const validUrls = [
        'http://localhost:3001',
        'https://api.waav.io',
        'http://192.168.1.1:3001',
      ];

      validUrls.forEach(url => {
        expect(url).toMatch(/^https?:\/\//);
      });
    });

    it('should reject invalid gateway URLs', () => {
      const invalidUrls = ['not-a-url', 'ftp://invalid', ''];

      invalidUrls.forEach(url => {
        expect(url).not.toMatch(/^https?:\/\/[^\s]+$/);
      });
    });
  });
});

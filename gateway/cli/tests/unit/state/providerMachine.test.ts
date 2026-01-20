/**
 * WaaV CLI Provider Machine Tests
 */

import { describe, it, expect } from 'vitest';
import { createActor } from 'xstate';
import {
  providerMachine,
  STT_PROVIDERS,
  TTS_PROVIDERS,
  REALTIME_PROVIDERS,
  getProvidersByType,
  getProviderById,
  getProviderTypeLabel,
  validateApiKeyFormat,
} from '../../../src/core/state/machines/providerMachine.js';

describe('providerMachine', () => {
  describe('initial state', () => {
    it('should start in selectType state', () => {
      const actor = createActor(providerMachine);
      actor.start();

      expect(actor.getSnapshot().value).toBe('selectType');
      actor.stop();
    });

    it('should have correct initial context', () => {
      const actor = createActor(providerMachine);
      actor.start();

      const context = actor.getSnapshot().context;
      expect(context.providerType).toBeNull();
      expect(context.selectedProvider).toBeNull();
      expect(context.apiKey).toBe('');
      expect(context.isValid).toBe(false);

      actor.stop();
    });
  });

  describe('provider selection flow', () => {
    it('should transition to selectProvider on SELECT_TYPE', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });

      expect(actor.getSnapshot().value).toBe('selectProvider');
      expect(actor.getSnapshot().context.providerType).toBe('stt');

      actor.stop();
    });

    it('should transition to configure on SELECT_PROVIDER', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({
        type: 'SELECT_PROVIDER',
        provider: STT_PROVIDERS[0]!,
      });

      expect(actor.getSnapshot().value).toMatchObject({ configure: 'editing' });
      expect(actor.getSnapshot().context.selectedProvider?.id).toBe('deepgram');

      actor.stop();
    });
  });

  describe('configuration', () => {
    it('should update API key', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_API_KEY', apiKey: 'test-api-key' });

      expect(actor.getSnapshot().context.apiKey).toBe('test-api-key');
      expect(actor.getSnapshot().context.isValid).toBe(false);

      actor.stop();
    });

    it('should update model', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_MODEL', model: 'nova-2' });

      expect(actor.getSnapshot().context.model).toBe('nova-2');

      actor.stop();
    });

    it('should update language', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_LANGUAGE', language: 'es-ES' });

      expect(actor.getSnapshot().context.language).toBe('es-ES');

      actor.stop();
    });

    it('should update additional options', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_OPTION', key: 'punctuate', value: true });

      expect(actor.getSnapshot().context.additionalOptions.punctuate).toBe(true);

      actor.stop();
    });
  });

  describe('validation', () => {
    it('should transition to validating state', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_API_KEY', apiKey: 'test-key' });
      actor.send({ type: 'VALIDATE' });

      expect(actor.getSnapshot().value).toMatchObject({ configure: 'validating' });
      expect(actor.getSnapshot().context.isValidating).toBe(true);

      actor.stop();
    });

    it('should set isValid on VALIDATION_SUCCESS', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_API_KEY', apiKey: 'test-key' });
      actor.send({ type: 'VALIDATE' });
      actor.send({ type: 'VALIDATION_SUCCESS' });

      expect(actor.getSnapshot().context.isValid).toBe(true);
      expect(actor.getSnapshot().context.validationMessage).toBeNull();

      actor.stop();
    });

    it('should set validation message on VALIDATION_FAILED', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_API_KEY', apiKey: 'invalid-key' });
      actor.send({ type: 'VALIDATE' });
      actor.send({ type: 'VALIDATION_FAILED', message: 'Invalid API key' });

      expect(actor.getSnapshot().context.isValid).toBe(false);
      expect(actor.getSnapshot().context.validationMessage).toBe('Invalid API key');

      actor.stop();
    });
  });

  describe('testing', () => {
    it('should allow test only when valid', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_API_KEY', apiKey: 'test-key' });
      actor.send({ type: 'VALIDATE' });
      actor.send({ type: 'VALIDATION_SUCCESS' });
      actor.send({ type: 'TEST' });

      expect(actor.getSnapshot().value).toMatchObject({ configure: 'testing' });
      expect(actor.getSnapshot().context.isTesting).toBe(true);

      actor.stop();
    });

    it('should record test result', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_API_KEY', apiKey: 'test-key' });
      actor.send({ type: 'VALIDATE' });
      actor.send({ type: 'VALIDATION_SUCCESS' });
      actor.send({ type: 'TEST' });
      actor.send({ type: 'TEST_SUCCESS', message: 'Connection successful' });

      expect(actor.getSnapshot().context.testResult).toBe('success');
      expect(actor.getSnapshot().context.testMessage).toBe('Connection successful');

      actor.stop();
    });
  });

  describe('saving', () => {
    it('should allow save only when valid', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_API_KEY', apiKey: 'test-key' });
      actor.send({ type: 'VALIDATE' });
      actor.send({ type: 'VALIDATION_SUCCESS' });
      actor.send({ type: 'SAVE' });

      expect(actor.getSnapshot().value).toMatchObject({ configure: 'saving' });

      actor.stop();
    });

    it('should complete on SAVE_SUCCESS', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'SET_API_KEY', apiKey: 'test-key' });
      actor.send({ type: 'VALIDATE' });
      actor.send({ type: 'VALIDATION_SUCCESS' });
      actor.send({ type: 'SAVE' });
      actor.send({ type: 'SAVE_SUCCESS' });

      expect(actor.getSnapshot().value).toBe('complete');
      expect(actor.getSnapshot().context.saved).toBe(true);

      actor.stop();
    });
  });

  describe('navigation', () => {
    it('should go back through states', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'SELECT_PROVIDER', provider: STT_PROVIDERS[0]! });
      actor.send({ type: 'BACK' });

      expect(actor.getSnapshot().value).toBe('selectProvider');

      actor.send({ type: 'BACK' });
      expect(actor.getSnapshot().value).toBe('selectType');

      actor.stop();
    });

    it('should cancel configuration', () => {
      const actor = createActor(providerMachine);
      actor.start();

      actor.send({ type: 'SELECT_TYPE', providerType: 'stt' });
      actor.send({ type: 'CANCEL' });

      expect(actor.getSnapshot().value).toBe('cancelled');

      actor.stop();
    });
  });
});

describe('provider helpers', () => {
  describe('STT_PROVIDERS', () => {
    it('should have expected providers', () => {
      const providerIds = STT_PROVIDERS.map(p => p.id);
      expect(providerIds).toContain('deepgram');
      expect(providerIds).toContain('openai');
      expect(providerIds).toContain('google');
      expect(providerIds).toContain('azure');
      expect(providerIds).toContain('assemblyai');
    });

    it('should have valid provider structure', () => {
      STT_PROVIDERS.forEach(provider => {
        expect(provider.id).toBeDefined();
        expect(provider.name).toBeDefined();
        expect(provider.type).toBe('stt');
        expect(provider.description).toBeDefined();
        expect(Array.isArray(provider.features)).toBe(true);
        expect(typeof provider.requiresApiKey).toBe('boolean');
      });
    });
  });

  describe('TTS_PROVIDERS', () => {
    it('should have expected providers', () => {
      const providerIds = TTS_PROVIDERS.map(p => p.id);
      expect(providerIds).toContain('elevenlabs');
      expect(providerIds).toContain('cartesia');
      expect(providerIds).toContain('openai_tts');
    });

    it('should have valid provider structure', () => {
      TTS_PROVIDERS.forEach(provider => {
        expect(provider.type).toBe('tts');
      });
    });
  });

  describe('REALTIME_PROVIDERS', () => {
    it('should have expected providers', () => {
      const providerIds = REALTIME_PROVIDERS.map(p => p.id);
      expect(providerIds).toContain('openai_realtime');
      expect(providerIds).toContain('hume');
    });

    it('should have valid provider structure', () => {
      REALTIME_PROVIDERS.forEach(provider => {
        expect(provider.type).toBe('realtime');
      });
    });
  });

  describe('getProvidersByType', () => {
    it('should return STT providers', () => {
      const providers = getProvidersByType('stt');
      expect(providers).toBe(STT_PROVIDERS);
    });

    it('should return TTS providers', () => {
      const providers = getProvidersByType('tts');
      expect(providers).toBe(TTS_PROVIDERS);
    });

    it('should return realtime providers', () => {
      const providers = getProvidersByType('realtime');
      expect(providers).toBe(REALTIME_PROVIDERS);
    });
  });

  describe('getProviderById', () => {
    it('should find STT provider by ID', () => {
      const provider = getProviderById('deepgram');
      expect(provider?.name).toBe('Deepgram');
    });

    it('should find TTS provider by ID', () => {
      const provider = getProviderById('elevenlabs');
      expect(provider?.name).toBe('ElevenLabs');
    });

    it('should find realtime provider by ID', () => {
      const provider = getProviderById('openai_realtime');
      expect(provider?.name).toBe('OpenAI Realtime');
    });

    it('should return undefined for unknown ID', () => {
      const provider = getProviderById('unknown');
      expect(provider).toBeUndefined();
    });
  });

  describe('getProviderTypeLabel', () => {
    it('should return correct labels', () => {
      expect(getProviderTypeLabel('stt')).toBe('Speech-to-Text');
      expect(getProviderTypeLabel('tts')).toBe('Text-to-Speech');
      expect(getProviderTypeLabel('realtime')).toBe('Realtime Voice');
    });
  });

  describe('validateApiKeyFormat', () => {
    it('should reject empty API key', () => {
      const result = validateApiKeyFormat('deepgram', '');
      expect(result.valid).toBe(false);
      expect(result.message).toBe('API key is required');
    });

    it('should reject whitespace-only API key', () => {
      const result = validateApiKeyFormat('deepgram', '   ');
      expect(result.valid).toBe(false);
    });

    it('should accept valid API key', () => {
      const result = validateApiKeyFormat('deepgram', 'dg-test-key');
      expect(result.valid).toBe(true);
    });

    it('should validate OpenAI key format', () => {
      const validResult = validateApiKeyFormat('openai', 'sk-test-key-123');
      expect(validResult.valid).toBe(true);

      const invalidResult = validateApiKeyFormat('openai', 'invalid-key');
      expect(invalidResult.valid).toBe(false);
      expect(invalidResult.message).toContain('sk-');
    });
  });
});

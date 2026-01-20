/**
 * WaaV CLI Setup Machine Tests
 */

import { describe, it, expect } from 'vitest';
import { createActor } from 'xstate';
import {
  setupMachine,
  SETUP_STEPS,
  getCurrentStep,
  getSetupProgress,
  isSetupComplete,
  getSetupSummary,
  type SetupMachineContext,
} from '../../../src/core/state/machines/setupMachine.js';

describe('setupMachine', () => {
  describe('initial state', () => {
    it('should start in welcome state', () => {
      const actor = createActor(setupMachine);
      actor.start();

      expect(actor.getSnapshot().value).toBe('welcome');
      actor.stop();
    });

    it('should have correct initial context', () => {
      const actor = createActor(setupMachine);
      actor.start();

      const context = actor.getSnapshot().context;
      expect(context.gatewayUrl).toBe('http://localhost:3001');
      expect(context.sttProvider).toBe('deepgram');
      expect(context.ttsProvider).toBe('elevenlabs');
      expect(context.currentStep).toBe(1);
      expect(context.totalSteps).toBe(6);

      actor.stop();
    });
  });

  describe('setup flow', () => {
    it('should transition from welcome to gateway on START', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });

      expect(actor.getSnapshot().value).toMatchObject({ gateway: 'input' });
      expect(actor.getSnapshot().context.currentStep).toBe(2);

      actor.stop();
    });

    it('should update gateway URL on SET_GATEWAY_URL', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });
      actor.send({ type: 'SET_GATEWAY_URL', url: 'https://api.waav.io' });

      expect(actor.getSnapshot().context.gatewayUrl).toBe('https://api.waav.io');

      actor.stop();
    });

    it('should transition to testing on TEST_GATEWAY', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });
      actor.send({ type: 'TEST_GATEWAY' });

      expect(actor.getSnapshot().value).toMatchObject({ gateway: 'testing' });

      actor.stop();
    });

    it('should proceed to STT provider on GATEWAY_CONNECTED', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });
      actor.send({ type: 'TEST_GATEWAY' });
      actor.send({ type: 'GATEWAY_CONNECTED' });

      expect(actor.getSnapshot().value).toMatchObject({ sttProvider: 'select' });
      expect(actor.getSnapshot().context.gatewayConnected).toBe(true);

      actor.stop();
    });

    it('should return to input on GATEWAY_FAILED', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });
      actor.send({ type: 'TEST_GATEWAY' });
      actor.send({ type: 'GATEWAY_FAILED', error: 'Connection refused' });

      expect(actor.getSnapshot().value).toMatchObject({ gateway: 'input' });
      expect(actor.getSnapshot().context.error).toBe('Connection refused');

      actor.stop();
    });
  });

  describe('STT provider configuration', () => {
    it('should select STT provider', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });
      actor.send({ type: 'SKIP' }); // Skip gateway testing
      actor.send({ type: 'SELECT_STT_PROVIDER', provider: 'openai' });

      expect(actor.getSnapshot().context.sttProvider).toBe('openai');
      expect(actor.getSnapshot().value).toMatchObject({ sttProvider: 'configure' });

      actor.stop();
    });

    it('should set STT API key', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });
      actor.send({ type: 'SKIP' });
      actor.send({ type: 'SELECT_STT_PROVIDER', provider: 'deepgram' });
      actor.send({ type: 'SET_STT_API_KEY', apiKey: 'test-key-123' });

      expect(actor.getSnapshot().context.sttApiKey).toBe('test-key-123');

      actor.stop();
    });

    it('should validate STT configuration', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });
      actor.send({ type: 'SKIP' });
      actor.send({ type: 'SELECT_STT_PROVIDER', provider: 'deepgram' });
      actor.send({ type: 'SET_STT_API_KEY', apiKey: 'test-key' });
      actor.send({ type: 'VALIDATE_STT' });

      expect(actor.getSnapshot().value).toMatchObject({ sttProvider: 'validating' });

      actor.stop();
    });
  });

  describe('TTS provider configuration', () => {
    it('should select TTS provider', () => {
      const actor = createActor(setupMachine);
      actor.start();

      // Navigate to TTS provider selection
      actor.send({ type: 'START' });
      actor.send({ type: 'SKIP' }); // Skip gateway
      actor.send({ type: 'SELECT_STT_PROVIDER', provider: 'deepgram' });
      actor.send({ type: 'SKIP' }); // Skip STT validation
      actor.send({ type: 'SELECT_TTS_PROVIDER', provider: 'cartesia' });

      expect(actor.getSnapshot().context.ttsProvider).toBe('cartesia');

      actor.stop();
    });
  });

  describe('audio configuration', () => {
    it('should select audio devices', () => {
      const actor = createActor(setupMachine);
      actor.start();

      // Navigate to audio configuration
      actor.send({ type: 'START' });
      actor.send({ type: 'SKIP' }); // Skip gateway
      actor.send({ type: 'SELECT_STT_PROVIDER', provider: 'deepgram' });
      actor.send({ type: 'SKIP' }); // Skip STT validation
      actor.send({ type: 'SELECT_TTS_PROVIDER', provider: 'elevenlabs' });
      actor.send({ type: 'SKIP' }); // Skip TTS validation

      actor.send({ type: 'SELECT_AUDIO_INPUT', device: 'mic-1' });
      actor.send({ type: 'SELECT_AUDIO_OUTPUT', device: 'speaker-1' });

      expect(actor.getSnapshot().context.audioInputDevice).toBe('mic-1');
      expect(actor.getSnapshot().context.audioOutputDevice).toBe('speaker-1');

      actor.stop();
    });
  });

  describe('summary and save', () => {
    it('should save configuration', () => {
      const actor = createActor(setupMachine);
      actor.start();

      // Navigate to summary
      actor.send({ type: 'START' });
      actor.send({ type: 'SKIP' }); // Skip gateway
      actor.send({ type: 'SELECT_STT_PROVIDER', provider: 'deepgram' });
      actor.send({ type: 'SKIP' });
      actor.send({ type: 'SELECT_TTS_PROVIDER', provider: 'elevenlabs' });
      actor.send({ type: 'SKIP' });
      actor.send({ type: 'SKIP' }); // Skip audio

      expect(actor.getSnapshot().value).toMatchObject({ summary: 'review' });

      actor.send({ type: 'SAVE_CONFIG' });
      expect(actor.getSnapshot().value).toMatchObject({ summary: 'saving' });

      actor.stop();
    });

    it('should complete setup on CONFIG_SAVED', () => {
      const actor = createActor(setupMachine);
      actor.start();

      // Navigate to summary
      actor.send({ type: 'START' });
      actor.send({ type: 'SKIP' });
      actor.send({ type: 'SELECT_STT_PROVIDER', provider: 'deepgram' });
      actor.send({ type: 'SKIP' });
      actor.send({ type: 'SELECT_TTS_PROVIDER', provider: 'elevenlabs' });
      actor.send({ type: 'SKIP' });
      actor.send({ type: 'SKIP' });
      actor.send({ type: 'SAVE_CONFIG' });
      actor.send({ type: 'CONFIG_SAVED' });

      expect(actor.getSnapshot().value).toBe('complete');
      expect(actor.getSnapshot().context.configSaved).toBe(true);

      actor.stop();
    });
  });

  describe('navigation', () => {
    it('should go back to previous step', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });
      expect(actor.getSnapshot().value).toMatchObject({ gateway: 'input' });

      actor.send({ type: 'BACK' });
      expect(actor.getSnapshot().value).toBe('welcome');

      actor.stop();
    });

    it('should cancel setup', () => {
      const actor = createActor(setupMachine);
      actor.start();

      actor.send({ type: 'START' });
      actor.send({ type: 'CANCEL' });

      expect(actor.getSnapshot().value).toBe('cancelled');

      actor.stop();
    });
  });
});

describe('setup machine helpers', () => {
  describe('SETUP_STEPS', () => {
    it('should have correct number of steps', () => {
      expect(SETUP_STEPS).toHaveLength(6);
    });

    it('should have required step properties', () => {
      SETUP_STEPS.forEach(step => {
        expect(step.id).toBeDefined();
        expect(step.title).toBeDefined();
        expect(step.description).toBeDefined();
        expect(typeof step.optional).toBe('boolean');
      });
    });
  });

  describe('getCurrentStep', () => {
    it('should return correct step info', () => {
      const step1 = getCurrentStep(1);
      expect(step1?.id).toBe('welcome');
      expect(step1?.title).toBe('Welcome');

      const step2 = getCurrentStep(2);
      expect(step2?.id).toBe('gateway');
    });

    it('should return undefined for invalid step', () => {
      expect(getCurrentStep(0)).toBeUndefined();
      expect(getCurrentStep(10)).toBeUndefined();
    });
  });

  describe('getSetupProgress', () => {
    it('should calculate progress percentage', () => {
      const context: SetupMachineContext = {
        gatewayUrl: '',
        gatewayConnected: false,
        sttProvider: '',
        ttsProvider: '',
        sttApiKey: '',
        ttsApiKey: '',
        sttValidated: false,
        ttsValidated: false,
        audioInputDevice: '',
        audioOutputDevice: '',
        audioDevicesTested: false,
        currentStep: 3,
        totalSteps: 6,
        error: null,
        validationErrors: {},
        configSaved: false,
      };

      expect(getSetupProgress(context)).toBe(50);
    });
  });

  describe('isSetupComplete', () => {
    it('should return true when config is saved', () => {
      const context: SetupMachineContext = {
        gatewayUrl: '',
        gatewayConnected: false,
        sttProvider: '',
        ttsProvider: '',
        sttApiKey: '',
        ttsApiKey: '',
        sttValidated: false,
        ttsValidated: false,
        audioInputDevice: '',
        audioOutputDevice: '',
        audioDevicesTested: false,
        currentStep: 6,
        totalSteps: 6,
        error: null,
        validationErrors: {},
        configSaved: true,
      };

      expect(isSetupComplete(context)).toBe(true);
    });
  });

  describe('getSetupSummary', () => {
    it('should generate summary object', () => {
      const context: SetupMachineContext = {
        gatewayUrl: 'http://localhost:3001',
        gatewayConnected: true,
        sttProvider: 'deepgram',
        ttsProvider: 'elevenlabs',
        sttApiKey: 'sk-xxx',
        ttsApiKey: 'el-xxx',
        sttValidated: true,
        ttsValidated: true,
        audioInputDevice: 'default',
        audioOutputDevice: 'default',
        audioDevicesTested: true,
        currentStep: 6,
        totalSteps: 6,
        error: null,
        validationErrors: {},
        configSaved: false,
      };

      const summary = getSetupSummary(context);

      expect(summary['Gateway URL']).toBe('http://localhost:3001');
      expect(summary['Gateway Status']).toBe('Connected');
      expect(summary['STT Provider']).toBe('deepgram');
      expect(summary['STT Status']).toBe('Validated');
      expect(summary['TTS Provider']).toBe('elevenlabs');
      expect(summary['TTS Status']).toBe('Validated');
      expect(summary['Audio Status']).toBe('Tested');
    });
  });
});

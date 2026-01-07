/**
 * Keyboard Shortcuts Tests
 */

import { jest, describe, test, expect, beforeEach, afterEach } from '@jest/globals';
import { KeyboardManager } from '../js/keyboard.js';

describe('KeyboardManager', () => {
  let keyboardManager;
  let mockState;
  let mockHandlers;

  beforeEach(() => {
    // Create mock state
    mockState = {
      connected: false,
      recording: false,
      currentTab: 'home',
      sidebarCollapsed: false,
      setConnected: jest.fn(),
      setRecording: jest.fn(),
      setCurrentTab: jest.fn(),
      setSidebarCollapsed: jest.fn(),
    };

    // Create mock handlers
    mockHandlers = {
      connect: jest.fn(),
      disconnect: jest.fn(),
      startRecording: jest.fn(),
      stopRecording: jest.fn(),
      speak: jest.fn(),
      toggleTheme: jest.fn(),
      toggleSidebar: jest.fn(),
      openCommandPalette: jest.fn(),
      showHelp: jest.fn(),
    };

    keyboardManager = new KeyboardManager(mockState, mockHandlers);
  });

  afterEach(() => {
    keyboardManager.destroy();
  });

  describe('initialization', () => {
    test('should register keyboard event listener', () => {
      const addEventListenerSpy = jest.spyOn(document, 'addEventListener');
      const km = new KeyboardManager(mockState, mockHandlers);

      expect(addEventListenerSpy).toHaveBeenCalledWith(
        'keydown',
        expect.any(Function)
      );

      km.destroy();
      addEventListenerSpy.mockRestore();
    });

    test('should have default shortcuts defined', () => {
      const shortcuts = keyboardManager.getShortcuts();

      expect(shortcuts).toContainEqual(
        expect.objectContaining({ key: 'k', ctrl: true })
      );
      expect(shortcuts).toContainEqual(
        expect.objectContaining({ key: 'Enter', ctrl: true })
      );
    });
  });

  describe('command palette', () => {
    test('should open command palette with Ctrl+K', () => {
      const event = new KeyboardEvent('keydown', {
        key: 'k',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.openCommandPalette).toHaveBeenCalled();
    });

    test('should open command palette with Cmd+K on Mac', () => {
      const event = new KeyboardEvent('keydown', {
        key: 'k',
        metaKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.openCommandPalette).toHaveBeenCalled();
    });
  });

  describe('connection shortcuts', () => {
    test('should connect with Ctrl+Enter when disconnected', () => {
      mockState.connected = false;

      const event = new KeyboardEvent('keydown', {
        key: 'Enter',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.connect).toHaveBeenCalled();
      expect(mockHandlers.disconnect).not.toHaveBeenCalled();
    });

    test('should disconnect with Ctrl+Enter when connected', () => {
      mockState.connected = true;

      const event = new KeyboardEvent('keydown', {
        key: 'Enter',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.disconnect).toHaveBeenCalled();
      expect(mockHandlers.connect).not.toHaveBeenCalled();
    });
  });

  describe('recording shortcuts', () => {
    test('should start recording with Space on STT tab when not recording', () => {
      mockState.currentTab = 'stt';
      mockState.recording = false;
      mockState.connected = true;

      const event = new KeyboardEvent('keydown', {
        key: ' ',
        bubbles: true,
      });

      // Ensure we're not in an input field
      document.body.focus();
      document.dispatchEvent(event);

      expect(mockHandlers.startRecording).toHaveBeenCalled();
    });

    test('should stop recording with Space when recording', () => {
      mockState.currentTab = 'stt';
      mockState.recording = true;
      mockState.connected = true;

      const event = new KeyboardEvent('keydown', {
        key: ' ',
        bubbles: true,
      });

      document.body.focus();
      document.dispatchEvent(event);

      expect(mockHandlers.stopRecording).toHaveBeenCalled();
    });

    test('should not trigger recording when focused on input', () => {
      mockState.currentTab = 'stt';
      mockState.recording = false;
      mockState.connected = true;

      // Create and focus an input element
      const input = document.createElement('input');
      document.body.appendChild(input);
      input.focus();

      const event = new KeyboardEvent('keydown', {
        key: ' ',
        bubbles: true,
      });

      input.dispatchEvent(event);

      expect(mockHandlers.startRecording).not.toHaveBeenCalled();
    });
  });

  describe('TTS shortcuts', () => {
    test('should speak with Ctrl+S on TTS tab', () => {
      mockState.currentTab = 'tts';
      mockState.connected = true;

      const event = new KeyboardEvent('keydown', {
        key: 's',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.speak).toHaveBeenCalled();
    });

    test('should not speak when not on TTS tab', () => {
      mockState.currentTab = 'stt';
      mockState.connected = true;

      const event = new KeyboardEvent('keydown', {
        key: 's',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.speak).not.toHaveBeenCalled();
    });
  });

  describe('tab switching', () => {
    test('should switch to tab 1 (home) with Ctrl+1', () => {
      const event = new KeyboardEvent('keydown', {
        key: '1',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockState.setCurrentTab).toHaveBeenCalledWith('home');
    });

    test('should switch to tab 2 (stt) with Ctrl+2', () => {
      const event = new KeyboardEvent('keydown', {
        key: '2',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockState.setCurrentTab).toHaveBeenCalledWith('stt');
    });

    test('should switch to tab 3 (tts) with Ctrl+3', () => {
      const event = new KeyboardEvent('keydown', {
        key: '3',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockState.setCurrentTab).toHaveBeenCalledWith('tts');
    });

    test('should switch tabs with Ctrl+4 through Ctrl+9', () => {
      const tabMap = {
        '4': 'livekit',
        '5': 'sip',
        '6': 'api',
        '7': 'ws',
        '8': 'audio',
        '9': 'metrics',
      };

      Object.entries(tabMap).forEach(([key, tab]) => {
        const event = new KeyboardEvent('keydown', {
          key,
          ctrlKey: true,
          bubbles: true,
        });

        document.dispatchEvent(event);

        expect(mockState.setCurrentTab).toHaveBeenCalledWith(tab);
      });
    });
  });

  describe('theme toggle', () => {
    test('should toggle theme with Ctrl+D', () => {
      const event = new KeyboardEvent('keydown', {
        key: 'd',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.toggleTheme).toHaveBeenCalled();
    });
  });

  describe('sidebar toggle', () => {
    test('should toggle sidebar with Ctrl+B', () => {
      const event = new KeyboardEvent('keydown', {
        key: 'b',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.toggleSidebar).toHaveBeenCalled();
    });
  });

  describe('help', () => {
    test('should show help with ? key', () => {
      const event = new KeyboardEvent('keydown', {
        key: '?',
        bubbles: true,
      });

      document.body.focus();
      document.dispatchEvent(event);

      expect(mockHandlers.showHelp).toHaveBeenCalled();
    });
  });

  describe('escape key', () => {
    test('should close modals with Escape', () => {
      // Create a mock modal
      const modal = document.createElement('div');
      modal.className = 'modal active';
      modal.setAttribute('data-closeable', 'true');
      document.body.appendChild(modal);

      const event = new KeyboardEvent('keydown', {
        key: 'Escape',
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(modal.classList.contains('active')).toBe(false);
    });
  });

  describe('custom shortcuts', () => {
    test('should allow registering custom shortcuts', () => {
      const customHandler = jest.fn();

      keyboardManager.registerShortcut({
        key: 'x',
        ctrl: true,
        description: 'Custom action',
        handler: customHandler,
      });

      const event = new KeyboardEvent('keydown', {
        key: 'x',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(customHandler).toHaveBeenCalled();
    });

    test('should allow unregistering shortcuts', () => {
      const customHandler = jest.fn();

      keyboardManager.registerShortcut({
        key: 'y',
        ctrl: true,
        handler: customHandler,
      });

      keyboardManager.unregisterShortcut('y', true);

      const event = new KeyboardEvent('keydown', {
        key: 'y',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(customHandler).not.toHaveBeenCalled();
    });
  });

  describe('shortcut list', () => {
    test('should return list of all shortcuts with descriptions', () => {
      const shortcuts = keyboardManager.getShortcuts();

      expect(shortcuts).toBeInstanceOf(Array);
      expect(shortcuts.length).toBeGreaterThan(0);

      shortcuts.forEach((shortcut) => {
        expect(shortcut).toHaveProperty('key');
        expect(shortcut).toHaveProperty('description');
      });
    });
  });

  describe('disabled state', () => {
    test('should not trigger shortcuts when disabled', () => {
      keyboardManager.disable();

      const event = new KeyboardEvent('keydown', {
        key: 'k',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.openCommandPalette).not.toHaveBeenCalled();
    });

    test('should re-enable shortcuts after enable()', () => {
      keyboardManager.disable();
      keyboardManager.enable();

      const event = new KeyboardEvent('keydown', {
        key: 'k',
        ctrlKey: true,
        bubbles: true,
      });

      document.dispatchEvent(event);

      expect(mockHandlers.openCommandPalette).toHaveBeenCalled();
    });
  });
});

/**
 * WaaV CLI Keyboard Context
 *
 * Provides global keyboard shortcut handling with support for
 * screen-specific shortcuts and global overrides.
 */

import React, {
  createContext,
  useState,
  useCallback,
  useMemo,
  type ReactNode,
} from 'react';
import { useInput, useApp } from 'ink';
import type { KeyboardShortcut, ScreenName } from '../types/navigation.js';
import { logger } from '../utils/logger.js';

/**
 * Key event from Ink's useInput
 */
export interface KeyEvent {
  /** The key pressed (single character or special name) */
  input: string;
  /** Whether escape was pressed */
  escape?: boolean;
  /** Whether return/enter was pressed */
  return?: boolean;
  /** Whether up arrow was pressed */
  upArrow?: boolean;
  /** Whether down arrow was pressed */
  downArrow?: boolean;
  /** Whether left arrow was pressed */
  leftArrow?: boolean;
  /** Whether right arrow was pressed */
  rightArrow?: boolean;
  /** Whether tab was pressed */
  tab?: boolean;
  /** Whether backspace was pressed */
  backspace?: boolean;
  /** Whether delete was pressed */
  delete?: boolean;
  /** Whether ctrl is held */
  ctrl?: boolean;
  /** Whether meta/cmd is held */
  meta?: boolean;
  /** Whether shift is held */
  shift?: boolean;
}

/**
 * Keyboard context value interface
 */
export interface KeyboardContextValue {
  /** Register a keyboard shortcut */
  registerShortcut: (shortcut: KeyboardShortcut) => () => void;
  /** Unregister a keyboard shortcut by key */
  unregisterShortcut: (key: string) => void;
  /** Register multiple shortcuts for a screen */
  registerScreenShortcuts: (screen: ScreenName, shortcuts: KeyboardShortcut[]) => () => void;
  /** Get all active shortcuts (for display in footer) */
  getActiveShortcuts: () => KeyboardShortcut[];
  /** Check if a key is blocked (e.g., by modal) */
  isKeyBlocked: (key: string) => boolean;
  /** Block keyboard input (e.g., when modal is open) */
  blockInput: () => () => void;
  /** Last key event (for debugging) */
  lastKeyEvent: KeyEvent | null;
}

/**
 * Default context value
 */
const defaultContextValue: KeyboardContextValue = {
  registerShortcut: () => () => {},
  unregisterShortcut: () => {},
  registerScreenShortcuts: () => () => {},
  getActiveShortcuts: () => [],
  isKeyBlocked: () => false,
  blockInput: () => () => {},
  lastKeyEvent: null,
};

/**
 * Keyboard context
 */
export const KeyboardContext = createContext<KeyboardContextValue>(defaultContextValue);

/**
 * Keyboard provider props
 */
interface KeyboardProviderProps {
  children: ReactNode;
  /** Handler for navigation via escape key */
  onEscapePress?: () => void;
  /** Handler for quit via 'q' key */
  onQuitPress?: () => void;
  /** Handler for help via '?' key */
  onHelpPress?: () => void;
  /** Handler for home via 'h' key */
  onHomePress?: () => void;
}

/**
 * Keyboard provider component
 *
 * Manages global keyboard shortcuts and provides registration API.
 */
export const KeyboardProvider: React.FC<KeyboardProviderProps> = ({
  children,
  onEscapePress,
  onQuitPress,
  onHelpPress,
  onHomePress,
}) => {
  const { exit } = useApp();

  // Registered shortcuts
  const [shortcuts, setShortcuts] = useState<Map<string, KeyboardShortcut>>(new Map());

  // Screen-specific shortcuts (for future use)
  const [, setScreenShortcuts] = useState<Map<ScreenName, KeyboardShortcut[]>>(new Map());

  // Input blocking state (e.g., when modal is open)
  const [inputBlocked, setInputBlocked] = useState(false);

  // Last key event for debugging
  const [lastKeyEvent, setLastKeyEvent] = useState<KeyEvent | null>(null);

  /**
   * Register a keyboard shortcut
   */
  const registerShortcut = useCallback((shortcut: KeyboardShortcut) => {
    setShortcuts(prev => {
      const next = new Map(prev);
      next.set(shortcut.key, shortcut);
      return next;
    });

    // Return unregister function
    return () => {
      setShortcuts(prev => {
        const next = new Map(prev);
        next.delete(shortcut.key);
        return next;
      });
    };
  }, []);

  /**
   * Unregister a shortcut by key
   */
  const unregisterShortcut = useCallback((key: string) => {
    setShortcuts(prev => {
      const next = new Map(prev);
      next.delete(key);
      return next;
    });
  }, []);

  /**
   * Register shortcuts for a specific screen
   */
  const registerScreenShortcuts = useCallback((screen: ScreenName, newShortcuts: KeyboardShortcut[]) => {
    setScreenShortcuts(prev => {
      const next = new Map(prev);
      next.set(screen, newShortcuts);
      return next;
    });

    // Return cleanup function
    return () => {
      setScreenShortcuts(prev => {
        const next = new Map(prev);
        next.delete(screen);
        return next;
      });
    };
  }, []);

  /**
   * Get all currently active shortcuts
   */
  const getActiveShortcuts = useCallback(() => {
    const allShortcuts: KeyboardShortcut[] = [];

    // Add global shortcuts
    shortcuts.forEach(shortcut => {
      if (shortcut.global) {
        allShortcuts.push(shortcut);
      }
    });

    // Add registered shortcuts
    shortcuts.forEach(shortcut => {
      if (!shortcut.global && !allShortcuts.some(s => s.key === shortcut.key)) {
        allShortcuts.push(shortcut);
      }
    });

    return allShortcuts;
  }, [shortcuts]);

  /**
   * Check if a key is blocked
   */
  const isKeyBlocked = useCallback((_key: string) => {
    if (inputBlocked) return true;
    return false;
  }, [inputBlocked]);

  /**
   * Block all keyboard input (returns unblock function)
   */
  const blockInput = useCallback(() => {
    setInputBlocked(true);
    return () => setInputBlocked(false);
  }, []);

  /**
   * Handle keyboard input
   */
  useInput((input, key) => {
    // Record key event
    const keyEvent: KeyEvent = {
      input,
      escape: key.escape,
      return: key.return,
      upArrow: key.upArrow,
      downArrow: key.downArrow,
      leftArrow: key.leftArrow,
      rightArrow: key.rightArrow,
      tab: key.tab,
      backspace: key.backspace,
      delete: key.delete,
      ctrl: key.ctrl,
      meta: key.meta,
      shift: key.shift,
    };
    setLastKeyEvent(keyEvent);

    // Skip if input is blocked
    if (inputBlocked) {
      logger.debug('Key blocked', { input, key });
      return;
    }

    // Build key string for matching
    let keyString = input.toLowerCase();
    if (key.ctrl) keyString = `ctrl+${keyString}`;
    if (key.meta) keyString = `meta+${keyString}`;
    if (key.shift) keyString = `shift+${keyString}`;

    // Handle global shortcuts first
    if (key.escape && onEscapePress) {
      onEscapePress();
      return;
    }

    if ((input === 'q' || input === 'Q') && onQuitPress) {
      onQuitPress();
      return;
    }

    if (input === '?' && onHelpPress) {
      onHelpPress();
      return;
    }

    if ((input === 'h' || input === 'H') && key.ctrl && onHomePress) {
      onHomePress();
      return;
    }

    // Handle Ctrl+C for quit
    if (key.ctrl && input === 'c') {
      logger.debug('Ctrl+C pressed, exiting');
      exit();
      return;
    }

    // Check registered shortcuts
    const shortcut = shortcuts.get(keyString) || shortcuts.get(input);
    if (shortcut) {
      logger.debug(`Shortcut triggered: ${shortcut.key}`, { description: shortcut.description });
      shortcut.handler();
      return;
    }
  }, { isActive: !inputBlocked });

  /**
   * Context value
   */
  const contextValue: KeyboardContextValue = useMemo(
    () => ({
      registerShortcut,
      unregisterShortcut,
      registerScreenShortcuts,
      getActiveShortcuts,
      isKeyBlocked,
      blockInput,
      lastKeyEvent,
    }),
    [
      registerShortcut,
      unregisterShortcut,
      registerScreenShortcuts,
      getActiveShortcuts,
      isKeyBlocked,
      blockInput,
      lastKeyEvent,
    ]
  );

  return (
    <KeyboardContext.Provider value={contextValue}>
      {children}
    </KeyboardContext.Provider>
  );
};

export default KeyboardProvider;

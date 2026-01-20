/**
 * WaaV CLI Keyboard Hook
 *
 * Provides access to keyboard context for registering shortcuts
 * and handling keyboard input.
 */

import { useContext, useEffect } from 'react';
import { KeyboardContext, type KeyboardContextValue } from '../context/KeyboardContext.js';
import type { KeyboardShortcut, ScreenName } from '../types/navigation.js';

/**
 * Hook to access keyboard context
 *
 * Must be used within a KeyboardProvider component.
 *
 * @returns Keyboard context value
 * @throws Error if used outside KeyboardProvider
 *
 * @example
 * ```tsx
 * const { registerShortcut } = useKeyboard();
 *
 * useEffect(() => {
 *   const unregister = registerShortcut({
 *     key: 'r',
 *     description: 'Refresh data',
 *     handler: () => fetchData(),
 *   });
 *
 *   return unregister;
 * }, []);
 * ```
 */
export function useKeyboard(): KeyboardContextValue {
  const context = useContext(KeyboardContext);

  if (!context) {
    throw new Error(
      'useKeyboard must be used within a KeyboardProvider. ' +
      'Ensure your component is wrapped with <KeyboardProvider>.'
    );
  }

  return context;
}

/**
 * Hook to register a keyboard shortcut with automatic cleanup
 *
 * @param shortcut - Shortcut configuration
 * @param deps - Dependencies for the handler (like useEffect deps)
 *
 * @example
 * ```tsx
 * useShortcut({
 *   key: 'r',
 *   description: 'Refresh',
 *   handler: () => refreshData(),
 * }, [refreshData]);
 * ```
 */
export function useShortcut(
  shortcut: KeyboardShortcut,
  deps: React.DependencyList = []
): void {
  const { registerShortcut } = useKeyboard();

  useEffect(() => {
    const unregister = registerShortcut(shortcut);
    return unregister;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [registerShortcut, ...deps]);
}

/**
 * Hook to register multiple shortcuts for a screen
 *
 * @param screen - Screen name for scoping
 * @param shortcuts - Array of shortcuts to register
 * @param deps - Dependencies for handlers
 *
 * @example
 * ```tsx
 * useScreenShortcuts('dashboard', [
 *   { key: '1', description: 'Overview tab', handler: () => setTab('overview') },
 *   { key: '2', description: 'Providers tab', handler: () => setTab('providers') },
 *   { key: 'r', description: 'Refresh', handler: () => refresh() },
 * ], [setTab, refresh]);
 * ```
 */
export function useScreenShortcuts(
  screen: ScreenName,
  shortcuts: KeyboardShortcut[],
  deps: React.DependencyList = []
): void {
  const { registerScreenShortcuts } = useKeyboard();

  useEffect(() => {
    const cleanup = registerScreenShortcuts(screen, shortcuts);
    return cleanup;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [registerScreenShortcuts, screen, ...deps]);
}

/**
 * Hook to get all active shortcuts (for display)
 *
 * @returns Array of active keyboard shortcuts
 */
export function useActiveShortcuts(): KeyboardShortcut[] {
  const { getActiveShortcuts } = useKeyboard();
  return getActiveShortcuts();
}

/**
 * Hook to temporarily block keyboard input
 *
 * Useful when displaying modals or dialogs that should capture all input.
 *
 * @param blocked - Whether input should be blocked
 *
 * @example
 * ```tsx
 * const [modalOpen, setModalOpen] = useState(false);
 * useBlockInput(modalOpen);
 * ```
 */
export function useBlockInput(blocked: boolean): void {
  const { blockInput } = useKeyboard();

  useEffect(() => {
    if (blocked) {
      const unblock = blockInput();
      return unblock;
    }
    return undefined;
  }, [blocked, blockInput]);
}

/**
 * Hook to get last key event (for debugging)
 *
 * @returns Last keyboard event or null
 */
export function useLastKeyEvent() {
  const { lastKeyEvent } = useKeyboard();
  return lastKeyEvent;
}

/**
 * Hook to create a navigation shortcut
 *
 * Convenience hook for common navigation patterns.
 *
 * @param key - Key to bind
 * @param description - Shortcut description
 * @param navigateFn - Navigation function to call
 *
 * @example
 * ```tsx
 * const { navigate } = useNavigation();
 * useNavigationShortcut('p', 'Browse providers', () => navigate('provider_browser'));
 * ```
 */
export function useNavigationShortcut(
  key: string,
  description: string,
  navigateFn: () => void
): void {
  useShortcut({
    key,
    description,
    handler: navigateFn,
  }, [navigateFn]);
}

export default useKeyboard;

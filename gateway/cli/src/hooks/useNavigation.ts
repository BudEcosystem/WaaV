/**
 * WaaV CLI Navigation Hook
 *
 * Provides access to navigation context with type safety and
 * helpful error messages.
 */

import { useContext } from 'react';
import { NavigationContext } from '../context/NavigationContext.js';
import type { NavigationContextValue } from '../types/navigation.js';

/**
 * Hook to access navigation context
 *
 * Must be used within a NavigationProvider component.
 *
 * @returns Navigation context value with navigation methods
 * @throws Error if used outside NavigationProvider
 *
 * @example
 * ```tsx
 * const { navigate, goBack, currentScreen, params } = useNavigation();
 *
 * // Navigate to a new screen
 * navigate('provider_config', { providerId: 'deepgram' });
 *
 * // Go back to previous screen
 * goBack();
 *
 * // Check current screen
 * if (currentScreen === 'main_menu') {
 *   // ...
 * }
 * ```
 */
export function useNavigation(): NavigationContextValue {
  const context = useContext(NavigationContext);

  if (!context) {
    throw new Error(
      'useNavigation must be used within a NavigationProvider. ' +
      'Ensure your component is wrapped with <NavigationProvider>.'
    );
  }

  return context;
}

/**
 * Hook to get current screen name only
 *
 * @returns Current screen name
 */
export function useCurrentScreen() {
  const { currentScreen } = useNavigation();
  return currentScreen;
}

/**
 * Hook to get current screen parameters
 *
 * @returns Current screen parameters
 */
export function useScreenParams() {
  const { params } = useNavigation();
  return params;
}

/**
 * Hook to get breadcrumbs
 *
 * @returns Breadcrumb trail for current navigation path
 */
export function useBreadcrumbs() {
  const { breadcrumbs } = useNavigation();
  return breadcrumbs;
}

/**
 * Hook to check if can go back
 *
 * @returns Whether there's history to go back to
 */
export function useCanGoBack() {
  const { canGoBack } = useNavigation();
  return canGoBack;
}

export default useNavigation;

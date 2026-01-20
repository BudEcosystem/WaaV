/**
 * WaaV CLI Navigation Context
 *
 * Provides SPA-style navigation state management with history stack,
 * breadcrumbs, and navigation methods for all screen components.
 */

import React, { createContext, useState, useCallback, useMemo, type ReactNode } from 'react';
import {
  type ScreenName,
  type ScreenParams,
  type ScreenHistoryEntry,
  type BreadcrumbItem,
  type NavigationContextValue,
  buildBreadcrumbs,
} from '../types/navigation.js';
import { logger } from '../utils/logger.js';

/**
 * Maximum history stack size to prevent memory issues
 */
const MAX_HISTORY_SIZE = 50;

/**
 * Navigation context - must be accessed through useNavigation hook
 */
export const NavigationContext = createContext<NavigationContextValue | null>(null);

/**
 * Navigation provider props
 */
interface NavigationProviderProps {
  children: ReactNode;
  /** Initial screen to display */
  initialScreen?: ScreenName;
  /** Initial parameters */
  initialParams?: ScreenParams;
}

/**
 * Navigation provider component
 *
 * Wraps the application and provides navigation state to all children.
 */
export const NavigationProvider: React.FC<NavigationProviderProps> = ({
  children,
  initialScreen = 'main_menu',
  initialParams = {},
}) => {
  // Current screen state
  const [currentScreen, setCurrentScreen] = useState<ScreenName>(initialScreen);
  const [params, setParams] = useState<ScreenParams>(initialParams);

  // Navigation history stack
  const [history, setHistory] = useState<ScreenHistoryEntry[]>([]);

  /**
   * Navigate to a new screen, pushing current screen to history
   */
  const navigate = useCallback((screen: ScreenName, newParams: ScreenParams = {}) => {
    logger.debug(`Navigating to ${screen}`, { params: newParams });

    // Push current screen to history before navigating
    setHistory(prev => {
      const entry: ScreenHistoryEntry = {
        screen: currentScreen,
        params,
        timestamp: Date.now(),
      };

      // Limit history size
      const newHistory = [...prev, entry];
      if (newHistory.length > MAX_HISTORY_SIZE) {
        return newHistory.slice(-MAX_HISTORY_SIZE);
      }
      return newHistory;
    });

    setCurrentScreen(screen);
    setParams(newParams);
  }, [currentScreen, params]);

  /**
   * Replace current screen without pushing to history
   */
  const replace = useCallback((screen: ScreenName, newParams: ScreenParams = {}) => {
    logger.debug(`Replacing screen with ${screen}`, { params: newParams });
    setCurrentScreen(screen);
    setParams(newParams);
  }, []);

  /**
   * Go back to the previous screen in history
   */
  const goBack = useCallback(() => {
    if (history.length === 0) {
      logger.debug('No history to go back to, staying on current screen');
      return;
    }

    const prevEntry = history[history.length - 1];
    if (!prevEntry) return;

    logger.debug(`Going back to ${prevEntry.screen}`);

    setHistory(prev => prev.slice(0, -1));
    setCurrentScreen(prevEntry.screen);
    setParams(prevEntry.params);
  }, [history]);

  /**
   * Go directly to main menu, clearing history
   */
  const goHome = useCallback(() => {
    logger.debug('Going home to main_menu');
    setHistory([]);
    setCurrentScreen('main_menu');
    setParams({});
  }, []);

  /**
   * Whether there's history to go back to
   */
  const canGoBack = history.length > 0;

  /**
   * Build breadcrumbs for current navigation path
   */
  const breadcrumbs: BreadcrumbItem[] = useMemo(
    () => buildBreadcrumbs(currentScreen, params),
    [currentScreen, params]
  );

  /**
   * Context value - memoized to prevent unnecessary re-renders
   */
  const contextValue: NavigationContextValue = useMemo(
    () => ({
      currentScreen,
      params,
      history,
      navigate,
      replace,
      goBack,
      goHome,
      canGoBack,
      breadcrumbs,
    }),
    [currentScreen, params, history, navigate, replace, goBack, goHome, canGoBack, breadcrumbs]
  );

  return (
    <NavigationContext.Provider value={contextValue}>
      {children}
    </NavigationContext.Provider>
  );
};

export default NavigationProvider;

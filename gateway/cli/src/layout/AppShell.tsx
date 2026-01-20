/**
 * WaaV CLI Application Shell
 *
 * Root layout component that provides all context providers,
 * header/footer, and screen rendering for the SPA architecture.
 */

import React, { useCallback } from 'react';
import { Box, useApp } from 'ink';
import { AppProvider } from '../context/AppContext.js';
import { NavigationProvider } from '../context/NavigationContext.js';
import { KeyboardProvider } from '../context/KeyboardContext.js';
import { Header, CompactHeader } from './Header.js';
import { Footer, CompactFooter } from './Footer.js';
import { ScreenRenderer, type ScreenRegistry } from './ScreenRenderer.js';
import { useNavigation } from '../hooks/useNavigation.js';
import type { GlobalOptions } from '../cli.js';
import type { ScreenName, KeyboardShortcut } from '../types/navigation.js';

/**
 * AppShell props
 */
export interface AppShellProps {
  /** Initial screen to display */
  initialScreen?: ScreenName;
  /** Global CLI options */
  options?: GlobalOptions;
  /** Whether to show splash screen first */
  showSplash?: boolean;
  /** CLI version */
  version?: string;
  /** Screen registry mapping names to components */
  screenRegistry: Partial<ScreenRegistry>;
  /** Whether to show header */
  showHeader?: boolean;
  /** Whether to show footer */
  showFooter?: boolean;
  /** Whether to use compact layout */
  compact?: boolean;
  /** Additional screen-specific shortcuts */
  screenShortcuts?: KeyboardShortcut[];
  /** Gateway status refresh interval in ms */
  gatewayRefreshInterval?: number;
}

/**
 * Inner shell component (uses contexts)
 */
const AppShellInner: React.FC<{
  screenRegistry: Partial<ScreenRegistry>;
  showHeader: boolean;
  showFooter: boolean;
  compact: boolean;
  screenShortcuts?: KeyboardShortcut[];
}> = ({
  screenRegistry,
  showHeader,
  showFooter,
  compact,
  screenShortcuts,
}) => {
  const { goBack, goHome, canGoBack } = useNavigation();

  // Handle screen completion
  const handleScreenComplete = useCallback((_result?: unknown) => {
    // Default behavior: go back or go home
    if (canGoBack) {
      goBack();
    } else {
      goHome();
    }
  }, [canGoBack, goBack, goHome]);

  // Handle screen error
  const handleScreenError = useCallback((error: Error) => {
    console.error('Screen error:', error);
    // Could show error modal or navigate to error screen
  }, []);

  // Determine minimum height based on terminal size
  const minHeight = process.stdout.rows ? Math.max(process.stdout.rows - 2, 10) : 24;

  return (
    <Box
      flexDirection="column"
      minHeight={minHeight}
    >
      {/* Header */}
      {showHeader && (
        compact ? (
          <CompactHeader />
        ) : (
          <Header
            showBreadcrumbs={true}
            showGatewayStatus={true}
            showVersion={true}
          />
        )
      )}

      {/* Main content area */}
      <Box flexDirection="column" flexGrow={1} paddingX={1}>
        <ScreenRenderer
          registry={screenRegistry}
          onScreenComplete={handleScreenComplete}
          onScreenError={handleScreenError}
        />
      </Box>

      {/* Footer */}
      {showFooter && (
        compact ? (
          <CompactFooter />
        ) : (
          <Footer
            shortcuts={screenShortcuts}
            showTimestamp={true}
            showGlobalShortcuts={true}
          />
        )
      )}
    </Box>
  );
};

/**
 * Keyboard handler component (uses navigation context)
 */
const KeyboardHandler: React.FC<{
  children: React.ReactNode;
}> = ({ children }) => {
  const { exit } = useApp();
  const { goBack, goHome, navigate, canGoBack } = useNavigation();

  // Handle escape - go back
  const handleEscape = useCallback(() => {
    if (canGoBack) {
      goBack();
    } else {
      goHome();
    }
  }, [canGoBack, goBack, goHome]);

  // Handle quit
  const handleQuit = useCallback(() => {
    exit();
  }, [exit]);

  // Handle help
  const handleHelp = useCallback(() => {
    navigate('help');
  }, [navigate]);

  // Handle home
  const handleHome = useCallback(() => {
    goHome();
  }, [goHome]);

  return (
    <KeyboardProvider
      onEscapePress={handleEscape}
      onQuitPress={handleQuit}
      onHelpPress={handleHelp}
      onHomePress={handleHome}
    >
      {children}
    </KeyboardProvider>
  );
};

/**
 * Application shell component
 *
 * Wraps the entire application with all necessary providers and layout components.
 * This is the main entry point for the SPA architecture.
 *
 * @example
 * ```tsx
 * <AppShell
 *   initialScreen="main_menu"
 *   screenRegistry={SCREEN_REGISTRY}
 *   options={globalOptions}
 * />
 * ```
 */
export const AppShell: React.FC<AppShellProps> = ({
  initialScreen = 'main_menu',
  options = {},
  showSplash = false,
  version = '1.0.0',
  screenRegistry,
  showHeader = true,
  showFooter = true,
  compact = false,
  screenShortcuts,
  gatewayRefreshInterval = 5000,
}) => {
  // Determine actual initial screen
  const actualInitialScreen = showSplash ? 'splash' : initialScreen;

  return (
    <AppProvider
      options={options}
      version={version}
      gatewayRefreshInterval={gatewayRefreshInterval}
    >
      <NavigationProvider initialScreen={actualInitialScreen}>
        <KeyboardHandler>
          <AppShellInner
            screenRegistry={screenRegistry}
            showHeader={showHeader}
            showFooter={showFooter}
            compact={compact}
            screenShortcuts={screenShortcuts}
          />
        </KeyboardHandler>
      </NavigationProvider>
    </AppProvider>
  );
};

/**
 * Minimal shell for testing or embedded use
 */
export const MinimalShell: React.FC<{
  children: React.ReactNode;
  options?: GlobalOptions;
  version?: string;
}> = ({ children, options = {}, version = '1.0.0' }) => {
  return (
    <AppProvider options={options} version={version} gatewayRefreshInterval={0}>
      <NavigationProvider initialScreen="main_menu">
        {children}
      </NavigationProvider>
    </AppProvider>
  );
};

export default AppShell;

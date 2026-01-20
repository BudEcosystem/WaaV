/**
 * WaaV CLI Screen Renderer Component
 *
 * Dynamically renders the current screen based on navigation state.
 * Uses React.lazy for code splitting and Suspense for loading states.
 */

import React, { Suspense, useMemo } from 'react';
import { Text, Box } from 'ink';
import { useNavigation, useCurrentScreen } from '../hooks/useNavigation.js';
import { Spinner } from '../components/common/index.js';
import type { ScreenName, ScreenProps } from '../types/navigation.js';

/**
 * Loading fallback component
 */
const LoadingScreen: React.FC<{ screenName?: ScreenName }> = ({ screenName }) => {
  return (
    <Box flexDirection="column" alignItems="center" justifyContent="center" paddingY={2}>
      <Spinner label={`Loading ${screenName?.replace(/_/g, ' ') || 'screen'}...`} />
    </Box>
  );
};

/**
 * Error fallback component
 */
const ErrorScreen: React.FC<{ error: Error; screenName?: ScreenName }> = ({ error, screenName }) => {
  return (
    <Box flexDirection="column" paddingY={1}>
      <Text color="red" bold>Failed to load screen</Text>
      {screenName && (
        <Text dimColor>Screen: {screenName}</Text>
      )}
      <Text color="red">{error.message}</Text>
      <Box marginTop={1}>
        <Text dimColor>Press [ESC] to go back</Text>
      </Box>
    </Box>
  );
};

/**
 * Not found fallback component
 */
const NotFoundScreen: React.FC<{ screenName: ScreenName }> = ({ screenName }) => {
  const { canGoBack } = useNavigation();

  return (
    <Box flexDirection="column" paddingY={1}>
      <Text color="yellow" bold>Screen not found</Text>
      <Text dimColor>The screen "{screenName}" is not yet implemented.</Text>
      <Box marginTop={1}>
        <Text dimColor>
          Press [ESC] to {canGoBack ? 'go back' : 'return to main menu'}
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Screen registry type for lazy loading
 * Maps screen names directly to lazy-loaded or regular React components
 */
export type ScreenRegistry = Record<ScreenName, React.LazyExoticComponent<React.FC<ScreenProps>> | React.FC<ScreenProps>>;

/**
 * Screen renderer props
 */
interface ScreenRendererProps {
  /** Screen registry mapping names to components */
  registry: Partial<ScreenRegistry>;
  /** Fallback for loading state */
  loadingFallback?: React.ReactNode;
  /** Callback when screen completes */
  onScreenComplete?: (result?: unknown) => void;
  /** Callback when screen errors */
  onScreenError?: (error: Error) => void;
}

/**
 * Error boundary for screen rendering
 */
class ScreenErrorBoundary extends React.Component<
  { children: React.ReactNode; screenName?: ScreenName; onError?: (error: Error) => void },
  { hasError: boolean; error: Error | null }
> {
  constructor(props: { children: React.ReactNode; screenName?: ScreenName; onError?: (error: Error) => void }) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error) {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: React.ErrorInfo) {
    console.error('Screen error:', error, errorInfo);
    this.props.onError?.(error);
  }

  render() {
    if (this.state.hasError && this.state.error) {
      return <ErrorScreen error={this.state.error} screenName={this.props.screenName} />;
    }

    return this.props.children;
  }
}

/**
 * Screen renderer component
 *
 * Renders the current screen from the navigation context using the provided registry.
 */
export const ScreenRenderer: React.FC<ScreenRendererProps> = ({
  registry,
  loadingFallback,
  onScreenComplete,
  onScreenError,
}) => {
  const currentScreen = useCurrentScreen();

  // Get the screen component from registry
  const ScreenComponent = useMemo(() => {
    return registry[currentScreen];
  }, [registry, currentScreen]);

  // Screen props to pass down
  const screenProps: ScreenProps = useMemo(
    () => ({
      onComplete: onScreenComplete,
      onError: onScreenError,
    }),
    [onScreenComplete, onScreenError]
  );

  // Handle missing screen
  if (!ScreenComponent) {
    return <NotFoundScreen screenName={currentScreen} />;
  }

  // Render the screen with error boundary and suspense
  return (
    <ScreenErrorBoundary screenName={currentScreen} onError={onScreenError}>
      <Suspense fallback={loadingFallback || <LoadingScreen screenName={currentScreen} />}>
        <ScreenComponent {...screenProps} />
      </Suspense>
    </ScreenErrorBoundary>
  );
};

/**
 * Simple screen wrapper for inline screen components
 */
export const ScreenWrapper: React.FC<{
  children: React.ReactNode;
  padding?: number;
}> = ({ children, padding = 1 }) => {
  return (
    <Box flexDirection="column" flexGrow={1} padding={padding}>
      {children}
    </Box>
  );
};

export default ScreenRenderer;

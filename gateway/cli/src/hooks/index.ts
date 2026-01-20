/**
 * WaaV CLI Hooks Module
 *
 * Re-exports all hooks for the SPA architecture.
 */

// Navigation hooks
export {
  useNavigation,
  useCurrentScreen,
  useScreenParams,
  useBreadcrumbs,
  useCanGoBack,
} from './useNavigation.js';

// App state hooks
export {
  useAppState,
  useConfig,
  useConfigLoading,
  useGatewayStatus,
  useGatewayClient,
  useCliOptions,
  useVersion,
  useConfiguredProviders,
} from './useAppState.js';

// Keyboard hooks
export {
  useKeyboard,
  useShortcut,
  useScreenShortcuts,
  useActiveShortcuts,
  useBlockInput,
  useLastKeyEvent,
  useNavigationShortcut,
} from './useKeyboard.js';

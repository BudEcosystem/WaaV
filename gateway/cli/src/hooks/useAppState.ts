/**
 * WaaV CLI App State Hook
 *
 * Provides access to application context including configuration,
 * gateway status, and shared services.
 */

import { useContext, useMemo } from 'react';
import { AppContext, type AppContextValue } from '../context/AppContext.js';
import type { WaavConfig, GatewayStatus } from '../types/index.js';

/**
 * Hook to access full app context
 *
 * Must be used within an AppProvider component.
 *
 * @returns Full app context value
 * @throws Error if used outside AppProvider
 *
 * @example
 * ```tsx
 * const { config, gatewayStatus, gatewayClient } = useAppState();
 *
 * // Access configuration
 * console.log(config.gateway.url);
 *
 * // Check gateway status
 * if (gatewayStatus?.healthy) {
 *   // Gateway is online
 * }
 * ```
 */
export function useAppState(): AppContextValue {
  const context = useContext(AppContext);

  if (!context) {
    throw new Error(
      'useAppState must be used within an AppProvider. ' +
      'Ensure your component is wrapped with <AppProvider>.'
    );
  }

  return context;
}

/**
 * Hook to access only the configuration
 *
 * @returns Current configuration
 */
export function useConfig(): WaavConfig {
  const { config } = useAppState();
  return config;
}

/**
 * Hook to check if config has loaded
 *
 * @returns Loading state and any error
 */
export function useConfigLoading(): { loaded: boolean; error: string | null } {
  const { configLoaded, configError } = useAppState();
  return { loaded: configLoaded, error: configError };
}

/**
 * Hook to access gateway status
 *
 * @returns Gateway status and connection state
 */
export function useGatewayStatus(): {
  status: GatewayStatus | null;
  isConnected: boolean;
  isConnecting: boolean;
  hasError: boolean;
  refresh: () => Promise<void>;
} {
  const { gatewayStatus, gatewayConnectionState, refreshGatewayStatus } = useAppState();

  return useMemo(() => ({
    status: gatewayStatus,
    isConnected: gatewayConnectionState === 'connected',
    isConnecting: gatewayConnectionState === 'connecting',
    hasError: gatewayConnectionState === 'error',
    refresh: refreshGatewayStatus,
  }), [gatewayStatus, gatewayConnectionState, refreshGatewayStatus]);
}

/**
 * Hook to access gateway client
 *
 * @returns Gateway client instance or null if not initialized
 */
export function useGatewayClient() {
  const { gatewayClient } = useAppState();
  return gatewayClient;
}

/**
 * Hook to access CLI options
 *
 * @returns Global CLI options
 */
export function useCliOptions() {
  const { options } = useAppState();
  return options;
}

/**
 * Hook to access CLI version
 *
 * @returns CLI version string
 */
export function useVersion() {
  const { version } = useAppState();
  return version;
}

/**
 * Hook to get configured providers
 *
 * @returns Object with configured provider info
 */
export function useConfiguredProviders(): {
  stt: string | undefined;
  tts: string | undefined;
  realtime: string | undefined;
  count: number;
} {
  const { config } = useAppState();

  return useMemo(() => ({
    stt: config.defaults?.stt_provider,
    tts: config.defaults?.tts_provider,
    realtime: config.defaults?.realtime_provider,
    count: Object.keys(config.providers || {}).length,
  }), [config.defaults, config.providers]);
}

export default useAppState;

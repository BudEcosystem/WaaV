/**
 * WaaV CLI Application Context
 *
 * Provides application-wide state including configuration,
 * gateway status, and shared services to all screen components.
 */

import React, {
  createContext,
  useState,
  useEffect,
  useCallback,
  useMemo,
  type ReactNode,
} from 'react';
import { configManager } from '../core/config/manager.js';
import { GatewayClient } from '../core/gateway/client.js';
import { logger } from '../utils/logger.js';
import type { WaavConfig, GatewayStatus } from '../types/index.js';
import type { GlobalOptions } from '../cli.js';

/**
 * Gateway connection state
 */
export type GatewayConnectionState = 'connecting' | 'connected' | 'disconnected' | 'error';

/**
 * App context value interface
 */
export interface AppContextValue {
  /** Current configuration */
  config: WaavConfig;
  /** Whether config has been loaded */
  configLoaded: boolean;
  /** Config error if loading failed */
  configError: string | null;
  /** Reload configuration from disk */
  reloadConfig: () => Promise<void>;
  /** Update a config value */
  updateConfig: <K extends keyof WaavConfig>(key: K, value: WaavConfig[K]) => void;
  /** Gateway status from health check */
  gatewayStatus: GatewayStatus | null;
  /** Gateway connection state */
  gatewayConnectionState: GatewayConnectionState;
  /** Manually refresh gateway status */
  refreshGatewayStatus: () => Promise<void>;
  /** Gateway client instance */
  gatewayClient: GatewayClient | null;
  /** Global CLI options */
  options: GlobalOptions;
  /** CLI version */
  version: string;
}

/**
 * Default context value (used before provider mounts)
 */
const defaultContextValue: AppContextValue = {
  config: {} as WaavConfig,
  configLoaded: false,
  configError: null,
  reloadConfig: async () => {},
  updateConfig: () => {},
  gatewayStatus: null,
  gatewayConnectionState: 'disconnected',
  refreshGatewayStatus: async () => {},
  gatewayClient: null,
  options: {},
  version: '1.0.0',
};

/**
 * Application context
 */
export const AppContext = createContext<AppContextValue>(defaultContextValue);

/**
 * App provider props
 */
interface AppProviderProps {
  children: ReactNode;
  /** Global CLI options */
  options?: GlobalOptions;
  /** CLI version */
  version?: string;
  /** Auto-refresh gateway status interval in ms (0 to disable) */
  gatewayRefreshInterval?: number;
}

/**
 * Gateway status refresh interval (default 5 seconds)
 */
const DEFAULT_GATEWAY_REFRESH_INTERVAL = 5000;

/**
 * Gateway status request timeout
 */
const GATEWAY_STATUS_TIMEOUT = 3000;

/**
 * App provider component
 *
 * Manages application state and provides it to all children.
 */
export const AppProvider: React.FC<AppProviderProps> = ({
  children,
  options = {},
  version = '1.0.0',
  gatewayRefreshInterval = DEFAULT_GATEWAY_REFRESH_INTERVAL,
}) => {
  // Configuration state
  const [config, setConfig] = useState<WaavConfig>({} as WaavConfig);
  const [configLoaded, setConfigLoaded] = useState(false);
  const [configError, setConfigError] = useState<string | null>(null);

  // Gateway state
  const [gatewayStatus, setGatewayStatus] = useState<GatewayStatus | null>(null);
  const [gatewayConnectionState, setGatewayConnectionState] = useState<GatewayConnectionState>('disconnected');
  const [gatewayClient, setGatewayClient] = useState<GatewayClient | null>(null);

  /**
   * Initialize configuration on mount
   */
  useEffect(() => {
    const initConfig = async () => {
      try {
        await configManager.initialize({
          configPath: options.config,
          profile: options.profile,
        });

        // Apply gateway URL override if provided
        if (options.gateway) {
          configManager.setGatewayUrl(options.gateway);
        }

        // Configure logger based on options
        logger.configure({
          level: options.verbose ? 'debug' : 'info',
          noColor: options.noColor ?? false,
          quiet: options.quiet ?? false,
        });

        const loadedConfig = configManager.get();
        setConfig(loadedConfig);
        setConfigLoaded(true);
        setConfigError(null);

        // Create gateway client
        const client = new GatewayClient({
          baseUrl: options.gateway ?? loadedConfig.gateway.url,
          apiKey: loadedConfig.gateway.auth?.api_key,
          timeout: GATEWAY_STATUS_TIMEOUT,
        });
        setGatewayClient(client);

        logger.debug('Configuration loaded successfully');
      } catch (err) {
        const errorMessage = err instanceof Error ? err.message : String(err);
        logger.error('Failed to load configuration', { error: errorMessage });
        setConfigError(errorMessage);
        setConfigLoaded(true);
      }
    };

    initConfig();
  }, [options.config, options.profile, options.gateway, options.verbose, options.noColor, options.quiet]);

  /**
   * Reload configuration from disk
   */
  const reloadConfig = useCallback(async () => {
    try {
      setConfigError(null);
      await configManager.load();
      const reloadedConfig = configManager.get();
      setConfig(reloadedConfig);
      logger.info('Configuration reloaded');
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : String(err);
      logger.error('Failed to reload configuration', { error: errorMessage });
      setConfigError(errorMessage);
    }
  }, []);

  /**
   * Update a configuration value
   */
  const updateConfig = useCallback(<K extends keyof WaavConfig>(key: K, value: WaavConfig[K]) => {
    setConfig(prev => {
      const updated = { ...prev, [key]: value };
      // Also update in config manager
      try {
        configManager.set(key as string, value as Record<string, unknown>);
      } catch (err) {
        logger.error('Failed to persist config update', { key, error: err });
      }
      return updated;
    });
  }, []);

  /**
   * Refresh gateway status
   */
  const refreshGatewayStatus = useCallback(async () => {
    if (!gatewayClient) {
      setGatewayConnectionState('disconnected');
      return;
    }

    try {
      setGatewayConnectionState('connecting');
      const status = await gatewayClient.getStatus();

      if (status) {
        setGatewayStatus(status);
        setGatewayConnectionState('connected');
      } else {
        setGatewayStatus(null);
        setGatewayConnectionState('disconnected');
      }
    } catch (err) {
      logger.debug('Gateway status check failed', { error: err });
      setGatewayStatus(null);
      setGatewayConnectionState('error');
    }
  }, [gatewayClient]);

  /**
   * Auto-refresh gateway status on interval
   */
  useEffect(() => {
    if (!configLoaded || gatewayRefreshInterval <= 0 || !gatewayClient) {
      return;
    }

    // Initial fetch
    refreshGatewayStatus();

    // Set up interval
    const intervalId = setInterval(refreshGatewayStatus, gatewayRefreshInterval);

    return () => {
      clearInterval(intervalId);
    };
  }, [configLoaded, gatewayRefreshInterval, gatewayClient, refreshGatewayStatus]);

  /**
   * Context value - memoized to prevent unnecessary re-renders
   */
  const contextValue: AppContextValue = useMemo(
    () => ({
      config,
      configLoaded,
      configError,
      reloadConfig,
      updateConfig,
      gatewayStatus,
      gatewayConnectionState,
      refreshGatewayStatus,
      gatewayClient,
      options,
      version,
    }),
    [
      config,
      configLoaded,
      configError,
      reloadConfig,
      updateConfig,
      gatewayStatus,
      gatewayConnectionState,
      refreshGatewayStatus,
      gatewayClient,
      options,
      version,
    ]
  );

  return (
    <AppContext.Provider value={contextValue}>
      {children}
    </AppContext.Provider>
  );
};

export default AppProvider;

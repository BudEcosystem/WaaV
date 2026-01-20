/**
 * WaaV CLI Navigation Types
 *
 * Type definitions for the SPA-style navigation system
 */

import type React from 'react';

/**
 * All available screen names in the SPA
 */
export type ScreenName =
  | 'splash'
  | 'main_menu'
  | 'help'
  | 'provider_browser'
  | 'provider_config'
  | 'provider_test'
  | 'dashboard'
  | 'voice_mode'
  | 'voice_test'
  | 'voice_config'
  | 'dag_list'
  | 'dag_editor'
  | 'dag_visualizer'
  | 'server_status'
  | 'server_logs'
  | 'config_show'
  | 'config_editor'
  | 'setup_wizard';

/**
 * Parameters that can be passed between screens
 */
export interface ScreenParams {
  /** Provider ID for provider-related screens */
  providerId?: string;
  /** Provider type filter */
  providerType?: 'stt' | 'tts' | 'realtime';
  /** DAG name for DAG-related screens */
  dagName?: string;
  /** Screen to return to after action completes */
  returnScreen?: ScreenName;
  /** Optional message to display */
  message?: string;
  /** Log level filter for logs screen */
  logLevel?: 'debug' | 'info' | 'warn' | 'error';
  /** Follow mode for logs */
  followLogs?: boolean;
  /** Voice mode type */
  voiceMode?: 'vad' | 'push_to_talk' | 'continuous';
  /** Generic params for extensibility */
  [key: string]: unknown;
}

/**
 * History entry for navigation stack
 */
export interface ScreenHistoryEntry {
  screen: ScreenName;
  params: ScreenParams;
  timestamp: number;
}

/**
 * Breadcrumb item for navigation trail
 */
export interface BreadcrumbItem {
  screen: ScreenName;
  title: string;
  params?: ScreenParams;
}

/**
 * Keyboard shortcut definition
 */
export interface KeyboardShortcut {
  /** Key combination (e.g., 'q', 'ctrl+s', 'escape') */
  key: string;
  /** Description of what the shortcut does */
  description: string;
  /** Whether this is a global shortcut (works everywhere) */
  global?: boolean;
  /** Handler function */
  handler: () => void;
}

/**
 * Navigation context value provided to all screens
 */
export interface NavigationContextValue {
  /** Current active screen */
  currentScreen: ScreenName;
  /** Parameters for current screen */
  params: ScreenParams;
  /** Navigation history stack */
  history: ScreenHistoryEntry[];
  /** Navigate to a new screen (pushes to history) */
  navigate: (screen: ScreenName, params?: ScreenParams) => void;
  /** Replace current screen (no history push) */
  replace: (screen: ScreenName, params?: ScreenParams) => void;
  /** Go back to previous screen */
  goBack: () => void;
  /** Go directly to main menu, clearing history */
  goHome: () => void;
  /** Whether there's a previous screen to go back to */
  canGoBack: boolean;
  /** Breadcrumb trail for current navigation path */
  breadcrumbs: BreadcrumbItem[];
}

/**
 * Base props for all screen components
 */
export interface ScreenProps {
  /** Called when screen completes its task */
  onComplete?: (result?: unknown) => void;
  /** Called when screen encounters an error */
  onError?: (error: Error) => void;
}

/**
 * Screen component type
 */
export type ScreenComponent = React.FC<ScreenProps>;

/**
 * Screen category for grouping
 */
export type ScreenCategory =
  | 'main'
  | 'provider'
  | 'voice'
  | 'dag'
  | 'server'
  | 'config'
  | 'setup';

/**
 * Screen registration metadata
 */
export interface ScreenRegistration {
  /** The screen component (can be lazy-loaded) */
  component: React.LazyExoticComponent<ScreenComponent> | ScreenComponent;
  /** Display title for the screen */
  title: string;
  /** Category for grouping */
  category: ScreenCategory;
  /** Parent screen for breadcrumb navigation */
  parent?: ScreenName;
  /** Screen-specific keyboard shortcuts */
  shortcuts?: KeyboardShortcut[];
}

/**
 * Complete screen registry mapping screen names to registrations
 */
export type ScreenRegistry = Partial<Record<ScreenName, ScreenRegistration>>;

/**
 * Screen metadata for display purposes
 */
export const SCREEN_METADATA: Record<ScreenName, { title: string; category: ScreenCategory; parent?: ScreenName }> = {
  splash: { title: 'WaaV', category: 'main' },
  main_menu: { title: 'Main Menu', category: 'main' },
  help: { title: 'Help', category: 'main', parent: 'main_menu' },
  provider_browser: { title: 'Browse Providers', category: 'provider', parent: 'main_menu' },
  provider_config: { title: 'Configure Provider', category: 'provider', parent: 'provider_browser' },
  provider_test: { title: 'Test Provider', category: 'provider', parent: 'provider_config' },
  dashboard: { title: 'Dashboard', category: 'main', parent: 'main_menu' },
  voice_mode: { title: 'Voice Mode', category: 'voice', parent: 'main_menu' },
  voice_test: { title: 'Audio Test', category: 'voice', parent: 'voice_mode' },
  voice_config: { title: 'Audio Configuration', category: 'voice', parent: 'voice_mode' },
  dag_list: { title: 'Pipelines', category: 'dag', parent: 'main_menu' },
  dag_editor: { title: 'Edit Pipeline', category: 'dag', parent: 'dag_list' },
  dag_visualizer: { title: 'Visualize Pipeline', category: 'dag', parent: 'dag_list' },
  server_status: { title: 'Server Status', category: 'server', parent: 'main_menu' },
  server_logs: { title: 'Server Logs', category: 'server', parent: 'server_status' },
  config_show: { title: 'Configuration', category: 'config', parent: 'main_menu' },
  config_editor: { title: 'Edit Configuration', category: 'config', parent: 'config_show' },
  setup_wizard: { title: 'Setup Wizard', category: 'setup', parent: 'main_menu' },
};

/**
 * Build breadcrumbs from current screen to root
 */
export function buildBreadcrumbs(
  currentScreen: ScreenName,
  params: ScreenParams
): BreadcrumbItem[] {
  const breadcrumbs: BreadcrumbItem[] = [];
  let screen: ScreenName | undefined = currentScreen;

  while (screen) {
    const meta: { title: string; category: ScreenCategory; parent?: ScreenName } = SCREEN_METADATA[screen];
    breadcrumbs.unshift({
      screen,
      title: meta.title,
      params: screen === currentScreen ? params : undefined,
    });
    screen = meta.parent;
  }

  return breadcrumbs;
}

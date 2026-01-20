/**
 * WaaV CLI Screens Module
 *
 * Screen registry and exports for the SPA architecture.
 * All screens are lazy-loaded for optimal performance.
 */

import React from 'react';
import type { ScreenName } from '../types/navigation.js';
import type { ScreenRegistry } from '../layout/ScreenRenderer.js';

/**
 * Lazy load screen components
 *
 * Each screen is imported dynamically to enable code splitting
 * and reduce initial bundle size.
 */

// Main screens
const SplashScreen = React.lazy(() => import('./SplashScreen.js'));
const MainMenuScreen = React.lazy(() => import('./MainMenuScreen.js'));
const HelpScreen = React.lazy(() => import('./HelpScreen.js'));

// Dashboard
const DashboardScreen = React.lazy(() => import('./dashboard/DashboardScreen.js'));

// Provider screens
const ProviderBrowserScreen = React.lazy(() => import('./provider/ProviderBrowserScreen.js'));
const ProviderConfigScreen = React.lazy(() => import('./provider/ProviderConfigScreen.js'));
const ProviderTestScreen = React.lazy(() => import('./provider/ProviderTestScreen.js'));

// Voice screens
const VoiceModeScreen = React.lazy(() => import('./voice/VoiceModeScreen.js'));
const VoiceTestScreen = React.lazy(() => import('./voice/VoiceTestScreen.js'));
const VoiceConfigScreen = React.lazy(() => import('./voice/VoiceConfigScreen.js'));

// DAG screens
const DAGListScreen = React.lazy(() => import('./dag/DAGListScreen.js'));
const DAGEditorScreen = React.lazy(() => import('./dag/DAGEditorScreen.js'));
const DAGVisualizerScreen = React.lazy(() => import('./dag/DAGVisualizerScreen.js'));

// Server screens
const ServerStatusScreen = React.lazy(() => import('./server/ServerStatusScreen.js'));
const ServerLogsScreen = React.lazy(() => import('./server/ServerLogsScreen.js'));

// Config screens
const ConfigShowScreen = React.lazy(() => import('./config/ConfigShowScreen.js'));
const ConfigEditorScreen = React.lazy(() => import('./config/ConfigEditorScreen.js'));

// Setup screens
const SetupWizardScreen = React.lazy(() => import('./setup/SetupWizardScreen.js'));

/**
 * Screen registry mapping screen names to their lazy-loaded components
 *
 * This registry is used by the ScreenRenderer to dynamically
 * load and render the appropriate screen based on navigation state.
 */
export const SCREEN_REGISTRY: Partial<ScreenRegistry> = {
  splash: SplashScreen,
  main_menu: MainMenuScreen,
  help: HelpScreen,
  dashboard: DashboardScreen,
  provider_browser: ProviderBrowserScreen,
  provider_config: ProviderConfigScreen,
  provider_test: ProviderTestScreen,
  voice_mode: VoiceModeScreen,
  voice_test: VoiceTestScreen,
  voice_config: VoiceConfigScreen,
  dag_list: DAGListScreen,
  dag_editor: DAGEditorScreen,
  dag_visualizer: DAGVisualizerScreen,
  server_status: ServerStatusScreen,
  server_logs: ServerLogsScreen,
  config_show: ConfigShowScreen,
  config_editor: ConfigEditorScreen,
  setup_wizard: SetupWizardScreen,
};

/**
 * Get screen title by name
 */
export function getScreenTitle(screen: ScreenName): string {
  const titles: Record<ScreenName, string> = {
    splash: 'WaaV',
    main_menu: 'Main Menu',
    help: 'Help',
    dashboard: 'Dashboard',
    provider_browser: 'Browse Providers',
    provider_config: 'Configure Provider',
    provider_test: 'Test Provider',
    voice_mode: 'Voice Mode',
    voice_test: 'Audio Test',
    voice_config: 'Audio Configuration',
    dag_list: 'Pipelines',
    dag_editor: 'Edit Pipeline',
    dag_visualizer: 'Visualize Pipeline',
    server_status: 'Server Status',
    server_logs: 'Server Logs',
    config_show: 'Configuration',
    config_editor: 'Edit Configuration',
    setup_wizard: 'Setup Wizard',
  };

  return titles[screen] || screen.replace(/_/g, ' ');
}

// Re-export screen components for direct use
export {
  SplashScreen,
  MainMenuScreen,
  HelpScreen,
  DashboardScreen,
  ProviderBrowserScreen,
  ProviderConfigScreen,
  ProviderTestScreen,
  VoiceModeScreen,
  VoiceTestScreen,
  VoiceConfigScreen,
  DAGListScreen,
  DAGEditorScreen,
  DAGVisualizerScreen,
  ServerStatusScreen,
  ServerLogsScreen,
  ConfigShowScreen,
  ConfigEditorScreen,
  SetupWizardScreen,
};

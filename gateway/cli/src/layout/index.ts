/**
 * WaaV CLI Layout Module
 *
 * Re-exports all layout components for the SPA architecture.
 */

// Main application shell
export {
  AppShell,
  MinimalShell,
  type AppShellProps,
} from './AppShell.js';

// Header components
export {
  Header,
  CompactHeader,
} from './Header.js';

// Footer components
export {
  Footer,
  CompactFooter,
  ShortcutsFooter,
} from './Footer.js';

// Screen renderer
export {
  ScreenRenderer,
  ScreenWrapper,
  type ScreenRegistry,
} from './ScreenRenderer.js';

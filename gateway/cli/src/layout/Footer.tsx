/**
 * WaaV CLI Footer Component
 *
 * Displays keyboard shortcuts and status information
 * at the bottom of the application shell.
 */

import React, { useState, useEffect } from 'react';
import { Text, Box } from 'ink';
import { useCanGoBack } from '../hooks/useNavigation.js';
import type { KeyboardShortcut } from '../types/navigation.js';

/**
 * Footer component props
 */
interface FooterProps {
  /** Additional screen-specific shortcuts to display */
  shortcuts?: KeyboardShortcut[];
  /** Whether to show the timestamp */
  showTimestamp?: boolean;
  /** Whether to show global shortcuts */
  showGlobalShortcuts?: boolean;
  /** Optional status message */
  statusMessage?: string;
  /** Status message color */
  statusColor?: string;
}

/**
 * Format a keyboard shortcut for display
 */
const formatShortcutKey = (key: string): string => {
  const keyMap: Record<string, string> = {
    'escape': 'ESC',
    'return': 'Enter',
    'ctrl+c': 'Ctrl+C',
    'ctrl+s': 'Ctrl+S',
    'ctrl+h': 'Ctrl+H',
    'space': 'Space',
    'tab': 'Tab',
  };

  return keyMap[key.toLowerCase()] || key.toUpperCase();
};

/**
 * Shortcut display component
 */
const ShortcutDisplay: React.FC<{ shortcut: KeyboardShortcut }> = ({ shortcut }) => {
  return (
    <Box marginRight={2}>
      <Text color="yellow">[{formatShortcutKey(shortcut.key)}]</Text>
      <Text dimColor> {shortcut.description}</Text>
    </Box>
  );
};

/**
 * Footer component for the application shell
 */
export const Footer: React.FC<FooterProps> = ({
  shortcuts = [],
  showTimestamp = true,
  showGlobalShortcuts = true,
  statusMessage,
  statusColor = 'gray',
}) => {
  const canGoBack = useCanGoBack();
  const [time, setTime] = useState<string>(new Date().toLocaleTimeString());

  // Update time every second
  useEffect(() => {
    if (!showTimestamp) return;

    const interval = setInterval(() => {
      setTime(new Date().toLocaleTimeString());
    }, 1000);

    return () => clearInterval(interval);
  }, [showTimestamp]);

  // Build shortcuts to display
  const displayShortcuts: KeyboardShortcut[] = [];

  // Add global shortcuts (conditionally show Back based on canGoBack)
  if (showGlobalShortcuts) {
    if (canGoBack) {
      displayShortcuts.push({ key: 'ESC', description: 'Back', handler: () => {}, global: true });
    }
    displayShortcuts.push({ key: '?', description: 'Help', handler: () => {}, global: true });
    displayShortcuts.push({ key: 'Q', description: 'Quit', handler: () => {}, global: true });
  }

  // Add screen-specific shortcuts
  shortcuts.forEach(shortcut => {
    if (!displayShortcuts.some(s => s.key === shortcut.key)) {
      displayShortcuts.push(shortcut);
    }
  });

  return (
    <Box
      flexDirection="row"
      justifyContent="space-between"
      paddingX={1}
      paddingY={0}
      borderStyle="single"
      borderColor="gray"
      borderBottom
      borderLeft
      borderRight
      borderTop={false}
    >
      {/* Left side: Keyboard shortcuts */}
      <Box flexGrow={1}>
        {displayShortcuts.slice(0, 6).map((shortcut, index) => (
          <ShortcutDisplay key={`${shortcut.key}-${index}`} shortcut={shortcut} />
        ))}
      </Box>

      {/* Right side: Status and timestamp */}
      <Box>
        {statusMessage && (
          <Box marginRight={2}>
            <Text color={statusColor}>{statusMessage}</Text>
          </Box>
        )}
        {showTimestamp && (
          <Text dimColor>{time}</Text>
        )}
      </Box>
    </Box>
  );
};

/**
 * Compact footer variant for space-constrained views
 */
export const CompactFooter: React.FC<{ hint?: string }> = ({ hint }) => {
  const canGoBack = useCanGoBack();

  return (
    <Box paddingX={1}>
      <Text dimColor>
        {canGoBack ? '[ESC] Back  ' : ''}
        [Q] Quit
        {hint ? `  │  ${hint}` : ''}
      </Text>
    </Box>
  );
};

/**
 * Footer with custom shortcuts array
 */
export const ShortcutsFooter: React.FC<{
  shortcuts: Array<{ key: string; description: string }>;
  rightText?: string;
}> = ({ shortcuts, rightText }) => {
  return (
    <Box
      flexDirection="row"
      justifyContent="space-between"
      paddingX={1}
      borderStyle="single"
      borderColor="gray"
      borderBottom
      borderLeft
      borderRight
      borderTop={false}
    >
      <Box>
        {shortcuts.map((shortcut, index) => (
          <Box key={`${shortcut.key}-${index}`} marginRight={2}>
            <Text color="yellow">[{shortcut.key}]</Text>
            <Text dimColor> {shortcut.description}</Text>
          </Box>
        ))}
      </Box>
      {rightText && (
        <Text dimColor>{rightText}</Text>
      )}
    </Box>
  );
};

export default Footer;

/**
 * WaaV CLI Alert Component
 *
 * Banner-style alerts for displaying success, warning, error, and info messages
 * with optional titles, icons, and dismissible functionality.
 */

import React, { useState, useEffect } from 'react';
import { Text, Box, useInput } from 'ink';

/**
 * Alert type configuration
 */
const ALERT_CONFIG = {
  success: {
    icon: '\u2713',  // ✓
    bgIcon: '\u2714', // ✔
    color: 'green',
    bgColor: 'greenBright',
    label: 'Success',
  },
  warning: {
    icon: '\u26A0',  // ⚠
    bgIcon: '!',
    color: 'yellow',
    bgColor: 'yellowBright',
    label: 'Warning',
  },
  error: {
    icon: '\u2717',  // ✗
    bgIcon: '\u2718', // ✘
    color: 'red',
    bgColor: 'redBright',
    label: 'Error',
  },
  info: {
    icon: '\u2139',  // ℹ
    bgIcon: 'i',
    color: 'blue',
    bgColor: 'blueBright',
    label: 'Info',
  },
  tip: {
    icon: '\u{1F4A1}', // 💡 (lightbulb)
    bgIcon: '*',
    color: 'cyan',
    bgColor: 'cyanBright',
    label: 'Tip',
  },
} as const;

export type AlertType = keyof typeof ALERT_CONFIG;

export interface AlertProps {
  /** Type of alert */
  type: AlertType;
  /** Main message content */
  message: string;
  /** Optional title (defaults to type label) */
  title?: string;
  /** Whether the alert can be dismissed */
  dismissible?: boolean;
  /** Callback when dismissed */
  onDismiss?: () => void;
  /** Auto-dismiss after N milliseconds */
  autoDismissMs?: number;
  /** Show border around alert */
  bordered?: boolean;
  /** Compact mode (single line) */
  compact?: boolean;
  /** Custom icon override */
  icon?: string;
  /** Width of the alert */
  width?: number | 'auto';
  /** Margin below the alert */
  marginBottom?: number;
}

/**
 * Alert Component
 *
 * Displays styled notification banners with type-specific colors and icons.
 */
export const Alert: React.FC<AlertProps> = ({
  type,
  message,
  title,
  dismissible = false,
  onDismiss,
  autoDismissMs,
  bordered = true,
  compact = false,
  icon,
  width,
  marginBottom = 0,
}) => {
  const [dismissed, setDismissed] = useState(false);
  const config = ALERT_CONFIG[type];
  const effectiveIcon = icon ?? config.icon;
  const effectiveTitle = title ?? config.label;

  // Handle keyboard input for dismissal
  useInput((input, key) => {
    if (dismissible && (key.escape || input === 'd' || input === 'x')) {
      setDismissed(true);
      onDismiss?.();
    }
  }, { isActive: dismissible && !dismissed });

  // Auto-dismiss timer
  useEffect(() => {
    if (autoDismissMs && !dismissed) {
      const timer = setTimeout(() => {
        setDismissed(true);
        onDismiss?.();
      }, autoDismissMs);
      return () => clearTimeout(timer);
    }
    return undefined;
  }, [autoDismissMs, dismissed, onDismiss]);

  if (dismissed) {
    return null;
  }

  // Compact single-line variant
  if (compact) {
    return (
      <Box marginBottom={marginBottom}>
        <Text color={config.color}>
          {effectiveIcon} {effectiveTitle}:
        </Text>
        <Text> {message}</Text>
        {dismissible && <Text dimColor> [ESC/d to dismiss]</Text>}
      </Box>
    );
  }

  // Full bordered variant
  if (bordered) {
    const borderChar = '\u2500'; // ─
    const cornerTL = '\u256D'; // ╭
    const cornerTR = '\u256E'; // ╮
    const cornerBL = '\u2570'; // ╰
    const cornerBR = '\u256F'; // ╯
    const vertical = '\u2502'; // │

    // Calculate widths
    const titleLine = `${effectiveIcon} ${effectiveTitle}`;
    const contentWidth = typeof width === 'number'
      ? width - 4 // Subtract borders and padding
      : Math.max(titleLine.length, message.length) + 2;

    const topBorder = cornerTL + borderChar.repeat(contentWidth + 2) + cornerTR;
    const bottomBorder = cornerBL + borderChar.repeat(contentWidth + 2) + cornerBR;

    return (
      <Box flexDirection="column" marginBottom={marginBottom}>
        {/* Top border */}
        <Text color={config.color}>{topBorder}</Text>

        {/* Title line */}
        <Box>
          <Text color={config.color}>{vertical} </Text>
          <Text color={config.color} bold>{titleLine}</Text>
          <Text>{' '.repeat(Math.max(0, contentWidth - titleLine.length))}</Text>
          <Text color={config.color}> {vertical}</Text>
        </Box>

        {/* Message line */}
        <Box>
          <Text color={config.color}>{vertical} </Text>
          <Text>  {message}</Text>
          <Text>{' '.repeat(Math.max(0, contentWidth - message.length - 2))}</Text>
          <Text color={config.color}> {vertical}</Text>
        </Box>

        {/* Dismiss hint (if applicable) */}
        {dismissible && (
          <Box>
            <Text color={config.color}>{vertical} </Text>
            <Text dimColor>  Press ESC or 'd' to dismiss</Text>
            <Text>{' '.repeat(Math.max(0, contentWidth - 27))}</Text>
            <Text color={config.color}> {vertical}</Text>
          </Box>
        )}

        {/* Bottom border */}
        <Text color={config.color}>{bottomBorder}</Text>
      </Box>
    );
  }

  // Simple variant (no border)
  return (
    <Box flexDirection="column" marginBottom={marginBottom}>
      <Box>
        <Text color={config.color} bold>
          {effectiveIcon} {effectiveTitle}
        </Text>
      </Box>
      <Box marginLeft={2}>
        <Text>{message}</Text>
      </Box>
      {dismissible && (
        <Box marginLeft={2}>
          <Text dimColor>Press ESC or 'd' to dismiss</Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * Inline alert - single-line status message
 */
export interface InlineAlertProps {
  type: AlertType;
  message: string;
  icon?: string;
}

export const InlineAlert: React.FC<InlineAlertProps> = ({
  type,
  message,
  icon,
}) => {
  const config = ALERT_CONFIG[type];
  const effectiveIcon = icon ?? config.icon;

  return (
    <Text>
      <Text color={config.color}>{effectiveIcon}</Text>
      <Text> {message}</Text>
    </Text>
  );
};

/**
 * Toast notification - auto-dismissing alert
 */
export interface ToastProps {
  type: AlertType;
  message: string;
  durationMs?: number;
  onComplete?: () => void;
}

export const Toast: React.FC<ToastProps> = ({
  type,
  message,
  durationMs = 3000,
  onComplete,
}) => {
  return (
    <Alert
      type={type}
      message={message}
      compact={true}
      autoDismissMs={durationMs}
      onDismiss={onComplete}
    />
  );
};

/**
 * Callout - highlighted information box
 */
export interface CalloutProps {
  type: AlertType;
  title?: string;
  children: React.ReactNode;
}

export const Callout: React.FC<CalloutProps> = ({
  type,
  title,
  children,
}) => {
  const config = ALERT_CONFIG[type];
  const vertical = '\u2503'; // ┃

  return (
    <Box flexDirection="column">
      {title && (
        <Box>
          <Text color={config.color}>{vertical} </Text>
          <Text color={config.color} bold>{config.icon} {title}</Text>
        </Box>
      )}
      <Box>
        <Text color={config.color}>{vertical} </Text>
        <Box flexDirection="column">
          {children}
        </Box>
      </Box>
    </Box>
  );
};

/**
 * Pre-configured alert shortcuts
 */
export const SuccessAlert: React.FC<Omit<AlertProps, 'type'>> = (props) => (
  <Alert type="success" {...props} />
);

export const WarningAlert: React.FC<Omit<AlertProps, 'type'>> = (props) => (
  <Alert type="warning" {...props} />
);

export const ErrorAlert: React.FC<Omit<AlertProps, 'type'>> = (props) => (
  <Alert type="error" {...props} />
);

export const InfoAlert: React.FC<Omit<AlertProps, 'type'>> = (props) => (
  <Alert type="info" {...props} />
);

export const TipAlert: React.FC<Omit<AlertProps, 'type'>> = (props) => (
  <Alert type="tip" {...props} />
);

export default Alert;

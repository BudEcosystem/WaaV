/**
 * WaaV CLI Badge Component
 *
 * Status badges and tags for visual indicators like provider types,
 * connection status, and categorization labels.
 */

import React from 'react';
import { Text, Box } from 'ink';

/**
 * Badge color variants
 */
const BADGE_COLORS = {
  default: { bg: 'gray', fg: 'white', text: 'gray' },
  primary: { bg: 'cyan', fg: 'black', text: 'cyan' },
  success: { bg: 'green', fg: 'black', text: 'green' },
  warning: { bg: 'yellow', fg: 'black', text: 'yellow' },
  error: { bg: 'red', fg: 'white', text: 'red' },
  info: { bg: 'blue', fg: 'white', text: 'blue' },
  magenta: { bg: 'magenta', fg: 'white', text: 'magenta' },
} as const;

/**
 * Provider type badge colors
 */
const PROVIDER_TYPE_COLORS = {
  stt: 'cyan',
  tts: 'magenta',
  realtime: 'yellow',
} as const;

export type BadgeVariant = keyof typeof BADGE_COLORS;

export interface BadgeProps {
  /** Badge label text */
  label: string;
  /** Color variant */
  variant?: BadgeVariant;
  /** Badge style: solid (filled), outline (bordered), or dot (with status dot) */
  style?: 'solid' | 'outline' | 'dot';
  /** Custom color (overrides variant) */
  color?: string;
  /** Icon to show before label */
  icon?: string;
  /** Make text bold */
  bold?: boolean;
  /** Make text uppercase */
  uppercase?: boolean;
  /** Add margin right */
  marginRight?: number;
}

/**
 * Badge Component
 *
 * Compact label with color-coding for quick visual identification.
 */
export const Badge: React.FC<BadgeProps> = ({
  label,
  variant = 'default',
  style = 'outline',
  color,
  icon,
  bold = false,
  uppercase = false,
  marginRight = 0,
}) => {
  const colors = BADGE_COLORS[variant];
  const effectiveColor = color ?? colors.text;
  const displayLabel = uppercase ? label.toUpperCase() : label;

  // Solid style: [LABEL] with background color effect (simulated with inverse)
  if (style === 'solid') {
    return (
      <Box marginRight={marginRight}>
        <Text color={effectiveColor} inverse bold>
          {icon ? ` ${icon} ${displayLabel} ` : ` ${displayLabel} `}
        </Text>
      </Box>
    );
  }

  // Dot style: ● Label
  if (style === 'dot') {
    return (
      <Box marginRight={marginRight}>
        <Text color={effectiveColor}>{'\u25CF'}</Text>
        <Text bold={bold}> {displayLabel}</Text>
      </Box>
    );
  }

  // Outline style: [Label]
  return (
    <Box marginRight={marginRight}>
      <Text color={effectiveColor}>[</Text>
      <Text color={effectiveColor} bold={bold}>
        {icon ? `${icon} ` : ''}{displayLabel}
      </Text>
      <Text color={effectiveColor}>]</Text>
    </Box>
  );
};

/**
 * StatusBadge - Pre-configured for status indicators
 */
export interface StatusBadgeProps {
  status: 'online' | 'offline' | 'connecting' | 'degraded' | 'healthy' | 'unhealthy' | 'unknown';
  showLabel?: boolean;
  compact?: boolean;
}

export const StatusBadge: React.FC<StatusBadgeProps> = ({
  status,
  showLabel = true,
  compact = false,
}) => {
  const statusConfig = {
    online: { color: 'green', icon: '\u25CF', label: 'Online' },
    offline: { color: 'red', icon: '\u25CB', label: 'Offline' },
    connecting: { color: 'yellow', icon: '\u25D4', label: 'Connecting' },
    degraded: { color: 'yellow', icon: '\u25D0', label: 'Degraded' },
    healthy: { color: 'green', icon: '\u25CF', label: 'Healthy' },
    unhealthy: { color: 'red', icon: '\u25CF', label: 'Unhealthy' },
    unknown: { color: 'gray', icon: '\u25CB', label: 'Unknown' },
  };

  const config = statusConfig[status];

  if (compact) {
    return (
      <Text color={config.color}>{config.icon}</Text>
    );
  }

  return (
    <Box>
      <Text color={config.color}>{config.icon}</Text>
      {showLabel && <Text> {config.label}</Text>}
    </Box>
  );
};

/**
 * ProviderTypeBadge - Badge for STT/TTS/Realtime provider types
 */
export interface ProviderTypeBadgeProps {
  type: 'stt' | 'tts' | 'realtime';
  style?: 'solid' | 'outline';
}

export const ProviderTypeBadge: React.FC<ProviderTypeBadgeProps> = ({
  type,
  style = 'outline',
}) => {
  const color = PROVIDER_TYPE_COLORS[type];
  const labels = {
    stt: 'STT',
    tts: 'TTS',
    realtime: 'RT',
  };

  return (
    <Badge
      label={labels[type]}
      color={color}
      style={style}
      uppercase={true}
      bold={true}
    />
  );
};

/**
 * ConfiguredBadge - Shows if a provider is configured
 */
export interface ConfiguredBadgeProps {
  configured: boolean;
  compact?: boolean;
}

export const ConfiguredBadge: React.FC<ConfiguredBadgeProps> = ({
  configured,
  compact = false,
}) => {
  if (compact) {
    return (
      <Text color={configured ? 'green' : 'gray'}>
        {configured ? '\u2713' : '\u2717'}
      </Text>
    );
  }

  return configured ? (
    <Badge label="Configured" variant="success" style="outline" />
  ) : (
    <Badge label="Not configured" variant="default" style="outline" />
  );
};

/**
 * CountBadge - Badge showing a count/number
 */
export interface CountBadgeProps {
  count: number;
  label?: string;
  variant?: BadgeVariant;
}

export const CountBadge: React.FC<CountBadgeProps> = ({
  count,
  label,
  variant = 'default',
}) => {
  const colors = BADGE_COLORS[variant];
  const displayText = label ? `${count} ${label}` : String(count);

  return (
    <Text color={colors.text}>({displayText})</Text>
  );
};

/**
 * TagGroup - Render multiple badges inline
 */
export interface TagGroupProps {
  children: React.ReactNode;
  separator?: string;
}

export const TagGroup: React.FC<TagGroupProps> = ({
  children,
  separator,
}) => {
  const childArray = React.Children.toArray(children);

  return (
    <Box>
      {childArray.map((child, index) => (
        <React.Fragment key={index}>
          {child}
          {separator && index < childArray.length - 1 && (
            <Text dimColor> {separator} </Text>
          )}
        </React.Fragment>
      ))}
    </Box>
  );
};

/**
 * LatencyBadge - Shows latency with color coding
 */
export interface LatencyBadgeProps {
  latencyMs: number;
  showUnit?: boolean;
}

export const LatencyBadge: React.FC<LatencyBadgeProps> = ({
  latencyMs,
  showUnit = true,
}) => {
  let variant: BadgeVariant = 'success';

  if (latencyMs > 500) {
    variant = 'error';
  } else if (latencyMs > 200) {
    variant = 'warning';
  }

  return (
    <Badge
      label={showUnit ? `${latencyMs}ms` : String(latencyMs)}
      variant={variant}
      style="outline"
    />
  );
};

/**
 * FeatureBadge - Badge for feature availability
 */
export interface FeatureBadgeProps {
  feature: string;
  available: boolean;
}

export const FeatureBadge: React.FC<FeatureBadgeProps> = ({
  feature,
  available,
}) => {
  return (
    <Box>
      <Text color={available ? 'green' : 'gray'}>
        {available ? '\u2713' : '\u2717'}
      </Text>
      <Text dimColor={!available}> {feature}</Text>
    </Box>
  );
};

/**
 * RegionBadge - Badge for provider regions
 */
export interface RegionBadgeProps {
  region: string;
  icon?: string;
}

export const RegionBadge: React.FC<RegionBadgeProps> = ({
  region,
}) => {
  // Map common regions to flag emojis (using ASCII alternatives for terminal compatibility)
  const regionIcons: Record<string, string> = {
    'us': 'US',
    'eu': 'EU',
    'asia': 'AS',
    'apac': 'AP',
    'global': 'GL',
  };

  const icon = regionIcons[region.toLowerCase()] ?? region.slice(0, 2).toUpperCase();

  return (
    <Badge
      label={icon}
      variant="info"
      style="outline"
    />
  );
};

export default Badge;

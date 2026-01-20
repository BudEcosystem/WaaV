/**
 * WaaV CLI Card Component
 *
 * Flexible card layouts for displaying content in organized sections
 * with headers, icons, and various layouts.
 */

import React from 'react';
import { Text, Box } from 'ink';
import { BoxFrame, type BoxFrameVariant, type BoxFrameBorderStyle } from './BoxFrame.js';

/**
 * Card Props
 */
export interface CardProps {
  /** Card title */
  title?: string;
  /** Icon displayed before title */
  icon?: string;
  /** Secondary text in header */
  subtitle?: string;
  /** Card width */
  width?: number | 'auto';
  /** Card height */
  height?: number | 'auto';
  /** Visual variant */
  variant?: BoxFrameVariant;
  /** Border style */
  borderStyle?: BoxFrameBorderStyle;
  /** Horizontal padding */
  paddingX?: number;
  /** Vertical padding */
  paddingY?: number;
  /** Right margin */
  marginRight?: number;
  /** Bottom margin */
  marginBottom?: number;
  /** Children content */
  children?: React.ReactNode;
}

/**
 * Card Component
 *
 * General-purpose card container with optional title and styling.
 */
export const Card: React.FC<CardProps> = ({
  title,
  icon,
  subtitle,
  width,
  height,
  variant = 'default',
  borderStyle = 'rounded',
  paddingX = 2,
  paddingY = 1,
  marginRight = 0,
  marginBottom = 0,
  children,
}) => {
  return (
    <BoxFrame
      title={title}
      icon={icon}
      subtitle={subtitle}
      width={width}
      height={height}
      variant={variant}
      borderStyle={borderStyle}
      paddingX={paddingX}
      paddingY={paddingY}
      marginRight={marginRight}
      marginBottom={marginBottom}
      showHeaderSeparator={false}
      boldTitle={true}
    >
      {children}
    </BoxFrame>
  );
};

/**
 * StatCard - Card displaying a single statistic/metric
 */
export interface StatCardProps {
  /** Stat label */
  label: string;
  /** Stat value */
  value: string | number;
  /** Optional icon */
  icon?: string;
  /** Optional unit suffix */
  unit?: string;
  /** Optional trend indicator */
  trend?: 'up' | 'down' | 'stable';
  /** Card width */
  width?: number;
  /** Visual variant */
  variant?: BoxFrameVariant;
  /** Right margin */
  marginRight?: number;
}

export const StatCard: React.FC<StatCardProps> = ({
  label,
  value,
  icon,
  unit,
  trend,
  width = 25,
  variant = 'default',
  marginRight = 0,
}) => {
  const trendIcons = {
    up: { char: '\u2191', color: 'green' },    // ↑
    down: { char: '\u2193', color: 'red' },    // ↓
    stable: { char: '\u2192', color: 'gray' }, // →
  };

  const trendConfig = trend ? trendIcons[trend] : null;

  return (
    <Card width={width} variant={variant} marginRight={marginRight}>
      <Box flexDirection="column">
        <Text dimColor>{icon ? `${icon} ` : ''}{label}</Text>
        <Box marginTop={0}>
          <Text bold>{value}</Text>
          {unit && <Text dimColor> {unit}</Text>}
          {trendConfig && (
            <Text color={trendConfig.color}> {trendConfig.char}</Text>
          )}
        </Box>
      </Box>
    </Card>
  );
};

/**
 * StatusCard - Card showing status with indicator
 */
export interface StatusCardProps {
  /** Card title */
  title: string;
  /** Status value */
  status: 'online' | 'offline' | 'degraded' | 'unknown';
  /** Additional info text */
  info?: string;
  /** Secondary info */
  subinfo?: string;
  /** Card width */
  width?: number;
  /** Right margin */
  marginRight?: number;
}

export const StatusCard: React.FC<StatusCardProps> = ({
  title,
  status,
  info,
  subinfo,
  width = 30,
  marginRight = 0,
}) => {
  const statusConfig = {
    online: { icon: '\u25CF', color: 'green', label: 'Online' },
    offline: { icon: '\u25CB', color: 'red', label: 'Offline' },
    degraded: { icon: '\u25D0', color: 'yellow', label: 'Degraded' },
    unknown: { icon: '\u25CB', color: 'gray', label: 'Unknown' },
  };

  const config = statusConfig[status];

  return (
    <Card title={title} width={width} variant="accent" marginRight={marginRight}>
      <Box flexDirection="column">
        <Box>
          <Text color={config.color}>{config.icon}</Text>
          <Text> {config.label}</Text>
        </Box>
        {info && <Text dimColor>{info}</Text>}
        {subinfo && <Text dimColor>{subinfo}</Text>}
      </Box>
    </Card>
  );
};

/**
 * InfoCard - Card for displaying key-value information
 */
export interface InfoCardProps {
  /** Card title */
  title: string;
  /** Icon */
  icon?: string;
  /** Key-value pairs to display */
  items: Array<{ label: string; value: string | number | boolean }>;
  /** Card width */
  width?: number;
  /** Variant */
  variant?: BoxFrameVariant;
  /** Right margin */
  marginRight?: number;
}

export const InfoCard: React.FC<InfoCardProps> = ({
  title,
  icon,
  items,
  width,
  variant = 'default',
  marginRight = 0,
}) => {
  return (
    <Card title={title} icon={icon} width={width} variant={variant} marginRight={marginRight}>
      <Box flexDirection="column">
        {items.map((item, index) => (
          <Box key={index}>
            <Box width={15}>
              <Text dimColor>{item.label}:</Text>
            </Box>
            <Text>
              {typeof item.value === 'boolean'
                ? item.value
                  ? 'Yes'
                  : 'No'
                : item.value}
            </Text>
          </Box>
        ))}
      </Box>
    </Card>
  );
};

/**
 * ProviderCard - Card for displaying provider information
 */
export interface ProviderCardProps {
  /** Provider name */
  name: string;
  /** Provider type */
  type: 'stt' | 'tts' | 'realtime';
  /** Whether configured */
  configured: boolean;
  /** Additional features/info */
  features?: string[];
  /** Languages supported (count or list) */
  languages?: number | string;
  /** Latency in ms */
  latencyMs?: number;
  /** On select callback */
  onSelect?: () => void;
  /** Is currently selected */
  selected?: boolean;
  /** Card width */
  width?: number;
}

export const ProviderCard: React.FC<ProviderCardProps> = ({
  name,
  type,
  configured,
  features = [],
  languages,
  latencyMs,
  selected = false,
  width = 40,
}) => {
  const typeColors = {
    stt: 'cyan',
    tts: 'magenta',
    realtime: 'yellow',
  };

  const variant: BoxFrameVariant = selected ? 'accent' : (configured ? 'success' : 'default');

  return (
    <Card
      width={width}
      variant={variant}
      borderStyle={selected ? 'double' : 'rounded'}
    >
      <Box flexDirection="column">
        {/* Header */}
        <Box>
          <Text bold>{configured ? '\u2605 ' : ''}{name}</Text>
          <Box marginLeft={1}>
            <Text color={typeColors[type]}>[{type.toUpperCase()}]</Text>
          </Box>
        </Box>

        {/* Features */}
        {features.length > 0 && (
          <Box marginTop={0}>
            <Text dimColor>{features.join(', ')}</Text>
          </Box>
        )}

        {/* Stats */}
        <Box marginTop={0}>
          {languages && (
            <Text dimColor>
              {typeof languages === 'number' ? `${languages}+ langs` : languages}
            </Text>
          )}
          {latencyMs !== undefined && (
            <Text dimColor={latencyMs > 500} color={latencyMs > 500 ? 'yellow' : undefined}>
              {languages ? ' | ' : ''}{latencyMs}ms
            </Text>
          )}
        </Box>

        {/* Status */}
        <Box marginTop={0}>
          {configured ? (
            <Text color="green">{'\u2713'} Configured</Text>
          ) : (
            <Text dimColor>{'\u2717'} Not configured</Text>
          )}
        </Box>
      </Box>
    </Card>
  );
};

/**
 * CardGrid - Layout helper for arranging cards in a grid
 */
export interface CardGridProps {
  /** Number of columns */
  columns?: number;
  /** Gap between cards */
  gap?: number;
  /** Children cards */
  children: React.ReactNode;
}

export const CardGrid: React.FC<CardGridProps> = ({
  columns = 2,
  gap = 1,
  children,
}) => {
  const childArray = React.Children.toArray(children);
  const rows: React.ReactNode[][] = [];

  for (let i = 0; i < childArray.length; i += columns) {
    rows.push(childArray.slice(i, i + columns));
  }

  return (
    <Box flexDirection="column">
      {rows.map((row, rowIndex) => (
        <Box key={rowIndex} marginBottom={rowIndex < rows.length - 1 ? gap : 0}>
          {row.map((child, colIndex) => (
            <Box key={colIndex} marginRight={colIndex < row.length - 1 ? gap : 0}>
              {child}
            </Box>
          ))}
        </Box>
      ))}
    </Box>
  );
};

/**
 * CardRow - Layout helper for horizontal card arrangement
 */
export interface CardRowProps {
  /** Gap between cards */
  gap?: number;
  /** Children cards */
  children: React.ReactNode;
}

export const CardRow: React.FC<CardRowProps> = ({
  gap = 1,
  children,
}) => {
  const childArray = React.Children.toArray(children);

  return (
    <Box>
      {childArray.map((child, index) => (
        <Box key={index} marginRight={index < childArray.length - 1 ? gap : 0}>
          {child}
        </Box>
      ))}
    </Box>
  );
};

export default Card;

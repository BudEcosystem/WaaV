/**
 * WaaV CLI Sparkline Component
 *
 * Inline mini graphs for real-time metric visualization (htop/nvtop style)
 * Renders historical data as compact ASCII bar graphs with color gradients
 */

import React from 'react';
import { Text, Box } from 'ink';

/**
 * Sparkline characters from lowest to highest
 * Using Unicode block elements for smooth gradients
 */
const SPARK_CHARS = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/**
 * Sparkline style variants
 */
export type SparklineStyle = 'default' | 'gradient' | 'monochrome' | 'minimal';

/**
 * Color threshold configuration for gradient coloring
 */
export interface SparklineColorThresholds {
  /** Threshold for green (0-1, default 0.6) */
  green: number;
  /** Threshold for yellow (0-1, default 0.8) */
  yellow: number;
  /** Above yellow threshold shows red */
}

/**
 * Sparkline component props
 */
export interface SparklineProps {
  /** Array of numeric values to display */
  data: number[];
  /** Width in characters (default: 20) */
  width?: number;
  /** Minimum value for scaling (auto-calculated if not provided) */
  min?: number;
  /** Maximum value for scaling (auto-calculated if not provided) */
  max?: number;
  /** Display style variant */
  style?: SparklineStyle;
  /** Color for monochrome style (default: 'cyan') */
  color?: string;
  /** Color thresholds for gradient style */
  colorThresholds?: SparklineColorThresholds;
  /** Show min/max labels */
  showBounds?: boolean;
  /** Show current value at the end */
  showValue?: boolean;
  /** Format function for displayed value */
  formatValue?: (value: number) => string;
  /** Label to display before sparkline */
  label?: string;
  /** Fixed label width for alignment */
  labelWidth?: number;
}

/**
 * Default color thresholds
 */
const DEFAULT_THRESHOLDS: SparklineColorThresholds = {
  green: 0.6,
  yellow: 0.8,
};

/**
 * Get color based on normalized value (0-1) and thresholds
 */
function getValueColor(
  normalizedValue: number,
  thresholds: SparklineColorThresholds
): string {
  if (normalizedValue <= thresholds.green) {
    return 'green';
  }
  if (normalizedValue <= thresholds.yellow) {
    return 'yellow';
  }
  return 'red';
}

/**
 * Get spark character index based on normalized value (0-1)
 */
function getSparkCharIndex(normalizedValue: number): number {
  const clamped = Math.max(0, Math.min(1, normalizedValue));
  return Math.floor(clamped * (SPARK_CHARS.length - 1));
}

/**
 * Sparkline Component
 *
 * Renders an inline mini graph showing historical data points as ASCII bars.
 * Supports auto-scaling, color gradients, and various display styles.
 *
 * @example
 * ```tsx
 * <Sparkline data={[10, 20, 15, 30, 25]} width={15} style="gradient" />
 * // Output: ▂▄▃▆▅ (with colors)
 * ```
 */
export const Sparkline: React.FC<SparklineProps> = ({
  data,
  width = 20,
  min: providedMin,
  max: providedMax,
  style = 'default',
  color = 'cyan',
  colorThresholds = DEFAULT_THRESHOLDS,
  showBounds = false,
  showValue = false,
  formatValue,
  label,
  labelWidth,
}) => {
  // Handle empty data
  if (data.length === 0) {
    const emptyChars = '─'.repeat(width);
    return (
      <Box>
        {label && (
          <Box width={labelWidth} marginRight={1}>
            <Text>{label}</Text>
          </Box>
        )}
        <Text dimColor>{emptyChars}</Text>
      </Box>
    );
  }

  // Get the most recent 'width' data points (or pad if less)
  const visibleData =
    data.length >= width
      ? data.slice(-width)
      : [...Array(width - data.length).fill(null), ...data];

  // Calculate min/max for scaling
  const numericData = data.filter((v): v is number => v !== null);
  const dataMin = providedMin ?? Math.min(...numericData);
  const dataMax = providedMax ?? Math.max(...numericData);
  const range = dataMax - dataMin || 1; // Avoid division by zero

  // Normalize values to 0-1 range
  const normalizedData = visibleData.map((value) =>
    value === null ? null : (value - dataMin) / range
  );

  // Build sparkline characters
  const sparkChars = normalizedData.map((normalized, index) => {
    if (normalized === null) {
      // Empty space for padding
      return { char: ' ', color: undefined, dimColor: true };
    }

    const charIndex = getSparkCharIndex(normalized);
    const char = SPARK_CHARS[charIndex];

    // Determine color based on style
    let charColor: string | undefined;
    let dimColor = false;

    switch (style) {
      case 'gradient':
        charColor = getValueColor(normalized, colorThresholds);
        break;
      case 'monochrome':
        charColor = color;
        break;
      case 'minimal':
        dimColor = true;
        break;
      case 'default':
      default:
        charColor = 'cyan';
        break;
    }

    return { char, color: charColor, dimColor, index };
  });

  // Get current value (last non-null value)
  const currentValue = data[data.length - 1] ?? 0;
  const formattedValue = formatValue
    ? formatValue(currentValue)
    : currentValue.toFixed(1);

  return (
    <Box>
      {/* Label */}
      {label && (
        <Box width={labelWidth} marginRight={1}>
          <Text>{label}</Text>
        </Box>
      )}

      {/* Min bound */}
      {showBounds && (
        <Box marginRight={1}>
          <Text dimColor>{dataMin.toFixed(0)}</Text>
        </Box>
      )}

      {/* Sparkline */}
      {sparkChars.map((spark, index) => (
        <Text
          key={index}
          color={spark.color as Parameters<typeof Text>[0]['color']}
          dimColor={spark.dimColor}
        >
          {spark.char}
        </Text>
      ))}

      {/* Max bound */}
      {showBounds && (
        <Box marginLeft={1}>
          <Text dimColor>{dataMax.toFixed(0)}</Text>
        </Box>
      )}

      {/* Current value */}
      {showValue && (
        <Box marginLeft={1}>
          <Text bold>{formattedValue}</Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * Compact sparkline with label and value on one line (htop style)
 */
export interface CompactSparklineProps extends Omit<SparklineProps, 'showValue' | 'showBounds'> {
  /** Unit suffix for value (e.g., '/s', 'ms', '%') */
  unit?: string;
}

export const CompactSparkline: React.FC<CompactSparklineProps> = ({
  data,
  label,
  unit = '',
  formatValue,
  ...rest
}) => {
  const currentValue = data.length > 0 ? (data[data.length - 1] ?? 0) : 0;
  const formatted = formatValue
    ? formatValue(currentValue)
    : currentValue.toFixed(0);

  return (
    <Box>
      {label && (
        <Box marginRight={1}>
          <Text dimColor>{label}:</Text>
        </Box>
      )}
      <Sparkline data={data} {...rest} />
      <Box marginLeft={1}>
        <Text bold>{formatted}</Text>
        <Text dimColor>{unit}</Text>
      </Box>
    </Box>
  );
};

/**
 * Multi-line sparkline group for comparing multiple metrics
 */
export interface SparklineGroupProps {
  /** Array of sparkline configurations */
  sparklines: Array<{
    label: string;
    data: number[];
    color?: string;
    unit?: string;
    formatValue?: (value: number) => string;
  }>;
  /** Width for each sparkline */
  width?: number;
  /** Fixed label width for alignment */
  labelWidth?: number;
  /** Style for all sparklines */
  style?: SparklineStyle;
}

export const SparklineGroup: React.FC<SparklineGroupProps> = ({
  sparklines,
  width = 20,
  labelWidth = 12,
  style = 'gradient',
}) => {
  return (
    <Box flexDirection="column">
      {sparklines.map((config, index) => (
        <Box key={index}>
          <Box width={labelWidth}>
            <Text>{config.label}</Text>
          </Box>
          <Sparkline
            data={config.data}
            width={width}
            style={style}
            color={config.color}
            showValue
            formatValue={config.formatValue}
          />
          {config.unit && (
            <Text dimColor>{config.unit}</Text>
          )}
        </Box>
      ))}
    </Box>
  );
};

/**
 * Real-time sparkline that maintains its own data buffer
 * Useful for streaming metrics
 */
export interface BufferedSparklineProps extends Omit<SparklineProps, 'data'> {
  /** Current value to append to buffer */
  value: number;
  /** Maximum buffer size (default: same as width) */
  bufferSize?: number;
  /** Internal data buffer (managed externally for React state) */
  buffer: number[];
}

export const BufferedSparkline: React.FC<BufferedSparklineProps> = ({
  value,
  bufferSize,
  buffer,
  width = 20,
  ...rest
}) => {
  // Use provided buffer directly (state managed by parent)
  return <Sparkline data={buffer} width={width} {...rest} />;
};

/**
 * Hook for managing sparkline buffer state
 * Returns [buffer, addValue] tuple
 */
export function useSparklineBuffer(maxSize: number = 20): [number[], (value: number) => void] {
  const [buffer, setBuffer] = React.useState<number[]>([]);

  const addValue = React.useCallback(
    (value: number) => {
      setBuffer((prev) => {
        const newBuffer = [...prev, value];
        return newBuffer.length > maxSize ? newBuffer.slice(-maxSize) : newBuffer;
      });
    },
    [maxSize]
  );

  return [buffer, addValue];
}

export default Sparkline;

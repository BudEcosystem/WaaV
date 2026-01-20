/**
 * WaaV CLI Gauge Component
 *
 * Horizontal bar gauges for displaying CPU/Memory usage (htop/nvtop style)
 * Supports color thresholds, labels, and various visual styles
 */

import React from 'react';
import { Text, Box } from 'ink';

/**
 * Gauge bar characters
 */
const GAUGE_CHARS = {
  filled: {
    default: '█',
    blocks: '▓',
    light: '░',
    thin: '━',
  },
  empty: {
    default: '░',
    blocks: '░',
    light: '·',
    thin: '─',
  },
};

/**
 * Gauge style variants
 */
export type GaugeStyle = 'default' | 'blocks' | 'light' | 'thin' | 'htop';

/**
 * Gauge orientation
 */
export type GaugeOrientation = 'horizontal' | 'vertical';

/**
 * Color threshold configuration
 */
export interface GaugeColorThresholds {
  /** Green threshold (0-1, default 0.6) */
  green: number;
  /** Yellow threshold (0-1, default 0.8) */
  yellow: number;
  /** Above yellow threshold shows red */
}

/**
 * Gauge component props
 */
export interface GaugeProps {
  /** Current value (will be clamped to 0-max range) */
  value: number;
  /** Maximum value (default: 100) */
  max?: number;
  /** Width in characters (default: 20) */
  width?: number;
  /** Visual style variant */
  style?: GaugeStyle;
  /** Color thresholds for automatic coloring */
  colorThresholds?: GaugeColorThresholds;
  /** Fixed color override (disables threshold coloring) */
  color?: string;
  /** Show percentage at the end */
  showPercent?: boolean;
  /** Show absolute value (e.g., "1.2G/4G") */
  showValue?: boolean;
  /** Format function for absolute value display */
  formatValue?: (value: number, max: number) => string;
  /** Label to display before gauge */
  label?: string;
  /** Fixed label width for alignment */
  labelWidth?: number;
  /** Show bracket characters around gauge */
  showBrackets?: boolean;
  /** Custom bracket characters [left, right] */
  brackets?: [string, string];
}

/**
 * Default color thresholds
 */
const DEFAULT_THRESHOLDS: GaugeColorThresholds = {
  green: 0.6,
  yellow: 0.8,
};

/**
 * Get color based on percentage and thresholds
 */
function getGaugeColor(
  percent: number,
  thresholds: GaugeColorThresholds
): string {
  if (percent <= thresholds.green) {
    return 'green';
  }
  if (percent <= thresholds.yellow) {
    return 'yellow';
  }
  return 'red';
}

/**
 * Format bytes to human-readable string
 */
function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes}B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)}K`;
  }
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)}M`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)}G`;
}

/**
 * Gauge Component (htop/nvtop style)
 *
 * Renders a horizontal progress bar with color-coded fill based on thresholds.
 * Commonly used for CPU, memory, and resource utilization display.
 *
 * @example
 * ```tsx
 * <Gauge value={52} max={100} label="CPU" style="htop" />
 * // Output: CPU [████████░░░░░░░░] 52%
 * ```
 */
export const Gauge: React.FC<GaugeProps> = ({
  value,
  max = 100,
  width = 20,
  style = 'default',
  colorThresholds = DEFAULT_THRESHOLDS,
  color,
  showPercent = true,
  showValue = false,
  formatValue,
  label,
  labelWidth,
  showBrackets = true,
  brackets = ['[', ']'],
}) => {
  // Clamp value to valid range
  const clampedValue = Math.max(0, Math.min(max, value));
  const percent = clampedValue / max;
  const percentDisplay = Math.round(percent * 100);

  // Calculate filled width
  const filledWidth = Math.round(percent * width);
  const emptyWidth = width - filledWidth;

  // Determine color
  const barColor = color ?? getGaugeColor(percent, colorThresholds);

  // Get characters based on style
  let filledChar: string;
  let emptyChar: string;

  if (style === 'htop') {
    // htop uses gradient characters based on percentage within each cell
    filledChar = '|';
    emptyChar = ' ';
  } else {
    filledChar = GAUGE_CHARS.filled[style] ?? GAUGE_CHARS.filled.default;
    emptyChar = GAUGE_CHARS.empty[style] ?? GAUGE_CHARS.empty.default;
  }

  // Format value display
  const valueDisplay = formatValue
    ? formatValue(clampedValue, max)
    : `${formatBytes(clampedValue)}/${formatBytes(max)}`;

  return (
    <Box>
      {/* Label */}
      {label && (
        <Box width={labelWidth} marginRight={1}>
          <Text>{label}</Text>
        </Box>
      )}

      {/* Left bracket */}
      {showBrackets && <Text dimColor>{brackets[0]}</Text>}

      {/* Filled portion */}
      {style === 'htop' ? (
        // htop-style with pipe characters
        <Text color={barColor as Parameters<typeof Text>[0]['color']}>
          {filledChar.repeat(filledWidth)}
        </Text>
      ) : (
        <Text color={barColor as Parameters<typeof Text>[0]['color']}>
          {filledChar.repeat(filledWidth)}
        </Text>
      )}

      {/* Empty portion */}
      <Text dimColor>{emptyChar.repeat(emptyWidth)}</Text>

      {/* Right bracket */}
      {showBrackets && <Text dimColor>{brackets[1]}</Text>}

      {/* Percentage */}
      {showPercent && (
        <Box marginLeft={1} width={4}>
          <Text>{percentDisplay.toString().padStart(3, ' ')}%</Text>
        </Box>
      )}

      {/* Absolute value */}
      {showValue && (
        <Box marginLeft={1}>
          <Text dimColor>{valueDisplay}</Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * CPU-style gauge with multiple cores (htop style)
 */
export interface CPUGaugeProps {
  /** Array of CPU usage percentages (0-100) for each core */
  cores: number[];
  /** Width for each gauge */
  width?: number;
  /** Show average at the end */
  showAverage?: boolean;
  /** Compact mode (single line summary) */
  compact?: boolean;
}

export const CPUGauge: React.FC<CPUGaugeProps> = ({
  cores,
  width = 20,
  showAverage = true,
  compact = false,
}) => {
  const average = cores.length > 0
    ? cores.reduce((a, b) => a + b, 0) / cores.length
    : 0;

  if (compact) {
    return (
      <Box>
        <Box width={4}>
          <Text>CPU</Text>
        </Box>
        <Gauge
          value={average}
          max={100}
          width={width}
          style="htop"
          showPercent
        />
        <Text dimColor> ({cores.length} cores)</Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      {cores.map((usage, index) => (
        <Box key={index}>
          <Box width={4}>
            <Text dimColor>{index}</Text>
          </Box>
          <Gauge
            value={usage}
            max={100}
            width={width}
            style="htop"
            showPercent
          />
        </Box>
      ))}
      {showAverage && (
        <Box marginTop={1}>
          <Box width={4}>
            <Text bold>Avg</Text>
          </Box>
          <Gauge
            value={average}
            max={100}
            width={width}
            style="default"
            showPercent
          />
        </Box>
      )}
    </Box>
  );
};

/**
 * Memory gauge with used/total display (htop style)
 */
export interface MemoryGaugeProps {
  /** Used memory in bytes */
  used: number;
  /** Total memory in bytes */
  total: number;
  /** Width in characters */
  width?: number;
  /** Label (default: "Mem") */
  label?: string;
  /** Show buffer/cache breakdown */
  breakdown?: {
    buffers?: number;
    cached?: number;
  };
}

export const MemoryGauge: React.FC<MemoryGaugeProps> = ({
  used,
  total,
  width = 30,
  label = 'Mem',
  breakdown,
}) => {
  const percent = total > 0 ? used / total : 0;

  // Calculate breakdown widths if provided
  let usedWidth = Math.round(percent * width);
  let bufferWidth = 0;
  let cacheWidth = 0;

  if (breakdown) {
    const bufferPercent = breakdown.buffers ? breakdown.buffers / total : 0;
    const cachePercent = breakdown.cached ? breakdown.cached / total : 0;
    bufferWidth = Math.round(bufferPercent * width);
    cacheWidth = Math.round(cachePercent * width);
    usedWidth = Math.max(0, usedWidth - bufferWidth - cacheWidth);
  }

  const emptyWidth = width - usedWidth - bufferWidth - cacheWidth;

  return (
    <Box>
      {/* Label */}
      <Box width={4}>
        <Text>{label}</Text>
      </Box>

      {/* Gauge */}
      <Text dimColor>[</Text>

      {/* Used (green/yellow/red based on percentage) */}
      <Text color={percent > 0.8 ? 'red' : percent > 0.6 ? 'yellow' : 'green'}>
        {'|'.repeat(usedWidth)}
      </Text>

      {/* Buffers (blue) */}
      {bufferWidth > 0 && (
        <Text color="blue">{'|'.repeat(bufferWidth)}</Text>
      )}

      {/* Cache (cyan) */}
      {cacheWidth > 0 && (
        <Text color="cyan">{'|'.repeat(cacheWidth)}</Text>
      )}

      {/* Empty */}
      <Text dimColor>{' '.repeat(Math.max(0, emptyWidth))}</Text>

      <Text dimColor>]</Text>

      {/* Value display */}
      <Box marginLeft={1}>
        <Text>{formatBytes(used)}</Text>
        <Text dimColor>/</Text>
        <Text>{formatBytes(total)}</Text>
      </Box>
    </Box>
  );
};

/**
 * Network gauge showing throughput
 */
export interface NetworkGaugeProps {
  /** Download speed in bytes/s */
  download: number;
  /** Upload speed in bytes/s */
  upload: number;
  /** Max speed for scaling */
  maxSpeed?: number;
  /** Width in characters */
  width?: number;
}

export const NetworkGauge: React.FC<NetworkGaugeProps> = ({
  download,
  upload,
  maxSpeed = 125000000, // Default 1 Gbps
  width = 20,
}) => {
  return (
    <Box flexDirection="column">
      <Box>
        <Box width={3}>
          <Text color="green">▼</Text>
        </Box>
        <Gauge
          value={download}
          max={maxSpeed}
          width={width}
          color="green"
          showPercent={false}
          showValue
          formatValue={(v) => `${formatBytes(v)}/s`}
        />
      </Box>
      <Box>
        <Box width={3}>
          <Text color="cyan">▲</Text>
        </Box>
        <Gauge
          value={upload}
          max={maxSpeed}
          width={width}
          color="cyan"
          showPercent={false}
          showValue
          formatValue={(v) => `${formatBytes(v)}/s`}
        />
      </Box>
    </Box>
  );
};

/**
 * Vertical gauge for multi-metric display
 */
export interface VerticalGaugeProps {
  /** Value (0-max) */
  value: number;
  /** Maximum value */
  max?: number;
  /** Height in lines */
  height?: number;
  /** Label below gauge */
  label?: string;
  /** Color thresholds */
  colorThresholds?: GaugeColorThresholds;
}

export const VerticalGauge: React.FC<VerticalGaugeProps> = ({
  value,
  max = 100,
  height = 8,
  label,
  colorThresholds = DEFAULT_THRESHOLDS,
}) => {
  const percent = Math.max(0, Math.min(1, value / max));
  const filledHeight = Math.round(percent * height);
  const color = getGaugeColor(percent, colorThresholds);

  const bars = [];
  for (let i = height - 1; i >= 0; i--) {
    const isFilled = i < filledHeight;
    bars.push(
      <Box key={i} justifyContent="center">
        <Text color={isFilled ? color as Parameters<typeof Text>[0]['color'] : undefined} dimColor={!isFilled}>
          {isFilled ? '█' : '░'}
        </Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column" alignItems="center">
      {bars}
      {label && (
        <Box marginTop={1}>
          <Text dimColor>{label}</Text>
        </Box>
      )}
      <Text>{Math.round(percent * 100)}%</Text>
    </Box>
  );
};

/**
 * Multi-gauge row for comparing metrics side by side
 */
export interface GaugeRowProps {
  /** Array of gauge configurations */
  gauges: Array<{
    label: string;
    value: number;
    max?: number;
    color?: string;
    unit?: string;
  }>;
  /** Width for each gauge */
  gaugeWidth?: number;
  /** Fixed label width */
  labelWidth?: number;
}

export const GaugeRow: React.FC<GaugeRowProps> = ({
  gauges,
  gaugeWidth = 15,
  labelWidth = 8,
}) => {
  return (
    <Box flexDirection="column">
      {gauges.map((config, index) => (
        <Gauge
          key={index}
          label={config.label}
          labelWidth={labelWidth}
          value={config.value}
          max={config.max}
          width={gaugeWidth}
          color={config.color}
          showPercent
        />
      ))}
    </Box>
  );
};

export default Gauge;

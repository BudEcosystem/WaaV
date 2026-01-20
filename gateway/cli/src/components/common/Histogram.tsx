/**
 * WaaV CLI Histogram Component
 *
 * Latency distribution visualization with percentile markers (nvtop style)
 * Renders horizontal bar charts with bucket labels and statistical summaries
 */

import React from 'react';
import { Text, Box } from 'ink';

/**
 * Histogram bar characters
 */
const BAR_CHARS = {
  full: '█',
  seven: '▇',
  six: '▆',
  five: '▅',
  four: '▄',
  three: '▃',
  two: '▂',
  one: '▁',
  empty: ' ',
};

/**
 * Histogram style variants
 */
export type HistogramStyle = 'bars' | 'blocks' | 'dots' | 'ascii';

/**
 * Histogram orientation
 */
export type HistogramOrientation = 'horizontal' | 'vertical';

/**
 * Single histogram bucket
 */
export interface HistogramBucket {
  /** Lower bound of bucket (inclusive) */
  min: number;
  /** Upper bound of bucket (exclusive, except last bucket) */
  max: number;
  /** Count of values in this bucket */
  count: number;
  /** Optional label override */
  label?: string;
}

/**
 * Percentile marker configuration
 */
export interface PercentileMarker {
  /** Percentile value (0-100) */
  percentile: number;
  /** Color for the marker */
  color?: string;
  /** Character to use for marker */
  char?: string;
}

/**
 * Histogram component props
 */
export interface HistogramProps {
  /** Pre-computed buckets or raw data values */
  data: HistogramBucket[] | number[];
  /** Number of buckets to create from raw data (default: 10) */
  bucketCount?: number;
  /** Width of the histogram (default: 40) */
  width?: number;
  /** Height (for vertical orientation) */
  height?: number;
  /** Visual style variant */
  style?: HistogramStyle;
  /** Display orientation */
  orientation?: HistogramOrientation;
  /** Show bucket labels on x-axis */
  showLabels?: boolean;
  /** Show count values on bars */
  showCounts?: boolean;
  /** Show percentile markers */
  percentileMarkers?: PercentileMarker[];
  /** Title for the histogram */
  title?: string;
  /** Unit for values (e.g., "ms", "s") */
  unit?: string;
  /** Color for bars (default: 'cyan') */
  color?: string;
  /** Maximum count for scaling (auto-calculated if not provided) */
  maxCount?: number;
}

/**
 * Default percentile markers for latency histograms
 */
const DEFAULT_PERCENTILE_MARKERS: PercentileMarker[] = [
  { percentile: 50, color: 'green', char: '▼' },
  { percentile: 90, color: 'yellow', char: '▼' },
  { percentile: 99, color: 'red', char: '▼' },
];

/**
 * Create histogram buckets from raw data
 */
function createBuckets(data: number[], bucketCount: number): HistogramBucket[] {
  if (data.length === 0) {
    return [];
  }

  const min = Math.min(...data);
  const max = Math.max(...data);
  const range = max - min || 1;
  const bucketSize = range / bucketCount;

  const buckets: HistogramBucket[] = [];

  for (let i = 0; i < bucketCount; i++) {
    const bucketMin = min + i * bucketSize;
    const bucketMax = min + (i + 1) * bucketSize;
    const count = data.filter(
      (v) => v >= bucketMin && (i === bucketCount - 1 ? v <= bucketMax : v < bucketMax)
    ).length;

    buckets.push({
      min: bucketMin,
      max: bucketMax,
      count,
    });
  }

  return buckets;
}

/**
 * Calculate percentile value from sorted data
 */
function calculatePercentile(sortedData: number[], percentile: number): number {
  if (sortedData.length === 0) {
    return 0;
  }
  const index = Math.ceil((percentile / 100) * sortedData.length) - 1;
  return sortedData[Math.max(0, Math.min(index, sortedData.length - 1))] ?? 0;
}

/**
 * Get bar character based on fill percentage
 */
function getBarChar(fillPercent: number, style: HistogramStyle): string {
  if (style === 'ascii') {
    return fillPercent > 0.5 ? '#' : fillPercent > 0 ? '-' : ' ';
  }
  if (style === 'dots') {
    return fillPercent > 0.5 ? '●' : fillPercent > 0 ? '○' : ' ';
  }

  // Block-based characters
  if (fillPercent >= 1) return BAR_CHARS.full;
  if (fillPercent >= 0.875) return BAR_CHARS.seven;
  if (fillPercent >= 0.75) return BAR_CHARS.six;
  if (fillPercent >= 0.625) return BAR_CHARS.five;
  if (fillPercent >= 0.5) return BAR_CHARS.four;
  if (fillPercent >= 0.375) return BAR_CHARS.three;
  if (fillPercent >= 0.25) return BAR_CHARS.two;
  if (fillPercent > 0) return BAR_CHARS.one;
  return BAR_CHARS.empty;
}

/**
 * Format bucket label
 */
function formatBucketLabel(value: number, unit: string): string {
  if (value >= 1000) {
    return `${(value / 1000).toFixed(0)}${unit === 'ms' ? 's' : 'k'}`;
  }
  return `${value.toFixed(0)}${unit}`;
}

/**
 * Histogram Component
 *
 * Renders a distribution of values as a bar chart, commonly used for
 * latency distribution visualization with percentile markers.
 *
 * @example
 * ```tsx
 * <Histogram
 *   data={latencies}
 *   title="Latency Distribution"
 *   unit="ms"
 *   percentileMarkers={[
 *     { percentile: 50, color: 'green' },
 *     { percentile: 99, color: 'red' }
 *   ]}
 * />
 * ```
 */
export const Histogram: React.FC<HistogramProps> = ({
  data,
  bucketCount = 10,
  width = 40,
  height = 8,
  style = 'bars',
  orientation = 'vertical',
  showLabels = true,
  showCounts = false,
  percentileMarkers = DEFAULT_PERCENTILE_MARKERS,
  title,
  unit = '',
  color = 'cyan',
  maxCount: providedMaxCount,
}) => {
  // Convert raw data to buckets if needed
  const buckets: HistogramBucket[] = Array.isArray(data) && typeof data[0] === 'number'
    ? createBuckets(data as number[], bucketCount)
    : (data as HistogramBucket[]);

  if (buckets.length === 0) {
    return (
      <Box>
        <Text dimColor>No data to display</Text>
      </Box>
    );
  }

  // Calculate max count for scaling
  const maxCount = providedMaxCount ?? Math.max(...buckets.map((b) => b.count));

  // Calculate percentiles from raw data
  const rawData = Array.isArray(data) && typeof data[0] === 'number'
    ? (data as number[]).sort((a, b) => a - b)
    : [];

  const percentileValues = percentileMarkers.map((marker) => ({
    ...marker,
    value: calculatePercentile(rawData, marker.percentile),
  }));

  // Total count for percentage calculations
  const totalCount = buckets.reduce((sum, b) => sum + b.count, 0);

  if (orientation === 'horizontal') {
    return (
      <HorizontalHistogram
        buckets={buckets}
        maxCount={maxCount}
        width={width}
        style={style}
        showCounts={showCounts}
        title={title}
        unit={unit}
        color={color}
        percentileValues={percentileValues}
        totalCount={totalCount}
      />
    );
  }

  return (
    <VerticalHistogram
      buckets={buckets}
      maxCount={maxCount}
      height={height}
      style={style}
      showLabels={showLabels}
      showCounts={showCounts}
      title={title}
      unit={unit}
      color={color}
      percentileValues={percentileValues}
      totalCount={totalCount}
    />
  );
};

/**
 * Vertical histogram (bars going up)
 */
interface VerticalHistogramInternalProps {
  buckets: HistogramBucket[];
  maxCount: number;
  height: number;
  style: HistogramStyle;
  showLabels: boolean;
  showCounts: boolean;
  title?: string;
  unit: string;
  color: string;
  percentileValues: Array<PercentileMarker & { value: number }>;
  totalCount: number;
}

const VerticalHistogram: React.FC<VerticalHistogramInternalProps> = ({
  buckets,
  maxCount,
  height,
  style,
  showLabels,
  showCounts,
  title,
  unit,
  color,
  percentileValues,
  totalCount,
}) => {
  // Build rows from top to bottom
  const rows: React.ReactNode[] = [];

  for (let row = height - 1; row >= 0; row--) {
    const threshold = (row + 1) / height;
    const rowContent = buckets.map((bucket, idx) => {
      const fillPercent = maxCount > 0 ? bucket.count / maxCount : 0;
      const filled = fillPercent >= threshold;
      const char = filled ? getBarChar(1, style) : ' ';

      return (
        <Text
          key={idx}
          color={filled ? color as Parameters<typeof Text>[0]['color'] : undefined}
          dimColor={!filled}
        >
          {char}
        </Text>
      );
    });

    rows.push(
      <Box key={row}>
        {rowContent}
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      {/* Title */}
      {title && (
        <Box marginBottom={1}>
          <Text bold>{title}</Text>
        </Box>
      )}

      {/* Percentile summary */}
      {percentileValues.length > 0 && (
        <Box marginBottom={1}>
          {percentileValues.map((p, idx) => (
            <Box key={idx} marginRight={2}>
              <Text color={p.color as Parameters<typeof Text>[0]['color']}>
                p{p.percentile}: {p.value.toFixed(0)}{unit}
              </Text>
            </Box>
          ))}
        </Box>
      )}

      {/* Histogram bars */}
      {rows}

      {/* X-axis labels */}
      {showLabels && (
        <Box marginTop={1}>
          {buckets.map((bucket, idx) => (
            <Box key={idx} width={1}>
              <Text dimColor>
                {idx === 0 || idx === buckets.length - 1
                  ? formatBucketLabel(bucket.min, unit)
                  : ''}
              </Text>
            </Box>
          ))}
        </Box>
      )}

      {/* Count summary */}
      {showCounts && (
        <Box marginTop={1}>
          <Text dimColor>Total: {totalCount} samples</Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * Horizontal histogram (bars going right)
 */
interface HorizontalHistogramInternalProps {
  buckets: HistogramBucket[];
  maxCount: number;
  width: number;
  style: HistogramStyle;
  showCounts: boolean;
  title?: string;
  unit: string;
  color: string;
  percentileValues: Array<PercentileMarker & { value: number }>;
  totalCount: number;
}

const HorizontalHistogram: React.FC<HorizontalHistogramInternalProps> = ({
  buckets,
  maxCount,
  width,
  style,
  showCounts,
  title,
  unit,
  color,
  percentileValues,
  totalCount,
}) => {
  const labelWidth = 8;
  const barWidth = width - labelWidth - (showCounts ? 8 : 0);

  return (
    <Box flexDirection="column">
      {/* Title */}
      {title && (
        <Box marginBottom={1}>
          <Text bold>{title}</Text>
        </Box>
      )}

      {/* Percentile summary */}
      {percentileValues.length > 0 && (
        <Box marginBottom={1}>
          {percentileValues.map((p, idx) => (
            <Box key={idx} marginRight={2}>
              <Text color={p.color as Parameters<typeof Text>[0]['color']}>
                p{p.percentile}: {p.value.toFixed(0)}{unit}
              </Text>
            </Box>
          ))}
        </Box>
      )}

      {/* Histogram rows */}
      {buckets.map((bucket, idx) => {
        const fillPercent = maxCount > 0 ? bucket.count / maxCount : 0;
        const filledWidth = Math.round(fillPercent * barWidth);
        const emptyWidth = barWidth - filledWidth;
        const barChar = style === 'ascii' ? '#' : style === 'dots' ? '●' : '█';
        const emptyChar = style === 'ascii' ? '-' : style === 'dots' ? '·' : '░';

        return (
          <Box key={idx}>
            {/* Bucket label */}
            <Box width={labelWidth}>
              <Text dimColor>
                {formatBucketLabel(bucket.min, unit).padStart(labelWidth - 1)}
              </Text>
            </Box>

            {/* Bar */}
            <Text color={color as Parameters<typeof Text>[0]['color']}>
              {barChar.repeat(filledWidth)}
            </Text>
            <Text dimColor>
              {emptyChar.repeat(emptyWidth)}
            </Text>

            {/* Count */}
            {showCounts && (
              <Box marginLeft={1} width={7}>
                <Text dimColor>({bucket.count})</Text>
              </Box>
            )}
          </Box>
        );
      })}

      {/* Total */}
      <Box marginTop={1}>
        <Text dimColor>Total: {totalCount} samples</Text>
      </Box>
    </Box>
  );
};

/**
 * Compact latency histogram for inline display
 */
export interface LatencyHistogramProps {
  /** Array of latency values in milliseconds */
  latencies: number[];
  /** Width in characters */
  width?: number;
  /** Show percentile markers */
  showPercentiles?: boolean;
}

export const LatencyHistogram: React.FC<LatencyHistogramProps> = ({
  latencies,
  width = 40,
  showPercentiles = true,
}) => {
  if (latencies.length === 0) {
    return (
      <Box>
        <Text dimColor>No latency data</Text>
      </Box>
    );
  }

  const sorted = [...latencies].sort((a, b) => a - b);
  const p50 = calculatePercentile(sorted, 50);
  const p90 = calculatePercentile(sorted, 90);
  const p99 = calculatePercentile(sorted, 99);

  return (
    <Box flexDirection="column">
      {/* Percentile summary */}
      {showPercentiles && (
        <Box marginBottom={1}>
          <Text>Latency (ms)  </Text>
          <Text color="green">p50: {p50.toFixed(0)}  </Text>
          <Text color="yellow">p90: {p90.toFixed(0)}  </Text>
          <Text color="red">p99: {p99.toFixed(0)}</Text>
        </Box>
      )}

      {/* Inline histogram */}
      <Histogram
        data={latencies}
        width={width}
        height={3}
        style="bars"
        orientation="vertical"
        showLabels={false}
        showCounts={false}
        percentileMarkers={[]}
        unit="ms"
      />

      {/* X-axis labels */}
      <Box>
        <Text dimColor>{' 0'}</Text>
        <Box flexGrow={1} justifyContent="center">
          <Text dimColor>{Math.round(Math.max(...latencies) / 2)}</Text>
        </Box>
        <Text dimColor>{Math.round(Math.max(...latencies))}+</Text>
      </Box>
    </Box>
  );
};

/**
 * Statistics summary component
 */
export interface StatsSummaryProps {
  /** Array of values */
  data: number[];
  /** Unit suffix */
  unit?: string;
  /** Show histogram visualization */
  showHistogram?: boolean;
}

export const StatsSummary: React.FC<StatsSummaryProps> = ({
  data,
  unit = '',
  showHistogram = false,
}) => {
  if (data.length === 0) {
    return (
      <Box>
        <Text dimColor>No data</Text>
      </Box>
    );
  }

  const sorted = [...data].sort((a, b) => a - b);
  const min = sorted[0] ?? 0;
  const max = sorted[sorted.length - 1] ?? 0;
  const sum = sorted.reduce((a, b) => a + b, 0);
  const mean = sum / sorted.length;
  const p50 = calculatePercentile(sorted, 50);
  const p90 = calculatePercentile(sorted, 90);
  const p99 = calculatePercentile(sorted, 99);

  // Standard deviation
  const squaredDiffs = sorted.map((v) => Math.pow(v - mean, 2));
  const avgSquaredDiff = squaredDiffs.reduce((a, b) => a + b, 0) / sorted.length;
  const stdDev = Math.sqrt(avgSquaredDiff);

  return (
    <Box flexDirection="column">
      <Box>
        <Box width={12}><Text dimColor>Count:</Text></Box>
        <Text>{data.length}</Text>
      </Box>
      <Box>
        <Box width={12}><Text dimColor>Min:</Text></Box>
        <Text>{min.toFixed(2)}{unit}</Text>
      </Box>
      <Box>
        <Box width={12}><Text dimColor>Max:</Text></Box>
        <Text>{max.toFixed(2)}{unit}</Text>
      </Box>
      <Box>
        <Box width={12}><Text dimColor>Mean:</Text></Box>
        <Text>{mean.toFixed(2)}{unit}</Text>
      </Box>
      <Box>
        <Box width={12}><Text dimColor>Std Dev:</Text></Box>
        <Text>{stdDev.toFixed(2)}{unit}</Text>
      </Box>
      <Box>
        <Box width={12}><Text dimColor>p50:</Text></Box>
        <Text color="green">{p50.toFixed(2)}{unit}</Text>
      </Box>
      <Box>
        <Box width={12}><Text dimColor>p90:</Text></Box>
        <Text color="yellow">{p90.toFixed(2)}{unit}</Text>
      </Box>
      <Box>
        <Box width={12}><Text dimColor>p99:</Text></Box>
        <Text color="red">{p99.toFixed(2)}{unit}</Text>
      </Box>

      {showHistogram && (
        <Box marginTop={1}>
          <Histogram
            data={data}
            height={4}
            style="bars"
            showLabels={false}
            showCounts={false}
            percentileMarkers={[]}
          />
        </Box>
      )}
    </Box>
  );
};

export default Histogram;

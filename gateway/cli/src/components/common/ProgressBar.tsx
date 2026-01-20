/**
 * WaaV CLI Progress Bar Component
 *
 * Animated progress bar with various styles
 */

import React from 'react';
import { Text, Box } from 'ink';
import { GRADIENTS } from '../branding/animations.js';

/**
 * Progress bar style variants
 */
export type ProgressBarStyle = 'default' | 'blocks' | 'gradient' | 'minimal' | 'fancy';

/**
 * Progress bar props
 */
interface ProgressBarProps {
  progress: number; // 0-1
  width?: number;
  style?: ProgressBarStyle;
  showPercent?: boolean;
  showLabel?: boolean;
  label?: string;
  color?: string;
}

/**
 * Get progress bar characters for style
 */
function getBarChars(style: ProgressBarStyle): {
  filled: string;
  empty: string;
  leftCap: string;
  rightCap: string;
} {
  switch (style) {
    case 'blocks':
      return { filled: '█', empty: '░', leftCap: '', rightCap: '' };
    case 'minimal':
      return { filled: '━', empty: '─', leftCap: '', rightCap: '' };
    case 'fancy':
      return { filled: '▓', empty: '░', leftCap: '┃', rightCap: '┃' };
    case 'gradient':
      return { filled: '█', empty: '░', leftCap: '', rightCap: '' };
    case 'default':
    default:
      return { filled: '█', empty: '░', leftCap: '[', rightCap: ']' };
  }
}

/**
 * Progress Bar Component
 */
export const ProgressBar: React.FC<ProgressBarProps> = ({
  progress,
  width = 30,
  style = 'default',
  showPercent = true,
  showLabel = false,
  label = '',
  color = 'green',
}) => {
  // Clamp progress between 0 and 1
  const clampedProgress = Math.max(0, Math.min(1, progress));
  const percent = Math.round(clampedProgress * 100);

  const { filled, empty, leftCap, rightCap } = getBarChars(style);
  const filledWidth = Math.round(clampedProgress * width);
  const emptyWidth = width - filledWidth;

  // Build the bar
  let bar: React.ReactNode;

  if (style === 'gradient' && filledWidth > 0) {
    // Apply gradient coloring
    const filledPart = GRADIENTS.waav(filled.repeat(filledWidth));
    const emptyPart = <Text dimColor>{empty.repeat(emptyWidth)}</Text>;

    bar = (
      <>
        <Text>{filledPart}</Text>
        {emptyPart}
      </>
    );
  } else {
    bar = (
      <>
        <Text color={color}>{filled.repeat(filledWidth)}</Text>
        <Text dimColor>{empty.repeat(emptyWidth)}</Text>
      </>
    );
  }

  return (
    <Box>
      {showLabel && label && (
        <Box marginRight={1}>
          <Text>{label}</Text>
        </Box>
      )}
      <Text dimColor>{leftCap}</Text>
      {bar}
      <Text dimColor>{rightCap}</Text>
      {showPercent && (
        <Box marginLeft={1}>
          <Text>{percent.toString().padStart(3, ' ')}%</Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * Indeterminate progress bar (animated)
 */
interface IndeterminateProgressProps {
  width?: number;
  style?: ProgressBarStyle;
  frame?: number;
  label?: string;
}

export const IndeterminateProgress: React.FC<IndeterminateProgressProps> = ({
  width = 30,
  style = 'default',
  frame = 0,
  label,
}) => {
  const { filled, empty, leftCap, rightCap } = getBarChars(style);
  const barWidth = 5;

  // Calculate position (bouncing animation)
  const maxPos = width - barWidth;
  const cycleLength = maxPos * 2;
  const pos = frame % cycleLength;
  const actualPos = pos < maxPos ? pos : cycleLength - pos;

  // Build the bar
  const beforeWidth = actualPos;
  const afterWidth = width - actualPos - barWidth;

  return (
    <Box>
      {label && (
        <Box marginRight={1}>
          <Text>{label}</Text>
        </Box>
      )}
      <Text dimColor>{leftCap}</Text>
      <Text dimColor>{empty.repeat(beforeWidth)}</Text>
      <Text color="cyan">{filled.repeat(barWidth)}</Text>
      <Text dimColor>{empty.repeat(Math.max(0, afterWidth))}</Text>
      <Text dimColor>{rightCap}</Text>
    </Box>
  );
};

/**
 * Step progress indicator (1/5, 2/5, etc.)
 */
interface StepProgressProps {
  current: number;
  total: number;
  showBar?: boolean;
  barWidth?: number;
  label?: string;
}

export const StepProgress: React.FC<StepProgressProps> = ({
  current,
  total,
  showBar = true,
  barWidth = 20,
  label,
}) => {
  const progress = current / total;

  return (
    <Box flexDirection="column">
      <Box>
        {label && (
          <Box marginRight={1}>
            <Text>{label}</Text>
          </Box>
        )}
        <Text>
          <Text color="cyan">{current}</Text>
          <Text dimColor>/</Text>
          <Text>{total}</Text>
        </Text>
      </Box>
      {showBar && (
        <Box marginTop={0}>
          <ProgressBar
            progress={progress}
            width={barWidth}
            showPercent={false}
            style="blocks"
          />
        </Box>
      )}
    </Box>
  );
};

/**
 * Download/Upload progress with speed and ETA
 */
interface TransferProgressProps {
  transferred: number; // bytes
  total: number; // bytes
  speed?: number; // bytes per second
  eta?: number; // seconds remaining
  label?: string;
  width?: number;
}

export const TransferProgress: React.FC<TransferProgressProps> = ({
  transferred,
  total,
  speed,
  eta,
  label = 'Downloading',
  width = 25,
}) => {
  const progress = total > 0 ? transferred / total : 0;

  // Format bytes
  const formatBytes = (bytes: number): string => {
    if (bytes < 1024) {
      return `${bytes} B`;
    }
    if (bytes < 1024 * 1024) {
      return `${(bytes / 1024).toFixed(1)} KB`;
    }
    if (bytes < 1024 * 1024 * 1024) {
      return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    }
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  };

  // Format time
  const formatTime = (seconds: number): string => {
    if (seconds < 60) {
      return `${Math.round(seconds)}s`;
    }
    if (seconds < 3600) {
      const mins = Math.floor(seconds / 60);
      const secs = Math.round(seconds % 60);
      return `${mins}m ${secs}s`;
    }
    const hours = Math.floor(seconds / 3600);
    const mins = Math.floor((seconds % 3600) / 60);
    return `${hours}h ${mins}m`;
  };

  return (
    <Box flexDirection="column">
      <Box>
        <Text>{label}</Text>
        <Text dimColor> {formatBytes(transferred)} / {formatBytes(total)}</Text>
      </Box>
      <Box marginTop={0}>
        <ProgressBar
          progress={progress}
          width={width}
          showPercent={true}
          style="gradient"
        />
      </Box>
      {(speed !== undefined || eta !== undefined) && (
        <Box marginTop={0}>
          {speed !== undefined && (
            <Text dimColor>{formatBytes(speed)}/s</Text>
          )}
          {eta !== undefined && speed !== undefined && (
            <Text dimColor> • </Text>
          )}
          {eta !== undefined && (
            <Text dimColor>ETA: {formatTime(eta)}</Text>
          )}
        </Box>
      )}
    </Box>
  );
};

export default ProgressBar;

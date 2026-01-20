/**
 * WaaV CLI Divider Component
 *
 * Visual separator lines for organizing content sections
 * with various styles, optional labels, and theming.
 */

import React from 'react';
import { Text, Box } from 'ink';

/**
 * Divider line characters
 */
const DIVIDER_CHARS = {
  thin: '\u2500',      // ─
  thick: '\u2501',     // ━
  double: '\u2550',    // ═
  dotted: '\u2508',    // ┈
  dashed: '\u254C',    // ╌
  wave: '\u223C',      // ∼
} as const;

export type DividerStyle = keyof typeof DIVIDER_CHARS;

/**
 * Divider props
 */
export interface DividerProps {
  /** Divider style */
  style?: DividerStyle;
  /** Width (number of characters or 'auto' for full width) */
  width?: number | 'auto';
  /** Color of the divider */
  color?: string;
  /** Dim the divider */
  dim?: boolean;
  /** Optional label in the center */
  label?: string;
  /** Label alignment */
  labelAlign?: 'left' | 'center' | 'right';
  /** Bold label text */
  boldLabel?: boolean;
  /** Margin above */
  marginTop?: number;
  /** Margin below */
  marginBottom?: number;
}

/**
 * Divider Component
 *
 * Horizontal line separator for visual content organization.
 */
export const Divider: React.FC<DividerProps> = ({
  style = 'thin',
  width = 60,
  color = 'gray',
  dim = true,
  label,
  labelAlign = 'center',
  boldLabel = false,
  marginTop = 0,
  marginBottom = 0,
}) => {
  const char = DIVIDER_CHARS[style];
  const effectiveWidth = typeof width === 'number' ? width : 60;

  // Simple divider without label
  if (!label) {
    return (
      <Box marginTop={marginTop} marginBottom={marginBottom}>
        <Text color={color} dimColor={dim}>
          {char.repeat(effectiveWidth)}
        </Text>
      </Box>
    );
  }

  // Divider with label
  const labelText = ` ${label} `;
  const labelLength = labelText.length;
  const remainingWidth = effectiveWidth - labelLength;

  if (remainingWidth < 4) {
    // Not enough space, just show label
    return (
      <Box marginTop={marginTop} marginBottom={marginBottom}>
        <Text color={color} bold={boldLabel}>{label}</Text>
      </Box>
    );
  }

  let leftWidth: number;
  let rightWidth: number;

  switch (labelAlign) {
    case 'left':
      leftWidth = 2;
      rightWidth = remainingWidth - 2;
      break;
    case 'right':
      leftWidth = remainingWidth - 2;
      rightWidth = 2;
      break;
    case 'center':
    default:
      leftWidth = Math.floor(remainingWidth / 2);
      rightWidth = remainingWidth - leftWidth;
      break;
  }

  return (
    <Box marginTop={marginTop} marginBottom={marginBottom}>
      <Text color={color} dimColor={dim}>
        {char.repeat(leftWidth)}
      </Text>
      <Text color={color} bold={boldLabel}>{labelText}</Text>
      <Text color={color} dimColor={dim}>
        {char.repeat(rightWidth)}
      </Text>
    </Box>
  );
};

/**
 * SectionDivider - Divider with section title
 */
export interface SectionDividerProps {
  /** Section title */
  title: string;
  /** Optional icon before title */
  icon?: string;
  /** Divider width */
  width?: number;
  /** Color */
  color?: string;
  /** Margin top */
  marginTop?: number;
  /** Margin bottom */
  marginBottom?: number;
}

export const SectionDivider: React.FC<SectionDividerProps> = ({
  title,
  icon,
  width = 60,
  color = 'cyan',
  marginTop = 1,
  marginBottom = 0,
}) => {
  const labelText = icon ? `${icon} ${title}` : title;

  return (
    <Divider
      label={labelText}
      labelAlign="left"
      boldLabel={true}
      width={width}
      color={color}
      dim={true}
      style="thin"
      marginTop={marginTop}
      marginBottom={marginBottom}
    />
  );
};

/**
 * Space - Simple vertical spacer
 */
export interface SpaceProps {
  /** Number of empty lines */
  lines?: number;
}

export const Space: React.FC<SpaceProps> = ({
  lines = 1,
}) => {
  return <Box height={lines} />;
};

/**
 * VerticalDivider - Vertical line separator for columns
 */
export interface VerticalDividerProps {
  /** Height in lines */
  height?: number;
  /** Character to use */
  char?: string;
  /** Color */
  color?: string;
  /** Dim */
  dim?: boolean;
  /** Margin left */
  marginLeft?: number;
  /** Margin right */
  marginRight?: number;
}

export const VerticalDivider: React.FC<VerticalDividerProps> = ({
  height = 1,
  char = '\u2502', // │
  color = 'gray',
  dim = true,
  marginLeft = 1,
  marginRight = 1,
}) => {
  const lines = Array(height).fill(char);

  return (
    <Box
      flexDirection="column"
      marginLeft={marginLeft}
      marginRight={marginRight}
    >
      {lines.map((line, i) => (
        <Text key={i} color={color} dimColor={dim}>{line}</Text>
      ))}
    </Box>
  );
};

/**
 * HeaderDivider - Full-width header with title
 */
export interface HeaderDividerProps {
  /** Header title */
  title: string;
  /** Optional icon */
  icon?: string;
  /** Right-aligned text */
  right?: string;
  /** Width */
  width?: number;
  /** Color */
  color?: string;
  /** Margin top */
  marginTop?: number;
  /** Margin bottom */
  marginBottom?: number;
}

export const HeaderDivider: React.FC<HeaderDividerProps> = ({
  title,
  icon,
  right,
  width = 60,
  color = 'cyan',
  marginTop = 0,
  marginBottom = 1,
}) => {
  const leftText = icon ? `${icon} ${title}` : title;
  const rightText = right ?? '';
  const middleWidth = width - leftText.length - rightText.length - 4;

  return (
    <Box flexDirection="column" marginTop={marginTop} marginBottom={marginBottom}>
      <Box>
        <Text color={color} bold>{leftText}</Text>
        <Text dimColor>{' '.repeat(Math.max(0, middleWidth))}</Text>
        {rightText && <Text dimColor>{rightText}</Text>}
      </Box>
      <Text color={color} dimColor>
        {DIVIDER_CHARS.thin.repeat(width)}
      </Text>
    </Box>
  );
};

/**
 * DoubleDivider - Double-line divider for major sections
 */
export interface DoubleDividerProps {
  /** Width */
  width?: number;
  /** Color */
  color?: string;
  /** Margin top */
  marginTop?: number;
  /** Margin bottom */
  marginBottom?: number;
}

export const DoubleDivider: React.FC<DoubleDividerProps> = ({
  width = 60,
  color = 'gray',
  marginTop = 0,
  marginBottom = 0,
}) => {
  return (
    <Box flexDirection="column" marginTop={marginTop} marginBottom={marginBottom}>
      <Text color={color} dimColor>
        {DIVIDER_CHARS.thin.repeat(width)}
      </Text>
      <Text color={color} dimColor>
        {DIVIDER_CHARS.thin.repeat(width)}
      </Text>
    </Box>
  );
};

export default Divider;

/**
 * WaaV CLI BoxFrame Component
 *
 * Enhanced box component with rounded corners, visual depth,
 * and styled variants for professional UI appearance.
 */

import React from 'react';
import { Text, Box } from 'ink';

/**
 * Box frame border characters
 */
const BORDER_CHARS = {
  rounded: {
    topLeft: '\u256D',     // ╭
    topRight: '\u256E',    // ╮
    bottomLeft: '\u2570',  // ╰
    bottomRight: '\u256F', // ╯
    horizontal: '\u2500',  // ─
    vertical: '\u2502',    // │
    headerSep: '\u251C',   // ├
    headerSepRight: '\u2524', // ┤
  },
  single: {
    topLeft: '\u250C',     // ┌
    topRight: '\u2510',    // ┐
    bottomLeft: '\u2514',  // └
    bottomRight: '\u2518', // ┘
    horizontal: '\u2500',  // ─
    vertical: '\u2502',    // │
    headerSep: '\u251C',   // ├
    headerSepRight: '\u2524', // ┤
  },
  double: {
    topLeft: '\u2554',     // ╔
    topRight: '\u2557',    // ╗
    bottomLeft: '\u255A',  // ╚
    bottomRight: '\u255D',  // ╝
    horizontal: '\u2550',  // ═
    vertical: '\u2551',    // ║
    headerSep: '\u2560',   // ╠
    headerSepRight: '\u2563', // ╣
  },
  heavy: {
    topLeft: '\u250F',     // ┏
    topRight: '\u2513',    // ┓
    bottomLeft: '\u2517',  // ┗
    bottomRight: '\u251B', // ┛
    horizontal: '\u2501',  // ━
    vertical: '\u2503',    // ┃
    headerSep: '\u2523',   // ┣
    headerSepRight: '\u252B', // ┫
  },
} as const;

/**
 * Variant color presets
 */
const VARIANT_COLORS = {
  default: { border: 'gray', title: 'white', icon: 'cyan' },
  accent: { border: 'cyan', title: 'cyan', icon: 'cyan' },
  primary: { border: 'cyan', title: 'cyan', icon: 'cyan' },
  success: { border: 'green', title: 'green', icon: 'green' },
  warning: { border: 'yellow', title: 'yellow', icon: 'yellow' },
  error: { border: 'red', title: 'red', icon: 'red' },
  info: { border: 'blue', title: 'blue', icon: 'blue' },
  magenta: { border: 'magenta', title: 'magenta', icon: 'magenta' },
} as const;

export type BoxFrameVariant = keyof typeof VARIANT_COLORS;
export type BoxFrameBorderStyle = keyof typeof BORDER_CHARS;

export interface BoxFrameProps {
  /** Title displayed in the header */
  title?: string;
  /** Icon displayed before the title */
  icon?: string;
  /** Secondary text displayed on the right of the header */
  subtitle?: string;
  /** Width of the box (default: auto/fill) */
  width?: number | 'auto' | '100%';
  /** Height of the box (default: auto) */
  height?: number | 'auto';
  /** Border style */
  borderStyle?: BoxFrameBorderStyle;
  /** Color variant */
  variant?: BoxFrameVariant;
  /** Custom border color (overrides variant) */
  borderColor?: string;
  /** Custom title color (overrides variant) */
  titleColor?: string;
  /** Padding inside the box */
  padding?: number;
  /** Horizontal padding */
  paddingX?: number;
  /** Vertical padding */
  paddingY?: number;
  /** Margin around the box */
  margin?: number;
  /** Horizontal margin */
  marginX?: number;
  /** Vertical margin */
  marginY?: number;
  /** Right margin */
  marginRight?: number;
  /** Bottom margin */
  marginBottom?: number;
  /** Show header separator line */
  showHeaderSeparator?: boolean;
  /** Dim the border */
  dimBorder?: boolean;
  /** Bold the title */
  boldTitle?: boolean;
  /** Children content */
  children?: React.ReactNode;
}

/**
 * BoxFrame Component
 *
 * Creates a visually appealing frame with customizable borders,
 * title bar, and styling variants.
 */
export const BoxFrame: React.FC<BoxFrameProps> = ({
  title,
  icon,
  subtitle,
  width = 'auto',
  height = 'auto',
  borderStyle = 'rounded',
  variant = 'default',
  borderColor,
  titleColor,
  padding,
  paddingX = 1,
  paddingY = 0,
  margin,
  marginX,
  marginY,
  marginRight,
  marginBottom,
  showHeaderSeparator = true,
  dimBorder = false,
  boldTitle = true,
  children,
}) => {
  const chars = BORDER_CHARS[borderStyle];
  const colors = VARIANT_COLORS[variant];
  const effectiveBorderColor = borderColor ?? colors.border;
  const effectiveTitleColor = titleColor ?? colors.title;

  // Calculate content width for line rendering
  const minContentWidth = 20;
  const titleLength = (icon ? icon.length + 1 : 0) + (title?.length ?? 0);
  const subtitleLength = subtitle ? subtitle.length + 2 : 0;
  const headerContentWidth = titleLength + subtitleLength;

  // For auto width, we use a reasonable default
  // For explicit width, subtract border and padding
  const contentWidth = typeof width === 'number'
    ? width - 2 - (paddingX * 2) // Subtract borders and padding
    : Math.max(minContentWidth, headerContentWidth + 4);

  // Resolve padding
  const effectivePaddingX = padding ?? paddingX;
  const effectivePaddingY = padding ?? paddingY;

  // Resolve margin
  const effectiveMarginX = margin ?? marginX ?? 0;
  const effectiveMarginY = margin ?? marginY ?? 0;
  const effectiveMarginRight = marginRight ?? effectiveMarginX;
  const effectiveMarginBottom = marginBottom ?? effectiveMarginY;

  // Build header text
  const headerText = title
    ? `${icon ? icon + ' ' : ''}${title}`
    : '';

  // Render top border with optional title
  const renderTopBorder = () => {
    if (title) {
      // Title in top border: ╭─ Title ────────╮
      const leftDash = chars.horizontal;
      const titleSection = ` ${headerText} `;
      const subtitleSection = subtitle ? `${subtitle} ` : '';
      const remainingWidth = contentWidth - titleSection.length - subtitleSection.length + (effectivePaddingX * 2);
      const rightDashes = chars.horizontal.repeat(Math.max(0, remainingWidth));

      return (
        <Text>
          <Text color={effectiveBorderColor} dimColor={dimBorder}>
            {chars.topLeft}{leftDash}
          </Text>
          <Text color={effectiveTitleColor} bold={boldTitle}>
            {titleSection}
          </Text>
          {subtitle && (
            <Text dimColor>{subtitleSection}</Text>
          )}
          <Text color={effectiveBorderColor} dimColor={dimBorder}>
            {rightDashes}{chars.topRight}
          </Text>
        </Text>
      );
    }

    // No title: ╭──────────────╮
    const fullLine = chars.horizontal.repeat(contentWidth + (effectivePaddingX * 2));
    return (
      <Text color={effectiveBorderColor} dimColor={dimBorder}>
        {chars.topLeft}{fullLine}{chars.topRight}
      </Text>
    );
  };

  // Render bottom border
  const renderBottomBorder = () => {
    const fullLine = chars.horizontal.repeat(contentWidth + (effectivePaddingX * 2));
    return (
      <Text color={effectiveBorderColor} dimColor={dimBorder}>
        {chars.bottomLeft}{fullLine}{chars.bottomRight}
      </Text>
    );
  };

  // Render header separator (optional line under title)
  const renderHeaderSeparator = () => {
    if (!showHeaderSeparator || !title) {
      return null;
    }
    const fullLine = chars.horizontal.repeat(contentWidth + (effectivePaddingX * 2));
    return (
      <Text color={effectiveBorderColor} dimColor={dimBorder}>
        {chars.headerSep}{fullLine}{chars.headerSepRight}
      </Text>
    );
  };

  return (
    <Box
      flexDirection="column"
      marginLeft={effectiveMarginX}
      marginRight={effectiveMarginRight}
      marginTop={effectiveMarginY}
      marginBottom={effectiveMarginBottom}
      width={typeof width === 'number' ? width : undefined}
      height={typeof height === 'number' ? height : undefined}
    >
      {/* Top border with title */}
      {renderTopBorder()}

      {/* Header separator */}
      {renderHeaderSeparator()}

      {/* Content area with side borders */}
      <Box flexDirection="row">
        <Text color={effectiveBorderColor} dimColor={dimBorder}>{chars.vertical}</Text>
        <Box
          flexDirection="column"
          paddingX={effectivePaddingX}
          paddingY={effectivePaddingY}
          flexGrow={1}
        >
          {children}
        </Box>
        <Text color={effectiveBorderColor} dimColor={dimBorder}>{chars.vertical}</Text>
      </Box>

      {/* Bottom border */}
      {renderBottomBorder()}
    </Box>
  );
};

/**
 * Mini card variant - compact box for status indicators
 */
export interface MiniCardProps {
  label: string;
  value: string | number;
  icon?: string;
  variant?: BoxFrameVariant;
  width?: number;
}

export const MiniCard: React.FC<MiniCardProps> = ({
  label,
  value,
  icon,
  variant = 'default',
  width = 20,
}) => {
  const colors = VARIANT_COLORS[variant];

  return (
    <BoxFrame
      borderStyle="rounded"
      variant={variant}
      width={width}
      showHeaderSeparator={false}
      paddingY={0}
    >
      <Box flexDirection="column">
        <Text dimColor>{icon ? `${icon} ` : ''}{label}</Text>
        <Text bold color={colors.title}>{value}</Text>
      </Box>
    </BoxFrame>
  );
};

/**
 * Panel variant - larger box for content sections
 */
export interface PanelProps {
  title: string;
  icon?: string;
  subtitle?: string;
  variant?: BoxFrameVariant;
  width?: number | 'auto';
  children?: React.ReactNode;
}

export const Panel: React.FC<PanelProps> = ({
  title,
  icon,
  subtitle,
  variant = 'default',
  width = 'auto',
  children,
}) => {
  return (
    <BoxFrame
      title={title}
      icon={icon}
      subtitle={subtitle}
      borderStyle="rounded"
      variant={variant}
      width={width}
      paddingX={2}
      paddingY={1}
      showHeaderSeparator={false}
      boldTitle={true}
    >
      {children}
    </BoxFrame>
  );
};

export default BoxFrame;

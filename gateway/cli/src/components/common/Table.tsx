/**
 * WaaV CLI Table Component
 *
 * Formatted table display with various styles
 */

import React from 'react';
import { Text, Box } from 'ink';

/**
 * Table column definition
 */
export interface TableColumn<T = Record<string, unknown>> {
  key: keyof T | string;
  header: string;
  width?: number;
  align?: 'left' | 'center' | 'right';
  render?: (value: unknown, row: T) => React.ReactNode;
}

/**
 * Table border style
 */
export type TableBorderStyle = 'none' | 'single' | 'double' | 'rounded' | 'bold';

/**
 * Table props
 */
interface TableProps<T = Record<string, unknown>> {
  data: T[];
  columns: TableColumn<T>[];
  borderStyle?: TableBorderStyle;
  showHeader?: boolean;
  maxRows?: number;
  emptyMessage?: string;
  striped?: boolean;
  highlightRow?: number;
}

/**
 * Border characters for different styles
 */
const BORDER_CHARS: Record<TableBorderStyle, {
  topLeft: string;
  topRight: string;
  bottomLeft: string;
  bottomRight: string;
  horizontal: string;
  vertical: string;
  topJoint: string;
  bottomJoint: string;
  leftJoint: string;
  rightJoint: string;
  cross: string;
}> = {
  none: {
    topLeft: '',
    topRight: '',
    bottomLeft: '',
    bottomRight: '',
    horizontal: '',
    vertical: ' ',
    topJoint: '',
    bottomJoint: '',
    leftJoint: '',
    rightJoint: '',
    cross: ' ',
  },
  single: {
    topLeft: '┌',
    topRight: '┐',
    bottomLeft: '└',
    bottomRight: '┘',
    horizontal: '─',
    vertical: '│',
    topJoint: '┬',
    bottomJoint: '┴',
    leftJoint: '├',
    rightJoint: '┤',
    cross: '┼',
  },
  double: {
    topLeft: '╔',
    topRight: '╗',
    bottomLeft: '╚',
    bottomRight: '╝',
    horizontal: '═',
    vertical: '║',
    topJoint: '╦',
    bottomJoint: '╩',
    leftJoint: '╠',
    rightJoint: '╣',
    cross: '╬',
  },
  rounded: {
    topLeft: '╭',
    topRight: '╮',
    bottomLeft: '╰',
    bottomRight: '╯',
    horizontal: '─',
    vertical: '│',
    topJoint: '┬',
    bottomJoint: '┴',
    leftJoint: '├',
    rightJoint: '┤',
    cross: '┼',
  },
  bold: {
    topLeft: '┏',
    topRight: '┓',
    bottomLeft: '┗',
    bottomRight: '┛',
    horizontal: '━',
    vertical: '┃',
    topJoint: '┳',
    bottomJoint: '┻',
    leftJoint: '┣',
    rightJoint: '┫',
    cross: '╋',
  },
};

/**
 * Pad string to width with alignment
 */
function padCell(content: string, width: number, align: 'left' | 'center' | 'right' = 'left'): string {
  if (content.length >= width) {
    return content.slice(0, width);
  }

  const padding = width - content.length;

  switch (align) {
    case 'right':
      return ' '.repeat(padding) + content;
    case 'center': {
      const leftPad = Math.floor(padding / 2);
      const rightPad = padding - leftPad;
      return ' '.repeat(leftPad) + content + ' '.repeat(rightPad);
    }
    case 'left':
    default:
      return content + ' '.repeat(padding);
  }
}

/**
 * Table Component
 */
export function Table<T = Record<string, unknown>>({
  data,
  columns,
  borderStyle = 'single',
  showHeader = true,
  maxRows,
  emptyMessage = 'No data',
  striped = false,
  highlightRow,
}: TableProps<T>): React.ReactElement {
  const chars = BORDER_CHARS[borderStyle];

  // Calculate column widths
  const columnWidths = columns.map(col => {
    if (col.width) {
      return col.width;
    }

    // Auto-calculate based on content
    const headerWidth = col.header.length;
    const maxContentWidth = data.reduce((max, row) => {
      const value = (row as Record<string, unknown>)[col.key as string];
      const content = String(value ?? '');
      return Math.max(max, content.length);
    }, 0);

    return Math.max(headerWidth, maxContentWidth, 4);
  });

  // Render horizontal border
  const renderBorder = (left: string, joint: string, right: string) => {
    if (borderStyle === 'none') {
      return null;
    }

    const line = columnWidths
      .map(w => chars.horizontal.repeat(w + 2))
      .join(joint);

    return (
      <Text dimColor>
        {left}{line}{right}
      </Text>
    );
  };

  // Render row
  const renderRow = (
    cells: React.ReactNode[],
    isHeader = false,
    rowIndex?: number
  ) => {
    const isHighlighted = rowIndex !== undefined && rowIndex === highlightRow;
    const isStriped = striped && rowIndex !== undefined && rowIndex % 2 === 1;

    return (
      <Box>
        {borderStyle !== 'none' && <Text dimColor>{chars.vertical}</Text>}
        {cells.map((cell, i) => (
          <React.Fragment key={i}>
            <Text
              bold={isHeader}
              color={isHighlighted ? 'cyan' : undefined}
              dimColor={isStriped && !isHighlighted}
            >
              {' '}{cell}{' '}
            </Text>
            {i < cells.length - 1 && borderStyle !== 'none' && (
              <Text dimColor>{chars.vertical}</Text>
            )}
          </React.Fragment>
        ))}
        {borderStyle !== 'none' && <Text dimColor>{chars.vertical}</Text>}
      </Box>
    );
  };

  // Get visible data
  const visibleData = maxRows ? data.slice(0, maxRows) : data;

  if (data.length === 0) {
    return (
      <Box>
        <Text dimColor>{emptyMessage}</Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      {/* Top border */}
      {renderBorder(chars.topLeft, chars.topJoint, chars.topRight)}

      {/* Header */}
      {showHeader && (
        <>
          {renderRow(
            columns.map((col, i) =>
              padCell(col.header, columnWidths[i] ?? 10, col.align)
            ),
            true
          )}
          {renderBorder(chars.leftJoint, chars.cross, chars.rightJoint)}
        </>
      )}

      {/* Data rows */}
      {visibleData.map((row, rowIndex) => {
        const cells = columns.map((col, colIndex) => {
          const value = (row as Record<string, unknown>)[col.key as string];

          if (col.render) {
            const rendered = col.render(value, row);
            if (typeof rendered === 'string') {
              return padCell(rendered, columnWidths[colIndex] ?? 10, col.align);
            }
            return rendered;
          }

          const content = String(value ?? '');
          return padCell(content, columnWidths[colIndex] ?? 10, col.align);
        });

        return (
          <React.Fragment key={rowIndex}>
            {renderRow(cells, false, rowIndex)}
          </React.Fragment>
        );
      })}

      {/* Bottom border */}
      {renderBorder(chars.bottomLeft, chars.bottomJoint, chars.bottomRight)}

      {/* More rows indicator */}
      {maxRows && data.length > maxRows && (
        <Box>
          <Text dimColor>... and {data.length - maxRows} more rows</Text>
        </Box>
      )}
    </Box>
  );
}

/**
 * Simple key-value table
 */
interface KeyValueTableProps {
  data: Record<string, React.ReactNode>;
  keyWidth?: number;
  borderStyle?: TableBorderStyle;
}

export const KeyValueTable: React.FC<KeyValueTableProps> = ({
  data,
  keyWidth = 15,
  borderStyle: _borderStyle = 'none',
}) => {
  const entries = Object.entries(data);

  return (
    <Box flexDirection="column">
      {entries.map(([key, value]) => (
        <Box key={key}>
          <Box width={keyWidth}>
            <Text bold>{key}:</Text>
          </Box>
          <Text>{value}</Text>
        </Box>
      ))}
    </Box>
  );
};

/**
 * Status table for showing provider/service status
 */
interface StatusItem {
  name: string;
  status: 'healthy' | 'degraded' | 'unhealthy' | 'unknown';
  message?: string;
}

interface StatusTableProps {
  items: StatusItem[];
  title?: string;
}

export const StatusTable: React.FC<StatusTableProps> = ({ items, title }) => {
  const getStatusIndicator = (status: StatusItem['status']): React.ReactNode => {
    switch (status) {
      case 'healthy':
        return <Text color="green">●</Text>;
      case 'degraded':
        return <Text color="yellow">●</Text>;
      case 'unhealthy':
        return <Text color="red">●</Text>;
      default:
        return <Text dimColor>○</Text>;
    }
  };

  return (
    <Box flexDirection="column">
      {title && (
        <Box marginBottom={1}>
          <Text bold>{title}</Text>
        </Box>
      )}
      {items.map(item => (
        <Box key={item.name}>
          {getStatusIndicator(item.status)}
          <Text> {item.name}</Text>
          {item.message && <Text dimColor> - {item.message}</Text>}
        </Box>
      ))}
    </Box>
  );
};

export default Table;

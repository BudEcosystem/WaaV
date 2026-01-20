/**
 * WaaV CLI SortableTable Component
 *
 * Interactive table with sortable columns and keyboard navigation (htop style)
 * Supports ascending/descending sort, column highlighting, and pagination
 */

import React, { useState, useCallback, useMemo } from 'react';
import { Text, Box, useInput } from 'ink';

/**
 * Sort direction
 */
export type SortDirection = 'asc' | 'desc' | null;

/**
 * Sortable column definition
 */
export interface SortableColumn<T = Record<string, unknown>> {
  /** Unique key for the column */
  key: keyof T | string;
  /** Column header text */
  header: string;
  /** Column width in characters */
  width?: number;
  /** Text alignment */
  align?: 'left' | 'center' | 'right';
  /** Whether column is sortable (default: true) */
  sortable?: boolean;
  /** Custom sort comparator */
  comparator?: (a: T, b: T) => number;
  /** Custom cell renderer */
  render?: (value: unknown, row: T, isSelected: boolean) => React.ReactNode;
  /** Sort key shortcut (1-9) */
  shortcut?: number;
}

/**
 * SortableTable border style
 */
export type SortableTableBorderStyle = 'none' | 'single' | 'double' | 'rounded' | 'bold';

/**
 * SortableTable component props
 */
export interface SortableTableProps<T = Record<string, unknown>> {
  /** Array of data rows */
  data: T[];
  /** Column definitions */
  columns: SortableColumn<T>[];
  /** Border style */
  borderStyle?: SortableTableBorderStyle;
  /** Show header row */
  showHeader?: boolean;
  /** Maximum visible rows (pagination) */
  maxRows?: number;
  /** Currently selected row index */
  selectedRow?: number;
  /** Selection change callback */
  onSelectionChange?: (index: number) => void;
  /** Initial sort column key */
  initialSortColumn?: string;
  /** Initial sort direction */
  initialSortDirection?: SortDirection;
  /** Sort change callback */
  onSortChange?: (column: string, direction: SortDirection) => void;
  /** Row select callback (Enter key) */
  onRowSelect?: (row: T, index: number) => void;
  /** Striped rows */
  striped?: boolean;
  /** Empty state message */
  emptyMessage?: string;
  /** Whether component is focused */
  isFocused?: boolean;
  /** Show keyboard shortcuts in header */
  showShortcuts?: boolean;
  /** Show sort indicator in header */
  showSortIndicator?: boolean;
  /** Title for the table */
  title?: string;
  /** Unique row key extractor */
  getRowKey?: (row: T, index: number) => string;
}

/**
 * Border characters for different styles
 */
const BORDER_CHARS: Record<SortableTableBorderStyle, {
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
    topLeft: '', topRight: '', bottomLeft: '', bottomRight: '',
    horizontal: '', vertical: ' ',
    topJoint: '', bottomJoint: '', leftJoint: '', rightJoint: '', cross: ' ',
  },
  single: {
    topLeft: '┌', topRight: '┐', bottomLeft: '└', bottomRight: '┘',
    horizontal: '─', vertical: '│',
    topJoint: '┬', bottomJoint: '┴', leftJoint: '├', rightJoint: '┤', cross: '┼',
  },
  double: {
    topLeft: '╔', topRight: '╗', bottomLeft: '╚', bottomRight: '╝',
    horizontal: '═', vertical: '║',
    topJoint: '╦', bottomJoint: '╩', leftJoint: '╠', rightJoint: '╣', cross: '╬',
  },
  rounded: {
    topLeft: '╭', topRight: '╮', bottomLeft: '╰', bottomRight: '╯',
    horizontal: '─', vertical: '│',
    topJoint: '┬', bottomJoint: '┴', leftJoint: '├', rightJoint: '┤', cross: '┼',
  },
  bold: {
    topLeft: '┏', topRight: '┓', bottomLeft: '┗', bottomRight: '┛',
    horizontal: '━', vertical: '┃',
    topJoint: '┳', bottomJoint: '┻', leftJoint: '┣', rightJoint: '┫', cross: '╋',
  },
};

/**
 * Sort indicators
 */
const SORT_INDICATORS = {
  asc: '▲',
  desc: '▼',
  none: '○',
};

/**
 * Pad string to width with alignment
 */
function padCell(content: string, width: number, align: 'left' | 'center' | 'right' = 'left'): string {
  // Handle ANSI codes by getting visible length
  // eslint-disable-next-line no-control-regex
  const visibleLength = content.replace(/\x1b\[[0-9;]*m/g, '').length;

  if (visibleLength >= width) {
    return content.slice(0, width);
  }

  const padding = width - visibleLength;

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
 * Default comparator for sorting
 */
function defaultComparator<T>(a: T, b: T, key: string): number {
  const aVal = (a as Record<string, unknown>)[key];
  const bVal = (b as Record<string, unknown>)[key];

  // Handle null/undefined
  if (aVal == null && bVal == null) return 0;
  if (aVal == null) return 1;
  if (bVal == null) return -1;

  // Numeric comparison
  if (typeof aVal === 'number' && typeof bVal === 'number') {
    return aVal - bVal;
  }

  // String comparison
  return String(aVal).localeCompare(String(bVal));
}

/**
 * SortableTable Component
 *
 * An interactive table with sortable columns and keyboard navigation.
 * Press 1-9 to sort by column, s to toggle sort direction.
 *
 * @example
 * ```tsx
 * <SortableTable
 *   data={providers}
 *   columns={[
 *     { key: 'name', header: 'Name', width: 20, shortcut: 1 },
 *     { key: 'status', header: 'Status', width: 10, shortcut: 2 },
 *     { key: 'latency', header: 'Latency', width: 10, shortcut: 3 },
 *   ]}
 *   showShortcuts
 * />
 * ```
 */
export function SortableTable<T extends Record<string, unknown>>({
  data,
  columns,
  borderStyle = 'single',
  showHeader = true,
  maxRows,
  selectedRow: controlledSelectedRow,
  onSelectionChange,
  initialSortColumn,
  initialSortDirection = 'asc',
  onSortChange,
  onRowSelect,
  striped = false,
  emptyMessage = 'No data',
  isFocused = true,
  showShortcuts = true,
  showSortIndicator = true,
  title,
  getRowKey,
}: SortableTableProps<T>): React.ReactElement {
  const chars = BORDER_CHARS[borderStyle];

  // Internal state
  const [sortColumn, setSortColumn] = useState<string | null>(initialSortColumn ?? null);
  const [sortDirection, setSortDirection] = useState<SortDirection>(
    initialSortColumn ? initialSortDirection : null
  );
  const [internalSelectedRow, setInternalSelectedRow] = useState(0);
  const [scrollOffset, setScrollOffset] = useState(0);

  // Use controlled or internal selection
  const selectedRowIndex = controlledSelectedRow ?? internalSelectedRow;

  // Calculate column widths
  const columnWidths = useMemo(() => {
    return columns.map(col => {
      if (col.width) return col.width;

      const headerWidth = col.header.length + (showShortcuts && col.shortcut ? 4 : 0) +
        (showSortIndicator && col.sortable !== false ? 2 : 0);

      const maxContentWidth = data.reduce((max, row) => {
        const value = row[col.key as keyof T];
        const content = String(value ?? '');
        return Math.max(max, content.length);
      }, 0);

      return Math.max(headerWidth, maxContentWidth, 4);
    });
  }, [columns, data, showShortcuts, showSortIndicator]);

  // Sort data
  const sortedData = useMemo(() => {
    if (!sortColumn || !sortDirection) return data;

    const column = columns.find(c => String(c.key) === sortColumn);
    if (!column) return data;

    const comparator = column.comparator ?? ((a, b) => defaultComparator(a, b, sortColumn));

    return [...data].sort((a, b) => {
      const result = comparator(a, b);
      return sortDirection === 'desc' ? -result : result;
    });
  }, [data, sortColumn, sortDirection, columns]);

  // Get visible rows
  const visibleRows = maxRows ? sortedData.slice(scrollOffset, scrollOffset + maxRows) : sortedData;

  // Handle sort toggle
  const handleSort = useCallback((columnKey: string) => {
    const column = columns.find(c => String(c.key) === columnKey);
    if (!column || column.sortable === false) return;

    let newDirection: SortDirection;

    if (sortColumn === columnKey) {
      // Toggle direction
      newDirection = sortDirection === 'asc' ? 'desc' : sortDirection === 'desc' ? null : 'asc';
    } else {
      newDirection = 'asc';
    }

    setSortColumn(newDirection ? columnKey : null);
    setSortDirection(newDirection);

    if (onSortChange) {
      onSortChange(columnKey, newDirection);
    }
  }, [columns, sortColumn, sortDirection, onSortChange]);

  // Handle row selection
  const handleSelectionChange = useCallback((newIndex: number) => {
    const clampedIndex = Math.max(0, Math.min(sortedData.length - 1, newIndex));

    if (onSelectionChange) {
      onSelectionChange(clampedIndex);
    } else {
      setInternalSelectedRow(clampedIndex);
    }

    // Adjust scroll for pagination
    if (maxRows) {
      if (clampedIndex < scrollOffset) {
        setScrollOffset(clampedIndex);
      } else if (clampedIndex >= scrollOffset + maxRows) {
        setScrollOffset(clampedIndex - maxRows + 1);
      }
    }
  }, [sortedData.length, maxRows, scrollOffset, onSelectionChange]);

  // Keyboard input handling
  useInput(
    (input, key) => {
      if (!isFocused) return;

      // Navigation
      if (key.upArrow || input === 'k') {
        handleSelectionChange(selectedRowIndex - 1);
        return;
      }

      if (key.downArrow || input === 'j') {
        handleSelectionChange(selectedRowIndex + 1);
        return;
      }

      if (key.pageUp) {
        handleSelectionChange(selectedRowIndex - (maxRows ?? 10));
        return;
      }

      if (key.pageDown) {
        handleSelectionChange(selectedRowIndex + (maxRows ?? 10));
        return;
      }

      // Home/End
      if (input === 'g' && !key.shift) {
        handleSelectionChange(0);
        return;
      }

      if (input === 'G' || (input === 'g' && key.shift)) {
        handleSelectionChange(sortedData.length - 1);
        return;
      }

      // Select row
      if (key.return && onRowSelect && sortedData[selectedRowIndex]) {
        onRowSelect(sortedData[selectedRowIndex], selectedRowIndex);
        return;
      }

      // Sort by column shortcut (1-9)
      const shortcutNum = parseInt(input, 10);
      if (shortcutNum >= 1 && shortcutNum <= 9) {
        const column = columns.find(c => c.shortcut === shortcutNum);
        if (column) {
          handleSort(String(column.key));
        }
        return;
      }

      // Toggle sort direction
      if (input === 's' && sortColumn) {
        handleSort(sortColumn);
        return;
      }

      // Reverse sort
      if (input === 'r' && sortColumn && sortDirection) {
        setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc');
        return;
      }
    },
    { isActive: isFocused }
  );

  // Render horizontal border
  const renderBorder = (left: string, joint: string, right: string) => {
    if (borderStyle === 'none') return null;

    const line = columnWidths
      .map(w => chars.horizontal.repeat(w + 2))
      .join(joint);

    return (
      <Text dimColor>
        {left}{line}{right}
      </Text>
    );
  };

  // Render header cell
  const renderHeaderCell = (col: SortableColumn<T>, width: number, _index: number) => {
    const isSorted = sortColumn === String(col.key);
    const sortIndicator = isSorted && sortDirection
      ? SORT_INDICATORS[sortDirection]
      : showSortIndicator && col.sortable !== false
        ? SORT_INDICATORS.none
        : '';

    const shortcutLabel = showShortcuts && col.shortcut
      ? `[${col.shortcut}]`
      : '';

    const headerText = `${col.header}${shortcutLabel}`;
    const fullHeader = showSortIndicator && col.sortable !== false
      ? `${headerText} ${sortIndicator}`
      : headerText;

    return (
      <Text
        key={String(col.key)}
        bold
        color={isSorted ? 'cyan' : undefined}
      >
        {' '}{padCell(fullHeader, width, col.align)}{' '}
      </Text>
    );
  };

  // Render data cell
  const renderDataCell = (
    row: T,
    col: SortableColumn<T>,
    width: number,
    isSelected: boolean
  ) => {
    const value = row[col.key as keyof T];

    if (col.render) {
      const rendered = col.render(value, row, isSelected);
      if (typeof rendered === 'string') {
        return padCell(rendered, width, col.align);
      }
      return rendered;
    }

    return padCell(String(value ?? ''), width, col.align);
  };

  // Empty state
  if (data.length === 0) {
    return (
      <Box flexDirection="column">
        {title && (
          <Box marginBottom={1}>
            <Text bold>{title}</Text>
          </Box>
        )}
        <Box>
          <Text dimColor>{emptyMessage}</Text>
        </Box>
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      {/* Title */}
      {title && (
        <Box marginBottom={1}>
          <Text bold>{title}</Text>
          {sortColumn && (
            <Text dimColor>
              {' '}(sorted by {sortColumn} {sortDirection === 'asc' ? '▲' : '▼'})
            </Text>
          )}
        </Box>
      )}

      {/* Top border */}
      {renderBorder(chars.topLeft, chars.topJoint, chars.topRight)}

      {/* Header */}
      {showHeader && (
        <>
          <Box>
            {borderStyle !== 'none' && <Text dimColor>{chars.vertical}</Text>}
            {columns.map((col, i) => (
              <React.Fragment key={String(col.key)}>
                {renderHeaderCell(col, columnWidths[i] ?? 10, i)}
                {i < columns.length - 1 && borderStyle !== 'none' && (
                  <Text dimColor>{chars.vertical}</Text>
                )}
              </React.Fragment>
            ))}
            {borderStyle !== 'none' && <Text dimColor>{chars.vertical}</Text>}
          </Box>
          {renderBorder(chars.leftJoint, chars.cross, chars.rightJoint)}
        </>
      )}

      {/* Data rows */}
      {visibleRows.map((row, visibleIndex) => {
        const actualIndex = scrollOffset + visibleIndex;
        const isSelected = actualIndex === selectedRowIndex;
        const isStriped = striped && actualIndex % 2 === 1;
        const rowKey = getRowKey ? getRowKey(row, actualIndex) : String(actualIndex);

        return (
          <Box key={rowKey}>
            {borderStyle !== 'none' && <Text dimColor>{chars.vertical}</Text>}
            {columns.map((col, colIndex) => (
              <React.Fragment key={String(col.key)}>
                <Text
                  color={isSelected ? 'cyan' : undefined}
                  dimColor={isStriped && !isSelected}
                  inverse={isSelected}
                >
                  {' '}{renderDataCell(row, col, columnWidths[colIndex] ?? 10, isSelected)}{' '}
                </Text>
                {colIndex < columns.length - 1 && borderStyle !== 'none' && (
                  <Text dimColor>{chars.vertical}</Text>
                )}
              </React.Fragment>
            ))}
            {borderStyle !== 'none' && <Text dimColor>{chars.vertical}</Text>}
          </Box>
        );
      })}

      {/* Bottom border */}
      {renderBorder(chars.bottomLeft, chars.bottomJoint, chars.bottomRight)}

      {/* Pagination indicator */}
      {maxRows && sortedData.length > maxRows && (
        <Box marginTop={1}>
          <Text dimColor>
            Showing {scrollOffset + 1}-{Math.min(scrollOffset + maxRows, sortedData.length)} of {sortedData.length}
          </Text>
        </Box>
      )}

      {/* Help */}
      <Box marginTop={1}>
        <Text dimColor>
          [↑↓/jk]Navigate [1-9]Sort by column [s]Toggle sort [r]Reverse [Enter]Select
        </Text>
      </Box>
    </Box>
  );
}

/**
 * Compact provider table for dashboard
 */
export interface ProviderTableRow {
  [key: string]: string | number;
  name: string;
  type: 'stt' | 'tts' | 'realtime';
  status: 'online' | 'offline' | 'degraded';
  latency: number;
  requests: number;
  errors: number;
}

export interface ProviderTableProps {
  providers: ProviderTableRow[];
  onSelect?: (provider: ProviderTableRow) => void;
}

export const ProviderTable: React.FC<ProviderTableProps> = ({
  providers,
  onSelect,
}) => {
  const columns: SortableColumn<ProviderTableRow>[] = [
    {
      key: 'name',
      header: 'Provider',
      width: 16,
      shortcut: 1,
    },
    {
      key: 'type',
      header: 'Type',
      width: 10,
      shortcut: 2,
      render: (value) => {
        const typeColors: Record<string, string> = {
          stt: 'blue',
          tts: 'green',
          realtime: 'magenta',
        };
        return (
          <Text color={typeColors[value as string] as Parameters<typeof Text>[0]['color']}>
            {String(value).toUpperCase()}
          </Text>
        );
      },
    },
    {
      key: 'status',
      header: 'Status',
      width: 10,
      shortcut: 3,
      render: (value) => {
        const statusIndicators: Record<string, { char: string; color: string }> = {
          online: { char: '●', color: 'green' },
          offline: { char: '●', color: 'red' },
          degraded: { char: '●', color: 'yellow' },
        };
        const statusValue = String(value);
        const indicator = statusIndicators[statusValue] ?? { char: '○', color: 'gray' };
        return (
          <Text color={indicator.color as Parameters<typeof Text>[0]['color']}>
            {indicator.char} {statusValue}
          </Text>
        );
      },
    },
    {
      key: 'latency',
      header: 'Latency',
      width: 10,
      align: 'right',
      shortcut: 4,
      render: (value) => {
        const latency = Number(value);
        return (
          <Text color={latency > 100 ? 'red' : latency > 50 ? 'yellow' : 'green'}>
            {latency}ms
          </Text>
        );
      },
    },
    {
      key: 'requests',
      header: 'Req/min',
      width: 10,
      align: 'right',
      shortcut: 5,
    },
    {
      key: 'errors',
      header: 'Errors',
      width: 8,
      align: 'right',
      shortcut: 6,
      render: (value) => (
        <Text color={(value as number) > 0 ? 'red' : undefined}>
          {((value as number) * 100).toFixed(1)}%
        </Text>
      ),
    },
  ];

  return (
    <SortableTable
      data={providers}
      columns={columns}
      borderStyle="rounded"
      striped
      initialSortColumn="name"
      onRowSelect={onSelect}
      showShortcuts
      maxRows={10}
    />
  );
};

/**
 * Hook for managing sortable table state
 */
export function useSortableTable<T>(initialData: T[] = []) {
  const [data, setData] = useState<T[]>(initialData);
  const [sortColumn, setSortColumn] = useState<string | null>(null);
  const [sortDirection, setSortDirection] = useState<SortDirection>(null);
  const [selectedRow, setSelectedRow] = useState(0);

  const handleSortChange = useCallback((column: string, direction: SortDirection) => {
    setSortColumn(column);
    setSortDirection(direction);
  }, []);

  return {
    data,
    setData,
    sortColumn,
    sortDirection,
    selectedRow,
    setSelectedRow,
    handleSortChange,
  };
}

export default SortableTable;

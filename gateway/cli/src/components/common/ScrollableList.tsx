/**
 * WaaV CLI ScrollableList Component
 *
 * Paged list view with vim-style navigation (htop style)
 * Supports line numbers, search/filter, and status bar
 */

import React, { useState, useEffect, useCallback } from 'react';
import { Text, Box, useInput, useApp } from 'ink';

/**
 * Line item with optional metadata
 */
export interface ScrollableListItem {
  /** Unique identifier */
  id: string;
  /** Display content */
  content: string;
  /** Optional color for the line */
  color?: string;
  /** Optional metadata */
  meta?: Record<string, unknown>;
  /** Whether this line is highlighted */
  highlighted?: boolean;
  /** Timestamp for log-style lists */
  timestamp?: Date;
  /** Level/severity (for logs) */
  level?: 'debug' | 'info' | 'warn' | 'error' | 'trace';
}

/**
 * ScrollableList component props
 */
export interface ScrollableListProps {
  /** Array of items to display */
  items: ScrollableListItem[];
  /** Visible height (number of lines) */
  height?: number;
  /** Show line numbers */
  showLineNumbers?: boolean;
  /** Width of line number column */
  lineNumberWidth?: number;
  /** Enable search/filter mode */
  enableSearch?: boolean;
  /** Current search query (controlled) */
  searchQuery?: string;
  /** Search query change callback */
  onSearchChange?: (query: string) => void;
  /** Selected line index */
  selectedIndex?: number;
  /** Selection change callback */
  onSelectionChange?: (index: number) => void;
  /** Whether to follow new items (auto-scroll to bottom) */
  follow?: boolean;
  /** Follow mode change callback */
  onFollowChange?: (follow: boolean) => void;
  /** Show status bar at bottom */
  showStatusBar?: boolean;
  /** Title for the list */
  title?: string;
  /** Empty state message */
  emptyMessage?: string;
  /** Whether the list is focused */
  isFocused?: boolean;
  /** Callback when item is selected (Enter key) */
  onSelect?: (item: ScrollableListItem, index: number) => void;
  /** Custom item renderer */
  renderItem?: (item: ScrollableListItem, index: number, isSelected: boolean) => React.ReactNode;
  /** Enable vim-style navigation (j/k) */
  vimNavigation?: boolean;
}

/**
 * Get level color for log items
 */
function getLevelColor(level?: string): string | undefined {
  switch (level) {
    case 'error':
      return 'red';
    case 'warn':
      return 'yellow';
    case 'info':
      return 'cyan';
    case 'debug':
      return 'gray';
    default:
      return undefined;
  }
}

/**
 * Format timestamp for display
 */
function formatTimestamp(date: Date): string {
  const hours = date.getHours().toString().padStart(2, '0');
  const minutes = date.getMinutes().toString().padStart(2, '0');
  const seconds = date.getSeconds().toString().padStart(2, '0');
  const ms = date.getMilliseconds().toString().padStart(3, '0');
  return `${hours}:${minutes}:${seconds}.${ms}`;
}

/**
 * Highlight search matches in text
 */
function highlightMatches(text: string, query: string): React.ReactNode[] {
  if (!query) {
    return [<Text key="0">{text}</Text>];
  }

  const parts: React.ReactNode[] = [];
  const lowerText = text.toLowerCase();
  const lowerQuery = query.toLowerCase();
  let lastIndex = 0;
  let matchIndex = lowerText.indexOf(lowerQuery);
  let partKey = 0;

  while (matchIndex !== -1) {
    // Add text before match
    if (matchIndex > lastIndex) {
      parts.push(<Text key={partKey++}>{text.slice(lastIndex, matchIndex)}</Text>);
    }

    // Add highlighted match
    parts.push(
      <Text key={partKey++} backgroundColor="yellow" color="black">
        {text.slice(matchIndex, matchIndex + query.length)}
      </Text>
    );

    lastIndex = matchIndex + query.length;
    matchIndex = lowerText.indexOf(lowerQuery, lastIndex);
  }

  // Add remaining text
  if (lastIndex < text.length) {
    parts.push(<Text key={partKey++}>{text.slice(lastIndex)}</Text>);
  }

  return parts.length > 0 ? parts : [<Text key="0">{text}</Text>];
}

/**
 * ScrollableList Component
 *
 * A paged list view with keyboard navigation support.
 * Supports vim-style navigation, search/filter, and follow mode.
 *
 * @example
 * ```tsx
 * <ScrollableList
 *   items={logLines}
 *   height={20}
 *   showLineNumbers
 *   enableSearch
 *   vimNavigation
 * />
 * ```
 */
export const ScrollableList: React.FC<ScrollableListProps> = ({
  items,
  height = 20,
  showLineNumbers = false,
  lineNumberWidth = 4,
  enableSearch = false,
  searchQuery = '',
  onSearchChange,
  selectedIndex: controlledSelectedIndex,
  onSelectionChange,
  follow = false,
  onFollowChange,
  showStatusBar = true,
  title,
  emptyMessage = 'No items',
  isFocused = true,
  onSelect,
  renderItem,
  vimNavigation = true,
}) => {
  const { exit } = useApp();

  // Internal state for scroll position
  const [scrollOffset, setScrollOffset] = useState(0);
  const [internalSelectedIndex, setInternalSelectedIndex] = useState(0);
  const [internalFollow, setInternalFollow] = useState(follow);
  const [searchMode, setSearchMode] = useState(false);
  const [internalSearchQuery, setInternalSearchQuery] = useState(searchQuery);

  // Use controlled or internal state
  const selectedIndex = controlledSelectedIndex ?? internalSelectedIndex;
  const currentSearchQuery = onSearchChange ? searchQuery : internalSearchQuery;
  const isFollowing = onFollowChange ? follow : internalFollow;

  // Filter items based on search query
  const filteredItems = currentSearchQuery
    ? items.filter((item) =>
        item.content.toLowerCase().includes(currentSearchQuery.toLowerCase())
      )
    : items;

  // Calculate visible window
  const _maxScroll = Math.max(0, filteredItems.length - height);
  const visibleItems = filteredItems.slice(scrollOffset, scrollOffset + height);

  // Auto-scroll to bottom when following
  useEffect(() => {
    if (isFollowing && filteredItems.length > 0) {
      const newOffset = Math.max(0, filteredItems.length - height);
      setScrollOffset(newOffset);
      const newSelectedIndex = filteredItems.length - 1;
      if (onSelectionChange) {
        onSelectionChange(newSelectedIndex);
      } else {
        setInternalSelectedIndex(newSelectedIndex);
      }
    }
  }, [filteredItems.length, isFollowing, height, onSelectionChange]);

  // Handle selection change
  const handleSelectionChange = useCallback(
    (newIndex: number) => {
      const clampedIndex = Math.max(0, Math.min(filteredItems.length - 1, newIndex));

      if (onSelectionChange) {
        onSelectionChange(clampedIndex);
      } else {
        setInternalSelectedIndex(clampedIndex);
      }

      // Adjust scroll to keep selection visible
      if (clampedIndex < scrollOffset) {
        setScrollOffset(clampedIndex);
      } else if (clampedIndex >= scrollOffset + height) {
        setScrollOffset(clampedIndex - height + 1);
      }

      // Disable follow mode when manually navigating
      if (isFollowing) {
        if (onFollowChange) {
          onFollowChange(false);
        } else {
          setInternalFollow(false);
        }
      }
    },
    [filteredItems.length, scrollOffset, height, isFollowing, onSelectionChange, onFollowChange]
  );

  // Handle keyboard input
  useInput(
    (input, key) => {
      if (!isFocused) return;

      // Search mode input
      if (searchMode) {
        if (key.escape) {
          setSearchMode(false);
          if (onSearchChange) {
            onSearchChange('');
          } else {
            setInternalSearchQuery('');
          }
          return;
        }
        if (key.return) {
          setSearchMode(false);
          return;
        }
        if (key.backspace || key.delete) {
          const newQuery = currentSearchQuery.slice(0, -1);
          if (onSearchChange) {
            onSearchChange(newQuery);
          } else {
            setInternalSearchQuery(newQuery);
          }
          return;
        }
        if (input && !key.ctrl && !key.meta) {
          const newQuery = currentSearchQuery + input;
          if (onSearchChange) {
            onSearchChange(newQuery);
          } else {
            setInternalSearchQuery(newQuery);
          }
        }
        return;
      }

      // Navigation keys
      if (key.upArrow || (vimNavigation && input === 'k')) {
        handleSelectionChange(selectedIndex - 1);
        return;
      }

      if (key.downArrow || (vimNavigation && input === 'j')) {
        handleSelectionChange(selectedIndex + 1);
        return;
      }

      if (key.pageUp || (vimNavigation && key.ctrl && input === 'u')) {
        handleSelectionChange(selectedIndex - Math.floor(height / 2));
        return;
      }

      if (key.pageDown || (vimNavigation && key.ctrl && input === 'd')) {
        handleSelectionChange(selectedIndex + Math.floor(height / 2));
        return;
      }

      // Home/End
      if (vimNavigation && input === 'g' && key.shift) {
        handleSelectionChange(filteredItems.length - 1);
        return;
      }

      if (vimNavigation && input === 'g') {
        handleSelectionChange(0);
        return;
      }

      // Toggle follow mode
      if (input === 'f' || input === 'F') {
        const newFollow = !isFollowing;
        if (onFollowChange) {
          onFollowChange(newFollow);
        } else {
          setInternalFollow(newFollow);
        }
        return;
      }

      // Search mode
      if (enableSearch && input === '/') {
        setSearchMode(true);
        return;
      }

      // Clear search
      if (enableSearch && key.escape) {
        if (onSearchChange) {
          onSearchChange('');
        } else {
          setInternalSearchQuery('');
        }
        return;
      }

      // Select item
      if (key.return && onSelect && filteredItems[selectedIndex]) {
        onSelect(filteredItems[selectedIndex], selectedIndex);
        return;
      }

      // Quit
      if (input === 'q') {
        exit();
      }
    },
    { isActive: isFocused }
  );

  // Count errors/warnings for status bar
  const errorCount = items.filter((item) => item.level === 'error').length;
  const warnCount = items.filter((item) => item.level === 'warn').length;

  return (
    <Box flexDirection="column">
      {/* Title bar */}
      {title && (
        <Box borderStyle="single" borderBottom={false} paddingX={1}>
          <Text bold>{title}</Text>
          {isFollowing && (
            <Text color="green"> [FOLLOW]</Text>
          )}
        </Box>
      )}

      {/* Search bar */}
      {searchMode && (
        <Box borderStyle="single" borderTop={false} borderBottom={false} paddingX={1}>
          <Text>/</Text>
          <Text color="yellow">{currentSearchQuery}</Text>
          <Text dimColor>▌</Text>
        </Box>
      )}

      {/* Filter indicator */}
      {!searchMode && currentSearchQuery && (
        <Box paddingX={1}>
          <Text dimColor>Filter: </Text>
          <Text color="yellow">{currentSearchQuery}</Text>
          <Text dimColor> ({filteredItems.length}/{items.length})</Text>
        </Box>
      )}

      {/* List content */}
      <Box flexDirection="column" height={height}>
        {filteredItems.length === 0 ? (
          <Box paddingX={1}>
            <Text dimColor>{emptyMessage}</Text>
          </Box>
        ) : (
          visibleItems.map((item, visibleIndex) => {
            const actualIndex = scrollOffset + visibleIndex;
            const isSelected = actualIndex === selectedIndex;
            const originalIndex = items.indexOf(item);

            // Custom renderer
            if (renderItem) {
              return (
                <Box key={item.id}>
                  {showLineNumbers && (
                    <Box width={lineNumberWidth} marginRight={1}>
                      <Text dimColor>
                        {(originalIndex + 1).toString().padStart(lineNumberWidth - 1, ' ')}
                      </Text>
                    </Box>
                  )}
                  {renderItem(item, actualIndex, isSelected)}
                </Box>
              );
            }

            // Default renderer
            const levelColor = getLevelColor(item.level);
            const lineColor = item.color ?? levelColor;

            return (
              <Box key={item.id}>
                {/* Line number */}
                {showLineNumbers && (
                  <Box width={lineNumberWidth} marginRight={1}>
                    <Text dimColor>
                      {(originalIndex + 1).toString().padStart(lineNumberWidth - 1, ' ')}
                    </Text>
                  </Box>
                )}

                {/* Selection indicator */}
                <Text color={isSelected ? 'cyan' : undefined}>
                  {isSelected ? '▶ ' : '  '}
                </Text>

                {/* Timestamp */}
                {item.timestamp && (
                  <Text dimColor>{formatTimestamp(item.timestamp)} </Text>
                )}

                {/* Level badge */}
                {item.level && (
                  <Text color={levelColor as Parameters<typeof Text>[0]['color']}>
                    [{item.level.toUpperCase().padEnd(5)}]
                  </Text>
                )}

                {/* Content */}
                <Text
                  color={lineColor as Parameters<typeof Text>[0]['color']}
                  inverse={isSelected}
                >
                  {' '}
                  {currentSearchQuery
                    ? highlightMatches(item.content, currentSearchQuery)
                    : item.content}
                </Text>
              </Box>
            );
          })
        )}
      </Box>

      {/* Status bar */}
      {showStatusBar && (
        <Box borderStyle="single" borderTop paddingX={1} justifyContent="space-between">
          <Box>
            <Text dimColor>Lines: </Text>
            <Text>{filteredItems.length}</Text>
            {errorCount > 0 && (
              <>
                <Text dimColor> | </Text>
                <Text color="red">Errors: {errorCount}</Text>
              </>
            )}
            {warnCount > 0 && (
              <>
                <Text dimColor> | </Text>
                <Text color="yellow">Warnings: {warnCount}</Text>
              </>
            )}
          </Box>
          <Box>
            <Text dimColor>
              {selectedIndex + 1}/{filteredItems.length}
              {' '}
              ({Math.round(((selectedIndex + 1) / filteredItems.length) * 100) || 0}%)
            </Text>
          </Box>
        </Box>
      )}

      {/* Help bar */}
      <Box paddingTop={1}>
        <Text dimColor>
          [↑↓/jk]Navigate [PgUp/PgDn]Page [F]ollow [/]Search [q]uit
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Log viewer specialized component
 */
export interface LogViewerProps {
  /** Array of log entries */
  logs: Array<{
    message: string;
    level: 'debug' | 'info' | 'warn' | 'error';
    timestamp: Date;
    source?: string;
  }>;
  /** Visible height */
  height?: number;
  /** Filter by log level */
  levelFilter?: Array<'debug' | 'info' | 'warn' | 'error'>;
  /** Filter by source */
  sourceFilter?: string;
  /** Follow mode */
  follow?: boolean;
}

export const LogViewer: React.FC<LogViewerProps> = ({
  logs,
  height = 20,
  levelFilter,
  sourceFilter,
  follow = true,
}) => {
  // Convert logs to ScrollableListItems
  const items: ScrollableListItem[] = logs
    .filter((log) => {
      if (levelFilter && !levelFilter.includes(log.level)) {
        return false;
      }
      if (sourceFilter && log.source && !log.source.includes(sourceFilter)) {
        return false;
      }
      return true;
    })
    .map((log, index) => ({
      id: `log-${index}`,
      content: log.source ? `[${log.source}] ${log.message}` : log.message,
      level: log.level,
      timestamp: log.timestamp,
    }));

  return (
    <ScrollableList
      items={items}
      height={height}
      showLineNumbers
      enableSearch
      follow={follow}
      showStatusBar
      title="Logs"
      vimNavigation
    />
  );
};

/**
 * Hook for managing scrollable list state
 */
export function useScrollableList(initialItems: ScrollableListItem[] = []) {
  const [items, setItems] = useState<ScrollableListItem[]>(initialItems);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [searchQuery, setSearchQuery] = useState('');
  const [follow, setFollow] = useState(false);

  const addItem = useCallback((item: ScrollableListItem) => {
    setItems((prev) => [...prev, item]);
  }, []);

  const clearItems = useCallback(() => {
    setItems([]);
    setSelectedIndex(0);
  }, []);

  const removeItem = useCallback((id: string) => {
    setItems((prev) => prev.filter((item) => item.id !== id));
  }, []);

  return {
    items,
    setItems,
    selectedIndex,
    setSelectedIndex,
    searchQuery,
    setSearchQuery,
    follow,
    setFollow,
    addItem,
    clearItems,
    removeItem,
  };
}

export default ScrollableList;

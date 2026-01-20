/**
 * WaaV CLI Select Component
 *
 * Interactive selection menu with keyboard navigation
 */

import React, { useState, useEffect, useMemo } from 'react';
import { Text, Box, useInput } from 'ink';

/**
 * Select item type
 */
export interface SelectItem<T = string> {
  value: T;
  label: string;
  description?: string;
  disabled?: boolean;
  icon?: string;
}

/**
 * Select component props
 */
interface SelectProps<T = string> {
  items: SelectItem<T>[];
  onSelect: (item: SelectItem<T>) => void;
  onHighlight?: (item: SelectItem<T>) => void;
  initialIndex?: number;
  limit?: number;
  indicatorComponent?: React.FC<{ isSelected: boolean }>;
  itemComponent?: React.FC<{ item: SelectItem<T>; isHighlighted: boolean }>;
  showDescription?: boolean;
  searchable?: boolean;
  focusOnMount?: boolean;
}

/**
 * Default indicator component
 */
const DefaultIndicator: React.FC<{ isSelected: boolean }> = ({ isSelected }) => (
  <Text color={isSelected ? 'cyan' : undefined}>
    {isSelected ? '❯' : ' '}
  </Text>
);

/**
 * Default item component
 */
function DefaultItem<T>({
  item,
  isHighlighted,
}: {
  item: SelectItem<T>;
  isHighlighted: boolean;
}): React.ReactElement {
  return (
    <Box>
      {item.icon && (
        <Box marginRight={1}>
          <Text>{item.icon}</Text>
        </Box>
      )}
      <Text
        color={item.disabled ? 'gray' : isHighlighted ? 'cyan' : undefined}
        dimColor={item.disabled}
        bold={isHighlighted}
      >
        {item.label}
      </Text>
    </Box>
  );
}

/**
 * Select Component
 */
export function Select<T = string>({
  items,
  onSelect,
  onHighlight,
  initialIndex = 0,
  limit = 10,
  indicatorComponent: Indicator = DefaultIndicator,
  itemComponent: ItemComponent,
  showDescription = true,
  searchable = false,
}: SelectProps<T>): React.ReactElement {
  const [highlightedIndex, setHighlightedIndex] = useState(initialIndex);
  const [searchQuery, setSearchQuery] = useState('');
  const [isSearching, setIsSearching] = useState(false);

  // Filter items based on search
  const filteredItems = useMemo(() => {
    if (!searchable || !searchQuery) {
      return items;
    }
    const query = searchQuery.toLowerCase();
    return items.filter(
      item =>
        item.label.toLowerCase().includes(query) ||
        item.description?.toLowerCase().includes(query)
    );
  }, [items, searchQuery, searchable]);

  // Calculate visible window
  const [visibleStartIndex, setVisibleStartIndex] = useState(0);

  useEffect(() => {
    // Adjust visible window when highlighted index changes
    if (highlightedIndex < visibleStartIndex) {
      setVisibleStartIndex(highlightedIndex);
    } else if (highlightedIndex >= visibleStartIndex + limit) {
      setVisibleStartIndex(highlightedIndex - limit + 1);
    }
  }, [highlightedIndex, limit]);

  // Notify on highlight change
  useEffect(() => {
    const item = filteredItems[highlightedIndex];
    if (item && onHighlight) {
      onHighlight(item);
    }
  }, [highlightedIndex, filteredItems, onHighlight]);

  // Handle keyboard input
  useInput((input, key) => {
    // Search mode
    if (searchable && isSearching) {
      if (key.escape) {
        setIsSearching(false);
        setSearchQuery('');
      } else if (key.backspace || key.delete) {
        setSearchQuery(q => q.slice(0, -1));
      } else if (key.return) {
        setIsSearching(false);
      } else if (input && !key.ctrl && !key.meta) {
        setSearchQuery(q => q + input);
        setHighlightedIndex(0);
      }
      return;
    }

    // Navigation
    if (key.upArrow || input === 'k') {
      setHighlightedIndex(i => {
        let newIndex = i - 1;
        if (newIndex < 0) {
          newIndex = filteredItems.length - 1;
        }
        // Skip disabled items
        while (filteredItems[newIndex]?.disabled && newIndex > 0) {
          newIndex--;
        }
        return newIndex;
      });
    } else if (key.downArrow || input === 'j') {
      setHighlightedIndex(i => {
        let newIndex = i + 1;
        if (newIndex >= filteredItems.length) {
          newIndex = 0;
        }
        // Skip disabled items
        while (filteredItems[newIndex]?.disabled && newIndex < filteredItems.length - 1) {
          newIndex++;
        }
        return newIndex;
      });
    } else if (key.return) {
      const item = filteredItems[highlightedIndex];
      if (item && !item.disabled) {
        onSelect(item);
      }
    } else if (searchable && input === '/') {
      setIsSearching(true);
    } else if (key.pageDown) {
      setHighlightedIndex(i => Math.min(filteredItems.length - 1, i + limit));
    } else if (key.pageUp) {
      setHighlightedIndex(i => Math.max(0, i - limit));
    } else if (input === 'g') {
      setHighlightedIndex(0);
    } else if (input === 'G') {
      setHighlightedIndex(filteredItems.length - 1);
    }
  });

  // Get visible items
  const visibleItems = filteredItems.slice(
    visibleStartIndex,
    visibleStartIndex + limit
  );

  const highlightedItem = filteredItems[highlightedIndex];

  // Render item using custom or default component
  const renderItem = (item: SelectItem<T>, isHighlighted: boolean) => {
    if (ItemComponent) {
      return <ItemComponent item={item} isHighlighted={isHighlighted} />;
    }
    return <DefaultItem item={item} isHighlighted={isHighlighted} />;
  };

  return (
    <Box flexDirection="column">
      {/* Search input */}
      {searchable && isSearching && (
        <Box marginBottom={1}>
          <Text color="cyan">Search: </Text>
          <Text>{searchQuery}</Text>
          <Text dimColor>│</Text>
        </Box>
      )}

      {/* Search indicator */}
      {searchable && !isSearching && searchQuery && (
        <Box marginBottom={1}>
          <Text dimColor>Filtered by: "{searchQuery}" (press / to search)</Text>
        </Box>
      )}

      {/* Scroll indicator (top) */}
      {visibleStartIndex > 0 && (
        <Box>
          <Text dimColor>  ↑ {visibleStartIndex} more</Text>
        </Box>
      )}

      {/* Items */}
      {visibleItems.map((item, index) => {
        const actualIndex = visibleStartIndex + index;
        const isHighlighted = actualIndex === highlightedIndex;

        return (
          <Box key={String(item.value)}>
            <Box marginRight={1}>
              <Indicator isSelected={isHighlighted} />
            </Box>
            {renderItem(item, isHighlighted)}
          </Box>
        );
      })}

      {/* Scroll indicator (bottom) */}
      {visibleStartIndex + limit < filteredItems.length && (
        <Box>
          <Text dimColor>  ↓ {filteredItems.length - visibleStartIndex - limit} more</Text>
        </Box>
      )}

      {/* Description */}
      {showDescription && highlightedItem?.description && (
        <Box marginTop={1} paddingLeft={2}>
          <Text dimColor>{highlightedItem.description}</Text>
        </Box>
      )}

      {/* Empty state */}
      {filteredItems.length === 0 && (
        <Box>
          <Text dimColor>No items found</Text>
        </Box>
      )}

      {/* Help text */}
      {searchable && !isSearching && (
        <Box marginTop={1}>
          <Text dimColor>↑↓ navigate • Enter select{searchable ? ' • / search' : ''}</Text>
        </Box>
      )}
    </Box>
  );
}

/**
 * Quick select for simple yes/no prompts
 */
interface ConfirmProps {
  message: string;
  onConfirm: (confirmed: boolean) => void;
  defaultValue?: boolean;
}

export const Confirm: React.FC<ConfirmProps> = ({
  message,
  onConfirm,
  defaultValue = false,
}) => {
  const [selected, setSelected] = useState(defaultValue);

  useInput((input, key) => {
    if (input === 'y' || input === 'Y') {
      onConfirm(true);
    } else if (input === 'n' || input === 'N') {
      onConfirm(false);
    } else if (key.leftArrow || key.rightArrow || key.tab) {
      setSelected(s => !s);
    } else if (key.return) {
      onConfirm(selected);
    }
  });

  return (
    <Box>
      <Text>{message} </Text>
      <Text
        color={selected ? 'green' : undefined}
        bold={selected}
        underline={selected}
      >
        Yes
      </Text>
      <Text dimColor> / </Text>
      <Text
        color={!selected ? 'red' : undefined}
        bold={!selected}
        underline={!selected}
      >
        No
      </Text>
    </Box>
  );
};

/**
 * Multi-select component
 */
interface MultiSelectProps<T = string> {
  items: SelectItem<T>[];
  onSubmit: (selected: SelectItem<T>[]) => void;
  initialSelected?: T[];
  limit?: number;
  minSelect?: number;
  maxSelect?: number;
}

export function MultiSelect<T = string>({
  items,
  onSubmit,
  initialSelected = [],
  limit = 10,
  minSelect = 0,
  maxSelect = Infinity,
}: MultiSelectProps<T>): React.ReactElement {
  const [highlightedIndex, setHighlightedIndex] = useState(0);
  const [selected, setSelected] = useState<Set<T>>(new Set(initialSelected));

  // Handle keyboard input
  useInput((input, key) => {
    if (key.upArrow || input === 'k') {
      setHighlightedIndex(i => (i > 0 ? i - 1 : items.length - 1));
    } else if (key.downArrow || input === 'j') {
      setHighlightedIndex(i => (i < items.length - 1 ? i + 1 : 0));
    } else if (input === ' ') {
      const item = items[highlightedIndex];
      if (item && !item.disabled) {
        setSelected(prev => {
          const next = new Set(prev);
          if (next.has(item.value)) {
            next.delete(item.value);
          } else if (next.size < maxSelect) {
            next.add(item.value);
          }
          return next;
        });
      }
    } else if (key.return) {
      if (selected.size >= minSelect) {
        const selectedItems = items.filter(item => selected.has(item.value));
        onSubmit(selectedItems);
      }
    } else if (input === 'a') {
      // Select all
      if (selected.size === items.filter(i => !i.disabled).length) {
        setSelected(new Set());
      } else {
        setSelected(new Set(items.filter(i => !i.disabled).map(i => i.value)));
      }
    }
  });

  return (
    <Box flexDirection="column">
      {items.slice(0, limit).map((item, index) => {
        const isHighlighted = index === highlightedIndex;
        const isSelected = selected.has(item.value);

        return (
          <Box key={String(item.value)}>
            <Text color={isHighlighted ? 'cyan' : undefined}>
              {isHighlighted ? '❯' : ' '}
            </Text>
            <Text color={isSelected ? 'green' : undefined}>
              {isSelected ? ' ◉' : ' ○'}
            </Text>
            <Text
              dimColor={item.disabled}
              bold={isHighlighted}
              color={item.disabled ? 'gray' : isHighlighted ? 'cyan' : undefined}
            >
              {' '}{item.label}
            </Text>
          </Box>
        );
      })}

      <Box marginTop={1}>
        <Text dimColor>
          ↑↓ navigate • Space toggle • Enter submit ({selected.size} selected)
        </Text>
      </Box>
    </Box>
  );
}

export default Select;

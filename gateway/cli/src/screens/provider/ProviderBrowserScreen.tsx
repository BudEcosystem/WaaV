/**
 * WaaV CLI Provider Browser Screen
 *
 * Interactive provider browser with search and filtering.
 * Placeholder implementation - will be fully integrated with the browse command.
 */

import React, { useState, useEffect, useMemo } from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import { useConfig } from '../../hooks/useAppState.js';
import { TextInput, Badge } from '../../components/common/index.js';
import { GRADIENTS } from '../../components/branding/index.js';
import { STT_PROVIDERS, TTS_PROVIDERS, REALTIME_PROVIDERS, type ProviderCategory } from '../../types/provider.js';
import type { ScreenProps } from '../../types/navigation.js';

/**
 * Simple provider info for display
 */
interface ProviderEntry {
  id: string;
  name: string;
  category: ProviderCategory;
  description: string;
  tags?: string[];
  configured?: boolean;
}

/**
 * Provider Browser Screen Component
 */
export const ProviderBrowserScreen: React.FC<ScreenProps> = () => {
  const { exit } = useApp();
  const { navigate, goBack, canGoBack, goHome, params } = useNavigation();
  const config = useConfig();

  const [searchQuery, setSearchQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [filterType, setFilterType] = useState<'all' | 'stt' | 'tts' | 'realtime'>(
    (params.providerType as 'stt' | 'tts' | 'realtime') || 'all'
  );
  const [isSearching, setIsSearching] = useState(false);

  // Build providers list from the constant arrays
  const allProviders = useMemo((): ProviderEntry[] => {
    const providers: ProviderEntry[] = [];

    // Add STT providers
    STT_PROVIDERS.forEach(id => {
      providers.push({
        id,
        name: id.charAt(0).toUpperCase() + id.slice(1).replace(/_/g, ' '),
        category: 'stt',
        description: `Speech-to-text provider: ${id}`,
        configured: !!config.providers?.[id],
      });
    });

    // Add TTS providers
    TTS_PROVIDERS.forEach(id => {
      providers.push({
        id,
        name: id.charAt(0).toUpperCase() + id.slice(1).replace(/_/g, ' '),
        category: 'tts',
        description: `Text-to-speech provider: ${id}`,
        configured: !!config.providers?.[id],
      });
    });

    // Add Realtime providers
    REALTIME_PROVIDERS.forEach(id => {
      providers.push({
        id,
        name: id.charAt(0).toUpperCase() + id.slice(1).replace(/_/g, ' '),
        category: 'realtime',
        description: `Realtime audio provider: ${id}`,
        configured: !!config.providers?.[id],
      });
    });

    return providers;
  }, [config.providers]);

  // Filter providers based on search and type
  const filteredProviders = useMemo(() => {
    return allProviders.filter(p => {
      const matchesType = filterType === 'all' || p.category === filterType;
      const matchesSearch = searchQuery === '' ||
        p.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
        p.description.toLowerCase().includes(searchQuery.toLowerCase()) ||
        (p.tags || []).some(t => t.toLowerCase().includes(searchQuery.toLowerCase()));
      return matchesType && matchesSearch;
    });
  }, [allProviders, filterType, searchQuery]);

  // Get visible providers (pagination)
  const maxVisible = 15;
  const startIndex = Math.max(0, selectedIndex - Math.floor(maxVisible / 2));
  const visibleProviders = filteredProviders.slice(startIndex, startIndex + maxVisible);

  // Reset selected index when filters change
  useEffect(() => {
    setSelectedIndex(0);
  }, [filterType, searchQuery]);

  // Handle keyboard input
  useInput((input, key) => {
    if (isSearching) {
      if (key.escape) {
        setIsSearching(false);
        setSearchQuery('');
      }
      return;
    }

    if (key.escape) {
      if (canGoBack) {
        goBack();
      } else {
        goHome();
      }
      return;
    }

    if (input === 'q') {
      exit();
      return;
    }

    // Navigation
    if (key.upArrow) {
      setSelectedIndex(i => Math.max(0, i - 1));
    }
    if (key.downArrow) {
      setSelectedIndex(i => Math.min(filteredProviders.length - 1, i + 1));
    }

    // Select provider
    if (key.return && filteredProviders[selectedIndex]) {
      navigate('provider_config', {
        providerId: filteredProviders[selectedIndex].id,
        providerType: filteredProviders[selectedIndex].category,
      });
    }

    // Search mode
    if (input === '/') {
      setIsSearching(true);
    }

    // Filter shortcuts
    if (input === '1') setFilterType('all');
    if (input === '2') setFilterType('stt');
    if (input === '3') setFilterType('tts');
    if (input === '4') setFilterType('realtime');
  });

  const selectedProvider = filteredProviders[selectedIndex];

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('Provider Browser')}</Text>
        <Text dimColor> - {filteredProviders.length} providers</Text>
      </Box>

      {/* Filter tabs */}
      <Box marginBottom={1}>
        {(['all', 'stt', 'tts', 'realtime'] as const).map((type, index) => (
          <Box key={type} marginRight={2}>
            <Text
              bold={filterType === type}
              color={filterType === type ? 'cyan' : undefined}
              dimColor={filterType !== type}
            >
              [{index + 1}] {type.toUpperCase()}
            </Text>
          </Box>
        ))}
        <Box marginLeft={2}>
          <Text dimColor>[/] Search</Text>
        </Box>
      </Box>

      {/* Search input */}
      {isSearching && (
        <Box marginBottom={1}>
          <Text color="yellow">Search: </Text>
          <TextInput
            value={searchQuery}
            onChange={setSearchQuery}
            onSubmit={() => setIsSearching(false)}
            placeholder="Type to search..."
          />
        </Box>
      )}

      {/* Main content */}
      <Box flexDirection="row" height={20}>
        {/* Provider list */}
        <Box flexDirection="column" width={40} borderStyle="single" borderColor="gray" paddingX={1}>
          {visibleProviders.length === 0 ? (
            <Text dimColor>No providers found</Text>
          ) : (
            visibleProviders.map((provider, index) => {
              const actualIndex = startIndex + index;
              const isSelected = actualIndex === selectedIndex;

              return (
                <Box key={provider.id}>
                  <Text color={isSelected ? 'cyan' : undefined} bold={isSelected}>
                    {isSelected ? '▶ ' : '  '}
                    {provider.name}
                  </Text>
                  {provider.configured && (
                    <Text color="green" dimColor> ✓</Text>
                  )}
                </Box>
              );
            })
          )}
        </Box>

        {/* Provider details */}
        <Box flexDirection="column" flexGrow={1} borderStyle="single" borderColor="gray" paddingX={2} marginLeft={1}>
          {selectedProvider ? (
            <>
              <Box marginBottom={1}>
                <Text bold color="cyan">{selectedProvider.name}</Text>
                <Text dimColor> ({selectedProvider.category.toUpperCase()})</Text>
              </Box>
              <Text wrap="wrap" dimColor>{selectedProvider.description}</Text>

              <Box marginTop={1} flexDirection="column">
                <Text bold>Category:</Text>
                <Box>
                  <Badge
                    label={selectedProvider.category.toUpperCase()}
                    variant={selectedProvider.category === 'stt' ? 'info' : selectedProvider.category === 'tts' ? 'success' : 'warning'}
                  />
                </Box>
              </Box>

              {selectedProvider.configured && (
                <Box marginTop={1}>
                  <Badge label="Configured" variant="success" />
                </Box>
              )}

              <Box marginTop={1}>
                <Text dimColor>Press [Enter] to configure</Text>
              </Box>
            </>
          ) : (
            <Text dimColor>Select a provider to view details</Text>
          )}
        </Box>
      </Box>

      {/* Footer */}
      <Box marginTop={1}>
        <Text dimColor>
          [↑↓] Navigate  [Enter] Configure  [1-4] Filter  [/] Search  [ESC] Back
        </Text>
      </Box>
    </Box>
  );
};

export default ProviderBrowserScreen;

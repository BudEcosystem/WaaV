/**
 * WaaV CLI Provider List Command
 *
 * List all available providers with search and filtering
 */

import React, { useState, useEffect, useMemo } from 'react';
import { render, Text, Box, useInput, useApp } from 'ink';
import TextInput from 'ink-text-input';
import { configManager } from '../../core/config/manager.js';
import {
  Table,
  type TableColumn,
  Badge,
  ProviderTypeBadge,
} from '../../components/common/index.js';
import { GRADIENTS } from '../../components/branding/index.js';
import { STT_PROVIDERS, TTS_PROVIDERS, REALTIME_PROVIDERS } from '../../types/provider.js';
import type { GlobalOptions } from '../../cli.js';

interface ProviderListOptions extends GlobalOptions {
  type?: 'stt' | 'tts' | 'realtime';
  configured?: boolean;
  search?: string;
  interactive?: boolean;
}

interface ProviderEntry {
  id: string;
  type: 'stt' | 'tts' | 'realtime';
  typeLabel: string;
  configured: boolean;
  description: string;
  tags: string[];
}

// Provider metadata for richer display
const PROVIDER_METADATA: Record<string, { description: string; tags: string[] }> = {
  // STT Providers
  deepgram: { description: 'Real-time STT with Nova-2 model', tags: ['streaming', 'fast', 'accurate'] },
  google: { description: 'Google Cloud Speech-to-Text', tags: ['multilingual', 'enterprise'] },
  azure: { description: 'Microsoft Azure Speech Services', tags: ['enterprise', 'custom-models'] },
  aws_transcribe: { description: 'Amazon Transcribe', tags: ['aws', 'enterprise'] },
  openai: { description: 'OpenAI Whisper API', tags: ['whisper', 'accurate'] },
  assemblyai: { description: 'AssemblyAI with LeMUR', tags: ['ai-powered', 'summarization'] },
  rev_ai: { description: 'Rev.ai transcription', tags: ['human-quality', 'async'] },
  speechmatics: { description: 'Speechmatics real-time', tags: ['accurate', 'multilingual'] },
  whisper: { description: 'Local Whisper model', tags: ['local', 'open-source', 'free'] },
  groq: { description: 'Groq Whisper (ultra-fast)', tags: ['fast', 'whisper'] },
  cartesia: { description: 'Cartesia Sonic', tags: ['low-latency', 'streaming'] },
  soniox: { description: 'Soniox speech recognition', tags: ['accurate', 'streaming'] },
  symbl: { description: 'Symbl.ai conversation intelligence', tags: ['conversation', 'insights'] },
  vosk: { description: 'Vosk offline recognition', tags: ['local', 'offline', 'free'] },
  picovoice: { description: 'Picovoice Leopard/Cheetah', tags: ['local', 'edge', 'privacy'] },
  alibaba: { description: 'Alibaba Cloud ASR', tags: ['chinese', 'asia'] },
  baidu: { description: 'Baidu Speech Recognition', tags: ['chinese', 'asia'] },
  tencent: { description: 'Tencent Cloud ASR', tags: ['chinese', 'asia'] },
  iflytek: { description: 'iFlytek Speech Recognition', tags: ['chinese', 'asia'] },
  sarvam: { description: 'Sarvam AI (Indian languages)', tags: ['indian', 'multilingual'] },
  bhashini: { description: 'Bhashini (Indian govt)', tags: ['indian', 'free', 'hindi'] },
  krutrim: { description: 'Krutrim AI (Indian)', tags: ['indian', 'ola'] },
  vakyansh: { description: 'Vakyansh (Indian open-source)', tags: ['indian', 'open-source'] },
  gladia: { description: 'Gladia transcription', tags: ['fast', 'accurate'] },
  lmnt: { description: 'LMNT speech recognition', tags: ['low-latency'] },
  eleven_labs: { description: 'ElevenLabs STT', tags: ['premium'] },
  hume: { description: 'Hume AI (emotional)', tags: ['emotion', 'empathic'] },

  // TTS Providers
  elevenlabs: { description: 'ElevenLabs voice synthesis', tags: ['premium', 'voice-cloning', 'realistic'] },
  aws_polly: { description: 'Amazon Polly', tags: ['aws', 'enterprise', 'neural'] },
  play_ht: { description: 'PlayHT voice generation', tags: ['voice-cloning', 'realistic'] },
  murf: { description: 'Murf AI voices', tags: ['professional', 'studio'] },
  lovo: { description: 'LOVO AI voice generator', tags: ['video', 'marketing'] },
  resemble: { description: 'Resemble AI voice cloning', tags: ['voice-cloning', 'custom'] },
  wellsaid: { description: 'WellSaid Labs voices', tags: ['enterprise', 'natural'] },
  coqui: { description: 'Coqui TTS (open-source)', tags: ['local', 'open-source', 'free'] },
  bark: { description: 'Bark (Suno) voice synthesis', tags: ['local', 'open-source', 'music'] },
  tortoise: { description: 'Tortoise TTS', tags: ['local', 'quality', 'slow'] },
  piper: { description: 'Piper TTS (fast local)', tags: ['local', 'fast', 'free'] },
  xtts: { description: 'XTTS v2 (Coqui)', tags: ['local', 'voice-cloning'] },
  unrealspeech: { description: 'Unreal Speech (cheap)', tags: ['budget', 'fast'] },
  fish: { description: 'Fish Speech', tags: ['local', 'open-source'] },
  neets: { description: 'Neets.ai TTS', tags: ['fast', 'streaming'] },
  styletts2: { description: 'StyleTTS 2', tags: ['local', 'expressive'] },
  microsoft_edge: { description: 'Microsoft Edge TTS', tags: ['free', 'edge'] },
  voicevox: { description: 'VOICEVOX (Japanese)', tags: ['japanese', 'local', 'anime'] },
  silero: { description: 'Silero TTS', tags: ['local', 'multilingual'] },
  espeak: { description: 'eSpeak NG', tags: ['local', 'free', 'robotic'] },
  festival: { description: 'Festival Speech', tags: ['local', 'free', 'academic'] },

  // Realtime Providers
  openai_realtime: { description: 'OpenAI Realtime API (GPT-4o)', tags: ['gpt-4o', 'voice-to-voice', 'premium'] },
  deepgram_voice_agent: { description: 'Deepgram Voice Agent', tags: ['low-latency', 'agent'] },
  livekit_agents: { description: 'LiveKit Agents Framework', tags: ['open-source', 'framework'] },
  vapi: { description: 'Vapi voice AI platform', tags: ['voice-ai', 'platform'] },
  retell: { description: 'Retell AI voice agents', tags: ['voice-agent', 'phone'] },
  bland: { description: 'Bland AI phone calls', tags: ['phone', 'agent'] },
};

/**
 * Get provider entry with metadata
 */
function getProviderEntry(id: string, type: 'stt' | 'tts' | 'realtime', configuredSet: Set<string>): ProviderEntry {
  const metadata = PROVIDER_METADATA[id] || { description: `${id} provider`, tags: [] };
  return {
    id,
    type,
    typeLabel: type.toUpperCase(),
    configured: configuredSet.has(id),
    description: metadata.description,
    tags: metadata.tags,
  };
}

/**
 * Interactive provider list component with search
 */
const InteractiveProviderList: React.FC<{
  options: ProviderListOptions;
  onSelect?: (provider: ProviderEntry) => void;
}> = ({ options, onSelect }) => {
  const { exit } = useApp();
  const [searchQuery, setSearchQuery] = useState(options.search || '');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [isSearchFocused, setIsSearchFocused] = useState(true);

  const config = configManager.get();
  const configuredProviders = new Set(Object.keys(config.providers));

  // Build full provider list
  const allProviders = useMemo(() => {
    const providers: ProviderEntry[] = [];

    if (!options.type || options.type === 'stt') {
      providers.push(...STT_PROVIDERS.map(id => getProviderEntry(id, 'stt', configuredProviders)));
    }
    if (!options.type || options.type === 'tts') {
      providers.push(...TTS_PROVIDERS.map(id => getProviderEntry(id, 'tts', configuredProviders)));
    }
    if (!options.type || options.type === 'realtime') {
      providers.push(...REALTIME_PROVIDERS.map(id => getProviderEntry(id, 'realtime', configuredProviders)));
    }

    // Filter to configured only if requested
    if (options.configured) {
      return providers.filter(p => p.configured);
    }

    return providers;
  }, [options.type, options.configured, configuredProviders]);

  // Filter providers by search query
  const filteredProviders = useMemo(() => {
    if (!searchQuery.trim()) {
      return allProviders;
    }

    const query = searchQuery.toLowerCase().trim();
    return allProviders.filter(provider => {
      // Search in ID
      if (provider.id.toLowerCase().includes(query)) {
        return true;
      }
      // Search in description
      if (provider.description.toLowerCase().includes(query)) {
        return true;
      }
      // Search in tags
      if (provider.tags.some(tag => tag.toLowerCase().includes(query))) {
        return true;
      }
      // Search in type
      if (provider.type.toLowerCase().includes(query)) {
        return true;
      }
      return false;
    });
  }, [allProviders, searchQuery]);

  // Reset selection when filter changes
  useEffect(() => {
    setSelectedIndex(0);
  }, [searchQuery]);

  // Handle keyboard input
  useInput((input, key) => {
    if (key.escape) {
      exit();
      return;
    }

    if (!isSearchFocused) {
      if (key.upArrow) {
        setSelectedIndex(i => Math.max(0, i - 1));
      } else if (key.downArrow) {
        setSelectedIndex(i => Math.min(filteredProviders.length - 1, i + 1));
      } else if (key.return && filteredProviders[selectedIndex]) {
        if (onSelect) {
          onSelect(filteredProviders[selectedIndex]);
        }
      } else if (input === '/') {
        setIsSearchFocused(true);
      }
    }

    if (key.tab) {
      setIsSearchFocused(!isSearchFocused);
    }
  });

  const maxDisplay = 15;
  const startIndex = Math.max(0, selectedIndex - Math.floor(maxDisplay / 2));
  const displayProviders = filteredProviders.slice(startIndex, startIndex + maxDisplay);

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Providers')}</Text>
        <Text dimColor> ({filteredProviders.length}/{allProviders.length})</Text>
      </Box>

      {/* Search box */}
      <Box marginBottom={1}>
        <Text color={isSearchFocused ? 'cyan' : 'gray'}>Search: </Text>
        <Box borderStyle="single" borderColor={isSearchFocused ? 'cyan' : 'gray'} paddingX={1}>
          <TextInput
            value={searchQuery}
            onChange={setSearchQuery}
            placeholder="Type to search providers, tags, features..."
            focus={isSearchFocused}
            onSubmit={() => setIsSearchFocused(false)}
          />
        </Box>
      </Box>

      {/* Type filter indicator */}
      {options.type && (
        <Box marginBottom={1}>
          <Text dimColor>Filter: </Text>
          <ProviderTypeBadge type={options.type} />
        </Box>
      )}

      {/* Provider list */}
      <Box flexDirection="column" borderStyle="single" borderColor="gray" paddingX={1}>
        {/* Header row */}
        <Box>
          <Box width={3}><Text dimColor> </Text></Box>
          <Box width={22}><Text bold dimColor>Provider</Text></Box>
          <Box width={10}><Text bold dimColor>Type</Text></Box>
          <Box width={8}><Text bold dimColor>Status</Text></Box>
          <Box><Text bold dimColor>Description</Text></Box>
        </Box>

        {filteredProviders.length === 0 ? (
          <Box paddingY={1}>
            <Text dimColor>No providers found matching "{searchQuery}"</Text>
          </Box>
        ) : (
          displayProviders.map((provider, index) => {
            const actualIndex = startIndex + index;
            const isSelected = actualIndex === selectedIndex && !isSearchFocused;

            return (
              <Box key={provider.id}>
                <Box width={3}>
                  <Text color={isSelected ? 'cyan' : undefined}>
                    {isSelected ? '>' : ' '}
                  </Text>
                </Box>
                <Box width={22}>
                  <Text
                    bold={isSelected}
                    color={isSelected ? 'cyan' : provider.configured ? 'green' : undefined}
                  >
                    {provider.id}
                  </Text>
                </Box>
                <Box width={10}>
                  <Text color={
                    provider.type === 'stt' ? 'blue' :
                    provider.type === 'tts' ? 'magenta' :
                    'yellow'
                  }>
                    {provider.typeLabel}
                  </Text>
                </Box>
                <Box width={8}>
                  {provider.configured ? (
                    <Text color="green">Ready</Text>
                  ) : (
                    <Text dimColor>-</Text>
                  )}
                </Box>
                <Box>
                  <Text dimColor wrap="truncate">{provider.description}</Text>
                </Box>
              </Box>
            );
          })
        )}

        {filteredProviders.length > maxDisplay && (
          <Box marginTop={1}>
            <Text dimColor>
              Showing {startIndex + 1}-{Math.min(startIndex + maxDisplay, filteredProviders.length)} of {filteredProviders.length}
            </Text>
          </Box>
        )}
      </Box>

      {/* Selected provider tags */}
      {filteredProviders[selectedIndex] && !isSearchFocused && (
        <Box marginTop={1}>
          <Text dimColor>Tags: </Text>
          {filteredProviders[selectedIndex].tags.map((tag) => (
            <Box key={tag} marginRight={1}>
              <Badge label={tag} variant="default" style="outline" />
            </Box>
          ))}
        </Box>
      )}

      {/* Help */}
      <Box marginTop={1}>
        <Text dimColor>
          [/] Search  [Tab] Toggle focus  [↑↓] Navigate  [Enter] Select  [Esc] Exit
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Simple provider list (non-interactive)
 */
const SimpleProviderList: React.FC<{ options: ProviderListOptions }> = ({ options }) => {
  const config = configManager.get();
  const configuredProviders = new Set(Object.keys(config.providers));

  // Get providers based on type filter
  let providers: ProviderEntry[] = [];

  if (!options.type || options.type === 'stt') {
    providers.push(...STT_PROVIDERS.map(id => getProviderEntry(id, 'stt', configuredProviders)));
  }
  if (!options.type || options.type === 'tts') {
    providers.push(...TTS_PROVIDERS.map(id => getProviderEntry(id, 'tts', configuredProviders)));
  }
  if (!options.type || options.type === 'realtime') {
    providers.push(...REALTIME_PROVIDERS.map(id => getProviderEntry(id, 'realtime', configuredProviders)));
  }

  // Filter to configured only if requested
  if (options.configured) {
    providers = providers.filter(p => p.configured);
  }

  // Filter by search query
  if (options.search) {
    const query = options.search.toLowerCase();
    providers = providers.filter(p =>
      p.id.toLowerCase().includes(query) ||
      p.description.toLowerCase().includes(query) ||
      p.tags.some(t => t.toLowerCase().includes(query))
    );
  }

  const columns: TableColumn<ProviderEntry>[] = [
    {
      key: 'configured',
      header: '',
      width: 2,
      render: (val) => val ? <Text color="green">✓</Text> : <Text dimColor>○</Text>,
    },
    { key: 'id', header: 'Provider', width: 20 },
    { key: 'typeLabel', header: 'Type', width: 10 },
    {
      key: 'description',
      header: 'Description',
      width: 40,
      render: (val) => <Text dimColor wrap="truncate">{String(val)}</Text>,
    },
  ];

  return (
    <Box flexDirection="column" paddingY={1}>
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Providers')}</Text>
        <Text dimColor> ({providers.length} total)</Text>
      </Box>

      {options.search && (
        <Box marginBottom={1}>
          <Text dimColor>Search: "{options.search}"</Text>
        </Box>
      )}

      <Table
        data={providers}
        columns={columns}
        borderStyle="single"
        maxRows={30}
      />

      <Box marginTop={1}>
        <Text dimColor>
          Use "waav provider list -i" for interactive search, or "waav provider add {'<name>'}" to configure
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Provider list command handler
 */
export async function providerListCommand(options: ProviderListOptions): Promise<void> {
  await configManager.initialize({
    configPath: options.config,
    profile: options.profile,
  });

  if (options.json) {
    const config = configManager.get();
    const configuredSet = new Set(Object.keys(config.providers));

    const result = {
      stt: STT_PROVIDERS.map(id => ({
        id,
        ...PROVIDER_METADATA[id],
        configured: configuredSet.has(id),
      })),
      tts: TTS_PROVIDERS.map(id => ({
        id,
        ...PROVIDER_METADATA[id],
        configured: configuredSet.has(id),
      })),
      realtime: REALTIME_PROVIDERS.map(id => ({
        id,
        ...PROVIDER_METADATA[id],
        configured: configuredSet.has(id),
      })),
      total: {
        stt: STT_PROVIDERS.length,
        tts: TTS_PROVIDERS.length,
        realtime: REALTIME_PROVIDERS.length,
        all: STT_PROVIDERS.length + TTS_PROVIDERS.length + REALTIME_PROVIDERS.length,
      },
      configured: Object.keys(config.providers),
    };

    // Apply search filter to JSON output
    if (options.search) {
      const query = options.search.toLowerCase();
      const filter = (arr: typeof result.stt) =>
        arr.filter(p =>
          p.id.toLowerCase().includes(query) ||
          p.description?.toLowerCase().includes(query) ||
          p.tags?.some((t: string) => t.toLowerCase().includes(query))
        );
      result.stt = filter(result.stt);
      result.tts = filter(result.tts);
      result.realtime = filter(result.realtime);
    }

    console.log(JSON.stringify(result, null, 2));
    return;
  }

  // Interactive mode
  if (options.interactive) {
    const { waitUntilExit } = render(<InteractiveProviderList options={options} />);
    await waitUntilExit();
    return;
  }

  // Simple list mode
  const { waitUntilExit } = render(<SimpleProviderList options={options} />);
  await waitUntilExit();
}

export default providerListCommand;

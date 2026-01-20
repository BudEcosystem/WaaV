/**
 * WaaV CLI Provider Browse Command
 *
 * Interactive provider browser with search, filtering, and configuration
 */

import React, { useState, useEffect, useMemo } from 'react';
import { render, Text, Box, useInput, useApp } from 'ink';
import TextInput from 'ink-text-input';
import { configManager } from '../../core/config/manager.js';
import {
  Badge,
  ProviderTypeBadge,
} from '../../components/common/index.js';
import { GRADIENTS } from '../../components/branding/index.js';
import { STT_PROVIDERS, TTS_PROVIDERS, REALTIME_PROVIDERS } from '../../types/provider.js';
import type { GlobalOptions } from '../../cli.js';

interface ProviderBrowseOptions extends GlobalOptions {
  type?: 'stt' | 'tts' | 'realtime';
}

interface ProviderEntry {
  id: string;
  name: string;
  type: 'stt' | 'tts' | 'realtime';
  description: string;
  tags: string[];
  features: string[];
  configured: boolean;
  website?: string;
  pricing?: string;
  localModel?: boolean; // Local inference (coming soon)
}

// Comprehensive provider metadata
const PROVIDER_DATA: Record<string, Omit<ProviderEntry, 'id' | 'type' | 'configured'>> = {
  // STT Providers
  deepgram: {
    name: 'Deepgram',
    description: 'Real-time speech recognition with Nova-2 model. Industry-leading accuracy and speed.',
    tags: ['streaming', 'fast', 'accurate', 'enterprise'],
    features: ['Real-time streaming', 'Word timestamps', 'Speaker diarization', 'Custom vocabulary'],
    website: 'https://deepgram.com',
    pricing: 'Pay-per-use, free tier available',
  },
  google: {
    name: 'Google Cloud STT',
    description: 'Google Cloud Speech-to-Text with 125+ languages and dialects.',
    tags: ['multilingual', 'enterprise', 'accurate'],
    features: ['125+ languages', 'Automatic punctuation', 'Speech adaptation', 'Multi-channel'],
    website: 'https://cloud.google.com/speech-to-text',
    pricing: 'Pay-per-use',
  },
  azure: {
    name: 'Microsoft Azure',
    description: 'Azure Speech Services with custom model training and neural voices.',
    tags: ['enterprise', 'custom-models', 'neural'],
    features: ['Custom Speech', 'Real-time transcription', 'Batch transcription', 'Pronunciation assessment'],
    website: 'https://azure.microsoft.com/services/cognitive-services/speech-services/',
    pricing: 'Pay-per-use, free tier',
  },
  aws_transcribe: {
    name: 'Amazon Transcribe',
    description: 'AWS automatic speech recognition service with custom vocabulary.',
    tags: ['aws', 'enterprise', 'scalable'],
    features: ['Custom vocabulary', 'Automatic language ID', 'Content redaction', 'Call analytics'],
    website: 'https://aws.amazon.com/transcribe/',
    pricing: 'Pay-per-use',
  },
  openai: {
    name: 'OpenAI Whisper',
    description: 'OpenAI Whisper API - highly accurate multilingual transcription.',
    tags: ['whisper', 'accurate', 'multilingual'],
    features: ['99 languages', 'Translation', 'Timestamps', 'High accuracy'],
    website: 'https://openai.com/whisper',
    pricing: 'Pay-per-use',
  },
  assemblyai: {
    name: 'AssemblyAI',
    description: 'AI-powered transcription with LeMUR for audio intelligence.',
    tags: ['ai-powered', 'summarization', 'accurate'],
    features: ['LeMUR AI', 'Speaker labels', 'Sentiment analysis', 'PII redaction'],
    website: 'https://assemblyai.com',
    pricing: 'Pay-per-use, free tier',
  },
  whisper: {
    name: 'Whisper (Local)',
    description: 'Run OpenAI Whisper locally - no API calls, full privacy.',
    tags: ['local', 'open-source', 'free', 'private'],
    features: ['Offline', 'No API costs', 'Privacy', 'Multiple model sizes'],
    localModel: true,
    pricing: 'Free (local compute)',
  },
  groq: {
    name: 'Groq Whisper',
    description: 'Ultra-fast Whisper inference on Groq LPU - 10x faster than real-time.',
    tags: ['fast', 'whisper', 'low-latency'],
    features: ['Ultra-fast', 'Whisper large-v3', 'Low latency', 'High throughput'],
    website: 'https://groq.com',
    pricing: 'Pay-per-use, generous free tier',
  },
  cartesia: {
    name: 'Cartesia Sonic',
    description: 'Low-latency speech recognition optimized for real-time applications.',
    tags: ['low-latency', 'streaming', 'real-time'],
    features: ['Sub-200ms latency', 'Streaming', 'Word-level timestamps'],
    website: 'https://cartesia.ai',
    pricing: 'Pay-per-use',
  },
  vosk: {
    name: 'Vosk',
    description: 'Offline speech recognition with small footprint models.',
    tags: ['local', 'offline', 'free', 'edge'],
    features: ['Offline', 'Small models', 'Multiple languages', 'Speaker identification'],
    localModel: true,
    website: 'https://alphacephei.com/vosk/',
    pricing: 'Free, open-source',
  },
  picovoice: {
    name: 'Picovoice',
    description: 'Speech recognition with Leopard and Cheetah. Cloud API available.',
    tags: ['edge', 'privacy', 'embedded', 'cloud'],
    features: ['Cloud API', 'Wake word', 'Intent recognition', 'Low latency'],
    website: 'https://picovoice.ai',
    pricing: 'Free tier, enterprise plans',
  },
  sarvam: {
    name: 'Sarvam AI',
    description: 'Indian language specialist with 10+ Indic languages.',
    tags: ['indian', 'multilingual', 'indic'],
    features: ['Hindi', 'Tamil', 'Telugu', 'Bengali', 'Marathi', 'Code-switching'],
    website: 'https://sarvam.ai',
    pricing: 'Pay-per-use',
  },
  bhashini: {
    name: 'Bhashini',
    description: 'Indian government AI platform for Indic languages - free access.',
    tags: ['indian', 'free', 'hindi', 'government'],
    features: ['22 Indian languages', 'Government backed', 'Free API', 'Translation'],
    website: 'https://bhashini.gov.in',
    pricing: 'Free',
  },
  gladia: {
    name: 'Gladia',
    description: 'Fast, accurate transcription with real-time capabilities.',
    tags: ['fast', 'accurate', 'real-time'],
    features: ['Real-time', 'Speaker diarization', 'Audio intelligence', 'Summarization'],
    website: 'https://gladia.io',
    pricing: 'Pay-per-use, free tier',
  },
  hume: {
    name: 'Hume AI',
    description: 'Emotionally intelligent AI with expression measurement.',
    tags: ['emotion', 'empathic', 'expressive'],
    features: ['Emotion detection', 'Prosody analysis', 'Empathic voice', 'Expression measurement'],
    website: 'https://hume.ai',
    pricing: 'Pay-per-use',
  },

  // TTS Providers
  elevenlabs: {
    name: 'ElevenLabs',
    description: 'Premium AI voice synthesis with voice cloning and emotional control.',
    tags: ['premium', 'voice-cloning', 'realistic', 'emotional'],
    features: ['Voice cloning', 'Emotional control', '29 languages', 'Sound effects'],
    website: 'https://elevenlabs.io',
    pricing: 'Free tier, paid plans',
  },
  aws_polly: {
    name: 'Amazon Polly',
    description: 'AWS text-to-speech with neural and standard voices.',
    tags: ['aws', 'enterprise', 'neural', 'ssml'],
    features: ['Neural TTS', 'SSML support', 'Multiple formats', 'Newscaster style'],
    website: 'https://aws.amazon.com/polly/',
    pricing: 'Pay-per-use',
  },
  play_ht: {
    name: 'PlayHT',
    description: 'AI voice generator with ultra-realistic voices and cloning.',
    tags: ['voice-cloning', 'realistic', 'streaming'],
    features: ['Voice cloning', 'Real-time streaming', 'Emotion control', '142 languages'],
    website: 'https://play.ht',
    pricing: 'Free tier, paid plans',
  },
  coqui: {
    name: 'Coqui TTS',
    description: 'Open-source TTS with XTTS for voice cloning.',
    tags: ['local', 'open-source', 'free', 'voice-cloning'],
    features: ['Open-source', 'Voice cloning', 'XTTS', 'Multilingual'],
    localModel: true,
    website: 'https://coqui.ai',
    pricing: 'Free, open-source',
  },
  piper: {
    name: 'Piper',
    description: 'Fast, local neural TTS optimized for Raspberry Pi.',
    tags: ['local', 'fast', 'free', 'edge'],
    features: ['Ultra-fast', 'Small models', 'Offline', 'Raspberry Pi'],
    localModel: true,
    website: 'https://github.com/rhasspy/piper',
    pricing: 'Free, open-source',
  },
  bark: {
    name: 'Bark (Suno)',
    description: 'Generative audio model that can create speech, music, and sound effects.',
    tags: ['local', 'open-source', 'music', 'creative'],
    features: ['Speech', 'Music', 'Sound effects', 'Laughter', 'Multilingual'],
    localModel: true,
    website: 'https://github.com/suno-ai/bark',
    pricing: 'Free, open-source',
  },
  unrealspeech: {
    name: 'Unreal Speech',
    description: 'Budget-friendly TTS API - 10x cheaper than alternatives.',
    tags: ['budget', 'fast', 'cheap'],
    features: ['Low cost', 'Fast', 'Multiple voices', 'Streaming'],
    website: 'https://unrealspeech.com',
    pricing: 'Very affordable',
  },
  microsoft_edge: {
    name: 'Microsoft Edge TTS',
    description: 'Free TTS using Edge browser voices - no API key needed.',
    tags: ['free', 'edge', 'no-api-key'],
    features: ['Free', 'No API key', 'Many voices', 'Good quality'],
    pricing: 'Free',
  },
  voicevox: {
    name: 'VOICEVOX',
    description: 'Japanese TTS with anime-style character voices.',
    tags: ['japanese', 'local', 'anime', 'character'],
    features: ['Anime voices', 'Japanese', 'Character personas', 'Expressive'],
    localModel: true,
    website: 'https://voicevox.hiroshiba.jp',
    pricing: 'Free',
  },

  // Realtime Providers
  openai_realtime: {
    name: 'OpenAI Realtime',
    description: 'GPT-4o voice-to-voice with function calling and interruption handling.',
    tags: ['gpt-4o', 'voice-to-voice', 'premium', 'intelligent'],
    features: ['Voice-to-voice', 'Function calling', 'Interruption', 'Low latency'],
    website: 'https://openai.com',
    pricing: 'Premium pricing',
  },
  deepgram_voice_agent: {
    name: 'Deepgram Voice Agent',
    description: 'Low-latency voice agent with Aura TTS and Nova STT.',
    tags: ['low-latency', 'agent', 'enterprise'],
    features: ['Sub-second latency', 'Agent framework', 'Custom voices'],
    website: 'https://deepgram.com',
    pricing: 'Pay-per-use',
  },
  livekit_agents: {
    name: 'LiveKit Agents',
    description: 'Open-source framework for building real-time voice agents.',
    tags: ['open-source', 'framework', 'flexible'],
    features: ['Open-source', 'WebRTC', 'Flexible', 'Self-hostable'],
    website: 'https://livekit.io',
    pricing: 'Open-source, cloud plans available',
  },
  vapi: {
    name: 'Vapi',
    description: 'Voice AI platform for building voice assistants and phone agents.',
    tags: ['voice-ai', 'platform', 'phone'],
    features: ['Voice assistants', 'Phone integration', 'No-code', 'Analytics'],
    website: 'https://vapi.ai',
    pricing: 'Free tier, pay-per-use',
  },
  retell: {
    name: 'Retell AI',
    description: 'Build human-like AI voice agents for phone calls.',
    tags: ['voice-agent', 'phone', 'enterprise'],
    features: ['Phone calls', 'Human-like', 'Low latency', 'Custom LLM'],
    website: 'https://retellai.com',
    pricing: 'Pay-per-use',
  },
  bland: {
    name: 'Bland AI',
    description: 'AI phone calling platform for outbound and inbound calls.',
    tags: ['phone', 'agent', 'calling'],
    features: ['Phone calls', 'Outbound', 'Inbound', 'Call transfers'],
    website: 'https://bland.ai',
    pricing: 'Pay-per-call',
  },

  // ============================================
  // Indian & Regional Providers
  // ============================================

  gnani: {
    name: 'Gnani AI',
    description: 'Indian STT with 14+ languages, voice biometrics, and gRPC/REST APIs.',
    tags: ['indian', 'multilingual', 'voice-biometrics', 'enterprise'],
    features: ['14+ Indic languages', 'Voice biometrics', 'Speaker verification', 'Real-time streaming', 'Code-mixing'],
    website: 'https://gnani.ai',
    pricing: 'Enterprise pricing',
  },
  reverie: {
    name: 'Reverie',
    description: 'Indian multilingual platform with 22+ languages and code-mixed Hinglish support.',
    tags: ['indian', 'multilingual', 'hinglish', 'enterprise'],
    features: ['22+ Indian languages', 'Code-mixed Hinglish', 'Transliteration', 'Real-time STT'],
    website: 'https://reverieinc.com',
    pricing: 'Enterprise pricing',
  },

  // ============================================
  // Voice Biometrics & Enterprise
  // ============================================

  phonexia: {
    name: 'Phonexia',
    description: 'Voice biometrics and speech analytics for enterprise and security applications.',
    tags: ['voice-biometrics', 'enterprise', 'security'],
    features: ['Voice biometrics', 'Speaker identification', 'Language detection', 'Cloud API'],
    website: 'https://phonexia.com',
    pricing: 'Enterprise pricing',
  },

  // ============================================
  // Russian Providers
  // ============================================

  yandex: {
    name: 'Yandex SpeechKit',
    description: 'Russian speech services with STT, TTS, and voice synthesis.',
    tags: ['russian', 'multilingual', 'enterprise'],
    features: ['Russian language', 'Streaming STT', 'Neural TTS', 'Voice cloning'],
    website: 'https://cloud.yandex.com/services/speechkit',
    pricing: 'Pay-per-use',
  },
  tinkoff: {
    name: 'Tinkoff VoiceKit',
    description: 'Russian speech recognition and synthesis platform.',
    tags: ['russian', 'enterprise', 'banking'],
    features: ['Russian STT', 'TTS', 'Call center integration', 'Custom models'],
    website: 'https://voicekit.tinkoff.ru',
    pricing: 'Enterprise pricing',
  },
  sberdevices: {
    name: 'SberDevices',
    description: 'Sberbank speech AI platform with Russian language specialization.',
    tags: ['russian', 'enterprise', 'salute'],
    features: ['Russian STT/TTS', 'Salute assistants', 'Smart home', 'Enterprise'],
    website: 'https://sberdevices.ru',
    pricing: 'Enterprise pricing',
  },

  // ============================================
  // Additional TTS Providers
  // ============================================

  speechify: {
    name: 'Speechify',
    description: 'AI voice generator and text-to-speech for content creators.',
    tags: ['content', 'voice-cloning', 'streaming'],
    features: ['Voice cloning', 'Audiobooks', 'Chrome extension', 'Mobile app'],
    website: 'https://speechify.com',
    pricing: 'Free tier, premium plans',
  },
  smallest: {
    name: 'Smallest.ai',
    description: 'Ultra-low latency TTS with ~100ms response time for real-time applications.',
    tags: ['low-latency', 'fast', 'real-time'],
    features: ['~100ms latency', 'Real-time streaming', 'Voice API', 'Low cost'],
    website: 'https://smallest.ai',
    pricing: 'Pay-per-use',
  },

  // ============================================
  // Additional TTS with missing metadata
  // ============================================

  murf: {
    name: 'Murf AI',
    description: 'AI voice generator with 120+ voices in 20+ languages for video & podcast.',
    tags: ['voice-over', 'video', 'podcast', 'enterprise'],
    features: ['120+ voices', 'Voice cloning', 'Video sync', 'Pitch control'],
    website: 'https://murf.ai',
    pricing: 'Free tier, paid plans',
  },
  wellsaid: {
    name: 'WellSaid Labs',
    description: 'Enterprise AI voice platform with ultra-realistic voice avatars.',
    tags: ['enterprise', 'realistic', 'voice-avatars'],
    features: ['Voice avatars', 'Enterprise SSO', 'Brand voices', 'SSML'],
    website: 'https://wellsaidlabs.com',
    pricing: 'Enterprise plans',
  },
  resemble: {
    name: 'Resemble AI',
    description: 'AI voice generator with real-time voice cloning and deepfake detection.',
    tags: ['voice-cloning', 'real-time', 'deepfake-detection'],
    features: ['Real-time cloning', 'Emotion control', 'Deepfake detection', 'Localize'],
    website: 'https://resemble.ai',
    pricing: 'Free tier, enterprise',
  },
  lovo: {
    name: 'LOVO AI',
    description: 'AI voice generator and video maker with 500+ voices.',
    tags: ['voice-over', 'video', 'content'],
    features: ['500+ voices', 'Video maker', 'Voice cloning', '100+ languages'],
    website: 'https://lovo.ai',
    pricing: 'Free tier, paid plans',
  },

  // ============================================
  // Asian Providers
  // ============================================

  alibaba: {
    name: 'Alibaba Cloud',
    description: 'Alibaba Cloud Intelligent Speech Service with Chinese language focus.',
    tags: ['chinese', 'enterprise', 'cloud'],
    features: ['Mandarin', 'Cantonese', 'Real-time STT', 'TTS', 'Custom models'],
    website: 'https://www.alibabacloud.com/product/intelligent-speech-interaction',
    pricing: 'Pay-per-use',
  },
  baidu: {
    name: 'Baidu Speech',
    description: 'Baidu AI speech recognition and synthesis for Chinese languages.',
    tags: ['chinese', 'enterprise', 'ai'],
    features: ['Mandarin', 'Dialects', 'Real-time', 'Emotion recognition'],
    website: 'https://ai.baidu.com/tech/speech',
    pricing: 'Pay-per-use',
  },
  tencent: {
    name: 'Tencent Cloud ASR',
    description: 'Tencent Cloud speech services with WeChat integration.',
    tags: ['chinese', 'enterprise', 'wechat'],
    features: ['Real-time STT', 'TTS', 'WeChat mini-programs', 'Custom hotwords'],
    website: 'https://cloud.tencent.com/product/asr',
    pricing: 'Pay-per-use',
  },
  iflytek: {
    name: 'iFlytek',
    description: 'Chinese AI giant with advanced speech recognition and synthesis.',
    tags: ['chinese', 'enterprise', 'research'],
    features: ['Best Chinese STT', 'Neural TTS', 'Dialects', 'Education'],
    website: 'https://www.iflytek.com',
    pricing: 'Enterprise pricing',
  },

  // ============================================
  // Local Models
  // ============================================

  xtts: {
    name: 'XTTS (Coqui)',
    description: 'State-of-the-art multilingual voice cloning with minimal samples.',
    tags: ['local', 'voice-cloning', 'multilingual', 'open-source'],
    features: ['24 languages', '3-second cloning', 'Streaming', 'Zero-shot'],
    localModel: true,
    website: 'https://huggingface.co/coqui/XTTS-v2',
    pricing: 'Free, open-source',
  },
  tortoise: {
    name: 'Tortoise TTS',
    description: 'High-quality but slower TTS with excellent voice cloning.',
    tags: ['local', 'high-quality', 'voice-cloning', 'slow'],
    features: ['Voice cloning', 'High quality', 'Multi-voice', 'Expressive'],
    localModel: true,
    website: 'https://github.com/neonbjb/tortoise-tts',
    pricing: 'Free, open-source',
  },
  silero: {
    name: 'Silero TTS',
    description: 'Fast, lightweight local TTS with multiple languages.',
    tags: ['local', 'fast', 'lightweight', 'free'],
    features: ['Fast inference', 'Small models', 'Multiple languages', 'SSML'],
    localModel: true,
    website: 'https://github.com/snakers4/silero-models',
    pricing: 'Free, open-source',
  },
  styletts2: {
    name: 'StyleTTS2',
    description: 'State-of-the-art local TTS with style diffusion.',
    tags: ['local', 'style-transfer', 'high-quality', 'open-source'],
    features: ['Style diffusion', 'Human-level quality', 'Zero-shot', 'Expressive'],
    localModel: true,
    website: 'https://github.com/yl4579/StyleTTS2',
    pricing: 'Free, open-source',
  },
  fish: {
    name: 'Fish Speech',
    description: 'Ultra-fast local TTS with streaming support.',
    tags: ['local', 'fast', 'streaming', 'open-source'],
    features: ['Fast inference', 'Streaming', 'Voice cloning', 'Multilingual'],
    localModel: true,
    website: 'https://github.com/fishaudio/fish-speech',
    pricing: 'Free, open-source',
  },
};

// Fill in any missing providers with default data
function getProviderData(id: string, type: 'stt' | 'tts' | 'realtime'): ProviderEntry {
  const data = PROVIDER_DATA[id];
  const configured = false; // Will be updated with real data

  if (data) {
    return { id, type, configured, ...data };
  }

  // Default for unknown providers
  return {
    id,
    type,
    name: id.replace(/_/g, ' ').replace(/\b\w/g, l => l.toUpperCase()),
    description: `${id} provider`,
    tags: [],
    features: [],
    configured,
  };
}

/**
 * Provider detail panel
 */
const ProviderDetail: React.FC<{ provider: ProviderEntry }> = ({ provider }) => {
  return (
    <Box flexDirection="column" borderStyle="single" borderColor="cyan" paddingX={2} paddingY={1}>
      <Box marginBottom={1}>
        <Text bold color="cyan">{provider.name}</Text>
        <Text> </Text>
        <ProviderTypeBadge type={provider.type} />
        {provider.configured && (
          <>
            <Text> </Text>
            <Badge label="Configured" variant="success" />
          </>
        )}
        {provider.localModel && (
          <>
            <Text> </Text>
            <Badge label="Local (Soon)" variant="warning" />
          </>
        )}
      </Box>

      <Box marginBottom={1}>
        <Text wrap="wrap">{provider.description}</Text>
      </Box>

      {provider.features.length > 0 && (
        <Box flexDirection="column" marginBottom={1}>
          <Text bold dimColor>Features:</Text>
          <Box flexWrap="wrap">
            {provider.features.map(feature => (
              <Box key={feature} marginRight={1}>
                <Text dimColor>- {feature}</Text>
              </Box>
            ))}
          </Box>
        </Box>
      )}

      <Box flexDirection="column">
        <Text bold dimColor>Tags:</Text>
        <Box flexWrap="wrap">
          {provider.tags.map(tag => (
            <Box key={tag} marginRight={1}>
              <Badge label={tag} variant="default" style="outline" />
            </Box>
          ))}
        </Box>
      </Box>

      {provider.website && (
        <Box marginTop={1}>
          <Text dimColor>Website: </Text>
          <Text color="blue" underline>{provider.website}</Text>
        </Box>
      )}

      {provider.pricing && (
        <Box>
          <Text dimColor>Pricing: </Text>
          <Text>{provider.pricing}</Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * Provider configuration panel
 * Handles cloud, hybrid, and local providers appropriately
 */
const ProviderConfigPanel: React.FC<{
  provider: ProviderEntry;
  onSave: (apiKey: string) => void;
  onCancel: () => void;
}> = ({ provider, onSave, onCancel }) => {
  const [apiKey, setApiKey] = useState('');
  const [isSaving, setIsSaving] = useState(false);

  // Determine if this provider needs API key configuration
  // Local models don't need API keys (and local inference is coming soon)
  const isLocalOnly = provider.localModel === true;
  const needsApiKey = !isLocalOnly;

  useInput((input, key) => {
    if (key.escape) {
      onCancel();
    }
  });

  const handleSubmit = () => {
    if (needsApiKey && apiKey.trim()) {
      setIsSaving(true);
      onSave(apiKey.trim());
    } else if (isLocalOnly) {
      // For local-only providers, just close the panel
      onCancel();
    }
  };

  return (
    <Box flexDirection="column" borderStyle="double" borderColor="cyan" paddingX={2} paddingY={1}>
      <Box marginBottom={1}>
        <Text bold color="cyan">Configure {provider.name}</Text>
        {isLocalOnly && (
          <>
            <Text> </Text>
            <Text color="yellow">(Local - Coming Soon)</Text>
          </>
        )}
      </Box>

      <Box marginBottom={1}>
        <Text wrap="wrap" dimColor>{provider.description}</Text>
      </Box>

      {/* Local-only provider notice - local inference not yet supported */}
      {isLocalOnly && (
        <Box marginBottom={1} flexDirection="column">
          <Text color="yellow">ℹ Local inference coming soon!</Text>
          <Text dimColor>  This provider will support local on-device inference in a future release.</Text>
          <Text dimColor>  No configuration needed at this time.</Text>
        </Box>
      )}

      {/* API key input for cloud and hybrid providers */}
      {needsApiKey && (
        <>
          {provider.website && (
            <Box marginBottom={1}>
              <Text dimColor>Get API key: </Text>
              <Text color="blue" underline>{provider.website}</Text>
            </Box>
          )}

          <Box marginBottom={1}>
            <Text>API Key: </Text>
            <Box borderStyle="single" borderColor="cyan" paddingX={1} width={50}>
              <TextInput
                value={apiKey}
                onChange={setApiKey}
                placeholder="Enter your API key..."
                mask="*"
                onSubmit={handleSubmit}
              />
            </Box>
          </Box>
        </>
      )}

      {provider.pricing && (
        <Box marginBottom={1}>
          <Text dimColor>Pricing: </Text>
          <Text>{provider.pricing}</Text>
        </Box>
      )}

      {isSaving ? (
        <Text color="yellow">Saving...</Text>
      ) : (
        <Box>
          {needsApiKey ? (
            <Text dimColor>[Enter] Save  [Esc] Cancel</Text>
          ) : (
            <Text dimColor>[Esc] Close</Text>
          )}
        </Box>
      )}
    </Box>
  );
};

/**
 * Interactive provider browser component
 */
const ProviderBrowser: React.FC<{ options: ProviderBrowseOptions }> = ({ options }) => {
  const { exit } = useApp();
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [isSearchFocused, setIsSearchFocused] = useState(true);
  const [selectedType, setSelectedType] = useState<'all' | 'stt' | 'tts' | 'realtime'>(
    options.type || 'all'
  );
  const [configuring, setConfiguring] = useState<ProviderEntry | null>(null);
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null);

  const [configuredProviders, setConfiguredProviders] = useState<Set<string>>(() => {
    const config = configManager.get();
    return new Set(Object.keys(config.providers));
  });

  // Build full provider list
  const allProviders = useMemo(() => {
    const providers: ProviderEntry[] = [];

    if (selectedType === 'all' || selectedType === 'stt') {
      providers.push(...STT_PROVIDERS.map(id => {
        const p = getProviderData(id, 'stt');
        p.configured = configuredProviders.has(id);
        return p;
      }));
    }
    if (selectedType === 'all' || selectedType === 'tts') {
      providers.push(...TTS_PROVIDERS.map(id => {
        const p = getProviderData(id, 'tts');
        p.configured = configuredProviders.has(id);
        return p;
      }));
    }
    if (selectedType === 'all' || selectedType === 'realtime') {
      providers.push(...REALTIME_PROVIDERS.map(id => {
        const p = getProviderData(id, 'realtime');
        p.configured = configuredProviders.has(id);
        return p;
      }));
    }

    return providers;
  }, [selectedType, configuredProviders]);

  // Filter providers by search query
  const filteredProviders = useMemo(() => {
    if (!searchQuery.trim()) {
      return allProviders;
    }

    const query = searchQuery.toLowerCase().trim();
    return allProviders.filter(provider => {
      if (provider.id.toLowerCase().includes(query)) {
        return true;
      }
      if (provider.name.toLowerCase().includes(query)) {
        return true;
      }
      if (provider.description.toLowerCase().includes(query)) {
        return true;
      }
      if (provider.tags.some(tag => tag.toLowerCase().includes(query))) {
        return true;
      }
      if (provider.features.some(f => f.toLowerCase().includes(query))) {
        return true;
      }
      return false;
    });
  }, [allProviders, searchQuery]);

  // Reset selection when filter changes
  useEffect(() => {
    setSelectedIndex(0);
  }, [searchQuery, selectedType]);

  // Handle saving provider configuration
  const handleSaveProvider = async (apiKey: string) => {
    if (!configuring) {
      return;
    }

    try {
      configManager.setProvider(configuring.id, { api_key: apiKey });
      await configManager.save();

      // Update configured providers set
      setConfiguredProviders(prev => new Set([...prev, configuring.id]));
      setMessage({ type: 'success', text: `${configuring.name} configured successfully!` });
      setConfiguring(null);

      // Clear message after 3 seconds
      setTimeout(() => setMessage(null), 3000);
    } catch (err) {
      setMessage({ type: 'error', text: `Failed to save: ${err}` });
    }
  };

  // Handle keyboard input
  useInput((input, key) => {
    // If configuring, don't handle other inputs
    if (configuring) {
      return;
    }

    if (key.escape) {
      exit();
      return;
    }

    // Type filter shortcuts
    if (!isSearchFocused) {
      if (input === '1') {
        setSelectedType('all');
      }
      if (input === '2') {
        setSelectedType('stt');
      }
      if (input === '3') {
        setSelectedType('tts');
      }
      if (input === '4') {
        setSelectedType('realtime');
      }

      if (key.upArrow) {
        setSelectedIndex(i => Math.max(0, i - 1));
      } else if (key.downArrow) {
        setSelectedIndex(i => Math.min(filteredProviders.length - 1, i + 1));
      } else if (key.return) {
        // Enter key - always allow configuration for any provider
        const provider = filteredProviders[selectedIndex];
        if (provider) {
          setConfiguring(provider);
        }
      } else if (input === '/') {
        setIsSearchFocused(true);
      }
    }

    if (key.tab) {
      setIsSearchFocused(!isSearchFocused);
    }
  });

  const selectedProvider = filteredProviders[selectedIndex];
  const maxDisplay = 12;
  const startIndex = Math.max(0, selectedIndex - Math.floor(maxDisplay / 2));
  const displayProviders = filteredProviders.slice(startIndex, startIndex + maxDisplay);

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Provider Browser')}</Text>
        <Text dimColor> - {allProviders.length} providers available</Text>
      </Box>

      {/* Search and filter bar */}
      <Box marginBottom={1}>
        <Box marginRight={2}>
          <Text color={isSearchFocused ? 'cyan' : 'gray'}>Search: </Text>
          <Box borderStyle="single" borderColor={isSearchFocused ? 'cyan' : 'gray'} paddingX={1} width={40}>
            <TextInput
              value={searchQuery}
              onChange={setSearchQuery}
              placeholder="Search providers..."
              focus={isSearchFocused}
              onSubmit={() => setIsSearchFocused(false)}
            />
          </Box>
        </Box>

        <Box>
          <Text dimColor>Filter: </Text>
          <Box marginLeft={1}>
            <Text
              color={selectedType === 'all' ? 'cyan' : 'gray'}
              bold={selectedType === 'all'}
            >
              [1] All
            </Text>
          </Box>
          <Box marginLeft={1}>
            <Text
              color={selectedType === 'stt' ? 'blue' : 'gray'}
              bold={selectedType === 'stt'}
            >
              [2] STT
            </Text>
          </Box>
          <Box marginLeft={1}>
            <Text
              color={selectedType === 'tts' ? 'magenta' : 'gray'}
              bold={selectedType === 'tts'}
            >
              [3] TTS
            </Text>
          </Box>
          <Box marginLeft={1}>
            <Text
              color={selectedType === 'realtime' ? 'yellow' : 'gray'}
              bold={selectedType === 'realtime'}
            >
              [4] Realtime
            </Text>
          </Box>
        </Box>
      </Box>

      {/* Main content area */}
      <Box>
        {/* Provider list */}
        <Box flexDirection="column" width={45} marginRight={1}>
          <Box borderStyle="single" borderColor="gray" flexDirection="column" paddingX={1}>
            {filteredProviders.length === 0 ? (
              <Box paddingY={1}>
                <Text dimColor>No providers found</Text>
              </Box>
            ) : (
              displayProviders.map((provider, index) => {
                const actualIndex = startIndex + index;
                const isSelected = actualIndex === selectedIndex && !isSearchFocused;

                return (
                  <Box key={`${provider.type}-${provider.id}`}>
                    <Box width={2}>
                      <Text color={isSelected ? 'cyan' : undefined}>
                        {isSelected ? '>' : ' '}
                      </Text>
                    </Box>
                    <Box width={3}>
                      {provider.configured ? (
                        <Text color="green">✓</Text>
                      ) : (
                        <Text dimColor>○</Text>
                      )}
                    </Box>
                    <Box width={22}>
                      <Text
                        bold={isSelected}
                        color={isSelected ? 'cyan' : provider.configured ? 'green' : undefined}
                      >
                        {provider.name.length > 20 ? provider.name.slice(0, 18) + '..' : provider.name}
                      </Text>
                    </Box>
                    <Box width={8}>
                      <Text color={
                        provider.type === 'stt' ? 'blue' :
                        provider.type === 'tts' ? 'magenta' :
                        'yellow'
                      } dimColor>
                        {provider.type.toUpperCase()}
                      </Text>
                    </Box>
                    {provider.localModel && (
                      <Text color="yellow" dimColor>L</Text>
                    )}
                  </Box>
                );
              })
            )}
          </Box>

          {filteredProviders.length > maxDisplay && (
            <Box marginTop={0}>
              <Text dimColor>
                {startIndex + 1}-{Math.min(startIndex + maxDisplay, filteredProviders.length)} of {filteredProviders.length}
              </Text>
            </Box>
          )}
        </Box>

        {/* Provider detail */}
        <Box flexDirection="column" flexGrow={1}>
          {selectedProvider && <ProviderDetail provider={selectedProvider} />}
        </Box>
      </Box>

      {/* Configuration panel */}
      {configuring && (
        <Box marginTop={1}>
          <ProviderConfigPanel
            provider={configuring}
            onSave={handleSaveProvider}
            onCancel={() => setConfiguring(null)}
          />
        </Box>
      )}

      {/* Message */}
      {message && (
        <Box marginTop={1}>
          <Text color={message.type === 'success' ? 'green' : 'red'}>
            {message.type === 'success' ? '✓ ' : '✗ '}{message.text}
          </Text>
        </Box>
      )}

      {/* Help */}
      <Box marginTop={1}>
        <Text dimColor>
          [/] Search  [Tab] Toggle  [↑↓] Navigate  [Enter] Configure  [1-4] Filter  [Esc] Exit
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Provider browse command handler
 */
export async function providerBrowseCommand(options: ProviderBrowseOptions): Promise<void> {
  await configManager.initialize({
    configPath: options.config,
    profile: options.profile,
  });

  const { waitUntilExit } = render(<ProviderBrowser options={options} />);
  await waitUntilExit();
}

export default providerBrowseCommand;

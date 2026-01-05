/**
 * Bud Widget - Embeddable voice widget for Bud Foundry AI Gateway
 *
 * Usage:
 *   <script src="https://cdn.bud.ai/bud-widget.js"></script>
 *   <bud-widget
 *     data-gateway-url="wss://api.bud.ai/ws"
 *     data-api-key="your-api-key"
 *     data-stt-provider="deepgram"
 *     data-tts-provider="elevenlabs"
 *     data-tts-voice="rachel">
 *   </bud-widget>
 *
 * JavaScript API:
 *   const widget = document.querySelector('bud-widget');
 *   widget.speak('Hello, world!');
 *   widget.addEventListener('transcript', (e) => console.log(e.detail.text));
 */

import { BudWidget, defineWidget } from './widget';
import { mergeConfig } from './config';
import type {
  WidgetConfig,
  WidgetState,
  TranscriptResult,
  AudioChunk,
  WidgetMetrics,
  WidgetEventMap,
  STTConfig,
  TTSConfig,
  FeatureFlags,
} from './types';

// Auto-register the custom element
defineWidget();

// Export everything for programmatic usage
export { BudWidget, defineWidget, mergeConfig };
export type {
  WidgetConfig,
  WidgetState,
  TranscriptResult,
  AudioChunk,
  WidgetMetrics,
  WidgetEventMap,
  STTConfig,
  TTSConfig,
  FeatureFlags,
};

// Factory function for programmatic creation
export function createWidget(config: Partial<WidgetConfig>): BudWidget {
  const widget = document.createElement('bud-widget') as BudWidget;

  // Set data attributes from config
  if (config.gatewayUrl) {
    widget.dataset.gatewayUrl = config.gatewayUrl;
  }
  if (config.apiKey) {
    widget.dataset.apiKey = config.apiKey;
  }
  if (config.theme) {
    widget.dataset.theme = config.theme;
  }
  if (config.position) {
    widget.dataset.position = config.position;
  }
  if (config.mode) {
    widget.dataset.mode = config.mode;
  }
  if (config.showMetrics) {
    widget.dataset.showMetrics = 'true';
  }
  if (config.stt?.provider) {
    widget.dataset.sttProvider = config.stt.provider;
  }
  if (config.stt?.language) {
    widget.dataset.sttLanguage = config.stt.language;
  }
  if (config.tts?.provider) {
    widget.dataset.ttsProvider = config.tts.provider;
  }
  if (config.tts?.voice) {
    widget.dataset.ttsVoice = config.tts.voice;
  }
  if (config.tts?.voiceId) {
    widget.dataset.ttsVoiceId = config.tts.voiceId;
  }

  return widget;
}

// Attach to window for UMD usage
if (typeof window !== 'undefined') {
  (window as any).BudWidget = {
    create: createWidget,
    define: defineWidget,
    Widget: BudWidget,
  };
}

// Default export
export default BudWidget;

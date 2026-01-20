/**
 * WaaV CLI Voice Start Command
 *
 * Start a voice conversation
 */

import React, { useState, useEffect, useRef } from 'react';
import { render, Text, Box, useInput, useApp } from 'ink';
import { configManager } from '../../core/config/manager.js';
import { WebSocketClient } from '../../core/gateway/websocket.js';
import { AudioRecorder } from '../../core/audio/recorder.js';
import { AudioPlayer } from '../../core/audio/player.js';
import { VAD } from '../../core/audio/vad.js';
import { GRADIENTS, VOICE_WAVE_FRAMES, levelMeter } from '../../components/branding/index.js';
import type { GlobalOptions } from '../../cli.js';
import type { STTResultMessage, TTSAudioMessage } from '../../types/index.js';

interface VoiceStartOptions extends GlobalOptions {
  stt?: string;
  tts?: string;
  mode?: 'vad' | 'push_to_talk' | 'continuous';
}

/**
 * Voice mode component
 */
const VoiceMode: React.FC<{ options: VoiceStartOptions }> = ({ options }) => {
  const { exit } = useApp();
  const [status, setStatus] = useState<'connecting' | 'ready' | 'listening' | 'processing' | 'speaking' | 'error'>('connecting');
  const [transcript, setTranscript] = useState<string[]>([]);
  const [currentTranscript, setCurrentTranscript] = useState('');
  const [audioLevel, setAudioLevel] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [frame, setFrame] = useState(0);

  const wsRef = useRef<WebSocketClient | null>(null);
  const recorderRef = useRef<AudioRecorder | null>(null);
  const playerRef = useRef<AudioPlayer | null>(null);
  const vadRef = useRef<VAD | null>(null);

  // Animation frame
  useEffect(() => {
    const interval = setInterval(() => {
      setFrame(f => (f + 1) % 60);
    }, 100);
    return () => clearInterval(interval);
  }, []);

  // Initialize voice session
  useEffect(() => {
    const init = async () => {
      try {
        await configManager.initialize({
          configPath: options.config,
          profile: options.profile,
        });

        const config = configManager.get();
        const gatewayUrl = options.gateway ?? config.gateway.url;
        const wsUrl = gatewayUrl.replace(/^http/, 'ws') + '/ws';

        // Create WebSocket client
        const ws = new WebSocketClient({
          url: wsUrl,
          apiKey: config.gateway.auth?.api_key,
        });

        // Handle events
        ws.on('ready', () => {
          setStatus('ready');
        });

        ws.on('transcript', (msg: STTResultMessage) => {
          if (msg.is_final) {
            setTranscript(prev => [...prev.slice(-4), `You: ${msg.transcript}`]);
            setCurrentTranscript('');
          } else {
            setCurrentTranscript(msg.transcript);
          }
        });

        ws.on('audio', (msg: TTSAudioMessage) => {
          setStatus('speaking');
          if (playerRef.current && msg.audio instanceof Uint8Array) {
            playerRef.current.write(Buffer.from(msg.audio));
          }
          if (msg.is_final) {
            setStatus('ready');
            playerRef.current?.end();
          }
        });

        ws.on('error', (msg) => {
          setError(msg.message);
          setStatus('error');
        });

        // Connect
        await ws.connect({
          sttConfig: {
            provider: options.stt ?? config.defaults.stt_provider,
            interim_results: true,
          },
          ttsConfig: {
            provider: options.tts ?? config.defaults.tts_provider,
          },
          features: {
            vad: config.audio.vad.enabled,
          },
        });

        wsRef.current = ws;

        // Create audio recorder
        const recorder = new AudioRecorder({
          sampleRate: config.audio.sample_rate,
        });

        recorder.on('data', (buffer) => {
          if (ws.isConnected()) {
            ws.sendAudio(buffer, {
              sampleRate: config.audio.sample_rate,
            });
          }
        });

        recorder.on('level', (level) => {
          setAudioLevel(level);
        });

        recorderRef.current = recorder;

        // Create audio player
        const player = new AudioPlayer({
          sampleRate: config.audio.sample_rate,
        });

        playerRef.current = player;

        // Create VAD
        const vad = new VAD({
          sampleRate: config.audio.sample_rate,
          threshold: config.audio.vad.threshold,
        });

        vadRef.current = vad;

        setStatus('ready');
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        setStatus('error');
      }
    };

    init();

    return () => {
      wsRef.current?.disconnect();
      recorderRef.current?.stop();
      playerRef.current?.stop();
    };
  }, [options]);

  // Handle keyboard input
  useInput((input, key) => {
    if (key.escape || (key.ctrl && input === 'c')) {
      wsRef.current?.disconnect();
      recorderRef.current?.stop();
      exit();
    }

    // Space to toggle recording in push-to-talk mode
    if (input === ' ') {
      if (recorderRef.current?.isRecording()) {
        recorderRef.current.stop();
        wsRef.current?.sendFlush();
        setStatus('processing');
      } else if (status === 'ready') {
        recorderRef.current?.start();
        setStatus('listening');
      }
    }

    // 'm' to mute
    if (input === 'm') {
      if (recorderRef.current?.isRecording()) {
        recorderRef.current.pause();
      } else {
        recorderRef.current?.resume();
      }
    }

    // 's' to stop TTS
    if (input === 's') {
      wsRef.current?.sendInterrupt();
      playerRef.current?.stop();
      setStatus('ready');
    }
  });

  // Get status color
  const getStatusColor = () => {
    switch (status) {
      case 'listening': return 'red';
      case 'speaking': return 'green';
      case 'processing': return 'yellow';
      case 'error': return 'red';
      default: return 'cyan';
    }
  };

  // Get status text
  const getStatusText = () => {
    switch (status) {
      case 'connecting': return 'Connecting...';
      case 'ready': return 'Ready';
      case 'listening': return 'Listening...';
      case 'processing': return 'Processing...';
      case 'speaking': return 'Speaking...';
      case 'error': return `Error: ${error}`;
    }
  };

  const waveFrame = VOICE_WAVE_FRAMES[frame % VOICE_WAVE_FRAMES.length] ?? '';

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Voice Mode')}</Text>
        <Text dimColor> [ESC] Exit</Text>
      </Box>

      {/* Status */}
      <Box marginBottom={1}>
        <Text color={getStatusColor()}>●</Text>
        <Text> {getStatusText()}</Text>
      </Box>

      {/* Audio visualizer */}
      {status === 'listening' && (
        <Box marginBottom={1} flexDirection="column" alignItems="center">
          <Text color="cyan">{waveFrame}</Text>
          <Box marginTop={0}>
            <Text dimColor>Volume: </Text>
            <Text>{levelMeter(audioLevel, 15)}</Text>
          </Box>
        </Box>
      )}

      {/* Transcript */}
      <Box flexDirection="column" borderStyle="single" paddingX={1} minHeight={6}>
        <Text bold color="cyan">Transcript</Text>
        {transcript.map((line, i) => (
          <Text key={i}>{line}</Text>
        ))}
        {currentTranscript && (
          <Text dimColor>{currentTranscript}</Text>
        )}
        {transcript.length === 0 && !currentTranscript && (
          <Text dimColor>Press Space to start talking...</Text>
        )}
      </Box>

      {/* Controls */}
      <Box marginTop={1}>
        <Text dimColor>[Space] Talk  [M] Mute  [S] Stop TTS</Text>
      </Box>
    </Box>
  );
};

/**
 * Voice start command handler
 */
export async function voiceStartCommand(options: VoiceStartOptions): Promise<void> {
  const { waitUntilExit } = render(<VoiceMode options={options} />);
  await waitUntilExit();
}

export default voiceStartCommand;

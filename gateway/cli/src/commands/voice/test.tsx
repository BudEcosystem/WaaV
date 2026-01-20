/**
 * WaaV CLI Voice Test Command
 *
 * Test microphone and speaker setup
 */

import React, { useState, useEffect } from 'react';
import { render, Text, Box, useInput, useApp } from 'ink';
import { AudioRecorder } from '../../core/audio/recorder.js';
import { AudioPlayer } from '../../core/audio/player.js';
import { configManager } from '../../core/config/manager.js';
import { getAudioSystemInfo } from '../../utils/platform.js';
import { Spinner, SuccessIndicator, ErrorIndicator } from '../../components/common/index.js';
import { levelMeter, GRADIENTS } from '../../components/branding/index.js';
import type { GlobalOptions } from '../../cli.js';

/**
 * Voice test component
 */
const VoiceTest: React.FC<{ options: GlobalOptions }> = ({ options }) => {
  const { exit } = useApp();
  const [step, setStep] = useState<'info' | 'mic' | 'speaker' | 'done'>('info');
  const [audioInfo, setAudioInfo] = useState<Awaited<ReturnType<typeof getAudioSystemInfo>> | null>(null);
  const [micLevel, setMicLevel] = useState(0);
  const [micStatus, setMicStatus] = useState<'idle' | 'recording' | 'error'>('idle');
  const [speakerStatus, setSpeakerStatus] = useState<'idle' | 'playing' | 'error'>('idle');
  const [error, setError] = useState<string | null>(null);
  const [recordedAudio, setRecordedAudio] = useState<Buffer[]>([]);

  // Get audio system info
  useEffect(() => {
    const init = async () => {
      try {
        await configManager.initialize({
          configPath: options.config,
          profile: options.profile,
        });

        const info = await getAudioSystemInfo();
        setAudioInfo(info);

        if (!info.soxInstalled) {
          setError('sox is not installed. Please install it for audio recording.');
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    };

    init();
  }, [options]);

  // Handle keyboard input
  useInput(async (input, key) => {
    if (key.escape || (key.ctrl && input === 'c')) {
      exit();
    }

    if (key.return) {
      if (step === 'info') {
        // Start microphone test
        setStep('mic');
        setMicStatus('recording');

        try {
          const recorder = new AudioRecorder();
          const chunks: Buffer[] = [];

          recorder.on('data', (buffer) => {
            chunks.push(buffer);
          });

          recorder.on('level', (level) => {
            setMicLevel(level);
          });

          recorder.on('error', (err) => {
            setMicStatus('error');
            setError(err.message);
          });

          await recorder.start();

          // Record for 3 seconds
          setTimeout(() => {
            recorder.stop();
            setRecordedAudio(chunks);
            setMicStatus('idle');
            setStep('speaker');
          }, 3000);
        } catch (err) {
          setMicStatus('error');
          setError(err instanceof Error ? err.message : String(err));
        }
      } else if (step === 'speaker') {
        // Play back recorded audio
        setSpeakerStatus('playing');

        try {
          if (recordedAudio.length > 0) {
            const player = new AudioPlayer();

            player.on('complete', () => {
              setSpeakerStatus('idle');
              setStep('done');
            });

            player.on('error', (err) => {
              setSpeakerStatus('error');
              setError(err.message);
            });

            const fullAudio = Buffer.concat(recordedAudio);
            await player.play(fullAudio);
          } else {
            setStep('done');
          }
        } catch (err) {
          setSpeakerStatus('error');
          setError(err instanceof Error ? err.message : String(err));
        }
      } else if (step === 'done') {
        exit();
      }
    }
  });

  return (
    <Box flexDirection="column" paddingY={1}>
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Audio Test')}</Text>
      </Box>

      {/* Audio system info */}
      {step === 'info' && audioInfo && (
        <Box flexDirection="column">
          <Text bold color="cyan">Audio System</Text>
          <Box marginLeft={2} flexDirection="column">
            <Text>Backend: {audioInfo.backend}</Text>
            <Text>sox: {audioInfo.soxInstalled ? `✓ v${audioInfo.soxVersion}` : '✗ Not installed'}</Text>
            <Text>Input: {audioInfo.defaultInput?.name ?? 'None detected'}</Text>
            <Text>Output: {audioInfo.defaultOutput?.name ?? 'None detected'}</Text>
          </Box>

          {error && (
            <Box marginTop={1}>
              <ErrorIndicator label={error} />
            </Box>
          )}

          <Box marginTop={2}>
            <Text dimColor>Press Enter to test microphone...</Text>
          </Box>
        </Box>
      )}

      {/* Microphone test */}
      {step === 'mic' && (
        <Box flexDirection="column">
          <Text bold color="cyan">Microphone Test</Text>
          <Box marginTop={1}>
            {micStatus === 'recording' ? (
              <Box flexDirection="column">
                <Spinner label="Recording for 3 seconds... Speak now!" />
                <Box marginTop={1}>
                  <Text>Level: {levelMeter(micLevel, 20)}</Text>
                </Box>
              </Box>
            ) : micStatus === 'error' ? (
              <ErrorIndicator label={error || 'Microphone test failed'} />
            ) : (
              <SuccessIndicator label="Recording complete!" />
            )}
          </Box>
        </Box>
      )}

      {/* Speaker test */}
      {step === 'speaker' && (
        <Box flexDirection="column">
          <Text bold color="cyan">Speaker Test</Text>
          <Box marginTop={1}>
            {speakerStatus === 'playing' ? (
              <Spinner label="Playing back recording..." />
            ) : speakerStatus === 'error' ? (
              <ErrorIndicator label={error || 'Playback failed'} />
            ) : (
              <Text dimColor>Press Enter to play back your recording...</Text>
            )}
          </Box>
        </Box>
      )}

      {/* Done */}
      {step === 'done' && (
        <Box flexDirection="column">
          <SuccessIndicator label="Audio test complete!" />
          <Box marginTop={1} flexDirection="column">
            <Text>Your microphone and speakers are working correctly.</Text>
            <Text dimColor>Run "waav voice start" to begin a voice conversation.</Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Press Enter to exit...</Text>
          </Box>
        </Box>
      )}
    </Box>
  );
};

/**
 * Voice test command handler
 */
export async function voiceTestCommand(options: GlobalOptions): Promise<void> {
  const { waitUntilExit } = render(<VoiceTest options={options} />);
  await waitUntilExit();
}

export default voiceTestCommand;

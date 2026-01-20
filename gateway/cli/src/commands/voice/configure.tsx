/**
 * WaaV CLI Voice Configure Command
 *
 * Audio device configuration interface
 */

import React, { useState, useEffect } from 'react';
import { render, Text, Box, useInput, useApp } from 'ink';
import { configManager } from '../../core/config/manager.js';
import { listInputDevices, listOutputDevices } from '../../utils/platform.js';
import { Spinner, SuccessIndicator, ErrorIndicator, Select } from '../../components/common/index.js';
import { GRADIENTS } from '../../components/branding/index.js';
import type { GlobalOptions } from '../../cli.js';
import type { AudioDeviceInfo } from '../../utils/platform.js';

interface ConfigStep {
  id: 'loading' | 'input' | 'output' | 'sample_rate' | 'vad' | 'mode' | 'summary';
  label: string;
}

const STEPS: ConfigStep[] = [
  { id: 'loading', label: 'Loading audio devices' },
  { id: 'input', label: 'Select input device' },
  { id: 'output', label: 'Select output device' },
  { id: 'sample_rate', label: 'Choose sample rate' },
  { id: 'vad', label: 'Configure VAD' },
  { id: 'mode', label: 'Choose voice mode' },
  { id: 'summary', label: 'Review configuration' },
];

const SAMPLE_RATES = [
  { label: '16000 Hz (Recommended for speech)', value: 16000 },
  { label: '22050 Hz (CD Quality)', value: 22050 },
  { label: '44100 Hz (High Quality)', value: 44100 },
  { label: '48000 Hz (Professional)', value: 48000 },
];

const VAD_THRESHOLDS = [
  { label: 'Low sensitivity (0.3)', value: 0.3, description: 'Ignores background noise but may miss quiet speech' },
  { label: 'Medium sensitivity (0.5)', value: 0.5, description: 'Balanced detection (recommended)' },
  { label: 'High sensitivity (0.7)', value: 0.7, description: 'Detects quiet speech but may pick up noise' },
];

const VOICE_MODES = [
  { label: 'Push to Talk', value: 'push_to_talk', description: 'Hold space bar to record' },
  { label: 'Voice Activity Detection', value: 'vad', description: 'Automatically detects when you speak' },
  { label: 'Continuous', value: 'continuous', description: 'Always listening (uses more bandwidth)' },
];

/**
 * Voice configuration component
 */
const VoiceConfigure: React.FC<{ options: GlobalOptions }> = ({ options }) => {
  const { exit } = useApp();
  const [currentStep, setCurrentStep] = useState<ConfigStep['id']>('loading');
  const [inputDevices, setInputDevices] = useState<AudioDeviceInfo[]>([]);
  const [outputDevices, setOutputDevices] = useState<AudioDeviceInfo[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Selected configuration
  const [selectedInput, setSelectedInput] = useState<string>('default');
  const [selectedOutput, setSelectedOutput] = useState<string>('default');
  const [selectedSampleRate, setSelectedSampleRate] = useState<number>(16000);
  const [selectedVadThreshold, setSelectedVadThreshold] = useState<number>(0.5);
  const [selectedVoiceMode, setSelectedVoiceMode] = useState<string>('push_to_talk');
  const [isSaving, setIsSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  // Load audio devices
  useEffect(() => {
    const loadDevices = async () => {
      try {
        await configManager.initialize({
          configPath: options.config,
          profile: options.profile,
        });

        const [inputs, outputs] = await Promise.all([
          listInputDevices(),
          listOutputDevices(),
        ]);

        setInputDevices(inputs);
        setOutputDevices(outputs);

        // Load current config values
        const config = configManager.get();
        if (config.audio.input_device) {
          setSelectedInput(config.audio.input_device);
        }
        if (config.audio.output_device) {
          setSelectedOutput(config.audio.output_device);
        }
        if (config.audio.sample_rate) {
          setSelectedSampleRate(config.audio.sample_rate);
        }
        if (config.audio.vad?.threshold) {
          setSelectedVadThreshold(config.audio.vad.threshold);
        }
        if (config.voice?.mode) {
          setSelectedVoiceMode(config.voice.mode);
        }

        setCurrentStep('input');
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    };

    loadDevices();
  }, [options]);

  // Handle keyboard navigation
  useInput((_input, key) => {
    if (key.escape) {
      exit();
    }
  });

  // Save configuration
  const saveConfig = async () => {
    setIsSaving(true);
    try {
      configManager.set('audio.input_device', selectedInput);
      configManager.set('audio.output_device', selectedOutput);
      configManager.set('audio.sample_rate', selectedSampleRate);
      configManager.set('audio.vad.enabled', true);
      configManager.set('audio.vad.threshold', selectedVadThreshold);
      configManager.set('voice.mode', selectedVoiceMode);
      await configManager.save();
      setSaved(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setIsSaving(false);
    }
  };

  // Navigation helpers
  const goToNextStep = () => {
    const currentIndex = STEPS.findIndex(s => s.id === currentStep);
    if (currentIndex < STEPS.length - 1) {
      setCurrentStep(STEPS[currentIndex + 1]?.id ?? 'summary');
    }
  };

  const goToPrevStep = () => {
    const currentIndex = STEPS.findIndex(s => s.id === currentStep);
    if (currentIndex > 1) {
      setCurrentStep(STEPS[currentIndex - 1]?.id ?? 'input');
    }
  };

  // Render progress bar
  const renderProgress = () => {
    const currentIndex = STEPS.findIndex(s => s.id === currentStep);
    const progress = Math.round((currentIndex / (STEPS.length - 1)) * 100);

    return (
      <Box marginBottom={1}>
        <Text dimColor>
          Step {currentIndex}/{STEPS.length - 1}
          {' '}
          {'█'.repeat(Math.floor(progress / 5))}
          {'░'.repeat(20 - Math.floor(progress / 5))}
          {' '}
          {progress}%
        </Text>
      </Box>
    );
  };

  if (error) {
    return (
      <Box flexDirection="column" paddingY={1}>
        <ErrorIndicator label={error} />
        <Text dimColor>Press ESC to exit</Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column" paddingY={1}>
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Audio Configuration')}</Text>
      </Box>

      {currentStep !== 'loading' && renderProgress()}

      {/* Loading step */}
      {currentStep === 'loading' && (
        <Spinner label="Detecting audio devices..." />
      )}

      {/* Input device selection */}
      {currentStep === 'input' && (
        <Box flexDirection="column">
          <Text bold color="cyan">Select Input Device (Microphone)</Text>
          <Box marginTop={1}>
            <Select
              items={inputDevices.map(d => ({
                label: d.isDefault ? `${d.name} (Default)` : d.name,
                value: d.id,
              }))}
              onSelect={(item) => {
                setSelectedInput(item.value);
                goToNextStep();
              }}
              initialIndex={inputDevices.findIndex(d => d.id === selectedInput)}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Use arrow keys to navigate, Enter to select</Text>
          </Box>
        </Box>
      )}

      {/* Output device selection */}
      {currentStep === 'output' && (
        <Box flexDirection="column">
          <Text bold color="cyan">Select Output Device (Speaker)</Text>
          <Box marginTop={1}>
            <Select
              items={outputDevices.map(d => ({
                label: d.isDefault ? `${d.name} (Default)` : d.name,
                value: d.id,
              }))}
              onSelect={(item) => {
                setSelectedOutput(item.value);
                goToNextStep();
              }}
              initialIndex={outputDevices.findIndex(d => d.id === selectedOutput)}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Use arrow keys to navigate, Enter to select</Text>
          </Box>
        </Box>
      )}

      {/* Sample rate selection */}
      {currentStep === 'sample_rate' && (
        <Box flexDirection="column">
          <Text bold color="cyan">Select Sample Rate</Text>
          <Box marginTop={1}>
            <Select
              items={SAMPLE_RATES.map(r => ({
                label: r.label,
                value: String(r.value),
              }))}
              onSelect={(item) => {
                setSelectedSampleRate(parseInt(item.value, 10));
                goToNextStep();
              }}
              initialIndex={SAMPLE_RATES.findIndex(r => r.value === selectedSampleRate)}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>16000 Hz is recommended for most speech recognition providers</Text>
          </Box>
        </Box>
      )}

      {/* VAD configuration */}
      {currentStep === 'vad' && (
        <Box flexDirection="column">
          <Text bold color="cyan">Voice Activity Detection Sensitivity</Text>
          <Box marginTop={1}>
            <Select
              items={VAD_THRESHOLDS.map(v => ({
                label: v.label,
                value: String(v.value),
                description: v.description,
              }))}
              onSelect={(item) => {
                setSelectedVadThreshold(parseFloat(item.value));
                goToNextStep();
              }}
              initialIndex={VAD_THRESHOLDS.findIndex(v => v.value === selectedVadThreshold)}
            />
          </Box>
        </Box>
      )}

      {/* Voice mode selection */}
      {currentStep === 'mode' && (
        <Box flexDirection="column">
          <Text bold color="cyan">Select Voice Mode</Text>
          <Box marginTop={1}>
            <Select
              items={VOICE_MODES.map(m => ({
                label: m.label,
                value: m.value,
                description: m.description,
              }))}
              onSelect={(item) => {
                setSelectedVoiceMode(item.value);
                goToNextStep();
              }}
              initialIndex={VOICE_MODES.findIndex(m => m.value === selectedVoiceMode)}
            />
          </Box>
        </Box>
      )}

      {/* Summary */}
      {currentStep === 'summary' && (
        <Box flexDirection="column">
          <Text bold color="cyan">Configuration Summary</Text>
          <Box marginTop={1} flexDirection="column" borderStyle="single" paddingX={1}>
            <Box>
              <Text dimColor>Input Device: </Text>
              <Text>{inputDevices.find(d => d.id === selectedInput)?.name ?? selectedInput}</Text>
            </Box>
            <Box>
              <Text dimColor>Output Device: </Text>
              <Text>{outputDevices.find(d => d.id === selectedOutput)?.name ?? selectedOutput}</Text>
            </Box>
            <Box>
              <Text dimColor>Sample Rate: </Text>
              <Text>{selectedSampleRate} Hz</Text>
            </Box>
            <Box>
              <Text dimColor>VAD Threshold: </Text>
              <Text>{selectedVadThreshold}</Text>
            </Box>
            <Box>
              <Text dimColor>Voice Mode: </Text>
              <Text>{VOICE_MODES.find(m => m.value === selectedVoiceMode)?.label}</Text>
            </Box>
          </Box>

          {!saved && !isSaving && (
            <Box marginTop={1}>
              <Select
                items={[
                  { label: 'Save configuration', value: 'save' },
                  { label: 'Go back', value: 'back' },
                  { label: 'Cancel', value: 'cancel' },
                ]}
                onSelect={(item) => {
                  if (item.value === 'save') {
                    saveConfig();
                  } else if (item.value === 'back') {
                    goToPrevStep();
                  } else {
                    exit();
                  }
                }}
              />
            </Box>
          )}

          {isSaving && (
            <Box marginTop={1}>
              <Spinner label="Saving configuration..." />
            </Box>
          )}

          {saved && (
            <Box marginTop={1} flexDirection="column">
              <SuccessIndicator label="Configuration saved successfully!" />
              <Box marginTop={1}>
                <Text dimColor>Run "waav voice test" to verify your audio setup</Text>
              </Box>
              <Box marginTop={1}>
                <Text dimColor>Press ESC to exit</Text>
              </Box>
            </Box>
          )}
        </Box>
      )}
    </Box>
  );
};

/**
 * Voice configure command handler
 */
export async function voiceConfigureCommand(options: GlobalOptions): Promise<void> {
  const { waitUntilExit } = render(<VoiceConfigure options={options} />);
  await waitUntilExit();
}

export default voiceConfigureCommand;

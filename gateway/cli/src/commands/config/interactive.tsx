/**
 * WaaV CLI Interactive Configuration Editor
 *
 * Full-screen TUI for editing all configuration options
 * with navigable sections and inline editing.
 */

import React, { useState, useEffect, useCallback } from 'react';
import { render, Text, Box, useInput, useApp } from 'ink';
import { configManager } from '../../core/config/manager.js';
import { maskSensitive } from '../../utils/helpers.js';
import { logger } from '../../utils/logger.js';
import {
  BoxFrame,
  TextInput,
  Spinner,
} from '../../components/common/index.js';
import { GRADIENTS } from '../../components/branding/index.js';
import type { GlobalOptions } from '../../cli.js';
import type { WaavConfig } from '../../types/config.js';

type ConfigSection = 'gateway' | 'providers' | 'audio' | 'voice' | 'ui';

interface ConfigField {
  key: string;
  label: string;
  path: string;
  type: 'text' | 'number' | 'boolean' | 'select' | 'password';
  options?: string[];
  description?: string;
}

const SECTIONS: { id: ConfigSection; label: string; icon: string }[] = [
  { id: 'gateway', label: 'Gateway', icon: '⚡' },
  { id: 'providers', label: 'Providers', icon: '🔌' },
  { id: 'audio', label: 'Audio', icon: '🎤' },
  { id: 'voice', label: 'Voice Mode', icon: '🗣️' },
  { id: 'ui', label: 'UI Settings', icon: '🎨' },
];

const GATEWAY_FIELDS: ConfigField[] = [
  { key: 'url', label: 'Gateway URL', path: 'gateway.url', type: 'text', description: 'URL of the WaaV gateway server' },
  { key: 'auth_key', label: 'API Key', path: 'gateway.auth.api_key', type: 'password', description: 'Authentication API key (optional)' },
  { key: 'timeout', label: 'Timeout (ms)', path: 'gateway.timeout', type: 'number', description: 'Request timeout in milliseconds' },
];

const PROVIDER_FIELDS: ConfigField[] = [
  { key: 'stt', label: 'Default STT Provider', path: 'defaults.stt_provider', type: 'select',
    options: ['deepgram', 'google', 'azure', 'whisper', 'assembly'], description: 'Speech-to-text provider' },
  { key: 'tts', label: 'Default TTS Provider', path: 'defaults.tts_provider', type: 'select',
    options: ['elevenlabs', 'cartesia', 'google', 'azure', 'openai'], description: 'Text-to-speech provider' },
  { key: 'realtime', label: 'Realtime Provider', path: 'defaults.realtime_provider', type: 'select',
    options: ['', 'openai', 'gemini'], description: 'Realtime audio provider (optional)' },
];

const AUDIO_FIELDS: ConfigField[] = [
  { key: 'input', label: 'Input Device', path: 'audio.input_device', type: 'text', description: 'Microphone device name' },
  { key: 'output', label: 'Output Device', path: 'audio.output_device', type: 'text', description: 'Speaker device name' },
  { key: 'sample_rate', label: 'Sample Rate (Hz)', path: 'audio.sample_rate', type: 'select',
    options: ['8000', '16000', '22050', '44100', '48000'], description: 'Audio sample rate' },
  { key: 'vad_enabled', label: 'VAD Enabled', path: 'audio.vad.enabled', type: 'boolean', description: 'Voice Activity Detection' },
  { key: 'vad_threshold', label: 'VAD Threshold', path: 'audio.vad.threshold', type: 'number', description: 'VAD sensitivity (0.0-1.0)' },
];

const VOICE_FIELDS: ConfigField[] = [
  { key: 'mode', label: 'Voice Mode', path: 'voice.mode', type: 'select',
    options: ['vad', 'push_to_talk', 'continuous'], description: 'Voice activation mode' },
  { key: 'ptt_key', label: 'Push-to-Talk Key', path: 'voice.push_to_talk_key', type: 'text', description: 'Key to hold for PTT mode' },
  { key: 'auto_play', label: 'Auto-play Response', path: 'voice.auto_play_response', type: 'boolean', description: 'Auto-play TTS response' },
];

const UI_FIELDS: ConfigField[] = [
  { key: 'theme', label: 'Theme', path: 'ui.theme', type: 'select', options: ['dark', 'light', 'auto'], description: 'Color theme' },
  { key: 'animations', label: 'Animations', path: 'ui.animations', type: 'boolean', description: 'Enable UI animations' },
  { key: 'verbose', label: 'Verbose Mode', path: 'ui.verbose', type: 'boolean', description: 'Show detailed output' },
];

const SECTION_FIELDS: Record<ConfigSection, ConfigField[]> = {
  gateway: GATEWAY_FIELDS,
  providers: PROVIDER_FIELDS,
  audio: AUDIO_FIELDS,
  voice: VOICE_FIELDS,
  ui: UI_FIELDS,
};

/**
 * Get nested value from config object
 */
function getConfigValue(config: WaavConfig, path: string): unknown {
  const parts = path.split('.');
  let current: unknown = config;
  for (const part of parts) {
    if (current === null || current === undefined) return undefined;
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}

/**
 * Format value for display
 */
function formatValue(value: unknown, field: ConfigField): string {
  if (value === undefined || value === null || value === '') {
    return '(not set)';
  }
  if (field.type === 'password' && typeof value === 'string') {
    return maskSensitive(value);
  }
  if (field.type === 'boolean') {
    return value ? 'Yes' : 'No';
  }
  return String(value);
}

/**
 * Field Editor Component
 */
const FieldEditor: React.FC<{
  field: ConfigField;
  value: unknown;
  onChange: (value: unknown) => void;
  onSubmit: () => void;
  onCancel: () => void;
}> = ({ field, value, onChange, onSubmit, onCancel }) => {
  const [inputValue, setInputValue] = useState(String(value ?? ''));

  useInput((input, key) => {
    if (key.escape) {
      onCancel();
      return;
    }
    if (key.return) {
      // Convert value based on type
      let finalValue: unknown = inputValue;
      if (field.type === 'number') {
        finalValue = parseFloat(inputValue) || 0;
      } else if (field.type === 'boolean') {
        finalValue = inputValue.toLowerCase() === 'yes' || inputValue.toLowerCase() === 'true' || inputValue === '1';
      }
      onChange(finalValue);
      onSubmit();
      return;
    }
  });

  if (field.type === 'select' && field.options) {
    return (
      <Box flexDirection="column">
        <Text color="yellow">{field.label}:</Text>
        {field.options.map((opt, idx) => (
          <Box key={opt || 'empty'} marginLeft={2}>
            <Text color={opt === String(value) ? 'cyan' : undefined}>
              {opt === String(value) ? '▶ ' : '  '}
              {opt || '(none)'}
            </Text>
          </Box>
        ))}
        <Text dimColor>Use ↑↓ to select, Enter to confirm, Esc to cancel</Text>
      </Box>
    );
  }

  if (field.type === 'boolean') {
    return (
      <Box flexDirection="column">
        <Text color="yellow">{field.label}: </Text>
        <Box marginLeft={2}>
          <Text color={value ? 'cyan' : undefined}>{value ? '▶ ' : '  '}Yes</Text>
        </Box>
        <Box marginLeft={2}>
          <Text color={!value ? 'cyan' : undefined}>{!value ? '▶ ' : '  '}No</Text>
        </Box>
        <Text dimColor>Use ↑↓ to select, Enter to confirm, Esc to cancel</Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      <Box>
        <Text color="yellow">{field.label}: </Text>
        <TextInput
          value={inputValue}
          onChange={setInputValue}
          mask={field.type === 'password' ? '*' : undefined}
          autoFocus
        />
      </Box>
      <Text dimColor>Enter to save, Esc to cancel</Text>
    </Box>
  );
};

/**
 * Section View Component
 */
const SectionView: React.FC<{
  section: ConfigSection;
  config: WaavConfig;
  selectedField: number;
  editingField: number | null;
  onEditField: (index: number) => void;
  onSaveField: (field: ConfigField, value: unknown) => void;
  onCancelEdit: () => void;
}> = ({ section, config, selectedField, editingField, onEditField, onSaveField, onCancelEdit }) => {
  const fields = SECTION_FIELDS[section];

  return (
    <Box flexDirection="column">
      {fields.map((field, index) => {
        const value = getConfigValue(config, field.path);
        const isSelected = index === selectedField;
        const isEditing = index === editingField;

        if (isEditing) {
          return (
            <Box key={field.key} marginY={1}>
              <FieldEditor
                field={field}
                value={value}
                onChange={(newValue) => onSaveField(field, newValue)}
                onSubmit={onCancelEdit}
                onCancel={onCancelEdit}
              />
            </Box>
          );
        }

        return (
          <Box key={field.key} marginY={0}>
            <Text color={isSelected ? 'cyan' : undefined}>
              {isSelected ? '▶ ' : '  '}
            </Text>
            <Box width={25}>
              <Text color={isSelected ? 'cyan' : 'white'} bold={isSelected}>
                {field.label}
              </Text>
            </Box>
            <Text color={value ? 'green' : 'gray'}>
              {formatValue(value, field)}
            </Text>
          </Box>
        );
      })}
    </Box>
  );
};

/**
 * Interactive Config Editor Component
 */
const InteractiveConfigEditor: React.FC<{
  options: GlobalOptions;
  onComplete: () => void;
}> = ({ options, onComplete }) => {
  const { exit } = useApp();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [config, setConfig] = useState<WaavConfig | null>(null);
  const [dirty, setDirty] = useState(false);
  const [message, setMessage] = useState<{ text: string; type: 'success' | 'error' | 'info' } | null>(null);

  // Navigation state
  const [selectedSection, setSelectedSection] = useState(0);
  const [selectedField, setSelectedField] = useState(0);
  const [editingField, setEditingField] = useState<number | null>(null);
  const [selectingOption, setSelectingOption] = useState(false);
  const [selectedOption, setSelectedOption] = useState(0);

  const currentSection = SECTIONS[selectedSection]?.id ?? 'gateway';
  const currentFields = SECTION_FIELDS[currentSection];
  const currentField = currentFields[selectedField];

  // Initialize config
  useEffect(() => {
    const init = async () => {
      try {
        await configManager.initialize({
          configPath: options.config,
          profile: options.profile,
        });
        setConfig(configManager.get());
        setLoading(false);
      } catch (err) {
        setMessage({ text: `Failed to load config: ${err}`, type: 'error' });
        setLoading(false);
      }
    };
    init();
  }, [options]);

  // Save configuration
  const saveConfig = useCallback(async () => {
    if (!dirty) return;
    setSaving(true);
    try {
      await configManager.save();
      setDirty(false);
      setMessage({ text: 'Configuration saved!', type: 'success' });
      setTimeout(() => setMessage(null), 2000);
    } catch (err) {
      setMessage({ text: `Failed to save: ${err}`, type: 'error' });
    }
    setSaving(false);
  }, [dirty]);

  // Update a field value
  const updateField = useCallback((field: ConfigField, value: unknown) => {
    configManager.setValue(field.path, value);
    setConfig({ ...configManager.get() });
    setDirty(true);
    setMessage({ text: `Updated ${field.label}`, type: 'info' });
    setTimeout(() => setMessage(null), 1500);
  }, []);

  // Handle keyboard navigation
  useInput((input, key) => {
    if (loading) return;

    // Handle option selection mode (for select/boolean fields)
    if (selectingOption && currentField) {
      const fieldValue = config ? getConfigValue(config, currentField.path) : undefined;

      if (currentField.type === 'boolean') {
        if (key.upArrow || key.downArrow || input === 'j' || input === 'k') {
          setSelectedOption((prev) => prev === 0 ? 1 : 0);
          return;
        }
        if (key.return) {
          updateField(currentField, selectedOption === 0);
          setSelectingOption(false);
          setEditingField(null);
          return;
        }
      } else if (currentField.type === 'select' && currentField.options) {
        const opts = currentField.options;
        if (key.upArrow || input === 'k') {
          setSelectedOption((prev) => Math.max(0, prev - 1));
          return;
        }
        if (key.downArrow || input === 'j') {
          setSelectedOption((prev) => Math.min(opts.length - 1, prev + 1));
          return;
        }
        if (key.return) {
          updateField(currentField, opts[selectedOption]);
          setSelectingOption(false);
          setEditingField(null);
          return;
        }
      }

      if (key.escape) {
        setSelectingOption(false);
        setEditingField(null);
        return;
      }
      return;
    }

    // Handle text editing mode
    if (editingField !== null) {
      // TextInput handles the input
      return;
    }

    // Global shortcuts
    if (key.escape || input === 'q') {
      if (dirty) {
        // TODO: Prompt to save
      }
      onComplete();
      return;
    }

    // Save
    if (input === 's' && key.ctrl) {
      saveConfig();
      return;
    }

    if (input === 's') {
      saveConfig();
      return;
    }

    // Section navigation (Tab or numbers)
    if (key.tab || (input >= '1' && input <= '5')) {
      if (key.tab) {
        if (key.shift) {
          setSelectedSection((prev) => (prev - 1 + SECTIONS.length) % SECTIONS.length);
        } else {
          setSelectedSection((prev) => (prev + 1) % SECTIONS.length);
        }
      } else {
        const idx = parseInt(input) - 1;
        if (idx >= 0 && idx < SECTIONS.length) {
          setSelectedSection(idx);
        }
      }
      setSelectedField(0);
      return;
    }

    // Field navigation
    if (key.upArrow || input === 'k') {
      setSelectedField((prev) => Math.max(0, prev - 1));
      return;
    }
    if (key.downArrow || input === 'j') {
      setSelectedField((prev) => Math.min(currentFields.length - 1, prev + 1));
      return;
    }

    // Edit field
    if (key.return || input === 'e') {
      if (currentField) {
        if (currentField.type === 'select' || currentField.type === 'boolean') {
          // Enter selection mode
          setEditingField(selectedField);
          setSelectingOption(true);
          // Pre-select current value
          if (currentField.type === 'boolean') {
            const val = config ? getConfigValue(config, currentField.path) : false;
            setSelectedOption(val ? 0 : 1);
          } else if (currentField.options) {
            const val = config ? String(getConfigValue(config, currentField.path) ?? '') : '';
            const idx = currentField.options.indexOf(val);
            setSelectedOption(idx >= 0 ? idx : 0);
          }
        } else {
          setEditingField(selectedField);
        }
      }
      return;
    }

    // Left/Right for section navigation
    if (key.leftArrow || input === 'h') {
      setSelectedSection((prev) => (prev - 1 + SECTIONS.length) % SECTIONS.length);
      setSelectedField(0);
      return;
    }
    if (key.rightArrow || input === 'l') {
      setSelectedSection((prev) => (prev + 1) % SECTIONS.length);
      setSelectedField(0);
      return;
    }
  }, { isActive: !loading && editingField === null || selectingOption });

  if (loading) {
    return (
      <Box flexDirection="column" paddingY={1}>
        <Spinner label="Loading configuration..." color="cyan" />
      </Box>
    );
  }

  if (!config) {
    return (
      <Box flexDirection="column" paddingY={1}>
        <Text color="red">Failed to load configuration</Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      {/* Header */}
      <BoxFrame
        title={`${GRADIENTS.waav('WaaV')} Configuration Editor`}
        variant="accent"
        width={80}
      >
        <Box justifyContent="space-between">
          <Text>
            Profile: <Text color="cyan">{configManager.getProfile()}</Text>
          </Text>
          <Text>
            {dirty && <Text color="yellow">[Modified] </Text>}
            <Text dimColor>Press [S] to save</Text>
          </Text>
        </Box>
      </BoxFrame>

      {/* Section tabs */}
      <Box marginY={1}>
        {SECTIONS.map((section, index) => (
          <Box key={section.id} marginRight={2}>
            <Text
              color={index === selectedSection ? 'cyan' : 'gray'}
              bold={index === selectedSection}
              inverse={index === selectedSection}
            >
              {' '}{index + 1}:{section.icon} {section.label}{' '}
            </Text>
          </Box>
        ))}
      </Box>

      {/* Current section content */}
      <BoxFrame
        title={`${SECTIONS[selectedSection]?.icon ?? ''} ${SECTIONS[selectedSection]?.label ?? ''}`}
        variant={selectedSection === 0 ? 'accent' : selectedSection === 1 ? 'success' : selectedSection === 2 ? 'warning' : selectedSection === 3 ? 'info' : 'error'}
        width={80}
        minHeight={12}
      >
        {selectingOption && currentField ? (
          // Selection mode for boolean/select fields
          <Box flexDirection="column">
            <Text color="yellow" bold>{currentField.label}</Text>
            {currentField.description && (
              <Text dimColor>{currentField.description}</Text>
            )}
            <Box marginTop={1} flexDirection="column">
              {currentField.type === 'boolean' ? (
                <>
                  <Text color={selectedOption === 0 ? 'cyan' : undefined}>
                    {selectedOption === 0 ? '▶ ' : '  '}Yes
                  </Text>
                  <Text color={selectedOption === 1 ? 'cyan' : undefined}>
                    {selectedOption === 1 ? '▶ ' : '  '}No
                  </Text>
                </>
              ) : (
                currentField.options?.map((opt, idx) => (
                  <Text key={opt || 'empty'} color={idx === selectedOption ? 'cyan' : undefined}>
                    {idx === selectedOption ? '▶ ' : '  '}{opt || '(none)'}
                  </Text>
                ))
              )}
            </Box>
            <Box marginTop={1}>
              <Text dimColor>↑↓ to select, Enter to confirm, Esc to cancel</Text>
            </Box>
          </Box>
        ) : editingField !== null && currentField ? (
          // Text edit mode
          <Box flexDirection="column">
            <Text color="yellow" bold>{currentField.label}</Text>
            {currentField.description && (
              <Text dimColor>{currentField.description}</Text>
            )}
            <Box marginTop={1}>
              <TextInput
                value={String(getConfigValue(config, currentField.path) ?? '')}
                onChange={(value) => {
                  let finalValue: unknown = value;
                  if (currentField.type === 'number') {
                    finalValue = parseFloat(value) || 0;
                  }
                  updateField(currentField, finalValue);
                }}
                onSubmit={() => setEditingField(null)}
                mask={currentField.type === 'password' ? '*' : undefined}
                autoFocus
              />
            </Box>
            <Box marginTop={1}>
              <Text dimColor>Enter to save, Esc to cancel</Text>
            </Box>
          </Box>
        ) : (
          // Normal view mode
          <SectionView
            section={currentSection}
            config={config}
            selectedField={selectedField}
            editingField={editingField}
            onEditField={setEditingField}
            onSaveField={updateField}
            onCancelEdit={() => setEditingField(null)}
          />
        )}
      </BoxFrame>

      {/* Field description */}
      {currentField && !selectingOption && editingField === null && (
        <Box marginTop={1} paddingX={1}>
          <Text dimColor>{currentField.description}</Text>
        </Box>
      )}

      {/* Message display */}
      {message && (
        <Box marginTop={1} paddingX={1}>
          <Text color={message.type === 'error' ? 'red' : message.type === 'success' ? 'green' : 'yellow'}>
            {message.text}
          </Text>
        </Box>
      )}

      {/* Help bar */}
      <Box marginTop={1} paddingX={1}>
        <Text dimColor>
          [↑↓/jk] Navigate  [←→/hl] Section  [Enter/e] Edit  [s] Save  [Tab] Next Section  [q] Quit
        </Text>
      </Box>

      {/* Config path */}
      <Box paddingX={1}>
        <Text dimColor>Config: {configManager.getConfigPath()}</Text>
      </Box>

      {saving && (
        <Box marginTop={1} paddingX={1}>
          <Spinner label="Saving..." color="yellow" />
        </Box>
      )}
    </Box>
  );
};

/**
 * Interactive config command handler
 */
export async function configInteractiveCommand(options: GlobalOptions): Promise<void> {
  return new Promise((resolve) => {
    const { waitUntilExit } = render(
      <InteractiveConfigEditor options={options} onComplete={() => resolve()} />
    );
    waitUntilExit().then(() => resolve());
  });
}

export default configInteractiveCommand;

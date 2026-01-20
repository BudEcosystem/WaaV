/**
 * WaaV CLI Text Input Component
 *
 * Text input field with various modes and validation
 */

import React, { useState, useEffect, useCallback } from 'react';
import { Text, Box, useInput } from 'ink';

/**
 * Text input props
 */
interface TextInputProps {
  value?: string;
  placeholder?: string;
  onChange?: (value: string) => void;
  onSubmit?: (value: string) => void;
  mask?: boolean | string;
  validate?: (value: string) => string | null; // Returns error message or null
  suggestions?: string[];
  autoFocus?: boolean;
  /** Dynamic focus control (alias for autoFocus with reactive updates) */
  focus?: boolean;
  disabled?: boolean;
  prefix?: string;
  suffix?: string;
  maxLength?: number;
}

/**
 * Text Input Component
 */
export const TextInput: React.FC<TextInputProps> = ({
  value: controlledValue,
  placeholder = '',
  onChange,
  onSubmit,
  mask = false,
  validate,
  suggestions = [],
  autoFocus = true,
  focus,
  disabled = false,
  prefix,
  suffix,
  maxLength,
}) => {
  // Use focus prop if provided, otherwise fall back to autoFocus
  const isActive = focus !== undefined ? focus : autoFocus;
  const [internalValue, setInternalValue] = useState(controlledValue ?? '');
  const [cursorPosition, setCursorPosition] = useState(internalValue.length);
  const [showCursor, setShowCursor] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [suggestionIndex, setSuggestionIndex] = useState(-1);

  // Use controlled or internal value
  const value = controlledValue ?? internalValue;

  // Cursor blink effect
  useEffect(() => {
    if (!isActive || disabled) {
      return;
    }

    const interval = setInterval(() => {
      setShowCursor(prev => !prev);
    }, 500);

    return () => clearInterval(interval);
  }, [isActive, disabled]);

  // Sync cursor position with value length
  useEffect(() => {
    setCursorPosition(Math.min(cursorPosition, value.length));
  }, [value, cursorPosition]);

  // Filter suggestions based on current value
  const matchingSuggestions = suggestions.filter(
    s => s.toLowerCase().startsWith(value.toLowerCase()) && s !== value
  );

  // Handle value change
  const handleChange = useCallback((newValue: string) => {
    if (maxLength && newValue.length > maxLength) {
      return;
    }

    if (controlledValue === undefined) {
      setInternalValue(newValue);
    }
    onChange?.(newValue);

    // Validate
    if (validate) {
      setError(validate(newValue));
    }
  }, [controlledValue, onChange, validate, maxLength]);

  // Handle keyboard input
  useInput((input, key) => {
    if (disabled || !isActive) {
      return;
    }

    // Submit on Enter
    if (key.return) {
      if (suggestionIndex >= 0 && matchingSuggestions[suggestionIndex]) {
        handleChange(matchingSuggestions[suggestionIndex]);
        setSuggestionIndex(-1);
      } else if (!error) {
        onSubmit?.(value);
      }
      return;
    }

    // Tab for autocomplete
    if (key.tab && matchingSuggestions.length > 0) {
      const suggestion = matchingSuggestions[Math.max(0, suggestionIndex)];
      if (suggestion) {
        handleChange(suggestion);
        setCursorPosition(suggestion.length);
      }
      return;
    }

    // Navigate suggestions
    if (key.downArrow && matchingSuggestions.length > 0) {
      setSuggestionIndex(i => Math.min(i + 1, matchingSuggestions.length - 1));
      return;
    }

    if (key.upArrow && matchingSuggestions.length > 0) {
      setSuggestionIndex(i => Math.max(i - 1, -1));
      return;
    }

    // Backspace
    if (key.backspace || key.delete) {
      if (cursorPosition > 0) {
        const newValue = value.slice(0, cursorPosition - 1) + value.slice(cursorPosition);
        handleChange(newValue);
        setCursorPosition(Math.max(0, cursorPosition - 1));
      }
      return;
    }

    // Left/Right arrows
    if (key.leftArrow) {
      setCursorPosition(Math.max(0, cursorPosition - 1));
      return;
    }

    if (key.rightArrow) {
      setCursorPosition(Math.min(value.length, cursorPosition + 1));
      return;
    }

    // Home (Ctrl+A) / End (Ctrl+E)
    if (key.ctrl && input === 'a') {
      setCursorPosition(0);
      return;
    }

    if (key.ctrl && input === 'e') {
      setCursorPosition(value.length);
      return;
    }

    // Clear line
    if (key.ctrl && input === 'u') {
      handleChange('');
      setCursorPosition(0);
      return;
    }

    // Kill to end
    if (key.ctrl && input === 'k') {
      handleChange(value.slice(0, cursorPosition));
      return;
    }

    // Regular character input
    if (input && !key.ctrl && !key.meta) {
      const newValue = value.slice(0, cursorPosition) + input + value.slice(cursorPosition);
      handleChange(newValue);
      setCursorPosition(cursorPosition + input.length);
      setSuggestionIndex(-1);
    }
  });

  // Render the displayed value (with mask if needed)
  const displayValue = mask
    ? typeof mask === 'string'
      ? mask.repeat(value.length)
      : '•'.repeat(value.length)
    : value;

  // Render with cursor
  const renderWithCursor = () => {
    if (disabled) {
      return <Text dimColor>{displayValue || placeholder}</Text>;
    }

    const beforeCursor = displayValue.slice(0, cursorPosition);
    const atCursor = displayValue[cursorPosition] || (value ? ' ' : placeholder[0] || ' ');
    const afterCursor = displayValue.slice(cursorPosition + 1) ||
      (value ? '' : placeholder.slice(1));

    return (
      <>
        <Text>{beforeCursor}</Text>
        <Text inverse={showCursor}>{atCursor}</Text>
        <Text dimColor={!value}>{afterCursor}</Text>
      </>
    );
  };

  return (
    <Box flexDirection="column">
      <Box>
        {prefix && <Text color="cyan">{prefix}</Text>}
        {renderWithCursor()}
        {suffix && <Text dimColor>{suffix}</Text>}
      </Box>

      {/* Suggestions */}
      {matchingSuggestions.length > 0 && value && (
        <Box marginTop={0} marginLeft={prefix ? prefix.length : 0}>
          {matchingSuggestions.slice(0, 5).map((suggestion, index) => (
            <Box key={suggestion}>
              <Text
                color={index === suggestionIndex ? 'cyan' : undefined}
                dimColor={index !== suggestionIndex}
              >
                {index === suggestionIndex ? '❯ ' : '  '}
                {suggestion}
              </Text>
            </Box>
          ))}
        </Box>
      )}

      {/* Error message */}
      {error && (
        <Box marginTop={0}>
          <Text color="red">✗ {error}</Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * Password input (always masked)
 */
export const PasswordInput: React.FC<Omit<TextInputProps, 'mask'>> = (props) => {
  return <TextInput {...props} mask />;
};

/**
 * Multi-line text input
 */
interface TextAreaProps {
  value?: string;
  placeholder?: string;
  onChange?: (value: string) => void;
  onSubmit?: (value: string) => void;
  rows?: number;
  disabled?: boolean;
}

export const TextArea: React.FC<TextAreaProps> = ({
  value: controlledValue,
  placeholder = '',
  onChange,
  onSubmit,
  rows = 3,
  disabled = false,
}) => {
  const [internalValue, setInternalValue] = useState(controlledValue ?? '');
  const [cursorLine, setCursorLine] = useState(0);
  const [cursorCol, setCursorCol] = useState(0);

  const value = controlledValue ?? internalValue;
  const lines = value.split('\n');

  const handleChange = useCallback((newValue: string) => {
    if (controlledValue === undefined) {
      setInternalValue(newValue);
    }
    onChange?.(newValue);
  }, [controlledValue, onChange]);

  useInput((input, key) => {
    if (disabled) {
      return;
    }

    // Submit with Ctrl+Enter
    if (key.return && key.ctrl) {
      onSubmit?.(value);
      return;
    }

    // New line with Enter
    if (key.return) {
      const newLines = [...lines];
      const currentLine = newLines[cursorLine] ?? '';
      newLines[cursorLine] = currentLine.slice(0, cursorCol);
      newLines.splice(cursorLine + 1, 0, currentLine.slice(cursorCol));
      handleChange(newLines.join('\n'));
      setCursorLine(cursorLine + 1);
      setCursorCol(0);
      return;
    }

    // Navigation
    if (key.upArrow) {
      if (cursorLine > 0) {
        setCursorLine(cursorLine - 1);
        setCursorCol(Math.min(cursorCol, (lines[cursorLine - 1] ?? '').length));
      }
      return;
    }

    if (key.downArrow) {
      if (cursorLine < lines.length - 1) {
        setCursorLine(cursorLine + 1);
        setCursorCol(Math.min(cursorCol, (lines[cursorLine + 1] ?? '').length));
      }
      return;
    }

    if (key.leftArrow) {
      if (cursorCol > 0) {
        setCursorCol(cursorCol - 1);
      } else if (cursorLine > 0) {
        setCursorLine(cursorLine - 1);
        setCursorCol((lines[cursorLine - 1] ?? '').length);
      }
      return;
    }

    if (key.rightArrow) {
      const currentLine = lines[cursorLine] ?? '';
      if (cursorCol < currentLine.length) {
        setCursorCol(cursorCol + 1);
      } else if (cursorLine < lines.length - 1) {
        setCursorLine(cursorLine + 1);
        setCursorCol(0);
      }
      return;
    }

    // Backspace
    if (key.backspace) {
      if (cursorCol > 0) {
        const newLines = [...lines];
        const currentLine = newLines[cursorLine] ?? '';
        newLines[cursorLine] = currentLine.slice(0, cursorCol - 1) + currentLine.slice(cursorCol);
        handleChange(newLines.join('\n'));
        setCursorCol(cursorCol - 1);
      } else if (cursorLine > 0) {
        const newLines = [...lines];
        const prevLine = newLines[cursorLine - 1] ?? '';
        const currentLine = newLines[cursorLine] ?? '';
        newLines[cursorLine - 1] = prevLine + currentLine;
        newLines.splice(cursorLine, 1);
        handleChange(newLines.join('\n'));
        setCursorLine(cursorLine - 1);
        setCursorCol(prevLine.length);
      }
      return;
    }

    // Character input
    if (input && !key.ctrl && !key.meta) {
      const newLines = [...lines];
      const currentLine = newLines[cursorLine] ?? '';
      newLines[cursorLine] = currentLine.slice(0, cursorCol) + input + currentLine.slice(cursorCol);
      handleChange(newLines.join('\n'));
      setCursorCol(cursorCol + input.length);
    }
  });

  return (
    <Box flexDirection="column" borderStyle="single" paddingX={1}>
      {lines.slice(0, rows).map((line, lineIndex) => (
        <Box key={lineIndex}>
          <Text dimColor>{String(lineIndex + 1).padStart(2, ' ')} │ </Text>
          {lineIndex === cursorLine ? (
            <>
              <Text>{line.slice(0, cursorCol)}</Text>
              <Text inverse>{line[cursorCol] || ' '}</Text>
              <Text>{line.slice(cursorCol + 1)}</Text>
            </>
          ) : (
            <Text>{line || (lineIndex === 0 && !value ? placeholder : '')}</Text>
          )}
        </Box>
      ))}
      {lines.length > rows && (
        <Text dimColor>... {lines.length - rows} more lines</Text>
      )}
      <Box marginTop={1}>
        <Text dimColor>Ctrl+Enter to submit</Text>
      </Box>
    </Box>
  );
};

export default TextInput;

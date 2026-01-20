/**
 * WaaV CLI Spinner Component
 *
 * Animated loading spinner with customizable styles
 */

import React, { useState, useEffect } from 'react';
import { Text, Box } from 'ink';
import {
  SPINNER_DOTS,
  SPINNER_CIRCLE,
  SPINNER_BOX,
  getRandomLoadingMessage,
} from '../branding/animations.js';

/**
 * Spinner style variants
 */
export type SpinnerStyle = 'dots' | 'circle' | 'box' | 'pulse' | 'bar';

/**
 * Spinner component props
 */
interface SpinnerProps {
  label?: string;
  style?: SpinnerStyle;
  color?: string;
  speed?: number;
  showRandomMessage?: boolean;
  messageInterval?: number;
}

/**
 * Get spinner frames for style
 */
function getSpinnerFrames(style: SpinnerStyle): string[] {
  switch (style) {
    case 'dots':
      return SPINNER_DOTS;
    case 'circle':
      return SPINNER_CIRCLE;
    case 'box':
      return SPINNER_BOX;
    case 'pulse':
      return ['◯', '◔', '◑', '◕', '●', '◕', '◑', '◔'];
    case 'bar':
      return ['▁', '▃', '▄', '▅', '▆', '▇', '█', '▇', '▆', '▅', '▄', '▃'];
    default:
      return SPINNER_DOTS;
  }
}

/**
 * Animated Spinner Component
 */
export const Spinner: React.FC<SpinnerProps> = ({
  label = 'Loading...',
  style = 'dots',
  color = 'cyan',
  speed = 80,
  showRandomMessage = false,
  messageInterval = 3000,
}) => {
  const [frame, setFrame] = useState(0);
  const [message, setMessage] = useState(label);

  const frames = getSpinnerFrames(style);

  // Animation loop
  useEffect(() => {
    const interval = setInterval(() => {
      setFrame(f => (f + 1) % frames.length);
    }, speed);

    return () => clearInterval(interval);
  }, [speed, frames.length]);

  // Random message rotation
  useEffect(() => {
    if (!showRandomMessage) {
      return;
    }

    const interval = setInterval(() => {
      setMessage(getRandomLoadingMessage());
    }, messageInterval);

    return () => clearInterval(interval);
  }, [showRandomMessage, messageInterval]);

  const spinnerChar = frames[frame] ?? frames[0] ?? '';

  return (
    <Box>
      <Text color={color}>{spinnerChar}</Text>
      <Text> {showRandomMessage ? message : label}</Text>
    </Box>
  );
};

/**
 * Success indicator with checkmark
 */
export const SuccessIndicator: React.FC<{ label?: string }> = ({ label = 'Done!' }) => {
  return (
    <Box>
      <Text color="green">✓</Text>
      <Text> {label}</Text>
    </Box>
  );
};

/**
 * Error indicator with X
 */
export const ErrorIndicator: React.FC<{ label?: string }> = ({ label = 'Error' }) => {
  return (
    <Box>
      <Text color="red">✗</Text>
      <Text> {label}</Text>
    </Box>
  );
};

/**
 * Warning indicator
 */
export const WarningIndicator: React.FC<{ label?: string }> = ({ label = 'Warning' }) => {
  return (
    <Box>
      <Text color="yellow">⚠</Text>
      <Text> {label}</Text>
    </Box>
  );
};

/**
 * Info indicator
 */
export const InfoIndicator: React.FC<{ label?: string }> = ({ label = 'Info' }) => {
  return (
    <Box>
      <Text color="blue">ℹ</Text>
      <Text> {label}</Text>
    </Box>
  );
};

/**
 * Loading step indicator with progress
 */
interface LoadingStepProps {
  status: 'pending' | 'loading' | 'complete' | 'error';
  label: string;
}

export const LoadingStep: React.FC<LoadingStepProps> = ({ status, label }) => {
  const [frame, setFrame] = useState(0);

  useEffect(() => {
    if (status === 'loading') {
      const interval = setInterval(() => {
        setFrame(f => (f + 1) % SPINNER_DOTS.length);
      }, 80);
      return () => clearInterval(interval);
    }
    return undefined;
  }, [status]);

  let indicator: React.ReactNode;
  let textColor: string | undefined;

  switch (status) {
    case 'pending':
      indicator = <Text dimColor>○</Text>;
      textColor = 'gray';
      break;
    case 'loading':
      indicator = <Text color="cyan">{SPINNER_DOTS[frame]}</Text>;
      textColor = undefined;
      break;
    case 'complete':
      indicator = <Text color="green">✓</Text>;
      textColor = undefined;
      break;
    case 'error':
      indicator = <Text color="red">✗</Text>;
      textColor = 'red';
      break;
  }

  return (
    <Box>
      {indicator}
      <Text color={textColor}> {label}</Text>
    </Box>
  );
};

/**
 * Multi-step loading indicator
 */
interface MultiStepLoaderProps {
  steps: Array<{
    id: string;
    label: string;
    status: 'pending' | 'loading' | 'complete' | 'error';
  }>;
  title?: string;
}

export const MultiStepLoader: React.FC<MultiStepLoaderProps> = ({ steps, title }) => {
  return (
    <Box flexDirection="column">
      {title && (
        <Box marginBottom={1}>
          <Text bold>{title}</Text>
        </Box>
      )}
      {steps.map(step => (
        <Box key={step.id} marginLeft={1}>
          <LoadingStep status={step.status} label={step.label} />
        </Box>
      ))}
    </Box>
  );
};

export default Spinner;

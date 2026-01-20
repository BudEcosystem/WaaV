/**
 * WaaV CLI Splash Screen Component
 *
 * Animated welcome screen shown on startup
 */

import React, { useState, useEffect, useCallback } from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { Logo } from './Logo.js';
import { pulsingText, GRADIENTS } from './animations.js';

/**
 * Splash screen props
 */
interface SplashProps {
  version?: string;
  onComplete: () => void;
  autoAdvance?: boolean;
  autoAdvanceDelay?: number;
  showPressKey?: boolean;
}

/**
 * Splash Screen Component
 *
 * Shows animated logo and waits for user input or auto-advances
 */
export const Splash: React.FC<SplashProps> = ({
  version = '1.0.0',
  onComplete,
  autoAdvance = false,
  autoAdvanceDelay = 3000,
  showPressKey = true,
}) => {
  const [frame, setFrame] = useState(0);
  const [, setAnimationComplete] = useState(false);
  const [canProceed, setCanProceed] = useState(false);
  const { exit } = useApp();

  // Animation frame counter
  useEffect(() => {
    const interval = setInterval(() => {
      setFrame(f => f + 1);
    }, 200);

    return () => clearInterval(interval);
  }, []);

  // Allow proceeding after a short delay
  useEffect(() => {
    const timeout = setTimeout(() => {
      setCanProceed(true);
    }, 1000);

    return () => clearTimeout(timeout);
  }, []);

  // Auto-advance timer
  useEffect(() => {
    if (autoAdvance && canProceed) {
      const timeout = setTimeout(() => {
        onComplete();
      }, autoAdvanceDelay);

      return () => clearTimeout(timeout);
    }
    return undefined;
  }, [autoAdvance, autoAdvanceDelay, canProceed, onComplete]);

  // Handle logo animation completion
  const handleAnimationComplete = useCallback(() => {
    setAnimationComplete(true);
  }, []);

  // Handle key press
  useInput((input, key) => {
    if (!canProceed) {
      return;
    }

    // Any key advances
    if (input || key.return || key.escape) {
      onComplete();
    }

    // Ctrl+C exits
    if (input === 'c' && key.ctrl) {
      exit();
    }
  });

  // Pulsing "Press any key" text
  const pressKeyText = showPressKey && canProceed
    ? pulsingText('Press any key to continue...', frame)
    : '';

  return (
    <Box
      flexDirection="column"
      alignItems="center"
      justifyContent="center"
      paddingY={2}
    >
      {/* Logo with animation */}
      <Logo
        size="large"
        animation="gradient"
        showSoundWave={true}
        showTagline={true}
        showVersion={true}
        version={version}
        animationSpeed={100}
        onAnimationComplete={handleAnimationComplete}
      />

      {/* Press any key prompt */}
      {showPressKey && (
        <Box marginTop={2}>
          <Text dimColor>{pressKeyText}</Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * Compact splash for quick display
 */
export const CompactSplash: React.FC<{
  version?: string;
  message?: string;
}> = ({
  version = '1.0.0',
  message,
}) => {
  const logoText = GRADIENTS.waav('WaaV');

  return (
    <Box flexDirection="column" paddingY={1}>
      <Box>
        <Text bold>{logoText}</Text>
        <Text dimColor> v{version}</Text>
      </Box>
      {message && (
        <Box marginTop={0}>
          <Text dimColor>{message}</Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * Welcome message for first run
 */
export const WelcomeMessage: React.FC<{ onContinue: () => void }> = ({ onContinue }) => {
  const [frame, setFrame] = useState(0);

  useEffect(() => {
    const interval = setInterval(() => {
      setFrame(f => f + 1);
    }, 200);
    return () => clearInterval(interval);
  }, []);

  useInput((input, key) => {
    if (key.return || input) {
      onContinue();
    }
  });

  return (
    <Box flexDirection="column" alignItems="center" paddingY={2}>
      <Logo size="medium" animation="gradient" showTagline={false} showVersion={false} />

      <Box marginTop={2} flexDirection="column" alignItems="center">
        <Text bold color="green">Welcome to WaaV CLI!</Text>
        <Text dimColor>
          Your gateway to 70+ voice AI providers
        </Text>
      </Box>

      <Box marginTop={2} flexDirection="column" paddingX={4}>
        <Text color="cyan">Quick Start:</Text>
        <Text>  1. Configure your first provider</Text>
        <Text>  2. Test your microphone</Text>
        <Text>  3. Start a voice conversation</Text>
      </Box>

      <Box marginTop={2}>
        <Text dimColor>{pulsingText('Press Enter to begin setup...', frame)}</Text>
      </Box>
    </Box>
  );
};

/**
 * Goodbye message
 */
export const GoodbyeMessage: React.FC = () => {
  const logoText = GRADIENTS.waav('WaaV');

  return (
    <Box flexDirection="column" paddingY={1}>
      <Text>
        Thanks for using {logoText}! See you next time.
      </Text>
    </Box>
  );
};

export default Splash;

/**
 * WaaV CLI Animated Logo Component
 *
 * Beautiful animated ASCII art logo with gradient effects
 */

import React, { useState, useEffect, useMemo } from 'react';
import { Text, Box } from 'ink';
import {
  WAAV_LOGO_BLOCK,
  WAAV_LOGO_SMALL,
  WAAV_LOGO_SLANT,
  SOUND_WAVE_FRAMES,
  GRADIENTS,
  revealText,
} from './animations.js';

/**
 * Logo size variants
 */
export type LogoSize = 'small' | 'medium' | 'large';

/**
 * Logo animation style
 */
export type LogoAnimation = 'none' | 'reveal' | 'gradient' | 'wave';

/**
 * Logo component props
 */
interface LogoProps {
  size?: LogoSize;
  animation?: LogoAnimation;
  showSoundWave?: boolean;
  showTagline?: boolean;
  showVersion?: boolean;
  version?: string;
  animationSpeed?: number;
  onAnimationComplete?: () => void;
}

/**
 * Get logo ASCII art based on size
 */
function getLogoArt(size: LogoSize): string {
  switch (size) {
    case 'small':
      return WAAV_LOGO_SMALL;
    case 'medium':
      return WAAV_LOGO_SLANT;
    case 'large':
    default:
      return WAAV_LOGO_BLOCK;
  }
}

/**
 * Animated Sound Wave Component
 */
const SoundWave: React.FC<{ frame: number }> = ({ frame }) => {
  const waveFrame = SOUND_WAVE_FRAMES[frame % SOUND_WAVE_FRAMES.length] ?? '';
  const coloredWave = GRADIENTS.waav(waveFrame);
  return <Text>{coloredWave}</Text>;
};

/**
 * Animated Logo Component
 */
export const Logo: React.FC<LogoProps> = ({
  size = 'large',
  animation = 'gradient',
  showSoundWave = true,
  showTagline = true,
  showVersion = true,
  version = '1.0.0',
  animationSpeed = 100,
  onAnimationComplete,
}) => {
  const [frame, setFrame] = useState(0);
  const [revealProgress, setRevealProgress] = useState(animation === 'none' ? 1 : 0);
  const [animationDone, setAnimationDone] = useState(animation === 'none');

  const logoArt = useMemo(() => getLogoArt(size), [size]);

  // Animation loop
  useEffect(() => {
    if (animation === 'none') {
      return;
    }

    const interval = setInterval(() => {
      setFrame(f => f + 1);

      // Handle reveal animation progress
      if (animation === 'reveal' && revealProgress < 1) {
        setRevealProgress(p => {
          const newProgress = Math.min(1, p + 0.05);
          if (newProgress >= 1 && !animationDone) {
            setAnimationDone(true);
            onAnimationComplete?.();
          }
          return newProgress;
        });
      }
    }, animationSpeed);

    // Mark gradient animation as done after some time
    if (animation === 'gradient' && !animationDone) {
      const timeout = setTimeout(() => {
        setAnimationDone(true);
        onAnimationComplete?.();
      }, 2000);
      return () => {
        clearInterval(interval);
        clearTimeout(timeout);
      };
    }

    return () => clearInterval(interval);
  }, [animation, animationSpeed, revealProgress, animationDone, onAnimationComplete]);

  // Render logo with animation
  const renderedLogo = useMemo(() => {
    let text = logoArt;

    // Apply reveal animation
    if (animation === 'reveal') {
      text = revealText(logoArt, revealProgress);
    }

    // Apply gradient coloring
    if (animation === 'gradient' || animation === 'wave') {
      return GRADIENTS.waav.multiline(text);
    }

    return text;
  }, [logoArt, animation, revealProgress, frame]);

  return (
    <Box flexDirection="column" alignItems="center">
      {/* Logo */}
      <Box>
        <Text>{renderedLogo}</Text>
      </Box>

      {/* Sound wave indicator */}
      {showSoundWave && (
        <Box marginTop={1}>
          <SoundWave frame={frame} />
        </Box>
      )}

      {/* Tagline */}
      {showTagline && (
        <Box marginTop={1}>
          <Text color="cyan">Voice AI Gateway CLI</Text>
        </Box>
      )}

      {/* Version and features */}
      {showVersion && (
        <Box marginTop={0}>
          <Text dimColor>
            v{version} | 70+ Providers | DAG Routing | LiveKit
          </Text>
        </Box>
      )}
    </Box>
  );
};

/**
 * Static logo (no animation) for quick rendering
 */
export const StaticLogo: React.FC<{ size?: LogoSize }> = ({ size = 'small' }) => {
  const logoArt = getLogoArt(size);
  const coloredLogo = GRADIENTS.waav.multiline(logoArt);

  return (
    <Box>
      <Text>{coloredLogo}</Text>
    </Box>
  );
};

/**
 * Mini logo for status bar
 */
export const MiniLogo: React.FC = () => {
  return (
    <Text bold color="cyan">
      WaaV
    </Text>
  );
};

export default Logo;

/**
 * WaaV CLI Splash Screen
 *
 * Welcome screen with animated logo and auto-advance to main menu.
 */

import React from 'react';
import { useNavigation } from '../hooks/useNavigation.js';
import { useVersion } from '../hooks/useAppState.js';
import { Splash } from '../components/branding/index.js';
import type { ScreenProps } from '../types/navigation.js';

/**
 * Splash Screen Component
 *
 * Displays the WaaV logo with optional animation and auto-advances
 * to the main menu after completion.
 */
export const SplashScreen: React.FC<ScreenProps> = ({ onComplete }) => {
  const { navigate } = useNavigation();
  const version = useVersion();

  // Handle splash completion - navigate to main menu
  const handleComplete = () => {
    if (onComplete) {
      onComplete();
    }
    navigate('main_menu');
  };

  return (
    <Splash
      version={version}
      onComplete={handleComplete}
      autoAdvance={false}
    />
  );
};

export default SplashScreen;

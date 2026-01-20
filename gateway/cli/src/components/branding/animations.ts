/**
 * WaaV CLI Animation Utilities
 *
 * Animation helpers for terminal UI effects
 */

import gradientString from 'gradient-string';
import chalk from 'chalk';

/**
 * WAAV ASCII Art Logo - Block Style
 */
export const WAAV_LOGO_BLOCK = `
██╗    ██╗ █████╗  █████╗ ██╗   ██╗
██║    ██║██╔══██╗██╔══██╗██║   ██║
██║ █╗ ██║███████║███████║██║   ██║
██║███╗██║██╔══██║██╔══██║╚██╗ ██╔╝
╚███╔███╔╝██║  ██║██║  ██║ ╚████╔╝
 ╚══╝╚══╝ ╚═╝  ╚═╝╚═╝  ╚═╝  ╚═══╝
`.trim();

/**
 * WAAV ASCII Art Logo - Slant Style
 */
export const WAAV_LOGO_SLANT = `
 _       __            _    __
| |     / /___ _____ _| |  / /
| | /| / / __ \`/ __ \`/ | / /
| |/ |/ / /_/ / /_/ /| V /
|__/|__/\\__,_/\\__,_/ |_|
`.trim();

/**
 * WAAV ASCII Art Logo - Small
 */
export const WAAV_LOGO_SMALL = `
╦ ╦┌─┐┌─┐╦  ╦
║║║├─┤├─┤╚╗╔╝
╚╩╝┴ ┴┴ ┴ ╚╝
`.trim();

/**
 * Sound wave animation frames
 */
export const SOUND_WAVE_FRAMES = [
  '▁▃▅▇▅▃▁',
  '▃▅▇▅▃▁▃',
  '▅▇▅▃▁▃▅',
  '▇▅▃▁▃▅▇',
  '▅▃▁▃▅▇▅',
  '▃▁▃▅▇▅▃',
  '▁▃▅▇▅▃▁',
  '▃▅▇▅▃▁▃',
];

/**
 * Voice wave animation frames (symmetrical)
 */
export const VOICE_WAVE_FRAMES = [
  '  ▁▂▃▄▃▂▁  ',
  ' ▁▂▃▄▅▄▃▂▁ ',
  '▁▂▃▄▅▆▅▄▃▂▁',
  ' ▂▃▄▅▆▅▄▃▂ ',
  '  ▃▄▅▆▅▄▃  ',
  ' ▂▃▄▅▆▅▄▃▂ ',
  '▁▂▃▄▅▆▅▄▃▂▁',
  ' ▁▂▃▄▅▄▃▂▁ ',
];

/**
 * Microphone animation frames
 */
export const MIC_FRAMES = [
  '🎙️ ',
  '🎤 ',
  '🎙️ ',
  '🎤 ',
];

/**
 * Spinner frames (dots style)
 */
export const SPINNER_DOTS = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/**
 * Spinner frames (circle style)
 */
export const SPINNER_CIRCLE = ['◜', '◠', '◝', '◞', '◡', '◟'];

/**
 * Spinner frames (box style)
 */
export const SPINNER_BOX = ['▖', '▘', '▝', '▗'];

/**
 * Pulse animation characters
 */
export const PULSE_CHARS = ['○', '◎', '●', '◎'];

/**
 * Recording indicator frames
 */
export const RECORDING_FRAMES = ['⏺', '⏺', ' ', ' '];

/**
 * Gradient definitions
 */
export const GRADIENTS = {
  waav: gradientString(['#00FFFF', '#FF00FF', '#FFFF00']),
  cyan_magenta: gradientString(['#00FFFF', '#FF00FF']),
  rainbow: gradientString.rainbow,
  pastel: gradientString.pastel,
  cristal: gradientString.cristal,
  teen: gradientString.teen,
  mind: gradientString.mind,
  morning: gradientString.morning,
  vice: gradientString.vice,
  passion: gradientString.passion,
  fruit: gradientString.fruit,
  instagram: gradientString.instagram,
  atlas: gradientString.atlas,
  retro: gradientString.retro,
  summer: gradientString.summer,
  sunset: gradientString(['#FF6B6B', '#FFE66D', '#4ECDC4']),
  ocean: gradientString(['#2193b0', '#6dd5ed']),
  forest: gradientString(['#11998e', '#38ef7d']),
  fire: gradientString(['#f12711', '#f5af19']),
};

/**
 * Apply animated gradient to text
 */
export function animatedGradient(text: string, frame: number): string {
  const gradients = [
    GRADIENTS.waav,
    GRADIENTS.cyan_magenta,
    GRADIENTS.rainbow,
    GRADIENTS.pastel,
  ];
  const idx = frame % gradients.length;
  return gradients[idx]?.multiline(text) ?? text;
}

/**
 * Create typewriter effect
 */
export function* typewriter(text: string, _delay = 50): Generator<string, void, unknown> {
  let current = '';
  for (const char of text) {
    current += char;
    yield current;
  }
}

/**
 * Create reveal animation (character by character)
 */
export function revealText(text: string, progress: number): string {
  const visibleChars = Math.floor(text.length * Math.min(1, Math.max(0, progress)));
  return text.slice(0, visibleChars);
}

/**
 * Create fade-in effect using brightness
 */
export function fadeIn(text: string, progress: number): string {
  const brightness = Math.min(1, Math.max(0, progress));

  if (brightness < 0.25) {
    return chalk.gray.dim(text);
  } else if (brightness < 0.5) {
    return chalk.gray(text);
  } else if (brightness < 0.75) {
    return chalk.white.dim(text);
  } else {
    return chalk.white(text);
  }
}

/**
 * Create pulsing text effect
 */
export function pulsingText(text: string, frame: number): string {
  const phases = [
    chalk.dim,
    chalk.dim,
    chalk.reset,
    chalk.bold,
    chalk.reset,
    chalk.dim,
  ];
  const phase = frame % phases.length;
  const style = phases[phase];
  return style ? style(text) : text;
}

/**
 * Create blinking cursor
 */
export function blinkingCursor(frame: number, char = '█'): string {
  return frame % 2 === 0 ? char : ' ';
}

/**
 * Create progress bar
 */
export function progressBar(
  progress: number,
  width = 30,
  options: {
    filled?: string;
    empty?: string;
    leftCap?: string;
    rightCap?: string;
    gradient?: boolean;
  } = {}
): string {
  const {
    filled = '█',
    empty = '░',
    leftCap = '',
    rightCap = '',
    gradient = false,
  } = options;

  const filledWidth = Math.round(progress * width);
  const emptyWidth = width - filledWidth;

  let bar = filled.repeat(filledWidth) + empty.repeat(emptyWidth);

  if (gradient && filledWidth > 0) {
    const filledPart = GRADIENTS.waav(filled.repeat(filledWidth));
    const emptyPart = chalk.gray(empty.repeat(emptyWidth));
    bar = filledPart + emptyPart;
  }

  return leftCap + bar + rightCap;
}

/**
 * Create audio level meter
 */
export function levelMeter(level: number, width = 20): string {
  const bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
  const filledBars = Math.round(level * width);

  let result = '';
  for (let i = 0; i < width; i++) {
    if (i < filledBars) {
      // Gradient from green to yellow to red
      const ratio = i / width;
      if (ratio < 0.5) {
        result += chalk.green(bars[7]);
      } else if (ratio < 0.75) {
        result += chalk.yellow(bars[7]);
      } else {
        result += chalk.red(bars[7]);
      }
    } else {
      result += chalk.gray(bars[0]);
    }
  }

  return result;
}

/**
 * Create waveform visualization
 */
export function waveform(levels: number[], width = 40): string {
  const bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

  // Resample levels to fit width
  const resampled: number[] = [];
  const step = levels.length / width;

  for (let i = 0; i < width; i++) {
    const idx = Math.floor(i * step);
    resampled.push(levels[idx] ?? 0);
  }

  return resampled
    .map(level => {
      const barIdx = Math.min(bars.length - 1, Math.floor(level * bars.length));
      return chalk.cyan(bars[barIdx]);
    })
    .join('');
}

/**
 * Center text within width
 */
export function centerText(text: string, width: number): string {
  const padding = Math.max(0, Math.floor((width - stripAnsi(text).length) / 2));
  return ' '.repeat(padding) + text;
}

/**
 * Strip ANSI codes from string
 */
export function stripAnsi(text: string): string {
  // eslint-disable-next-line no-control-regex
  return text.replace(/\u001b\[[0-9;]*m/g, '');
}

/**
 * Get text width (excluding ANSI codes)
 */
export function textWidth(text: string): number {
  return stripAnsi(text).length;
}

/**
 * Create box around text
 */
export function box(
  content: string,
  options: {
    title?: string;
    padding?: number;
    borderColor?: typeof chalk.blue;
    titleColor?: typeof chalk.bold;
  } = {}
): string {
  const {
    title,
    padding = 1,
    borderColor = chalk.cyan,
    titleColor = chalk.bold.cyan,
  } = options;

  const lines = content.split('\n');
  const contentWidth = Math.max(...lines.map(l => textWidth(l)));
  const boxWidth = contentWidth + padding * 2;

  const horizontal = '─'.repeat(boxWidth);
  const paddingStr = ' '.repeat(padding);

  const result: string[] = [];

  // Top border
  if (title) {
    const titleStr = ` ${title} `;
    const leftWidth = Math.floor((boxWidth - titleStr.length) / 2);
    const rightWidth = boxWidth - leftWidth - titleStr.length;
    result.push(
      borderColor('┌') +
      borderColor('─'.repeat(leftWidth)) +
      titleColor(titleStr) +
      borderColor('─'.repeat(rightWidth)) +
      borderColor('┐')
    );
  } else {
    result.push(borderColor('┌' + horizontal + '┐'));
  }

  // Content
  for (const line of lines) {
    const lineWidth = textWidth(line);
    const rightPad = ' '.repeat(contentWidth - lineWidth);
    result.push(
      borderColor('│') + paddingStr + line + rightPad + paddingStr + borderColor('│')
    );
  }

  // Bottom border
  result.push(borderColor('└' + horizontal + '┘'));

  return result.join('\n');
}

/**
 * Create animated loading message
 */
export function loadingMessage(message: string, frame: number): string {
  const spinner = SPINNER_DOTS[frame % SPINNER_DOTS.length];
  return `${chalk.cyan(spinner)} ${message}`;
}

/**
 * Playful loading messages
 */
export const LOADING_MESSAGES = [
  'Warming up the voice engines...',
  'Teaching the AI to listen...',
  'Connecting to the mothership...',
  'Brewing some audio magic...',
  'Getting the speakers ready...',
  'Tuning the frequencies...',
  'Calibrating the microphones...',
  'Summoning the voice spirits...',
  'Decoding the sound waves...',
  'Preparing for liftoff...',
];

/**
 * Get random loading message
 */
export function getRandomLoadingMessage(): string {
  const idx = Math.floor(Math.random() * LOADING_MESSAGES.length);
  return LOADING_MESSAGES[idx] ?? LOADING_MESSAGES[0] ?? 'Loading...';
}

/**
 * Success messages
 */
export const SUCCESS_MESSAGES = [
  'Boom! Done!',
  'Nailed it!',
  'All systems go!',
  'Ready to rock!',
  'Good to go!',
  'Looking sharp!',
  'Mission accomplished!',
];

/**
 * Get random success message
 */
export function getRandomSuccessMessage(): string {
  const idx = Math.floor(Math.random() * SUCCESS_MESSAGES.length);
  return SUCCESS_MESSAGES[idx] ?? SUCCESS_MESSAGES[0] ?? 'Done!';
}

/**
 * Error emojis
 */
export const ERROR_EMOJIS = ['😅', '🙈', '😬', '🤔', '💭', '🔧'];

/**
 * Get random error emoji
 */
export function getRandomErrorEmoji(): string {
  const idx = Math.floor(Math.random() * ERROR_EMOJIS.length);
  return ERROR_EMOJIS[idx] ?? '❌';
}

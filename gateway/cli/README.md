# WaaV CLI

Interactive terminal interface for managing and monitoring the WaaV Voice AI Gateway.

## Features

- **SPA Architecture**: Single-page application experience with seamless screen navigation
- **70+ Provider Browser**: Search and configure STT, TTS, and Realtime providers
- **Real-time Dashboard**: htop-style monitoring with sparklines, gauges, and metrics
- **Voice Mode**: Interactive voice conversations with push-to-talk and VAD modes
- **DAG Pipeline Editor**: Visual pipeline configuration with ASCII visualization
- **Keyboard-Driven**: Full keyboard navigation with global shortcuts

## Installation

```bash
# From the cli directory
npm install
npm run build

# Link globally (optional)
npm link
```

## Usage

```bash
# Interactive mode with splash screen
waav

# Direct screen access
waav dashboard           # Analytics dashboard
waav provider browse     # Provider browser
waav config show         # View configuration
waav server status       # Server status
waav voice start         # Start voice mode

# Non-interactive commands
waav doctor              # Diagnose issues
waav provider test <id>  # Test provider connectivity
waav completion bash     # Generate shell completions
```

## Architecture

The CLI uses a Single-Page Application (SPA) architecture built with:

- **React Ink**: Terminal UI framework
- **Context Providers**: Navigation, App State, Keyboard handling
- **Lazy Loading**: Code-split screens with React.lazy()
- **Commander.js**: CLI argument parsing

### Directory Structure

```
cli/src/
├── context/           # React contexts (Navigation, App, Keyboard)
├── layout/            # Shell components (Header, Footer, ScreenRenderer)
├── screens/           # Screen components (lazy-loaded)
│   ├── dashboard/
│   ├── provider/
│   ├── voice/
│   ├── dag/
│   ├── server/
│   ├── config/
│   └── setup/
├── hooks/             # Custom hooks (useNavigation, useAppState)
├── components/        # Reusable UI components
├── commands/          # CLI command handlers
└── types/             # TypeScript type definitions
```

## Keyboard Shortcuts

### Global Shortcuts
| Key | Action |
|-----|--------|
| `ESC` | Go back / Cancel |
| `?` | Show help |
| `Q` | Quit application |
| `Ctrl+H` | Go to main menu |
| `Ctrl+C` | Exit |

### Navigation
| Key | Action |
|-----|--------|
| `↑↓` | Move selection |
| `Enter` | Select/Confirm |
| `1-9` | Quick access shortcuts |
| `/` | Search (in lists) |

## Screens

| Screen | Description |
|--------|-------------|
| Main Menu | Primary navigation hub |
| Dashboard | Real-time metrics and monitoring |
| Provider Browser | Search and configure 70+ providers |
| Provider Config | Configure provider credentials |
| Voice Mode | Interactive voice conversations |
| DAG Editor | Pipeline configuration |
| Server Status | Gateway health and stats |
| Server Logs | Real-time log viewer |
| Config Editor | Configuration management |
| Setup Wizard | First-time setup |

## Development

```bash
# Development with watch mode
npm run dev

# Type checking
npm run typecheck

# Linting
npm run lint

# Build for production
npm run build
```

## Configuration

The CLI reads configuration from:
1. `~/.waav/config.yaml` (default)
2. `WAAV_CONFIG` environment variable
3. `--config` command line flag

Example configuration:
```yaml
gateway:
  url: http://localhost:3001
  auth:
    api_key: your-api-key

defaults:
  stt_provider: deepgram
  tts_provider: elevenlabs
  realtime_provider: openai

providers:
  deepgram:
    api_key: ${DEEPGRAM_API_KEY}
  elevenlabs:
    api_key: ${ELEVENLABS_API_KEY}
```

## License

MIT

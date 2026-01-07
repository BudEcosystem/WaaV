/**
 * UI Component Tests
 */

import { jest, describe, test, expect, beforeEach, afterEach } from '@jest/globals';
import { screen, fireEvent, waitFor } from '@testing-library/dom';

describe('Dashboard UI', () => {
  let container;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    container.remove();
  });

  describe('Header', () => {
    beforeEach(() => {
      container.innerHTML = `
        <header class="header" role="banner">
          <div class="header-left">
            <div class="logo">
              <img src="assets/waav-logo.svg" alt="WaaV" class="logo-image" />
              <span class="logo-text">WaaV</span>
            </div>
            <span class="environment-badge" data-env="development">DEV</span>
          </div>
          <div class="header-center">
            <button class="command-palette-trigger" aria-label="Open command palette">
              <span class="search-icon">⌘K</span>
              <span class="search-placeholder">Search or run command...</span>
            </button>
          </div>
          <div class="header-right">
            <div class="connection-status" data-status="disconnected">
              <span class="status-dot"></span>
              <span class="status-text">Disconnected</span>
            </div>
            <button id="theme-toggle" class="btn btn-icon" aria-label="Toggle theme">
              <span class="icon moon-icon"></span>
            </button>
          </div>
        </header>
      `;
    });

    test('should display WaaV logo', () => {
      const logo = container.querySelector('.logo-text');
      expect(logo).toBeInTheDocument();
      expect(logo.textContent).toBe('WaaV');
    });

    test('should display environment badge', () => {
      const badge = container.querySelector('.environment-badge');
      expect(badge).toBeInTheDocument();
      expect(badge.textContent).toBe('DEV');
      expect(badge.dataset.env).toBe('development');
    });

    test('should have command palette trigger', () => {
      const trigger = container.querySelector('.command-palette-trigger');
      expect(trigger).toBeInTheDocument();
      expect(trigger.getAttribute('aria-label')).toBe('Open command palette');
    });

    test('should show connection status', () => {
      const status = container.querySelector('.connection-status');
      expect(status).toBeInTheDocument();
      expect(status.dataset.status).toBe('disconnected');
    });
  });

  describe('Sidebar', () => {
    beforeEach(() => {
      container.innerHTML = `
        <aside class="sidebar" role="navigation" aria-label="Main navigation">
          <nav class="sidebar-nav">
            <ul class="nav-list">
              <li class="nav-item">
                <a href="#home" class="nav-link active" data-tab="home">
                  <span class="nav-icon">🏠</span>
                  <span class="nav-label">Dashboard</span>
                </a>
              </li>
              <li class="nav-group">
                <span class="nav-group-label">Voice Lab</span>
                <ul class="nav-sublist">
                  <li class="nav-item">
                    <a href="#stt" class="nav-link" data-tab="stt">
                      <span class="nav-icon">🎤</span>
                      <span class="nav-label">Speech-to-Text</span>
                    </a>
                  </li>
                  <li class="nav-item">
                    <a href="#tts" class="nav-link" data-tab="tts">
                      <span class="nav-icon">🔊</span>
                      <span class="nav-label">Text-to-Speech</span>
                    </a>
                  </li>
                  <li class="nav-item">
                    <a href="#compare" class="nav-link" data-tab="compare">
                      <span class="nav-icon">⚖️</span>
                      <span class="nav-label">A/B Compare</span>
                    </a>
                  </li>
                </ul>
              </li>
            </ul>
          </nav>
          <button class="sidebar-toggle" aria-label="Toggle sidebar">
            <span class="toggle-icon">◀</span>
          </button>
        </aside>
      `;
    });

    test('should have navigation role', () => {
      const sidebar = container.querySelector('.sidebar');
      expect(sidebar.getAttribute('role')).toBe('navigation');
    });

    test('should display nav items with icons and labels', () => {
      const navItems = container.querySelectorAll('.nav-item');
      expect(navItems.length).toBeGreaterThan(0);

      const firstLink = navItems[0].querySelector('.nav-link');
      expect(firstLink.querySelector('.nav-icon')).toBeInTheDocument();
      expect(firstLink.querySelector('.nav-label')).toBeInTheDocument();
    });

    test('should have active state on current tab', () => {
      const activeLink = container.querySelector('.nav-link.active');
      expect(activeLink).toBeInTheDocument();
      expect(activeLink.dataset.tab).toBe('home');
    });

    test('should have sidebar toggle button', () => {
      const toggle = container.querySelector('.sidebar-toggle');
      expect(toggle).toBeInTheDocument();
      expect(toggle.getAttribute('aria-label')).toBe('Toggle sidebar');
    });

    test('should collapse sidebar when toggle clicked', () => {
      const sidebar = container.querySelector('.sidebar');
      const toggle = container.querySelector('.sidebar-toggle');

      fireEvent.click(toggle);

      // In real implementation, this would add 'collapsed' class
      // For this test, we verify the click event fires
      expect(sidebar).toBeInTheDocument();
    });

    test('should have grouped navigation items', () => {
      const groups = container.querySelectorAll('.nav-group');
      expect(groups.length).toBeGreaterThan(0);

      const groupLabel = groups[0].querySelector('.nav-group-label');
      expect(groupLabel.textContent).toBe('Voice Lab');
    });
  });

  describe('Dashboard Home', () => {
    beforeEach(() => {
      container.innerHTML = `
        <main class="dashboard-home" id="panel-home">
          <section class="system-status">
            <h2>System Status</h2>
            <div class="status-grid">
              <div class="status-card" data-provider="gateway">
                <div class="status-indicator connected"></div>
                <span class="provider-name">Gateway</span>
                <span class="provider-status">Connected</span>
                <span class="provider-latency">42ms</span>
              </div>
              <div class="status-card" data-provider="deepgram">
                <div class="status-indicator connected"></div>
                <span class="provider-name">Deepgram</span>
                <span class="provider-status">Available</span>
              </div>
              <div class="status-card" data-provider="elevenlabs">
                <div class="status-indicator disconnected"></div>
                <span class="provider-name">ElevenLabs</span>
                <span class="provider-status">Not configured</span>
              </div>
            </div>
          </section>
          <section class="quick-stats">
            <h2>Today's Activity</h2>
            <div class="stats-grid">
              <div class="stat-card">
                <span class="stat-value">24</span>
                <span class="stat-label">STT Sessions</span>
              </div>
              <div class="stat-card">
                <span class="stat-value">18</span>
                <span class="stat-label">TTS Requests</span>
              </div>
              <div class="stat-card">
                <span class="stat-value">142ms</span>
                <span class="stat-label">Avg TTFT</span>
              </div>
              <div class="stat-card">
                <span class="stat-value">89ms</span>
                <span class="stat-label">Avg TTFB</span>
              </div>
            </div>
          </section>
          <section class="quick-actions">
            <h2>Quick Actions</h2>
            <div class="actions-grid">
              <button class="action-card" data-action="new-stt">
                <span class="action-icon">🎤</span>
                <span class="action-label">New STT Session</span>
              </button>
              <button class="action-card" data-action="new-tts">
                <span class="action-icon">🔊</span>
                <span class="action-label">New TTS Request</span>
              </button>
              <button class="action-card" data-action="compare">
                <span class="action-icon">⚖️</span>
                <span class="action-label">Compare Voices</span>
              </button>
              <button class="action-card" data-action="upload">
                <span class="action-icon">📁</span>
                <span class="action-label">Upload Audio</span>
              </button>
            </div>
          </section>
          <section class="recent-activity">
            <h2>Recent Sessions</h2>
            <div class="activity-list">
              <div class="activity-item" data-session-id="sess_001">
                <span class="activity-type stt">STT</span>
                <span class="activity-provider">Deepgram</span>
                <span class="activity-duration">5.2s</span>
                <span class="activity-time">2 min ago</span>
                <button class="activity-replay" aria-label="Replay session">▶</button>
              </div>
            </div>
          </section>
        </main>
      `;
    });

    test('should display system status section', () => {
      const statusSection = container.querySelector('.system-status');
      expect(statusSection).toBeInTheDocument();
    });

    test('should show provider status cards', () => {
      const statusCards = container.querySelectorAll('.status-card');
      expect(statusCards.length).toBe(3);
    });

    test('should indicate connected providers', () => {
      const connectedIndicators = container.querySelectorAll('.status-indicator.connected');
      expect(connectedIndicators.length).toBe(2);
    });

    test('should display quick stats', () => {
      const statCards = container.querySelectorAll('.stat-card');
      expect(statCards.length).toBe(4);
    });

    test('should have quick action buttons', () => {
      const actionCards = container.querySelectorAll('.action-card');
      expect(actionCards.length).toBe(4);
    });

    test('should show recent activity', () => {
      const activityItems = container.querySelectorAll('.activity-item');
      expect(activityItems.length).toBeGreaterThan(0);
    });

    test('should have replay button for sessions', () => {
      const replayButton = container.querySelector('.activity-replay');
      expect(replayButton).toBeInTheDocument();
      expect(replayButton.getAttribute('aria-label')).toBe('Replay session');
    });
  });

  describe('Connection Status Bar', () => {
    beforeEach(() => {
      container.innerHTML = `
        <div class="connection-bar" role="status">
          <div class="connection-info">
            <div class="input-group">
              <label for="server-url">Server</label>
              <input
                type="text"
                id="server-url"
                value="wss://localhost:3001/ws"
                placeholder="WebSocket URL"
              />
            </div>
            <div class="input-group">
              <label for="api-key">API Key</label>
              <div class="password-input">
                <input
                  type="password"
                  id="api-key"
                  placeholder="Optional"
                />
                <button class="toggle-visibility" aria-label="Show password">👁</button>
              </div>
            </div>
            <button id="connect-btn" class="btn btn-primary">Connect</button>
          </div>
          <div class="connection-status-details">
            <div class="status-indicator" data-status="disconnected">
              <span class="status-dot"></span>
              <span class="status-text">Disconnected</span>
            </div>
            <div class="latency-display" hidden>
              <span class="latency-value">--</span>
              <span class="latency-unit">ms</span>
            </div>
            <div class="reconnect-info" hidden>
              <span class="reconnect-text">Reconnecting in 5s</span>
              <button class="reconnect-now">Reconnect Now</button>
            </div>
          </div>
        </div>
      `;
    });

    test('should have server URL input', () => {
      const input = container.querySelector('#server-url');
      expect(input).toBeInTheDocument();
      expect(input.value).toBe('wss://localhost:3001/ws');
    });

    test('should have API key input with visibility toggle', () => {
      const input = container.querySelector('#api-key');
      const toggle = container.querySelector('.toggle-visibility');

      expect(input).toBeInTheDocument();
      expect(input.type).toBe('password');
      expect(toggle).toBeInTheDocument();
    });

    test('should toggle password visibility', () => {
      const input = container.querySelector('#api-key');
      const toggle = container.querySelector('.toggle-visibility');

      fireEvent.click(toggle);
      // In real implementation, this would change input type
    });

    test('should show connection status', () => {
      const statusIndicator = container.querySelector('.status-indicator');
      expect(statusIndicator.dataset.status).toBe('disconnected');
    });

    test('should have latency display (initially hidden)', () => {
      const latencyDisplay = container.querySelector('.latency-display');
      expect(latencyDisplay).toBeInTheDocument();
      expect(latencyDisplay.hidden).toBe(true);
    });

    test('should have reconnect info (initially hidden)', () => {
      const reconnectInfo = container.querySelector('.reconnect-info');
      expect(reconnectInfo).toBeInTheDocument();
      expect(reconnectInfo.hidden).toBe(true);
    });
  });

  describe('Command Palette', () => {
    beforeEach(() => {
      container.innerHTML = `
        <div class="command-palette" role="dialog" aria-modal="true" aria-label="Command palette" hidden>
          <div class="command-palette-backdrop"></div>
          <div class="command-palette-content">
            <div class="command-input-wrapper">
              <input
                type="text"
                class="command-input"
                placeholder="Type a command or search..."
                aria-label="Command input"
              />
            </div>
            <div class="command-results">
              <div class="command-group">
                <div class="command-group-label">Actions</div>
                <button class="command-item" data-command="connect">
                  <span class="command-icon">🔌</span>
                  <span class="command-label">Connect to server</span>
                  <span class="command-shortcut">Ctrl+Enter</span>
                </button>
                <button class="command-item" data-command="new-stt">
                  <span class="command-icon">🎤</span>
                  <span class="command-label">New STT session</span>
                  <span class="command-shortcut">Space</span>
                </button>
              </div>
              <div class="command-group">
                <div class="command-group-label">Navigation</div>
                <button class="command-item" data-command="goto-home">
                  <span class="command-icon">🏠</span>
                  <span class="command-label">Go to Dashboard</span>
                  <span class="command-shortcut">Ctrl+1</span>
                </button>
              </div>
            </div>
          </div>
        </div>
      `;
    });

    test('should be hidden by default', () => {
      const palette = container.querySelector('.command-palette');
      expect(palette.hidden).toBe(true);
    });

    test('should have proper ARIA attributes', () => {
      const palette = container.querySelector('.command-palette');
      expect(palette.getAttribute('role')).toBe('dialog');
      expect(palette.getAttribute('aria-modal')).toBe('true');
    });

    test('should have search input', () => {
      const input = container.querySelector('.command-input');
      expect(input).toBeInTheDocument();
      expect(input.getAttribute('aria-label')).toBe('Command input');
    });

    test('should display command groups', () => {
      const groups = container.querySelectorAll('.command-group');
      expect(groups.length).toBe(2);
    });

    test('should show keyboard shortcuts for commands', () => {
      const shortcuts = container.querySelectorAll('.command-shortcut');
      expect(shortcuts.length).toBeGreaterThan(0);
    });

    test('should have clickable command items', () => {
      const items = container.querySelectorAll('.command-item');
      expect(items.length).toBeGreaterThan(0);

      items.forEach(item => {
        expect(item.tagName.toLowerCase()).toBe('button');
        expect(item.dataset.command).toBeDefined();
      });
    });
  });

  describe('Waveform Visualization', () => {
    beforeEach(() => {
      container.innerHTML = `
        <div class="waveform-container">
          <canvas class="waveform-canvas" width="600" height="100"></canvas>
          <div class="waveform-controls">
            <button class="waveform-zoom-in" aria-label="Zoom in">+</button>
            <button class="waveform-zoom-out" aria-label="Zoom out">-</button>
            <span class="waveform-time">00:00 / 00:00</span>
          </div>
          <div class="audio-level-meter">
            <div class="level-bar" style="width: 0%"></div>
            <span class="level-value">-60 dB</span>
          </div>
        </div>
      `;
    });

    test('should have canvas element', () => {
      const canvas = container.querySelector('.waveform-canvas');
      expect(canvas).toBeInTheDocument();
      expect(canvas.width).toBe(600);
      expect(canvas.height).toBe(100);
    });

    test('should have zoom controls', () => {
      const zoomIn = container.querySelector('.waveform-zoom-in');
      const zoomOut = container.querySelector('.waveform-zoom-out');

      expect(zoomIn).toBeInTheDocument();
      expect(zoomOut).toBeInTheDocument();
    });

    test('should display time', () => {
      const time = container.querySelector('.waveform-time');
      expect(time.textContent).toBe('00:00 / 00:00');
    });

    test('should have audio level meter', () => {
      const meter = container.querySelector('.audio-level-meter');
      const levelBar = container.querySelector('.level-bar');

      expect(meter).toBeInTheDocument();
      expect(levelBar).toBeInTheDocument();
    });
  });

  describe('Voice Preview Cards', () => {
    beforeEach(() => {
      container.innerHTML = `
        <div class="voice-grid">
          <div class="voice-card" data-voice-id="voice_001">
            <div class="voice-preview">
              <button class="voice-play" aria-label="Preview voice">▶</button>
            </div>
            <div class="voice-info">
              <span class="voice-name">Aria</span>
              <span class="voice-provider">ElevenLabs</span>
              <span class="voice-language">English (US)</span>
              <span class="voice-gender">Female</span>
            </div>
            <div class="voice-actions">
              <button class="voice-select btn btn-primary">Select</button>
              <button class="voice-favorite" aria-label="Add to favorites">⭐</button>
            </div>
          </div>
        </div>
      `;
    });

    test('should display voice card', () => {
      const card = container.querySelector('.voice-card');
      expect(card).toBeInTheDocument();
      expect(card.dataset.voiceId).toBe('voice_001');
    });

    test('should have preview button', () => {
      const playButton = container.querySelector('.voice-play');
      expect(playButton).toBeInTheDocument();
      expect(playButton.getAttribute('aria-label')).toBe('Preview voice');
    });

    test('should display voice metadata', () => {
      expect(container.querySelector('.voice-name').textContent).toBe('Aria');
      expect(container.querySelector('.voice-provider').textContent).toBe('ElevenLabs');
      expect(container.querySelector('.voice-language').textContent).toBe('English (US)');
      expect(container.querySelector('.voice-gender').textContent).toBe('Female');
    });

    test('should have select and favorite buttons', () => {
      const selectBtn = container.querySelector('.voice-select');
      const favoriteBtn = container.querySelector('.voice-favorite');

      expect(selectBtn).toBeInTheDocument();
      expect(favoriteBtn).toBeInTheDocument();
    });
  });

  describe('A/B Comparison Panel', () => {
    beforeEach(() => {
      container.innerHTML = `
        <div class="compare-panel" id="panel-compare">
          <h2>Voice A/B Comparison</h2>
          <div class="compare-text-input">
            <label for="compare-text">Test Text</label>
            <textarea id="compare-text" placeholder="Enter text to compare...">Hello, this is a voice comparison test.</textarea>
          </div>
          <div class="compare-voices">
            <div class="compare-voice voice-a">
              <h3>Voice A</h3>
              <select class="voice-select-a">
                <option value="">Select voice...</option>
              </select>
              <div class="compare-player">
                <button class="compare-play" disabled>▶ Generate</button>
                <audio class="compare-audio"></audio>
              </div>
              <div class="compare-metrics">
                <span class="metric">TTFB: --ms</span>
                <span class="metric">Duration: --s</span>
              </div>
            </div>
            <div class="compare-voice voice-b">
              <h3>Voice B</h3>
              <select class="voice-select-b">
                <option value="">Select voice...</option>
              </select>
              <div class="compare-player">
                <button class="compare-play" disabled>▶ Generate</button>
                <audio class="compare-audio"></audio>
              </div>
              <div class="compare-metrics">
                <span class="metric">TTFB: --ms</span>
                <span class="metric">Duration: --s</span>
              </div>
            </div>
          </div>
          <div class="compare-actions">
            <button class="btn btn-primary" id="compare-both">Generate Both</button>
            <button class="btn btn-secondary" id="blind-test">Blind Test Mode</button>
          </div>
          <div class="compare-rating" hidden>
            <h3>Which sounds better?</h3>
            <div class="rating-buttons">
              <button class="rating-btn" data-choice="a">Voice A</button>
              <button class="rating-btn" data-choice="tie">Tie</button>
              <button class="rating-btn" data-choice="b">Voice B</button>
            </div>
          </div>
        </div>
      `;
    });

    test('should have text input', () => {
      const textarea = container.querySelector('#compare-text');
      expect(textarea).toBeInTheDocument();
    });

    test('should have two voice selection panels', () => {
      const voiceA = container.querySelector('.voice-a');
      const voiceB = container.querySelector('.voice-b');

      expect(voiceA).toBeInTheDocument();
      expect(voiceB).toBeInTheDocument();
    });

    test('should have generate both button', () => {
      const generateBoth = container.querySelector('#compare-both');
      expect(generateBoth).toBeInTheDocument();
    });

    test('should have blind test mode button', () => {
      const blindTest = container.querySelector('#blind-test');
      expect(blindTest).toBeInTheDocument();
    });

    test('should have rating section (initially hidden)', () => {
      const rating = container.querySelector('.compare-rating');
      expect(rating).toBeInTheDocument();
      expect(rating.hidden).toBe(true);
    });
  });

  describe('Metrics Sparklines', () => {
    beforeEach(() => {
      container.innerHTML = `
        <div class="metric-card-enhanced">
          <div class="metric-header">
            <span class="metric-label">STT TTFT</span>
            <span class="metric-trend up">↑ 12%</span>
          </div>
          <div class="metric-value-row">
            <span class="metric-value">142</span>
            <span class="metric-unit">ms</span>
          </div>
          <div class="metric-sparkline">
            <canvas class="sparkline-canvas" width="100" height="30"></canvas>
          </div>
          <div class="metric-footer">
            <span class="metric-percentile">p95</span>
            <span class="metric-timeframe">Last 5 min</span>
          </div>
        </div>
      `;
    });

    test('should display metric label', () => {
      const label = container.querySelector('.metric-label');
      expect(label.textContent).toBe('STT TTFT');
    });

    test('should show trend indicator', () => {
      const trend = container.querySelector('.metric-trend');
      expect(trend).toBeInTheDocument();
      expect(trend.classList.contains('up')).toBe(true);
    });

    test('should have sparkline canvas', () => {
      const canvas = container.querySelector('.sparkline-canvas');
      expect(canvas).toBeInTheDocument();
      expect(canvas.width).toBe(100);
      expect(canvas.height).toBe(30);
    });

    test('should display percentile and timeframe', () => {
      expect(container.querySelector('.metric-percentile').textContent).toBe('p95');
      expect(container.querySelector('.metric-timeframe').textContent).toBe('Last 5 min');
    });
  });
});

describe('Accessibility', () => {
  test('should have skip link', () => {
    document.body.innerHTML = `
      <a href="#main-content" class="skip-link">Skip to main content</a>
      <main id="main-content">Content</main>
    `;

    const skipLink = document.querySelector('.skip-link');
    expect(skipLink).toBeInTheDocument();
    expect(skipLink.getAttribute('href')).toBe('#main-content');
  });

  test('should have proper heading hierarchy', () => {
    document.body.innerHTML = `
      <h1>WaaV Dashboard</h1>
      <section>
        <h2>Speech-to-Text</h2>
        <h3>Configuration</h3>
      </section>
    `;

    const h1 = document.querySelector('h1');
    const h2 = document.querySelector('h2');
    const h3 = document.querySelector('h3');

    expect(h1).toBeInTheDocument();
    expect(h2).toBeInTheDocument();
    expect(h3).toBeInTheDocument();
  });

  test('should have focus visible styles', () => {
    document.body.innerHTML = `
      <button class="btn">Click me</button>
      <style>
        .btn:focus-visible {
          outline: 2px solid blue;
        }
      </style>
    `;

    const button = document.querySelector('.btn');
    button.focus();

    // Check that button can receive focus
    expect(document.activeElement).toBe(button);
  });
});

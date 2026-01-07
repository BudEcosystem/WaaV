/**
 * WaaV Dashboard Keyboard Shortcuts Manager
 * Handles keyboard shortcuts and command palette
 */

export class KeyboardManager {
  constructor(state, handlers) {
    this.state = state;
    this.handlers = handlers;
    this.shortcuts = new Map();
    this.enabled = true;

    // Register default shortcuts
    this._registerDefaultShortcuts();

    // Bind event handler
    this._handleKeydown = this._handleKeydown.bind(this);
    document.addEventListener('keydown', this._handleKeydown);
  }

  /**
   * Register default keyboard shortcuts
   */
  _registerDefaultShortcuts() {
    // Command palette
    this.registerShortcut({
      key: 'k',
      ctrl: true,
      description: 'Open command palette',
      handler: () => this.handlers.openCommandPalette?.(),
    });

    // Connection toggle
    this.registerShortcut({
      key: 'Enter',
      ctrl: true,
      description: 'Connect/Disconnect',
      handler: () => {
        if (this.state.connected) {
          this.handlers.disconnect?.();
        } else {
          this.handlers.connect?.();
        }
      },
    });

    // Recording toggle (Space on STT tab)
    this.registerShortcut({
      key: ' ',
      description: 'Start/Stop recording',
      condition: () => this.state.currentTab === 'stt' && this.state.connected,
      requiresNoFocus: true,
      handler: () => {
        if (this.state.recording) {
          this.handlers.stopRecording?.();
        } else {
          this.handlers.startRecording?.();
        }
      },
    });

    // TTS speak
    this.registerShortcut({
      key: 's',
      ctrl: true,
      description: 'Speak text (TTS)',
      condition: () => this.state.currentTab === 'tts' && this.state.connected,
      handler: () => this.handlers.speak?.(),
      preventDefault: true,
    });

    // Tab switching (Ctrl+1-9)
    const tabs = ['home', 'stt', 'tts', 'livekit', 'sip', 'api', 'ws', 'audio', 'metrics'];
    tabs.forEach((tab, index) => {
      this.registerShortcut({
        key: String(index + 1),
        ctrl: true,
        description: `Go to ${tab} tab`,
        handler: () => this.state.setCurrentTab(tab),
      });
    });

    // Theme toggle
    this.registerShortcut({
      key: 'd',
      ctrl: true,
      description: 'Toggle dark mode',
      handler: () => this.handlers.toggleTheme?.(),
      preventDefault: true,
    });

    // Sidebar toggle
    this.registerShortcut({
      key: 'b',
      ctrl: true,
      description: 'Toggle sidebar',
      handler: () => this.handlers.toggleSidebar?.(),
      preventDefault: true,
    });

    // Help
    this.registerShortcut({
      key: '?',
      description: 'Show keyboard shortcuts',
      requiresNoFocus: true,
      handler: () => this.handlers.showHelp?.(),
    });

    // Escape to close modals
    this.registerShortcut({
      key: 'Escape',
      description: 'Close modal/dialog',
      handler: () => this._closeActiveModal(),
    });
  }

  /**
   * Register a new shortcut
   */
  registerShortcut(options) {
    const {
      key,
      ctrl = false,
      alt = false,
      shift = false,
      meta = false,
      description = '',
      handler,
      condition = () => true,
      requiresNoFocus = false,
      preventDefault = true,
    } = options;

    const shortcutKey = this._buildShortcutKey(key, ctrl, alt, shift, meta);

    this.shortcuts.set(shortcutKey, {
      key,
      ctrl,
      alt,
      shift,
      meta,
      description,
      handler,
      condition,
      requiresNoFocus,
      preventDefault,
    });
  }

  /**
   * Unregister a shortcut
   */
  unregisterShortcut(key, ctrl = false, alt = false, shift = false, meta = false) {
    const shortcutKey = this._buildShortcutKey(key, ctrl, alt, shift, meta);
    this.shortcuts.delete(shortcutKey);
  }

  /**
   * Build unique key for shortcut map
   */
  _buildShortcutKey(key, ctrl, alt, shift, meta) {
    const parts = [];
    if (ctrl) parts.push('ctrl');
    if (alt) parts.push('alt');
    if (shift) parts.push('shift');
    if (meta) parts.push('meta');
    parts.push(key.toLowerCase());
    return parts.join('+');
  }

  /**
   * Handle keydown events
   */
  _handleKeydown(event) {
    if (!this.enabled) return;

    // Build shortcut key from event
    const shortcutKey = this._buildShortcutKey(
      event.key,
      event.ctrlKey || event.metaKey, // Treat Cmd as Ctrl on Mac
      event.altKey,
      event.shiftKey,
      false // We already handle meta above
    );

    const shortcut = this.shortcuts.get(shortcutKey);
    if (!shortcut) return;

    // Check if shortcut requires no focus
    if (shortcut.requiresNoFocus && this._isInputFocused()) {
      return;
    }

    // Check condition
    if (!shortcut.condition()) {
      return;
    }

    // Prevent default browser behavior
    if (shortcut.preventDefault) {
      event.preventDefault();
    }

    // Execute handler
    try {
      shortcut.handler();
    } catch (error) {
      console.error('Error executing shortcut handler:', error);
    }
  }

  /**
   * Check if an input element is focused
   */
  _isInputFocused() {
    const activeElement = document.activeElement;
    if (!activeElement) return false;

    const tagName = activeElement.tagName.toLowerCase();
    const isEditable = activeElement.isContentEditable;
    const isInput = ['input', 'textarea', 'select'].includes(tagName);

    return isInput || isEditable;
  }

  /**
   * Close active modal
   */
  _closeActiveModal() {
    const modal = document.querySelector('.modal.active, .command-palette:not([hidden])');
    if (modal) {
      modal.classList.remove('active');
      if (modal.hasAttribute('hidden') === false) {
        modal.hidden = true;
      }
    }
  }

  /**
   * Get all registered shortcuts
   */
  getShortcuts() {
    const shortcuts = [];
    this.shortcuts.forEach((shortcut, key) => {
      shortcuts.push({
        key: shortcut.key,
        ctrl: shortcut.ctrl,
        alt: shortcut.alt,
        shift: shortcut.shift,
        meta: shortcut.meta,
        description: shortcut.description,
        displayKey: this._formatShortcutDisplay(shortcut),
      });
    });
    return shortcuts;
  }

  /**
   * Format shortcut for display
   */
  _formatShortcutDisplay(shortcut) {
    const parts = [];
    const isMac = navigator.platform.toUpperCase().indexOf('MAC') >= 0;

    if (shortcut.ctrl) parts.push(isMac ? '⌘' : 'Ctrl');
    if (shortcut.alt) parts.push(isMac ? '⌥' : 'Alt');
    if (shortcut.shift) parts.push('⇧');

    // Format special keys
    let keyDisplay = shortcut.key;
    switch (shortcut.key) {
      case ' ': keyDisplay = 'Space'; break;
      case 'Enter': keyDisplay = '↵'; break;
      case 'Escape': keyDisplay = 'Esc'; break;
      case 'ArrowUp': keyDisplay = '↑'; break;
      case 'ArrowDown': keyDisplay = '↓'; break;
      case 'ArrowLeft': keyDisplay = '←'; break;
      case 'ArrowRight': keyDisplay = '→'; break;
    }

    parts.push(keyDisplay.toUpperCase());
    return parts.join(isMac ? '' : '+');
  }

  /**
   * Enable keyboard shortcuts
   */
  enable() {
    this.enabled = true;
  }

  /**
   * Disable keyboard shortcuts
   */
  disable() {
    this.enabled = false;
  }

  /**
   * Clean up
   */
  destroy() {
    document.removeEventListener('keydown', this._handleKeydown);
    this.shortcuts.clear();
  }
}

/**
 * Command Palette Manager
 */
export class CommandPalette {
  constructor(keyboardManager, state, handlers) {
    this.keyboardManager = keyboardManager;
    this.state = state;
    this.handlers = handlers;
    this.element = null;
    this.input = null;
    this.results = null;
    this.selectedIndex = 0;
    this.filteredCommands = [];

    this._createDOM();
    this._bindEvents();
  }

  /**
   * Create command palette DOM
   */
  _createDOM() {
    this.element = document.createElement('div');
    this.element.className = 'command-palette';
    this.element.setAttribute('role', 'dialog');
    this.element.setAttribute('aria-modal', 'true');
    this.element.setAttribute('aria-label', 'Command palette');
    this.element.hidden = true;

    this.element.innerHTML = `
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
        <div class="command-results"></div>
      </div>
    `;

    document.body.appendChild(this.element);

    this.input = this.element.querySelector('.command-input');
    this.results = this.element.querySelector('.command-results');
  }

  /**
   * Bind events
   */
  _bindEvents() {
    // Close on backdrop click
    this.element.querySelector('.command-palette-backdrop').addEventListener('click', () => {
      this.close();
    });

    // Filter on input
    this.input.addEventListener('input', () => {
      this._filterCommands(this.input.value);
    });

    // Keyboard navigation
    this.input.addEventListener('keydown', (e) => {
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          this._selectNext();
          break;
        case 'ArrowUp':
          e.preventDefault();
          this._selectPrevious();
          break;
        case 'Enter':
          e.preventDefault();
          this._executeSelected();
          break;
        case 'Escape':
          this.close();
          break;
      }
    });
  }

  /**
   * Get available commands
   */
  _getCommands() {
    const commands = [];

    // Add shortcuts as commands
    this.keyboardManager.getShortcuts().forEach(shortcut => {
      commands.push({
        id: `shortcut-${shortcut.key}`,
        label: shortcut.description,
        shortcut: shortcut.displayKey,
        group: 'Shortcuts',
        handler: () => {
          // Find and execute the shortcut
          const key = this.keyboardManager._buildShortcutKey(
            shortcut.key,
            shortcut.ctrl,
            shortcut.alt,
            shortcut.shift,
            shortcut.meta
          );
          const sc = this.keyboardManager.shortcuts.get(key);
          if (sc) sc.handler();
        },
      });
    });

    // Add navigation commands
    const tabs = [
      { id: 'home', label: 'Dashboard', icon: '🏠' },
      { id: 'stt', label: 'Speech-to-Text', icon: '🎤' },
      { id: 'tts', label: 'Text-to-Speech', icon: '🔊' },
      { id: 'compare', label: 'A/B Compare', icon: '⚖️' },
      { id: 'livekit', label: 'LiveKit', icon: '📡' },
      { id: 'sip', label: 'SIP', icon: '📞' },
      { id: 'api', label: 'API Explorer', icon: '🔌' },
      { id: 'ws', label: 'WebSocket Debug', icon: '🔧' },
      { id: 'audio', label: 'Audio Tools', icon: '🎵' },
      { id: 'metrics', label: 'Metrics', icon: '📊' },
    ];

    tabs.forEach(tab => {
      commands.push({
        id: `nav-${tab.id}`,
        label: `Go to ${tab.label}`,
        icon: tab.icon,
        group: 'Navigation',
        handler: () => this.state.setCurrentTab(tab.id),
      });
    });

    // Add action commands
    commands.push({
      id: 'action-connect',
      label: this.state.connected ? 'Disconnect' : 'Connect to server',
      icon: '🔌',
      group: 'Actions',
      handler: () => {
        if (this.state.connected) {
          this.handlers.disconnect?.();
        } else {
          this.handlers.connect?.();
        }
      },
    });

    commands.push({
      id: 'action-theme',
      label: 'Toggle dark mode',
      icon: '🌙',
      group: 'Actions',
      handler: () => this.handlers.toggleTheme?.(),
    });

    return commands;
  }

  /**
   * Filter commands by query
   */
  _filterCommands(query) {
    const commands = this._getCommands();
    const lowerQuery = query.toLowerCase();

    if (!query) {
      this.filteredCommands = commands;
    } else {
      this.filteredCommands = commands.filter(cmd =>
        cmd.label.toLowerCase().includes(lowerQuery)
      );
    }

    this.selectedIndex = 0;
    this._renderResults();
  }

  /**
   * Render filtered results
   */
  _renderResults() {
    // Group commands
    const groups = {};
    this.filteredCommands.forEach(cmd => {
      const group = cmd.group || 'Other';
      if (!groups[group]) groups[group] = [];
      groups[group].push(cmd);
    });

    // Render
    let html = '';
    let index = 0;

    Object.entries(groups).forEach(([groupName, commands]) => {
      html += `<div class="command-group">`;
      html += `<div class="command-group-label">${groupName}</div>`;

      commands.forEach(cmd => {
        const isSelected = index === this.selectedIndex;
        html += `
          <button
            class="command-item ${isSelected ? 'selected' : ''}"
            data-command="${cmd.id}"
            data-index="${index}"
          >
            ${cmd.icon ? `<span class="command-icon">${cmd.icon}</span>` : ''}
            <span class="command-label">${cmd.label}</span>
            ${cmd.shortcut ? `<span class="command-shortcut">${cmd.shortcut}</span>` : ''}
          </button>
        `;
        index++;
      });

      html += `</div>`;
    });

    this.results.innerHTML = html;

    // Add click handlers
    this.results.querySelectorAll('.command-item').forEach(item => {
      item.addEventListener('click', () => {
        const idx = parseInt(item.dataset.index, 10);
        this.selectedIndex = idx;
        this._executeSelected();
      });
    });
  }

  /**
   * Select next item
   */
  _selectNext() {
    if (this.selectedIndex < this.filteredCommands.length - 1) {
      this.selectedIndex++;
      this._renderResults();
    }
  }

  /**
   * Select previous item
   */
  _selectPrevious() {
    if (this.selectedIndex > 0) {
      this.selectedIndex--;
      this._renderResults();
    }
  }

  /**
   * Execute selected command
   */
  _executeSelected() {
    const command = this.filteredCommands[this.selectedIndex];
    if (command) {
      this.close();
      command.handler();
    }
  }

  /**
   * Open command palette
   */
  open() {
    this.element.hidden = false;
    this.input.value = '';
    this._filterCommands('');
    this.input.focus();
  }

  /**
   * Close command palette
   */
  close() {
    this.element.hidden = true;
    this.input.value = '';
  }

  /**
   * Toggle command palette
   */
  toggle() {
    if (this.element.hidden) {
      this.open();
    } else {
      this.close();
    }
  }

  /**
   * Check if palette is open
   */
  isOpen() {
    return !this.element.hidden;
  }
}

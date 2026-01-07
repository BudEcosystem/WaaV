/**
 * SSML Editor Module for WaaV Dashboard
 * Provides SSML editing, validation, and tag insertion capabilities
 */

/**
 * SSML Validator - validates SSML markup syntax
 */
export class SSMLValidator {
  constructor() {
    this.supportedTags = [
      'speak', 'break', 'emphasis', 'prosody', 'say-as',
      'sub', 'phoneme', 'p', 's', 'w', 'voice', 'lang',
      'mark', 'audio', 'amazon:effect', 'amazon:breath',
      'amazon:auto-breaths', 'amazon:domain'
    ];

    this.emphasisLevels = ['reduced', 'none', 'moderate', 'strong'];
    this.breakStrengths = ['none', 'x-weak', 'weak', 'medium', 'strong', 'x-strong'];
    this.prosodyRates = ['x-slow', 'slow', 'medium', 'fast', 'x-fast'];
    this.prosodyPitches = ['x-low', 'low', 'medium', 'high', 'x-high'];
    this.prosodyVolumes = ['silent', 'x-soft', 'soft', 'medium', 'loud', 'x-loud'];
    this.sayAsTypes = [
      'cardinal', 'ordinal', 'characters', 'spell-out', 'fraction',
      'unit', 'date', 'time', 'telephone', 'address', 'interjection',
      'expletive', 'number', 'digits'
    ];
  }

  /**
   * Validate SSML markup
   * @param {string} ssml - SSML string to validate
   * @returns {{ valid: boolean, errors: string[] }}
   */
  validate(ssml) {
    const errors = [];

    // Check for speak wrapper
    if (!ssml.trim().startsWith('<speak>') || !ssml.trim().endsWith('</speak>')) {
      errors.push('SSML must be wrapped in <speak> tags');
      return { valid: false, errors };
    }

    // Parse and validate tags
    const tagRegex = /<\/?([a-zA-Z:_-]+)([^>]*)>/g;
    const tagStack = [];
    let match;

    while ((match = tagRegex.exec(ssml)) !== null) {
      const fullMatch = match[0];
      const tagName = match[1].toLowerCase();
      const attributes = match[2];
      const isClosing = fullMatch.startsWith('</');
      const isSelfClosing = fullMatch.endsWith('/>');

      // Check if tag is supported
      if (!this.supportedTags.includes(tagName)) {
        errors.push(`Invalid SSML tag: <${tagName}>`);
        continue;
      }

      if (isSelfClosing) {
        // Validate self-closing tag attributes
        this.validateTagAttributes(tagName, attributes, errors);
      } else if (isClosing) {
        // Check for matching opening tag
        if (tagStack.length === 0 || tagStack[tagStack.length - 1] !== tagName) {
          errors.push(`Unclosed tag: expected </${tagStack[tagStack.length - 1] || 'unknown'}>, found </${tagName}>`);
        } else {
          tagStack.pop();
        }
      } else {
        // Opening tag
        tagStack.push(tagName);
        this.validateTagAttributes(tagName, attributes, errors);
      }
    }

    // Check for unclosed tags
    if (tagStack.length > 0) {
      errors.push(`Unclosed tag: <${tagStack[tagStack.length - 1]}>`);
    }

    return { valid: errors.length === 0, errors };
  }

  /**
   * Validate tag attributes
   */
  validateTagAttributes(tagName, attributes, errors) {
    const attrMap = this.parseAttributes(attributes);

    switch (tagName) {
      case 'break':
        if (attrMap.time && !this.isValidTime(attrMap.time)) {
          errors.push(`Invalid break time format: ${attrMap.time}`);
        }
        if (attrMap.strength && !this.breakStrengths.includes(attrMap.strength)) {
          errors.push(`Invalid break strength: ${attrMap.strength}`);
        }
        break;

      case 'emphasis':
        if (attrMap.level && !this.emphasisLevels.includes(attrMap.level)) {
          errors.push(`Invalid emphasis level: ${attrMap.level}`);
        }
        break;

      case 'prosody':
        if (attrMap.rate && !this.isValidProsodyRate(attrMap.rate)) {
          errors.push(`Invalid prosody rate: ${attrMap.rate}`);
        }
        if (attrMap.pitch && !this.isValidProsodyPitch(attrMap.pitch)) {
          errors.push(`Invalid prosody pitch: ${attrMap.pitch}`);
        }
        if (attrMap.volume && !this.isValidProsodyVolume(attrMap.volume)) {
          errors.push(`Invalid prosody volume: ${attrMap.volume}`);
        }
        break;

      case 'say-as':
        if (attrMap['interpret-as'] && !this.sayAsTypes.includes(attrMap['interpret-as'])) {
          errors.push(`Invalid say-as interpret-as: ${attrMap['interpret-as']}`);
        }
        break;
    }
  }

  /**
   * Parse attributes from string
   */
  parseAttributes(attrString) {
    const attrs = {};
    const regex = /([a-zA-Z-]+)=["']([^"']*)["']/g;
    let match;
    while ((match = regex.exec(attrString)) !== null) {
      attrs[match[1]] = match[2];
    }
    return attrs;
  }

  /**
   * Check if time format is valid (e.g., "500ms", "1s", "1.5s")
   */
  isValidTime(time) {
    return /^\d+(\.\d+)?(ms|s)$/.test(time);
  }

  /**
   * Check if prosody rate is valid
   */
  isValidProsodyRate(rate) {
    if (this.prosodyRates.includes(rate)) return true;
    return /^[+-]?\d+(\.\d+)?%$/.test(rate);
  }

  /**
   * Check if prosody pitch is valid
   */
  isValidProsodyPitch(pitch) {
    if (this.prosodyPitches.includes(pitch)) return true;
    return /^[+-]?\d+(\.\d+)?(%|Hz|st)$/.test(pitch);
  }

  /**
   * Check if prosody volume is valid
   */
  isValidProsodyVolume(volume) {
    if (this.prosodyVolumes.includes(volume)) return true;
    return /^[+-]?\d+(\.\d+)?dB$/.test(volume);
  }

  /**
   * Get list of supported SSML tags
   */
  getSupportedTags() {
    return [...this.supportedTags];
  }
}

/**
 * SSML Parser - handles SSML parsing and manipulation
 */
export class SSMLParser {
  constructor() {
    this.validator = new SSMLValidator();
  }

  /**
   * Wrap text in speak tags if not already wrapped
   */
  wrapInSpeak(text) {
    const trimmed = text.trim();
    if (trimmed.startsWith('<speak>') && trimmed.endsWith('</speak>')) {
      return trimmed;
    }
    return `<speak>${trimmed}</speak>`;
  }

  /**
   * Remove speak tags
   */
  stripSpeak(ssml) {
    return ssml
      .replace(/^<speak>\s*/i, '')
      .replace(/\s*<\/speak>$/i, '')
      .trim();
  }

  /**
   * Insert a tag at the specified position
   * @param {string} text - Original text
   * @param {number} start - Start position
   * @param {string} tagName - Tag name to insert
   * @param {Object} attributes - Tag attributes
   * @param {number} [end] - End position for wrapping selection
   */
  insertTag(text, start, tagName, attributes = {}, end = null) {
    const attrString = Object.entries(attributes)
      .map(([key, value]) => `${key}="${value}"`)
      .join(' ');

    // Self-closing tags
    const selfClosingTags = ['break', 'mark', 'audio'];

    if (selfClosingTags.includes(tagName)) {
      const tag = attrString ? `<${tagName} ${attrString}/>` : `<${tagName}/>`;
      return text.slice(0, start) + tag + text.slice(start);
    }

    // Wrapping tags
    const openTag = attrString ? `<${tagName} ${attrString}>` : `<${tagName}>`;
    const closeTag = `</${tagName}>`;

    if (end !== null && end > start) {
      const selection = text.slice(start, end);
      return text.slice(0, start) + openTag + selection + closeTag + text.slice(end);
    }

    // Insert empty tag at cursor
    return text.slice(0, start) + openTag + closeTag + text.slice(start);
  }

  /**
   * Extract plain text from SSML
   */
  extractPlainText(ssml) {
    // Remove all tags but preserve text content
    return ssml
      .replace(/<break[^>]*\/>/gi, ' ')
      .replace(/<[^>]+>/g, '')
      .replace(/\s+/g, ' ')
      .trim();
  }

  /**
   * Get information about the tag at a specific position
   * Returns null for the outer speak tag - only returns inner SSML tags
   */
  getTagAtPosition(ssml, position) {
    // Find the innermost tag containing the position (excluding speak wrapper)
    const tagRegex = /<([a-zA-Z:_-]+)([^>]*)>([^<]*)<\/\1>/g;
    let match;
    let result = null;

    while ((match = tagRegex.exec(ssml)) !== null) {
      const tagStart = match.index;
      const tagEnd = tagStart + match[0].length;
      const tagName = match[1].toLowerCase();

      // Skip the outer speak tag
      if (tagName === 'speak') continue;

      if (position >= tagStart && position <= tagEnd) {
        const attributes = this.parseAttributes(match[2]);
        result = { tag: tagName, attributes, start: tagStart, end: tagEnd };
      }
    }

    return result;
  }

  /**
   * Parse attributes string into object
   */
  parseAttributes(attrString) {
    const attrs = {};
    const regex = /([a-zA-Z-]+)=["']([^"']*)["']/g;
    let match;
    while ((match = regex.exec(attrString)) !== null) {
      attrs[match[1]] = match[2];
    }
    return attrs;
  }

  /**
   * Generate SSML template string
   */
  generateSSMLTemplate(tagName, attributes = {}, content = '') {
    const attrString = Object.entries(attributes)
      .map(([key, value]) => `${key}="${value}"`)
      .join(' ');

    const selfClosingTags = ['break', 'mark', 'audio'];

    if (selfClosingTags.includes(tagName)) {
      return attrString ? `<${tagName} ${attrString}/>` : `<${tagName}/>`;
    }

    const openTag = attrString ? `<${tagName} ${attrString}>` : `<${tagName}>`;
    return `${openTag}${content}</${tagName}>`;
  }
}

/**
 * SSML Editor - UI component for editing SSML
 */
export class SSMLEditor {
  constructor(container, options = {}) {
    this.container = container;
    this.options = {
      autoValidate: true,
      showLineNumbers: false,
      ...options
    };

    this.validator = new SSMLValidator();
    this.parser = new SSMLParser();
    this.mode = 'text'; // 'text' or 'ssml'
    this.listeners = {};

    this.quickTemplates = [
      { name: 'Pause', id: 'pause', template: '<break time="500ms"/>' },
      { name: 'Long Pause', id: 'long-pause', template: '<break time="1s"/>' },
      { name: 'Whisper', id: 'whisper', template: '<amazon:effect name="whispered">{text}</amazon:effect>' },
      { name: 'Spell Out', id: 'spell-out', template: '<say-as interpret-as="characters">{text}</say-as>' },
      { name: 'Slow', id: 'slow', template: '<prosody rate="slow">{text}</prosody>' },
      { name: 'Fast', id: 'fast', template: '<prosody rate="fast">{text}</prosody>' },
      { name: 'Loud', id: 'loud', template: '<prosody volume="loud">{text}</prosody>' },
      { name: 'Soft', id: 'soft', template: '<prosody volume="soft">{text}</prosody>' },
      { name: 'High Pitch', id: 'high-pitch', template: '<prosody pitch="high">{text}</prosody>' },
      { name: 'Low Pitch', id: 'low-pitch', template: '<prosody pitch="low">{text}</prosody>' },
      { name: 'Emphasis', id: 'emphasis', template: '<emphasis level="strong">{text}</emphasis>' },
      { name: 'Phone Number', id: 'phone', template: '<say-as interpret-as="telephone">{text}</say-as>' },
      { name: 'Date', id: 'date', template: '<say-as interpret-as="date" format="mdy">{text}</say-as>' },
    ];

    this.render();
    this.bindEvents();
  }

  /**
   * Render the editor UI
   */
  render() {
    const editor = document.createElement('div');
    editor.className = 'ssml-editor';

    editor.innerHTML = `
      <div class="ssml-toolbar">
        <div class="ssml-toolbar-group">
          <button class="ssml-btn" data-tag="break" title="Insert pause">
            <span class="ssml-btn-icon">⏸</span>
            <span class="ssml-btn-label">Break</span>
          </button>
          <button class="ssml-btn" data-tag="emphasis" title="Add emphasis">
            <span class="ssml-btn-icon">❗</span>
            <span class="ssml-btn-label">Emphasis</span>
          </button>
          <button class="ssml-btn" data-tag="prosody" title="Change rate/pitch">
            <span class="ssml-btn-icon">🎛️</span>
            <span class="ssml-btn-label">Prosody</span>
          </button>
          <button class="ssml-btn" data-tag="say-as" title="Interpret as type">
            <span class="ssml-btn-icon">🔢</span>
            <span class="ssml-btn-label">Say-As</span>
          </button>
          <button class="ssml-btn" data-tag="sub" title="Substitute text">
            <span class="ssml-btn-icon">↔️</span>
            <span class="ssml-btn-label">Sub</span>
          </button>
          <button class="ssml-btn" data-tag="phoneme" title="Phonetic pronunciation">
            <span class="ssml-btn-icon">🔤</span>
            <span class="ssml-btn-label">Phoneme</span>
          </button>
        </div>
        <div class="ssml-toolbar-group">
          <select class="ssml-templates-select" title="Quick templates">
            <option value="">Templates...</option>
            ${this.quickTemplates.map(t => `<option value="${t.id}">${t.name}</option>`).join('')}
          </select>
        </div>
        <div class="ssml-toolbar-group ssml-toolbar-right">
          <button class="ssml-btn ssml-btn-secondary" data-action="format" title="Auto-format">
            <span class="ssml-btn-icon">📐</span>
          </button>
          <button class="ssml-btn ssml-btn-secondary" data-action="validate" title="Validate SSML">
            <span class="ssml-btn-icon">✓</span>
          </button>
          <div class="ssml-mode-toggle">
            <button class="ssml-mode-btn active" data-mode="text">Text</button>
            <button class="ssml-mode-btn" data-mode="ssml">SSML</button>
          </div>
        </div>
      </div>
      <div class="ssml-editor-body">
        <textarea class="ssml-textarea" placeholder="Enter text to speak..."></textarea>
        <div class="ssml-preview hidden"></div>
      </div>
      <div class="ssml-status">
        <span class="ssml-status-indicator" data-status="empty"></span>
        <span class="ssml-status-text">Ready</span>
      </div>
    `;

    this.container.appendChild(editor);

    // Cache elements
    this.editorEl = editor;
    this.toolbar = editor.querySelector('.ssml-toolbar');
    this.textarea = editor.querySelector('.ssml-textarea');
    this.preview = editor.querySelector('.ssml-preview');
    this.status = editor.querySelector('.ssml-status');
    this.statusIndicator = editor.querySelector('.ssml-status-indicator');
    this.statusText = editor.querySelector('.ssml-status-text');
  }

  /**
   * Bind event listeners
   */
  bindEvents() {
    // Toolbar buttons
    this.toolbar.addEventListener('click', (e) => {
      const btn = e.target.closest('.ssml-btn');
      if (!btn) return;

      const tag = btn.dataset.tag;
      const action = btn.dataset.action;

      if (tag) {
        this.handleTagButton(tag);
      } else if (action) {
        this.handleAction(action);
      }
    });

    // Mode toggle
    this.toolbar.querySelectorAll('.ssml-mode-btn').forEach(btn => {
      btn.addEventListener('click', () => {
        const mode = btn.dataset.mode;
        this.setMode(mode);
      });
    });

    // Template select
    const templateSelect = this.toolbar.querySelector('.ssml-templates-select');
    if (templateSelect) {
      templateSelect.addEventListener('change', (e) => {
        if (e.target.value) {
          this.applyQuickTemplate(e.target.value);
          e.target.value = '';
        }
      });
    }

    // Textarea events
    this.textarea.addEventListener('input', () => {
      this.emit('change', this.getText());
      if (this.options.autoValidate) {
        this.updateStatus();
      }
    });

    this.textarea.addEventListener('keydown', (e) => {
      // Tab key inserts spaces
      if (e.key === 'Tab') {
        e.preventDefault();
        const start = this.textarea.selectionStart;
        const end = this.textarea.selectionEnd;
        const value = this.textarea.value;
        this.textarea.value = value.substring(0, start) + '  ' + value.substring(end);
        this.textarea.selectionStart = this.textarea.selectionEnd = start + 2;
      }
    });
  }

  /**
   * Handle toolbar tag button click
   */
  handleTagButton(tagName) {
    const defaults = {
      break: { time: '500ms' },
      emphasis: { level: 'strong' },
      prosody: { rate: 'medium' },
      'say-as': { 'interpret-as': 'characters' },
      sub: { alias: '' },
      phoneme: { alphabet: 'ipa', ph: '' }
    };

    this.insertTag(tagName, defaults[tagName] || {});
  }

  /**
   * Handle toolbar action button click
   */
  handleAction(action) {
    switch (action) {
      case 'format':
        this.autoFormat();
        break;
      case 'validate':
        const result = this.validate();
        this.showValidationResult(result);
        break;
    }
  }

  /**
   * Get current text content
   */
  getText() {
    return this.textarea.value;
  }

  /**
   * Set text content
   */
  setText(text) {
    this.textarea.value = text;
    this.emit('change', text);
    this.updateStatus();
  }

  /**
   * Get content as SSML (with speak tags)
   */
  getSSML() {
    return this.parser.wrapInSpeak(this.getText());
  }

  /**
   * Get plain text (stripped of all SSML tags)
   */
  getPlainText() {
    return this.parser.extractPlainText(this.getText());
  }

  /**
   * Validate current content
   */
  validate() {
    const ssml = this.getSSML();
    const result = this.validator.validate(ssml);
    this.emit('validate', result);
    return result;
  }

  /**
   * Insert a tag at cursor position or around selection
   */
  insertTag(tagName, attributes = {}) {
    const start = this.textarea.selectionStart;
    const end = this.textarea.selectionEnd;
    const text = this.getText();

    const newText = this.parser.insertTag(text, start, tagName, attributes, end > start ? end : null);
    this.setText(newText);

    // Position cursor after the inserted tag
    this.textarea.focus();
    const insertedLength = newText.length - text.length;
    this.textarea.selectionStart = this.textarea.selectionEnd = end + insertedLength;
  }

  /**
   * Apply a quick template
   */
  applyQuickTemplate(templateId) {
    const template = this.quickTemplates.find(t => t.id === templateId);
    if (!template) return;

    const start = this.textarea.selectionStart;
    const end = this.textarea.selectionEnd;
    const text = this.getText();
    const selection = text.slice(start, end) || 'text';

    const ssmlTemplate = template.template.replace('{text}', selection);
    const newText = text.slice(0, start) + ssmlTemplate + text.slice(end);

    this.setText(newText);
    this.textarea.focus();
  }

  /**
   * Get available quick templates
   */
  getQuickTemplates() {
    return [...this.quickTemplates];
  }

  /**
   * Auto-format SSML with proper indentation
   */
  autoFormat() {
    const text = this.getText();
    let formatted = text;

    // Add newlines after opening tags
    formatted = formatted.replace(/(<[^/][^>]*>)([^<])/g, '$1\n$2');
    // Add newlines before closing tags
    formatted = formatted.replace(/([^>])(<\/)/g, '$1\n$2');

    // Basic indentation
    const lines = formatted.split('\n');
    let indent = 0;
    const formattedLines = lines.map(line => {
      const trimmed = line.trim();
      if (trimmed.startsWith('</')) {
        indent = Math.max(0, indent - 1);
      }
      const result = '  '.repeat(indent) + trimmed;
      if (trimmed.startsWith('<') && !trimmed.startsWith('</') && !trimmed.endsWith('/>')) {
        indent++;
      }
      return result;
    });

    const result = formattedLines.join('\n');
    this.setText(result);
    return result;
  }

  /**
   * Get current editor mode
   */
  getMode() {
    return this.mode;
  }

  /**
   * Set editor mode
   */
  setMode(mode) {
    this.mode = mode;

    // Update mode buttons
    this.toolbar.querySelectorAll('.ssml-mode-btn').forEach(btn => {
      btn.classList.toggle('active', btn.dataset.mode === mode);
    });

    // Toggle preview
    if (mode === 'ssml') {
      this.showPreview();
    } else {
      this.hidePreview();
    }
  }

  /**
   * Show SSML preview
   */
  showPreview() {
    this.preview.classList.remove('hidden');
    this.updatePreview();
  }

  /**
   * Hide SSML preview
   */
  hidePreview() {
    this.preview.classList.add('hidden');
  }

  /**
   * Update preview content
   */
  updatePreview() {
    const ssml = this.getSSML();
    // Syntax highlight the SSML
    const highlighted = this.highlightSSML(ssml);
    this.preview.innerHTML = `<pre>${highlighted}</pre>`;
  }

  /**
   * Highlight SSML syntax
   */
  highlightSSML(ssml) {
    return ssml
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/&lt;([a-zA-Z:_-]+)/g, '&lt;<span class="ssml-tag">$1</span>')
      .replace(/&lt;\/([a-zA-Z:_-]+)&gt;/g, '&lt;/<span class="ssml-tag">$1</span>&gt;')
      .replace(/([a-zA-Z-]+)=["']([^"']*)["']/g, '<span class="ssml-attr">$1</span>=<span class="ssml-value">"$2"</span>');
  }

  /**
   * Update status indicator
   */
  updateStatus() {
    const text = this.getText();

    if (!text) {
      this.statusIndicator.dataset.status = 'empty';
      this.statusText.textContent = 'Ready';
      return;
    }

    const result = this.validator.validate(this.getSSML());

    if (result.valid) {
      this.statusIndicator.dataset.status = 'valid';
      this.statusText.textContent = 'Valid SSML';
    } else {
      this.statusIndicator.dataset.status = 'error';
      this.statusText.textContent = result.errors[0] || 'Invalid SSML';
    }
  }

  /**
   * Show validation result
   */
  showValidationResult(result) {
    if (result.valid) {
      this.statusIndicator.dataset.status = 'valid';
      this.statusText.textContent = '✓ Valid SSML';
    } else {
      this.statusIndicator.dataset.status = 'error';
      this.statusText.textContent = `✗ ${result.errors.length} error(s): ${result.errors[0]}`;
    }
  }

  /**
   * Add event listener
   */
  on(event, handler) {
    if (!this.listeners[event]) {
      this.listeners[event] = [];
    }
    this.listeners[event].push(handler);
  }

  /**
   * Remove event listener
   */
  off(event, handler) {
    if (!this.listeners[event]) return;
    this.listeners[event] = this.listeners[event].filter(h => h !== handler);
  }

  /**
   * Emit event
   */
  emit(event, ...args) {
    if (!this.listeners[event]) return;
    this.listeners[event].forEach(handler => handler(...args));
  }

  /**
   * Destroy the editor
   */
  destroy() {
    if (this.editorEl && this.editorEl.parentNode) {
      this.editorEl.parentNode.removeChild(this.editorEl);
    }
    this.listeners = {};
  }
}

export default SSMLEditor;

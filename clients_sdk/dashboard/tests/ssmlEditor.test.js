/**
 * SSML Editor Tests
 */

import { jest, describe, test, expect, beforeEach } from '@jest/globals';
import { SSMLEditor, SSMLValidator, SSMLParser } from '../js/ssmlEditor.js';

describe('SSMLValidator', () => {
  let validator;

  beforeEach(() => {
    validator = new SSMLValidator();
  });

  describe('validate', () => {
    test('should validate valid SSML', () => {
      const ssml = '<speak>Hello world</speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(true);
      expect(result.errors).toHaveLength(0);
    });

    test('should validate SSML with break tag', () => {
      const ssml = '<speak>Hello<break time="500ms"/>world</speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(true);
    });

    test('should validate SSML with emphasis', () => {
      const ssml = '<speak><emphasis level="strong">Important</emphasis> text</speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(true);
    });

    test('should validate SSML with prosody', () => {
      const ssml = '<speak><prosody rate="slow" pitch="high">Slow and high</prosody></speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(true);
    });

    test('should validate SSML with say-as', () => {
      const ssml = '<speak><say-as interpret-as="date">2024-01-15</say-as></speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(true);
    });

    test('should validate SSML with sub', () => {
      const ssml = '<speak><sub alias="World Wide Web Consortium">W3C</sub></speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(true);
    });

    test('should validate SSML with phoneme', () => {
      const ssml = '<speak><phoneme alphabet="ipa" ph="təˈmeɪtoʊ">tomato</phoneme></speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(true);
    });

    test('should validate nested tags', () => {
      const ssml = '<speak><prosody rate="fast"><emphasis level="moderate">Fast emphasized</emphasis></prosody></speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(true);
    });

    test('should detect missing speak tag', () => {
      const ssml = 'Hello world';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('SSML must be wrapped in <speak> tags');
    });

    test('should detect unclosed tags', () => {
      const ssml = '<speak><emphasis>Hello</speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Unclosed tag'))).toBe(true);
    });

    test('should detect invalid tags', () => {
      const ssml = '<speak><invalid>text</invalid></speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Invalid SSML tag'))).toBe(true);
    });

    test('should detect invalid emphasis level', () => {
      const ssml = '<speak><emphasis level="invalid">text</emphasis></speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Invalid emphasis level'))).toBe(true);
    });

    test('should detect invalid break time format', () => {
      const ssml = '<speak>Hello<break time="invalid"/>world</speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Invalid break time'))).toBe(true);
    });

    test('should detect invalid prosody rate', () => {
      const ssml = '<speak><prosody rate="invalid">text</prosody></speak>';
      const result = validator.validate(ssml);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Invalid prosody rate'))).toBe(true);
    });
  });

  describe('getSupportedTags', () => {
    test('should return list of supported SSML tags', () => {
      const tags = validator.getSupportedTags();
      expect(tags).toContain('speak');
      expect(tags).toContain('break');
      expect(tags).toContain('emphasis');
      expect(tags).toContain('prosody');
      expect(tags).toContain('say-as');
      expect(tags).toContain('sub');
      expect(tags).toContain('phoneme');
      expect(tags).toContain('p');
      expect(tags).toContain('s');
    });
  });
});

describe('SSMLParser', () => {
  let parser;

  beforeEach(() => {
    parser = new SSMLParser();
  });

  describe('wrapInSpeak', () => {
    test('should wrap plain text in speak tags', () => {
      const result = parser.wrapInSpeak('Hello world');
      expect(result).toBe('<speak>Hello world</speak>');
    });

    test('should not double-wrap if already has speak tags', () => {
      const result = parser.wrapInSpeak('<speak>Hello world</speak>');
      expect(result).toBe('<speak>Hello world</speak>');
    });
  });

  describe('stripSpeak', () => {
    test('should remove speak tags', () => {
      const result = parser.stripSpeak('<speak>Hello world</speak>');
      expect(result).toBe('Hello world');
    });

    test('should handle text without speak tags', () => {
      const result = parser.stripSpeak('Hello world');
      expect(result).toBe('Hello world');
    });
  });

  describe('insertTag', () => {
    test('should insert break tag', () => {
      const result = parser.insertTag('Hello world', 5, 'break', { time: '500ms' });
      expect(result).toBe('Hello<break time="500ms"/> world');
    });

    test('should wrap selection with emphasis', () => {
      const result = parser.insertTag('Hello world', 0, 'emphasis', { level: 'strong' }, 5);
      expect(result).toBe('<emphasis level="strong">Hello</emphasis> world');
    });

    test('should insert prosody around selection', () => {
      const result = parser.insertTag('Hello world', 6, 'prosody', { rate: 'slow' }, 11);
      expect(result).toBe('Hello <prosody rate="slow">world</prosody>');
    });

    test('should insert say-as around selection', () => {
      const result = parser.insertTag('Call 555-1234', 5, 'say-as', { 'interpret-as': 'telephone' }, 13);
      expect(result).toBe('Call <say-as interpret-as="telephone">555-1234</say-as>');
    });

    test('should insert sub around selection', () => {
      const result = parser.insertTag('W3C is great', 0, 'sub', { alias: 'World Wide Web Consortium' }, 3);
      expect(result).toBe('<sub alias="World Wide Web Consortium">W3C</sub> is great');
    });
  });

  describe('extractPlainText', () => {
    test('should extract plain text from SSML', () => {
      const ssml = '<speak>Hello <emphasis level="strong">world</emphasis></speak>';
      const result = parser.extractPlainText(ssml);
      expect(result).toBe('Hello world');
    });

    test('should handle break tags', () => {
      const ssml = '<speak>Hello<break time="500ms"/>world</speak>';
      const result = parser.extractPlainText(ssml);
      expect(result).toBe('Hello world');
    });

    test('should handle nested tags', () => {
      const ssml = '<speak><prosody rate="fast"><emphasis>Text</emphasis></prosody></speak>';
      const result = parser.extractPlainText(ssml);
      expect(result).toBe('Text');
    });
  });

  describe('getTagAtPosition', () => {
    test('should return tag info at cursor position', () => {
      const ssml = '<speak><emphasis level="strong">Hello</emphasis></speak>';
      const result = parser.getTagAtPosition(ssml, 15);
      expect(result).not.toBeNull();
      expect(result.tag).toBe('emphasis');
      expect(result.attributes.level).toBe('strong');
    });

    test('should return null if not inside a tag', () => {
      const ssml = '<speak>Hello world</speak>';
      const result = parser.getTagAtPosition(ssml, 10);
      expect(result).toBeNull();
    });
  });

  describe('generateSSMLTemplate', () => {
    test('should generate break template', () => {
      const template = parser.generateSSMLTemplate('break', { time: '1s' });
      expect(template).toBe('<break time="1s"/>');
    });

    test('should generate emphasis template', () => {
      const template = parser.generateSSMLTemplate('emphasis', { level: 'moderate' }, 'text');
      expect(template).toBe('<emphasis level="moderate">text</emphasis>');
    });

    test('should generate prosody template', () => {
      const template = parser.generateSSMLTemplate('prosody', { rate: 'fast', pitch: 'high' }, 'text');
      expect(template).toBe('<prosody rate="fast" pitch="high">text</prosody>');
    });
  });
});

describe('SSMLEditor', () => {
  let editor;
  let container;

  beforeEach(() => {
    // Create container element
    container = document.createElement('div');
    container.id = 'ssml-editor-container';
    document.body.appendChild(container);

    editor = new SSMLEditor(container);
  });

  afterEach(() => {
    if (container && container.parentNode) {
      container.parentNode.removeChild(container);
    }
  });

  describe('initialization', () => {
    test('should create editor elements', () => {
      expect(container.querySelector('.ssml-editor')).not.toBeNull();
      expect(container.querySelector('.ssml-toolbar')).not.toBeNull();
      expect(container.querySelector('.ssml-textarea')).not.toBeNull();
    });

    test('should create toolbar buttons', () => {
      const toolbar = container.querySelector('.ssml-toolbar');
      expect(toolbar.querySelector('[data-tag="break"]')).not.toBeNull();
      expect(toolbar.querySelector('[data-tag="emphasis"]')).not.toBeNull();
      expect(toolbar.querySelector('[data-tag="prosody"]')).not.toBeNull();
      expect(toolbar.querySelector('[data-tag="say-as"]')).not.toBeNull();
    });

    test('should have mode toggle', () => {
      expect(container.querySelector('.ssml-mode-toggle')).not.toBeNull();
    });
  });

  describe('getText', () => {
    test('should return current text', () => {
      editor.setText('Hello world');
      expect(editor.getText()).toBe('Hello world');
    });
  });

  describe('setText', () => {
    test('should set text content', () => {
      editor.setText('Test content');
      const textarea = container.querySelector('.ssml-textarea');
      expect(textarea.value).toBe('Test content');
    });
  });

  describe('getSSML', () => {
    test('should return SSML with speak tags', () => {
      editor.setText('Hello world');
      const ssml = editor.getSSML();
      expect(ssml).toBe('<speak>Hello world</speak>');
    });

    test('should preserve existing SSML tags', () => {
      editor.setText('<speak><emphasis>Hello</emphasis></speak>');
      const ssml = editor.getSSML();
      expect(ssml).toContain('<emphasis>Hello</emphasis>');
    });
  });

  describe('validate', () => {
    test('should return validation result', () => {
      editor.setText('<speak>Hello world</speak>');
      const result = editor.validate();
      expect(result.valid).toBe(true);
    });

    test('should detect errors', () => {
      editor.setText('<speak><invalid>text</invalid></speak>');
      const result = editor.validate();
      expect(result.valid).toBe(false);
    });
  });

  describe('insertTag', () => {
    test('should insert tag at cursor position', () => {
      editor.setText('Hello world');
      const textarea = container.querySelector('.ssml-textarea');
      textarea.selectionStart = 5;
      textarea.selectionEnd = 5;

      editor.insertTag('break', { time: '500ms' });

      expect(editor.getText()).toContain('<break time="500ms"/>');
    });

    test('should wrap selection with tag', () => {
      editor.setText('Hello world');
      const textarea = container.querySelector('.ssml-textarea');
      textarea.selectionStart = 6;
      textarea.selectionEnd = 11;

      editor.insertTag('emphasis', { level: 'strong' });

      expect(editor.getText()).toContain('<emphasis level="strong">world</emphasis>');
    });
  });

  describe('events', () => {
    test('should emit change event on text change', () => {
      const handler = jest.fn();
      editor.on('change', handler);

      editor.setText('New content');

      expect(handler).toHaveBeenCalled();
    });

    test('should emit validate event after validation', () => {
      const handler = jest.fn();
      editor.on('validate', handler);

      editor.setText('<speak>Hello</speak>');
      editor.validate();

      expect(handler).toHaveBeenCalledWith(expect.objectContaining({ valid: true }));
    });

    test('should support removing listeners', () => {
      const handler = jest.fn();
      editor.on('change', handler);
      editor.off('change', handler);

      editor.setText('New content');

      expect(handler).not.toHaveBeenCalled();
    });
  });

  describe('mode toggle', () => {
    test('should toggle between text and SSML mode', () => {
      expect(editor.getMode()).toBe('text');

      editor.setMode('ssml');
      expect(editor.getMode()).toBe('ssml');

      editor.setMode('text');
      expect(editor.getMode()).toBe('text');
    });

    test('should show preview in SSML mode', () => {
      editor.setText('<speak>Hello <emphasis>world</emphasis></speak>');
      editor.setMode('ssml');

      const preview = container.querySelector('.ssml-preview');
      expect(preview).not.toBeNull();
    });
  });

  describe('toolbar buttons', () => {
    test('should insert break when break button clicked', () => {
      editor.setText('Hello world');
      const textarea = container.querySelector('.ssml-textarea');
      textarea.selectionStart = 5;
      textarea.selectionEnd = 5;

      const breakBtn = container.querySelector('[data-tag="break"]');
      breakBtn.click();

      expect(editor.getText()).toContain('<break');
    });
  });

  describe('quick templates', () => {
    test('should have quick template options', () => {
      const templates = editor.getQuickTemplates();
      expect(templates).toContainEqual(expect.objectContaining({ name: 'Pause' }));
      expect(templates).toContainEqual(expect.objectContaining({ name: 'Whisper' }));
      expect(templates).toContainEqual(expect.objectContaining({ name: 'Spell Out' }));
    });

    test('should apply quick template', () => {
      editor.setText('Hello world');
      const textarea = container.querySelector('.ssml-textarea');
      textarea.selectionStart = 6;
      textarea.selectionEnd = 11;

      editor.applyQuickTemplate('whisper');

      expect(editor.getText()).toContain('<amazon:effect name="whispered">world</amazon:effect>');
    });
  });

  describe('getPlainText', () => {
    test('should extract plain text from SSML content', () => {
      editor.setText('<speak>Hello <emphasis>world</emphasis></speak>');
      expect(editor.getPlainText()).toBe('Hello world');
    });
  });

  describe('autoFormat', () => {
    test('should format SSML with proper indentation', () => {
      editor.setText('<speak><prosody rate="fast"><emphasis>Text</emphasis></prosody></speak>');
      const formatted = editor.autoFormat();
      expect(formatted).toContain('\n');
    });
  });

  describe('destroy', () => {
    test('should clean up editor', () => {
      editor.destroy();
      expect(container.querySelector('.ssml-editor')).toBeNull();
    });
  });
});

describe('SSML Tag Attributes', () => {
  let validator;

  beforeEach(() => {
    validator = new SSMLValidator();
  });

  describe('break tag', () => {
    test('should accept time in milliseconds', () => {
      const result = validator.validate('<speak><break time="250ms"/></speak>');
      expect(result.valid).toBe(true);
    });

    test('should accept time in seconds', () => {
      const result = validator.validate('<speak><break time="1s"/></speak>');
      expect(result.valid).toBe(true);
    });

    test('should accept strength attribute', () => {
      const result = validator.validate('<speak><break strength="medium"/></speak>');
      expect(result.valid).toBe(true);
    });
  });

  describe('prosody tag', () => {
    test('should accept rate as percentage', () => {
      const result = validator.validate('<speak><prosody rate="150%">text</prosody></speak>');
      expect(result.valid).toBe(true);
    });

    test('should accept rate as keyword', () => {
      const result = validator.validate('<speak><prosody rate="x-slow">text</prosody></speak>');
      expect(result.valid).toBe(true);
    });

    test('should accept pitch as percentage', () => {
      const result = validator.validate('<speak><prosody pitch="+20%">text</prosody></speak>');
      expect(result.valid).toBe(true);
    });

    test('should accept volume as keyword', () => {
      const result = validator.validate('<speak><prosody volume="loud">text</prosody></speak>');
      expect(result.valid).toBe(true);
    });
  });

  describe('say-as tag', () => {
    test('should accept cardinal interpret-as', () => {
      const result = validator.validate('<speak><say-as interpret-as="cardinal">123</say-as></speak>');
      expect(result.valid).toBe(true);
    });

    test('should accept ordinal interpret-as', () => {
      const result = validator.validate('<speak><say-as interpret-as="ordinal">1</say-as></speak>');
      expect(result.valid).toBe(true);
    });

    test('should accept characters interpret-as', () => {
      const result = validator.validate('<speak><say-as interpret-as="characters">ABC</say-as></speak>');
      expect(result.valid).toBe(true);
    });

    test('should accept date interpret-as', () => {
      const result = validator.validate('<speak><say-as interpret-as="date" format="mdy">01/15/2024</say-as></speak>');
      expect(result.valid).toBe(true);
    });
  });
});

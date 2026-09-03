/**
 * Shared test harness: a minimal jsdom DOM + the built widget ESM bundle.
 *
 * The bundle auto-registers the `<bud-widget>` custom element on import, so the
 * DOM globals (window/document/HTMLElement/customElements/CustomEvent) must be
 * installed BEFORE importing it. We install them here and re-export the loaded
 * module so every test file shares one jsdom + one module instance.
 *
 * Run with the stock Node test runner from the widget dir:
 *   node --test test/
 * (Node resolves `jsdom` from the widget's own node_modules.)
 */
import { JSDOM } from 'jsdom';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export const WIDGET_DIR = path.resolve(__dirname, '..');
export const BUNDLE_PATH = path.join(WIDGET_DIR, 'dist', 'bud-widget.esm.js');

// Install a jsdom window + the DOM globals the bundle touches at import time.
const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>', {
  url: 'http://localhost/',
});
const win = dom.window;
globalThis.window = win;
globalThis.document = win.document;
globalThis.HTMLElement = win.HTMLElement;
globalThis.customElements = win.customElements;
globalThis.CustomEvent = win.CustomEvent;
globalThis.Event = win.Event;
globalThis.Node = win.Node;
if (typeof globalThis.performance === 'undefined') {
  globalThis.performance = win.performance;
}

// Import AFTER globals are installed so defineWidget() can register the element.
export const widget = await import(`file://${BUNDLE_PATH}`);

/**
 * Build a `<bud-widget>` carrying the given `data-*` attributes and return it.
 * Keys are camelCase dataset names (e.g. `llmModel` -> `data-llm-model`).
 */
export function makeWidgetElement(dataAttrs = {}) {
  const el = win.document.createElement('bud-widget');
  for (const [k, v] of Object.entries(dataAttrs)) {
    if (v !== undefined && v !== null) {
      el.dataset[k] = String(v);
    }
  }
  return el;
}

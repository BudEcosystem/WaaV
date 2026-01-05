/**
 * Widget CSS styles
 */

export const widgetStyles = `
  :host {
    --bud-primary: #6366f1;
    --bud-primary-hover: #4f46e5;
    --bud-success: #10b981;
    --bud-warning: #f59e0b;
    --bud-error: #ef4444;
    --bud-bg: #ffffff;
    --bud-bg-secondary: #f3f4f6;
    --bud-text: #1f2937;
    --bud-text-secondary: #6b7280;
    --bud-border: #e5e7eb;
    --bud-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.1), 0 8px 10px -6px rgba(0, 0, 0, 0.1);
    --bud-radius: 12px;
    --bud-transition: 150ms ease;

    display: block;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
    font-size: 14px;
    line-height: 1.5;
    position: fixed;
    z-index: 9999;
  }

  :host([data-theme="dark"]) {
    --bud-bg: #1f2937;
    --bud-bg-secondary: #374151;
    --bud-text: #f9fafb;
    --bud-text-secondary: #9ca3af;
    --bud-border: #4b5563;
  }

  :host([data-position="bottom-right"]) {
    bottom: 20px;
    right: 20px;
  }

  :host([data-position="bottom-left"]) {
    bottom: 20px;
    left: 20px;
  }

  :host([data-position="top-right"]) {
    top: 20px;
    right: 20px;
  }

  :host([data-position="top-left"]) {
    top: 20px;
    left: 20px;
  }

  .bud-widget {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .bud-button {
    width: 56px;
    height: 56px;
    border-radius: 50%;
    border: none;
    background: var(--bud-primary);
    color: white;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    box-shadow: var(--bud-shadow);
    transition: transform var(--bud-transition), background var(--bud-transition);
  }

  .bud-button:hover {
    background: var(--bud-primary-hover);
    transform: scale(1.05);
  }

  .bud-button:active {
    transform: scale(0.95);
  }

  .bud-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
    transform: none;
  }

  .bud-button svg {
    width: 24px;
    height: 24px;
    fill: currentColor;
  }

  .bud-button.listening {
    background: var(--bud-success);
    animation: pulse 2s infinite;
  }

  .bud-button.speaking {
    background: var(--bud-warning);
  }

  .bud-button.error {
    background: var(--bud-error);
  }

  .bud-button.connecting {
    background: var(--bud-primary);
  }

  .bud-button.connecting svg {
    animation: spin 1s linear infinite;
  }

  @keyframes pulse {
    0% {
      box-shadow: 0 0 0 0 rgba(16, 185, 129, 0.4);
    }
    70% {
      box-shadow: 0 0 0 10px rgba(16, 185, 129, 0);
    }
    100% {
      box-shadow: 0 0 0 0 rgba(16, 185, 129, 0);
    }
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .bud-panel {
    position: absolute;
    bottom: 70px;
    right: 0;
    width: 300px;
    background: var(--bud-bg);
    border-radius: var(--bud-radius);
    box-shadow: var(--bud-shadow);
    border: 1px solid var(--bud-border);
    overflow: hidden;
    opacity: 0;
    transform: translateY(10px);
    pointer-events: none;
    transition: opacity var(--bud-transition), transform var(--bud-transition);
  }

  .bud-panel.open {
    opacity: 1;
    transform: translateY(0);
    pointer-events: auto;
  }

  .bud-panel-header {
    padding: 12px 16px;
    background: var(--bud-bg-secondary);
    border-bottom: 1px solid var(--bud-border);
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .bud-panel-title {
    font-weight: 600;
    color: var(--bud-text);
  }

  .bud-panel-status {
    font-size: 12px;
    color: var(--bud-text-secondary);
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .bud-panel-status::before {
    content: '';
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--bud-text-secondary);
  }

  .bud-panel-status.connected::before {
    background: var(--bud-success);
  }

  .bud-panel-status.error::before {
    background: var(--bud-error);
  }

  .bud-panel-content {
    padding: 16px;
    max-height: 300px;
    overflow-y: auto;
  }

  .bud-transcript {
    background: var(--bud-bg-secondary);
    border-radius: 8px;
    padding: 12px;
    margin-bottom: 12px;
    min-height: 80px;
    color: var(--bud-text);
  }

  .bud-transcript-text {
    white-space: pre-wrap;
    word-break: break-word;
  }

  .bud-transcript-text.interim {
    color: var(--bud-text-secondary);
    font-style: italic;
  }

  .bud-transcript-empty {
    color: var(--bud-text-secondary);
    text-align: center;
    font-style: italic;
  }

  .bud-metrics {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
    padding: 12px;
    background: var(--bud-bg-secondary);
    border-radius: 8px;
    font-size: 12px;
  }

  .bud-metric {
    display: flex;
    flex-direction: column;
  }

  .bud-metric-label {
    color: var(--bud-text-secondary);
  }

  .bud-metric-value {
    font-weight: 600;
    color: var(--bud-text);
    font-family: ui-monospace, SFMono-Regular, 'SF Mono', Menlo, monospace;
  }

  .bud-visualizer {
    height: 40px;
    display: flex;
    align-items: flex-end;
    justify-content: center;
    gap: 2px;
    margin-top: 12px;
  }

  .bud-visualizer-bar {
    width: 4px;
    background: var(--bud-primary);
    border-radius: 2px;
    transition: height 50ms ease;
  }

  @media (prefers-color-scheme: dark) {
    :host([data-theme="auto"]) {
      --bud-bg: #1f2937;
      --bud-bg-secondary: #374151;
      --bud-text: #f9fafb;
      --bud-text-secondary: #9ca3af;
      --bud-border: #4b5563;
    }
  }
`;

/**
 * Request Logger for Dashboard
 */

export class Logger {
  constructor(element, maxEntries = 100) {
    this.element = element;
    this.maxEntries = maxEntries;
  }

  log(method, endpoint, details = {}) {
    // Guard against null element
    if (!this.element) {
      console.log(`[Logger] ${method} ${endpoint}`, details);
      return;
    }

    const entry = document.createElement('div');
    entry.className = 'log-entry';

    const time = new Date().toLocaleTimeString();
    const detailsStr = Object.entries(details)
      .map(([k, v]) => `${k}: ${v}`)
      .join(', ');

    entry.innerHTML = `
      <div class="log-time">${time}</div>
      <div>
        <span class="log-method">${method}</span>
        <span class="log-endpoint">${endpoint}</span>
        ${details.duration ? `<span class="log-duration">${details.duration}</span>` : ''}
      </div>
      ${detailsStr ? `<div class="log-details">${detailsStr}</div>` : ''}
    `;

    this.element.insertBefore(entry, this.element.firstChild);

    // Limit entries
    while (this.element.children.length > this.maxEntries) {
      this.element.removeChild(this.element.lastChild);
    }
  }

  clear() {
    if (this.element) {
      this.element.innerHTML = '';
    }
  }
}

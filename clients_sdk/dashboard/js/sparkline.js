/**
 * Sparkline Module for WaaV Dashboard
 * Lightweight inline charts for metrics visualization
 */

/**
 * Sparkline - renders small inline charts
 */
export class Sparkline {
  constructor(container, options = {}) {
    this.container = container;
    this.options = {
      type: options.type || 'line', // line, area, bar
      strokeColor: options.strokeColor || '#6366f1',
      fillColor: options.fillColor || 'rgba(99, 102, 241, 0.1)',
      lineWidth: options.lineWidth || 1.5,
      maxPoints: options.maxPoints || 50,
      autoRender: options.autoRender ?? true,
      padding: options.padding || 2,
      threshold: options.threshold || null,
      thresholdColor: options.thresholdColor || 'rgba(239, 68, 68, 0.5)',
      gradient: options.gradient || null,
      positiveColor: options.positiveColor || null,
      negativeColor: options.negativeColor || null,
      baseline: options.baseline ?? null,
      animated: options.animated || false,
      animationDuration: options.animationDuration || 300,
      showDot: options.showDot ?? true,
      dotColor: options.dotColor || options.strokeColor || '#6366f1',
      dotRadius: options.dotRadius || 2,
      minValue: options.minValue ?? null,
      maxValue: options.maxValue ?? null,
      ...options,
    };

    this.data = [];
    this.canvas = null;
    this.ctx = null;
    this.animationFrame = null;

    this.init();
  }

  /**
   * Initialize canvas
   */
  init() {
    this.canvas = document.createElement('canvas');
    this.canvas.className = 'sparkline-canvas';
    this.canvas.style.display = 'block';
    this.canvas.style.width = '100%';
    this.canvas.style.height = '100%';
    this.container.appendChild(this.canvas);

    try {
      this.ctx = this.canvas.getContext('2d');
    } catch (e) {
      // Canvas not supported (e.g., in test environment)
      this.ctx = null;
    }
    this.resize();
  }

  /**
   * Resize canvas to container
   */
  resize() {
    const rect = this.container.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;

    this.canvas.width = (rect.width || 80) * dpr;
    this.canvas.height = (rect.height || 24) * dpr;
    this.canvas.style.width = `${rect.width || 80}px`;
    this.canvas.style.height = `${rect.height || 24}px`;

    this.width = rect.width || 80;
    this.height = rect.height || 24;

    if (this.ctx) {
      this.ctx.scale(dpr, dpr);
    }

    if (this.data.length > 0) {
      this.render();
    }
  }

  /**
   * Add a data point
   */
  addPoint(value) {
    this.data.push(value);
    if (this.data.length > this.options.maxPoints) {
      this.data.shift();
    }
    if (this.options.autoRender) {
      this.render();
    }
  }

  /**
   * Set entire data array
   */
  setData(data) {
    this.data = data.slice(-this.options.maxPoints);
    if (this.options.autoRender) {
      this.render();
    }
  }

  /**
   * Clear data
   */
  clear() {
    this.data = [];
    if (this.options.autoRender) {
      this.render();
    }
  }

  /**
   * Get minimum value
   */
  getMin() {
    if (this.data.length === 0) return null;
    return Math.min(...this.data);
  }

  /**
   * Get maximum value
   */
  getMax() {
    if (this.data.length === 0) return null;
    return Math.max(...this.data);
  }

  /**
   * Get average value
   */
  getAverage() {
    if (this.data.length === 0) return null;
    return this.data.reduce((a, b) => a + b, 0) / this.data.length;
  }

  /**
   * Get last value
   */
  getLast() {
    if (this.data.length === 0) return null;
    return this.data[this.data.length - 1];
  }

  /**
   * Render the sparkline
   */
  render() {
    if (!this.ctx) return;

    const { width, height } = this;
    const padding = this.options.padding;

    // Clear canvas
    this.ctx.clearRect(0, 0, width, height);

    if (this.data.length === 0) return;

    // Calculate bounds
    const minVal = this.options.minValue ?? Math.min(...this.data);
    const maxVal = this.options.maxValue ?? Math.max(...this.data);
    const range = maxVal - minVal || 1;

    // Draw based on type
    switch (this.options.type) {
      case 'bar':
        this.renderBars(minVal, range, padding);
        break;
      case 'area':
        this.renderArea(minVal, range, padding);
        break;
      case 'line':
      default:
        this.renderLine(minVal, range, padding);
        break;
    }

    // Draw threshold line
    if (this.options.threshold !== null) {
      this.renderThreshold(minVal, range, padding);
    }
  }

  /**
   * Render line chart
   */
  renderLine(minVal, range, padding) {
    const { width, height, ctx, data, options } = this;
    const drawWidth = width - padding * 2;
    const drawHeight = height - padding * 2;

    ctx.beginPath();
    ctx.lineWidth = options.lineWidth;
    ctx.strokeStyle = options.strokeColor;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';

    const points = this.calculatePoints(data, minVal, range, drawWidth, drawHeight, padding);

    // Draw line
    points.forEach((point, i) => {
      if (i === 0) {
        ctx.moveTo(point.x, point.y);
      } else {
        ctx.lineTo(point.x, point.y);
      }
    });

    ctx.stroke();

    // Draw end dot
    if (options.showDot && points.length > 0) {
      const lastPoint = points[points.length - 1];
      ctx.beginPath();
      ctx.arc(lastPoint.x, lastPoint.y, options.dotRadius, 0, Math.PI * 2);
      ctx.fillStyle = options.dotColor;
      ctx.fill();
    }
  }

  /**
   * Render area chart
   */
  renderArea(minVal, range, padding) {
    const { width, height, ctx, data, options } = this;
    const drawWidth = width - padding * 2;
    const drawHeight = height - padding * 2;

    const points = this.calculatePoints(data, minVal, range, drawWidth, drawHeight, padding);

    // Draw fill
    ctx.beginPath();
    ctx.moveTo(padding, height - padding);

    points.forEach((point, i) => {
      if (i === 0) {
        ctx.lineTo(point.x, point.y);
      } else {
        ctx.lineTo(point.x, point.y);
      }
    });

    ctx.lineTo(points[points.length - 1].x, height - padding);
    ctx.closePath();

    // Apply gradient if specified
    if (options.gradient && options.gradient.length >= 2) {
      const gradient = ctx.createLinearGradient(0, 0, 0, height);
      gradient.addColorStop(0, options.gradient[0]);
      gradient.addColorStop(1, options.gradient[1]);
      ctx.fillStyle = gradient;
    } else {
      ctx.fillStyle = options.fillColor;
    }
    ctx.fill();

    // Draw line on top
    ctx.beginPath();
    ctx.lineWidth = options.lineWidth;
    ctx.strokeStyle = options.strokeColor;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';

    points.forEach((point, i) => {
      if (i === 0) {
        ctx.moveTo(point.x, point.y);
      } else {
        ctx.lineTo(point.x, point.y);
      }
    });

    ctx.stroke();

    // Draw end dot
    if (options.showDot && points.length > 0) {
      const lastPoint = points[points.length - 1];
      ctx.beginPath();
      ctx.arc(lastPoint.x, lastPoint.y, options.dotRadius, 0, Math.PI * 2);
      ctx.fillStyle = options.dotColor;
      ctx.fill();
    }
  }

  /**
   * Render bar chart
   */
  renderBars(minVal, range, padding) {
    const { width, height, ctx, data, options } = this;
    const drawWidth = width - padding * 2;
    const drawHeight = height - padding * 2;

    const barWidth = (drawWidth / data.length) * 0.8;
    const barGap = (drawWidth / data.length) * 0.2;

    data.forEach((value, i) => {
      const x = padding + i * (barWidth + barGap);
      const normalizedValue = (value - minVal) / range;
      const barHeight = normalizedValue * drawHeight;
      const y = height - padding - barHeight;

      // Determine color based on value
      let color = options.strokeColor;
      if (options.positiveColor && options.negativeColor && options.baseline !== null) {
        color = value >= options.baseline ? options.positiveColor : options.negativeColor;
      }

      ctx.fillStyle = color;
      ctx.fillRect(x, y, barWidth, barHeight);
    });
  }

  /**
   * Render threshold line
   */
  renderThreshold(minVal, range, padding) {
    const { width, height, ctx, options } = this;
    const drawHeight = height - padding * 2;

    const normalizedThreshold = (options.threshold - minVal) / range;
    const y = height - padding - normalizedThreshold * drawHeight;

    ctx.beginPath();
    ctx.strokeStyle = options.thresholdColor;
    ctx.lineWidth = 1;
    ctx.setLineDash([3, 3]);
    ctx.moveTo(padding, y);
    ctx.lineTo(width - padding, y);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  /**
   * Calculate point coordinates
   */
  calculatePoints(data, minVal, range, drawWidth, drawHeight, padding) {
    return data.map((value, i) => {
      const x = padding + (i / (data.length - 1 || 1)) * drawWidth;
      const normalizedValue = (value - minVal) / range;
      const y = this.height - padding - normalizedValue * drawHeight;
      return { x, y, value };
    });
  }

  /**
   * Destroy the sparkline
   */
  destroy() {
    if (this.animationFrame) {
      cancelAnimationFrame(this.animationFrame);
    }
    if (this.canvas && this.canvas.parentNode) {
      this.canvas.parentNode.removeChild(this.canvas);
    }
    this.canvas = null;
    this.ctx = null;
    this.data = [];
  }
}

/**
 * SparklineManager - manages multiple sparklines
 */
export class SparklineManager {
  constructor(container, optionsMap = {}) {
    this.container = container;
    this.optionsMap = optionsMap;
    this.sparklines = new Map();

    this.init();
  }

  /**
   * Initialize sparklines from data attributes
   */
  init() {
    const elements = this.container.querySelectorAll('[data-sparkline]');

    elements.forEach((el) => {
      const name = el.dataset.sparkline;
      const containerEl = el.querySelector('.sparkline-container');

      if (containerEl) {
        const options = this.optionsMap[name] || {};
        const sparkline = new Sparkline(containerEl, options);
        this.sparklines.set(name, sparkline);
      }
    });
  }

  /**
   * Get a sparkline by name
   */
  getSparkline(name) {
    return this.sparklines.get(name) || null;
  }

  /**
   * Add a point to a specific sparkline
   */
  addPoint(name, value) {
    const sparkline = this.sparklines.get(name);
    if (sparkline) {
      sparkline.addPoint(value);
    }
  }

  /**
   * Update all sparklines with new values
   */
  updateAll(values) {
    Object.entries(values).forEach(([name, value]) => {
      this.addPoint(name, value);
    });
  }

  /**
   * Clear all sparklines
   */
  clearAll() {
    this.sparklines.forEach((sparkline) => sparkline.clear());
  }

  /**
   * Destroy all sparklines
   */
  destroy() {
    this.sparklines.forEach((sparkline) => sparkline.destroy());
    this.sparklines.clear();
  }
}

export default Sparkline;

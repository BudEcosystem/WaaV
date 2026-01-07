/**
 * Tests for Sparkline Module
 */

import { Sparkline, SparklineManager } from '../js/sparkline.js';

describe('Sparkline', () => {
  let container;

  beforeEach(() => {
    container = document.createElement('div');
    container.style.width = '100px';
    container.style.height = '30px';
    document.body.appendChild(container);
  });

  afterEach(() => {
    document.body.removeChild(container);
  });

  describe('creation', () => {
    test('should create canvas element', () => {
      const sparkline = new Sparkline(container);
      expect(container.querySelector('canvas')).toBeTruthy();
    });

    test('should set default options', () => {
      const sparkline = new Sparkline(container);
      expect(sparkline.options.strokeColor).toBe('#6366f1');
      expect(sparkline.options.fillColor).toBe('rgba(99, 102, 241, 0.1)');
      expect(sparkline.options.lineWidth).toBe(1.5);
    });

    test('should accept custom options', () => {
      const sparkline = new Sparkline(container, {
        strokeColor: '#ff0000',
        lineWidth: 2,
      });
      expect(sparkline.options.strokeColor).toBe('#ff0000');
      expect(sparkline.options.lineWidth).toBe(2);
    });

    test('should set canvas dimensions', () => {
      const sparkline = new Sparkline(container);
      const canvas = container.querySelector('canvas');
      expect(canvas.width).toBeGreaterThan(0);
      expect(canvas.height).toBeGreaterThan(0);
    });
  });

  describe('data management', () => {
    test('should add data points', () => {
      const sparkline = new Sparkline(container);
      sparkline.addPoint(10);
      sparkline.addPoint(20);
      expect(sparkline.data.length).toBe(2);
    });

    test('should respect maxPoints limit', () => {
      const sparkline = new Sparkline(container, { maxPoints: 5 });
      for (let i = 0; i < 10; i++) {
        sparkline.addPoint(i);
      }
      expect(sparkline.data.length).toBe(5);
    });

    test('should set data array', () => {
      const sparkline = new Sparkline(container);
      sparkline.setData([1, 2, 3, 4, 5]);
      expect(sparkline.data).toEqual([1, 2, 3, 4, 5]);
    });

    test('should clear data', () => {
      const sparkline = new Sparkline(container);
      sparkline.setData([1, 2, 3]);
      sparkline.clear();
      expect(sparkline.data.length).toBe(0);
    });

    test('should get min value', () => {
      const sparkline = new Sparkline(container);
      sparkline.setData([5, 2, 8, 1, 9]);
      expect(sparkline.getMin()).toBe(1);
    });

    test('should get max value', () => {
      const sparkline = new Sparkline(container);
      sparkline.setData([5, 2, 8, 1, 9]);
      expect(sparkline.getMax()).toBe(9);
    });

    test('should get average value', () => {
      const sparkline = new Sparkline(container);
      sparkline.setData([10, 20, 30]);
      expect(sparkline.getAverage()).toBe(20);
    });

    test('should get last value', () => {
      const sparkline = new Sparkline(container);
      sparkline.setData([1, 2, 3, 4, 5]);
      expect(sparkline.getLast()).toBe(5);
    });

    test('should return null for empty data', () => {
      const sparkline = new Sparkline(container);
      expect(sparkline.getMin()).toBeNull();
      expect(sparkline.getMax()).toBeNull();
      expect(sparkline.getAverage()).toBeNull();
      expect(sparkline.getLast()).toBeNull();
    });
  });

  describe('rendering', () => {
    test('should render without errors with empty data', () => {
      const sparkline = new Sparkline(container);
      expect(() => sparkline.render()).not.toThrow();
    });

    test('should render with single point', () => {
      const sparkline = new Sparkline(container);
      sparkline.addPoint(10);
      expect(() => sparkline.render()).not.toThrow();
    });

    test('should render with multiple points', () => {
      const sparkline = new Sparkline(container);
      sparkline.setData([1, 2, 3, 4, 5]);
      expect(() => sparkline.render()).not.toThrow();
    });

    test('should auto-render when adding points with autoRender enabled', () => {
      const sparkline = new Sparkline(container, { autoRender: true });
      let renderCalled = false;
      const originalRender = sparkline.render.bind(sparkline);
      sparkline.render = () => {
        renderCalled = true;
        originalRender();
      };
      sparkline.addPoint(10);
      expect(renderCalled).toBe(true);
    });
  });

  describe('chart types', () => {
    test('should render line chart', () => {
      const sparkline = new Sparkline(container, { type: 'line' });
      sparkline.setData([1, 2, 3]);
      expect(() => sparkline.render()).not.toThrow();
    });

    test('should render area chart', () => {
      const sparkline = new Sparkline(container, { type: 'area' });
      sparkline.setData([1, 2, 3]);
      expect(() => sparkline.render()).not.toThrow();
    });

    test('should render bar chart', () => {
      const sparkline = new Sparkline(container, { type: 'bar' });
      sparkline.setData([1, 2, 3]);
      expect(() => sparkline.render()).not.toThrow();
    });
  });

  describe('threshold', () => {
    test('should support threshold line', () => {
      const sparkline = new Sparkline(container, {
        threshold: 50,
        thresholdColor: '#ff0000',
      });
      sparkline.setData([30, 60, 40, 70]);
      expect(() => sparkline.render()).not.toThrow();
    });
  });

  describe('resize', () => {
    test('should handle resize', () => {
      const sparkline = new Sparkline(container);
      container.style.width = '200px';
      expect(() => sparkline.resize()).not.toThrow();
    });
  });

  describe('destroy', () => {
    test('should remove canvas on destroy', () => {
      const sparkline = new Sparkline(container);
      sparkline.destroy();
      expect(container.querySelector('canvas')).toBeNull();
    });
  });
});

describe('SparklineManager', () => {
  let container;

  beforeEach(() => {
    container = document.createElement('div');
    container.innerHTML = `
      <div class="metric" data-sparkline="latency">
        <span class="metric-value">100ms</span>
        <div class="sparkline-container"></div>
      </div>
      <div class="metric" data-sparkline="throughput">
        <span class="metric-value">50 req/s</span>
        <div class="sparkline-container"></div>
      </div>
    `;
    document.body.appendChild(container);
  });

  afterEach(() => {
    document.body.removeChild(container);
  });

  describe('initialization', () => {
    test('should create sparklines for elements with data-sparkline', () => {
      const manager = new SparklineManager(container);
      expect(manager.sparklines.size).toBe(2);
    });

    test('should accept custom options per sparkline', () => {
      const manager = new SparklineManager(container, {
        latency: { strokeColor: '#ff0000' },
        throughput: { strokeColor: '#00ff00' },
      });
      expect(manager.sparklines.get('latency').options.strokeColor).toBe('#ff0000');
      expect(manager.sparklines.get('throughput').options.strokeColor).toBe('#00ff00');
    });
  });

  describe('data updates', () => {
    test('should update specific sparkline', () => {
      const manager = new SparklineManager(container);
      manager.addPoint('latency', 100);
      manager.addPoint('latency', 150);
      expect(manager.sparklines.get('latency').data.length).toBe(2);
    });

    test('should update all sparklines', () => {
      const manager = new SparklineManager(container);
      manager.updateAll({
        latency: 100,
        throughput: 50,
      });
      expect(manager.sparklines.get('latency').data.length).toBe(1);
      expect(manager.sparklines.get('throughput').data.length).toBe(1);
    });
  });

  describe('getSparkline', () => {
    test('should get sparkline by name', () => {
      const manager = new SparklineManager(container);
      const sparkline = manager.getSparkline('latency');
      expect(sparkline).toBeTruthy();
    });

    test('should return null for non-existent sparkline', () => {
      const manager = new SparklineManager(container);
      expect(manager.getSparkline('nonexistent')).toBeNull();
    });
  });

  describe('clear', () => {
    test('should clear all sparklines', () => {
      const manager = new SparklineManager(container);
      manager.addPoint('latency', 100);
      manager.addPoint('throughput', 50);
      manager.clearAll();
      expect(manager.sparklines.get('latency').data.length).toBe(0);
      expect(manager.sparklines.get('throughput').data.length).toBe(0);
    });
  });

  describe('destroy', () => {
    test('should destroy all sparklines', () => {
      const manager = new SparklineManager(container);
      manager.destroy();
      expect(manager.sparklines.size).toBe(0);
    });
  });
});

describe('Sparkline color schemes', () => {
  let container;

  beforeEach(() => {
    container = document.createElement('div');
    container.style.width = '100px';
    container.style.height = '30px';
    document.body.appendChild(container);
  });

  afterEach(() => {
    document.body.removeChild(container);
  });

  test('should support gradient color', () => {
    const sparkline = new Sparkline(container, {
      gradient: ['#ff0000', '#00ff00'],
    });
    sparkline.setData([1, 2, 3, 4, 5]);
    expect(() => sparkline.render()).not.toThrow();
  });

  test('should support positive/negative colors', () => {
    const sparkline = new Sparkline(container, {
      positiveColor: '#00ff00',
      negativeColor: '#ff0000',
      baseline: 0,
    });
    sparkline.setData([-2, -1, 0, 1, 2]);
    expect(() => sparkline.render()).not.toThrow();
  });
});

describe('Sparkline animation', () => {
  let container;

  beforeEach(() => {
    container = document.createElement('div');
    container.style.width = '100px';
    container.style.height = '30px';
    document.body.appendChild(container);
  });

  afterEach(() => {
    document.body.removeChild(container);
  });

  test('should support animation option', () => {
    const sparkline = new Sparkline(container, {
      animated: true,
      animationDuration: 100,
    });
    sparkline.setData([1, 2, 3, 4, 5]);
    expect(() => sparkline.render()).not.toThrow();
  });
});

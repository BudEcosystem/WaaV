/**
 * Metrics Collector for Dashboard
 */

export class MetricsCollector {
  constructor(maxSamples = 100) {
    this.maxSamples = maxSamples;
    this.sttTtft = [];
    this.ttsTtfb = [];
    this.e2e = [];
    this.wsConnect = null;
    this.startTime = Date.now();
  }

  recordSTTTtft(ms) {
    this._addSample(this.sttTtft, ms);
  }

  recordTTSTtfb(ms) {
    this._addSample(this.ttsTtfb, ms);
  }

  recordE2E(ms) {
    this._addSample(this.e2e, ms);
  }

  recordWSConnect(ms) {
    this.wsConnect = ms;
  }

  _addSample(array, value) {
    array.push({
      value,
      timestamp: Date.now(),
    });

    // Limit samples
    if (array.length > this.maxSamples) {
      array.shift();
    }
  }

  _calculatePercentiles(array) {
    if (array.length === 0) {
      return { p50: null, p95: null, p99: null, min: null, max: null, mean: null, last: null };
    }

    const values = array.map((s) => s.value).sort((a, b) => a - b);
    const n = values.length;

    const percentile = (p) => values[Math.min(Math.floor(p * n), n - 1)];

    return {
      p50: percentile(0.5),
      p95: percentile(0.95),
      p99: percentile(0.99),
      min: values[0],
      max: values[n - 1],
      mean: values.reduce((a, b) => a + b, 0) / n,
      last: array[array.length - 1].value,
    };
  }

  getSummary() {
    return {
      sttTtft: this._calculatePercentiles(this.sttTtft),
      ttsTtfb: this._calculatePercentiles(this.ttsTtfb),
      e2e: this._calculatePercentiles(this.e2e),
      wsConnect: this.wsConnect,
      sampleCounts: {
        sttTtft: this.sttTtft.length,
        ttsTtfb: this.ttsTtfb.length,
        e2e: this.e2e.length,
      },
      collectionDuration: Date.now() - this.startTime,
    };
  }

  reset() {
    this.sttTtft = [];
    this.ttsTtfb = [];
    this.e2e = [];
    this.wsConnect = null;
    this.startTime = Date.now();
  }

  export() {
    const lines = ['timestamp,type,value'];

    this.sttTtft.forEach((s) => {
      lines.push(`${s.timestamp},stt_ttft,${s.value}`);
    });

    this.ttsTtfb.forEach((s) => {
      lines.push(`${s.timestamp},tts_ttfb,${s.value}`);
    });

    this.e2e.forEach((s) => {
      lines.push(`${s.timestamp},e2e,${s.value}`);
    });

    return lines.join('\n');
  }

  getTimeSeries(type) {
    switch (type) {
      case 'stt_ttft':
        return this.sttTtft;
      case 'tts_ttfb':
        return this.ttsTtfb;
      case 'e2e':
        return this.e2e;
      default:
        return [];
    }
  }
}

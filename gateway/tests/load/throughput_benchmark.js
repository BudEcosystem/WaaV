/**
 * k6 Pure Throughput Benchmark
 *
 * Measures gateway-added latency by testing the health endpoint
 * with no artificial delays to find maximum RPS and latency percentiles.
 *
 * Run: k6 run --vus 50 --duration 30s tests/load/throughput_benchmark.js
 */

import http from 'k6/http';
import { check } from 'k6';
import { Trend, Counter, Rate } from 'k6/metrics';

// Custom metrics for detailed analysis
const latencyP50 = new Trend('latency_p50', true);
const latencyP90 = new Trend('latency_p90', true);
const latencyP99 = new Trend('latency_p99', true);
const throughput = new Counter('throughput_rps');
const errorRate = new Rate('error_rate');

const BASE_URL = __ENV.BASE_URL || 'http://localhost:3001';

export let options = {
    // Default settings - override with CLI flags
    vus: __ENV.VUS ? parseInt(__ENV.VUS) : 50,
    duration: __ENV.DURATION || '30s',

    thresholds: {
        // TensorZero-level targets
        http_req_duration: ['p(50)<1', 'p(90)<2', 'p(99)<5'],  // Sub-millisecond targets
        error_rate: ['rate<0.001'],  // 0.1% error rate
    },

    // Disable connection reuse to test full connection overhead
    // batch: 1,
    // batchPerHost: 1,
};

export default function() {
    const res = http.get(`${BASE_URL}/`);

    const success = check(res, {
        'status is 200': (r) => r.status === 200,
    });

    latencyP50.add(res.timings.duration);
    latencyP90.add(res.timings.duration);
    latencyP99.add(res.timings.duration);
    throughput.add(1);
    errorRate.add(!success);
}

export function handleSummary(data) {
    const metrics = data.metrics;
    const duration = metrics.http_req_duration?.values || {};
    const reqs = metrics.http_reqs?.values || {};
    const errors = metrics.error_rate?.values || {};

    const rps = reqs.rate?.toFixed(2) || 'N/A';
    const totalReqs = reqs.count || 0;
    const p50 = duration['p(50)']?.toFixed(3) || 'N/A';
    const p90 = duration['p(90)']?.toFixed(3) || 'N/A';
    const p99 = duration['p(99)']?.toFixed(3) || 'N/A';
    const max = duration.max?.toFixed(3) || 'N/A';
    const avg = duration.avg?.toFixed(3) || 'N/A';
    const errRate = ((errors.rate || 0) * 100).toFixed(4);

    const vus = __ENV.VUS || options.vus;

    let summary = `
================================================================================
 THROUGHPUT BENCHMARK RESULTS
================================================================================

 Configuration:
   VUs: ${vus}
   Duration: ${options.duration}
   Target: ${BASE_URL}/

 Throughput:
   Requests/sec: ${rps}
   Total Requests: ${totalReqs}

 Latency (ms):
   P50:  ${p50}
   P90:  ${p90}
   P99:  ${p99}
   Max:  ${max}
   Avg:  ${avg}

 Errors:
   Error Rate: ${errRate}%

 TensorZero Comparison (Target):
   P50 Target:  <0.5ms  | Actual: ${p50}ms ${parseFloat(p50) < 0.5 ? '✓' : '✗'}
   P90 Target:  <1.0ms  | Actual: ${p90}ms ${parseFloat(p90) < 1.0 ? '✓' : '✗'}
   P99 Target:  <2.0ms  | Actual: ${p99}ms ${parseFloat(p99) < 2.0 ? '✓' : '✗'}
   RPS Target:  >10000  | Actual: ${rps} ${parseFloat(rps) > 10000 ? '✓' : '✗'}

================================================================================
`;

    // JSON output for programmatic analysis
    const jsonResult = {
        vus: parseInt(vus),
        duration: options.duration,
        rps: parseFloat(rps),
        total_requests: totalReqs,
        latency: {
            p50: parseFloat(p50),
            p90: parseFloat(p90),
            p99: parseFloat(p99),
            max: parseFloat(max),
            avg: parseFloat(avg),
        },
        error_rate: parseFloat(errRate),
        meets_targets: {
            p50: parseFloat(p50) < 0.5,
            p90: parseFloat(p90) < 1.0,
            p99: parseFloat(p99) < 2.0,
            rps: parseFloat(rps) > 10000,
        }
    };

    return {
        'stdout': summary,
        [`/tmp/benchmark_${vus}vus.json`]: JSON.stringify(jsonResult, null, 2),
    };
}

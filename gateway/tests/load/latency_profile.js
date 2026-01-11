/**
 * k6 Latency Profile Test
 *
 * Measures gateway latency with proper P50, P90, P99, P99.9 percentiles.
 * Run: k6 run --vus 50 --duration 30s tests/load/latency_profile.js
 */

import http from 'k6/http';
import { check } from 'k6';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:3001';

export let options = {
    vus: __ENV.VUS ? parseInt(__ENV.VUS) : 50,
    duration: __ENV.DURATION || '30s',
    summaryTrendStats: ['p(50)', 'p(90)', 'p(95)', 'p(99)', 'p(99.9)', 'avg', 'max'],
    thresholds: {
        http_req_duration: ['p(50)<0.5', 'p(90)<1', 'p(99)<2'],
    },
};

export default function() {
    const res = http.get(`${BASE_URL}/`);
    check(res, { 'status is 200': (r) => r.status === 200 });
}

export function handleSummary(data) {
    const metrics = data.metrics || {};
    const duration = metrics.http_req_duration?.values || {};
    const reqs = metrics.http_reqs?.values || {};

    const result = {
        timestamp: new Date().toISOString(),
        vus: options.vus,
        duration: options.duration,
        throughput: {
            rps: (reqs.rate || 0).toFixed(2),
            total_requests: reqs.count || 0,
        },
        latency_ms: {
            p50: duration['p(50)']?.toFixed(3) || 'N/A',
            p90: duration['p(90)']?.toFixed(3) || 'N/A',
            p95: duration['p(95)']?.toFixed(3) || 'N/A',
            p99: duration['p(99)']?.toFixed(3) || 'N/A',
            p999: duration['p(99.9)']?.toFixed(3) || 'N/A',
            avg: duration.avg?.toFixed(3) || 'N/A',
            max: duration.max?.toFixed(3) || 'N/A',
        },
    };

    const summary = `
================================================================================
 LATENCY PROFILE RESULTS - ${options.vus} VUs
================================================================================

 Throughput: ${result.throughput.rps} req/s (${result.throughput.total_requests} total)

 Latency Distribution:
   P50:   ${result.latency_ms.p50}ms
   P90:   ${result.latency_ms.p90}ms
   P95:   ${result.latency_ms.p95}ms
   P99:   ${result.latency_ms.p99}ms
   P99.9: ${result.latency_ms.p999}ms
   Avg:   ${result.latency_ms.avg}ms
   Max:   ${result.latency_ms.max}ms

 TensorZero Targets:
   P50 < 0.5ms:  ${parseFloat(result.latency_ms.p50) < 0.5 ? '✓ PASS' : '✗ FAIL'}
   P90 < 1.0ms:  ${parseFloat(result.latency_ms.p90) < 1.0 ? '✓ PASS' : '✗ FAIL'}
   P99 < 2.0ms:  ${parseFloat(result.latency_ms.p99) < 2.0 ? '✓ PASS' : '✗ FAIL'}
   RPS > 10000:  ${parseFloat(result.throughput.rps) > 10000 ? '✓ PASS' : '✗ FAIL'}

================================================================================
`;

    return {
        'stdout': summary,
        [`/tmp/latency_${options.vus}vus.json`]: JSON.stringify(result, null, 2),
    };
}

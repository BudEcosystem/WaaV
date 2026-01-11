/**
 * k6 REST API Throughput Load Test
 *
 * Tests the WaaV Gateway REST endpoints under load:
 * - Health check endpoint (/)
 * - Voices endpoint (/voices)
 * - Speak endpoint (/speak)
 *
 * Run: k6 run tests/load/rest_throughput.js
 * Run with env: k6 run -e BASE_URL=http://localhost:3001 tests/load/rest_throughput.js
 */

import http from 'k6/http';
import { check, sleep, group } from 'k6';
import { Counter, Rate, Trend } from 'k6/metrics';

// Custom metrics
const healthCheckDuration = new Trend('health_check_duration', true);
const voicesEndpointDuration = new Trend('voices_endpoint_duration', true);
const speakEndpointDuration = new Trend('speak_endpoint_duration', true);
const errorRate = new Rate('error_rate');
const requestCount = new Counter('request_count');

// Test configuration
export let options = {
    scenarios: {
        // Ramp-up load test
        ramp_up: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '30s', target: 50 },   // Ramp to 50 VUs
                { duration: '1m', target: 100 },   // Ramp to 100 VUs
                { duration: '2m', target: 100 },   // Sustain 100 VUs
                { duration: '30s', target: 200 },  // Spike to 200 VUs
                { duration: '1m', target: 200 },   // Sustain spike
                { duration: '30s', target: 0 },    // Ramp down
            ],
            gracefulRampDown: '30s',
        },
    },
    thresholds: {
        // Response time thresholds
        http_req_duration: ['p(95)<500', 'p(99)<1000'],
        health_check_duration: ['p(95)<100', 'p(99)<200'],
        voices_endpoint_duration: ['p(95)<300', 'p(99)<500'],
        speak_endpoint_duration: ['p(95)<1000', 'p(99)<2000'],

        // Error rate threshold
        error_rate: ['rate<0.01'],  // Less than 1% errors

        // Request count (minimum throughput)
        request_count: ['count>1000'],
    },
};

const BASE_URL = __ENV.BASE_URL || 'http://localhost:3001';

// Test data
const speakPayload = JSON.stringify({
    text: 'Hello, this is a load test for the WaaV Gateway.',
    voice_id: 'test-voice',
    provider: 'elevenlabs',
});

const headers = {
    'Content-Type': 'application/json',
};

export default function() {
    group('Health Check', function() {
        const start = Date.now();
        const res = http.get(`${BASE_URL}/`);
        healthCheckDuration.add(Date.now() - start);

        const success = check(res, {
            'health check status is 200': (r) => r.status === 200,
            'health check has status field': (r) => {
                try {
                    const body = JSON.parse(r.body);
                    return body.status !== undefined;
                } catch {
                    return false;
                }
            },
        });

        errorRate.add(!success);
        requestCount.add(1);
    });

    sleep(0.1);

    group('Voices Endpoint', function() {
        const start = Date.now();
        const res = http.get(`${BASE_URL}/voices`);
        voicesEndpointDuration.add(Date.now() - start);

        const success = check(res, {
            'voices status is 200 or 400': (r) => r.status === 200 || r.status === 400,
            'voices response is valid JSON': (r) => {
                try {
                    JSON.parse(r.body);
                    return true;
                } catch {
                    return false;
                }
            },
        });

        errorRate.add(!success);
        requestCount.add(1);
    });

    sleep(0.1);

    group('Speak Endpoint', function() {
        const start = Date.now();
        const res = http.post(`${BASE_URL}/speak`, speakPayload, { headers });
        speakEndpointDuration.add(Date.now() - start);

        const success = check(res, {
            // Speak endpoint may fail without valid API keys, but should respond
            'speak endpoint responds': (r) => r.status !== 0,
            'speak status is not 5xx': (r) => r.status < 500,
        });

        errorRate.add(!success);
        requestCount.add(1);
    });

    sleep(0.2);
}

export function handleSummary(data) {
    return {
        'stdout': textSummary(data, { indent: ' ', enableColors: true }),
        'tests/load/rest_throughput_summary.json': JSON.stringify(data, null, 2),
    };
}

function textSummary(data, options) {
    const indent = options.indent || '  ';
    const enableColors = options.enableColors !== false;

    const green = enableColors ? '\x1b[32m' : '';
    const red = enableColors ? '\x1b[31m' : '';
    const yellow = enableColors ? '\x1b[33m' : '';
    const reset = enableColors ? '\x1b[0m' : '';

    let summary = '\n';
    summary += '='.repeat(60) + '\n';
    summary += `${indent}REST THROUGHPUT LOAD TEST SUMMARY\n`;
    summary += '='.repeat(60) + '\n\n';

    // Metrics summary
    if (data.metrics) {
        summary += `${indent}HTTP Request Duration:\n`;
        if (data.metrics.http_req_duration) {
            const m = data.metrics.http_req_duration.values;
            summary += `${indent}${indent}avg: ${m.avg?.toFixed(2) || 'N/A'}ms\n`;
            summary += `${indent}${indent}p95: ${m['p(95)']?.toFixed(2) || 'N/A'}ms\n`;
            summary += `${indent}${indent}p99: ${m['p(99)']?.toFixed(2) || 'N/A'}ms\n`;
        }

        summary += `\n${indent}Custom Metrics:\n`;

        if (data.metrics.health_check_duration) {
            const m = data.metrics.health_check_duration.values;
            summary += `${indent}${indent}Health Check p95: ${m['p(95)']?.toFixed(2) || 'N/A'}ms\n`;
        }

        if (data.metrics.voices_endpoint_duration) {
            const m = data.metrics.voices_endpoint_duration.values;
            summary += `${indent}${indent}Voices Endpoint p95: ${m['p(95)']?.toFixed(2) || 'N/A'}ms\n`;
        }

        if (data.metrics.speak_endpoint_duration) {
            const m = data.metrics.speak_endpoint_duration.values;
            summary += `${indent}${indent}Speak Endpoint p95: ${m['p(95)']?.toFixed(2) || 'N/A'}ms\n`;
        }

        if (data.metrics.error_rate) {
            const rate = data.metrics.error_rate.values.rate;
            const color = rate < 0.01 ? green : red;
            summary += `${indent}${indent}Error Rate: ${color}${(rate * 100).toFixed(2)}%${reset}\n`;
        }

        if (data.metrics.request_count) {
            summary += `${indent}${indent}Total Requests: ${data.metrics.request_count.values.count}\n`;
        }
    }

    summary += '\n' + '='.repeat(60) + '\n';

    return summary;
}

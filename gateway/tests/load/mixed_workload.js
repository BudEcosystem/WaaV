/**
 * k6 Mixed Workload Load Test
 *
 * Simulates realistic traffic patterns with:
 * - 70% REST API calls
 * - 30% WebSocket connections
 *
 * Run: k6 run tests/load/mixed_workload.js
 * Run with env: k6 run -e HTTP_URL=http://localhost:3001 -e WS_URL=ws://localhost:3001 tests/load/mixed_workload.js
 */

import http from 'k6/http';
import ws from 'k6/ws';
import { check, sleep, group } from 'k6';
import { Counter, Rate, Trend } from 'k6/metrics';
import { SharedArray } from 'k6/data';

// Custom metrics
const restLatency = new Trend('rest_latency', true);
const wsLatency = new Trend('ws_latency', true);
const totalRequests = new Counter('total_requests');
const restRequests = new Counter('rest_requests');
const wsConnections = new Counter('ws_connections');
const errorRate = new Rate('error_rate');

// Test configuration
export let options = {
    scenarios: {
        // Mixed traffic simulation
        mixed_traffic: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '30s', target: 30 },   // Ramp up
                { duration: '1m', target: 60 },    // Medium load
                { duration: '2m', target: 100 },   // High load
                { duration: '1m', target: 100 },   // Sustain
                { duration: '30s', target: 0 },    // Ramp down
            ],
            gracefulRampDown: '30s',
        },
    },
    thresholds: {
        // Overall latency
        http_req_duration: ['p(95)<500'],
        rest_latency: ['p(95)<300'],
        ws_latency: ['p(95)<1000'],

        // Error rate
        error_rate: ['rate<0.02'],

        // Minimum throughput
        total_requests: ['count>500'],
    },
};

const HTTP_URL = __ENV.HTTP_URL || 'http://localhost:3001';
const WS_URL = __ENV.WS_URL || 'ws://localhost:3001';

const headers = {
    'Content-Type': 'application/json',
};

// Sample text payloads for speak endpoint
const textSamples = [
    'Hello, this is a test message.',
    'The quick brown fox jumps over the lazy dog.',
    'Testing the voice gateway with various text lengths.',
    'Real-time audio processing requires low latency systems.',
    'Artificial intelligence is transforming voice technology.',
];

function getRandomText() {
    return textSamples[Math.floor(Math.random() * textSamples.length)];
}

function doRestRequests() {
    group('REST API Calls', function() {
        // Health check (most frequent)
        const healthStart = Date.now();
        const healthRes = http.get(`${HTTP_URL}/`);
        restLatency.add(Date.now() - healthStart);
        restRequests.add(1);
        totalRequests.add(1);

        const healthSuccess = check(healthRes, {
            'health status 200': (r) => r.status === 200,
        });
        errorRate.add(!healthSuccess);

        sleep(0.05);

        // Voices endpoint (less frequent)
        if (Math.random() < 0.3) {
            const voicesStart = Date.now();
            const voicesRes = http.get(`${HTTP_URL}/voices`);
            restLatency.add(Date.now() - voicesStart);
            restRequests.add(1);
            totalRequests.add(1);

            const voicesSuccess = check(voicesRes, {
                'voices responds': (r) => r.status !== 0,
            });
            errorRate.add(!voicesSuccess);
        }

        sleep(0.05);

        // Speak endpoint (occasional)
        if (Math.random() < 0.2) {
            const speakPayload = JSON.stringify({
                text: getRandomText(),
                voice_id: 'test-voice',
                provider: 'elevenlabs',
            });

            const speakStart = Date.now();
            const speakRes = http.post(`${HTTP_URL}/speak`, speakPayload, { headers });
            restLatency.add(Date.now() - speakStart);
            restRequests.add(1);
            totalRequests.add(1);

            const speakSuccess = check(speakRes, {
                'speak responds': (r) => r.status !== 0,
                'speak not 5xx': (r) => r.status < 500,
            });
            errorRate.add(!speakSuccess);
        }
    });
}

function doWebSocketSession() {
    group('WebSocket Session', function() {
        const url = `${WS_URL}/ws`;
        const wsStart = Date.now();

        const res = ws.connect(url, {}, function(socket) {
            wsLatency.add(Date.now() - wsStart);
            wsConnections.add(1);
            totalRequests.add(1);

            socket.on('open', () => {
                // Send config
                socket.send(JSON.stringify({
                    type: 'config',
                    stt_config: {
                        provider: 'deepgram',
                        language: 'en-US',
                    },
                    tts_config: {
                        provider: 'elevenlabs',
                        voice_id: 'test-voice',
                    },
                }));
            });

            socket.on('message', (msg) => {
                try {
                    const data = JSON.parse(msg);
                    if (data.type === 'ready') {
                        // Send a few audio chunks
                        for (let i = 0; i < 5; i++) {
                            socket.setTimeout(() => {
                                if (socket.readyState === 1) {
                                    socket.sendBinary(new ArrayBuffer(1600));  // 50ms chunk
                                }
                            }, i * 50);
                        }
                    }
                } catch (e) {
                    // Binary or parse error
                }
            });

            socket.on('error', () => {
                errorRate.add(1);
            });

            // Close after brief session
            socket.setTimeout(() => {
                socket.close();
            }, 2000);
        });

        const success = check(res, {
            'WebSocket connected': (r) => r && r.status === 101,
        });

        if (!success) {
            errorRate.add(1);
        }
    });
}

export default function() {
    // 70% REST, 30% WebSocket
    const isWebSocket = Math.random() < 0.3;

    if (isWebSocket) {
        doWebSocketSession();
        sleep(2);  // WebSocket sessions take longer
    } else {
        doRestRequests();
        sleep(0.2);
    }
}

export function handleSummary(data) {
    return {
        'stdout': textSummary(data, { indent: ' ', enableColors: true }),
        'tests/load/mixed_workload_summary.json': JSON.stringify(data, null, 2),
    };
}

function textSummary(data, options) {
    const indent = options.indent || '  ';
    const enableColors = options.enableColors !== false;

    const green = enableColors ? '\x1b[32m' : '';
    const red = enableColors ? '\x1b[31m' : '';
    const reset = enableColors ? '\x1b[0m' : '';

    let summary = '\n';
    summary += '='.repeat(60) + '\n';
    summary += `${indent}MIXED WORKLOAD LOAD TEST SUMMARY\n`;
    summary += '='.repeat(60) + '\n\n';

    if (data.metrics) {
        summary += `${indent}Latency Metrics:\n`;

        if (data.metrics.rest_latency) {
            const m = data.metrics.rest_latency.values;
            summary += `${indent}${indent}REST Latency p95: ${m['p(95)']?.toFixed(2) || 'N/A'}ms\n`;
        }

        if (data.metrics.ws_latency) {
            const m = data.metrics.ws_latency.values;
            summary += `${indent}${indent}WebSocket Latency p95: ${m['p(95)']?.toFixed(2) || 'N/A'}ms\n`;
        }

        summary += `\n${indent}Traffic Distribution:\n`;

        if (data.metrics.rest_requests) {
            summary += `${indent}${indent}REST Requests: ${data.metrics.rest_requests.values.count}\n`;
        }

        if (data.metrics.ws_connections) {
            summary += `${indent}${indent}WebSocket Connections: ${data.metrics.ws_connections.values.count}\n`;
        }

        if (data.metrics.total_requests) {
            summary += `${indent}${indent}Total Operations: ${data.metrics.total_requests.values.count}\n`;
        }

        if (data.metrics.error_rate) {
            const rate = data.metrics.error_rate.values.rate;
            const color = rate < 0.02 ? green : red;
            summary += `${indent}${indent}Error Rate: ${color}${(rate * 100).toFixed(2)}%${reset}\n`;
        }
    }

    summary += '\n' + '='.repeat(60) + '\n';

    return summary;
}

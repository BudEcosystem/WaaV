/**
 * k6 WebSocket Load Test
 *
 * Tests the WaaV Gateway WebSocket endpoints under load:
 * - Connection establishment
 * - Config message handling
 * - Audio streaming simulation
 * - Message throughput
 *
 * Run: k6 run tests/load/websocket_load.js
 * Run with env: k6 run -e BASE_URL=ws://localhost:3001 tests/load/websocket_load.js
 */

import ws from 'k6/ws';
import { check, sleep } from 'k6';
import { Counter, Rate, Trend } from 'k6/metrics';

// Custom metrics
const wsConnectDuration = new Trend('ws_connect_duration', true);
const wsConfigDuration = new Trend('ws_config_duration', true);
const wsMessageLatency = new Trend('ws_message_latency', true);
const wsConnectionsOpened = new Counter('ws_connections_opened');
const wsConnectionsClosed = new Counter('ws_connections_closed');
const wsMessagesReceived = new Counter('ws_messages_received');
const wsMessagesSent = new Counter('ws_messages_sent');
const wsErrorRate = new Rate('ws_error_rate');

// Test configuration
export let options = {
    scenarios: {
        // Concurrent WebSocket connections
        websocket_load: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '20s', target: 25 },   // Ramp to 25 concurrent connections
                { duration: '30s', target: 50 },   // Ramp to 50 concurrent connections
                { duration: '1m', target: 100 },   // Ramp to 100 concurrent connections
                { duration: '1m', target: 100 },   // Sustain 100 connections
                { duration: '30s', target: 50 },   // Ramp down to 50
                { duration: '20s', target: 0 },    // Ramp down to 0
            ],
            gracefulRampDown: '30s',
        },
    },
    thresholds: {
        // WebSocket specific thresholds
        ws_connect_duration: ['p(95)<1000', 'p(99)<2000'],
        ws_config_duration: ['p(95)<500', 'p(99)<1000'],
        ws_message_latency: ['p(95)<200', 'p(99)<500'],

        // Connection management
        ws_connections_opened: ['count>50'],

        // Error rate
        ws_error_rate: ['rate<0.05'],  // Less than 5% errors (WS is more prone to issues)
    },
};

const BASE_URL = __ENV.BASE_URL || 'ws://localhost:3001';

// Generate simulated audio chunk (silence)
function generateAudioChunk(size) {
    // 16-bit PCM silence (zeros)
    const buffer = new ArrayBuffer(size);
    return buffer;
}

// Config message for STT/TTS setup
const configMessage = JSON.stringify({
    type: 'config',
    stt_config: {
        provider: 'deepgram',
        language: 'en-US',
        sample_rate: 16000,
        channels: 1,
        encoding: 'linear16',
        punctuation: true,
        model: 'nova-3',
    },
    tts_config: {
        provider: 'deepgram',
        voice_id: 'aura-luna-en',
        sample_rate: 22050,
        audio_format: 'pcm',
        model: '',  // Empty string for default model
        speaking_rate: 1.0,
        connection_timeout: 30,
        request_timeout: 60,
    },
});

export default function() {
    const url = `${BASE_URL}/ws`;
    const connectStart = Date.now();

    const res = ws.connect(url, {}, function(socket) {
        const connectDuration = Date.now() - connectStart;
        wsConnectDuration.add(connectDuration);
        wsConnectionsOpened.add(1);

        let configReceived = false;
        let configStart = 0;

        socket.on('open', () => {
            // Send config message
            configStart = Date.now();
            socket.send(configMessage);
            wsMessagesSent.add(1);
        });

        socket.on('message', (msg) => {
            wsMessagesReceived.add(1);

            try {
                const data = JSON.parse(msg);

                if (data.type === 'ready' && !configReceived) {
                    configReceived = true;
                    wsConfigDuration.add(Date.now() - configStart);

                    // Start sending audio chunks
                    for (let i = 0; i < 10; i++) {
                        socket.setTimeout(() => {
                            if (socket.readyState === 1) {  // OPEN
                                const audioChunk = generateAudioChunk(3200);  // 100ms at 16kHz
                                socket.sendBinary(audioChunk);
                                wsMessagesSent.add(1);
                            }
                        }, i * 100);
                    }
                }

                if (data.type === 'transcript' || data.type === 'stt_result') {
                    // Calculate latency from audio send to transcript receive
                    wsMessageLatency.add(Date.now() - configStart);
                }

                if (data.type === 'error') {
                    wsErrorRate.add(1);
                }
            } catch (e) {
                // Binary message or parse error - not necessarily an error
            }
        });

        socket.on('error', (e) => {
            wsErrorRate.add(1);
        });

        socket.on('close', () => {
            wsConnectionsClosed.add(1);
        });

        // Keep connection open for test duration
        socket.setTimeout(() => {
            socket.close();
        }, 5000);
    });

    const success = check(res, {
        'WebSocket connected': (r) => r && r.status === 101,
    });

    if (!success) {
        wsErrorRate.add(1);
    }

    sleep(1);
}

export function handleSummary(data) {
    return {
        'stdout': textSummary(data, { indent: ' ', enableColors: true }),
        'tests/load/websocket_load_summary.json': JSON.stringify(data, null, 2),
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
    summary += `${indent}WEBSOCKET LOAD TEST SUMMARY\n`;
    summary += '='.repeat(60) + '\n\n';

    if (data.metrics) {
        summary += `${indent}Connection Metrics:\n`;

        if (data.metrics.ws_connect_duration) {
            const m = data.metrics.ws_connect_duration.values;
            summary += `${indent}${indent}Connect Duration p95: ${m['p(95)']?.toFixed(2) || 'N/A'}ms\n`;
        }

        if (data.metrics.ws_config_duration) {
            const m = data.metrics.ws_config_duration.values;
            summary += `${indent}${indent}Config Duration p95: ${m['p(95)']?.toFixed(2) || 'N/A'}ms\n`;
        }

        if (data.metrics.ws_message_latency) {
            const m = data.metrics.ws_message_latency.values;
            summary += `${indent}${indent}Message Latency p95: ${m['p(95)']?.toFixed(2) || 'N/A'}ms\n`;
        }

        summary += `\n${indent}Connection Stats:\n`;

        if (data.metrics.ws_connections_opened) {
            summary += `${indent}${indent}Connections Opened: ${data.metrics.ws_connections_opened.values.count}\n`;
        }

        if (data.metrics.ws_connections_closed) {
            summary += `${indent}${indent}Connections Closed: ${data.metrics.ws_connections_closed.values.count}\n`;
        }

        if (data.metrics.ws_messages_sent) {
            summary += `${indent}${indent}Messages Sent: ${data.metrics.ws_messages_sent.values.count}\n`;
        }

        if (data.metrics.ws_messages_received) {
            summary += `${indent}${indent}Messages Received: ${data.metrics.ws_messages_received.values.count}\n`;
        }

        if (data.metrics.ws_error_rate) {
            const rate = data.metrics.ws_error_rate.values.rate;
            const color = rate < 0.05 ? green : red;
            summary += `${indent}${indent}Error Rate: ${color}${(rate * 100).toFixed(2)}%${reset}\n`;
        }
    }

    summary += '\n' + '='.repeat(60) + '\n';

    return summary;
}

/**
 * Bud Foundry Dashboard - Main Application
 */

import { State } from './state.js';
import { WebSocketManager } from './websocket.js?v=5';
import { MetricsCollector } from './metrics.js';
import { Logger } from './logger.js';
import { AudioManager } from './audio.js';

// Initialize state
const state = new State();
const metrics = new MetricsCollector();
const logger = new Logger(document.getElementById('request-log'));

// Managers (initialized after DOM)
let wsManager = null;
let audioManager = null;

// DOM Elements
const elements = {
  // Connection
  serverUrl: document.getElementById('server-url'),
  apiKey: document.getElementById('api-key'),
  connectBtn: document.getElementById('connect-btn'),
  statusIndicator: document.getElementById('status-indicator'),
  statusText: document.getElementById('status-text'),

  // Tabs
  tabs: document.querySelectorAll('.tab'),
  tabPanels: document.querySelectorAll('.tab-panel'),

  // Quick metrics
  quickTtft: document.getElementById('quick-ttft'),
  quickTtfb: document.getElementById('quick-ttfb'),

  // Theme
  themeToggle: document.getElementById('theme-toggle'),
};

/**
 * Initialize the dashboard
 */
function init() {
  // Set default server URL based on current host
  initializeServerUrl();

  // Check for secure context (required for microphone access)
  checkSecureContext();

  // Setup tab navigation
  setupTabs();

  // Setup connection
  setupConnection();

  // Setup STT panel
  setupSTT();

  // Setup TTS panel
  setupTTS();

  // Setup LiveKit panel
  setupLiveKit();

  // Setup SIP panel
  setupSIP();

  // Setup API Explorer
  setupAPIExplorer();

  // Setup WS Debug
  setupWSDebug();

  // Setup Audio Tools
  setupAudioTools();

  // Setup Metrics panel
  setupMetrics();

  // Setup theme toggle
  setupTheme();

  console.log('Dashboard initialized');
}

/**
 * Initialize server URL based on current host
 */
function initializeServerUrl() {
  const host = window.location.hostname;
  const wsProtocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const defaultUrl = `${wsProtocol}//${host}:3001/ws`;
  elements.serverUrl.value = defaultUrl;
  console.log('Default server URL set to:', defaultUrl);
}

/**
 * Check if running in a secure context (required for getUserMedia)
 */
function checkSecureContext() {
  if (!window.isSecureContext) {
    console.warn('Not running in secure context. Microphone access will be blocked.');
    console.warn('To use microphone, either:');
    console.warn('1. Access via localhost');
    console.warn('2. Use HTTPS');
    console.warn('3. In Chrome, add this IP to chrome://flags/#unsafely-treat-insecure-origin-as-secure');
  }
}

/**
 * Tab Navigation
 */
function setupTabs() {
  elements.tabs.forEach((tab) => {
    tab.addEventListener('click', () => {
      const tabId = tab.dataset.tab;

      // Update active tab
      elements.tabs.forEach((t) => t.classList.remove('active'));
      tab.classList.add('active');

      // Show corresponding panel
      elements.tabPanels.forEach((panel) => {
        panel.classList.toggle('active', panel.id === `panel-${tabId}`);
      });
    });
  });
}

/**
 * Connection Management
 */
function setupConnection() {
  elements.connectBtn.addEventListener('click', async () => {
    if (state.connected) {
      disconnect();
    } else {
      await connect();
    }
  });
}

async function connect() {
  const url = elements.serverUrl.value;
  const apiKey = elements.apiKey.value;

  if (!url) {
    alert('Please enter a server URL');
    return;
  }

  updateConnectionStatus('connecting');

  try {
    wsManager = new WebSocketManager(url, apiKey);

    wsManager.onOpen = () => {
      state.connected = true;
      updateConnectionStatus('connected');
      logger.log('WS', 'Connected', { url });

      // Send initial config
      wsManager.sendConfig({
        stt: getSTTConfig(),
        tts: getTTSConfig(),
      });
    };

    wsManager.onMessage = (data) => {
      handleWSMessage(data);
    };

    wsManager.onClose = () => {
      state.connected = false;
      updateConnectionStatus('disconnected');
      logger.log('WS', 'Disconnected');
    };

    wsManager.onError = (error) => {
      console.error('WebSocket error:', error);
      updateConnectionStatus('error');
    };

    await wsManager.connect();
  } catch (error) {
    console.error('Connection failed:', error);
    updateConnectionStatus('error');
    const errorMsg = error?.message || error?.toString() || 'Unknown error';
    alert('Connection failed: ' + errorMsg);
  }
}

function disconnect() {
  if (wsManager) {
    wsManager.disconnect();
    wsManager = null;
  }
  state.connected = false;
  updateConnectionStatus('disconnected');
}

function updateConnectionStatus(status) {
  elements.statusIndicator.className = 'status-indicator ' + status;

  switch (status) {
    case 'connected':
      elements.statusText.textContent = 'Connected';
      elements.connectBtn.textContent = 'Disconnect';
      break;
    case 'connecting':
      elements.statusText.textContent = 'Connecting...';
      elements.connectBtn.textContent = 'Cancel';
      break;
    case 'error':
      elements.statusText.textContent = 'Error';
      elements.connectBtn.textContent = 'Reconnect';
      break;
    default:
      elements.statusText.textContent = 'Disconnected';
      elements.connectBtn.textContent = 'Connect';
  }
}

/**
 * Handle WebSocket messages
 */
function handleWSMessage(data) {
  // Log to WS debug panel
  addWSLogEntry('in', data);

  const type = data.type;

  switch (type) {
    case 'ready':
      logger.log('WS', 'Ready', { stream_id: data.stream_id });
      break;

    case 'stt_result':
      handleSTTResult(data);
      break;

    case 'tts_audio':
      handleTTSAudio(data);
      break;

    case 'tts_playback_complete':
      handleTTSComplete();
      break;

    case 'pong':
      const latency = Date.now() - data.timestamp;
      logger.log('WS', 'Pong', { latency: latency + 'ms' });
      break;

    case 'error':
      logger.log('ERROR', data.message);
      break;
  }
}

/**
 * STT Panel
 */
function setupSTT() {
  const recordBtn = document.getElementById('stt-record-btn');
  const uploadBtn = document.getElementById('stt-upload-btn');
  const fileInput = document.getElementById('stt-file-input');
  const applyConfigBtn = document.getElementById('stt-apply-config');

  recordBtn.addEventListener('click', async () => {
    if (state.recording) {
      stopRecording();
    } else {
      await startRecording();
    }
  });

  uploadBtn.addEventListener('click', () => {
    fileInput.click();
  });

  fileInput.addEventListener('change', (e) => {
    const file = e.target.files[0];
    if (file) {
      uploadAudioFile(file);
    }
  });

  applyConfigBtn.addEventListener('click', () => {
    if (!state.connected) {
      alert('Please connect first');
      return;
    }
    wsManager.sendConfig({
      stt: getSTTConfig(),
      tts: getTTSConfig(),
    });
    logger.log('STT', 'Config applied', { provider: getSTTConfig().provider });
  });
}

async function startRecording() {
  if (!state.connected) {
    alert('Please connect first');
    return;
  }

  // Check for secure context
  if (!window.isSecureContext) {
    alert(
      'Microphone access requires a secure connection.\n\n' +
      'Options to fix this:\n' +
      '1. Access via localhost on this machine\n' +
      '2. Use HTTPS (see console for setup instructions)\n' +
      '3. In Chrome: go to chrome://flags/#unsafely-treat-insecure-origin-as-secure and add http://' + window.location.hostname + ':8080'
    );
    console.log('To enable HTTPS, run the dashboard with a self-signed certificate.');
    return;
  }

  try {
    audioManager = new AudioManager({
      sampleRate: 16000,
    });

    audioManager.onData = (data) => {
      if (wsManager && state.connected) {
        wsManager.sendAudio(data);
      }
    };

    audioManager.onLevel = (level) => {
      updateVisualizerLevel(level);
    };

    await audioManager.startRecording();
    state.recording = true;

    const recordBtn = document.getElementById('stt-record-btn');
    recordBtn.innerHTML = `
      <svg class="icon" viewBox="0 0 24 24">
        <rect x="6" y="6" width="12" height="12"/>
      </svg>
      Stop Recording
    `;
    recordBtn.classList.add('recording');

    // Track TTFT timing
    state.sttStartTime = Date.now();

  } catch (error) {
    console.error('Failed to start recording:', error);
    const errorMsg = error?.message || 'Unknown error';
    alert('Failed to access microphone: ' + errorMsg);
  }
}

function stopRecording() {
  if (audioManager) {
    audioManager.stopRecording();
    audioManager = null;
  }

  state.recording = false;

  const recordBtn = document.getElementById('stt-record-btn');
  recordBtn.innerHTML = `
    <svg class="icon" viewBox="0 0 24 24">
      <path d="M12 14c1.66 0 3-1.34 3-3V5c0-1.66-1.34-3-3-3S9 3.34 9 5v6c0 1.66 1.34 3 3 3z"/>
      <path d="M17 11c0 2.76-2.24 5-5 5s-5-2.24-5-5H5c0 3.53 2.61 6.43 6 6.92V21h2v-3.08c3.39-.49 6-3.39 6-6.92h-2z"/>
    </svg>
    Start Recording
  `;
  recordBtn.classList.remove('recording');
}

function handleSTTResult(data) {
  const transcript = data.transcript || '';
  const isFinal = data.is_final;

  // Calculate TTFT on first result
  if (state.sttStartTime && !state.sttFirstResult) {
    const ttft = Date.now() - state.sttStartTime;
    metrics.recordSTTTtft(ttft);
    updateQuickMetrics();
    state.sttFirstResult = true;
    logger.log('STT', 'TTFT', { ttft: ttft + 'ms' });
  }

  const transcriptEl = document.getElementById('stt-transcript');

  if (isFinal) {
    state.transcript += transcript + ' ';
    state.interimTranscript = '';
  } else {
    state.interimTranscript = transcript;
  }

  transcriptEl.innerHTML = `
    <div class="transcript-text">${state.transcript}</div>
    ${state.interimTranscript ? `<div class="transcript-text interim">${state.interimTranscript}</div>` : ''}
  `;
}

function updateVisualizerLevel(level) {
  const visualizer = document.getElementById('stt-visualizer');
  const barCount = 20;

  let html = '';
  for (let i = 0; i < barCount; i++) {
    const height = Math.random() * level * 100;
    html += `<div class="visualizer-bar" style="height: ${Math.max(4, height)}px;"></div>`;
  }
  visualizer.innerHTML = html;
}

async function uploadAudioFile(file) {
  if (!state.connected) {
    alert('Please connect first');
    return;
  }

  logger.log('Audio', 'Processing file', { name: file.name, size: file.size });

  try {
    // Read the file as ArrayBuffer
    const arrayBuffer = await file.arrayBuffer();

    // Decode the audio
    const audioContext = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: 16000 });
    const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);

    // Get mono audio data (use first channel)
    const channelData = audioBuffer.getChannelData(0);

    // Resample if necessary (target: 16kHz)
    let samples = channelData;
    if (audioBuffer.sampleRate !== 16000) {
      samples = resampleAudio(channelData, audioBuffer.sampleRate, 16000);
    }

    // Convert Float32 to Int16
    const int16Data = new Int16Array(samples.length);
    for (let i = 0; i < samples.length; i++) {
      const s = Math.max(-1, Math.min(1, samples[i]));
      int16Data[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
    }

    // Send in chunks (to simulate streaming)
    const chunkSize = 1600; // 100ms at 16kHz
    const totalChunks = Math.ceil(int16Data.length / chunkSize);

    logger.log('Audio', 'Sending audio', {
      duration: (samples.length / 16000).toFixed(2) + 's',
      chunks: totalChunks
    });

    // Track TTFT timing
    state.sttStartTime = Date.now();
    state.sttFirstResult = false;

    for (let i = 0; i < int16Data.length; i += chunkSize) {
      const chunk = int16Data.slice(i, i + chunkSize);
      wsManager.sendAudio(chunk);
      // Small delay between chunks to simulate real-time streaming
      await new Promise(resolve => setTimeout(resolve, 50));
    }

    logger.log('Audio', 'File sent successfully');
    audioContext.close();

  } catch (error) {
    console.error('Failed to process audio file:', error);
    alert('Failed to process audio file: ' + (error.message || error));
  }
}

/**
 * Resample audio to target sample rate using linear interpolation
 */
function resampleAudio(samples, fromRate, toRate) {
  const ratio = fromRate / toRate;
  const newLength = Math.round(samples.length / ratio);
  const result = new Float32Array(newLength);

  for (let i = 0; i < newLength; i++) {
    const srcIndex = i * ratio;
    const srcIndexFloor = Math.floor(srcIndex);
    const srcIndexCeil = Math.min(srcIndexFloor + 1, samples.length - 1);
    const t = srcIndex - srcIndexFloor;
    result[i] = samples[srcIndexFloor] * (1 - t) + samples[srcIndexCeil] * t;
  }

  return result;
}

function getSTTConfig() {
  const config = {
    provider: document.getElementById('stt-provider').value,
    language: document.getElementById('stt-language').value,
    model: document.getElementById('stt-model').value,
    sample_rate: 16000,
    channels: 1,
    encoding: 'linear16',
    punctuation: document.getElementById('stt-punctuation').checked,
  };

  // Include provider API key if set
  const apiKey = document.getElementById('stt-api-key').value;
  if (apiKey) {
    config.api_key = apiKey;
  }

  return config;
}

/**
 * TTS Panel
 */
function setupTTS() {
  const loadVoicesBtn = document.getElementById('tts-load-voices');
  const speakBtn = document.getElementById('tts-speak-btn');
  const stopBtn = document.getElementById('tts-stop-btn');
  const applyConfigBtn = document.getElementById('tts-apply-config');

  loadVoicesBtn.addEventListener('click', loadVoices);
  speakBtn.addEventListener('click', speak);
  stopBtn.addEventListener('click', stopSpeaking);

  applyConfigBtn.addEventListener('click', () => {
    if (!state.connected) {
      alert('Please connect first');
      return;
    }
    wsManager.sendConfig({
      stt: getSTTConfig(),
      tts: getTTSConfig(),
    });
    logger.log('TTS', 'Config applied', { provider: getTTSConfig().provider });
  });
}

async function loadVoices() {
  const provider = document.getElementById('tts-provider').value;
  const providerApiKey = document.getElementById('tts-api-key').value;
  const voiceSelect = document.getElementById('tts-voice');

  try {
    const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

    // Build headers with both gateway auth and provider API key
    const headers = {};
    if (elements.apiKey.value) {
      headers['Authorization'] = `Bearer ${elements.apiKey.value}`;
    }
    if (providerApiKey) {
      headers['X-Provider-Api-Key'] = providerApiKey;
    }

    const response = await fetch(`${baseUrl}/voices?provider=${provider}`, { headers });

    if (!response.ok) {
      const errorText = await response.text();
      throw new Error(errorText || 'Failed to load voices');
    }

    const voices = await response.json();

    voiceSelect.innerHTML = '<option value="">Select voice...</option>';
    voices.forEach((voice) => {
      const option = document.createElement('option');
      option.value = voice.id || voice.voice_id;
      option.textContent = voice.name || voice.id;
      voiceSelect.appendChild(option);
    });

    logger.log('TTS', 'Voices loaded', { count: voices.length, provider });
  } catch (error) {
    console.error('Failed to load voices:', error);
    alert('Failed to load voices: ' + error.message);
  }
}

function speak() {
  if (!state.connected) {
    alert('Please connect first');
    return;
  }

  const text = document.getElementById('tts-text').value;
  if (!text) {
    alert('Please enter text to speak');
    return;
  }

  state.ttsStartTime = Date.now();
  state.ttsFirstAudio = false;

  wsManager.send({
    type: 'speak',
    text,
    flush: true,
  });

  const playerEl = document.getElementById('tts-player');
  playerEl.querySelector('.player-status').textContent = 'Speaking...';

  logger.log('TTS', 'Speak', { text: text.substring(0, 50) + '...' });
}

function stopSpeaking() {
  if (wsManager) {
    wsManager.send({ type: 'clear' });
  }

  const playerEl = document.getElementById('tts-player');
  playerEl.querySelector('.player-status').textContent = 'Stopped';
}

function handleTTSAudio(data) {
  // Calculate TTFB on first audio
  if (state.ttsStartTime && !state.ttsFirstAudio) {
    const ttfb = Date.now() - state.ttsStartTime;
    metrics.recordTTSTtfb(ttfb);
    updateQuickMetrics();
    state.ttsFirstAudio = true;
    logger.log('TTS', 'TTFB', { ttfb: ttfb + 'ms' });
  }

  // Play audio (would need Web Audio API implementation)
  const playerEl = document.getElementById('tts-player');
  playerEl.querySelector('.player-status').textContent = 'Playing audio...';
}

function handleTTSComplete() {
  const playerEl = document.getElementById('tts-player');
  playerEl.querySelector('.player-status').textContent = 'Complete';
  logger.log('TTS', 'Playback complete');
}

function getTTSConfig() {
  const config = {
    provider: document.getElementById('tts-provider').value,
    voice_id: document.getElementById('tts-voice').value,
    model: document.getElementById('tts-model').value,
    sample_rate: 24000,
  };

  // Include provider API key if set
  const apiKey = document.getElementById('tts-api-key').value;
  if (apiKey) {
    config.api_key = apiKey;
  }

  return config;
}

/**
 * LiveKit Panel
 */
function setupLiveKit() {
  const generateTokenBtn = document.getElementById('lk-generate-token');
  const listRoomsBtn = document.getElementById('lk-list-rooms');

  generateTokenBtn.addEventListener('click', generateLiveKitToken);
  listRoomsBtn.addEventListener('click', listLiveKitRooms);
}

async function generateLiveKitToken() {
  const roomName = document.getElementById('lk-room').value;
  const identity = document.getElementById('lk-identity').value;
  const name = document.getElementById('lk-name').value;

  if (!roomName || !identity) {
    alert('Room name and identity are required');
    return;
  }

  try {
    const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

    const response = await fetch(`${baseUrl}/livekit/token`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(elements.apiKey.value ? { Authorization: `Bearer ${elements.apiKey.value}` } : {}),
      },
      body: JSON.stringify({ room_name: roomName, identity, name }),
    });

    const data = await response.json();

    const resultEl = document.getElementById('lk-token-result');
    resultEl.textContent = JSON.stringify(data, null, 2);

    logger.log('LiveKit', 'Token generated', { room: roomName });
  } catch (error) {
    console.error('Failed to generate token:', error);
    alert('Failed to generate token: ' + error.message);
  }
}

async function listLiveKitRooms() {
  try {
    const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

    const response = await fetch(`${baseUrl}/livekit/rooms`, {
      headers: elements.apiKey.value ? { Authorization: `Bearer ${elements.apiKey.value}` } : {},
    });

    const data = await response.json();

    const resultEl = document.getElementById('lk-rooms-result');
    resultEl.textContent = JSON.stringify(data, null, 2);

    logger.log('LiveKit', 'Rooms listed', { count: data.length });
  } catch (error) {
    console.error('Failed to list rooms:', error);
    alert('Failed to list rooms: ' + error.message);
  }
}

/**
 * SIP Panel
 */
function setupSIP() {
  const createHookBtn = document.getElementById('sip-create-hook');
  const listHooksBtn = document.getElementById('sip-list-hooks');

  createHookBtn.addEventListener('click', createSIPHook);
  listHooksBtn.addEventListener('click', listSIPHooks);
}

async function createSIPHook() {
  const host = document.getElementById('sip-host').value;
  const webhookUrl = document.getElementById('sip-webhook').value;

  if (!host || !webhookUrl) {
    alert('Host and webhook URL are required');
    return;
  }

  try {
    const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

    const response = await fetch(`${baseUrl}/sip/hooks`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(elements.apiKey.value ? { Authorization: `Bearer ${elements.apiKey.value}` } : {}),
      },
      body: JSON.stringify({ host, webhook_url: webhookUrl }),
    });

    const data = await response.json();
    alert('Hook created successfully');
    listSIPHooks();

    logger.log('SIP', 'Hook created', { host });
  } catch (error) {
    console.error('Failed to create hook:', error);
    alert('Failed to create hook: ' + error.message);
  }
}

async function listSIPHooks() {
  try {
    const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

    const response = await fetch(`${baseUrl}/sip/hooks`, {
      headers: elements.apiKey.value ? { Authorization: `Bearer ${elements.apiKey.value}` } : {},
    });

    const data = await response.json();

    const resultEl = document.getElementById('sip-hooks-result');
    resultEl.textContent = JSON.stringify(data, null, 2);

    logger.log('SIP', 'Hooks listed', { count: data.length });
  } catch (error) {
    console.error('Failed to list hooks:', error);
    alert('Failed to list hooks: ' + error.message);
  }
}

/**
 * API Explorer
 */
function setupAPIExplorer() {
  const sendBtn = document.getElementById('api-send');
  sendBtn.addEventListener('click', sendAPIRequest);
}

async function sendAPIRequest() {
  const method = document.getElementById('api-method').value;
  const endpoint = document.getElementById('api-endpoint').value;
  const bodyText = document.getElementById('api-body').value;

  const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

  const options = {
    method,
    headers: {
      'Content-Type': 'application/json',
      ...(elements.apiKey.value ? { Authorization: `Bearer ${elements.apiKey.value}` } : {}),
    },
  };

  if (bodyText && (method === 'POST' || method === 'PUT')) {
    try {
      options.body = bodyText;
    } catch (e) {
      alert('Invalid JSON body');
      return;
    }
  }

  const startTime = Date.now();

  try {
    const response = await fetch(baseUrl + endpoint, options);
    const duration = Date.now() - startTime;

    let data;
    const contentType = response.headers.get('content-type');
    if (contentType && contentType.includes('application/json')) {
      data = await response.json();
    } else {
      data = await response.text();
    }

    const resultEl = document.getElementById('api-response');
    resultEl.textContent = JSON.stringify(data, null, 2);

    logger.log(method, endpoint, { status: response.status, duration: duration + 'ms' });
  } catch (error) {
    console.error('API request failed:', error);
    document.getElementById('api-response').textContent = 'Error: ' + error.message;
  }
}

/**
 * WS Debug
 */
function setupWSDebug() {
  const templateSelect = document.getElementById('ws-template');
  const messageTextarea = document.getElementById('ws-message');
  const sendBtn = document.getElementById('ws-send');

  templateSelect.addEventListener('change', () => {
    const template = templateSelect.value;
    if (template) {
      messageTextarea.value = getWSTemplate(template);
    }
  });

  sendBtn.addEventListener('click', () => {
    sendWSMessage(messageTextarea.value);
  });
}

function getWSTemplate(name) {
  const templates = {
    config: JSON.stringify({
      type: 'config',
      audio: true,
      stt_config: {
        provider: 'deepgram',
        language: 'en-US',
        model: 'nova-3',
      },
    }, null, 2),
    speak: JSON.stringify({
      type: 'speak',
      text: 'Hello, world!',
      flush: true,
    }, null, 2),
    clear: JSON.stringify({ type: 'clear' }, null, 2),
    ping: JSON.stringify({
      type: 'ping',
      timestamp: Date.now(),
    }, null, 2),
  };

  return templates[name] || '';
}

function sendWSMessage(messageText) {
  if (!state.connected) {
    alert('Please connect first');
    return;
  }

  try {
    const message = JSON.parse(messageText);
    wsManager.send(message);
    addWSLogEntry('out', message);
    logger.log('WS', 'Sent', { type: message.type });
  } catch (error) {
    alert('Invalid JSON: ' + error.message);
  }
}

function addWSLogEntry(direction, data) {
  const logEl = document.getElementById('ws-log');
  const time = new Date().toLocaleTimeString();

  const entry = document.createElement('div');
  entry.className = 'message';
  entry.innerHTML = `
    <span class="message-time">${time}</span>
    <span class="message-direction ${direction}">${direction === 'in' ? '←' : '→'}</span>
    <span class="message-content">${JSON.stringify(data, null, 2)}</span>
  `;

  logEl.insertBefore(entry, logEl.firstChild);

  // Limit log entries
  while (logEl.children.length > 50) {
    logEl.removeChild(logEl.lastChild);
  }
}

/**
 * Audio Tools
 */
function setupAudioTools() {
  const refreshBtn = document.getElementById('audio-refresh-devices');
  const testRecordBtn = document.getElementById('audio-test-record');
  const testPlayBtn = document.getElementById('audio-test-play');

  refreshBtn.addEventListener('click', refreshAudioDevices);
  testRecordBtn.addEventListener('click', testRecord);

  // Initial device list
  refreshAudioDevices();
}

async function refreshAudioDevices() {
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    const audioInputs = devices.filter((d) => d.kind === 'audioinput');

    const select = document.getElementById('audio-input-device');
    select.innerHTML = '';

    audioInputs.forEach((device) => {
      const option = document.createElement('option');
      option.value = device.deviceId;
      option.textContent = device.label || `Microphone ${select.children.length + 1}`;
      select.appendChild(option);
    });
  } catch (error) {
    console.error('Failed to enumerate devices:', error);
  }
}

async function testRecord() {
  const btn = document.getElementById('audio-test-record');
  btn.disabled = true;
  btn.textContent = 'Recording...';

  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });

    // Record for 5 seconds
    setTimeout(() => {
      stream.getTracks().forEach((t) => t.stop());
      btn.disabled = false;
      btn.textContent = 'Record 5s';
      document.getElementById('audio-test-play').disabled = false;
    }, 5000);
  } catch (error) {
    console.error('Failed to record:', error);
    btn.disabled = false;
    btn.textContent = 'Record 5s';
    alert('Failed to access microphone: ' + error.message);
  }
}

/**
 * Metrics Panel
 */
function setupMetrics() {
  const resetBtn = document.getElementById('metrics-reset');
  const exportBtn = document.getElementById('metrics-export');

  resetBtn.addEventListener('click', () => {
    metrics.reset();
    updateMetricsDisplay();
    logger.log('Metrics', 'Reset');
  });

  exportBtn.addEventListener('click', exportMetrics);

  // Update metrics periodically
  setInterval(updateMetricsDisplay, 1000);
}

function updateMetricsDisplay() {
  const data = metrics.getSummary();

  document.getElementById('metric-stt-ttft').textContent = data.sttTtft.p95 ? Math.round(data.sttTtft.p95) : '-';
  document.getElementById('metric-tts-ttfb').textContent = data.ttsTtfb.p95 ? Math.round(data.ttsTtfb.p95) : '-';
  document.getElementById('metric-e2e').textContent = data.e2e.p95 ? Math.round(data.e2e.p95) : '-';
  document.getElementById('metric-ws-connect').textContent = data.wsConnect ? Math.round(data.wsConnect) : '-';

  // Update SLO status
  updateSLOStatus('slo-stt', data.sttTtft.p95, 200);
  updateSLOStatus('slo-tts', data.ttsTtfb.p95, 150);
  updateSLOStatus('slo-e2e', data.e2e.p95, 1000);
}

function updateSLOStatus(elementId, value, threshold) {
  const el = document.getElementById(elementId);
  if (!value) {
    el.className = 'slo-item';
    el.querySelector('.slo-status').textContent = '-';
  } else if (value <= threshold) {
    el.className = 'slo-item pass';
  } else if (value <= threshold * 1.5) {
    el.className = 'slo-item warn';
  } else {
    el.className = 'slo-item fail';
  }
}

function updateQuickMetrics() {
  const data = metrics.getSummary();
  elements.quickTtft.textContent = data.sttTtft.last ? Math.round(data.sttTtft.last) + 'ms' : '-';
  elements.quickTtfb.textContent = data.ttsTtfb.last ? Math.round(data.ttsTtfb.last) + 'ms' : '-';
}

function exportMetrics() {
  const data = metrics.export();
  const blob = new Blob([data], { type: 'text/csv' });
  const url = URL.createObjectURL(blob);

  const a = document.createElement('a');
  a.href = url;
  a.download = `bud-metrics-${Date.now()}.csv`;
  a.click();

  URL.revokeObjectURL(url);
}

/**
 * Theme Toggle
 */
function setupTheme() {
  const savedTheme = localStorage.getItem('theme') || 'light';
  document.documentElement.dataset.theme = savedTheme;

  elements.themeToggle.addEventListener('click', () => {
    const current = document.documentElement.dataset.theme;
    const next = current === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = next;
    localStorage.setItem('theme', next);
  });
}

// Initialize when DOM is ready
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', init);
} else {
  init();
}

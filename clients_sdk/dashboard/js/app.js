/**
 * WaaV Dashboard - Main Application
 * AI Voice Gateway Dashboard with full feature set
 */

import { State } from './state.js';
import { WebSocketManager } from './websocket.js?v=6';
import { MetricsCollector } from './metrics.js';
import { Logger } from './logger.js?v=2';
import { AudioManager } from './audio.js';
import { KeyboardManager, CommandPalette } from './keyboard.js';
import { SessionHistory } from './sessionHistory.js';
import { SSMLEditor } from './ssmlEditor.js';
import { BatchProcessor } from './batchProcessor.js';
import { Sparkline, SparklineManager } from './sparkline.js';

// Initialize core state
const state = new State();
const metrics = new MetricsCollector();
const sessionHistory = new SessionHistory();

// Initialize logger (will be set after DOM ready)
let logger = null;

// Managers (initialized after DOM)
let wsManager = null;
let audioManager = null;
let keyboardManager = null;
let commandPalette = null;
let currentSession = null;
let ssmlEditor = null;
let ttsInputMode = 'text'; // 'text' or 'ssml'
let batchProcessor = null;
let metricsSparklines = {};

// Waveform visualization
let waveformCanvas = null;
let waveformCtx = null;
let waveformAnimationId = null;
let audioAnalyser = null;

// Toast notification queue
const toastQueue = [];

// DOM Elements cache
const elements = {};

/**
 * Initialize the dashboard
 */
function init() {
  console.log('[WaaV] Initializing dashboard...');

  // Cache DOM elements
  cacheElements();

  // Initialize logger
  logger = new Logger(elements.requestLog);

  // Set default server URL
  initializeServerUrl();

  // Check secure context
  checkSecureContext();

  // Setup sidebar navigation
  setupSidebar();

  // Setup header
  setupHeader();

  // Setup connection
  setupConnection();

  // Setup all panels
  setupDashboardHome();
  setupSTT();
  setupTTS();
  setupABComparison();
  setupLiveKit();
  setupSIP();
  setupAPIExplorer();
  setupWSDebug();
  setupAudioTools();
  setupMetrics();

  // Setup keyboard shortcuts
  setupKeyboardShortcuts();

  // Setup theme
  setupTheme();

  // Load initial state
  loadInitialState();

  console.log('[WaaV] Dashboard initialized successfully');
}

/**
 * Cache DOM elements for faster access
 */
function cacheElements() {
  elements.sidebar = document.getElementById('sidebar');
  elements.sidebarToggle = document.getElementById('sidebar-toggle');
  elements.navItems = document.querySelectorAll('.nav-link[data-tab]');
  elements.tabPanels = document.querySelectorAll('.tab-panel');
  elements.headerTitle = document.getElementById('header-title');

  elements.serverUrl = document.getElementById('server-url');
  elements.apiKey = document.getElementById('api-key');
  elements.connectBtn = document.getElementById('connect-btn');
  elements.statusIndicator = document.getElementById('conn-indicator');
  elements.statusDot = document.querySelector('#quick-conn-status .status-dot');
  elements.statusText = document.querySelector('#quick-conn-status .status-label');

  elements.quickTtft = document.getElementById('quick-ttft');
  elements.quickTtfb = document.getElementById('quick-ttfb');

  elements.themeToggle = document.getElementById('theme-toggle');
  elements.searchBtn = document.getElementById('search-btn');
  elements.helpBtn = document.getElementById('help-btn');

  elements.requestLog = document.getElementById('request-log');
  elements.toastContainer = document.getElementById('toast-container');
  elements.shortcutsModal = document.getElementById('shortcuts-modal');
}

/**
 * Initialize server URL based on current host
 */
function initializeServerUrl() {
  const host = window.location.hostname;
  const wsProtocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const defaultUrl = `${wsProtocol}//${host}:3001/ws`;
  if (elements.serverUrl) {
    elements.serverUrl.value = defaultUrl;
  }
  console.log('[WaaV] Default server URL:', defaultUrl);
}

/**
 * Check secure context
 */
function checkSecureContext() {
  if (!window.isSecureContext) {
    console.warn('[WaaV] Not in secure context. Microphone access may be blocked.');
    showToast('warning', 'Secure Context Required',
      'Microphone access requires HTTPS or localhost.');
  }
}

/**
 * Setup sidebar navigation
 */
function setupSidebar() {
  // Toggle sidebar collapse
  if (elements.sidebarToggle) {
    elements.sidebarToggle.addEventListener('click', () => {
      elements.sidebar.classList.toggle('collapsed');
      state.sidebarCollapsed = elements.sidebar.classList.contains('collapsed');
    });
  }

  // Apply saved sidebar state
  if (state.sidebarCollapsed && elements.sidebar) {
    elements.sidebar.classList.add('collapsed');
  }

  // Navigation items
  elements.navItems.forEach((item) => {
    item.addEventListener('click', () => {
      const tabId = item.dataset.tab;
      switchTab(tabId);
    });
  });
}

/**
 * Switch to a tab
 */
function switchTab(tabId) {
  // Update navigation
  elements.navItems.forEach((item) => {
    item.classList.toggle('active', item.dataset.tab === tabId);
  });

  // Update panels
  elements.tabPanels.forEach((panel) => {
    panel.classList.toggle('active', panel.id === `panel-${tabId}`);
  });

  // Update header title
  const titles = {
    home: 'Dashboard',
    stt: 'Speech-to-Text',
    tts: 'Text-to-Speech',
    compare: 'A/B Voice Comparison',
    livekit: 'LiveKit Integration',
    sip: 'SIP Configuration',
    api: 'API Explorer',
    ws: 'WebSocket Debug',
    audio: 'Audio Tools',
    metrics: 'Performance Metrics',
  };
  if (elements.headerTitle) {
    elements.headerTitle.textContent = titles[tabId] || 'Dashboard';
  }

  // Update state
  state.currentTab = tabId;
}

/**
 * Setup header
 */
function setupHeader() {
  // Search button opens command palette
  if (elements.searchBtn) {
    elements.searchBtn.addEventListener('click', () => {
      if (commandPalette) {
        commandPalette.toggle();
      }
    });
  }

  // Help button shows shortcuts
  if (elements.helpBtn) {
    elements.helpBtn.addEventListener('click', showShortcutsModal);
  }
}

/**
 * Setup connection management
 */
function setupConnection() {
  if (elements.connectBtn) {
    elements.connectBtn.addEventListener('click', async () => {
      if (state.connected) {
        disconnect();
      } else {
        await connect();
      }
    });
  }
}

/**
 * Connect to server
 */
async function connect() {
  const url = elements.serverUrl?.value;
  const apiKey = elements.apiKey?.value;

  if (!url) {
    showToast('error', 'Connection Error', 'Please enter a server URL');
    return;
  }

  updateConnectionStatus('connecting');

  try {
    wsManager = new WebSocketManager(url, apiKey);

    wsManager.onOpen = () => {
      state.connected = true;
      updateConnectionStatus('connected');
      showToast('success', 'Connected', 'Successfully connected to WaaV Gateway');
      logger?.log('WS', 'Connected', { url });

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
      logger?.log('WS', 'Disconnected');
    };

    wsManager.onError = (error) => {
      console.error('[WaaV] WebSocket error:', error);
      updateConnectionStatus('error');
      showToast('error', 'Connection Error', 'WebSocket connection failed');
    };

    await wsManager.connect();
  } catch (error) {
    console.error('[WaaV] Connection failed:', error);
    updateConnectionStatus('error');
    showToast('error', 'Connection Failed', error?.message || 'Unknown error');
  }
}

/**
 * Disconnect from server
 */
function disconnect() {
  if (wsManager) {
    wsManager.disconnect();
    wsManager = null;
  }
  state.connected = false;
  updateConnectionStatus('disconnected');
  showToast('info', 'Disconnected', 'Connection closed');
}

/**
 * Update connection status UI
 */
function updateConnectionStatus(status) {
  // Update sidebar connection indicator
  if (elements.statusIndicator) {
    elements.statusIndicator.dataset.status = status;
  }

  // Update header status dot
  if (elements.statusDot) {
    elements.statusDot.dataset.status = status;
  }

  const statusTexts = {
    connected: 'Connected',
    connecting: 'Connecting...',
    error: 'Error',
    disconnected: 'Disconnected',
  };

  const btnTexts = {
    connected: 'Disconnect',
    connecting: 'Cancel',
    error: 'Reconnect',
    disconnected: 'Connect',
  };

  // Update header status text
  if (elements.statusText) {
    elements.statusText.textContent = statusTexts[status] || 'Disconnected';
  }

  if (elements.connectBtn) {
    elements.connectBtn.textContent = btnTexts[status] || 'Connect';
  }

  // Update dashboard home status
  updateDashboardStatus();
}

/**
 * Handle WebSocket messages
 */
function handleWSMessage(data) {
  addWSLogEntry('in', data);

  switch (data.type) {
    case 'ready':
      logger?.log('WS', 'Ready', { stream_id: data.stream_id });
      break;

    case 'stt_result':
      handleSTTResult(data);
      break;

    case 'tts_audio':
    case 'audio':
      handleTTSAudio(data);
      break;

    case 'tts_playback_complete':
      handleTTSComplete();
      break;

    case 'pong':
      const latency = Date.now() - data.timestamp;
      logger?.log('WS', 'Pong', { latency: latency + 'ms' });
      break;

    case 'error':
      logger?.log('ERROR', data.message);
      showToast('error', 'Server Error', data.message);
      break;
  }
}

/**
 * Dashboard Home Panel
 */
function setupDashboardHome() {
  // Quick action buttons
  const quickActions = document.querySelectorAll('.quick-action-btn[data-action]');
  quickActions.forEach((btn) => {
    btn.addEventListener('click', () => {
      const action = btn.dataset.action;
      handleQuickAction(action);
    });
  });

  // Load recent sessions
  loadRecentSessions();
}

function handleQuickAction(action) {
  switch (action) {
    case 'new-stt':
      switchTab('stt');
      break;
    case 'new-tts':
      switchTab('tts');
      break;
    case 'compare':
      switchTab('compare');
      break;
    case 'settings':
      // Could open settings modal
      showToast('info', 'Settings', 'Settings panel coming soon');
      break;
  }
}

async function loadRecentSessions() {
  try {
    const sessions = await sessionHistory.listSessions({ limit: 5 });
    const listEl = document.getElementById('recent-sessions-list');
    if (!listEl) return;

    if (sessions.length === 0) {
      listEl.innerHTML = '<p class="text-muted">No recent sessions</p>';
      return;
    }

    listEl.innerHTML = sessions.map((session) => `
      <div class="session-item" data-id="${session.id}">
        <div class="session-info">
          <div class="session-type-icon">${session.type === 'stt' ? '🎤' : '🔊'}</div>
          <div class="session-details">
            <div class="session-name">${session.type.toUpperCase()} - ${session.provider}</div>
            <div class="session-time">${formatTimeAgo(session.startTime)}</div>
          </div>
        </div>
        <div class="session-duration">${formatDuration(session.duration)}</div>
      </div>
    `).join('');

    // Add click handlers
    listEl.querySelectorAll('.session-item').forEach((item) => {
      item.addEventListener('click', () => {
        const sessionId = item.dataset.id;
        viewSession(sessionId);
      });
    });
  } catch (error) {
    console.error('[WaaV] Failed to load sessions:', error);
  }
}

function updateDashboardStatus() {
  // Update gateway status
  const gatewayStatus = document.getElementById('gateway-status');
  if (gatewayStatus) {
    gatewayStatus.className = 'status-badge ' + (state.connected ? 'online' : 'offline');
    gatewayStatus.textContent = state.connected ? 'Online' : 'Offline';
  }

  // Update stats
  const statsData = metrics.getSummary();
  const sessionsToday = document.getElementById('stat-sessions-today');
  if (sessionsToday) {
    sessionsToday.textContent = statsData.totalRequests || '0';
  }
}

async function viewSession(sessionId) {
  try {
    const session = await sessionHistory.getSession(sessionId);
    if (session) {
      // Show session details (could open a modal or switch to a details view)
      console.log('[WaaV] View session:', session);
      showToast('info', 'Session', `Viewing session ${sessionId.substring(0, 8)}...`);
    }
  } catch (error) {
    console.error('[WaaV] Failed to load session:', error);
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
  const clearBtn = document.getElementById('stt-clear-btn');

  if (recordBtn) {
    recordBtn.addEventListener('click', async () => {
      if (state.recording) {
        await stopRecording();
      } else {
        await startRecording();
      }
    });
  }

  if (uploadBtn) {
    uploadBtn.addEventListener('click', () => fileInput?.click());
  }

  if (fileInput) {
    fileInput.addEventListener('change', (e) => {
      const files = e.target.files;
      if (files && files.length > 0) {
        if (files.length === 1) {
          uploadAudioFile(files[0]);
        } else {
          handleBatchUpload(files);
        }
      }
      // Reset input for subsequent uploads
      fileInput.value = '';
    });
  }

  // Setup batch processing
  setupBatchProcessor();

  if (applyConfigBtn) {
    applyConfigBtn.addEventListener('click', applySTTConfig);
  }

  if (clearBtn) {
    clearBtn.addEventListener('click', clearTranscript);
  }

  // Export buttons
  setupExportButtons();

  // Initialize waveform canvas
  initWaveformCanvas();
}

function initWaveformCanvas() {
  waveformCanvas = document.getElementById('stt-waveform');
  if (waveformCanvas) {
    waveformCtx = waveformCanvas.getContext('2d');
    // Set canvas size
    const rect = waveformCanvas.parentElement?.getBoundingClientRect();
    if (rect) {
      waveformCanvas.width = rect.width || 400;
      waveformCanvas.height = 80;
    }
  }
}

function drawWaveform(dataArray) {
  if (!waveformCtx || !waveformCanvas) return;

  const width = waveformCanvas.width;
  const height = waveformCanvas.height;
  const bufferLength = dataArray.length;

  waveformCtx.fillStyle = getComputedStyle(document.documentElement)
    .getPropertyValue('--bg').trim() || '#f9fafb';
  waveformCtx.fillRect(0, 0, width, height);

  waveformCtx.lineWidth = 2;
  waveformCtx.strokeStyle = getComputedStyle(document.documentElement)
    .getPropertyValue('--primary').trim() || '#6366f1';
  waveformCtx.beginPath();

  const sliceWidth = width / bufferLength;
  let x = 0;

  for (let i = 0; i < bufferLength; i++) {
    const v = dataArray[i] / 128.0;
    const y = (v * height) / 2;

    if (i === 0) {
      waveformCtx.moveTo(x, y);
    } else {
      waveformCtx.lineTo(x, y);
    }

    x += sliceWidth;
  }

  waveformCtx.lineTo(width, height / 2);
  waveformCtx.stroke();
}

async function startRecording() {
  if (!state.connected) {
    showToast('warning', 'Not Connected', 'Please connect to the server first');
    return;
  }

  if (!window.isSecureContext) {
    showToast('error', 'Secure Context Required',
      'Microphone access requires HTTPS or localhost');
    return;
  }

  try {
    audioManager = new AudioManager({ sampleRate: 16000 });

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

    // Create session
    currentSession = await sessionHistory.createSession({
      type: 'stt',
      provider: document.getElementById('stt-provider')?.value || 'deepgram',
      config: getSTTConfig(),
    });

    state.sttStartTime = Date.now();

    updateRecordButton(true);
    showToast('success', 'Recording', 'Recording started');
    logger?.log('STT', 'Recording started');

  } catch (error) {
    console.error('[WaaV] Failed to start recording:', error);
    showToast('error', 'Microphone Error', error?.message || 'Failed to access microphone');
  }
}

async function stopRecording() {
  if (audioManager) {
    audioManager.stopRecording();
    audioManager = null;
  }

  state.recording = false;
  updateRecordButton(false);

  // End session
  if (currentSession) {
    await sessionHistory.updateSession(currentSession.id, {
      endTime: Date.now(),
      transcript: state.transcript,
      status: 'completed',
    });
    currentSession = null;
  }

  showToast('info', 'Recording Stopped', 'Recording has been stopped');
  logger?.log('STT', 'Recording stopped');

  // Refresh recent sessions
  loadRecentSessions();
}

function updateRecordButton(isRecording) {
  const recordBtn = document.getElementById('stt-record-btn');
  if (!recordBtn) return;

  if (isRecording) {
    recordBtn.innerHTML = `
      <svg class="icon" viewBox="0 0 24 24" fill="currentColor">
        <rect x="6" y="6" width="12" height="12"/>
      </svg>
      Stop Recording
    `;
    recordBtn.classList.add('btn-danger');
    recordBtn.classList.remove('btn-primary');
  } else {
    recordBtn.innerHTML = `
      <svg class="icon" viewBox="0 0 24 24" fill="currentColor">
        <path d="M12 14c1.66 0 3-1.34 3-3V5c0-1.66-1.34-3-3-3S9 3.34 9 5v6c0 1.66 1.34 3 3 3z"/>
        <path d="M17 11c0 2.76-2.24 5-5 5s-5-2.24-5-5H5c0 3.53 2.61 6.43 6 6.92V21h2v-3.08c3.39-.49 6-3.39 6-6.92h-2z"/>
      </svg>
      Start Recording
    `;
    recordBtn.classList.remove('btn-danger');
    recordBtn.classList.add('btn-primary');
  }
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
    logger?.log('STT', 'TTFT', { ttft: ttft + 'ms' });

    // Update session metrics
    if (currentSession) {
      sessionHistory.updateMetrics(currentSession.id, { ttft });
    }
  }

  const transcriptEl = document.getElementById('stt-transcript');
  if (!transcriptEl) return;

  if (isFinal) {
    state.transcript += transcript + ' ';
    state.interimTranscript = '';
  } else {
    state.interimTranscript = transcript;
  }

  transcriptEl.innerHTML = `
    <div class="transcript-text">${state.transcript || ''}</div>
    ${state.interimTranscript ? `<div class="transcript-text interim">${state.interimTranscript}</div>` : ''}
  `;

  if (!state.transcript && !state.interimTranscript) {
    transcriptEl.innerHTML = '<p class="transcript-placeholder">Start speaking to see transcription...</p>';
  }
}

function updateVisualizerLevel(level) {
  const visualizer = document.getElementById('stt-visualizer');
  if (!visualizer) return;

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
    showToast('warning', 'Not Connected', 'Please connect to the server first');
    return;
  }

  logger?.log('Audio', 'Processing file', { name: file.name, size: file.size });
  showToast('info', 'Processing', `Processing ${file.name}...`);

  try {
    const arrayBuffer = await file.arrayBuffer();
    const audioContext = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: 16000 });
    const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);

    const channelData = audioBuffer.getChannelData(0);
    let samples = channelData;
    if (audioBuffer.sampleRate !== 16000) {
      samples = resampleAudio(channelData, audioBuffer.sampleRate, 16000);
    }

    const int16Data = new Int16Array(samples.length);
    for (let i = 0; i < samples.length; i++) {
      const s = Math.max(-1, Math.min(1, samples[i]));
      int16Data[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
    }

    const chunkSize = 1600;
    state.sttStartTime = Date.now();
    state.sttFirstResult = false;

    for (let i = 0; i < int16Data.length; i += chunkSize) {
      const chunk = int16Data.slice(i, i + chunkSize);
      wsManager.sendAudio(chunk);
      await new Promise((resolve) => setTimeout(resolve, 50));
    }

    logger?.log('Audio', 'File sent successfully');
    showToast('success', 'Upload Complete', 'Audio file processed successfully');
    audioContext.close();

  } catch (error) {
    console.error('[WaaV] Failed to process audio file:', error);
    showToast('error', 'Processing Error', error.message || 'Failed to process audio');
  }
}

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

/**
 * Batch Processing Functions
 */
function setupBatchProcessor() {
  batchProcessor = new BatchProcessor({
    concurrency: 2,
    processFn: processBatchFile,
  });

  // Event listeners
  batchProcessor.on('itemAdded', updateBatchQueueUI);
  batchProcessor.on('itemRemoved', updateBatchQueueUI);
  batchProcessor.on('queueCleared', updateBatchQueueUI);
  batchProcessor.on('itemStart', (item) => {
    updateBatchItemUI(item);
    logger?.log('Batch', 'Processing started', { file: item.name });
  });
  batchProcessor.on('itemComplete', (item) => {
    updateBatchItemUI(item);
    logger?.log('Batch', 'Processing complete', { file: item.name });
  });
  batchProcessor.on('itemError', (item, error) => {
    updateBatchItemUI(item);
    logger?.log('Batch', 'Processing error', { file: item.name, error: error.message });
  });
  batchProcessor.on('progress', updateBatchProgress);
  batchProcessor.on('complete', handleBatchComplete);

  // UI buttons
  const processBtn = document.getElementById('stt-process-queue');
  const clearBtn = document.getElementById('stt-clear-queue');

  if (processBtn) {
    processBtn.addEventListener('click', () => {
      if (!state.connected) {
        showToast('warning', 'Not Connected', 'Please connect to the server first');
        return;
      }
      batchProcessor.processAll();
    });
  }

  if (clearBtn) {
    clearBtn.addEventListener('click', () => {
      batchProcessor.clearQueue();
      hideBatchQueue();
    });
  }
}

async function processBatchFile(file, item) {
  if (!state.connected) {
    throw new Error('Not connected to server');
  }

  const arrayBuffer = await file.arrayBuffer();
  const audioContext = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: 16000 });
  const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);

  const channelData = audioBuffer.getChannelData(0);
  let samples = channelData;
  if (audioBuffer.sampleRate !== 16000) {
    samples = resampleAudio(channelData, audioBuffer.sampleRate, 16000);
  }

  const int16Data = new Int16Array(samples.length);
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    int16Data[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
  }

  const chunkSize = 1600;
  const totalChunks = Math.ceil(int16Data.length / chunkSize);

  for (let i = 0; i < int16Data.length; i += chunkSize) {
    const chunk = int16Data.slice(i, i + chunkSize);
    wsManager.sendAudio(chunk);
    item.setProgress(Math.round((i / int16Data.length) * 100));
    await new Promise((resolve) => setTimeout(resolve, 50));
  }

  audioContext.close();
  return `Processed ${file.name}`;
}

function handleBatchUpload(files) {
  const items = batchProcessor.addFiles(Array.from(files));
  if (items.length > 0) {
    showBatchQueue();
    showToast('info', 'Files Added', `${items.length} file(s) added to queue`);
    logger?.log('Batch', 'Files added', { count: items.length });
  }
}

function showBatchQueue() {
  const queueEl = document.getElementById('stt-batch-queue');
  if (queueEl) {
    queueEl.classList.remove('hidden');
  }
}

function hideBatchQueue() {
  const queueEl = document.getElementById('stt-batch-queue');
  if (queueEl) {
    queueEl.classList.add('hidden');
  }
}

function updateBatchQueueUI() {
  const listEl = document.getElementById('stt-queue-list');
  if (!listEl || !batchProcessor) return;

  const items = batchProcessor.queue.getAll();
  if (items.length === 0) {
    listEl.innerHTML = '<div class="queue-empty">No files in queue</div>';
    return;
  }

  listEl.innerHTML = items.map(item => `
    <div class="queue-item" data-id="${item.id}">
      <div class="queue-item-info">
        <span class="queue-item-name">${escapeHtml(item.name)}</span>
        <span class="queue-item-size">${formatFileSize(item.size)}</span>
      </div>
      <div class="queue-item-status">
        <span class="status-badge status-${item.status}">${item.status}</span>
        ${item.status === 'processing' ? `<span class="progress-text">${item.progress}%</span>` : ''}
      </div>
      <button class="btn btn-icon btn-sm queue-item-remove" data-id="${item.id}" title="Remove">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <line x1="18" y1="6" x2="6" y2="18"/>
          <line x1="6" y1="6" x2="18" y2="18"/>
        </svg>
      </button>
    </div>
  `).join('');

  // Add remove handlers
  listEl.querySelectorAll('.queue-item-remove').forEach(btn => {
    btn.addEventListener('click', (e) => {
      const id = e.currentTarget.dataset.id;
      batchProcessor.queue.remove(id);
    });
  });
}

function updateBatchItemUI(item) {
  const itemEl = document.querySelector(`.queue-item[data-id="${item.id}"]`);
  if (!itemEl) return;

  const statusBadge = itemEl.querySelector('.status-badge');
  if (statusBadge) {
    statusBadge.className = `status-badge status-${item.status}`;
    statusBadge.textContent = item.status;
  }

  const statusArea = itemEl.querySelector('.queue-item-status');
  if (statusArea && item.status === 'processing') {
    let progressEl = statusArea.querySelector('.progress-text');
    if (!progressEl) {
      progressEl = document.createElement('span');
      progressEl.className = 'progress-text';
      statusArea.appendChild(progressEl);
    }
    progressEl.textContent = `${item.progress}%`;
  }

  if (item.status === 'completed') {
    itemEl.classList.add('completed');
  } else if (item.status === 'error') {
    itemEl.classList.add('error');
  }
}

function updateBatchProgress({ completed, total, percent }) {
  logger?.log('Batch', 'Progress', { completed, total, percent });
}

function handleBatchComplete({ total, completed, errors }) {
  if (errors > 0) {
    showToast('warning', 'Batch Complete', `${completed}/${total} files processed, ${errors} error(s)`);
  } else {
    showToast('success', 'Batch Complete', `All ${total} files processed successfully`);
  }
  logger?.log('Batch', 'Batch complete', { total, completed, errors });
}

function formatFileSize(bytes) {
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
  return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
}

function escapeHtml(text) {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}

function applySTTConfig() {
  if (!state.connected) {
    showToast('warning', 'Not Connected', 'Please connect first');
    return;
  }
  wsManager.sendConfig({ stt: getSTTConfig(), tts: getTTSConfig() });
  showToast('success', 'Config Applied', 'STT configuration updated');
  logger?.log('STT', 'Config applied', { provider: getSTTConfig().provider });
}

function clearTranscript() {
  state.transcript = '';
  state.interimTranscript = '';
  const transcriptEl = document.getElementById('stt-transcript');
  if (transcriptEl) {
    transcriptEl.innerHTML = '<p class="transcript-placeholder">Start speaking to see transcription...</p>';
  }
  showToast('info', 'Cleared', 'Transcript cleared');
}

function setupExportButtons() {
  const exportBtns = document.querySelectorAll('.export-btn[data-format]');
  exportBtns.forEach((btn) => {
    btn.addEventListener('click', () => {
      const format = btn.dataset.format;
      exportTranscript(format);
    });
  });
}

function exportTranscript(format) {
  if (!state.transcript) {
    showToast('warning', 'No Content', 'No transcript to export');
    return;
  }

  let content = '';
  let filename = '';
  let mimeType = '';

  switch (format) {
    case 'txt':
      content = state.transcript;
      filename = `transcript-${Date.now()}.txt`;
      mimeType = 'text/plain';
      break;
    case 'json':
      content = JSON.stringify({ transcript: state.transcript, timestamp: Date.now() }, null, 2);
      filename = `transcript-${Date.now()}.json`;
      mimeType = 'application/json';
      break;
    case 'srt':
      content = `1\n00:00:00,000 --> 00:01:00,000\n${state.transcript}\n`;
      filename = `transcript-${Date.now()}.srt`;
      mimeType = 'text/plain';
      break;
    default:
      return;
  }

  downloadFile(content, filename, mimeType);
  showToast('success', 'Exported', `Transcript exported as ${format.toUpperCase()}`);
}

function downloadFile(content, filename, mimeType) {
  const blob = new Blob([content], { type: mimeType });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

function getSTTConfig() {
  return {
    provider: document.getElementById('stt-provider')?.value || 'deepgram',
    language: document.getElementById('stt-language')?.value || 'en-US',
    model: document.getElementById('stt-model')?.value || 'nova-3',
    sample_rate: 16000,
    channels: 1,
    encoding: 'linear16',
    punctuation: document.getElementById('stt-punctuation')?.checked ?? true,
    api_key: document.getElementById('stt-api-key')?.value || undefined,
  };
}

/**
 * TTS Panel
 */
function setupTTS() {
  const loadVoicesBtn = document.getElementById('tts-load-voices');
  const speakBtn = document.getElementById('tts-speak-btn');
  const stopBtn = document.getElementById('tts-stop-btn');
  const applyConfigBtn = document.getElementById('tts-apply-config');

  if (loadVoicesBtn) loadVoicesBtn.addEventListener('click', loadVoices);
  if (speakBtn) speakBtn.addEventListener('click', speak);
  if (stopBtn) stopBtn.addEventListener('click', stopSpeaking);
  if (applyConfigBtn) applyConfigBtn.addEventListener('click', applyTTSConfig);

  // Voice card selection
  setupVoiceCards();

  // Setup SSML Editor
  setupSSMLEditor();
}

/**
 * Setup SSML Editor mode toggle and initialization
 */
function setupSSMLEditor() {
  const textModeBtn = document.getElementById('tts-mode-text');
  const ssmlModeBtn = document.getElementById('tts-mode-ssml');
  const textModeContainer = document.getElementById('tts-text-mode');
  const ssmlModeContainer = document.getElementById('tts-ssml-mode');

  // Mode toggle buttons
  if (textModeBtn) {
    textModeBtn.addEventListener('click', () => {
      switchTTSInputMode('text');
    });
  }

  if (ssmlModeBtn) {
    ssmlModeBtn.addEventListener('click', () => {
      switchTTSInputMode('ssml');
    });
  }

  // Initialize SSML Editor
  if (ssmlModeContainer) {
    ssmlEditor = new SSMLEditor(ssmlModeContainer, {
      autoValidate: true,
    });

    // Transfer content when switching modes
    ssmlEditor.on('change', () => {
      // Keep char count updated
      const charCount = document.getElementById('tts-char-count');
      if (charCount && ttsInputMode === 'ssml') {
        charCount.textContent = ssmlEditor.getPlainText().length;
      }
    });
  }
}

/**
 * Switch between plain text and SSML input modes
 */
function switchTTSInputMode(mode) {
  const textModeBtn = document.getElementById('tts-mode-text');
  const ssmlModeBtn = document.getElementById('tts-mode-ssml');
  const textModeContainer = document.getElementById('tts-text-mode');
  const ssmlModeContainer = document.getElementById('tts-ssml-mode');
  const ttsTextarea = document.getElementById('tts-text');

  ttsInputMode = mode;

  // Update button states
  if (textModeBtn) textModeBtn.classList.toggle('active', mode === 'text');
  if (ssmlModeBtn) ssmlModeBtn.classList.toggle('active', mode === 'ssml');

  // Transfer content between modes
  if (mode === 'ssml') {
    // Switch to SSML mode
    if (textModeContainer) textModeContainer.classList.add('hidden');
    if (ssmlModeContainer) ssmlModeContainer.classList.remove('hidden');

    // Transfer plain text to SSML editor
    if (ssmlEditor && ttsTextarea) {
      const currentText = ttsTextarea.value;
      ssmlEditor.setText(currentText);
    }

    showToast('info', 'SSML Mode', 'SSML editor enabled. Use the toolbar to add tags.');
  } else {
    // Switch to plain text mode
    if (textModeContainer) textModeContainer.classList.remove('hidden');
    if (ssmlModeContainer) ssmlModeContainer.classList.add('hidden');

    // Transfer plain text from SSML editor
    if (ssmlEditor && ttsTextarea) {
      ttsTextarea.value = ssmlEditor.getPlainText();
      // Update char count
      const charCount = document.getElementById('tts-char-count');
      if (charCount) charCount.textContent = ttsTextarea.value.length;
    }
  }
}

function setupVoiceCards() {
  const voiceCards = document.querySelectorAll('.voice-card');
  voiceCards.forEach((card) => {
    card.addEventListener('click', () => {
      voiceCards.forEach((c) => c.classList.remove('selected'));
      card.classList.add('selected');
      const voiceId = card.dataset.voiceId;
      const voiceSelect = document.getElementById('tts-voice');
      if (voiceSelect) voiceSelect.value = voiceId;

      // Auto-apply TTS config when voice is selected
      if (state.connected && wsManager) {
        wsManager.sendConfig({ stt: getSTTConfig(), tts: getTTSConfig() });
        logger?.log('TTS', 'Voice selected, config applied', { voiceId });
      }
    });

    // Preview button
    const previewBtn = card.querySelector('.voice-preview-btn');
    if (previewBtn) {
      previewBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        previewVoice(card.dataset.voiceId);
      });
    }
  });
}

async function loadVoices() {
  const provider = document.getElementById('tts-provider')?.value;
  const providerApiKey = document.getElementById('tts-api-key')?.value;
  const voiceSelect = document.getElementById('tts-voice');
  const voiceGrid = document.getElementById('tts-voice-grid');

  try {
    const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

    const headers = {};
    if (elements.apiKey.value) headers['Authorization'] = `Bearer ${elements.apiKey.value}`;
    if (providerApiKey) headers['X-Provider-Api-Key'] = providerApiKey;

    const response = await fetch(`${baseUrl}/voices?provider=${provider}`, { headers });

    if (!response.ok) throw new Error(await response.text());

    const voicesResponse = await response.json();

    // API returns { provider: [...voices...] } - extract the array
    const voices = Array.isArray(voicesResponse)
      ? voicesResponse
      : voicesResponse[provider] || Object.values(voicesResponse)[0] || [];

    // Update select dropdown
    if (voiceSelect) {
      voiceSelect.innerHTML = '<option value="">Select voice...</option>';
      voices.forEach((voice) => {
        const option = document.createElement('option');
        option.value = voice.id || voice.voice_id;
        option.textContent = voice.name || voice.id;
        voiceSelect.appendChild(option);
      });
    }

    // Update voice grid
    if (voiceGrid) {
      voiceGrid.innerHTML = voices.slice(0, 8).map((voice) => `
        <div class="voice-card" data-voice-id="${voice.id || voice.voice_id}">
          <div class="voice-card-header">
            <div class="voice-avatar">${(voice.name || 'V')[0].toUpperCase()}</div>
            <div class="voice-info">
              <h4>${voice.name || voice.id}</h4>
              <p>${voice.language || provider}</p>
            </div>
          </div>
          <div class="voice-tags">
            ${voice.gender ? `<span class="voice-tag">${voice.gender}</span>` : ''}
            ${voice.accent ? `<span class="voice-tag">${voice.accent}</span>` : ''}
          </div>
          <button class="voice-preview-btn">▶ Preview</button>
          <div class="selected-badge">✓</div>
        </div>
      `).join('');

      setupVoiceCards();
    }

    logger?.log('TTS', 'Voices loaded', { count: voices.length, provider });
    showToast('success', 'Voices Loaded', `${voices.length} voices available`);

  } catch (error) {
    console.error('[WaaV] Failed to load voices:', error);
    showToast('error', 'Load Failed', error.message || 'Failed to load voices');
  }
}

function previewVoice(voiceId) {
  if (!state.connected) {
    showToast('warning', 'Not Connected', 'Please connect first');
    return;
  }

  wsManager.send({
    type: 'speak',
    text: 'Hello! This is a preview of this voice.',
    voice_id: voiceId,
    flush: true,
  });

  showToast('info', 'Preview', `Playing voice preview...`);
}

async function speak() {
  if (!state.connected) {
    showToast('warning', 'Not Connected', 'Please connect first');
    return;
  }

  let text;
  let ssml = null;

  // Get text based on current input mode
  if (ttsInputMode === 'ssml' && ssmlEditor) {
    // Validate SSML first
    const validation = ssmlEditor.validate();
    if (!validation.valid) {
      showToast('error', 'Invalid SSML', validation.errors[0] || 'Please fix SSML errors');
      return;
    }
    ssml = ssmlEditor.getSSML();
    text = ssmlEditor.getPlainText();
  } else {
    text = document.getElementById('tts-text')?.value;
  }

  if (!text && !ssml) {
    showToast('warning', 'No Text', 'Please enter text to speak');
    return;
  }

  state.ttsStartTime = Date.now();
  state.ttsFirstAudio = false;

  // Create TTS session
  currentSession = await sessionHistory.createSession({
    type: 'tts',
    provider: document.getElementById('tts-provider')?.value || 'elevenlabs',
    config: getTTSConfig(),
  });

  // Send with SSML if available
  const message = { type: 'speak', flush: true };
  if (ssml) {
    message.ssml = ssml;
    message.text = text; // Fallback text
  } else {
    message.text = text;
  }
  wsManager.send(message);

  updatePlayerStatus('Speaking...');
  logger?.log('TTS', 'Speak', {
    mode: ttsInputMode,
    text: text.substring(0, 50) + '...',
    hasSSML: !!ssml,
  });
}

function stopSpeaking() {
  if (wsManager) {
    wsManager.send({ type: 'clear' });
  }
  updatePlayerStatus('Stopped');
  showToast('info', 'Stopped', 'Audio playback stopped');
}

function handleTTSAudio(data) {
  if (state.ttsStartTime && !state.ttsFirstAudio) {
    const ttfb = Date.now() - state.ttsStartTime;
    metrics.recordTTSTtfb(ttfb);
    updateQuickMetrics();
    state.ttsFirstAudio = true;
    logger?.log('TTS', 'TTFB', { ttfb: ttfb + 'ms' });

    if (currentSession) {
      sessionHistory.updateMetrics(currentSession.id, { ttfb });
    }
  }

  updatePlayerStatus('Playing audio...');
}

function handleTTSComplete() {
  updatePlayerStatus('Complete');
  logger?.log('TTS', 'Playback complete');

  if (currentSession) {
    sessionHistory.endSession(currentSession.id);
    currentSession = null;
    loadRecentSessions();
  }
}

function updatePlayerStatus(status) {
  const statusEl = document.querySelector('#tts-player .player-status');
  if (statusEl) statusEl.textContent = status;
}

function applyTTSConfig() {
  if (!state.connected) {
    showToast('warning', 'Not Connected', 'Please connect first');
    return;
  }
  wsManager.sendConfig({ stt: getSTTConfig(), tts: getTTSConfig() });
  showToast('success', 'Config Applied', 'TTS configuration updated');
  logger?.log('TTS', 'Config applied', { provider: getTTSConfig().provider });
}

function getTTSConfig() {
  const provider = document.getElementById('tts-provider')?.value || 'elevenlabs';
  const voiceId = document.getElementById('tts-voice')?.value || '';
  const modelInput = document.getElementById('tts-model')?.value;

  // Provider-specific default voices (used when no voice is selected)
  const defaultVoices = {
    deepgram: 'aura-2-apollo-en',
    elevenlabs: 'rachel',
    google: 'en-US-Neural2-J',
    azure: 'en-US-JennyNeural',
    cartesia: 'sonic-english-default',
    openai: 'alloy',
  };

  // Provider-specific model defaults
  // For Deepgram: model = voice name (API expects model=voice_name)
  // For others: use provider-specific model names
  const effectiveVoiceId = voiceId || defaultVoices[provider] || '';
  const defaultModels = {
    deepgram: effectiveVoiceId, // Deepgram API uses voice_id as the model parameter
    elevenlabs: 'eleven_turbo_v2',
    google: 'en-US-Neural2-J',
    azure: 'en-US-JennyNeural',
    cartesia: 'sonic-3',
    openai: 'tts-1',
  };

  return {
    provider,
    voice_id: effectiveVoiceId,
    model: modelInput || defaultModels[provider] || '',
    sample_rate: 24000,
    api_key: document.getElementById('tts-api-key')?.value || undefined,
  };
}

/**
 * A/B Voice Comparison
 */
function setupABComparison() {
  // Use compare-* IDs to match the HTML structure
  const generateBothBtn = document.getElementById('compare-both');
  const generateABtn = document.getElementById('compare-generate-a');
  const generateBBtn = document.getElementById('compare-generate-b');
  const playABtn = document.getElementById('compare-play-a');
  const playBBtn = document.getElementById('compare-play-b');
  const blindModeCheckbox = document.getElementById('compare-blind-mode');

  console.log('[A/B Setup] generateABtn found:', !!generateABtn, 'generateBBtn found:', !!generateBBtn);

  if (generateBothBtn) {
    generateBothBtn.addEventListener('click', generateABComparison);
  }

  if (generateABtn) {
    generateABtn.addEventListener('click', () => {
      console.log('[A/B] Generate A clicked');
      generateSingleVoice('a');
    });
  }
  if (generateBBtn) {
    generateBBtn.addEventListener('click', () => {
      console.log('[A/B] Generate B clicked');
      generateSingleVoice('b');
    });
  }
  if (playABtn) playABtn.addEventListener('click', () => playABSample('a'));
  if (playBBtn) playBBtn.addEventListener('click', () => playABSample('b'));

  if (blindModeCheckbox) {
    blindModeCheckbox.addEventListener('change', () => {
      const isBlind = blindModeCheckbox.checked;
      document.querySelectorAll('.compare-voice-name').forEach((el) => {
        el.style.visibility = isBlind ? 'hidden' : 'visible';
      });
    });
  }

  // Populate voice dropdowns when provider changes
  const providerA = document.getElementById('compare-provider-a');
  const providerB = document.getElementById('compare-provider-b');

  if (providerA) {
    providerA.addEventListener('change', () => loadCompareVoices('a', providerA.value));
  }
  if (providerB) {
    providerB.addEventListener('change', () => loadCompareVoices('b', providerB.value));
  }
}

async function generateABComparison() {
  const text = document.getElementById('compare-text')?.value;
  if (!text) {
    showToast('warning', 'No Text', 'Please enter text for comparison');
    return;
  }

  if (!state.connected) {
    showToast('warning', 'Not Connected', 'Please connect first');
    return;
  }

  showToast('info', 'Generating', 'Generating both voice samples...');

  // Generate both voices in parallel
  await Promise.all([
    generateSingleVoice('a'),
    generateSingleVoice('b'),
  ]);

  showToast('success', 'Complete', 'Both voice samples generated');
}

async function generateSingleVoice(side) {
  const text = document.getElementById('compare-text')?.value;
  const provider = document.getElementById(`compare-provider-${side}`)?.value || 'deepgram';
  const voiceId = document.getElementById(`compare-voice-${side}-select`)?.value;
  const audioEl = document.getElementById(`compare-audio-${side}`);
  const ttfbEl = document.getElementById(`compare-ttfb-${side}`);
  const durationEl = document.getElementById(`compare-duration-${side}`);

  if (!text) {
    showToast('warning', 'No Text', 'Please enter text for comparison');
    return;
  }

  if (!voiceId) {
    showToast('warning', 'No Voice', `Please select a voice for Side ${side.toUpperCase()}`);
    return;
  }

  // Note: A/B Compare uses REST API (/speak), no WebSocket connection required

  const startTime = Date.now();

  try {
    // Use REST API to generate audio
    const serverUrl = elements.serverUrl?.value || 'ws://localhost:3001/ws';
    const baseUrl = serverUrl.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');
    const apiKey = document.getElementById('api-key')?.value;

    const response = await fetch(`${baseUrl}/speak`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(apiKey && { 'Authorization': `Bearer ${apiKey}` }),
      },
      body: JSON.stringify({
        text,
        tts_config: {
          provider,
          voice_id: voiceId,
          model: voiceId, // For most providers, model matches voice_id
        },
      }),
    });

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }

    const ttfb = Date.now() - startTime;
    if (ttfbEl) ttfbEl.textContent = ttfb;

    const audioBlob = await response.blob();
    const audioUrl = URL.createObjectURL(audioBlob);

    if (audioEl) {
      audioEl.src = audioUrl;
      audioEl.onloadedmetadata = () => {
        if (durationEl) durationEl.textContent = audioEl.duration.toFixed(2);
      };
    }

    logger?.log('A/B Compare', `Generated ${side.toUpperCase()}`, { provider, voiceId, ttfb });
  } catch (error) {
    showToast('error', 'Generation Failed', error.message);
    logger?.log('ERROR', `A/B Compare ${side.toUpperCase()} failed`, { error: error.message });
  }
}

function playABSample(side) {
  const audioEl = document.getElementById(`compare-audio-${side}`);
  if (audioEl && audioEl.src) {
    audioEl.play();
  } else {
    showToast('warning', 'No Audio', `Generate audio for Side ${side.toUpperCase()} first`);
  }
}

async function loadCompareVoices(side, provider) {
  const voiceSelect = document.getElementById(`compare-voice-${side}-select`);
  if (!voiceSelect) return;

  voiceSelect.innerHTML = '<option value="">Loading voices...</option>';

  try {
    const serverUrl = elements.serverUrl?.value || 'ws://localhost:3001/ws';
    const baseUrl = serverUrl.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');
    const apiKey = document.getElementById('api-key')?.value;

    const response = await fetch(`${baseUrl}/voices?provider=${provider}`, {
      headers: {
        ...(apiKey && { 'Authorization': `Bearer ${apiKey}` }),
      },
    });

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }

    const data = await response.json();
    // API returns { provider: [...] } format, e.g., { deepgram: [...] }
    const voices = data[provider] || data.voices || [];

    voiceSelect.innerHTML = '<option value="">Select voice...</option>';
    voices.forEach((voice) => {
      const option = document.createElement('option');
      option.value = voice.voice_id || voice.id || voice.name;
      option.textContent = voice.name || voice.voice_id || voice.id;
      voiceSelect.appendChild(option);
    });

    logger?.log('A/B Compare', `Loaded ${voices.length} voices for ${side.toUpperCase()}`, { provider });
  } catch (error) {
    voiceSelect.innerHTML = '<option value="">Failed to load voices</option>';
    logger?.log('ERROR', `Failed to load voices for ${side}`, { error: error.message });
  }
}

/**
 * LiveKit Panel
 */
function setupLiveKit() {
  const generateTokenBtn = document.getElementById('lk-generate-token');
  const listRoomsBtn = document.getElementById('lk-list-rooms');

  if (generateTokenBtn) generateTokenBtn.addEventListener('click', generateLiveKitToken);
  if (listRoomsBtn) listRoomsBtn.addEventListener('click', listLiveKitRooms);
}

async function generateLiveKitToken() {
  const roomName = document.getElementById('lk-room')?.value;
  const identity = document.getElementById('lk-identity')?.value;
  const name = document.getElementById('lk-name')?.value;

  if (!roomName || !identity) {
    showToast('warning', 'Missing Fields', 'Room name and identity are required');
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
    document.getElementById('lk-token-result').textContent = JSON.stringify(data, null, 2);
    logger?.log('LiveKit', 'Token generated', { room: roomName });
    showToast('success', 'Token Generated', 'LiveKit token created successfully');

  } catch (error) {
    console.error('[WaaV] Failed to generate token:', error);
    showToast('error', 'Generation Failed', error.message);
  }
}

async function listLiveKitRooms() {
  try {
    const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

    const response = await fetch(`${baseUrl}/livekit/rooms`, {
      headers: elements.apiKey.value ? { Authorization: `Bearer ${elements.apiKey.value}` } : {},
    });

    const data = await response.json();
    document.getElementById('lk-rooms-result').textContent = JSON.stringify(data, null, 2);
    logger?.log('LiveKit', 'Rooms listed', { count: data.length });

  } catch (error) {
    console.error('[WaaV] Failed to list rooms:', error);
    showToast('error', 'List Failed', error.message);
  }
}

/**
 * SIP Panel
 */
function setupSIP() {
  const createHookBtn = document.getElementById('sip-create-hook');
  const listHooksBtn = document.getElementById('sip-list-hooks');

  if (createHookBtn) createHookBtn.addEventListener('click', createSIPHook);
  if (listHooksBtn) listHooksBtn.addEventListener('click', listSIPHooks);
}

async function createSIPHook() {
  const host = document.getElementById('sip-host')?.value;
  const webhookUrl = document.getElementById('sip-webhook')?.value;

  if (!host || !webhookUrl) {
    showToast('warning', 'Missing Fields', 'Host and webhook URL are required');
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

    await response.json();
    showToast('success', 'Hook Created', 'SIP hook created successfully');
    listSIPHooks();
    logger?.log('SIP', 'Hook created', { host });

  } catch (error) {
    console.error('[WaaV] Failed to create hook:', error);
    showToast('error', 'Creation Failed', error.message);
  }
}

async function listSIPHooks() {
  try {
    const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

    const response = await fetch(`${baseUrl}/sip/hooks`, {
      headers: elements.apiKey.value ? { Authorization: `Bearer ${elements.apiKey.value}` } : {},
    });

    const data = await response.json();
    document.getElementById('sip-hooks-result').textContent = JSON.stringify(data, null, 2);
    logger?.log('SIP', 'Hooks listed', { count: data.length });

  } catch (error) {
    console.error('[WaaV] Failed to list hooks:', error);
    showToast('error', 'List Failed', error.message);
  }
}

/**
 * API Explorer
 */
function setupAPIExplorer() {
  const sendBtn = document.getElementById('api-send');
  if (sendBtn) sendBtn.addEventListener('click', sendAPIRequest);
}

async function sendAPIRequest() {
  const method = document.getElementById('api-method')?.value || 'GET';
  const endpoint = document.getElementById('api-endpoint')?.value || '/health';
  const bodyText = document.getElementById('api-body')?.value;

  const baseUrl = elements.serverUrl.value.replace('ws://', 'http://').replace('wss://', 'https://').replace('/ws', '');

  const options = {
    method,
    headers: {
      'Content-Type': 'application/json',
      ...(elements.apiKey.value ? { Authorization: `Bearer ${elements.apiKey.value}` } : {}),
    },
  };

  if (bodyText && (method === 'POST' || method === 'PUT')) {
    options.body = bodyText;
  }

  const startTime = Date.now();

  try {
    const response = await fetch(baseUrl + endpoint, options);
    const duration = Date.now() - startTime;

    let data;
    const contentType = response.headers.get('content-type');
    if (contentType?.includes('application/json')) {
      data = await response.json();
    } else {
      data = await response.text();
    }

    document.getElementById('api-response').textContent = JSON.stringify(data, null, 2);
    logger?.log(method, endpoint, { status: response.status, duration: duration + 'ms' });

  } catch (error) {
    console.error('[WaaV] API request failed:', error);
    document.getElementById('api-response').textContent = 'Error: ' + error.message;
    showToast('error', 'Request Failed', error.message);
  }
}

/**
 * WS Debug
 */
function setupWSDebug() {
  const templateSelect = document.getElementById('ws-template');
  const messageTextarea = document.getElementById('ws-message');
  const sendBtn = document.getElementById('ws-send');

  if (templateSelect) {
    templateSelect.addEventListener('change', () => {
      const template = templateSelect.value;
      if (template && messageTextarea) {
        messageTextarea.value = getWSTemplate(template);
      }
    });
  }

  if (sendBtn) {
    sendBtn.addEventListener('click', () => {
      if (messageTextarea) sendWSMessage(messageTextarea.value);
    });
  }
}

function getWSTemplate(name) {
  const templates = {
    config: JSON.stringify({
      type: 'config',
      audio: true,
      stt_config: { provider: 'deepgram', language: 'en-US', model: 'nova-3' },
    }, null, 2),
    speak: JSON.stringify({ type: 'speak', text: 'Hello, world!', flush: true }, null, 2),
    clear: JSON.stringify({ type: 'clear' }, null, 2),
    ping: JSON.stringify({ type: 'ping', timestamp: Date.now() }, null, 2),
  };
  return templates[name] || '';
}

function sendWSMessage(messageText) {
  if (!state.connected) {
    showToast('warning', 'Not Connected', 'Please connect first');
    return;
  }

  try {
    const message = JSON.parse(messageText);
    wsManager.send(message);
    addWSLogEntry('out', message);
    logger?.log('WS', 'Sent', { type: message.type });
  } catch (error) {
    showToast('error', 'Invalid JSON', error.message);
  }
}

function addWSLogEntry(direction, data) {
  const logEl = document.getElementById('ws-log');
  if (!logEl) return;

  const time = new Date().toLocaleTimeString();
  const entry = document.createElement('div');
  entry.className = 'message';
  entry.innerHTML = `
    <span class="message-time">${time}</span>
    <span class="message-direction ${direction}">${direction === 'in' ? '←' : '→'}</span>
    <span class="message-content">${JSON.stringify(data, null, 2)}</span>
  `;

  logEl.insertBefore(entry, logEl.firstChild);

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

  if (refreshBtn) refreshBtn.addEventListener('click', refreshAudioDevices);
  if (testRecordBtn) testRecordBtn.addEventListener('click', testRecord);

  refreshAudioDevices();
}

async function refreshAudioDevices() {
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    const audioInputs = devices.filter((d) => d.kind === 'audioinput');

    const select = document.getElementById('audio-input-device');
    if (!select) return;

    select.innerHTML = '';
    audioInputs.forEach((device) => {
      const option = document.createElement('option');
      option.value = device.deviceId;
      option.textContent = device.label || `Microphone ${select.children.length + 1}`;
      select.appendChild(option);
    });

  } catch (error) {
    console.error('[WaaV] Failed to enumerate devices:', error);
  }
}

async function testRecord() {
  const btn = document.getElementById('audio-test-record');
  if (!btn) return;

  btn.disabled = true;
  btn.textContent = 'Recording...';

  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });

    setTimeout(() => {
      stream.getTracks().forEach((t) => t.stop());
      btn.disabled = false;
      btn.textContent = 'Record 5s';
      const playBtn = document.getElementById('audio-test-play');
      if (playBtn) playBtn.disabled = false;
      showToast('success', 'Test Complete', 'Audio test recording completed');
    }, 5000);

  } catch (error) {
    console.error('[WaaV] Failed to record:', error);
    btn.disabled = false;
    btn.textContent = 'Record 5s';
    showToast('error', 'Microphone Error', error.message);
  }
}

/**
 * Metrics Panel
 */
function setupMetrics() {
  const resetBtn = document.getElementById('metrics-reset');
  const exportBtn = document.getElementById('metrics-export');

  if (resetBtn) {
    resetBtn.addEventListener('click', () => {
      metrics.reset();
      updateMetricsDisplay();
      showToast('info', 'Reset', 'Metrics have been reset');
      logger?.log('Metrics', 'Reset');
    });
  }

  if (exportBtn) exportBtn.addEventListener('click', exportMetrics);

  // Initialize sparklines for metrics
  initMetricsSparklines();

  setInterval(updateMetricsDisplay, 1000);
}

function updateMetricsDisplay() {
  const data = metrics.getSummary();

  const setMetric = (id, value) => {
    const el = document.getElementById(id);
    if (el) el.textContent = value ? Math.round(value) : '-';
  };

  setMetric('metric-stt-ttft', data.sttTtft?.p95);
  setMetric('metric-tts-ttfb', data.ttsTtfb?.p95);
  setMetric('metric-e2e', data.e2e?.p95);
  setMetric('metric-ws-connect', data.wsConnect);

  // Update SLO status
  updateSLOStatus('slo-stt', data.sttTtft?.p95, 200);
  updateSLOStatus('slo-tts', data.ttsTtfb?.p95, 150);
  updateSLOStatus('slo-e2e', data.e2e?.p95, 1000);

  // Update sparklines
  updateMetricsSparklines(data);
}

function initMetricsSparklines() {
  const sparklineConfig = {
    lineWidth: 1.5,
    maxPoints: 30,
    type: 'area',
    showDot: true,
    dotRadius: 2,
  };

  const sparklineConfigs = {
    'stt-ttft': {
      canvas: document.getElementById('sparkline-stt-ttft'),
      strokeColor: '#6366f1',
      fillColor: 'rgba(99, 102, 241, 0.15)',
    },
    'tts-ttfb': {
      canvas: document.getElementById('sparkline-tts-ttfb'),
      strokeColor: '#8b5cf6',
      fillColor: 'rgba(139, 92, 246, 0.15)',
    },
    'e2e': {
      canvas: document.getElementById('sparkline-e2e'),
      strokeColor: '#10b981',
      fillColor: 'rgba(16, 185, 129, 0.15)',
    },
  };

  Object.entries(sparklineConfigs).forEach(([key, config]) => {
    if (config.canvas) {
      const container = config.canvas.parentElement;
      if (container) {
        // Remove existing canvas (we'll let Sparkline create its own)
        config.canvas.remove();
        metricsSparklines[key] = new Sparkline(container, {
          ...sparklineConfig,
          strokeColor: config.strokeColor,
          fillColor: config.fillColor,
          dotColor: config.strokeColor,
        });
      }
    }
  });
}

function updateMetricsSparklines(data) {
  if (metricsSparklines['stt-ttft'] && data.sttTtft?.last) {
    metricsSparklines['stt-ttft'].addPoint(data.sttTtft.last);
  }
  if (metricsSparklines['tts-ttfb'] && data.ttsTtfb?.last) {
    metricsSparklines['tts-ttfb'].addPoint(data.ttsTtfb.last);
  }
  if (metricsSparklines['e2e'] && data.e2e?.last) {
    metricsSparklines['e2e'].addPoint(data.e2e.last);
  }
}

function updateSLOStatus(elementId, value, threshold) {
  const el = document.getElementById(elementId);
  if (!el) return;

  const statusEl = el.querySelector('.slo-status');

  if (!value) {
    el.className = 'slo-item';
    if (statusEl) statusEl.textContent = '-';
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
  if (elements.quickTtft) {
    elements.quickTtft.textContent = data.sttTtft?.last ? Math.round(data.sttTtft.last) + 'ms' : '-';
  }
  if (elements.quickTtfb) {
    elements.quickTtfb.textContent = data.ttsTtfb?.last ? Math.round(data.ttsTtfb.last) + 'ms' : '-';
  }
}

function exportMetrics() {
  const data = metrics.export();
  downloadFile(data, `waav-metrics-${Date.now()}.csv`, 'text/csv');
  showToast('success', 'Exported', 'Metrics exported to CSV');
}

/**
 * Keyboard Shortcuts
 */
function setupKeyboardShortcuts() {
  const handlers = {
    openCommandPalette: () => commandPalette?.toggle(),
    connect: () => connect(),
    disconnect: () => disconnect(),
    startRecording: () => startRecording(),
    stopRecording: () => stopRecording(),
    speak: () => speak(),
    toggleTheme: () => toggleTheme(),
    toggleSidebar: () => elements.sidebar?.classList.toggle('collapsed'),
    showHelp: () => showShortcutsModal(),
  };

  keyboardManager = new KeyboardManager(state, handlers);
  commandPalette = new CommandPalette(keyboardManager, state, handlers);
}

function showShortcutsModal() {
  if (elements.shortcutsModal) {
    elements.shortcutsModal.classList.add('active');
  }

  // Close button
  const closeBtn = elements.shortcutsModal?.querySelector('.modal-close');
  if (closeBtn) {
    closeBtn.addEventListener('click', () => {
      elements.shortcutsModal.classList.remove('active');
    });
  }

  // Click outside to close
  const backdrop = elements.shortcutsModal?.querySelector('.modal-backdrop');
  if (backdrop) {
    backdrop.addEventListener('click', () => {
      elements.shortcutsModal.classList.remove('active');
    });
  }
}

/**
 * Theme
 */
function setupTheme() {
  const savedTheme = localStorage.getItem('theme') || 'light';
  document.documentElement.dataset.theme = savedTheme;
  state.theme = savedTheme;

  if (elements.themeToggle) {
    elements.themeToggle.addEventListener('click', toggleTheme);
  }
}

function toggleTheme() {
  const current = document.documentElement.dataset.theme;
  const next = current === 'dark' ? 'light' : 'dark';
  document.documentElement.dataset.theme = next;
  localStorage.setItem('theme', next);
  state.theme = next;
}

/**
 * Load initial state
 */
function loadInitialState() {
  // Set initial connection status
  updateConnectionStatus('disconnected');

  // Apply saved tab
  if (state.currentTab) {
    switchTab(state.currentTab);
  } else {
    switchTab('home');
  }
}

/**
 * Toast Notifications
 */
function showToast(type, title, message) {
  if (!elements.toastContainer) return;

  const toast = document.createElement('div');
  toast.className = `toast toast-${type}`;

  const icons = {
    success: '✓',
    error: '✕',
    warning: '⚠',
    info: 'ℹ',
  };

  toast.innerHTML = `
    <span class="toast-icon">${icons[type] || 'ℹ'}</span>
    <div class="toast-content">
      <div class="toast-title">${title}</div>
      <div class="toast-message">${message}</div>
    </div>
    <button class="toast-close">✕</button>
  `;

  elements.toastContainer.appendChild(toast);

  // Close button
  toast.querySelector('.toast-close').addEventListener('click', () => {
    toast.remove();
  });

  // Auto-dismiss after 5 seconds
  setTimeout(() => {
    toast.style.animation = 'slideIn 0.3s ease reverse';
    setTimeout(() => toast.remove(), 300);
  }, 5000);
}

/**
 * Utility functions
 */
function formatTimeAgo(timestamp) {
  const seconds = Math.floor((Date.now() - timestamp) / 1000);
  if (seconds < 60) return 'Just now';
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

function formatDuration(ms) {
  if (!ms) return '-';
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60000)}:${String(Math.floor((ms % 60000) / 1000)).padStart(2, '0')}`;
}

// Initialize when DOM is ready
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', init);
} else {
  init();
}

// Export for testing
export {
  state,
  metrics,
  sessionHistory,
  connect,
  disconnect,
  startRecording,
  stopRecording,
  speak,
  showToast,
};

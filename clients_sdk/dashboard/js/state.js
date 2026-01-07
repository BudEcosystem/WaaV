/**
 * WaaV Dashboard State Management
 * Enhanced state management with event emitters and persistence
 */

export class State {
  constructor() {
    // Connection state
    this._connected = false;
    this._connectionDetails = null;
    this._reconnectAttempts = 0;

    // Recording state
    this._recording = false;
    this._playing = false;

    // Transcript state
    this._transcript = '';
    this._interimTranscript = '';

    // STT timing
    this._sttStartTime = null;
    this._sttFirstResult = false;
    this._sttProvider = 'deepgram';

    // TTS state
    this._ttsStartTime = null;
    this._ttsFirstAudio = false;
    this._ttsQueue = [];

    // Navigation state
    this._currentTab = 'home';
    this._sidebarCollapsed = false;

    // Environment
    this._environment = 'development';
    this._theme = 'light';

    // Event listeners
    this._listeners = new Map();

    // Load persisted preferences
    this.loadPreferences();
  }

  // Connection getters/setters
  get connected() { return this._connected; }
  set connected(value) {
    this._connected = value;
    this._emit('connectionChange', value);
  }

  setConnected(value) {
    this.connected = value;
  }

  get connectionDetails() { return this._connectionDetails; }
  set connectionDetails(value) {
    this._connectionDetails = value;
    this._emit('connectionDetailsChange', value);
  }

  get reconnectAttempts() { return this._reconnectAttempts; }
  set reconnectAttempts(value) {
    this._reconnectAttempts = value;
    this._emit('reconnectAttemptsChange', value);
  }

  // Recording getters/setters
  get recording() { return this._recording; }
  set recording(value) {
    this._recording = value;
    this._emit('recordingChange', value);
  }

  setRecording(value) {
    this.recording = value;
  }

  get playing() { return this._playing; }
  set playing(value) {
    this._playing = value;
    this._emit('playingChange', value);
  }

  // Transcript getters/setters
  get transcript() { return this._transcript; }
  set transcript(value) {
    this._transcript = value;
    this._emit('transcriptChange', value);
  }

  get interimTranscript() { return this._interimTranscript; }
  set interimTranscript(value) {
    this._interimTranscript = value;
    this._emit('interimTranscriptChange', value);
  }

  // STT getters/setters
  get sttStartTime() { return this._sttStartTime; }
  set sttStartTime(value) {
    this._sttStartTime = value;
    this._sttFirstResult = false;
  }

  get sttFirstResult() { return this._sttFirstResult; }
  set sttFirstResult(value) { this._sttFirstResult = value; }

  get sttProvider() { return this._sttProvider; }
  set sttProvider(value) {
    this._sttProvider = value;
    this._emit('sttProviderChange', value);
  }

  // TTS getters/setters
  get ttsStartTime() { return this._ttsStartTime; }
  set ttsStartTime(value) {
    this._ttsStartTime = value;
    this._ttsFirstAudio = false;
  }

  get ttsFirstAudio() { return this._ttsFirstAudio; }
  set ttsFirstAudio(value) { this._ttsFirstAudio = value; }

  get ttsQueue() { return this._ttsQueue; }
  set ttsQueue(value) {
    this._ttsQueue = value;
    this._emit('ttsQueueChange', value);
  }

  // Navigation getters/setters
  get currentTab() { return this._currentTab; }
  set currentTab(value) {
    const oldTab = this._currentTab;
    this._currentTab = value;
    this._emit('tabChange', value, oldTab);
  }

  setCurrentTab(value) {
    this.currentTab = value;
  }

  get sidebarCollapsed() { return this._sidebarCollapsed; }
  set sidebarCollapsed(value) {
    this._sidebarCollapsed = value;
    this._emit('sidebarChange', value);
    this.savePreferences();
  }

  setSidebarCollapsed(value) {
    this.sidebarCollapsed = value;
  }

  // Environment getters/setters
  get environment() { return this._environment; }
  set environment(value) {
    this._environment = value;
    this._emit('environmentChange', value);
    this.savePreferences();
  }

  get theme() { return this._theme; }
  set theme(value) {
    this._theme = value;
    this._emit('themeChange', value);
    this.savePreferences();
  }

  // Event emitter methods
  on(event, callback) {
    if (!this._listeners.has(event)) {
      this._listeners.set(event, new Set());
    }
    this._listeners.get(event).add(callback);
    return () => this.off(event, callback);
  }

  off(event, callback) {
    if (this._listeners.has(event)) {
      this._listeners.get(event).delete(callback);
    }
  }

  _emit(event, ...args) {
    if (this._listeners.has(event)) {
      this._listeners.get(event).forEach(callback => {
        try {
          callback(...args);
        } catch (error) {
          console.error(`Error in event listener for ${event}:`, error);
        }
      });
    }
  }

  // Persistence methods
  savePreferences() {
    const preferences = {
      sidebarCollapsed: this._sidebarCollapsed,
      theme: this._theme,
      environment: this._environment,
      sttProvider: this._sttProvider,
    };
    try {
      localStorage.setItem('waav_preferences', JSON.stringify(preferences));
    } catch (error) {
      console.warn('Failed to save preferences:', error);
    }
  }

  loadPreferences() {
    try {
      const stored = localStorage.getItem('waav_preferences');
      if (stored) {
        const preferences = JSON.parse(stored);
        if (preferences.sidebarCollapsed !== undefined) {
          this._sidebarCollapsed = preferences.sidebarCollapsed;
        }
        if (preferences.theme) {
          this._theme = preferences.theme;
        }
        if (preferences.environment) {
          this._environment = preferences.environment;
        }
        if (preferences.sttProvider) {
          this._sttProvider = preferences.sttProvider;
        }
      }
    } catch (error) {
      console.warn('Failed to load preferences:', error);
    }
  }

  // Reset state
  reset() {
    this._connected = false;
    this._connectionDetails = null;
    this._reconnectAttempts = 0;
    this._recording = false;
    this._playing = false;
    this._transcript = '';
    this._interimTranscript = '';
    this._sttStartTime = null;
    this._sttFirstResult = false;
    this._ttsStartTime = null;
    this._ttsFirstAudio = false;
    this._ttsQueue = [];
    this._currentTab = 'home';

    this._emit('reset');
  }

  // Get full state snapshot
  getSnapshot() {
    return {
      connected: this._connected,
      connectionDetails: this._connectionDetails,
      reconnectAttempts: this._reconnectAttempts,
      recording: this._recording,
      playing: this._playing,
      transcript: this._transcript,
      interimTranscript: this._interimTranscript,
      sttStartTime: this._sttStartTime,
      sttFirstResult: this._sttFirstResult,
      sttProvider: this._sttProvider,
      ttsStartTime: this._ttsStartTime,
      ttsFirstAudio: this._ttsFirstAudio,
      ttsQueue: this._ttsQueue,
      currentTab: this._currentTab,
      sidebarCollapsed: this._sidebarCollapsed,
      environment: this._environment,
      theme: this._theme,
    };
  }
}

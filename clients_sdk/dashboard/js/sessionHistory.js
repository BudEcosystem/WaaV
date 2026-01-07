/**
 * WaaV Dashboard Session History
 * Stores and manages session history using IndexedDB
 */

const DB_NAME = 'waav_dashboard';
const DB_VERSION = 1;
const STORE_NAME = 'sessions';

export class SessionHistory {
  constructor() {
    this.db = null;
    this._initPromise = this._initDB();
  }

  /**
   * Initialize IndexedDB
   */
  async _initDB() {
    return new Promise((resolve, reject) => {
      const request = indexedDB.open(DB_NAME, DB_VERSION);

      request.onerror = () => {
        console.error('Failed to open IndexedDB:', request.error);
        reject(request.error);
      };

      request.onsuccess = () => {
        this.db = request.result;
        resolve(this.db);
      };

      request.onupgradeneeded = (event) => {
        const db = event.target.result;

        // Create sessions object store
        if (!db.objectStoreNames.contains(STORE_NAME)) {
          const store = db.createObjectStore(STORE_NAME, { keyPath: 'id' });

          // Create indexes for querying
          store.createIndex('type', 'type', { unique: false });
          store.createIndex('provider', 'provider', { unique: false });
          store.createIndex('startTime', 'startTime', { unique: false });
          store.createIndex('status', 'status', { unique: false });
        }
      };
    });
  }

  /**
   * Ensure DB is initialized
   */
  async _ensureDB() {
    if (!this.db) {
      await this._initPromise;
    }
    return this.db;
  }

  /**
   * Generate unique session ID
   */
  _generateId() {
    return `sess_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
  }

  /**
   * Create a new session
   */
  async createSession(options = {}) {
    await this._ensureDB();

    const session = {
      id: this._generateId(),
      type: options.type || 'stt',
      provider: options.provider || 'unknown',
      config: options.config || {},
      transcript: options.transcript || '',
      audioData: options.audioData || null,
      startTime: options.startTime || Date.now(),
      endTime: null,
      duration: 0,
      status: 'active',
      metrics: {
        ttft: null,
        ttfb: null,
        duration: null,
        audioLength: null,
      },
      metadata: options.metadata || {},
    };

    return new Promise((resolve, reject) => {
      const transaction = this.db.transaction([STORE_NAME], 'readwrite');
      const store = transaction.objectStore(STORE_NAME);
      const request = store.add(session);

      request.onsuccess = () => resolve(session);
      request.onerror = () => reject(request.error);
    });
  }

  /**
   * Get a session by ID
   */
  async getSession(id) {
    await this._ensureDB();

    return new Promise((resolve, reject) => {
      const transaction = this.db.transaction([STORE_NAME], 'readonly');
      const store = transaction.objectStore(STORE_NAME);
      const request = store.get(id);

      request.onsuccess = () => resolve(request.result || null);
      request.onerror = () => reject(request.error);
    });
  }

  /**
   * Update a session
   */
  async updateSession(id, updates) {
    await this._ensureDB();

    const session = await this.getSession(id);
    if (!session) {
      throw new Error(`Session ${id} not found`);
    }

    const updatedSession = { ...session, ...updates };

    return new Promise((resolve, reject) => {
      const transaction = this.db.transaction([STORE_NAME], 'readwrite');
      const store = transaction.objectStore(STORE_NAME);
      const request = store.put(updatedSession);

      request.onsuccess = () => resolve(updatedSession);
      request.onerror = () => reject(request.error);
    });
  }

  /**
   * Update session metrics
   */
  async updateMetrics(id, metrics) {
    const session = await this.getSession(id);
    if (!session) {
      throw new Error(`Session ${id} not found`);
    }

    return this.updateSession(id, {
      metrics: { ...session.metrics, ...metrics },
    });
  }

  /**
   * End a session
   */
  async endSession(id) {
    const session = await this.getSession(id);
    if (!session) {
      throw new Error(`Session ${id} not found`);
    }

    const endTime = Date.now();
    const duration = endTime - session.startTime;

    return this.updateSession(id, {
      endTime,
      duration,
      status: 'completed',
    });
  }

  /**
   * List sessions with optional filters
   */
  async listSessions(filters = {}) {
    await this._ensureDB();

    return new Promise((resolve, reject) => {
      const transaction = this.db.transaction([STORE_NAME], 'readonly');
      const store = transaction.objectStore(STORE_NAME);

      // Use index if filtering by type or provider
      let request;
      if (filters.type) {
        const index = store.index('type');
        request = index.getAll(filters.type);
      } else if (filters.provider) {
        const index = store.index('provider');
        request = index.getAll(filters.provider);
      } else {
        request = store.getAll();
      }

      request.onsuccess = () => {
        let sessions = request.result || [];

        // Apply additional filters
        if (filters.startDate) {
          sessions = sessions.filter(s => s.startTime >= filters.startDate);
        }
        if (filters.endDate) {
          sessions = sessions.filter(s => s.startTime <= filters.endDate);
        }
        if (filters.status) {
          sessions = sessions.filter(s => s.status === filters.status);
        }

        // Sort by startTime descending (most recent first)
        sessions.sort((a, b) => b.startTime - a.startTime);

        // Apply limit
        if (filters.limit) {
          sessions = sessions.slice(0, filters.limit);
        }

        resolve(sessions);
      };

      request.onerror = () => reject(request.error);
    });
  }

  /**
   * Search sessions by transcript content
   */
  async searchSessions(query) {
    const sessions = await this.listSessions();
    const lowerQuery = query.toLowerCase();

    return sessions.filter(session =>
      session.transcript &&
      session.transcript.toLowerCase().includes(lowerQuery)
    );
  }

  /**
   * Delete a session
   */
  async deleteSession(id) {
    await this._ensureDB();

    return new Promise((resolve, reject) => {
      const transaction = this.db.transaction([STORE_NAME], 'readwrite');
      const store = transaction.objectStore(STORE_NAME);
      const request = store.delete(id);

      request.onsuccess = () => resolve(true);
      request.onerror = () => reject(request.error);
    });
  }

  /**
   * Clear all sessions
   */
  async clearAll() {
    await this._ensureDB();

    return new Promise((resolve, reject) => {
      const transaction = this.db.transaction([STORE_NAME], 'readwrite');
      const store = transaction.objectStore(STORE_NAME);
      const request = store.clear();

      request.onsuccess = () => resolve(true);
      request.onerror = () => reject(request.error);
    });
  }

  /**
   * Export a session in various formats
   */
  async exportSession(id, format = 'json') {
    const session = await this.getSession(id);
    if (!session) {
      throw new Error(`Session ${id} not found`);
    }

    switch (format) {
      case 'json':
        return JSON.stringify(session, null, 2);

      case 'text':
        return session.transcript || '';

      case 'srt':
        return this._toSRT(session);

      case 'vtt':
        return this._toVTT(session);

      default:
        throw new Error(`Unknown export format: ${format}`);
    }
  }

  /**
   * Convert session to SRT subtitle format
   */
  _toSRT(session) {
    if (!session.transcript) return '';

    // Simple SRT generation - in real implementation would use word timings
    const lines = [];
    lines.push('1');
    lines.push('00:00:00,000 --> 00:00:' + String(Math.floor(session.duration / 1000)).padStart(2, '0') + ',000');
    lines.push(session.transcript);
    lines.push('');

    return lines.join('\n');
  }

  /**
   * Convert session to VTT subtitle format
   */
  _toVTT(session) {
    if (!session.transcript) return '';

    const lines = [];
    lines.push('WEBVTT');
    lines.push('');
    lines.push('00:00:00.000 --> 00:00:' + String(Math.floor((session.duration || 5000) / 1000)).padStart(2, '0') + '.000');
    lines.push(session.transcript);
    lines.push('');

    return lines.join('\n');
  }

  /**
   * Get session statistics
   */
  async getStatistics(filters = {}) {
    const sessions = await this.listSessions(filters);

    const stats = {
      totalSessions: sessions.length,
      sttSessions: sessions.filter(s => s.type === 'stt').length,
      ttsSessions: sessions.filter(s => s.type === 'tts').length,
      totalDuration: sessions.reduce((sum, s) => sum + (s.duration || 0), 0),
      avgTTFT: 0,
      avgTTFB: 0,
      providers: {},
    };

    // Calculate average metrics
    const ttftValues = sessions
      .filter(s => s.metrics?.ttft)
      .map(s => s.metrics.ttft);
    const ttfbValues = sessions
      .filter(s => s.metrics?.ttfb)
      .map(s => s.metrics.ttfb);

    if (ttftValues.length > 0) {
      stats.avgTTFT = ttftValues.reduce((a, b) => a + b, 0) / ttftValues.length;
    }
    if (ttfbValues.length > 0) {
      stats.avgTTFB = ttfbValues.reduce((a, b) => a + b, 0) / ttfbValues.length;
    }

    // Count by provider
    sessions.forEach(s => {
      stats.providers[s.provider] = (stats.providers[s.provider] || 0) + 1;
    });

    return stats;
  }
}

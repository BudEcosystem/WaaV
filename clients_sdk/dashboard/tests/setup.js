/**
 * Jest Test Setup for WaaV Dashboard
 */

import { jest, beforeAll, afterAll, afterEach } from '@jest/globals';
import '@testing-library/jest-dom';

// Mock localStorage with proper jest functions
const localStorageMock = (() => {
  let store = {};
  return {
    getItem: jest.fn((key) => store[key] || null),
    setItem: jest.fn((key, value) => { store[key] = value; }),
    removeItem: jest.fn((key) => { delete store[key]; }),
    clear: jest.fn(() => { store = {}; }),
    get store() { return store; },
    set store(val) { store = val; },
  };
})();
global.localStorage = localStorageMock;

// Mock sessionStorage
const sessionStorageMock = (() => {
  let store = {};
  return {
    getItem: jest.fn((key) => store[key] || null),
    setItem: jest.fn((key, value) => { store[key] = value; }),
    removeItem: jest.fn((key) => { delete store[key]; }),
    clear: jest.fn(() => { store = {}; }),
  };
})();
global.sessionStorage = sessionStorageMock;

// Mock WebSocket
class MockWebSocket {
  constructor(url) {
    this.url = url;
    this.readyState = MockWebSocket.CONNECTING;
    this.onopen = null;
    this.onclose = null;
    this.onmessage = null;
    this.onerror = null;

    // Auto-connect after a tick
    setTimeout(() => {
      this.readyState = MockWebSocket.OPEN;
      if (this.onopen) this.onopen({ target: this });
    }, 0);
  }

  send(data) {
    this.lastSentData = data;
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
    if (this.onclose) this.onclose({ target: this });
  }

  // Test helper to simulate incoming message
  simulateMessage(data) {
    if (this.onmessage) {
      this.onmessage({ data: JSON.stringify(data) });
    }
  }

  // Test helper to simulate error
  simulateError(error) {
    if (this.onerror) {
      this.onerror(error);
    }
  }
}

MockWebSocket.CONNECTING = 0;
MockWebSocket.OPEN = 1;
MockWebSocket.CLOSING = 2;
MockWebSocket.CLOSED = 3;

global.WebSocket = MockWebSocket;

// Mock MediaDevices
const mockMediaStream = {
  getTracks: () => [{ stop: jest.fn() }],
};

global.navigator.mediaDevices = {
  getUserMedia: jest.fn().mockResolvedValue(mockMediaStream),
  enumerateDevices: jest.fn().mockResolvedValue([
    { kind: 'audioinput', deviceId: 'default', label: 'Default Microphone' },
    { kind: 'audioinput', deviceId: 'mic1', label: 'USB Microphone' },
  ]),
};

// Mock AudioContext
class MockAudioContext {
  constructor(options = {}) {
    this.sampleRate = options.sampleRate || 44100;
    this.state = 'running';
  }

  createAnalyser() {
    return {
      connect: jest.fn(),
      fftSize: 2048,
      frequencyBinCount: 1024,
      getByteFrequencyData: jest.fn(),
      getByteTimeDomainData: jest.fn(),
    };
  }

  createMediaStreamSource() {
    return { connect: jest.fn() };
  }

  createScriptProcessor() {
    return {
      connect: jest.fn(),
      onaudioprocess: null,
    };
  }

  close() {
    this.state = 'closed';
    return Promise.resolve();
  }

  decodeAudioData(buffer) {
    return Promise.resolve({
      sampleRate: this.sampleRate,
      length: 1000,
      numberOfChannels: 1,
      duration: 1,
      getChannelData: () => new Float32Array(1000),
    });
  }
}

global.AudioContext = MockAudioContext;
global.webkitAudioContext = MockAudioContext;

// Mock IndexedDB with proper in-memory storage
const createMockIndexedDB = () => {
  // Shared storage across all mock instances
  const databases = {};

  const createMockRequest = (result, error = null) => {
    const request = {
      result,
      error,
      onsuccess: null,
      onerror: null,
    };
    setTimeout(() => {
      if (error && request.onerror) {
        request.onerror({ target: request });
      } else if (request.onsuccess) {
        request.onsuccess({ target: request });
      }
    }, 0);
    return request;
  };

  const getOrCreateStore = (dbName, storeName) => {
    if (!databases[dbName]) {
      databases[dbName] = { stores: {}, version: 1 };
    }
    if (!databases[dbName].stores[storeName]) {
      databases[dbName].stores[storeName] = { data: {}, indexes: {} };
    }
    return databases[dbName].stores[storeName];
  };

  const createMockObjectStore = (dbName, storeName) => {
    const storeData = getOrCreateStore(dbName, storeName);

    return {
      add: jest.fn((value) => {
        const key = value.id || `id_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
        storeData.data[key] = { ...value };
        return createMockRequest(key);
      }),
      get: jest.fn((key) => {
        return createMockRequest(storeData.data[key] || null);
      }),
      getAll: jest.fn((query) => {
        let results = Object.values(storeData.data);
        if (query !== undefined) {
          results = results.filter(item => {
            return Object.values(item).includes(query);
          });
        }
        return createMockRequest(results);
      }),
      put: jest.fn((value) => {
        const key = value.id || `id_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
        storeData.data[key] = { ...value };
        return createMockRequest(key);
      }),
      delete: jest.fn((key) => {
        delete storeData.data[key];
        return createMockRequest(undefined);
      }),
      clear: jest.fn(() => {
        storeData.data = {};
        return createMockRequest(undefined);
      }),
      createIndex: jest.fn((indexName, keyPath, options) => {
        storeData.indexes[indexName] = { keyPath, options };
      }),
      index: jest.fn((indexName) => ({
        getAll: jest.fn((value) => {
          const index = storeData.indexes[indexName];
          if (!index) return createMockRequest([]);
          const results = Object.values(storeData.data).filter(item =>
            item[index.keyPath] === value
          );
          return createMockRequest(results);
        }),
      })),
    };
  };

  const createMockTransaction = (dbName) => ({
    objectStore: jest.fn((storeName) => createMockObjectStore(dbName, storeName)),
    oncomplete: null,
    onerror: null,
    abort: jest.fn(),
  });

  const createMockDB = (name, version) => {
    if (!databases[name]) {
      databases[name] = { stores: {}, version };
    }
    databases[name].version = version;

    return {
      transaction: jest.fn((storeNames, mode) => createMockTransaction(name)),
      createObjectStore: jest.fn((storeName, options) => {
        getOrCreateStore(name, storeName);
        if (options?.keyPath) {
          databases[name].stores[storeName].keyPath = options.keyPath;
        }
        return createMockObjectStore(name, storeName);
      }),
      objectStoreNames: {
        contains: jest.fn((storeName) => !!(databases[name]?.stores?.[storeName])),
      },
      close: jest.fn(),
      name,
      version,
    };
  };

  return {
    open: jest.fn((name, version) => {
      const request = {
        result: null,
        onsuccess: null,
        onerror: null,
        onupgradeneeded: null,
      };
      setTimeout(() => {
        const needsUpgrade = !databases[name] || databases[name].version < version;
        const db = createMockDB(name, version);
        request.result = db;
        if (request.onupgradeneeded && needsUpgrade) {
          request.onupgradeneeded({ target: { result: db } });
        }
        if (request.onsuccess) {
          request.onsuccess({ target: request });
        }
      }, 0);
      return request;
    }),
    deleteDatabase: jest.fn((name) => {
      delete databases[name];
      return createMockRequest(undefined);
    }),
    _reset: () => {
      Object.keys(databases).forEach(key => delete databases[key]);
    },
    _getStore: (dbName, storeName) => databases[dbName]?.stores?.[storeName]?.data || {},
  };
};

global.indexedDB = createMockIndexedDB();

// Mock URL.createObjectURL
global.URL.createObjectURL = jest.fn(() => 'blob:mock-url');
global.URL.revokeObjectURL = jest.fn();

// Mock ResizeObserver
global.ResizeObserver = class ResizeObserver {
  constructor(callback) {
    this.callback = callback;
  }
  observe() {}
  unobserve() {}
  disconnect() {}
};

// Mock IntersectionObserver
global.IntersectionObserver = class IntersectionObserver {
  constructor(callback) {
    this.callback = callback;
  }
  observe() {}
  unobserve() {}
  disconnect() {}
};

// Console error spy for catching React warnings
const originalError = console.error;
beforeAll(() => {
  console.error = (...args) => {
    if (
      typeof args[0] === 'string' &&
      args[0].includes('Warning:')
    ) {
      return;
    }
    originalError.call(console, ...args);
  };
});

afterAll(() => {
  console.error = originalError;
});

// Clean up after each test
afterEach(() => {
  jest.clearAllMocks();
  document.body.innerHTML = '';
  localStorageMock.store = {};
  sessionStorageMock.clear();
  if (global.indexedDB._reset) {
    global.indexedDB._reset();
  }
});

export { jest };

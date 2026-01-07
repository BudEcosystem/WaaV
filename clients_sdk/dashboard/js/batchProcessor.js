/**
 * Batch Processor Module for WaaV Dashboard
 * Handles batch processing of audio files for STT
 */

/**
 * Represents a single item in the batch queue
 */
export class BatchItem {
  constructor(file) {
    this.id = this.generateId();
    this.file = file;
    this.name = file.name;
    this.size = file.size;
    this.type = file.type;
    this.status = 'pending'; // pending, processing, completed, error
    this.progress = 0;
    this.result = null;
    this.error = null;
    this.startTime = null;
    this.endTime = null;
  }

  /**
   * Generate unique ID
   */
  generateId() {
    return `batch_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
  }

  /**
   * Set item status
   */
  setStatus(status) {
    this.status = status;
    if (status === 'processing') {
      this.startTime = Date.now();
    }
  }

  /**
   * Set item as completed with result
   */
  setCompleted(result) {
    this.status = 'completed';
    this.result = result;
    this.progress = 100;
    this.endTime = Date.now();
  }

  /**
   * Set item as error with message
   */
  setError(message) {
    this.status = 'error';
    this.error = message;
    this.endTime = Date.now();
  }

  /**
   * Update progress (0-100)
   */
  setProgress(progress) {
    this.progress = Math.min(100, Math.max(0, progress));
  }

  /**
   * Get processing duration in ms
   */
  getDuration() {
    if (!this.startTime) return 0;
    const end = this.endTime || Date.now();
    return end - this.startTime;
  }

  /**
   * Convert to plain object for export
   */
  toJSON() {
    return {
      id: this.id,
      filename: this.name,
      size: this.size,
      status: this.status,
      transcript: this.result,
      error: this.error,
      duration: this.getDuration(),
    };
  }
}

/**
 * Batch Queue - manages a collection of batch items
 */
export class BatchQueue {
  constructor() {
    this.items = new Map();
    this.listeners = {};
    this.allowedTypes = [
      'audio/wav', 'audio/wave', 'audio/x-wav',
      'audio/mpeg', 'audio/mp3',
      'audio/ogg', 'audio/vorbis',
      'audio/mp4', 'audio/x-m4a', 'audio/m4a',
      'audio/flac', 'audio/x-flac',
      'audio/webm',
      'audio/aac',
      'audio/aiff', 'audio/x-aiff',
    ];
  }

  /**
   * Get queue size
   */
  get size() {
    return this.items.size;
  }

  /**
   * Check if file type is valid audio
   */
  isValidAudio(file) {
    // Check by MIME type
    if (this.allowedTypes.includes(file.type)) {
      return true;
    }
    // Check by extension as fallback
    const ext = file.name.split('.').pop()?.toLowerCase();
    const validExtensions = ['wav', 'mp3', 'ogg', 'm4a', 'flac', 'webm', 'aac', 'aiff'];
    return validExtensions.includes(ext);
  }

  /**
   * Add a file to the queue
   */
  add(file) {
    if (!this.isValidAudio(file)) {
      throw new Error('Invalid audio file');
    }

    const item = new BatchItem(file);
    this.items.set(item.id, item);
    this.emit('itemAdded', item);
    return item;
  }

  /**
   * Add multiple files
   */
  addMultiple(files) {
    const items = [];
    for (const file of files) {
      try {
        items.push(this.add(file));
      } catch (error) {
        console.warn(`[BatchQueue] Skipping invalid file: ${file.name}`);
      }
    }
    return items;
  }

  /**
   * Remove item by ID
   */
  remove(id) {
    if (this.items.has(id)) {
      this.items.delete(id);
      this.emit('itemRemoved', id);
    }
  }

  /**
   * Clear all items
   */
  clear() {
    this.items.clear();
    this.emit('cleared');
  }

  /**
   * Get item by ID
   */
  get(id) {
    return this.items.get(id) || null;
  }

  /**
   * Get all items
   */
  getAll() {
    return Array.from(this.items.values());
  }

  /**
   * Get pending items
   */
  getPending() {
    return this.getAll().filter(item => item.status === 'pending');
  }

  /**
   * Get completed items
   */
  getCompleted() {
    return this.getAll().filter(item => item.status === 'completed');
  }

  /**
   * Get items with errors
   */
  getErrors() {
    return this.getAll().filter(item => item.status === 'error');
  }

  /**
   * Add event listener
   */
  on(event, handler) {
    if (!this.listeners[event]) {
      this.listeners[event] = [];
    }
    this.listeners[event].push(handler);
  }

  /**
   * Remove event listener
   */
  off(event, handler) {
    if (!this.listeners[event]) return;
    this.listeners[event] = this.listeners[event].filter(h => h !== handler);
  }

  /**
   * Emit event
   */
  emit(event, ...args) {
    if (!this.listeners[event]) return;
    this.listeners[event].forEach(handler => handler(...args));
  }
}

/**
 * Batch Processor - processes queue items with concurrency control
 */
export class BatchProcessor {
  constructor(options = {}) {
    this.options = {
      concurrency: options.concurrency || 2,
      processFn: options.processFn || null,
      ...options,
    };

    this.queue = new BatchQueue();
    this.isProcessing = false;
    this.isStopped = false;
    this.activeCount = 0;
    this.listeners = {};

    // Forward queue events
    this.queue.on('itemAdded', (item) => this.emit('itemAdded', item));
    this.queue.on('itemRemoved', (id) => this.emit('itemRemoved', id));
    this.queue.on('cleared', () => this.emit('queueCleared'));
  }

  /**
   * Add single file to queue
   */
  addFile(file) {
    return this.queue.add(file);
  }

  /**
   * Add multiple files to queue
   */
  addFiles(files) {
    return this.queue.addMultiple(files);
  }

  /**
   * Clear the queue
   */
  clearQueue() {
    this.queue.clear();
  }

  /**
   * Process all pending items
   */
  async processAll() {
    if (this.isProcessing) {
      console.warn('[BatchProcessor] Already processing');
      return;
    }

    this.isProcessing = true;
    this.isStopped = false;
    this.emit('start');

    const pending = this.queue.getPending();
    const total = pending.length;
    let completed = 0;

    // Process with concurrency control using a simple semaphore pattern
    const process = async (item) => {
      if (this.isStopped) return;

      this.activeCount++;
      item.setStatus('processing');
      this.emit('itemStart', item);

      try {
        let result;
        if (this.options.processFn) {
          result = await this.options.processFn(item.file, item);
        } else {
          // Default: simulate processing
          await new Promise(resolve => setTimeout(resolve, 100));
          result = `Processed: ${item.name}`;
        }

        item.setCompleted(result);
        this.emit('itemComplete', item);
      } catch (error) {
        item.setError(error.message || 'Processing failed');
        this.emit('itemError', item, error);
      } finally {
        this.activeCount--;
        completed++;
        this.emit('progress', { completed, total, percent: Math.round((completed / total) * 100) });
      }
    };

    // Create processing pool
    const pool = [];
    for (const item of pending) {
      if (this.isStopped) break;

      // Wait if at concurrency limit
      while (this.activeCount >= this.options.concurrency && !this.isStopped) {
        await new Promise(resolve => setTimeout(resolve, 10));
      }

      if (this.isStopped) break;

      pool.push(process(item));
    }

    // Wait for all to complete
    await Promise.all(pool);

    this.isProcessing = false;
    this.emit('complete', {
      total,
      completed: this.queue.getCompleted().length,
      errors: this.queue.getErrors().length,
    });
  }

  /**
   * Stop processing
   */
  stop() {
    this.isStopped = true;
    this.isProcessing = false;
    this.emit('stopped');
  }

  /**
   * Get all completed results
   */
  getResults() {
    return this.queue.getCompleted();
  }

  /**
   * Export results in specified format
   */
  exportResults(format = 'json') {
    const results = this.getResults();

    switch (format) {
      case 'json':
        return JSON.stringify(results.map(item => ({
          filename: item.name,
          transcript: item.result,
          duration: item.getDuration(),
        })), null, 2);

      case 'text':
        return results.map(item =>
          `=== ${item.name} ===\n${item.result}\n`
        ).join('\n');

      case 'csv':
        const header = 'filename,transcript,duration_ms\n';
        const rows = results.map(item =>
          `"${item.name}","${(item.result || '').replace(/"/g, '""')}",${item.getDuration()}`
        ).join('\n');
        return header + rows;

      default:
        throw new Error(`Unknown export format: ${format}`);
    }
  }

  /**
   * Add event listener
   */
  on(event, handler) {
    if (!this.listeners[event]) {
      this.listeners[event] = [];
    }
    this.listeners[event].push(handler);
  }

  /**
   * Remove event listener
   */
  off(event, handler) {
    if (!this.listeners[event]) return;
    this.listeners[event] = this.listeners[event].filter(h => h !== handler);
  }

  /**
   * Emit event
   */
  emit(event, ...args) {
    if (!this.listeners[event]) return;
    this.listeners[event].forEach(handler => handler(...args));
  }
}

export default BatchProcessor;

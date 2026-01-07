/**
 * Batch Processor Tests
 */

import { jest, describe, test, expect, beforeEach, afterEach } from '@jest/globals';
import { BatchProcessor, BatchItem, BatchQueue } from '../js/batchProcessor.js';

describe('BatchItem', () => {
  describe('creation', () => {
    test('should create item with file', () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = new BatchItem(file);

      expect(item.id).toBeDefined();
      expect(item.file).toBe(file);
      expect(item.name).toBe('test.wav');
      expect(item.status).toBe('pending');
      expect(item.progress).toBe(0);
    });

    test('should generate unique IDs', () => {
      const file1 = new File(['test1'], 'test1.wav', { type: 'audio/wav' });
      const file2 = new File(['test2'], 'test2.wav', { type: 'audio/wav' });
      const item1 = new BatchItem(file1);
      const item2 = new BatchItem(file2);

      expect(item1.id).not.toBe(item2.id);
    });

    test('should track file size', () => {
      const content = 'a'.repeat(1024);
      const file = new File([content], 'test.wav', { type: 'audio/wav' });
      const item = new BatchItem(file);

      expect(item.size).toBe(1024);
    });
  });

  describe('status updates', () => {
    test('should update status to processing', () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = new BatchItem(file);

      item.setStatus('processing');

      expect(item.status).toBe('processing');
    });

    test('should update status to completed with result', () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = new BatchItem(file);

      item.setCompleted('Hello world');

      expect(item.status).toBe('completed');
      expect(item.result).toBe('Hello world');
      expect(item.progress).toBe(100);
    });

    test('should update status to error with message', () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = new BatchItem(file);

      item.setError('Processing failed');

      expect(item.status).toBe('error');
      expect(item.error).toBe('Processing failed');
    });

    test('should update progress', () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = new BatchItem(file);

      item.setProgress(50);

      expect(item.progress).toBe(50);
    });
  });
});

describe('BatchQueue', () => {
  let queue;

  beforeEach(() => {
    queue = new BatchQueue();
  });

  describe('add', () => {
    test('should add single file', () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = queue.add(file);

      expect(queue.size).toBe(1);
      expect(item.name).toBe('test.wav');
    });

    test('should add multiple files', () => {
      const files = [
        new File(['test1'], 'test1.wav', { type: 'audio/wav' }),
        new File(['test2'], 'test2.wav', { type: 'audio/wav' }),
        new File(['test3'], 'test3.wav', { type: 'audio/wav' }),
      ];

      const items = queue.addMultiple(files);

      expect(queue.size).toBe(3);
      expect(items).toHaveLength(3);
    });

    test('should reject non-audio files', () => {
      const file = new File(['test'], 'test.txt', { type: 'text/plain' });

      expect(() => queue.add(file)).toThrow('Invalid audio file');
    });

    test('should accept common audio formats', () => {
      const formats = [
        { name: 'test.wav', type: 'audio/wav' },
        { name: 'test.mp3', type: 'audio/mpeg' },
        { name: 'test.ogg', type: 'audio/ogg' },
        { name: 'test.m4a', type: 'audio/mp4' },
        { name: 'test.flac', type: 'audio/flac' },
        { name: 'test.webm', type: 'audio/webm' },
      ];

      formats.forEach(({ name, type }) => {
        const file = new File(['test'], name, { type });
        expect(() => queue.add(file)).not.toThrow();
      });
    });
  });

  describe('remove', () => {
    test('should remove item by id', () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = queue.add(file);

      queue.remove(item.id);

      expect(queue.size).toBe(0);
    });

    test('should not throw for non-existent id', () => {
      expect(() => queue.remove('non-existent-id')).not.toThrow();
    });
  });

  describe('clear', () => {
    test('should remove all items', () => {
      queue.addMultiple([
        new File(['test1'], 'test1.wav', { type: 'audio/wav' }),
        new File(['test2'], 'test2.wav', { type: 'audio/wav' }),
      ]);

      queue.clear();

      expect(queue.size).toBe(0);
    });
  });

  describe('get', () => {
    test('should return item by id', () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = queue.add(file);

      const retrieved = queue.get(item.id);

      expect(retrieved).toBe(item);
    });

    test('should return null for non-existent id', () => {
      expect(queue.get('non-existent-id')).toBeNull();
    });
  });

  describe('getAll', () => {
    test('should return all items', () => {
      queue.addMultiple([
        new File(['test1'], 'test1.wav', { type: 'audio/wav' }),
        new File(['test2'], 'test2.wav', { type: 'audio/wav' }),
      ]);

      const items = queue.getAll();

      expect(items).toHaveLength(2);
    });
  });

  describe('getPending', () => {
    test('should return only pending items', () => {
      const file1 = new File(['test1'], 'test1.wav', { type: 'audio/wav' });
      const file2 = new File(['test2'], 'test2.wav', { type: 'audio/wav' });
      const item1 = queue.add(file1);
      queue.add(file2);

      item1.setStatus('completed');

      const pending = queue.getPending();

      expect(pending).toHaveLength(1);
      expect(pending[0].name).toBe('test2.wav');
    });
  });

  describe('events', () => {
    test('should emit itemAdded event', () => {
      const handler = jest.fn();
      queue.on('itemAdded', handler);

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      queue.add(file);

      expect(handler).toHaveBeenCalled();
    });

    test('should emit itemRemoved event', () => {
      const handler = jest.fn();
      queue.on('itemRemoved', handler);

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = queue.add(file);
      queue.remove(item.id);

      expect(handler).toHaveBeenCalledWith(item.id);
    });

    test('should emit cleared event', () => {
      const handler = jest.fn();
      queue.on('cleared', handler);

      queue.clear();

      expect(handler).toHaveBeenCalled();
    });
  });
});

describe('BatchProcessor', () => {
  let processor;
  let mockProcessFn;

  beforeEach(() => {
    mockProcessFn = jest.fn().mockResolvedValue('Transcribed text');
    processor = new BatchProcessor({
      processFn: mockProcessFn,
      concurrency: 2,
    });
  });

  afterEach(() => {
    processor.stop();
  });

  describe('initialization', () => {
    test('should create with default options', () => {
      const defaultProcessor = new BatchProcessor();
      expect(defaultProcessor.isProcessing).toBe(false);
    });

    test('should accept concurrency option', () => {
      const customProcessor = new BatchProcessor({ concurrency: 4 });
      expect(customProcessor.options.concurrency).toBe(4);
    });
  });

  describe('queue management', () => {
    test('should add files to queue', () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);

      expect(processor.queue.size).toBe(1);
    });

    test('should add multiple files', () => {
      const files = [
        new File(['test1'], 'test1.wav', { type: 'audio/wav' }),
        new File(['test2'], 'test2.wav', { type: 'audio/wav' }),
      ];
      processor.addFiles(files);

      expect(processor.queue.size).toBe(2);
    });

    test('should clear queue', () => {
      processor.addFile(new File(['test'], 'test.wav', { type: 'audio/wav' }));
      processor.clearQueue();

      expect(processor.queue.size).toBe(0);
    });
  });

  describe('processing', () => {
    test('should process single file', async () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);

      await processor.processAll();

      expect(mockProcessFn).toHaveBeenCalledTimes(1);
    });

    test('should process multiple files', async () => {
      const files = [
        new File(['test1'], 'test1.wav', { type: 'audio/wav' }),
        new File(['test2'], 'test2.wav', { type: 'audio/wav' }),
        new File(['test3'], 'test3.wav', { type: 'audio/wav' }),
      ];
      processor.addFiles(files);

      await processor.processAll();

      expect(mockProcessFn).toHaveBeenCalledTimes(3);
    });

    test('should update item status to completed', async () => {
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = processor.addFile(file);

      await processor.processAll();

      expect(item.status).toBe('completed');
      expect(item.result).toBe('Transcribed text');
    });

    test('should update item status to error on failure', async () => {
      mockProcessFn.mockRejectedValueOnce(new Error('Processing failed'));
      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      const item = processor.addFile(file);

      await processor.processAll();

      expect(item.status).toBe('error');
      expect(item.error).toBe('Processing failed');
    });

    test('should respect concurrency limit', async () => {
      const processOrder = [];
      let activeCount = 0;
      let maxActive = 0;

      mockProcessFn.mockImplementation(async () => {
        activeCount++;
        maxActive = Math.max(maxActive, activeCount);
        processOrder.push('start');
        await new Promise((resolve) => setTimeout(resolve, 10));
        activeCount--;
        processOrder.push('end');
        return 'result';
      });

      const files = Array(5).fill(null).map((_, i) =>
        new File(['test'], `test${i}.wav`, { type: 'audio/wav' })
      );
      processor.addFiles(files);

      await processor.processAll();

      expect(maxActive).toBeLessThanOrEqual(2); // concurrency is 2
    });
  });

  describe('stop', () => {
    test('should stop processing', async () => {
      // Create a processor with concurrency 1 to test stop behavior
      const slowProcessor = new BatchProcessor({
        concurrency: 1,
        processFn: async () => {
          await new Promise(resolve => setTimeout(resolve, 50));
          return 'result';
        },
      });

      const files = [
        new File(['test1'], 'test1.wav', { type: 'audio/wav' }),
        new File(['test2'], 'test2.wav', { type: 'audio/wav' }),
        new File(['test3'], 'test3.wav', { type: 'audio/wav' }),
      ];
      slowProcessor.addFiles(files);

      const processPromise = slowProcessor.processAll();

      // Wait for first file to start processing
      await new Promise(resolve => setTimeout(resolve, 10));
      slowProcessor.stop();

      await processPromise;

      // With stop called early, not all files should be processed
      expect(slowProcessor.isStopped).toBe(true);
      expect(slowProcessor.isProcessing).toBe(false);
    });

    test('should emit stopped event', async () => {
      const handler = jest.fn();
      processor.on('stopped', handler);

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);

      const processPromise = processor.processAll();
      processor.stop();
      await processPromise;

      expect(handler).toHaveBeenCalled();
    });
  });

  describe('events', () => {
    test('should emit start event', async () => {
      const handler = jest.fn();
      processor.on('start', handler);

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);
      await processor.processAll();

      expect(handler).toHaveBeenCalled();
    });

    test('should emit complete event', async () => {
      const handler = jest.fn();
      processor.on('complete', handler);

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);
      await processor.processAll();

      expect(handler).toHaveBeenCalled();
    });

    test('should emit itemStart event', async () => {
      const handler = jest.fn();
      processor.on('itemStart', handler);

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);
      await processor.processAll();

      expect(handler).toHaveBeenCalled();
    });

    test('should emit itemComplete event', async () => {
      const handler = jest.fn();
      processor.on('itemComplete', handler);

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);
      await processor.processAll();

      expect(handler).toHaveBeenCalled();
    });

    test('should emit itemError event on failure', async () => {
      mockProcessFn.mockRejectedValueOnce(new Error('Failed'));
      const handler = jest.fn();
      processor.on('itemError', handler);

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);
      await processor.processAll();

      expect(handler).toHaveBeenCalled();
    });

    test('should emit progress event', async () => {
      const handler = jest.fn();
      processor.on('progress', handler);

      const files = [
        new File(['test1'], 'test1.wav', { type: 'audio/wav' }),
        new File(['test2'], 'test2.wav', { type: 'audio/wav' }),
      ];
      processor.addFiles(files);
      await processor.processAll();

      expect(handler).toHaveBeenCalled();
    });
  });

  describe('getResults', () => {
    test('should return all completed results', async () => {
      mockProcessFn
        .mockResolvedValueOnce('Result 1')
        .mockResolvedValueOnce('Result 2');

      const files = [
        new File(['test1'], 'test1.wav', { type: 'audio/wav' }),
        new File(['test2'], 'test2.wav', { type: 'audio/wav' }),
      ];
      processor.addFiles(files);
      await processor.processAll();

      const results = processor.getResults();

      expect(results).toHaveLength(2);
      expect(results[0].result).toBe('Result 1');
      expect(results[1].result).toBe('Result 2');
    });
  });

  describe('exportResults', () => {
    test('should export results as JSON', async () => {
      mockProcessFn.mockResolvedValueOnce('Transcribed text');

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);
      await processor.processAll();

      const json = processor.exportResults('json');
      const parsed = JSON.parse(json);

      expect(parsed).toHaveLength(1);
      expect(parsed[0].transcript).toBe('Transcribed text');
      expect(parsed[0].filename).toBe('test.wav');
    });

    test('should export results as text', async () => {
      mockProcessFn
        .mockResolvedValueOnce('First transcript')
        .mockResolvedValueOnce('Second transcript');

      const files = [
        new File(['test1'], 'test1.wav', { type: 'audio/wav' }),
        new File(['test2'], 'test2.wav', { type: 'audio/wav' }),
      ];
      processor.addFiles(files);
      await processor.processAll();

      const text = processor.exportResults('text');

      expect(text).toContain('test1.wav');
      expect(text).toContain('First transcript');
      expect(text).toContain('test2.wav');
      expect(text).toContain('Second transcript');
    });

    test('should export results as CSV', async () => {
      mockProcessFn.mockResolvedValueOnce('Hello world');

      const file = new File(['test'], 'test.wav', { type: 'audio/wav' });
      processor.addFile(file);
      await processor.processAll();

      const csv = processor.exportResults('csv');

      expect(csv).toContain('filename,transcript');
      expect(csv).toContain('test.wav');
      expect(csv).toContain('Hello world');
    });
  });
});

describe('Batch Processing Integration', () => {
  test('should handle large batch', async () => {
    const mockProcessFn = jest.fn().mockResolvedValue('result');
    const processor = new BatchProcessor({
      processFn: mockProcessFn,
      concurrency: 5,
    });

    const files = Array(20).fill(null).map((_, i) =>
      new File(['test'], `file${i}.wav`, { type: 'audio/wav' })
    );
    processor.addFiles(files);

    await processor.processAll();

    expect(mockProcessFn).toHaveBeenCalledTimes(20);
    expect(processor.getResults()).toHaveLength(20);

    processor.stop();
  });
});

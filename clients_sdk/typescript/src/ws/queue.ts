/**
 * Message Queue for WebSocket
 * Buffers messages during disconnection/reconnection
 */

import type { OutgoingMessage } from '../types/messages.js';

/**
 * Queue configuration
 */
export interface MessageQueueConfig {
  /** Maximum number of messages to buffer (default: 100) */
  maxSize?: number;
  /** Maximum age of messages in milliseconds (default: 60000) */
  maxAge?: number;
  /** Whether to drop oldest messages when full (default: true) */
  dropOldest?: boolean;
}

/**
 * Queued message with metadata
 */
interface QueuedMessage {
  /** The message */
  message: OutgoingMessage;
  /** Timestamp when queued */
  timestamp: number;
  /** Binary data if applicable */
  binaryData?: ArrayBuffer | Uint8Array;
}

/**
 * Default queue configuration
 */
const DEFAULT_QUEUE_CONFIG: Required<MessageQueueConfig> = {
  maxSize: 100,
  maxAge: 60000, // 1 minute
  dropOldest: true,
};

/**
 * Message queue for buffering during disconnection
 */
export class MessageQueue {
  private queue: QueuedMessage[] = [];
  private config: Required<MessageQueueConfig>;
  private droppedCount = 0;

  constructor(config: MessageQueueConfig = {}) {
    this.config = { ...DEFAULT_QUEUE_CONFIG, ...config };
  }

  /**
   * Add message to queue
   */
  enqueue(message: OutgoingMessage, binaryData?: ArrayBuffer | Uint8Array): boolean {
    // Remove expired messages first
    this.removeExpired();

    // Check if queue is full
    if (this.queue.length >= this.config.maxSize) {
      if (this.config.dropOldest) {
        this.queue.shift();
        this.droppedCount++;
      } else {
        // Drop new message
        this.droppedCount++;
        return false;
      }
    }

    this.queue.push({
      message,
      timestamp: Date.now(),
      binaryData,
    });

    return true;
  }

  /**
   * Dequeue next message
   */
  dequeue(): { message: OutgoingMessage; binaryData?: ArrayBuffer | Uint8Array } | null {
    this.removeExpired();

    const item = this.queue.shift();
    if (!item) return null;

    return {
      message: item.message,
      binaryData: item.binaryData,
    };
  }

  /**
   * Peek at next message without removing
   */
  peek(): OutgoingMessage | null {
    this.removeExpired();
    return this.queue[0]?.message ?? null;
  }

  /**
   * Get all queued messages and clear queue
   */
  drain(): Array<{ message: OutgoingMessage; binaryData?: ArrayBuffer | Uint8Array }> {
    this.removeExpired();

    const messages = this.queue.map((item) => ({
      message: item.message,
      binaryData: item.binaryData,
    }));

    this.queue = [];
    return messages;
  }

  /**
   * Remove expired messages
   */
  private removeExpired(): void {
    const now = Date.now();
    const cutoff = now - this.config.maxAge;

    const originalLength = this.queue.length;
    this.queue = this.queue.filter((item) => item.timestamp > cutoff);

    this.droppedCount += originalLength - this.queue.length;
  }

  /**
   * Get current queue size
   */
  size(): number {
    this.removeExpired();
    return this.queue.length;
  }

  /**
   * Check if queue is empty
   */
  isEmpty(): boolean {
    return this.size() === 0;
  }

  /**
   * Check if queue is full
   */
  isFull(): boolean {
    return this.size() >= this.config.maxSize;
  }

  /**
   * Get number of dropped messages
   */
  getDroppedCount(): number {
    return this.droppedCount;
  }

  /**
   * Clear queue
   */
  clear(): void {
    this.queue = [];
  }

  /**
   * Reset dropped count
   */
  resetDroppedCount(): void {
    this.droppedCount = 0;
  }

  /**
   * Get queue statistics
   */
  getStats(): {
    size: number;
    maxSize: number;
    droppedCount: number;
    oldestAge: number | null;
  } {
    this.removeExpired();

    return {
      size: this.queue.length,
      maxSize: this.config.maxSize,
      droppedCount: this.droppedCount,
      oldestAge: this.queue.length > 0 ? Date.now() - this.queue[0]!.timestamp : null,
    };
  }
}

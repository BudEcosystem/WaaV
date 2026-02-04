"""
Tests for WebSocket message queue with backpressure.
"""

import time
import pytest

from bud_waav.ws.queue import MessageQueue, QueueConfig, QueueStats


class TestQueueConfig:
    """Tests for queue configuration."""

    def test_defaults(self):
        """Should have reasonable defaults."""
        config = QueueConfig()
        assert config.max_size == 256
        assert config.max_age_ms == 60_000.0
        assert config.drop_oldest is True

    def test_custom_config(self):
        """Should accept custom values."""
        config = QueueConfig(max_size=10, max_age_ms=5000.0, drop_oldest=False)
        assert config.max_size == 10
        assert config.max_age_ms == 5000.0
        assert config.drop_oldest is False


class TestMessageQueue:
    """Tests for the message queue."""

    def test_empty_queue(self):
        """New queue should be empty."""
        q = MessageQueue()
        assert q.is_empty
        assert not q.is_full
        assert q.size == 0

    def test_enqueue_dequeue(self):
        """Should enqueue and dequeue messages in FIFO order."""
        q = MessageQueue()
        q.enqueue("msg1")
        q.enqueue("msg2")
        q.enqueue("msg3")

        assert q.size == 3
        assert q.dequeue() == "msg1"
        assert q.dequeue() == "msg2"
        assert q.dequeue() == "msg3"
        assert q.dequeue() is None

    def test_enqueue_binary(self):
        """Should handle binary data."""
        q = MessageQueue()
        data = b"\x00\x01\x02\x03"
        q.enqueue(data)
        assert q.dequeue() == data

    def test_mixed_text_and_binary(self):
        """Should handle mixed text and binary messages."""
        q = MessageQueue()
        q.enqueue("text message")
        q.enqueue(b"\x00\x01\x02")
        q.enqueue("another text")

        assert q.dequeue() == "text message"
        assert q.dequeue() == b"\x00\x01\x02"
        assert q.dequeue() == "another text"

    def test_peek(self):
        """Peek should return oldest message without removing it."""
        q = MessageQueue()
        q.enqueue("first")
        q.enqueue("second")

        assert q.peek() == "first"
        assert q.size == 2  # Not removed
        assert q.peek() == "first"  # Same result

    def test_peek_empty(self):
        """Peek on empty queue should return None."""
        q = MessageQueue()
        assert q.peek() is None

    def test_drain(self):
        """Drain should return all messages and empty the queue."""
        q = MessageQueue()
        q.enqueue("a")
        q.enqueue("b")
        q.enqueue("c")

        messages = q.drain()
        assert messages == ["a", "b", "c"]
        assert q.is_empty

    def test_drain_empty(self):
        """Drain on empty queue should return empty list."""
        q = MessageQueue()
        assert q.drain() == []

    def test_clear(self):
        """Clear should remove all messages and return count."""
        q = MessageQueue()
        q.enqueue("a")
        q.enqueue("b")
        q.enqueue("c")

        count = q.clear()
        assert count == 3
        assert q.is_empty

    def test_clear_empty(self):
        """Clear on empty queue should return 0."""
        q = MessageQueue()
        assert q.clear() == 0

    def test_is_full(self):
        """is_full should be True when at capacity."""
        q = MessageQueue(QueueConfig(max_size=3))
        q.enqueue("a")
        q.enqueue("b")
        assert not q.is_full

        q.enqueue("c")
        assert q.is_full

    def test_drop_oldest_policy(self):
        """With drop_oldest=True, oldest messages should be dropped when full."""
        q = MessageQueue(QueueConfig(max_size=3, drop_oldest=True))
        q.enqueue("a")
        q.enqueue("b")
        q.enqueue("c")
        # Queue is full. Adding one more should drop 'a'
        result = q.enqueue("d")
        assert result is True  # Accepted (oldest was dropped)
        assert q.size == 3

        messages = q.drain()
        assert messages == ["b", "c", "d"]

    def test_drop_newest_policy(self):
        """With drop_oldest=False, newest (incoming) messages should be rejected."""
        q = MessageQueue(QueueConfig(max_size=3, drop_oldest=False))
        q.enqueue("a")
        q.enqueue("b")
        q.enqueue("c")

        result = q.enqueue("d")
        assert result is False  # Rejected

        messages = q.drain()
        assert messages == ["a", "b", "c"]

    def test_dropped_count_tracked(self):
        """Should track total dropped message count."""
        q = MessageQueue(QueueConfig(max_size=2, drop_oldest=True))
        q.enqueue("a")
        q.enqueue("b")
        q.enqueue("c")  # Drops 'a'
        q.enqueue("d")  # Drops 'b'

        stats = q.get_stats()
        assert stats.dropped_count == 2

    def test_dropped_count_drop_newest(self):
        """Drop newest policy should also track dropped count."""
        q = MessageQueue(QueueConfig(max_size=2, drop_oldest=False))
        q.enqueue("a")
        q.enqueue("b")
        q.enqueue("c")  # Rejected
        q.enqueue("d")  # Rejected

        stats = q.get_stats()
        assert stats.dropped_count == 2

    def test_get_stats(self):
        """Should return accurate queue statistics."""
        q = MessageQueue(QueueConfig(max_size=10))
        q.enqueue("a")
        q.enqueue("b")

        stats = q.get_stats()
        assert stats.size == 2
        assert stats.max_size == 10
        assert stats.dropped_count == 0
        assert stats.oldest_age_ms >= 0

    def test_get_stats_empty(self):
        """Stats on empty queue should show zero age."""
        q = MessageQueue()
        stats = q.get_stats()
        assert stats.size == 0
        assert stats.oldest_age_ms == 0.0

    def test_reset_stats(self):
        """reset_stats should clear dropped count."""
        q = MessageQueue(QueueConfig(max_size=1, drop_oldest=True))
        q.enqueue("a")
        q.enqueue("b")  # Drops 'a'

        q.reset_stats()
        stats = q.get_stats()
        assert stats.dropped_count == 0

    def test_message_expiration(self):
        """Messages older than max_age_ms should be expired."""
        q = MessageQueue(QueueConfig(max_age_ms=1.0))  # 1ms expiration

        q.enqueue("old-message")
        time.sleep(0.01)  # Wait 10ms

        # Message should be expired
        assert q.dequeue() is None
        assert q.is_empty

    def test_expiration_during_enqueue(self):
        """Expired messages should be cleaned up during enqueue."""
        q = MessageQueue(QueueConfig(max_size=2, max_age_ms=1.0))

        q.enqueue("old")
        time.sleep(0.01)

        # This should expire the old message and then add the new one
        q.enqueue("new")
        assert q.size == 1
        assert q.dequeue() == "new"

    def test_large_queue(self):
        """Should handle large number of messages."""
        q = MessageQueue(QueueConfig(max_size=10000))
        for i in range(5000):
            q.enqueue(f"msg-{i}")

        assert q.size == 5000

        messages = q.drain()
        assert len(messages) == 5000
        assert messages[0] == "msg-0"
        assert messages[-1] == "msg-4999"

    def test_queue_stats_oldest_age(self):
        """oldest_age_ms should reflect the age of the oldest message."""
        q = MessageQueue()
        q.enqueue("first")
        time.sleep(0.02)  # Wait 20ms
        q.enqueue("second")

        stats = q.get_stats()
        assert stats.oldest_age_ms >= 15  # At least ~15ms old (with tolerance)

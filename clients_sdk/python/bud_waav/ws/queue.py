"""
Message queue with backpressure for WebSocket reliability.

Buffers messages during disconnection/reconnection periods with configurable
drop policies and automatic message expiration.
"""

import time
from collections import deque
from dataclasses import dataclass, field
from typing import Optional, Union


@dataclass
class QueueConfig:
    """Configuration for the message queue.

    Attributes:
        max_size: Maximum number of messages to buffer. Default 256.
        max_age_ms: Maximum age of a message in milliseconds before
            automatic expiration. Default 60000 (60 seconds).
        drop_oldest: When True, drops oldest messages when full.
            When False, drops newest (incoming) messages. Default True.
    """

    max_size: int = 256
    max_age_ms: float = 60_000.0
    drop_oldest: bool = True


@dataclass
class QueueStats:
    """Statistics about the message queue.

    Attributes:
        size: Current number of messages in the queue.
        max_size: Maximum configured queue size.
        dropped_count: Total number of messages dropped since creation/reset.
        oldest_age_ms: Age of the oldest message in milliseconds, or 0.0 if empty.
    """

    size: int
    max_size: int
    dropped_count: int
    oldest_age_ms: float


@dataclass(order=True)
class _QueueEntry:
    """Internal queue entry with timestamp."""

    timestamp_ms: float
    data: Union[str, bytes] = field(compare=False)


class MessageQueue:
    """Buffered message queue for WebSocket reliability.

    Stores messages during disconnection periods and drains them
    on reconnection. Supports configurable drop policies, message
    expiration, and backpressure statistics.

    Example::

        queue = MessageQueue(QueueConfig(max_size=128, max_age_ms=30000))

        # Buffer messages while disconnected
        queue.enqueue('{"type": "speak", "text": "Hello"}')
        queue.enqueue(b'\\x00\\x01\\x02')  # Binary audio

        # On reconnect, drain and resend
        for msg in queue.drain():
            await ws.send(msg)
    """

    def __init__(self, config: Optional[QueueConfig] = None) -> None:
        self._config = config or QueueConfig()
        self._queue: deque[_QueueEntry] = deque()
        self._dropped_count: int = 0

    @property
    def size(self) -> int:
        """Current number of messages in the queue."""
        return len(self._queue)

    @property
    def is_empty(self) -> bool:
        """Whether the queue is empty."""
        return len(self._queue) == 0

    @property
    def is_full(self) -> bool:
        """Whether the queue is at capacity."""
        return len(self._queue) >= self._config.max_size

    def enqueue(self, data: Union[str, bytes]) -> bool:
        """Add a message to the queue.

        Applies the configured drop policy if the queue is full.
        Expired messages are purged before checking capacity.

        Args:
            data: Message data (text or binary).

        Returns:
            True if the message was accepted, False if it was dropped.
        """
        self._expire_old()

        if len(self._queue) >= self._config.max_size:
            if self._config.drop_oldest:
                self._queue.popleft()
                self._dropped_count += 1
            else:
                self._dropped_count += 1
                return False

        now_ms = time.monotonic() * 1000.0
        self._queue.append(_QueueEntry(timestamp_ms=now_ms, data=data))
        return True

    def dequeue(self) -> Optional[Union[str, bytes]]:
        """Remove and return the oldest message.

        Skips expired messages.

        Returns:
            Message data, or None if the queue is empty.
        """
        self._expire_old()
        if self._queue:
            return self._queue.popleft().data
        return None

    def peek(self) -> Optional[Union[str, bytes]]:
        """View the oldest message without removing it.

        Returns:
            Message data, or None if empty.
        """
        self._expire_old()
        if self._queue:
            return self._queue[0].data
        return None

    def drain(self) -> list[Union[str, bytes]]:
        """Remove and return all messages in order.

        Expired messages are excluded.

        Returns:
            List of all buffered messages, oldest first.
        """
        self._expire_old()
        messages = [entry.data for entry in self._queue]
        self._queue.clear()
        return messages

    def clear(self) -> int:
        """Remove all messages from the queue.

        Returns:
            Number of messages that were cleared.
        """
        count = len(self._queue)
        self._queue.clear()
        return count

    def get_stats(self) -> QueueStats:
        """Get queue statistics.

        Returns:
            Current queue statistics.
        """
        self._expire_old()
        oldest_age_ms = 0.0
        if self._queue:
            now_ms = time.monotonic() * 1000.0
            oldest_age_ms = now_ms - self._queue[0].timestamp_ms

        return QueueStats(
            size=len(self._queue),
            max_size=self._config.max_size,
            dropped_count=self._dropped_count,
            oldest_age_ms=oldest_age_ms,
        )

    def reset_stats(self) -> None:
        """Reset the dropped message counter."""
        self._dropped_count = 0

    def _expire_old(self) -> None:
        """Remove messages older than max_age_ms."""
        if not self._queue:
            return

        cutoff_ms = time.monotonic() * 1000.0 - self._config.max_age_ms
        while self._queue and self._queue[0].timestamp_ms < cutoff_ms:
            self._queue.popleft()
            self._dropped_count += 1

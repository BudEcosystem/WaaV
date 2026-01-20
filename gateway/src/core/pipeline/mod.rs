//! Audio processing pipeline utilities.
//!
//! This module provides:
//! - [`FramePriority`] - Priority levels for audio frames
//! - [`PriorityFrame`] - Audio frame with priority metadata
//! - [`FramePriorityQueue`] - Thread-safe priority queue for audio frames
//!
//! # Design Goals
//!
//! The priority queue is designed for real-time audio processing with:
//! - Bounded capacity with backpressure handling
//! - O(log n) insert and O(1) pop operations
//! - Thread-safe concurrent access using `parking_lot::Mutex`
//! - Pre-allocated capacity to minimize runtime allocations
//!
//! # Priority Levels
//!
//! - `Critical`: Interruption signals, system control messages
//! - `High`: User speech during bot speaking (for interruption)
//! - `Normal`: Regular audio frames
//! - `Low`: Background/fill audio, silence frames
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::pipeline::{FramePriorityQueue, FramePriority, PriorityFrame};
//!
//! let queue = FramePriorityQueue::new(1000);
//!
//! // Push frames with different priorities
//! queue.push(PriorityFrame::new(audio_data, FramePriority::Normal));
//! queue.push(PriorityFrame::new(interruption_signal, FramePriority::Critical));
//!
//! // Critical frame comes out first
//! let frame = queue.pop().unwrap();
//! assert_eq!(frame.priority, FramePriority::Critical);
//! ```

pub mod queue;

pub use queue::{FramePriority, FramePriorityQueue, PriorityFrame, QueueSnapshot};

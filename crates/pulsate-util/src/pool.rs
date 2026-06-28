//! A simple per-worker buffer pool.
//!
//! Read/write buffers are pooled and reused to avoid per-request allocation
//! churn on the hot path (`docs/02-architecture.md#memory-model`). A bounded
//! free-list keyed by a target capacity.

use std::cell::RefCell;

use bytes::BytesMut;

/// A bounded pool of reusable [`BytesMut`] buffers.
///
/// Not `Sync`: a pool is owned by a single worker and accessed without locks,
/// matching the share-nothing-per-task data-plane model. Buffers handed out via
/// [`BufferPool::take`] are returned with [`BufferPool::give`]; the pool caps how
/// many it retains so idle memory stays bounded.
#[derive(Debug)]
pub struct BufferPool {
    free: RefCell<Vec<BytesMut>>,
    buf_capacity: usize,
    max_retained: usize,
}

impl BufferPool {
    /// Create a pool that hands out buffers of `buf_capacity` bytes and retains
    /// at most `max_retained` of them.
    #[must_use]
    pub fn new(buf_capacity: usize, max_retained: usize) -> Self {
        Self {
            free: RefCell::new(Vec::new()),
            buf_capacity,
            max_retained,
        }
    }

    /// Take a cleared buffer, reusing a pooled one when available.
    #[must_use]
    pub fn take(&self) -> BytesMut {
        if let Some(mut buf) = self.free.borrow_mut().pop() {
            buf.clear();
            buf
        } else {
            BytesMut::with_capacity(self.buf_capacity)
        }
    }

    /// Return a buffer to the pool. Oversized buffers and overflow past
    /// `max_retained` are dropped rather than retained, keeping memory bounded.
    pub fn give(&self, buf: BytesMut) {
        let mut free = self.free.borrow_mut();
        if free.len() < self.max_retained && buf.capacity() <= self.buf_capacity {
            free.push(buf);
        }
    }

    /// The number of buffers currently retained (for tests/metrics).
    #[must_use]
    pub fn retained(&self) -> usize {
        self.free.borrow().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_then_give_reuses_buffer() {
        let pool = BufferPool::new(1024, 4);
        let buf = pool.take();
        assert!(buf.capacity() >= 1024);
        pool.give(buf);
        assert_eq!(pool.retained(), 1);
        // Next take draws from the pool.
        let _ = pool.take();
        assert_eq!(pool.retained(), 0);
    }

    #[test]
    fn retained_count_is_bounded() {
        let pool = BufferPool::new(64, 2);
        for _ in 0..10 {
            pool.give(BytesMut::with_capacity(64));
        }
        assert_eq!(pool.retained(), 2);
    }

    #[test]
    fn oversized_buffers_are_not_retained() {
        let pool = BufferPool::new(64, 4);
        pool.give(BytesMut::with_capacity(4096));
        assert_eq!(pool.retained(), 0);
    }
}

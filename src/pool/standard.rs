// src/pool/standard.rs
//! Standard mutex-based buffer pool.
//!
//! # Pool Size Limits
//!
//! When a buffer is returned and the pool is at `max_pool_size`:
//! - The buffer is securely zeroed (via `zeroize`) and dropped.
//! - This prevents unbounded memory growth.
//!
//! # Memory Safety
//!
//! All buffers are securely zeroed **before** being returned to the pool.
//! This prevents a previous user's sensitive data from being readable by
//! the next acquirer — even through indirect paths such as `resize()` or
//! unsafe pointer access into the backing Vec.
//!
//! Buffers are also zeroed on drop via `#[zeroize(drop)]` on `Buffer`.
//!
//! # Fix vs original
//!
//! The original code called `buffer.reset()` (clears `pos`/`len` metadata only)
//! before returning a buffer to the pool.  The raw bytes in `data[0..old_len]`
//! remained in memory and were accessible to the next acquirer via `resize()`,
//! `set_len()`, or `get_write_ptr()`.  This is now fixed by calling
//! `buffer.burn()` (full zeroize + metadata reset) before any pool insertion.

use super::config::PoolConfig;
use super::stats::PoolStats;
use crate::buffer::Buffer;
use std::sync::{Arc, Mutex};

pub(crate) struct PoolInner {
    pub(crate) buffers: Vec<Buffer>,
    pub(crate) config: PoolConfig,
    pub(crate) total_allocated: usize,
    pub(crate) total_acquired: usize,
    pub(crate) total_returned: usize,
}

/// Standard thread-safe buffer pool backed by a `Mutex`.
///
/// Simple and reliable; suitable for most applications.
/// For maximum throughput in high-contention scenarios, prefer
/// [`crate::pool::FastBufferPool`].
///
/// # Thread Safety
///
/// Can be shared across threads via `Arc`.
///
/// # Example
///
/// ```rust
/// use secbuf::prelude::*;
///
/// let pool = BufferPool::new(PoolConfig {
///     buffer_size: 4096,
///     max_pool_size: 100,
///     min_pool_size: 10,
/// });
///
/// let mut buf = pool.acquire();
/// buf.put_u32(42)?;
/// // Buffer is burned (zeroed) then returned to the pool on drop.
/// # Ok::<(), secbuf::BufferError>(())
/// ```
pub struct BufferPool {
    pub(crate) inner: Arc<Mutex<PoolInner>>,
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new(PoolConfig::default())
    }
}

impl BufferPool {
    /// Creates a new buffer pool and pre-warms it with `config.min_pool_size` buffers.
    pub fn new(config: PoolConfig) -> Self {
        let mut buffers = Vec::with_capacity(config.min_pool_size);
        for _ in 0..config.min_pool_size {
            buffers.push(Buffer::new(config.buffer_size));
        }
        Self {
            inner: Arc::new(Mutex::new(PoolInner {
                buffers,
                config,
                total_allocated: 0,
                total_acquired: 0,
                total_returned: 0,
            })),
        }
    }

    /// Acquires a buffer from the pool, allocating a fresh one if the pool is empty.
    ///
    /// The returned [`PooledBuffer`] is automatically burned and returned (or
    /// dropped) when it goes out of scope.
    pub fn acquire(&self) -> PooledBuffer {
        let mut inner = self.inner.lock().unwrap();
        inner.total_acquired += 1;

        let buffer = inner.buffers.pop().unwrap_or_else(|| {
            inner.total_allocated += 1;
            Buffer::new(inner.config.buffer_size)
        });

        PooledBuffer {
            buffer: Some(buffer),
            pool: Arc::clone(&self.inner),
        }
    }

    /// Number of buffers currently idle in the pool.
    pub fn available(&self) -> usize {
        self.inner.lock().unwrap().buffers.len()
    }

    /// Returns a snapshot of pool statistics.
    pub fn stats(&self) -> PoolStats {
        let inner = self.inner.lock().unwrap();
        PoolStats {
            available: inner.buffers.len(),
            total_allocated: inner.total_allocated,
            total_acquired: inner.total_acquired,
            total_returned: inner.total_returned,
            buffer_size: inner.config.buffer_size,
            max_pool_size: inner.config.max_pool_size,
        }
    }

    /// Truncates idle buffers to `min_pool_size`, freeing excess memory.
    ///
    /// Excess buffers are securely zeroed before being dropped.
    pub fn shrink(&self) {
        let mut inner = self.inner.lock().unwrap();
        let min_size = inner.config.min_pool_size;
        inner.buffers.truncate(min_size);
        inner.buffers.shrink_to_fit();
    }

    /// Removes all idle buffers from the pool.
    ///
    /// Buffers are securely zeroed before being dropped (via `#[zeroize(drop)]`).
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.buffers.clear();
    }

    /// Grows the idle pool up to `min(target_size, max_pool_size)`.
    pub fn grow(&self, target_size: usize) {
        let mut inner = self.inner.lock().unwrap();
        let max_size = inner.config.max_pool_size;
        let buffer_size = inner.config.buffer_size;
        let target = target_size.min(max_size);
        while inner.buffers.len() < target {
            inner.buffers.push(Buffer::new(buffer_size));
        }
    }
}

/// A buffer borrowed from a [`BufferPool`].
///
/// On drop the buffer is **burned** (all bytes securely zeroed, positions reset)
/// and then returned to the pool if space permits — otherwise it is securely
/// dropped.
///
/// Use [`leak`](Self::leak) or [`drop_now`](Self::drop_now) to opt out of
/// automatic return.
pub struct PooledBuffer {
    pub(crate) buffer: Option<Buffer>,
    pub(crate) pool: Arc<Mutex<PoolInner>>,
}

impl PooledBuffer {
    /// Extracts the buffer from the pool wrapper without returning it.
    ///
    /// The caller is responsible for cleanup; the buffer will still be zeroed
    /// when eventually dropped via `#[zeroize(drop)]`.
    pub fn leak(mut self) -> Buffer {
        self.buffer.take().unwrap()
    }

    /// Immediately and securely drops the buffer, bypassing pool return.
    pub fn drop_now(mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            buffer.burn();
            drop(buffer);
        }
    }

    /// Capacity of the underlying buffer.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.buffer.as_ref().unwrap().capacity()
    }
}

impl std::ops::Deref for PooledBuffer {
    type Target = Buffer;
    fn deref(&self) -> &Self::Target {
        self.buffer.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer.as_mut().unwrap()
    }
}

impl Drop for PooledBuffer {
    /// Burns the buffer (secure zero of all bytes + metadata reset) and returns
    /// it to the pool if space is available.
    ///
    /// **Why `burn()` instead of `reset()`?**
    ///
    /// `reset()` only clears `pos` and `len`.  The raw bytes in the backing
    /// allocation remain, and the next acquirer can expose them through
    /// `resize()`, `set_len()`, or `get_write_ptr()`.  `burn()` calls
    /// `vec.zeroize()` first, so the next acquirer always receives a
    /// clean, zeroed buffer — consistent with this library's security contract.
    fn drop(&mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            // Securely zero BEFORE returning to pool.
            buffer.burn();

            let mut inner = self.pool.lock().unwrap();
            inner.total_returned += 1;

            if inner.buffers.len() < inner.config.max_pool_size {
                inner.buffers.push(buffer);
            }
            // else: buffer is already burned; the Vec<u8> will be freed here.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_basic() {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });
        assert_eq!(pool.available(), 2);
        let _buf = pool.acquire();
        assert_eq!(pool.stats().total_acquired, 1);
    }

    #[test]
    fn test_returned_buffer_is_clean() {
        // Verify that a buffer returned to the pool has been zeroed.
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 64,
            max_pool_size: 2,
            min_pool_size: 0,
        });

        {
            let mut buf = pool.acquire();
            buf.put_bytes(&[0xFF; 16]).unwrap();
        } // dropped → burned → returned to pool

        // Re-acquire and verify bytes in the valid range are zero.
        let mut buf2 = pool.acquire();
        // burn() zeroes data[0..capacity], so after resize we can inspect.
        buf2.set_len(16).unwrap();
        assert_eq!(&buf2.as_slice()[..16], &[0u8; 16]);
    }

    #[test]
    fn test_drop_now() {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });
        let initial_available = pool.available();
        {
            let mut buf = pool.acquire();
            buf.put_u32(12345).unwrap();
            buf.drop_now();
        }
        assert_eq!(pool.available(), initial_available - 1);
    }

    #[test]
    fn test_leak() {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });
        let pooled = pool.acquire();
        let _owned = pooled.leak();
        assert_eq!(pool.stats().total_acquired, 1);
    }

    #[test]
    fn test_normal_return() {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });
        let initial = pool.available();
        { let _buf = pool.acquire(); } // normal drop
        assert_eq!(pool.available(), initial);
    }

    #[test]
    fn test_grow_shrink() {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 64,
            max_pool_size: 20,
            min_pool_size: 0,
        });
        pool.grow(10);
        assert_eq!(pool.available(), 10);
        pool.shrink();
        assert_eq!(pool.available(), 0);
    }
}
// src/pool/standard.rs
//! Standard mutex-based buffer pool
//!
//! # Pool Size Limits
//!
//! When buffers are returned and the pool is at `max_pool_size`:
//! - The buffer is securely zeroed (via zeroize crate)
//! - The buffer is dropped and not retained in the pool
//! - This prevents unbounded memory growth
//!
//! # Memory Safety
//!
//! All buffers are automatically and securely zeroed on drop using the zeroize
//! crate, which provides compiler-resistant memory clearing.

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

/// Standard thread-safe buffer pool using Mutex.
///
/// Simple and reliable implementation suitable for most applications.
/// For maximum performance in high-contention scenarios, use [`crate::pool::FastBufferPool`].
///
/// # Thread Safety
///
/// This pool uses a `Mutex` for synchronization and can be safely shared across
/// threads using `Arc`.
///
/// # Performance Characteristics
///
/// - Acquire: O(1) amortized
/// - Return: O(1) amortized
/// - Mutex contention in high-concurrency scenarios
///
/// For lock-free operation, consider [`crate::pool::FastBufferPool`].
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
/// // Buffer automatically returned on drop
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
    /// Creates a new buffer pool with the given configuration.
    ///
    /// Pre-warms the pool with `config.min_pool_size` buffers.
    pub fn new(config: PoolConfig) -> Self {
        let mut buffers = Vec::with_capacity(config.min_pool_size);

        // Pre-warm the pool
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

    /// Acquires a buffer from the pool.
    ///
    /// If no buffers are available, allocates a new one.
    /// The buffer is automatically returned to the pool when dropped.
    ///
    /// # Returns
    ///
    /// A `PooledBuffer` that will be automatically returned to the pool when dropped.
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

    /// Returns the number of available buffers in the pool.
    pub fn available(&self) -> usize {
        self.inner.lock().unwrap().buffers.len()
    }

    /// Returns pool statistics.
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

    /// Shrinks the pool to minimum size.
    ///
    /// Removes excess buffers beyond `min_pool_size`. Removed buffers are
    /// securely zeroed before being dropped.
    pub fn shrink(&self) {
        let mut inner = self.inner.lock().unwrap();
        let min_size = inner.config.min_pool_size;
        inner.buffers.truncate(min_size);
        inner.buffers.shrink_to_fit();
    }

    /// Clears all buffers from the pool.
    ///
    /// All cleared buffers are securely zeroed before being dropped.
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.buffers.clear();
    }

    /// Grows the pool to the specified size.
    ///
    /// Allocates new buffers and adds them to the pool up to the
    /// lesser of `target_size` or `config.max_pool_size`.
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

/// A buffer borrowed from the standard pool.
///
/// Automatically returned to the pool when dropped, unless `leak()` or `drop_now()` is called.
///
/// # Automatic Cleanup
///
/// When dropped normally, the buffer is:
/// 1. Reset (position and length cleared)
/// 2. Returned to the pool if space available
/// 3. Otherwise, securely zeroed and dropped (not retained in pool)
///
/// To prevent the buffer from being returned to the pool, use `leak()` or `drop_now()`.
pub struct PooledBuffer {
    pub(crate) buffer: Option<Buffer>,
    pub(crate) pool: Arc<Mutex<PoolInner>>,
}

impl PooledBuffer {
    /// Leaks the buffer, preventing return to pool.
    ///
    /// Returns the owned buffer. The caller is responsible for eventual cleanup.
    /// The buffer will still be securely zeroed when finally dropped due to the
    /// `#[zeroize(drop)]` attribute on `Buffer`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use secbuf::prelude::*;
    ///
    /// let pool = BufferPool::new(PoolConfig::default());
    /// let pooled_buf = pool.acquire();
    /// let owned_buf = pooled_buf.leak();
    /// // owned_buf is now independent of the pool
    /// ```
    pub fn leak(mut self) -> Buffer {
        self.buffer.take().unwrap()
    }

    /// Immediately drops the buffer without returning it to the pool.
    ///
    /// The buffer is securely zeroed before being dropped.
    /// This is useful when you want to ensure a buffer is not reused.
    ///
    /// # Example
    ///
    /// ```rust
    /// use secbuf::prelude::*;
    ///
    /// let pool = BufferPool::new(PoolConfig::default());
    /// let mut buf = pool.acquire();
    /// buf.put_bytes(b"sensitive data").unwrap();
    /// buf.drop_now(); // Securely erased, not returned to pool
    /// ```
    pub fn drop_now(mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            buffer.burn(); // Explicit secure erasure
            drop(buffer);
        }
    }

    /// Returns the buffer's capacity.
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
    /// Automatically returns the buffer to the pool.
    ///
    /// The buffer is reset and returned to the pool if space is available.
    /// If the pool is at max capacity, the buffer is securely zeroed and dropped.
    fn drop(&mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            buffer.reset();

            let mut inner = self.pool.lock().unwrap();
            inner.total_returned += 1;

            if inner.buffers.len() < inner.config.max_pool_size {
                inner.buffers.push(buffer);
            }
            // else: buffer is dropped here and securely zeroed via #[zeroize(drop)]
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
        let stats = pool.stats();
        assert_eq!(stats.total_acquired, 1);
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
            buf.drop_now(); // Should not return to pool
        }

        // Pool should not have gained a buffer back
        assert_eq!(pool.available(), initial_available - 1);
    }

    #[test]
    fn test_leak() {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });

        let pooled_buf = pool.acquire();
        let _owned_buf = pooled_buf.leak();

        let stats = pool.stats();
        assert_eq!(stats.total_acquired, 1);
        // Leaked buffer doesn't return to pool
    }

    #[test]
    fn test_normal_return() {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });

        let initial_available = pool.available();

        {
            let _buf = pool.acquire();
        } // Normal drop - should return to pool

        assert_eq!(pool.available(), initial_available);
    }
}

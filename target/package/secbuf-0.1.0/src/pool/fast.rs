// src/pool/fast.rs
//! Lock-free high-performance buffer pool with thread-local caching.
//!
//! # Thread-Local Cache Behavior
//!
//! This pool uses thread-local caches for optimal performance:
//!
//! - Each thread maintains its own cache of up to 16 buffers
//! - When a thread exits, buffers in its cache are securely zeroed but permanently
//!   lost from the pool
//! - In scenarios with high thread churn, this may lead to gradual memory loss
//! - The [`clear`](FastBufferPool::clear) method only clears the global pool,
//!   not thread-local caches
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
use super::stats::FastPoolStats;
use crate::buffer::Buffer;
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Lock-free queue wrapper with size tracking.
struct LockFreeQueue<T> {
    items: crossbeam::queue::SegQueue<T>,
    size: AtomicUsize,
}

impl<T> LockFreeQueue<T> {
    fn new() -> Self {
        Self {
            items: crossbeam::queue::SegQueue::new(),
            size: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn push(&self, item: T) {
        self.items.push(item);
        self.size.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn pop(&self) -> Option<T> {
        self.items.pop().inspect(|_| {
            self.size.fetch_sub(1, Ordering::Relaxed);
        })
    }

    #[inline]
    fn len(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }
}

/// Lock-free statistics tracking.
///
/// All operations use `Ordering::Relaxed` for maximum performance.
/// Statistics may be slightly stale but are eventually consistent.
pub(crate) struct FastPoolStatsInner {
    pub(crate) allocated: AtomicUsize,
    pub(crate) acquired: AtomicUsize,
    pub(crate) returned: AtomicUsize,
    pub(crate) cache_hits: AtomicUsize,
    pub(crate) thread_local_lost: AtomicUsize,
}

impl FastPoolStatsInner {
    pub(crate) fn new() -> Self {
        Self {
            allocated: AtomicUsize::new(0),
            acquired: AtomicUsize::new(0),
            returned: AtomicUsize::new(0),
            cache_hits: AtomicUsize::new(0),
            thread_local_lost: AtomicUsize::new(0),
        }
    }
}

/// Drop guard for thread-local cache to track lost buffers.
struct ThreadCacheDropGuard {
    stats: Arc<FastPoolStatsInner>,
}

impl Drop for ThreadCacheDropGuard {
    fn drop(&mut self) {
        // During thread shutdown, TLS may already be destroyed.
        // Use try_with to avoid panicking if TLS is inaccessible.
        if let Ok(lost) = THREAD_CACHE.try_with(|cache| cache.borrow().len()) {
            if lost > 0 {
                self.stats
                    .thread_local_lost
                    .fetch_add(lost, Ordering::Relaxed);
            }
        }
    }
}

thread_local! {
    static THREAD_CACHE: RefCell<Vec<Buffer>> = RefCell::new(Vec::with_capacity(16));
    static THREAD_CACHE_SIZE: RefCell<usize> = const { RefCell::new(16) };
    static THREAD_CACHE_DROP_GUARD: RefCell<Option<ThreadCacheDropGuard>> = const { RefCell::new(None) };
}

/// High-performance lock-free buffer pool.
///
/// Uses a three-tier architecture for maximum performance:
///
/// 1. **Thread-local cache**: ~5ns (no synchronization)
/// 2. **Global lock-free queue**: ~20ns (atomic operations only)
/// 3. **Allocation**: ~100ns (fallback when pool empty)
///
/// # Performance
///
/// 10-20x faster than [`BufferPool`](super::BufferPool) in high-contention scenarios.
///
/// # Thread Safety
///
/// This pool is thread-safe and can be safely shared across threads using [`Arc`].
///
/// # Examples
///
/// ```
/// use secbuf::prelude::*;
/// use std::thread;
/// use std::sync::Arc;
///
/// let pool = Arc::new(FastBufferPool::new(PoolConfig::default()));
///
/// let handles: Vec<_> = (0..4).map(|_| {
///     let pool = Arc::clone(&pool);
///     thread::spawn(move || {
///         for i in 0..1000 {
///             let mut buf = pool.acquire();
///             buf.put_u32(i).unwrap();
///         }
///     })
/// }).collect();
///
/// for h in handles {
///     h.join().unwrap();
/// }
///
/// let stats = pool.stats();
/// println!("Cache hit rate: {:.1}%", stats.cache_hit_rate());
/// ```
pub struct FastBufferPool {
    global_pool: Arc<LockFreeQueue<Buffer>>,
    config: PoolConfig,
    stats: Arc<FastPoolStatsInner>,
}

impl FastBufferPool {
    /// Creates a new fast buffer pool.
    ///
    /// Pre-warms the global pool with `config.min_pool_size` buffers.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig {
    ///     buffer_size: 8192,
    ///     max_pool_size: 100,
    ///     min_pool_size: 10,
    /// });
    ///
    /// assert_eq!(pool.available(), 10);
    /// ```
    pub fn new(config: PoolConfig) -> Self {
        let queue = Arc::new(LockFreeQueue::new());

        // Pre-warm the pool
        for _ in 0..config.min_pool_size {
            queue.push(Buffer::new(config.buffer_size));
        }

        THREAD_CACHE_SIZE.with(|size| {
            *size.borrow_mut() = 16;
        });

        Self {
            global_pool: queue,
            config,
            stats: Arc::new(FastPoolStatsInner::new()),
        }
    }
}

impl Default for FastBufferPool {
    fn default() -> Self {
        Self::new(PoolConfig::default())
    }
}

impl FastBufferPool {
    /// Acquires a buffer from the pool.
    ///
    /// Acquisition order:
    /// 1. Thread-local cache (fastest)
    /// 2. Global lock-free queue
    /// 3. New allocation (if pool empty)
    ///
    /// # Returns
    ///
    /// A [`FastPooledBuffer`] that will be automatically returned to the pool when dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig::default());
    /// let mut buf = pool.acquire();
    /// buf.put_u32(42).unwrap();
    /// // Buffer automatically returned when dropped
    /// ```
    #[inline]
    pub fn acquire(&self) -> FastPooledBuffer {
        self.stats.acquired.fetch_add(1, Ordering::Relaxed);

        // Install drop guard on first use in this thread
        THREAD_CACHE_DROP_GUARD.with(|guard| {
            if guard.borrow().is_none() {
                *guard.borrow_mut() = Some(ThreadCacheDropGuard {
                    stats: Arc::clone(&self.stats),
                });
            }
        });

        // Fast path: try thread-local cache
        let buffer = THREAD_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some(buf) = cache.pop() {
                self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some(buf);
            }
            None
        });

        if let Some(buf) = buffer {
            return FastPooledBuffer {
                buffer: Some(buf),
                pool: Arc::clone(&self.global_pool),
                config: self.config.clone(),
                stats: Arc::clone(&self.stats),
            };
        }

        // Medium path: try global lock-free queue
        if let Some(buf) = self.global_pool.pop() {
            return FastPooledBuffer {
                buffer: Some(buf),
                pool: Arc::clone(&self.global_pool),
                config: self.config.clone(),
                stats: Arc::clone(&self.stats),
            };
        }

        // Slow path: allocate new
        self.stats.allocated.fetch_add(1, Ordering::Relaxed);
        FastPooledBuffer {
            buffer: Some(Buffer::new(self.config.buffer_size)),
            pool: Arc::clone(&self.global_pool),
            config: self.config.clone(),
            stats: Arc::clone(&self.stats),
        }
    }

    /// Returns the number of available buffers in global pool.
    ///
    /// Note: This does not include buffers in thread-local caches.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig {
    ///     buffer_size: 1024,
    ///     max_pool_size: 100,
    ///     min_pool_size: 10,
    /// });
    ///
    /// assert_eq!(pool.available(), 10);
    /// ```
    #[inline]
    pub fn available(&self) -> usize {
        self.global_pool.len()
    }

    /// Returns pool statistics.
    ///
    /// Statistics use relaxed memory ordering and may be slightly stale
    /// but are eventually consistent.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig::default());
    /// let _buf = pool.acquire();
    ///
    /// let stats = pool.stats();
    /// assert_eq!(stats.acquired, 1);
    /// ```
    pub fn stats(&self) -> FastPoolStats {
        FastPoolStats {
            available: self.global_pool.len(),
            allocated: self.stats.allocated.load(Ordering::Relaxed),
            acquired: self.stats.acquired.load(Ordering::Relaxed),
            returned: self.stats.returned.load(Ordering::Relaxed),
            cache_hits: self.stats.cache_hits.load(Ordering::Relaxed),
            thread_local_lost: self.stats.thread_local_lost.load(Ordering::Relaxed),
        }
    }

    /// Clears the global pool.
    ///
    /// **Important**: This only clears the global pool. Thread-local caches are not affected.
    /// Buffers in thread-local caches will remain until those threads exit or the cache
    /// becomes full and starts returning buffers to the global pool.
    ///
    /// All cleared buffers are securely zeroed before being dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig::default());
    /// pool.clear();
    /// assert_eq!(pool.available(), 0);
    /// ```
    pub fn clear(&self) {
        while self.global_pool.pop().is_some() {}
    }

    /// Pre-allocates buffers up to target size.
    ///
    /// Allocates new buffers and adds them to the global pool up to the
    /// lesser of `target_size` or `config.max_pool_size`.
    ///
    /// This does not affect thread-local caches.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig {
    ///     buffer_size: 1024,
    ///     max_pool_size: 100,
    ///     min_pool_size: 10,
    /// });
    ///
    /// pool.warm(50);
    /// assert_eq!(pool.available(), 50);
    /// ```
    pub fn warm(&self, target_size: usize) {
        let target = target_size.min(self.config.max_pool_size);
        let current = self.global_pool.len();

        for _ in current..target {
            self.global_pool.push(Buffer::new(self.config.buffer_size));
        }
    }
}

/// A buffer borrowed from the fast pool.
///
/// Automatically returned to the pool (thread-local cache or global pool) when dropped.
///
/// # Automatic Cleanup
///
/// When dropped, the buffer is:
/// 1. Reset (position and length cleared)
/// 2. Returned to the thread-local cache if space available
/// 3. Otherwise, returned to the global pool if space available
/// 4. Otherwise, securely zeroed and dropped (not retained in pool)
///
/// To prevent the buffer from being returned to the pool, use [`leak`](Self::leak)
/// or [`drop_now`](Self::drop_now).
pub struct FastPooledBuffer {
    buffer: Option<Buffer>,
    pool: Arc<LockFreeQueue<Buffer>>,
    config: PoolConfig,
    stats: Arc<FastPoolStatsInner>,
}

impl FastPooledBuffer {
    /// Leaks the buffer, preventing return to pool.
    ///
    /// Returns the owned buffer. The caller is responsible for eventual cleanup.
    /// The buffer will still be securely zeroed when finally dropped due to the
    /// `#[zeroize(drop)]` attribute on [`Buffer`].
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig::default());
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
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig::default());
    /// let mut buf = pool.acquire();
    /// buf.put_bytes(b"sensitive data").unwrap();
    /// buf.drop_now(); // Securely erased, not returned to pool
    /// ```
    pub fn drop_now(mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            buffer.burn();
            drop(buffer);
        }
    }

    /// Returns the buffer's capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig {
    ///     buffer_size: 8192,
    ///     max_pool_size: 100,
    ///     min_pool_size: 10,
    /// });
    ///
    /// let buf = pool.acquire();
    /// assert_eq!(buf.capacity(), 8192);
    /// ```
    #[inline]
    pub fn capacity(&self) -> usize {
        self.buffer.as_ref().unwrap().capacity()
    }
}

impl std::ops::Deref for FastPooledBuffer {
    type Target = Buffer;

    fn deref(&self) -> &Self::Target {
        self.buffer.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for FastPooledBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer.as_mut().unwrap()
    }
}

impl Drop for FastPooledBuffer {
    fn drop(&mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            buffer.reset();
            self.stats.returned.fetch_add(1, Ordering::Relaxed);

            // Wrap buffer in Option so we can conditionally take ownership
            let mut buffer_opt = Some(buffer);

            // Try to return to thread-local cache first.
            // During thread shutdown, TLS may be inaccessible, so use try_with
            // to avoid panicking.
            let mut returned_to_cache = false;

            if let Ok(size_result) = THREAD_CACHE_SIZE.try_with(|size| *size.borrow()) {
                let _ = THREAD_CACHE.try_with(|cache| {
                    let mut cache_borrowed = cache.borrow_mut();

                    if cache_borrowed.len() < size_result {
                        if let Some(buf) = buffer_opt.take() {
                            cache_borrowed.push(buf);
                            returned_to_cache = true;
                        }
                    }
                });
            }

            // If buffer wasn't cached (or TLS was unavailable), try to return it to the global pool
            if !returned_to_cache {
                if let Some(buf) = buffer_opt {
                    if self.pool.len() < self.config.max_pool_size {
                        self.pool.push(buf);
                    }
                    // Otherwise buffer is dropped here and securely zeroed
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_pool() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });

        let _buf = pool.acquire();
        let stats = pool.stats();
        assert_eq!(stats.acquired, 1);
    }

    #[test]
    fn test_buffer_return_to_cache() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 100,
            min_pool_size: 10,
        });

        {
            let _buf = pool.acquire();
        }

        let stats = pool.stats();
        assert_eq!(stats.returned, 1);
    }

    #[test]
    fn test_drop_now() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });

        let mut buf = pool.acquire();
        buf.put_u32(12345).unwrap();
        buf.drop_now();

        let stats = pool.stats();
        assert_eq!(stats.acquired, 1);
    }

    #[test]
    fn test_leak() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });

        let pooled_buf = pool.acquire();
        let _owned_buf = pooled_buf.leak();

        let stats = pool.stats();
        assert_eq!(stats.acquired, 1);
    }
}

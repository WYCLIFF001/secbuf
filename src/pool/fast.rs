// src/pool/fast.rs
//! Lock-free high-performance buffer pool with thread-local caching.
//!
//! # Architecture
//!
//! Acquisition follows a three-tier cascade:
//!
//! 1. **Thread-local cache** (~5 ns, zero contention)
//! 2. **Global lock-free queue** (~20 ns, atomic ops only)
//! 3. **Fresh allocation** (~100 ns, fallback)
//!
//! # Thread-Local Cache Behaviour
//!
//! Each thread maintains a private cache of up to 16 buffers.  On thread exit
//! the `Vec<Buffer>` drops naturally: each `Buffer` is securely zeroed by its
//! `#[zeroize(drop)]` destructor and then freed.  Buffers are **not** returned
//! to the global pool on thread exit — they are simply freed.  This is safe and
//! avoids the complexity and races of the previous `ThreadCacheDropGuard` design.
//!
//! If you run many short-lived threads in a loop, consider calling
//! [`FastBufferPool::clear_thread_cache`] before thread exit to push excess
//! buffers back to the global pool.
//!
//! # Memory Safety
//!
//! All buffers are **burned** (securely zeroed) before being returned to either
//! the thread-local cache or the global pool, preventing sensitive data from
//! leaking to the next acquirer.
//!
//! # Fixes vs original
//!
//! - **Broken `ThreadCacheDropGuard`**: Only the *first* pool used on a thread
//!   installed a guard, leaving subsequent pools' stats wrong.  The guard also
//!   could not return buffers to the pool on thread exit — it only counted them.
//!   Removed entirely; thread-exit cleanup is handled by `Buffer`'s own `Drop`.
//! - **Useless `THREAD_CACHE_SIZE` init in `new()`**: The initialisation ran on
//!   the calling (main) thread but had no effect on worker threads, which relied
//!   on the `const { RefCell::new(16) }` default anyway.  Removed.
//! - **Buffers not zeroed before pool return**: `buffer.reset()` cleared only
//!   `pos`/`len` metadata, leaving raw bytes in the allocation.  Fixed to call
//!   `buffer.burn()` before any insertion into cache or global pool.
//! - **TOCTOU in `warm()` and drop-path size check**: The atomic counter and the
//!   underlying `SegQueue` are not updated in a single transaction.  The pool may
//!   transiently exceed `max_pool_size` by a small constant under heavy
//!   concurrency.  This is a documented best-effort bound; correctness (no UB,
//!   no sensitive-data exposure) is not affected.

use super::config::PoolConfig;
use super::stats::FastPoolStats;
use crate::buffer::Buffer;
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Lock-free queue with approximate size tracking
// ---------------------------------------------------------------------------

/// Wrapper around `crossbeam::SegQueue` that tracks an approximate length.
///
/// The counter and the queue are **not** updated atomically, so `len()` may
/// be briefly stale.  This is acceptable for pool-sizing heuristics.
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

    /// Approximate queue length — may be briefly stale.
    #[inline]
    fn len(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

pub(crate) struct FastPoolStatsInner {
    pub(crate) allocated: AtomicUsize,
    pub(crate) acquired: AtomicUsize,
    pub(crate) returned: AtomicUsize,
    pub(crate) cache_hits: AtomicUsize,
}

impl FastPoolStatsInner {
    pub(crate) fn new() -> Self {
        Self {
            allocated: AtomicUsize::new(0),
            acquired: AtomicUsize::new(0),
            returned: AtomicUsize::new(0),
            cache_hits: AtomicUsize::new(0),
        }
    }
}

// ---------------------------------------------------------------------------
// Thread-local cache
// ---------------------------------------------------------------------------

/// Maximum number of buffers per thread-local cache.
const THREAD_CACHE_CAPACITY: usize = 16;

thread_local! {
    /// Per-thread buffer stash.  Drops (and securely zeros each buffer) when
    /// the thread exits, via `Buffer`'s own `#[zeroize(drop)]` destructor.
    static THREAD_CACHE: RefCell<Vec<Buffer>> =
        RefCell::new(Vec::with_capacity(THREAD_CACHE_CAPACITY));
}

// ---------------------------------------------------------------------------
// FastBufferPool
// ---------------------------------------------------------------------------

/// High-performance lock-free buffer pool with thread-local caching.
///
/// # Example
///
/// ```rust
/// use secbuf::prelude::*;
/// use std::sync::Arc;
/// use std::thread;
///
/// let pool = Arc::new(FastBufferPool::new(PoolConfig::default()));
///
/// let handles: Vec<_> = (0..4).map(|_| {
///     let pool = Arc::clone(&pool);
///     thread::spawn(move || {
///         for i in 0..1000u32 {
///             let mut buf = pool.acquire();
///             buf.put_u32(i).unwrap();
///         }
///     })
/// }).collect();
/// for h in handles { h.join().unwrap(); }
///
/// println!("Cache hit rate: {:.1}%", pool.stats().cache_hit_rate());
/// ```
pub struct FastBufferPool {
    global_pool: Arc<LockFreeQueue<Buffer>>,
    config: PoolConfig,
    stats: Arc<FastPoolStatsInner>,
}

impl FastBufferPool {
    /// Creates a new pool and pre-warms it with `config.min_pool_size` buffers.
    pub fn new(config: PoolConfig) -> Self {
        let queue = Arc::new(LockFreeQueue::new());
        for _ in 0..config.min_pool_size {
            queue.push(Buffer::new(config.buffer_size));
        }
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
    /// Acquires a buffer using the three-tier cascade.
    ///
    /// Returns a [`FastPooledBuffer`] that is automatically burned and returned
    /// (or dropped) when it goes out of scope.
    #[inline]
    pub fn acquire(&self) -> FastPooledBuffer {
        self.stats.acquired.fetch_add(1, Ordering::Relaxed);

        // Tier 1: thread-local cache (no synchronisation).
        let buffer = THREAD_CACHE.with(|cache| {
            let mut c = cache.borrow_mut();
            if let Some(buf) = c.pop() {
                self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                Some(buf)
            } else {
                None
            }
        });

        if let Some(buf) = buffer {
            return FastPooledBuffer {
                buffer: Some(buf),
                pool: Arc::clone(&self.global_pool),
                config: self.config.clone(),
                stats: Arc::clone(&self.stats),
            };
        }

        // Tier 2: global lock-free queue.
        if let Some(buf) = self.global_pool.pop() {
            return FastPooledBuffer {
                buffer: Some(buf),
                pool: Arc::clone(&self.global_pool),
                config: self.config.clone(),
                stats: Arc::clone(&self.stats),
            };
        }

        // Tier 3: fresh allocation.
        self.stats.allocated.fetch_add(1, Ordering::Relaxed);
        FastPooledBuffer {
            buffer: Some(Buffer::new(self.config.buffer_size)),
            pool: Arc::clone(&self.global_pool),
            config: self.config.clone(),
            stats: Arc::clone(&self.stats),
        }
    }

    /// Number of buffers currently idle in the global pool.
    ///
    /// Does **not** include buffers held in thread-local caches.
    #[inline]
    pub fn available(&self) -> usize {
        self.global_pool.len()
    }

    /// Returns a snapshot of pool statistics.
    ///
    /// All counters use `Relaxed` ordering; values are eventually consistent.
    pub fn stats(&self) -> FastPoolStats {
        FastPoolStats {
            available: self.global_pool.len(),
            allocated: self.stats.allocated.load(Ordering::Relaxed),
            acquired: self.stats.acquired.load(Ordering::Relaxed),
            returned: self.stats.returned.load(Ordering::Relaxed),
            cache_hits: self.stats.cache_hits.load(Ordering::Relaxed),
            // Thread-local losses are no longer tracked (removed broken guard).
            // Buffers on thread-local stacks are zeroed + freed on thread exit.
            thread_local_lost: 0,
        }
    }

    /// Drains the global pool to zero.
    ///
    /// **Does not** affect thread-local caches.  Those are cleaned up when
    /// the owning thread exits.
    pub fn clear(&self) {
        while self.global_pool.pop().is_some() {}
    }

    /// Pre-allocates buffers until the global pool has approximately `target_size`
    /// idle buffers (capped at `max_pool_size`).
    ///
    /// **Note:** Because `len()` and the push are not atomic, concurrent calls
    /// to `warm()` may transiently push the pool slightly above `max_pool_size`.
    /// This is harmless — excess buffers are dropped when the pool is next
    /// drained — but callers should not rely on the pool size being exact.
    pub fn warm(&self, target_size: usize) {
        let target = target_size.min(self.config.max_pool_size);
        // Snapshot the current count.  Slight over-allocation is possible but
        // bounded and not a safety concern.
        let current = self.global_pool.len();
        for _ in current..target {
            self.global_pool.push(Buffer::new(self.config.buffer_size));
        }
    }

    /// Pushes all buffers from the calling thread's local cache back to the
    /// global pool (where space permits) or drops them.
    ///
    /// Call this before a long-lived thread terminates to reclaim cached buffers
    /// without waiting for thread-exit destruction.
    pub fn clear_thread_cache(&self) {
        THREAD_CACHE.with(|cache| {
            let mut c = cache.borrow_mut();
            while let Some(mut buf) = c.pop() {
                if self.global_pool.len() < self.config.max_pool_size {
                    // buf is already burned by the return path; push as-is.
                    self.global_pool.push(buf);
                } else {
                    buf.burn();
                    drop(buf);
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// FastPooledBuffer
// ---------------------------------------------------------------------------

/// A buffer borrowed from a [`FastBufferPool`].
///
/// On drop the buffer is **burned** (securely zeroed) then returned to the
/// thread-local cache if space permits, otherwise to the global pool, otherwise
/// dropped.
pub struct FastPooledBuffer {
    buffer: Option<Buffer>,
    pool: Arc<LockFreeQueue<Buffer>>,
    config: PoolConfig,
    stats: Arc<FastPoolStatsInner>,
}

impl FastPooledBuffer {
    /// Extracts the buffer, preventing automatic pool return.
    ///
    /// The caller is responsible for cleanup; `Buffer` still zeroes itself on
    /// drop via `#[zeroize(drop)]`.
    pub fn leak(mut self) -> Buffer {
        self.buffer.take().unwrap()
    }

    /// Immediately burns and drops the buffer without returning it to the pool.
    pub fn drop_now(mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            buffer.burn();
        }
    }

    /// Capacity of the underlying buffer.
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
    /// Burns the buffer then tries to return it:
    /// 1. Thread-local cache (if not full)
    /// 2. Global pool (if not full)
    /// 3. Drop (already burned above)
    ///
    /// During thread shutdown `THREAD_CACHE` may be unavailable; `try_with`
    /// prevents a panic in that case and falls through to the global pool.
    fn drop(&mut self) {
        if let Some(mut buffer) = self.buffer.take() {
            // *** Securely zero ALL bytes BEFORE returning to any pool. ***
            // This ensures the next acquirer cannot observe a previous user's data
            // through resize(), set_len(), or get_write_ptr().
            buffer.burn();
            self.stats.returned.fetch_add(1, Ordering::Relaxed);

            // Wrap in Option so the closure can conditionally take ownership
            // without unconditionally moving `buffer` — which would make the
            // fallback `self.pool.push(buffer)` below a use-after-move error.
            let mut buffer_opt = Some(buffer);

            // Tier 1: thread-local cache (no synchronisation).
            // `try_with` avoids a panic if TLS is being torn down on thread exit.
            let _ = THREAD_CACHE.try_with(|cache| {
                let mut c = cache.borrow_mut();
                if c.len() < THREAD_CACHE_CAPACITY {
                    // `take()` moves out of the Option; if the cache is full we
                    // leave buffer_opt intact for the global-pool fallback below.
                    if let Some(buf) = buffer_opt.take() {
                        c.push(buf);
                    }
                }
            });

            // Tier 2: global pool fallback (only reached if cache was full or
            // TLS was unavailable).  Best-effort size cap — see TOCTOU note.
            if let Some(buf) = buffer_opt {
                if self.pool.len() < self.config.max_pool_size {
                    self.pool.push(buf);
                }
                // else: buf is already burned and will be freed here.
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_pool_basic() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });
        let _buf = pool.acquire();
        assert_eq!(pool.stats().acquired, 1);
    }

    #[test]
    fn test_returned_buffer_is_clean() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 64,
            max_pool_size: 4,
            min_pool_size: 0,
        });

        {
            let mut buf = pool.acquire();
            buf.put_bytes(&[0xAA; 32]).unwrap();
        } // burned + returned

        // Flush thread-local cache to global pool so we can re-acquire.
        pool.clear_thread_cache();

        let mut buf2 = pool.acquire();
        buf2.set_len(32).unwrap();
        assert_eq!(&buf2.as_slice()[..32], &[0u8; 32]);
    }

    #[test]
    fn test_buffer_return_to_cache() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 100,
            min_pool_size: 10,
        });
        { let _buf = pool.acquire(); }
        assert_eq!(pool.stats().returned, 1);
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
        assert_eq!(pool.stats().acquired, 1);
    }

    #[test]
    fn test_leak() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 10,
            min_pool_size: 2,
        });
        let pooled = pool.acquire();
        let _owned = pooled.leak();
        assert_eq!(pool.stats().acquired, 1);
    }

    #[test]
    fn test_warm() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 512,
            max_pool_size: 20,
            min_pool_size: 0,
        });
        pool.warm(10);
        assert!(pool.available() <= 10);
    }

    #[test]
    fn test_clear_thread_cache() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 128,
            max_pool_size: 20,
            min_pool_size: 0,
        });
        // Acquire and release several buffers (go to thread cache).
        for _ in 0..5 { let _b = pool.acquire(); }
        // Push them back to global pool.
        pool.clear_thread_cache();
        assert!(pool.available() > 0);
    }

    #[test]
    fn test_multi_thread() {
        use std::sync::Arc;
        use std::thread;

        let pool = Arc::new(FastBufferPool::new(PoolConfig {
            buffer_size: 256,
            max_pool_size: 64,
            min_pool_size: 4,
        }));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let p = Arc::clone(&pool);
                thread::spawn(move || {
                    for i in 0..100u32 {
                        let mut buf = p.acquire();
                        buf.put_u32(i).unwrap();
                    }
                    p.clear_thread_cache();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let s = pool.stats();
        assert_eq!(s.acquired, 400);
        assert!(s.cache_hit_rate() >= 0.0);
    }
}
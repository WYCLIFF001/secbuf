// src/pool/stats.rs
//! Statistics tracking for buffer pools.

/// Statistics for standard buffer pool.
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Number of buffers currently available in the pool
    pub available: usize,
    /// Total number of buffers allocated since pool creation
    pub total_allocated: usize,
    /// Total number of acquire() calls
    pub total_acquired: usize,
    /// Total number of buffers returned to pool
    pub total_returned: usize,
    /// Size of each buffer in bytes
    pub buffer_size: usize,
    /// Maximum number of buffers the pool can hold
    pub max_pool_size: usize,
}

impl PoolStats {
    /// Returns the number of buffers currently in use (acquired but not returned).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = BufferPool::new(PoolConfig::default());
    /// let _buf = pool.acquire();
    ///
    /// let stats = pool.stats();
    /// assert_eq!(stats.in_use(), 1);
    /// ```
    pub fn in_use(&self) -> usize {
        self.total_acquired.saturating_sub(self.total_returned)
    }

    /// Returns the pool hit rate as a percentage (0.0-100.0).
    ///
    /// A higher hit rate indicates better buffer reuse and fewer allocations.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = BufferPool::new(PoolConfig::default());
    /// for _ in 0..10 {
    ///     let _buf = pool.acquire();
    /// }
    ///
    /// let stats = pool.stats();
    /// assert!(stats.hit_rate() >= 0.0);
    /// assert!(stats.hit_rate() <= 100.0);
    /// ```
    pub fn hit_rate(&self) -> f64 {
        if self.total_acquired == 0 {
            return 0.0;
        }
        let reused = self.total_acquired.saturating_sub(self.total_allocated);
        (reused as f64 / self.total_acquired as f64) * 100.0
    }
}

/// Statistics for fast buffer pool with thread-local caching.
#[derive(Debug, Clone)]
pub struct FastPoolStats {
    /// Number of buffers currently available in the global pool
    pub available: usize,
    /// Total number of new buffers allocated
    pub allocated: usize,
    /// Total number of acquire() calls
    pub acquired: usize,
    /// Total number of buffers returned
    pub returned: usize,
    /// Number of times a buffer was acquired from thread-local cache
    pub cache_hits: usize,
    /// Number of buffers lost when threads exit with cached buffers
    pub thread_local_lost: usize,
}

impl FastPoolStats {
    /// Returns the number of buffers currently in use.
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
    /// assert_eq!(stats.in_use(), 1);
    /// ```
    pub fn in_use(&self) -> usize {
        self.acquired.saturating_sub(self.returned)
    }

    /// Returns the thread-local cache hit rate as a percentage.
    ///
    /// A higher cache hit rate indicates better thread-local cache utilization.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig::default());
    /// for _ in 0..10 {
    ///     let _buf = pool.acquire();
    /// }
    ///
    /// let stats = pool.stats();
    /// assert!(stats.cache_hit_rate() >= 0.0);
    /// assert!(stats.cache_hit_rate() <= 100.0);
    /// ```
    pub fn cache_hit_rate(&self) -> f64 {
        if self.acquired == 0 {
            return 0.0;
        }
        (self.cache_hits as f64 / self.acquired as f64) * 100.0
    }

    /// Returns the overall pool hit rate (cache + global pool) as a percentage.
    ///
    /// This indicates how often buffers were reused vs freshly allocated.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig::default());
    /// for _ in 0..10 {
    ///     let _buf = pool.acquire();
    /// }
    ///
    /// let stats = pool.stats();
    /// assert!(stats.pool_hit_rate() >= 0.0);
    /// assert!(stats.pool_hit_rate() <= 100.0);
    /// ```
    pub fn pool_hit_rate(&self) -> f64 {
        if self.acquired == 0 {
            return 0.0;
        }
        let hits = self.acquired.saturating_sub(self.allocated);
        (hits as f64 / self.acquired as f64) * 100.0
    }

    /// Estimates total leaked memory from dead threads.
    ///
    /// Returns the number of bytes lost to thread-local caches when threads exit.
    ///
    /// # Arguments
    ///
    /// * `buffer_size` - The size of each buffer in bytes
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
    /// let stats = pool.stats();
    /// let leaked_bytes = stats.leaked_bytes(8192);
    /// assert_eq!(leaked_bytes, stats.thread_local_lost * 8192);
    /// ```
    pub fn leaked_bytes(&self, buffer_size: usize) -> usize {
        self.thread_local_lost * buffer_size
    }

    /// Checks if the leak rate is concerning.
    ///
    /// Returns `true` if more than 10% of allocated buffers have been lost
    /// to dead threads.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let pool = FastBufferPool::new(PoolConfig::default());
    /// let stats = pool.stats();
    ///
    /// if stats.has_leak_concern() {
    ///     eprintln!("Warning: High buffer leak rate detected!");
    /// }
    /// ```
    pub fn has_leak_concern(&self) -> bool {
        if self.allocated == 0 {
            return false;
        }
        let leak_rate = (self.thread_local_lost as f64 / self.allocated as f64) * 100.0;
        leak_rate > 10.0
    }
}

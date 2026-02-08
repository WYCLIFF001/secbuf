// src/pool/config.rs
//! Configuration for buffer pools

/// Configuration for buffer pool behavior.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Size of each buffer in the pool (bytes)
    pub buffer_size: usize,
    /// Maximum number of buffers to keep in pool
    pub max_pool_size: usize,
    /// Number of buffers to pre-allocate at startup
    pub min_pool_size: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            buffer_size: 8192,  // 8KB buffers
            max_pool_size: 100, // Keep up to 100 buffers
            min_pool_size: 10,  // Pre-warm with 10
        }
    }
}

impl PoolConfig {
    /// Configuration for embedded systems (low memory).
    pub fn small() -> Self {
        Self {
            buffer_size: 1024,
            max_pool_size: 20,
            min_pool_size: 5,
        }
    }

    /// Configuration for high-throughput servers.
    pub fn large() -> Self {
        Self {
            buffer_size: 65536, // 64KB buffers
            max_pool_size: 1000,
            min_pool_size: 50,
        }
    }

    /// Configuration for network packet processing (MTU-sized).
    pub fn network() -> Self {
        Self {
            buffer_size: 1500, // Standard MTU
            max_pool_size: 500,
            min_pool_size: 20,
        }
    }
}

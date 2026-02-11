// src/lib.rs
//! # High-Performance Buffer Library
//!
//! A zero-copy, lock-free buffer management library optimized for network protocols,
//! data serialization, and high-throughput applications.
//!
//! Features:
//! - Secure memory zeroing using `zeroize` crate (compiler-resistant)
//! - Connection-scoped lifecycle management with automatic cleanup on drop
//! - Zero-copy circular buffers for streaming I/O
//! - Thread-safe buffer pooling with lock-free design
//! - Optional SIMD acceleration for bulk operations
//! - Automatic secure erasure on connection termination (mimics Dropbear behavior)

#![warn(missing_docs)]
#![warn(clippy::all)]
#![allow(clippy::missing_safety_doc)]

pub mod buffer;
pub mod circular;
pub mod connection;
pub mod error;
pub mod pool;

// Re-export main types
pub use buffer::Buffer;
pub use circular::CircularBuffer;
pub use connection::{ConnectionBufferConfig, ConnectionBuffers, PooledConnectionBuffers};
pub use error::{BufferError, Result};
pub use pool::{
    BufferPool, FastBufferPool, FastPoolStats, FastPooledBuffer, PoolConfig, PoolStats,
    PooledBuffer,
};

/// Commonly used imports.
pub mod prelude {
    pub use crate::buffer::Buffer;
    pub use crate::circular::CircularBuffer;
    pub use crate::connection::{ConnectionBufferConfig, ConnectionBuffers, PooledConnectionBuffers};
    pub use crate::error::{BufferError, Result};
    pub use crate::pool::{
        BufferPool, FastBufferPool, FastPoolStats, FastPooledBuffer, PoolConfig, PoolStats,
        PooledBuffer,
    };
}

#[cfg(test)]
mod tests {
    use super::prelude::*;

    #[test]
    fn test_basic_buffer() {
        let mut buf = Buffer::new(1024);
        buf.put_u32(42).unwrap();
        buf.put_byte(0xFF).unwrap();

        buf.set_pos(0).unwrap();
        assert_eq!(buf.get_u32().unwrap(), 42);
        assert_eq!(buf.get_byte().unwrap(), 0xFF);
    }

    #[test]
    fn test_standard_pool() {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 100,
            min_pool_size: 10,
        });

        let mut buffers = Vec::new();
        for i in 0..50 {
            let mut buf = pool.acquire();
            buf.put_u32(i).unwrap();
            buffers.push(buf);
        }

        drop(buffers);

        let stats = pool.stats();
        assert!(stats.total_acquired >= 50);
    }

    #[test]
    fn test_fast_pool() {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 100,
            min_pool_size: 10,
        });

        let mut buffers = Vec::new();
        for i in 0..50 {
            let mut buf = pool.acquire();
            buf.put_u32(i).unwrap();
            buffers.push(buf);
        }

        drop(buffers);

        let stats = pool.stats();
        assert!(stats.acquired >= 50);
    }

    #[test]
    fn test_circular_buffer() {
        let mut ring = CircularBuffer::new(256);

        ring.write(b"Chunk 1 | ").unwrap();
        ring.write(b"Chunk 2 | ").unwrap();
        ring.write(b"Chunk 3").unwrap();

        let mut output = vec![0u8; ring.used()];
        ring.read(&mut output).unwrap();

        assert_eq!(&output, b"Chunk 1 | Chunk 2 | Chunk 3");
    }

    #[test]
    fn test_unchecked_ops() {
        let mut buf = Buffer::new(1024);

        buf.put_u32(0x12345678).unwrap();
        buf.put_u64(0xDEADBEEFCAFEBABE).unwrap();

        unsafe {
            buf.put_u32_unchecked(0xABCDEF00);
        }

        buf.set_pos(0).unwrap();
        assert_eq!(buf.get_u32().unwrap(), 0x12345678);
        assert_eq!(buf.get_u64().unwrap(), 0xDEADBEEFCAFEBABE);
        assert_eq!(buf.get_u32().unwrap(), 0xABCDEF00);
    }
}
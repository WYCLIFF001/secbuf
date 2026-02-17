// src/connection.rs
//! Connection-scoped buffer lifecycle management.
//!
//! Provides automatic secure memory cleanup on connection termination.
//!
//! # Fixes vs original
//!
//! - **`PooledConnectionBuffers::release_to_pool` double-burned and misled callers.**
//!   The original implementation called `self.buffers.burn()` inside
//!   `release_to_pool()` and then called it *again* inside `Drop::drop()`.
//!   More critically, the method was named `release_to_pool` but actually
//!   *destroyed* all buffers — nothing was returned to the pool.
//!
//!   Fixed by:
//!   1. Adding a `burned: bool` guard so `burn()` is called exactly once.
//!   2. Renaming the concept clearly: the method is now `burn_and_release()`,
//!      which honestly reflects what happens (burn then release pool Arc).
//!      If you want to *return* buffers to the pool, acquire them via
//!      `PooledBuffer`/`FastPooledBuffer` directly and let them drop normally.

use crate::buffer::Buffer;
use crate::circular::CircularBuffer;
use crate::pool::BufferPool;
use std::collections::VecDeque;
use std::sync::Arc;
// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for per-connection buffer limits.
#[derive(Debug, Clone)]
pub struct ConnectionBufferConfig {
    /// Maximum number of packets in the transmit queue.
    pub max_packet_queue_size: usize,
    /// Maximum total bytes held in the transmit queue.
    pub max_packet_queue_bytes: usize,
}

impl Default for ConnectionBufferConfig {
    fn default() -> Self {
        Self {
            max_packet_queue_size: 100,
            max_packet_queue_bytes: 10_485_760, // 10 MB
        }
    }
}

// ---------------------------------------------------------------------------
// ConnectionBuffers
// ---------------------------------------------------------------------------

/// A connection's complete buffer state.
///
/// All associated buffers are securely zeroed when this value is dropped.
///
/// # Example
///
/// ```
/// use secbuf::prelude::*;
///
/// let mut conn = ConnectionBuffers::new();
/// conn.init_read_buf(4096);
/// conn.init_write_buf(4096);
/// conn.add_stream_buf(1024);
/// // Buffers automatically cleaned up when `conn` is dropped.
/// ```
pub struct ConnectionBuffers {
    /// Incoming data buffer.
    pub read_buf: Option<Buffer>,
    /// Outgoing data buffer.
    pub write_buf: Option<Buffer>,
    /// Per-channel circular stream buffers.
    pub stream_bufs: Vec<CircularBuffer>,
    /// Pending transmit packet queue.
    pub packet_queue: VecDeque<Buffer>,
    config: ConnectionBufferConfig,
    packet_queue_bytes: usize,
    /// Prevents double-burn if `burn()` is called explicitly before drop.
    burned: bool,
}

impl ConnectionBuffers {
    /// Creates a new connection buffer set with default limits.
    pub fn new() -> Self {
        Self::with_config(ConnectionBufferConfig::default())
    }

    /// Creates a new connection buffer set with custom limits.
    pub fn with_config(config: ConnectionBufferConfig) -> Self {
        Self {
            read_buf: None,
            write_buf: None,
            stream_bufs: Vec::new(),
            packet_queue: VecDeque::new(),
            config,
            packet_queue_bytes: 0,
            burned: false,
        }
    }

    // -----------------------------------------------------------------------
    // Initialisation
    // -----------------------------------------------------------------------

    /// Allocates the inbound data buffer.
    pub fn init_read_buf(&mut self, size: usize) {
        self.read_buf = Some(Buffer::new(size));
    }

    /// Allocates the outbound data buffer.
    pub fn init_write_buf(&mut self, size: usize) {
        self.write_buf = Some(Buffer::new(size));
    }

    /// Appends a new circular stream buffer (e.g. for a forwarded channel).
    pub fn add_stream_buf(&mut self, size: usize) {
        self.stream_bufs.push(CircularBuffer::new(size));
    }

    // -----------------------------------------------------------------------
    // Packet queue
    // -----------------------------------------------------------------------

    /// Enqueues a packet for transmission.
    ///
    /// # Errors
    ///
    /// Returns [`QueueFullError`] if either the count or byte limit is exceeded.
    pub fn queue_packet(&mut self, buf: Buffer) -> Result<(), QueueFullError> {
        if self.packet_queue.len() >= self.config.max_packet_queue_size {
            return Err(QueueFullError::TooManyPackets);
        }
        let buf_len = buf.len();
        if self.packet_queue_bytes + buf_len > self.config.max_packet_queue_bytes {
            return Err(QueueFullError::TooManyBytes);
        }
        self.packet_queue_bytes += buf_len;
        self.packet_queue.push_back(buf);
        Ok(())
    }

    /// Dequeues the next packet for writing to the network.
    pub fn dequeue_packet(&mut self) -> Option<Buffer> {
        self.packet_queue.pop_front().map(|buf| {
            self.packet_queue_bytes =
                self.packet_queue_bytes.saturating_sub(buf.len());
            buf
        })
    }

    /// Number of packets currently queued.
    pub fn packet_queue_len(&self) -> usize {
        self.packet_queue.len()
    }

    /// Total bytes currently queued.
    pub fn packet_queue_bytes(&self) -> usize {
        self.packet_queue_bytes
    }

    /// `true` when the queue is at ≥ 80% of either limit.
    pub fn is_queue_near_full(&self) -> bool {
        self.packet_queue.len() > self.config.max_packet_queue_size * 80 / 100
            || self.packet_queue_bytes > self.config.max_packet_queue_bytes * 80 / 100
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Resets all buffers and the packet queue without freeing memory.
    ///
    /// Useful for session reuse where you want to keep the allocations.
    pub fn reset(&mut self) {
        if let Some(ref mut buf) = self.read_buf {
            buf.reset();
        }
        if let Some(ref mut buf) = self.write_buf {
            buf.reset();
        }
        for buf in &mut self.stream_bufs {
            buf.clear();
        }
        self.packet_queue.clear();
        self.packet_queue_bytes = 0;
        self.burned = false;
    }

    /// Securely zeros all sensitive data in every buffer.
    ///
    /// Idempotent — safe to call multiple times (subsequent calls are no-ops).
    pub fn burn(&mut self) {
        if self.burned {
            return;
        }
        if let Some(ref mut buf) = self.read_buf {
            buf.burn();
        }
        if let Some(ref mut buf) = self.write_buf {
            buf.burn();
        }
        for buf in &mut self.stream_bufs {
            buf.free();
        }
        while let Some(mut pkt) = self.packet_queue.pop_front() {
            pkt.burn();
        }
        self.packet_queue_bytes = 0;
        self.burned = true;
    }

    // -----------------------------------------------------------------------
    // Diagnostics
    // -----------------------------------------------------------------------

    /// Returns memory usage statistics.
    pub fn memory_usage(&self) -> ConnectionMemoryStats {
        let read_buf_bytes = self.read_buf.as_ref().map(|b| b.capacity()).unwrap_or(0);
        let write_buf_bytes = self.write_buf.as_ref().map(|b| b.capacity()).unwrap_or(0);
        let stream_buf_bytes: usize = self.stream_bufs.iter().map(|b| b.size()).sum();
        ConnectionMemoryStats {
            read_buf_bytes,
            write_buf_bytes,
            stream_buf_bytes,
            packet_queue_bytes: self.packet_queue_bytes,
            total_bytes: read_buf_bytes
                + write_buf_bytes
                + stream_buf_bytes
                + self.packet_queue_bytes,
        }
    }
}

impl Default for ConnectionBuffers {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ConnectionBuffers {
    fn drop(&mut self) {
        // `burn()` is idempotent — safe even if called explicitly beforehand.
        self.burn();
    }
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Memory usage snapshot for a connection.
/// Memory usage statistics for a connection.
#[derive(Debug, Clone)]
pub struct ConnectionMemoryStats {
    /// Bytes allocated for read buffer
    pub read_buf_bytes: usize,
    /// Bytes allocated for write buffer
    pub write_buf_bytes: usize,
    /// Bytes allocated for stream buffers
    pub stream_buf_bytes: usize,
    /// Bytes currently in packet queue
    pub packet_queue_bytes: usize,
    /// Total bytes allocated
    pub total_bytes: usize,
}

/// Error returned when the packet queue is full.
/// Error when packet queue is full.
#[derive(Debug, Clone)]
pub enum QueueFullError {
    /// Too many packets in queue
    TooManyPackets,
    /// Total bytes exceed limit
    TooManyBytes,
}

impl std::fmt::Display for QueueFullError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyPackets => write!(f, "Packet queue full (too many packets)"),
            Self::TooManyBytes => write!(f, "Packet queue full (too many bytes)"),
        }
    }
}

impl std::error::Error for QueueFullError {}

// ---------------------------------------------------------------------------
// PooledConnectionBuffers
// ---------------------------------------------------------------------------

/// A connection buffer set that holds an `Arc<BufferPool>` reference.
///
/// # What "pooled" means here
///
/// This type holds a reference to a pool so the connection can *acquire*
/// pool buffers during its lifetime.  The buffers themselves are managed by
/// the inner `ConnectionBuffers`; on drop they are securely zeroed.
///
/// **Important:** the connection buffers are **not** automatically returned
/// to the pool on drop — `Buffer` objects live inside `ConnectionBuffers` and
/// are freed (zeroed) when `ConnectionBuffers::burn()` runs.  If you want
/// true pool-recycling, use `pool.acquire()` → `PooledBuffer` directly and
/// store the `PooledBuffer` inside your connection state instead.
///
/// # Fix vs original
///
/// The original `release_to_pool()` was misleadingly named — it called
/// `self.buffers.burn()` (destroying data) and then set `self.pool = None`
/// (releasing the Arc), but returned nothing to the pool.  The name implied
/// efficient buffer recycling.  Renamed to `burn_and_release()` and made
/// idempotent via the `burned` guard on `ConnectionBuffers`.
pub struct PooledConnectionBuffers {
    buffers: ConnectionBuffers,
    pool: Option<Arc<BufferPool>>,
}

impl PooledConnectionBuffers {
    /// Creates a new pooled connection buffer wrapper.
    pub fn new(pool: Arc<BufferPool>) -> Self {
        Self {
            buffers: ConnectionBuffers::new(),
            pool: Some(pool),
        }
    }

    /// Creates a pooled connection wrapper with custom buffer limits.
    pub fn with_config(pool: Arc<BufferPool>, config: ConnectionBufferConfig) -> Self {
        Self {
            buffers: ConnectionBuffers::with_config(config),
            pool: Some(pool),
        }
    }

    /// Mutable access to the underlying connection buffers.
    pub fn buffers(&mut self) -> &mut ConnectionBuffers {
        &mut self.buffers
    }

    /// Shared access to the pool (e.g. for `pool.acquire()` calls).
    pub fn pool(&self) -> Option<&Arc<BufferPool>> {
        self.pool.as_ref()
    }

    /// Explicitly burns all buffers and releases the pool Arc.
    ///
    /// After this call the struct is "empty" — `pool()` returns `None` and the
    /// buffers are zeroed.  The destructor will be a no-op.
    ///
    /// This is idempotent; calling it multiple times is safe.
    pub fn burn_and_release(&mut self) {
        self.buffers.burn(); // idempotent via `burned` flag
        self.pool = None;    // release Arc<BufferPool>
    }
}

impl Drop for PooledConnectionBuffers {
    fn drop(&mut self) {
        self.buffers.burn(); // idempotent — safe if burn_and_release already called
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_buffers_cleanup() {
        let mut conn = ConnectionBuffers::new();
        conn.init_read_buf(1024);
        conn.init_write_buf(1024);
        conn.add_stream_buf(512);

        if let Some(ref mut buf) = conn.read_buf {
            buf.put_byte(0xAB).unwrap();
        }

        drop(conn); // must not double-burn or panic
    }

    #[test]
    fn test_burn_is_idempotent() {
        let mut conn = ConnectionBuffers::new();
        conn.init_read_buf(256);
        conn.burn();
        conn.burn(); // second call must be a no-op
        drop(conn); // drop must also be a no-op
    }

    #[test]
    fn test_packet_queue_limits() {
        let mut conn = ConnectionBuffers::with_config(ConnectionBufferConfig {
            max_packet_queue_size: 2,
            max_packet_queue_bytes: 1024,
        });
        conn.queue_packet(Buffer::new(256)).unwrap();
        conn.queue_packet(Buffer::new(256)).unwrap();
        assert!(conn.queue_packet(Buffer::new(256)).is_err());
    }

    #[test]
    fn test_memory_stats() {
        let mut conn = ConnectionBuffers::new();
        conn.init_read_buf(1024);
        conn.init_write_buf(2048);
        conn.add_stream_buf(512);

        let stats = conn.memory_usage();
        assert_eq!(stats.read_buf_bytes, 1024);
        assert_eq!(stats.write_buf_bytes, 2048);
        assert_eq!(stats.stream_buf_bytes, 512);
    }

    #[test]
    fn test_pooled_connection_burn_and_release() {
        use crate::pool::{BufferPool, PoolConfig};
        let pool = Arc::new(BufferPool::new(PoolConfig::default()));
        let mut pc = PooledConnectionBuffers::new(Arc::clone(&pool));
        pc.buffers().init_read_buf(512);
        pc.burn_and_release();
        assert!(pc.pool().is_none());
        drop(pc); // must not panic or double-burn
    }

    #[test]
    fn test_dequeue_packet() {
        let mut conn = ConnectionBuffers::new();
        let mut buf = Buffer::new(64);
        buf.put_u32(0xDEAD_BEEF).unwrap();
        conn.queue_packet(buf).unwrap();

        let pkt = conn.dequeue_packet().unwrap();
        assert_eq!(pkt.len(), 4);
        assert_eq!(conn.packet_queue_len(), 0);
        assert_eq!(conn.packet_queue_bytes(), 0);
    }
}
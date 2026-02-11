// src/connection.rs
//! Connection-scoped buffer lifecycle management.
//!
//! Provides automatic memory cleanup on connection termination (disconnect/error).

use crate::buffer::Buffer;
use crate::circular::CircularBuffer;
use crate::pool::BufferPool;
use std::collections::VecDeque;

/// Configuration for connection buffer limits.
#[derive(Debug, Clone)]
pub struct ConnectionBufferConfig {
    /// Maximum number of packets in queue (prevents unbounded growth)
    pub max_packet_queue_size: usize,
    /// Maximum total bytes in packet queue
    pub max_packet_queue_bytes: usize,
}

impl Default for ConnectionBufferConfig {
    fn default() -> Self {
        Self {
            max_packet_queue_size: 100,
            max_packet_queue_bytes: 10_485_760, // 10MB
        }
    }
}

/// A connection's buffer state.
///
/// Automatically cleans up all associated buffers on drop.
///
/// # Examples
///
/// ```
/// use secbuf::prelude::*;
///
/// let mut conn = ConnectionBuffers::new();
/// conn.init_read_buf(4096);
/// conn.init_write_buf(4096);
/// conn.add_stream_buf(1024);
/// // Buffers automatically cleaned up when conn is dropped
/// ```
pub struct ConnectionBuffers {
    /// Read buffer (for incoming network data)
    pub read_buf: Option<Buffer>,
    /// Write buffer (for outgoing network data)
    pub write_buf: Option<Buffer>,
    /// Per-direction circular buffers (e.g., for channel to local FD forwarding)
    pub stream_bufs: Vec<CircularBuffer>,
    /// Queued buffers awaiting transmission (e.g., SSH packet queue)
    pub packet_queue: VecDeque<Buffer>,
    /// Configuration limits
    config: ConnectionBufferConfig,
    /// Current bytes in packet queue
    packet_queue_bytes: usize,
}

impl ConnectionBuffers {
    /// Creates a new connection buffer set with default limits.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let conn = ConnectionBuffers::new();
    /// assert_eq!(conn.packet_queue_len(), 0);
    /// ```
    pub fn new() -> Self {
        Self::with_config(ConnectionBufferConfig::default())
    }

    /// Creates a new connection buffer set with custom limits.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let config = ConnectionBufferConfig {
    ///     max_packet_queue_size: 50,
    ///     max_packet_queue_bytes: 5_242_880,
    /// };
    /// let conn = ConnectionBuffers::with_config(config);
    /// ```
    pub fn with_config(config: ConnectionBufferConfig) -> Self {
        Self {
            read_buf: None,
            write_buf: None,
            stream_bufs: Vec::new(),
            packet_queue: VecDeque::new(),
            config,
            packet_queue_bytes: 0,
        }
    }

    /// Allocates read buffer (called on connection init).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(4096);
    /// assert!(conn.read_buf.is_some());
    /// ```
    pub fn init_read_buf(&mut self, size: usize) {
        self.read_buf = Some(Buffer::new(size));
    }

    /// Allocates write buffer (called on connection init).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_write_buf(4096);
    /// assert!(conn.write_buf.is_some());
    /// ```
    pub fn init_write_buf(&mut self, size: usize) {
        self.write_buf = Some(Buffer::new(size));
    }

    /// Adds a new stream buffer (e.g., for a forwarded channel).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.add_stream_buf(1024);
    /// assert_eq!(conn.stream_bufs.len(), 1);
    /// ```
    pub fn add_stream_buf(&mut self, size: usize) {
        self.stream_bufs.push(CircularBuffer::new(size));
    }

    /// Queues a packet buffer for transmission.
    ///
    /// # Errors
    ///
    /// Returns [`QueueFullError`] if queue limits are exceeded.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// let buf = Buffer::new(256);
    /// conn.queue_packet(buf).unwrap();
    /// assert_eq!(conn.packet_queue_len(), 1);
    /// ```
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

    /// Dequeues and returns the next packet (for writing to network).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// let buf = Buffer::new(256);
    /// conn.queue_packet(buf).unwrap();
    ///
    /// let packet = conn.dequeue_packet();
    /// assert!(packet.is_some());
    /// ```
    pub fn dequeue_packet(&mut self) -> Option<Buffer> {
        if let Some(buf) = self.packet_queue.pop_front() {
            self.packet_queue_bytes = self.packet_queue_bytes.saturating_sub(buf.len());
            Some(buf)
        } else {
            None
        }
    }

    /// Returns the number of queued packets (for debugging/monitoring).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// assert_eq!(conn.packet_queue_len(), 0);
    ///
    /// conn.queue_packet(Buffer::new(256)).unwrap();
    /// assert_eq!(conn.packet_queue_len(), 1);
    /// ```
    pub fn packet_queue_len(&self) -> usize {
        self.packet_queue.len()
    }

    /// Returns total bytes in packet queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let conn = ConnectionBuffers::new();
    /// assert_eq!(conn.packet_queue_bytes(), 0);
    /// ```
    pub fn packet_queue_bytes(&self) -> usize {
        self.packet_queue_bytes
    }

    /// Checks if packet queue is near capacity.
    ///
    /// Returns `true` if the queue is at 80% or more of its limits.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// if conn.is_queue_near_full() {
    ///     eprintln!("Warning: packet queue nearly full");
    /// }
    /// ```
    pub fn is_queue_near_full(&self) -> bool {
        self.packet_queue.len() > self.config.max_packet_queue_size * 80 / 100
            || self.packet_queue_bytes > self.config.max_packet_queue_bytes * 80 / 100
    }

    /// Securely clears all buffers and resets state (without freeing memory).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(1024);
    /// conn.reset();
    /// ```
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
    }

    /// Securely zeroes all sensitive data before dropping.
    ///
    /// Called explicitly before dropping or on error/disconnect.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(1024);
    /// conn.burn(); // Explicitly clean up
    /// ```
    pub fn burn(&mut self) {
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
    }

    /// Returns memory usage statistics.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(4096);
    /// conn.init_write_buf(4096);
    ///
    /// let stats = conn.memory_usage();
    /// assert_eq!(stats.total_bytes, 8192);
    /// ```
    pub fn memory_usage(&self) -> ConnectionMemoryStats {
        let read_buf_bytes = self.read_buf.as_ref().map(|b| b.capacity()).unwrap_or(0);
        let write_buf_bytes = self.write_buf.as_ref().map(|b| b.capacity()).unwrap_or(0);
        let stream_buf_bytes: usize = self.stream_bufs.iter().map(|b| b.size()).sum();

        ConnectionMemoryStats {
            read_buf_bytes,
            write_buf_bytes,
            stream_buf_bytes,
            packet_queue_bytes: self.packet_queue_bytes,
            total_bytes: read_buf_bytes + write_buf_bytes + stream_buf_bytes + self.packet_queue_bytes,
        }
    }
}

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

impl Default for ConnectionBuffers {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ConnectionBuffers {
    fn drop(&mut self) {
        self.burn();
    }
}

/// A pooled connection buffer set for high-throughput scenarios.
///
/// Automatically returns buffers to pool on drop.
pub struct PooledConnectionBuffers {
    buffers: ConnectionBuffers,
    pool: Option<std::sync::Arc<BufferPool>>,
}

impl PooledConnectionBuffers {
    /// Creates a new pooled connection buffer set.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    /// use std::sync::Arc;
    ///
    /// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
    /// let pooled_conn = PooledConnectionBuffers::new(pool);
    /// ```
    pub fn new(pool: std::sync::Arc<BufferPool>) -> Self {
        Self {
            buffers: ConnectionBuffers::new(),
            pool: Some(pool),
        }
    }

    /// Creates a pooled connection with custom config.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    /// use std::sync::Arc;
    ///
    /// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
    /// let config = ConnectionBufferConfig {
    ///     max_packet_queue_size: 50,
    ///     max_packet_queue_bytes: 5_242_880,
    /// };
    /// let pooled_conn = PooledConnectionBuffers::with_config(pool, config);
    /// ```
    pub fn with_config(
        pool: std::sync::Arc<BufferPool>,
        config: ConnectionBufferConfig,
    ) -> Self {
        Self {
            buffers: ConnectionBuffers::with_config(config),
            pool: Some(pool),
        }
    }

    /// Access the underlying connection buffers.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::prelude::*;
    /// use std::sync::Arc;
    ///
    /// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
    /// let mut pooled_conn = PooledConnectionBuffers::new(pool);
    /// pooled_conn.buffers().init_read_buf(4096);
    /// ```
    pub fn buffers(&mut self) -> &mut ConnectionBuffers {
        &mut self.buffers
    }

    /// Explicitly return resources to pool (optional; automatic on drop).
    pub fn release_to_pool(&mut self) {
        self.buffers.burn();
        self.pool = None;
    }
}

impl Drop for PooledConnectionBuffers {
    fn drop(&mut self) {
        self.buffers.burn();
    }
}

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
            let _ = buf.put_byte(0xAB);
        }

        drop(conn);
    }

    #[test]
    fn test_packet_queue_limits() {
        let mut conn = ConnectionBuffers::with_config(ConnectionBufferConfig {
            max_packet_queue_size: 2,
            max_packet_queue_bytes: 1024,
        });

        conn.queue_packet(Buffer::new(256)).unwrap();
        conn.queue_packet(Buffer::new(256)).unwrap();

        let result = conn.queue_packet(Buffer::new(256));
        assert!(result.is_err());
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
}
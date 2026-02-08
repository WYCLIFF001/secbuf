// src/connection.rs - Enhanced with aggressive memory management
//! Connection-scoped buffer lifecycle with aggressive cleanup

use crate::buffer::Buffer;
use crate::circular::CircularBuffer;
use crate::pool::BufferPool;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Configuration for connection buffer limits.
#[derive(Debug, Clone)]
pub struct ConnectionBufferConfig {
    /// Maximum number of packets in queue
    pub max_packet_queue_size: usize,
    /// Maximum total bytes in packet queue
    pub max_packet_queue_bytes: usize,
    /// Idle timeout before aggressive cleanup (seconds)
    pub idle_timeout_secs: u64,
    /// Enable aggressive memory shrinking
    pub enable_aggressive_shrinking: bool,
}

impl Default for ConnectionBufferConfig {
    fn default() -> Self {
        Self {
            max_packet_queue_size: 100,
            max_packet_queue_bytes: 10_485_760, // 10MB
            idle_timeout_secs: 60,
            enable_aggressive_shrinking: true,
        }
    }
}

/// A connection's buffer state with aggressive memory management.
///
/// Enhanced features:
/// - Automatic cleanup on drop
/// - Idle detection and memory reclamation
/// - Periodic shrinking of oversized buffers
/// - Memory pressure monitoring
pub struct ConnectionBuffers {
    /// Read buffer
    pub read_buf: Option<Buffer>,
    /// Write buffer
    pub write_buf: Option<Buffer>,
    /// Stream buffers
    pub stream_bufs: Vec<CircularBuffer>,
    /// Packet queue
    pub packet_queue: VecDeque<Buffer>,
    /// Configuration
    config: ConnectionBufferConfig,
    /// Current bytes in packet queue
    packet_queue_bytes: usize,
    /// Last activity timestamp
    last_activity: Instant,
    /// Cleanup counter for periodic operations
    cleanup_counter: usize,
}

impl ConnectionBuffers {
    /// Creates a new connection buffers instance with default configuration.
    ///
    /// All buffers are initially unallocated. Call initialization methods
    /// to allocate specific buffers as needed.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let conn = ConnectionBuffers::new();
    /// assert_eq!(conn.packet_queue_len(), 0);
    /// ```
    pub fn new() -> Self {
        Self::with_config(ConnectionBufferConfig::default())
    }

    /// Creates a new connection buffers instance with custom configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for buffer limits and cleanup behavior
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::{ConnectionBuffers, ConnectionBufferConfig};
    ///
    /// let config = ConnectionBufferConfig {
    ///     max_packet_queue_size: 50,
    ///     max_packet_queue_bytes: 5_242_880, // 5MB
    ///     idle_timeout_secs: 30,
    ///     enable_aggressive_shrinking: true,
    /// };
    ///
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
            last_activity: Instant::now(),
            cleanup_counter: 0,
        }
    }

    /// Initializes the read buffer with the specified size.
    ///
    /// Creates and allocates a buffer for reading incoming data.
    /// Updates the last activity timestamp.
    ///
    /// # Arguments
    ///
    /// * `size` - Buffer size in bytes
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(8192); // 8KB read buffer
    /// ```
    pub fn init_read_buf(&mut self, size: usize) {
        self.read_buf = Some(Buffer::new(size));
        self.touch();
    }

    /// Initializes the write buffer with the specified size.
    ///
    /// Creates and allocates a buffer for outgoing data.
    /// Updates the last activity timestamp.
    ///
    /// # Arguments
    ///
    /// * `size` - Buffer size in bytes
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_write_buf(8192); // 8KB write buffer
    /// ```
    pub fn init_write_buf(&mut self, size: usize) {
        self.write_buf = Some(Buffer::new(size));
        self.touch();
    }

    /// Adds a new circular buffer for streaming data.
    ///
    /// Circular buffers are ideal for continuous data streams like
    /// SSH channels or multiplexed connections.
    ///
    /// # Arguments
    ///
    /// * `size` - Buffer size in bytes
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.add_stream_buf(4096); // Add 4KB stream buffer
    /// conn.add_stream_buf(4096); // Add another
    /// ```
    pub fn add_stream_buf(&mut self, size: usize) {
        self.stream_bufs.push(CircularBuffer::new(size));
        self.touch();
    }

    /// Queues a packet for later processing.
    ///
    /// Useful for buffering incoming packets when processing is deferred
    /// or rate-limited.
    ///
    /// # Arguments
    ///
    /// * `buf` - Buffer containing the packet data
    ///
    /// # Errors
    ///
    /// Returns [`QueueFullError::TooManyPackets`] if packet count limit is reached.
    /// Returns [`QueueFullError::TooManyBytes`] if total bytes limit is reached.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::{ConnectionBuffers, Buffer};
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// let mut packet = Buffer::new(1024);
    /// packet.put_bytes(b"packet data")?;
    ///
    /// conn.queue_packet(packet)?;
    /// assert_eq!(conn.packet_queue_len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
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
        self.touch();
        Ok(())
    }

    /// Dequeues the next packet from the queue.
    ///
    /// Returns `None` if the queue is empty.
    ///
    /// # Returns
    ///
    /// The oldest queued packet, or `None` if queue is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::{ConnectionBuffers, Buffer};
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// let packet = Buffer::new(1024);
    /// conn.queue_packet(packet)?;
    ///
    /// let dequeued = conn.dequeue_packet();
    /// assert!(dequeued.is_some());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn dequeue_packet(&mut self) -> Option<Buffer> {
        if let Some(buf) = self.packet_queue.pop_front() {
            self.packet_queue_bytes = self.packet_queue_bytes.saturating_sub(buf.len());
            self.touch();
            Some(buf)
        } else {
            None
        }
    }

    /// Returns the number of packets in the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let conn = ConnectionBuffers::new();
    /// assert_eq!(conn.packet_queue_len(), 0);
    /// ```
    pub fn packet_queue_len(&self) -> usize {
        self.packet_queue.len()
    }

    /// Returns the total bytes currently in the packet queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let conn = ConnectionBuffers::new();
    /// assert_eq!(conn.packet_queue_bytes(), 0);
    /// ```
    pub fn packet_queue_bytes(&self) -> usize {
        self.packet_queue_bytes
    }

    /// Returns `true` if the packet queue is near capacity (>80% full).
    ///
    /// Use this to implement backpressure or flow control.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let conn = ConnectionBuffers::new();
    /// if conn.is_queue_near_full() {
    ///     // Implement backpressure
    /// }
    /// ```
    pub fn is_queue_near_full(&self) -> bool {
        self.packet_queue.len() > self.config.max_packet_queue_size * 80 / 100
            || self.packet_queue_bytes > self.config.max_packet_queue_bytes * 80 / 100
    }

    /// Updates the last activity timestamp.
    ///
    /// Called automatically by operations that use the buffers.
    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Returns the duration since last activity.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    /// use std::time::Duration;
    ///
    /// let conn = ConnectionBuffers::new();
    /// let idle = conn.idle_duration();
    /// assert!(idle < Duration::from_secs(1));
    /// ```
    pub fn idle_duration(&self) -> Duration {
        self.last_activity.elapsed()
    }

    /// Returns `true` if connection has been idle beyond the configured threshold.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::{ConnectionBuffers, ConnectionBufferConfig};
    ///
    /// let config = ConnectionBufferConfig {
    ///     idle_timeout_secs: 30,
    ///     ..Default::default()
    /// };
    /// let conn = ConnectionBuffers::with_config(config);
    ///
    /// if conn.is_idle() {
    ///     // Perform cleanup
    /// }
    /// ```
    pub fn is_idle(&self) -> bool {
        self.idle_duration() > Duration::from_secs(self.config.idle_timeout_secs)
    }

    /// Resets buffers **without** zeroing memory.
    ///
    /// Use for performance when buffer reuse is expected. All buffers are cleared
    /// but memory allocations are retained.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(1024);
    /// conn.reset(); // Clear but keep allocation
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
        self.touch();
    }

    /// Securely zeros all sensitive data.
    ///
    /// All buffer contents are overwritten with zeros using secure zeroing.
    /// Allocations are retained for reuse.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(1024);
    /// conn.burn(); // Securely zero all data
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

    /// Aggressive cleanup: zeros and frees all memory.
    ///
    /// All buffers are securely zeroed and their memory is released.
    /// This is the most thorough cleanup operation.
    ///
    /// # Use Cases
    ///
    /// - Connection termination
    /// - Memory pressure situations
    /// - Long idle periods
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(8192);
    /// conn.init_write_buf(8192);
    ///
    /// conn.aggressive_cleanup(); // Free everything
    /// ```
    pub fn aggressive_cleanup(&mut self) {
        // Burn and free all buffers
        if let Some(mut buf) = self.read_buf.take() {
            buf.burn_and_free_memory();
        }
        if let Some(mut buf) = self.write_buf.take() {
            buf.burn_and_free_memory();
        }

        // Free all stream buffers
        for buf in self.stream_bufs.drain(..) {
            drop(buf); // Will trigger secure drop
        }
        self.stream_bufs.shrink_to_fit();

        // Burn all packets
        while let Some(mut pkt) = self.packet_queue.pop_front() {
            pkt.burn();
        }
        self.packet_queue.shrink_to_fit();
        self.packet_queue_bytes = 0;
    }

    /// Periodic maintenance: shrink oversized buffers if idle.
    ///
    /// Call this periodically (e.g., every minute) to reclaim memory from idle connections.
    /// Only performs shrinking if aggressive shrinking is enabled and connection is idle.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(65536);
    ///
    /// // Call periodically
    /// conn.periodic_cleanup(); // May shrink if idle
    /// ```
    pub fn periodic_cleanup(&mut self) {
        self.cleanup_counter += 1;

        if !self.config.enable_aggressive_shrinking {
            return;
        }

        // Only cleanup if idle
        if !self.is_idle() {
            return;
        }

        // Cleanup read buffer if wasteful
        if let Some(ref mut buf) = self.read_buf {
            if buf.is_wasteful() {
                buf.aggressive_shrink(4096); // Keep 4KB
            }
        }

        // Cleanup write buffer if wasteful
        if let Some(ref mut buf) = self.write_buf {
            if buf.is_wasteful() {
                buf.aggressive_shrink(4096); // Keep 4KB
            }
        }

        // Cleanup idle stream buffers
        for stream_buf in &mut self.stream_bufs {
            if stream_buf.is_idle() {
                stream_buf.clear_and_free_if_idle();
            }
        }

        // Shrink packet queue if oversized
        if self.packet_queue.is_empty() && self.packet_queue.capacity() > 20 {
            self.packet_queue.shrink_to_fit();
        }
    }

    /// Force aggressive shrinking regardless of idle state.
    ///
    /// Use when memory pressure is detected externally (e.g., by monitoring tools).
    /// Shrinks all buffers to minimal sizes immediately.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(65536);
    ///
    /// // Memory pressure detected
    /// conn.force_shrink();
    /// ```
    pub fn force_shrink(&mut self) {
        if let Some(ref mut buf) = self.read_buf {
            if buf.is_wasteful() {
                buf.reset_and_trim(4096);
            }
        }

        if let Some(ref mut buf) = self.write_buf {
            if buf.is_wasteful() {
                buf.reset_and_trim(4096);
            }
        }

        for stream_buf in &mut self.stream_bufs {
            stream_buf.clear_and_free_if_idle();
        }

        self.packet_queue.shrink_to_fit();
    }

    /// Returns memory usage statistics.
    ///
    /// Provides detailed breakdown of memory allocation and usage across all buffers.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(8192);
    ///
    /// let stats = conn.memory_usage();
    /// println!("Total bytes: {}", stats.total_bytes);
    /// println!("Efficiency: {:.1}%", stats.efficiency() * 100.0);
    /// ```
    pub fn memory_usage(&self) -> ConnectionMemoryStats {
        let read_buf_bytes = self.read_buf.as_ref().map(|b| b.capacity()).unwrap_or(0);
        let write_buf_bytes = self.write_buf.as_ref().map(|b| b.capacity()).unwrap_or(0);
        let stream_buf_bytes: usize = self
            .stream_bufs
            .iter()
            .map(|b| if b.is_allocated() { b.size() } else { 0 })
            .sum();

        let read_buf_used = self.read_buf.as_ref().map(|b| b.len()).unwrap_or(0);
        let write_buf_used = self.write_buf.as_ref().map(|b| b.len()).unwrap_or(0);
        let stream_buf_used: usize = self.stream_bufs.iter().map(|b| b.used()).sum();

        let total_bytes =
            read_buf_bytes + write_buf_bytes + stream_buf_bytes + self.packet_queue_bytes;
        let total_used = read_buf_used + write_buf_used + stream_buf_used + self.packet_queue_bytes;

        ConnectionMemoryStats {
            read_buf_bytes,
            write_buf_bytes,
            stream_buf_bytes,
            packet_queue_bytes: self.packet_queue_bytes,
            total_bytes,
            total_used,
            total_wasted: total_bytes.saturating_sub(total_used),
            is_idle: self.is_idle(),
            idle_seconds: self.idle_duration().as_secs(),
        }
    }

    /// Returns detailed diagnostics for debugging memory issues.
    ///
    /// Provides buffer-level diagnostic information useful for troubleshooting
    /// memory leaks or inefficiencies.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(8192);
    ///
    /// let diag = conn.diagnostics();
    /// println!("Cleanup counter: {}", diag.cleanup_counter);
    /// ```
    pub fn diagnostics(&self) -> ConnectionDiagnostics {
        ConnectionDiagnostics {
            read_buf_info: self.read_buf.as_ref().map(|b| BufferDiagnostic {
                capacity: b.capacity(),
                length: b.len(),
                overhead: b.memory_overhead(),
                is_wasteful: b.is_wasteful(),
            }),
            write_buf_info: self.write_buf.as_ref().map(|b| BufferDiagnostic {
                capacity: b.capacity(),
                length: b.len(),
                overhead: b.memory_overhead(),
                is_wasteful: b.is_wasteful(),
            }),
            stream_buf_count: self.stream_bufs.len(),
            packet_queue_len: self.packet_queue.len(),
            packet_queue_cap: self.packet_queue.capacity(),
            cleanup_counter: self.cleanup_counter,
        }
    }
}

/// Memory usage statistics for a connection.
///
/// Provides comprehensive breakdown of memory allocation and usage across
/// all buffers in a connection.
///
/// # Examples
///
/// ```
/// use secbuf::ConnectionBuffers;
///
/// let mut conn = ConnectionBuffers::new();
/// conn.init_read_buf(8192);
///
/// let stats = conn.memory_usage();
/// if stats.is_problematic() {
///     println!("Warning: Connection using {} bytes with {:.1}% efficiency",
///              stats.total_bytes, stats.efficiency() * 100.0);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ConnectionMemoryStats {
    /// Bytes allocated for read buffer
    pub read_buf_bytes: usize,
    /// Bytes allocated for write buffer
    pub write_buf_bytes: usize,
    /// Bytes allocated for all stream buffers
    pub stream_buf_bytes: usize,
    /// Bytes currently in packet queue
    pub packet_queue_bytes: usize,
    /// Total bytes allocated across all buffers
    pub total_bytes: usize,
    /// Total bytes actually containing data
    pub total_used: usize,
    /// Total bytes allocated but unused (overhead)
    pub total_wasted: usize,
    /// Whether connection is idle
    pub is_idle: bool,
    /// Seconds since last activity
    pub idle_seconds: u64,
}

impl ConnectionMemoryStats {
    /// Returns memory efficiency ratio (0.0 to 1.0).
    ///
    /// Higher values indicate better memory utilization (less waste).
    ///
    /// # Returns
    ///
    /// - `1.0` = Perfect efficiency (no waste)
    /// - `0.5` = Half the allocated memory is wasted
    /// - `0.0` = All memory is wasted
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(8192);
    ///
    /// let stats = conn.memory_usage();
    /// println!("Memory efficiency: {:.1}%", stats.efficiency() * 100.0);
    /// ```
    pub fn efficiency(&self) -> f64 {
        if self.total_bytes == 0 {
            return 1.0;
        }
        self.total_used as f64 / self.total_bytes as f64
    }

    /// Returns `true` if memory usage indicates a problem.
    ///
    /// Flags connections as problematic if:
    /// - Efficiency < 50% (more than half the memory is wasted), OR
    /// - Connection is idle AND wasting >1MB
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::ConnectionBuffers;
    ///
    /// let mut conn = ConnectionBuffers::new();
    /// conn.init_read_buf(65536);
    ///
    /// let stats = conn.memory_usage();
    /// if stats.is_problematic() {
    ///     // Trigger cleanup or logging
    /// }
    /// ```
    pub fn is_problematic(&self) -> bool {
        // Flag if >50% waste or >1MB wasted when idle
        (self.efficiency() < 0.5) || (self.is_idle && self.total_wasted > 1_048_576)
    }
}

/// Diagnostic information for a single buffer.
///
/// Provides detailed metrics for analyzing individual buffer performance.
#[derive(Debug, Clone)]
pub struct BufferDiagnostic {
    /// Total allocated capacity
    pub capacity: usize,
    /// Bytes of valid data
    pub length: usize,
    /// Unused capacity (overhead)
    pub overhead: usize,
    /// Whether buffer is considered wasteful
    pub is_wasteful: bool,
}

/// Connection-level diagnostics for debugging.
///
/// Provides detailed diagnostic information about all buffers in a connection,
/// useful for troubleshooting memory issues.
///
/// # Examples
///
/// ```
/// use secbuf::ConnectionBuffers;
///
/// let mut conn = ConnectionBuffers::new();
/// conn.init_read_buf(8192);
/// conn.init_write_buf(4096);
///
/// let diag = conn.diagnostics();
/// if let Some(ref read_info) = diag.read_buf_info {
///     println!("Read buffer: {} / {} bytes (wasteful: {})",
///              read_info.length, read_info.capacity, read_info.is_wasteful);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ConnectionDiagnostics {
    /// Read buffer diagnostics (if allocated)
    pub read_buf_info: Option<BufferDiagnostic>,
    /// Write buffer diagnostics (if allocated)
    pub write_buf_info: Option<BufferDiagnostic>,
    /// Number of stream buffers
    pub stream_buf_count: usize,
    /// Current packets in queue
    pub packet_queue_len: usize,
    /// Allocated capacity of packet queue
    pub packet_queue_cap: usize,
    /// Number of periodic cleanup operations performed
    pub cleanup_counter: usize,
}

/// Error returned when packet queue reaches capacity.
#[derive(Debug, Clone)]
pub enum QueueFullError {
    /// Queue has reached maximum packet count
    TooManyPackets,
    /// Queue has reached maximum byte limit
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
        // Aggressive cleanup on drop
        self.aggressive_cleanup();
    }
}

/// Pooled connection buffers with automatic cleanup on drop.
///
/// This wrapper manages connection buffers with automatic resource cleanup
/// when dropped. Integrates with buffer pools for efficient memory management
/// in server applications.
///
/// # Lifecycle
///
/// 1. **Creation**: Buffers are associated with a buffer pool
/// 2. **Usage**: Access buffers via [`buffers()`](Self::buffers) method
/// 3. **Cleanup**: Automatic aggressive cleanup on drop
///
/// # Memory Management
///
/// All buffers are securely zeroed and memory is freed when:
/// - The instance is dropped (goes out of scope)
/// - [`release_to_pool()`](Self::release_to_pool) is called explicitly
///
/// This ensures no sensitive data remains in memory after connection termination.
///
/// # Examples
///
/// ## Basic Usage
///
/// ```
/// use secbuf::{PooledConnectionBuffers, BufferPool, PoolConfig};
/// use std::sync::Arc;
///
/// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
/// let mut conn = PooledConnectionBuffers::new(Arc::clone(&pool));
///
/// // Access and use secbufs
/// conn.buffers().init_read_buf(8192);
/// conn.buffers().init_write_buf(8192);
///
/// // Automatic cleanup on drop
/// ```
///
/// ## With Custom Configuration
///
/// ```
/// use secbuf::{
///     PooledConnectionBuffers, BufferPool, PoolConfig,
///     ConnectionBufferConfig
/// };
/// use std::sync::Arc;
///
/// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
/// let config = ConnectionBufferConfig {
///     max_packet_queue_size: 50,
///     max_packet_queue_bytes: 5_242_880, // 5MB
///     idle_timeout_secs: 30,
///     enable_aggressive_shrinking: true,
/// };
///
/// let mut conn = PooledConnectionBuffers::with_config(
///     Arc::clone(&pool),
///     config
/// );
/// ```
///
/// ## Manual Release
///
/// ```
/// use secbuf::{PooledConnectionBuffers, BufferPool, PoolConfig};
/// use std::sync::Arc;
///
/// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
/// let mut conn = PooledConnectionBuffers::new(Arc::clone(&pool));
///
/// conn.buffers().init_read_buf(8192);
///
/// // Explicit cleanup before drop
/// conn.release_to_pool();
/// ```
pub struct PooledConnectionBuffers {
    /// Internal connection buffers
    buffers: ConnectionBuffers,
    /// Optional reference to buffer pool (None after release)
    pool: Option<std::sync::Arc<BufferPool>>,
}

impl PooledConnectionBuffers {
    /// Creates new pooled connection buffers with default configuration.
    ///
    /// Associates the buffers with a buffer pool for resource management.
    /// All cleanup happens automatically on drop.
    ///
    /// # Arguments
    ///
    /// * `pool` - Shared reference to the buffer pool
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::{PooledConnectionBuffers, BufferPool, PoolConfig};
    /// use std::sync::Arc;
    ///
    /// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
    /// let conn = PooledConnectionBuffers::new(Arc::clone(&pool));
    /// ```
    pub fn new(pool: std::sync::Arc<BufferPool>) -> Self {
        Self {
            buffers: ConnectionBuffers::new(),
            pool: Some(pool),
        }
    }

    /// Creates new pooled connection buffers with custom configuration.
    ///
    /// Allows customization of buffer limits, timeouts, and cleanup behavior
    /// while still maintaining pool integration.
    ///
    /// # Arguments
    ///
    /// * `pool` - Shared reference to the buffer pool
    /// * `config` - Custom configuration for connection buffers
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::{
    ///     PooledConnectionBuffers, BufferPool, PoolConfig,
    ///     ConnectionBufferConfig
    /// };
    /// use std::sync::Arc;
    ///
    /// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
    ///
    /// let config = ConnectionBufferConfig {
    ///     max_packet_queue_size: 50,
    ///     max_packet_queue_bytes: 5_242_880, // 5MB
    ///     idle_timeout_secs: 30,
    ///     enable_aggressive_shrinking: true,
    /// };
    ///
    /// let conn = PooledConnectionBuffers::with_config(
    ///     Arc::clone(&pool),
    ///     config
    /// );
    /// ```
    pub fn with_config(pool: std::sync::Arc<BufferPool>, config: ConnectionBufferConfig) -> Self {
        Self {
            buffers: ConnectionBuffers::with_config(config),
            pool: Some(pool),
        }
    }

    /// Returns a mutable reference to the underlying connection buffers.
    ///
    /// Use this to access all connection buffer methods including initialization,
    /// I/O operations, and diagnostics.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::{PooledConnectionBuffers, BufferPool, PoolConfig};
    /// use std::sync::Arc;
    ///
    /// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
    /// let mut conn = PooledConnectionBuffers::new(Arc::clone(&pool));
    ///
    /// // Access underlying buffers
    /// conn.buffers().init_read_buf(8192);
    /// conn.buffers().init_write_buf(4096);
    ///
    /// // Check queue status
    /// let queue_len = conn.buffers().packet_queue_len();
    /// ```
    pub fn buffers(&mut self) -> &mut ConnectionBuffers {
        &mut self.buffers
    }

    /// Explicitly releases buffers and performs aggressive cleanup.
    ///
    /// This method:
    /// 1. Securely zeros all buffer data
    /// 2. Frees all allocated memory
    /// 3. Removes pool reference
    ///
    /// After calling this method, the buffers are cleaned but the struct
    /// remains valid (just without pool association). This is useful when
    /// you want explicit control over cleanup timing.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::{PooledConnectionBuffers, BufferPool, PoolConfig, Buffer};
    /// use std::sync::Arc;
    ///
    /// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
    /// let mut conn = PooledConnectionBuffers::new(Arc::clone(&pool));
    ///
    /// conn.buffers().init_read_buf(8192);
    /// // ... use secbufs ...
    ///
    /// // Explicit cleanup before scope ends
    /// conn.release_to_pool();
    /// // conn is still valid but buffers are cleaned
    /// ```
    pub fn release_to_pool(&mut self) {
        self.buffers.aggressive_cleanup();
        self.pool = None;
    }
}

impl Drop for PooledConnectionBuffers {
    /// Automatically performs aggressive cleanup when dropped.
    ///
    /// This ensures all sensitive data is securely zeroed and all memory
    /// is freed when the connection ends, preventing:
    /// - Memory leaks
    /// - Data leakage
    /// - Resource exhaustion in long-running servers
    ///
    /// # Cleanup Process
    ///
    /// 1. All buffer contents are securely zeroed using compiler-resistant zeroing
    /// 2. Memory allocations are freed
    /// 3. Stream buffers are dropped with secure cleanup
    /// 4. Packet queue is drained and zeroed
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::{PooledConnectionBuffers, BufferPool, PoolConfig};
    /// use std::sync::Arc;
    ///
    /// let pool = Arc::new(BufferPool::new(PoolConfig::default()));
    ///
    /// {
    ///     let mut conn = PooledConnectionBuffers::new(Arc::clone(&pool));
    ///     conn.buffers().init_read_buf(8192);
    ///     // ... use secbufs ...
    /// } // <- Automatic aggressive cleanup happens here
    /// ```
    fn drop(&mut self) {
        self.buffers.aggressive_cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_detection() {
        let mut conn = ConnectionBuffers::new();
        conn.init_read_buf(1024);

        assert!(!conn.is_idle()); // Just created

        // Simulate long idle by manipulating last_activity
        conn.last_activity = Instant::now() - Duration::from_secs(120);
        assert!(conn.is_idle());
    }

    #[test]
    fn test_periodic_cleanup() {
        let mut conn = ConnectionBuffers::with_config(ConnectionBufferConfig {
            idle_timeout_secs: 0, // Immediately idle
            enable_aggressive_shrinking: true,
            ..Default::default()
        });

        conn.init_read_buf(65536); // 64KB
        if let Some(ref mut buf) = conn.read_buf {
            buf.put_bytes(b"small").unwrap();
        }

        // Simulate idle
        conn.last_activity = Instant::now() - Duration::from_secs(1);

        conn.periodic_cleanup();

        if let Some(ref buf) = conn.read_buf {
            assert!(buf.capacity() < 65536); // Should have shrunk
        }
    }

    #[test]
    fn test_memory_stats() {
        let mut conn = ConnectionBuffers::new();
        conn.init_read_buf(1024);
        conn.init_write_buf(2048);

        let stats = conn.memory_usage();
        assert_eq!(stats.read_buf_bytes, 1024);
        assert_eq!(stats.write_buf_bytes, 2048);
        assert!(stats.efficiency() < 1.0); // Should have waste
    }

    #[test]
    fn test_aggressive_cleanup() {
        let mut conn = ConnectionBuffers::new();
        conn.init_read_buf(1024);
        conn.add_stream_buf(512);

        conn.aggressive_cleanup();

        assert!(conn.read_buf.is_none());
        assert_eq!(conn.stream_bufs.len(), 0);
    }
}

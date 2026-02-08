// src/buffer/core.rs
//! Core buffer structure with aggressive memory management
//!
//! This module provides the fundamental [`Buffer`] type with position tracking,
//! automatic secure memory zeroing on drop, and aggressive memory cleanup capabilities
//! to prevent memory spikes and unbounded growth in long-running services.
//!
//! # Memory Management Strategy
//!
//! The buffer provides three levels of cleanup:
//! 1. **Automatic**: Memory is securely zeroed on drop (via `#[zeroize(drop)]`)
//! 2. **Periodic**: Reset and trim oversized buffers during idle periods
//! 3. **Aggressive**: Force shrink when memory pressure is detected
//!
//! # Examples
//!
//! ```
//! use secbuf::Buffer;
//! # use secbuf::BufferError;
//!
//! // Basic usage
//! let mut buf = Buffer::new(1024);
//! buf.put_u32(42)?;
//! buf.put_bytes(b"hello")?;
//!
//! // Detect waste
//! if buf.is_wasteful() {
//!     println!("Buffer has {} bytes overhead", buf.memory_overhead());
//! }
//!
//! // Aggressive cleanup when needed
//! buf.aggressive_shrink(512); // Keep only 512 bytes capacity
//! # Ok::<(), BufferError>(())
//! ```

use crate::error::{BufferError, Result};
use zeroize::Zeroize;

/// Maximum single increment to prevent integer overflow
const BUF_MAX_INCR: usize = 1_000_000_000;

/// Maximum buffer size (1GB)
pub const BUF_MAX_SIZE: usize = 1_000_000_000;

/// A high-performance linear buffer with aggressive memory management.
///
/// The buffer automatically and securely zeros its memory on drop using
/// the [`zeroize`] crate, which provides compiler-resistant memory clearing.
///
/// # Memory Safety
///
/// All buffer memory is automatically zeroed when the buffer is dropped,
/// preventing sensitive data from remaining in memory. Additional cleanup
/// methods provide explicit control for long-running services.
///
/// # Memory Management
///
/// The buffer provides several cleanup strategies:
///
/// - [`reset`](Self::reset): Clear position/length without zeroing (fast, for reuse)
/// - [`burn`](Self::burn): Securely zero all memory
/// - [`burn_and_free_memory`](Self::burn_and_free_memory): Zero and release capacity
/// - [`aggressive_shrink`](Self::aggressive_shrink): Force capacity reduction
/// - [`reset_and_trim`](Self::reset_and_trim): Conditional shrinking for efficiency
///
/// # Waste Detection
///
/// Use [`is_wasteful`](Self::is_wasteful) and [`memory_overhead`](Self::memory_overhead)
/// to detect buffers with excessive unused capacity that should be shrunk.
///
/// # Examples
///
/// ```
/// use secbuf::Buffer;
/// # use secbuf::BufferError;
///
/// let mut buf = Buffer::try_new(1024)?;
/// buf.put_u32(42)?;
/// buf.put_bytes(b"hello")?;
///
/// // Check for waste
/// if buf.is_wasteful() {
///     // Trim to reasonable size
///     buf.reset_and_trim(512);
/// }
/// # Ok::<(), BufferError>(())
/// ```
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct Buffer {
    /// Internal data storage (securely erased on drop)
    pub(crate) data: Vec<u8>,
    /// Current read/write position
    pub pos: usize,
    /// Length of valid data
    pub len: usize,
}

impl Buffer {
    /// Creates a new buffer with zeroed memory.
    ///
    /// This allocates a `Vec<u8>` of the specified size and initializes it with zeros.
    /// The allocation is eager (not lazy) - all memory is allocated immediately.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::SizeTooBig`] if `size` exceeds [`BUF_MAX_SIZE`] (1GB).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    ///
    /// let buf = Buffer::try_new(8192)?;
    /// assert_eq!(buf.capacity(), 8192);
    /// assert_eq!(buf.len(), 0);
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    #[inline]
    pub fn try_new(size: usize) -> Result<Self> {
        if size > BUF_MAX_SIZE {
            return Err(BufferError::SizeTooBig);
        }
        Ok(Self {
            data: vec![0; size],
            pos: 0,
            len: 0,
        })
    }

    /// Creates a new buffer with zeroed memory.
    ///
    /// # Panics
    ///
    /// Panics if `size` exceeds [`BUF_MAX_SIZE`] (1GB).
    /// Prefer [`try_new`](Self::try_new) for fallible construction.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    ///
    /// let buf = Buffer::new(8192);
    /// assert_eq!(buf.capacity(), 8192);
    /// ```
    #[inline]
    pub fn new(size: usize) -> Self {
        Self::try_new(size).expect("Buffer size exceeds maximum")
    }

    /// Creates a new buffer with pre-allocated capacity but zero length.
    ///
    /// The internal `Vec` is allocated with the specified capacity but has zero length.
    /// You must call [`set_len`](Self::set_len) or use write operations to extend the
    /// valid data region.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::SizeTooBig`] if `capacity` exceeds [`BUF_MAX_SIZE`] (1GB).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::try_with_capacity(8192)?;
    /// assert_eq!(buf.capacity(), 8192);
    /// assert_eq!(buf.len(), 0);
    ///
    /// // Must set length before writing to ensure capacity exists
    /// buf.set_len(100)?;
    /// buf.set_pos(0)?;
    /// buf.put_u32(42)?;
    /// assert_eq!(buf.len(), 100);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn try_with_capacity(capacity: usize) -> Result<Self> {
        if capacity > BUF_MAX_SIZE {
            return Err(BufferError::SizeTooBig);
        }
        let mut data = vec![0; capacity];
        // Initialize to capacity with zeros so the memory exists
        data.resize(capacity, 0);
        Ok(Self {
            data,
            pos: 0,
            len: 0,
        })
    }

    /// Creates a new buffer with pre-allocated capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` exceeds [`BUF_MAX_SIZE`] (1GB).
    /// Prefer [`try_with_capacity`](Self::try_with_capacity) for fallible construction.
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self::try_with_capacity(capacity).expect("Buffer capacity exceeds maximum")
    }

    /// Creates a new buffer from existing data.
    ///
    /// The buffer's length is set to the vector's length, and the position
    /// is set to 0.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    ///
    /// let data = vec![1, 2, 3, 4, 5];
    /// let buf = Buffer::from_vec(data);
    /// assert_eq!(buf.len(), 5);
    /// assert_eq!(buf.pos(), 0);
    /// ```
    pub fn from_vec(data: Vec<u8>) -> Self {
        let len = data.len();
        Self { data, pos: 0, len }
    }

    /// Returns the total capacity of the buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    ///
    /// let buf = Buffer::new(1024);
    /// assert_eq!(buf.capacity(), 1024);
    /// ```
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }

    /// Returns the length of valid data in the buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// assert_eq!(buf.len(), 0);
    ///
    /// buf.put_u32(42)?;
    /// assert_eq!(buf.len(), 4);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the buffer contains no valid data.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// assert!(buf.is_empty());
    ///
    /// buf.put_u32(42)?;
    /// assert!(!buf.is_empty());
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the current read/write position.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// assert_eq!(buf.pos(), 0);
    ///
    /// buf.put_u32(42)?;
    /// assert_eq!(buf.pos(), 4);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline(always)]
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Returns the number of bytes available to read from current position.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_u32(42)?;
    /// buf.put_u32(43)?;
    /// buf.set_pos(0)?;
    ///
    /// assert_eq!(buf.remaining(), 8);
    /// buf.get_u32()?;
    /// assert_eq!(buf.remaining(), 4);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline(always)]
    pub fn remaining(&self) -> usize {
        self.len.saturating_sub(self.pos)
    }

    /// Checks if at least `count` bytes are available to read.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_u32(42)?;
    /// buf.set_pos(0)?;
    ///
    /// assert!(buf.has_remaining(4));
    /// assert!(!buf.has_remaining(5));
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline(always)]
    pub fn has_remaining(&self, count: usize) -> bool {
        self.remaining() >= count
    }

    /// Sets the read/write position.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::PositionOutOfBounds`] if `pos` exceeds the buffer length.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_u32(42)?;
    /// buf.set_pos(0)?;
    ///
    /// assert_eq!(buf.get_u32()?, 42);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn set_pos(&mut self, pos: usize) -> Result<()> {
        if pos > self.len {
            return Err(BufferError::PositionOutOfBounds);
        }
        self.pos = pos;
        Ok(())
    }

    /// Sets the length of valid data.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::PositionOutOfBounds`] if `len` exceeds buffer capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.set_len(100)?;
    /// assert_eq!(buf.len(), 100);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn set_len(&mut self, len: usize) -> Result<()> {
        if len > self.data.len() {
            return Err(BufferError::PositionOutOfBounds);
        }
        self.len = len;
        Ok(())
    }

    /// Increments the position by `n` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::IncrementTooLarge`] if `n` exceeds max increment.
    /// Returns [`BufferError::PositionOutOfBounds`] if new position exceeds length.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_u32(42)?;
    /// buf.set_pos(0)?;
    /// buf.incr_pos(4)?;
    /// assert_eq!(buf.pos(), 4);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn incr_pos(&mut self, n: usize) -> Result<()> {
        if n > BUF_MAX_INCR {
            return Err(BufferError::IncrementTooLarge);
        }
        let new_pos = self.pos.saturating_add(n);
        if new_pos > self.len {
            return Err(BufferError::PositionOutOfBounds);
        }
        self.pos = new_pos;
        Ok(())
    }

    /// Increments the length by `n` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::IncrementTooLarge`] if `n` exceeds max increment.
    /// Returns [`BufferError::PositionOutOfBounds`] if new length exceeds capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.incr_len(100)?;
    /// assert_eq!(buf.len(), 100);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn incr_len(&mut self, n: usize) -> Result<()> {
        if n > BUF_MAX_INCR {
            return Err(BufferError::IncrementTooLarge);
        }
        let new_len = self.len.saturating_add(n);
        if new_len > self.data.len() {
            return Err(BufferError::PositionOutOfBounds);
        }
        self.len = new_len;
        Ok(())
    }

    /// Increments the write position (alias for `incr_len`).
    ///
    /// This is typically called after using [`get_write_ptr`](Self::get_write_ptr)
    /// to write data directly.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// let write_slice = buf.get_write_ptr(100)?;
    /// // ... fill write_slice with data ...
    /// buf.incr_write_pos(100)?;
    /// assert_eq!(buf.len(), 100);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn incr_write_pos(&mut self, n: usize) -> Result<()> {
        self.incr_len(n)
    }

    /// Resets position and length to zero without zeroing memory.
    ///
    /// Use this for buffer reuse when the old data doesn't need to be
    /// securely erased. For secure erasure, use [`burn`](Self::burn).
    ///
    /// # Performance
    ///
    /// This is the fastest reset method as it only updates metadata.
    /// Memory contents remain but will be overwritten on next write.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_u32(42)?;
    /// buf.reset();
    /// assert_eq!(buf.pos(), 0);
    /// assert_eq!(buf.len(), 0);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn reset(&mut self) {
        self.pos = 0;
        self.len = 0;
    }

    /// Resets position and length to zero without zeroing memory.
    ///
    /// Convenience alias for [`reset`](Self::reset) that matches common
    /// collection API patterns.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_bytes(b"hello")?;
    /// buf.clear();
    /// assert!(buf.is_empty());
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn clear(&mut self) {
        self.reset()
    }

    /// Securely zeros all buffer memory and resets position and length.
    ///
    /// Uses compiler-resistant zeroing via the [`zeroize`] crate.
    /// Use this when the buffer contains sensitive data that must be
    /// cleared before reuse or when security is more important than performance.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_bytes(b"sensitive data")?;
    /// buf.burn();
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn burn(&mut self) {
        self.data.zeroize();
        self.pos = 0;
        self.len = 0;
    }

    /// Consumes the buffer and securely frees its memory.
    ///
    /// Equivalent to Dropbear's `buf_burn_free()` pattern. Provides explicit
    /// ownership-consuming cleanup.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_bytes(b"secret")?;
    /// buf.burn_free(); // Buffer consumed and securely erased
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn burn_free(mut self) {
        self.data.zeroize();
        drop(self);
    }

    /// Aggressive cleanup: zeros memory AND shrinks capacity to zero.
    ///
    /// Use when buffer won't be reused and you want to free memory immediately.
    /// This is more aggressive than [`burn`](Self::burn) which keeps capacity.
    ///
    /// # Use Cases
    ///
    /// - Connection termination in long-running services
    /// - Memory pressure situations
    /// - One-time buffer usage
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(8192);
    /// buf.put_bytes(b"data")?;
    ///
    /// buf.burn_and_free_memory();
    ///
    /// assert_eq!(buf.len(), 0);
    /// assert_eq!(buf.capacity(), 0); // Memory freed
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn burn_and_free_memory(&mut self) {
        self.data.zeroize();
        self.data.clear();
        self.data.shrink_to_fit();
        self.pos = 0;
        self.len = 0;
    }

    /// Resets and aggressively shrinks if capacity is much larger than needed.
    ///
    /// Useful for long-lived buffers that had temporary spikes. This method
    /// intelligently shrinks only when wasteful (capacity >4x target AND >8KB).
    ///
    /// # Parameters
    ///
    /// - `target_size`: Desired capacity after shrinking
    ///
    /// # Shrinking Criteria
    ///
    /// Shrinks if:
    /// - capacity > target_size * 4 AND
    /// - capacity > 8192 bytes
    ///
    /// # Use Cases
    ///
    /// - Periodic cleanup in idle connections
    /// - After processing large messages
    /// - Memory pressure mitigation
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(65536); // 64KB
    /// buf.put_bytes(b"small")?;
    ///
    /// // Shrinks because 64KB >> 1KB * 4
    /// buf.reset_and_trim(1024);
    ///
    /// assert!(buf.capacity() <= 4096);
    /// assert_eq!(buf.len(), 0);
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn reset_and_trim(&mut self, target_size: usize) {
        self.reset();

        let cap = self.data.capacity();
        let should_shrink = cap > target_size * 4 && cap > 8192;

        if should_shrink {
            // Zero current data first
            self.data.zeroize();

            // Resize to target
            self.data.resize(target_size, 0);
            self.data.shrink_to_fit();
        }
    }

    /// Aggressive shrink: zeros everything and shrinks to minimal size.
    ///
    /// Forces capacity reduction regardless of current usage. Use when
    /// memory pressure is detected or buffer won't be used for a while.
    ///
    /// # Parameters
    ///
    /// - `keep_size`: Bytes of capacity to retain (0 = free all)
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(8192);
    /// buf.put_bytes(b"data")?;
    ///
    /// // Free all memory
    /// buf.aggressive_shrink(0);
    /// assert_eq!(buf.capacity(), 0);
    ///
    /// // Or keep minimal capacity
    /// let mut buf2 = Buffer::new(8192);
    /// buf2.aggressive_shrink(1024);
    /// assert_eq!(buf2.capacity(), 1024);
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn aggressive_shrink(&mut self, keep_size: usize) {
        // Zero all data
        self.data.zeroize();

        // Reset state
        self.pos = 0;
        self.len = 0;

        // Shrink to requested size
        if keep_size == 0 {
            self.data.clear();
            self.data.shrink_to_fit();
        } else {
            self.data.resize(keep_size, 0);
            self.data.shrink_to_fit();
        }
    }

    /// Checks if buffer capacity is wasteful compared to usage.
    ///
    /// Returns `true` if capacity is >4x larger than length AND >8KB.
    /// Use this to detect buffers that should be shrunk.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(65536); // 64KB
    /// buf.put_bytes(b"tiny")?;
    ///
    /// assert!(buf.is_wasteful()); // 64KB for 4 bytes is wasteful
    ///
    /// let mut buf2 = Buffer::new(1024);
    /// buf2.put_bytes(&vec![0u8; 1000])?;
    ///
    /// assert!(!buf2.is_wasteful()); // Good ratio
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn is_wasteful(&self) -> bool {
        let cap = self.data.capacity();
        cap > self.len * 4 && cap > 8192
    }

    /// Returns memory overhead in bytes (capacity - length).
    ///
    /// This is the amount of allocated but unused memory. High overhead
    /// indicates opportunities for shrinking.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// assert_eq!(buf.memory_overhead(), 1024); // All overhead
    ///
    /// buf.put_bytes(&vec![0u8; 512])?;
    /// assert_eq!(buf.memory_overhead(), 512); // Half overhead
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn memory_overhead(&self) -> usize {
        self.data.capacity().saturating_sub(self.len)
    }

    /// Creates an explicit, unsecure clone of this buffer.
    ///
    /// This creates a deep copy of the buffer data. Use with caution as it
    /// creates additional copies of potentially sensitive data that must be
    /// separately managed.
    ///
    /// # Security Warning
    ///
    /// The cloned buffer is independent and will be zeroed separately on drop.
    /// Ensure you track all clones properly to avoid leaving sensitive data in memory.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    ///
    /// let buf1 = Buffer::new(1024);
    /// let buf2 = buf1.clone_unsecure();
    /// assert_eq!(buf1.capacity(), buf2.capacity());
    /// ```
    pub fn clone_unsecure(&self) -> Self {
        Self {
            data: self.data.clone(),
            pos: self.pos,
            len: self.len,
        }
    }

    /// Returns a slice of all valid data in the buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_bytes(b"hello")?;
    /// assert_eq!(buf.as_slice(), b"hello");
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len]
    }

    /// Returns a mutable slice of all valid data.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_bytes(b"hello")?;
    /// buf.as_mut_slice()[0] = b'H';
    /// assert_eq!(buf.as_slice(), b"Hello");
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data[..self.len]
    }

    /// Resizes the buffer, preserving existing data.
    ///
    /// If shrinking, data beyond `new_size` is securely zeroed before release.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::SizeTooBig`] if `new_size` exceeds [`BUF_MAX_SIZE`].
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.resize(2048)?;
    /// assert_eq!(buf.capacity(), 2048);
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn resize(&mut self, new_size: usize) -> Result<()> {
        if new_size > BUF_MAX_SIZE {
            return Err(BufferError::SizeTooBig);
        }

        // If shrinking, zero the memory being released
        if new_size < self.data.len() {
            self.data[new_size..].zeroize();
        }

        self.data.resize(new_size, 0);
        self.len = self.len.min(new_size);
        self.pos = self.pos.min(new_size);
        Ok(())
    }

    /// Ensures the buffer has at least the specified additional capacity.
    ///
    /// Similar to [`Vec::reserve`].
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    ///
    /// let mut buf = Buffer::new(100);
    /// buf.reserve(1000);
    /// assert!(buf.capacity() >= 1100);
    /// ```
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.data.reserve(additional);
    }

    /// Shrinks the buffer capacity to fit the current length.
    ///
    /// Frees unused memory. Data being released is securely zeroed.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_bytes(b"hello")?;
    /// buf.shrink_to_fit();
    /// assert_eq!(buf.capacity(), 5);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn shrink_to_fit(&mut self) {
        // Zero unused portion before shrinking
        if self.len < self.data.len() {
            self.data[self.len..].zeroize();
        }

        self.data.truncate(self.len);
        self.data.shrink_to_fit();
    }
}

impl AsRef<[u8]> for Buffer {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_burn_and_free_memory() {
        let mut buf = Buffer::new(1024);
        buf.put_bytes(b"sensitive").unwrap();

        let initial_cap = buf.capacity();
        assert_eq!(initial_cap, 1024);

        buf.burn_and_free_memory();

        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 0);
    }

    #[test]
    fn test_reset_and_trim() {
        let mut buf = Buffer::new(65536); // 64KB
        buf.put_bytes(b"small").unwrap();

        // Should shrink because capacity (64KB) > target (1KB) * 4
        buf.reset_and_trim(1024);

        assert!(buf.capacity() <= 4096); // Should be trimmed
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_reset_and_trim_no_shrink() {
        let mut buf = Buffer::new(4096); // 4KB
        buf.put_bytes(b"data").unwrap();

        // Should NOT shrink because capacity is close to target
        buf.reset_and_trim(2048);

        assert_eq!(buf.capacity(), 4096); // Unchanged
    }

    #[test]
    fn test_is_wasteful() {
        // Empty buffer with large capacity is wasteful
        let buf = Buffer::new(65536); // 64KB
        assert!(buf.is_wasteful()); // Changed: empty buffer IS wasteful

        let mut buf = Buffer::new(65536);
        buf.put_bytes(b"tiny").unwrap();
        assert!(buf.is_wasteful()); // 64KB for 4 bytes is wasteful

        let mut buf2 = Buffer::new(1024);
        buf2.put_bytes(&vec![0u8; 1000]).unwrap();
        assert!(!buf2.is_wasteful()); // Good ratio
    }

    #[test]
    fn test_aggressive_shrink() {
        let mut buf = Buffer::new(8192);
        buf.put_bytes(b"data").unwrap();

        buf.aggressive_shrink(0);

        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 0);
    }

    #[test]
    fn test_aggressive_shrink_keep_size() {
        let mut buf = Buffer::new(8192);
        buf.put_bytes(b"data").unwrap();

        buf.aggressive_shrink(1024);

        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 1024);
    }

    #[test]
    fn test_memory_overhead() {
        let mut buf = Buffer::new(1024);
        assert_eq!(buf.memory_overhead(), 1024); // All overhead

        buf.put_bytes(&vec![0u8; 512]).unwrap();
        assert_eq!(buf.memory_overhead(), 512); // Half overhead
    }
}

// src/circular/buffer.rs - Enhanced with aggressive memory management
//! Circular buffer with aggressive cleanup and memory safety

use crate::error::{BufferError, Result};
use zeroize::Zeroize;

/// Maximum circular buffer size (100MB)
pub const MAX_CBUF_SIZE: usize = 100_000_000;

/// Circular buffer with aggressive memory management.
///
/// Enhanced features:
/// - Lazy allocation (allocates on first write)
/// - Automatic secure zeroing on drop
/// - Explicit cleanup methods
/// - Memory pressure detection
/// - Aggressive shrinking capabilities
pub struct CircularBuffer {
    /// Internal storage (lazily allocated, auto-zeroized on drop)
    data: Option<Box<[u8]>>,
    /// Total size of the buffer
    size: usize,
    /// Number of bytes currently in buffer
    used: usize,
    /// Read position
    read_pos: usize,
    /// Write position
    write_pos: usize,
    /// Whether size is power-of-2 (enables fast modulo)
    is_pow2: bool,
    /// Track if buffer has ever been allocated (for memory pressure detection)
    was_allocated: bool,
}

impl CircularBuffer {
    /// Creates a new circular buffer with lazy allocation.
    ///
    /// The buffer memory is **not** allocated immediately. Instead, allocation
    /// happens on the first write operation. This allows creating many buffers
    /// without consuming memory until they're actually used.
    ///
    /// # Arguments
    ///
    /// * `size` - Buffer capacity in bytes (maximum [`MAX_CBUF_SIZE`] = 100MB)
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::SizeTooBig`] if `size` exceeds [`MAX_CBUF_SIZE`].
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let buf = CircularBuffer::try_new(4096)?;
    /// assert_eq!(buf.size(), 4096);
    /// assert!(!buf.is_allocated()); // Memory not yet allocated
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn try_new(size: usize) -> Result<Self> {
        if size > MAX_CBUF_SIZE {
            return Err(BufferError::SizeTooBig);
        }

        Ok(Self {
            data: None,
            size,
            used: 0,
            read_pos: 0,
            write_pos: 0,
            is_pow2: size.is_power_of_two(),
            was_allocated: false,
        })
    }

    /// Creates a new circular buffer with lazy allocation.
    ///
    /// # Panics
    ///
    /// Panics if `size` exceeds [`MAX_CBUF_SIZE`] (100MB).
    /// Prefer [`try_new`](Self::try_new) for fallible construction.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let buf = CircularBuffer::new(8192);
    /// assert_eq!(buf.size(), 8192);
    /// ```
    pub fn new(size: usize) -> Self {
        Self::try_new(size).expect("Circular buffer size exceeds maximum")
    }

    /// Creates a new power-of-2 sized circular buffer for fast modulo operations.
    ///
    /// Power-of-2 sizes enable bitwise AND instead of modulo division for position
    /// wrapping, providing ~2x performance improvement for position calculations.
    ///
    /// # Arguments
    ///
    /// * `size_log2` - Base-2 logarithm of desired size (e.g., 12 for 4KB, 16 for 64KB)
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::SizeTooBig`] if resulting size exceeds [`MAX_CBUF_SIZE`]
    /// or if `size_log2` would overflow.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// // 2^12 = 4096 bytes
    /// let buf = CircularBuffer::try_new_pow2(12)?;
    /// assert_eq!(buf.size(), 4096);
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn try_new_pow2(size_log2: u32) -> Result<Self> {
        let size = 1_usize
            .checked_shl(size_log2)
            .ok_or(BufferError::SizeTooBig)?;

        if size > MAX_CBUF_SIZE {
            return Err(BufferError::SizeTooBig);
        }

        Ok(Self {
            data: None,
            size,
            used: 0,
            read_pos: 0,
            write_pos: 0,
            is_pow2: true,
            was_allocated: false,
        })
    }

    /// Creates a new power-of-2 sized circular buffer.
    ///
    /// # Panics
    ///
    /// Panics if resulting size exceeds maximum.
    /// Prefer [`try_new_pow2`](Self::try_new_pow2) for fallible construction.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let buf = CircularBuffer::new_pow2(14); // 2^14 = 16KB
    /// assert_eq!(buf.size(), 16384);
    /// ```
    pub fn new_pow2(size_log2: u32) -> Self {
        Self::try_new_pow2(size_log2).expect("Circular buffer size exceeds maximum")
    }

    /// Returns the number of bytes currently stored in the buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// assert_eq!(buf.used(), 0);
    ///
    /// buf.write(b"hello")?;
    /// assert_eq!(buf.used(), 5);
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    #[inline(always)]
    pub fn used(&self) -> usize {
        self.used
    }

    /// Returns the number of bytes available for writing.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// assert_eq!(buf.available(), 256);
    ///
    /// buf.write(b"hello")?;
    /// assert_eq!(buf.available(), 251);
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    #[inline(always)]
    pub fn available(&self) -> usize {
        self.size - self.used
    }

    /// Returns the total capacity of the buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let buf = CircularBuffer::new(1024);
    /// assert_eq!(buf.size(), 1024);
    /// ```
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns `true` if the buffer contains no data.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// assert!(buf.is_empty());
    ///
    /// buf.write(b"data")?;
    /// assert!(!buf.is_empty());
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    /// Returns `true` if the buffer is full (no space for writing).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(4);
    /// assert!(!buf.is_full());
    ///
    /// buf.write(b"full")?;
    /// assert!(buf.is_full());
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.used == self.size
    }

    /// Returns `true` if buffer memory has been allocated.
    ///
    /// Due to lazy allocation, the buffer may exist but have no allocated memory
    /// until the first write operation.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// assert!(!buf.is_allocated()); // Not allocated yet
    ///
    /// buf.write(b"data")?;
    /// assert!(buf.is_allocated()); // Now allocated
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    #[inline(always)]
    pub fn is_allocated(&self) -> bool {
        self.data.is_some()
    }

    /// Returns `true` if buffer was allocated but is now empty.
    ///
    /// This indicates potential waste - memory is allocated but unused.
    /// Consider calling [`clear_and_free_if_idle`](Self::clear_and_free_if_idle)
    /// to reclaim memory.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"data")?;
    /// assert!(!buf.is_idle()); // Has data
    ///
    /// let mut tmp = vec![0u8; 4];
    /// buf.read(&mut tmp)?;
    /// assert!(buf.is_idle()); // Allocated but empty
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    #[inline(always)]
    pub fn is_idle(&self) -> bool {
        self.was_allocated && self.is_empty() && self.data.is_some()
    }

    #[inline(always)]
    fn wrap_pos(&self, pos: usize, delta: usize) -> usize {
        let new_pos = pos + delta;
        if self.is_pow2 {
            new_pos & (self.size - 1)
        } else {
            new_pos % self.size
        }
    }

    /// Returns read pointers for zero-copy reading.
    ///
    /// Due to the circular nature, data may wrap around. This returns two slices:
    /// - First slice: data from read position to end of buffer (or all data if no wrap)
    /// - Second slice: wrapped data from start of buffer (empty if no wrap)
    ///
    /// # Returns
    ///
    /// `(first_chunk, second_chunk)` where `second_chunk` is empty if data doesn't wrap.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"hello world")?;
    ///
    /// let (chunk1, chunk2) = buf.read_ptrs();
    /// assert_eq!(chunk1, b"hello world");
    /// assert_eq!(chunk2, b""); // No wrap in this case
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn read_ptrs(&self) -> (&[u8], &[u8]) {
        if self.used == 0 || self.data.is_none() {
            return (&[], &[]);
        }

        let buffer = self.data.as_ref().unwrap();
        let len1 = self.used.min(self.size - self.read_pos);
        let p1 = &buffer[self.read_pos..self.read_pos + len1];

        if len1 < self.used {
            let len2 = self.used - len1;
            let p2 = &buffer[0..len2];
            (p1, p2)
        } else {
            (p1, &[])
        }
    }

    /// Returns a mutable slice for zero-copy writing.
    ///
    /// Allocates buffer memory on first call (lazy allocation). The returned slice
    /// is guaranteed to be contiguous (does not wrap).
    ///
    /// After writing data, you **must** call [`incr_write`](Self::incr_write) to
    /// update the write position.
    ///
    /// # Arguments
    ///
    /// * `len` - Number of bytes needed for writing
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::InsufficientSpace`] if `len` exceeds available space.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// let write_slice = buf.write_ptr(10)?;
    /// write_slice.copy_from_slice(b"0123456789");
    /// buf.incr_write(10)?;
    /// assert_eq!(buf.used(), 10);
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn write_ptr(&mut self, len: usize) -> Result<&mut [u8]> {
        if len > self.available() {
            return Err(BufferError::InsufficientSpace);
        }

        // Lazy allocation
        if self.data.is_none() {
            let boxed = vec![0; self.size].into_boxed_slice();
            self.data = Some(boxed);
            self.was_allocated = true;
        }

        let buffer = self.data.as_mut().unwrap();
        Ok(&mut buffer[self.write_pos..self.write_pos + len])
    }

    /// Increments the write position after using [`write_ptr`](Self::write_ptr).
    ///
    /// This commits written data to the buffer.
    ///
    /// # Arguments
    ///
    /// * `len` - Number of bytes written
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::InsufficientSpace`] if `len` exceeds available space.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// let slice = buf.write_ptr(5)?;
    /// slice.copy_from_slice(b"hello");
    /// buf.incr_write(5)?;
    /// assert_eq!(buf.used(), 5);
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn incr_write(&mut self, len: usize) -> Result<()> {
        if len > self.available() {
            return Err(BufferError::InsufficientSpace);
        }

        self.used += len;
        self.write_pos = self.wrap_pos(self.write_pos, len);
        Ok(())
    }

    /// Increments the read position, discarding data.
    ///
    /// Use this to consume data without actually copying it (e.g., after processing
    /// data from [`read_ptrs`](Self::read_ptrs)).
    ///
    /// # Arguments
    ///
    /// * `len` - Number of bytes to discard
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::BufferOverflow`] if `len` exceeds available data.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"discard me")?;
    ///
    /// buf.incr_read(10)?;
    /// assert_eq!(buf.used(), 0);
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn incr_read(&mut self, len: usize) -> Result<()> {
        if len > self.used {
            return Err(BufferError::BufferOverflow);
        }

        self.used -= len;
        self.read_pos = self.wrap_pos(self.read_pos, len);
        Ok(())
    }

    /// Writes data to the buffer.
    ///
    /// Handles wraparound automatically. Allocates buffer memory on first write
    /// (lazy allocation).
    ///
    /// # Arguments
    ///
    /// * `data` - Data to write
    ///
    /// # Returns
    ///
    /// Number of bytes written (always equals `data.len()` on success).
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::InsufficientSpace`] if buffer is too full.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// let written = buf.write(b"hello world")?;
    /// assert_eq!(written, 11);
    /// assert_eq!(buf.used(), 11);
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn write(&mut self, data: &[u8]) -> Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }

        if self.available() < data.len() {
            return Err(BufferError::InsufficientSpace);
        }

        // Lazy allocation
        if self.data.is_none() {
            let boxed = vec![0; self.size].into_boxed_slice();
            self.data = Some(boxed);
            self.was_allocated = true;
        }

        let buffer = self.data.as_mut().unwrap();

        // Two-part copy for wrap-around
        let until_end = self.size - self.write_pos;
        let first_chunk = data.len().min(until_end);

        buffer[self.write_pos..self.write_pos + first_chunk].copy_from_slice(&data[..first_chunk]);

        if first_chunk < data.len() {
            let second_chunk = data.len() - first_chunk;
            buffer[0..second_chunk].copy_from_slice(&data[first_chunk..]);
        }

        self.write_pos = self.wrap_pos(self.write_pos, data.len());
        self.used += data.len();

        Ok(data.len())
    }

    /// Reads data from the buffer, removing it.
    ///
    /// Handles wraparound automatically. Reads up to `output.len()` bytes or
    /// all available data, whichever is smaller.
    ///
    /// # Arguments
    ///
    /// * `output` - Buffer to read into
    ///
    /// # Returns
    ///
    /// Number of bytes actually read.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"hello world")?;
    ///
    /// let mut output = vec![0u8; 5];
    /// let read = buf.read(&mut output)?;
    /// assert_eq!(read, 5);
    /// assert_eq!(&output, b"hello");
    /// assert_eq!(buf.used(), 6); // " world" remains
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn read(&mut self, output: &mut [u8]) -> Result<usize> {
        if self.used == 0 {
            return Ok(0);
        }

        let buffer = match &self.data {
            Some(b) => b,
            None => return Ok(0),
        };

        let to_read = output.len().min(self.used);
        let until_end = self.size - self.read_pos;
        let first_chunk_len = to_read.min(until_end);

        output[..first_chunk_len]
            .copy_from_slice(&buffer[self.read_pos..self.read_pos + first_chunk_len]);

        if first_chunk_len < to_read {
            let second_chunk_len = to_read - first_chunk_len;
            output[first_chunk_len..to_read].copy_from_slice(&buffer[0..second_chunk_len]);
        }

        self.read_pos = self.wrap_pos(self.read_pos, to_read);
        self.used -= to_read;

        Ok(to_read)
    }

    /// Reads data without removing it (peek operation).
    ///
    /// Identical to [`read`](Self::read) but doesn't advance the read position
    /// or modify buffer state.
    ///
    /// # Arguments
    ///
    /// * `output` - Buffer to read into
    ///
    /// # Returns
    ///
    /// Number of bytes read.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"hello")?;
    ///
    /// let mut output = vec![0u8; 5];
    /// buf.peek(&mut output)?;
    /// assert_eq!(&output, b"hello");
    /// assert_eq!(buf.used(), 5); // Data still in buffer
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn peek(&self, output: &mut [u8]) -> Result<usize> {
        if self.used == 0 {
            return Ok(0);
        }

        let buffer = match &self.data {
            Some(b) => b,
            None => return Ok(0),
        };

        let to_read = output.len().min(self.used);
        let until_end = self.size - self.read_pos;
        let first_chunk_len = to_read.min(until_end);

        output[..first_chunk_len]
            .copy_from_slice(&buffer[self.read_pos..self.read_pos + first_chunk_len]);

        if first_chunk_len < to_read {
            let second_chunk_len = to_read - first_chunk_len;
            output[first_chunk_len..to_read].copy_from_slice(&buffer[0..second_chunk_len]);
        }

        Ok(to_read)
    }

    /// Clears the buffer **without** freeing memory.
    ///
    /// Resets positions and counters but keeps the allocation intact.
    /// Use for performance when buffer will be reused soon.
    ///
    /// For memory reclamation, use [`free`](Self::free) instead.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"data")?;
    ///
    /// buf.clear();
    /// assert_eq!(buf.used(), 0);
    /// assert!(buf.is_allocated()); // Memory still allocated
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    #[inline]
    pub fn clear(&mut self) {
        self.used = 0;
        self.read_pos = 0;
        self.write_pos = 0;
    }

    /// Securely zeros and frees memory.
    ///
    /// Use when buffer won't be used again soon. Memory is zeroed before
    /// being released to prevent data leaks.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"sensitive data")?;
    ///
    /// buf.free();
    /// assert_eq!(buf.used(), 0);
    /// assert!(!buf.is_allocated()); // Memory freed
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn free(&mut self) {
        if let Some(data) = self.data.take() {
            let mut vec = data.into_vec();
            vec.zeroize();
            drop(vec);
        }
        self.clear();
    }

    /// Consumes and securely frees the buffer.
    ///
    /// Ownership-consuming version of [`free`](Self::free).
    /// Memory is zeroed before being released.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"secret")?;
    /// buf.burn_free(); // Buffer consumed
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn burn_free(mut self) {
        if let Some(data) = self.data.take() {
            let mut vec = data.into_vec();
            vec.zeroize();
            drop(vec);
        }
        drop(self);
    }

    /// Clears and optionally frees memory if buffer is idle.
    ///
    /// Clears the buffer unconditionally. If the buffer is idle (allocated but empty),
    /// it also frees the memory. Use this for periodic cleanup in long-lived connections.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"data")?;
    /// let mut tmp = vec![0u8; 4];
    /// buf.read(&mut tmp)?; // Now idle (allocated but empty)
    ///
    /// buf.clear_and_free_if_idle();
    /// assert!(!buf.is_allocated()); // Memory freed
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn clear_and_free_if_idle(&mut self) {
        self.clear();

        // Free memory if buffer was allocated but has been idle
        if self.is_idle() {
            self.free();
        }
    }

    /// Aggressively zeros all data without freeing allocation.
    ///
    /// Faster than [`free`](Self::free) when buffer will be reused.
    /// Memory is zeroed in-place but the allocation is retained.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(256);
    /// buf.write(b"sensitive")?;
    ///
    /// buf.zero_in_place();
    /// assert_eq!(buf.used(), 0);
    /// assert!(buf.is_allocated()); // Memory still allocated but zeroed
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn zero_in_place(&mut self) {
        if let Some(ref mut data) = self.data {
            let mut vec = std::mem::take(data).into_vec();
            vec.zeroize();
            *data = vec.into_boxed_slice();
        }
        self.clear();
    }

    /// Returns memory usage statistics.
    ///
    /// Provides insight into allocation status, usage, and waste for monitoring
    /// and optimization.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::CircularBuffer;
    ///
    /// let mut buf = CircularBuffer::new(1024);
    /// buf.write(b"test")?;
    ///
    /// let stats = buf.memory_stats();
    /// assert!(stats.allocated);
    /// assert_eq!(stats.size, 1024);
    /// assert_eq!(stats.used, 4);
    /// assert_eq!(stats.wasted, 1020);
    /// # Ok::<(), secbuf::BufferError>(())
    /// ```
    pub fn memory_stats(&self) -> CircularBufferMemoryStats {
        CircularBufferMemoryStats {
            allocated: self.data.is_some(),
            size: self.size,
            used: self.used,
            wasted: if self.data.is_some() {
                self.size - self.used
            } else {
                0
            },
        }
    }
}

/// Memory statistics for a circular buffer.
#[derive(Debug, Clone, Copy)]
pub struct CircularBufferMemoryStats {
    /// Whether buffer memory is allocated
    pub allocated: bool,
    /// Total size of buffer
    pub size: usize,
    /// Bytes currently in use
    pub used: usize,
    /// Bytes allocated but unused (wasted)
    pub wasted: usize,
}

impl Drop for CircularBuffer {
    fn drop(&mut self) {
        if let Some(data) = self.data.take() {
            let mut vec = data.into_vec();
            vec.zeroize();
            drop(vec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_idle() {
        let mut buf = CircularBuffer::new(256);
        assert!(!buf.is_idle()); // Not yet allocated

        buf.write(b"data").unwrap();
        assert!(!buf.is_idle()); // Has data

        let mut tmp = vec![0u8; 4];
        buf.read(&mut tmp).unwrap();
        assert!(buf.is_idle()); // Allocated but empty
    }

    #[test]
    fn test_clear_and_free_if_idle() {
        let mut buf = CircularBuffer::new(256);
        buf.write(b"test").unwrap();

        buf.clear_and_free_if_idle();
        assert!(buf.is_empty());
        assert!(!buf.is_allocated()); // Should have freed
    }

    #[test]
    fn test_zero_in_place() {
        let mut buf = CircularBuffer::new(256);
        buf.write(b"sensitive").unwrap();

        buf.zero_in_place();
        assert!(buf.is_empty());
        assert!(buf.is_allocated()); // Memory still allocated
    }

    #[test]
    fn test_memory_stats() {
        let mut buf = CircularBuffer::new(1024);
        let stats = buf.memory_stats();
        assert!(!stats.allocated);
        assert_eq!(stats.wasted, 0);

        buf.write(b"test").unwrap();
        let stats = buf.memory_stats();
        assert!(stats.allocated);
        assert_eq!(stats.used, 4);
        assert_eq!(stats.wasted, 1020);
    }
}

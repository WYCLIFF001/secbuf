// src/buffer/core.rs
//! Core buffer structure and basic operations
//!
//! This module provides the fundamental [`Buffer`] type with position tracking
//! and automatic secure memory zeroing on drop.

use crate::error::{BufferError, Result};
use zeroize::Zeroize;

/// Maximum single increment to prevent integer overflow
pub const BUF_MAX_INCR: usize = 1_000_000_000;
/// Maximum buffer size (1GB)
pub const BUF_MAX_SIZE: usize = 1_000_000_000;

/// A high-performance linear buffer with position tracking.
///
/// The buffer automatically and securely zeros its memory on drop using
/// the [`zeroize`] crate, which provides compiler-resistant memory clearing.
///
/// # Memory Safety
///
/// All buffer memory is automatically zeroed when the buffer is dropped,
/// preventing sensitive data from remaining in memory.
///
/// # Examples
///
/// ```
/// use secbuf::Buffer;
/// # use secbuf::BufferError;
///
/// let mut buf = Buffer::new(1024);
/// buf.put_u32(42)?;
/// buf.put_bytes(b"hello")?;
/// # Ok::<(), BufferError>(())
/// ```
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct Buffer {
    /// Internal data storage (securely erased on drop)
    pub(crate) data: Vec<u8>,
    /// Current read/write position
    pub(crate) pos: usize,
    /// Length of valid data
    pub(crate) len: usize,
}

impl Buffer {
    /// Creates a new buffer with zeroed memory.
    ///
    /// For large buffers (>4KB), the OS typically provides zero-filled pages
    /// efficiently via demand paging, making this nearly as fast as uninitialized
    /// allocation.
    ///
    /// # Panics
    ///
    /// Panics if `size` exceeds [`BUF_MAX_SIZE`] (1GB).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    ///
    /// let buf = Buffer::new(8192);
    /// assert_eq!(buf.capacity(), 8192);
    /// assert_eq!(buf.len(), 0);
    /// ```
    #[inline]
    pub fn new(size: usize) -> Self {
        assert!(
            size <= BUF_MAX_SIZE,
            "Buffer size {} exceeds maximum {}",
            size,
            BUF_MAX_SIZE
        );
        Self {
            data: vec![0; size],
            pos: 0,
            len: 0,
        }
    }

    /// Creates a new buffer with pre-allocated capacity but zero length.
    ///
    /// This is more efficient than [`new`](Self::new) when you know the maximum
    /// size but want to grow the buffer incrementally. The internal `Vec` will
    /// only be zeroed for the portions actually used.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` exceeds [`BUF_MAX_SIZE`] (1GB).
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::with_capacity(8192);
    /// assert_eq!(buf.capacity(), 8192);
    /// assert_eq!(buf.len(), 0);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity <= BUF_MAX_SIZE,
            "Buffer capacity {} exceeds maximum {}",
            capacity,
            BUF_MAX_SIZE
        );
        Self {
            data: Vec::with_capacity(capacity),
            pos: 0,
            len: 0,
        }
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
    /// When using [`with_capacity`](Self::with_capacity), this will grow the
    /// internal `Vec` if needed.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::SizeTooBig`] if `len` exceeds [`BUF_MAX_SIZE`].
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::with_capacity(1024);
    /// buf.set_len(100)?;
    /// assert_eq!(buf.len(), 100);
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn set_len(&mut self, len: usize) -> Result<()> {
        if len > BUF_MAX_SIZE {
            return Err(BufferError::SizeTooBig);
        }

        // Grow Vec if needed (for with_capacity() usage)
        if len > self.data.len() {
            self.data.resize(len, 0);
        }

        self.len = len;
        self.pos = self.pos.min(len);
        Ok(())
    }

    /// Increments the position by `incr`.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::IncrementTooLarge`] if the increment is too large
    /// or would exceed the buffer length.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_bytes(b"hello")?;
    /// buf.set_pos(0)?;
    ///
    /// buf.incr_pos(2)?;
    /// assert_eq!(buf.pos(), 2);
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn incr_pos(&mut self, incr: usize) -> Result<()> {
        if incr > BUF_MAX_INCR || self.pos + incr > self.len {
            return Err(BufferError::IncrementTooLarge);
        }
        self.pos += incr;
        Ok(())
    }

    /// Decrements the position by `decr`.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::PositionOutOfBounds`] if `decr` exceeds the current position.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_u32(42)?;
    ///
    /// buf.decr_pos(2)?;
    /// assert_eq!(buf.pos(), 2);
    /// # Ok::<(), BufferError>(())
    /// ```
    pub fn decr_pos(&mut self, decr: usize) -> Result<()> {
        if decr > self.pos {
            return Err(BufferError::PositionOutOfBounds);
        }
        self.pos -= decr;
        Ok(())
    }

    /// Increments the length by `incr`.
    ///
    /// When using [`with_capacity`](Self::with_capacity), this will grow the
    /// internal `Vec` if needed.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::IncrementTooLarge`] if the increment is too large.
    /// Returns [`BufferError::SizeTooBig`] if the new length would exceed [`BUF_MAX_SIZE`].
    pub fn incr_len(&mut self, incr: usize) -> Result<()> {
        if incr > BUF_MAX_INCR {
            return Err(BufferError::IncrementTooLarge);
        }

        let new_len = self.len + incr;
        if new_len > BUF_MAX_SIZE {
            return Err(BufferError::SizeTooBig);
        }

        // Grow Vec if needed
        if new_len > self.data.len() {
            self.data.resize(new_len, 0);
        }

        self.len = new_len;
        Ok(())
    }

    /// Increments the write position and updates length if needed.
    ///
    /// When using [`with_capacity`](Self::with_capacity), this will grow the
    /// internal `Vec` if needed.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::IncrementTooLarge`] if the increment is too large
    /// or the new position would exceed [`BUF_MAX_SIZE`].
    pub fn incr_write_pos(&mut self, incr: usize) -> Result<()> {
        if incr > BUF_MAX_INCR {
            return Err(BufferError::IncrementTooLarge);
        }

        let new_pos = self.pos + incr;
        if new_pos > BUF_MAX_SIZE {
            return Err(BufferError::IncrementTooLarge);
        }

        // Grow Vec if needed
        if new_pos > self.data.len() {
            self.data.resize(new_pos, 0);
        }

        self.pos = new_pos;
        if self.pos > self.len {
            self.len = self.pos;
        }
        Ok(())
    }

    /// Resets the buffer for reuse by clearing position and length.
    ///
    /// This does not free memory or zero the contents. Use [`burn`](Self::burn)
    /// for secure erasure.
    ///
    /// # Examples
    ///
    /// ```
    /// use secbuf::Buffer;
    /// # use secbuf::BufferError;
    ///
    /// let mut buf = Buffer::new(1024);
    /// buf.put_u32(42)?;
    /// assert_eq!(buf.len(), 4);
    ///
    /// buf.reset();
    /// assert_eq!(buf.len(), 0);
    /// assert_eq!(buf.pos(), 0);
    /// # Ok::<(), BufferError>(())
    /// ```
    #[inline]
    pub fn reset(&mut self) {
        self.pos = 0;
        self.len = 0;
    }

    /// Securely zeros all buffer memory and resets position and length.
    ///
    /// Uses compiler-resistant zeroing via the [`zeroize`] crate.
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
        // Use as_mut_slice() NOT self.data.zeroize() — Vec::zeroize() calls
        // Vec::clear() which sets data.len() to 0, breaking every subsequent
        // bounds check (put_u32 checks pos + 4 > data.len()) after pool return.
        // Slice zeroize wipes the bytes but preserves data.len() == capacity.
        self.data.as_mut_slice().zeroize();
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
        self.data.as_mut_slice().zeroize();
        drop(self);
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
    /// Frees unused memory.
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
    fn test_new() {
        let buf = Buffer::new(1024);
        assert_eq!(buf.capacity(), 1024);
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.pos(), 0);
    }

    #[test]
    fn test_with_capacity() {
        let mut buf = Buffer::with_capacity(1024);
        assert_eq!(buf.capacity(), 1024);
        assert_eq!(buf.len(), 0);

        buf.data.extend_from_slice(b"hello");
        buf.len = 5;

        assert_eq!(buf.len(), 5);
        assert_eq!(buf.as_slice(), b"hello");
    }

    #[test]
    fn test_from_vec() {
        let data = vec![1, 2, 3, 4, 5];
        let buf = Buffer::from_vec(data);
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.as_slice(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_incr_len_grows_vec() {
        let mut buf = Buffer::with_capacity(100);

        buf.incr_len(50).unwrap();
        assert_eq!(buf.len(), 50);
        assert!(buf.data.len() >= 50);
    }

    #[test]
    fn test_shrink_to_fit() {
        let mut buf = Buffer::new(1024);
        buf.len = 100;

        buf.shrink_to_fit();
        assert_eq!(buf.data.len(), 100);
        assert_eq!(buf.len(), 100);
    }

    #[test]
    fn test_reset() {
        let mut buf = Buffer::new(1024);
        buf.pos = 50;
        buf.len = 100;

        buf.reset();
        assert_eq!(buf.pos(), 0);
        assert_eq!(buf.len(), 0);
    }
}
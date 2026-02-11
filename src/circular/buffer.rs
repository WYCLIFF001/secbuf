// src/circular/buffer.rs
//! Circular (ring) buffer implementation with secure memory management
//!
//! # Memory Safety
//!
//! The circular buffer uses lazy allocation and secure memory clearing:
//!
//! - Memory is only allocated when first written to
//! - All data is securely zeroed on drop using compiler-resistant clearing
//! - The `free()` and `burn_free()` methods provide explicit cleanup for
//!   connection/session termination patterns
//!
//! # Performance
//!
//! - Power-of-2 sizes use fast bitwise modulo (recommended)
//! - Non-power-of-2 sizes use standard modulo (slightly slower)
//! - Lazy allocation reduces memory footprint for unused buffers
//!
//! BUGFIX: Previous implementation had incorrect memory clearing in drop/free/burn_free.
//! It was creating a Vec copy of the Box<[u8]>, zeroing the copy, then dropping the
//! original unzeroed data. This has been fixed to properly convert and zero the original.

use crate::error::{BufferError, Result};
use zeroize::Zeroize;

/// Maximum circular buffer size (100MB)
const MAX_CBUF_SIZE: usize = 100_000_000;

/// A circular (ring) buffer for efficient streaming data.
///
/// Memory is automatically securely zeroed on drop.
///
/// # Example
///
/// ```rust
/// use secbuf::prelude::*;
///
/// let mut ring = CircularBuffer::new(1024);
/// ring.write(b"Hello, ")?;
/// ring.write(b"World!")?;
///
/// let mut output = vec![0u8; ring.used()];
/// ring.read(&mut output)?;
/// assert_eq!(&output, b"Hello, World!");
/// # Ok::<(), secbuf::BufferError>(())
/// ```
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
}

impl CircularBuffer {
    /// Creates a new circular buffer with the specified size.
    ///
    /// Memory is not allocated until first write.
    ///
    /// # Panics
    ///
    /// Panics if size exceeds `MAX_CBUF_SIZE` (100MB).
    pub fn new(size: usize) -> Self {
        assert!(
            size <= MAX_CBUF_SIZE,
            "Circular buffer size {} exceeds maximum {}",
            size,
            MAX_CBUF_SIZE
        );

        Self {
            data: None,
            size,
            used: 0,
            read_pos: 0,
            write_pos: 0,
            is_pow2: size.is_power_of_two(),
        }
    }

    /// Creates a buffer with power-of-2 size for optimal performance.
    ///
    /// Uses fast bitwise AND for position wrapping instead of modulo.
    ///
    /// # Arguments
    ///
    /// * `size_log2` - Log base 2 of the desired size (e.g., 10 for 1024 bytes)
    ///
    /// # Panics
    ///
    /// Panics if resulting size exceeds `MAX_CBUF_SIZE`.
    pub fn new_pow2(size_log2: u32) -> Self {
        let size = 1 << size_log2;
        assert!(
            size <= MAX_CBUF_SIZE,
            "Circular buffer size {} exceeds maximum {}",
            size,
            MAX_CBUF_SIZE
        );

        Self {
            data: None,
            size,
            used: 0,
            read_pos: 0,
            write_pos: 0,
            is_pow2: true,
        }
    }

    /// Returns the number of bytes currently in the buffer.
    #[inline(always)]
    pub fn used(&self) -> usize {
        self.used
    }

    /// Returns the available space for writing.
    #[inline(always)]
    pub fn available(&self) -> usize {
        self.size - self.used
    }

    /// Returns the total size of the buffer.
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns `true` if the buffer is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    /// Returns `true` if the buffer is full.
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.used == self.size
    }

    /// Wraps a position around the buffer size.
    ///
    /// Uses fast bitwise AND for power-of-2 sizes, standard modulo otherwise.
    #[inline(always)]
    fn wrap_pos(&self, pos: usize, delta: usize) -> usize {
        let new_pos = pos + delta;
        if self.is_pow2 {
            new_pos & (self.size - 1)
        } else {
            new_pos % self.size
        }
    }

    /// Returns slices for reading data (zero-copy).
    ///
    /// Due to the circular nature, data may wrap around, requiring two slices.
    /// The second slice will be empty if data is contiguous.
    ///
    /// # Returns
    ///
    /// A tuple of `(&[u8], &[u8])` where the second slice is empty if no wrap-around.
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

    /// Returns a mutable slice for writing (zero-copy).
    ///
    /// Performs lazy allocation if buffer has not yet been allocated.
    ///
    /// # Errors
    ///
    /// Returns `BufferError::InsufficientSpace` if requested length exceeds available space.
    pub fn write_ptr(&mut self, len: usize) -> Result<&mut [u8]> {
        if len > self.available() {
            return Err(BufferError::InsufficientSpace);
        }

        // Lazy allocation using Box for automatic secure zeroing
        if self.data.is_none() {
            let boxed = vec![0; self.size].into_boxed_slice();
            self.data = Some(boxed);
        }

        let buffer = self.data.as_mut().unwrap();
        Ok(&mut buffer[self.write_pos..self.write_pos + len])
    }

    /// Advances the write position after writing data.
    ///
    /// Must be called after writing to the slice returned by `write_ptr()`.
    pub fn incr_write(&mut self, len: usize) -> Result<()> {
        if len > self.available() {
            return Err(BufferError::InsufficientSpace);
        }

        self.used += len;
        self.write_pos = self.wrap_pos(self.write_pos, len);
        Ok(())
    }

    /// Advances the read position after reading data.
    ///
    /// Must be called after reading from the slices returned by `read_ptrs()`.
    pub fn incr_read(&mut self, len: usize) -> Result<()> {
        if len > self.used {
            return Err(BufferError::BufferOverflow);
        }

        self.used -= len;
        self.read_pos = self.wrap_pos(self.read_pos, len);
        Ok(())
    }

    /// Writes data into the circular buffer.
    ///
    /// # Errors
    ///
    /// Returns `BufferError::InsufficientSpace` if data doesn't fit.
    pub fn write(&mut self, data: &[u8]) -> Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }

        if self.available() < data.len() {
            return Err(BufferError::InsufficientSpace);
        }

        // Lazy allocation using Box for secure zeroing
        if self.data.is_none() {
            let boxed = vec![0; self.size].into_boxed_slice();
            self.data = Some(boxed);
        }

        let mut written = 0;
        let mut src_pos = 0;

        while written < data.len() {
            // Calculate contiguous write space
            let contig = if self.used == self.size {
                0
            } else if self.write_pos < self.read_pos {
                self.read_pos - self.write_pos
            } else {
                self.size - self.write_pos
            };

            if contig == 0 {
                break;
            }

            let to_write = (data.len() - written).min(contig);

            // Get buffer reference once before the loop
            let buffer = self.data.as_mut().unwrap();

            // Copy data
            buffer[self.write_pos..self.write_pos + to_write]
                .copy_from_slice(&data[src_pos..src_pos + to_write]);

            // Update positions after borrowing ends
            let new_write_pos = self.wrap_pos(self.write_pos, to_write);
            self.write_pos = new_write_pos;
            self.used += to_write;
            written += to_write;
            src_pos += to_write;
        }

        Ok(written)
    }

    /// Reads data from the circular buffer.
    ///
    /// Reads up to `output.len()` bytes or all available data, whichever is smaller.
    ///
    /// # Returns
    ///
    /// Number of bytes actually read.
    pub fn read(&mut self, output: &mut [u8]) -> Result<usize> {
        if self.used == 0 {
            return Ok(0);
        }

        let buffer = match &self.data {
            Some(b) => b,
            None => return Ok(0),
        };

        let to_read = output.len().min(self.used);
        let mut read = 0;

        while read < to_read {
            let until_end = self.size - self.read_pos;
            let chunk_size = (to_read - read).min(until_end);

            output[read..read + chunk_size]
                .copy_from_slice(&buffer[self.read_pos..self.read_pos + chunk_size]);

            self.read_pos = self.wrap_pos(self.read_pos, chunk_size);
            self.used -= chunk_size;
            read += chunk_size;
        }

        Ok(read)
    }

    /// Peeks at data without consuming it.
    ///
    /// Similar to `read()` but doesn't advance the read position.
    pub fn peek(&self, output: &mut [u8]) -> Result<usize> {
        if self.used == 0 {
            return Ok(0);
        }

        let buffer = match &self.data {
            Some(b) => b,
            None => return Ok(0),
        };

        let to_read = output.len().min(self.used);
        let mut read = 0;
        let mut read_pos = self.read_pos;

        while read < to_read {
            let until_end = self.size - read_pos;
            let chunk_size = (to_read - read).min(until_end);

            output[read..read + chunk_size]
                .copy_from_slice(&buffer[read_pos..read_pos + chunk_size]);

            read_pos = self.wrap_pos(read_pos, chunk_size);
            read += chunk_size;
        }

        Ok(read)
    }

    /// Clears the buffer without freeing memory.
    ///
    /// Resets positions and used count, but keeps allocated memory for reuse.
    #[inline]
    pub fn clear(&mut self) {
        self.used = 0;
        self.read_pos = 0;
        self.write_pos = 0;
    }

    /// Frees the buffer and securely zeroes memory.
    ///
    /// Explicit cleanup for connection/session termination.
    /// Converts the Box<[u8]> to Vec<u8> in-place and securely zeros it.
    ///
    /// BUGFIX: Previous implementation created a copy with .to_vec(), which zeroed
    /// the copy but left the original data unzeroed. Fixed to properly convert and
    /// zero the original allocation.
    pub fn free(&mut self) {
        if let Some(data) = self.data.take() {
            // Convert Box<[u8]> to Vec<u8> without copying
            let mut vec = data.into_vec();
            // Securely zero the actual allocation
            vec.zeroize();
        }
        self.clear();
    }

    /// Consumes and securely frees the buffer.
    ///
    /// Equivalent to Dropbear's cbuf_free() pattern.
    /// Provides explicit ownership-consuming cleanup.
    ///
    /// BUGFIX: Same fix as free() - now properly zeros the original allocation.
    pub fn burn_free(mut self) {
        if let Some(data) = self.data.take() {
            let mut vec = data.into_vec();
            vec.zeroize();
        }
        drop(self);
    }
}

impl Drop for CircularBuffer {
    /// Automatically secures and frees memory on drop.
    ///
    /// BUGFIX: Previous implementation created a Vec copy with .to_vec(), zeroed the copy,
    /// then dropped the original unzeroed Box<[u8]>. This meant sensitive data could remain
    /// in the original allocation.
    ///
    /// Fixed to properly convert Box<[u8]> to Vec<u8> in-place using into_vec(), which
    /// maintains ownership of the original allocation and allows us to securely zero it
    /// before it's freed.
    fn drop(&mut self) {
        if let Some(data) = self.data.take() {
            // Convert Box<[u8]> to Vec<u8> without copying - maintains same allocation
            let mut vec = data.into_vec();
            // Securely zero the original allocation
            vec.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circular_basic() {
        let buf = CircularBuffer::new(256);
        assert_eq!(buf.size(), 256);
        assert_eq!(buf.used(), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_write_read() {
        let mut buf = CircularBuffer::new(256);

        buf.write(b"Hello").unwrap();
        assert_eq!(buf.used(), 5);

        let mut output = vec![0u8; 5];
        buf.read(&mut output).unwrap();
        assert_eq!(&output, b"Hello");
        assert!(buf.is_empty());
    }

    #[test]
    fn test_wrap_around() {
        let mut buf = CircularBuffer::new(8);

        buf.write(b"12345").unwrap();
        let mut tmp = vec![0u8; 3];
        buf.read(&mut tmp).unwrap(); // Read 3, leaves 2

        buf.write(b"6789").unwrap(); // Should wrap
        assert_eq!(buf.used(), 6);

        let mut output = vec![0u8; 6];
        buf.read(&mut output).unwrap();
        assert_eq!(&output, b"456789");
    }

    #[test]
    fn test_free() {
        let mut buf = CircularBuffer::new(1024);
        buf.write(b"sensitive").unwrap();
        buf.free();

        assert!(buf.is_empty());
        assert_eq!(buf.used(), 0);
    }

    #[test]
    fn test_burn_free() {
        let mut buf = CircularBuffer::new(1024);
        buf.write(b"secret").unwrap();
        buf.burn_free(); // Consumes buffer
    }
}
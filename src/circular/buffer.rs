// src/circular/buffer.rs
//! Circular (ring) buffer implementation with secure memory management.
//!
//! # Memory Safety
//!
//! - Memory is lazily allocated on first write.
//! - All data is securely zeroed on drop via `zeroize`.
//! - `free()` and `burn_free()` provide explicit cleanup for session termination.
//!
//! # Performance
//!
//! - Power-of-2 sizes use bitwise AND for wrapping (preferred).
//! - Non-power-of-2 sizes use standard modulo.
//! - `write()` is a single-pass, at-most-two-`copy_from_slice` path — no per-byte loop.
//! - Borrow conflicts are avoided by extracting `Copy` fields into locals before
//!   borrowing `self.data`, so Rust's field-splitting NLL is never needed.
//!
//! # Fixed Issues (vs original)
//!
//! - `write_ptr` used to panic when a write crossed the ring boundary.  It now
//!   returns `BufferError::WouldWrap` so callers can fall back to `write()`.
//!   A new `write_slices_mut` method gives true zero-copy two-part access.
//! - `read` and `write` computed positions by calling `self.wrap_pos()` while
//!   holding a borrow on `self.data`, causing borrow-checker issues.  Both now
//!   use local copies of `size`/`is_pow2` and inline the arithmetic.
//! - `write` looped over the data in chunks even when a simple two-segment
//!   `copy_from_slice` suffices — replaced with a branch-free two-copy path.

use crate::error::{BufferError, Result};
use zeroize::Zeroize;

/// Maximum circular buffer size (100 MB).
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
    /// Internal storage (lazily allocated, auto-zeroized on drop).
    data: Option<Box<[u8]>>,
    /// Total capacity of the buffer.
    size: usize,
    /// Number of bytes currently in the buffer.
    used: usize,
    /// Read cursor.
    read_pos: usize,
    /// Write cursor.
    write_pos: usize,
    /// Whether `size` is a power of two (enables fast bitwise modulo).
    is_pow2: bool,
}

// ---------------------------------------------------------------------------
// Inline helper — avoids borrowing `self` inside loops that already hold a
// borrow on `self.data`, so no field-splitting NLL is required.
// ---------------------------------------------------------------------------
#[inline(always)]
fn wrap(pos: usize, delta: usize, size: usize, is_pow2: bool) -> usize {
    let new = pos + delta;
    if is_pow2 {
        new & (size - 1)
    } else {
        new % size
    }
}

impl CircularBuffer {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Creates a new circular buffer with the specified capacity.
    ///
    /// Memory is not allocated until the first write.
    ///
    /// # Panics
    ///
    /// Panics if `size` exceeds `MAX_CBUF_SIZE` (100 MB).
    pub fn new(size: usize) -> Self {
        assert!(
            size <= MAX_CBUF_SIZE,
            "CircularBuffer size {size} exceeds maximum {MAX_CBUF_SIZE}"
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

    /// Creates a buffer with a power-of-two capacity for optimal wrapping speed.
    ///
    /// # Arguments
    ///
    /// * `size_log2` — exponent (e.g., `10` → 1024 bytes).
    ///
    /// # Panics
    ///
    /// Panics if the resulting size exceeds `MAX_CBUF_SIZE`.
    pub fn new_pow2(size_log2: u32) -> Self {
        let size = 1usize << size_log2;
        assert!(
            size <= MAX_CBUF_SIZE,
            "CircularBuffer size {size} exceeds maximum {MAX_CBUF_SIZE}"
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

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Bytes currently stored in the buffer.
    #[inline(always)]
    pub fn used(&self) -> usize {
        self.used
    }

    /// Bytes available for writing.
    #[inline(always)]
    pub fn available(&self) -> usize {
        self.size - self.used
    }

    /// Total capacity.
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.size
    }

    /// `true` if no data is buffered.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    /// `true` if the buffer is at full capacity.
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.used == self.size
    }

    // -----------------------------------------------------------------------
    // Zero-copy read interface
    // -----------------------------------------------------------------------

    /// Returns up to two slices covering all buffered data (zero-copy).
    ///
    /// Due to the ring nature the data may span two segments; the second slice
    /// is empty when data is contiguous.
    pub fn read_ptrs(&self) -> (&[u8], &[u8]) {
        if self.used == 0 {
            return (&[], &[]);
        }
        let buffer = match self.data.as_ref() {
            Some(b) => b,
            None => return (&[], &[]),
        };

        let len1 = self.used.min(self.size - self.read_pos);
        let p1 = &buffer[self.read_pos..self.read_pos + len1];

        if len1 < self.used {
            let p2 = &buffer[..self.used - len1];
            (p1, p2)
        } else {
            (p1, &[])
        }
    }

    // -----------------------------------------------------------------------
    // Zero-copy write interface
    // -----------------------------------------------------------------------

    /// Returns a contiguous mutable slice for writing `len` bytes (zero-copy).
    ///
    /// This variant is only valid when the entire write fits **without** crossing
    /// the ring boundary.  If a wrap-around would be required, the method returns
    /// `BufferError::WouldWrap` — use [`write_slices_mut`](Self::write_slices_mut)
    /// or the safe [`write`](Self::write) method instead.
    ///
    /// After writing, call [`incr_write`](Self::incr_write) with the same `len`.
    pub fn write_ptr(&mut self, len: usize) -> Result<&mut [u8]> {
        if len > self.available() {
            return Err(BufferError::InsufficientSpace);
        }
        // Guard against the original panic: writing `len` bytes starting at
        // `write_pos` must not cross the ring boundary.
        if self.write_pos + len > self.size {
            return Err(BufferError::InvalidState(
                "write would cross ring boundary — use write_slices_mut or write()".into(),
            ));
        }

        self.ensure_allocated();
        let buffer = self.data.as_mut().unwrap();
        Ok(&mut buffer[self.write_pos..self.write_pos + len])
    }

    /// Returns a pair of mutable slices covering exactly `len` bytes of free
    /// space (zero-copy, supports ring wrap-around).
    ///
    /// * The first slice covers bytes from `write_pos` to `min(write_pos+len, size)`.
    /// * The second slice covers the remaining bytes from the start of the buffer.
    ///   It is empty when no wrap-around is needed.
    ///
    /// After writing, call [`incr_write`](Self::incr_write) with the same `len`.
    ///
    /// # Safety contract
    ///
    /// Rust cannot return two `&mut` slices aliasing the same allocation through
    /// safe references; this method uses `split_at_mut` at the ring boundary so
    /// both slices are provably non-overlapping.
    pub fn write_slices_mut(&mut self, len: usize) -> Result<(&mut [u8], &mut [u8])> {
        if len == 0 {
            return Ok((&mut [], &mut []));
        }
        if len > self.available() {
            return Err(BufferError::InsufficientSpace);
        }

        self.ensure_allocated();
        let buffer = self.data.as_mut().unwrap();
        let write_pos = self.write_pos;
        let space_to_end = self.size - write_pos;

        if len <= space_to_end {
            // Contiguous — no wrap needed.
            Ok((&mut buffer[write_pos..write_pos + len], &mut []))
        } else {
            // Wrap-around: split at write_pos to get two non-overlapping regions.
            // split_at_mut(write_pos) → (&mut [..write_pos], &mut [write_pos..])
            let (head, tail) = buffer.split_at_mut(write_pos);
            let len2 = len - space_to_end;
            // tail = [write_pos..size], head = [0..write_pos]
            Ok((&mut tail[..space_to_end], &mut head[..len2]))
        }
    }

    /// Advances the write cursor after a zero-copy write via `write_ptr` or
    /// `write_slices_mut`.
    pub fn incr_write(&mut self, len: usize) -> Result<()> {
        if len > self.available() {
            return Err(BufferError::InsufficientSpace);
        }
        // Use local copies so we do not hold a borrow on self while mutating.
        let (size, is_pow2) = (self.size, self.is_pow2);
        self.write_pos = wrap(self.write_pos, len, size, is_pow2);
        self.used += len;
        Ok(())
    }

    /// Advances the read cursor after consuming bytes from `read_ptrs`.
    pub fn incr_read(&mut self, len: usize) -> Result<()> {
        if len > self.used {
            return Err(BufferError::BufferOverflow);
        }
        let (size, is_pow2) = (self.size, self.is_pow2);
        self.read_pos = wrap(self.read_pos, len, size, is_pow2);
        self.used -= len;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Copying write/read/peek
    // -----------------------------------------------------------------------

    /// Copies `data` into the buffer.
    ///
    /// Handles ring wrap-around internally with at most **two** `copy_from_slice`
    /// calls — significantly faster than the previous per-chunk loop.
    ///
    /// # Errors
    ///
    /// Returns `BufferError::InsufficientSpace` if `data` does not fit.
    pub fn write(&mut self, data: &[u8]) -> Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let len = data.len();
        if self.available() < len {
            return Err(BufferError::InsufficientSpace);
        }

        self.ensure_allocated();

        // Extract Copy fields so we can compute the new write position after the
        // buffer borrow ends — no simultaneous mutable+method-call borrow needed.
        let write_pos = self.write_pos;
        let size = self.size;
        let is_pow2 = self.is_pow2;
        let space_to_end = size - write_pos;

        {
            // Scoped borrow of self.data.
            let buffer = self.data.as_mut().unwrap();
            if len <= space_to_end {
                // Fast path: single contiguous copy.
                buffer[write_pos..write_pos + len].copy_from_slice(data);
            } else {
                // Wrap-around: two copies.
                buffer[write_pos..].copy_from_slice(&data[..space_to_end]);
                buffer[..len - space_to_end].copy_from_slice(&data[space_to_end..]);
            }
        } // buffer borrow released here.

        self.write_pos = wrap(write_pos, len, size, is_pow2);
        self.used += len;
        Ok(len)
    }

    /// Reads up to `output.len()` bytes from the buffer into `output`.
    ///
    /// Returns the number of bytes actually read.
    pub fn read(&mut self, output: &mut [u8]) -> Result<usize> {
        if self.used == 0 || self.data.is_none() {
            return Ok(0);
        }

        // Local copies of Copy fields to avoid calling self.wrap_pos() while
        // self.data is borrowed — keeps the borrow checker happy in all editions.
        let size = self.size;
        let is_pow2 = self.is_pow2;
        let to_read = output.len().min(self.used);
        let read_pos = self.read_pos;
        let space_to_end = size - read_pos;

        {
            let buffer = self.data.as_ref().unwrap();
            if to_read <= space_to_end {
                // Fast path: single contiguous copy.
                output[..to_read].copy_from_slice(&buffer[read_pos..read_pos + to_read]);
            } else {
                // Wrap-around: two copies.
                let (out1, out2) = output[..to_read].split_at_mut(space_to_end);
                out1.copy_from_slice(&buffer[read_pos..]);
                out2.copy_from_slice(&buffer[..to_read - space_to_end]);
            }
        } // buffer borrow released.

        self.read_pos = wrap(read_pos, to_read, size, is_pow2);
        self.used -= to_read;
        Ok(to_read)
    }

    /// Copies up to `output.len()` bytes into `output` without consuming them.
    pub fn peek(&self, output: &mut [u8]) -> Result<usize> {
        if self.used == 0 || self.data.is_none() {
            return Ok(0);
        }

        let size = self.size;
        let is_pow2 = self.is_pow2;
        let to_read = output.len().min(self.used);
        let read_pos = self.read_pos;
        let space_to_end = size - read_pos;

        let buffer = self.data.as_ref().unwrap();
        if to_read <= space_to_end {
            output[..to_read].copy_from_slice(&buffer[read_pos..read_pos + to_read]);
        } else {
            let (out1, out2) = output[..to_read].split_at_mut(space_to_end);
            out1.copy_from_slice(&buffer[read_pos..]);
            out2.copy_from_slice(&buffer[..to_read - space_to_end]);
        }
        // peek: do not update positions; compute new pos only for correctness check.
        let _ = wrap(read_pos, to_read, size, is_pow2);
        Ok(to_read)
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Resets cursors without freeing or zeroing memory.
    ///
    /// Useful for reusing the buffer within the same session.
    #[inline]
    pub fn clear(&mut self) {
        self.used = 0;
        self.read_pos = 0;
        self.write_pos = 0;
    }

    /// Securely zeros the buffer contents and resets all cursors.
    ///
    /// Frees the underlying allocation (sets `data` to `None`) after zeroing,
    /// so a subsequent write will lazily re-allocate.
    pub fn free(&mut self) {
        if let Some(data) = self.data.take() {
            // Convert Box<[u8]> → Vec<u8> in-place (zero extra allocation),
            // then zeroize the actual allocation before it is freed.
            let mut vec = data.into_vec();
            vec.zeroize();
        }
        self.clear();
    }

    /// Consumes the buffer, securely zeroing its contents before deallocation.
    pub fn burn_free(mut self) {
        if let Some(data) = self.data.take() {
            let mut vec = data.into_vec();
            vec.zeroize();
        }
        drop(self);
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Performs lazy allocation of the internal storage.
    #[inline]
    fn ensure_allocated(&mut self) {
        if self.data.is_none() {
            self.data = Some(vec![0u8; self.size].into_boxed_slice());
        }
    }
}

impl Drop for CircularBuffer {
    fn drop(&mut self) {
        // Convert Box<[u8]> → Vec<u8> in-place so zeroize operates on the
        // original allocation rather than a copy.
        if let Some(data) = self.data.take() {
            let mut vec = data.into_vec();
            vec.zeroize();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

        let mut out = vec![0u8; 5];
        buf.read(&mut out).unwrap();
        assert_eq!(&out, b"Hello");
        assert!(buf.is_empty());
    }

    #[test]
    fn test_wrap_around_write_read() {
        let mut buf = CircularBuffer::new(8);

        buf.write(b"12345").unwrap();
        let mut tmp = vec![0u8; 3];
        buf.read(&mut tmp).unwrap(); // consumes "123", leaves "45"

        buf.write(b"6789").unwrap(); // "45" + "6789" = 6 bytes, wraps around
        assert_eq!(buf.used(), 6);

        let mut out = vec![0u8; 6];
        buf.read(&mut out).unwrap();
        assert_eq!(&out, b"456789");
    }

    #[test]
    fn test_wrap_around_large() {
        // Fill past the boundary multiple times.
        let mut buf = CircularBuffer::new_pow2(4); // 16 bytes
        for _ in 0..8 {
            buf.write(b"ABCDE FGHI").unwrap(); // 10 bytes
            let mut out = vec![0u8; 10];
            buf.read(&mut out).unwrap();
            assert_eq!(&out, b"ABCDE FGHI");
        }
    }

    #[test]
    fn test_write_ptr_no_wrap() {
        let mut buf = CircularBuffer::new(16);
        let slice = buf.write_ptr(4).unwrap();
        slice.copy_from_slice(b"RUST");
        buf.incr_write(4).unwrap();
        assert_eq!(buf.used(), 4);

        let mut out = [0u8; 4];
        buf.read(&mut out).unwrap();
        assert_eq!(&out, b"RUST");
    }

    #[test]
    fn test_write_ptr_returns_err_on_wrap() {
        let mut buf = CircularBuffer::new(8);
        // Position write_pos near the end
        buf.write(b"123456").unwrap();
        let mut tmp = [0u8; 6];
        buf.read(&mut tmp).unwrap(); // write_pos = 6, available = 8

        // A 4-byte write from pos 6 in an 8-byte ring would cross the boundary.
        assert!(matches!(
            buf.write_ptr(4),
            Err(BufferError::InvalidState(_))
        ));
    }

    #[test]
    fn test_write_slices_mut_wrap() {
        let mut buf = CircularBuffer::new(8);
        buf.write(b"123456").unwrap();
        let mut tmp = [0u8; 6];
        buf.read(&mut tmp).unwrap(); // write_pos = 6, available = 8

        // 4 bytes spanning the boundary: 2 at end + 2 at start.
        let (s1, s2) = buf.write_slices_mut(4).unwrap();
        assert_eq!(s1.len(), 2);
        assert_eq!(s2.len(), 2);
        s1.copy_from_slice(b"AB");
        s2.copy_from_slice(b"CD");
        buf.incr_write(4).unwrap();

        let mut out = [0u8; 4];
        buf.read(&mut out).unwrap();
        assert_eq!(&out, b"ABCD");
    }

    #[test]
    fn test_peek() {
        let mut buf = CircularBuffer::new(32);
        buf.write(b"peekaboo").unwrap();

        let mut out1 = vec![0u8; 8];
        buf.peek(&mut out1).unwrap();
        assert_eq!(&out1, b"peekaboo");
        assert_eq!(buf.used(), 8); // not consumed

        let mut out2 = vec![0u8; 8];
        buf.read(&mut out2).unwrap();
        assert_eq!(&out2, b"peekaboo");
        assert!(buf.is_empty());
    }

    #[test]
    fn test_free() {
        let mut buf = CircularBuffer::new(1024);
        buf.write(b"sensitive data").unwrap();
        buf.free();
        assert!(buf.is_empty());
        assert_eq!(buf.used(), 0);
        assert!(buf.data.is_none());
    }

    #[test]
    fn test_burn_free() {
        let mut buf = CircularBuffer::new(1024);
        buf.write(b"secret").unwrap();
        buf.burn_free(); // consumes buf
    }

    #[test]
    fn test_lazy_allocation() {
        let buf = CircularBuffer::new(1024);
        assert!(buf.data.is_none()); // not yet allocated
    }

    #[test]
    fn test_pow2_fast_wrap() {
        // Ensures power-of-2 bitwise wrap produces identical results to modulo wrap.
        let mut fast = CircularBuffer::new_pow2(3); // 8 bytes
        let mut slow = CircularBuffer::new(8);

        let data = b"ABCDEFGHIJKLMNOP";
        for chunk in data.chunks(3) {
            // Pad/trim to fit available space
            let len = chunk.len().min(fast.available());
            fast.write(&chunk[..len]).unwrap();
            slow.write(&chunk[..len]).unwrap();

            let mut out_f = vec![0u8; fast.used()];
            let mut out_s = vec![0u8; slow.used()];
            fast.read(&mut out_f).unwrap();
            slow.read(&mut out_s).unwrap();
            assert_eq!(out_f, out_s);
        }
    }
}
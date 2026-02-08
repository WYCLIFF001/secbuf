// src/buffer/ops.rs
//! Buffer read/write operations

use super::core::Buffer;
use crate::error::{BufferError, Result};

/// Maximum SSH-style string length (400KB - reasonable for SSH packets)
const MAX_STRING_LEN: usize = 400_000;

/// Maximum pointer/slice length to prevent accidental huge allocations (100MB)
const MAX_PTR_LEN: usize = 100_000_000;

impl Buffer {
    /// Writes a `u32` in big-endian format with bounds checking.
    #[inline]
    pub fn put_u32(&mut self, val: u32) -> Result<()> {
        if self.pos + 4 > self.data.len() {
            return Err(BufferError::BufferOverflow);
        }
        unsafe {
            self.put_u32_unchecked(val);
        }
        Ok(())
    }

    /// Reads a `u32` in big-endian format with bounds checking.
    #[inline]
    pub fn get_u32(&mut self) -> Result<u32> {
        if self.pos + 4 > self.len {
            return Err(BufferError::BufferOverflow);
        }
        Ok(unsafe { self.get_u32_unchecked() })
    }

    /// Writes a `u64` in big-endian format with bounds checking.
    #[inline]
    pub fn put_u64(&mut self, val: u64) -> Result<()> {
        if self.pos + 8 > self.data.len() {
            return Err(BufferError::BufferOverflow);
        }
        unsafe {
            self.put_u64_unchecked(val);
        }
        Ok(())
    }

    /// Reads a `u64` in big-endian format with bounds checking.
    #[inline]
    pub fn get_u64(&mut self) -> Result<u64> {
        if self.pos + 8 > self.len {
            return Err(BufferError::BufferOverflow);
        }
        Ok(unsafe { self.get_u64_unchecked() })
    }

    /// Writes bytes with a single bounds check.
    #[inline]
    pub fn put_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if self.pos + bytes.len() > self.data.len() {
            return Err(BufferError::BufferOverflow);
        }
        unsafe {
            self.put_bytes_unchecked(bytes);
        }
        Ok(())
    }

    /// Reads bytes, returning an owned `Vec`.
    #[inline]
    pub fn get_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        if self.pos + len > self.len {
            return Err(BufferError::BufferOverflow);
        }
        let bytes = unsafe { self.get_bytes_unchecked(len) };
        Ok(bytes.to_vec())
    }

    /// Reads bytes as a slice reference (zero-copy).
    #[inline]
    pub fn get_bytes_ref(&mut self, len: usize) -> Result<&[u8]> {
        if self.pos + len > self.len {
            return Err(BufferError::BufferOverflow);
        }
        Ok(unsafe { self.get_bytes_unchecked(len) })
    }

    /// Gets a reference to data at current position without advancing.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::BufferOverflow`] if:
    /// - `len` exceeds [`MAX_PTR_LEN`] (prevents accidental huge allocations)
    /// - Current position + `len` exceeds valid data length
    pub fn get_ptr(&self, len: usize) -> Result<&[u8]> {
        if len > MAX_PTR_LEN {
            return Err(BufferError::InvalidData(format!(
                "Requested slice length {} exceeds maximum {}",
                len, MAX_PTR_LEN
            )));
        }
        if self.pos + len > self.len {
            return Err(BufferError::BufferOverflow);
        }
        Ok(&self.data[self.pos..self.pos + len])
    }

    /// Gets a mutable reference to data at current position (for writing).
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::BufferOverflow`] if:
    /// - `len` exceeds [`MAX_PTR_LEN`] (prevents accidental huge allocations)
    /// - Current position + `len` exceeds buffer capacity
    pub fn get_write_ptr(&mut self, len: usize) -> Result<&mut [u8]> {
        if len > MAX_PTR_LEN {
            return Err(BufferError::InvalidData(format!(
                "Requested slice length {} exceeds maximum {}",
                len, MAX_PTR_LEN
            )));
        }
        if self.pos + len > self.data.len() {
            return Err(BufferError::BufferOverflow);
        }
        Ok(&mut self.data[self.pos..self.pos + len])
    }

    /// Writes a single byte.
    #[inline]
    pub fn put_byte(&mut self, val: u8) -> Result<()> {
        if self.pos >= self.data.len() {
            return Err(BufferError::BufferOverflow);
        }
        self.data[self.pos] = val;
        self.pos += 1;
        if self.pos > self.len {
            self.len = self.pos;
        }
        Ok(())
    }

    /// Reads a single byte.
    #[inline]
    pub fn get_byte(&mut self) -> Result<u8> {
        if self.pos >= self.len {
            return Err(BufferError::BufferOverflow);
        }
        let byte = self.data[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    /// Reads a boolean (0 = false, non-zero = true).
    #[inline]
    pub fn get_bool(&mut self) -> Result<bool> {
        Ok(self.get_byte()? != 0)
    }

    /// Writes an SSH-style string (4-byte length prefix + data).
    #[inline]
    pub fn put_string(&mut self, s: &[u8]) -> Result<()> {
        if s.len() > MAX_STRING_LEN {
            return Err(BufferError::InvalidString);
        }
        self.put_u32(s.len() as u32)?;
        self.put_bytes(s)
    }

    /// Reads an SSH-style string (4-byte length prefix + data).
    #[inline]
    pub fn get_string(&mut self) -> Result<Vec<u8>> {
        let len = self.get_u32()? as usize;
        if len > MAX_STRING_LEN {
            return Err(BufferError::InvalidString);
        }
        self.get_bytes(len)
    }

    /// Skips over an SSH-style string without reading the data.
    #[inline]
    pub fn eat_string(&mut self) -> Result<()> {
        let len = self.get_u32()? as usize;
        if len > MAX_STRING_LEN {
            return Err(BufferError::InvalidString);
        }
        self.incr_pos(len)
    }

    /// SIMD-accelerated bulk copy for large buffers (≥64 bytes).
    ///
    /// # Performance
    ///
    /// This method uses AVX2 instructions on x86_64 when available for
    /// significantly faster copies of large data (2-3x faster for 1KB+ buffers).
    ///
    /// On other architectures or when AVX2 is unavailable, falls back to
    /// the standard implementation.
    #[cfg(target_arch = "x86_64")]
    #[inline]
    pub fn put_bytes_fast(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() < 64 {
            return self.put_bytes(bytes);
        }

        if self.pos + bytes.len() > self.data.len() {
            return Err(BufferError::BufferOverflow);
        }

        if is_x86_feature_detected!("avx2") {
            unsafe {
                self.put_bytes_avx2_impl(bytes);
            }
        } else {
            unsafe {
                self.put_bytes_unchecked(bytes);
            }
        }

        Ok(())
    }

    /// AVX2 implementation for fast bulk copy.
    ///
    /// # Safety
    ///
    /// Caller must ensure:
    /// - AVX2 is available (checked by `is_x86_feature_detected!`)
    /// - `self.pos + bytes.len() <= self.data.len()` (bounds checked by caller)
    ///
    /// # Implementation Details
    ///
    /// Uses 256-bit (32-byte) AVX2 loads and stores to copy data in chunks.
    /// Remaining bytes (<32) are copied with standard memcpy.
    #[cfg(target_arch = "x86_64")]
    #[inline]
    unsafe fn put_bytes_avx2_impl(&mut self, bytes: &[u8]) {
        use std::arch::x86_64::*;

        let mut src = bytes.as_ptr();
        let mut dst = unsafe { self.data.as_mut_ptr().add(self.pos) };
        let mut remaining = bytes.len();

        // Copy 32-byte chunks using AVX2
        while remaining >= 32 {
            let chunk = unsafe { _mm256_loadu_si256(src as *const __m256i) };
            unsafe { _mm256_storeu_si256(dst as *mut __m256i, chunk) };
            src = unsafe { src.add(32) };
            dst = unsafe { dst.add(32) };
            remaining -= 32;
        }

        // Copy remaining bytes (<32)
        unsafe { std::ptr::copy_nonoverlapping(src, dst, remaining) };

        self.pos += bytes.len();
        if self.pos > self.len {
            self.len = self.pos;
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    #[inline]
    pub fn put_bytes_fast(&mut self, bytes: &[u8]) -> Result<()> {
        self.put_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_ptr_max_len() {
        let buf = Buffer::new(1024);
        let result = buf.get_ptr(MAX_PTR_LEN + 1);
        assert!(matches!(result, Err(BufferError::InvalidData(_))));
    }

    #[test]
    fn test_get_write_ptr_max_len() {
        let mut buf = Buffer::new(1024);
        let result = buf.get_write_ptr(MAX_PTR_LEN + 1);
        assert!(matches!(result, Err(BufferError::InvalidData(_))));
    }

    #[test]
    fn test_put_get_u32() {
        let mut buf = Buffer::new(1024);
        buf.put_u32(0x12345678).unwrap();
        buf.set_pos(0).unwrap();
        assert_eq!(buf.get_u32().unwrap(), 0x12345678);
    }

    #[test]
    fn test_put_get_u64() {
        let mut buf = Buffer::new(1024);
        buf.put_u64(0xDEADBEEFCAFEBABE).unwrap();
        buf.set_pos(0).unwrap();
        assert_eq!(buf.get_u64().unwrap(), 0xDEADBEEFCAFEBABE);
    }

    #[test]
    fn test_put_get_string() {
        let mut buf = Buffer::new(1024);
        buf.put_string(b"Hello, World!").unwrap();
        buf.set_pos(0).unwrap();
        assert_eq!(buf.get_string().unwrap(), b"Hello, World!");
    }

    #[test]
    fn test_string_too_long() {
        let mut buf = Buffer::new(10 * 1024 * 1024); // 10MB buffer
        let long_string = vec![0u8; MAX_STRING_LEN + 1];
        let result = buf.put_string(&long_string);
        assert!(matches!(result, Err(BufferError::InvalidString)));
    }

    #[test]
    fn test_eat_string() {
        let mut buf = Buffer::new(1024);
        buf.put_string(b"skip this").unwrap();
        buf.put_u32(42).unwrap();

        buf.set_pos(0).unwrap();
        buf.eat_string().unwrap();
        assert_eq!(buf.get_u32().unwrap(), 42);
    }
}

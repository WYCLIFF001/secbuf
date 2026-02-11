// src/buffer/ops.rs
//! Buffer read/write operations

use super::core::Buffer;
use crate::error::{BufferError, Result};

/// Maximum SSH-style string length
const MAX_STRING_LEN: usize = 400_000;

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
    pub fn get_ptr(&self, len: usize) -> Result<&[u8]> {
        if len > 1_000_000_000 || self.pos + len > self.len {
            return Err(BufferError::BufferOverflow);
        }
        Ok(&self.data[self.pos..self.pos + len])
    }

    /// Gets a mutable reference to data at current position (for writing).
    pub fn get_write_ptr(&mut self, len: usize) -> Result<&mut [u8]> {
        if len > 1_000_000_000 || self.pos + len > self.data.len() {
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

    /// SIMD-accelerated bulk copy for large buffers (â‰¥64 bytes).
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

    #[cfg(target_arch = "x86_64")]
    #[inline]
    unsafe fn put_bytes_avx2_impl(&mut self, bytes: &[u8]) {
        use std::arch::x86_64::*;

        let mut src = bytes.as_ptr();
        let mut dst = unsafe { self.data.as_mut_ptr().add(self.pos) };
        let mut remaining = bytes.len();

        while remaining >= 32 {
            let chunk = unsafe { _mm256_loadu_si256(src as *const __m256i) };
            unsafe { _mm256_storeu_si256(dst as *mut __m256i, chunk) };
            src = unsafe { src.add(32) };
            dst = unsafe { dst.add(32) };
            remaining -= 32;
        }

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
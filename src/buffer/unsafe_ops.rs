// src/buffer/unsafe_ops.rs
//! Unchecked (unsafe) buffer operations for maximum performance

use super::core::Buffer;

impl Buffer {
    /// Writes a `u32` in big-endian format without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller MUST guarantee: `self.pos + 4 <= self.capacity()`.
    #[inline(always)]
    pub unsafe fn put_u32_unchecked(&mut self, val: u32) {
        debug_assert!(
            self.pos + 4 <= self.capacity(),
            "put_u32_unchecked: pos {} + 4 > capacity {}",
            self.pos,
            self.capacity()
        );

        let ptr = unsafe { self.data.as_mut_ptr().add(self.pos) };
        unsafe { ptr.cast::<u32>().write_unaligned(val.to_be()) };
        self.pos += 4;
        if self.pos > self.len {
            self.len = self.pos;
        }
    }

    /// Reads a `u32` in big-endian format without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller MUST guarantee: `self.pos + 4 <= self.len`.
    #[inline(always)]
    pub unsafe fn get_u32_unchecked(&mut self) -> u32 {
        debug_assert!(
            self.pos + 4 <= self.len,
            "get_u32_unchecked: pos {} + 4 > len {}",
            self.pos,
            self.len
        );

        let ptr = unsafe { self.data.as_ptr().add(self.pos) };
        let val = unsafe { ptr.cast::<u32>().read_unaligned() };
        self.pos += 4;
        u32::from_be(val)
    }

    /// Writes a `u64` in big-endian format without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller MUST guarantee: `self.pos + 8 <= self.capacity()`.
    #[inline(always)]
    pub unsafe fn put_u64_unchecked(&mut self, val: u64) {
        debug_assert!(self.pos + 8 <= self.capacity());

        let ptr = unsafe { self.data.as_mut_ptr().add(self.pos) };
        unsafe { ptr.cast::<u64>().write_unaligned(val.to_be()) };
        self.pos += 8;
        if self.pos > self.len {
            self.len = self.pos;
        }
    }

    /// Reads a `u64` in big-endian format without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller MUST guarantee: `self.pos + 8 <= self.len`.
    #[inline(always)]
    pub unsafe fn get_u64_unchecked(&mut self) -> u64 {
        debug_assert!(self.pos + 8 <= self.len);

        let ptr = unsafe { self.data.as_ptr().add(self.pos) };
        let val = unsafe { ptr.cast::<u64>().read_unaligned() };
        self.pos += 8;
        u64::from_be(val)
    }

    /// Writes bytes without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller MUST guarantee: `self.pos + bytes.len() <= self.capacity()`.
    #[inline]
    pub unsafe fn put_bytes_unchecked(&mut self, bytes: &[u8]) {
        debug_assert!(self.pos + bytes.len() <= self.capacity());

        let ptr = unsafe { self.data.as_mut_ptr().add(self.pos) };
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
        self.pos += bytes.len();
        if self.pos > self.len {
            self.len = self.pos;
        }
    }

    /// Reads bytes without bounds checking, returning a slice.
    ///
    /// # Safety
    ///
    /// Caller MUST guarantee: `self.pos + len <= self.len`.
    #[inline]
    pub unsafe fn get_bytes_unchecked(&mut self, len: usize) -> &[u8] {
        debug_assert!(self.pos + len <= self.len);

        let ptr = unsafe { self.data.as_ptr().add(self.pos) };
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        self.pos += len;
        slice
    }
}

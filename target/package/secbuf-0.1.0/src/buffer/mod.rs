// src/buffer/mod.rs
//! High-performance buffer implementation

pub mod core;
pub(crate) mod ops;
pub(crate) mod unsafe_ops;

pub use core::Buffer;

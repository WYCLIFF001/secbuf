// src/pool/mod.rs
//! Buffer pool implementation with multiple modes.

pub(crate) mod config;
pub(crate) mod fast;
pub(crate) mod standard;
pub(crate) mod stats;

pub use config::PoolConfig;
pub use fast::{FastBufferPool, FastPooledBuffer};
pub use standard::{BufferPool, PooledBuffer};
pub use stats::{FastPoolStats, PoolStats};

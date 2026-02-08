# secbuf

[![Crates.io](https://img.shields.io/crates/v/secbuf.svg)](https://crates.io/crates/secbuf)
[![Documentation](https://docs.rs/secbuf/badge.svg)](https://docs.rs/secbuf)
[![License](https://img.shields.io/crates/l/secbuf.svg)](https://github.com/yourusername/secbuf#license)
[![Build Status](https://img.shields.io/github/actions/workflow/status/yourusername/secbuf/ci.yml?branch=main)](https://github.com/yourusername/secbuf/actions)

**Secure, high-performance buffer management for Rust with automatic memory zeroing and aggressive cleanup.**

`secbuf` provides memory-safe buffers that automatically zero their contents on drop, preventing sensitive data from lingering in memory. Inspired by [Dropbear SSH](https://github.com/mkj/dropbear)'s buffer management patterns, it's designed for security-critical applications like network servers, cryptographic systems, and protocol implementations.

## ✨ Features

- 🔒 **Automatic Secure Zeroing** - All buffers use compiler-resistant memory clearing via [`zeroize`](https://crates.io/crates/zeroize)
- ⚡ **High Performance** - Lock-free buffer pools with thread-local caching (~5ns allocation)
- 🎯 **Zero-Copy Operations** - Efficient circular buffers for streaming data
- 🧹 **Aggressive Memory Management** - Detect and reclaim idle buffers automatically
- 🔄 **Connection Lifecycle** - Built-in connection-scoped buffer management
- 🚀 **SIMD Acceleration** - Optional AVX2 support for bulk operations (x86_64)
- 📊 **Memory Diagnostics** - Detailed statistics and waste detection

## 🚀 Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
secbuf = "0.1"
```

### Basic Usage

```rust
use secbuf::prelude::*;

fn main() -> Result<(), BufferError> {
    // Create a buffer
    let mut buf = Buffer::new(1024);
    
    // Write data
    buf.put_u32(42)?;
    buf.put_string(b"Hello, secure world!")?;
    
    // Read data back
    buf.set_pos(0)?;
    assert_eq!(buf.get_u32()?, 42);
    assert_eq!(buf.get_string()?, b"Hello, secure world!");
    
    // Buffer is automatically and securely zeroed on drop
    Ok(())
}
```

### Buffer Pooling

```rust
use secbuf::prelude::*;
use std::sync::Arc;

// Create a buffer pool
let pool = Arc::new(BufferPool::new(PoolConfig {
    buffer_size: 8192,
    max_pool_size: 100,
    min_pool_size: 10,
}));

// Acquire buffers (automatically returned on drop)
let mut buf1 = pool.acquire();
buf1.put_bytes(b"pooled buffer")?;

// Buffers are reused, not reallocated
let mut buf2 = pool.acquire();

// Check pool statistics
let stats = pool.stats();
println!("Pool hit rate: {:.1}%", stats.hit_rate());
```

### Circular Buffers for Streaming

```rust
use secbuf::CircularBuffer;

// Create a circular buffer with lazy allocation
let mut ring = CircularBuffer::new(4096);

// Write data
ring.write(b"chunk 1")?;
ring.write(b"chunk 2")?;

// Read data
let mut output = vec![0u8; 14];
ring.read(&mut output)?;

assert_eq!(&output, b"chunk 1chunk 2");
```

### Connection Management

```rust
use secbuf::{ConnectionBuffers, ConnectionBufferConfig};

// Create connection-scoped buffers
let mut conn = ConnectionBuffers::with_config(ConnectionBufferConfig {
    max_packet_queue_size: 100,
    max_packet_queue_bytes: 10_485_760, // 10MB
    idle_timeout_secs: 60,
    enable_aggressive_shrinking: true,
});

// Initialize buffers as needed
conn.init_read_buf(8192);
conn.init_write_buf(8192);
conn.add_stream_buf(4096);

// Automatic cleanup on drop with secure zeroing
```

## 🏗️ Architecture

### Buffer Types

| Type | Use Case | Key Features |
|------|----------|--------------|
| **`Buffer`** | Linear read/write | Position tracking, SSH-style strings, SIMD support |
| **`CircularBuffer`** | Streaming I/O | Lazy allocation, wrap-around, zero-copy |
| **`BufferPool`** | Standard pooling | Mutex-based, simple, reliable |
| **`FastBufferPool`** | High-throughput | Lock-free, thread-local cache, 10-20x faster |
| **`ConnectionBuffers`** | Connection lifecycle | Multiple buffers, packet queue, diagnostics |

### Memory Management Levels

1. **Automatic** - Secure zeroing on drop (always enabled)
2. **Periodic** - Idle detection and shrinking (configurable)
3. **Aggressive** - Force cleanup on memory pressure

## 📖 Examples

### SSH-Style Protocol Buffer

```rust
use secbuf::Buffer;

// Write an SSH-style packet
let mut buf = Buffer::new(1024);
buf.put_string(b"ssh-userauth")?;      // Method name
buf.put_string(b"alice")?;              // Username
buf.put_string(b"ssh-connection")?;     // Service
buf.put_string(b"password")?;           // Auth type
buf.put_byte(0)?;                       // Password change flag
buf.put_string(b"secret123")?;          // Password

// Read it back
buf.set_pos(0)?;
let method = buf.get_string()?;
let username = buf.get_string()?;
// ... process packet

// Password is automatically zeroed when buf drops
```

### High-Performance Server

```rust
use secbuf::{FastBufferPool, PoolConfig};
use std::sync::Arc;
use std::thread;

let pool = Arc::new(FastBufferPool::new(PoolConfig {
    buffer_size: 8192,
    max_pool_size: 1000,
    min_pool_size: 50,
}));

// Spawn worker threads
let handles: Vec<_> = (0..8).map(|_| {
    let pool = Arc::clone(&pool);
    thread::spawn(move || {
        for _ in 0..10_000 {
            let mut buf = pool.acquire(); // ~5ns from thread-local cache
            buf.put_u32(42).unwrap();
            // Automatically returned to pool on drop
        }
    })
}).collect();

for h in handles {
    h.join().unwrap();
}

// Check performance stats
let stats = pool.stats();
println!("Cache hit rate: {:.1}%", stats.cache_hit_rate());
println!("Pool hit rate: {:.1}%", stats.pool_hit_rate());
```

### Memory Diagnostics

```rust
use secbuf::ConnectionBuffers;

let mut conn = ConnectionBuffers::new();
conn.init_read_buf(65536);

// Check memory usage
let stats = conn.memory_usage();
println!("Total: {} bytes", stats.total_bytes);
println!("Used: {} bytes", stats.total_used);
println!("Wasted: {} bytes", stats.total_wasted);
println!("Efficiency: {:.1}%", stats.efficiency() * 100.0);

if stats.is_problematic() {
    println!("Warning: Inefficient memory usage detected!");
    conn.force_shrink(); // Reclaim wasted memory
}
```

## 🔒 Security Guarantees

### Automatic Memory Zeroing

All buffers are zeroed on drop using the [`zeroize`](https://crates.io/crates/zeroize) crate:

```rust
{
    let mut buf = Buffer::new(1024);
    buf.put_string(b"sensitive data")?;
} // <- Memory is securely zeroed here (compiler can't optimize it away)
```

### Explicit Cleanup

For extra control:

```rust
let mut buf = Buffer::new(1024);
buf.put_bytes(b"secret")?;

// Immediate secure zeroing
buf.burn();

// Or zero + free memory
buf.burn_and_free_memory();
```

### Connection Cleanup

```rust
let mut conn = ConnectionBuffers::new();
conn.init_read_buf(8192);
// ... use buffers

// Secure cleanup
conn.burn(); // Zero all data
conn.aggressive_cleanup(); // Zero + free all memory
```

## ⚡ Performance

### Buffer Pool Comparison

| Operation | `BufferPool` | `FastBufferPool` | Speedup |
|-----------|--------------|------------------|---------|
| Acquire (cache hit) | ~100ns | ~5ns | **20x** |
| Acquire (pool hit) | ~100ns | ~20ns | **5x** |
| Acquire (miss) | ~100ns | ~100ns | 1x |

### SIMD Acceleration

On x86_64 with AVX2:

```rust
// Automatically uses AVX2 if available
buf.put_bytes_fast(&large_data)?; // 2-3x faster for 1KB+ data
```

## 🧪 Testing

Run the test suite:

```bash
cargo test
```

Run with all features:

```bash
cargo test --all-features
```

Benchmark:

```bash
cargo bench
```

## 📚 Documentation

- [API Documentation](https://docs.rs/secbuf)
- [Examples](https://github.com/WYCLIFF001/secbuf/tree/main/examples)

## 🛠️ Feature Flags

Currently, all features are enabled by default. Future versions may add:

- `simd` - SIMD acceleration (planned)
- `anyhow` - Error conversion support (planned)

## 🤝 Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure `cargo test` passes
5. Submit a pull request


### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

## 🙏 Acknowledgments

- Inspired by [Dropbear SSH](https://github.com/mkj/dropbear)'s secure buffer management
- Built on the excellent [`zeroize`](https://crates.io/crates/zeroize) crate by RustCrypto
- Lock-free queues via [`crossbeam`](https://crates.io/crates/crossbeam)



## 🔗 Related Projects

- [`bytes`](https://crates.io/crates/bytes) - General-purpose buffer management
- [`zeroize`](https://crates.io/crates/zeroize) - Secure memory zeroing
- [`buffer-pool`](https://crates.io/crates/buffer-pool) - Simple buffer pooling

---

**Made with ❤️ for the Rust community**
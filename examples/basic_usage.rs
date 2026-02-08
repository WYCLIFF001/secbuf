// examples/basic_usage.rs
//! Basic usage example of the buffer module

use secbuf::prelude::*;

fn main() -> Result<()> {
    println!("=== Basic Buffer Usage ===\n");

    // 1. Create and use a basic buffer
    let mut buf = Buffer::new(1024);

    // Write some data
    buf.put_u32(12345)?;
    buf.put_bytes(b"Hello, World!")?;
    buf.put_byte(0xFF)?;

    println!("Buffer length: {}", buf.len());
    println!("Buffer position: {}", buf.pos());

    // Read the data back
    buf.set_pos(0)?;
    let num = buf.get_u32()?;
    let bytes = buf.get_bytes(13)?;
    let byte = buf.get_byte()?;

    println!("Read u32: {}", num);
    println!("Read bytes: {:?}", String::from_utf8_lossy(&bytes));
    println!("Read byte: 0x{:02X}", byte);

    println!("\n=== SSH-Style String Operations ===\n");

    // SSH-style strings (length-prefixed)
    let mut buf2 = Buffer::new(1024);
    buf2.put_string(b"This is a length-prefixed string")?;
    buf2.put_string(b"Another one!")?;

    buf2.set_pos(0)?;
    let str1 = buf2.get_string()?;
    let str2 = buf2.get_string()?;

    println!("String 1: {:?}", String::from_utf8_lossy(&str1));
    println!("String 2: {:?}", String::from_utf8_lossy(&str2));

    println!("\n=== Buffer Pool Usage ===\n");

    // 2. Use a buffer pool for efficiency
    let pool = BufferPool::new(PoolConfig {
        buffer_size: 4096,
        max_pool_size: 50,
        min_pool_size: 5,
    });

    println!("Pool initialized with {} buffers", pool.available());

    // Acquire buffers from the pool
    {
        let mut buf1 = pool.acquire();
        let mut buf2 = pool.acquire();

        buf1.put_bytes(b"Buffer 1")?;
        buf2.put_bytes(b"Buffer 2")?;

        println!(
            "Acquired 2 buffers, pool has {} available",
            pool.available()
        );

        // Buffers are automatically returned when dropped
    }

    println!("Buffers returned, pool has {} available", pool.available());

    // Check pool statistics
    let stats = pool.stats();
    println!(
        "Pool stats: acquired={}, returned={}, hit_rate={:.1}%",
        stats.total_acquired,
        stats.total_returned,
        stats.hit_rate()
    );

    println!("\n=== Fast Buffer Pool ===\n");

    // 3. Use fast pool for high performance
    let fast_pool = FastBufferPool::default();

    {
        let mut buf = fast_pool.acquire();
        buf.put_string(b"Fast pool buffer")?;
    }

    let fast_stats = fast_pool.stats();
    println!(
        "Fast pool stats: acquired={}, cache_hit_rate={:.1}%",
        fast_stats.acquired,
        fast_stats.cache_hit_rate()
    );

    println!("\n=== Circular Buffer Usage ===\n");

    // 4. Use a circular buffer for streaming
    let mut cbuf = CircularBuffer::new(256);

    cbuf.write(b"First chunk of data")?;
    println!("Circular buffer used: {}/{}", cbuf.used(), cbuf.size());

    cbuf.write(b" | Second chunk")?;
    println!("Circular buffer used: {}/{}", cbuf.used(), cbuf.size());

    // Read some data
    let mut output = vec![0u8; 15];
    cbuf.read(&mut output)?;
    println!(
        "Read from circular buffer: {:?}",
        String::from_utf8_lossy(&output)
    );
    println!("Circular buffer used: {}/{}", cbuf.used(), cbuf.size());

    // Write more data (wraps around)
    cbuf.write(b" | Third chunk after wrap")?;
    println!("After wrap, buffer used: {}/{}", cbuf.used(), cbuf.size());

    Ok(())
}

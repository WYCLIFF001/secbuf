// examples/network_simulation.rs
//! Simulates network packet handling with buffer pooling

use secbuf::prelude::*;
use std::time::Instant;

fn main() -> Result<()> {
    println!("=== Network Packet Simulation ===\n");

    // Create a buffer pool for packet handling
    let pool = BufferPool::new(PoolConfig {
        buffer_size: 1500, // MTU size
        max_pool_size: 1000,
        min_pool_size: 100,
    });

    let num_packets = 10_000;
    let start = Instant::now();

    // Simulate processing packets
    for i in 0..num_packets {
        let mut packet = pool.acquire();

        // Write packet header
        packet.put_u32(i)?; // Sequence number
        packet.put_u32(1400)?; // Payload length

        // Write payload
        let payload = vec![0x42; 1400];
        packet.put_bytes(&payload)?;

        // Process packet (simulate)
        packet.set_pos(0)?;
        let seq = packet.get_u32()?;
        let len = packet.get_u32()?;

        if i % 1000 == 0 {
            println!("Processed packet {} (seq={}, len={})", i, seq, len);
        }

        // Packet buffer is automatically returned to pool when dropped
    }

    let elapsed = start.elapsed();
    println!("\nProcessed {} packets in {:?}", num_packets, elapsed);
    println!(
        "Average: {:.2} µs per packet",
        elapsed.as_micros() as f64 / num_packets as f64
    );

    let stats = pool.stats();
    println!("\nPool Statistics:");
    println!("  Available: {}", stats.available);
    println!("  Total allocated: {}", stats.total_allocated);
    println!("  Total acquired: {}", stats.total_acquired);
    println!("  Total returned: {}", stats.total_returned);
    println!("  In use: {}", stats.in_use());
    println!("  Hit rate: {:.1}%", stats.hit_rate());

    // Compare with non-pooled allocation
    println!("\n=== Non-Pooled Comparison ===\n");
    let start = Instant::now();

    for i in 0..num_packets {
        let mut packet = Buffer::new(1500); // Allocate fresh each time
        packet.put_u32(i)?;
        packet.put_u32(1400)?;
        let payload = vec![0x42; 1400];
        packet.put_bytes(&payload)?;

        // Drop happens here, buffer is freed
    }

    let elapsed_no_pool = start.elapsed();
    println!("Non-pooled time: {:?}", elapsed_no_pool);
    println!(
        "Speedup: {:.2}x faster with pooling",
        elapsed_no_pool.as_secs_f64() / elapsed.as_secs_f64()
    );

    // Fast pool comparison
    println!("\n=== Fast Pool Comparison ===\n");

    let fast_pool = FastBufferPool::new(PoolConfig {
        buffer_size: 1500,
        max_pool_size: 1000,
        min_pool_size: 100,
    });

    let start = Instant::now();
    for i in 0..num_packets {
        let mut packet = fast_pool.acquire();
        packet.put_u32(i)?;
        packet.put_u32(1400)?;
        let payload = vec![0x42; 1400];
        packet.put_bytes(&payload)?;
    }
    let elapsed_fast = start.elapsed();

    println!("Fast pool time: {:?}", elapsed_fast);
    println!(
        "Speedup vs standard: {:.2}x faster",
        elapsed.as_secs_f64() / elapsed_fast.as_secs_f64()
    );

    let fast_stats = fast_pool.stats();
    println!("\nFast Pool Statistics:");
    println!("  Cache hit rate: {:.1}%", fast_stats.cache_hit_rate());
    println!("  Pool hit rate: {:.1}%", fast_stats.pool_hit_rate());

    Ok(())
}

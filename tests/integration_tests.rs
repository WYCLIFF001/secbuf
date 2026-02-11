// tests/integration_tests.rs
//! Integration tests for the buffer module

use secbuf::prelude::*;

#[test]
fn test_ssh_packet_simulation() {
    // Simulate SSH packet structure: len | padding | payload | MAC
    let mut packet = Buffer::new(2048);

    let payload = b"SSH-2.0-OpenSSH_8.0";
    let padding_len = 8u8;
    let mac = [0xAB; 32]; // Simulated HMAC

    // Write packet
    packet
        .put_u32((payload.len() + 1 + padding_len as usize) as u32)
        .unwrap();
    packet.put_byte(padding_len).unwrap();
    packet.put_bytes(payload).unwrap();
    packet.put_bytes(&vec![0; padding_len as usize]).unwrap();
    packet.put_bytes(&mac).unwrap();

    // Read packet back
    packet.set_pos(0).unwrap();
    let pkt_len = packet.get_u32().unwrap();
    let pad_len = packet.get_byte().unwrap();

    assert_eq!(pkt_len, (payload.len() + 1 + padding_len as usize) as u32);
    assert_eq!(pad_len, padding_len);
}

#[test]
fn test_buffer_pool_concurrency() {
    use std::sync::Arc;
    use std::thread;

    let pool = Arc::new(BufferPool::new(PoolConfig {
        buffer_size: 1024,
        max_pool_size: 100,
        min_pool_size: 10,
    }));

    let mut handles = vec![];

    for i in 0..10 {
        let pool_clone = Arc::clone(&pool);
        let handle = thread::spawn(move || {
            for j in 0..100 {
                let mut buf = pool_clone.acquire();
                buf.put_u32(i * 100 + j).unwrap();
                buf.put_bytes(b"thread data").unwrap();
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let stats = pool.stats();
    assert_eq!(stats.total_acquired, 1000);
}

#[test]
fn test_circular_buffer_stress() {
    let mut cbuf = CircularBuffer::new(1024);

    for _ in 0..100 {
        cbuf.write(b"ABCDEFGHIJ").unwrap();
        let mut output = vec![0u8; 5];
        cbuf.read(&mut output).unwrap();
    }

    assert!(cbuf.used() > 0);
    assert_eq!(cbuf.used(), 500);

    let mut final_buf = vec![0u8; cbuf.used()];
    cbuf.read(&mut final_buf).unwrap();
    assert_eq!(cbuf.used(), 0);
}

#[test]
fn test_buffer_resize_preserves_data() {
    let mut buf = Buffer::new(100);
    buf.put_u32(0x12345678).unwrap();
    buf.put_bytes(b"preserved data").unwrap();

    let original_len = buf.len();
    buf.resize(1000).unwrap();

    assert_eq!(buf.capacity(), 1000);
    assert_eq!(buf.len(), original_len);

    buf.set_pos(0).unwrap();
    assert_eq!(buf.get_u32().unwrap(), 0x12345678);
    assert_eq!(buf.get_bytes(14).unwrap(), b"preserved data");
}

#[test]
fn test_large_string_operations() {
    let mut buf = Buffer::new(100_000);

    let large_string = vec![b'X'; 50_000];
    buf.put_string(&large_string).unwrap();

    buf.set_pos(0).unwrap();
    let retrieved = buf.get_string().unwrap();

    assert_eq!(retrieved.len(), 50_000);
    assert_eq!(retrieved, large_string);
}

#[test]
fn test_pool_statistics_accuracy() {
    let pool = BufferPool::new(PoolConfig {
        buffer_size: 512,
        max_pool_size: 20,
        min_pool_size: 5,
    });

    let initial_stats = pool.stats();
    assert_eq!(initial_stats.available, 5);

    let buffers: Vec<_> = (0..10).map(|_| pool.acquire()).collect();
    let mid_stats = pool.stats();
    assert_eq!(mid_stats.total_acquired, 10);

    drop(buffers);
    let final_stats = pool.stats();
    assert_eq!(final_stats.total_returned, 10);
    assert!(final_stats.hit_rate() > 0.0);
}

#[test]
fn test_circular_buffer_read_write_pointers() {
    let mut cbuf = CircularBuffer::new(100);

    cbuf.write(b"First").unwrap();
    cbuf.write(b"Second").unwrap();

    let (p1, _p2) = cbuf.read_ptrs();
    assert_eq!(p1, b"FirstSecond");
    assert_eq!(_p2.len(), 0);

    let mut discard = vec![0u8; 5];
    cbuf.read(&mut discard).unwrap();

    cbuf.write(b"Third").unwrap();

    let (p1, _p2) = cbuf.read_ptrs();
    assert!(!p1.is_empty());
}

#[test]
fn test_buffer_error_handling() {
    let mut buf = Buffer::new(10);

    assert!(buf.set_pos(100).is_err());
    assert!(buf.put_bytes(&vec![0; 20]).is_err());

    buf.put_bytes(&vec![0; 5]).unwrap();
    buf.set_pos(0).unwrap();
    assert!(buf.get_bytes(10).is_err());
}

#[test]
fn test_circular_buffer_peek_doesnt_consume() {
    let mut cbuf = CircularBuffer::new(100);
    cbuf.write(b"Test Data").unwrap();

    let used_before = cbuf.used();

    let mut peek_buf = vec![0u8; 9];
    cbuf.peek(&mut peek_buf).unwrap();

    assert_eq!(cbuf.used(), used_before);
    assert_eq!(&peek_buf, b"Test Data");

    let mut read_buf = vec![0u8; 9];
    cbuf.read(&mut read_buf).unwrap();

    assert_eq!(cbuf.used(), 0);
    assert_eq!(read_buf, peek_buf);
}

#[test]
fn test_buffer_clone_independence() {
    let mut original = Buffer::new(100);
    original.put_u32(42).unwrap();

    let mut cloned = original.clone();
    cloned.set_pos(0).unwrap();
    cloned.put_u32(99).unwrap();

    original.set_pos(0).unwrap();
    assert_eq!(original.get_u32().unwrap(), 42);

    cloned.set_pos(0).unwrap();
    assert_eq!(cloned.get_u32().unwrap(), 99);
}

#[test]
fn test_pool_max_size_enforcement() {
    let pool = BufferPool::new(PoolConfig {
        buffer_size: 256,
        max_pool_size: 5,
        min_pool_size: 2,
    });

    for _ in 0..20 {
        let buf = pool.acquire();
        drop(buf);
    }

    assert!(pool.available() <= 5);
}

#[test]
fn test_buffer_remaining() {
    let mut buf = Buffer::new(100);
    buf.put_bytes(&vec![0; 50]).unwrap();

    buf.set_pos(0).unwrap();
    assert_eq!(buf.remaining(), 50);
    assert!(buf.has_remaining(50));
    assert!(!buf.has_remaining(51));

    buf.incr_pos(30).unwrap();
    assert_eq!(buf.remaining(), 20);
}

#[test]
fn test_u64_operations() {
    let mut buf = Buffer::new(100);
    buf.put_u64(0x123456789ABCDEF0).unwrap();

    buf.set_pos(0).unwrap();
    assert_eq!(buf.get_u64().unwrap(), 0x123456789ABCDEF0);
}

#[test]
fn test_bool_operations() {
    let mut buf = Buffer::new(10);
    buf.put_byte(1).unwrap();
    buf.put_byte(0).unwrap();
    buf.put_byte(255).unwrap();

    buf.set_pos(0).unwrap();
    assert_eq!(buf.get_bool().unwrap(), true);
    assert_eq!(buf.get_bool().unwrap(), false);
    assert_eq!(buf.get_bool().unwrap(), true);
}

#[test]
fn test_circular_buffer_full_cycle() {
    let mut cbuf = CircularBuffer::new(100);

    cbuf.write(&vec![b'A'; 100]).unwrap();
    assert!(cbuf.is_full());
    assert_eq!(cbuf.available(), 0);

    assert!(cbuf.write(b"X").is_err());

    let mut output = vec![0u8; 100];
    cbuf.read(&mut output).unwrap();
    assert!(cbuf.is_empty());
    assert_eq!(cbuf.available(), 100);
}

#[test]
fn test_buffer_write_then_read_pattern() {
    let mut buf = Buffer::new(1024);

    buf.put_u32(0xDEADBEEF).unwrap();
    buf.put_string(b"Hello, Buffer!").unwrap();
    buf.put_u64(0x123456789ABCDEF0).unwrap();

    let written_len = buf.len();

    buf.set_pos(0).unwrap();

    assert_eq!(buf.get_u32().unwrap(), 0xDEADBEEF);
    assert_eq!(buf.get_string().unwrap(), b"Hello, Buffer!");
    assert_eq!(buf.get_u64().unwrap(), 0x123456789ABCDEF0);

    assert_eq!(buf.pos(), written_len);
}

#[test]
fn test_fast_pool_concurrency() {
    use std::sync::Arc;
    use std::thread;

    let pool = Arc::new(FastBufferPool::new(PoolConfig {
        buffer_size: 1024,
        max_pool_size: 100,
        min_pool_size: 10,
    }));

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let pool = Arc::clone(&pool);
            thread::spawn(move || {
                for i in 0..100 {
                    let mut buf = pool.acquire();
                    buf.put_u32(i).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let stats = pool.stats();
    assert_eq!(stats.acquired, 400);
}
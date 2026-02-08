// benches/buffer_bench.rs
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use secbuf::prelude::*;
use std::hint::black_box;

fn bench_buffer_write_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_operations");

    for size in [256, 1024, 4096, 16384].iter() {
        group.bench_with_input(BenchmarkId::new("write_read", size), size, |b, &size| {
            b.iter(|| {
                let mut buf = Buffer::new(size);
                buf.put_u32(black_box(12345)).unwrap();
                buf.put_bytes(black_box(b"test data")).unwrap();
                buf.set_pos(0).unwrap();
                let _ = buf.get_u32().unwrap();
                let _ = buf.get_bytes(9).unwrap();
            });
        });
    }

    group.finish();
}

fn bench_pool_vs_direct(c: &mut Criterion) {
    let mut group = c.benchmark_group("pool_comparison");

    // With pool
    group.bench_function("with_pool", |b| {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 100,
            min_pool_size: 10,
        });

        b.iter(|| {
            let mut buf = pool.acquire();
            buf.put_u32(black_box(42)).unwrap();
            buf.put_bytes(black_box(&[0u8; 512])).unwrap();
        });
    });

    // Direct allocation
    group.bench_function("direct_alloc", |b| {
        b.iter(|| {
            let mut buf = Buffer::new(1024);
            buf.put_u32(black_box(42)).unwrap();
            buf.put_bytes(black_box(&[0u8; 512])).unwrap();
        });
    });

    group.finish();
}

fn bench_fast_pool_vs_standard(c: &mut Criterion) {
    let mut group = c.benchmark_group("fast_vs_standard_pool");

    // Standard pool
    group.bench_function("standard_pool", |b| {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 100,
            min_pool_size: 10,
        });

        b.iter(|| {
            let mut buf = pool.acquire();
            buf.put_u32(black_box(42)).unwrap();
        });
    });

    // Fast pool
    group.bench_function("fast_pool", |b| {
        let pool = FastBufferPool::new(PoolConfig {
            buffer_size: 1024,
            max_pool_size: 100,
            min_pool_size: 10,
        });

        b.iter(|| {
            let mut buf = pool.acquire();
            buf.put_u32(black_box(42)).unwrap();
        });
    });

    group.finish();
}

fn bench_circular_buffer(c: &mut Criterion) {
    let mut group = c.benchmark_group("circular_buffer");

    group.bench_function("write_read", |b| {
        b.iter(|| {
            let mut cbuf = CircularBuffer::new(1024);
            cbuf.write(black_box(b"test data chunk")).unwrap();
            let mut output = vec![0u8; 15];
            cbuf.read(&mut output).unwrap();
        });
    });

    group.bench_function("wraparound", |b| {
        let mut cbuf = CircularBuffer::new(64);
        cbuf.write(&[0u8; 50]).unwrap();
        let mut discard = vec![0u8; 30];
        cbuf.read(&mut discard).unwrap();

        b.iter(|| {
            cbuf.write(black_box(&[1u8; 40])).unwrap();
            let mut output = vec![0u8; 40];
            cbuf.read(&mut output).unwrap();
        });
    });

    group.finish();
}

fn bench_ssh_strings(c: &mut Criterion) {
    let mut group = c.benchmark_group("ssh_strings");

    for len in [32, 128, 512, 2048].iter() {
        let data = vec![b'X'; *len];

        group.bench_with_input(BenchmarkId::new("put_string", len), &data, |b, data| {
            b.iter(|| {
                let mut buf = Buffer::new(data.len() + 100);
                buf.put_string(black_box(data)).unwrap();
            });
        });

        group.bench_with_input(BenchmarkId::new("get_string", len), &data, |b, data| {
            let mut buf = Buffer::new(data.len() + 100);
            buf.put_string(data).unwrap();

            b.iter(|| {
                buf.set_pos(0).unwrap();
                let _ = buf.get_string().unwrap();
            });
        });
    }

    group.finish();
}

fn bench_packet_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("packet_processing");

    // Simulate processing 100 network packets
    group.bench_function("pooled_packets", |b| {
        let pool = BufferPool::new(PoolConfig {
            buffer_size: 1500,
            max_pool_size: 200,
            min_pool_size: 50,
        });

        b.iter(|| {
            for i in 0..100 {
                let mut packet = pool.acquire();
                packet.put_u32(black_box(i)).unwrap();
                packet.put_u32(black_box(1400)).unwrap();
                packet.put_bytes(black_box(&[0x42; 1400])).unwrap();

                packet.set_pos(0).unwrap();
                let _ = packet.get_u32().unwrap();
                let _ = packet.get_u32().unwrap();
            }
        });
    });

    group.bench_function("direct_packets", |b| {
        b.iter(|| {
            for i in 0..100 {
                let mut packet = Buffer::new(1500);
                packet.put_u32(black_box(i)).unwrap();
                packet.put_u32(black_box(1400)).unwrap();
                packet.put_bytes(black_box(&[0x42; 1400])).unwrap();

                packet.set_pos(0).unwrap();
                let _ = packet.get_u32().unwrap();
                let _ = packet.get_u32().unwrap();
            }
        });
    });

    group.finish();
}

fn bench_unchecked_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("checked_vs_unchecked");

    group.bench_function("checked_u32", |b| {
        b.iter(|| {
            let mut buf = Buffer::new(1024);
            for i in 0..100 {
                buf.put_u32(black_box(i)).unwrap();
            }
        });
    });

    group.bench_function("unchecked_u32", |b| {
        b.iter(|| {
            let mut buf = Buffer::new(1024);
            for i in 0..100 {
                unsafe {
                    buf.put_u32_unchecked(black_box(i));
                }
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_buffer_write_read,
    bench_pool_vs_direct,
    bench_fast_pool_vs_standard,
    bench_circular_buffer,
    bench_ssh_strings,
    bench_packet_processing,
    bench_unchecked_operations
);

criterion_main!(benches);

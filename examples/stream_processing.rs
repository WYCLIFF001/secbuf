// examples/stream_processing.rs
//! Demonstrates streaming data processing with circular buffers

use secbuf::prelude::*;

fn main() -> Result<()> {
    println!("=== Stream Processing Example ===\n");

    let mut stream = CircularBuffer::new(1024);

    // Simulate streaming data input
    let chunks: Vec<&[u8]> = vec![
        b"This is the first chunk of streaming data. ",
        b"Here comes the second chunk with more information. ",
        b"And finally, the third chunk to complete the message.",
    ];

    println!("Writing chunks to stream...");
    for (i, chunk) in chunks.iter().enumerate() {
        stream.write(chunk)?;
        println!(
            "Chunk {}: wrote {} bytes (buffer: {}/{})",
            i + 1,
            chunk.len(),
            stream.used(),
            stream.size()
        );
    }

    // Process the stream in smaller pieces
    println!("\nReading stream in 50-byte chunks:");
    let mut total_read = 0;
    let mut chunk_num = 1;

    while !stream.is_empty() {
        let mut output = vec![0u8; 50];
        let read = stream.read(&mut output)?;
        output.truncate(read);

        println!(
            "Chunk {}: {:?}",
            chunk_num,
            String::from_utf8_lossy(&output)
        );
        total_read += read;
        chunk_num += 1;
    }

    println!("\nTotal read: {} bytes", total_read);

    // Demonstrate wraparound
    println!("\n=== Wraparound Demonstration ===\n");

    let mut ring = CircularBuffer::new(64);

    // Fill most of the buffer
    ring.write(b"1234567890123456789012345678901234567890")?;
    println!("Initial write: {} bytes", ring.used());

    // Read some data
    let mut discard = vec![0u8; 20];
    ring.read(&mut discard)?;
    println!(
        "After reading 20 bytes: {} used, {} available",
        ring.used(),
        ring.available()
    );

    // Write more data (will wrap around)
    ring.write(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ")?;
    println!("After wraparound write: {} used", ring.used());

    // Read all remaining data
    let mut final_data = vec![0u8; ring.used()];
    ring.read(&mut final_data)?;
    println!("Final data: {:?}", String::from_utf8_lossy(&final_data));

    // Demonstrate peek (non-destructive read)
    println!("\n=== Peek Demonstration ===\n");

    let mut peekbuf = CircularBuffer::new(128);
    peekbuf.write(b"Sensitive data that we want to inspect")?;

    let mut peek_output = vec![0u8; 20];
    peekbuf.peek(&mut peek_output)?;
    println!("Peeked: {:?}", String::from_utf8_lossy(&peek_output));
    println!("Buffer still has: {} bytes", peekbuf.used());

    // Now actually read it
    let mut read_output = vec![0u8; peekbuf.used()];
    peekbuf.read(&mut read_output)?;
    println!("Read: {:?}", String::from_utf8_lossy(&read_output));
    println!("Buffer now has: {} bytes", peekbuf.used());

    // Demonstrate zero-copy read pointers
    println!("\n=== Zero-Copy Read Pointers ===\n");

    let mut zcbuf = CircularBuffer::new(100);
    zcbuf.write(b"Zero-copy read example")?;

    let (ptr1, ptr2) = zcbuf.read_ptrs();
    println!("Primary slice: {:?}", String::from_utf8_lossy(ptr1));
    println!("Secondary slice: {:?}", String::from_utf8_lossy(ptr2));
    println!("Data is still in buffer: {} bytes", zcbuf.used());

    Ok(())
}

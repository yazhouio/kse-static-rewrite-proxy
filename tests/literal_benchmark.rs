use std::hint::black_box;
use std::time::Instant;

use kse_static_rewrite_proxy::literal::StreamingLiteralRewriter;

#[test]
#[ignore = "run with: cargo test --release --test literal_benchmark -- --ignored --nocapture"]
fn dense_literal_rewrite_throughput() {
    const SOURCE: &[u8] = b"/extensions-static/";
    const REPLACEMENT: &[u8] = b"/regions/region:shenzhen/extensions-static/";
    const ITERATIONS: usize = 20;
    const INPUT_REPETITIONS: usize = 300_000;

    let input = b"prefix:/extensions-static/main.js;".repeat(INPUT_REPETITIONS);
    let started = Instant::now();
    let mut output_bytes = 0;

    for _ in 0..ITERATIONS {
        let mut rewriter = StreamingLiteralRewriter::new(SOURCE, REPLACEMENT, usize::MAX).unwrap();
        for chunk in input.chunks(64 * 1024) {
            output_bytes += black_box(rewriter.push(black_box(chunk)).unwrap()).len();
        }
        output_bytes += black_box(rewriter.finish().unwrap()).len();
    }

    let elapsed = started.elapsed();
    let input_bytes = input.len() * ITERATIONS;
    let expected_output_bytes = b"prefix:/regions/region:shenzhen/extensions-static/main.js;".len()
        * INPUT_REPETITIONS
        * ITERATIONS;
    assert_eq!(output_bytes, expected_output_bytes);
    eprintln!(
        "{:.1} MiB/s ({} -> {} bytes in {:.3}s)",
        input_bytes as f64 / elapsed.as_secs_f64() / (1024.0 * 1024.0),
        input_bytes,
        output_bytes,
        elapsed.as_secs_f64()
    );
}

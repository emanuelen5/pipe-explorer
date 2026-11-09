//! `StageOutput`: ingesting streamed output and the copies made of it.

mod common;

use std::hint::black_box;

use common::{Flavor, split_streams, stage_output, text};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use pipe_explorer::executor::{CombinedLine, StageOutput};

/// Lines of output per benchmark.
const LINES: usize = 100_000;
/// Lines per `StageUpdate` message.  The executor batches whatever arrived
/// during its 100 ms throttle window, so this varies in practice.
const CHUNK_LINES: usize = 1_000;
/// Every n-th line goes to stderr.
const STDERR_EVERY: usize = 10;
/// Samples per benchmark; fewer than criterion's default of 100 because a
/// single iteration takes up to ~150 ms.
const SAMPLES: usize = 20;

type Chunk = (Vec<u8>, Vec<u8>, Vec<CombinedLine>);

fn chunks(input: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = input.split_inclusive('\n').collect();
    lines
        .chunks(CHUNK_LINES)
        .map(|chunk| split_streams(&chunk.concat(), STDERR_EVERY))
        .collect()
}

/// Appending streamed chunks, as the UI does for every `StageUpdate`.
fn append_stream(c: &mut Criterion) {
    let mut group = c.benchmark_group("output/append_stream");
    group.sample_size(SAMPLES);
    for flavor in Flavor::ALL {
        let input = text(flavor, LINES);
        let chunks = chunks(&input);
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.iter_batched(
                || chunks.clone(),
                |chunks| {
                    let mut out = StageOutput::empty();
                    for (stdout, stderr, combined) in chunks {
                        out.append_data(&stdout, &stderr, combined);
                    }
                    out
                },
                BatchSize::LargeInput,
            )
        });
    }
    group.finish();
}

/// Building the final output when a stage finishes.
fn new(c: &mut Criterion) {
    let mut group = c.benchmark_group("output/new");
    group.sample_size(SAMPLES);
    for flavor in Flavor::ALL {
        let input = text(flavor, LINES);
        let streams = split_streams(&input, STDERR_EVERY);
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.iter_batched(
                || streams.clone(),
                |(stdout, stderr, combined)| StageOutput::new(stdout, stderr, Some(0), combined),
                BatchSize::LargeInput,
            )
        });
    }
    group.finish();
}

/// Copying a finished output; happens on every cache store and cache hit.
fn clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("output/clone");
    group.sample_size(SAMPLES);
    for flavor in Flavor::ALL {
        let input = text(flavor, LINES);
        let out = stage_output(&input, STDERR_EVERY);
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.iter(|| black_box(&out).clone())
        });
    }
    group.finish();
}

criterion_group!(benches, append_stream, new, clone);
criterion_main!(benches);

//! Output search: matching a query against a whole stage's output.
//!
//! This runs when a search is confirmed and again every time a stage
//! finishes while a search is active.

mod common;

use std::hint::black_box;

use common::{Flavor, text};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use pipe_explorer::search::SearchState;

const LINES: usize = 100_000;

fn compute(c: &mut Criterion) {
    let mut group = c.benchmark_group("search/compute");
    let input = text(Flavor::Plain, LINES);
    group.throughput(Throughput::Bytes(input.len() as u64));

    let queries = [
        // A match on every line.
        ("dense_literal", "file"),
        // A single match.
        ("rare_literal", "file_00099999"),
        ("regex", r"module_0\d7/file_\d+5\.txt"),
        // Vim-style `\c`: case-insensitive.
        ("ignore_case", r"\cFILE"),
    ];
    for (label, query) in queries {
        let mut search = SearchState {
            query: query.to_string(),
            ..SearchState::default()
        };
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                search.compute(black_box(&input));
                search.matches.len()
            })
        });
    }
    group.finish();
}

criterion_group!(benches, compute);
criterion_main!(benches);

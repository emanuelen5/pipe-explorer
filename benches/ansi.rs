//! ANSI handling: indexing, per-frame parsing of the visible window, and
//! stripping escape codes before searching or piping to the next stage.

mod common;

use std::collections::HashMap;
use std::hint::black_box;

use common::{Flavor, TERM_HEIGHT, text};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use pipe_explorer::ansi::{
    AnsiLineIndex, ansi_text_to_visible_lines, strip_ansi_sgr, strip_ansi_sgr_bytes,
};

/// Lines of input for the throughput benchmarks.
const LINES: usize = 100_000;

/// Building the line index; done once for every chunk of streamed output.
fn line_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("ansi/line_index");
    for flavor in Flavor::ALL {
        let input = text(flavor, LINES);
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(flavor.name()),
            &input,
            |b, input| {
                b.iter(|| {
                    let mut index = AnsiLineIndex::new();
                    index.extend(black_box(input));
                    index
                })
            },
        );
    }
    group.finish();
}

/// Parsing the lines on screen into styled spans; done on every redraw.
fn visible_window(c: &mut Criterion) {
    let mut group = c.benchmark_group("ansi/visible_window");
    let rows = TERM_HEIGHT as usize;
    let start = LINES / 2;
    for flavor in Flavor::ALL {
        let input = text(flavor, LINES);
        let mut index = AnsiLineIndex::new();
        index.extend(&input);

        let no_highlights = HashMap::new();
        // One search match near the start of every visible line, with the
        // current match in the middle of the screen.
        let highlights: HashMap<usize, Vec<(usize, usize, bool)>> = (start..start + rows)
            .map(|line| (line, vec![(2, 8, line == start + rows / 2)]))
            .collect();

        for (label, hl) in [("no_search", &no_highlights), ("search", &highlights)] {
            group.bench_function(BenchmarkId::new(flavor.name(), label), |b| {
                b.iter(|| {
                    ansi_text_to_visible_lines(black_box(&input), start, rows, hl, Some(&index))
                })
            });
        }
    }
    group.finish();
}

/// Removing SGR sequences: the `&str` variant runs before every search, the
/// byte variant on everything that is piped into the next stage.
fn strip_sgr(c: &mut Criterion) {
    let mut group = c.benchmark_group("ansi/strip_sgr");
    for flavor in [Flavor::Plain, Flavor::Ansi] {
        let input = text(flavor, LINES);
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("str", flavor.name()),
            &input,
            |b, input| b.iter(|| strip_ansi_sgr(black_box(input))),
        );
        group.bench_with_input(
            BenchmarkId::new("bytes", flavor.name()),
            &input,
            |b, input| b.iter(|| strip_ansi_sgr_bytes(black_box(input.as_bytes()))),
        );
    }
    group.finish();
}

criterion_group!(benches, line_index, visible_window, strip_sgr);
criterion_main!(benches);

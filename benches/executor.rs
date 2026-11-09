//! Running pipelines end to end: spawning the stages, relaying output
//! between them, and replaying results from the cache.
//!
//! These spawn real processes, so expect more noise than in the other
//! benchmarks.  Unix only.

#[cfg(unix)]
mod unix {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::Duration;

    use criterion::{BenchmarkId, Criterion, Throughput};
    use pipe_explorer::executor::{ExecutorCache, OutputMode, StreamMsg, run_pipeline_streaming};
    use pipe_explorer::pipeline::StageOptions;

    /// Lines produced by the first stage.
    const LINES: u64 = 100_000;

    /// Run `commands` to completion and return the number of UI messages sent.
    /// Panics if the pipeline reports an error, so a broken setup can't pass
    /// as a fast result.
    fn run(cache: &mut ExecutorCache, commands: &[String], force: bool) -> usize {
        let stages = commands.len();
        // Use plain `sh` rather than the user's shell so results are comparable.
        let options = vec![
            StageOptions {
                interactive: false,
                shell: Some("sh".to_string()),
            };
            stages
        ];
        let modes = vec![OutputMode::Stdout; stages];
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        run_pipeline_streaming(
            cache,
            commands,
            stages - 1,
            force,
            &modes,
            &options,
            &cancel,
            &tx,
        );
        drop(tx);

        let mut messages = 0;
        for msg in rx {
            if let StreamMsg::AllDone { error: Some(e) } = &msg {
                panic!("pipeline {commands:?} failed: {e}");
            }
            messages += 1;
        }
        messages
    }

    fn commands(cmds: &[&str]) -> Vec<String> {
        cmds.iter().map(|s| s.to_string()).collect()
    }

    pub fn pipeline(c: &mut Criterion) {
        let mut group = c.benchmark_group("executor/pipeline");
        group
            .sample_size(10)
            .measurement_time(Duration::from_secs(10));

        // Fixed cost of a run: spawning a stage and noticing that it exited.
        let startup = commands(&["true"]);
        group.bench_function(BenchmarkId::from_parameter("startup"), |b| {
            let mut cache = ExecutorCache::new();
            b.iter(|| run(&mut cache, &startup, true))
        });

        group.throughput(Throughput::Elements(LINES));

        // Collecting the output of a single stage.
        let single = commands(&[&format!("seq {LINES}")]);
        group.bench_function(BenchmarkId::from_parameter("single"), |b| {
            let mut cache = ExecutorCache::new();
            b.iter(|| run(&mut cache, &single, true))
        });

        // Relaying output through two more stages.
        let relay = commands(&[&format!("seq {LINES}"), "cat", "cat"]);
        group.bench_function(BenchmarkId::from_parameter("relay"), |b| {
            let mut cache = ExecutorCache::new();
            b.iter(|| run(&mut cache, &relay, true))
        });

        // The same pipeline served entirely from the cache, e.g. after an
        // edit that is undone.
        group.bench_function(BenchmarkId::from_parameter("cached"), |b| {
            let mut cache = ExecutorCache::new();
            run(&mut cache, &relay, true);
            b.iter(|| run(&mut cache, &relay, false))
        });

        group.finish();
    }
}

#[cfg(unix)]
criterion::criterion_group!(benches, unix::pipeline);
#[cfg(unix)]
criterion::criterion_main!(benches);

#[cfg(not(unix))]
fn main() {}

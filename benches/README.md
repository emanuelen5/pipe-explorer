# Benchmarks

Benchmarks use [criterion](https://docs.rs/criterion). Each file covers one
part of the application. They run on synthetic output generated in
`common/mod.rs`, so results are comparable between runs.

| Target     | What it measures                                                                         |
| ---------- | ---------------------------------------------------------------------------------------- |
| `ansi`     | Building the line index, parsing the visible lines into styled spans, stripping escapes |
| `output`   | Appending streamed chunks to a `StageOutput`, building one, cloning one                  |
| `search`   | Matching a query against a stage's whole output                                          |
| `render`   | Drawing a frame, and key presses (editor cursor, scrolling, `n`/`N`) plus the redraw     |
| `executor` | Running pipelines end to end with real processes, and replaying them from the cache     |

`render` runs every case at 10k and 1M lines of output. Only one screenful
is ever drawn, so both sizes should take about the same time. If the 1M
case is clearly slower, something scans the whole output on every frame
(see [issue #48](https://github.com/emanuelen5/pipe-explorer/issues/48)).

`executor` spawns processes through `sh`, so it is noisier than the others
and only runs on Unix.

## Running

```sh
cargo bench                          # everything (takes a few minutes)
cargo bench --bench render           # one target
cargo bench --bench render -- combined   # only benchmarks whose name matches
```

HTML reports end up in `target/criterion/report/index.html`.

## Comparing a change

Save a baseline before the change, then compare against it:

```sh
git switch main && cargo bench -- --save-baseline main
git switch my-branch && cargo bench -- --baseline main
```

Criterion then reports the change for every benchmark and flags the
statistically significant ones.

## In CI

The Benchmarks workflow (`.github/workflows/benchmarks.yml`) has two jobs:

- **Build benchmarks** compiles them on every push and pull request, so they
  keep up with changes to the code.
- **Compare with base branch** runs on pull requests. It benchmarks the
  base branch and then the pull request on the same runner, and fails if a
  benchmark got more than 15% slower. The job summary lists the benchmarks
  that got slower or faster. `executor` is left out because it is too noisy
  on shared runners.

Both runs use the pull request's benchmark code, so new and changed
benchmarks are compared as well. A benchmark that uses code the pull
request adds can't be built against the base branch. The summary lists
those as not compared.

Shared runners are noisy enough that the comparison can occasionally fail
without a real regression, and smaller regressions go unnoticed. Rerun the
job, or confirm locally with the baseline commands above.

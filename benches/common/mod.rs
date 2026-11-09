//! Deterministic synthetic output shared by the benchmarks.
//!
//! Every generator is a pure function of its arguments so results stay
//! comparable between runs and machines.

#![allow(dead_code)] // Each bench target only uses some of these.

use pipe_explorer::executor::{CombinedLine, StageOutput};

/// Line counts used for size-parameterised benchmarks.  Work that should be
/// independent of the amount of output (e.g. drawing a frame) is measured at
/// both sizes; the two results should be about the same.
pub const SIZES: [usize; 2] = [10_000, 1_000_000];

/// Size of the terminal used by the rendering benchmarks.
pub const TERM_WIDTH: u16 = 200;
pub const TERM_HEIGHT: u16 = 50;

/// The kind of text a stage produces.
#[derive(Clone, Copy, Debug)]
pub enum Flavor {
    /// `find`-style paths, ~45 bytes per line, no escape codes.
    Plain,
    /// `grep --color`-style lines with a few SGR escape sequences each.
    Ansi,
    /// ~300-byte log lines that wrap several times on screen.
    Long,
}

impl Flavor {
    pub const ALL: [Flavor; 3] = [Flavor::Plain, Flavor::Ansi, Flavor::Long];

    pub fn name(self) -> &'static str {
        match self {
            Flavor::Plain => "plain",
            Flavor::Ansi => "ansi",
            Flavor::Long => "long",
        }
    }
}

/// Generate `n` newline-terminated lines of the given flavor.
pub fn text(flavor: Flavor, n: usize) -> String {
    let mut out = String::with_capacity(n * 64);
    for i in 0..n {
        match flavor {
            Flavor::Plain => {
                out.push_str(&format!("./src/module_{:03}/file_{i:08}.txt\n", i % 997));
            }
            Flavor::Ansi => {
                out.push_str(&format!(
                    "\x1b[35m./src/module_{:03}/file_{i:08}.txt\x1b[0m:\x1b[32m{}\x1b[0m: \
                     let \x1b[1;31mvalue\x1b[0m = compute({i});\n",
                    i % 997,
                    i % 4096,
                ));
            }
            Flavor::Long => {
                out.push_str(&format!(
                    "2026-11-09T12:{:02}:{:02}.{:06}Z INFO request_id={i:016x} \
                     method=GET path=/api/v1/projects/{}/pipelines/{}/stages status=200 \
                     duration_ms={} user_agent=\"Mozilla/5.0 (X11; Linux x86_64) \
                     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36\" \
                     message=\"served cached stage output for pipeline run\"\n",
                    (i / 60) % 60,
                    i % 60,
                    i % 1_000_000,
                    i % 113,
                    i % 7,
                    i % 250,
                ));
            }
        }
    }
    out
}

/// Build a finished [`StageOutput`] from `text`, the way the executor does:
/// one [`CombinedLine`] per line, with every `stderr_every`-th line written
/// to stderr instead of stdout (`0` means everything goes to stdout).
pub fn stage_output(text: &str, stderr_every: usize) -> StageOutput {
    let (stdout, stderr, combined) = split_streams(text, stderr_every);
    StageOutput::new(stdout, stderr, Some(0), combined)
}

/// Split `text` into stdout bytes, stderr bytes and the interleaved
/// per-line record, as the executor's collector thread does.
pub fn split_streams(text: &str, stderr_every: usize) -> (Vec<u8>, Vec<u8>, Vec<CombinedLine>) {
    let mut stdout = Vec::with_capacity(text.len());
    let mut stderr = Vec::new();
    let mut combined = Vec::new();
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let is_stderr = stderr_every != 0 && i % stderr_every == stderr_every - 1;
        if is_stderr {
            stderr.extend_from_slice(line.as_bytes());
        } else {
            stdout.extend_from_slice(line.as_bytes());
        }
        combined.push(CombinedLine {
            is_stderr,
            content: line.as_bytes().to_vec(),
        });
    }
    (stdout, stderr, combined)
}

/// Human-readable label for a line count, e.g. `10k` or `1M`.
pub fn size_label(n: usize) -> String {
    if n >= 1_000_000 && n.is_multiple_of(1_000_000) {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 && n.is_multiple_of(1_000) {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

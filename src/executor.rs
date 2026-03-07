use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::ansi::{AnsiLineIndex, strip_ansi_sgr_bytes};

/// The display / pipe mode for stage output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Stdout,
    Stderr,
    Combined,
}

/// A single chunk captured from either stdout or stderr of a child process,
/// representing one line of output (including the trailing `\n`, if any).
#[derive(Debug, Clone)]
pub struct CombinedLine {
    /// `true` if this chunk came from stderr; `false` for stdout.
    pub is_stderr: bool,
    /// Raw bytes of the line (may include a trailing newline).
    pub content: Vec<u8>,
}

/// The result of executing a single pipeline stage.
#[derive(Debug, Clone)]
pub struct StageOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    /// Lines from stdout and stderr interleaved in the order they were received.
    pub combined: Vec<CombinedLine>,
    /// Cached UTF-8 text for stdout (kept in sync with `stdout` bytes).
    stdout_text: String,
    /// Cached UTF-8 text for stderr (kept in sync with `stderr` bytes).
    stderr_text: String,
    /// Running newline counts — updated incrementally by `append_data()`.
    stdout_newlines: usize,
    stderr_newlines: usize,
    combined_newlines: usize,
    /// Pre-built line indexes for O(1) scroll seeking.
    stdout_line_index: AnsiLineIndex,
    stderr_line_index: AnsiLineIndex,
}

impl StageOutput {
    /// Create a new stage output, eagerly computing all caches.
    pub fn new(
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        exit_code: Option<i32>,
        combined: Vec<CombinedLine>,
    ) -> Self {
        let stdout_text = String::from_utf8_lossy(&stdout).into_owned();
        let stderr_text = String::from_utf8_lossy(&stderr).into_owned();
        let stdout_newlines = bytecount::count(&stdout, b'\n');
        let stderr_newlines = bytecount::count(&stderr, b'\n');
        let combined_newlines = combined
            .iter()
            .flat_map(|l| l.content.iter())
            .filter(|&&b| b == b'\n')
            .count();
        let mut stdout_line_index = AnsiLineIndex::new();
        stdout_line_index.extend(&stdout_text);
        let mut stderr_line_index = AnsiLineIndex::new();
        stderr_line_index.extend(&stderr_text);
        Self {
            stdout,
            stderr,
            exit_code,
            combined,
            stdout_text,
            stderr_text,
            stdout_newlines,
            stderr_newlines,
            combined_newlines,
            stdout_line_index,
            stderr_line_index,
        }
    }

    /// Create an empty stage output (used as a placeholder during streaming).
    pub fn empty() -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: None,
            combined: Vec::new(),
            stdout_text: String::new(),
            stderr_text: String::new(),
            stdout_newlines: 0,
            stderr_newlines: 0,
            combined_newlines: 0,
            stdout_line_index: AnsiLineIndex::new(),
            stderr_line_index: AnsiLineIndex::new(),
        }
    }

    /// Append new data from a streaming update.
    ///
    /// Line counts are updated incrementally (only the new bytes are scanned),
    /// avoiding the O(total) cost of rescanning the full buffer every update.
    /// UTF-8 text caches are extended in-place when the new bytes are valid
    /// UTF-8 (the common case), falling back to a full recompute only when a
    /// multi-byte sequence is split across chunk boundaries.
    pub fn append_data(
        &mut self,
        new_stdout: &[u8],
        new_stderr: &[u8],
        new_combined: Vec<CombinedLine>,
    ) {
        // Incremental newline counts — only scan new bytes.
        self.stdout_newlines += bytecount::count(new_stdout, b'\n');
        self.stderr_newlines += bytecount::count(new_stderr, b'\n');
        self.combined_newlines += new_combined
            .iter()
            .flat_map(|l| l.content.iter())
            .filter(|&&b| b == b'\n')
            .count();

        // Append raw bytes.
        self.stdout.extend_from_slice(new_stdout);
        self.stderr.extend_from_slice(new_stderr);
        self.combined.extend(new_combined);

        // Incremental UTF-8 text caches.
        append_text_incremental(&mut self.stdout_text, new_stdout, &self.stdout);
        append_text_incremental(&mut self.stderr_text, new_stderr, &self.stderr);

        // Extend line indexes (only scans the newly appended portion).
        self.stdout_line_index.extend(&self.stdout_text);
        self.stderr_line_index.extend(&self.stderr_text);
    }

    /// Get the pre-built line index for the given output mode.
    /// Returns `None` for Combined mode (no persistent index).
    pub fn line_index(&self, mode: OutputMode) -> Option<&AnsiLineIndex> {
        match mode {
            OutputMode::Stdout => Some(&self.stdout_line_index),
            OutputMode::Stderr => Some(&self.stderr_line_index),
            OutputMode::Combined => None,
        }
    }

    /// Cached UTF-8 text for stdout (zero-cost borrow).
    pub fn stdout_text(&self) -> &str {
        &self.stdout_text
    }

    /// Cached UTF-8 text for stderr (zero-cost borrow).
    pub fn stderr_text(&self) -> &str {
        &self.stderr_text
    }

    #[cfg(test)]
    pub fn stdout_str(&self) -> String {
        self.stdout_text.clone()
    }

    #[cfg(test)]
    pub fn stderr_str(&self) -> String {
        self.stderr_text.clone()
    }

    /// Number of lines in stdout (`lines().count()` semantics, O(1)).
    pub fn stdout_line_count(&self) -> usize {
        if self.stdout.is_empty() {
            return 0;
        }
        self.stdout_newlines
            + if self.stdout.last() != Some(&b'\n') {
                1
            } else {
                0
            }
    }

    /// Number of lines in stderr (`lines().count()` semantics, O(1)).
    pub fn stderr_line_count(&self) -> usize {
        if self.stderr.is_empty() {
            return 0;
        }
        self.stderr_newlines
            + if self.stderr.last() != Some(&b'\n') {
                1
            } else {
                0
            }
    }

    /// Number of display lines for the given output mode (O(1)).
    ///
    /// This matches the number of `Line` items produced by the ANSI parser:
    /// each `\n` ends a line, and a non-empty trailing segment without `\n`
    /// counts as one more line — but a trailing `\n` also produces an extra
    /// empty line.
    pub fn display_line_count(&self, mode: OutputMode) -> usize {
        match mode {
            OutputMode::Stdout => {
                if self.stdout.is_empty() {
                    0
                } else {
                    self.stdout_newlines + 1
                }
            }
            OutputMode::Stderr => {
                if self.stderr.is_empty() {
                    0
                } else {
                    self.stderr_newlines + 1
                }
            }
            OutputMode::Combined => {
                if self.combined.is_empty() {
                    0
                } else {
                    self.combined_newlines + 1
                }
            }
        }
    }
}

/// Try to append `new_bytes` to `text` as UTF-8.  If the new chunk is valid
/// UTF-8, it is pushed directly onto the existing `String` (zero-copy for the
/// existing prefix).  If it is not (e.g. a multi-byte char was split across
/// streaming chunks), the entire `full_buf` is re-decoded with lossy fallback.
fn append_text_incremental(text: &mut String, new_bytes: &[u8], full_buf: &[u8]) {
    if new_bytes.is_empty() {
        return;
    }
    match std::str::from_utf8(new_bytes) {
        Ok(s) => text.push_str(s),
        Err(_) => {
            // Rare: a multi-byte sequence straddles the chunk boundary.
            *text = String::from_utf8_lossy(full_buf).into_owned();
        }
    }
}

/// Cache key: (command_string, sha256 of stdin bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    command: String,
    stdin_hash: Vec<u8>,
}

/// Caches stage outputs keyed by (command, stdin_hash).
#[derive(Debug, Default)]
pub struct ExecutorCache {
    cache: HashMap<CacheKey, StageOutput>,
}

impl ExecutorCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run a shell command with the given stdin bytes.
    /// Returns a cached result if available; otherwise executes and caches.
    #[cfg(test)]
    pub fn run(&mut self, command: &str, stdin: &[u8], force: bool) -> anyhow::Result<StageOutput> {
        let key = CacheKey {
            command: command.to_string(),
            stdin_hash: sha256(stdin),
        };

        if !force {
            if let Some(cached) = self.cache.get(&key) {
                return Ok(cached.clone());
            }
        }

        let output = run_shell_command(command, stdin)?;
        self.cache.insert(key, output.clone());
        Ok(output)
    }

    /// Look up a cached result without executing.
    pub fn lookup(&self, command: &str, stdin: &[u8]) -> Option<&StageOutput> {
        let key = CacheKey {
            command: command.to_string(),
            stdin_hash: sha256(stdin),
        };
        self.cache.get(&key)
    }

    /// Store a result in the cache.
    pub fn store(&mut self, command: &str, stdin: &[u8], output: StageOutput) {
        let key = CacheKey {
            command: command.to_string(),
            stdin_hash: sha256(stdin),
        };
        self.cache.insert(key, output);
    }
}

fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// Read all lines from `reader` line-by-line and send each as a `CombinedLine` to `tx`.
///
/// Each line includes its trailing `\n` when present.  A final partial line (with no
/// trailing `\n`) is sent as-is.  Stops when EOF is reached or the receiver is dropped.
fn read_to_channel(mut reader: impl Read, is_stderr: bool, tx: std_mpsc::Sender<CombinedLine>) {
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=pos).collect();
                    if tx
                        .send(CombinedLine {
                            is_stderr,
                            content: line,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Err(_) => break,
        }
    }
    // Send any remaining bytes (last line without trailing \n).
    if !pending.is_empty() {
        let _ = tx.send(CombinedLine {
            is_stderr,
            content: pending,
        });
    }
}

#[cfg(test)]
fn run_shell_command(command: &str, stdin_bytes: &[u8]) -> anyhow::Result<StageOutput> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        // Hint and encourage color output so ANSI can be rendered in the TUI.
        // Downstream stages still receive de-ANSI'd stdin in execute_pipeline_stages.
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("CLICOLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .env("FORCE_COLOR", "3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Write stdin on a separate thread to avoid deadlock: if the child's
    // stdout/stderr pipe buffers fill up while we're still writing stdin,
    // both sides would block forever.  By writing in a background thread,
    // the stdout/stderr reader threads can drain concurrently.
    let stdin_handle = child.stdin.take().unwrap();
    let stdin_data = stdin_bytes.to_vec();
    let writer = std::thread::spawn(move || {
        let mut w = stdin_handle;
        let _ = w.write_all(&stdin_data);
        // dropping `w` closes the pipe, signalling EOF
    });

    // Capture stdout and stderr concurrently so we can record their interleaving order.
    let (tx, rx) = std_mpsc::channel::<CombinedLine>();

    let tx_out = tx.clone();
    let stdout_pipe = child.stdout.take().unwrap();
    let stdout_thread = std::thread::spawn(move || {
        read_to_channel(stdout_pipe, false, tx_out);
    });

    // `tx` is consumed (transferred) here to become the stderr sender; `tx_out` (cloned
    // above) remains the stdout sender.  Dropping both senders closes the channel.
    let tx_err = tx;
    let stderr_pipe = child.stderr.take().unwrap();
    let stderr_thread = std::thread::spawn(move || {
        read_to_channel(stderr_pipe, true, tx_err);
    });

    let status = child.wait()?;
    let _ = writer.join();
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    // Collect lines in the order they arrived (temporal interleaving).
    let combined: Vec<CombinedLine> = rx.try_iter().collect();

    let stdout: Vec<u8> = combined
        .iter()
        .filter(|l| !l.is_stderr)
        .flat_map(|l| l.content.iter().copied())
        .collect();
    let stderr: Vec<u8> = combined
        .iter()
        .filter(|l| l.is_stderr)
        .flat_map(|l| l.content.iter().copied())
        .collect();

    Ok(StageOutput::new(stdout, stderr, status.code(), combined))
}

/// Execute all stages of the pipeline up to and including `up_to_stage`.
/// Returns the outputs for each stage.
///
/// `output_modes[i]` controls which output of stage `i` is piped as stdin to
/// stage `i+1`.
#[cfg(test)]
pub fn execute_pipeline_stages(
    cache: &mut ExecutorCache,
    commands: &[String],
    up_to: usize,
    force: bool,
    output_modes: &[OutputMode],
) -> anyhow::Result<Vec<StageOutput>> {
    let mut outputs: Vec<StageOutput> = Vec::new();
    let mut stdin: Vec<u8> = Vec::new();

    for (i, cmd) in commands.iter().take(up_to + 1).enumerate() {
        let out = cache.run(cmd, &stdin, force)?;
        let mode = output_modes.get(i).copied().unwrap_or(OutputMode::Stdout);
        stdin = match mode {
            OutputMode::Stdout => strip_ansi_sgr_bytes(&out.stdout),
            OutputMode::Stderr => strip_ansi_sgr_bytes(&out.stderr),
            OutputMode::Combined => {
                let combined_bytes: Vec<u8> = out
                    .combined
                    .iter()
                    .flat_map(|l| l.content.iter().copied())
                    .collect();
                strip_ansi_sgr_bytes(&combined_bytes)
            }
        };
        outputs.push(out);
    }

    Ok(outputs)
}

// ---------------------------------------------------------------------------
// Streaming pipeline execution
// ---------------------------------------------------------------------------

/// Messages sent from the streaming executor to the UI.
#[derive(Debug)]
pub enum StreamMsg {
    /// Incremental update for a stage's output buffers.
    StageUpdate {
        stage_idx: usize,
        new_stdout: Vec<u8>,
        new_stderr: Vec<u8>,
        new_combined: Vec<CombinedLine>,
    },
    /// A stage's process has exited.
    StageDone {
        stage_idx: usize,
        exit_code: Option<i32>,
    },
    /// The entire pipeline execution has finished.
    AllDone { error: Option<String> },
}

/// The minimum interval between UI update messages for each stage.
const UI_THROTTLE: Duration = Duration::from_millis(100);

/// Spawn a child process for the given shell command.
fn spawn_shell(command: &str) -> anyhow::Result<std::process::Child> {
    Ok(Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("CLICOLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .env("FORCE_COLOR", "3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?)
}

/// Extract the output stream to relay to the next stage (with ANSI stripping).
fn extract_relay_bytes(output: &StageOutput, mode: OutputMode) -> Vec<u8> {
    match mode {
        OutputMode::Stdout => strip_ansi_sgr_bytes(&output.stdout),
        OutputMode::Stderr => strip_ansi_sgr_bytes(&output.stderr),
        OutputMode::Combined => {
            let combined_bytes: Vec<u8> = output
                .combined
                .iter()
                .flat_map(|l| l.content.iter().copied())
                .collect();
            strip_ansi_sgr_bytes(&combined_bytes)
        }
    }
}

/// Run the pipeline with true concurrent streaming between stages.
///
/// Stages are spawned as concurrent OS processes. Data flows from stage N's
/// selected output stream (after ANSI stripping) to stage N+1's stdin in real
/// time. UI update messages are throttled to at most one per [`UI_THROTTLE`]
/// interval per stage.
///
/// `cancel` can be set to `true` to abort execution — all child processes will
/// be killed and threads will exit.
///
/// Cache behaviour: before spawning processes, we check the cache sequentially
/// (since each key depends on the previous stage's full output hash). Cached
/// stages emit immediate `StageUpdate` + `StageDone`. From the first cache
/// miss onward, all remaining stages stream concurrently.
pub fn run_pipeline_streaming(
    cache: &mut ExecutorCache,
    commands: &[String],
    up_to: usize,
    force: bool,
    output_modes: &[OutputMode],
    cancel: &Arc<AtomicBool>,
    ui_tx: &std_mpsc::Sender<StreamMsg>,
) {
    let count = commands.len().min(up_to + 1);
    if count == 0 {
        let _ = ui_tx.send(StreamMsg::AllDone { error: None });
        return;
    }

    // ------------------------------------------------------------------
    // Phase 1: serve as many stages as possible from cache
    // ------------------------------------------------------------------
    let mut initial_stdin: Vec<u8> = Vec::new();
    let mut miss_idx: Option<usize> = None;

    if !force {
        for i in 0..count {
            if cancel.load(Ordering::Relaxed) {
                let _ = ui_tx.send(StreamMsg::AllDone {
                    error: Some("cancelled".to_string()),
                });
                return;
            }
            if let Some(hit) = cache.lookup(&commands[i], &initial_stdin) {
                let hit = hit.clone();
                let _ = ui_tx.send(StreamMsg::StageUpdate {
                    stage_idx: i,
                    new_stdout: hit.stdout.clone(),
                    new_stderr: hit.stderr.clone(),
                    new_combined: hit.combined.clone(),
                });
                let _ = ui_tx.send(StreamMsg::StageDone {
                    stage_idx: i,
                    exit_code: hit.exit_code,
                });
                let mode = output_modes.get(i).copied().unwrap_or(OutputMode::Stdout);
                initial_stdin = extract_relay_bytes(&hit, mode);
            } else {
                miss_idx = Some(i);
                break;
            }
        }
    } else {
        miss_idx = Some(0);
    }

    let miss_idx = match miss_idx {
        Some(idx) => idx,
        None => {
            let _ = ui_tx.send(StreamMsg::AllDone { error: None });
            return;
        }
    };

    // ------------------------------------------------------------------
    // Phase 2: stream remaining stages concurrently
    //
    // Two-pass setup:
    //  1. Pre-create all inter-stage stdin channels.
    //  2. Spawn child processes + reader/writer/collector threads.
    //  3. Feed initial stdin, drop extra senders, wait for completion.
    // ------------------------------------------------------------------
    let stream_count = count - miss_idx;

    // Pre-create stdin data channels for each streaming stage.
    let mut stdin_txs: Vec<std_mpsc::Sender<Vec<u8>>> = Vec::with_capacity(stream_count);
    let mut stdin_rxs: Vec<Option<std_mpsc::Receiver<Vec<u8>>>> = Vec::with_capacity(stream_count);
    for _ in 0..stream_count {
        let (tx, rx) = std_mpsc::channel::<Vec<u8>>();
        stdin_txs.push(tx);
        stdin_rxs.push(Some(rx));
    }

    // Save initial_stdin for caching before we move it.
    let initial_stdin_for_cache = initial_stdin.clone();

    struct LiveStage {
        child: std::process::Child,
        collector: std::thread::JoinHandle<Option<StageOutput>>,
    }

    let mut live: Vec<Option<LiveStage>> = Vec::with_capacity(stream_count);

    for j in 0..stream_count {
        let i = miss_idx + j;
        if cancel.load(Ordering::Relaxed) {
            for ls in live.iter_mut().flatten() {
                let _ = ls.child.kill();
            }
            let _ = ui_tx.send(StreamMsg::AllDone {
                error: Some("cancelled".to_string()),
            });
            return;
        }

        let mut child = match spawn_shell(&commands[i]) {
            Ok(c) => c,
            Err(e) => {
                for ls in live.iter_mut().flatten() {
                    let _ = ls.child.kill();
                }
                let _ = ui_tx.send(StreamMsg::AllDone {
                    error: Some(format!("stage {}: {}", i, e)),
                });
                return;
            }
        };

        let stdin_pipe = child.stdin.take().unwrap();
        let stdout_pipe = child.stdout.take().unwrap();
        let stderr_pipe = child.stderr.take().unwrap();

        // Writer thread: stdin_rx → child stdin pipe.
        let data_rx = stdin_rxs[j].take().unwrap();
        let cancel_w = cancel.clone();
        std::thread::spawn(move || {
            let mut w = stdin_pipe;
            while let Ok(chunk) = data_rx.recv() {
                if cancel_w.load(Ordering::Relaxed) {
                    break;
                }
                if w.write_all(&chunk).is_err() {
                    break;
                }
            }
        });

        // stdout / stderr readers → combined channel.
        let (combined_tx, combined_rx) = std_mpsc::channel::<CombinedLine>();
        let tx_out = combined_tx.clone();
        std::thread::spawn(move || {
            read_to_channel(stdout_pipe, false, tx_out);
        });
        std::thread::spawn(move || {
            read_to_channel(stderr_pipe, true, combined_tx);
        });

        // Collector / relay thread.
        let mode = output_modes.get(i).copied().unwrap_or(OutputMode::Stdout);
        let relay_tx: Option<std_mpsc::Sender<Vec<u8>>> = if j + 1 < stream_count {
            Some(stdin_txs[j + 1].clone())
        } else {
            None
        };
        let ui_tx_c = ui_tx.clone();
        let cancel_c = cancel.clone();

        let collector = std::thread::spawn(move || {
            let mut stdout_buf: Vec<u8> = Vec::new();
            let mut stderr_buf: Vec<u8> = Vec::new();
            let mut combined_buf: Vec<CombinedLine> = Vec::new();
            let mut pend_out: Vec<u8> = Vec::new();
            let mut pend_err: Vec<u8> = Vec::new();
            let mut pend_comb: Vec<CombinedLine> = Vec::new();
            let mut last_ui = Instant::now();

            loop {
                if cancel_c.load(Ordering::Relaxed) {
                    return None;
                }

                let line = match combined_rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(l) => Some(l),
                    Err(std_mpsc::RecvTimeoutError::Timeout) => None,
                    Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                        // Flush remaining.
                        if !pend_out.is_empty() || !pend_err.is_empty() || !pend_comb.is_empty() {
                            let _ = ui_tx_c.send(StreamMsg::StageUpdate {
                                stage_idx: i,
                                new_stdout: std::mem::take(&mut pend_out),
                                new_stderr: std::mem::take(&mut pend_err),
                                new_combined: std::mem::take(&mut pend_comb),
                            });
                        }
                        return Some(StageOutput::new(stdout_buf, stderr_buf, None, combined_buf));
                    }
                };

                if let Some(line) = line {
                    // Relay to next stage immediately.
                    if let Some(ref relay) = relay_tx {
                        let bytes = match mode {
                            OutputMode::Stdout if !line.is_stderr => {
                                Some(strip_ansi_sgr_bytes(&line.content))
                            }
                            OutputMode::Stderr if line.is_stderr => {
                                Some(strip_ansi_sgr_bytes(&line.content))
                            }
                            OutputMode::Combined => Some(strip_ansi_sgr_bytes(&line.content)),
                            _ => None,
                        };
                        if let Some(b) = bytes {
                            let _ = relay.send(b);
                        }
                    }

                    if line.is_stderr {
                        stderr_buf.extend_from_slice(&line.content);
                        pend_err.extend_from_slice(&line.content);
                    } else {
                        stdout_buf.extend_from_slice(&line.content);
                        pend_out.extend_from_slice(&line.content);
                    }
                    pend_comb.push(line.clone());
                    combined_buf.push(line);
                }

                // Throttled UI send.
                if last_ui.elapsed() >= UI_THROTTLE
                    && (!pend_out.is_empty() || !pend_err.is_empty() || !pend_comb.is_empty())
                {
                    let _ = ui_tx_c.send(StreamMsg::StageUpdate {
                        stage_idx: i,
                        new_stdout: std::mem::take(&mut pend_out),
                        new_stderr: std::mem::take(&mut pend_err),
                        new_combined: std::mem::take(&mut pend_comb),
                    });
                    last_ui = Instant::now();
                }
            }
        });

        live.push(Some(LiveStage { child, collector }));
    }

    // Feed initial stdin to stage 0.
    if !initial_stdin.is_empty() {
        let _ = stdin_txs[0].send(initial_stdin);
    }
    // Drop all our copies of stdin_txs. The only remaining senders are the
    // relay_tx clones held by each collector for the *next* stage's channel.
    // Stage 0: no upstream relay → channel closes → writer EOF.
    // Stage j>0: collector j-1 holds a clone → channel open until that collector ends.
    drop(stdin_txs);

    // ------------------------------------------------------------------
    // Wait for children, collect outputs, cache, send StageDone / AllDone.
    // ------------------------------------------------------------------
    let mut prev_stdin = initial_stdin_for_cache;

    for (j, slot) in live.iter_mut().enumerate() {
        let i = miss_idx + j;
        if let Some(mut ls) = slot.take() {
            let exit_code = ls.child.wait().ok().and_then(|s| s.code());
            let mut output = ls.collector.join().ok().flatten();

            if let Some(ref mut out) = output {
                out.exit_code = exit_code;
                cache.store(&commands[i], &prev_stdin, out.clone());
                let mode = output_modes.get(i).copied().unwrap_or(OutputMode::Stdout);
                prev_stdin = extract_relay_bytes(out, mode);
            }

            let _ = ui_tx.send(StreamMsg::StageDone {
                stage_idx: i,
                exit_code,
            });
        }
    }

    let _ = ui_tx.send(StreamMsg::AllDone { error: None });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_command_echo() {
        let mut cache = ExecutorCache::new();
        let out = cache.run("echo hello", b"", false).unwrap();
        assert_eq!(out.stdout_str().trim(), "hello");
        assert_eq!(out.exit_code, Some(0));
    }

    #[test]
    fn test_cache_hit() {
        let mut cache = ExecutorCache::new();
        // Run once to populate cache
        let out1 = cache.run("echo cached", b"", false).unwrap();
        // Second call should return cached result
        let out2 = cache.run("echo cached", b"", false).unwrap();
        assert_eq!(out1.stdout, out2.stdout);
    }

    #[test]
    fn test_force_rerun() {
        let mut cache = ExecutorCache::new();
        cache.run("echo hello", b"", false).unwrap();
        // force=true should bypass cache (result is still the same for deterministic command)
        let out = cache.run("echo hello", b"", true).unwrap();
        assert_eq!(out.stdout_str().trim(), "hello");
    }

    #[test]
    fn test_stdin_piped() {
        let mut cache = ExecutorCache::new();
        let out = cache.run("cat", b"hello world", false).unwrap();
        assert_eq!(out.stdout_str(), "hello world");
    }

    #[test]
    fn test_execute_pipeline_stages() {
        let mut cache = ExecutorCache::new();
        let commands = vec!["echo hello world".to_string(), "wc -w".to_string()];
        let outputs = execute_pipeline_stages(&mut cache, &commands, 1, false, &[]).unwrap();
        assert_eq!(outputs.len(), 2);
        let word_count: u32 = outputs[1].stdout_str().trim().parse().unwrap();
        assert_eq!(word_count, 2);
    }

    #[test]
    fn test_execute_pipeline_strips_ansi_for_next_stage_input() {
        let mut cache = ExecutorCache::new();
        let commands = vec![
            "printf '\\033[31mhello\\033[0m\\n'".to_string(),
            "wc -c".to_string(),
        ];
        let outputs = execute_pipeline_stages(&mut cache, &commands, 1, false, &[]).unwrap();
        assert_eq!(outputs.len(), 2);

        // Downstream stage receives "hello\n" (6 bytes), not ANSI sequences.
        let byte_count: u32 = outputs[1].stdout_str().trim().parse().unwrap();
        assert_eq!(byte_count, 6);

        // Original stage output still retains ANSI bytes for UI color rendering.
        assert!(outputs[0].stdout.windows(2).any(|w| w == [0x1b, b'[']));
    }

    #[test]
    fn test_combined_interleaving_captures_stderr() {
        let mut cache = ExecutorCache::new();
        // Command writes to both stdout and stderr.
        let out = cache.run("echo out; echo err >&2", b"", false).unwrap();
        assert_eq!(out.stdout_str().trim(), "out");
        assert_eq!(out.stderr_str().trim(), "err");
        // combined should have both lines.
        let combined_text: String = out
            .combined
            .iter()
            .map(|l| String::from_utf8_lossy(&l.content).into_owned())
            .collect();
        assert!(combined_text.contains("out"));
        assert!(combined_text.contains("err"));
    }

    #[test]
    fn test_execute_pipeline_stderr_as_next_input() {
        let mut cache = ExecutorCache::new();
        // Stage 0 prints to stderr; stage 1 counts bytes of its stdin.
        let commands = vec!["echo errline >&2".to_string(), "wc -c".to_string()];
        // OutputMode::Stderr for stage 0 → stage 1 receives stderr of stage 0.
        let outputs =
            execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr])
                .unwrap();
        assert_eq!(outputs.len(), 2);
        let byte_count: u32 = outputs[1].stdout_str().trim().parse().unwrap();
        // "errline\n" is 8 bytes.
        assert_eq!(byte_count, 8);
    }

    /// Different output modes produce different stdin for downstream stages and
    /// therefore separate cache entries.  Switching back to a previously-used
    /// mode returns the original cached result (cache hit by stdin hash).
    #[test]
    fn test_output_mode_switches_use_separate_cache_entries() {
        let mut cache = ExecutorCache::new();
        // Stage 0 writes distinct content to stdout and stderr.
        // Stage 1 (cat) just passes its stdin through.
        let commands = vec![
            "echo stdout_data; echo stderr_data >&2".to_string(),
            "cat".to_string(),
        ];

        // Run with Stdout mode: stage 1 receives "stdout_data\n".
        let out_a = execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout])
            .unwrap();
        assert_eq!(out_a[1].stdout_str().trim(), "stdout_data");

        // Run with Stderr mode: stage 1 receives "stderr_data\n".
        let out_b = execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr])
            .unwrap();
        assert_eq!(out_b[1].stdout_str().trim(), "stderr_data");

        // Switch back to Stdout (force=false): should be a cache hit — same
        // result as the first run.
        let out_a2 =
            execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout])
                .unwrap();
        assert_eq!(out_a2[1].stdout, out_a[1].stdout);

        // Switch back to Stderr (force=false): cache hit — same as second run.
        let out_b2 =
            execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr])
                .unwrap();
        assert_eq!(out_b2[1].stdout, out_b[1].stdout);
    }

    /// Combined output mode pipes both stdout and stderr (interleaved) as stdin
    /// to the next stage, producing a result different from Stdout-only or
    /// Stderr-only modes.
    #[test]
    fn test_combined_output_mode_pipes_both_streams() {
        let mut cache = ExecutorCache::new();
        let commands = vec!["echo out; echo err >&2".to_string(), "wc -l".to_string()];

        // Stdout mode: 1 line ("out\n").
        let out = execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout])
            .unwrap();
        let lines_stdout: u32 = out[1].stdout_str().trim().parse().unwrap();
        assert_eq!(lines_stdout, 1);

        // Stderr mode: 1 line ("err\n").
        let out = execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr])
            .unwrap();
        let lines_stderr: u32 = out[1].stdout_str().trim().parse().unwrap();
        assert_eq!(lines_stderr, 1);

        // Combined mode: 2 lines ("out\n" + "err\n").
        let out = execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Combined])
            .unwrap();
        let lines_combined: u32 = out[1].stdout_str().trim().parse().unwrap();
        assert_eq!(lines_combined, 2);
    }

    /// Switching back to a previously-used output mode is a cache hit: the
    /// downstream stage is not re-executed because the (command, stdin_hash)
    /// pair is already in the cache.
    #[test]
    fn test_cache_hit_when_switching_output_mode_back() {
        let mut cache = ExecutorCache::new();
        // Use a command that writes a unique timestamp to a temp file on each
        // invocation.  We avoid that complexity by instead counting cache
        // entries via a proxy: run the pipeline three times, toggling modes,
        // and confirm byte-identical results when returning to the first mode.
        let commands = vec![
            "printf 'hello from stdout'; printf 'hello from stderr' >&2".to_string(),
            "cat".to_string(),
        ];

        // First run: Stdout.
        let run1 = execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout])
            .unwrap();

        // Second run: Stderr (different stdin → different cache key).
        let run2 = execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr])
            .unwrap();

        // Third run: Stdout again (force=false → must be served from cache).
        let run3 = execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout])
            .unwrap();

        // run1 and run3 must be byte-identical (cache hit).
        assert_eq!(run1[1].stdout, run3[1].stdout);
        assert_eq!(run1[1].stderr, run3[1].stderr);
        // run2 must differ from run1 (different input stream).
        assert_ne!(run1[1].stdout, run2[1].stdout);
    }

    // ---------------------------------------------------------------
    // StageOutput::append_data tests
    // ---------------------------------------------------------------

    #[test]
    fn append_data_accumulates_stdout_bytes() {
        let mut out = StageOutput::empty();
        out.append_data(b"hello\n", b"", vec![]);
        out.append_data(b"world\n", b"", vec![]);
        assert_eq!(out.stdout, b"hello\nworld\n");
        assert_eq!(out.stdout_text(), "hello\nworld\n");
    }

    #[test]
    fn append_data_accumulates_stderr_bytes() {
        let mut out = StageOutput::empty();
        out.append_data(b"", b"err1\n", vec![]);
        out.append_data(b"", b"err2\n", vec![]);
        assert_eq!(out.stderr, b"err1\nerr2\n");
        assert_eq!(out.stderr_text(), "err1\nerr2\n");
    }

    #[test]
    fn append_data_increments_line_counts() {
        let mut out = StageOutput::empty();
        out.append_data(b"a\nb\n", b"e\n", vec![]);
        assert_eq!(out.stdout_line_count(), 2);
        assert_eq!(out.stderr_line_count(), 1);

        out.append_data(b"c\n", b"", vec![]);
        assert_eq!(out.stdout_line_count(), 3);
    }

    #[test]
    fn append_data_display_line_count_includes_trailing() {
        let mut out = StageOutput::empty();
        out.append_data(b"line1\nline2\n", b"", vec![]);
        // 2 newlines + trailing empty line = 3 display lines
        assert_eq!(out.display_line_count(OutputMode::Stdout), 3);
    }

    #[test]
    fn append_data_extends_line_index() {
        let mut out = StageOutput::empty();
        out.append_data(b"first\n", b"", vec![]);
        let idx = out.line_index(OutputMode::Stdout).unwrap();
        assert_eq!(idx.line_count(), 2); // line 0 + line 1

        out.append_data(b"second\n", b"", vec![]);
        let idx = out.line_index(OutputMode::Stdout).unwrap();
        assert_eq!(idx.line_count(), 3); // line 0, 1, 2
    }

    #[test]
    fn append_data_combined_interleaving() {
        let mut out = StageOutput::empty();
        let lines = vec![
            CombinedLine {
                is_stderr: false,
                content: b"out\n".to_vec(),
            },
            CombinedLine {
                is_stderr: true,
                content: b"err\n".to_vec(),
            },
        ];
        out.append_data(b"out\n", b"err\n", lines);
        assert_eq!(out.combined.len(), 2);
        assert!(!out.combined[0].is_stderr);
        assert!(out.combined[1].is_stderr);
        assert_eq!(out.display_line_count(OutputMode::Combined), 3);
    }

    #[test]
    fn append_data_matches_new_from_scratch() {
        // Building incrementally via append_data should produce the same
        // line counts and text as building via StageOutput::new.
        let stdout = b"hello\nworld\n";
        let stderr = b"err\n";

        let from_new = StageOutput::new(stdout.to_vec(), stderr.to_vec(), Some(0), vec![]);

        let mut from_append = StageOutput::empty();
        from_append.append_data(&stdout[..6], &stderr[..], vec![]);
        from_append.append_data(&stdout[6..], b"", vec![]);

        assert_eq!(from_append.stdout_text(), from_new.stdout_text());
        assert_eq!(from_append.stderr_text(), from_new.stderr_text());
        assert_eq!(
            from_append.stdout_line_count(),
            from_new.stdout_line_count()
        );
        assert_eq!(
            from_append.stderr_line_count(),
            from_new.stderr_line_count()
        );
        assert_eq!(
            from_append.display_line_count(OutputMode::Stdout),
            from_new.display_line_count(OutputMode::Stdout)
        );
    }

    #[test]
    fn append_data_with_ansi_preserves_style_in_index() {
        let mut out = StageOutput::empty();
        out.append_data(b"\x1b[31mred\n", b"", vec![]);
        out.append_data(b"still\n", b"", vec![]);

        let idx = out.line_index(OutputMode::Stdout).unwrap();
        // Line 1 should inherit the red style from line 0's escape.
        assert_eq!(idx.line_count(), 3); // lines 0, 1, 2
        // Verify style carry-over by rendering line 1 and checking color.
        let lines = crate::ansi::ansi_text_to_visible_lines(
            out.stdout_text(),
            1,
            1,
            &std::collections::HashMap::new(),
            Some(idx),
        );
        assert!(!lines.is_empty());
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.style.fg == Some(ratatui::style::Color::Red))
        );
    }

    #[test]
    fn no_line_index_for_combined_mode() {
        let out = StageOutput::empty();
        assert!(out.line_index(OutputMode::Combined).is_none());
    }
}

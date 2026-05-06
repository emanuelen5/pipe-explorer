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
    let mut child = Command::new(get_shell())
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

/// Return the path to the shell that should be used for spawning commands.
///
/// Tries to detect the parent process's actual shell executable, then falls
/// back to `$SHELL`, and finally to `"sh"`.
fn get_shell() -> String {
    get_parent_shell()
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "sh".to_string())
}

/// Return `true` when `path` looks like a Unix shell executable.
///
/// The heuristic checks whether the file-name component ends with `"sh"`
/// (covers `bash`, `zsh`, `fish`, `dash`, `ash`, `ksh`, `tcsh`, `csh`, `sh`,
/// etc.).  The check is intentionally loose so that unusual shell names still
/// work, while programs like `cargo` or `python` are correctly rejected.
fn is_shell_path(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with("sh"))
        .unwrap_or(false)
}

/// Attempt to identify the shell executable that is the parent of the current
/// process by inspecting the parent process directly.
///
/// Returns `None` when the lookup is not supported on the current platform,
/// when any OS call fails, or when the parent process is not a shell; the
/// caller should fall back to `$SHELL` / `"sh"`.
#[cfg(target_os = "linux")]
fn get_parent_shell() -> Option<String> {
    // Parse the parent PID from /proc/self/status (no unsafe required).
    let ppid: u32 = std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("PPid:")
                .and_then(|s| s.trim().parse().ok())
        })?;
    // Resolve the parent's executable via the /proc symlink.
    let exe = std::fs::read_link(format!("/proc/{ppid}/exe"))
        .ok()
        .and_then(|p| p.to_str().map(String::from))?;
    if is_shell_path(&exe) { Some(exe) } else { None }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn getppid() -> i32;
    // Declared in <libproc.h>; available in libSystem (always linked on macOS).
    fn proc_pidpath(pid: i32, buffer: *mut u8, buffersize: u32) -> i32;
}

#[cfg(target_os = "macos")]
fn get_parent_shell() -> Option<String> {
    // PROC_PIDPATHINFO_MAXSIZE = 4 * MAXPATHLEN (4 * 1024) from <libproc.h>.
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
    let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let ppid = unsafe { getppid() };
    let ret = unsafe { proc_pidpath(ppid, buf.as_mut_ptr(), buf.len() as u32) };
    if ret <= 0 {
        return None;
    }
    // Use the null terminator position when present; otherwise clamp to the
    // number of bytes actually written (ret), bounded by the buffer length.
    let len = buf
        .iter()
        .position(|&b| b == 0)
        .unwrap_or((ret as usize).min(buf.len()));
    let exe = String::from_utf8(buf[..len].to_vec()).ok()?;
    if is_shell_path(&exe) { Some(exe) } else { None }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn get_parent_shell() -> Option<String> {
    None
}

/// Spawn a child process for the given shell command.
fn spawn_shell(command: &str, interactive: bool) -> anyhow::Result<std::process::Child> {
    let mut cmd = Command::new(get_shell());
    if interactive {
        cmd.arg("-i");
        // Put interactive shells in their own session so they cannot call
        // tcsetpgrp() on the parent's controlling terminal.  Without this,
        // `zsh -i` tries to grab the foreground process group, which
        // suspends or kills pipe-explorer.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid() is async-signal-safe (POSIX).
            unsafe {
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
    }
    cmd.arg("-c")
        .arg(command)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("CLICOLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .env("FORCE_COLOR", "3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(cmd.spawn()?)
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
    interactive_flags: &[bool],
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

        let stage_interactive = interactive_flags.get(i).copied().unwrap_or(false);
        let mut child = match spawn_shell(&commands[i], stage_interactive) {
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
#[path = "tests/executor.rs"]
mod tests;

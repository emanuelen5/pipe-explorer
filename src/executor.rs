use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc as std_mpsc;

use sha2::{Digest, Sha256};

use crate::ansi::strip_ansi_sgr_bytes;

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
}

impl StageOutput {
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// Count the number of lines in stdout without allocating a full String.
    pub fn stdout_line_count(&self) -> usize {
        if self.stdout.is_empty() {
            return 0;
        }
        self.stdout.iter().filter(|&&b| b == b'\n').count()
            + if self.stdout.last() != Some(&b'\n') { 1 } else { 0 }
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

    /// Invalidate cached entry for a given command and stdin.
    #[allow(dead_code)]
    pub fn invalidate(&mut self, command: &str, stdin: &[u8]) {
        let key = CacheKey {
            command: command.to_string(),
            stdin_hash: sha256(stdin),
        };
        self.cache.remove(&key);
    }

    /// Clear all cached entries.
    pub fn clear(&mut self) {
        self.cache.clear();
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
                    if tx.send(CombinedLine { is_stderr, content: line }).is_err() {
                        return;
                    }
                }
            }
            Err(_) => break,
        }
    }
    // Send any remaining bytes (last line without trailing \n).
    if !pending.is_empty() {
        let _ = tx.send(CombinedLine { is_stderr, content: pending });
    }
}

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

    Ok(StageOutput {
        stdout,
        stderr,
        exit_code: status.code(),
        combined,
    })
}

/// Execute all stages of the pipeline up to and including `up_to_stage`.
/// Returns the outputs for each stage.
///
/// `use_stderr_as_next_input[i]` – when `true`, the stderr of stage `i` is piped as stdin
/// to stage `i+1` instead of stdout.  Missing entries default to `false` (use stdout).
pub fn execute_pipeline_stages(
    cache: &mut ExecutorCache,
    commands: &[String],
    up_to: usize,
    force: bool,
    use_stderr_as_next_input: &[bool],
) -> anyhow::Result<Vec<StageOutput>> {
    let mut outputs: Vec<StageOutput> = Vec::new();
    let mut stdin: Vec<u8> = Vec::new();

    for (i, cmd) in commands.iter().take(up_to + 1).enumerate() {
        let out = cache.run(cmd, &stdin, force)?;
        let use_stderr = use_stderr_as_next_input.get(i).copied().unwrap_or(false);
        stdin = if use_stderr {
            strip_ansi_sgr_bytes(&out.stderr)
        } else {
            strip_ansi_sgr_bytes(&out.stdout)
        };
        outputs.push(out);
    }

    Ok(outputs)
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
        let commands = vec![
            "echo errline >&2".to_string(),
            "wc -c".to_string(),
        ];
        // use_stderr=true for stage 0 → stage 1 receives stderr of stage 0.
        let outputs =
            execute_pipeline_stages(&mut cache, &commands, 1, false, &[true]).unwrap();
        assert_eq!(outputs.len(), 2);
        let byte_count: u32 = outputs[1].stdout_str().trim().parse().unwrap();
        // "errline\n" is 8 bytes.
        assert_eq!(byte_count, 8);
    }
}

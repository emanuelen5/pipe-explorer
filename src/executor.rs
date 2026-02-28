use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

/// The result of executing a single pipeline stage.
#[derive(Debug, Clone)]
pub struct StageOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
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

fn run_shell_command(command: &str, stdin_bytes: &[u8]) -> anyhow::Result<StageOutput> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Write stdin on a separate thread to avoid deadlock: if the child's
    // stdout/stderr pipe buffers fill up while we're still writing stdin,
    // both sides would block forever.  By writing in a background thread,
    // wait_with_output() can drain stdout/stderr concurrently.
    let stdin_handle = child.stdin.take().unwrap();
    let stdin_data = stdin_bytes.to_vec();
    let writer = std::thread::spawn(move || {
        let mut w = stdin_handle;
        let _ = w.write_all(&stdin_data);
        // dropping `w` closes the pipe, signalling EOF
    });

    let output = child.wait_with_output()?;
    let _ = writer.join();

    Ok(StageOutput {
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: output.status.code(),
    })
}

/// Execute all stages of the pipeline up to and including `up_to_stage`.
/// Returns the outputs for each stage.
pub fn execute_pipeline_stages(
    cache: &mut ExecutorCache,
    commands: &[String],
    up_to: usize,
    force: bool,
) -> anyhow::Result<Vec<StageOutput>> {
    let mut outputs: Vec<StageOutput> = Vec::new();
    let mut stdin: Vec<u8> = Vec::new();

    for cmd in commands.iter().take(up_to + 1) {
        let out = cache.run(cmd, &stdin, force)?;
        stdin = out.stdout.clone();
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
        let outputs = execute_pipeline_stages(&mut cache, &commands, 1, false).unwrap();
        assert_eq!(outputs.len(), 2);
        let word_count: u32 = outputs[1].stdout_str().trim().parse().unwrap();
        assert_eq!(word_count, 2);
    }
}

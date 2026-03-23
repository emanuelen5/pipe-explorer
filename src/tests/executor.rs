use super::*;

#[test]
fn test_is_shell_path_recognises_common_shells() {
    for path in &[
        "/bin/sh",
        "/bin/bash",
        "/usr/bin/zsh",
        "/usr/bin/fish",
        "/bin/dash",
        "/usr/bin/ksh",
        "/bin/ash",
        "/usr/local/bin/bash",
    ] {
        assert!(
            is_shell_path(path),
            "{path} should be recognised as a shell"
        );
    }
}

#[test]
fn test_is_shell_path_rejects_non_shells() {
    for path in &[
        "/usr/bin/cargo",
        "/usr/bin/python3",
        "/usr/bin/node",
        "",
        "/usr/bin/grep",
    ] {
        assert!(
            !is_shell_path(path),
            "{path} should not be recognised as a shell"
        );
    }
}

/// Verify that `get_parent_shell()` either returns a valid shell path or `None`.
///
/// This test is Linux-only because that platform's `/proc` filesystem provides
/// a straightforward way to inspect process exe paths.  The macOS
/// implementation (via `proc_pidpath`) is only exercised in integration/release
/// builds; CI tests run exclusively on Linux.
#[cfg(target_os = "linux")]
#[test]
fn test_get_parent_shell_is_shell_or_none() {
    if let Some(shell) = get_parent_shell() {
        assert!(
            std::path::Path::new(&shell).exists(),
            "detected shell path does not exist: {shell}"
        );
        assert!(
            is_shell_path(&shell),
            "detected parent shell '{shell}' does not look like a shell"
        );
    }
}

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
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr]).unwrap();
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
    let out_a =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout]).unwrap();
    assert_eq!(out_a[1].stdout_str().trim(), "stdout_data");

    // Run with Stderr mode: stage 1 receives "stderr_data\n".
    let out_b =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr]).unwrap();
    assert_eq!(out_b[1].stdout_str().trim(), "stderr_data");

    // Switch back to Stdout (force=false): should be a cache hit — same
    // result as the first run.
    let out_a2 =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout]).unwrap();
    assert_eq!(out_a2[1].stdout, out_a[1].stdout);

    // Switch back to Stderr (force=false): cache hit — same as second run.
    let out_b2 =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr]).unwrap();
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
    let out =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout]).unwrap();
    let lines_stdout: u32 = out[1].stdout_str().trim().parse().unwrap();
    assert_eq!(lines_stdout, 1);

    // Stderr mode: 1 line ("err\n").
    let out =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr]).unwrap();
    let lines_stderr: u32 = out[1].stdout_str().trim().parse().unwrap();
    assert_eq!(lines_stderr, 1);

    // Combined mode: 2 lines ("out\n" + "err\n").
    let out =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Combined]).unwrap();
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
    let run1 =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout]).unwrap();

    // Second run: Stderr (different stdin → different cache key).
    let run2 =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stderr]).unwrap();

    // Third run: Stdout again (force=false → must be served from cache).
    let run3 =
        execute_pipeline_stages(&mut cache, &commands, 1, false, &[OutputMode::Stdout]).unwrap();

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

// ---------------------------------------------------------------
// inject_pre_fill / relay_bytes tests
// ---------------------------------------------------------------

/// `relay_bytes` returns ANSI-stripped stdout for `OutputMode::Stdout`.
#[test]
fn relay_bytes_stdout_strips_ansi() {
    let out = StageOutput::new(
        b"\x1b[31mhello\x1b[0m\n".to_vec(),
        b"err\n".to_vec(),
        Some(0),
        vec![],
    );
    assert_eq!(out.relay_bytes(OutputMode::Stdout), b"hello\n");
}

/// `relay_bytes` returns ANSI-stripped stderr for `OutputMode::Stderr`.
#[test]
fn relay_bytes_stderr_strips_ansi() {
    let out = StageOutput::new(
        b"out\n".to_vec(),
        b"\x1b[31merr\x1b[0m\n".to_vec(),
        Some(0),
        vec![],
    );
    assert_eq!(out.relay_bytes(OutputMode::Stderr), b"err\n");
}

/// `relay_bytes` for `Combined` mode interleaves stdout and stderr lines in
/// arrival order (via `combined`) and strips ANSI sequences.
#[test]
fn relay_bytes_combined_interleaves_and_strips_ansi() {
    let out = StageOutput::new(
        b"out\n".to_vec(),
        b"err\n".to_vec(),
        Some(0),
        vec![
            CombinedLine {
                is_stderr: false,
                content: b"\x1b[32mout\x1b[0m\n".to_vec(),
            },
            CombinedLine {
                is_stderr: true,
                content: b"\x1b[31merr\x1b[0m\n".to_vec(),
            },
        ],
    );
    let relay = out.relay_bytes(OutputMode::Combined);
    assert_eq!(relay, b"out\nerr\n");
}

/// `inject_pre_fill` inserts entries into the cache so that subsequent
/// lookups are cache hits.  This simulates the scenario where a stage was
/// running when the user inserted a new stage after it: the accumulated
/// output is injected so the executor can serve it immediately without
/// restarting the command.
#[test]
fn inject_pre_fill_prevents_stage_restart() {
    let mut cache = ExecutorCache::new();

    // Simulate stage 0 ("echo hello") having already run and produced output.
    // In practice this output would have been accumulated incrementally while
    // the command was streaming; here we construct it directly.
    let stage0_output = StageOutput::new(b"hello\n".to_vec(), b"".to_vec(), Some(0), vec![]);

    // Pre-fill the cache as `trigger_exec` would do when a new stage is
    // inserted after stage 0.
    let entries = vec![PreFillEntry {
        command: "echo hello".to_string(),
        stdin: vec![],
        output: stage0_output.clone(),
    }];
    cache.inject_pre_fill(&entries);

    // A subsequent lookup for stage 0 must be a cache hit with the same data.
    let hit = cache
        .lookup("echo hello", b"")
        .expect("expected cache hit after inject_pre_fill");
    assert_eq!(hit.stdout, stage0_output.stdout);

    // A pipeline run that includes stage 0 and a new stage 1 (wc -w) should
    // now serve stage 0 from cache and only execute stage 1.
    let commands = vec!["echo hello".to_string(), "wc -w".to_string()];
    let outputs = execute_pipeline_stages(&mut cache, &commands, 1, false, &[]).unwrap();
    assert_eq!(outputs.len(), 2);
    // Stage 1 counts words in "hello\n" → 1 word.
    let word_count: u32 = outputs[1].stdout_str().trim().parse().unwrap();
    assert_eq!(word_count, 1);
}

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

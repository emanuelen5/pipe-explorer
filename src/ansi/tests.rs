use std::collections::HashMap;

use ratatui::style::Color;

use super::*;

/// Parse all lines from ANSI text with optional search highlights.
/// Test-only helper — production code uses the windowed `ansi_text_to_visible_lines`.
fn ansi_text_to_lines_with_highlights(
    text: &str,
    highlights: &HashMap<usize, Vec<(usize, usize, bool)>>,
) -> Vec<Line<'static>> {
    // Delegate to the windowed parser with start=0, max=usize::MAX.
    ansi_text_to_visible_lines(text, 0, usize::MAX, highlights, None)
}

#[test]
fn strip_ansi_removes_sgr_sequences() {
    let input = "\u{1b}[31mred\u{1b}[0m plain \u{1b}[1;34mbold-blue\u{1b}[0m";
    let output = strip_ansi_sgr(input);
    assert_eq!(output, "red plain bold-blue");
}

#[test]
fn strip_ansi_preserves_newlines() {
    let input = "first\n\u{1b}[32msecond\u{1b}[0m\nthird";
    let output = strip_ansi_sgr(input);
    assert_eq!(output, "first\nsecond\nthird");
}

#[test]
fn strip_ansi_bytes_removes_escape_sequences() {
    let input = b"\x1b[31mhello\x1b[0m\n";
    let output = strip_ansi_sgr_bytes(input);
    assert_eq!(output, b"hello\n");
}

#[test]
fn highlight_preserves_text_and_applies_background() {
    let input = "\x1b[31merror\x1b[0m ok";
    let mut highlights: HashMap<usize, Vec<(usize, usize, bool)>> = HashMap::new();
    highlights.insert(0, vec![(0, 5, true)]);

    let lines = ansi_text_to_lines_with_highlights(input, &highlights);
    assert_eq!(lines.len(), 1);
    let combined = lines[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>();
    assert_eq!(combined, "error ok");
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|s| s.style.bg == Some(Color::Yellow))
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|s| s.style.fg == Some(Color::Black))
    );
}

// ---------------------------------------------------------------
// AnsiLineIndex tests
// ---------------------------------------------------------------

#[test]
fn line_index_empty_text() {
    let idx = AnsiLineIndex::new();
    assert_eq!(idx.offsets.len(), 1); // line 0 at offset 0
    assert_eq!(idx.offsets[0], 0);
    assert_eq!(idx.scanned_up_to, 0);
}

#[test]
fn line_index_single_line_no_newline() {
    let mut idx = AnsiLineIndex::new();
    idx.extend("hello");
    assert_eq!(idx.offsets.len(), 1); // only line 0
    assert_eq!(idx.scanned_up_to, 5);
}

#[test]
fn line_index_multiple_lines() {
    let mut idx = AnsiLineIndex::new();
    idx.extend("aaa\nbbb\nccc\n");
    // Lines: "aaa\n" (offset 0), "bbb\n" (offset 4), "ccc\n" (offset 8), "" (offset 12)
    assert_eq!(idx.offsets, vec![0, 4, 8, 12]);
}

#[test]
fn line_index_tracks_ansi_style_across_lines() {
    let mut idx = AnsiLineIndex::new();
    // Set red foreground on line 0, then newline.
    idx.extend("\x1b[31mred text\nstill red\n");
    // Line 0 starts with default style, line 1 should carry the red style.
    assert_eq!(idx.styles[0], AnsiStyleState::default());
    assert_eq!(idx.styles[1].fg, Some(Color::Red));
    // Line 2 should also have red since no reset was issued.
    assert_eq!(idx.styles[2].fg, Some(Color::Red));
}

#[test]
fn line_index_style_reset_between_lines() {
    let mut idx = AnsiLineIndex::new();
    idx.extend("\x1b[31mred\x1b[0m\nplain\n");
    // Line 0 starts default, line 1 starts with reset (default).
    assert_eq!(idx.styles[0], AnsiStyleState::default());
    assert_eq!(idx.styles[1], AnsiStyleState::default());
}

#[test]
fn line_index_incremental_extend() {
    let mut idx = AnsiLineIndex::new();
    idx.extend("aaa\n");
    assert_eq!(idx.offsets, vec![0, 4]);
    assert_eq!(idx.scanned_up_to, 4);

    // Extend with more data — only new bytes should be scanned.
    idx.extend("aaa\nbbb\n");
    assert_eq!(idx.offsets, vec![0, 4, 8]);
    assert_eq!(idx.scanned_up_to, 8);
}

#[test]
fn line_index_incremental_does_not_rescan() {
    let mut idx = AnsiLineIndex::new();
    let text = "line1\nline2\nline3\n";
    idx.extend(text);
    let offsets_after_first = idx.offsets.clone();

    // Calling extend again with the same text should be a no-op.
    idx.extend(text);
    assert_eq!(idx.offsets, offsets_after_first);
}

#[test]
fn line_index_style_carries_across_incremental_extends() {
    let mut idx = AnsiLineIndex::new();
    idx.extend("\x1b[32m");
    // No newline yet, so only line 0 offset.
    assert_eq!(idx.offsets.len(), 1);

    // Now extend with a newline — the style should carry over.
    let full = "\x1b[32mgreen\nnext\n";
    idx.extend(full);
    assert_eq!(idx.styles[1].fg, Some(Color::Green));
}

// ---------------------------------------------------------------
// ansi_text_to_visible_lines tests
// ---------------------------------------------------------------

/// Helper: extract plain text from parsed Lines.
fn lines_to_text(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn visible_lines_full_range() {
    let text = "line0\nline1\nline2\n";
    let lines = ansi_text_to_visible_lines(text, 0, 10, &HashMap::new(), None);
    assert_eq!(lines_to_text(&lines), vec!["line0", "line1", "line2", ""]);
}

#[test]
fn visible_lines_window_middle() {
    let text = "line0\nline1\nline2\nline3\nline4\n";
    let lines = ansi_text_to_visible_lines(text, 1, 2, &HashMap::new(), None);
    assert_eq!(lines_to_text(&lines), vec!["line1", "line2"]);
}

#[test]
fn visible_lines_window_past_end() {
    let text = "a\nb\n";
    let lines = ansi_text_to_visible_lines(text, 0, 100, &HashMap::new(), None);
    assert_eq!(lines_to_text(&lines), vec!["a", "b", ""]);
}

#[test]
fn visible_lines_empty_text() {
    let lines = ansi_text_to_visible_lines("", 0, 10, &HashMap::new(), None);
    assert!(lines.is_empty());
}

#[test]
fn visible_lines_preserves_ansi_color() {
    let text = "\x1b[31mred\x1b[0m\n";
    let lines = ansi_text_to_visible_lines(text, 0, 1, &HashMap::new(), None);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines_to_text(&lines), vec!["red"]);
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|s| s.style.fg == Some(Color::Red))
    );
}

#[test]
fn visible_lines_ansi_style_carries_past_scroll() {
    // Set red on line 0, then scroll to line 1 — should still be red.
    let text = "\x1b[31mred line0\nstill red line1\n";
    let lines = ansi_text_to_visible_lines(text, 1, 1, &HashMap::new(), None);
    assert_eq!(lines_to_text(&lines), vec!["still red line1"]);
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|s| s.style.fg == Some(Color::Red))
    );
}

#[test]
fn visible_lines_with_line_index_matches_without() {
    let text = "\x1b[31mline0\n\x1b[32mline1\n\x1b[0mline2\nline3\nline4\n";

    let mut idx = AnsiLineIndex::new();
    idx.extend(text);

    // Compare windowed output with and without line index for various scroll positions.
    for start in 0..5 {
        let without = ansi_text_to_visible_lines(text, start, 2, &HashMap::new(), None);
        let with = ansi_text_to_visible_lines(text, start, 2, &HashMap::new(), Some(&idx));
        assert_eq!(
            lines_to_text(&without),
            lines_to_text(&with),
            "text mismatch at start_line={}",
            start
        );
        // Also verify styles match.
        for (j, (a, b)) in without.iter().zip(with.iter()).enumerate() {
            for (k, (sa, sb)) in a.spans.iter().zip(b.spans.iter()).enumerate() {
                assert_eq!(
                    sa.style, sb.style,
                    "style mismatch at start={} line={} span={}",
                    start, j, k
                );
            }
        }
    }
}

#[test]
fn visible_lines_with_highlights() {
    let text = "hello world\n";
    let mut highlights: HashMap<usize, Vec<(usize, usize, bool)>> = HashMap::new();
    highlights.insert(0, vec![(0, 5, true)]); // highlight "hello"
    let lines = ansi_text_to_visible_lines(text, 0, 1, &highlights, None);
    assert_eq!(lines_to_text(&lines), vec!["hello world"]);
    // The highlighted span should have yellow background (current match).
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|s| s.style.bg == Some(Color::Yellow))
    );
}

#[test]
fn visible_lines_start_beyond_content() {
    let text = "only\n";
    let lines = ansi_text_to_visible_lines(text, 100, 10, &HashMap::new(), None);
    assert!(lines.is_empty());
}

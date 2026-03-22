use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::executor::StageOutput;
use crate::pipeline::parse_pipeline;

fn make_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn make_stage_output(stdout: &str) -> StageOutput {
    StageOutput::new(stdout.as_bytes().to_vec(), vec![], Some(0), vec![])
}

fn make_error_stage_output(exit_code: i32) -> StageOutput {
    StageOutput::new(vec![], b"error".to_vec(), Some(exit_code), vec![])
}

/// Parses a text description with pointer indicators, used in tests to visually describe
/// a string and byte offsets into it.
///
/// The first line of `description` is the text. The two subsequent lines must contain a `^`
/// character; the column position of `^` gives the corresponding byte offset into the
/// text. Offsets are returned in the order the pointer lines appear.
///
/// # Example
/// ```
/// // "hello world" with first pointer at 'o' (offset 4) and second at 'h' (offset 0)
/// #[rustfmt::skip]
/// let (text, input, before, after) = parse_pointer_description(concat!(
///     "hello world\n",
///     "    ^      \n",
///     "^          ",
/// ));
/// assert_cursor(text, before, after);
/// ```
fn parse_pointer_description<'a>(
    description: &'a str,
    cursor_before: &'a str,
    cursor_after: &'a str,
) -> (&'a str, &'a str, usize, usize) {
    // Make sure it's on a single line
    assert!(description.lines().count() == 1);
    let before_caret = cursor_before
        .find('^')
        .expect("cursor_before must contain '^'");
    let after_caret = cursor_after
        .find('^')
        .expect("cursor_after must contain '^'");
    assert!(
        before_caret < description.len() + 1,
        "cursor_before '^' must be within description"
    );
    assert!(
        after_caret < description.len() + 1,
        "cursor_after '^' must be within description"
    );
    // Include final character
    if before_caret == after_caret {
        (description, "", before_caret, after_caret)
    } else if after_caret < before_caret {
        (
            description,
            &description[0..before_caret],
            before_caret,
            after_caret,
        )
    } else {
        (
            description,
            &description[before_caret..],
            before_caret,
            after_caret,
        )
    }
}

fn assert_cursor(text: &str, cursor_expected: usize, cursor_actual: usize) {
    if cursor_expected == cursor_actual {
        return;
    }

    let cursor_1 = " ".repeat(cursor_expected) + "^";
    let cursor_2 = " ".repeat(cursor_actual) + "^";
    #[rustfmt::skip]
    panic!(concat!(
        "Expected cursor:\n",
        "  \"{}\"\n",
        "   {}\n",
        "Actual cursor:\n",
        "  \"{}\"\n",
        "   {}\n"
    ), text, cursor_1, text, cursor_2);
}

#[test]
fn test_parse_pointer_description_goes_to_left() {
    #[rustfmt::skip]
    let (text, input, before, after) = parse_pointer_description(
        "hello world",
        "    ^      ",
        " ^         ",
    );
    assert_eq!(text, "hello world");
    assert_eq!(input, "hell");
    assert_eq!(before, 4);
    assert_eq!(after, 1);
}

#[test]
fn test_parse_pointer_description_goes_to_right() {
    #[rustfmt::skip]
    let (text, input, before, after) = parse_pointer_description(
        "hello world",
        "   ^       ",
        "     ^     ",
    );
    assert_eq!(text, "hello world");
    assert_eq!(input, "lo world");
    assert_eq!(before, 3);
    assert_eq!(after, 5);
}

#[test]
fn test_parse_pointer_no_change() {
    #[rustfmt::skip]
    let (text, input, before, after) = parse_pointer_description(
        "hello world",
        "    ^      ",
        "    ^      ",
    );
    assert_eq!(text, "hello world");
    assert_eq!(input, "");
    assert_eq!(before, 4);
    assert_eq!(after, 4);
}

#[test]
fn test_editor_scroll_cursor_in_view() {
    // Cursor at col 3, inner_width = 10: no scroll needed.
    assert_eq!(compute_editor_scroll(0, "hel", 10), 0);
}

#[test]
fn test_editor_scroll_cursor_past_right_edge() {
    // inner_width = 5; cursor at col 7 → scroll_x must become 3.
    assert_eq!(compute_editor_scroll(0, "0123456", 5), 3);
}

#[test]
fn test_editor_scroll_cursor_before_left_edge() {
    // scroll_x is 5, cursor at col 2 → scroll snaps back.
    assert_eq!(compute_editor_scroll(5, "01", 5), 2);
}

#[test]
fn test_editor_scroll_no_change_when_visible() {
    // scroll_x = 3, inner_width = 5, cursor at col 4: 3 <= 4 < 8, no change.
    assert_eq!(compute_editor_scroll(3, "0123", 5), 3);
}

#[test]
fn test_editor_scroll_zero_inner_width() {
    // Should return current scroll unchanged (no-op).
    assert_eq!(compute_editor_scroll(0, "hello", 0), 0);
}

#[test]
fn test_word_left_from_middle_of_word() {
    #[rustfmt::skip]
    let (text, input, _before, after) = parse_pointer_description(
        "hello world",
        "         ^ ",
        "      ^    ",
    );
    assert_cursor(text, after, word_left_pos(input));
}

#[test]
fn test_word_left_with_trailing_whitespace() {
    #[rustfmt::skip]
    let (text, input, _before, after) = parse_pointer_description(
        "hello world ",
        "           ^",
        "      ^     ",
    );
    assert_cursor(text, after, word_left_pos(input));
}

#[test]
fn test_word_left_from_start_of_word() {
    #[rustfmt::skip]
    let (text, input, _before, after) = parse_pointer_description(
        "hello world",
        "     ^",
        "^     "
    );
    assert_cursor(text, after, word_left_pos(input));
}

#[test]
fn test_word_left_at_beginning() {
    #[rustfmt::skip]
    let (text, input, _before, after) = parse_pointer_description(
        "hello world",
        "^          ",
        "^          ",
    );
    assert_cursor(text, after, word_left_pos(input));
}

#[test]
fn test_word_left_only_whitespace() {
    #[rustfmt::skip]
    let (text, input, _before, after) = parse_pointer_description(
        "           ",
        "        ^  ",
        "^          ",
    );
    assert_cursor(text, after, word_left_pos(input));
}

#[test]
fn test_word_right_from_start_of_word() {
    #[rustfmt::skip]
    let (text, input, before, after) = parse_pointer_description(
        "hello world",
        "^          ",
        "      ^     ",
    );
    assert_cursor(text, after, before + word_right_pos(input));
}

#[test]
fn test_word_right_from_whitespace() {
    #[rustfmt::skip]
    let (text, input, before, after) = parse_pointer_description(
        "hello world ",
        "      ^     ",
        "            ^",
    );
    assert_cursor(text, after, before + word_right_pos(input));
}

#[test]
fn test_word_right_at_last_word() {
    #[rustfmt::skip]
    let (text, input, before, after) = parse_pointer_description(
        "hello world",
        "      ^    ",
        "           ^",
    );
    assert_cursor(text, after, before + word_right_pos(input));
}

#[test]
fn test_word_right_at_end() {
    #[rustfmt::skip]
    let (text, input, before, after) = parse_pointer_description(
        "hello world",
        "           ^",
        "           ^",
    );
    assert_cursor(text, after, before + word_right_pos(input));
}

#[test]
fn test_word_right_only_whitespace() {
    #[rustfmt::skip]
    let (text, input, before, after) = parse_pointer_description(
        "           ",
        "  ^        ",
        "           ^",
    );
    assert_cursor(text, after, before + word_right_pos(input));
}

#[test]
fn test_editor_ctrl_left_jumps_to_word_start() {
    let (text, _input, before, after) = parse_pointer_description(
        "hello world",
        "           ^",
        "      ^     ",
    );
    let mut editor = EditorState::new(text.to_string());
    editor.cursor = before;
    let key = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);
    editor.handle_key(key);
    assert_eq!(editor.cursor, after);
}

#[test]
fn test_editor_ctrl_right_jumps_to_next_word_start() {
    let (text, _input, before, after) = parse_pointer_description(
        "hello world",
        "^          ",
        "      ^     ",
    );
    let mut editor = EditorState::new(text.to_string());
    editor.cursor = before;
    let key = KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL);
    editor.handle_key(key);
    assert_eq!(editor.cursor, after);
}

#[test]
fn test_editor_ctrl_left_at_beginning_stays() {
    let (text, _input, before, after) = parse_pointer_description(
        "hello world",
        "^          ",
        "^           ",
    );
    let mut editor = EditorState::new(text.to_string());
    editor.cursor = before;
    let key = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);
    editor.handle_key(key);
    assert_eq!(editor.cursor, after);
}

#[test]
fn test_editor_ctrl_right_at_end_stays() {
    let (text, _input, before, after) = parse_pointer_description(
        "hello world",
        "           ^",
        "           ^",
    );
    let mut editor = EditorState::new(text.to_string());
    editor.cursor = before;
    let key = KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL);
    editor.handle_key(key);
    assert_eq!(editor.cursor, after);
}

#[test]
fn test_editor_backspace_multibyte_char() {
    // '│' is a 3-byte UTF-8 character (U+2502, bytes E2 94 82).
    // Cursor placed right after it; backspace should remove the whole character.
    let mut editor = EditorState::new("a│b".to_string());
    // cursor after '│': 'a' = 1 byte, '│' = 3 bytes → byte offset 4
    editor.cursor = 4;
    let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
    editor.handle_key(key);
    assert_eq!(editor.content, "ab");
    assert_eq!(editor.cursor, 1);
}

/// Navigate left preserves all stage_outputs (no exec triggered).
#[tokio::test]
async fn test_navigate_left_preserves_stage_outputs() {
    let pipeline = parse_pipeline("echo a | echo b | echo c");
    let mut app = App::new(pipeline);

    // Simulate all three stages having been executed already.
    app.stage_outputs = vec![
        make_stage_output("a\n"),
        make_stage_output("b\n"),
        make_stage_output("c\n"),
    ];
    app.pipeline.selected = 2; // currently on stage 3

    // Navigate left twice.
    app.handle_event(make_key(KeyCode::Left));
    app.handle_event(make_key(KeyCode::Left));

    assert_eq!(app.pipeline.selected, 0);
    // All three stage outputs must still be present.
    assert_eq!(app.stage_outputs.len(), 3);
    // No background execution should have been triggered.
    assert!(!app.running);
}

/// Navigate right to an already-computed stage preserves all stage_outputs.
#[tokio::test]
async fn test_navigate_right_to_cached_stage_preserves_outputs() {
    let pipeline = parse_pipeline("echo a | echo b | echo c");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![
        make_stage_output("a\n"),
        make_stage_output("b\n"),
        make_stage_output("c\n"),
    ];
    app.pipeline.selected = 0;

    // Navigate right to stage 1 (already cached).
    app.handle_event(make_key(KeyCode::Right));

    assert_eq!(app.pipeline.selected, 1);
    assert_eq!(app.stage_outputs.len(), 3);
    assert!(!app.running);
}

/// Navigate right to a new (uncached) stage triggers execution.
#[tokio::test]
async fn test_navigate_right_to_new_stage_triggers_exec() {
    let pipeline = parse_pipeline("echo a | echo b | echo c");
    let mut app = App::new(pipeline);

    // Only stage 0 has been computed.
    app.stage_outputs = vec![make_stage_output("a\n")];
    app.pipeline.selected = 0;

    // Navigate right to stage 1 (not yet cached).
    app.handle_event(make_key(KeyCode::Right));

    assert_eq!(app.pipeline.selected, 1);
    // Execution should have been triggered.
    assert!(app.running);
}

/// Error status of downstream stages is preserved when navigating left.
#[tokio::test]
async fn test_error_status_preserved_after_navigate_left() {
    let pipeline = parse_pipeline("echo a | false | echo c");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![
        make_stage_output("a\n"),
        make_error_stage_output(1),
        make_stage_output("c\n"),
    ];
    app.pipeline.selected = 2;

    // Navigate back to stage 0.
    app.handle_event(make_key(KeyCode::Left));
    app.handle_event(make_key(KeyCode::Left));

    assert_eq!(app.pipeline.selected, 0);
    // Error exit code of stage 1 must be preserved.
    assert_eq!(app.stage_outputs[1].exit_code, Some(1));
    assert_eq!(app.stage_outputs.len(), 3);
}

/// Changing output mode on stage 0 invalidates downstream stage_outputs so that
/// following stages will recalculate with the newly selected input stream.
#[tokio::test]
async fn test_set_output_mode_invalidates_downstream_outputs() {
    let pipeline = parse_pipeline("echo a | echo b | echo c");
    let mut app = App::new(pipeline);

    // All three stages have been executed.
    app.stage_outputs = vec![
        make_stage_output("a\n"),
        make_stage_output("b\n"),
        make_stage_output("c\n"),
    ];
    app.pipeline.selected = 0;

    // Change mode on stage 0 → should invalidate stages 1 and 2.
    app.set_output_mode(app.pipeline.selected, OutputMode::Stderr);

    assert_eq!(app.stage_views[0].output_mode, OutputMode::Stderr);
    // Only stage 0's output remains; downstream outputs are purged.
    assert_eq!(app.stage_outputs.len(), 1);
}

/// After changing output mode, navigating right triggers re-execution of
/// the now-invalidated downstream stage.
#[tokio::test]
async fn test_output_mode_change_causes_downstream_reexec_on_navigate() {
    let pipeline = parse_pipeline("echo a | echo b");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![make_stage_output("a\n"), make_stage_output("b\n")];
    app.pipeline.selected = 0;

    // Change mode: stage 1's inputs might now differ.
    app.set_output_mode(app.pipeline.selected, OutputMode::Stderr);

    // Navigate right: stage 1 is no longer in stage_outputs → exec is triggered.
    app.handle_event(make_key(KeyCode::Right));

    assert_eq!(app.pipeline.selected, 1);
    assert!(
        app.running,
        "execution must be triggered for the invalidated downstream stage"
    );
}

/// Changing output mode on the last stage does not purge any outputs since
/// there are no downstream stages.
#[tokio::test]
async fn test_set_output_mode_on_last_stage_preserves_all_outputs() {
    let pipeline = parse_pipeline("echo a | echo b");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![make_stage_output("a\n"), make_stage_output("b\n")];
    app.pipeline.selected = 1; // last stage

    app.set_output_mode(app.pipeline.selected, OutputMode::Stderr);

    // Both outputs must still be present because stage 1 is the last stage.
    assert_eq!(app.stage_outputs.len(), 2);
    assert_eq!(app.stage_views[1].output_mode, OutputMode::Stderr);
}

/// Switching output mode back and forth truncates downstream outputs every
/// time, even when returning to the original mode.  This ensures the UI
/// always triggers re-execution (which will be a cache hit in the background
/// executor).
#[tokio::test]
async fn test_switch_mode_back_and_forth_truncates_each_time() {
    let pipeline = parse_pipeline("echo a | cat | wc -c");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![
        make_stage_output("a\n"),
        make_stage_output("a\n"),
        make_stage_output("2\n"),
    ];
    app.pipeline.selected = 0;

    // Switch to stderr → downstream truncated.
    app.set_output_mode(0, OutputMode::Stderr);
    assert_eq!(app.stage_outputs.len(), 1);

    // Pretend stages 1-2 were re-executed with stderr input.
    app.stage_outputs = vec![
        make_stage_output("a\n"),
        make_stage_output(""),
        make_stage_output("0\n"),
    ];

    // Switch back to stdout → downstream truncated again.
    app.set_output_mode(0, OutputMode::Stdout);
    assert_eq!(app.stage_outputs.len(), 1);
    assert_eq!(app.stage_views[0].output_mode, OutputMode::Stdout);
}

/// After switching output mode back to the original, navigating to a
/// downstream stage triggers re-execution.  The background executor's
/// cache (keyed by command + sha256(stdin)) still holds the old result,
/// so this will be a cache hit — no actual subprocess is spawned.
#[tokio::test]
async fn test_switch_mode_back_triggers_reexec_for_cache_hit() {
    let pipeline = parse_pipeline("echo a | cat");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![make_stage_output("a\n"), make_stage_output("a\n")];
    app.pipeline.selected = 0;

    // Switch to stderr (truncates downstream).
    app.set_output_mode(0, OutputMode::Stderr);
    assert_eq!(app.stage_outputs.len(), 1);

    // Switch back to stdout (truncates downstream again).
    app.set_output_mode(0, OutputMode::Stdout);
    assert_eq!(app.stage_outputs.len(), 1);

    // Navigate right: stage 1 is missing from stage_outputs → exec triggered.
    app.handle_event(make_key(KeyCode::Right));
    assert_eq!(app.pipeline.selected, 1);
    assert!(
        app.running,
        "execution must be triggered so the executor cache can serve the result"
    );
}

/// Pressing 'm' to cycle output mode on a non-last stage truncates
/// downstream outputs, just like calling set_output_mode directly.
#[tokio::test]
async fn test_key_m_cycles_mode_and_invalidates_downstream() {
    let pipeline = parse_pipeline("echo a | cat | wc -c");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![
        make_stage_output("a\n"),
        make_stage_output("a\n"),
        make_stage_output("2\n"),
    ];
    app.pipeline.selected = 0;

    // Press 'm' → Stdout → Stderr, downstream truncated.
    app.handle_event(make_key(KeyCode::Char('m')));
    assert_eq!(app.stage_views[0].output_mode, OutputMode::Stderr);
    assert_eq!(app.stage_outputs.len(), 1);
}

// ---------------------------------------------------------------
// Search navigation tests
// ---------------------------------------------------------------

/// Helper: set up an app with pre-populated output and search matches.
/// Output has "aa" on lines 0, 2, 5, 5 (two matches on line 5), 8.
fn make_app_with_search() -> App {
    let pipeline = parse_pipeline("echo test");
    let mut app = App::new(pipeline);
    // 10 lines of output; "aa" appears on lines 0, 2, 5 (twice), and 8.
    let text = "aa\nbb\naa\ncc\ndd\naaXaa\nff\ngg\naa\nii\n";
    app.stage_outputs = vec![make_stage_output(text)];
    app.view_mut().search.query = "aa".to_string();
    app.compute_search_matches();
    // Matches should be: (0,0,2), (2,0,2), (5,0,2), (5,3,5), (8,0,2)
    assert_eq!(app.view().search.matches.len(), 5);
    app
}

#[tokio::test]
async fn test_confirm_search_starts_from_scroll_position() {
    let pipeline = parse_pipeline("echo test");
    let mut app = App::new(pipeline);
    let text = "aa\nbb\naa\ncc\naa\n";
    app.stage_outputs = vec![make_stage_output(text)];
    // Scroll to line 2 before searching.
    app.view_mut().scroll = 2;
    app.view_mut().search.query = "aa".to_string();
    app.confirm_search();
    // Should land on the match at line 2 (index 1), not line 0.
    assert_eq!(app.view().search.match_idx, 1);
    assert_eq!(app.view().scroll, 2);
}

#[tokio::test]
async fn test_confirm_search_wraps_when_no_match_after_scroll() {
    let pipeline = parse_pipeline("echo test");
    let mut app = App::new(pipeline);
    let text = "aa\nbb\ncc\n";
    app.stage_outputs = vec![make_stage_output(text)];
    // Scroll past all matches.
    app.view_mut().scroll = 2;
    app.view_mut().search.query = "aa".to_string();
    app.confirm_search();
    // Should wrap to match 0 at line 0.
    assert_eq!(app.view().search.match_idx, 0);
    assert_eq!(app.view().scroll, 0);
}

#[tokio::test]
async fn test_search_next_sequential_same_line() {
    let mut app = make_app_with_search();
    // Start at match 0 (line 0).
    app.view_mut().search.match_idx = 0;
    app.view_mut().scroll = 0;
    // Step through matches sequentially.
    app.search_next();
    assert_eq!(app.view().search.match_idx, 1);
    assert_eq!(app.view().scroll, 2);

    // Advance to line 5 (first match there).
    app.search_next();
    assert_eq!(app.view().search.match_idx, 2);
    assert_eq!(app.view().scroll, 5);

    // Next: should land on the second match on line 5 (idx 3), NOT skip to line 8.
    app.search_next();
    assert_eq!(app.view().search.match_idx, 3);
    assert_eq!(app.view().scroll, 5);

    // Next: now move to line 8.
    app.search_next();
    assert_eq!(app.view().search.match_idx, 4);
    assert_eq!(app.view().scroll, 8);
}

#[tokio::test]
async fn test_search_next_wraps_at_end() {
    let mut app = make_app_with_search();
    // Position on the last match.
    app.view_mut().search.match_idx = 4;
    app.view_mut().scroll = 8;
    app.search_next();
    assert_eq!(app.view().search.match_idx, 0);
    assert_eq!(app.view().scroll, 0);
}

#[tokio::test]
async fn test_search_next_after_manual_scroll() {
    let mut app = make_app_with_search();
    // Currently on match 0 (line 0), user scrolls to line 4.
    app.view_mut().search.match_idx = 0;
    app.view_mut().scroll = 4;
    // Press n: should jump to first match after line 4 → line 5 (match idx 2).
    app.search_next();
    assert_eq!(app.view().search.match_idx, 2);
    assert_eq!(app.view().scroll, 5);
}

#[tokio::test]
async fn test_search_next_after_scroll_past_all_matches_wraps() {
    let mut app = make_app_with_search();
    app.view_mut().search.match_idx = 4;
    app.view_mut().scroll = 9; // past all matches
    app.search_next();
    assert_eq!(app.view().search.match_idx, 0);
    assert_eq!(app.view().scroll, 0);
}

#[tokio::test]
async fn test_search_prev_sequential_same_line() {
    let mut app = make_app_with_search();
    // Start at match 4 (line 8).
    app.view_mut().search.match_idx = 4;
    app.view_mut().scroll = 8;
    app.search_prev();
    assert_eq!(app.view().search.match_idx, 3);
    assert_eq!(app.view().scroll, 5);

    // Prev again: second match on line 5 → first match on line 5.
    app.search_prev();
    assert_eq!(app.view().search.match_idx, 2);
    assert_eq!(app.view().scroll, 5);

    // Prev again: line 2.
    app.search_prev();
    assert_eq!(app.view().search.match_idx, 1);
    assert_eq!(app.view().scroll, 2);
}

#[tokio::test]
async fn test_search_prev_wraps_at_beginning() {
    let mut app = make_app_with_search();
    app.view_mut().search.match_idx = 0;
    app.view_mut().scroll = 0;
    app.search_prev();
    assert_eq!(app.view().search.match_idx, 4);
    assert_eq!(app.view().scroll, 8);
}

#[tokio::test]
async fn test_search_prev_after_manual_scroll() {
    let mut app = make_app_with_search();
    // Currently on match 4 (line 8), user scrolls to line 6.
    app.view_mut().search.match_idx = 4;
    app.view_mut().scroll = 6;
    // Press N: should jump to last match before line 6 → line 5 match idx 3.
    app.search_prev();
    assert_eq!(app.view().search.match_idx, 3);
    assert_eq!(app.view().scroll, 5);
}

#[tokio::test]
async fn test_search_prev_after_scroll_before_all_matches_wraps() {
    let mut app = make_app_with_search();
    // Current match is on line 5, user scrolls to line 0 (before match at line 0 uses <).
    // Actually line 0 has a match, so let's say we manipulate match_idx to be on line 5
    // but scroll to 0 — partition_point(line < 0) = 0 → wraps to last.
    app.view_mut().search.match_idx = 2;
    app.view_mut().scroll = 0;
    // Since cur_line (5) != scroll (0), binary search kicks in.
    // partition_point(line < 0) = 0 → wraps to last match.
    app.search_prev();
    assert_eq!(app.view().search.match_idx, 4);
    assert_eq!(app.view().scroll, 8);
}

#[tokio::test]
async fn test_search_next_no_matches_is_noop() {
    let pipeline = parse_pipeline("echo test");
    let mut app = App::new(pipeline);
    app.stage_outputs = vec![make_stage_output("no match here\n")];
    app.view_mut().search.query = "zzz".to_string();
    app.compute_search_matches();
    assert!(app.view().search.matches.is_empty());
    // Should not panic or change anything.
    app.search_next();
    app.search_prev();
    assert_eq!(app.view().scroll, 0);
}

/// Pressing down past the end of output should not accumulate phantom scroll.
#[tokio::test]
async fn test_scroll_down_clamped_to_visible_height() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    // Simulate 10 lines of output and a visible height of 3.
    app.stage_outputs = vec![make_stage_output("1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n")];
    app.visible_output_lines = 3;
    app.visible_output_width = 80;

    // display_line_count = 11 (10 newlines + 1 trailing empty line), visible_height = 3,
    // so max useful scroll = 11 - 3 = 8.
    // Scrolling down many more times should not push scroll beyond 8.
    for _ in 0..20 {
        app.scroll_down(1);
    }
    assert_eq!(
        app.view().scroll,
        8,
        "scroll should be clamped to total - visible"
    );
}

/// After reaching the bottom, pressing up once should immediately move the view.
#[tokio::test]
async fn test_scroll_up_after_bottom_has_no_hysteresis() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![make_stage_output("1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n")];
    app.visible_output_lines = 3;
    app.visible_output_width = 80;

    // Scroll all the way to the bottom.
    for _ in 0..20 {
        app.scroll_down(1);
    }
    let bottom = app.view().scroll;
    assert_eq!(bottom, 8, "should be at the real bottom");

    // A single scroll_up should move away from bottom immediately.
    app.scroll_up(1);
    assert_eq!(app.view().scroll, 7, "one up from bottom should reach 6");
}

/// The G/End key should jump to the correct bottom position (no hysteresis).
#[tokio::test]
async fn test_g_key_jumps_to_correct_bottom() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![make_stage_output("1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n")];
    app.visible_output_lines = 3;
    app.visible_output_width = 80;

    // Press G (jump to bottom).
    app.handle_event(make_key(KeyCode::Char('G')));
    assert_eq!(
        app.view().scroll,
        8,
        "G should set scroll to total - visible"
    );

    // One up should immediately change scroll.
    app.handle_event(make_key(KeyCode::Char('k')));
    assert_eq!(app.view().scroll, 7, "k after G should reach 6");
}

/// When lines wrap, max scroll should allow scrolling further so the last line is visible.
#[tokio::test]
async fn test_scroll_accounts_for_line_wrapping() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    // 5 lines of output, each 20 chars wide. At width=10 each line wraps to 2 visual rows.
    // 5 lines × 2 visual rows = 10 visual rows total, plus 1 row for trailing empty line.
    // With visible_height = 6 (6 visual rows on screen):
    //   Walking backward:
    //     line 5 (empty): 1 row (accum = 1)
    //     line 4: 2 rows (accum = 3)
    //     line 3: 2 rows (accum = 5)
    //     line 2: 2 rows → accum would be 7 > 6, and lines_from_end=3 > 0 → STOP
    //   lines_from_end = 3, max_scroll = 6 - 3 = 3
    //   Lines 3-5 take 5 visual rows, fitting within 6.
    let long_line = "abcdefghij".repeat(2); // 20 chars
    let content = format!("{0}\n{0}\n{0}\n{0}\n{0}\n", long_line);
    app.stage_outputs = vec![make_stage_output(&content)];
    app.visible_output_lines = 6;
    app.visible_output_width = 10;

    let max_scroll = app.compute_max_scroll();
    assert_eq!(max_scroll, 3, "max_scroll should account for wrapped lines");

    // Scrolling down many times should clamp to 3.
    for _ in 0..20 {
        app.scroll_down(1);
    }
    assert_eq!(app.view().scroll, 3, "scroll should be clamped to wrap-aware max");

    // G should jump to the correct bottom.
    app.view_mut().scroll = 0;
    app.handle_event(make_key(KeyCode::Char('G')));
    assert_eq!(app.view().scroll, 3, "G should use wrap-aware max_scroll");
}

/// When all lines fit within visible height (even with wrapping), max scroll should be 0.
#[tokio::test]
async fn test_scroll_wrapping_all_fits() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    // 2 lines of 20 chars. At width=10, each wraps to 2 rows → 4 visual rows.
    // visible_height = 5 → all fits, max_scroll = 0.
    let content = "abcdefghijklmnopqrst\nabcdefghijklmnopqrst\n";
    app.stage_outputs = vec![make_stage_output(content)];
    app.visible_output_lines = 5;
    app.visible_output_width = 10;

    assert_eq!(app.compute_max_scroll(), 0, "all content fits — max_scroll should be 0");
}

/// Lines with ANSI escape sequences should have their display width computed
/// from the visible text only (escapes don't consume columns).
#[tokio::test]
async fn test_scroll_wrapping_with_ansi() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    // Each visible line is exactly 5 chars ("hello"), but the raw text contains
    // ANSI codes that bulk it up.  At width=5 no wrapping should occur.
    let line = "\x1b[31mhello\x1b[0m";
    let content = format!("{0}\n{0}\n{0}\n{0}\n{0}\n", line);
    app.stage_outputs = vec![make_stage_output(&content)];
    app.visible_output_lines = 3;
    app.visible_output_width = 5;

    // 6 display lines (5 newlines + trailing empty).  ANSI-stripped width = 5.
    // At width=5, each line takes 1 visual row → no wrapping.
    // max_scroll = 6 - 3 = 3 (same as without wrapping).
    assert_eq!(app.compute_max_scroll(), 3, "ANSI codes should not affect wrapping");
}

/// Long lines followed by short lines: the short lines must be fully visible
/// at max scroll (reproduces the real-world bug where the last few lines were
/// clipped because a long line was incorrectly included in the visible window).
#[tokio::test]
async fn test_scroll_long_lines_followed_by_short() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    // 3 long lines (40 chars each → 4 visual rows at width=10) + 3 short lines.
    let long = "a".repeat(40);
    let content = format!("{0}\n{0}\n{0}\nshort1\nshort2\nshort3\n", long);
    app.stage_outputs = vec![make_stage_output(&content)];
    app.visible_output_lines = 6;
    app.visible_output_width = 10;

    // 7 logical lines (3 long + 3 short + 1 trailing empty).
    // Walking backward:
    //   line 6 (empty): 1 row (accum=1)
    //   line 5 ("short3"): 1 row (accum=2)
    //   line 4 ("short2"): 1 row (accum=3)
    //   line 3 ("short1"): 1 row (accum=4)
    //   line 2 (40 chars): 4 rows → accum would be 8 > 6, stop.
    // lines_from_end = 4, max_scroll = 7 - 4 = 3.
    // Lines 3-6 take 4 visual rows, all fit in 6 rows. All short lines visible.
    let max_scroll = app.compute_max_scroll();
    assert_eq!(max_scroll, 3, "short trailing lines must be fully visible");

    // Verify pressing G shows the short lines.
    app.handle_event(make_key(KeyCode::Char('G')));
    assert_eq!(app.view().scroll, 3);
}

// ---------------------------------------------------------------
// Stage deletion tests
// ---------------------------------------------------------------

/// Deleting the only stage clears stage_outputs and stops running.
#[tokio::test]
async fn test_delete_last_stage_clears_outputs() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![make_stage_output("a\n")];
    app.running = true;

    // Press 'd' to delete the only stage.
    app.handle_event(make_key(KeyCode::Char('d')));

    assert!(app.pipeline.is_empty(), "pipeline should be empty");
    assert!(app.stage_outputs.is_empty(), "stage_outputs should be cleared");
    assert!(!app.running, "running should be false after deleting all stages");
}

/// After deleting all stages, a stale StageUpdate message must not
/// re-populate stage_outputs (the output pane must stay blank).
#[tokio::test]
async fn test_stale_stage_update_ignored_when_pipeline_empty() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![make_stage_output("a\n")];

    // Delete the only stage.
    app.handle_event(make_key(KeyCode::Char('d')));
    assert!(app.pipeline.is_empty());
    assert!(app.stage_outputs.is_empty());

    // Simulate a stale StreamMsg::StageUpdate arriving from the old execution.
    let stale = StreamMsg::StageUpdate {
        stage_idx: 0,
        new_stdout: b"old output\n".to_vec(),
        new_stderr: vec![],
        new_combined: vec![],
    };
    app.handle_stream_msg(stale);

    // stage_outputs must remain empty; old data must not appear.
    assert!(
        app.stage_outputs.is_empty(),
        "stale StageUpdate must not populate stage_outputs after all stages are deleted"
    );
    assert_eq!(app.current_output_text(), "");
}

/// Deleting the last stage via the confirm-delete path also clears outputs
/// and stops the running flag.
#[tokio::test]
async fn test_confirm_delete_then_immediate_delete_clears_outputs() {
    let pipeline = parse_pipeline("echo a | echo b");
    let mut app = App::new(pipeline);

    app.stage_outputs = vec![make_stage_output("a\n"), make_stage_output("b\n")];
    app.pipeline.selected = 0; // select first (non-last) stage

    // Delete first stage — goes to ConfirmingDelete mode.
    app.handle_event(make_key(KeyCode::Char('d')));
    assert!(matches!(app.mode, AppMode::ConfirmingDelete));

    // Confirm the delete with 'y'.
    app.handle_event(make_key(KeyCode::Char('y')));
    assert_eq!(app.pipeline.len(), 1, "one stage should remain after confirm delete");
    assert!(app.running, "exec should be triggered for the remaining stage");

    // Now delete the last remaining stage (immediate delete, no confirm).
    app.handle_event(make_key(KeyCode::Char('d')));

    assert!(app.pipeline.is_empty(), "pipeline should be empty");
    assert!(app.stage_outputs.is_empty(), "stage_outputs should be cleared");
    assert!(!app.running, "running should be false after deleting all stages");
}

fn make_ctrl_z() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL))
}

// ---------------------------------------------------------------
// Undo (Ctrl+Z) tests
// ---------------------------------------------------------------

/// Ctrl+Z with no history does nothing.
#[tokio::test]
async fn test_undo_with_no_history_is_noop() {
    let pipeline = parse_pipeline("echo a | echo b");
    let mut app = App::new(pipeline);
    app.pipeline.selected = 1;

    app.handle_event(make_ctrl_z());

    // Pipeline should be unchanged.
    assert_eq!(app.pipeline.len(), 2);
    assert_eq!(app.pipeline.selected, 1);
}

/// Ctrl+Z after editing a stage command restores the original command.
#[tokio::test]
async fn test_undo_edit_restores_original_command() {
    let pipeline = parse_pipeline("echo a | echo b");
    let mut app = App::new(pipeline);
    app.pipeline.selected = 1;

    // Simulate editing stage 1: confirm_edit saves to history if changed.
    app.mode = AppMode::Editing {
        editor: EditorState::new("echo CHANGED".to_string()),
        pending_new_stage: false,
    };
    app.confirm_edit();

    assert_eq!(app.pipeline.stages[1].command, "echo CHANGED");

    // Undo should restore "echo b".
    app.handle_event(make_ctrl_z());

    assert_eq!(
        app.pipeline.stages[1].command, "echo b",
        "undo should restore the original command"
    );
    assert_eq!(app.pipeline.len(), 2);
}

/// Ctrl+Z after confirming an edit with no change does not add a history entry.
#[tokio::test]
async fn test_undo_noop_edit_does_not_save_history() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    // Confirm edit with same content → no history entry.
    app.mode = AppMode::Editing {
        editor: EditorState::new("echo a".to_string()),
        pending_new_stage: false,
    };
    app.confirm_edit();

    // Undo should have no history to restore.
    app.handle_event(make_ctrl_z());

    assert_eq!(app.pipeline.stages[0].command, "echo a");
    assert_eq!(app.pipeline.len(), 1);
}

/// Ctrl+Z after an immediate delete (last/only stage) restores the pipeline.
#[tokio::test]
async fn test_undo_immediate_delete_restores_stage() {
    let pipeline = parse_pipeline("echo a | echo b");
    let mut app = App::new(pipeline);
    app.pipeline.selected = 1; // select last stage

    app.handle_event(make_key(KeyCode::Char('d')));
    assert_eq!(app.pipeline.len(), 1, "last stage should be deleted immediately");

    // Undo should restore the deleted stage.
    app.handle_event(make_ctrl_z());

    assert_eq!(app.pipeline.len(), 2, "undo should restore the deleted stage");
    assert_eq!(app.pipeline.stages[1].command, "echo b");
    assert_eq!(app.pipeline.selected, 1);
}

/// Ctrl+Z after a confirmed delete restores the pipeline.
#[tokio::test]
async fn test_undo_confirmed_delete_restores_stage() {
    let pipeline = parse_pipeline("echo a | echo b | echo c");
    let mut app = App::new(pipeline);
    app.pipeline.selected = 0; // select non-last stage → requires confirmation

    app.handle_event(make_key(KeyCode::Char('d')));
    assert!(matches!(app.mode, AppMode::ConfirmingDelete));

    app.handle_event(make_key(KeyCode::Char('y')));
    assert_eq!(app.pipeline.len(), 2, "first stage should be deleted after confirmation");

    // Undo should restore the deleted stage.
    app.handle_event(make_ctrl_z());

    assert_eq!(app.pipeline.len(), 3, "undo should restore the deleted stage");
    assert_eq!(app.pipeline.stages[0].command, "echo a");
    assert_eq!(app.pipeline.stages[1].command, "echo b");
}

/// Ctrl+Z after inserting a new stage removes it.
#[tokio::test]
async fn test_undo_insert_removes_new_stage() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);
    app.pipeline.selected = 0;

    // Press 'o' to insert a new stage.
    app.handle_event(make_key(KeyCode::Char('o')));
    assert_eq!(app.pipeline.len(), 2, "inserting should add a new stage");
    assert!(matches!(app.mode, AppMode::Editing { .. }));

    // Confirm the edit with a command.
    app.mode = AppMode::Editing {
        editor: EditorState::new("grep foo".to_string()),
        pending_new_stage: true,
    };
    app.confirm_edit();
    assert_eq!(app.pipeline.len(), 2);
    assert_eq!(app.pipeline.stages[1].command, "grep foo");

    // Undo should remove the inserted stage entirely.
    app.handle_event(make_ctrl_z());

    assert_eq!(app.pipeline.len(), 1, "undo should remove the inserted stage");
    assert_eq!(app.pipeline.stages[0].command, "echo a");
}

/// Ctrl+Z after inserting and cancelling the edit also undoes the insert.
#[tokio::test]
async fn test_undo_insert_cancelled_restores_pipeline() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);
    app.pipeline.selected = 0;

    // Press 'o' to insert a new stage (saves history), then cancel.
    app.handle_event(make_key(KeyCode::Char('o')));
    assert_eq!(app.pipeline.len(), 2);
    app.cancel_edit(); // removes the pending empty stage
    assert_eq!(app.pipeline.len(), 1, "cancel should remove the pending stage");

    // Undo should still restore (to the same single-stage pipeline).
    app.handle_event(make_ctrl_z());
    assert_eq!(app.pipeline.len(), 1);
    assert_eq!(app.pipeline.stages[0].command, "echo a");
}

/// Multiple undos traverse the history stack sequentially.
#[tokio::test]
async fn test_undo_multiple_times() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);
    app.pipeline.selected = 0;

    // Edit 1: "echo a" → "echo b"
    app.mode = AppMode::Editing {
        editor: EditorState::new("echo b".to_string()),
        pending_new_stage: false,
    };
    app.confirm_edit();
    assert_eq!(app.pipeline.stages[0].command, "echo b");

    // Edit 2: "echo b" → "echo c"
    app.mode = AppMode::Editing {
        editor: EditorState::new("echo c".to_string()),
        pending_new_stage: false,
    };
    app.confirm_edit();
    assert_eq!(app.pipeline.stages[0].command, "echo c");

    // First undo → "echo b"
    app.handle_event(make_ctrl_z());
    assert_eq!(app.pipeline.stages[0].command, "echo b");

    // Second undo → "echo a"
    app.handle_event(make_ctrl_z());
    assert_eq!(app.pipeline.stages[0].command, "echo a");

    // Third undo → nothing left in history, stays at "echo a"
    app.handle_event(make_ctrl_z());
    assert_eq!(app.pipeline.stages[0].command, "echo a");
}

/// Deleting all stages and then undoing restores the pipeline.
#[tokio::test]
async fn test_undo_delete_only_stage_restores_pipeline() {
    let pipeline = parse_pipeline("echo hello");
    let mut app = App::new(pipeline);

    app.handle_event(make_key(KeyCode::Char('d')));
    assert!(app.pipeline.is_empty(), "only stage should be deleted immediately");

    app.handle_event(make_ctrl_z());

    assert_eq!(app.pipeline.len(), 1, "undo should restore the only stage");
    assert_eq!(app.pipeline.stages[0].command, "echo hello");
}

fn make_ctrl_shift_z() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::CONTROL))
}

// ---------------------------------------------------------------
// Redo (Ctrl+Shift+Z) tests
// ---------------------------------------------------------------

/// Ctrl+Shift+Z with no redo history is a no-op.
#[tokio::test]
async fn test_redo_with_no_history_is_noop() {
    let pipeline = parse_pipeline("echo a | echo b");
    let mut app = App::new(pipeline);

    app.handle_event(make_ctrl_shift_z());

    assert_eq!(app.pipeline.len(), 2);
    assert_eq!(app.pipeline.stages[0].command, "echo a");
}

/// Undo followed by redo restores the modified state.
#[tokio::test]
async fn test_redo_after_undo_restores_modified_state() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    // Edit: "echo a" → "echo b"
    app.mode = AppMode::Editing {
        editor: EditorState::new("echo b".to_string()),
        pending_new_stage: false,
    };
    app.confirm_edit();
    assert_eq!(app.pipeline.stages[0].command, "echo b");

    // Undo → "echo a"
    app.handle_event(make_ctrl_z());
    assert_eq!(app.pipeline.stages[0].command, "echo a");

    // Redo → "echo b"
    app.handle_event(make_ctrl_shift_z());
    assert_eq!(app.pipeline.stages[0].command, "echo b");
}

/// Making a new change after undo clears the redo stack.
#[tokio::test]
async fn test_new_change_after_undo_clears_redo_stack() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    // Edit 1: → "echo b"
    app.mode = AppMode::Editing {
        editor: EditorState::new("echo b".to_string()),
        pending_new_stage: false,
    };
    app.confirm_edit();

    // Undo → "echo a"
    app.handle_event(make_ctrl_z());
    assert_eq!(app.pipeline.stages[0].command, "echo a");

    // New edit (different from redo): "echo a" → "echo c"
    app.mode = AppMode::Editing {
        editor: EditorState::new("echo c".to_string()),
        pending_new_stage: false,
    };
    app.confirm_edit();
    assert_eq!(app.pipeline.stages[0].command, "echo c");

    // Redo should be a no-op (redo stack was cleared by the new edit).
    app.handle_event(make_ctrl_shift_z());
    assert_eq!(
        app.pipeline.stages[0].command, "echo c",
        "redo should be no-op after a new change"
    );
}

/// Multiple undo/redo cycles traverse history correctly.
#[tokio::test]
async fn test_undo_redo_multiple_cycles() {
    let pipeline = parse_pipeline("echo a");
    let mut app = App::new(pipeline);

    // Three sequential edits.
    for cmd in &["echo b", "echo c", "echo d"] {
        app.mode = AppMode::Editing {
            editor: EditorState::new(cmd.to_string()),
            pending_new_stage: false,
        };
        app.confirm_edit();
    }
    assert_eq!(app.pipeline.stages[0].command, "echo d");

    // Undo twice → "echo b"
    app.handle_event(make_ctrl_z());
    assert_eq!(app.pipeline.stages[0].command, "echo c");
    app.handle_event(make_ctrl_z());
    assert_eq!(app.pipeline.stages[0].command, "echo b");

    // Redo once → "echo c"
    app.handle_event(make_ctrl_shift_z());
    assert_eq!(app.pipeline.stages[0].command, "echo c");

    // Redo again → "echo d"
    app.handle_event(make_ctrl_shift_z());
    assert_eq!(app.pipeline.stages[0].command, "echo d");

    // Redo with nothing left → stays at "echo d"
    app.handle_event(make_ctrl_shift_z());
    assert_eq!(app.pipeline.stages[0].command, "echo d");
}

/// Undo of a delete followed by redo re-deletes the stage.
#[tokio::test]
async fn test_undo_redo_delete() {
    let pipeline = parse_pipeline("echo a | echo b");
    let mut app = App::new(pipeline);
    app.pipeline.selected = 1;

    // Delete last stage.
    app.handle_event(make_key(KeyCode::Char('d')));
    assert_eq!(app.pipeline.len(), 1);

    // Undo: restores both stages.
    app.handle_event(make_ctrl_z());
    assert_eq!(app.pipeline.len(), 2);
    assert_eq!(app.pipeline.stages[1].command, "echo b");

    // Redo: re-deletes.
    app.handle_event(make_ctrl_shift_z());
    assert_eq!(app.pipeline.len(), 1);
    assert_eq!(app.pipeline.stages[0].command, "echo a");
}

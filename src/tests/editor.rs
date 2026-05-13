use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;

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
    #[rustfmt::skip]
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
    #[rustfmt::skip]
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
    #[rustfmt::skip]
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
    #[rustfmt::skip]
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

// --- Ctrl+K / Ctrl+U / Ctrl+Y kill-yank tests ---

#[test]
fn test_editor_ctrl_k_kills_to_end() {
    let mut editor = EditorState::new("hello world".to_string());
    editor.cursor = 5;
    let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
    editor.handle_key(key);
    assert_eq!(editor.content, "hello");
    assert_eq!(editor.cursor, 5);
    assert_eq!(editor.cut_buffer, " world");
}

#[test]
fn test_editor_ctrl_k_at_end_kills_nothing() {
    let mut editor = EditorState::new("hello".to_string());
    editor.cursor = 5;
    let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
    editor.handle_key(key);
    assert_eq!(editor.content, "hello");
    assert_eq!(editor.cursor, 5);
    assert_eq!(editor.cut_buffer, "");
}

#[test]
fn test_editor_ctrl_u_kills_to_beginning() {
    let mut editor = EditorState::new("hello world".to_string());
    editor.cursor = 5;
    let key = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
    editor.handle_key(key);
    assert_eq!(editor.content, " world");
    assert_eq!(editor.cursor, 0);
    assert_eq!(editor.cut_buffer, "hello");
}

#[test]
fn test_editor_ctrl_u_at_beginning_kills_nothing() {
    let mut editor = EditorState::new("hello".to_string());
    editor.cursor = 0;
    let key = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
    editor.handle_key(key);
    assert_eq!(editor.content, "hello");
    assert_eq!(editor.cursor, 0);
    assert_eq!(editor.cut_buffer, "");
}

#[test]
fn test_editor_ctrl_y_pastes_after_ctrl_k() {
    let mut editor = EditorState::new("hello world".to_string());
    editor.cursor = 5;
    // Kill to end
    editor.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert_eq!(editor.content, "hello");
    // Move cursor to beginning
    editor.cursor = 0;
    // Yank
    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(editor.content, " worldhello");
    assert_eq!(editor.cursor, 6);
}

#[test]
fn test_editor_ctrl_y_pastes_after_ctrl_u() {
    let mut editor = EditorState::new("hello world".to_string());
    editor.cursor = 5;
    // Kill to beginning
    editor.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(editor.content, " world");
    assert_eq!(editor.cursor, 0);
    // Move cursor to end
    editor.cursor = editor.content.len();
    // Yank
    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(editor.content, " worldhello");
    assert_eq!(editor.cursor, 11);
}

#[test]
fn test_editor_ctrl_y_with_empty_buffer_does_nothing() {
    let mut editor = EditorState::new("hello".to_string());
    editor.cursor = 3;
    // Yank without prior kill — buffer is empty
    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(editor.content, "hello");
    assert_eq!(editor.cursor, 3);
}

#[test]
fn test_editor_ctrl_k_then_ctrl_k_overwrites_buffer() {
    let mut editor = EditorState::new("abc def ghi".to_string());
    editor.cursor = 4;
    // First kill: saves "def ghi"
    editor.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert_eq!(editor.cut_buffer, "def ghi");
    // Reset content and cursor for a second kill
    editor.content = "xyz 123".to_string();
    editor.cursor = 4;
    editor.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    // Buffer should now contain the second kill, not the first
    assert_eq!(editor.cut_buffer, "123");
    assert_eq!(editor.content, "xyz ");
}

#[test]
fn test_editor_ctrl_y_can_paste_multiple_times() {
    let mut editor = EditorState::new("hello world".to_string());
    editor.cursor = 5;
    // Kill " world"
    editor.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert_eq!(editor.content, "hello");
    // Yank at end
    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(editor.content, "hello world");
    // Yank again at end
    editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(editor.content, "hello world world");
}

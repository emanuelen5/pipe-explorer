use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Result of [`EditorState::handle_key`].
///
/// Common editing keys (movement, insertion, deletion) are handled internally
/// and return [`Handled`](EditorKeyResult::Handled).  Action keys that depend
/// on the context (Enter, Esc, Tab, …) are returned as
/// [`Unhandled`](EditorKeyResult::Unhandled) so that each call-site can
/// provide its own behaviour — following the Dependency Inversion Principle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKeyResult {
    /// The key was consumed by the editor (text was modified or cursor moved).
    Handled,
    /// The key was **not** consumed — the caller should handle it.
    Unhandled,
}

/// Inline text-editor state (used for command editing and save-to-file dialogs).
#[derive(Debug, Clone)]
pub struct EditorState {
    /// The text being edited.
    pub content: String,
    /// Cursor position within `content` (byte index).
    pub cursor: usize,
    /// Horizontal scroll offset (in display columns).
    pub scroll_x: usize,
    /// Kill buffer for Ctrl+K / Ctrl+U / Ctrl+Y (yank) functionality.
    pub(crate) cut_buffer: String,
}

impl EditorState {
    /// Create a new editor pre-filled with `content`, cursor at the end.
    pub fn new(content: String) -> Self {
        let cursor = content.len();
        Self {
            content,
            cursor,
            scroll_x: 0,
            cut_buffer: String::new(),
        }
    }

    /// Create an empty editor.
    pub fn empty() -> Self {
        Self {
            content: String::new(),
            cursor: 0,
            scroll_x: 0,
            cut_buffer: String::new(),
        }
    }

    /// Adjust horizontal scroll so the cursor stays visible within `inner_width` columns.
    pub fn update_scroll(&mut self, inner_width: usize) {
        if inner_width == 0 {
            return;
        }
        self.scroll_x =
            compute_editor_scroll(self.scroll_x, &self.content[..self.cursor], inner_width);
    }

    /// Handle a key event that mutates the editor buffer (movement, insertion, deletion).
    ///
    /// Returns [`EditorKeyResult::Handled`] when the key was consumed (text
    /// editing or cursor movement).  Returns [`EditorKeyResult::Unhandled`]
    /// for action keys (Enter, Esc, Tab, etc.) that should be handled by
    /// the caller's context-specific logic.
    pub fn handle_key(&mut self, key: KeyEvent) -> EditorKeyResult {
        match key.code {
            // --- Action keys: delegate to caller ---
            KeyCode::Enter | KeyCode::Esc | KeyCode::Tab => return EditorKeyResult::Unhandled,
            // --- Common editing keys ---
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    // Find previous char boundary (handles multi-byte characters)
                    let prev = self.content[..self.cursor]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.content.remove(prev);
                    self.cursor = prev;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.content.len() {
                    self.content.remove(self.cursor);
                }
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let new_cursor = word_left_pos(&self.content[..self.cursor]);
                debug_assert!(self.content.is_char_boundary(new_cursor));
                self.cursor = new_cursor;
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let delta = word_right_pos(&self.content[self.cursor..]);
                let new_cursor = self.cursor + delta;
                debug_assert!(self.content.is_char_boundary(new_cursor));
                self.cursor = new_cursor;
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    let s = &self.content[..self.cursor];
                    self.cursor = s.char_indices().last().map(|(i, _)| i).unwrap_or(0);
                }
            }
            KeyCode::Right => {
                if self.cursor < self.content.len() {
                    let s = &self.content[self.cursor..];
                    let next = s
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.cursor + i)
                        .unwrap_or(self.content.len());
                    self.cursor = next;
                }
            }
            KeyCode::Home => {
                self.cursor = 0;
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = 0;
            }
            KeyCode::End => {
                self.cursor = self.content.len();
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.content.len();
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cut_buffer = self.content[self.cursor..].to_string();
                self.content.truncate(self.cursor);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cut_buffer = self.content[..self.cursor].to_string();
                self.content.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.cut_buffer.is_empty() {
                    self.content.insert_str(self.cursor, &self.cut_buffer);
                    self.cursor += self.cut_buffer.len();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.content.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            _ => return EditorKeyResult::Unhandled,
        }
        EditorKeyResult::Handled
    }
}

/// Return the byte offset within `before_cursor` where the previous word begins.
/// Skips trailing whitespace, then skips backwards over the word characters.
pub(crate) fn word_left_pos(before_cursor: &str) -> usize {
    let chars: Vec<(usize, char)> = before_cursor.char_indices().collect();
    let n = chars.len();
    let mut i = n;
    while i > 0 && chars[i - 1].1.is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].1.is_whitespace() {
        i -= 1;
    }
    if i == 0 { 0 } else { chars[i].0 }
}

/// Return the byte offset within `after_cursor` where the next word begins.
/// Skips forward over the current word characters, then over any whitespace.
pub(crate) fn word_right_pos(after_cursor: &str) -> usize {
    let mut iter = after_cursor.char_indices().peekable();
    while matches!(iter.peek(), Some((_, c)) if !c.is_whitespace()) {
        iter.next();
    }
    while matches!(iter.peek(), Some((_, c)) if c.is_whitespace()) {
        iter.next();
    }
    iter.peek().map(|(i, _)| *i).unwrap_or(after_cursor.len())
}

/// Compute the new horizontal scroll offset so `before_cursor` (text before the cursor)
/// keeps the cursor visible within a text area of `inner_width` columns.
///
/// * Scrolls right if the cursor column has moved past the right edge.
/// * Scrolls left  if the cursor column has moved before the left edge.
/// * Leaves `current_scroll_x` unchanged when the cursor is already in view.
pub(crate) fn compute_editor_scroll(
    current_scroll_x: usize,
    before_cursor: &str,
    inner_width: usize,
) -> usize {
    if inner_width == 0 {
        return current_scroll_x;
    }
    let cursor_col = before_cursor.chars().count();
    if cursor_col >= current_scroll_x + inner_width {
        cursor_col + 1 - inner_width
    } else if cursor_col < current_scroll_x {
        cursor_col
    } else {
        current_scroll_x
    }
}

#[cfg(test)]
#[path = "tests/editor.rs"]
mod tests;

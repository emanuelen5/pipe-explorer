use std::borrow::Cow;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::ansi::AnsiLineIndex;
pub use crate::executor::OutputMode;
use crate::executor::{ExecutorCache, StageOutput, StreamMsg, run_pipeline_streaming};
use crate::pipeline::Pipeline;
use crate::search::{SearchHistory, SearchState};
use crate::ui;

/// Per-stage view state (output mode, search, scroll position).
#[derive(Debug)]
pub struct StageViewState {
    pub output_mode: OutputMode,
    pub search: SearchState,
    pub scroll: usize,
}

impl Default for StageViewState {
    fn default() -> Self {
        Self {
            output_mode: OutputMode::Stdout,
            search: SearchState::default(),
            scroll: 0,
        }
    }
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
}

impl EditorState {
    /// Create a new editor pre-filled with `content`, cursor at the end.
    pub fn new(content: String) -> Self {
        let cursor = content.len();
        Self {
            content,
            cursor,
            scroll_x: 0,
        }
    }

    /// Create an empty editor.
    pub fn empty() -> Self {
        Self {
            content: String::new(),
            cursor: 0,
            scroll_x: 0,
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
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let pos = self.cursor - 1;
                    self.content.remove(pos);
                    self.cursor = pos;
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
            KeyCode::End => {
                self.cursor = self.content.len();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.content.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            _ => {}
        }
    }
}

/// The current interaction mode of the application.
#[derive(Debug)]
pub enum AppMode {
    Normal,
    Editing {
        editor: EditorState,
        pending_new_stage: bool,
    },
    Saving(EditorState),
    ConfirmingDelete,
    Searching,
    /// Vim-style `:command` mode.
    Command(EditorState),
}

/// A request sent to the long-lived background executor task.
struct ExecRequest {
    commands: Vec<String>,
    up_to: usize,
    force: bool,
    output_modes: Vec<OutputMode>,
    cancel: Arc<AtomicBool>,
    result_tx: std_mpsc::Sender<StreamMsg>,
}

/// Full application state.
pub struct App {
    pub pipeline: Pipeline,
    pub mode: AppMode,
    /// Per-stage view state (output mode, search, scroll).
    pub stage_views: Vec<StageViewState>,
    /// Cached outputs per stage (only up to the currently selected stage).
    /// During streaming, entries are incrementally filled in.
    pub stage_outputs: Vec<StageOutput>,
    /// Error message to display (e.g. command failed).
    pub error_message: Option<String>,
    /// Is a command currently running in the background?
    pub running: bool,
    /// Show help overlay?
    pub show_help: bool,
    /// Cancellation token shared with the current execution.
    cancel_token: Arc<AtomicBool>,
    /// Sender for triggering background execution.
    exec_tx: mpsc::Sender<ExecRequest>,
    /// Receiver for streaming execution results.
    exec_rx: mpsc::Receiver<StreamMsg>,
    /// Number of visible lines in the output pane (updated each frame by the renderer).
    pub visible_output_lines: usize,
    /// Search history shared across all pipeline stages.
    pub search_history: SearchHistory,
}

/// Return the byte offset within `before_cursor` where the previous word begins.
/// Skips trailing whitespace, then skips backwards over the word characters.
fn word_left_pos(before_cursor: &str) -> usize {
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
fn word_right_pos(after_cursor: &str) -> usize {
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
fn compute_editor_scroll(
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

impl App {
    pub fn new(pipeline: Pipeline) -> Self {
        let (exec_tx, mut request_rx) = mpsc::channel::<ExecRequest>(8);
        // Dummy receiver — replaced by trigger_exec before the event loop polls.
        let (_dummy_tx, exec_rx) = mpsc::channel::<StreamMsg>(1);

        // Long-lived background task: owns the executor cache, processes
        // requests sequentially (cancelled requests exit almost instantly).
        tokio::spawn(async move {
            let mut cache = ExecutorCache::new();
            while let Some(req) = request_rx.recv().await {
                let mut c = cache;
                let (returned_cache,) = tokio::task::spawn_blocking(move || {
                    run_pipeline_streaming(
                        &mut c,
                        &req.commands,
                        req.up_to,
                        req.force,
                        &req.output_modes,
                        &req.cancel,
                        &req.result_tx,
                    );
                    (c,)
                })
                .await
                .expect("executor task panicked");
                cache = returned_cache;
            }
        });

        let stage_count = pipeline.len();
        Self {
            pipeline,
            mode: AppMode::Normal,
            stage_views: (0..stage_count)
                .map(|_| StageViewState::default())
                .collect(),
            stage_outputs: Vec::new(),
            error_message: None,
            running: false,
            show_help: false,
            cancel_token: Arc::new(AtomicBool::new(false)),
            exec_tx,
            exec_rx,
            visible_output_lines: 1, // Minimum value
            search_history: SearchHistory::default(),
        }
    }

    /// Trigger asynchronous execution of stages up to and including `selected`.
    ///
    /// Cancels any in-flight execution, creates a fresh result channel, and
    /// pre-fills `stage_outputs` with empty placeholders so incremental
    /// `StreamMsg::StageUpdate` messages can append data in-place.
    pub fn trigger_exec(&mut self, force: bool) {
        if self.pipeline.is_empty() {
            return;
        }

        // Cancel any in-flight execution.
        self.cancel_token.store(true, Ordering::Relaxed);

        // New cancel token for the new execution.
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_token = cancel.clone();

        // New result channel (drops old receiver, silencing stale messages).
        let (tokio_tx, tokio_rx) = mpsc::channel::<StreamMsg>(256);
        self.exec_rx = tokio_rx;

        // Pre-fill stage_outputs with empty entries for incremental building.
        let count = (self.pipeline.selected + 1).min(self.pipeline.len());
        self.stage_outputs = (0..count).map(|_| StageOutput::empty()).collect();
        self.running = true;
        self.error_message = None;

        let commands: Vec<String> = self
            .pipeline
            .stages
            .iter()
            .map(|s| s.command.clone())
            .collect();
        let up_to = self.pipeline.selected;
        let output_modes: Vec<OutputMode> =
            self.stage_views.iter().map(|v| v.output_mode).collect();

        // Create the sync channel for the executor.
        let (sync_tx, sync_rx) = std_mpsc::channel::<StreamMsg>();

        let request = ExecRequest {
            commands,
            up_to,
            force,
            output_modes,
            cancel,
            result_tx: sync_tx,
        };

        // Send request to the long-lived background task (fire-and-forget).
        let tx = self.exec_tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(request).await;
        });

        // Bridge: sync_rx → tokio_tx (runs in a blocking thread).
        tokio::task::spawn_blocking(move || {
            while let Ok(msg) = sync_rx.recv() {
                if tokio_tx.blocking_send(msg).is_err() {
                    break;
                }
            }
        });
    }

    /// Change the output mode of a given stage.
    ///
    /// When the mode changes for stage `stage_idx`, any downstream stage outputs
    /// are invalidated (truncated) because their stdin will differ.  The executor-
    /// level cache still keeps previous results keyed by `(command, sha256(stdin))`,
    /// so switching back to a previously-used mode will be a cache hit.
    pub fn set_output_mode(&mut self, stage_idx: usize, mode: OutputMode) {
        // Ensure stage_views is large enough.
        while self.stage_views.len() <= stage_idx {
            self.stage_views.push(StageViewState::default());
        }
        self.stage_views[stage_idx].output_mode = mode;
        self.stage_views[stage_idx].scroll = 0;

        // Invalidate downstream stage outputs: keep only up to stage_idx (inclusive).
        let keep = stage_idx + 1;
        if self.stage_outputs.len() > keep {
            self.stage_outputs.truncate(keep);
        }

        self.compute_search_matches();
    }

    /// Handle a streaming message from the background executor.
    /// Returns `true` if the UI should redraw.
    fn handle_stream_msg(&mut self, msg: StreamMsg) -> bool {
        match msg {
            StreamMsg::StageUpdate {
                stage_idx,
                new_stdout,
                new_stderr,
                new_combined,
            } => {
                // Grow stage_outputs if an earlier cached stage sent data
                // before the streaming stages fully initialised.
                while self.stage_outputs.len() <= stage_idx {
                    self.stage_outputs.push(StageOutput::empty());
                }
                if let Some(out) = self.stage_outputs.get_mut(stage_idx) {
                    out.append_data(&new_stdout, &new_stderr, new_combined);
                }
                true
            }
            StreamMsg::StageDone {
                stage_idx,
                exit_code,
            } => {
                if let Some(out) = self.stage_outputs.get_mut(stage_idx) {
                    out.exit_code = exit_code;
                }
                true
            }
            StreamMsg::AllDone { error } => {
                self.running = false;
                if let Some(err) = error {
                    if err != "cancelled" {
                        self.error_message = Some(err);
                    }
                }
                self.compute_search_matches();
                true
            }
        }
    }

    /// Get the current stage's view state (read-only).
    pub fn view(&self) -> &StageViewState {
        self.stage_views
            .get(self.pipeline.selected)
            .unwrap_or_else(|| {
                // Should not happen, but provide a safe fallback
                static DEFAULT: StageViewState = StageViewState {
                    output_mode: OutputMode::Stdout,
                    search: SearchState::empty(),
                    scroll: 0,
                };
                &DEFAULT
            })
    }

    /// Get the current stage's view state (mutable).
    pub fn view_mut(&mut self) -> &mut StageViewState {
        let idx = self.pipeline.selected;
        // Ensure vec is large enough
        while self.stage_views.len() <= idx {
            self.stage_views.push(StageViewState::default());
        }
        &mut self.stage_views[idx]
    }

    /// Return the text to display in the output pager.
    ///
    /// Returns a `Cow<str>` to avoid allocating when possible (stdout/stderr
    /// are borrowed directly from the byte buffers when they're valid UTF-8).
    pub fn current_output_text(&self) -> Cow<'_, str> {
        if self.pipeline.is_empty() || self.stage_outputs.is_empty() {
            return Cow::Borrowed("");
        }
        let idx = self
            .pipeline
            .selected
            .min(self.stage_outputs.len().saturating_sub(1));
        let out = &self.stage_outputs[idx];
        match self.view().output_mode {
            OutputMode::Stdout => Cow::Borrowed(out.stdout_text()),
            OutputMode::Stderr => Cow::Borrowed(out.stderr_text()),
            OutputMode::Combined => Cow::Owned(
                out.combined
                    .iter()
                    .map(|l| String::from_utf8_lossy(&l.content))
                    .collect::<Vec<_>>()
                    .concat(),
            ),
        }
    }

    /// Get the pre-built ANSI line index for the current output.
    /// Returns `None` for Combined mode or when no output exists.
    pub fn current_line_index(&self) -> Option<&AnsiLineIndex> {
        if self.pipeline.is_empty() || self.stage_outputs.is_empty() {
            return None;
        }
        let idx = self
            .pipeline
            .selected
            .min(self.stage_outputs.len().saturating_sub(1));
        self.stage_outputs[idx].line_index(self.view().output_mode)
    }

    /// Returns a vector of booleans (one per line of `current_output_text()`) indicating
    /// whether each line originated from stderr.  Only meaningful in Combined mode;
    /// returns an empty vec otherwise.
    pub fn combined_stderr_map(&self) -> Vec<bool> {
        if self.pipeline.is_empty() || self.stage_outputs.is_empty() {
            return vec![];
        }
        let idx = self
            .pipeline
            .selected
            .min(self.stage_outputs.len().saturating_sub(1));
        self.stage_outputs[idx]
            .combined
            .iter()
            .map(|cl| cl.is_stderr)
            .collect()
    }

    /// Number of display lines in current output (zero-allocation).
    pub fn output_line_count(&self) -> usize {
        if self.pipeline.is_empty() || self.stage_outputs.is_empty() {
            return 0;
        }
        let idx = self
            .pipeline
            .selected
            .min(self.stage_outputs.len().saturating_sub(1));
        self.stage_outputs[idx].display_line_count(self.view().output_mode)
    }

    /// Scroll down by `n` lines.
    pub fn scroll_down(&mut self, n: usize) {
        let total = self.output_line_count();
        let max_scroll = total.saturating_sub(self.visible_output_lines);
        let scroll = &mut self.view_mut().scroll;
        *scroll = (*scroll + n).min(max_scroll);
    }

    /// Scroll up by `n` lines.
    pub fn scroll_up(&mut self, n: usize) {
        let scroll = &mut self.view_mut().scroll;
        *scroll = scroll.saturating_sub(n);
    }

    /// Save current output text to a file.
    pub fn save_output(&self, path: &str) -> Result<()> {
        let content = self.current_output_text();
        std::fs::write(path, content.as_bytes())?;
        Ok(())
    }

    /// (Re-)compute search matches for the current output. Resets match index to 0.
    pub fn compute_search_matches(&mut self) {
        let content = crate::ansi::strip_ansi_sgr(&self.current_output_text());
        self.view_mut().search.compute(&content);
    }

    /// Enter search mode, clearing any previous query.
    pub fn start_search(&mut self) {
        self.view_mut().search.clear();
        self.search_history.reset_navigation();
        self.mode = AppMode::Searching;
    }

    /// Confirm the search query and compute matches.
    pub fn confirm_search(&mut self) {
        let query = self.view().search.query.clone();
        self.search_history.push(&query);
        self.compute_search_matches();
        self.mode = AppMode::Normal;
        // Jump to the first match at or after the current scroll position,
        // so the search starts from where the user is looking rather than
        // from the top of the buffer.
        let scroll = self.view().scroll;
        let matches = &self.view().search.matches;
        if !matches.is_empty() {
            // matches is sorted by line — find first match with line >= scroll.
            let idx = matches.partition_point(|&(line, _, _)| line < scroll);
            // If no match at/after scroll, wrap to first match.
            let idx = if idx >= matches.len() { 0 } else { idx };
            self.view_mut().search.match_idx = idx;
            let (line, _, _) = self.view().search.matches[idx];
            self.view_mut().scroll = line;
        }
    }

    /// Cancel search and clear all highlights.
    pub fn cancel_search(&mut self) {
        self.view_mut().search.clear();
        self.search_history.reset_navigation();
        self.mode = AppMode::Normal;
    }

    /// Advance to the next search match relative to the current scroll position.
    ///
    /// If the current match is still on the scroll line (user hasn't scrolled
    /// away), simply step to `match_idx + 1` so that multiple matches on the
    /// same line are visited one by one.  If the user has scrolled away, jump
    /// to the first match *after* the current scroll position.
    pub fn search_next(&mut self) {
        let view = self.view_mut();
        if view.search.matches.is_empty() {
            return;
        }
        let scroll = view.scroll;
        let cur_line = view.search.matches[view.search.match_idx].0;

        if cur_line == scroll {
            // Still on the same line as the current match — step sequentially.
            view.search.match_idx = (view.search.match_idx + 1) % view.search.matches.len();
        } else {
            // User scrolled away — binary search for first match after scroll.
            let idx = view
                .search
                .matches
                .partition_point(|&(line, _, _)| line <= scroll);
            view.search.match_idx = if idx < view.search.matches.len() {
                idx
            } else {
                0 // wrap
            };
        }
        let (line, _, _) = view.search.matches[view.search.match_idx];
        view.scroll = line;
    }

    /// Go back to the previous search match relative to the current scroll position.
    ///
    /// Same logic as `search_next` but in reverse: sequential step when on
    /// the current match's line, binary search when the user has scrolled away.
    pub fn search_prev(&mut self) {
        let view = self.view_mut();
        if view.search.matches.is_empty() {
            return;
        }
        let scroll = view.scroll;
        let cur_line = view.search.matches[view.search.match_idx].0;

        if cur_line == scroll {
            // Still on the same line — step sequentially backwards.
            if view.search.match_idx == 0 {
                view.search.match_idx = view.search.matches.len() - 1;
            } else {
                view.search.match_idx -= 1;
            }
        } else {
            // User scrolled away — find last match before scroll.
            let idx = view
                .search
                .matches
                .partition_point(|&(line, _, _)| line < scroll);
            view.search.match_idx = if idx > 0 {
                idx - 1
            } else {
                view.search.matches.len() - 1 // wrap
            };
        }
        let (line, _, _) = view.search.matches[view.search.match_idx];
        view.scroll = line;
    }

    /// Start editing the current stage's command.
    pub fn start_editing(&mut self) {
        let cmd = self
            .pipeline
            .selected_stage()
            .map(|s| s.command.clone())
            .unwrap_or_default();
        self.mode = AppMode::Editing {
            editor: EditorState::new(cmd),
            pending_new_stage: false,
        };
    }

    pub fn update_edit(&mut self) {
        // Use the current text and execute it in the stage, without closing the editor
        if let AppMode::Editing { editor, .. } = &mut self.mode {
            if let Some(stage) = self.pipeline.selected_stage_mut() {
                stage.command = editor.content.clone();
            }
        }
        self.trigger_exec(false);
    }

    /// Confirm an edit and update the pipeline stage.
    pub fn confirm_edit(&mut self) {
        let mut is_modified = false;
        if let AppMode::Editing { editor, .. } = std::mem::replace(&mut self.mode, AppMode::Normal)
        {
            if let Some(stage) = self.pipeline.selected_stage_mut() {
                is_modified = stage.command != editor.content;
                stage.command = editor.content;
            }
        }
        if is_modified {
            self.trigger_exec(false);
        }
    }

    /// Cancel editing.
    pub fn cancel_edit(&mut self) {
        let pending = matches!(
            self.mode,
            AppMode::Editing {
                pending_new_stage: true,
                ..
            }
        );
        if pending {
            let removed_idx = self.pipeline.selected;
            self.pipeline.remove_selected();
            self.remove_stage_view(removed_idx);
        }
        self.mode = AppMode::Normal;
    }

    /// Start the save-to-file dialog.
    pub fn start_saving(&mut self) {
        self.mode = AppMode::Saving(EditorState::empty());
    }

    /// Confirm saving output to a file.
    pub fn confirm_save(&mut self) {
        if let AppMode::Saving(editor) = std::mem::replace(&mut self.mode, AppMode::Normal) {
            if !editor.content.is_empty() {
                if let Err(e) = self.save_output(&editor.content) {
                    self.error_message = Some(format!("Save failed: {}", e));
                }
            }
        }
    }

    /// Return a mutable reference to the active `EditorState`, if any.
    pub fn editor_mut(&mut self) -> Option<&mut EditorState> {
        match &mut self.mode {
            AppMode::Editing { editor, .. } => Some(editor),
            AppMode::Saving(editor) => Some(editor),
            AppMode::Command(editor) => Some(editor),
            _ => None,
        }
    }

    /// Adjust the horizontal scroll of the editor so the cursor remains visible.
    ///
    /// `inner_width` is the number of visible columns in the editor text area
    /// (dialog width minus left/right borders).
    pub fn update_editor_scroll(&mut self, inner_width: usize) {
        if let Some(editor) = self.editor_mut() {
            editor.update_scroll(inner_width);
        }
    }

    /// Handle a single keyboard key event in Normal mode.
    fn handle_normal_key(&mut self, key: KeyEvent) -> bool {
        let quit = match key.code {
            KeyCode::Char('q') => true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
            _ => false,
        };
        if quit {
            return true; // signal quit
        }

        match key.code {
            // Navigation between stages
            KeyCode::Right | KeyCode::Char('l') => {
                self.pipeline.select_next();
                // Only trigger execution if this stage's output isn't already cached.
                if self.pipeline.selected >= self.stage_outputs.len() {
                    self.trigger_exec(false);
                }
                self.compute_search_matches();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.pipeline.select_prev();
                // Only trigger execution if this stage's output isn't already cached.
                if self.pipeline.selected >= self.stage_outputs.len() {
                    self.trigger_exec(false);
                }
                self.compute_search_matches();
            }

            // Editing
            KeyCode::Char('e') => {
                if !self.pipeline.is_empty() {
                    self.start_editing();
                }
            }

            // Add new stage
            KeyCode::Char('o') => {
                self.pipeline.insert_after_selected();
                self.sync_stage_views();
                self.start_editing();
                if let AppMode::Editing {
                    pending_new_stage, ..
                } = &mut self.mode
                {
                    *pending_new_stage = true;
                }
            }

            // Search forward/back
            KeyCode::Char('n') => {
                self.search_next();
            }
            KeyCode::Char('N') => {
                self.search_prev();
            }

            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_down(20)
            }
            // Delete stage
            KeyCode::Char('d') | KeyCode::Delete => {
                if !self.pipeline.is_empty() {
                    let is_last = self.pipeline.selected == self.pipeline.len() - 1;
                    if is_last || self.pipeline.len() == 1 {
                        // Last stage or only stage: delete immediately
                        let removed_idx = self.pipeline.selected;
                        self.pipeline.remove_selected();
                        self.remove_stage_view(removed_idx);
                        if !self.pipeline.is_empty() {
                            self.trigger_exec(false);
                        } else {
                            self.stage_outputs.clear();
                        }
                    } else {
                        // Not at the end: show confirmation prompt
                        self.mode = AppMode::ConfirmingDelete;
                    }
                }
            }

            // Rerun
            KeyCode::Char('r') => {
                self.trigger_exec(true);
            }

            // Save
            KeyCode::Char('s') => {
                self.start_saving();
            }

            // Output mode
            KeyCode::Char('m') => {
                let idx = self.pipeline.selected;
                let new_mode = match self.view().output_mode {
                    OutputMode::Stdout => OutputMode::Stderr,
                    OutputMode::Stderr => OutputMode::Combined,
                    OutputMode::Combined => OutputMode::Stdout,
                };
                self.set_output_mode(idx, new_mode);
            }
            KeyCode::Char('1') => {
                let idx = self.pipeline.selected;
                self.set_output_mode(idx, OutputMode::Stdout);
            }
            KeyCode::Char('2') => {
                let idx = self.pipeline.selected;
                self.set_output_mode(idx, OutputMode::Stderr);
            }
            KeyCode::Char('3') => {
                let idx = self.pipeline.selected;
                self.set_output_mode(idx, OutputMode::Combined);
            }

            // Pager scrolling
            KeyCode::Char('j') | KeyCode::Down => self.scroll_down(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_up(1),
            KeyCode::PageDown | KeyCode::Char('f')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.scroll_down(20);
            }
            KeyCode::PageDown => self.scroll_down(20),
            KeyCode::PageUp | KeyCode::Char('b')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.scroll_up(20);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_up(20)
            }
            KeyCode::PageUp => self.scroll_up(20),
            KeyCode::Char('g') | KeyCode::Home => {
                self.view_mut().scroll = 0;
            }
            KeyCode::Char('G') | KeyCode::End => {
                let total = self.output_line_count();
                self.view_mut().scroll = total.saturating_sub(self.visible_output_lines);
                print!(
                    "total lines: {}, scroll set to: {}",
                    total,
                    self.view().scroll
                );
            }

            // Search
            KeyCode::Char('/') => {
                self.start_search();
            }

            // Clear active search
            KeyCode::Esc => {
                self.cancel_search();
            }

            // Help
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
            }

            // Enter command mode (vim-style ':')
            KeyCode::Char(':') => {
                self.mode = AppMode::Command(EditorState::empty());
            }

            _ => {}
        }
        false
    }

    /// Handle a key event in the inline editor.
    fn handle_editor_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => self.cancel_edit(),
            KeyCode::Tab => self.update_edit(),
            KeyCode::Enter => {
                if matches!(self.mode, AppMode::Editing { .. }) {
                    self.confirm_edit();
                } else if matches!(self.mode, AppMode::Saving(_)) {
                    self.confirm_save();
                }
            }
            _ => {
                if let Some(editor) = self.editor_mut() {
                    editor.handle_key(key);
                }
            }
        }
        false
    }

    /// Handle a key event in the delete confirmation prompt.
    fn handle_confirm_delete_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let removed_idx = self.pipeline.selected;
                self.pipeline.remove_selected();
                self.remove_stage_view(removed_idx);
                self.mode = AppMode::Normal;
                if !self.pipeline.is_empty() {
                    self.trigger_exec(false);
                } else {
                    self.stage_outputs.clear();
                }
            }
            _ => {
                // Any other key cancels the delete
                self.mode = AppMode::Normal;
            }
        }
        false
    }

    /// Handle a key event while entering a search query.
    fn handle_search_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => self.cancel_search(),
            KeyCode::Enter => self.confirm_search(),
            KeyCode::Up => {
                let current = self.view().search.query.clone();
                if let Some(text) = self.search_history.navigate_up(&current) {
                    let len = text.len();
                    let view = self.view_mut();
                    view.search.query = text;
                    view.search.cursor = len;
                }
            }
            KeyCode::Down => {
                let current = self.view().search.query.clone();
                if let Some(text) = self.search_history.navigate_down(&current) {
                    let len = text.len();
                    let view = self.view_mut();
                    view.search.query = text;
                    view.search.cursor = len;
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::ALT) => {
                if let Some(text) = self.search_history.revert_current() {
                    let len = text.len();
                    let view = self.view_mut();
                    view.search.query = text;
                    view.search.cursor = len;
                }
            }
            KeyCode::Backspace => {
                let view = self.view_mut();
                if view.search.cursor > 0 {
                    let pos = view.search.cursor - 1;
                    view.search.query.remove(pos);
                    view.search.cursor = pos;
                }
            }
            KeyCode::Delete => {
                let view = self.view_mut();
                if view.search.cursor < view.search.query.len() {
                    view.search.query.remove(view.search.cursor);
                }
            }
            KeyCode::Left => {
                let view = self.view_mut();
                if view.search.cursor > 0 {
                    let s = &view.search.query[..view.search.cursor];
                    view.search.cursor = s.char_indices().last().map(|(i, _)| i).unwrap_or(0);
                }
            }
            KeyCode::Right => {
                let view = self.view_mut();
                if view.search.cursor < view.search.query.len() {
                    let s_len = view.search.query.len();
                    let cursor = view.search.cursor;
                    let next = view.search.query[cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| cursor + i)
                        .unwrap_or(s_len);
                    view.search.cursor = next;
                }
            }
            KeyCode::Home => {
                self.view_mut().search.cursor = 0;
            }
            KeyCode::End => {
                let len = self.view().search.query.len();
                self.view_mut().search.cursor = len;
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let view = self.view_mut();
                view.search.query.insert(view.search.cursor, c);
                view.search.cursor += c.len_utf8();
            }
            _ => {}
        }
        false
    }

    /// Known commands supported in command mode.
    const KNOWN_COMMANDS: &'static [&'static str] = &["help", "quit"];

    /// Handle a key event while in command mode (`:` prompt).
    fn handle_command_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
            }
            KeyCode::Enter => {
                let cmd = if let AppMode::Command(ref editor) = self.mode {
                    editor.content.trim().to_string()
                } else {
                    String::new()
                };
                self.mode = AppMode::Normal;
                // Dispatch the command
                if cmd == "h" || cmd == "help" {
                    self.show_help = true;
                } else if cmd == "q" || cmd == "quit" {
                    return true;
                }
            }
            KeyCode::Tab => {
                // Tab-complete the current content against known commands.
                if let AppMode::Command(ref mut editor) = self.mode {
                    let completed = Self::KNOWN_COMMANDS
                        .iter()
                        .find(|&&cmd| cmd.starts_with(editor.content.as_str()))
                        .copied();
                    if let Some(cmd) = completed {
                        editor.content = cmd.to_string();
                        editor.cursor = cmd.len();
                    }
                }
            }
            _ => {
                if let Some(editor) = self.editor_mut() {
                    editor.handle_key(key);
                }
            }
        }
        false
    }

    /// Ensure stage_views has an entry for every pipeline stage.
    fn sync_stage_views(&mut self) {
        while self.stage_views.len() < self.pipeline.len() {
            self.stage_views.push(StageViewState::default());
        }
    }

    /// Remove a stage view at the given index.
    fn remove_stage_view(&mut self, idx: usize) {
        if idx < self.stage_views.len() {
            self.stage_views.remove(idx);
        }
    }

    /// Handle a terminal event. Returns `true` if the app should quit.
    pub fn handle_event(&mut self, event: Event) -> bool {
        // Close help on any key
        if self.show_help {
            self.show_help = false;
            return false;
        }

        match event {
            Event::Key(key) => {
                if matches!(self.mode, AppMode::Normal) {
                    self.handle_normal_key(key)
                } else if matches!(self.mode, AppMode::Editing { .. } | AppMode::Saving(_)) {
                    self.handle_editor_key(key)
                } else if matches!(self.mode, AppMode::ConfirmingDelete) {
                    self.handle_confirm_delete_key(key)
                } else if matches!(self.mode, AppMode::Command(_)) {
                    self.handle_command_key(key)
                } else {
                    self.handle_search_key(key)
                }
            }
            _ => false,
        }
    }
}

/// Width cap (columns) for the editor overlay dialog.
use crate::ui::EDITOR_DIALOG_MAX_WIDTH;
/// Width cap (columns) for the save dialog.
use crate::ui::SAVE_DIALOG_MAX_WIDTH;

/// Compute the inner text width for an editor dialog given the terminal width.
fn editor_inner_width(terminal_width: u16, dialog_max_width: u16) -> usize {
    let dialog_width = terminal_width.saturating_sub(4).min(dialog_max_width);
    dialog_width.saturating_sub(2) as usize
}

/// Run the TUI event loop.
pub async fn run(app: &mut App) -> Result<()> {
    let mut terminal = setup_terminal()?;

    // Ensure terminal is always restored, even on error or panic.
    let result = run_inner(app, &mut terminal).await;

    // Restore terminal unconditionally before propagating any error.
    restore_terminal(&mut terminal)?;
    result
}

async fn run_inner(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    // Trigger initial execution if there are stages.
    if !app.pipeline.is_empty() {
        app.trigger_exec(false);
    }

    let mut event_stream = EventStream::new();
    let mut dirty = true; // draw the initial frame

    #[allow(unused_assignments)]
    loop {
        // Redraw only when something changed.
        if dirty {
            if let Ok(size) = terminal.size() {
                let max_w = match &app.mode {
                    AppMode::Editing { .. } => Some(EDITOR_DIALOG_MAX_WIDTH),
                    AppMode::Saving(_) => Some(SAVE_DIALOG_MAX_WIDTH),
                    _ => None,
                };
                if let Some(w) = max_w {
                    let inner_w = editor_inner_width(size.width, w);
                    app.update_editor_scroll(inner_w);
                }
            }

            terminal.draw(|frame| {
                ui::render(frame, app);
            })?;
            dirty = false;
        }

        // Wait for either a terminal event or a streaming executor message.
        tokio::select! {
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        if app.handle_event(event) {
                            break;
                        }
                        dirty = true;
                    }
                    Some(Err(e)) => {
                        return Err(anyhow::anyhow!("{}", e));
                    }
                    None => break,
                }
            }
            Some(msg) = app.exec_rx.recv() => {
                dirty = app.handle_stream_msg(msg);
            }
        }
    }

    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
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

        // Max useful scroll = total_lines - visible_height = 10 - 3 = 7.
        // Scrolling down many more times should not push scroll beyond 7.
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
}

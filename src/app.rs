use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

pub use crate::executor::OutputMode;
use crate::executor::{ExecutorCache, StageOutput, StreamMsg, run_pipeline_streaming};
use crate::pipeline::Pipeline;
use crate::search::SearchState;
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
                    out.stdout.extend_from_slice(&new_stdout);
                    out.stderr.extend_from_slice(&new_stderr);
                    out.combined.extend(new_combined);
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
    pub fn current_output_text(&self) -> String {
        if self.pipeline.is_empty() || self.stage_outputs.is_empty() {
            return String::new();
        }
        let idx = self
            .pipeline
            .selected
            .min(self.stage_outputs.len().saturating_sub(1));
        let out = &self.stage_outputs[idx];
        match self.view().output_mode {
            OutputMode::Stdout => out.stdout_str(),
            OutputMode::Stderr => out.stderr_str(),
            OutputMode::Combined => out
                .combined
                .iter()
                .map(|l| String::from_utf8_lossy(&l.content).into_owned())
                .collect(),
        }
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

    /// Number of lines in current output.
    pub fn output_line_count(&self) -> usize {
        self.current_output_text().lines().count()
    }

    /// Scroll down by `n` lines.
    pub fn scroll_down(&mut self, n: usize) {
        let total = self.output_line_count();
        let scroll = &mut self.view_mut().scroll;
        *scroll = (*scroll + n).min(total.saturating_sub(1));
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
        self.mode = AppMode::Searching;
    }

    /// Confirm the search query and compute matches.
    pub fn confirm_search(&mut self) {
        self.compute_search_matches();
        self.mode = AppMode::Normal;
        // Scroll to the first match if any.
        let first_line = self.view().search.matches.first().map(|&(line, _, _)| line);
        if let Some(line) = first_line {
            self.view_mut().scroll = line;
        }
    }

    /// Cancel search and clear all highlights.
    pub fn cancel_search(&mut self) {
        self.view_mut().search.clear();
        self.mode = AppMode::Normal;
    }

    /// Advance to the next search match.
    pub fn search_next(&mut self) {
        let view = self.view_mut();
        if view.search.matches.is_empty() {
            return;
        }
        view.search.match_idx = (view.search.match_idx + 1) % view.search.matches.len();
        let (line, _, _) = view.search.matches[view.search.match_idx];
        view.scroll = line;
    }

    /// Go back to the previous search match.
    pub fn search_prev(&mut self) {
        let view = self.view_mut();
        if view.search.matches.is_empty() {
            return;
        }
        if view.search.match_idx == 0 {
            view.search.match_idx = view.search.matches.len() - 1;
        } else {
            view.search.match_idx -= 1;
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

    /// Confirm an edit and update the pipeline stage.
    pub fn confirm_edit(&mut self) {
        if let AppMode::Editing { editor, .. } = std::mem::replace(&mut self.mode, AppMode::Normal)
        {
            if let Some(stage) = self.pipeline.selected_stage_mut() {
                stage.command = editor.content;
            }
        }
        self.trigger_exec(false);
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

    /// Return a shared reference to the active `EditorState`, if any.
    #[allow(dead_code)]
    pub fn editor(&self) -> Option<&EditorState> {
        match &self.mode {
            AppMode::Editing { editor, .. } => Some(editor),
            AppMode::Saving(editor) => Some(editor),
            _ => None,
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
                self.view_mut().scroll = total.saturating_sub(1);
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
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
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
                if app.show_help {
                    ui::render_help(frame, frame.area());
                }
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
        StageOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: vec![],
            exit_code: Some(0),
            combined: vec![],
        }
    }

    fn make_error_stage_output(exit_code: i32) -> StageOutput {
        StageOutput {
            stdout: vec![],
            stderr: b"error".to_vec(),
            exit_code: Some(exit_code),
            combined: vec![],
        }
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
}

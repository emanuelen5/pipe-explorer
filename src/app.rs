use std::io;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::executor::{ExecutorCache, StageOutput, execute_pipeline_stages};
use crate::pipeline::Pipeline;
use crate::search::SearchState;
use crate::ui;

/// The display mode for stage output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Stdout,
    Stderr,
    Combined,
}

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
        self.scroll_x = compute_editor_scroll(
            self.scroll_x,
            &self.content[..self.cursor],
            inner_width,
        );
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
}

/// Messages sent from the background executor task to the main event loop.
#[derive(Debug)]
enum ExecMsg {
    Done {
        outputs: Vec<StageOutput>,
        error: Option<String>,
    },
}

/// Full application state.
pub struct App {
    pub pipeline: Pipeline,
    pub mode: AppMode,
    /// Per-stage view state (output mode, search, scroll).
    pub stage_views: Vec<StageViewState>,
    /// Cached outputs per stage (only up to the currently selected stage).
    pub stage_outputs: Vec<StageOutput>,
    /// Error message to display (e.g. command failed).
    pub error_message: Option<String>,
    /// Is a command currently running in the background?
    pub running: bool,
    /// Show help overlay?
    pub show_help: bool,
    /// The executor cache.
    cache: ExecutorCache,
    /// Sender for triggering background execution.
    exec_tx: mpsc::Sender<(Vec<String>, usize, bool)>,
    /// Receiver for execution results.
    exec_rx: mpsc::Receiver<ExecMsg>,
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
        let (exec_tx, mut inner_rx) = mpsc::channel::<(Vec<String>, usize, bool)>(8);
        let (inner_tx, exec_rx) = mpsc::channel::<ExecMsg>(8);

        // Spawn a background task that runs commands and sends results back.
        // The actual execution uses spawn_blocking to avoid starving the
        // tokio runtime with blocking process I/O.
        tokio::spawn(async move {
            let mut cache = ExecutorCache::new();
            while let Some((commands, up_to, force)) = inner_rx.recv().await {
                let mut c = cache;
                let (result, returned_cache) = tokio::task::spawn_blocking(move || {
                    let r = execute_pipeline_stages(&mut c, &commands, up_to, force);
                    (r, c)
                })
                .await
                .expect("executor task panicked");
                cache = returned_cache;
                let msg = match result {
                    Ok(outputs) => ExecMsg::Done {
                        outputs,
                        error: None,
                    },
                    Err(e) => ExecMsg::Done {
                        outputs: vec![],
                        error: Some(e.to_string()),
                    },
                };
                let _ = inner_tx.send(msg).await;
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
            cache: ExecutorCache::new(),
            exec_tx,
            exec_rx,
        }
    }

    /// Trigger asynchronous execution of stages up to and including `selected`.
    pub fn trigger_exec(&mut self, force: bool) {
        if self.pipeline.is_empty() {
            return;
        }
        let commands: Vec<String> = self
            .pipeline
            .stages
            .iter()
            .map(|s| s.command.clone())
            .collect();
        let up_to = self.pipeline.selected;
        let tx = self.exec_tx.clone();
        self.running = true;
        self.error_message = None;
        tokio::spawn(async move {
            let _ = tx.send((commands, up_to, force)).await;
        });
    }

    /// Poll for any completed execution result. Returns true if the UI should redraw.
    pub fn poll_exec_result(&mut self) -> bool {
        match self.exec_rx.try_recv() {
            Ok(ExecMsg::Done { outputs, error }) => {
                self.running = false;
                if let Some(err) = error {
                    self.error_message = Some(err);
                } else {
                    self.stage_outputs = outputs;
                    self.error_message = None;
                }
                self.view_mut().scroll = 0;
                self.compute_search_matches();
                true
            }
            Err(_) => false,
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
            OutputMode::Combined => {
                format!("{}{}", out.stdout_str(), out.stderr_str())
            }
        }
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
        let content = self.current_output_text();
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
        if let AppMode::Editing { editor, .. } =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        {
            if let Some(stage) = self.pipeline.selected_stage_mut() {
                stage.command = editor.content;
            }
        }
        // Invalidate downstream cache and re-run
        self.cache.clear();
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
        if let AppMode::Saving(editor) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        {
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
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.pipeline.select_next();
                // Only trigger execution if this stage's output isn't already cached.
                if self.pipeline.selected >= self.stage_outputs.len() {
                    self.trigger_exec(false);
                }
                self.compute_search_matches();
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.pipeline.select_prev();
                // Only trigger execution if this stage's output isn't already cached.
                if self.pipeline.selected >= self.stage_outputs.len() {
                    self.trigger_exec(false);
                }
                self.compute_search_matches();
            }

            // Editing
            KeyCode::Char('e') | KeyCode::Enter => {
                if !self.pipeline.is_empty() {
                    self.start_editing();
                }
            }

            // Add new stage (also always available via 'a'; 'n' navigates when search is active)
            KeyCode::Char('a') => {
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

            // 'n': add new stage when no search is active; next match when search is active
            KeyCode::Char('n') => {
                if !self.view().search.matches.is_empty() {
                    self.search_next();
                } else {
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
            }

            // Navigate to previous search match
            KeyCode::Char('p') => {
                if !self.view().search.matches.is_empty() {
                    self.search_prev();
                }
            }

            // Delete stage
            KeyCode::Char('d') => {
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
            KeyCode::Char('1') => {
                self.view_mut().output_mode = OutputMode::Stdout;
                self.view_mut().scroll = 0;
                self.compute_search_matches();
            }
            KeyCode::Char('2') => {
                self.view_mut().output_mode = OutputMode::Stderr;
                self.view_mut().scroll = 0;
                self.compute_search_matches();
            }
            KeyCode::Char('3') => {
                self.view_mut().output_mode = OutputMode::Combined;
                self.view_mut().scroll = 0;
                self.compute_search_matches();
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
    // Trigger initial execution if there are stages
    if !app.pipeline.is_empty() {
        app.trigger_exec(false);
    }

    let mut event_stream = EventStream::new();

    loop {
        // Poll for completed background execution
        let exec_done = app.poll_exec_result();

        // Keep editor horizontal scroll in sync with the cursor before drawing.
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

        // Draw the UI
        terminal.draw(|frame| {
            ui::render(frame, app);
            if app.show_help {
                ui::render_help(frame, frame.area());
            }
        })?;

        // Wait for the next event (with a short timeout so we can poll exec results)
        tokio::select! {
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        if app.handle_event(event) {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        return Err(anyhow::anyhow!("{}", e));
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(50)), if app.running => {
                // Just loop to poll exec results when running
            }
        }

        // If execution completed, redraw
        if exec_done {
            terminal.draw(|frame| {
                ui::render(frame, app);
                if app.show_help {
                    ui::render_help(frame, frame.area());
                }
            })?;
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
        }
    }

    fn make_error_stage_output(exit_code: i32) -> StageOutput {
        StageOutput {
            stdout: vec![],
            stderr: b"error".to_vec(),
            exit_code: Some(exit_code),
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
}

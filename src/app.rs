use std::io;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::executor::{ExecutorCache, StageOutput, execute_pipeline_stages};
use crate::pipeline::Pipeline;
use crate::ui;

/// The display mode for stage output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Stdout,
    Stderr,
    Combined,
}

/// The current interaction mode of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Normal,
    Editing,
    Saving,
    ConfirmingDelete,
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
    pub output_mode: OutputMode,
    /// Cached outputs per stage (only up to the currently selected stage).
    pub stage_outputs: Vec<StageOutput>,
    /// Vertical scroll offset for the output pager.
    pub scroll: usize,
    /// Error message to display (e.g. command failed).
    pub error_message: Option<String>,
    /// Is a command currently running in the background?
    pub running: bool,
    /// Content of the inline text editor (command or filename).
    pub editor_content: String,
    /// Cursor position within the editor content (byte index).
    pub editor_cursor: usize,
    /// Show help overlay?
    pub show_help: bool,
    /// True while editing a freshly inserted stage (remove on cancel).
    pending_new_stage: bool,
    /// The executor cache.
    cache: ExecutorCache,
    /// Sender for triggering background execution.
    exec_tx: mpsc::Sender<(Vec<String>, usize, bool)>,
    /// Receiver for execution results.
    exec_rx: mpsc::Receiver<ExecMsg>,
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
                let (result, returned_cache) =
                    tokio::task::spawn_blocking(move || {
                        let r = execute_pipeline_stages(&mut c, &commands, up_to, force);
                        (r, c)
                    })
                    .await
                    .expect("executor task panicked");
                cache = returned_cache;
                let msg = match result {
                    Ok(outputs) => ExecMsg::Done { outputs, error: None },
                    Err(e) => ExecMsg::Done {
                        outputs: vec![],
                        error: Some(e.to_string()),
                    },
                };
                let _ = inner_tx.send(msg).await;
            }
        });

        Self {
            pipeline,
            mode: AppMode::Normal,
            output_mode: OutputMode::Stdout,
            stage_outputs: Vec::new(),
            scroll: 0,
            error_message: None,
            running: false,
            editor_content: String::new(),
            editor_cursor: 0,
            show_help: false,
            pending_new_stage: false,
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
                self.scroll = 0;
                true
            }
            Err(_) => false,
        }
    }

    /// Return the text to display in the output pager.
    pub fn current_output_text(&self) -> String {
        if self.pipeline.is_empty() || self.stage_outputs.is_empty() {
            return String::new();
        }
        let idx = self.pipeline.selected.min(self.stage_outputs.len().saturating_sub(1));
        let out = &self.stage_outputs[idx];
        match self.output_mode {
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
        self.scroll = (self.scroll + n).min(total.saturating_sub(1));
    }

    /// Scroll up by `n` lines.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
    }

    /// Save current output text to a file.
    pub fn save_output(&self, path: &str) -> Result<()> {
        let content = self.current_output_text();
        std::fs::write(path, content.as_bytes())?;
        Ok(())
    }

    /// Start editing the current stage's command.
    pub fn start_editing(&mut self) {
        let cmd = self
            .pipeline
            .selected_stage()
            .map(|s| s.command.clone())
            .unwrap_or_default();
        self.editor_content = cmd.clone();
        self.editor_cursor = cmd.len();
        self.mode = AppMode::Editing;
    }

    /// Confirm an edit and update the pipeline stage.
    pub fn confirm_edit(&mut self) {
        let new_cmd = self.editor_content.clone();
        if let Some(stage) = self.pipeline.selected_stage_mut() {
            stage.command = new_cmd;
        }
        self.mode = AppMode::Normal;
        self.pending_new_stage = false;
        // Invalidate downstream cache and re-run
        self.cache.clear();
        self.trigger_exec(false);
    }

    /// Cancel editing.
    pub fn cancel_edit(&mut self) {
        if self.pending_new_stage {
            self.pipeline.remove_selected();
            self.pending_new_stage = false;
        }
        self.mode = AppMode::Normal;
        self.editor_content.clear();
        self.editor_cursor = 0;
    }

    /// Start the save-to-file dialog.
    pub fn start_saving(&mut self) {
        self.editor_content = String::new();
        self.editor_cursor = 0;
        self.mode = AppMode::Saving;
    }

    /// Confirm saving output to a file.
    pub fn confirm_save(&mut self) {
        let path = self.editor_content.clone();
        self.mode = AppMode::Normal;
        self.editor_content.clear();
        self.editor_cursor = 0;
        if !path.is_empty() {
            if let Err(e) = self.save_output(&path) {
                self.error_message = Some(format!("Save failed: {}", e));
            }
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
                self.scroll = 0;
                self.trigger_exec(false);
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.pipeline.select_prev();
                self.scroll = 0;
                self.trigger_exec(false);
            }

            // Editing
            KeyCode::Char('e') | KeyCode::Enter => {
                if !self.pipeline.is_empty() {
                    self.start_editing();
                }
            }

            // Add new stage
            KeyCode::Char('n') | KeyCode::Char('a') => {
                self.pipeline.insert_after_selected();
                self.start_editing();
                self.pending_new_stage = true;
            }

            // Delete stage
            KeyCode::Char('d') => {
                if !self.pipeline.is_empty() {
                    let is_last = self.pipeline.selected == self.pipeline.len() - 1;
                    if is_last || self.pipeline.len() == 1 {
                        // Last stage or only stage: delete immediately
                        self.pipeline.remove_selected();
                        self.scroll = 0;
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
                self.output_mode = OutputMode::Stdout;
                self.scroll = 0;
            }
            KeyCode::Char('2') => {
                self.output_mode = OutputMode::Stderr;
                self.scroll = 0;
            }
            KeyCode::Char('3') => {
                self.output_mode = OutputMode::Combined;
                self.scroll = 0;
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
                self.scroll = 0;
            }
            KeyCode::Char('G') | KeyCode::End => {
                let total = self.output_line_count();
                self.scroll = total.saturating_sub(1);
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
            KeyCode::Enter => match self.mode {
                AppMode::Editing => self.confirm_edit(),
                AppMode::Saving => self.confirm_save(),
                _ => {}
            },
            KeyCode::Backspace => {
                if self.editor_cursor > 0 {
                    let pos = self.editor_cursor - 1;
                    self.editor_content.remove(pos);
                    self.editor_cursor = pos;
                }
            }
            KeyCode::Delete => {
                if self.editor_cursor < self.editor_content.len() {
                    self.editor_content.remove(self.editor_cursor);
                }
            }
            KeyCode::Left => {
                if self.editor_cursor > 0 {
                    // Move back by one char boundary
                    let s = &self.editor_content[..self.editor_cursor];
                    self.editor_cursor = s
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
            }
            KeyCode::Right => {
                if self.editor_cursor < self.editor_content.len() {
                    let s = &self.editor_content[self.editor_cursor..];
                    let next = s.char_indices().nth(1).map(|(i, _)| self.editor_cursor + i)
                        .unwrap_or(self.editor_content.len());
                    self.editor_cursor = next;
                }
            }
            KeyCode::Home => {
                self.editor_cursor = 0;
            }
            KeyCode::End => {
                self.editor_cursor = self.editor_content.len();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.editor_content.insert(self.editor_cursor, c);
                self.editor_cursor += c.len_utf8();
            }
            _ => {}
        }
        false
    }

    /// Handle a key event in the delete confirmation prompt.
    fn handle_confirm_delete_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.pipeline.remove_selected();
                self.scroll = 0;
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

    /// Handle a terminal event. Returns `true` if the app should quit.
    pub fn handle_event(&mut self, event: Event) -> bool {
        // Close help on any key
        if self.show_help {
            self.show_help = false;
            return false;
        }

        match event {
            Event::Key(key) => match self.mode {
                AppMode::Normal => self.handle_normal_key(key),
                AppMode::Editing | AppMode::Saving => self.handle_editor_key(key),
                AppMode::ConfirmingDelete => self.handle_confirm_delete_key(key),
            },
            _ => false,
        }
    }
}

/// Run the TUI event loop.
pub async fn run(mut app: App) -> Result<()> {
    let mut terminal = setup_terminal()?;

    // Trigger initial execution if there are stages
    if !app.pipeline.is_empty() {
        app.trigger_exec(false);
    }

    let mut event_stream = EventStream::new();

    loop {
        // Poll for completed background execution
        let exec_done = app.poll_exec_result();

        // Draw the UI
        terminal.draw(|frame| {
            ui::render(frame, &app);
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
                ui::render(frame, &app);
                if app.show_help {
                    ui::render_help(frame, frame.area());
                }
            })?;
        }
    }

    restore_terminal(&mut terminal)?;
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

use std::time::Duration;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use std::io::Write;

use crate::ansi::ansi_text_to_visible_lines;
use crate::app::{App, AppMode, HistoryBrowser, OptionsTab, OutputMode};
use crate::editor::EditorState;

/// Maximum width (columns) of the command editor overlay dialog.
pub const EDITOR_DIALOG_MAX_WIDTH: u16 = 120;
/// Maximum width (columns) of the save-to-file dialog.
pub const SAVE_DIALOG_MAX_WIDTH: u16 = 60;

/// How long a stage/pipe highlight remains visible after the last data chunk was received.
/// Slightly longer than the executor's UI_THROTTLE (100 ms) to prevent blinking between
/// consecutive throttled updates.
const DATA_ACTIVE_TIMEOUT: Duration = Duration::from_millis(101);

pub fn trigger_terminal_bell() {
    let _ = write!(std::io::stdout(), "\x07");
    let _ = std::io::stdout().flush();
}

/// Render the full TUI.
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Split: stages bar (top), output (middle), status bar (bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Stages bar (2 content rows: counts + command)
            Constraint::Min(0),    // Output pager
            Constraint::Length(1), // Status bar
        ])
        .split(area);

    render_stages_bar(frame, app, chunks[0]);
    render_output(frame, app, chunks[1]);
    render_status_bar(frame, app, chunks[2]);

    // Overlay modal dialogs on top
    match &app.mode {
        AppMode::Editing { .. } => render_editor_overlay(frame, app, area),
        AppMode::Saving(_) => render_save_overlay(frame, app, area),
        AppMode::ConfirmingDelete => render_confirm_delete_overlay(frame, app, area),
        AppMode::BrowsingHistory(browser) => {
            render_history_browser(frame, browser, area);
        }
        _ => {}
    }
    if app.show_help {
        render_help(frame, frame.area());
    }
    if app.show_options {
        render_options(frame, app, frame.area());
    }
}

/// Build the pipe connector string based on a stage's output mode.
/// This is the shell syntax that redirects the right stream into the pipe.
pub fn pipe_connector(mode: OutputMode) -> &'static str {
    match mode {
        OutputMode::Stdout => " | ",
        OutputMode::Combined => " 2>&1 | ",
        OutputMode::Stderr => " 2>&1 >/dev/null | ",
    }
}

/// Render the pipeline stages bar at the top as a single copyable command.
fn render_stages_bar(frame: &mut Frame, app: &App, area: Rect) {
    if app.pipeline.is_empty() {
        let msg = Paragraph::new("No stages — press 'o' to add a new stage")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP).title("— Pipeline "));
        frame.render_widget(msg, area);
        return;
    }

    // --- Build the selected-stage detail for the bottom title ---
    let selected = app.pipeline.selected;
    let sel_output = app.stage_outputs.get(selected);
    let sel_exit = sel_output.and_then(|o| o.exit_code);
    let sel_error = matches!(sel_exit, Some(code) if code != 0);

    let sel_stdout = sel_output.map(|o| o.stdout_line_count()).unwrap_or(0);
    let sel_stderr = sel_output.map(|o| o.stderr_line_count()).unwrap_or(0);
    let sel_mode = app
        .stage_views
        .get(selected)
        .map(|v| v.output_mode)
        .unwrap_or(OutputMode::Stdout);
    let detail_label = match sel_mode {
        OutputMode::Stdout => format!("{}/[{}]", sel_stdout, sel_stderr),
        OutputMode::Stderr => format!("[{}]/{}", sel_stdout, sel_stderr),
        OutputMode::Combined => {
            format!("{}+{}={}", sel_stdout, sel_stderr, sel_stdout + sel_stderr)
        }
    };

    let any_error = app
        .stage_outputs
        .iter()
        .any(|o| matches!(o.exit_code, Some(c) if c != 0));
    let block_style = if sel_error {
        Style::default().fg(Color::Red)
    } else {
        Style::default()
    };
    let title = if any_error {
        "— Pipeline ✗ "
    } else {
        "— Pipeline "
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .title(title)
        .title_top(
            ratatui::text::Line::from(format!("— {} -", detail_label)).alignment(Alignment::Right),
        )
        .style(block_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // --- Line 1: per-stage line counts, aligned above each command segment ---
    // --- Line 2: pure copyable bash command ---
    let mut count_spans: Vec<Span> = Vec::new();
    let mut cmd_spans: Vec<Span> = Vec::new();
    let n = app.pipeline.len();

    for (i, stage) in app.pipeline.stages.iter().enumerate() {
        let is_selected = i == selected;

        let cmd_text = if stage.command.is_empty() {
            "<empty>"
        } else {
            &stage.command
        };

        let stage_output = app.stage_outputs.get(i);
        let stage_exit = stage_output.and_then(|o| o.exit_code);
        let stage_error = matches!(stage_exit, Some(code) if code != 0);
        let stage_mode = mode_for_stage(app, i);
        // A stage is actively receiving output when its last StageUpdate
        // arrived within DATA_ACTIVE_TIMEOUT. This is slightly longer than
        // the executor's throttle interval so the highlight doesn't blink.
        let data_is_active = stage
            .last_update
            .is_some_and(|t| t.elapsed() < DATA_ACTIVE_TIMEOUT);

        // Compute the line count label for this stage.
        let line_count = stage_output
            .map(|o| match stage_mode {
                OutputMode::Stdout => o.stdout_line_count(),
                OutputMode::Stderr => o.stderr_line_count(),
                OutputMode::Combined => o.stdout_line_count() + o.stderr_line_count(),
            })
            .unwrap_or(0);
        let error_mark = if stage_error { "✗" } else { "" };
        let effective_interactive = stage.overrides.resolve(&app.global_defaults).interactive;
        let interactive_mark = if effective_interactive { "ⁱ" } else { "" };
        let count_label = format!("{}{}{}", interactive_mark, line_count, error_mark);

        // The connector that follows this command (empty for the last stage).
        let connector = if i + 1 < n {
            pipe_connector(stage_mode)
        } else {
            ""
        };

        // The segment width is command + connector; pad the count label to match.
        let segment_width = cmd_text.len() + connector.len();
        let padded_count = format!("{:<width$}", count_label, width = segment_width);

        let count_style = if stage_error {
            Style::default().fg(Color::Red)
        } else if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if data_is_active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        count_spans.push(Span::styled(padded_count, count_style));

        // Command span.
        let cmd_style = if is_selected && stage_error {
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default()
                .fg(Color::White)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if stage_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::White)
        };
        cmd_spans.push(Span::styled(cmd_text.to_string(), cmd_style));

        // Connector span on the command line.
        // The pipe is highlighted while data is actively flowing through it:
        // either stage i is producing output (data entering the connector) or
        // stage i+1 is actively consuming output (data leaving the connector).
        if !connector.is_empty() {
            let next_data_is_active = app.pipeline.stages.get(i + 1).is_some_and(|s| {
                s.last_update
                    .is_some_and(|t| t.elapsed() < DATA_ACTIVE_TIMEOUT)
            });
            let connector_style = if data_is_active || next_data_is_active {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            cmd_spans.push(Span::styled(connector, connector_style));
        }
    }

    let text = Text::from(vec![Line::from(count_spans), Line::from(cmd_spans)]);
    let para = Paragraph::new(text);
    frame.render_widget(para, inner);
}

/// Get the output mode for a given stage (used to determine the pipe connector).
fn mode_for_stage(app: &App, stage_idx: usize) -> OutputMode {
    app.stage_views
        .get(stage_idx)
        .map(|v| v.output_mode)
        .unwrap_or(OutputMode::Stdout)
}

/// Render the output pager area.
fn render_output(frame: &mut Frame, app: &mut App, area: Rect) {
    app.visible_output_lines = area.height.saturating_sub(2).max(1) as usize;
    app.visible_output_width = area.width.max(1) as usize;

    let exit_info = if !app.stage_outputs.is_empty() {
        let idx = app
            .pipeline
            .selected
            .min(app.stage_outputs.len().saturating_sub(1));
        match app.stage_outputs[idx].exit_code {
            Some(0) => " ✓ ".to_string(),
            Some(code) => format!(" ✗ exit:{} ", code),
            None => String::new(),
        }
    } else {
        String::new()
    };

    // Extract immutable view state before we may need &mut app later.
    let output_mode = app.view().output_mode;
    let search_query = app.view().search.query.clone();
    let search_matches: Vec<(usize, usize, usize)> = app
        .view()
        .search
        .matches
        .iter()
        .map(|&(line, start, end)| (line, start, end))
        .collect();
    let search_match_idx = app.view().search.match_idx;

    let mode_label = match output_mode {
        OutputMode::Stdout => "stdout",
        OutputMode::Stderr => "stderr",
        OutputMode::Combined => "combined",
    };

    // Include search match count when a search is active.
    let search_span = if !search_query.is_empty() {
        if search_matches.is_empty() {
            Span::styled(" [no matches]", Style::default().fg(Color::Red))
        } else {
            Span::styled(
                format!(" [{}/{}]", search_match_idx + 1, search_matches.len()),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        }
    } else {
        Span::raw("")
    };

    let title_line = Line::from(vec![
        Span::raw(format!("— {} — {}", mode_label, exit_info)),
        search_span,
        Span::raw(" "),
    ]);

    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .title(title_line);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.error_message.is_some() {
        let msg = app.error_message.as_deref().unwrap_or("");
        let para = Paragraph::new(msg).style(Style::default().fg(Color::Red));
        frame.render_widget(para, inner);
        return;
    }

    if app.pipeline.is_empty() {
        return;
    }

    // Use the efficient byte-level line count (no String allocation).
    let total_lines = app.output_line_count();
    if total_lines == 0 {
        let hint = Paragraph::new("(no output)")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(hint, inner);
        return;
    }

    let visible_height = inner.height as usize;
    let max_scroll = app.compute_max_scroll();
    // Clamp and persist: when the window resizes, the old scroll position may
    // exceed the new max.  Writing back keeps app state consistent so the next
    // key press doesn't use a stale value.
    let scroll = app.view().scroll.min(max_scroll);
    app.view_mut().scroll = scroll;

    let raw_content = app.current_output_text();

    // Build search highlight map (only entries in the visible window matter,
    // but we provide the full map — the windowed parser skips invisible lines).
    let line_match_map = if !search_matches.is_empty() {
        let mut map: std::collections::HashMap<usize, Vec<(usize, usize, bool)>> =
            std::collections::HashMap::new();
        let window_end = scroll + visible_height;
        let lo = search_matches.partition_point(|&(line, _, _)| line < scroll);
        let hi = search_matches.partition_point(|&(line, _, _)| line < window_end);
        for idx in lo..hi {
            let (line, start, end) = search_matches[idx];
            let is_current = idx == search_match_idx;
            map.entry(line).or_default().push((start, end, is_current));
        }
        map
    } else {
        std::collections::HashMap::new()
    };

    // Only parse visible lines — skip ANSI content before `scroll`, stop
    // after `visible_height` lines.  When a pre-built line index is
    // available (stdout/stderr), phase 1 is skipped entirely via O(1) lookup.
    let line_index = app.current_line_index();
    let mut lines = ansi_text_to_visible_lines(
        &raw_content,
        scroll,
        visible_height,
        &line_match_map,
        line_index,
    );

    // In combined mode, prepend a 1-column margin indicating the source stream:
    // a yellow "│" for stderr lines, a space for stdout lines.
    // `stderr_map` has one entry per `CombinedLine` (== one entry per rendered line);
    // `unwrap_or(false)` handles any transient mismatch (e.g. trailing empty lines).
    let stderr_map = app.combined_stderr_map();
    if matches!(output_mode, OutputMode::Combined) {
        for (i, line) in lines.iter_mut().enumerate() {
            // Map from visible index back to the absolute line index.
            let abs_i = scroll + i;
            let is_stderr = stderr_map.get(abs_i).copied().unwrap_or(false);
            if is_stderr {
                for span in &mut line.spans {
                    if span.style.bg.is_none() {
                        span.style = span.style.fg(Color::DarkGray);
                    }
                }
            }
            let margin = if is_stderr {
                Span::styled(">", Style::default().bg(Color::Yellow).fg(Color::Black))
            } else {
                Span::raw(" ")
            };
            line.spans.insert(0, margin);
        }
    }

    let text = Text::from(lines);
    let para = Paragraph::new(text).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);

    // Render scrollbar hint at bottom-right of inner area
    if max_scroll > 0 {
        let pct = ((scroll as f64 / max_scroll as f64) * 100.0).round() as usize;
        let hint = format!(" {}% ", pct.min(100));
        let hint_len = hint.len() as u16;
        if inner.width > hint_len + 2 {
            let hint_area = Rect::new(
                inner.x + inner.width - hint_len,
                inner.y + inner.height.saturating_sub(1),
                hint_len,
                1,
            );
            let hint_widget = Paragraph::new(hint).style(Style::default().fg(Color::DarkGray));
            frame.render_widget(hint_widget, hint_area);
        }
    }
}

/// Render the status bar at the bottom.
fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    if matches!(app.mode, AppMode::Searching) {
        // Show the search prompt like Vim: /query_with_cursor
        render_search_bar(frame, app, area);
        return;
    }

    if matches!(app.mode, AppMode::Command(_)) {
        render_command_bar(frame, app, area);
        return;
    }

    let search_nav_hint = if !app.view().search.matches.is_empty() {
        "  [n]ext-match  [N]prev-match  [Esc]clear-search"
    } else {
        ""
    };

    let (mode_str, hints) = match &app.mode {
        AppMode::Normal => {
            let running = if app.running { " ⟳ Running…" } else { "" };
            (
                format!("NORMAL{}", running),
                format!(
                    "[q]uit  [e]edit  [O]prepend  [o|]append  [d,Del] [hl←→]switch  \
                     [m]cycle output  [s]ave  [r]erun  \
                     [/]search  [?,:h]help{}",
                    search_nav_hint
                ),
            )
        }
        AppMode::Editing { .. } => (
            "EDIT".to_string(),
            "[Enter]confirm  [Esc]cancel".to_string(),
        ),
        AppMode::Saving(_) => (
            "SAVE".to_string(),
            "[Enter]confirm  [Esc]cancel".to_string(),
        ),
        AppMode::ConfirmingDelete => (
            "DELETE?".to_string(),
            "[y]confirm delete  [any]cancel".to_string(),
        ),
        AppMode::BrowsingHistory(_) => (
            "HISTORY".to_string(),
            "[↑↓]navigate  [Enter]load  [Del/x]delete  [q/Esc]close".to_string(),
        ),
        AppMode::Searching | AppMode::Command(_) => unreachable!(),
    };

    let left = Span::styled(
        format!(" {} ", mode_str),
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let right = Span::styled(format!(" {} ", hints), Style::default().fg(Color::DarkGray));

    let status = Paragraph::new(Line::from(vec![left, Span::raw("  "), right]));
    frame.render_widget(status, area);
}

/// Render the vim-style search bar (shown in place of the status bar when searching).
fn render_search_bar(frame: &mut Frame, app: &App, area: Rect) {
    let view = app.view();
    let cursor_pos = view.search.cursor;
    let content = &view.search.query;

    let prefix = Span::styled(
        "/",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let cursor_spans = if cursor_pos < content.len() {
        let (before, after) = content.split_at(cursor_pos);
        let mut chars = after.chars();
        let cur_ch = chars.next().unwrap_or(' ');
        let rest: String = chars.collect();
        vec![
            Span::raw(before.to_owned()),
            Span::styled(
                cur_ch.to_string(),
                Style::default().fg(Color::Black).bg(Color::White),
            ),
            Span::raw(rest),
        ]
    } else {
        vec![
            Span::raw(content.clone()),
            Span::styled(" ", Style::default().fg(Color::Black).bg(Color::White)),
        ]
    };

    let mut spans = vec![prefix];
    spans.extend(cursor_spans);
    spans.push(Span::styled(
        "  [Enter]search  [Esc]cancel",
        Style::default().fg(Color::DarkGray),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Render the vim-style command bar (shown in place of the status bar when in Command mode).
fn render_command_bar(frame: &mut Frame, app: &App, area: Rect) {
    let editor = match &app.mode {
        AppMode::Command(editor) => editor,
        _ => return,
    };
    let cursor_pos = editor.cursor;
    let content = &editor.content;

    let prefix = Span::styled(
        ":",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let cursor_spans = if cursor_pos < content.len() {
        let (before, after) = content.split_at(cursor_pos);
        let mut chars = after.chars();
        let cur_ch = chars.next().unwrap_or(' ');
        let rest: String = chars.collect();
        vec![
            Span::raw(before.to_owned()),
            Span::styled(
                cur_ch.to_string(),
                Style::default().fg(Color::Black).bg(Color::White),
            ),
            Span::raw(rest),
        ]
    } else {
        vec![
            Span::raw(content.clone()),
            Span::styled(" ", Style::default().fg(Color::Black).bg(Color::White)),
        ]
    };

    let mut spans = vec![prefix];
    spans.extend(cursor_spans);

    if let Some(completions) = &app.command_completions {
        spans.push(Span::styled(
            format!("  {}", completions),
            Style::default().fg(Color::Cyan),
        ));
    } else {
        spans.push(Span::styled(
            "  [Tab]complete  [Enter]run  [Esc]cancel",
            Style::default().fg(Color::DarkGray),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Render a generic editor dialog popup: a bordered box with a title and the
/// editor content displayed with a visible cursor.
fn render_editor_dialog(
    frame: &mut Frame,
    editor: &EditorState,
    title: &str,
    max_width: u16,
    area: Rect,
) {
    let height = 3u16;
    let width = area.width.saturating_sub(4).min(max_width);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let dialog_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, dialog_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", title))
        .style(Style::default().bg(Color::DarkGray));

    let cursor_pos = editor.cursor;
    let content = &editor.content;
    let display = if cursor_pos < content.len() {
        let (before, after) = content.split_at(cursor_pos);
        let mut chars = after.chars();
        let cur_ch = chars.next().unwrap_or(' ');
        let rest: String = chars.collect();
        vec![
            Span::raw(before.to_string()),
            Span::styled(
                cur_ch.to_string(),
                Style::default().fg(Color::Black).bg(Color::White),
            ),
            Span::raw(rest),
        ]
    } else {
        vec![
            Span::raw(content.clone()),
            Span::styled(" ", Style::default().fg(Color::Black).bg(Color::White)),
        ]
    };

    let scroll_x = editor.scroll_x.min(u16::MAX as usize) as u16;
    let para = Paragraph::new(Line::from(display))
        .block(block)
        .scroll((0, scroll_x));
    frame.render_widget(para, dialog_area);
}

/// Render the inline command editor overlay.
fn render_editor_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let editor = match &app.mode {
        AppMode::Editing { editor, .. } => editor,
        _ => return,
    };
    let stage_num = app.pipeline.selected + 1;
    render_editor_dialog(
        frame,
        editor,
        &format!("Edit Stage {} Command", stage_num),
        EDITOR_DIALOG_MAX_WIDTH,
        area,
    );
}

/// Render the save-to-file dialog overlay.
fn render_save_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let editor = match &app.mode {
        AppMode::Saving(editor) => editor,
        _ => return,
    };
    render_editor_dialog(
        frame,
        editor,
        "Save Output To File",
        SAVE_DIALOG_MAX_WIDTH,
        area,
    );
}

/// Render the delete confirmation overlay.
fn render_confirm_delete_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let stage_num = app.pipeline.selected + 1;
    let total = app.pipeline.len();
    let height = 5u16;
    let width = area.width.saturating_sub(4).min(60);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let dialog_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, dialog_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" ⚠ Confirm Delete ")
        .style(Style::default().bg(Color::DarkGray).fg(Color::Yellow));

    let text = Text::from(vec![
        Line::from(format!(
            "Deleting stage {} of {} may break downstream stages.",
            stage_num, total
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to confirm, any other key to cancel"),
        ]),
    ]);

    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    frame.render_widget(para, dialog_area);
}

/// Render the interactive history browser overlay.
fn render_history_browser(frame: &mut Frame, browser: &HistoryBrowser, area: Rect) {
    let width = area.width.saturating_sub(4).min(90);
    // Reserve height for borders (2) + footer (1); show as many entries as fit.
    let max_visible = (area.height.saturating_sub(5)) as usize;
    let entry_count = browser.entries.len();
    let visible_count = entry_count.min(max_visible);
    let height = (visible_count as u16) + 4; // 2 border + 1 footer + 1 padding

    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let dialog_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, dialog_area);

    let title = if browser.confirming_delete {
        " ⚠️ Delete this entry? [y]es / [any]cancel "
    } else {
        " History — ↑↓ navigate, Enter load, Del/x delete, q/Esc close "
    };

    let block =
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(if browser.confirming_delete {
                Style::default().bg(Color::DarkGray).fg(Color::Yellow)
            } else {
                Style::default().bg(Color::DarkGray)
            });

    // Compute scroll offset to keep the selected entry visible.
    let scroll = if browser.selected < max_visible {
        0
    } else {
        browser.selected - max_visible + 1
    };

    let inner_width = width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = browser
        .entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(max_visible)
        .map(|(i, entry)| {
            let pipeline_str = entry.commands.join(" | ");
            let label = format!("{:>3}  {}", i + 1, pipeline_str);
            // Truncate to fit width.
            let display: String = if label.len() > inner_width {
                format!("{}…", &label[..inner_width.saturating_sub(1)])
            } else {
                label
            };
            let style = if i == browser.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Span::styled(display, style))
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, dialog_area);
}

/// Render a simple list of keybindings for the help overlay.
fn render_help(frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = vec![
        ListItem::new("→ / l            Next stage"),
        ListItem::new("← / h            Previous stage"),
        ListItem::new("e                Edit current stage command"),
        ListItem::new("O                Insert new stage before current"),
        ListItem::new("o / |            Insert new stage after current"),
        ListItem::new("d / Delete       Delete current stage"),
        ListItem::new("u                Undo last pipeline change"),
        ListItem::new("Ctrl+R           Redo last undone change"),
        ListItem::new("r                Re-run (bypass cache)"),
        ListItem::new("s                Save output to file"),
        ListItem::new("m/1/2/3          Switch between stdout/stderr/combined views"),
        ListItem::new("/                Start search (regex, \\c=ignore case, \\C=match case)"),
        ListItem::new("n / N            Next / previous search match"),
        ListItem::new("Esc              Clear search highlights"),
        ListItem::new("j / ↓            Scroll down"),
        ListItem::new("k / ↑            Scroll up"),
        ListItem::new("Ctrl+d / Ctrl+u  Half-page down/up"),
        ListItem::new("Ctrl+f / Ctrl+b  Full page down/up"),
        ListItem::new("PgDn / Ctrl+f    Page down"),
        ListItem::new("PgUp / Ctrl+b    Page up"),
        ListItem::new("g / Home         Go to top"),
        ListItem::new("G / End          Go to bottom"),
        ListItem::new("q / Ctrl+c       Quit"),
    ];

    let width = area.width.min(60);
    let height = (items.len() as u16) + 2;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let help_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, help_area);
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Help — press ? to close ")
            .style(Style::default().bg(Color::DarkGray)),
    );
    frame.render_widget(list, help_area);
}

/// Render the per-stage options overlay.
fn render_options(frame: &mut Frame, app: &App, area: Rect) {
    let selected = app.pipeline.selected;
    let width = area.width.min(50);

    // --- Build tab header ---
    let stage_tab_style = if app.options_tab == OptionsTab::Stage {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let global_tab_style = if app.options_tab == OptionsTab::Global {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let tab_line = Line::from(vec![
        Span::styled(format!(" Stage {} ", selected + 1), stage_tab_style),
        Span::raw("│"),
        Span::styled(" Global ", global_tab_style),
    ]);

    // --- Build body items ---
    let items: Vec<ListItem> = match app.options_tab {
        OptionsTab::Stage => {
            let stage = &app.pipeline.stages[selected];
            let interactive_label = match stage.overrides.interactive {
                Some(true) => "[i] Interactive shell:  ON  (stage override)",
                Some(false) => "[i] Interactive shell:  OFF (stage override)",
                None => {
                    if app.global_defaults.interactive {
                        "[i] Interactive shell:  ON  (inherited)"
                    } else {
                        "[i] Interactive shell:  OFF (inherited)"
                    }
                }
            };
            let shell_label = match &stage.overrides.shell {
                Some(s) => format!("[s] Shell: {}  (stage override)", s),
                None => match &app.global_defaults.shell {
                    Some(s) => format!("[s] Shell: {}  (inherited)", s),
                    None => "[s] Shell: auto-detect  (inherited)".to_string(),
                },
            };
            let reset_hint = if stage.overrides.has_overrides() {
                "[r] Reset overrides to inherited"
            } else {
                ""
            };
            let mut items = vec![
                ListItem::new(tab_line),
                ListItem::new(interactive_label),
                ListItem::new(shell_label),
            ];
            if !reset_hint.is_empty() {
                items.push(ListItem::new(reset_hint));
            }
            items
        }
        OptionsTab::Global => {
            let interactive_label = if app.global_defaults.interactive {
                "[i] Interactive shell:  ON"
            } else {
                "[i] Interactive shell:  OFF"
            };
            let shell_label = match &app.global_defaults.shell {
                Some(s) => format!("[s] Shell: {}", s),
                None => "[s] Shell: auto-detect".to_string(),
            };
            let items = vec![
                ListItem::new(tab_line),
                ListItem::new(interactive_label),
                ListItem::new(shell_label),
            ];
            items
        }
    };

    let height = (items.len() as u16) + 2;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let opts_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, opts_area);
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Options — Tab to switch, Esc to close ")
            .style(Style::default().bg(Color::DarkGray)),
    );
    frame.render_widget(list, opts_area);

    // Render the shell editor popup on top of the options overlay.
    if let Some(ref editor) = app.options_shell_editor {
        let title = match app.options_tab {
            OptionsTab::Stage => format!("Stage {} Shell", selected + 1),
            OptionsTab::Global => "Global Shell".to_string(),
        };
        render_editor_dialog(frame, editor, &title, EDITOR_DIALOG_MAX_WIDTH, area);
    }
}

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::ansi::ansi_text_to_visible_lines;
use crate::app::{App, AppMode, OutputMode};

/// Maximum width (columns) of the command editor overlay dialog.
pub const EDITOR_DIALOG_MAX_WIDTH: u16 = 120;
/// Maximum width (columns) of the save-to-file dialog.
pub const SAVE_DIALOG_MAX_WIDTH: u16 = 60;

/// Render the full TUI.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Split: stages bar (top), output (middle), status bar (bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // Stages bar (2 content rows: counts + command)
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
        _ => {}
    }
    if app.show_help {
        render_help(frame, frame.area());
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
            .block(Block::default().borders(Borders::ALL).title(" Pipeline "));
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
        " Pipeline ✗ "
    } else {
        " Pipeline "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(
            ratatui::text::Line::from(format!(" {} ", detail_label)).alignment(Alignment::Right),
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

        // Compute the line count label for this stage.
        let line_count = stage_output
            .map(|o| match stage_mode {
                OutputMode::Stdout => o.stdout_line_count(),
                OutputMode::Stderr => o.stderr_line_count(),
                OutputMode::Combined => o.stdout_line_count() + o.stderr_line_count(),
            })
            .unwrap_or(0);
        let error_mark = if stage_error { "✗" } else { "" };
        let count_label = format!("{}{}", line_count, error_mark);

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
        if !connector.is_empty() {
            cmd_spans.push(Span::styled(
                connector,
                Style::default().fg(Color::DarkGray),
            ));
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
fn render_output(frame: &mut Frame, app: &App, area: Rect) {
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

    let view = app.view();

    let mode_label = match view.output_mode {
        OutputMode::Stdout => "stdout",
        OutputMode::Stderr => "stderr",
        OutputMode::Combined => "combined",
    };

    // Include search match count when a search is active.
    let search_info = if !view.search.query.is_empty() {
        if view.search.matches.is_empty() {
            " [no matches]".to_string()
        } else {
            format!(
                " [{}/{}]",
                view.search.match_idx + 1,
                view.search.matches.len()
            )
        }
    } else {
        String::new()
    };

    let title = format!(" Output ({}) — {}{} ", mode_label, exit_info, search_info,);

    let block = Block::default().borders(Borders::ALL).title(title);
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

    let raw_content = app.current_output_text();
    if raw_content.is_empty() {
        let hint = Paragraph::new("(no output)")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(hint, inner);
        return;
    }

    // Use the efficient byte-level line count (no String allocation).
    let total_lines = app.output_line_count();
    let visible_height = inner.height as usize;
    let scroll = view.scroll.min(total_lines.saturating_sub(visible_height));

    // Build search highlight map (only entries in the visible window matter,
    // but we provide the full map — the windowed parser skips invisible lines).
    let line_match_map = if !view.search.matches.is_empty() {
        let mut map: std::collections::HashMap<usize, Vec<(usize, usize, bool)>> =
            std::collections::HashMap::new();
        // matches is sorted by line index — use binary search to find only the
        // matches whose line falls in [scroll, scroll + visible_height).
        // This avoids iterating all 90k+ matches when only ~50 are visible.
        let window_end = scroll + visible_height;
        let lo = view
            .search
            .matches
            .partition_point(|&(line, _, _)| line < scroll);
        let hi = view
            .search
            .matches
            .partition_point(|&(line, _, _)| line < window_end);
        for idx in lo..hi {
            let (line, start, end) = view.search.matches[idx];
            let is_current = idx == view.search.match_idx;
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
    if !matches!(app.view().output_mode, OutputMode::Combined) {
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
    if total_lines > visible_height {
        let pct = if total_lines <= visible_height {
            100
        } else {
            (scroll * 100) / (total_lines - visible_height)
        };
        let hint = format!(" {}% ", pct);
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
                    "[q]uit  [e]edit  [o]new  [d/Del] [h/l/←/→]switch  \
                     [m]cycle output  [s]ave  [r]erun  \
                     [/]search  [?/:h]help{}",
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
    spans.push(Span::styled(
        "  [Tab]complete  [Enter]run  [Esc]cancel",
        Style::default().fg(Color::DarkGray),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Render the inline command editor overlay.
fn render_editor_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let editor = match &app.mode {
        AppMode::Editing { editor, .. } => editor,
        _ => return,
    };

    let height = 3u16;
    let width = area.width.saturating_sub(4).min(EDITOR_DIALOG_MAX_WIDTH);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let dialog_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, dialog_area);
    let stage_num = app.pipeline.selected + 1;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Edit Stage {} Command ", stage_num))
        .style(Style::default().bg(Color::DarkGray));

    let cursor_pos = editor.cursor;
    let content = &editor.content;
    // Build spans showing the cursor position
    let display = if cursor_pos < content.len() {
        let (before, after) = content.split_at(cursor_pos);
        let mut chars = after.chars();
        let cur_ch = chars.next().unwrap_or(' ');
        let rest: String = chars.collect();
        // Build spans
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

/// Render the save-to-file dialog overlay.
fn render_save_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let editor = match &app.mode {
        AppMode::Saving(editor) => editor,
        _ => return,
    };

    let height = 3u16;
    let width = area.width.saturating_sub(4).min(60);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let dialog_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, dialog_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Save Output To File ")
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

/// Render a simple list of keybindings for the help overlay.
fn render_help(frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = vec![
        ListItem::new("→ / l          Next stage"),
        ListItem::new("← / h          Previous stage"),
        ListItem::new("e              Edit current stage command"),
        ListItem::new("o              Add new stage"),
        ListItem::new("d / Delete     Delete current stage"),
        ListItem::new("r              Re-run (bypass cache)"),
        ListItem::new("s              Save output to file"),
        ListItem::new("m/1/2/3        Switch between stdout/stderr/combined views"),
        ListItem::new("/              Start search (regex, \\c=ignore case, \\C=match case)"),
        ListItem::new("n / N          Next / previous search match"),
        ListItem::new("Esc            Clear search highlights"),
        ListItem::new("j / ↓          Scroll down"),
        ListItem::new("k / ↑          Scroll up"),
        ListItem::new("Ctrl+d / Ctrl+u  Half-page down/up"),
        ListItem::new("Ctrl+f / Ctrl+b  Full page down/up"),
        ListItem::new("PgDn / Ctrl+f  Page down"),
        ListItem::new("PgUp / Ctrl+b  Page up"),
        ListItem::new("g / Home       Go to top"),
        ListItem::new("G / End        Go to bottom"),
        ListItem::new("q / Ctrl+c     Quit"),
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

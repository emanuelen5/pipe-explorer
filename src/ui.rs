use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::ansi::{ansi_text_to_lines, ansi_text_to_lines_with_highlights};
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
            Constraint::Length(3), // Stages bar
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
}

/// Render the pipeline stages bar at the top.
fn render_stages_bar(frame: &mut Frame, app: &App, area: Rect) {
    if app.pipeline.is_empty() {
        let msg = Paragraph::new("No stages — press 'n' to add a new stage")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL).title(" Pipeline "));
        frame.render_widget(msg, area);
        return;
    }

    // Create evenly divided sub-areas for each stage
    let n = app.pipeline.len();
    let widths: Vec<Constraint> = (0..n).map(|_| Constraint::Ratio(1, n as u32)).collect();
    let stage_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(widths)
        .split(area);

    for (i, stage) in app.pipeline.stages.iter().enumerate() {
        let is_selected = i == app.pipeline.selected;

        let stage_output = app.stage_outputs.get(i);
        let exit_code = stage_output.and_then(|o| o.exit_code);
        let is_error = matches!(exit_code, Some(code) if code != 0);
        let line_count = stage_output.map(|o| o.stdout_line_count());

        let title = match (is_error, line_count) {
            (true, Some(lines)) => format!(" Stage {} ✗ ({} lines) ", i + 1, lines),
            (false, Some(lines)) => format!(" Stage {} ({} lines) ", i + 1, lines),
            (true, None) => format!(" Stage {} ✗ ", i + 1),
            (false, None) => format!(" Stage {} ", i + 1),
        };

        let stdout_count = app
            .stage_outputs
            .get(i)
            .map(|o| o.stdout_str().lines().count())
            .unwrap_or(0);

        let stderr_count = app
            .stage_outputs
            .get(i)
            .map(|o| o.stderr_str().lines().count())
            .unwrap_or(0);

        let stage_view = app.stage_views.get(i);
        let line_count_label = match stage_view
            .map(|v| v.output_mode)
            .unwrap_or(OutputMode::Stdout)
        {
            OutputMode::Stdout => format!("{}/[{}]", stdout_count, stderr_count),
            OutputMode::Stderr => format!("[{}]/{}", stdout_count, stderr_count),
            OutputMode::Combined => format!(
                "{}+{}={}",
                stdout_count,
                stderr_count,
                stdout_count + stderr_count
            ),
        };

        let style = if is_selected && is_error {
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default()
                .fg(Color::White)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::White)
        };

        let cmd_display = if stage.command.is_empty() {
            "<empty>".to_string()
        } else {
            stage.command.clone()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_bottom(ratatui::text::Line::from(line_count_label).alignment(Alignment::Right))
            .style(style);
        let paragraph = Paragraph::new(cmd_display).block(block).style(style);
        frame.render_widget(paragraph, stage_areas[i]);
    }
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

    let title = format!(
        " Output ({}) — Stage {}{}{} ",
        mode_label,
        app.pipeline.selected + 1,
        exit_info,
        search_info,
    );

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

    let has_matches = !view.search.matches.is_empty();
    let lines: Vec<Line> = if has_matches {
        let mut line_match_map: std::collections::HashMap<usize, Vec<(usize, usize, bool)>> =
            std::collections::HashMap::new();
        for (idx, &(line, start, end)) in view.search.matches.iter().enumerate() {
            let is_current = idx == view.search.match_idx;
            line_match_map
                .entry(line)
                .or_default()
                .push((start, end, is_current));
        }
        ansi_text_to_lines_with_highlights(&raw_content, &line_match_map)
    } else {
        ansi_text_to_lines(&raw_content)
    };

    let total_lines = lines.len();
    let visible_height = inner.height as usize;
    let scroll = view.scroll.min(total_lines.saturating_sub(visible_height));

    let text = Text::from(lines);
    let para = Paragraph::new(text)
        .scroll((scroll as u16, 0))
        .wrap(Wrap { trim: false });
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

    let search_nav_hint = if !app.view().search.matches.is_empty() {
        "  [n]ext-match  [p]rev-match  [Esc]clear-search"
    } else {
        ""
    };

    let (mode_str, hints) = match &app.mode {
        AppMode::Normal => {
            let running = if app.running { " ⟳ Running…" } else { "" };
            (
                format!("NORMAL{}", running),
                format!(
                    "[q]uit  [e/Enter]edit  [a]new  [d]el  [Tab/←/→]switch  \
                     [1]stdout  [2]stderr  [3]combined  [s]ave  [r]erun  \
                     [/]search  [j/k/PgDn/PgUp/gg/G]scroll{}",
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
        AppMode::Searching => unreachable!(),
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
pub fn render_help(frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = vec![
        ListItem::new("Tab / → / l    Next stage"),
        ListItem::new("Shift+Tab / ← / h  Previous stage"),
        ListItem::new("e / Enter      Edit current stage command"),
        ListItem::new("a / n          Add new stage (n only when no search active)"),
        ListItem::new("d              Delete current stage"),
        ListItem::new("r              Re-run (bypass cache)"),
        ListItem::new("s              Save output to file"),
        ListItem::new("1              Show stdout"),
        ListItem::new("2              Show stderr"),
        ListItem::new("3              Show combined (stdout+stderr)"),
        ListItem::new("/              Start search (regex, \\c=ignore case, \\C=match case)"),
        ListItem::new("n / p          Next / previous search match"),
        ListItem::new("Esc            Clear search highlights"),
        ListItem::new("j / ↓          Scroll down"),
        ListItem::new("k / ↑          Scroll up"),
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


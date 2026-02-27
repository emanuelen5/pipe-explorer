use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::app::{App, AppMode, OutputMode};

/// Render the full TUI.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Split: stages bar (top), output (middle), status bar (bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Stages bar
            Constraint::Min(0),     // Output pager
            Constraint::Length(1),  // Status bar
        ])
        .split(area);

    render_stages_bar(frame, app, chunks[0]);
    render_output(frame, app, chunks[1]);
    render_status_bar(frame, app, chunks[2]);

    // Overlay modal dialogs on top
    match app.mode {
        AppMode::Editing => render_editor_overlay(frame, app, area),
        AppMode::Saving => render_save_overlay(frame, app, area),
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
        let title = format!(" Stage {} ", i + 1);
        let style = if is_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
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
            .style(style);
        let paragraph = Paragraph::new(cmd_display)
            .block(block)
            .style(style);
        frame.render_widget(paragraph, stage_areas[i]);
    }
}

/// Render the output pager area.
fn render_output(frame: &mut Frame, app: &App, area: Rect) {
    let exit_info = if !app.stage_outputs.is_empty() {
        let idx = app.pipeline.selected.min(app.stage_outputs.len().saturating_sub(1));
        match app.stage_outputs[idx].exit_code {
            Some(0) => " ✓ ".to_string(),
            Some(code) => format!(" ✗ exit:{} ", code),
            None => String::new(),
        }
    } else {
        String::new()
    };

    let mode_label = match app.output_mode {
        OutputMode::Stdout => "stdout",
        OutputMode::Stderr => "stderr",
        OutputMode::Combined => "combined",
    };
    let title = format!(
        " Output ({}) — Stage {}{} ",
        mode_label,
        app.pipeline.selected + 1,
        exit_info,
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

    let content = app.current_output_text();
    if content.is_empty() {
        let hint = Paragraph::new("(no output)")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(hint, inner);
        return;
    }

    let lines: Vec<Line> = content
        .lines()
        .map(|l| Line::from(Span::raw(l.to_owned())))
        .collect();

    let total_lines = lines.len();
    let visible_height = inner.height as usize;
    let scroll = app.scroll.min(total_lines.saturating_sub(visible_height));

    // Show scroll indicator in title if needed
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
            let hint_widget = Paragraph::new(hint)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(hint_widget, hint_area);
        }
    }
}

/// Render the status bar at the bottom.
fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let (mode_str, hints) = match app.mode {
        AppMode::Normal => {
            let running = if app.running { " ⟳ Running…" } else { "" };
            (
                format!("NORMAL{}", running),
                "[q]uit  [e/Enter]edit  [n]ew  [d]el  [Tab/←/→]switch  \
                 [1]stdout  [2]stderr  [3]combined  [s]ave  [r]erun  \
                 [j/k/PgDn/PgUp/gg/G]scroll",
            )
        }
        AppMode::Editing => ("EDIT".to_string(), "[Enter]confirm  [Esc]cancel"),
        AppMode::Saving => ("SAVE".to_string(), "[Enter]confirm  [Esc]cancel"),
    };

    let left = Span::styled(
        format!(" {} ", mode_str),
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let right = Span::styled(
        format!(" {} ", hints),
        Style::default().fg(Color::DarkGray),
    );

    let status = Paragraph::new(Line::from(vec![left, Span::raw("  "), right]));
    frame.render_widget(status, area);
}

/// Render the inline command editor overlay.
fn render_editor_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(4).min(80);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height / 2;
    let dialog_area = Rect::new(x, y, width, 3);

    frame.render_widget(Clear, dialog_area);
    let stage_num = app.pipeline.selected + 1;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Edit Stage {} Command ", stage_num))
        .style(Style::default().bg(Color::DarkGray));

    let cursor_pos = app.editor_cursor;
    let content = &app.editor_content;
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
            Span::styled(
                " ",
                Style::default().fg(Color::Black).bg(Color::White),
            ),
        ]
    };

    let para = Paragraph::new(Line::from(display)).block(block);
    frame.render_widget(para, dialog_area);
}

/// Render the save-to-file dialog overlay.
fn render_save_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(4).min(60);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + area.height / 2;
    let dialog_area = Rect::new(x, y, width, 3);

    frame.render_widget(Clear, dialog_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Save Output To File ")
        .style(Style::default().bg(Color::DarkGray));

    let cursor_pos = app.editor_cursor;
    let content = &app.editor_content;
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
            Span::styled(
                " ",
                Style::default().fg(Color::Black).bg(Color::White),
            ),
        ]
    };

    let para = Paragraph::new(Line::from(display)).block(block);
    frame.render_widget(para, dialog_area);
}

/// Render a simple list of keybindings for the help overlay.
pub fn render_help(frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = vec![
        ListItem::new("Tab / → / l    Next stage"),
        ListItem::new("Shift+Tab / ← / h  Previous stage"),
        ListItem::new("e / Enter      Edit current stage command"),
        ListItem::new("n / a          Add new stage after current"),
        ListItem::new("d              Delete current stage"),
        ListItem::new("r              Re-run (bypass cache)"),
        ListItem::new("s              Save output to file"),
        ListItem::new("1              Show stdout"),
        ListItem::new("2              Show stderr"),
        ListItem::new("3              Show combined (stdout+stderr)"),
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
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help — press ? to close ")
                .style(Style::default().bg(Color::DarkGray)),
        );
    frame.render_widget(list, help_area);
}

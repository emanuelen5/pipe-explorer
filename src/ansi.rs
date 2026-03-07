use std::collections::HashMap;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct AnsiStyleState {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HighlightKind {
    #[default]
    None,
    Other,
    Current,
}

fn style_with_highlight(base: AnsiStyleState, highlight: HighlightKind) -> Style {
    let style = base.as_style();
    match highlight {
        HighlightKind::None => style,
        HighlightKind::Other => style.fg(Color::Black).bg(Color::Gray),
        HighlightKind::Current => style
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    }
}

fn highlight_kind_for_range(
    highlights: &HashMap<usize, Vec<(usize, usize, bool)>>,
    line_idx: usize,
    start: usize,
    end: usize,
) -> HighlightKind {
    let Some(ranges) = highlights.get(&line_idx) else {
        return HighlightKind::None;
    };

    let mut kind = HighlightKind::None;
    for &(hl_start, hl_end, is_current) in ranges {
        // overlap between [start, end) and [hl_start, hl_end)
        if start < hl_end && hl_start < end {
            if is_current {
                return HighlightKind::Current;
            }
            kind = HighlightKind::Other;
        }
    }
    kind
}

impl AnsiStyleState {
    fn as_style(self) -> Style {
        let mut style = Style::default();
        if let Some(fg) = self.fg {
            style = style.fg(fg);
        }
        if let Some(bg) = self.bg {
            style = style.bg(bg);
        }
        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    }

    fn is_default(self) -> bool {
        self.fg.is_none() && self.bg.is_none() && !self.bold
    }
}

fn color_from_basic_ansi(code: u16) -> Option<Color> {
    Some(match code {
        30 | 40 => Color::Black,
        31 | 41 => Color::Red,
        32 | 42 => Color::Green,
        33 | 43 => Color::Yellow,
        34 | 44 => Color::Blue,
        35 | 45 => Color::Magenta,
        36 | 46 => Color::Cyan,
        37 | 47 => Color::Gray,
        90 | 100 => Color::DarkGray,
        91 | 101 => Color::LightRed,
        92 | 102 => Color::LightGreen,
        93 | 103 => Color::LightYellow,
        94 | 104 => Color::LightBlue,
        95 | 105 => Color::LightMagenta,
        96 | 106 => Color::LightCyan,
        97 | 107 => Color::White,
        _ => return None,
    })
}

fn parse_ansi_number(param: Option<&str>) -> Option<u16> {
    param.and_then(|p| p.parse::<u16>().ok())
}

pub(crate) fn apply_sgr_sequence(params: &str, style: &mut AnsiStyleState) {
    let mut parts = params.split(';').peekable();
    if params.is_empty() {
        *style = AnsiStyleState::default();
        return;
    }

    while let Some(raw) = parts.next() {
        let code = if raw.is_empty() {
            0
        } else {
            raw.parse::<u16>().unwrap_or(0)
        };

        match code {
            0 => *style = AnsiStyleState::default(),
            1 => style.bold = true,
            22 => style.bold = false,
            30..=37 | 90..=97 => {
                style.fg = color_from_basic_ansi(code);
            }
            40..=47 | 100..=107 => {
                style.bg = color_from_basic_ansi(code);
            }
            39 => style.fg = None,
            49 => style.bg = None,
            38 | 48 => {
                let is_fg = code == 38;
                match parse_ansi_number(parts.next()) {
                    Some(5) => {
                        if let Some(n) = parse_ansi_number(parts.next()) {
                            let color = Color::Indexed(n.min(u8::MAX as u16) as u8);
                            if is_fg {
                                style.fg = Some(color);
                            } else {
                                style.bg = Some(color);
                            }
                        }
                    }
                    Some(2) => {
                        let r = parse_ansi_number(parts.next()).unwrap_or(0).min(255) as u8;
                        let g = parse_ansi_number(parts.next()).unwrap_or(0).min(255) as u8;
                        let b = parse_ansi_number(parts.next()).unwrap_or(0).min(255) as u8;
                        let color = Color::Rgb(r, g, b);
                        if is_fg {
                            style.fg = Some(color);
                        } else {
                            style.bg = Some(color);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Pre-computed index of line-start byte offsets and ANSI style state.
///
/// Built incrementally as text grows (via `extend`), allowing
/// `ansi_text_to_visible_lines` to jump directly to any scroll position
/// without scanning all preceding bytes.
#[derive(Clone, Debug)]
pub struct AnsiLineIndex {
    /// Byte offset of the start of each line in the source text.
    offsets: Vec<usize>,
    /// ANSI style state at the start of each line.
    styles: Vec<AnsiStyleState>,
    /// How many bytes of the text have been scanned so far.
    scanned_up_to: usize,
    /// Style state at `scanned_up_to` (carried forward on next extend).
    trailing_style: AnsiStyleState,
}

impl AnsiLineIndex {
    /// Create an empty index (line 0 starts at byte 0 with default style).
    pub fn new() -> Self {
        Self {
            offsets: vec![0],
            styles: vec![AnsiStyleState::default()],
            scanned_up_to: 0,
            trailing_style: AnsiStyleState::default(),
        }
    }

    /// Extend the index by scanning any new content in `text` past what was
    /// previously scanned.  Only the new bytes are visited.
    pub fn extend(&mut self, text: &str) {
        let bytes = text.as_bytes();
        if bytes.len() <= self.scanned_up_to {
            return;
        }
        let mut i = self.scanned_up_to;
        let mut style = self.trailing_style;

        while i < bytes.len() {
            if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                let mut j = i + 2;
                while j < bytes.len() {
                    if (bytes[j] as char).is_ascii_alphabetic() {
                        break;
                    }
                    j += 1;
                }
                if j < bytes.len() {
                    if bytes[j] == b'm' {
                        let params = &text[i + 2..j];
                        apply_sgr_sequence(params, &mut style);
                    }
                    i = j + 1;
                    continue;
                }
            }
            if bytes[i] == b'\n' {
                self.offsets.push(i + 1);
                self.styles.push(style);
            }
            i += 1;
        }

        self.scanned_up_to = bytes.len();
        self.trailing_style = style;
    }
}

pub fn strip_ansi_sgr(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut out = String::new();

    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            while j < bytes.len() {
                let ch = bytes[j] as char;
                if ch.is_ascii_alphabetic() {
                    break;
                }
                j += 1;
            }
            if j < bytes.len() {
                i = j + 1;
                continue;
            }
        }

        let Some(ch) = text[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

pub fn strip_ansi_sgr_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            while j < bytes.len() {
                let b = bytes[j];
                if (0x40..=0x7E).contains(&b) {
                    break;
                }
                j += 1;
            }
            if j < bytes.len() {
                i = j + 1;
                continue;
            }
        }

        out.push(bytes[i]);
        i += 1;
    }

    out
}

/// Flush the current text segment into a `Span` and push it onto `spans`.
fn flush_segment(
    segment: &mut String,
    style: AnsiStyleState,
    highlight: HighlightKind,
    spans: &mut Vec<Span<'static>>,
) {
    if segment.is_empty() {
        return;
    }
    let text = std::mem::take(segment);
    if style.is_default() && highlight == HighlightKind::None {
        spans.push(Span::raw(text));
    } else {
        spans.push(Span::styled(text, style_with_highlight(style, highlight)));
    }
}

#[allow(dead_code)] // used by tests
pub fn ansi_text_to_lines(text: &str) -> Vec<Line<'static>> {
    ansi_text_to_lines_with_highlights(text, &HashMap::new())
}

#[allow(dead_code)] // used by tests + ansi_text_to_lines
pub fn ansi_text_to_lines_with_highlights(
    text: &str,
    highlights: &HashMap<usize, Vec<(usize, usize, bool)>>,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut segment = String::new();
    let mut style = AnsiStyleState::default();
    let mut line_idx = 0usize;
    let mut plain_col = 0usize;
    let mut segment_style: Option<(AnsiStyleState, HighlightKind)> = None;

    let bytes = text.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            while j < bytes.len() {
                let ch = bytes[j] as char;
                if ch.is_ascii_alphabetic() {
                    break;
                }
                j += 1;
            }

            if j < bytes.len() {
                let final_ch = bytes[j] as char;
                if final_ch == 'm' {
                    let (seg_style, seg_highlight) =
                        segment_style.unwrap_or((style, HighlightKind::None));
                    flush_segment(&mut segment, seg_style, seg_highlight, &mut spans);
                    let params = &text[i + 2..j];
                    apply_sgr_sequence(params, &mut style);
                    segment_style = None;
                }
                i = j + 1;
                continue;
            }
        }

        let Some(ch) = text[i..].chars().next() else {
            break;
        };
        let ch_len = ch.len_utf8();
        i += ch_len;
        if ch == '\n' {
            let (seg_style, seg_highlight) = segment_style.unwrap_or((style, HighlightKind::None));
            flush_segment(&mut segment, seg_style, seg_highlight, &mut spans);
            lines.push(Line::from(std::mem::take(&mut spans)));
            segment_style = None;
            plain_col = 0;
            line_idx += 1;
        } else {
            let highlight =
                highlight_kind_for_range(highlights, line_idx, plain_col, plain_col + ch_len);
            let desired = (style, highlight);
            if let Some(current) = segment_style {
                if current != desired {
                    let (seg_style, seg_highlight) = current;
                    flush_segment(&mut segment, seg_style, seg_highlight, &mut spans);
                    segment_style = Some(desired);
                }
            } else {
                segment_style = Some(desired);
            }
            segment.push(ch);
            plain_col += ch_len;
        }
    }

    let (seg_style, seg_highlight) = segment_style.unwrap_or((style, HighlightKind::None));
    flush_segment(&mut segment, seg_style, seg_highlight, &mut spans);
    if !spans.is_empty() || text.ends_with('\n') {
        lines.push(Line::from(spans));
    }

    lines
}

/// Parse only the visible window `[start_line, start_line + max_lines)` from
/// ANSI-escaped text.
///
/// The function uses a two-phase approach:
///  1. **Byte scan** – lines before `start_line` are scanned at the raw byte
///     level (no UTF-8 decoding, no `String` allocations).  Only ANSI SGR
///     escape sequences and `\n` bytes are recognised so that style state
///     carries over correctly.
///  2. **Full parse** – lines inside the visible window are parsed
///     character-by-character to build styled `Span`/`Line` objects.
///
/// This eliminates the main bottleneck (50 % of CPU in profiling) caused by
/// per-character UTF-8 decoding of all lines above the scroll position.
pub fn ansi_text_to_visible_lines(
    text: &str,
    start_line: usize,
    max_lines: usize,
    highlights: &HashMap<usize, Vec<(usize, usize, bool)>>,
    line_index: Option<&AnsiLineIndex>,
) -> Vec<Line<'static>> {
    let end_line = start_line + max_lines;
    let bytes = text.as_bytes();
    let mut style = AnsiStyleState::default();
    let mut line_idx = 0usize;
    let mut i = 0usize;

    // --- Phase 1: seek to `start_line` -------------------------------------------
    // If a pre-built line index is available and covers `start_line`, jump
    // directly to the byte offset and restore the ANSI style — O(1).
    // Otherwise fall back to a byte-level scan.
    let indexed = line_index
        .filter(|idx| start_line < idx.offsets.len())
        .map(|idx| (idx.offsets[start_line], idx.styles[start_line]));

    if let Some((offset, cached_style)) = indexed {
        i = offset;
        style = cached_style;
        line_idx = start_line;
    } else {
        while i < bytes.len() && line_idx < start_line {
            if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                let mut j = i + 2;
                while j < bytes.len() {
                    if (bytes[j] as char).is_ascii_alphabetic() {
                        break;
                    }
                    j += 1;
                }
                if j < bytes.len() {
                    if bytes[j] == b'm' {
                        let params = &text[i + 2..j];
                        apply_sgr_sequence(params, &mut style);
                    }
                    i = j + 1;
                } else {
                    i += 1;
                }
                continue;
            }
            if bytes[i] == b'\n' {
                line_idx += 1;
            }
            i += 1;
        }
    }

    // --- Phase 2: full parse for the visible window ------------------------------
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut segment = String::new();
    let mut plain_col = 0usize;
    let mut segment_style: Option<(AnsiStyleState, HighlightKind)> = None;

    while i < bytes.len() {
        if line_idx >= end_line {
            break;
        }

        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            while j < bytes.len() {
                if (bytes[j] as char).is_ascii_alphabetic() {
                    break;
                }
                j += 1;
            }
            if j < bytes.len() {
                if bytes[j] == b'm' {
                    let (ss, sh) = segment_style.unwrap_or((style, HighlightKind::None));
                    flush_segment(&mut segment, ss, sh, &mut spans);
                    segment_style = None;
                    let params = &text[i + 2..j];
                    apply_sgr_sequence(params, &mut style);
                }
                i = j + 1;
                continue;
            }
        }

        let Some(ch) = text[i..].chars().next() else {
            break;
        };
        let ch_len = ch.len_utf8();
        i += ch_len;

        if ch == '\n' {
            let (ss, sh) = segment_style.unwrap_or((style, HighlightKind::None));
            flush_segment(&mut segment, ss, sh, &mut spans);
            lines.push(Line::from(std::mem::take(&mut spans)));
            segment_style = None;
            plain_col = 0;
            line_idx += 1;
        } else {
            let highlight =
                highlight_kind_for_range(highlights, line_idx, plain_col, plain_col + ch_len);
            let desired = (style, highlight);
            if let Some(current) = segment_style {
                if current != desired {
                    let (ss, sh) = current;
                    flush_segment(&mut segment, ss, sh, &mut spans);
                    segment_style = Some(desired);
                }
            } else {
                segment_style = Some(desired);
            }
            segment.push(ch);
            plain_col += ch_len;
        }
    }

    // Handle the last visible line (no trailing '\n').
    if line_idx >= start_line && line_idx < end_line {
        let (ss, sh) = segment_style.unwrap_or((style, HighlightKind::None));
        flush_segment(&mut segment, ss, sh, &mut spans);
        if !spans.is_empty() || text.ends_with('\n') {
            lines.push(Line::from(spans));
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ratatui::style::Color;

    use super::{ansi_text_to_lines_with_highlights, strip_ansi_sgr, strip_ansi_sgr_bytes};

    #[test]
    fn strip_ansi_removes_sgr_sequences() {
        let input = "\u{1b}[31mred\u{1b}[0m plain \u{1b}[1;34mbold-blue\u{1b}[0m";
        let output = strip_ansi_sgr(input);
        assert_eq!(output, "red plain bold-blue");
    }

    #[test]
    fn strip_ansi_preserves_newlines() {
        let input = "first\n\u{1b}[32msecond\u{1b}[0m\nthird";
        let output = strip_ansi_sgr(input);
        assert_eq!(output, "first\nsecond\nthird");
    }

    #[test]
    fn strip_ansi_bytes_removes_escape_sequences() {
        let input = b"\x1b[31mhello\x1b[0m\n";
        let output = strip_ansi_sgr_bytes(input);
        assert_eq!(output, b"hello\n");
    }

    #[test]
    fn highlight_preserves_text_and_applies_background() {
        let input = "\x1b[31merror\x1b[0m ok";
        let mut highlights: HashMap<usize, Vec<(usize, usize, bool)>> = HashMap::new();
        highlights.insert(0, vec![(0, 5, true)]);

        let lines = ansi_text_to_lines_with_highlights(input, &highlights);
        assert_eq!(lines.len(), 1);
        let combined = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert_eq!(combined, "error ok");
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.style.bg == Some(Color::Yellow))
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.style.fg == Some(Color::Black))
        );
    }
}

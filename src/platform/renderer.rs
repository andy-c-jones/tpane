use std::io::{self, Stdout};

use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::RenderableContent;
use alacritty_terminal::index::{Column, Line, Point};
use anyhow::Result;
use crossterm::event::{KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect as TuiRect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TuiLine, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use crate::core::layout::{Layout, PaneId};
use crate::platform::pane::PaneState;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Enter raw mode and alternate screen, return a ratatui Terminal.
pub fn init_terminal() -> Result<Tui> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
    )?;
    let backend = CrosstermBackend::new(io::stdout());
    Ok(Terminal::new(backend)?)
}

/// Restore the terminal to its previous state.
pub fn restore_terminal(tui: &mut Tui) -> Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        tui.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture,
    )?;
    tui.show_cursor()?;
    Ok(())
}

/// Render the full tpane UI: one bordered block per pane, content from the Term grid.
pub fn render(
    tui: &mut Tui,
    layout: &Layout,
    panes: &std::collections::HashMap<PaneId, PaneState>,
    terminal_size: (u16, u16),
    prefix_active: bool,
) -> Result<()> {
    tui.draw(|frame| {
        let (w, h) = terminal_size;

        // Reserve space for cheatsheet bar when prefix is active.
        let cheatsheet_height: u16 = if prefix_active { 3 } else { 0 };
        let pane_area_h = h.saturating_sub(cheatsheet_height);
        let rects = layout.compute_rects(w, pane_area_h);

        for (pane_id, rect) in &rects {
            let is_active = *pane_id == layout.active;
            let tui_rect = TuiRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            };

            let border_style = if is_active {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let title = if is_active { " tpane [active] " } else { " tpane " };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title);

            // Inner area available for terminal content (inside borders).
            let inner = block.inner(tui_rect);
            frame.render_widget(block, tui_rect);

            if let Some(pane) = panes.get(pane_id) {
                let content = term_to_text(pane, inner.width, inner.height);
                let para = Paragraph::new(content);
                frame.render_widget(para, inner);
            }
        }

        // Render cheatsheet bar at the bottom.
        if prefix_active && cheatsheet_height > 0 && h > cheatsheet_height {
            render_cheatsheet(frame, w, h, cheatsheet_height);
        }
    })?;
    Ok(())
}

/// Draw a styled cheatsheet bar showing available keybindings.
fn render_cheatsheet(
    frame: &mut ratatui::Frame,
    w: u16,
    h: u16,
    bar_height: u16,
) {
    let bar_rect = TuiRect {
        x: 0,
        y: h.saturating_sub(bar_height),
        width: w,
        height: bar_height,
    };

    let key_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let sep_style = Style::default().fg(Color::DarkGray);
    let desc_style = Style::default().fg(Color::White);
    let title_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let bindings: &[(&str, &str)] = &[
        ("Ctrl+←", "Split Left"),
        ("Ctrl+→", "Split Right"),
        ("Ctrl+↑", "Split Up"),
        ("Ctrl+↓", "Split Down"),
        ("←↑↓→", "Focus"),
        ("w", "Close"),
        ("q", "Quit"),
    ];

    let mut spans: Vec<Span> = vec![
        Span::styled(" tpane ", title_style),
        Span::styled("│ ", sep_style),
    ];
    for (i, (key, desc)) in bindings.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │ ", sep_style));
        }
        spans.push(Span::styled(*key, key_style));
        spans.push(Span::styled(" ", desc_style));
        spans.push(Span::styled(*desc, desc_style));
    }

    let line = TuiLine::from(spans);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Keybindings ", title_style));
    let para = Paragraph::new(Text::from(line)).block(block);
    frame.render_widget(para, bar_rect);
}

/// Convert the alacritty Term grid into ratatui Text for display.
fn term_to_text(
    pane: &PaneState,
    width: u16,
    height: u16,
) -> Text<'static> {
    let term = pane.term.lock();
    let content: RenderableContent<'_> = term.renderable_content();

    let rows = height as usize;
    let cols = width as usize;
    let mut lines: Vec<TuiLine<'static>> = Vec::with_capacity(rows);

    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        for col in 0..cols {
            let point = Point::new(Line(row as i32 - content.display_offset as i32), Column(col));
            // Access cell via the grid directly
            let cell = term.grid()[point].clone();
            let (ch, style) = cell_to_span(&cell);

            if style == current_style {
                current_text.push(ch);
            } else {
                if !current_text.is_empty() {
                    spans.push(Span::styled(current_text.clone(), current_style));
                    current_text.clear();
                }
                current_text.push(ch);
                current_style = style;
            }
        }
        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }
        lines.push(TuiLine::from(spans));
    }

    Text::from(lines)
}

/// Convert an alacritty cell to a (char, ratatui Style) pair.
fn cell_to_span(cell: &Cell) -> (char, Style) {
    use alacritty_terminal::term::color::Colors;
    use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor};

    let ch = if cell.c == '\0' { ' ' } else { cell.c };

    let fg = ansi_color_to_ratatui(cell.fg);
    let bg = ansi_color_to_ratatui(cell.bg);

    let mut style = Style::default();
    if let Some(c) = fg {
        style = style.fg(c);
    }
    if let Some(c) = bg {
        style = style.bg(c);
    }
    if cell.flags.contains(alacritty_terminal::term::cell::Flags::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.flags.contains(alacritty_terminal::term::cell::Flags::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.flags.contains(alacritty_terminal::term::cell::Flags::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    (ch, style)
}

fn ansi_color_to_ratatui(color: alacritty_terminal::vte::ansi::Color) -> Option<Color> {
    use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor};
    match color {
        AColor::Named(NamedColor::Black)   => Some(Color::Black),
        AColor::Named(NamedColor::Red)     => Some(Color::Red),
        AColor::Named(NamedColor::Green)   => Some(Color::Green),
        AColor::Named(NamedColor::Yellow)  => Some(Color::Yellow),
        AColor::Named(NamedColor::Blue)    => Some(Color::Blue),
        AColor::Named(NamedColor::Magenta) => Some(Color::Magenta),
        AColor::Named(NamedColor::Cyan)    => Some(Color::Cyan),
        AColor::Named(NamedColor::White)   => Some(Color::White),
        AColor::Named(NamedColor::BrightBlack)   => Some(Color::DarkGray),
        AColor::Named(NamedColor::BrightRed)     => Some(Color::LightRed),
        AColor::Named(NamedColor::BrightGreen)   => Some(Color::LightGreen),
        AColor::Named(NamedColor::BrightYellow)  => Some(Color::LightYellow),
        AColor::Named(NamedColor::BrightBlue)    => Some(Color::LightBlue),
        AColor::Named(NamedColor::BrightMagenta) => Some(Color::LightMagenta),
        AColor::Named(NamedColor::BrightCyan)    => Some(Color::LightCyan),
        AColor::Named(NamedColor::BrightWhite)   => Some(Color::White),
        AColor::Spec(rgb) => Some(Color::Rgb(rgb.r, rgb.g, rgb.b)),
        AColor::Indexed(i) => Some(Color::Indexed(i)),
        _ => None,
    }
}

/// Translate a crossterm key event into bytes to send to the PTY.
pub fn key_event_to_bytes(event: &crossterm::event::KeyEvent) -> Option<Vec<u8>> {
    use crossterm::event::{KeyCode, KeyModifiers};
    if event.kind != KeyEventKind::Press {
        return None;
    }
    let bytes = match event.code {
        KeyCode::Char(c) => {
            if event.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+letter → control byte
                let b = c.to_ascii_lowercase() as u8;
                if b >= b'a' && b <= b'z' {
                    vec![b - b'a' + 1]
                } else {
                    return None;
                }
            } else {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Home => vec![0x1b, b'[', b'H'],
        KeyCode::End => vec![0x1b, b'[', b'F'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],
        KeyCode::F(n) => {
            let bytes = f_key_bytes(n);
            if bytes.is_empty() { return None; }
            bytes
        }
        _ => return None,
    };
    Some(bytes)
}

fn f_key_bytes(n: u8) -> Vec<u8> {
    match n {
        1 => vec![0x1b, b'O', b'P'],
        2 => vec![0x1b, b'O', b'Q'],
        3 => vec![0x1b, b'O', b'R'],
        4 => vec![0x1b, b'O', b'S'],
        5 => vec![0x1b, b'[', b'1', b'5', b'~'],
        6 => vec![0x1b, b'[', b'1', b'7', b'~'],
        7 => vec![0x1b, b'[', b'1', b'8', b'~'],
        8 => vec![0x1b, b'[', b'1', b'9', b'~'],
        9 => vec![0x1b, b'[', b'2', b'0', b'~'],
        10 => vec![0x1b, b'[', b'2', b'1', b'~'],
        11 => vec![0x1b, b'[', b'2', b'3', b'~'],
        12 => vec![0x1b, b'[', b'2', b'4', b'~'],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: crossterm::event::KeyEventState::empty() }
    }
    fn release(code: KeyCode) -> KeyEvent {
        KeyEvent { code, modifiers: KeyModifiers::empty(), kind: KeyEventKind::Release, state: crossterm::event::KeyEventState::empty() }
    }
    fn repeat(code: KeyCode) -> KeyEvent {
        KeyEvent { code, modifiers: KeyModifiers::empty(), kind: KeyEventKind::Repeat, state: crossterm::event::KeyEventState::empty() }
    }

    // ── key_event_to_bytes ────────────────────────────────────────────────────

    #[test]
    fn key_release_returns_none() {
        assert!(key_event_to_bytes(&release(KeyCode::Char('a'))).is_none());
    }

    #[test]
    fn key_repeat_returns_none() {
        assert!(key_event_to_bytes(&repeat(KeyCode::Char('a'))).is_none());
    }

    #[test]
    fn ascii_char_roundtrip() {
        let event = press(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(key_event_to_bytes(&event), Some(vec![b'a']));
    }

    #[test]
    fn unicode_char_encoded_as_utf8() {
        let event = press(KeyCode::Char('é'), KeyModifiers::empty());
        let bytes = key_event_to_bytes(&event).unwrap();
        // 'é' is U+00E9, 2-byte UTF-8: 0xC3 0xA9
        assert_eq!(bytes, vec![0xC3, 0xA9]);
    }

    #[test]
    fn ctrl_alpha_sends_control_byte() {
        for (ch, expected) in [('a', 1u8), ('c', 3), ('z', 26)] {
            let event = press(KeyCode::Char(ch), KeyModifiers::CONTROL);
            assert_eq!(key_event_to_bytes(&event), Some(vec![expected]), "ctrl+{ch}");
        }
    }

    #[test]
    fn ctrl_non_alpha_returns_none() {
        let event = press(KeyCode::Char('1'), KeyModifiers::CONTROL);
        assert!(key_event_to_bytes(&event).is_none());
    }

    #[test]
    fn special_keys() {
        let cases = [
            (KeyCode::Enter,     vec![b'\r']),
            (KeyCode::Backspace, vec![0x7f]),
            (KeyCode::Tab,       vec![b'\t']),
            (KeyCode::Esc,       vec![0x1b]),
            (KeyCode::Up,        vec![0x1b, b'[', b'A']),
            (KeyCode::Down,      vec![0x1b, b'[', b'B']),
            (KeyCode::Right,     vec![0x1b, b'[', b'C']),
            (KeyCode::Left,      vec![0x1b, b'[', b'D']),
            (KeyCode::Home,      vec![0x1b, b'[', b'H']),
            (KeyCode::End,       vec![0x1b, b'[', b'F']),
            (KeyCode::Delete,    vec![0x1b, b'[', b'3', b'~']),
            (KeyCode::PageUp,    vec![0x1b, b'[', b'5', b'~']),
            (KeyCode::PageDown,  vec![0x1b, b'[', b'6', b'~']),
        ];
        for (code, expected) in cases {
            let event = press(code, KeyModifiers::empty());
            assert_eq!(key_event_to_bytes(&event), Some(expected), "{code:?}");
        }
    }

    #[test]
    fn function_keys_f1_f12_produce_bytes() {
        for n in 1u8..=12 {
            let event = press(KeyCode::F(n), KeyModifiers::empty());
            let result = key_event_to_bytes(&event);
            assert!(result.is_some(), "F{n} returned None");
            assert!(!result.unwrap().is_empty(), "F{n} returned empty bytes");
        }
    }

    #[test]
    fn function_keys_f13_plus_return_none() {
        for n in [13u8, 20, 99] {
            let event = press(KeyCode::F(n), KeyModifiers::empty());
            assert!(key_event_to_bytes(&event).is_none(), "F{n} should return None");
        }
    }

    #[test]
    fn unsupported_key_returns_none() {
        let event = press(KeyCode::Insert, KeyModifiers::empty());
        assert!(key_event_to_bytes(&event).is_none());
    }

    // ── f_key_bytes ───────────────────────────────────────────────────────────

    #[test]
    fn f_key_bytes_boundaries() {
        assert!(!f_key_bytes(1).is_empty());
        assert!(!f_key_bytes(12).is_empty());
        assert!(f_key_bytes(0).is_empty());
        assert!(f_key_bytes(13).is_empty());
    }
}


use std::io::{self, Stdout};
use std::time::Instant;

use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::RenderableContent;
use alacritty_terminal::index::{Column, Line, Point};
use anyhow::Result;
use crossterm::event::{KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Rect as TuiRect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TuiLine, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use crate::core::layout::{Layout, PaneId};
use crate::core::selection::Selection;
use crate::platform::pane::PaneState;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Braille spinner frames — a smooth rotating dot pattern.
const BRAILLE_SPINNER: &[char] = &[
    '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏',
];

/// Start time used to derive animation frame from wall clock.
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

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
    selection: Option<&Selection>,
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

            // Use the terminal's OSC title if set, otherwise "tpane".
            let pane_title = panes.get(pane_id)
                .map(|p| p.title())
                .filter(|t| !t.is_empty());
            let title = if is_active {
                match &pane_title {
                    Some(t) => format!(" {} [active] ", t),
                    None => " tpane [active] ".to_string(),
                }
            } else {
                match &pane_title {
                    Some(t) => format!(" {} ", t),
                    None => " tpane ".to_string(),
                }
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title);

            // Inner area available for terminal content (inside borders).
            let inner = block.inner(tui_rect);
            frame.render_widget(block, tui_rect);

            if let Some(pane) = panes.get(pane_id) {
                if term_has_visible_content(pane, inner.width, inner.height) {
                    // Get selection range for this pane (if any).
                    let sel_range = selection
                        .filter(|s| s.pane_id == *pane_id && !s.is_empty())
                        .map(|s| s.ordered());

                    let content = term_to_text(pane, inner.width, inner.height, sel_range);
                    let para = Paragraph::new(content);
                    frame.render_widget(para, inner);
                } else {
                    // Braille loading throbber for panes still spawning.
                    let start = START.get_or_init(Instant::now);
                    let elapsed_ms = start.elapsed().as_millis() as usize;
                    let frame_idx = (elapsed_ms / 80) % BRAILLE_SPINNER.len();
                    let spinner = BRAILLE_SPINNER[frame_idx];

                    let loading = TuiLine::from(vec![
                        Span::styled(
                            format!(" {} Loading shell…", spinner),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]);
                    let para = Paragraph::new(Text::from(loading))
                        .alignment(Alignment::Center);
                    // Center vertically by rendering into a sub-rect.
                    let center_y = inner.y + inner.height / 2;
                    let center_rect = TuiRect {
                        x: inner.x,
                        y: center_y,
                        width: inner.width,
                        height: 1,
                    };
                    frame.render_widget(para, center_rect);
                }
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
        ("←↑↓→", "Focus Pane"),
        ("w", "Close Pane"),
        ("q", "Quit tpane"),
    ];

    // Global bindings shown after the separator.
    let global_bindings: &[(&str, &str)] = &[
        ("Ctrl+Shift+C", "Copy"),
        ("Ctrl+Shift+V", "Paste"),
        ("Right-Click", "Copy"),
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
    // Global bindings (always available, not prefix-dependent).
    for (key, desc) in global_bindings {
        spans.push(Span::styled(" │ ", sep_style));
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

/// Check if the terminal grid has any visible (non-space, non-null) content.
/// Used to decide whether to show the loading throbber or real terminal content.
fn term_has_visible_content(pane: &PaneState, width: u16, height: u16) -> bool {
    let term = pane.term.lock();
    let content: RenderableContent<'_> = term.renderable_content();
    let rows = height as usize;
    let cols = width as usize;

    for row in 0..rows {
        for col in 0..cols {
            let point = Point::new(Line(row as i32 - content.display_offset as i32), Column(col));
            let c = term.grid()[point].c;
            if c != '\0' && c != ' ' {
                return true;
            }
        }
    }
    false
}

/// Convert the alacritty Term grid into ratatui Text for display.
/// If `sel_range` is Some, cells within the selection are rendered with inverted colors.
fn term_to_text(
    pane: &PaneState,
    width: u16,
    height: u16,
    sel_range: Option<((u16, u16), (u16, u16))>,
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
            let (ch, mut style) = cell_to_span(&cell);

            // Apply selection highlight (inverted colors).
            if let Some(((sc, sr), (ec, er))) = sel_range {
                let r = row as u16;
                let c = col as u16;
                let in_sel = if sr == er {
                    r == sr && c >= sc && c <= ec
                } else if r == sr {
                    c >= sc
                } else if r == er {
                    c <= ec
                } else {
                    r > sr && r < er
                };
                if in_sel {
                    // Swap fg/bg for selection highlight.
                    let fg = style.bg.unwrap_or(Color::Black);
                    let bg = style.fg.unwrap_or(Color::White);
                    style = style.fg(fg).bg(bg);
                }
            }

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
    let mods = event.modifiers;
    let bytes = match event.code {
        KeyCode::Char(c) => {
            if mods.contains(KeyModifiers::CONTROL) {
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
        KeyCode::Tab => {
            if mods.contains(KeyModifiers::SHIFT) {
                vec![0x1b, b'[', b'Z'] // Shift+Tab = CSI Z
            } else {
                vec![b'\t']
            }
        }
        KeyCode::BackTab => vec![0x1b, b'[', b'Z'], // Shift+Tab
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up    => csi_with_modifier(b'A', mods),
        KeyCode::Down  => csi_with_modifier(b'B', mods),
        KeyCode::Right => csi_with_modifier(b'C', mods),
        KeyCode::Left  => csi_with_modifier(b'D', mods),
        KeyCode::Home  => csi_with_modifier(b'H', mods),
        KeyCode::End   => csi_with_modifier(b'F', mods),
        KeyCode::Insert => vec![0x1b, b'[', b'2', b'~'],
        KeyCode::Delete => tilde_with_modifier(3, mods),
        KeyCode::PageUp => tilde_with_modifier(5, mods),
        KeyCode::PageDown => tilde_with_modifier(6, mods),
        KeyCode::F(n) => {
            let bytes = f_key_bytes(n);
            if bytes.is_empty() { return None; }
            bytes
        }
        _ => return None,
    };
    Some(bytes)
}

/// Encode CSI sequences with modifier parameter (e.g. \e[1;2A for Shift+Up).
fn csi_with_modifier(code: u8, mods: crossterm::event::KeyModifiers) -> Vec<u8> {
    let m = modifier_param(mods);
    if m > 1 {
        // \e[1;{mod}{code}
        let ms = m.to_string();
        let mut v = vec![0x1b, b'[', b'1', b';'];
        v.extend_from_slice(ms.as_bytes());
        v.push(code);
        v
    } else {
        vec![0x1b, b'[', code]
    }
}

/// Encode tilde sequences with modifier parameter (e.g. \e[3;2~ for Shift+Delete).
fn tilde_with_modifier(n: u8, mods: crossterm::event::KeyModifiers) -> Vec<u8> {
    let m = modifier_param(mods);
    if m > 1 {
        let ms = m.to_string();
        let mut v = vec![0x1b, b'[', b'0' + n, b';'];
        v.extend_from_slice(ms.as_bytes());
        v.push(b'~');
        v
    } else {
        vec![0x1b, b'[', b'0' + n, b'~']
    }
}

/// xterm modifier parameter: 1 + (shift?1:0) + (alt?2:0) + (ctrl?4:0).
fn modifier_param(mods: crossterm::event::KeyModifiers) -> u8 {
    use crossterm::event::KeyModifiers;
    let mut m: u8 = 1;
    if mods.contains(KeyModifiers::SHIFT) { m += 1; }
    if mods.contains(KeyModifiers::ALT)   { m += 2; }
    if mods.contains(KeyModifiers::CONTROL) { m += 4; }
    m
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
    fn insert_key_returns_bytes() {
        let event = press(KeyCode::Insert, KeyModifiers::empty());
        assert_eq!(key_event_to_bytes(&event), Some(vec![0x1b, b'[', b'2', b'~']));
    }

    // ── f_key_bytes ───────────────────────────────────────────────────────────

    #[test]
    fn f_key_bytes_boundaries() {
        assert!(!f_key_bytes(1).is_empty());
        assert!(!f_key_bytes(12).is_empty());
        assert!(f_key_bytes(0).is_empty());
        assert!(f_key_bytes(13).is_empty());
    }

    // ── BackTab / Shift+Tab ──────────────────────────────────────────────────

    #[test]
    fn backtab_returns_csi_z() {
        let event = press(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(key_event_to_bytes(&event), Some(vec![0x1b, b'[', b'Z']));
    }

    #[test]
    fn shift_tab_returns_csi_z() {
        let event = press(KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(key_event_to_bytes(&event), Some(vec![0x1b, b'[', b'Z']));
    }

    // ── modifier-aware arrow sequences ───────────────────────────────────────

    #[test]
    fn shift_up_returns_modified_csi() {
        let event = press(KeyCode::Up, KeyModifiers::SHIFT);
        // \e[1;2A
        assert_eq!(key_event_to_bytes(&event), Some(vec![0x1b, b'[', b'1', b';', b'2', b'A']));
    }

    #[test]
    fn ctrl_right_returns_modified_csi() {
        let event = press(KeyCode::Right, KeyModifiers::CONTROL);
        // \e[1;5C
        assert_eq!(key_event_to_bytes(&event), Some(vec![0x1b, b'[', b'1', b';', b'5', b'C']));
    }

    #[test]
    fn plain_arrow_no_modifier() {
        let event = press(KeyCode::Left, KeyModifiers::empty());
        assert_eq!(key_event_to_bytes(&event), Some(vec![0x1b, b'[', b'D']));
    }
}


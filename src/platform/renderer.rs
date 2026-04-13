//! Terminal UI rendering and key-to-PTY byte translation.
//!
//! This module draws pane borders/content/cheatsheet and provides helpers for
//! translating crossterm key events into byte sequences expected by shells.

use std::collections::HashMap;
use std::io::{self, Stdout, Write};
use std::time::Instant;

use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::CursorShape;
use anyhow::Result;
use crossterm::event::KeyEventKind;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Rect as TuiRect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TuiLine, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use crate::core::commands::Command;
use crate::core::keymap::{KeyChord, KeyMap};
use crate::core::layout::{Layout, PaneId};
use crate::core::selection::Selection;
use crate::platform::pane::PaneState;

/// Concrete terminal type used by tpane's live renderer.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Braille spinner frames — a smooth rotating dot pattern.
const BRAILLE_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Start time used to derive animation frame from wall clock.
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PaneRenderKey {
    content_version: u64,
    width: u16,
    height: u16,
    sel_range: Option<((u16, u16), (u16, u16))>,
    ready: bool,
}

#[derive(Debug, Clone)]
struct PaneRenderCache {
    key: PaneRenderKey,
    content: Option<Vec<TuiLine<'static>>>,
    /// Cursor position relative to the pane's inner content area, if visible.
    cursor_pos: Option<(u16, u16)>,
    /// Cells in this pane that carry OSC 8 hyperlinks.
    hyperlinks: Vec<HyperlinkCell>,
}

/// A single terminal cell that carries an OSC 8 hyperlink.
///
/// Style is captured after all transforms (selection inversion, cursor inversion)
/// so that re-rendering in the post-draw pass is pixel-accurate.
#[derive(Debug, Clone)]
struct HyperlinkCell {
    /// Row relative to the pane's inner content area.
    row: u16,
    /// Column relative to the pane's inner content area.
    col: u16,
    ch: char,
    style: Style,
    uri: String,
    id: String,
}

/// Cache object reused across frames by the live renderer.
///
/// This stores pane content/title snapshots plus cheatsheet layout derivations
/// so repeated renders avoid unnecessary recomputation when inputs are stable.
#[derive(Default)]
pub struct RenderCache {
    pane_content: HashMap<PaneId, PaneRenderCache>,
    pane_titles: HashMap<PaneId, (u64, Option<String>)>,
    cheatsheet_entries: Option<Vec<(String, &'static str)>>,
    cheatsheet_layouts: HashMap<usize, CheatsheetGridLayout>,
}

/// Enter raw mode and alternate screen, return a ratatui Terminal.
///
/// # Errors
///
/// Returns terminal setup errors from crossterm or ratatui initialization.
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
///
/// # Errors
///
/// Returns an error if terminal teardown operations fail.
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
///
/// # Behavior
///
/// When `prefix_active` is true, a cheatsheet bar is rendered at the bottom and
/// pane geometry is computed against the remaining vertical space.
///
/// # Errors
///
/// Returns ratatui backend errors from frame drawing.
pub fn render(
    tui: &mut Tui,
    cache: &mut RenderCache,
    layout: &Layout,
    panes: &std::collections::HashMap<PaneId, PaneState>,
    keymap: &KeyMap,
    terminal_size: (u16, u16),
    prefix_active: bool,
    selection: Option<&Selection>,
) -> Result<()> {
    let (w, h) = terminal_size;

    // Compute cheatsheet height before the draw closure so it is available for
    // the post-draw hyperlink pass that needs the same pane geometry.
    let cheatsheet_height: u16 = if prefix_active {
        cheatsheet_bar_height_cached(cache, w, keymap)
    } else {
        0
    };
    let pane_area_h = h.saturating_sub(cheatsheet_height);

    tui.draw(|frame| {
        let (rects, dividers) = layout.compute_geometry(w, pane_area_h);
        cache.pane_content.retain(|id, _| rects.contains_key(id));
        cache.pane_titles.retain(|id, _| rects.contains_key(id));

        for (pane_id, rect) in &rects {
            // Skip panes with no visible area (can happen during resize).
            if rect.width < 2 || rect.height < 2 {
                continue;
            }

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
            let pane_title = panes.get(pane_id).and_then(|pane| {
                let title_version = pane.title_version();
                match cache.pane_titles.get(pane_id) {
                    Some((cached_version, cached_title)) if *cached_version == title_version => {
                        cached_title.clone()
                    }
                    _ => {
                        let title = pane.title();
                        let cached = if title.is_empty() { None } else { Some(title) };
                        cache
                            .pane_titles
                            .insert(*pane_id, (title_version, cached.clone()));
                        cached
                    }
                }
            });
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

            // Skip rendering content if inner area is empty.
            if inner.width == 0 || inner.height == 0 {
                continue;
            }

            if let Some(pane) = panes.get(pane_id) {
                // Get selection range for this pane (if any).
                let sel_range = selection
                    .filter(|s| s.pane_id == *pane_id && !s.is_empty())
                    .map(|s| s.ordered());
                let key = PaneRenderKey {
                    content_version: pane.content_version(),
                    width: inner.width,
                    height: inner.height,
                    sel_range,
                    ready: pane.is_ready(),
                };
                let content = match cache.pane_content.get(pane_id) {
                    Some(cached) if cached.key == key => cached.content.as_ref(),
                    _ => {
                        let built = term_to_lines(pane, inner.width, inner.height, sel_range);
                        cache.pane_content.insert(
                            *pane_id,
                            PaneRenderCache {
                                key,
                                content: built.lines,
                                cursor_pos: built.cursor_pos,
                                hyperlinks: built.hyperlinks,
                            },
                        );
                        cache
                            .pane_content
                            .get(pane_id)
                            .and_then(|cached| cached.content.as_ref())
                    }
                };

                if let Some(lines) = content {
                    let buf = frame.buffer_mut();
                    for (row, line) in lines.iter().take(inner.height as usize).enumerate() {
                        buf.set_line(inner.x, inner.y + row as u16, line, inner.width);
                    }
                } else {
                    // Braille loading throbber for panes still spawning.
                    let start = START.get_or_init(Instant::now);
                    let elapsed_ms = start.elapsed().as_millis() as usize;
                    let frame_idx = (elapsed_ms / 80) % BRAILLE_SPINNER.len();
                    let spinner = BRAILLE_SPINNER[frame_idx];

                    let loading = TuiLine::from(vec![Span::styled(
                        format!(" {} Loading shell…", spinner),
                        Style::default().fg(Color::DarkGray),
                    )]);
                    let para = Paragraph::new(Text::from(loading)).alignment(Alignment::Center);
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

                // Position the hardware cursor inside the active pane.
                if is_active {
                    if let Some((col, row)) =
                        cache.pane_content.get(pane_id).and_then(|c| c.cursor_pos)
                    {
                        frame.set_cursor_position((inner.x + col, inner.y + row));
                    }
                }
            }
        }

        // Render cheatsheet bar at the bottom.
        if prefix_active && cheatsheet_height > 0 && h > cheatsheet_height {
            render_cheatsheet(frame, cache, keymap, w, h, cheatsheet_height);
        }

        // Render subtle grab-handle dots on each divider so users know they're draggable.
        let handle_style = Style::default().fg(Color::DarkGray);
        let buf = frame.buffer_mut();
        for div in &dividers {
            match div.orientation {
                crate::core::layout::Orientation::Vertical => {
                    // Paint a thin vertical line of '│' in the gap column.
                    for row in div.span_start..div.span_end {
                        buf[(div.position, row)]
                            .set_symbol("│")
                            .set_style(handle_style);
                    }
                }
                crate::core::layout::Orientation::Horizontal => {
                    // Paint a thin horizontal line of '─' in the gap row.
                    for col in div.span_start..div.span_end {
                        buf[(col, div.position)]
                            .set_symbol("─")
                            .set_style(handle_style);
                    }
                }
            }
        }
    })?;

    // Post-draw pass: re-emit OSC 8 hyperlink sequences directly to stdout so the
    // outer terminal emulator can make links ctrl+clickable.  ratatui's cell-based
    // diff renderer knows nothing about OSC 8, so we annotate the hyperlink cells
    // ourselves after every draw.
    let (rects, _) = layout.compute_geometry(w, pane_area_h);
    emit_hyperlink_annotations(cache, layout, &rects)?;

    Ok(())
}

/// After `tui.draw()` completes, re-emit OSC 8 hyperlink sequences directly to
/// stdout so the outer terminal emulator can make links ctrl+clickable.
///
/// ratatui's diff renderer writes terminal cells without any knowledge of OSC 8
/// hyperlinks.  This pass iterates over every pane that has hyperlink cells in
/// the current render cache and overwrites those cells — with the same character
/// and style that ratatui already drew — but wrapped in OSC 8 open/close
/// sequences.  The outer terminal emulator then associates those cells with the
/// URI and enables ctrl+click navigation.
///
/// Consecutive cells on the same row that share the same `(uri, id)` are
/// coalesced into a single OSC 8 run to minimise escape-sequence overhead.
///
/// After annotating, SGR is reset to avoid color bleeding and the hardware
/// cursor is restored to the position ratatui left it at.
fn emit_hyperlink_annotations(
    cache: &RenderCache,
    layout: &Layout,
    rects: &HashMap<PaneId, crate::core::layout::Rect>,
) -> io::Result<()> {
    if cache.pane_content.values().all(|c| c.hyperlinks.is_empty()) {
        return Ok(());
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();

    for (pane_id, cached) in &cache.pane_content {
        if cached.hyperlinks.is_empty() {
            continue;
        }
        let rect = match rects.get(pane_id) {
            Some(r) => r,
            None => continue,
        };
        let inner_x = rect.x + 1;
        let inner_y = rect.y + 1;

        let cells = &cached.hyperlinks;
        let mut i = 0;
        while i < cells.len() {
            let start = &cells[i];

            // Extend the run as long as cells are on the same row, have
            // consecutive columns, and share the same (uri, id) pair.
            let mut j = i + 1;
            while j < cells.len() {
                let prev = &cells[j - 1];
                let curr = &cells[j];
                if curr.uri == start.uri
                    && curr.id == start.id
                    && curr.row == prev.row
                    && curr.col == prev.col + 1
                {
                    j += 1;
                } else {
                    break;
                }
            }

            // 1-indexed absolute screen position.
            let abs_row = inner_y + start.row + 1;
            let abs_col = inner_x + start.col + 1;

            write!(out, "\x1b[{abs_row};{abs_col}H")?;
            write!(out, "\x1b]8;;{}\x1b\\", start.uri)?;

            let mut cur_style: Option<Style> = None;
            for cell in &cells[i..j] {
                if Some(cell.style) != cur_style {
                    write!(out, "{}", style_to_sgr(&cell.style))?;
                    cur_style = Some(cell.style);
                }
                write!(out, "{}", cell.ch)?;
            }

            write!(out, "\x1b]8;;\x1b\\")?;
            i = j;
        }
    }

    // Reset SGR so no color state leaks to subsequent terminal output.
    write!(out, "\x1b[0m")?;

    // Restore the cursor to where ratatui left it (active pane cursor).
    if let Some(rect) = rects.get(&layout.active) {
        let inner_x = rect.x + 1;
        let inner_y = rect.y + 1;
        if let Some(cached) = cache.pane_content.get(&layout.active) {
            if let Some((col, row)) = cached.cursor_pos {
                write!(out, "\x1b[{};{}H", inner_y + row + 1, inner_x + col + 1)?;
            }
        }
    }

    out.flush()?;
    Ok(())
}

/// Build an ANSI SGR escape sequence for a ratatui [`Style`].
///
/// Always starts with a full reset (`\x1b[0m`) so previously active
/// attributes do not bleed into the re-rendered cell.
fn style_to_sgr(style: &Style) -> String {
    let mut codes: Vec<String> = vec!["0".to_string()];
    if let Some(fg) = style.fg {
        if let Some(code) = ratatui_color_to_ansi_code(fg, true) {
            codes.push(code);
        }
    }
    if let Some(bg) = style.bg {
        if let Some(code) = ratatui_color_to_ansi_code(bg, false) {
            codes.push(code);
        }
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        codes.push("1".to_string());
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        codes.push("3".to_string());
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        codes.push("4".to_string());
    }
    format!("\x1b[{}m", codes.join(";"))
}

/// Map a ratatui [`Color`] to its ANSI SGR numeric code.
///
/// Returns `None` for [`Color::Reset`] (handled by the leading `0` in [`style_to_sgr`])
/// and any unrecognised variants.
fn ratatui_color_to_ansi_code(color: Color, is_fg: bool) -> Option<String> {
    let base: u8 = if is_fg { 30 } else { 40 };
    Some(match color {
        Color::Reset => return None,
        Color::Black => base.to_string(),
        Color::Red => (base + 1).to_string(),
        Color::Green => (base + 2).to_string(),
        Color::Yellow => (base + 3).to_string(),
        Color::Blue => (base + 4).to_string(),
        Color::Magenta => (base + 5).to_string(),
        Color::Cyan => (base + 6).to_string(),
        Color::White => (base + 7).to_string(),
        Color::DarkGray => {
            if is_fg {
                "90".to_string()
            } else {
                "100".to_string()
            }
        }
        Color::LightRed => {
            if is_fg {
                "91".to_string()
            } else {
                "101".to_string()
            }
        }
        Color::LightGreen => {
            if is_fg {
                "92".to_string()
            } else {
                "102".to_string()
            }
        }
        Color::LightYellow => {
            if is_fg {
                "93".to_string()
            } else {
                "103".to_string()
            }
        }
        Color::LightBlue => {
            if is_fg {
                "94".to_string()
            } else {
                "104".to_string()
            }
        }
        Color::LightMagenta => {
            if is_fg {
                "95".to_string()
            } else {
                "105".to_string()
            }
        }
        Color::LightCyan => {
            if is_fg {
                "96".to_string()
            } else {
                "106".to_string()
            }
        }
        Color::Indexed(i) => {
            if is_fg {
                format!("38;5;{i}")
            } else {
                format!("48;5;{i}")
            }
        }
        Color::Rgb(r, g, b) => {
            if is_fg {
                format!("38;2;{r};{g};{b}")
            } else {
                format!("48;2;{r};{g};{b}")
            }
        }
        _ => return None,
    })
}

/// Compute the cheatsheet bar height for a given terminal width.
/// Returns lines_of_bindings + 2 (for top/bottom border).
///
/// # Notes
///
/// The `+2` accounts for border rows around content rows.
pub fn cheatsheet_bar_height(w: u16, keymap: &KeyMap) -> u16 {
    let entries = cheatsheet_bindings(keymap);
    let inner_w = w.saturating_sub(2) as usize;
    let layout = compute_cheatsheet_grid_layout(&entries, inner_w);
    (layout.rows as u16) + 2 // +2 for border
}

fn cheatsheet_bar_height_cached(cache: &mut RenderCache, w: u16, keymap: &KeyMap) -> u16 {
    let inner_w = w.saturating_sub(2) as usize;
    let layout = cheatsheet_layout_cached(cache, keymap, inner_w);
    (layout.rows as u16) + 2
}

/// Draw a styled cheatsheet bar showing available keybindings.
/// Renders bindings in an aligned grid that adapts to terminal width.
fn render_cheatsheet(
    frame: &mut ratatui::Frame,
    cache: &mut RenderCache,
    keymap: &KeyMap,
    w: u16,
    h: u16,
    _bar_height: u16,
) {
    let key_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let sep_style = Style::default().fg(Color::DarkGray);
    let desc_style = Style::default().fg(Color::White);
    let title_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    // Available width inside the border (2 chars for left/right border).
    let inner_w = w.saturating_sub(2) as usize;
    let layout = cheatsheet_layout_cached(cache, keymap, inner_w);
    let entries = cheatsheet_entries_cached(cache, keymap).clone();

    // Build rows as an aligned grid.
    let mut lines: Vec<TuiLine> = Vec::new();
    for row in 0..layout.rows {
        let mut spans: Vec<Span> = Vec::new();

        for col in 0..layout.cols {
            if col > 0 {
                spans.push(Span::styled(" │ ", sep_style));
            }

            let idx = row * layout.cols + col;
            if let Some((key, desc)) = entries.get(idx) {
                let entry_width = cheatsheet_entry_width_chars((key.as_str(), *desc));
                let pad = layout.col_widths[col].saturating_sub(entry_width);

                spans.push(Span::styled(key.clone(), key_style));
                spans.push(Span::styled(" ", desc_style));
                spans.push(Span::styled(*desc, desc_style));
                if pad > 0 {
                    spans.push(Span::styled(" ".repeat(pad), desc_style));
                }
            }
        }

        lines.push(TuiLine::from(spans));
    }

    // Dynamic bar height: lines + 2 for border.
    let content_rows = lines.len() as u16;
    let bar_height = (content_rows + 2).min(h);

    let bar_rect = TuiRect {
        x: 0,
        y: h.saturating_sub(bar_height),
        width: w,
        height: bar_height,
    };

    if bar_rect.width < 2 || bar_rect.height < 2 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Keybindings ", title_style));
    let para = Paragraph::new(Text::from(lines)).block(block);
    frame.render_widget(para, bar_rect);
}

fn cheatsheet_entries_cached<'a>(
    cache: &'a mut RenderCache,
    keymap: &KeyMap,
) -> &'a Vec<(String, &'static str)> {
    cache
        .cheatsheet_entries
        .get_or_insert_with(|| cheatsheet_bindings(keymap))
}

fn cheatsheet_layout_cached(
    cache: &mut RenderCache,
    keymap: &KeyMap,
    inner_w: usize,
) -> CheatsheetGridLayout {
    if let Some(layout) = cache.cheatsheet_layouts.get(&inner_w) {
        return layout.clone();
    }

    let layout = {
        let entries = cheatsheet_entries_cached(cache, keymap);
        compute_cheatsheet_grid_layout(entries, inner_w)
    };
    cache.cheatsheet_layouts.insert(inner_w, layout.clone());
    layout
}

fn cheatsheet_bindings(keymap: &KeyMap) -> Vec<(String, &'static str)> {
    let mut entries: Vec<(String, &'static str)> = Vec::new();

    push_prefix_binding(&mut entries, keymap, Command::SplitLeft, "Split Left");
    push_prefix_binding(&mut entries, keymap, Command::SplitRight, "Split Right");
    push_prefix_binding(&mut entries, keymap, Command::SplitUp, "Split Up");
    push_prefix_binding(&mut entries, keymap, Command::SplitDown, "Split Down");
    push_prefix_binding(&mut entries, keymap, Command::FocusLeft, "Focus Left");
    push_prefix_binding(&mut entries, keymap, Command::FocusRight, "Focus Right");
    push_prefix_binding(&mut entries, keymap, Command::FocusUp, "Focus Up");
    push_prefix_binding(&mut entries, keymap, Command::FocusDown, "Focus Down");
    push_prefix_binding(&mut entries, keymap, Command::ClosePane, "Close Pane");
    push_prefix_binding(&mut entries, keymap, Command::Quit, "Quit");

    push_direct_binding(&mut entries, keymap, Command::ResizeLeft, "Resize Left");
    push_direct_binding(&mut entries, keymap, Command::ResizeRight, "Resize Right");
    push_direct_binding(&mut entries, keymap, Command::ResizeUp, "Resize Up");
    push_direct_binding(&mut entries, keymap, Command::ResizeDown, "Resize Down");

    entries.push(("Ctrl+Shift+C".to_string(), "Copy"));
    entries.push(("Ctrl+Shift+V".to_string(), "Paste"));
    entries.push(("Right-Click".to_string(), "Copy"));
    entries
}

#[derive(Debug, Clone)]
struct CheatsheetGridLayout {
    cols: usize,
    rows: usize,
    col_widths: Vec<usize>,
}

const CHEATSHEET_COL_SEPARATOR_WIDTH: usize = 3; // " │ "
const CHEATSHEET_MIN_COLUMN_WIDTH: usize = 18;
const CHEATSHEET_MAX_COLUMNS: usize = 3;

fn compute_cheatsheet_grid_layout(
    entries: &[(String, &'static str)],
    inner_w: usize,
) -> CheatsheetGridLayout {
    if entries.is_empty() {
        return CheatsheetGridLayout {
            cols: 1,
            rows: 1,
            col_widths: vec![0],
        };
    }

    let max_cols = entries
        .len()
        .min(CHEATSHEET_MAX_COLUMNS)
        .min((inner_w / CHEATSHEET_MIN_COLUMN_WIDTH).max(1));

    for cols in (1..=max_cols).rev() {
        let rows = entries.len().div_ceil(cols);
        let mut col_widths = vec![0usize; cols];

        for (idx, entry) in entries.iter().enumerate() {
            let col = idx % cols;
            col_widths[col] =
                col_widths[col].max(cheatsheet_entry_width_chars((entry.0.as_str(), entry.1)));
        }

        let total_width: usize = col_widths.iter().sum::<usize>()
            + CHEATSHEET_COL_SEPARATOR_WIDTH * cols.saturating_sub(1);
        if total_width <= inner_w {
            return CheatsheetGridLayout {
                cols,
                rows,
                col_widths,
            };
        }
    }

    let mut width = 0usize;
    for entry in entries {
        width = width.max(cheatsheet_entry_width_chars((entry.0.as_str(), entry.1)));
    }
    width = width.min(inner_w.max(1));
    CheatsheetGridLayout {
        cols: 1,
        rows: entries.len(),
        col_widths: vec![width],
    }
}

fn cheatsheet_entry_width_chars(entry: (&str, &str)) -> usize {
    entry.0.chars().count() + 1 + entry.1.chars().count()
}

fn push_prefix_binding(
    entries: &mut Vec<(String, &'static str)>,
    keymap: &KeyMap,
    command: Command,
    desc: &'static str,
) {
    if let Some(key) = display_key_for(keymap.prefix_chords_for_command(command)) {
        entries.push((key, desc));
    }
}

fn push_direct_binding(
    entries: &mut Vec<(String, &'static str)>,
    keymap: &KeyMap,
    command: Command,
    desc: &'static str,
) {
    if let Some(key) = display_key_for(keymap.direct_chords_for_command(command)) {
        entries.push((key, desc));
    }
}

fn display_key_for(chords: Vec<KeyChord>) -> Option<String> {
    let mut keys: Vec<String> = chords.iter().map(key_chord_to_display).collect();
    keys.sort();
    keys.into_iter().next()
}

fn key_chord_to_display(chord: &KeyChord) -> String {
    use crossterm::event::{KeyCode, KeyModifiers};

    let mut parts: Vec<String> = Vec::new();
    if chord.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if chord.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if chord.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift".to_string());
    }

    let key = match chord.code {
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("F{n}"),
        _ => format!("{:?}", chord.code),
    };
    parts.push(key);
    parts.join("+")
}

/// Return value of [`term_to_lines`]: rendered content and optional cursor position.
struct TermLines {
    /// Rendered ratatui lines, or `None` when the pane has no visible content yet.
    lines: Option<Vec<TuiLine<'static>>>,
    /// Cursor position as `(col, row)` relative to the pane's inner content area,
    /// or `None` when the cursor is hidden or scrolled off the visible viewport.
    cursor_pos: Option<(u16, u16)>,
    /// Cells that carry OSC 8 hyperlinks, collected after all style transforms.
    hyperlinks: Vec<HyperlinkCell>,
}

/// Convert the alacritty Term grid into ratatui Text for display.
/// If `sel_range` is Some, cells within the selection are rendered with inverted colors.
///
/// Reads cells directly from the grid by reference in a single pass — no
/// intermediate snapshot allocation. The cursor position is computed in the
/// same lock acquisition to avoid snapshot skew.
fn term_to_lines(
    pane: &PaneState,
    width: u16,
    height: u16,
    sel_range: Option<((u16, u16), (u16, u16))>,
) -> TermLines {
    let rows = height as usize;
    let cols = width as usize;

    let term = pane.term.lock();
    let content = term.renderable_content();
    let display_offset = content.display_offset as i32;

    // When DECTCEM hides the hardware cursor (e.g. ink/readline apps), compute
    // the cursor's viewport cell so we can paint a software block-cursor there.
    let soft_cursor: Option<(usize, usize)> = if content.cursor.shape == CursorShape::Hidden {
        point_to_viewport(content.display_offset, content.cursor.point)
            .filter(|vp| vp.line < rows && vp.column.0 < cols)
            .map(|vp| (vp.column.0, vp.line))
    } else {
        None
    };

    let mut lines: Vec<TuiLine<'static>> = Vec::with_capacity(rows);
    let mut has_visible_content = false;
    let mut hyperlinks: Vec<HyperlinkCell> = Vec::new();

    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        for col in 0..cols {
            let point = Point::new(Line(row as i32 - display_offset), Column(col));
            let cell = &term.grid()[point];
            let (ch, mut style) = cell_to_span(cell);
            if ch != ' ' {
                has_visible_content = true;
            }

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
                    let fg = style.bg.unwrap_or(Color::Black);
                    let bg = style.fg.unwrap_or(Color::White);
                    style = style.fg(fg).bg(bg);
                }
            }

            // Software cursor: invert colors at cursor cell when DECTCEM is off.
            if soft_cursor == Some((col, row)) {
                let fg = style.bg.unwrap_or(Color::Black);
                let bg = style.fg.unwrap_or(Color::White);
                style = style.fg(fg).bg(bg);
            }

            // Collect hyperlink metadata after all style transforms so that the
            // stored style matches what is actually rendered on screen.
            if let Some(hyperlink) = cell.hyperlink() {
                hyperlinks.push(HyperlinkCell {
                    row: row as u16,
                    col: col as u16,
                    ch,
                    style,
                    uri: hyperlink.uri().to_owned(),
                    id: hyperlink.id().to_owned(),
                });
            }

            if style == current_style {
                current_text.push(ch);
            } else {
                if !current_text.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut current_text),
                        current_style,
                    ));
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

    // Compute cursor position in the same snapshot to avoid skew.
    let cursor_pos = if content.cursor.shape != CursorShape::Hidden {
        point_to_viewport(content.display_offset, content.cursor.point)
            .filter(|vp| vp.line < height as usize && vp.column.0 < width as usize)
            .map(|vp| (vp.column.0 as u16, vp.line as u16))
    } else {
        None
    };

    drop(term);

    TermLines {
        lines: if has_visible_content {
            Some(lines)
        } else {
            None
        },
        cursor_pos,
        hyperlinks,
    }
}

/// Convert an alacritty cell to a (char, ratatui Style) pair.
fn cell_to_span(cell: &Cell) -> (char, Style) {
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
    if cell
        .flags
        .contains(alacritty_terminal::term::cell::Flags::BOLD)
    {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell
        .flags
        .contains(alacritty_terminal::term::cell::Flags::ITALIC)
    {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell
        .flags
        .contains(alacritty_terminal::term::cell::Flags::UNDERLINE)
    {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    (ch, style)
}

fn ansi_color_to_ratatui(color: alacritty_terminal::vte::ansi::Color) -> Option<Color> {
    use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor};
    match color {
        AColor::Named(NamedColor::Black) => Some(Color::Black),
        AColor::Named(NamedColor::Red) => Some(Color::Red),
        AColor::Named(NamedColor::Green) => Some(Color::Green),
        AColor::Named(NamedColor::Yellow) => Some(Color::Yellow),
        AColor::Named(NamedColor::Blue) => Some(Color::Blue),
        AColor::Named(NamedColor::Magenta) => Some(Color::Magenta),
        AColor::Named(NamedColor::Cyan) => Some(Color::Cyan),
        AColor::Named(NamedColor::White) => Some(Color::White),
        AColor::Named(NamedColor::BrightBlack) => Some(Color::DarkGray),
        AColor::Named(NamedColor::BrightRed) => Some(Color::LightRed),
        AColor::Named(NamedColor::BrightGreen) => Some(Color::LightGreen),
        AColor::Named(NamedColor::BrightYellow) => Some(Color::LightYellow),
        AColor::Named(NamedColor::BrightBlue) => Some(Color::LightBlue),
        AColor::Named(NamedColor::BrightMagenta) => Some(Color::LightMagenta),
        AColor::Named(NamedColor::BrightCyan) => Some(Color::LightCyan),
        AColor::Named(NamedColor::BrightWhite) => Some(Color::White),
        AColor::Spec(rgb) => Some(Color::Rgb(rgb.r, rgb.g, rgb.b)),
        AColor::Indexed(i) => Some(Color::Indexed(i)),
        _ => None,
    }
}

/// Translate a crossterm key event into bytes to send to the PTY.
///
/// # Behavior
///
/// Only [`KeyEventKind::Press`] events are translated; release and repeat
/// events return `None`.
///
/// # Examples
///
/// ```text
/// Enter -> \\r
/// Left  -> ESC [ D
/// Ctrl+C -> 0x03
/// ```
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
        KeyCode::Up => csi_with_modifier(b'A', mods),
        KeyCode::Down => csi_with_modifier(b'B', mods),
        KeyCode::Right => csi_with_modifier(b'C', mods),
        KeyCode::Left => csi_with_modifier(b'D', mods),
        KeyCode::Home => csi_with_modifier(b'H', mods),
        KeyCode::End => csi_with_modifier(b'F', mods),
        KeyCode::Insert => vec![0x1b, b'[', b'2', b'~'],
        KeyCode::Delete => tilde_with_modifier(3, mods),
        KeyCode::PageUp => tilde_with_modifier(5, mods),
        KeyCode::PageDown => tilde_with_modifier(6, mods),
        KeyCode::F(n) => {
            let bytes = f_key_bytes(n);
            if bytes.is_empty() {
                return None;
            }
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
    if mods.contains(KeyModifiers::SHIFT) {
        m += 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        m += 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        m += 4;
    }
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

/// Encode a mouse scroll event as bytes to write to the PTY.
///
/// # Behavior
///
/// When `sgr` is `true`, uses SGR mouse encoding (`\x1b[<btn;col;rowM`).
/// Otherwise falls back to X10 encoding when coordinates are within the
/// representable range (≤ 222), and returns empty bytes for out-of-range coords.
///
/// `col` and `row` are zero-indexed pane-content-local coordinates.
pub fn encode_mouse_scroll(col: u16, row: u16, up: bool, sgr: bool) -> Vec<u8> {
    let button: u16 = if up { 64 } else { 65 };
    if sgr {
        format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes()
    } else {
        // X10/normal encoding: each value is offset by 32; valid up to char 255.
        // Coordinates above 222 would overflow a single byte — skip them.
        if col > 222 || row > 222 {
            return Vec::new();
        }
        vec![
            b'\x1b',
            b'[',
            b'M',
            (button as u8) + 32,
            col as u8 + 33,
            row as u8 + 33,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::commands::Command;
    use crate::core::keymap::{KeyChord, KeyMap};
    use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Rgb};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::empty(),
        }
    }
    fn release(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Release,
            state: crossterm::event::KeyEventState::empty(),
        }
    }
    fn repeat(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Repeat,
            state: crossterm::event::KeyEventState::empty(),
        }
    }

    #[test]
    fn cheatsheet_includes_default_resize_hotkeys() {
        let km = KeyMap::default();
        let entries = cheatsheet_bindings(&km);
        assert!(entries
            .iter()
            .any(|(k, d)| k == "Alt+Shift+←" && *d == "Resize Left"));
        assert!(entries
            .iter()
            .any(|(k, d)| k == "Alt+Shift+→" && *d == "Resize Right"));
        assert!(entries
            .iter()
            .any(|(k, d)| k == "Alt+Shift+↑" && *d == "Resize Up"));
        assert!(entries
            .iter()
            .any(|(k, d)| k == "Alt+Shift+↓" && *d == "Resize Down"));
    }

    #[test]
    fn cheatsheet_uses_configured_hotkeys() {
        let mut km = KeyMap::new();
        km.bind(KeyChord::parse("x").unwrap(), Command::ClosePane);
        km.bind_direct(KeyChord::parse("alt+h").unwrap(), Command::ResizeLeft);
        let entries = cheatsheet_bindings(&km);
        assert!(entries.iter().any(|(k, d)| k == "x" && *d == "Close Pane"));
        assert!(entries
            .iter()
            .any(|(k, d)| k == "Alt+h" && *d == "Resize Left"));
    }

    #[test]
    fn cheatsheet_grid_uses_multiple_columns_when_wide() {
        let entries = vec![
            ("A".to_string(), "One"),
            ("B".to_string(), "Two"),
            ("C".to_string(), "Three"),
            ("D".to_string(), "Four"),
        ];
        let layout = compute_cheatsheet_grid_layout(&entries, 80);
        assert!(layout.cols > 1);
        assert_eq!(layout.rows, entries.len().div_ceil(layout.cols));
    }

    #[test]
    fn cheatsheet_grid_falls_back_to_single_column_when_narrow() {
        let entries = vec![
            ("Ctrl+Shift+Left".to_string(), "Resize Left"),
            ("Ctrl+Shift+Right".to_string(), "Resize Right"),
        ];
        let layout = compute_cheatsheet_grid_layout(&entries, 20);
        assert_eq!(layout.cols, 1);
        assert_eq!(layout.rows, entries.len());
    }

    #[test]
    fn cheatsheet_grid_for_empty_entries_returns_single_empty_row() {
        let entries: Vec<(String, &'static str)> = Vec::new();
        let layout = compute_cheatsheet_grid_layout(&entries, 10);
        assert_eq!(layout.cols, 1);
        assert_eq!(layout.rows, 1);
        assert_eq!(layout.col_widths, vec![0]);
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
            assert_eq!(
                key_event_to_bytes(&event),
                Some(vec![expected]),
                "ctrl+{ch}"
            );
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
            (KeyCode::Enter, vec![b'\r']),
            (KeyCode::Backspace, vec![0x7f]),
            (KeyCode::Tab, vec![b'\t']),
            (KeyCode::Esc, vec![0x1b]),
            (KeyCode::Up, vec![0x1b, b'[', b'A']),
            (KeyCode::Down, vec![0x1b, b'[', b'B']),
            (KeyCode::Right, vec![0x1b, b'[', b'C']),
            (KeyCode::Left, vec![0x1b, b'[', b'D']),
            (KeyCode::Home, vec![0x1b, b'[', b'H']),
            (KeyCode::End, vec![0x1b, b'[', b'F']),
            (KeyCode::Delete, vec![0x1b, b'[', b'3', b'~']),
            (KeyCode::PageUp, vec![0x1b, b'[', b'5', b'~']),
            (KeyCode::PageDown, vec![0x1b, b'[', b'6', b'~']),
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
            assert!(
                key_event_to_bytes(&event).is_none(),
                "F{n} should return None"
            );
        }
    }

    #[test]
    fn insert_key_returns_bytes() {
        let event = press(KeyCode::Insert, KeyModifiers::empty());
        assert_eq!(
            key_event_to_bytes(&event),
            Some(vec![0x1b, b'[', b'2', b'~'])
        );
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
        assert_eq!(
            key_event_to_bytes(&event),
            Some(vec![0x1b, b'[', b'1', b';', b'2', b'A'])
        );
    }

    #[test]
    fn ctrl_right_returns_modified_csi() {
        let event = press(KeyCode::Right, KeyModifiers::CONTROL);
        // \e[1;5C
        assert_eq!(
            key_event_to_bytes(&event),
            Some(vec![0x1b, b'[', b'1', b';', b'5', b'C'])
        );
    }

    #[test]
    fn plain_arrow_no_modifier() {
        let event = press(KeyCode::Left, KeyModifiers::empty());
        assert_eq!(key_event_to_bytes(&event), Some(vec![0x1b, b'[', b'D']));
    }

    #[test]
    fn modifier_param_combines_shift_alt_ctrl() {
        assert_eq!(modifier_param(KeyModifiers::empty()), 1);
        assert_eq!(modifier_param(KeyModifiers::SHIFT), 2);
        assert_eq!(modifier_param(KeyModifiers::ALT | KeyModifiers::CONTROL), 7);
        assert_eq!(
            modifier_param(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL),
            8
        );
    }

    #[test]
    fn csi_and_tilde_helpers_encode_modifiers() {
        assert_eq!(
            csi_with_modifier(b'A', KeyModifiers::empty()),
            vec![0x1b, b'[', b'A']
        );
        assert_eq!(
            csi_with_modifier(b'D', KeyModifiers::ALT | KeyModifiers::CONTROL),
            vec![0x1b, b'[', b'1', b';', b'7', b'D']
        );
        assert_eq!(
            tilde_with_modifier(3, KeyModifiers::empty()),
            vec![0x1b, b'[', b'3', b'~']
        );
        assert_eq!(
            tilde_with_modifier(6, KeyModifiers::SHIFT),
            vec![0x1b, b'[', b'6', b';', b'2', b'~']
        );
    }

    #[test]
    fn key_chord_display_formats_special_keys_and_picks_sorted_first() {
        let left = KeyChord::parse("ctrl+left").unwrap();
        let f5 = KeyChord::parse("f5").unwrap();
        let space = KeyChord::parse("space").unwrap();

        assert_eq!(key_chord_to_display(&left), "Ctrl+←");
        assert_eq!(key_chord_to_display(&f5), "F5");
        assert_eq!(key_chord_to_display(&space), "Space");

        let first = display_key_for(vec![
            KeyChord::parse("z").unwrap(),
            KeyChord::parse("a").unwrap(),
        ]);
        assert_eq!(first.as_deref(), Some("a"));
    }

    #[test]
    fn ansi_color_mapping_covers_named_spec_index_and_default_none() {
        assert_eq!(
            ansi_color_to_ratatui(AColor::Named(NamedColor::Blue)),
            Some(Color::Blue)
        );
        assert_eq!(
            ansi_color_to_ratatui(AColor::Spec(Rgb {
                r: 10,
                g: 20,
                b: 30
            })),
            Some(Color::Rgb(10, 20, 30))
        );
        assert_eq!(
            ansi_color_to_ratatui(AColor::Indexed(42)),
            Some(Color::Indexed(42))
        );
        assert_eq!(
            ansi_color_to_ratatui(AColor::Named(NamedColor::Foreground)),
            None
        );
    }

    #[test]
    fn encode_mouse_scroll_sgr_up() {
        // Button 64 = scroll up; col/row are 0-indexed, SGR is 1-indexed.
        let bytes = encode_mouse_scroll(3, 5, true, true);
        assert_eq!(bytes, b"\x1b[<64;4;6M");
    }

    #[test]
    fn encode_mouse_scroll_sgr_down() {
        let bytes = encode_mouse_scroll(0, 0, false, true);
        assert_eq!(bytes, b"\x1b[<65;1;1M");
    }

    #[test]
    fn encode_mouse_scroll_x10_up_in_range() {
        // X10: button+32, col+33, row+33
        let bytes = encode_mouse_scroll(0, 0, true, false);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 64 + 32, 33, 33]);
    }

    #[test]
    fn encode_mouse_scroll_x10_out_of_range_returns_empty() {
        let bytes = encode_mouse_scroll(223, 0, true, false);
        assert!(
            bytes.is_empty(),
            "coords > 222 must return empty in X10 mode"
        );
    }
}

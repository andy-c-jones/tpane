//! Main tpane application coordinator.
//!
//! [`App`] owns the runtime pane set and layout state, processes input events,
//! dispatches commands, and keeps backend pane geometry synchronized.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};

use crate::core::commands::Command;
use crate::core::keymap::KeyMap;
use crate::core::layout::{
    Direction, DividerInfo, Layout, Orientation, PaneId, SplitHandle, SplitPosition,
};
use crate::core::selection::Selection;
use crate::platform::renderer::{cheatsheet_bar_height, key_event_to_bytes};
use crate::traits::{AppEvent, Clipboard, EventSource, PaneBackend, PaneFactory, Renderer};

/// How much the ratio changes per resize step (approx 2% of total size).
const RESIZE_STEP: f64 = 0.02;

/// State for an ongoing divider drag (mouse-driven resize).
struct DividerDrag {
    first_pane: PaneId,
    second_pane: PaneId,
    split_handle: Option<SplitHandle>,
    orientation: Orientation,
    /// Start of the split rect along the split axis (x for Vertical, y for Horizontal).
    rect_start: u16,
    /// Total size of the split rect along the split axis.
    rect_size: u16,
    /// Last processed mouse coordinate along the split axis.
    last_axis_coord: Option<u16>,
}

/// Central application state, generic over the pane backend.
pub struct App<B: PaneBackend> {
    pub layout: Layout,
    pub panes: HashMap<PaneId, B>,
    keymap: KeyMap,
    terminal_size: (u16, u16),
    running: bool,
    /// True when the prefix key has been pressed and we're awaiting the command key.
    prefix_active: bool,
    /// Show keybinding cheatsheet when prefix is active.
    show_cheatsheet: bool,
    /// Current text selection (if any).
    pub selection: Option<Selection>,
    /// Screen coords where a left-button drag began, used to distinguish click from drag.
    drag_origin: Option<(u16, u16)>,
    /// Whether we're actively dragging (mouse moved since button down).
    dragging: bool,
    /// Active divider drag for mouse-driven pane resize.
    resize_drag: Option<DividerDrag>,
}

impl<B: PaneBackend> App<B> {
    /// Create a new app instance with one root pane spawned by `factory`.
    pub fn new<F: PaneFactory<B>>(
        keymap: KeyMap,
        terminal_size: (u16, u16),
        show_cheatsheet: bool,
        factory: &F,
    ) -> Result<Self> {
        let layout = Layout::new();
        let mut panes = HashMap::new();

        let (w, h) = terminal_size;
        let pane_w = w.saturating_sub(2).max(4);
        let pane_h = h.saturating_sub(2).max(4);
        let root_id = layout.active;
        let pane = factory.spawn(root_id, pane_w, pane_h)?;
        panes.insert(root_id, pane);

        Ok(App {
            layout,
            panes,
            keymap,
            terminal_size,
            running: true,
            prefix_active: false,
            show_cheatsheet,
            selection: None,
            drag_origin: None,
            dragging: false,
            resize_drag: None,
        })
    }

    /// Run the event loop until quit.
    pub fn run<F: PaneFactory<B>, R: Renderer<B>>(
        &mut self,
        events: &mut dyn EventSource,
        renderer: &mut R,
        factory: &F,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        let mut needs_render = true;
        while self.running {
            if needs_render {
                let show_bar = self.prefix_active && self.show_cheatsheet;
                renderer.render(
                    &self.layout,
                    &self.panes,
                    &self.keymap,
                    self.terminal_size,
                    show_bar,
                    self.selection.as_ref(),
                )?;
                needs_render = false;
            }

            match events.next_event(Duration::from_millis(16))? {
                Some(AppEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                    needs_render |= self.handle_key(key, factory, clipboard)?;
                }
                // Repeat events are only forwarded to direct bindings and raw PTY input;
                // prefix-key and global-shortcut handling is guarded inside handle_key.
                Some(AppEvent::Key(key)) if key.kind == KeyEventKind::Repeat => {
                    needs_render |= self.handle_key_repeat(key, factory)?;
                }
                Some(AppEvent::Mouse(mouse)) => {
                    self.handle_mouse(mouse, clipboard);
                    needs_render = true;
                }
                Some(AppEvent::Resize(w, h)) => {
                    self.handle_resize(w, h);
                    needs_render = true;
                }
                Some(AppEvent::PaneData { .. }) => {
                    needs_render = true;
                }
                Some(AppEvent::PaneExit { pane_id }) => {
                    if self.panes.contains_key(&pane_id) {
                        self.close_pane(pane_id);
                        needs_render = true;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_key<F: PaneFactory<B>>(
        &mut self,
        key: crossterm::event::KeyEvent,
        factory: &F,
        clipboard: &mut dyn Clipboard,
    ) -> Result<bool> {
        // Global shortcuts that work regardless of prefix mode.
        if self.handle_global_shortcuts(&key, clipboard)? {
            return Ok(true);
        }

        if self.prefix_active {
            self.prefix_active = false;
            if let Some(cmd) = self.keymap.lookup_prefix(&key).cloned() {
                self.dispatch(cmd, factory)?;
            }
            return Ok(true);
        }

        // Check for prefix key.
        if self.keymap.is_prefix(&key) {
            self.prefix_active = true;
            return Ok(true);
        }

        // Check direct bindings (holdable; work without prefix key).
        if let Some(cmd) = self.keymap.lookup_direct(&key).cloned() {
            self.dispatch(cmd, factory)?;
            return Ok(true);
        }

        // Forward raw bytes to the active pane.
        if let Some(bytes) = key_event_to_bytes(&key) {
            if let Some(pane) = self.panes.get_mut(&self.layout.active) {
                pane.write_input(&bytes)?;
            }
            return Ok(false);
        }
        Ok(false)
    }

    /// Handle key-repeat events: only direct bindings and raw PTY forwarding fire on repeat.
    /// Prefix-key logic and global shortcuts are intentionally excluded.
    fn handle_key_repeat<F: PaneFactory<B>>(
        &mut self,
        key: crossterm::event::KeyEvent,
        factory: &F,
    ) -> Result<bool> {
        // Direct bindings fire on repeat so they can be held to move edges continuously.
        if let Some(cmd) = self.keymap.lookup_direct(&key).cloned() {
            self.dispatch(cmd, factory)?;
            return Ok(true);
        }

        // Forward raw bytes to the active pane (e.g. holding a character key).
        if let Some(bytes) = key_event_to_bytes(&key) {
            if let Some(pane) = self.panes.get_mut(&self.layout.active) {
                pane.write_input(&bytes)?;
            }
            return Ok(false);
        }
        Ok(false)
    }

    /// Handle global shortcuts (Ctrl+Shift+C/V) that work in any mode.
    /// Returns true if the key was consumed.
    fn handle_global_shortcuts(
        &mut self,
        key: &crossterm::event::KeyEvent,
        clipboard: &mut dyn Clipboard,
    ) -> Result<bool> {
        let mods = key.modifiers;
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        if !mods.contains(ctrl_shift) {
            return Ok(false);
        }
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.copy_selection(clipboard);
                Ok(true)
            }
            KeyCode::Char('v') | KeyCode::Char('V') => {
                let target = self.layout.active;
                self.paste_clipboard_to(target, clipboard)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn copy_selection(&mut self, clipboard: &mut dyn Clipboard) {
        if let Some(ref sel) = self.selection {
            if sel.is_empty() {
                return;
            }
            let (start, end) = sel.ordered();
            if let Some(pane) = self.panes.get(&sel.pane_id) {
                let text = pane.selected_text(start, end, sel.display_offset);
                if !text.is_empty() {
                    let _ = clipboard.set_text(&text);
                }
            }
        }
        self.selection = None;
    }

    fn paste_clipboard_to(&mut self, pane_id: PaneId, clipboard: &mut dyn Clipboard) -> Result<()> {
        if let Ok(text) = clipboard.get_text() {
            if !text.is_empty() {
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    // Bracketed paste mode: wrap content so shells don't execute line-by-line.
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(b"\x1b[200~");
                    bytes.extend_from_slice(text.as_bytes());
                    bytes.extend_from_slice(b"\x1b[201~");
                    pane.write_input(&bytes)?;
                }
            }
        }
        Ok(())
    }

    fn dispatch<F: PaneFactory<B>>(&mut self, cmd: Command, factory: &F) -> Result<()> {
        match cmd {
            Command::SplitVertical => {
                self.split(Orientation::Vertical, SplitPosition::After, None, factory)?
            }
            Command::SplitHorizontal => {
                self.split(Orientation::Horizontal, SplitPosition::After, None, factory)?
            }
            Command::SplitLeft => {
                self.split(Orientation::Vertical, SplitPosition::Before, None, factory)?
            }
            Command::SplitRight => {
                self.split(Orientation::Vertical, SplitPosition::After, None, factory)?
            }
            Command::SplitUp => self.split(
                Orientation::Horizontal,
                SplitPosition::Before,
                None,
                factory,
            )?,
            Command::SplitDown => {
                self.split(Orientation::Horizontal, SplitPosition::After, None, factory)?
            }
            Command::ClosePane => {
                let id = self.layout.active;
                self.close_pane(id);
            }
            Command::FocusNext => self.layout.focus_next(),
            Command::FocusPrev => self.layout.focus_prev(),
            Command::FocusLeft => self
                .layout
                .focus_direction(Direction::Left, self.terminal_size),
            Command::FocusRight => self
                .layout
                .focus_direction(Direction::Right, self.terminal_size),
            Command::FocusUp => self
                .layout
                .focus_direction(Direction::Up, self.terminal_size),
            Command::FocusDown => self
                .layout
                .focus_direction(Direction::Down, self.terminal_size),
            Command::ResizeLeft => {
                self.layout.adjust_pane_ratio(
                    self.layout.active,
                    Orientation::Vertical,
                    -RESIZE_STEP,
                );
                self.refresh_pane_sizes();
            }
            Command::ResizeRight => {
                self.layout.adjust_pane_ratio(
                    self.layout.active,
                    Orientation::Vertical,
                    RESIZE_STEP,
                );
                self.refresh_pane_sizes();
            }
            Command::ResizeUp => {
                self.layout.adjust_pane_ratio(
                    self.layout.active,
                    Orientation::Horizontal,
                    -RESIZE_STEP,
                );
                self.refresh_pane_sizes();
            }
            Command::ResizeDown => {
                self.layout.adjust_pane_ratio(
                    self.layout.active,
                    Orientation::Horizontal,
                    RESIZE_STEP,
                );
                self.refresh_pane_sizes();
            }
            Command::Quit => self.running = false,
        }
        Ok(())
    }

    fn split<F: PaneFactory<B>>(
        &mut self,
        orientation: Orientation,
        position: SplitPosition,
        initial_ratio: Option<f64>,
        factory: &F,
    ) -> Result<()> {
        self.selection = None;
        let (w, h) = self.terminal_size;

        // `initial_ratio` is the fraction of space that the **active (original) pane** keeps
        // after the split. When position is After (e.g. split_right), the original becomes the
        // first child so internal_ratio == initial_ratio. When position is Before (e.g.
        // split_left), the original becomes the second child so we invert the fraction.
        //
        // The user-provided ratio is clamped to [0.05, 0.95] *before* inversion so that
        // the original pane always receives the requested proportion (e.g. 0.98 is clamped to
        // 0.95, meaning the original keeps 95% and the new pane gets 5%, regardless of
        // SplitPosition).
        let new_id = match initial_ratio {
            Some(ratio) => {
                let clamped = ratio.clamp(0.05, 0.95);
                let internal_ratio = match position {
                    SplitPosition::After => clamped,
                    SplitPosition::Before => 1.0 - clamped,
                };
                self.layout
                    .split_with_position_and_ratio(orientation, position, internal_ratio)
            }
            None => self.layout.split_with_position(orientation, position),
        };

        let rects = self.layout.compute_rects(w, h);
        for (id, pane) in self.panes.iter_mut() {
            if let Some(rect) = rects.get(id) {
                let inner_w = rect.width.saturating_sub(2).max(4);
                let inner_h = rect.height.saturating_sub(2).max(2);
                pane.resize(inner_w, inner_h);
            }
        }

        let rect = rects.get(&new_id).copied().unwrap_or_default();
        let inner_w = rect.width.saturating_sub(2).max(4);
        let inner_h = rect.height.saturating_sub(2).max(2);
        let pane = factory.spawn(new_id, inner_w, inner_h)?;
        self.panes.insert(new_id, pane);
        Ok(())
    }

    fn close_pane(&mut self, id: PaneId) {
        // Clear selection if it belongs to the closing pane.
        if let Some(ref sel) = self.selection {
            if sel.pane_id == id {
                self.selection = None;
            }
        }
        if !self.layout.close_pane(id) {
            self.running = false;
            return;
        }
        self.panes.remove(&id);
        self.refresh_pane_sizes();
    }

    /// Run startup layout commands (from `tpane.on_startup { ... }` in main.lua).
    /// Each command may carry an optional split ratio; non-split commands ignore the ratio.
    ///
    /// The `ratio` for split commands is the fraction of space that the **currently active pane
    /// keeps** after the split — e.g. `split_right(0.7)` leaves the original pane at 70% and
    /// the new right pane at 30%.  The value is clamped to [0.05, 0.95].  `None` uses the
    /// default 50/50 split.
    pub fn apply_startup_commands<F: PaneFactory<B>>(
        &mut self,
        cmds: &[(Command, Option<f64>)],
        factory: &F,
    ) -> Result<()> {
        for (cmd, ratio) in cmds {
            match cmd {
                Command::SplitVertical => {
                    self.split(Orientation::Vertical, SplitPosition::After, *ratio, factory)?
                }
                Command::SplitHorizontal => self.split(
                    Orientation::Horizontal,
                    SplitPosition::After,
                    *ratio,
                    factory,
                )?,
                Command::SplitLeft => self.split(
                    Orientation::Vertical,
                    SplitPosition::Before,
                    *ratio,
                    factory,
                )?,
                Command::SplitRight => {
                    self.split(Orientation::Vertical, SplitPosition::After, *ratio, factory)?
                }
                Command::SplitUp => self.split(
                    Orientation::Horizontal,
                    SplitPosition::Before,
                    *ratio,
                    factory,
                )?,
                Command::SplitDown => self.split(
                    Orientation::Horizontal,
                    SplitPosition::After,
                    *ratio,
                    factory,
                )?,
                other => self.dispatch(other.clone(), factory)?,
            }
        }
        Ok(())
    }

    /// Recompute pane geometry and notify all backends of their new size.
    fn refresh_pane_sizes(&mut self) {
        let (w, h) = self.terminal_size;
        let rects = self.layout.compute_rects(w, h);
        for (pid, pane) in self.panes.iter_mut() {
            if let Some(rect) = rects.get(pid) {
                let inner_w = rect.width.saturating_sub(2).max(4);
                let inner_h = rect.height.saturating_sub(2).max(2);
                pane.resize(inner_w, inner_h);
            }
        }
    }

    fn handle_resize(&mut self, w: u16, h: u16) {
        self.selection = None;
        self.terminal_size = (w, h);
        self.refresh_pane_sizes();
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent, clipboard: &mut dyn Clipboard) {
        let (w, h) = self.terminal_size;
        let cheatsheet_h = if self.prefix_active && self.show_cheatsheet {
            cheatsheet_bar_height(w, &self.keymap)
        } else {
            0
        };
        let pane_area_h = h.saturating_sub(cheatsheet_h);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Clear any previous selection.
                self.selection = None;
                self.dragging = false;
                self.resize_drag = None;

                // Check if the click lands on a divider first.
                let dividers = self.layout.compute_dividers(w, pane_area_h);
                if let Some(div) = Self::find_divider_at(&dividers, mouse.column, mouse.row) {
                    let axis_coord = match div.orientation {
                        Orientation::Vertical => mouse.column,
                        Orientation::Horizontal => mouse.row,
                    };
                    self.resize_drag = Some(DividerDrag {
                        first_pane: div.first_pane,
                        second_pane: div.second_pane,
                        split_handle: self.layout.split_handle(div.first_pane, div.second_pane),
                        orientation: div.orientation,
                        rect_start: div.rect_start,
                        rect_size: div.rect_size,
                        last_axis_coord: Some(axis_coord),
                    });
                    return;
                }

                // Find which pane was clicked.
                let rects = self.layout.compute_rects(w, pane_area_h);
                if let Some((pane_id, rect)) = Self::find_pane_at(&rects, mouse.column, mouse.row) {
                    self.layout.set_active(pane_id);

                    // Only start selection if click is inside the inner area (not border).
                    let inner_x = rect.x + 1;
                    let inner_y = rect.y + 1;
                    let inner_w = rect.width.saturating_sub(2);
                    let inner_h = rect.height.saturating_sub(2);

                    if mouse.column >= inner_x
                        && mouse.column < inner_x + inner_w
                        && mouse.row >= inner_y
                        && mouse.row < inner_y + inner_h
                    {
                        let col = mouse.column - inner_x;
                        let row = mouse.row - inner_y;
                        self.drag_origin = Some((mouse.column, mouse.row));
                        self.selection = Some(Selection {
                            pane_id,
                            start: (col, row),
                            end: (col, row),
                            display_offset: 0,
                        });
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Divider drag: update the split ratio.
                if let Some(ref mut drag) = self.resize_drag {
                    if drag.rect_size > 0 {
                        let axis_coord = match drag.orientation {
                            Orientation::Vertical => mouse.column,
                            Orientation::Horizontal => mouse.row,
                        };

                        if drag.last_axis_coord == Some(axis_coord) {
                            return;
                        }
                        drag.last_axis_coord = Some(axis_coord);

                        let new_ratio = (axis_coord.saturating_sub(drag.rect_start)) as f64
                            / drag.rect_size as f64;
                        if let Some(handle) = drag.split_handle.as_ref() {
                            self.layout.set_split_ratio_with_handle(handle, new_ratio);
                        } else {
                            let (fp, sp) = (drag.first_pane, drag.second_pane);
                            self.layout.set_split_ratio(fp, sp, new_ratio);
                        }
                        self.refresh_pane_sizes();
                    }
                    return;
                }

                self.dragging = true;
                if let Some(ref mut sel) = self.selection {
                    let rects = self.layout.compute_rects(w, pane_area_h);
                    if let Some(rect) = rects.get(&sel.pane_id) {
                        let inner_x = rect.x + 1;
                        let inner_y = rect.y + 1;
                        let inner_w = rect.width.saturating_sub(2);
                        let inner_h = rect.height.saturating_sub(2);

                        // Clamp to inner bounds.
                        let col = mouse.column.max(inner_x).min(inner_x + inner_w - 1) - inner_x;
                        let row = mouse.row.max(inner_y).min(inner_y + inner_h - 1) - inner_y;
                        sel.end = (col, row);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Finish divider drag.
                if self.resize_drag.is_some() {
                    self.resize_drag = None;
                    return;
                }
                // If no drag happened (just a click), clear the selection.
                if !self.dragging {
                    self.selection = None;
                }
                self.drag_origin = None;
                self.dragging = false;
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.selection.is_some() {
                    // Selection exists: copy it to clipboard.
                    self.copy_selection(clipboard);
                } else {
                    let rects = self.layout.compute_rects(w, pane_area_h);
                    // No selection: paste clipboard into the clicked pane.
                    let target = Self::find_pane_at(&rects, mouse.column, mouse.row)
                        .map(|(id, _)| id)
                        .unwrap_or(self.layout.active);
                    let _ = self.paste_clipboard_to(target, clipboard);
                }
            }
            _ => {}
        }
    }

    /// Find the divider at the given screen coordinates, if any.
    fn find_divider_at(dividers: &[DividerInfo], col: u16, row: u16) -> Option<DividerInfo> {
        for div in dividers {
            let hit = match div.orientation {
                Orientation::Vertical => {
                    col == div.position && row >= div.span_start && row < div.span_end
                }
                Orientation::Horizontal => {
                    row == div.position && col >= div.span_start && col < div.span_end
                }
            };
            if hit {
                return Some(*div);
            }
        }
        None
    }

    /// Find which pane contains the given screen coordinates.
    fn find_pane_at(
        rects: &HashMap<PaneId, crate::core::layout::Rect>,
        col: u16,
        row: u16,
    ) -> Option<(PaneId, crate::core::layout::Rect)> {
        for (pane_id, rect) in rects {
            if col >= rect.x
                && col < rect.x + rect.width
                && row >= rect.y
                && row < rect.y + rect.height
            {
                return Some((*pane_id, *rect));
            }
        }
        None
    }

    /// Return whether the event loop should keep running.
    ///
    /// This is primarily used by headless tests to assert quit/close behavior.
    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Return the number of currently open panes.
    #[allow(dead_code)]
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Return the [`PaneId`] of the active (focused) pane.
    #[allow(dead_code)]
    pub fn active_pane(&self) -> PaneId {
        self.layout.active
    }

    /// Return whether prefix mode is currently active.
    ///
    /// When this is `true`, the next non-global key is resolved via prefix
    /// bindings rather than being forwarded to the active pane.
    #[allow(dead_code)]
    pub fn is_prefix_active(&self) -> bool {
        self.prefix_active
    }

    /// Process one [`AppEvent`] without triggering a render.
    ///
    /// This helper mirrors the event-handling paths used by [`Self::run`], but
    /// skips renderer invocation to make unit tests deterministic and fast.
    #[allow(dead_code)]
    pub fn process_event<F: PaneFactory<B>>(
        &mut self,
        event: AppEvent,
        factory: &F,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        match event {
            AppEvent::Key(key) if key.kind == KeyEventKind::Press => {
                let _ = self.handle_key(key, factory, clipboard)?;
            }
            AppEvent::Key(key) if key.kind == KeyEventKind::Repeat => {
                let _ = self.handle_key_repeat(key, factory)?;
            }
            AppEvent::Mouse(mouse) => {
                self.handle_mouse(mouse, clipboard);
            }
            AppEvent::Resize(w, h) => {
                self.handle_resize(w, h);
            }
            AppEvent::PaneData { .. } => {}
            AppEvent::PaneExit { pane_id } => {
                if self.panes.contains_key(&pane_id) {
                    self.close_pane(pane_id);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

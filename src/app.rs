use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyEventKind, MouseEventKind, MouseButton, KeyCode, KeyModifiers};

use crate::core::commands::Command;
use crate::core::keymap::KeyMap;
use crate::core::layout::{Layout, Orientation, PaneId, SplitPosition};
use crate::core::selection::Selection;
use crate::platform::renderer::key_event_to_bytes;
use crate::traits::{AppEvent, Clipboard, EventSource, PaneBackend, PaneFactory, Renderer};

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
}

impl<B: PaneBackend> App<B> {
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
        while self.running {
            let show_bar = self.prefix_active && self.show_cheatsheet;
            renderer.render(&self.layout, &self.panes, self.terminal_size, show_bar, self.selection.as_ref())?;

            match events.next_event(Duration::from_millis(16))? {
                Some(AppEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                    self.handle_key(key, factory, clipboard)?;
                }
                Some(AppEvent::Mouse(mouse)) => {
                    self.handle_mouse(mouse, clipboard);
                }
                Some(AppEvent::Resize(w, h)) => {
                    self.handle_resize(w, h);
                }
                Some(AppEvent::PaneData { .. }) => {}
                Some(AppEvent::PaneExit { pane_id }) => {
                    if self.panes.contains_key(&pane_id) {
                        self.close_pane(pane_id);
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
    ) -> Result<()> {
        // Global shortcuts that work regardless of prefix mode.
        if self.handle_global_shortcuts(&key, clipboard)? {
            return Ok(());
        }

        if self.prefix_active {
            self.prefix_active = false;
            if let Some(cmd) = self.keymap.lookup_prefix(&key).cloned() {
                self.dispatch(cmd, factory)?;
            }
            return Ok(());
        }

        // Check for prefix key.
        if self.keymap.is_prefix(&key) {
            self.prefix_active = true;
            return Ok(());
        }

        // Forward raw bytes to the active pane.
        if let Some(bytes) = key_event_to_bytes(&key) {
            if let Some(pane) = self.panes.get_mut(&self.layout.active) {
                pane.write_input(&bytes)?;
            }
        }
        Ok(())
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
                self.paste_clipboard(clipboard)?;
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

    fn paste_clipboard(&mut self, clipboard: &mut dyn Clipboard) -> Result<()> {
        if let Ok(text) = clipboard.get_text() {
            if !text.is_empty() {
                if let Some(pane) = self.panes.get_mut(&self.layout.active) {
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
            Command::SplitVertical => self.split(Orientation::Vertical, SplitPosition::After, factory)?,
            Command::SplitHorizontal => self.split(Orientation::Horizontal, SplitPosition::After, factory)?,
            Command::SplitLeft => self.split(Orientation::Vertical, SplitPosition::Before, factory)?,
            Command::SplitRight => self.split(Orientation::Vertical, SplitPosition::After, factory)?,
            Command::SplitUp => self.split(Orientation::Horizontal, SplitPosition::Before, factory)?,
            Command::SplitDown => self.split(Orientation::Horizontal, SplitPosition::After, factory)?,
            Command::ClosePane => {
                let id = self.layout.active;
                self.close_pane(id);
            }
            Command::FocusNext => self.layout.focus_next(),
            Command::FocusPrev => self.layout.focus_prev(),
            Command::Quit => self.running = false,
        }
        Ok(())
    }

    fn split<F: PaneFactory<B>>(
        &mut self,
        orientation: Orientation,
        position: SplitPosition,
        factory: &F,
    ) -> Result<()> {
        self.selection = None;
        let (w, h) = self.terminal_size;
        let new_id = self.layout.split_with_position(orientation, position);

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
        let rects = self.layout.compute_rects(w, h);
        for (id, pane) in self.panes.iter_mut() {
            if let Some(rect) = rects.get(id) {
                let inner_w = rect.width.saturating_sub(2).max(4);
                let inner_h = rect.height.saturating_sub(2).max(2);
                pane.resize(inner_w, inner_h);
            }
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent, clipboard: &mut dyn Clipboard) {
        let (w, h) = self.terminal_size;
        let cheatsheet_h = if self.prefix_active && self.show_cheatsheet { 3 } else { 0 };
        let rects = self.layout.compute_rects(w, h.saturating_sub(cheatsheet_h));

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Clear any previous selection.
                self.selection = None;
                self.dragging = false;

                // Find which pane was clicked.
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
                self.dragging = true;
                if let Some(ref mut sel) = self.selection {
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
                // If no drag happened (just a click), clear the selection.
                if !self.dragging {
                    self.selection = None;
                }
                self.drag_origin = None;
                self.dragging = false;
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.copy_selection(clipboard);
            }
            _ => {}
        }
    }

    /// Find which pane contains the given screen coordinates.
    fn find_pane_at(
        rects: &HashMap<PaneId, crate::core::layout::Rect>,
        col: u16,
        row: u16,
    ) -> Option<(PaneId, crate::core::layout::Rect)> {
        for (pane_id, rect) in rects {
            if col >= rect.x && col < rect.x + rect.width
                && row >= rect.y && row < rect.y + rect.height
            {
                return Some((*pane_id, *rect));
            }
        }
        None
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    pub fn active_pane(&self) -> PaneId {
        self.layout.active
    }

    pub fn is_prefix_active(&self) -> bool {
        self.prefix_active
    }

    /// Process a single event without a render step (useful for tests).
    pub fn process_event<F: PaneFactory<B>>(
        &mut self,
        event: AppEvent,
        factory: &F,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        match event {
            AppEvent::Key(key) if key.kind == KeyEventKind::Press => {
                self.handle_key(key, factory, clipboard)?;
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

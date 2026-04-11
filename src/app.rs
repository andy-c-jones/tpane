use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::KeyEventKind;

use crate::core::commands::Command;
use crate::core::keymap::KeyMap;
use crate::core::layout::{Layout, Orientation, PaneId, SplitPosition};
use crate::platform::renderer::key_event_to_bytes;
use crate::traits::{AppEvent, EventSource, PaneBackend, PaneFactory, Renderer};

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
        })
    }

    /// Run the event loop until quit.
    pub fn run<F: PaneFactory<B>, R: Renderer<B>>(
        &mut self,
        events: &mut dyn EventSource,
        renderer: &mut R,
        factory: &F,
    ) -> Result<()> {
        while self.running {
            let show_bar = self.prefix_active && self.show_cheatsheet;
            renderer.render(&self.layout, &self.panes, self.terminal_size, show_bar)?;

            match events.next_event(Duration::from_millis(16))? {
                Some(AppEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                    self.handle_key(key, factory)?;
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
    ) -> Result<()> {
        if self.prefix_active {
            self.prefix_active = false;
            if let Some(cmd) = self.keymap.lookup_prefix(&key).cloned() {
                self.dispatch(cmd, factory)?;
            }
            // If no binding matched, the prefix sequence is consumed and discarded.
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
    pub fn process_event<F: PaneFactory<B>>(&mut self, event: AppEvent, factory: &F) -> Result<()> {
        match event {
            AppEvent::Key(key) if key.kind == KeyEventKind::Press => {
                self.handle_key(key, factory)?;
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

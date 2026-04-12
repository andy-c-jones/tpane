use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyEvent, MouseEvent};

use crate::core::layout::PaneId;
use crate::core::selection::Selection;

/// Unified event type for the App event loop.
#[derive(Debug, Clone)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    PaneData { pane_id: PaneId },
    PaneExit { pane_id: PaneId },
}

/// Provides events (keyboard, resize, pane I/O) to the App.
pub trait EventSource {
    /// Block up to `timeout`, returning the next event if available.
    fn next_event(&mut self, timeout: Duration) -> Result<Option<AppEvent>>;
}

/// Per-pane backend: manages shell I/O and terminal content.
pub trait PaneBackend: Send {
    /// Write raw bytes (keyboard input) to the pane's shell.
    fn write_input(&mut self, bytes: &[u8]) -> Result<()>;
    /// Notify the backend that the pane geometry changed.
    fn resize(&mut self, cols: u16, rows: u16);
    /// Extract text from a rectangular selection region.
    /// Coordinates are pane-grid-local (col, row).
    fn selected_text(&self, _start: (u16, u16), _end: (u16, u16), _display_offset: usize) -> String {
        String::new()
    }
}

/// Factory for creating pane backends.
pub trait PaneFactory<B: PaneBackend> {
    fn spawn(&self, id: PaneId, cols: u16, rows: u16) -> Result<B>;
}

/// Renders the tpane UI.
pub trait Renderer<B: PaneBackend> {
    fn render(
        &mut self,
        layout: &crate::core::layout::Layout,
        panes: &std::collections::HashMap<PaneId, B>,
        terminal_size: (u16, u16),
        prefix_active: bool,
        selection: Option<&Selection>,
    ) -> Result<()>;
}

/// Clipboard abstraction for testability.
pub trait Clipboard {
    fn get_text(&mut self) -> Result<String>;
    fn set_text(&mut self, text: &str) -> Result<()>;
}


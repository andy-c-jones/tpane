//! Core abstractions used to decouple app logic from platform details.
//!
//! # Overview
//!
//! [`crate::app::App`] depends on these traits rather than concrete terminal or
//! PTY implementations. This keeps core behavior testable with headless
//! adapters while production wiring lives in [`crate::platform`].

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyEvent, MouseEvent};

use crate::core::keymap::KeyMap;
use crate::core::layout::PaneId;
use crate::core::selection::Selection;

/// Unified event type for the App event loop.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Keyboard input from the UI event source.
    Key(KeyEvent),
    /// Mouse input from the UI event source.
    Mouse(MouseEvent),
    /// Terminal resize notification (`width`, `height`).
    Resize(u16, u16),
    /// A pane has produced output and should be considered dirty for render.
    PaneData {
        #[allow(dead_code)]
        pane_id: PaneId,
    },
    /// A pane's shell process has exited.
    PaneExit { pane_id: PaneId },
}

/// Provides events (keyboard, resize, pane I/O) to the app loop.
///
/// # Behavior
///
/// Implementations should return quickly when no events are available after the
/// provided timeout so [`crate::app::App::run`] can continue rendering and
/// housekeeping.
pub trait EventSource {
    /// Block up to `timeout`, returning the next event if available.
    fn next_event(&mut self, timeout: Duration) -> Result<Option<AppEvent>>;
}

/// Per-pane backend: manages shell I/O and terminal content.
///
/// # Notes
///
/// The app only assumes resize and byte-input behavior. Selection extraction is
/// optional and defaults to an empty string for backends that do not model
/// terminal buffers.
pub trait PaneBackend: Send {
    /// Write raw bytes (keyboard input) to the pane's shell.
    fn write_input(&mut self, bytes: &[u8]) -> Result<()>;
    /// Notify the backend that the pane geometry changed.
    fn resize(&mut self, cols: u16, rows: u16);
    /// Extract text from a rectangular selection region.
    /// Coordinates are pane-grid-local (col, row).
    fn selected_text(
        &self,
        _start: (u16, u16),
        _end: (u16, u16),
        _display_offset: usize,
    ) -> String {
        String::new()
    }
}

/// Factory for creating pane backends.
///
/// # Behavior
///
/// `spawn` should return a backend already associated with the given
/// [`PaneId`], with geometry matching `cols`/`rows`.
pub trait PaneFactory<B: PaneBackend> {
    fn spawn(&self, id: PaneId, cols: u16, rows: u16) -> Result<B>;
}

/// Renders the tpane UI.
///
/// # Behavior
///
/// The `prefix_active` argument communicates whether prefix-mode UX elements
/// (such as cheatsheets) should be visible.
pub trait Renderer<B: PaneBackend> {
    fn render(
        &mut self,
        layout: &crate::core::layout::Layout,
        panes: &std::collections::HashMap<PaneId, B>,
        keymap: &KeyMap,
        terminal_size: (u16, u16),
        prefix_active: bool,
        selection: Option<&Selection>,
    ) -> Result<()>;
}

/// Clipboard abstraction for testability.
///
/// # Errors
///
/// Implementations should return contextual errors when clipboard I/O fails,
/// rather than silently succeeding.
pub trait Clipboard {
    fn get_text(&mut self) -> Result<String>;
    fn set_text(&mut self, text: &str) -> Result<()>;
}

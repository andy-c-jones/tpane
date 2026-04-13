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

    // ── Scrollback ────────────────────────────────────────────────────────────

    /// Returns whether the pane's terminal is in alternate-screen mode.
    ///
    /// Full-screen TUI applications (e.g. bubbletea apps) use the alternate
    /// screen; normal shell sessions stay on the main (primary) screen.
    fn is_alt_screen(&self) -> bool {
        false
    }

    /// Returns whether the pane's terminal has any mouse-event reporting mode
    /// enabled (click, drag, or motion tracking).
    fn is_mouse_mode(&self) -> bool {
        false
    }

    /// Returns whether the pane's terminal uses SGR (`\x1b[<…M`) mouse
    /// encoding instead of the legacy X10 format.
    fn is_sgr_mouse(&self) -> bool {
        false
    }

    /// Returns whether the pane's terminal has alternate-scroll mode enabled.
    ///
    /// When this is active and the terminal is in alternate-screen mode, mouse
    /// wheel events should be translated to cursor-up/down key sequences rather
    /// than scrolling the scrollback buffer.
    fn is_alternate_scroll(&self) -> bool {
        false
    }

    /// Returns the current scrollback display offset (0 = at the bottom).
    fn display_offset(&self) -> usize {
        0
    }

    /// Scroll the terminal's viewport up by one page (towards history).
    fn scroll_page_up(&mut self) {}

    /// Scroll the terminal's viewport down by one page (towards present).
    fn scroll_page_down(&mut self) {}

    /// Scroll the terminal's viewport by `lines` lines.
    ///
    /// Positive values scroll up (towards history); negative values scroll
    /// down (towards the most recent output).
    fn scroll_by_lines(&mut self, _lines: i32) {}

    /// Snap the terminal's viewport to the most recent output.
    fn scroll_to_bottom(&mut self) {}
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

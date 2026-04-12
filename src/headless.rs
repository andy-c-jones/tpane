//! Headless implementations of traits for testing without a real terminal.
//!
//! # Usage
//!
//! These adapters are primarily consumed by [`crate::tests_headless`] to
//! exercise [`crate::app::App`] behavior without spawning PTYs or entering raw
//! terminal mode.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use anyhow::Result;

use crate::core::keymap::KeyMap;
use crate::core::layout::{Layout, PaneId};
use crate::core::selection::Selection;
use crate::traits::{AppEvent, Clipboard, EventSource, PaneBackend, PaneFactory, Renderer};

// ── HeadlessEventSource ──────────────────────────────────────────────────────

/// Event source backed by a simple queue. Push events in, App drains them.
pub struct HeadlessEventSource {
    queue: VecDeque<AppEvent>,
}

impl HeadlessEventSource {
    /// Create an empty in-memory event queue.
    ///
    /// # Examples
    ///
    /// ```text
    /// let mut events = HeadlessEventSource::new();
    /// events.push(AppEvent::Resize(120, 40));
    /// ```
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// Append a single event to the queue.
    pub fn push(&mut self, event: AppEvent) {
        self.queue.push_back(event);
    }

    /// Append multiple events to the queue in order.
    ///
    /// This is useful for setting up scripted event sequences in tests.
    pub fn push_all(&mut self, events: impl IntoIterator<Item = AppEvent>) {
        self.queue.extend(events);
    }
}

impl EventSource for HeadlessEventSource {
    fn next_event(&mut self, _timeout: Duration) -> Result<Option<AppEvent>> {
        Ok(self.queue.pop_front())
    }
}

// ── HeadlessPaneBackend ──────────────────────────────────────────────────────

/// Pane backend that records operations without real PTY/VT state.
pub struct HeadlessPaneBackend {
    /// Pane identifier assigned by the layout.
    pub id: PaneId,
    /// Last known pane width in columns.
    pub cols: u16,
    /// Last known pane height in rows.
    pub rows: u16,
    /// History of byte writes forwarded by the app.
    pub input_log: Vec<Vec<u8>>,
    /// History of resize calls performed by the app.
    pub resize_log: Vec<(u16, u16)>,
}

impl HeadlessPaneBackend {
    /// Construct a mock pane backend with initial geometry.
    ///
    /// # Behavior
    ///
    /// The initial size is recorded in fields, while `resize_log` starts empty
    /// until explicit resize operations occur.
    pub fn new(id: PaneId, cols: u16, rows: u16) -> Self {
        Self {
            id,
            cols,
            rows,
            input_log: Vec::new(),
            resize_log: Vec::new(),
        }
    }
}

impl PaneBackend for HeadlessPaneBackend {
    fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        self.input_log.push(bytes.to_vec());
        Ok(())
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.resize_log.push((cols, rows));
    }
}

// ── HeadlessPaneFactory ──────────────────────────────────────────────────────

/// Factory that creates HeadlessPaneBackend instances.
pub struct HeadlessPaneFactory;

impl PaneFactory<HeadlessPaneBackend> for HeadlessPaneFactory {
    fn spawn(&self, id: PaneId, cols: u16, rows: u16) -> Result<HeadlessPaneBackend> {
        Ok(HeadlessPaneBackend::new(id, cols, rows))
    }
}

// ── HeadlessRenderer ─────────────────────────────────────────────────────────

/// Renderer that counts frames without producing real output.
pub struct HeadlessRenderer {
    /// Number of times `render` was invoked.
    pub frame_count: usize,
    /// Last value of the `prefix_active` argument passed to `render`.
    pub last_cheatsheet_visible: bool,
}

impl HeadlessRenderer {
    /// Create a renderer that records render calls for assertions.
    pub fn new() -> Self {
        Self {
            frame_count: 0,
            last_cheatsheet_visible: false,
        }
    }
}

impl Renderer<HeadlessPaneBackend> for HeadlessRenderer {
    fn render(
        &mut self,
        _layout: &Layout,
        _panes: &HashMap<PaneId, HeadlessPaneBackend>,
        _keymap: &KeyMap,
        _terminal_size: (u16, u16),
        prefix_active: bool,
        _selection: Option<&Selection>,
    ) -> Result<()> {
        self.frame_count += 1;
        self.last_cheatsheet_visible = prefix_active;
        Ok(())
    }
}

// ── HeadlessClipboard ────────────────────────────────────────────────────────

/// In-memory clipboard for headless testing.
pub struct HeadlessClipboard {
    /// Stored clipboard contents.
    pub content: String,
}

impl HeadlessClipboard {
    /// Create an empty in-memory clipboard.
    pub fn new() -> Self {
        Self {
            content: String::new(),
        }
    }
}

impl Clipboard for HeadlessClipboard {
    fn get_text(&mut self) -> Result<String> {
        Ok(self.content.clone())
    }

    fn set_text(&mut self, text: &str) -> Result<()> {
        self.content = text.to_string();
        Ok(())
    }
}

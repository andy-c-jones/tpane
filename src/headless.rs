//! Headless implementations of traits for testing without a real terminal.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use anyhow::Result;

use crate::core::layout::{Layout, PaneId};
use crate::traits::{AppEvent, EventSource, PaneBackend, PaneFactory, Renderer};

// ── HeadlessEventSource ──────────────────────────────────────────────────────

/// Event source backed by a simple queue. Push events in, App drains them.
pub struct HeadlessEventSource {
    queue: VecDeque<AppEvent>,
}

impl HeadlessEventSource {
    pub fn new() -> Self {
        Self { queue: VecDeque::new() }
    }

    pub fn push(&mut self, event: AppEvent) {
        self.queue.push_back(event);
    }

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
    pub id: PaneId,
    pub cols: u16,
    pub rows: u16,
    pub input_log: Vec<Vec<u8>>,
    pub resize_log: Vec<(u16, u16)>,
}

impl HeadlessPaneBackend {
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
    pub frame_count: usize,
}

impl HeadlessRenderer {
    pub fn new() -> Self {
        Self { frame_count: 0 }
    }
}

impl Renderer<HeadlessPaneBackend> for HeadlessRenderer {
    fn render(
        &mut self,
        _layout: &Layout,
        _panes: &HashMap<PaneId, HeadlessPaneBackend>,
        _terminal_size: (u16, u16),
    ) -> Result<()> {
        self.frame_count += 1;
        Ok(())
    }
}

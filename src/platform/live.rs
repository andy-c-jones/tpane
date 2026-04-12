//! Live (real terminal) implementations of the trait abstractions.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};

use crate::core::layout::{Layout, PaneId};
use crate::platform::pane::{PaneEvent, PaneState};
use crate::platform::renderer::{self, Tui};
use crate::traits::{AppEvent, EventSource, PaneBackend, PaneFactory, Renderer};

// ── LiveEventSource ──────────────────────────────────────────────────────────

/// Merges crossterm terminal events with PTY pane events.
pub struct LiveEventSource {
    pane_rx: mpsc::Receiver<PaneEvent>,
}

impl LiveEventSource {
    pub fn new(pane_rx: mpsc::Receiver<PaneEvent>) -> Self {
        Self { pane_rx }
    }
}

impl EventSource for LiveEventSource {
    fn next_event(&mut self, timeout: Duration) -> Result<Option<AppEvent>> {
        // Drain pane events first (non-blocking).
        match self.pane_rx.try_recv() {
            Ok(PaneEvent::Data { pane_id, .. }) => {
                return Ok(Some(AppEvent::PaneData { pane_id }));
            }
            Ok(PaneEvent::Exit { pane_id }) => {
                return Ok(Some(AppEvent::PaneExit { pane_id }));
            }
            Err(_) => {}
        }

        // Poll crossterm for keyboard/mouse/resize.
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    return Ok(Some(AppEvent::Key(key)));
                }
                Event::Mouse(mouse) => {
                    return Ok(Some(AppEvent::Mouse(mouse)));
                }
                Event::Resize(w, h) => {
                    return Ok(Some(AppEvent::Resize(w, h)));
                }
                _ => {}
            }
        }

        Ok(None)
    }
}

// ── PaneBackend for PaneState ────────────────────────────────────────────────

impl PaneBackend for PaneState {
    fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        PaneState::write_input(self, bytes)
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        PaneState::resize(self, cols, rows);
    }

    fn selected_text(&self, start: (u16, u16), end: (u16, u16), display_offset: usize) -> String {
        PaneState::extract_text(self, start, end, display_offset)
    }
}

// ── LivePaneFactory ──────────────────────────────────────────────────────────

/// Creates real PaneState instances backed by PTY + alacritty_terminal.
pub struct LivePaneFactory {
    event_tx: mpsc::Sender<PaneEvent>,
    event_rx: Option<mpsc::Receiver<PaneEvent>>,
}

impl LivePaneFactory {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { event_tx: tx, event_rx: Some(rx) }
    }

    /// Take ownership of the event receiver (call once, before running the app).
    pub fn event_rx(&mut self) -> mpsc::Receiver<PaneEvent> {
        self.event_rx.take().expect("event_rx already taken")
    }
}

impl PaneFactory<PaneState> for LivePaneFactory {
    fn spawn(&self, id: PaneId, cols: u16, rows: u16) -> Result<PaneState> {
        PaneState::spawn(id, cols, rows, self.event_tx.clone())
    }
}

// ── LiveRenderer ─────────────────────────────────────────────────────────────

/// Wraps the real ratatui terminal for rendering.
pub struct LiveRenderer<'a> {
    tui: &'a mut Tui,
}

impl<'a> LiveRenderer<'a> {
    pub fn new(tui: &'a mut Tui) -> Self {
        Self { tui }
    }
}

impl<'a> Renderer<PaneState> for LiveRenderer<'a> {
    fn render(
        &mut self,
        layout: &Layout,
        panes: &HashMap<PaneId, PaneState>,
        terminal_size: (u16, u16),
        prefix_active: bool,
        selection: Option<&crate::core::selection::Selection>,
    ) -> Result<()> {
        renderer::render(self.tui, layout, panes, terminal_size, prefix_active, selection)
    }
}

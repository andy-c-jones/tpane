//! Live (real terminal) implementations of the trait abstractions.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind, MouseButton, MouseEventKind};

use crate::core::keymap::KeyMap;
use crate::core::layout::{Layout, PaneId};
use crate::platform::pane::{PaneEvent, PaneState};
use crate::platform::renderer::{self, RenderCache, Tui};
use crate::traits::{AppEvent, EventSource, PaneBackend, PaneFactory, Renderer};

const MAX_PANE_EVENTS_PER_TICK: usize = 128;
const MAX_CROSSTERM_EVENTS_PER_TICK: usize = 128;
const PANE_EVENT_CHANNEL_CAPACITY: usize = 1024;

// ── LiveEventSource ──────────────────────────────────────────────────────────

/// Merges crossterm terminal events with PTY pane events.
pub struct LiveEventSource {
    pane_rx: mpsc::Receiver<PaneEvent>,
    queued: VecDeque<AppEvent>,
    queued_pane_data: HashSet<PaneId>,
}

impl LiveEventSource {
    pub fn new(pane_rx: mpsc::Receiver<PaneEvent>) -> Self {
        Self {
            pane_rx,
            queued: VecDeque::new(),
            queued_pane_data: HashSet::new(),
        }
    }

    fn queue_event_coalesced(&mut self, event: AppEvent) {
        match event {
            AppEvent::PaneData { pane_id } => {
                if self.queued_pane_data.insert(pane_id) {
                    self.queued.push_back(AppEvent::PaneData { pane_id });
                }
            }
            AppEvent::Mouse(mouse) if mouse.kind == MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(AppEvent::Mouse(last)) = self.queued.back_mut() {
                    if last.kind == MouseEventKind::Drag(MouseButton::Left) {
                        *last = mouse;
                        return;
                    }
                }
                self.queued.push_back(AppEvent::Mouse(mouse));
            }
            AppEvent::Resize(w, h) => {
                if let Some(AppEvent::Resize(last_w, last_h)) = self.queued.back_mut() {
                    *last_w = w;
                    *last_h = h;
                    return;
                }
                self.queued.push_back(AppEvent::Resize(w, h));
            }
            other => self.queued.push_back(other),
        }
    }

    fn pop_queued_event(&mut self) -> Option<AppEvent> {
        let event = self.queued.pop_front()?;
        if let AppEvent::PaneData { pane_id } = event {
            self.queued_pane_data.remove(&pane_id);
            Some(AppEvent::PaneData { pane_id })
        } else {
            Some(event)
        }
    }

    fn map_crossterm_event(event: Event) -> Option<AppEvent> {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => Some(AppEvent::Key(key)),
            Event::Mouse(mouse) => Some(AppEvent::Mouse(mouse)),
            Event::Resize(w, h) => Some(AppEvent::Resize(w, h)),
            _ => None,
        }
    }
}

impl EventSource for LiveEventSource {
    fn next_event(&mut self, timeout: Duration) -> Result<Option<AppEvent>> {
        if let Some(event) = self.pop_queued_event() {
            return Ok(Some(event));
        }

        // Drain pane events first (non-blocking).
        for _ in 0..MAX_PANE_EVENTS_PER_TICK {
            let pane_event = match self.pane_rx.try_recv() {
                Ok(event) => event,
                Err(_) => break,
            };
            let app_event = match pane_event {
                PaneEvent::Data { pane_id } => AppEvent::PaneData { pane_id },
                PaneEvent::Exit { pane_id } => AppEvent::PaneExit { pane_id },
            };
            self.queue_event_coalesced(app_event);
        }
        if let Some(event) = self.pop_queued_event() {
            return Ok(Some(event));
        }

        // Poll crossterm for keyboard/mouse/resize.
        if event::poll(timeout)? {
            if let Some(event) = Self::map_crossterm_event(event::read()?) {
                self.queue_event_coalesced(event);
            }

            let mut drained = 0usize;
            while drained < MAX_CROSSTERM_EVENTS_PER_TICK && event::poll(Duration::from_millis(0))? {
                if let Some(event) = Self::map_crossterm_event(event::read()?) {
                    self.queue_event_coalesced(event);
                }
                drained += 1;
            }

            return Ok(self.pop_queued_event());
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
    event_tx: mpsc::SyncSender<PaneEvent>,
    event_rx: Option<mpsc::Receiver<PaneEvent>>,
}

impl LivePaneFactory {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel(PANE_EVENT_CHANNEL_CAPACITY);
        Self {
            event_tx: tx,
            event_rx: Some(rx),
        }
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
    cache: RenderCache,
}

impl<'a> LiveRenderer<'a> {
    pub fn new(tui: &'a mut Tui) -> Self {
        Self {
            tui,
            cache: RenderCache::default(),
        }
    }
}

impl<'a> Renderer<PaneState> for LiveRenderer<'a> {
    fn render(
        &mut self,
        layout: &Layout,
        panes: &HashMap<PaneId, PaneState>,
        keymap: &KeyMap,
        terminal_size: (u16, u16),
        prefix_active: bool,
        selection: Option<&crate::core::selection::Selection>,
    ) -> Result<()> {
        renderer::render(
            self.tui,
            &mut self.cache,
            layout,
            panes,
            keymap,
            terminal_size,
            prefix_active,
            selection,
        )
    }
}

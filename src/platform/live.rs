//! Live (real terminal) implementations of the trait abstractions.
//!
//! # Overview
//!
//! This module binds together:
//! - crossterm input
//! - pane PTY events
//! - ratatui rendering
//!
//! to satisfy the trait contracts in [`crate::traits`].

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
    /// Create a new live event source from a pane-event receiver.
    ///
    /// # Notes
    ///
    /// Events are queued internally and coalesced to reduce redundant render
    /// churn during drag/resize heavy input.
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
    /// Poll and return the next app event.
    ///
    /// # Behavior
    ///
    /// Pane events are drained first, then crossterm events are polled.
    /// Drag/resize/pane-data events may be coalesced before delivery.
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
            while drained < MAX_CROSSTERM_EVENTS_PER_TICK && event::poll(Duration::from_millis(0))?
            {
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

    fn is_alt_screen(&self) -> bool {
        PaneState::is_alt_screen(self)
    }

    fn is_mouse_mode(&self) -> bool {
        PaneState::is_mouse_mode(self)
    }

    fn is_sgr_mouse(&self) -> bool {
        PaneState::is_sgr_mouse(self)
    }

    fn is_alternate_scroll(&self) -> bool {
        PaneState::is_alternate_scroll(self)
    }

    fn display_offset(&self) -> usize {
        PaneState::display_offset(self)
    }

    fn scroll_page_up(&mut self) {
        PaneState::scroll_page_up(self)
    }

    fn scroll_page_down(&mut self) {
        PaneState::scroll_page_down(self)
    }

    fn scroll_by_lines(&mut self, lines: i32) {
        PaneState::scroll_by_lines(self, lines)
    }

    fn scroll_to_bottom(&mut self) {
        PaneState::scroll_to_bottom(self)
    }
}

// ── LivePaneFactory ──────────────────────────────────────────────────────────

/// Creates real PaneState instances backed by PTY + alacritty_terminal.
pub struct LivePaneFactory {
    event_tx: mpsc::SyncSender<PaneEvent>,
    event_rx: Option<mpsc::Receiver<PaneEvent>>,
    cwd: std::path::PathBuf,
}

impl LivePaneFactory {
    /// Create a pane factory and its internal bounded event channel.
    ///
    /// Captures the current working directory at construction time so that
    /// every spawned pane starts in the directory where tpane was launched.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel(PANE_EVENT_CHANNEL_CAPACITY);
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        Self {
            event_tx: tx,
            event_rx: Some(rx),
            cwd,
        }
    }

    /// Take ownership of the event receiver (call once, before running the app).
    ///
    /// # Panics
    ///
    /// Panics if called more than once.
    pub fn event_rx(&mut self) -> mpsc::Receiver<PaneEvent> {
        self.event_rx.take().expect("event_rx already taken")
    }
}

impl PaneFactory<PaneState> for LivePaneFactory {
    fn spawn(&self, id: PaneId, cols: u16, rows: u16) -> Result<PaneState> {
        PaneState::spawn(id, cols, rows, self.event_tx.clone(), self.cwd.clone())
    }
}

// ── LiveRenderer ─────────────────────────────────────────────────────────────

/// Wraps the real ratatui terminal for rendering.
pub struct LiveRenderer<'a> {
    tui: &'a mut Tui,
    cache: RenderCache,
}

impl<'a> LiveRenderer<'a> {
    /// Create a live renderer bound to a ratatui terminal instance.
    pub fn new(tui: &'a mut Tui) -> Self {
        Self {
            tui,
            cache: RenderCache::default(),
        }
    }
}

impl<'a> Renderer<PaneState> for LiveRenderer<'a> {
    /// Render one frame using [`crate::platform::renderer::render`].
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEvent,
        MouseEventKind,
    };

    fn key_event(kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::empty(),
            kind,
            state: KeyEventState::empty(),
        }
    }

    fn mouse_event(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn source() -> LiveEventSource {
        let (_tx, rx) = mpsc::channel();
        LiveEventSource::new(rx)
    }

    #[test]
    fn queue_event_coalesces_pane_data_and_pop_clears_dedupe_marker() {
        let mut src = source();
        let pane_id = PaneId(7);

        src.queue_event_coalesced(AppEvent::PaneData { pane_id });
        src.queue_event_coalesced(AppEvent::PaneData { pane_id });
        assert_eq!(src.queued.len(), 1);

        match src.pop_queued_event() {
            Some(AppEvent::PaneData { pane_id: popped }) => assert_eq!(popped, pane_id),
            _ => panic!("expected pane data event"),
        }
        assert!(src.queued_pane_data.is_empty());

        src.queue_event_coalesced(AppEvent::PaneData { pane_id });
        assert_eq!(src.queued.len(), 1);
    }

    #[test]
    fn queue_event_coalesces_mouse_drag_and_resize_tail_events() {
        let mut src = source();

        src.queue_event_coalesced(AppEvent::Mouse(mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            3,
            4,
        )));
        src.queue_event_coalesced(AppEvent::Mouse(mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            9,
            4,
        )));
        assert_eq!(src.queued.len(), 1);
        match src.queued.front() {
            Some(AppEvent::Mouse(mouse)) => assert_eq!(mouse.column, 9),
            _ => panic!("expected coalesced mouse drag"),
        }

        src.queue_event_coalesced(AppEvent::Resize(80, 24));
        src.queue_event_coalesced(AppEvent::Resize(120, 40));
        assert_eq!(src.queued.len(), 2);
        match src.queued.back() {
            Some(AppEvent::Resize(w, h)) => assert_eq!((*w, *h), (120, 40)),
            _ => panic!("expected coalesced resize"),
        }
    }

    #[test]
    fn map_crossterm_event_accepts_press_mouse_resize_and_ignores_release() {
        let mapped_press =
            LiveEventSource::map_crossterm_event(Event::Key(key_event(KeyEventKind::Press)));
        assert!(matches!(mapped_press, Some(AppEvent::Key(_))));

        let mapped_release =
            LiveEventSource::map_crossterm_event(Event::Key(key_event(KeyEventKind::Release)));
        assert!(mapped_release.is_none());

        let mapped_mouse = LiveEventSource::map_crossterm_event(Event::Mouse(mouse_event(
            MouseEventKind::Moved,
            1,
            1,
        )));
        assert!(matches!(mapped_mouse, Some(AppEvent::Mouse(_))));

        let mapped_resize = LiveEventSource::map_crossterm_event(Event::Resize(90, 30));
        match mapped_resize {
            Some(AppEvent::Resize(w, h)) => assert_eq!((w, h), (90, 30)),
            _ => panic!("expected resize event"),
        }
    }
}

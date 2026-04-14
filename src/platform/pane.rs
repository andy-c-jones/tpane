//! PTY-backed pane runtime and terminal event handling.
//!
//! A [`PaneState`] owns alacritty terminal state plus PTY handles, and bridges
//! shell I/O/events into the app's event loop.
//!
//! # Notes
//!
//! PTY spawn is asynchronous so panes can render immediately while the shell
//! process initializes in the background.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{mpsc, Arc};
use std::thread;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use crate::core::layout::PaneId;

// ── App-level event types sent from pane reader threads ──────────────────────

/// Pane lifecycle/data events produced by PTY reader threads.
///
/// These events are consumed by [`crate::platform::live::LiveEventSource`] and
/// transformed into [`crate::traits::AppEvent`] values for the app loop.
#[derive(Debug)]
pub enum PaneEvent {
    /// Terminal output arrived for this pane.
    Data { pane_id: PaneId },
    /// The pane's shell process exited.
    Exit { pane_id: PaneId },
}

// ── EventListener implementation for alacritty Term ──────────────────────────

/// Forwards terminal events to the main event loop via an mpsc sender.
///
/// This listener is attached to the alacritty terminal state and forwards
/// relevant events (title changes, PTY replies, color/size requests).
#[derive(Clone)]
pub struct TpaneEventListener {
    #[allow(dead_code)]
    sender: mpsc::SyncSender<PaneEvent>,
    #[allow(dead_code)]
    pane_id: PaneId,
    title: Arc<Mutex<String>>,
    title_version: Arc<AtomicU64>,
    /// Used to write terminal responses (e.g. DA1 replies) back to the PTY.
    reply_tx: SyncSender<String>,
    /// Current pane size packed as `(cols as u32) << 16 | rows as u32`.
    /// Updated by PaneState::resize so TextAreaSizeRequest replies are accurate.
    packed_size: Arc<AtomicU32>,
}

impl TpaneEventListener {
    fn send_reply(&self, s: String) {
        if let Err(e) = self.reply_tx.try_send(s) {
            log::warn!(
                "pane {:?}: failed to enqueue PTY reply: {}",
                self.pane_id,
                e
            );
        }
    }
}

impl EventListener for TpaneEventListener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Title(t) => {
                *self.title.lock() = t;
                self.title_version.fetch_add(1, Ordering::Relaxed);
            }
            TermEvent::ResetTitle => {
                self.title.lock().clear();
                self.title_version.fetch_add(1, Ordering::Relaxed);
            }
            // The terminal emulator needs to send a response back to the shell
            // (e.g. the DA1 Primary Device Attribute reply that fish waits for).
            TermEvent::PtyWrite(s) => {
                self.send_reply(s);
            }
            // Color queries: return terminal-like defaults so programs can adapt
            // to a sensible palette instead of assuming pure black.
            TermEvent::ColorRequest(index, formatter) => {
                self.send_reply(formatter(default_color_for_query(index)));
            }
            // Text-area size queries: respond with the actual pane dimensions.
            TermEvent::TextAreaSizeRequest(formatter) => {
                let packed = self.packed_size.load(Ordering::Relaxed);
                let size = alacritty_terminal::event::WindowSize {
                    num_lines: (packed & 0xffff) as u16,
                    num_cols: (packed >> 16) as u16,
                    cell_width: 0,
                    cell_height: 0,
                };
                self.send_reply(formatter(size));
            }
            _ => {}
        }
    }
}

// ── Per-pane state ────────────────────────────────────────────────────────────

/// Holds the PTY handles once the background spawn completes.
struct PtyHandles {
    writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
}

/// All runtime state for a single pane.
pub struct PaneState {
    /// Pane identifier matching layout membership.
    #[allow(dead_code)]
    pub id: PaneId,
    /// The VT emulator — created immediately so rendering always works.
    pub term: Arc<FairMutex<Term<TpaneEventListener>>>,
    /// PTY handles — None until background spawn completes.
    pty: Arc<Mutex<Option<PtyHandles>>>,
    /// Buffered input written before the PTY is ready.
    input_buffer: Arc<Mutex<Vec<u8>>>,
    /// Width currently allocated to this pane.
    pub cols: u16,
    /// Height currently allocated to this pane.
    pub rows: u16,
    /// Pending resize to apply once PTY is ready.
    pending_resize: Arc<Mutex<Option<(u16, u16)>>>,
    /// True once the PTY background spawn has completed and shell is connected.
    #[allow(dead_code)]
    ready: Arc<std::sync::atomic::AtomicBool>,
    /// Terminal title set via OSC escape sequences (e.g. by the shell or running program).
    title: Arc<Mutex<String>>,
    /// Shared pane size for the event listener (kept in sync with cols/rows).
    packed_size: Arc<AtomicU32>,
    /// Monotonic counter incremented when terminal content might have changed.
    content_version: Arc<AtomicU64>,
    /// Monotonic counter incremented when pane title changes.
    title_version: Arc<AtomicU64>,
}

impl PaneState {
    /// Spawn a new pane. The Term is created immediately so the pane renders right
    /// away; the actual PTY + shell launch happens on a background thread.
    ///
    /// # Errors
    ///
    /// Returns setup errors that occur before the background spawn thread
    /// starts.
    pub fn spawn(
        id: PaneId,
        cols: u16,
        rows: u16,
        event_tx: mpsc::SyncSender<PaneEvent>,
        cwd: std::path::PathBuf,
    ) -> Result<Self> {
        // Bounded channel for responses the terminal emulator needs to send back
        // to the shell (e.g. DA1 Primary Device Attribute replies).
        let (reply_tx, reply_rx) = mpsc::sync_channel::<String>(64);

        // Shared size for accurate TextAreaSizeRequest replies; updated on resize.
        let packed_size = Arc::new(AtomicU32::new(pack_size(cols, rows)));
        let content_version = Arc::new(AtomicU64::new(0));

        // Create the alacritty Term immediately (cheap, no I/O).
        let term_config = TermConfig {
            kitty_keyboard: true,
            ..TermConfig::default()
        };
        let title: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let title_version = Arc::new(AtomicU64::new(0));
        let listener = TpaneEventListener {
            sender: event_tx.clone(),
            pane_id: id,
            title: title.clone(),
            title_version: title_version.clone(),
            reply_tx,
            packed_size: packed_size.clone(),
        };
        let term_size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        let term = Arc::new(FairMutex::new(Term::new(term_config, &term_size, listener)));

        let pty: Arc<Mutex<Option<PtyHandles>>> = Arc::new(Mutex::new(None));
        let input_buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let pending_resize: Arc<Mutex<Option<(u16, u16)>>> = Arc::new(Mutex::new(None));
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Spawn PTY on a background thread so we don't block the render loop.
        {
            let term_arc = term.clone();
            let pty_ref = pty.clone();
            let buf_ref = input_buffer.clone();
            let resize_ref = pending_resize.clone();
            let ready_ref = ready.clone();
            let content_version_ref = content_version.clone();
            thread::spawn(move || {
                if let Err(e) = spawn_pty(
                    id,
                    cols,
                    rows,
                    event_tx,
                    term_arc,
                    pty_ref,
                    buf_ref,
                    resize_ref,
                    ready_ref,
                    content_version_ref,
                    reply_rx,
                    cwd,
                ) {
                    log::error!("Failed to spawn PTY for pane {:?}: {}", id, e);
                }
            });
        }

        Ok(PaneState {
            id,
            term,
            pty,
            input_buffer,
            cols,
            rows,
            pending_resize,
            ready,
            title,
            packed_size,
            content_version,
            title_version,
        })
    }

    /// Write bytes (keyboard input) to the PTY, or buffer if not ready yet.
    ///
    /// # Errors
    ///
    /// Returns an error only when writing to an already-ready PTY fails.
    pub fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        let mut pty_guard = self.pty.lock();
        if let Some(ref mut handles) = *pty_guard {
            handles.writer.write_all(bytes).context("writing to PTY")
        } else {
            self.input_buffer.lock().extend_from_slice(bytes);
            Ok(())
        }
    }

    /// Resize the PTY and the Term when the pane geometry changes.
    ///
    /// # Behavior
    ///
    /// If the PTY is not yet ready, the latest size is recorded and applied
    /// once startup completes.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if self.cols == cols && self.rows == rows {
            return;
        }

        self.cols = cols;
        self.rows = rows;
        self.packed_size
            .store(pack_size(cols, rows), Ordering::Relaxed);
        let term_size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        self.term.lock().resize(term_size);
        self.content_version.fetch_add(1, Ordering::Relaxed);

        let mut pty_guard = self.pty.lock();
        if let Some(ref mut handles) = *pty_guard {
            let size = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            let _ = handles.master.resize(size);
        } else {
            *self.pending_resize.lock() = Some((cols, rows));
        }
    }

    /// Whether the PTY has finished spawning and the shell is connected.
    #[allow(dead_code)]
    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The terminal title set by the running program via OSC escape sequences.
    /// Returns an empty string if no title has been set.
    pub fn title(&self) -> String {
        self.title.lock().clone()
    }

    /// Monotonic counter of terminal content mutations.
    pub fn content_version(&self) -> u64 {
        self.content_version.load(Ordering::Relaxed)
    }

    /// Monotonic counter of title mutations.
    pub fn title_version(&self) -> u64 {
        self.title_version.load(Ordering::Relaxed)
    }

    // ── Scrollback ────────────────────────────────────────────────────────────

    /// Returns whether the terminal is currently in alternate-screen mode.
    pub fn is_alt_screen(&self) -> bool {
        self.term.lock().mode().contains(TermMode::ALT_SCREEN)
    }

    /// Returns whether any mouse-event reporting mode is currently enabled.
    pub fn is_mouse_mode(&self) -> bool {
        self.term.lock().mode().contains(TermMode::MOUSE_MODE)
    }

    /// Returns whether SGR mouse encoding (`\x1b[<…M`) is enabled.
    pub fn is_sgr_mouse(&self) -> bool {
        self.term.lock().mode().contains(TermMode::SGR_MOUSE)
    }

    /// Returns whether alternate-scroll mode is enabled and in alt screen.
    ///
    /// When true, mouse wheel events should be translated to cursor-key sequences
    /// rather than scrolling the scrollback buffer.
    pub fn is_alternate_scroll(&self) -> bool {
        let mode = *self.term.lock().mode();
        mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL)
    }

    /// Current scrollback display offset (0 = at the bottom / most recent output).
    pub fn display_offset(&self) -> usize {
        self.term.lock().renderable_content().display_offset
    }

    /// Scroll the viewport up by one page towards history.
    pub fn scroll_page_up(&mut self) {
        self.term.lock().scroll_display(Scroll::PageUp);
        self.content_version.fetch_add(1, Ordering::Relaxed);
    }

    /// Scroll the viewport down by one page towards the most recent output.
    pub fn scroll_page_down(&mut self) {
        self.term.lock().scroll_display(Scroll::PageDown);
        self.content_version.fetch_add(1, Ordering::Relaxed);
    }

    /// Scroll the viewport by `lines` lines (positive = up, negative = down).
    pub fn scroll_by_lines(&mut self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.content_version.fetch_add(1, Ordering::Relaxed);
    }

    /// Snap the viewport to the most recent output (display_offset → 0).
    pub fn scroll_to_bottom(&mut self) {
        self.term.lock().scroll_display(Scroll::Bottom);
        self.content_version.fetch_add(1, Ordering::Relaxed);
    }

    /// Extract text from the terminal grid between two pane-grid-local positions.
    /// Handles line wrapping: wrapped lines don't get a newline inserted.
    ///
    /// # Notes
    ///
    /// `display_offset` is the scrollback offset captured at selection start so
    /// that copy-from-scrollback uses the correct grid rows even if the viewport
    /// has since moved.
    pub fn extract_text(
        &self,
        start: (u16, u16),
        end: (u16, u16),
        display_offset: usize,
    ) -> String {
        let term = self.term.lock();
        let offset = display_offset as i32;

        let (sc, sr) = start;
        let (ec, er) = end;

        let mut result = String::new();
        for row in sr..=er {
            let col_start = if row == sr { sc } else { 0 };
            let col_end = if row == er {
                ec
            } else {
                self.cols.saturating_sub(1)
            };

            let mut line = String::new();
            for col in col_start..=col_end {
                let point = alacritty_terminal::index::Point::new(
                    alacritty_terminal::index::Line(row as i32 - offset),
                    alacritty_terminal::index::Column(col as usize),
                );
                let c = term.grid()[point].c;
                // Skip wide-char spacer cells (null char with WIDE_CHAR_SPACER flag).
                if c == '\0' {
                    continue;
                }
                line.push(c);
            }

            // Trim trailing spaces from each line.
            let trimmed = line.trim_end();
            result.push_str(trimmed);

            // Add newline between lines, but not after the last line.
            // Skip newline for wrapped lines (the terminal treats them as one logical line).
            if row < er {
                let line_point = alacritty_terminal::index::Point::new(
                    alacritty_terminal::index::Line(row as i32 - offset),
                    alacritty_terminal::index::Column(0),
                );
                let flags = term.grid()[line_point].flags;
                let is_wrapped = flags.contains(alacritty_terminal::term::cell::Flags::WRAPLINE);
                if !is_wrapped {
                    result.push('\n');
                }
            }
        }
        result
    }
}
fn spawn_pty(
    id: PaneId,
    cols: u16,
    rows: u16,
    event_tx: mpsc::SyncSender<PaneEvent>,
    term: Arc<FairMutex<Term<TpaneEventListener>>>,
    pty_slot: Arc<Mutex<Option<PtyHandles>>>,
    input_buffer: Arc<Mutex<Vec<u8>>>,
    pending_resize: Arc<Mutex<Option<(u16, u16)>>>,
    ready: Arc<std::sync::atomic::AtomicBool>,
    content_version: Arc<AtomicU64>,
    reply_rx: mpsc::Receiver<String>,
    cwd: std::path::PathBuf,
) -> Result<()> {
    let pty_system = NativePtySystem::default();
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = pty_system.openpty(size).context("opening PTY")?;

    let shell = shell_command();
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");
    cmd.cwd(&cwd);

    let _child = pair.slave.spawn_command(cmd).context("spawning shell")?;

    let pty_writer = pair.master.take_writer().context("getting PTY writer")?;
    let mut pty_reader = pair
        .master
        .try_clone_reader()
        .context("cloning PTY reader")?;

    drop(pair.slave);

    // Apply any resize that happened while we were spawning.
    if let Some((c, r)) = pending_resize.lock().take() {
        let sz = PtySize {
            rows: r,
            cols: c,
            pixel_width: 0,
            pixel_height: 0,
        };
        let _ = pair.master.resize(sz);
    }

    // Flush any buffered input that was typed before the PTY was ready.
    {
        let mut handles = PtyHandles {
            writer: pty_writer,
            master: pair.master,
        };
        let buffered: Vec<u8> = std::mem::take(&mut *input_buffer.lock());
        if !buffered.is_empty() {
            let _ = handles.writer.write_all(&buffered);
        }
        *pty_slot.lock() = Some(handles);
    }

    // Notify the UI that the PTY is connected (but shell may still be loading).
    send_pane_data(&event_tx, id);

    // Reader loop — reads PTY output and feeds the term.
    // Mark ready on first output so the throbber shows until the shell renders.
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut buf = [0u8; 65536];
    loop {
        match pty_reader.read(&mut buf) {
            Ok(0) | Err(_) => {
                let _ = event_tx.send(PaneEvent::Exit { pane_id: id });
                break;
            }
            Ok(n) => {
                {
                    let mut t = term.lock();
                    processor.advance(&mut *t, &buf[..n]);
                }
                content_version.fetch_add(1, Ordering::Relaxed);
                // Forward any terminal replies (e.g. DA1 response) back to the shell.
                let mut replies = String::new();
                while let Ok(reply) = reply_rx.try_recv() {
                    replies.push_str(&reply);
                }
                if !replies.is_empty() {
                    let mut guard = pty_slot.lock();
                    if let Some(ref mut h) = *guard {
                        let _ = h.writer.write_all(replies.as_bytes());
                        let _ = h.writer.flush();
                    }
                }
                // Mark ready on first real output from the shell.
                if !ready.load(std::sync::atomic::Ordering::Relaxed) {
                    ready.store(true, std::sync::atomic::Ordering::Release);
                }
                send_pane_data(&event_tx, id);
            }
        }
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Minimal Dimensions impl for Term::new / Term::resize.
struct TermSize {
    cols: usize,
    rows: usize,
}

impl alacritty_terminal::grid::Dimensions for TermSize {
    fn columns(&self) -> usize {
        self.cols
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn total_lines(&self) -> usize {
        self.rows
    }
}

/// Return the shell to launch: $SHELL on Unix, or cmd.exe on Windows.
fn shell_command() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
}

/// Pack (cols, rows) into a single u32 for lock-free sharing with the event listener.
fn pack_size(cols: u16, rows: u16) -> u32 {
    (cols as u32) << 16 | (rows as u32)
}

fn default_color_for_query(index: usize) -> alacritty_terminal::vte::ansi::Rgb {
    use alacritty_terminal::vte::ansi::Rgb;

    // xterm-like defaults for ANSI base + dynamic foreground/background/cursor.
    const ANSI_BASE: [Rgb; 16] = [
        Rgb {
            r: 0x00,
            g: 0x00,
            b: 0x00,
        }, // black
        Rgb {
            r: 0xcd,
            g: 0x00,
            b: 0x00,
        }, // red
        Rgb {
            r: 0x00,
            g: 0xcd,
            b: 0x00,
        }, // green
        Rgb {
            r: 0xcd,
            g: 0xcd,
            b: 0x00,
        }, // yellow
        Rgb {
            r: 0x00,
            g: 0x00,
            b: 0xee,
        }, // blue
        Rgb {
            r: 0xcd,
            g: 0x00,
            b: 0xcd,
        }, // magenta
        Rgb {
            r: 0x00,
            g: 0xcd,
            b: 0xcd,
        }, // cyan
        Rgb {
            r: 0xe5,
            g: 0xe5,
            b: 0xe5,
        }, // white
        Rgb {
            r: 0x7f,
            g: 0x7f,
            b: 0x7f,
        }, // bright black
        Rgb {
            r: 0xff,
            g: 0x00,
            b: 0x00,
        }, // bright red
        Rgb {
            r: 0x00,
            g: 0xff,
            b: 0x00,
        }, // bright green
        Rgb {
            r: 0xff,
            g: 0xff,
            b: 0x00,
        }, // bright yellow
        Rgb {
            r: 0x5c,
            g: 0x5c,
            b: 0xff,
        }, // bright blue
        Rgb {
            r: 0xff,
            g: 0x00,
            b: 0xff,
        }, // bright magenta
        Rgb {
            r: 0x00,
            g: 0xff,
            b: 0xff,
        }, // bright cyan
        Rgb {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        }, // bright white
    ];

    match index {
        0..=15 => ANSI_BASE[index],
        256 => Rgb {
            r: 0xd0,
            g: 0xd0,
            b: 0xd0,
        }, // foreground
        257 => Rgb {
            r: 0x1e,
            g: 0x1e,
            b: 0x1e,
        }, // background
        258 => Rgb {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        }, // cursor
        267 => Rgb {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        }, // bright foreground
        268 => Rgb {
            r: 0x1e,
            g: 0x1e,
            b: 0x1e,
        }, // dim background
        _ => Rgb {
            r: 0xd0,
            g: 0xd0,
            b: 0xd0,
        },
    }
}

fn send_pane_data(event_tx: &mpsc::SyncSender<PaneEvent>, pane_id: PaneId) {
    match event_tx.try_send(PaneEvent::Data { pane_id }) {
        Ok(_) => {}
        Err(mpsc::TrySendError::Full(_)) => {}
        Err(mpsc::TrySendError::Disconnected(_)) => {}
    }
}

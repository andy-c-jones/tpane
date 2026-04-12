use std::io::{Read, Write};
use std::sync::{Arc, mpsc};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::sync::mpsc::SyncSender;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use crate::core::layout::PaneId;

// ── App-level event types sent from pane reader threads ──────────────────────

#[derive(Debug)]
pub enum PaneEvent {
    Data { pane_id: PaneId },
    Exit {
        pane_id: PaneId,
    },
}

// ── EventListener implementation for alacritty Term ──────────────────────────

/// Forwards terminal events to the main event loop via an mpsc sender.
#[derive(Clone)]
pub struct TpaneEventListener {
    #[allow(dead_code)]
    sender: mpsc::Sender<PaneEvent>,
    #[allow(dead_code)]
    pane_id: PaneId,
    title: Arc<Mutex<String>>,
    /// Used to write terminal responses (e.g. DA1 replies) back to the PTY.
    reply_tx: SyncSender<String>,
    /// Current pane size packed as `(cols as u32) << 16 | rows as u32`.
    /// Updated by PaneState::resize so TextAreaSizeRequest replies are accurate.
    packed_size: Arc<AtomicU32>,
}

impl TpaneEventListener {
    fn send_reply(&self, s: String) {
        if let Err(e) = self.reply_tx.try_send(s) {
            log::warn!("pane {:?}: failed to enqueue PTY reply: {}", self.pane_id, e);
        }
    }
}

impl EventListener for TpaneEventListener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Title(t) => {
                *self.title.lock() = t;
            }
            TermEvent::ResetTitle => {
                self.title.lock().clear();
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
    #[allow(dead_code)]
    pub id: PaneId,
    /// The VT emulator — created immediately so rendering always works.
    pub term: Arc<FairMutex<Term<TpaneEventListener>>>,
    /// PTY handles — None until background spawn completes.
    pty: Arc<Mutex<Option<PtyHandles>>>,
    /// Buffered input written before the PTY is ready.
    input_buffer: Arc<Mutex<Vec<u8>>>,
    /// Width/height currently allocated to this pane.
    pub cols: u16,
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
}

impl PaneState {
    /// Spawn a new pane. The Term is created immediately so the pane renders right
    /// away; the actual PTY + shell launch happens on a background thread.
    pub fn spawn(
        id: PaneId,
        cols: u16,
        rows: u16,
        event_tx: mpsc::Sender<PaneEvent>,
    ) -> Result<Self> {
        // Bounded channel for responses the terminal emulator needs to send back
        // to the shell (e.g. DA1 Primary Device Attribute replies).
        let (reply_tx, reply_rx) = mpsc::sync_channel::<String>(64);

        // Shared size for accurate TextAreaSizeRequest replies; updated on resize.
        let packed_size = Arc::new(AtomicU32::new(pack_size(cols, rows)));

        // Create the alacritty Term immediately (cheap, no I/O).
        let term_config = TermConfig {
            kitty_keyboard: true,
            ..TermConfig::default()
        };
        let title: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let listener = TpaneEventListener {
            sender: event_tx.clone(),
            pane_id: id,
            title: title.clone(),
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
                    reply_rx,
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
        })
    }

    /// Write bytes (keyboard input) to the PTY, or buffer if not ready yet.
    pub fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        let mut pty_guard = self.pty.lock();
        if let Some(ref mut handles) = *pty_guard {
            handles.writer.write_all(bytes).context("writing to PTY")?;
            handles.writer.flush().context("flushing PTY writer")
        } else {
            self.input_buffer.lock().extend_from_slice(bytes);
            Ok(())
        }
    }

    /// Resize the PTY and the Term when the pane geometry changes.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if self.cols == cols && self.rows == rows {
            return;
        }

        self.cols = cols;
        self.rows = rows;
        self.packed_size.store(pack_size(cols, rows), Ordering::Relaxed);
        let term_size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        self.term.lock().resize(term_size);

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

    /// Extract text from the terminal grid between two pane-grid-local positions.
    /// Handles line wrapping: wrapped lines don't get a newline inserted.
    pub fn extract_text(
        &self,
        start: (u16, u16),
        end: (u16, u16),
        _display_offset: usize,
    ) -> String {
        let term = self.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;

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
    event_tx: mpsc::Sender<PaneEvent>,
    term: Arc<FairMutex<Term<TpaneEventListener>>>,
    pty_slot: Arc<Mutex<Option<PtyHandles>>>,
    input_buffer: Arc<Mutex<Vec<u8>>>,
    pending_resize: Arc<Mutex<Option<(u16, u16)>>>,
    ready: Arc<std::sync::atomic::AtomicBool>,
    reply_rx: mpsc::Receiver<String>,
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
            let _ = handles.writer.flush();
        }
        *pty_slot.lock() = Some(handles);
    }

    // Notify the UI that the PTY is connected (but shell may still be loading).
    let _ = event_tx.send(PaneEvent::Data {
        pane_id: id,
    });

    // Reader loop — reads PTY output and feeds the term.
    // Mark ready on first output so the throbber shows until the shell renders.
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut buf = [0u8; 4096];
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
                // Forward any terminal replies (e.g. DA1 response) back to the shell.
                while let Ok(reply) = reply_rx.try_recv() {
                    let mut guard = pty_slot.lock();
                    if let Some(ref mut h) = *guard {
                        let _ = h.writer.write_all(reply.as_bytes());
                        let _ = h.writer.flush();
                    }
                }
                // Mark ready on first real output from the shell.
                if !ready.load(std::sync::atomic::Ordering::Relaxed) {
                    ready.store(true, std::sync::atomic::Ordering::Release);
                }
                let _ = event_tx.send(PaneEvent::Data { pane_id: id });
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
        Rgb { r: 0x00, g: 0x00, b: 0x00 }, // black
        Rgb { r: 0xcd, g: 0x00, b: 0x00 }, // red
        Rgb { r: 0x00, g: 0xcd, b: 0x00 }, // green
        Rgb { r: 0xcd, g: 0xcd, b: 0x00 }, // yellow
        Rgb { r: 0x00, g: 0x00, b: 0xee }, // blue
        Rgb { r: 0xcd, g: 0x00, b: 0xcd }, // magenta
        Rgb { r: 0x00, g: 0xcd, b: 0xcd }, // cyan
        Rgb { r: 0xe5, g: 0xe5, b: 0xe5 }, // white
        Rgb { r: 0x7f, g: 0x7f, b: 0x7f }, // bright black
        Rgb { r: 0xff, g: 0x00, b: 0x00 }, // bright red
        Rgb { r: 0x00, g: 0xff, b: 0x00 }, // bright green
        Rgb { r: 0xff, g: 0xff, b: 0x00 }, // bright yellow
        Rgb { r: 0x5c, g: 0x5c, b: 0xff }, // bright blue
        Rgb { r: 0xff, g: 0x00, b: 0xff }, // bright magenta
        Rgb { r: 0x00, g: 0xff, b: 0xff }, // bright cyan
        Rgb { r: 0xff, g: 0xff, b: 0xff }, // bright white
    ];

    match index {
        0..=15 => ANSI_BASE[index],
        256 => Rgb { r: 0xd0, g: 0xd0, b: 0xd0 }, // foreground
        257 => Rgb { r: 0x1e, g: 0x1e, b: 0x1e }, // background
        258 => Rgb { r: 0xff, g: 0xff, b: 0xff }, // cursor
        267 => Rgb { r: 0xff, g: 0xff, b: 0xff }, // bright foreground
        268 => Rgb { r: 0x1e, g: 0x1e, b: 0x1e }, // dim background
        _ => Rgb { r: 0xd0, g: 0xd0, b: 0xd0 },
    }
}

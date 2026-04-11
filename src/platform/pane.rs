use std::io::{Read, Write};
use std::sync::{Arc, mpsc};
use std::thread;

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
    Data { pane_id: PaneId, bytes: Vec<u8> },
    Exit { pane_id: PaneId },
}

// ── EventListener implementation for alacritty Term ──────────────────────────

/// Forwards terminal events to the main event loop via an mpsc sender.
#[derive(Clone)]
pub struct TpaneEventListener {
    sender: mpsc::Sender<PaneEvent>,
    pane_id: PaneId,
}

impl EventListener for TpaneEventListener {
    fn send_event(&self, _event: TermEvent) {
        // We don't need to act on terminal-internal events (bell, clipboard, etc.)
        // for basic multiplexer functionality. Extend here later if needed.
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
        // Create the alacritty Term immediately (cheap, no I/O).
        let term_config = TermConfig { kitty_keyboard: true, ..TermConfig::default() };
        let listener = TpaneEventListener { sender: event_tx.clone(), pane_id: id };
        let term_size = TermSize { cols: cols as usize, rows: rows as usize };
        let term = Arc::new(FairMutex::new(Term::new(term_config, &term_size, listener)));

        let pty: Arc<Mutex<Option<PtyHandles>>> = Arc::new(Mutex::new(None));
        let input_buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let pending_resize: Arc<Mutex<Option<(u16, u16)>>> = Arc::new(Mutex::new(None));

        // Spawn PTY on a background thread so we don't block the render loop.
        {
            let term_arc = term.clone();
            let pty_ref = pty.clone();
            let buf_ref = input_buffer.clone();
            let resize_ref = pending_resize.clone();
            thread::spawn(move || {
                if let Err(e) = spawn_pty(id, cols, rows, event_tx, term_arc, pty_ref, buf_ref, resize_ref) {
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
        self.cols = cols;
        self.rows = rows;
        let term_size = TermSize { cols: cols as usize, rows: rows as usize };
        self.term.lock().resize(term_size);

        let mut pty_guard = self.pty.lock();
        if let Some(ref mut handles) = *pty_guard {
            let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
            let _ = handles.master.resize(size);
        } else {
            *self.pending_resize.lock() = Some((cols, rows));
        }
    }
}

/// Background PTY spawn — does the heavy I/O work off the main thread.
fn spawn_pty(
    id: PaneId,
    cols: u16,
    rows: u16,
    event_tx: mpsc::Sender<PaneEvent>,
    term: Arc<FairMutex<Term<TpaneEventListener>>>,
    pty_slot: Arc<Mutex<Option<PtyHandles>>>,
    input_buffer: Arc<Mutex<Vec<u8>>>,
    pending_resize: Arc<Mutex<Option<(u16, u16)>>>,
) -> Result<()> {
    let pty_system = NativePtySystem::default();
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system.openpty(size).context("opening PTY")?;

    let shell = shell_command();
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");

    let _child = pair.slave.spawn_command(cmd).context("spawning shell")?;

    let pty_writer = pair.master.take_writer().context("getting PTY writer")?;
    let mut pty_reader = pair.master.try_clone_reader().context("cloning PTY reader")?;

    drop(pair.slave);

    // Apply any resize that happened while we were spawning.
    if let Some((c, r)) = pending_resize.lock().take() {
        let sz = PtySize { rows: r, cols: c, pixel_width: 0, pixel_height: 0 };
        let _ = pair.master.resize(sz);
    }

    // Flush any buffered input that was typed before the PTY was ready.
    {
        let mut handles = PtyHandles { writer: pty_writer, master: pair.master };
        let buffered: Vec<u8> = std::mem::take(&mut *input_buffer.lock());
        if !buffered.is_empty() {
            let _ = handles.writer.write_all(&buffered);
            let _ = handles.writer.flush();
        }
        *pty_slot.lock() = Some(handles);
    }

    // Notify the UI that content is available.
    let _ = event_tx.send(PaneEvent::Data { pane_id: id, bytes: Vec::new() });

    // Reader loop — reads PTY output and feeds the term.
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
                let _ = event_tx.send(PaneEvent::Data {
                    pane_id: id,
                    bytes: buf[..n].to_vec(),
                });
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

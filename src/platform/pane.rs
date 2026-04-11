use std::io::Write;
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

/// All runtime state for a single pane.
pub struct PaneState {
    pub id: PaneId,
    /// The VT emulator — guarded by alacritty's fair mutex for multi-reader safety.
    pub term: Arc<FairMutex<Term<TpaneEventListener>>>,
    /// Send raw bytes to the PTY (shell input).
    pty_writer: Box<dyn Write + Send>,
    /// Width/height currently allocated to this pane.
    pub cols: u16,
    pub rows: u16,
    /// Handle to resize the PTY when geometry changes.
    pty_master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
}

impl PaneState {
    /// Spawn a new pane: creates PTY, launches shell, starts reader thread.
    pub fn spawn(
        id: PaneId,
        cols: u16,
        rows: u16,
        event_tx: mpsc::Sender<PaneEvent>,
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
        let pair = pty_system.openpty(size).context("opening PTY")?;

        // Determine shell command.
        let shell = shell_command();
        let mut cmd = CommandBuilder::new(&shell);
        cmd.env("TERM", "xterm-256color");

        let _child = pair.slave.spawn_command(cmd).context("spawning shell")?;

        // Writer for sending input to the PTY.
        let pty_writer = pair.master.take_writer().context("getting PTY writer")?;

        // Reader for PTY output.
        let mut pty_reader = pair.master.try_clone_reader().context("cloning PTY reader")?;

        // Create the alacritty Term.
        let term_config = TermConfig { kitty_keyboard: true, ..TermConfig::default() };
        let listener = TpaneEventListener { sender: event_tx.clone(), pane_id: id };
        let term_size = TermSize { cols: cols as usize, rows: rows as usize };
        let term = Arc::new(FairMutex::new(Term::new(term_config, &term_size, listener)));

        // Spawn reader thread: reads PTY output bytes and sends to main event loop.
        {
            let term_arc = term.clone();
            let tx = event_tx.clone();
            thread::spawn(move || {
                let mut processor = Processor::<StdSyncHandler>::new();
                let mut buf = [0u8; 4096];
                loop {
                    match pty_reader.read(&mut buf) {
                        Ok(0) | Err(_) => {
                            let _ = tx.send(PaneEvent::Exit { pane_id: id });
                            break;
                        }
                        Ok(n) => {
                            {
                                let mut term = term_arc.lock();
                                processor.advance(&mut *term, &buf[..n]);
                            }
                            let _ = tx.send(PaneEvent::Data {
                                pane_id: id,
                                bytes: buf[..n].to_vec(),
                            });
                        }
                    }
                }
            });
        }

        Ok(PaneState {
            id,
            term,
            pty_writer,
            cols,
            rows,
            pty_master: Arc::new(Mutex::new(pair.master)),
        })
    }

    /// Write bytes (keyboard input) to the PTY.
    pub fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        self.pty_writer.write_all(bytes).context("writing to PTY")?;
        self.pty_writer.flush().context("flushing PTY writer")
    }

    /// Resize the PTY and the Term when the pane geometry changes.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
        let _ = self.pty_master.lock().resize(size);

        let term_size = TermSize { cols: cols as usize, rows: rows as usize };
        self.term.lock().resize(term_size);
    }
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

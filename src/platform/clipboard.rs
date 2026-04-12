//! System clipboard adapter for production runtime.
//!
//! This implementation satisfies [`crate::traits::Clipboard`] using
//! [`arboard::Clipboard`].

use anyhow::{Context, Result};

use crate::traits::Clipboard;

/// System clipboard backed by `arboard`.
pub struct SystemClipboard {
    inner: arboard::Clipboard,
}

impl SystemClipboard {
    /// Create a system clipboard instance.
    ///
    /// # Panics
    ///
    /// Panics if the process cannot access the platform clipboard backend.
    pub fn new() -> Self {
        Self {
            inner: arboard::Clipboard::new().expect("failed to access system clipboard"),
        }
    }
}

impl Clipboard for SystemClipboard {
    /// Read text from the system clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error with context when clipboard reads fail.
    fn get_text(&mut self) -> Result<String> {
        self.inner.get_text().context("reading from clipboard")
    }

    /// Write text to the system clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error with context when clipboard writes fail.
    fn set_text(&mut self, text: &str) -> Result<()> {
        self.inner
            .set_text(text.to_string())
            .context("writing to clipboard")
    }
}

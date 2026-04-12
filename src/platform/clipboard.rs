use anyhow::{Context, Result};

use crate::traits::Clipboard;

/// System clipboard backed by `arboard`.
pub struct SystemClipboard {
    inner: arboard::Clipboard,
}

impl SystemClipboard {
    pub fn new() -> Self {
        Self {
            inner: arboard::Clipboard::new().expect("failed to access system clipboard"),
        }
    }
}

impl Clipboard for SystemClipboard {
    fn get_text(&mut self) -> Result<String> {
        self.inner.get_text().context("reading from clipboard")
    }

    fn set_text(&mut self, text: &str) -> Result<()> {
        self.inner.set_text(text.to_string()).context("writing to clipboard")
    }
}

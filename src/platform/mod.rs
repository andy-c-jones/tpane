//! Live platform integrations.
//!
//! These modules provide PTY-backed panes, terminal rendering, event merging,
//! and clipboard adapters used by the production executable.
//!
//! # Module relationships
//!
//! - [`crate::platform::pane`]: PTY and terminal emulation per pane
//! - [`crate::platform::live`]: event loop adapters and factories
//! - [`crate::platform::renderer`]: ratatui drawing and key translation
//! - [`crate::platform::clipboard`]: system clipboard bridge

pub mod clipboard;
pub mod live;
pub mod pane;
pub mod renderer;

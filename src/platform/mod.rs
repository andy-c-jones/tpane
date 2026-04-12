//! Live platform integrations.
//!
//! These modules provide PTY-backed panes, terminal rendering, event merging,
//! and clipboard adapters used by the production executable.

pub mod clipboard;
pub mod live;
pub mod pane;
pub mod renderer;

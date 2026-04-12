//! Core domain logic for tpane.
//!
//! These modules are intentionally platform-agnostic and focus on command
//! parsing, key mapping, pane layout, and text selection behavior.

pub mod commands;
pub mod keymap;
pub mod layout;
pub mod selection;

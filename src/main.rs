//! tpane executable entry point and runtime wiring.
//!
//! This module bootstraps configuration, terminal state, and live platform
//! adapters, then hands control to [`crate::app::App`].
//!
//! # Notes
//!
//! Most application behavior lives in [`crate::app`] and core/platform modules;
//! this file focuses on wiring and lifecycle.

mod app;
mod config;
mod core;
#[cfg(test)]
mod headless;
mod platform;
#[cfg(test)]
mod tests_headless;
mod traits;

use anyhow::Result;

use crate::config::lua::LuaConfig;
use crate::platform::live::{LiveEventSource, LivePaneFactory, LiveRenderer};
use crate::platform::renderer;

fn main() -> Result<()> {
    env_logger::init();

    // Parse optional -N flag (1–9) to load a named layout at startup.
    let layout_index: Option<u8> = std::env::args().skip(1).find_map(|arg| {
        if arg.len() == 2 && arg.starts_with('-') {
            arg.chars().nth(1)?.to_digit(10).and_then(|d| {
                let n = d as u8;
                if n >= 1 {
                    Some(n)
                } else {
                    None
                }
            })
        } else {
            None
        }
    });

    let config = LuaConfig::load()?;
    let mut tui = renderer::init_terminal()?;

    let result = run(config, layout_index, &mut tui);

    renderer::restore_terminal(&mut tui)?;

    result
}

/// Construct and run the live application with real event, renderer, and clipboard backends.
///
/// If `layout_index` is `Some(n)`, the named layout `n` is applied at startup instead
/// of the default `on_startup` commands.
///
/// # Errors
///
/// Returns errors from terminal size probing, pane spawning, startup command
/// execution, and the main app loop.
fn run(config: LuaConfig, layout_index: Option<u8>, tui: &mut renderer::Tui) -> Result<()> {
    let size = crossterm::terminal::size()?;
    let mut factory = LivePaneFactory::new();
    let mut app = app::App::new(
        config.keymap,
        size,
        config.show_cheatsheet,
        config.named_layouts.clone(),
        &factory,
    )?;

    // Apply either the requested named layout or the default on_startup sequence.
    let startup_steps = layout_index
        .and_then(|n| config.named_layouts.get(&n).cloned())
        .unwrap_or(config.startup_commands);
    app.apply_layout(&startup_steps, &factory)?;

    let mut events = LiveEventSource::new(factory.event_rx());
    let mut renderer = LiveRenderer::new(tui);
    let mut clipboard = platform::clipboard::SystemClipboard::new();
    app.run(&mut events, &mut renderer, &factory, &mut clipboard)
}

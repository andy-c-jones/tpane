//! tpane executable entry point and runtime wiring.
//!
//! This module bootstraps configuration, terminal state, and live platform
//! adapters, then hands control to [`crate::app::App`].

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

    let config = LuaConfig::load()?;
    let mut tui = renderer::init_terminal()?;

    let result = run(config, &mut tui);

    renderer::restore_terminal(&mut tui)?;

    result
}

/// Construct and run the live application with real event, renderer, and clipboard backends.
fn run(config: LuaConfig, tui: &mut renderer::Tui) -> Result<()> {
    let size = crossterm::terminal::size()?;
    let mut factory = LivePaneFactory::new();
    let mut app = app::App::new(config.keymap, size, config.show_cheatsheet, &factory)?;
    app.apply_startup_commands(&config.startup_commands, &factory)?;
    let mut events = LiveEventSource::new(factory.event_rx());
    let mut renderer = LiveRenderer::new(tui);
    let mut clipboard = platform::clipboard::SystemClipboard::new();
    app.run(&mut events, &mut renderer, &factory, &mut clipboard)
}

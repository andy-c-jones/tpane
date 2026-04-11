mod app;
mod config;
mod core;
#[cfg(test)]
mod headless;
mod platform;
mod traits;
#[cfg(test)]
mod tests_headless;

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

fn run(config: LuaConfig, tui: &mut renderer::Tui) -> Result<()> {
    let size = crossterm::terminal::size()?;
    let mut factory = LivePaneFactory::new();
    let mut app = app::App::new(config.keymap, size, &factory)?;
    let mut events = LiveEventSource::new(factory.event_rx());
    let mut renderer = LiveRenderer::new(tui);
    app.run(&mut events, &mut renderer, &factory)
}

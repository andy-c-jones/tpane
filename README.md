# tpane

A cross-platform terminal multiplexer that works like a tiling window manager.

tpane splits your terminal into panes — vertically then horizontally — and lets you manage them with simple keybindings. Configuration is done in Lua, so you can remap keys and script startup layouts.

## Features

- **Tiling pane management** — binary tree layout with vertical and horizontal splits
- **Cross-platform** — Linux and Windows Terminal (via ConPTY)
- **Lua configuration** — keybindings, startup layouts, and extensible commands
- **Per-pane VT emulation** — powered by [alacritty_terminal](https://crates.io/crates/alacritty_terminal)
- **Kitty keyboard protocol** support

## Installation

```sh
cargo install --path .
```

Requires Rust 1.75+.

## Usage

```sh
tpane
```

On first run, tpane creates a default config at:

| Platform | Path |
|----------|------|
| Linux/macOS | `~/.config/tpane/main.lua` |
| Windows | `%APPDATA%\tpane\main.lua` |

## Default Keybindings

| Key | Action |
|-----|--------|
| `Ctrl+Shift+T` | Split vertical |
| `Ctrl+Shift+V` | Split vertical |
| `Ctrl+Shift+H` | Split horizontal |
| `Ctrl+Shift+W` | Close pane |
| `Ctrl+Shift+N` | Focus next pane |
| `Ctrl+Shift+P` | Focus previous pane |
| `Ctrl+Shift+Q` | Quit |

## Configuration

Edit `main.lua` to customize keybindings:

```lua
-- Remap split to Ctrl+Shift+S
tpane.bind("ctrl+shift+s", "split_vertical")

-- Define a startup layout
tpane.on_startup(function()
  tpane.split_vertical()
  tpane.split_horizontal()
end)
```

### Available commands

| Command | Description |
|---------|-------------|
| `split_vertical` | Split the active pane left/right |
| `split_horizontal` | Split the active pane top/bottom |
| `split` | Alias for `split_vertical` |
| `close` | Close the active pane |
| `focus_next` | Move focus to the next pane |
| `focus_prev` | Move focus to the previous pane |
| `quit` | Exit tpane |

### Key format

Modifier keys are joined with `+`. Supported modifiers: `ctrl`, `shift`, `alt` (or `meta`).

Key names: `a`–`z`, `0`–`9`, `f1`–`f12`, `enter`, `space`, `tab`, `backspace`, `delete`, `escape`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`.

## Development

```sh
# Build
cargo build

# Run tests (85 tests, ~78% coverage)
cargo test

# Coverage report (requires cargo-llvm-cov)
cargo llvm-cov --html
# Report at target/llvm-cov/html/index.html
```

### Architecture

tpane uses trait-based abstractions for testability:

- **`EventSource`** — provides keyboard, resize, and pane I/O events
- **`PaneBackend`** — per-pane shell I/O and terminal content
- **`PaneFactory`** — creates pane backends
- **`Renderer`** — draws the UI

`App<B: PaneBackend>` is generic over the backend. Production uses real PTY/terminal implementations; tests use headless mocks that run without a terminal.

## Roadmap

- [ ] Switch VT backend to [libghostty-vt](https://github.com/ghostty-org/ghostty) once Rust bindings are available
- [ ] Mouse support
- [ ] Scrollback buffer
- [ ] Named panes and pane navigation by name
- [ ] Plugin system via Lua

## License

MIT

# tpane

A cross-platform terminal multiplexer that works like a tiling window manager.

tpane splits your terminal into panes — vertically then horizontally — and lets you manage them with simple keybindings. Configuration is done in Lua, so you can remap keys and script startup layouts.

<img width="2560" height="1412" alt="image" src="https://github.com/user-attachments/assets/6f9fb07c-329e-4ebf-9667-5f6ca259093c" />

## Features

- **Tiling pane management** — binary tree layout with vertical and horizontal splits
- **Cross-platform** — Linux and Windows Terminal (via ConPTY)
- **Lua configuration** — keybindings, startup layouts, and extensible commands
- **Per-pane VT emulation** — powered by [alacritty_terminal](https://crates.io/crates/alacritty_terminal)
- **Mouse interactions** — click to focus, drag dividers to resize, and selection copy/paste helpers
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

tpane uses a **prefix key** system (like tmux). Press `Ctrl+B` first, then the command key:

| After `Ctrl+B` | Action |
|-----------------|--------|
| `Ctrl+←` | Split left (new pane on the left) |
| `Ctrl+→` | Split right (new pane on the right) |
| `Ctrl+↑` | Split up (new pane above) |
| `Ctrl+↓` | Split down (new pane below) |
| `←` | Focus nearest pane on the left |
| `→` | Focus nearest pane on the right |
| `↑` | Focus nearest pane above |
| `↓` | Focus nearest pane below |
| `w` | Close pane |
| `q` | Quit |

Direct (no prefix) resize keybindings:

| Key | Action |
|-----|--------|
| `Alt+Shift+←` | Grow pane to the left |
| `Alt+Shift+→` | Grow pane to the right |
| `Alt+Shift+↑` | Grow pane upward |
| `Alt+Shift+↓` | Grow pane downward |

## Configuration

Edit `main.lua` to customize keybindings (these are applied after the prefix key):

```lua
-- Remap close to 'x' (after Ctrl+B)
tpane.bind("x", "close")

-- Define a startup layout
tpane.on_startup(function()
  tpane.split_vertical()
  tpane.split_horizontal()
end)
```

### Available commands

| Command | Description |
|---------|-------------|
| `split_left` | Split active pane, new pane on the left |
| `split_right` | Split active pane, new pane on the right |
| `split_up` | Split active pane, new pane above |
| `split_down` | Split active pane, new pane below |
| `split_vertical` | Split the active pane left/right (alias for `split_right`) |
| `split_horizontal` | Split the active pane top/bottom (alias for `split_down`) |
| `split` | Alias for `split_vertical` |
| `close` | Close the active pane |
| `close_pane` | Alias for `close` |
| `focus_next` | Move focus to the next pane |
| `focus_prev` | Move focus to the previous pane |
| `focus_left` | Move focus to the nearest pane on the left |
| `focus_right` | Move focus to the nearest pane on the right |
| `focus_up` | Move focus to the nearest pane above |
| `focus_down` | Move focus to the nearest pane below |
| `resize_left` | Grow the active pane to the left |
| `resize_right` | Grow the active pane to the right |
| `resize_up` | Grow the active pane upward |
| `resize_down` | Grow the active pane downward |
| `quit` | Exit tpane |

### Key format

Modifier keys are joined with `+`. Supported modifiers: `ctrl`, `shift`, `alt` (or `meta`).

Key names: `a`–`z`, `0`–`9`, `f1`–`f12`, `enter`, `space`, `tab`, `backspace`, `delete`, `escape`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`.

## Development

```sh
# Build
cargo build

# Run tests
cargo test

# CI-style tests (nextest profile)
cargo nextest run --workspace --all-targets --profile ci

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

## License

MIT

# Copilot instructions for `tpane`

## Build, test, and lint commands

- If `cargo` is not in PATH in this environment, run `source "$HOME/.cargo/env"` first.
- Build: `cargo build`
- Full test suite: `cargo test`
- Single test (rust test harness): `cargo test tests_headless::tests::ctrl_b_activates_prefix_mode`
- CI-style tests (nextest profile from `.config/nextest.toml`): `cargo nextest run --workspace --all-targets --profile ci`
- Single test with nextest: `cargo nextest run --profile ci tests_headless::tests::ctrl_b_activates_prefix_mode`
- Formatting check (also enforced in CI and by pre-commit hook): `cargo fmt --all -- --check`
- Coverage: `cargo llvm-cov --workspace --all-targets --summary-only`

## High-level architecture

- `src/main.rs` wires everything together: it loads `LuaConfig`, initializes the terminal, constructs `App`, applies startup commands, and runs with live platform adapters.
- Core behavior is trait-driven (`src/traits.rs`): `EventSource`, `PaneBackend`, `PaneFactory`, `Renderer`, and `Clipboard`.
- `App<B: PaneBackend>` in `src/app.rs` is the main coordinator:
  - runs the event loop
  - handles key/mouse/resize/pane-exit events
  - dispatches commands
  - keeps runtime pane backends (`HashMap<PaneId, B>`) in sync with layout geometry
- `src/core/layout.rs` is pure layout logic (binary split tree + focus history + spatial navigation). It computes pane rectangles and divider metadata; it does not perform terminal/PTY I/O.
- Live platform implementation is split by concern:
  - `src/platform/pane.rs`: per-pane PTY + alacritty terminal state
  - `src/platform/live.rs`: merges crossterm input with pane events and provides live factories/renderers
  - `src/platform/renderer.rs`: draws borders/content/cheatsheet, translates keys to PTY byte sequences, and provides `encode_mouse_scroll`
  - `src/platform/clipboard.rs`: system clipboard adapter
- Test/headless path mirrors live abstractions:
  - `src/headless.rs`: in-memory implementations of the same traits
  - `src/tests_headless.rs`: integration-style behavior tests against `App` using headless components
- Config path is Lua-based (`src/config/lua.rs` + `src/config/defaults.rs`): on first run, default `main.lua` is written, then runtime bindings/startup commands are resolved from Lua.

## Key codebase conventions

- Keep `App` generic over `PaneBackend` and interact through traits; avoid introducing direct dependencies on live platform types inside core app logic.
- Key handling precedence in `App` is intentional:
  1. global shortcuts (`Ctrl+Shift+C/V`)
  2. prefix-mode command resolution
  3. prefix-key activation
  4. direct bindings (non-prefix, holdable)
  5. PageUp/PageDown scrollback interception (main-screen only)
  6. fallback raw key forwarding to active pane (snaps scrollback to bottom first)
- `KeyEventKind::Repeat` is deliberately restricted to direct bindings + raw forwarding; repeat events must not trigger prefix-key/global-shortcut behavior.
- Split ratio semantics are user-facing in terms of **original pane keeps X%**, while internal layout ratios are **first child gets X%**; conversion/clamping is handled in `App::split` and `Layout`.
- Geometry conventions are consistent across app/renderer:
  - pane backend sizes are inner content sizes (border excluded)
  - widths/heights are clamped with `saturating_sub(...).max(...)` to stay usable on small terminals
- When changing behavior, prefer adding/updating coverage in `src/tests_headless.rs` (end-to-end app behavior) and keep low-level unit tests in module-local `#[cfg(test)]` blocks.
- `PaneBackend` scrollback methods (`is_alt_screen`, `is_mouse_mode`, `is_sgr_mouse`, `is_alternate_scroll`, `display_offset`, `scroll_page_up/down`, `scroll_by_lines`, `scroll_to_bottom`) all have default no-op implementations so existing backends are unaffected. Implement them in `src/platform/live.rs` by delegating to `PaneState` methods.
- Mouse scroll routing has three cases (checked in order): mouse-reporting mode active → forward encoded bytes via `encode_mouse_scroll`; alt-screen + alternate-scroll → send cursor-up/down arrow sequences; otherwise → scroll the terminal buffer with `scroll_by_lines`. Do **not** auto-snap to bottom on passive `PaneData` events; only snap on explicit user key input sent to the PTY.
- Selection's `display_offset` must be captured from the pane at drag-start so that copy-from-scrollback uses the correct grid rows if the viewport moves before copy is invoked.

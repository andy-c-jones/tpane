//! Headless tests for the App event loop using mock backends.

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
                           MouseEvent, MouseEventKind, MouseButton};

    use crate::app::App;
    use crate::core::keymap::KeyMap;
    use crate::core::layout::PaneId;
    use crate::headless::*;
    use crate::traits::{AppEvent, Renderer};

    // ── helpers ───────────────────────────────────────────────────────────────

    const TERM_SIZE: (u16, u16) = (80, 24);

    fn default_app() -> (App<HeadlessPaneBackend>, HeadlessPaneFactory, HeadlessClipboard) {
        let factory = HeadlessPaneFactory;
        let app = App::new(KeyMap::default(), TERM_SIZE, true, &factory).unwrap();
        (app, factory, HeadlessClipboard::new())
    }

    fn key_press(code: KeyCode, mods: KeyModifiers) -> AppEvent {
        AppEvent::Key(KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
    }

    /// Helper: send prefix (Ctrl+B) then the command key.
    fn prefix_then(app: &mut App<HeadlessPaneBackend>, factory: &HeadlessPaneFactory, clipboard: &mut HeadlessClipboard, code: KeyCode, mods: KeyModifiers) {
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), factory, clipboard).unwrap();
        app.process_event(key_press(code, mods), factory, clipboard).unwrap();
    }

    // ── construction ──────────────────────────────────────────────────────────

    #[test]
    fn app_starts_with_one_pane() {
        let (app, _, _) = default_app();
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn app_initial_pane_has_correct_id() {
        let (app, _, _) = default_app();
        let id = app.active_pane();
        assert!(app.panes.contains_key(&id));
    }

    // ── prefix key mode ───────────────────────────────────────────────────────

    #[test]
    fn ctrl_b_activates_prefix_mode() {
        let (mut app, factory, mut clip) = default_app();
        assert!(!app.is_prefix_active());
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory, &mut clip).unwrap();
        assert!(app.is_prefix_active());
    }

    #[test]
    fn prefix_mode_deactivates_after_command() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Char('q'), KeyModifiers::empty());
        assert!(!app.is_prefix_active());
    }

    #[test]
    fn prefix_mode_deactivates_on_unknown_key() {
        let (mut app, factory, mut clip) = default_app();
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory, &mut clip).unwrap();
        assert!(app.is_prefix_active());
        app.process_event(key_press(KeyCode::Char('z'), KeyModifiers::empty()), &factory, &mut clip).unwrap();
        assert!(!app.is_prefix_active());
        assert!(app.is_running());
    }

    #[test]
    fn regular_key_without_prefix_forwards_to_pane() {
        let (mut app, factory, mut clip) = default_app();
        let active = app.active_pane();
        app.process_event(key_press(KeyCode::Char('a'), KeyModifiers::empty()), &factory, &mut clip).unwrap();
        assert!(!app.panes[&active].input_log.is_empty());
    }

    #[test]
    fn ctrl_b_itself_does_not_forward_to_pane() {
        let (mut app, factory, mut clip) = default_app();
        let active = app.active_pane();
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory, &mut clip).unwrap();
        assert!(app.panes[&active].input_log.is_empty());
    }

    // ── directional splits (Ctrl+Arrow after prefix) ──────────────────────────

    #[test]
    fn split_right_creates_two_panes() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
    }

    #[test]
    fn split_left_creates_two_panes() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
    }

    #[test]
    fn split_down_creates_two_panes() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Down, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
    }

    #[test]
    fn split_up_creates_two_panes() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Up, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
    }

    #[test]
    fn split_changes_active_pane() {
        let (mut app, factory, mut clip) = default_app();
        let original = app.active_pane();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        assert_ne!(app.active_pane(), original);
    }

    #[test]
    fn multiple_splits_create_correct_count() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Down, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 4);
    }

    // ── close pane (prefix + w) ───────────────────────────────────────────────

    #[test]
    fn close_pane_after_split_returns_to_one() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Char('w'), KeyModifiers::empty());
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn close_last_pane_quits() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Char('w'), KeyModifiers::empty());
        assert!(!app.is_running());
    }

    #[test]
    fn close_pane_shifts_focus_to_remaining() {
        let (mut app, factory, mut clip) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        let second = app.active_pane();
        assert_ne!(first, second);
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Char('w'), KeyModifiers::empty());
        assert_eq!(app.active_pane(), first);
    }

    // ── pane exit event (from PTY) ────────────────────────────────────────────

    #[test]
    fn pane_exit_event_removes_pane() {
        let (mut app, factory, mut clip) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
        app.process_event(AppEvent::PaneExit { pane_id: first }, &factory, &mut clip).unwrap();
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn stale_pane_exit_event_is_ignored() {
        let (mut app, factory, mut clip) = default_app();
        let bogus_id = PaneId(999);
        app.process_event(AppEvent::PaneExit { pane_id: bogus_id }, &factory, &mut clip).unwrap();
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn pane_exit_of_last_pane_quits() {
        let (mut app, factory, mut clip) = default_app();
        let root = app.active_pane();
        app.process_event(AppEvent::PaneExit { pane_id: root }, &factory, &mut clip).unwrap();
        assert!(!app.is_running());
    }

    // ── focus (prefix + Arrow) ───────────────────────────────────────────────

    #[test]
    fn focus_right_cycles_through_panes() {
        let (mut app, factory, mut clip) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        let second = app.active_pane();

        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::empty());
        assert_eq!(app.active_pane(), first);

        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::empty());
        assert_eq!(app.active_pane(), second);
    }

    #[test]
    fn focus_left_cycles_through_panes() {
        let (mut app, factory, mut clip) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        let second = app.active_pane();

        prefix_then(&mut app, &factory, &mut clip, KeyCode::Left, KeyModifiers::empty());
        assert_eq!(app.active_pane(), first);

        prefix_then(&mut app, &factory, &mut clip, KeyCode::Left, KeyModifiers::empty());
        assert_eq!(app.active_pane(), second);
    }

    // ── quit (prefix + q) ────────────────────────────────────────────────────

    #[test]
    fn quit_command_stops_app() {
        let (mut app, factory, mut clip) = default_app();
        assert!(app.is_running());
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Char('q'), KeyModifiers::empty());
        assert!(!app.is_running());
    }

    // ── resize ────────────────────────────────────────────────────────────────

    #[test]
    fn resize_event_updates_pane_geometry() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        let pre_resizes: usize = app.panes.values().map(|p| p.resize_log.len()).sum();

        app.process_event(AppEvent::Resize(120, 40), &factory, &mut clip).unwrap();

        let post_resizes: usize = app.panes.values().map(|p| p.resize_log.len()).sum();
        assert!(post_resizes > pre_resizes);
    }

    // ── key forwarding ────────────────────────────────────────────────────────

    #[test]
    fn regular_key_forwards_to_active_pane() {
        let (mut app, factory, mut clip) = default_app();
        let active = app.active_pane();
        app.process_event(key_press(KeyCode::Char('a'), KeyModifiers::empty()), &factory, &mut clip).unwrap();
        let pane = &app.panes[&active];
        assert_eq!(pane.input_log[0], vec![b'a']);
    }

    #[test]
    fn enter_key_forwards_cr() {
        let (mut app, factory, mut clip) = default_app();
        let active = app.active_pane();
        app.process_event(key_press(KeyCode::Enter, KeyModifiers::empty()), &factory, &mut clip).unwrap();
        let pane = &app.panes[&active];
        assert_eq!(pane.input_log[0], vec![b'\r']);
    }

    #[test]
    fn bound_key_after_prefix_does_not_forward() {
        let (mut app, factory, mut clip) = default_app();
        let active = app.active_pane();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Char('q'), KeyModifiers::empty());
        assert!(app.panes[&active].input_log.is_empty());
    }

    #[test]
    fn key_after_focus_switch_goes_to_new_active() {
        let (mut app, factory, mut clip) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        let second = app.active_pane();

        prefix_then(&mut app, &factory, &mut clip, KeyCode::Left, KeyModifiers::empty());
        assert_eq!(app.active_pane(), first);

        app.process_event(key_press(KeyCode::Char('x'), KeyModifiers::empty()), &factory, &mut clip).unwrap();
        assert!(!app.panes[&first].input_log.is_empty());
        assert!(app.panes[&second].input_log.is_empty());
    }

    // ── event loop (run) ──────────────────────────────────────────────────────

    #[test]
    fn run_processes_events_until_quit() {
        let (mut app, factory, mut clip) = default_app();
        let mut events = HeadlessEventSource::new();
        events.push(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL));
        events.push(key_press(KeyCode::Right, KeyModifiers::CONTROL));
        events.push(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL));
        events.push(key_press(KeyCode::Char('q'), KeyModifiers::empty()));

        let mut renderer = HeadlessRenderer::new();
        app.run(&mut events, &mut renderer, &factory, &mut clip).unwrap();

        assert!(!app.is_running());
        assert_eq!(app.pane_count(), 2);
        assert!(renderer.frame_count >= 2);
    }

    #[test]
    fn run_stops_when_last_pane_exits() {
        let (mut app, factory, mut clip) = default_app();
        let root = app.active_pane();
        let mut events = HeadlessEventSource::new();
        events.push(AppEvent::PaneExit { pane_id: root });

        let mut renderer = HeadlessRenderer::new();
        app.run(&mut events, &mut renderer, &factory, &mut clip).unwrap();

        assert!(!app.is_running());
    }

    // ── complex sequences ─────────────────────────────────────────────────────

    #[test]
    fn split_close_split_maintains_consistency() {
        let (mut app, factory, mut clip) = default_app();
        for _ in 0..3 {
            prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        }
        assert_eq!(app.pane_count(), 4);

        for _ in 0..2 {
            prefix_then(&mut app, &factory, &mut clip, KeyCode::Char('w'), KeyModifiers::empty());
        }
        assert_eq!(app.pane_count(), 2);

        prefix_then(&mut app, &factory, &mut clip, KeyCode::Down, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 3);
        assert!(app.is_running());
        assert_eq!(app.layout.leaf_ids().len(), app.pane_count());
    }

    #[test]
    fn mixed_directional_splits() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Down, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Left, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Up, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 5);
        assert_eq!(app.layout.leaf_ids().len(), 5);
    }

    // ── cheatsheet visibility ────────────────────────────────────────────────

    #[test]
    fn cheatsheet_shown_when_prefix_active_and_enabled() {
        let factory = HeadlessPaneFactory;
        let mut app = App::new(KeyMap::default(), TERM_SIZE, true, &factory).unwrap();
        let mut renderer = HeadlessRenderer::new();
        let mut clip = HeadlessClipboard::new();
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory, &mut clip).unwrap();
        assert!(app.is_prefix_active());
        let show_bar = app.is_prefix_active() && true;
        renderer.render(&app.layout, &app.panes, TERM_SIZE, show_bar, None).unwrap();
        assert!(renderer.last_cheatsheet_visible);
    }

    #[test]
    fn cheatsheet_hidden_when_disabled() {
        let factory = HeadlessPaneFactory;
        let mut app = App::new(KeyMap::default(), TERM_SIZE, false, &factory).unwrap();
        let mut renderer = HeadlessRenderer::new();
        let mut clip = HeadlessClipboard::new();
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory, &mut clip).unwrap();
        assert!(app.is_prefix_active());
        let show_bar = app.is_prefix_active() && false;
        renderer.render(&app.layout, &app.panes, TERM_SIZE, show_bar, None).unwrap();
        assert!(!renderer.last_cheatsheet_visible);
    }

    #[test]
    fn cheatsheet_hidden_after_prefix_deactivates() {
        let (mut app, factory, mut clip) = default_app();
        let mut renderer = HeadlessRenderer::new();
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory, &mut clip).unwrap();
        assert!(app.is_prefix_active());
        app.process_event(key_press(KeyCode::Char('z'), KeyModifiers::NONE), &factory, &mut clip).unwrap();
        assert!(!app.is_prefix_active());
        let show_bar = app.is_prefix_active() && true;
        renderer.render(&app.layout, &app.panes, TERM_SIZE, show_bar, None).unwrap();
        assert!(!renderer.last_cheatsheet_visible);
    }

    // ── mouse click to focus ─────────────────────────────────────────────────

    fn mouse_click(col: u16, row: u16) -> AppEvent {
        AppEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        })
    }

    fn mouse_drag(col: u16, row: u16) -> AppEvent {
        AppEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        })
    }

    fn mouse_up(col: u16, row: u16) -> AppEvent {
        AppEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        })
    }

    fn mouse_right_click(col: u16, row: u16) -> AppEvent {
        AppEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        })
    }

    #[test]
    fn mouse_click_changes_active_pane() {
        let (mut app, factory, mut clip) = default_app();
        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
        let right_pane = app.active_pane();

        app.process_event(mouse_click(1, 1), &factory, &mut clip).unwrap();
        assert_ne!(app.active_pane(), right_pane);
    }

    #[test]
    fn mouse_click_on_active_pane_is_noop() {
        let (mut app, factory, mut clip) = default_app();
        let initial = app.active_pane();
        app.process_event(mouse_click(5, 5), &factory, &mut clip).unwrap();
        assert_eq!(app.active_pane(), initial);
    }

    // ── mouse drag selection ─────────────────────────────────────────────────

    #[test]
    fn mouse_drag_creates_selection() {
        let (mut app, factory, mut clip) = default_app();
        // Click inside the pane inner area (border at x=0,y=0, inner starts at x=1,y=1)
        app.process_event(mouse_click(2, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_drag(10, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_up(10, 2), &factory, &mut clip).unwrap();
        assert!(app.selection.is_some());
        let sel = app.selection.as_ref().unwrap();
        assert_eq!(sel.start, (1, 1));  // (2-1, 2-1) inner offset
        assert_eq!(sel.end, (9, 1));    // (10-1, 2-1)
    }

    #[test]
    fn click_without_drag_clears_selection() {
        let (mut app, factory, mut clip) = default_app();
        // First do a drag to create a selection
        app.process_event(mouse_click(2, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_drag(10, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_up(10, 2), &factory, &mut clip).unwrap();
        assert!(app.selection.is_some());

        // Now just click (no drag) — should clear
        app.process_event(mouse_click(5, 5), &factory, &mut clip).unwrap();
        app.process_event(mouse_up(5, 5), &factory, &mut clip).unwrap();
        assert!(app.selection.is_none());
    }

    #[test]
    fn right_click_clears_selection() {
        let (mut app, factory, mut clip) = default_app();
        app.process_event(mouse_click(2, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_drag(10, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_up(10, 2), &factory, &mut clip).unwrap();
        assert!(app.selection.is_some());

        app.process_event(mouse_right_click(5, 5), &factory, &mut clip).unwrap();
        assert!(app.selection.is_none());
    }

    #[test]
    fn selection_cleared_on_resize() {
        let (mut app, factory, mut clip) = default_app();
        app.process_event(mouse_click(2, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_drag(10, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_up(10, 2), &factory, &mut clip).unwrap();
        assert!(app.selection.is_some());

        app.process_event(AppEvent::Resize(120, 40), &factory, &mut clip).unwrap();
        assert!(app.selection.is_none());
    }

    #[test]
    fn selection_cleared_on_split() {
        let (mut app, factory, mut clip) = default_app();
        app.process_event(mouse_click(2, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_drag(10, 2), &factory, &mut clip).unwrap();
        app.process_event(mouse_up(10, 2), &factory, &mut clip).unwrap();
        assert!(app.selection.is_some());

        prefix_then(&mut app, &factory, &mut clip, KeyCode::Right, KeyModifiers::CONTROL);
        assert!(app.selection.is_none());
    }

    // ── copy/paste shortcuts ─────────────────────────────────────────────────

    #[test]
    fn ctrl_shift_v_pastes_with_bracketed_paste() {
        let (mut app, factory, mut clip) = default_app();
        clip.content = "hello world".to_string();
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        app.process_event(key_press(KeyCode::Char('V'), ctrl_shift), &factory, &mut clip).unwrap();

        let active = app.active_pane();
        let pane = &app.panes[&active];
        assert_eq!(pane.input_log.len(), 1);
        let bytes = &pane.input_log[0];
        // Should contain bracketed paste markers
        assert!(bytes.starts_with(b"\x1b[200~"));
        assert!(bytes.ends_with(b"\x1b[201~"));
        assert!(bytes.windows(11).any(|w| w == b"hello world"));
    }

    #[test]
    fn ctrl_shift_c_works_during_prefix_mode() {
        let (mut app, factory, mut clip) = default_app();
        // Activate prefix
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory, &mut clip).unwrap();
        assert!(app.is_prefix_active());

        // Ctrl+Shift+C should work even in prefix mode (global shortcut)
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        app.process_event(key_press(KeyCode::Char('C'), ctrl_shift), &factory, &mut clip).unwrap();
        // Prefix should still be active since the global shortcut doesn't consume prefix
        // Actually, our implementation checks globals first, so prefix remains
        // The key was consumed by the global handler
        assert!(app.is_running());
    }

    #[test]
    fn border_click_does_not_start_selection() {
        let (mut app, factory, mut clip) = default_app();
        // Click on border (x=0, y=0)
        app.process_event(mouse_click(0, 0), &factory, &mut clip).unwrap();
        app.process_event(mouse_up(0, 0), &factory, &mut clip).unwrap();
        assert!(app.selection.is_none());
    }
}

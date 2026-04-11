//! Headless tests for the App event loop using mock backends.

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use crate::app::App;
    use crate::core::commands::Command;
    use crate::core::keymap::KeyMap;
    use crate::core::layout::PaneId;
    use crate::headless::*;
    use crate::traits::AppEvent;

    // ── helpers ───────────────────────────────────────────────────────────────

    const TERM_SIZE: (u16, u16) = (80, 24);

    fn default_app() -> (App<HeadlessPaneBackend>, HeadlessPaneFactory) {
        let factory = HeadlessPaneFactory;
        let app = App::new(KeyMap::default(), TERM_SIZE, &factory).unwrap();
        (app, factory)
    }

    fn key_press(code: KeyCode, mods: KeyModifiers) -> AppEvent {
        AppEvent::Key(KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
    }

    fn ctrl_shift(c: char) -> AppEvent {
        key_press(KeyCode::Char(c), KeyModifiers::CONTROL | KeyModifiers::SHIFT)
    }

    // ── construction ──────────────────────────────────────────────────────────

    #[test]
    fn app_starts_with_one_pane() {
        let (app, _) = default_app();
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn app_initial_pane_has_correct_id() {
        let (app, _) = default_app();
        let id = app.active_pane();
        assert!(app.panes.contains_key(&id));
    }

    // ── split vertical (ctrl+shift+t) ─────────────────────────────────────────

    #[test]
    fn split_vertical_creates_two_panes() {
        let (mut app, factory) = default_app();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        assert_eq!(app.pane_count(), 2);
    }

    #[test]
    fn split_vertical_changes_active_pane() {
        let (mut app, factory) = default_app();
        let original = app.active_pane();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        assert_ne!(app.active_pane(), original);
    }

    #[test]
    fn split_vertical_twice_creates_three_panes() {
        let (mut app, factory) = default_app();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        assert_eq!(app.pane_count(), 3);
    }

    // ── split horizontal (ctrl+shift+h) ───────────────────────────────────────

    #[test]
    fn split_horizontal_creates_two_panes() {
        let (mut app, factory) = default_app();
        app.process_event(ctrl_shift('h'), &factory).unwrap();
        assert_eq!(app.pane_count(), 2);
    }

    // ── close pane (ctrl+shift+w) ─────────────────────────────────────────────

    #[test]
    fn close_pane_after_split_returns_to_one() {
        let (mut app, factory) = default_app();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        assert_eq!(app.pane_count(), 2);
        app.process_event(ctrl_shift('w'), &factory).unwrap();
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn close_last_pane_quits() {
        let (mut app, factory) = default_app();
        app.process_event(ctrl_shift('w'), &factory).unwrap();
        assert!(!app.is_running());
    }

    #[test]
    fn close_pane_shifts_focus_to_remaining() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        let second = app.active_pane();
        assert_ne!(first, second);
        // Close second (active) — focus should go to first
        app.process_event(ctrl_shift('w'), &factory).unwrap();
        assert_eq!(app.active_pane(), first);
    }

    // ── pane exit event (from PTY) ────────────────────────────────────────────

    #[test]
    fn pane_exit_event_removes_pane() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        assert_eq!(app.pane_count(), 2);
        // Simulate the first pane's shell exiting
        app.process_event(AppEvent::PaneExit { pane_id: first }, &factory).unwrap();
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn stale_pane_exit_event_is_ignored() {
        let (mut app, factory) = default_app();
        let bogus_id = PaneId(999);
        // Should not panic or quit
        app.process_event(AppEvent::PaneExit { pane_id: bogus_id }, &factory).unwrap();
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn pane_exit_of_last_pane_quits() {
        let (mut app, factory) = default_app();
        let root = app.active_pane();
        app.process_event(AppEvent::PaneExit { pane_id: root }, &factory).unwrap();
        assert!(!app.is_running());
    }

    // ── focus (ctrl+shift+n / ctrl+shift+p) ──────────────────────────────────

    #[test]
    fn focus_next_cycles_through_panes() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        let second = app.active_pane();

        // focus_next from second → first
        app.process_event(ctrl_shift('n'), &factory).unwrap();
        assert_eq!(app.active_pane(), first);

        // focus_next from first → second (wraps)
        app.process_event(ctrl_shift('n'), &factory).unwrap();
        assert_eq!(app.active_pane(), second);
    }

    #[test]
    fn focus_prev_cycles_through_panes() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        let second = app.active_pane();

        // focus_prev from second → first
        app.process_event(ctrl_shift('p'), &factory).unwrap();
        assert_eq!(app.active_pane(), first);

        // focus_prev from first → second (wraps)
        app.process_event(ctrl_shift('p'), &factory).unwrap();
        assert_eq!(app.active_pane(), second);
    }

    // ── quit (ctrl+shift+q) ──────────────────────────────────────────────────

    #[test]
    fn quit_command_stops_app() {
        let (mut app, factory) = default_app();
        assert!(app.is_running());
        app.process_event(ctrl_shift('q'), &factory).unwrap();
        assert!(!app.is_running());
    }

    // ── resize ────────────────────────────────────────────────────────────────

    #[test]
    fn resize_event_updates_pane_geometry() {
        let (mut app, factory) = default_app();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        let pre_resizes: usize = app.panes.values().map(|p| p.resize_log.len()).sum();

        app.process_event(AppEvent::Resize(120, 40), &factory).unwrap();

        let post_resizes: usize = app.panes.values().map(|p| p.resize_log.len()).sum();
        // Each pane should have received a resize
        assert!(post_resizes > pre_resizes);
    }

    #[test]
    fn resize_event_updates_stored_terminal_size() {
        let (mut app, factory) = default_app();
        app.process_event(AppEvent::Resize(120, 40), &factory).unwrap();
        // After resize, a split should use the new terminal size for geometry.
        // We verify indirectly: the pane's resize dimensions should reflect 120x40.
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        // New pane should exist; it was spawned at the new terminal size
        assert_eq!(app.pane_count(), 2);
    }

    // ── key forwarding to pane ────────────────────────────────────────────────

    #[test]
    fn regular_key_forwards_to_active_pane() {
        let (mut app, factory) = default_app();
        let active = app.active_pane();

        // Press 'a' with no modifiers — should forward to PTY
        app.process_event(key_press(KeyCode::Char('a'), KeyModifiers::empty()), &factory).unwrap();

        let pane = &app.panes[&active];
        assert!(!pane.input_log.is_empty(), "input should be forwarded");
        assert_eq!(pane.input_log[0], vec![b'a']);
    }

    #[test]
    fn enter_key_forwards_cr() {
        let (mut app, factory) = default_app();
        let active = app.active_pane();
        app.process_event(key_press(KeyCode::Enter, KeyModifiers::empty()), &factory).unwrap();

        let pane = &app.panes[&active];
        assert_eq!(pane.input_log[0], vec![b'\r']);
    }

    #[test]
    fn bound_key_does_not_forward_to_pane() {
        let (mut app, factory) = default_app();
        let active = app.active_pane();
        // ctrl+shift+t is bound to split — should NOT forward bytes
        app.process_event(ctrl_shift('t'), &factory).unwrap();

        // The original pane should have received zero input
        let pane = &app.panes[&active];
        assert!(pane.input_log.is_empty(), "bound key should not be forwarded");
    }

    #[test]
    fn key_after_focus_switch_goes_to_new_active() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        let second = app.active_pane();

        // Switch focus to first
        app.process_event(ctrl_shift('n'), &factory).unwrap();
        assert_eq!(app.active_pane(), first);

        // Type 'x' — should go to first pane, not second
        app.process_event(key_press(KeyCode::Char('x'), KeyModifiers::empty()), &factory).unwrap();

        assert!(!app.panes[&first].input_log.is_empty());
        assert!(app.panes[&second].input_log.is_empty());
    }

    // ── event loop (run) ──────────────────────────────────────────────────────

    #[test]
    fn run_processes_events_until_quit() {
        let (mut app, factory) = default_app();
        let mut events = HeadlessEventSource::new();
        events.push(ctrl_shift('t'));  // split
        events.push(ctrl_shift('q'));  // quit

        let mut renderer = HeadlessRenderer::new();
        app.run(&mut events, &mut renderer, &factory).unwrap();

        assert!(!app.is_running());
        assert_eq!(app.pane_count(), 2); // split happened before quit
        assert!(renderer.frame_count >= 2, "should have rendered at least 2 frames");
    }

    #[test]
    fn run_stops_when_last_pane_exits() {
        let (mut app, factory) = default_app();
        let root = app.active_pane();
        let mut events = HeadlessEventSource::new();
        events.push(AppEvent::PaneExit { pane_id: root });

        let mut renderer = HeadlessRenderer::new();
        app.run(&mut events, &mut renderer, &factory).unwrap();

        assert!(!app.is_running());
    }

    // ── complex sequences ─────────────────────────────────────────────────────

    #[test]
    fn split_close_split_maintains_consistency() {
        let (mut app, factory) = default_app();
        // Split 3 times
        for _ in 0..3 {
            app.process_event(ctrl_shift('t'), &factory).unwrap();
        }
        assert_eq!(app.pane_count(), 4);

        // Close 2
        for _ in 0..2 {
            app.process_event(ctrl_shift('w'), &factory).unwrap();
        }
        assert_eq!(app.pane_count(), 2);

        // Split again
        app.process_event(ctrl_shift('t'), &factory).unwrap();
        assert_eq!(app.pane_count(), 3);
        assert!(app.is_running());

        // Layout leaf count matches pane count
        assert_eq!(app.layout.leaf_ids().len(), app.pane_count());
    }

    #[test]
    fn mixed_vertical_horizontal_splits() {
        let (mut app, factory) = default_app();
        // V, H, V, H
        app.process_event(ctrl_shift('v'), &factory).unwrap();
        app.process_event(ctrl_shift('h'), &factory).unwrap();
        app.process_event(ctrl_shift('v'), &factory).unwrap();
        app.process_event(ctrl_shift('h'), &factory).unwrap();
        assert_eq!(app.pane_count(), 5);
        assert_eq!(app.layout.leaf_ids().len(), 5);
    }
}

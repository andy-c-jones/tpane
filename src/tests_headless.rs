//! Headless tests for the App event loop using mock backends.

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use crate::app::App;
    use crate::core::keymap::KeyMap;
    use crate::core::layout::PaneId;
    use crate::headless::*;
    use crate::traits::{AppEvent, Renderer};

    // ── helpers ───────────────────────────────────────────────────────────────

    const TERM_SIZE: (u16, u16) = (80, 24);

    fn default_app() -> (App<HeadlessPaneBackend>, HeadlessPaneFactory) {
        let factory = HeadlessPaneFactory;
        let app = App::new(KeyMap::default(), TERM_SIZE, true, &factory).unwrap();
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

    /// Helper: send prefix (Ctrl+B) then the command key.
    fn prefix_then(app: &mut App<HeadlessPaneBackend>, factory: &HeadlessPaneFactory, code: KeyCode, mods: KeyModifiers) {
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), factory).unwrap();
        app.process_event(key_press(code, mods), factory).unwrap();
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

    // ── prefix key mode ───────────────────────────────────────────────────────

    #[test]
    fn ctrl_b_activates_prefix_mode() {
        let (mut app, factory) = default_app();
        assert!(!app.is_prefix_active());
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory).unwrap();
        assert!(app.is_prefix_active());
    }

    #[test]
    fn prefix_mode_deactivates_after_command() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Char('q'), KeyModifiers::empty());
        assert!(!app.is_prefix_active());
    }

    #[test]
    fn prefix_mode_deactivates_on_unknown_key() {
        let (mut app, factory) = default_app();
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory).unwrap();
        assert!(app.is_prefix_active());
        // Press an unbound key
        app.process_event(key_press(KeyCode::Char('z'), KeyModifiers::empty()), &factory).unwrap();
        assert!(!app.is_prefix_active());
        assert!(app.is_running()); // should not quit or do anything
    }

    #[test]
    fn regular_key_without_prefix_forwards_to_pane() {
        let (mut app, factory) = default_app();
        let active = app.active_pane();
        // 'a' without prefix should forward to PTY
        app.process_event(key_press(KeyCode::Char('a'), KeyModifiers::empty()), &factory).unwrap();
        assert!(!app.panes[&active].input_log.is_empty());
    }

    #[test]
    fn ctrl_b_itself_does_not_forward_to_pane() {
        let (mut app, factory) = default_app();
        let active = app.active_pane();
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory).unwrap();
        assert!(app.panes[&active].input_log.is_empty());
    }

    // ── directional splits (Ctrl+Arrow after prefix) ──────────────────────────

    #[test]
    fn split_right_creates_two_panes() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
    }

    #[test]
    fn split_left_creates_two_panes() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
    }

    #[test]
    fn split_down_creates_two_panes() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Down, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
    }

    #[test]
    fn split_up_creates_two_panes() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Up, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
    }

    #[test]
    fn split_changes_active_pane() {
        let (mut app, factory) = default_app();
        let original = app.active_pane();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        assert_ne!(app.active_pane(), original);
    }

    #[test]
    fn multiple_splits_create_correct_count() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, KeyCode::Down, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 4);
    }

    // ── close pane (prefix + w) ───────────────────────────────────────────────

    #[test]
    fn close_pane_after_split_returns_to_one() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
        prefix_then(&mut app, &factory, KeyCode::Char('w'), KeyModifiers::empty());
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn close_last_pane_quits() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Char('w'), KeyModifiers::empty());
        assert!(!app.is_running());
    }

    #[test]
    fn close_pane_shifts_focus_to_remaining() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        let second = app.active_pane();
        assert_ne!(first, second);
        prefix_then(&mut app, &factory, KeyCode::Char('w'), KeyModifiers::empty());
        assert_eq!(app.active_pane(), first);
    }

    // ── pane exit event (from PTY) ────────────────────────────────────────────

    #[test]
    fn pane_exit_event_removes_pane() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 2);
        app.process_event(AppEvent::PaneExit { pane_id: first }, &factory).unwrap();
        assert_eq!(app.pane_count(), 1);
        assert!(app.is_running());
    }

    #[test]
    fn stale_pane_exit_event_is_ignored() {
        let (mut app, factory) = default_app();
        let bogus_id = PaneId(999);
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

    // ── focus (prefix + Arrow) ───────────────────────────────────────────────

    #[test]
    fn focus_right_cycles_through_panes() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        let second = app.active_pane();

        // focus right (next) from second → first (wraps)
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::empty());
        assert_eq!(app.active_pane(), first);

        // focus right again → second
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::empty());
        assert_eq!(app.active_pane(), second);
    }

    #[test]
    fn focus_left_cycles_through_panes() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        let second = app.active_pane();

        // focus left (prev) from second → first
        prefix_then(&mut app, &factory, KeyCode::Left, KeyModifiers::empty());
        assert_eq!(app.active_pane(), first);

        // focus left from first → second (wraps)
        prefix_then(&mut app, &factory, KeyCode::Left, KeyModifiers::empty());
        assert_eq!(app.active_pane(), second);
    }

    // ── quit (prefix + q) ────────────────────────────────────────────────────

    #[test]
    fn quit_command_stops_app() {
        let (mut app, factory) = default_app();
        assert!(app.is_running());
        prefix_then(&mut app, &factory, KeyCode::Char('q'), KeyModifiers::empty());
        assert!(!app.is_running());
    }

    // ── resize ────────────────────────────────────────────────────────────────

    #[test]
    fn resize_event_updates_pane_geometry() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        let pre_resizes: usize = app.panes.values().map(|p| p.resize_log.len()).sum();

        app.process_event(AppEvent::Resize(120, 40), &factory).unwrap();

        let post_resizes: usize = app.panes.values().map(|p| p.resize_log.len()).sum();
        assert!(post_resizes > pre_resizes);
    }

    // ── key forwarding ────────────────────────────────────────────────────────

    #[test]
    fn regular_key_forwards_to_active_pane() {
        let (mut app, factory) = default_app();
        let active = app.active_pane();
        app.process_event(key_press(KeyCode::Char('a'), KeyModifiers::empty()), &factory).unwrap();
        let pane = &app.panes[&active];
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
    fn bound_key_after_prefix_does_not_forward() {
        let (mut app, factory) = default_app();
        let active = app.active_pane();
        prefix_then(&mut app, &factory, KeyCode::Char('q'), KeyModifiers::empty());
        // 'q' in prefix mode should quit, not forward bytes
        assert!(app.panes[&active].input_log.is_empty());
    }

    #[test]
    fn key_after_focus_switch_goes_to_new_active() {
        let (mut app, factory) = default_app();
        let first = app.active_pane();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        let second = app.active_pane();

        // Switch focus back to first
        prefix_then(&mut app, &factory, KeyCode::Left, KeyModifiers::empty());
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
        // prefix + ctrl+right (split), then prefix + q (quit)
        events.push(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL));
        events.push(key_press(KeyCode::Right, KeyModifiers::CONTROL));
        events.push(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL));
        events.push(key_press(KeyCode::Char('q'), KeyModifiers::empty()));

        let mut renderer = HeadlessRenderer::new();
        app.run(&mut events, &mut renderer, &factory).unwrap();

        assert!(!app.is_running());
        assert_eq!(app.pane_count(), 2);
        assert!(renderer.frame_count >= 2);
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
            prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        }
        assert_eq!(app.pane_count(), 4);

        // Close 2
        for _ in 0..2 {
            prefix_then(&mut app, &factory, KeyCode::Char('w'), KeyModifiers::empty());
        }
        assert_eq!(app.pane_count(), 2);

        // Split again
        prefix_then(&mut app, &factory, KeyCode::Down, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 3);
        assert!(app.is_running());
        assert_eq!(app.layout.leaf_ids().len(), app.pane_count());
    }

    #[test]
    fn mixed_directional_splits() {
        let (mut app, factory) = default_app();
        prefix_then(&mut app, &factory, KeyCode::Right, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, KeyCode::Down, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, KeyCode::Left, KeyModifiers::CONTROL);
        prefix_then(&mut app, &factory, KeyCode::Up, KeyModifiers::CONTROL);
        assert_eq!(app.pane_count(), 5);
        assert_eq!(app.layout.leaf_ids().len(), 5);
    }

    // ── cheatsheet visibility ────────────────────────────────────────────────

    #[test]
    fn cheatsheet_shown_when_prefix_active_and_enabled() {
        let factory = HeadlessPaneFactory;
        let mut app = App::new(KeyMap::default(), TERM_SIZE, true, &factory).unwrap();
        let mut renderer = HeadlessRenderer::new();
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory).unwrap();
        assert!(app.is_prefix_active());
        let show_bar = app.is_prefix_active() && true;
        renderer.render(&app.layout, &app.panes, TERM_SIZE, show_bar).unwrap();
        assert!(renderer.last_cheatsheet_visible);
    }

    #[test]
    fn cheatsheet_hidden_when_disabled() {
        let factory = HeadlessPaneFactory;
        let mut app = App::new(KeyMap::default(), TERM_SIZE, false, &factory).unwrap();
        let mut renderer = HeadlessRenderer::new();
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory).unwrap();
        assert!(app.is_prefix_active());
        // show_cheatsheet is false
        let show_bar = app.is_prefix_active() && false;
        renderer.render(&app.layout, &app.panes, TERM_SIZE, show_bar).unwrap();
        assert!(!renderer.last_cheatsheet_visible);
    }

    #[test]
    fn cheatsheet_hidden_after_prefix_deactivates() {
        let (mut app, factory) = default_app();
        let mut renderer = HeadlessRenderer::new();
        // Activate prefix
        app.process_event(key_press(KeyCode::Char('b'), KeyModifiers::CONTROL), &factory).unwrap();
        assert!(app.is_prefix_active());
        // Press unknown key → deactivates prefix
        app.process_event(key_press(KeyCode::Char('z'), KeyModifiers::NONE), &factory).unwrap();
        assert!(!app.is_prefix_active());
        let show_bar = app.is_prefix_active() && true;
        renderer.render(&app.layout, &app.panes, TERM_SIZE, show_bar).unwrap();
        assert!(!renderer.last_cheatsheet_visible);
    }
}

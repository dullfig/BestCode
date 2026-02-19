//! Key binding dispatch for the TUI.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::TuiApp;

/// Handle a key event, mutating app state.
pub fn handle_key(app: &mut TuiApp, key: KeyEvent) {
    // Global bindings
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
            return;
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
            return;
        }
        KeyCode::Tab => {
            app.focus = app.focus.next();
            return;
        }
        _ => {}
    }

    // Pane-specific bindings
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.move_down(),
        KeyCode::Char('k') | KeyCode::Up => app.move_up(),
        _ => {}
    }
}

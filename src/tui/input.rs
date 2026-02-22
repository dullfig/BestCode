//! Key binding dispatch for the TUI.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{AgentStatus, TuiApp};

/// Handle a key event, mutating app state.
pub fn handle_key(app: &mut TuiApp, key: KeyEvent) {
    // Ctrl+C always quits
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }

    match key.code {
        // Submit task
        KeyCode::Enter => {
            if !app.input_text.is_empty() {
                let text = app.input_text.clone();
                app.chat_log.push(super::app::ChatEntry {
                    role: "user".into(),
                    text: text.clone(),
                });
                app.input_text.clear();
                app.agent_status = AgentStatus::Thinking;
                app.message_auto_scroll = true;
                app.pending_task = Some(text);
            }
        }
        // Clear input
        KeyCode::Esc => {
            app.input_text.clear();
        }
        // Scroll messages
        KeyCode::Up => app.scroll_messages_up(),
        KeyCode::Down => app.scroll_messages_down(),
        KeyCode::PageUp => {
            for _ in 0..10 {
                app.scroll_messages_up();
            }
        }
        KeyCode::PageDown => {
            for _ in 0..10 {
                app.scroll_messages_down();
            }
        }
        // Edit input
        KeyCode::Backspace => {
            app.input_text.pop();
        }
        // Type into input
        KeyCode::Char(c) => {
            app.input_text.push(c);
        }
        _ => {}
    }
}

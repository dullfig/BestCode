//! TuiApp — the TEA model.
//!
//! All state lives here. Update receives TuiMessages, mutates state.
//! View reads state to produce ratatui widgets. No side effects in view.

use crate::kernel::context_store::{ContextInventory, SegmentMeta, SegmentStatus};
use crate::kernel::journal::JournalEntry;
use crate::kernel::thread_table::ThreadRecord;
use crate::pipeline::events::PipelineEvent;

use super::event::TuiMessage;

/// Which pane has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Threads,
    Messages,
    Context,
}

impl FocusedPane {
    /// Cycle to the next pane.
    pub fn next(self) -> Self {
        match self {
            Self::Threads => Self::Messages,
            Self::Messages => Self::Context,
            Self::Context => Self::Threads,
        }
    }
}

/// Lightweight view of a thread record (no kernel lifetime).
#[derive(Debug, Clone)]
pub struct ThreadView {
    pub uuid: String,
    pub chain: String,
    pub profile: String,
    pub created_at: u64,
}

impl From<&ThreadRecord> for ThreadView {
    fn from(r: &ThreadRecord) -> Self {
        Self {
            uuid: r.uuid.clone(),
            chain: r.chain.clone(),
            profile: r.profile.clone(),
            created_at: r.created_at,
        }
    }
}

/// Lightweight view of a message/journal entry.
#[derive(Debug, Clone)]
pub struct MessageView {
    pub message_id: String,
    pub from: String,
    pub to: String,
    pub status: String,
    pub thread_id: String,
}

impl From<&JournalEntry> for MessageView {
    fn from(e: &JournalEntry) -> Self {
        let status = format!("{:?}", e.status);
        Self {
            message_id: e.message_id.clone(),
            from: e.from.clone(),
            to: e.to.clone(),
            status,
            thread_id: e.thread_id.clone(),
        }
    }
}

/// Lightweight view of a segment.
#[derive(Debug, Clone)]
pub struct SegmentView {
    pub id: String,
    pub tag: String,
    pub size: usize,
    pub status: SegmentStatus,
    pub relevance: f32,
}

impl From<&SegmentMeta> for SegmentView {
    fn from(m: &SegmentMeta) -> Self {
        Self {
            id: m.id.clone(),
            tag: m.tag.clone(),
            size: m.size,
            status: m.status,
            relevance: m.relevance,
        }
    }
}

/// Lightweight view of a thread's context.
#[derive(Debug, Clone)]
pub struct ContextView {
    pub thread_id: String,
    pub segments: Vec<SegmentView>,
    pub active_count: usize,
    pub shelved_count: usize,
    pub total_bytes: usize,
    pub active_bytes: usize,
}

impl From<&ContextInventory> for ContextView {
    fn from(inv: &ContextInventory) -> Self {
        Self {
            thread_id: inv.thread_id.clone(),
            segments: inv.segments.iter().map(SegmentView::from).collect(),
            active_count: inv.active_count,
            shelved_count: inv.shelved_count,
            total_bytes: inv.total_bytes,
            active_bytes: inv.active_bytes,
        }
    }
}

/// The main TUI application state (TEA model).
pub struct TuiApp {
    /// Which pane has keyboard focus.
    pub focus: FocusedPane,
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Currently selected thread index.
    pub selected_thread: usize,
    /// Scroll offset for the messages pane.
    pub message_scroll: u16,
    /// Recent pipeline events (ring buffer).
    pub event_log: Vec<PipelineEvent>,
    /// Thread list (refreshed from kernel on tick).
    pub threads: Vec<ThreadView>,
    /// Messages/journal entries for the selected thread.
    pub messages: Vec<MessageView>,
    /// Context for the selected thread.
    pub context: Option<ContextView>,
    /// Total input tokens across all API calls.
    pub total_input_tokens: u64,
    /// Total output tokens across all API calls.
    pub total_output_tokens: u64,
}

/// Maximum number of events in the ring buffer.
const EVENT_LOG_CAPACITY: usize = 256;

impl TuiApp {
    /// Create a new TuiApp with default state.
    pub fn new() -> Self {
        Self {
            focus: FocusedPane::Threads,
            should_quit: false,
            selected_thread: 0,
            message_scroll: 0,
            event_log: Vec::new(),
            threads: Vec::new(),
            messages: Vec::new(),
            context: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    }

    /// Handle a TUI message (TEA update).
    pub fn update(&mut self, msg: TuiMessage) {
        match msg {
            TuiMessage::Input(key) => {
                super::input::handle_key(self, key);
            }
            TuiMessage::Pipeline(event) => {
                self.handle_pipeline_event(event);
            }
            TuiMessage::Tick => {
                // Kernel refresh handled externally by runner
            }
            TuiMessage::Render => {
                // Render handled externally by runner
            }
            TuiMessage::Quit => {
                self.should_quit = true;
            }
        }
    }

    /// Handle a pipeline event.
    fn handle_pipeline_event(&mut self, event: PipelineEvent) {
        if let PipelineEvent::TokenUsage {
            input_tokens,
            output_tokens,
            ..
        } = &event
        {
            self.total_input_tokens += *input_tokens as u64;
            self.total_output_tokens += *output_tokens as u64;
        }

        self.event_log.push(event);
        if self.event_log.len() > EVENT_LOG_CAPACITY {
            self.event_log.remove(0);
        }
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected_thread > 0 {
            self.selected_thread -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let max = if self.threads.is_empty() {
            0
        } else {
            self.threads.len() - 1
        };
        if self.selected_thread < max {
            self.selected_thread += 1;
        }
    }
}

impl Default for TuiApp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::context_store::{ContextInventory, SegmentMeta, SegmentStatus};
    use crate::kernel::thread_table::ThreadRecord;
    use crate::pipeline::events::PipelineEvent;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn app_default_state() {
        let app = TuiApp::new();
        assert_eq!(app.focus, FocusedPane::Threads);
        assert!(!app.should_quit);
        assert_eq!(app.selected_thread, 0);
    }

    #[test]
    fn app_cycle_focus() {
        let mut app = TuiApp::new();
        assert_eq!(app.focus, FocusedPane::Threads);

        let tab = TuiMessage::Input(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.update(tab.clone());
        assert_eq!(app.focus, FocusedPane::Messages);

        app.update(tab.clone());
        assert_eq!(app.focus, FocusedPane::Context);

        app.update(tab);
        assert_eq!(app.focus, FocusedPane::Threads);
    }

    #[test]
    fn app_quit_on_q() {
        let mut app = TuiApp::new();
        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        assert!(app.should_quit);
    }

    #[test]
    fn app_quit_on_ctrl_c() {
        let mut app = TuiApp::new();
        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(app.should_quit);
    }

    #[test]
    fn app_move_up_down() {
        let mut app = TuiApp::new();
        app.threads = vec![
            ThreadView {
                uuid: "a".into(),
                chain: "system.org".into(),
                profile: "admin".into(),
                created_at: 0,
            },
            ThreadView {
                uuid: "b".into(),
                chain: "system.org.handler".into(),
                profile: "admin".into(),
                created_at: 0,
            },
        ];

        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.selected_thread, 1);

        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.selected_thread, 0);
    }

    #[test]
    fn app_move_clamped() {
        let mut app = TuiApp::new();
        // No threads — moving shouldn't panic or underflow
        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.selected_thread, 0);

        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.selected_thread, 0);

        // One thread — can't go past it
        app.threads = vec![ThreadView {
            uuid: "a".into(),
            chain: "c".into(),
            profile: "p".into(),
            created_at: 0,
        }];
        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.selected_thread, 0);
    }

    #[test]
    fn tui_message_variants() {
        // Just verify all variants construct without panic
        let _input = TuiMessage::Input(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        let _pipeline = TuiMessage::Pipeline(PipelineEvent::MessageInjected {
            thread_id: "t".into(),
            target: "e".into(),
            profile: "a".into(),
        });
        let _tick = TuiMessage::Tick;
        let _render = TuiMessage::Render;
        let _quit = TuiMessage::Quit;
    }

    #[test]
    fn app_pipeline_event_logged() {
        let mut app = TuiApp::new();
        app.update(TuiMessage::Pipeline(PipelineEvent::MessageInjected {
            thread_id: "t1".into(),
            target: "echo".into(),
            profile: "admin".into(),
        }));
        assert_eq!(app.event_log.len(), 1);
    }

    #[test]
    fn app_token_usage_accumulates() {
        let mut app = TuiApp::new();
        app.update(TuiMessage::Pipeline(PipelineEvent::TokenUsage {
            thread_id: "t1".into(),
            input_tokens: 100,
            output_tokens: 50,
        }));
        app.update(TuiMessage::Pipeline(PipelineEvent::TokenUsage {
            thread_id: "t2".into(),
            input_tokens: 200,
            output_tokens: 100,
        }));
        assert_eq!(app.total_input_tokens, 300);
        assert_eq!(app.total_output_tokens, 150);
    }

    #[test]
    fn thread_view_from_record() {
        let record = ThreadRecord {
            uuid: "uuid-1".into(),
            chain: "system.org".into(),
            profile: "admin".into(),
            created_at: 12345,
        };
        let view = ThreadView::from(&record);
        assert_eq!(view.uuid, "uuid-1");
        assert_eq!(view.chain, "system.org");
        assert_eq!(view.profile, "admin");
        assert_eq!(view.created_at, 12345);
    }

    #[test]
    fn context_view_from_inventory() {
        let inv = ContextInventory {
            thread_id: "t1".into(),
            segments: vec![
                SegmentMeta {
                    id: "s1".into(),
                    tag: "code".into(),
                    size: 100,
                    status: SegmentStatus::Active,
                    relevance: 0.9,
                    created_at: 0,
                },
                SegmentMeta {
                    id: "s2".into(),
                    tag: "msg".into(),
                    size: 50,
                    status: SegmentStatus::Shelved,
                    relevance: 0.3,
                    created_at: 0,
                },
            ],
            active_count: 1,
            shelved_count: 1,
            total_bytes: 150,
            active_bytes: 100,
        };
        let view = ContextView::from(&inv);
        assert_eq!(view.thread_id, "t1");
        assert_eq!(view.segments.len(), 2);
        assert_eq!(view.active_count, 1);
        assert_eq!(view.shelved_count, 1);
        assert_eq!(view.total_bytes, 150);
        assert_eq!(view.active_bytes, 100);
    }
}

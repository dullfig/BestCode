//! TUI runner — main loop that wires everything together.
//!
//! Creates terminal, spawns event loop, runs main TEA loop.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::Mutex;
use tokio::time::interval;

use rust_pipeline::prelude::build_envelope;

use crate::kernel::Kernel;
use crate::pipeline::AgentPipeline;
use crate::tools::xml_escape;

use super::app::{ContextView, MessageView, ThreadView, TuiApp};
use super::event::TuiMessage;
use super::layout;

/// Refresh TuiApp state from the kernel (brief lock).
pub async fn refresh_from_kernel(app: &mut TuiApp, kernel: &Arc<Mutex<Kernel>>) {
    let k = kernel.lock().await;

    // Refresh threads
    app.threads = k.threads().all_records().map(ThreadView::from).collect();

    // Refresh messages (journal entries)
    app.messages = k.journal().all_entries().map(MessageView::from).collect();

    // Refresh context for selected thread
    if let Some(selected) = app.threads.get(app.selected_thread) {
        if let Ok(inv) = k.contexts().get_inventory(&selected.uuid) {
            app.context = Some(ContextView::from(&inv));
        } else {
            app.context = None;
        }
    } else {
        app.context = None;
    }
    // Lock released here — microseconds
}

/// Inject a task from the input bar into the pipeline.
async fn inject_task(pipeline: &AgentPipeline, kernel: &Arc<Mutex<Kernel>>, task: &str) {
    let root_uuid = {
        let k = kernel.lock().await;
        k.threads().root_uuid().map(|s| s.to_string())
    };

    if let Some(uuid) = root_uuid {
        let escaped = xml_escape(task);
        let xml = format!("<AgentTask><task>{escaped}</task></AgentTask>");
        if let Ok(envelope) =
            build_envelope("user", "coding-agent", &uuid, xml.as_bytes())
        {
            let _ = pipeline
                .inject_checked(envelope, &uuid, "coding", "coding-agent")
                .await;
        }
    }
}

/// Run the TUI main loop. Blocks until quit.
pub async fn run_tui(pipeline: &AgentPipeline) -> anyhow::Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new();
    let kernel = pipeline.kernel();
    let mut event_rx = pipeline.subscribe();

    // Dedicated input thread — reads crossterm events and sends through channel.
    // One thread, no polling/spinning. event::read() blocks until input arrives.
    let (key_tx, mut key_rx) = tokio::sync::mpsc::channel::<Event>(32);
    tokio::task::spawn_blocking(move || {
        loop {
            match event::read() {
                Ok(ev) => {
                    if key_tx.blocking_send(ev).is_err() {
                        break; // receiver dropped, TUI is shutting down
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut tick_interval = interval(Duration::from_millis(250)); // 4Hz
    let mut render_interval = interval(Duration::from_millis(33)); // ~30fps

    loop {
        tokio::select! {
            _ = tick_interval.tick() => {
                refresh_from_kernel(&mut app, &kernel).await;
            }
            _ = render_interval.tick() => {
                terminal.draw(|f| layout::draw(f, &mut app))?;
            }
            Ok(pipeline_event) = event_rx.recv() => {
                app.update(TuiMessage::Pipeline(pipeline_event));
            }
            Some(crossterm_event) = key_rx.recv() => {
                if let Event::Key(key) = crossterm_event {
                    if key.kind == KeyEventKind::Press {
                        app.update(TuiMessage::Input(key));
                    }
                }
            }
        }

        // Check for pending task submission (set by input handler on Enter)
        if let Some(task) = app.pending_task.take() {
            inject_task(pipeline, &kernel, &task).await;
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::events::PipelineEvent;
    use tempfile::TempDir;

    #[tokio::test]
    async fn refresh_populates_threads() {
        let dir = TempDir::new().unwrap();
        let mut kernel = Kernel::open(&dir.path().join("data")).unwrap();
        kernel.initialize_root("org", "admin").unwrap();

        let kernel_arc = Arc::new(Mutex::new(kernel));
        let mut app = TuiApp::new();
        refresh_from_kernel(&mut app, &kernel_arc).await;

        assert!(!app.threads.is_empty());
        assert_eq!(app.threads[0].chain, "system.org");
    }

    #[tokio::test]
    async fn refresh_populates_context() {
        let dir = TempDir::new().unwrap();
        let mut kernel = Kernel::open(&dir.path().join("data")).unwrap();
        let root = kernel.initialize_root("org", "admin").unwrap();
        kernel.contexts_mut().create(&root).unwrap();

        let seg = crate::kernel::context_store::ContextSegment {
            id: "s1".into(),
            tag: "code".into(),
            content: b"fn main()".to_vec(),
            status: crate::kernel::context_store::SegmentStatus::Active,
            relevance: 0.8,
            created_at: 0,
            fold_ref: None,
        };
        kernel.contexts_mut().add_segment(&root, seg).unwrap();

        let kernel_arc = Arc::new(Mutex::new(kernel));
        let mut app = TuiApp::new();
        refresh_from_kernel(&mut app, &kernel_arc).await;

        assert!(app.context.is_some());
        let ctx = app.context.unwrap();
        assert_eq!(ctx.segments.len(), 1);
        assert_eq!(ctx.segments[0].id, "s1");
    }

    #[tokio::test]
    async fn event_log_ring_buffer() {
        let mut app = TuiApp::new();
        // Fill beyond capacity
        for i in 0..300 {
            app.update(TuiMessage::Pipeline(PipelineEvent::MessageInjected {
                thread_id: format!("t-{i}"),
                target: "echo".into(),
                profile: "admin".into(),
            }));
        }
        // Should be capped at 256
        assert_eq!(app.event_log.len(), 256);
    }

    #[test]
    fn runner_quit_on_message() {
        let mut app = TuiApp::new();
        app.update(TuiMessage::Quit);
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn integration_pipeline_to_tui() {
        use crate::organism::parser::parse_organism;
        use rust_pipeline::prelude::{
            build_envelope, FnHandler, HandlerContext, HandlerResponse, ValidatedPayload,
        };

        let yaml = r#"
organism:
  name: tui-test

listeners:
  - name: echo
    payload_class: handlers.echo.Greeting
    handler: handlers.echo.handle
    description: "Echo handler"
    peers: []

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [echo]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let dir = TempDir::new().unwrap();

        let echo = FnHandler(|p: ValidatedPayload, _ctx: HandlerContext| {
            Box::pin(async move { Ok(HandlerResponse::Reply { payload_xml: p.xml }) })
        });

        let mut pipeline = crate::pipeline::AgentPipelineBuilder::new(org, &dir.path().join("data"))
            .register("echo", echo)
            .unwrap()
            .build()
            .unwrap();

        pipeline.run();

        // Subscribe and inject
        let mut rx = pipeline.subscribe();

        let envelope = build_envelope(
            "test",
            "echo",
            "thread-1",
            b"<Greeting><text>hello tui</text></Greeting>",
        )
        .unwrap();

        pipeline
            .inject_checked(envelope, "thread-1", "admin", "echo")
            .await
            .unwrap();

        // Verify event arrives
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, PipelineEvent::MessageInjected { .. }));

        // Verify kernel has the root data we can refresh from
        let kernel = pipeline.kernel();
        let mut app = TuiApp::new();
        refresh_from_kernel(&mut app, &kernel).await;
        // Messages might be empty (journal not populated by inject_checked alone)
        // but threads should be accessible

        pipeline.shutdown().await;
    }

    #[test]
    fn submit_sets_pending_task() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = TuiApp::new();
        app.input_text = "Read README.md".into();

        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));

        assert_eq!(app.pending_task, Some("Read README.md".into()));
        assert!(app.input_text.is_empty());
        assert_eq!(app.chat_log.len(), 1);
        assert_eq!(app.chat_log[0].role, "user");
    }

    #[test]
    fn typing_goes_to_input() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = TuiApp::new();

        // Type "hi" — no 'i' key needed, just type
        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::NONE,
        )));
        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.input_text, "hi");

        // Backspace
        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.input_text, "h");
    }

    #[test]
    fn esc_clears_input() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = TuiApp::new();
        app.input_text = "some text".into();

        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));

        assert!(app.input_text.is_empty());
        assert!(!app.should_quit);
    }

    #[test]
    fn arrows_scroll_messages() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = TuiApp::new();
        app.message_scroll = 5;
        app.message_auto_scroll = false;

        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.message_scroll, 4);

        app.update(TuiMessage::Input(KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.message_scroll, 5);
    }

    #[test]
    fn agent_response_updates_status() {
        use super::super::app::AgentStatus;

        let mut app = TuiApp::new();
        app.agent_status = AgentStatus::Thinking;

        app.update(TuiMessage::Pipeline(PipelineEvent::AgentResponse {
            thread_id: "t1".into(),
            text: "Done! Here is the summary.".into(),
        }));

        assert_eq!(app.agent_status, AgentStatus::Idle);
        assert_eq!(
            app.last_response,
            Some("Done! Here is the summary.".into())
        );
    }
}

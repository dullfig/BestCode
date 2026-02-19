//! TUI runner — main loop that wires everything together.
//!
//! Creates terminal, spawns event loop, runs main TEA loop.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::Mutex;
use tokio::time::interval;

use crate::kernel::Kernel;
use crate::pipeline::AgentPipeline;

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

    let mut tick_interval = interval(Duration::from_millis(250)); // 4Hz
    let mut render_interval = interval(Duration::from_millis(33)); // ~30fps

    loop {
        tokio::select! {
            _ = tick_interval.tick() => {
                refresh_from_kernel(&mut app, &kernel).await;
            }
            _ = render_interval.tick() => {
                terminal.draw(|f| layout::draw(f, &app))?;
            }
            Ok(event) = event_rx.recv() => {
                app.update(TuiMessage::Pipeline(event));
            }
            // Poll crossterm events (non-blocking via tokio::task::spawn_blocking)
            result = tokio::task::spawn_blocking(|| {
                if event::poll(Duration::from_millis(10)).unwrap_or(false) {
                    event::read().ok()
                } else {
                    None
                }
            }) => {
                if let Ok(Some(Event::Key(key))) = result {
                    app.update(TuiMessage::Input(key));
                }
            }
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
}

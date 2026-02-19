//! Three-pane layout for the TUI.
//!
//! ```text
//! ┌────────────┬──────────────────────────┐
//! │  Threads   │     Messages             │
//! │  (25%)     │     (Fill)               │
//! │            ├──────────────────────────┤
//! │            │     Context (40%)        │
//! │            │     (foldable tree)      │
//! ├────────────┴──────────────────────────┤
//! │  Status: [Threads: 3] [Tokens: 12K]  │
//! └───────────────────────────────────────┘
//! ```

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use super::app::{FocusedPane, TuiApp};
use super::dashboard;

/// Draw the full TUI layout.
pub fn draw(f: &mut Frame, app: &TuiApp) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(f.area());

    let main_area = outer[0];
    let status_area = outer[1];

    // Main: left (threads) + right (messages/context)
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(main_area);

    let left = columns[0];
    let right = columns[1];

    // Right: messages (top 60%) + context (bottom 40%)
    let right_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(right);

    let messages_area = right_split[0];
    let context_area = right_split[1];

    draw_threads(f, app, left);
    draw_messages(f, app, messages_area);
    draw_context(f, app, context_area);
    draw_status(f, app, status_area);
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn draw_threads(f: &mut Frame, app: &TuiApp, area: Rect) {
    let focused = app.focus == FocusedPane::Threads;
    let block = Block::default()
        .title(" Threads ")
        .borders(Borders::ALL)
        .border_style(border_style(focused));

    let items: Vec<ListItem> = app
        .threads
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let style = if i == app.selected_thread {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let chain_short = t.chain.split('.').last().unwrap_or(&t.chain);
            ListItem::new(Line::from(vec![
                Span::styled(chain_short, style),
                Span::styled(format!(" [{}]", t.profile), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn draw_messages(f: &mut Frame, app: &TuiApp, area: Rect) {
    let focused = app.focus == FocusedPane::Messages;
    let block = Block::default()
        .title(" Messages ")
        .borders(Borders::ALL)
        .border_style(border_style(focused));

    let items: Vec<ListItem> = app
        .messages
        .iter()
        .map(|m| {
            let status_color = match m.status.as_str() {
                "Dispatched" => Color::Yellow,
                "Delivered" => Color::Green,
                "Failed" => Color::Red,
                _ => Color::White,
            };
            ListItem::new(Line::from(vec![
                Span::styled(&m.status, Style::default().fg(status_color)),
                Span::raw(" "),
                Span::raw(format!("{} → {}", m.from, m.to)),
            ]))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn draw_context(f: &mut Frame, app: &TuiApp, area: Rect) {
    let focused = app.focus == FocusedPane::Context;
    let block = Block::default()
        .title(" Context ")
        .borders(Borders::ALL)
        .border_style(border_style(focused));

    if let Some(ctx) = &app.context {
        let lines: Vec<Line> = ctx
            .segments
            .iter()
            .map(|s| {
                let status_char = match s.status {
                    crate::kernel::context_store::SegmentStatus::Active => "A",
                    crate::kernel::context_store::SegmentStatus::Shelved => "S",
                };
                Line::from(vec![
                    Span::styled(
                        format!("[{status_char}] "),
                        Style::default().fg(if status_char == "A" {
                            Color::Green
                        } else {
                            Color::DarkGray
                        }),
                    ),
                    Span::raw(format!("{}:{}", s.tag, s.id)),
                    Span::styled(
                        format!(" {:.0}%", s.relevance * 100.0),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!(" {}", dashboard::format_bytes(s.size)),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            })
            .collect();
        let para = Paragraph::new(lines).block(block);
        f.render_widget(para, area);
    } else {
        let para = Paragraph::new("No context selected").block(block);
        f.render_widget(para, area);
    }
}

fn draw_status(f: &mut Frame, app: &TuiApp, area: Rect) {
    let focus_name = match app.focus {
        FocusedPane::Threads => "Threads",
        FocusedPane::Messages => "Messages",
        FocusedPane::Context => "Context",
    };
    let status = Line::from(vec![
        Span::styled(
            format!(" [Threads: {}]", app.threads.len()),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  "),
        Span::styled(
            format!("[Journal: {}]", app.messages.len()),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "[Tokens: {}/{}]",
                dashboard::format_tokens(app.total_input_tokens),
                dashboard::format_tokens(app.total_output_tokens),
            ),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw("  "),
        Span::styled(
            format!("[Focus: {focus_name}]"),
            Style::default().fg(Color::Green),
        ),
        Span::raw("  "),
        Span::styled(
            "q:Quit  Tab:Focus  j/k:Move",
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let para = Paragraph::new(status);
    f.render_widget(para, area);
}

//! Three-pane layout with input bar for the TUI.
//!
//! ```text
//! ┌────────────┬──────────────────────────┐
//! │  Threads   │     Messages             │
//! │  (25%)     │     (Fill)               │
//! │            ├──────────────────────────┤
//! │            │     Context (40%)        │
//! │            │     (foldable tree)      │
//! ├────────────┴──────────────────────────┤
//! │ > Type a task and press Enter...       │
//! ├────────────────────────────────────────┤
//! │ [Status: idle] [Tokens: 12K/3K] [i:Input Tab:Focus q:Quit] │
//! └────────────────────────────────────────┘
//! ```

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Wrap,
};
use ratatui::Frame;

use super::app::{AgentStatus, FocusedPane, TuiApp};
use super::dashboard;

/// Draw the full TUI layout.
pub fn draw(f: &mut Frame, app: &mut TuiApp) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),     // main area
            Constraint::Length(3),  // input bar
            Constraint::Length(1),  // status bar
        ])
        .split(f.area());

    let main_area = outer[0];
    let input_area = outer[1];
    let status_area = outer[2];

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
    draw_input(f, app, input_area);
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

fn draw_messages(f: &mut Frame, app: &mut TuiApp, area: Rect) {
    let focused = app.focus == FocusedPane::Messages;
    let block = Block::default()
        .title(" Messages ")
        .borders(Borders::ALL)
        .border_style(border_style(focused));

    let mut lines: Vec<Line> = Vec::new();

    for entry in &app.chat_log {
        match entry.role.as_str() {
            "user" => {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(
                        "[You] ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(&entry.text),
                ]));
            }
            "agent" => {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![Span::styled(
                    "[Agent]",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )]));
                for text_line in entry.text.lines() {
                    lines.push(Line::from(text_line.to_string()));
                }
            }
            _ => {}
        }
    }

    // Show thinking indicator when waiting
    if app.agent_status == AgentStatus::Thinking {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "thinking...",
            Style::default().fg(Color::Yellow),
        )]));
    } else if let AgentStatus::ToolCall(ref name) = app.agent_status {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            format!("using {name}..."),
            Style::default().fg(Color::Cyan),
        )]));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No messages yet. Press 'i' to type a task.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Clamp scroll so we never scroll past content.
    // Account for line wrapping: each line may occupy multiple visual rows.
    let inner_height = area.height.saturating_sub(2) as u16;
    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let total_lines: u16 = lines
        .iter()
        .map(|line| {
            let width: usize = line.spans.iter().map(|s| s.content.len()).sum();
            if width == 0 {
                1
            } else {
                ((width + inner_width - 1) / inner_width) as u16
            }
        })
        .sum();
    let max_scroll = total_lines.saturating_sub(inner_height);
    let scroll = if app.message_auto_scroll {
        max_scroll
    } else {
        app.message_scroll.min(max_scroll)
    };
    // Write clamped value back so up/down keys work immediately
    app.message_scroll = scroll;

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(para, area);

    // Scrollbar
    if total_lines > inner_height {
        let mut scrollbar_state = ScrollbarState::new(max_scroll as usize)
            .position(scroll as usize);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None),
            area,
            &mut scrollbar_state,
        );
    }
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
                    crate::kernel::context_store::SegmentStatus::Folded => "F",
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

fn draw_input(f: &mut Frame, app: &TuiApp, area: Rect) {
    let focused = app.focus == FocusedPane::Input;
    let block = Block::default()
        .title(" Task ")
        .borders(Borders::ALL)
        .border_style(border_style(focused));

    let display_text = if app.input_text.is_empty() {
        "\u{2588}".to_string() // block cursor
    } else {
        format!("{}\u{2588}", app.input_text) // text + cursor
    };

    let style = Style::default().fg(Color::White);

    let para = Paragraph::new(display_text).style(style).block(block);
    f.render_widget(para, area);
}

fn draw_status(f: &mut Frame, app: &TuiApp, area: Rect) {
    let status_text = match &app.agent_status {
        AgentStatus::Idle => Span::styled("idle", Style::default().fg(Color::Green)),
        AgentStatus::Thinking => Span::styled("thinking...", Style::default().fg(Color::Yellow)),
        AgentStatus::ToolCall(name) => Span::styled(
            format!("tool: {name}"),
            Style::default().fg(Color::Cyan),
        ),
        AgentStatus::Error(msg) => Span::styled(
            format!("error: {msg}"),
            Style::default().fg(Color::Red),
        ),
    };

    let focus_name = match app.focus {
        FocusedPane::Threads => "Threads",
        FocusedPane::Messages => "Messages",
        FocusedPane::Context => "Context",
        FocusedPane::Input => "Input",
    };

    let status = Line::from(vec![
        Span::styled(" [", Style::default().fg(Color::DarkGray)),
        status_text,
        Span::styled("]", Style::default().fg(Color::DarkGray)),
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
            format!("[Threads: {}]", app.threads.len()),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  "),
        Span::styled(
            format!("[{focus_name}]"),
            Style::default().fg(Color::Green),
        ),
        Span::raw("  "),
        Span::styled(
            "Enter:Send  \u{2191}\u{2193}:Scroll  PgUp/Dn:Fast  Esc:Clear  Ctrl+C:Quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let para = Paragraph::new(status);
    f.render_widget(para, area);
}

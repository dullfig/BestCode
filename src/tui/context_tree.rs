//! Foldable context tree — tui-tree-widget integration.
//!
//! Renders context segments as a collapsible tree:
//! ```text
//! Thread: system.bestcode.coding-agent
//!  [v] Active (3 segments, 4.2 KB)
//!      message:msg-001  [||||    ] 0.95  "Read src/main.rs"
//!      code:src/main.rs [|||     ] 0.87  fn main() { ... }
//!  [>] Shelved (2 segments, 1.1 KB)   ← collapsed
//! ```

use ratatui::style::Color;
use tui_tree_widget::TreeItem;

use super::app::ContextView;
use super::dashboard;
use crate::kernel::context_store::SegmentStatus;

/// Build a tree from a ContextView.
pub fn build_context_tree<'a>(ctx: &ContextView) -> Vec<TreeItem<'a, String>> {
    let active_segs: Vec<_> = ctx
        .segments
        .iter()
        .filter(|s| s.status == SegmentStatus::Active)
        .collect();

    let shelved_segs: Vec<_> = ctx
        .segments
        .iter()
        .filter(|s| s.status == SegmentStatus::Shelved)
        .collect();

    let mut items = Vec::new();

    // Active group
    let active_label = format!(
        "Active ({} segments, {})",
        active_segs.len(),
        dashboard::format_bytes(ctx.active_bytes),
    );
    let active_children: Vec<TreeItem<'a, String>> = active_segs
        .iter()
        .map(|s| {
            let bar = relevance_bar(s.relevance);
            let label = format!(
                "{}:{}  [{bar}] {:.2}  {}",
                s.tag,
                s.id,
                s.relevance,
                dashboard::format_bytes(s.size),
            );
            TreeItem::new_leaf(format!("active-{}", s.id), label)
        })
        .collect();
    items.push(
        TreeItem::new(
            "active".to_string(),
            active_label,
            active_children,
        )
        .expect("tree item creation"),
    );

    // Shelved group
    let shelved_bytes = ctx.total_bytes.saturating_sub(ctx.active_bytes);
    let shelved_label = format!(
        "Shelved ({} segments, {})",
        shelved_segs.len(),
        dashboard::format_bytes(shelved_bytes),
    );
    let shelved_children: Vec<TreeItem<'a, String>> = shelved_segs
        .iter()
        .map(|s| {
            let bar = relevance_bar(s.relevance);
            let label = format!(
                "{}:{}  [{bar}] {:.2}  {}",
                s.tag,
                s.id,
                s.relevance,
                dashboard::format_bytes(s.size),
            );
            TreeItem::new_leaf(format!("shelved-{}", s.id), label)
        })
        .collect();
    items.push(
        TreeItem::new(
            "shelved".to_string(),
            shelved_label,
            shelved_children,
        )
        .expect("tree item creation"),
    );

    items
}

/// Create a relevance bar string.
/// Red (0-0.3), yellow (0.3-0.7), green (0.7-1.0).
pub fn relevance_bar(relevance: f32) -> String {
    let filled = (relevance * 8.0).round() as usize;
    let empty = 8usize.saturating_sub(filled);
    format!("{}{}", "|".repeat(filled), " ".repeat(empty))
}

/// Get the color for a relevance score.
pub fn relevance_color(relevance: f32) -> Color {
    if relevance >= 0.7 {
        Color::Green
    } else if relevance >= 0.3 {
        Color::Yellow
    } else {
        Color::Red
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ContextView, SegmentView};
    use crate::kernel::context_store::SegmentStatus;

    #[test]
    fn build_tree_empty() {
        let ctx = ContextView {
            thread_id: "t1".into(),
            segments: vec![],
            active_count: 0,
            shelved_count: 0,
            total_bytes: 0,
            active_bytes: 0,
        };
        let tree = build_context_tree(&ctx);
        assert_eq!(tree.len(), 2); // Active and Shelved groups always present
    }

    #[test]
    fn build_tree_groups() {
        let ctx = ContextView {
            thread_id: "t1".into(),
            segments: vec![
                SegmentView {
                    id: "s1".into(),
                    tag: "code".into(),
                    size: 100,
                    status: SegmentStatus::Active,
                    relevance: 0.9,
                },
                SegmentView {
                    id: "s2".into(),
                    tag: "msg".into(),
                    size: 50,
                    status: SegmentStatus::Shelved,
                    relevance: 0.3,
                },
            ],
            active_count: 1,
            shelved_count: 1,
            total_bytes: 150,
            active_bytes: 100,
        };
        let tree = build_context_tree(&ctx);
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn build_tree_metadata() {
        let ctx = ContextView {
            thread_id: "t1".into(),
            segments: vec![SegmentView {
                id: "code:main.rs".into(),
                tag: "code".into(),
                size: 2048,
                status: SegmentStatus::Active,
                relevance: 0.87,
            }],
            active_count: 1,
            shelved_count: 0,
            total_bytes: 2048,
            active_bytes: 2048,
        };
        let tree = build_context_tree(&ctx);
        // Active group exists with one child
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn relevance_bar_high() {
        let bar = relevance_bar(0.9);
        let filled = bar.chars().filter(|c| *c == '|').count();
        assert!(filled >= 7); // 0.9 * 8 = 7.2, rounds to 7
    }

    #[test]
    fn relevance_bar_medium() {
        let bar = relevance_bar(0.5);
        let filled = bar.chars().filter(|c| *c == '|').count();
        assert_eq!(filled, 4);
    }

    #[test]
    fn relevance_bar_low() {
        let bar = relevance_bar(0.1);
        let filled = bar.chars().filter(|c| *c == '|').count();
        assert_eq!(filled, 1);
    }
}

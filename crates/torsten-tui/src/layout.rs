//! Adaptive layout system for the Torsten TUI dashboard.
//!
//! The dashboard has five panels arranged in a fixed grid with a gap between
//! the last panel row and the footer.  Panels do NOT expand vertically to fill
//! the remaining space.
//!
//! Layout (standard, >= 80x28):
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────────────┐
//! │ Header (1 line)                                                            │
//! ├────────────────────────────────┬───────────────────────────────────────────┤
//! │ Node (fixed height)            │ Chain (fixed height)                      │
//! ├────────────────────────────────┼───────────────────────────────────────────┤
//! │ Connections (fixed height)     │ Resources (fixed height)                  │
//! ├────────────────────────────────┴───────────────────────────────────────────┤
//! │ Peers (fixed height)                                                       │
//! │                                                                            │
//! │ (gap)                                                                      │
//! ├────────────────────────────────────────────────────────────────────────────┤
//! │ Footer (1 line)                                                            │
//! └────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Compact layout (< 80 cols or < 28 rows): stacks all panels in a single
//! column; still maintains a gap above the footer.

use ratatui::layout::{Constraint, Layout, Rect};

/// Fixed panel heights (in lines, including borders).
///
/// These values are tuned so the five panels and header/footer fit comfortably
/// in an 80x28 terminal without expanding to fill the full height.
pub const PANEL_NODE_H: u16 = 7;
pub const PANEL_CHAIN_H: u16 = 11;
pub const PANEL_CONNECTIONS_H: u16 = 8;
pub const PANEL_RESOURCES_H: u16 = 6;
pub const PANEL_PEERS_H: u16 = 7;

/// Layout mode (auto-detected or overridden).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Two-column grid layout (>= 80 cols, >= 28 rows).
    Standard,
    /// Single-column stacked layout (< 80 cols or < 28 rows).
    Compact,
}

impl LayoutMode {
    /// Auto-detect layout mode from terminal dimensions.
    pub fn detect(width: u16, height: u16) -> Self {
        if width >= 80 && height >= 28 {
            LayoutMode::Standard
        } else {
            LayoutMode::Compact
        }
    }
}

/// Pre-computed layout rectangles for each dashboard panel.
pub struct DashboardLayout {
    /// Active layout mode (reserved for future use / tests).
    #[allow(dead_code)]
    pub mode: LayoutMode,
    /// Single-line header area.
    pub header: Rect,
    /// Node info panel area.
    pub node: Rect,
    /// Chain status panel area.
    pub chain: Rect,
    /// Connections panel area.
    pub connections: Rect,
    /// Resources panel area.
    pub resources: Rect,
    /// Peers panel area.
    pub peers: Rect,
    /// Footer (keyboard shortcuts) area.
    pub footer: Rect,
}

/// Compute the dashboard layout from the given terminal area.
///
/// When `override_mode` is `Some`, that mode is used regardless of terminal size.
pub fn compute_layout(area: Rect, override_mode: Option<LayoutMode>) -> DashboardLayout {
    let mode = override_mode.unwrap_or_else(|| LayoutMode::detect(area.width, area.height));
    match mode {
        LayoutMode::Standard => compute_standard_layout(area),
        LayoutMode::Compact => compute_compact_layout(area),
    }
}

/// Standard (two-column) layout.
///
/// Row heights are fixed; any remaining space becomes a gap above the footer.
fn compute_standard_layout(area: Rect) -> DashboardLayout {
    // The top row uses max(NODE_H, CHAIN_H) to keep both panels aligned.
    let top_h = PANEL_NODE_H.max(PANEL_CHAIN_H);
    // The middle row uses max(CONNECTIONS_H, RESOURCES_H).
    let mid_h = PANEL_CONNECTIONS_H.max(PANEL_RESOURCES_H);

    // Vertical split: header | top row | mid row | peers | gap | footer.
    // Use Min(0) for the gap so it absorbs any leftover space.
    let vertical = Layout::vertical([
        Constraint::Length(1),             // header
        Constraint::Length(top_h),         // row 1: Node + Chain
        Constraint::Length(mid_h),         // row 2: Connections + Resources
        Constraint::Length(PANEL_PEERS_H), // row 3: Peers
        Constraint::Min(0),                // gap (absorbs leftover)
        Constraint::Length(1),             // footer
    ])
    .split(area);

    let header = vertical[0];
    let row1 = vertical[1];
    let row2 = vertical[2];
    let peers = vertical[3];
    let footer = vertical[5];

    // Row 1: Node (38%) | Chain (62%)
    let row1_cols =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).split(row1);

    // Row 2: Connections (38%) | Resources (62%)
    let row2_cols =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).split(row2);

    DashboardLayout {
        mode: LayoutMode::Standard,
        header,
        node: row1_cols[0],
        chain: row1_cols[1],
        connections: row2_cols[0],
        resources: row2_cols[1],
        peers,
        footer,
    }
}

/// Compact (single-column) layout.
///
/// All panels are stacked vertically with fixed heights.  Any remaining space
/// becomes a gap above the footer.
fn compute_compact_layout(area: Rect) -> DashboardLayout {
    let vertical = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(PANEL_NODE_H),
        Constraint::Length(PANEL_CHAIN_H),
        Constraint::Length(PANEL_CONNECTIONS_H),
        Constraint::Length(PANEL_RESOURCES_H),
        Constraint::Length(PANEL_PEERS_H),
        Constraint::Min(0),    // gap
        Constraint::Length(1), // footer
    ])
    .split(area);

    DashboardLayout {
        mode: LayoutMode::Compact,
        header: vertical[0],
        node: vertical[1],
        chain: vertical[2],
        connections: vertical[3],
        resources: vertical[4],
        peers: vertical[5],
        footer: vertical[7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_mode_detect_standard() {
        assert_eq!(LayoutMode::detect(80, 28), LayoutMode::Standard);
        assert_eq!(LayoutMode::detect(200, 50), LayoutMode::Standard);
    }

    #[test]
    fn test_layout_mode_detect_compact() {
        assert_eq!(LayoutMode::detect(79, 28), LayoutMode::Compact);
        assert_eq!(LayoutMode::detect(80, 27), LayoutMode::Compact);
        assert_eq!(LayoutMode::detect(40, 15), LayoutMode::Compact);
    }

    #[test]
    fn test_standard_layout_non_zero_rects() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = compute_layout(area, Some(LayoutMode::Standard));
        assert!(layout.header.width > 0 && layout.header.height == 1);
        assert!(layout.node.width > 0 && layout.node.height > 0);
        assert!(layout.chain.width > 0 && layout.chain.height > 0);
        assert!(layout.connections.width > 0 && layout.connections.height > 0);
        assert!(layout.resources.width > 0 && layout.resources.height > 0);
        assert!(layout.peers.width > 0 && layout.peers.height > 0);
        assert_eq!(layout.footer.height, 1);
    }

    #[test]
    fn test_compact_layout_full_width() {
        let area = Rect::new(0, 0, 60, 60);
        let layout = compute_layout(area, Some(LayoutMode::Compact));
        assert_eq!(layout.node.width, area.width);
        assert_eq!(layout.chain.width, area.width);
        assert_eq!(layout.connections.width, area.width);
        assert_eq!(layout.resources.width, area.width);
        assert_eq!(layout.peers.width, area.width);
    }

    #[test]
    fn test_footer_is_one_line() {
        for mode in [LayoutMode::Standard, LayoutMode::Compact] {
            let area = Rect::new(0, 0, 120, 50);
            let layout = compute_layout(area, Some(mode));
            assert_eq!(layout.footer.height, 1, "footer must be 1 line in {mode:?}");
        }
    }

    #[test]
    fn test_panels_do_not_fill_space() {
        // With plenty of vertical space, the gap should absorb leftover rows.
        let area = Rect::new(0, 0, 120, 60);
        let layout = compute_layout(area, Some(LayoutMode::Standard));
        let top_h = PANEL_NODE_H.max(PANEL_CHAIN_H);
        let mid_h = PANEL_CONNECTIONS_H.max(PANEL_RESOURCES_H);
        let used = 1 + top_h + mid_h + PANEL_PEERS_H + 1; // header + rows + footer
                                                          // Total used < total area — there must be a gap.
        assert!(
            used < area.height,
            "gap should exist between panels and footer"
        );
        // Peers panel bottom edge should be well above the footer.
        let peers_bottom = layout.peers.y + layout.peers.height;
        assert!(
            peers_bottom < layout.footer.y,
            "peers bottom ({peers_bottom}) should be above footer ({})",
            layout.footer.y
        );
    }
}

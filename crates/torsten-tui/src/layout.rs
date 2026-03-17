//! Adaptive layout system for the Torsten TUI dashboard.
//!
//! Three layout modes auto-selected from terminal dimensions:
//!
//! **Wide** (>= 120 cols, >= 30 rows):
//! ```text
//! ┌─────────────────────────────────────────────────── Header (1 line) ───────────────────────────────────────────────────┐
//! ├────────── Node (38%) ──────────────┬──────────────────────────── Chain (62%) ────────────────────────────────────────┤
//! ├────── Connections (38%) ───────────┼──────────────────────────── Resources (62%) ──────────────────────────────────── ┤
//! ├─────────────────────────────────────────────── Peers (full width) ───────────────────────────────────────────────────┤
//! │ (gap)                                                                                                                  │
//! ├─────────────────────────────────────────────────── Footer (1 line) ──────────────────────────────────────────────────┘
//! ```
//!
//! **Standard** (>= 80 cols, >= 28 rows — same two-column grid, narrower):
//! ```text
//! ┌──────────────── Header (1 line) ──────────────────┐
//! ├── Node (38%) ──┬──────── Chain (62%) ──────────── ┤
//! ├─ Connections ──┼──────── Resources ─────────────── ┤
//! ├───────────────────── Peers (full width) ──────────┤
//! │ (gap)                                             │
//! ├──────────────── Footer (1 line) ──────────────────┘
//! ```
//!
//! The epoch progress bar appears **only** inside the Chain panel — it is NOT
//! duplicated in the header.  The header is a single line carrying the status
//! pill, epoch number, era, network, tip-diff indicator, and uptime.
//!
//! **Compact** (< 80 cols or < 28 rows — single-column stacked):
//! ```text
//! ┌── Header ──┐
//! ├── Node ────┤
//! ├── Chain ───┤
//! ├── Conn ────┤
//! ├── Resources┤
//! ├── Peers ───┤
//! │   (gap)    │
//! └── Footer ──┘
//! ```

use ratatui::layout::{Constraint, Layout, Rect};

// ---------------------------------------------------------------------------
// Fixed panel heights (lines including borders)
// ---------------------------------------------------------------------------

/// Node panel: Role + Network + Version + Era + Uptime + Peers + Forged = 7 content rows + sync bar (1) + 2 borders = 10
pub const PANEL_NODE_H: u16 = 10;
/// Chain panel: epoch bar (1) + 8 data rows + mempool gauge (1) + 2 borders = 12.
/// The epoch progress bar lives here (not in the header).
pub const PANEL_CHAIN_H: u16 = 12;
/// Connections panel: P2P + Inbound + Outbound + Cold/Warm/Hot + Uni/Bi/Duplex = 5 content rows + 2 borders = 7
pub const PANEL_CONNECTIONS_H: u16 = 7;
/// Resources panel: CPU + Mem live + Mem RSS + mem bar + sparkline = 5 content rows + 2 borders = 7
pub const PANEL_RESOURCES_H: u16 = 7;
/// Peers panel: RTT bar + 2 band rows + Low/Avg/High = 4 content rows + 2 borders = 6
pub const PANEL_PEERS_H: u16 = 6;
/// Header area height: 1 line (status pill + key metrics only; epoch bar is in Chain panel).
pub const HEADER_H: u16 = 1;

/// Header area height for the compact layout: 2 lines (status line + epoch progress bar).
/// The compact header uses the HeaderBar widget which renders both lines.
pub const HEADER_COMPACT_H: u16 = 2;

// ---------------------------------------------------------------------------
// Layout mode
// ---------------------------------------------------------------------------

/// Layout mode (auto-detected or overridden).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Two-column grid layout, wide terminal (>= 120 cols, >= 30 rows).
    Wide,
    /// Two-column grid layout, standard terminal (>= 80 cols, >= 28 rows).
    Standard,
    /// Single-column stacked layout (< 80 cols or < 28 rows).
    Compact,
}

impl LayoutMode {
    /// Auto-detect layout mode from terminal dimensions.
    pub fn detect(width: u16, height: u16) -> Self {
        if width >= 120 && height >= 30 {
            LayoutMode::Wide
        } else if width >= 80 && height >= 28 {
            LayoutMode::Standard
        } else {
            LayoutMode::Compact
        }
    }
}

// ---------------------------------------------------------------------------
// DashboardLayout
// ---------------------------------------------------------------------------

/// Pre-computed layout rectangles for each dashboard panel.
pub struct DashboardLayout {
    /// Active layout mode (reserved for future use / tests).
    #[allow(dead_code)]
    pub mode: LayoutMode,
    /// Two-line header area (status summary + epoch progress bar).
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
        LayoutMode::Wide | LayoutMode::Standard => compute_two_column_layout(area, mode),
        LayoutMode::Compact => compute_compact_layout(area),
    }
}

// ---------------------------------------------------------------------------
// Two-column layout (Wide and Standard share the same structure)
// ---------------------------------------------------------------------------

fn compute_two_column_layout(area: Rect, mode: LayoutMode) -> DashboardLayout {
    // Row heights: use the taller of the two side-by-side panels.
    let top_h = PANEL_NODE_H.max(PANEL_CHAIN_H);
    let mid_h = PANEL_CONNECTIONS_H.max(PANEL_RESOURCES_H);

    // Vertical split: header | top row | mid row | peers | gap | footer.
    let vertical = Layout::vertical([
        Constraint::Length(HEADER_H),      // header (2 lines)
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

    // Column split: 38% left / 62% right.
    // In Wide mode we allow slightly more room for Chain by keeping the same ratio
    // but the wider terminal naturally gives both panels more absolute space.
    let left_pct = if mode == LayoutMode::Wide { 36 } else { 38 };
    let right_pct = 100 - left_pct;

    let row1_cols = Layout::horizontal([
        Constraint::Percentage(left_pct),
        Constraint::Percentage(right_pct),
    ])
    .split(row1);

    let row2_cols = Layout::horizontal([
        Constraint::Percentage(left_pct),
        Constraint::Percentage(right_pct),
    ])
    .split(row2);

    DashboardLayout {
        mode,
        header,
        node: row1_cols[0],
        chain: row1_cols[1],
        connections: row2_cols[0],
        resources: row2_cols[1],
        peers,
        footer,
    }
}

// ---------------------------------------------------------------------------
// Compact layout
// ---------------------------------------------------------------------------

fn compute_compact_layout(area: Rect) -> DashboardLayout {
    let vertical = Layout::vertical([
        Constraint::Length(HEADER_COMPACT_H), // 2-line compact header (status + epoch bar)
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_mode_detect_wide() {
        assert_eq!(LayoutMode::detect(120, 30), LayoutMode::Wide);
        assert_eq!(LayoutMode::detect(200, 50), LayoutMode::Wide);
    }

    #[test]
    fn test_layout_mode_detect_standard() {
        assert_eq!(LayoutMode::detect(80, 28), LayoutMode::Standard);
        assert_eq!(LayoutMode::detect(119, 30), LayoutMode::Standard);
        assert_eq!(LayoutMode::detect(120, 29), LayoutMode::Standard);
    }

    #[test]
    fn test_layout_mode_detect_compact() {
        assert_eq!(LayoutMode::detect(79, 28), LayoutMode::Compact);
        assert_eq!(LayoutMode::detect(80, 27), LayoutMode::Compact);
        assert_eq!(LayoutMode::detect(40, 15), LayoutMode::Compact);
    }

    #[test]
    fn test_wide_layout_non_zero_rects() {
        let area = Rect::new(0, 0, 160, 50);
        let layout = compute_layout(area, Some(LayoutMode::Wide));
        assert_eq!(layout.header.height, HEADER_H);
        assert!(layout.node.width > 0 && layout.node.height > 0);
        assert!(layout.chain.width > 0 && layout.chain.height > 0);
        assert!(layout.connections.width > 0 && layout.connections.height > 0);
        assert!(layout.resources.width > 0 && layout.resources.height > 0);
        assert!(layout.peers.width > 0 && layout.peers.height > 0);
        assert_eq!(layout.footer.height, 1);
    }

    #[test]
    fn test_standard_layout_non_zero_rects() {
        let area = Rect::new(0, 0, 100, 40);
        let layout = compute_layout(area, Some(LayoutMode::Standard));
        assert_eq!(layout.header.height, HEADER_H);
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
        for mode in [LayoutMode::Wide, LayoutMode::Standard, LayoutMode::Compact] {
            let area = Rect::new(0, 0, 160, 60);
            let layout = compute_layout(area, Some(mode));
            assert_eq!(layout.footer.height, 1, "footer must be 1 line in {mode:?}");
        }
    }

    #[test]
    fn test_panels_do_not_fill_space() {
        // With plenty of vertical space the gap should absorb leftover rows.
        let area = Rect::new(0, 0, 120, 60);
        let layout = compute_layout(area, Some(LayoutMode::Standard));
        let top_h = PANEL_NODE_H.max(PANEL_CHAIN_H);
        let mid_h = PANEL_CONNECTIONS_H.max(PANEL_RESOURCES_H);
        let used = HEADER_H + top_h + mid_h + PANEL_PEERS_H + 1; // header + rows + footer
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

    #[test]
    fn test_header_is_one_line() {
        // Header is exactly 1 line — the epoch bar lives in the Chain panel, not here.
        assert_eq!(HEADER_H, 1, "HEADER_H constant must be 1");
        for mode in [LayoutMode::Wide, LayoutMode::Standard] {
            let area = Rect::new(0, 0, 160, 60);
            let layout = compute_layout(area, Some(mode));
            assert_eq!(
                layout.header.height, HEADER_H,
                "header must be {HEADER_H} line in {mode:?}"
            );
        }
    }

    #[test]
    fn test_compact_header_is_two_lines() {
        // Compact mode uses a 2-line header (status + epoch progress bar).
        assert_eq!(HEADER_COMPACT_H, 2, "HEADER_COMPACT_H constant must be 2");
        let area = Rect::new(0, 0, 60, 60);
        let layout = compute_layout(area, Some(LayoutMode::Compact));
        assert_eq!(
            layout.header.height, HEADER_COMPACT_H,
            "compact header must be {HEADER_COMPACT_H} lines"
        );
    }

    #[test]
    fn test_two_column_panels_same_y() {
        // Node/Chain must start at the same y; Connections/Resources must start at the same y.
        let area = Rect::new(0, 0, 160, 60);
        let layout = compute_layout(area, Some(LayoutMode::Wide));
        assert_eq!(layout.node.y, layout.chain.y, "Node and Chain y must match");
        assert_eq!(
            layout.connections.y, layout.resources.y,
            "Connections and Resources y must match"
        );
    }
}

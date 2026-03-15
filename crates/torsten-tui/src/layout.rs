//! Adaptive layout system for the Torsten TUI dashboard.
//!
//! Automatically selects a layout mode based on terminal dimensions:
//! - **Full** (>= 120 cols, >= 30 rows): header + 3 rows of panels
//! - **Standard** (>= 80 cols, >= 24 rows): header + 2x2 grid
//! - **Compact** (< 80 cols or < 24 rows): single column, no borders

use ratatui::layout::{Constraint, Layout, Rect};

/// Layout mode determined by terminal size or manual override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// >= 120 cols, >= 30 rows: header + 3 rows of panels.
    Full,
    /// >= 80 cols, >= 24 rows: header + 2x2 grid (default).
    Standard,
    /// < 80 cols or < 24 rows: single column, no borders.
    Compact,
}

impl LayoutMode {
    /// Detect the appropriate layout mode from terminal dimensions.
    pub fn detect(width: u16, height: u16) -> Self {
        if width >= 120 && height >= 30 {
            LayoutMode::Full
        } else if width >= 80 && height >= 24 {
            LayoutMode::Standard
        } else {
            LayoutMode::Compact
        }
    }
}

/// Pre-computed layout rectangles for each dashboard panel area.
pub struct DashboardLayout {
    /// The active layout mode.
    pub mode: LayoutMode,
    /// Header area (2-line status bar).
    pub header: Rect,
    /// Chain Status panel area.
    pub chain_status: Rect,
    /// Peers panel area.
    pub peers: Rect,
    /// Performance panel area.
    pub performance: Rect,
    /// Governance panel area.
    pub governance: Rect,
    /// Footer area (keyboard shortcuts).
    pub footer: Rect,
}

/// Compute the dashboard layout from the given terminal area and optional mode override.
///
/// When `override_mode` is `None`, the layout mode is auto-detected from the area
/// dimensions. When `Some(mode)`, that mode is used regardless of terminal size.
pub fn compute_layout(area: Rect, override_mode: Option<LayoutMode>) -> DashboardLayout {
    let mode = override_mode.unwrap_or_else(|| LayoutMode::detect(area.width, area.height));

    match mode {
        LayoutMode::Full => compute_full_layout(area, mode),
        LayoutMode::Standard => compute_standard_layout(area, mode),
        LayoutMode::Compact => compute_compact_layout(area, mode),
    }
}

/// Full layout: header (2) + top row (40%) + middle row (30%) + bottom row (30%) + footer (1).
fn compute_full_layout(area: Rect, mode: LayoutMode) -> DashboardLayout {
    let vertical = Layout::vertical([
        Constraint::Length(2),      // Header
        Constraint::Percentage(40), // Top row: Chain Status + Peers
        Constraint::Percentage(30), // Middle row: Performance
        Constraint::Percentage(30), // Bottom row: Governance
        Constraint::Length(1),      // Footer
    ])
    .split(area);

    let header = vertical[0];
    let footer = vertical[4];

    // Top row: Chain Status (62%) | Peers (38%)
    let top_cols = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(vertical[1]);

    // Middle row: Performance (full width)
    let performance = vertical[2];

    // Bottom row: Governance (full width)
    let governance = vertical[3];

    DashboardLayout {
        mode,
        header,
        chain_status: top_cols[0],
        peers: top_cols[1],
        performance,
        governance,
        footer,
    }
}

/// Standard layout: header (2) + body (2x2 grid) + footer (1).
fn compute_standard_layout(area: Rect, mode: LayoutMode) -> DashboardLayout {
    let vertical = Layout::vertical([
        Constraint::Length(2), // Header
        Constraint::Min(8),    // Body
        Constraint::Length(1), // Footer
    ])
    .split(area);

    let header = vertical[0];
    let body = vertical[1];
    let footer = vertical[2];

    // Body: 2 rows
    let rows =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(body);

    // Top row: Chain Status | Peers
    let top_cols =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(rows[0]);

    // Bottom row: Performance | Governance
    let bottom_cols =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(rows[1]);

    DashboardLayout {
        mode,
        header,
        chain_status: top_cols[0],
        peers: top_cols[1],
        performance: bottom_cols[0],
        governance: bottom_cols[1],
        footer,
    }
}

/// Compact layout: header (2) + stacked panels (single column) + footer (1).
fn compute_compact_layout(area: Rect, mode: LayoutMode) -> DashboardLayout {
    let vertical = Layout::vertical([
        Constraint::Length(2),      // Header
        Constraint::Percentage(30), // Chain Status
        Constraint::Percentage(20), // Peers
        Constraint::Percentage(30), // Performance
        Constraint::Percentage(20), // Governance
        Constraint::Length(1),      // Footer
    ])
    .split(area);

    DashboardLayout {
        mode,
        header: vertical[0],
        chain_status: vertical[1],
        peers: vertical[2],
        performance: vertical[3],
        governance: vertical[4],
        footer: vertical[5],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_mode_detect_full() {
        assert_eq!(LayoutMode::detect(120, 30), LayoutMode::Full);
        assert_eq!(LayoutMode::detect(200, 50), LayoutMode::Full);
    }

    #[test]
    fn test_layout_mode_detect_standard() {
        assert_eq!(LayoutMode::detect(80, 24), LayoutMode::Standard);
        assert_eq!(LayoutMode::detect(119, 30), LayoutMode::Standard);
        assert_eq!(LayoutMode::detect(120, 29), LayoutMode::Standard);
    }

    #[test]
    fn test_layout_mode_detect_compact() {
        assert_eq!(LayoutMode::detect(79, 24), LayoutMode::Compact);
        assert_eq!(LayoutMode::detect(80, 23), LayoutMode::Compact);
        assert_eq!(LayoutMode::detect(40, 15), LayoutMode::Compact);
    }

    #[test]
    fn test_compute_layout_auto_detect() {
        let area = Rect::new(0, 0, 120, 30);
        let layout = compute_layout(area, None);
        assert_eq!(layout.mode, LayoutMode::Full);

        let area = Rect::new(0, 0, 80, 24);
        let layout = compute_layout(area, None);
        assert_eq!(layout.mode, LayoutMode::Standard);

        let area = Rect::new(0, 0, 60, 20);
        let layout = compute_layout(area, None);
        assert_eq!(layout.mode, LayoutMode::Compact);
    }

    #[test]
    fn test_compute_layout_override() {
        let area = Rect::new(0, 0, 200, 50);
        let layout = compute_layout(area, Some(LayoutMode::Compact));
        assert_eq!(layout.mode, LayoutMode::Compact);
    }

    #[test]
    fn test_standard_layout_rects_non_zero() {
        let area = Rect::new(0, 0, 100, 30);
        let layout = compute_layout(area, Some(LayoutMode::Standard));
        assert!(layout.header.width > 0 && layout.header.height > 0);
        assert!(layout.chain_status.width > 0 && layout.chain_status.height > 0);
        assert!(layout.peers.width > 0 && layout.peers.height > 0);
        assert!(layout.performance.width > 0 && layout.performance.height > 0);
        assert!(layout.governance.width > 0 && layout.governance.height > 0);
        assert!(layout.footer.width > 0);
    }

    #[test]
    fn test_full_layout_rects_non_zero() {
        let area = Rect::new(0, 0, 140, 40);
        let layout = compute_layout(area, Some(LayoutMode::Full));
        assert!(layout.header.width > 0 && layout.header.height > 0);
        assert!(layout.chain_status.width > 0 && layout.chain_status.height > 0);
        assert!(layout.peers.width > 0 && layout.peers.height > 0);
        assert!(layout.performance.width > 0 && layout.performance.height > 0);
        assert!(layout.governance.width > 0 && layout.governance.height > 0);
        assert!(layout.footer.width > 0);
    }

    #[test]
    fn test_compact_layout_single_column() {
        let area = Rect::new(0, 0, 60, 20);
        let layout = compute_layout(area, Some(LayoutMode::Compact));
        // In compact mode, all panels span full width.
        assert_eq!(layout.chain_status.width, area.width);
        assert_eq!(layout.peers.width, area.width);
        assert_eq!(layout.performance.width, area.width);
        assert_eq!(layout.governance.width, area.width);
    }

    #[test]
    fn test_header_is_two_lines() {
        let area = Rect::new(0, 0, 100, 30);
        let layout = compute_layout(area, Some(LayoutMode::Standard));
        assert_eq!(layout.header.height, 2);
    }

    #[test]
    fn test_footer_is_one_line() {
        let area = Rect::new(0, 0, 100, 30);
        let layout = compute_layout(area, Some(LayoutMode::Standard));
        assert_eq!(layout.footer.height, 1);
    }
}

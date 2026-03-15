//! Layout and rendering for the Torsten TUI dashboard.
//!
//! Builds a 4-panel layout with:
//! - Top-left: Chain Status (sync progress, block/slot/epoch info)
//! - Top-right: Peers (hot/warm/cold breakdown, connection counts)
//! - Bottom-left: Performance (block rate sparkline, UTxO count, memory, mempool)
//! - Bottom-right: Governance (treasury, DReps, proposals, pools)
//! - Footer: keyboard shortcuts

use crate::app::{ActivePanel, App};
use crate::widgets::sparkline_history::SparklineHistory;
use crate::widgets::sync_progress::SyncProgressBar;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
    Frame,
};

/// Color palette constants for a cohesive dark-theme look.
const ACCENT_BLUE: Color = Color::Rgb(100, 149, 237); // Cornflower blue
const ACCENT_CYAN: Color = Color::Rgb(0, 210, 210);
const ACCENT_GREEN: Color = Color::Rgb(80, 220, 100);
const ACCENT_YELLOW: Color = Color::Rgb(255, 215, 0);
const ACCENT_RED: Color = Color::Rgb(255, 80, 80);
const DIM_WHITE: Color = Color::Rgb(160, 160, 170);
const BRIGHT_WHITE: Color = Color::Rgb(230, 230, 240);
const BORDER_NORMAL: Color = Color::Rgb(70, 70, 85);
const BORDER_ACTIVE: Color = ACCENT_BLUE;
const TITLE_COLOR: Color = Color::Rgb(180, 180, 200);

/// The poll interval in seconds (must match the actual interval in main.rs).
const POLL_INTERVAL_SECS: f64 = 2.0;

/// Render the complete dashboard frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let size = frame.area();

    // Main vertical split: body + footer
    let vertical = Layout::vertical([
        Constraint::Min(8),    // Body
        Constraint::Length(1), // Footer
    ])
    .split(size);

    let body = vertical[0];
    let footer_area = vertical[1];

    // Body: 2 rows
    let rows =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(body);

    // Top row: Chain Status | Peers
    let top_cols =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(rows[0]);

    // Bottom row: Performance | Governance
    let bottom_cols =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(rows[1]);

    // Render each panel
    render_chain_status(frame, app, top_cols[0]);
    render_peers(frame, app, top_cols[1]);
    render_performance(frame, app, bottom_cols[0]);
    render_governance(frame, app, bottom_cols[1]);
    render_footer(frame, app, footer_area);

    // Help overlay (rendered last, on top)
    if app.show_help {
        render_help_overlay(frame, size);
    }
}

/// Create a styled block with optional active-panel highlight.
fn panel_block(title: &str, is_active: bool) -> Block<'_> {
    let border_color = if is_active {
        BORDER_ACTIVE
    } else {
        BORDER_NORMAL
    };
    let title_style = if is_active {
        Style::default()
            .fg(ACCENT_BLUE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TITLE_COLOR)
    };

    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(title, title_style),
            Span::styled(" ", Style::default()),
        ]))
        .padding(Padding::new(1, 1, 0, 0))
}

/// Helper to create a metric line: "  Label:  Value"
fn metric_line<'a>(label: &'a str, value: String, value_color: Color) -> Line<'a> {
    Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(label, Style::default().fg(DIM_WHITE)),
        Span::styled(
            value,
            Style::default()
                .fg(value_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Render the Chain Status panel (top-left).
fn render_chain_status(frame: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == ActivePanel::Chain;
    let block = panel_block("Chain Status", is_active);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    // Sync status indicator
    let (status_label, is_synced, is_stalled) = app.sync_status();
    let status_color = if is_synced {
        ACCENT_GREEN
    } else if is_stalled {
        ACCENT_RED
    } else {
        ACCENT_YELLOW
    };
    let pct = app.sync_progress_pct();

    let status_indicator = if is_synced {
        "\u{25CF}" // ● filled circle
    } else if is_stalled {
        "\u{25CB}" // ○ empty circle
    } else {
        "\u{25D4}" // ◔ circle with upper-right quadrant
    };

    let connection_status = if app.metrics.connected {
        Span::styled(
            format!("  {} {} {:.2}%", status_indicator, status_label, pct),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            "  \u{25CB} Disconnected",
            Style::default().fg(ACCENT_RED).add_modifier(Modifier::BOLD),
        )
    };

    // Build metric lines
    let block_num = App::format_number(app.metrics.get_u64("torsten_block_number"));
    let slot_num = App::format_number(app.metrics.get_u64("torsten_slot_number"));
    let epoch = app.metrics.get_u64("torsten_epoch_number");
    let tip_age = app.metrics.get_u64("torsten_tip_age_seconds");
    let uptime = App::format_uptime(app.metrics.get_u64("torsten_uptime_seconds"));
    let rollbacks = app.metrics.get_u64("torsten_rollback_count_total");

    let era_label = if epoch > 0 { "Conway" } else { "Unknown" };

    let mut lines = vec![
        Line::from(vec![connection_status]),
        Line::default(), // spacer
    ];

    // Progress bar row
    lines.push(Line::default());

    lines.extend([
        metric_line("Block:    ", block_num, BRIGHT_WHITE),
        metric_line("Slot:     ", slot_num, BRIGHT_WHITE),
        metric_line(
            "Epoch:    ",
            format!("{} ({})", App::format_number(epoch), era_label),
            BRIGHT_WHITE,
        ),
        metric_line(
            "Tip age:  ",
            format!("{}s", tip_age),
            if tip_age < 30 {
                ACCENT_GREEN
            } else if tip_age < 120 {
                ACCENT_YELLOW
            } else {
                ACCENT_RED
            },
        ),
        metric_line("Uptime:   ", uptime, DIM_WHITE),
    ]);

    if rollbacks > 0 {
        lines.push(metric_line(
            "Rollbacks: ",
            App::format_number(rollbacks),
            ACCENT_YELLOW,
        ));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);

    // Render the sync progress bar on the 3rd line of inner area
    if inner.height > 2 && inner.width > 4 {
        let bar_area = Rect {
            x: inner.x + 2,
            y: inner.y + 2,
            width: inner.width.saturating_sub(4),
            height: 1,
        };
        frame.render_widget(SyncProgressBar::new(pct, is_synced, is_stalled), bar_area);
    }
}

/// Render the Peers panel (top-right).
fn render_peers(frame: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == ActivePanel::Peers;
    let block = panel_block("Peers", is_active);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let hot = app.metrics.get_u64("torsten_peers_hot");
    let warm = app.metrics.get_u64("torsten_peers_warm");
    let cold = app.metrics.get_u64("torsten_peers_cold");
    let total = app.metrics.get_u64("torsten_peers_connected");
    let n2n_active = app.metrics.get_u64("torsten_n2n_connections_active");
    let n2c_active = app.metrics.get_u64("torsten_n2c_connections_active");

    // Compute average handshake RTT if available
    let rtt_sum = app.metrics.get("torsten_peer_handshake_rtt_ms_sum");
    let rtt_count = app.metrics.get("torsten_peer_handshake_rtt_ms_count");
    let avg_rtt = if rtt_count > 0.0 {
        rtt_sum / rtt_count
    } else {
        0.0
    };

    let lines = vec![
        Line::default(),
        metric_line("Connected: ", App::format_number(total), BRIGHT_WHITE),
        Line::default(),
        Line::from(vec![
            Span::styled("  Hot:  ", Style::default().fg(DIM_WHITE)),
            Span::styled(
                format!("{}", hot),
                Style::default()
                    .fg(ACCENT_GREEN)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Warm: ", Style::default().fg(DIM_WHITE)),
            Span::styled(
                format!("{}", warm),
                Style::default()
                    .fg(ACCENT_YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Cold: ", Style::default().fg(DIM_WHITE)),
            Span::styled(
                format!("{}", cold),
                Style::default()
                    .fg(Color::Rgb(120, 120, 140))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::default(),
        metric_line("N2N active: ", App::format_number(n2n_active), DIM_WHITE),
        metric_line("N2C active: ", App::format_number(n2c_active), DIM_WHITE),
        Line::default(),
        metric_line(
            "Latency:    ",
            if avg_rtt > 0.0 {
                format!("{:.0}ms avg", avg_rtt)
            } else {
                "--".to_string()
            },
            ACCENT_CYAN,
        ),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render the Performance panel (bottom-left).
fn render_performance(frame: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == ActivePanel::Performance;
    let block = panel_block("Performance", is_active);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let bps = app.blocks_per_second(POLL_INTERVAL_SECS);
    let utxo_count = App::format_number(app.metrics.get_u64("torsten_utxo_count"));
    let mem_bytes = app.metrics.get_u64("torsten_mem_resident_bytes");
    let mem_str = App::format_bytes(mem_bytes);
    let mempool_txs = app.metrics.get_u64("torsten_mempool_tx_count");
    let mempool_bytes = app.metrics.get_u64("torsten_mempool_bytes");
    let blocks_applied = App::format_number(app.metrics.get_u64("torsten_blocks_applied_total"));
    let blocks_received = App::format_number(app.metrics.get_u64("torsten_blocks_received_total"));

    // Leave space for sparkline on the right side
    let sparkline_width = inner.width.saturating_sub(2).min(30) as usize;

    let lines = vec![
        Line::default(),
        metric_line("Blocks/s:    ", App::format_number(bps as u64), ACCENT_CYAN),
        Line::default(), // sparkline row placeholder
        Line::default(),
        metric_line("UTxOs:       ", utxo_count, BRIGHT_WHITE),
        metric_line(
            "Memory:      ",
            mem_str,
            if mem_bytes > 8_000_000_000 {
                ACCENT_RED
            } else if mem_bytes > 4_000_000_000 {
                ACCENT_YELLOW
            } else {
                ACCENT_GREEN
            },
        ),
        metric_line(
            "Mempool:     ",
            format!("{} txs ({})", mempool_txs, App::format_bytes(mempool_bytes)),
            BRIGHT_WHITE,
        ),
        Line::default(),
        metric_line("Applied:     ", blocks_applied, DIM_WHITE),
        metric_line("Received:    ", blocks_received, DIM_WHITE),
    ];

    frame.render_widget(Paragraph::new(lines), inner);

    // Render sparkline on the right portion of the panel, aligned with Blocks/s row
    if inner.height > 2 && !app.block_rate_history.is_empty() {
        let spark_area = Rect {
            x: inner.right().saturating_sub(sparkline_width as u16 + 1),
            y: inner.y + 2,
            width: sparkline_width as u16,
            height: 1,
        };
        frame.render_widget(
            SparklineHistory::new(&app.block_rate_history, ACCENT_CYAN),
            spark_area,
        );
    }
}

/// Render the Governance panel (bottom-right).
fn render_governance(frame: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == ActivePanel::Governance;
    let block = panel_block("Governance", is_active);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let treasury = app.metrics.get_u64("torsten_treasury_lovelace");
    let dreps = App::format_number(app.metrics.get_u64("torsten_drep_count"));
    let proposals = App::format_number(app.metrics.get_u64("torsten_proposal_count"));
    let pools = App::format_number(app.metrics.get_u64("torsten_pool_count"));
    let delegations = App::format_number(app.metrics.get_u64("torsten_delegation_count"));

    let lines = vec![
        Line::default(),
        metric_line(
            "Treasury:    ",
            format!("{} ADA", App::format_ada(treasury)),
            ACCENT_GREEN,
        ),
        Line::default(),
        metric_line("DReps:       ", dreps, BRIGHT_WHITE),
        metric_line("Proposals:   ", proposals, BRIGHT_WHITE),
        Line::default(),
        metric_line("Pools:       ", pools, ACCENT_CYAN),
        metric_line("Delegations: ", delegations, DIM_WHITE),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render the footer with keyboard shortcuts.
fn render_footer(frame: &mut Frame, _app: &App, area: Rect) {
    let shortcuts = Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            "[q]",
            Style::default()
                .fg(ACCENT_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("uit  ", Style::default().fg(DIM_WHITE)),
        Span::styled(
            "[Tab]",
            Style::default()
                .fg(ACCENT_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cycle panels  ", Style::default().fg(DIM_WHITE)),
        Span::styled(
            "[h]",
            Style::default()
                .fg(ACCENT_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("elp  ", Style::default().fg(DIM_WHITE)),
        Span::styled("  \u{2502}  ", Style::default().fg(BORDER_NORMAL)),
        Span::styled(
            "torsten-tui",
            Style::default().fg(Color::Rgb(100, 100, 120)),
        ),
    ]);

    frame.render_widget(Paragraph::new(shortcuts), area);
}

/// Render a centered help overlay on top of everything.
fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let overlay_width = 44u16;
    let overlay_height = 14u16;

    let x = area.x + area.width.saturating_sub(overlay_width) / 2;
    let y = area.y + area.height.saturating_sub(overlay_height) / 2;
    let overlay_area = Rect::new(
        x,
        y,
        overlay_width.min(area.width),
        overlay_height.min(area.height),
    );

    // Clear the background
    frame.render_widget(Clear, overlay_area);

    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT_BLUE))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                "Help",
                Style::default()
                    .fg(ACCENT_BLUE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
        ]))
        .padding(Padding::new(2, 2, 1, 0));

    let help_lines = vec![
        Line::from(Span::styled(
            "Torsten Node Dashboard",
            Style::default()
                .fg(BRIGHT_WHITE)
                .add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        help_key_line("q / Esc", "Quit"),
        help_key_line("Tab", "Cycle panels"),
        help_key_line("h", "Toggle this help"),
        help_key_line("r", "Force refresh metrics"),
        Line::default(),
        Line::from(Span::styled(
            "Polls metrics every 2 seconds.",
            Style::default().fg(DIM_WHITE),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Press any key to close.",
            Style::default().fg(Color::Rgb(100, 100, 120)),
        )),
    ];

    let inner = help_block.inner(overlay_area);
    frame.render_widget(help_block, overlay_area);
    frame.render_widget(Paragraph::new(help_lines), inner);
}

/// Helper for a help dialog key binding line.
fn help_key_line<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{:>10}", key),
            Style::default()
                .fg(ACCENT_CYAN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(desc, Style::default().fg(DIM_WHITE)),
    ])
}

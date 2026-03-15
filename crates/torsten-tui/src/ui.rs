//! Layout and rendering for the Torsten TUI dashboard.
//!
//! Builds a 4-panel layout with:
//! - Top-left: Chain Status (sync progress, block/slot/epoch info)
//! - Top-right: Peers (hot/warm/cold breakdown, connection counts)
//! - Bottom-left: Performance (block rate sparkline, UTxO count, memory, mempool)
//! - Bottom-right: Governance (treasury, DReps, proposals, pools)
//! - Footer: keyboard shortcuts

use crate::app::{ActivePanel, App};
use crate::theme::Theme;
use crate::widgets::sparkline_history::SparklineHistory;
use crate::widgets::sync_progress::SyncProgressBar;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
    Frame,
};

/// The poll interval in seconds (must match the actual interval in main.rs).
const POLL_INTERVAL_SECS: f64 = 2.0;

/// Render the complete dashboard frame.
pub fn draw(frame: &mut Frame, app: &App, theme: &Theme) {
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
    render_chain_status(frame, app, top_cols[0], theme);
    render_peers(frame, app, top_cols[1], theme);
    render_performance(frame, app, bottom_cols[0], theme);
    render_governance(frame, app, bottom_cols[1], theme);
    render_footer(frame, app, footer_area, theme);

    // Help overlay (rendered last, on top)
    if app.show_help {
        render_help_overlay(frame, size, theme);
    }
}

/// Create a styled block with optional active-panel highlight.
fn panel_block<'a>(title: &'a str, is_active: bool, theme: &Theme) -> Block<'a> {
    let border_color = if is_active {
        theme.border_active
    } else {
        theme.border
    };
    let title_style = if is_active {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.title)
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
fn metric_line<'a>(label: &'a str, value: String, value_color: Color, muted: Color) -> Line<'a> {
    Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(label, Style::default().fg(muted)),
        Span::styled(
            value,
            Style::default()
                .fg(value_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Render the Chain Status panel (top-left).
fn render_chain_status(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let is_active = app.active_panel == ActivePanel::Chain;
    let block = panel_block("Chain Status", is_active, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    // Sync status indicator
    let (status_label, is_synced, is_stalled) = app.sync_status();
    let status_color = if is_synced {
        theme.success
    } else if is_stalled {
        theme.error
    } else {
        theme.warning
    };
    let pct = app.sync_progress_pct();

    let status_indicator = if is_synced {
        "\u{25CF}" // filled circle
    } else if is_stalled {
        "\u{25CB}" // empty circle
    } else {
        "\u{25D4}" // circle with upper-right quadrant
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
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
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
        metric_line("Block:    ", block_num, theme.fg, theme.muted),
        metric_line("Slot:     ", slot_num, theme.fg, theme.muted),
        metric_line(
            "Epoch:    ",
            format!("{} ({})", App::format_number(epoch), era_label),
            theme.fg,
            theme.muted,
        ),
        metric_line(
            "Tip age:  ",
            format!("{}s", tip_age),
            if tip_age < 30 {
                theme.success
            } else if tip_age < 120 {
                theme.warning
            } else {
                theme.error
            },
            theme.muted,
        ),
        metric_line("Uptime:   ", uptime, theme.muted, theme.muted),
    ]);

    if rollbacks > 0 {
        lines.push(metric_line(
            "Rollbacks: ",
            App::format_number(rollbacks),
            theme.warning,
            theme.muted,
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
        frame.render_widget(
            SyncProgressBar::new(pct, is_synced, is_stalled)
                .fill_color_synced(theme.success)
                .fill_color_syncing(theme.warning)
                .fill_color_stalled(theme.error)
                .empty_color(theme.gauge_empty),
            bar_area,
        );
    }
}

/// Render the Peers panel (top-right).
fn render_peers(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let is_active = app.active_panel == ActivePanel::Peers;
    let block = panel_block("Peers", is_active, theme);
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
        metric_line(
            "Connected: ",
            App::format_number(total),
            theme.fg,
            theme.muted,
        ),
        Line::default(),
        Line::from(vec![
            Span::styled("  Hot:  ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{}", hot),
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Warm: ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{}", warm),
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Cold: ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{}", cold),
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::default(),
        metric_line(
            "N2N active: ",
            App::format_number(n2n_active),
            theme.muted,
            theme.muted,
        ),
        metric_line(
            "N2C active: ",
            App::format_number(n2c_active),
            theme.muted,
            theme.muted,
        ),
        Line::default(),
        metric_line(
            "Latency:    ",
            if avg_rtt > 0.0 {
                format!("{:.0}ms avg", avg_rtt)
            } else {
                "--".to_string()
            },
            theme.info,
            theme.muted,
        ),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render the Performance panel (bottom-left).
fn render_performance(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let is_active = app.active_panel == ActivePanel::Performance;
    let block = panel_block("Performance", is_active, theme);
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
        metric_line(
            "Blocks/s:    ",
            App::format_number(bps as u64),
            theme.info,
            theme.muted,
        ),
        Line::default(), // sparkline row placeholder
        Line::default(),
        metric_line("UTxOs:       ", utxo_count, theme.fg, theme.muted),
        metric_line(
            "Memory:      ",
            mem_str,
            if mem_bytes > 8_000_000_000 {
                theme.error
            } else if mem_bytes > 4_000_000_000 {
                theme.warning
            } else {
                theme.success
            },
            theme.muted,
        ),
        metric_line(
            "Mempool:     ",
            format!("{} txs ({})", mempool_txs, App::format_bytes(mempool_bytes)),
            theme.fg,
            theme.muted,
        ),
        Line::default(),
        metric_line("Applied:     ", blocks_applied, theme.muted, theme.muted),
        metric_line("Received:    ", blocks_received, theme.muted, theme.muted),
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
            SparklineHistory::new(&app.block_rate_history)
                .spark_low(theme.spark_low)
                .spark_mid(theme.spark_mid)
                .spark_high(theme.spark_high),
            spark_area,
        );
    }
}

/// Render the Governance panel (bottom-right).
fn render_governance(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let is_active = app.active_panel == ActivePanel::Governance;
    let block = panel_block("Governance", is_active, theme);
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
            theme.success,
            theme.muted,
        ),
        Line::default(),
        metric_line("DReps:       ", dreps, theme.fg, theme.muted),
        metric_line("Proposals:   ", proposals, theme.fg, theme.muted),
        Line::default(),
        metric_line("Pools:       ", pools, theme.info, theme.muted),
        metric_line("Delegations: ", delegations, theme.muted, theme.muted),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render the footer with keyboard shortcuts.
fn render_footer(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let shortcuts = Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            "[q]",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("uit  ", Style::default().fg(theme.muted)),
        Span::styled(
            "[Tab]",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cycle panels  ", Style::default().fg(theme.muted)),
        Span::styled(
            "[t]",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("heme  ", Style::default().fg(theme.muted)),
        Span::styled(
            "[h]",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("elp  ", Style::default().fg(theme.muted)),
        Span::styled("  \u{2502}  ", Style::default().fg(theme.border)),
        Span::styled(
            format!("torsten-tui [{}]", app.current_theme().name),
            Style::default().fg(theme.border),
        ),
    ]);

    frame.render_widget(Paragraph::new(shortcuts), area);
}

/// Render a centered help overlay on top of everything.
fn render_help_overlay(frame: &mut Frame, area: Rect, theme: &Theme) {
    let overlay_width = 44u16;
    let overlay_height = 15u16;

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
        .border_style(Style::default().fg(theme.accent))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                "Help",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
        ]))
        .padding(Padding::new(2, 2, 1, 0));

    let help_lines = vec![
        Line::from(Span::styled(
            "Torsten Node Dashboard",
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        help_key_line("q / Esc", "Quit", theme),
        help_key_line("Tab", "Cycle panels", theme),
        help_key_line("t", "Cycle theme", theme),
        help_key_line("h", "Toggle this help", theme),
        help_key_line("r", "Force refresh metrics", theme),
        Line::default(),
        Line::from(Span::styled(
            "Polls metrics every 2 seconds.",
            Style::default().fg(theme.muted),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Press any key to close.",
            Style::default().fg(theme.border),
        )),
    ];

    let inner = help_block.inner(overlay_area);
    frame.render_widget(help_block, overlay_area);
    frame.render_widget(Paragraph::new(help_lines), inner);
}

/// Helper for a help dialog key binding line.
fn help_key_line<'a>(key: &'a str, desc: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{:>10}", key),
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(desc, Style::default().fg(theme.muted)),
    ])
}

//! Layout and rendering for the Torsten TUI dashboard.
//!
//! Uses an adaptive layout system that auto-detects the best layout mode
//! based on terminal dimensions:
//! - Full (>= 120x30): header + 3 rows of panels
//! - Standard (>= 80x24): header + 2x2 grid
//! - Compact (< 80 or < 24): single column, no borders

use crate::app::{ActivePanel, App};
use crate::layout::{compute_layout, LayoutMode};
use crate::theme::Theme;
use crate::widgets::epoch_progress::EpochProgress;
use crate::widgets::header_bar::HeaderBar;
use crate::widgets::mempool_gauge::MempoolGauge;
use crate::widgets::sparkline_history::SparklineHistory;
use crate::widgets::sync_progress::SyncProgressBar;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
    Frame,
};

/// The poll interval in seconds (must match the actual interval in main.rs).
const POLL_INTERVAL_SECS: f64 = 2.0;

/// Render the complete dashboard frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let size = frame.area();
    let layout = compute_layout(size, app.layout_mode);
    let theme = app.theme();

    // Fill the entire terminal with the theme background.
    frame.render_widget(
        ratatui::widgets::Block::default()
            .style(Style::default().bg(theme.bg).fg(theme.fg)),
        size,
    );

    // Render header bar
    render_header(frame, app, layout.header, theme);

    // Render panels based on layout mode
    match layout.mode {
        LayoutMode::Compact => {
            // Compact mode: no borders, minimal rendering
            render_chain_status_compact(frame, app, layout.chain_status, theme);
            render_peers_compact(frame, app, layout.peers, theme);
            render_performance_compact(frame, app, layout.performance, theme);
            render_governance_compact(frame, app, layout.governance, theme);
        }
        _ => {
            // Full and Standard modes use bordered panels
            render_chain_status(frame, app, layout.chain_status, theme);
            render_peers(frame, app, layout.peers, theme);
            render_performance(frame, app, layout.performance, theme);
            render_governance(frame, app, layout.governance, theme);
        }
    }

    render_footer(frame, app, layout.footer, theme);

    // Help overlay (rendered last, on top)
    if app.show_help {
        render_help_overlay(frame, size, theme);
    }
}

/// Render the header bar widget. Uses line 1 for status summary and line 2
/// for the epoch countdown progress bar showing slot position and time remaining.
fn render_header(frame: &mut Frame, app: &App, area: Rect, _theme: Theme) {
    let (_, is_synced, is_stalled) = app.sync_status();
    let pct = app.sync_progress_pct();
    let uptime = App::format_uptime(app.metrics.get_u64("torsten_uptime_seconds"));
    let epoch = app.metrics.get_u64("torsten_epoch_number");
    let tip_age = app.metrics.get_u64("torsten_tip_age_seconds");

    let header = HeaderBar {
        sync_pct: pct,
        is_synced,
        is_stalled,
        epoch,
        tip_age,
        uptime,
        epoch_progress: app.epoch_progress_pct / 100.0,
        connected: app.metrics.connected,
    };

    // Line 1: rendered by the HeaderBar widget (it uses both lines).
    // We override line 2 with our EpochProgress widget for better detail.
    frame.render_widget(header, area);

    // Overlay line 2 with the EpochProgress widget if we have space.
    if area.height >= 2 {
        let epoch_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: 1,
        };
        let epoch_len = app.epoch_length();
        frame.render_widget(
            EpochProgress::new(app.slot_in_epoch, epoch_len, app.epoch_time_remaining_secs),
            epoch_area,
        );
    }
}

/// Create a styled block with optional active-panel highlight, using theme colors.
fn panel_block<'a>(title: &'a str, is_active: bool, theme: Theme) -> Block<'a> {
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

/// Helper to create a metric line with right-aligned values.
///
/// The `width` parameter controls the total line width for alignment.
/// When 0, falls back to simple left-aligned layout.
fn metric_line_aligned<'a>(
    label: &'a str,
    value: String,
    value_color: Color,
    label_color: Color,
    width: u16,
) -> Line<'a> {
    if width == 0 {
        return metric_line(label, value, value_color, label_color);
    }

    // Compute padding needed to right-align the value within the available width.
    // Format: "  Label    Value" where value is right-aligned.
    let prefix_len = 2 + label.len(); // "  " + label
    let value_len = value.len();
    let total = prefix_len + value_len;
    let available = width as usize;
    let padding = available.saturating_sub(total);

    Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(label, Style::default().fg(label_color)),
        Span::styled(" ".repeat(padding), Style::default()),
        Span::styled(
            value,
            Style::default()
                .fg(value_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Helper to create a metric line: "  Label:  Value"
fn metric_line<'a>(label: &'a str, value: String, value_color: Color, label_color: Color) -> Line<'a> {
    Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(label, Style::default().fg(label_color)),
        Span::styled(
            value,
            Style::default()
                .fg(value_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Render the Chain Status panel (bordered).
fn render_chain_status(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let is_active = app.active_panel == ActivePanel::Chain;
    let block = panel_block("Chain Status", is_active, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let w = inner.width.saturating_sub(4); // account for padding

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
            Style::default().fg(theme.error).add_modifier(Modifier::BOLD),
        )
    };

    // Build metric lines
    let block_num = App::format_number(app.metrics.get_u64("torsten_block_number"));
    let slot_num = App::format_number(app.metrics.get_u64("torsten_slot_number"));
    let epoch = app.metrics.get_u64("torsten_epoch_number");
    // tip_age is already computed by the node dynamically as
    // current_time - slot_time, so `torsten_tip_age_seconds` is accurate.
    let tip_age = app.metrics.get_u64("torsten_tip_age_seconds");
    let uptime = App::format_uptime(app.metrics.get_u64("torsten_uptime_seconds"));
    let rollbacks = app.metrics.get_u64("torsten_rollback_count_total");

    let era_label = if epoch > 0 { "Conway" } else { "Unknown" };

    let tip_color = if tip_age < 30 {
        theme.success
    } else if tip_age < 120 {
        theme.warning
    } else {
        theme.error
    };

    let mut lines = vec![
        Line::from(vec![connection_status]),
        Line::default(), // spacer
    ];

    // Progress bar row
    lines.push(Line::default());

    lines.extend([
        metric_line_aligned("Block:    ", block_num, theme.fg, theme.muted, w),
        metric_line_aligned("Slot:     ", slot_num, theme.fg, theme.muted, w),
        metric_line_aligned(
            "Epoch:    ",
            format!("{} ({})", App::format_number(epoch), era_label),
            theme.fg,
            theme.muted,
            w,
        ),
        metric_line_aligned(
            "Tip age:  ",
            format!("{}s", tip_age),
            tip_color,
            theme.muted,
            w,
        ),
        metric_line_aligned("Uptime:   ", uptime, theme.muted, theme.muted, w),
    ]);

    if rollbacks > 0 {
        lines.push(metric_line_aligned(
            "Rollbacks: ",
            App::format_number(rollbacks),
            theme.warning,
            theme.muted,
            w,
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

/// Render the Peers panel (bordered).
///
/// Shows hot/warm/cold peer classification, outbound/inbound/duplex connection
/// counts (replacing the old N2N/N2C active counts), chainsync idle duration,
/// and average handshake RTT.
fn render_peers(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let is_active = app.active_panel == ActivePanel::Peers;
    let block = panel_block("Peers", is_active, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let w = inner.width.saturating_sub(4);

    let hot = app.metrics.get_u64("torsten_peers_hot");
    let warm = app.metrics.get_u64("torsten_peers_warm");
    let cold = app.metrics.get_u64("torsten_peers_cold");
    let total = app.metrics.get_u64("torsten_peers_connected");
    let outbound = app.metrics.get_u64("torsten_peers_outbound");
    let inbound = app.metrics.get_u64("torsten_peers_inbound");
    let duplex = app.metrics.get_u64("torsten_peers_duplex");
    let chainsync_idle = app.metrics.get_u64("torsten_chainsync_idle_seconds");

    // Compute average handshake RTT if available
    let rtt_sum = app.metrics.get("torsten_peer_handshake_rtt_ms_sum");
    let rtt_count = app.metrics.get("torsten_peer_handshake_rtt_ms_count");
    let avg_rtt = if rtt_count > 0.0 {
        rtt_sum / rtt_count
    } else {
        0.0
    };

    // Compute average block-fetch latency (histogram sum / count).
    let fetch_sum = app.metrics.get("torsten_peer_block_fetch_ms_sum");
    let fetch_count = app.metrics.get("torsten_peer_block_fetch_ms_count");
    let avg_fetch_ms = if fetch_count > 0.0 {
        fetch_sum / fetch_count
    } else {
        0.0
    };

    // Chainsync idle color: green < 5s, yellow < 30s, red >= 30s.
    let idle_color = if chainsync_idle < 5 {
        theme.success
    } else if chainsync_idle < 30 {
        theme.warning
    } else {
        theme.error
    };

    let mut lines = vec![
        Line::default(),
        metric_line_aligned("Connected: ", App::format_number(total), theme.fg, theme.muted, w),
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
                    .fg(theme.muted)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::default(),
        // Outbound/inbound/duplex replace the old N2N/N2C active counts.
        // These reflect actual connection directionality from the P2P manager.
        metric_line_aligned("Outbound:   ", App::format_number(outbound), theme.info, theme.muted, w),
        metric_line_aligned("Inbound:    ", App::format_number(inbound), theme.info, theme.muted, w),
        metric_line_aligned("Duplex:     ", App::format_number(duplex), theme.muted, theme.muted, w),
        Line::default(),
        metric_line_aligned(
            "Sync idle:  ",
            if chainsync_idle == 0 {
                "active".to_string()
            } else {
                format!("{}s", chainsync_idle)
            },
            idle_color,
            theme.muted,
            w,
        ),
        metric_line_aligned(
            "RTT avg:    ",
            if avg_rtt > 0.0 {
                format!("{:.0}ms", avg_rtt)
            } else {
                "--".to_string()
            },
            theme.info,
            theme.muted,
            w,
        ),
        metric_line_aligned(
            "Fetch avg:  ",
            if avg_fetch_ms > 0.0 {
                format!("{:.1}ms/blk", avg_fetch_ms)
            } else {
                "--".to_string()
            },
            theme.info,
            theme.muted,
            w,
        ),
    ];

    // Show N2C connection count when non-zero (local clients attached).
    let n2c_active = app.metrics.get_u64("torsten_n2c_connections_active");
    if n2c_active > 0 {
        lines.push(metric_line_aligned(
            "N2C clients:",
            App::format_number(n2c_active),
            theme.muted,
            theme.muted,
            w,
        ));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render the Performance panel (bordered).
///
/// Layout (row indices within inner area):
/// - Row 0: blank spacer
/// - Row 1: Blocks/s label
/// - Row 2: block-rate sparkline
/// - Row 3: TX/s label
/// - Row 4: tx-rate sparkline
/// - Row 5: blank spacer
/// - Row 6: UTxOs
/// - Row 7: Memory label
/// - Row 8: memory sparkline
/// - Row 9: Mempool label
/// - Row 10: mempool gauge + depth sparkline
/// - Rows 11+: block production (forged, leader checks, missed) when producing
fn render_performance(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let is_active = app.active_panel == ActivePanel::Performance;
    let block = panel_block("Performance", is_active, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let w = inner.width.saturating_sub(4);

    let bps = app.blocks_per_second(POLL_INTERVAL_SECS);
    let tps = app.txs_per_second(POLL_INTERVAL_SECS);
    let utxo_count = App::format_number(app.metrics.get_u64("torsten_utxo_count"));
    let mem_bytes = app.metrics.get_u64("torsten_mem_resident_bytes");
    let mem_str = App::format_bytes(mem_bytes);
    let mempool_txs = app.metrics.get_u64("torsten_mempool_tx_count");
    let mempool_bytes = app.metrics.get_u64("torsten_mempool_bytes");
    let blocks_applied = App::format_number(app.metrics.get_u64("torsten_blocks_applied_total"));
    let blocks_received = App::format_number(app.metrics.get_u64("torsten_blocks_received_total"));
    let blocks_forged = app.metrics.get_u64("torsten_blocks_forged_total");

    let mem_color = if mem_bytes > 8_000_000_000 {
        theme.error
    } else if mem_bytes > 4_000_000_000 {
        theme.warning
    } else {
        theme.success
    };

    // Sparkline width: up to 30 columns, anchored to the right of the inner area.
    let sparkline_width = inner.width.saturating_sub(2).min(30) as usize;

    // Build the text rows. Rows 2, 4, 8, and 10 are placeholder lines that will
    // be overlaid with sparklines/gauges after the Paragraph is rendered.
    let mut lines = vec![
        Line::default(),                                                                  // row 0
        metric_line_aligned("Blocks/s:    ", format!("{}", bps as u64), theme.info, theme.muted, w), // row 1
        Line::default(),                                                                  // row 2: block sparkline
        metric_line_aligned("TX/s:        ", format!("{}", tps as u64), theme.success, theme.muted, w), // row 3
        Line::default(),                                                                  // row 4: tx sparkline
        Line::default(),                                                                  // row 5
        metric_line_aligned("UTxOs:       ", utxo_count, theme.fg, theme.muted, w),     // row 6
        metric_line_aligned("Memory:      ", mem_str, mem_color, theme.muted, w),        // row 7
        Line::default(),                                                                  // row 8: memory sparkline
        metric_line_aligned(
            "Mempool:     ",
            format!("{} txs ({})", mempool_txs, App::format_bytes(mempool_bytes)),
            theme.fg,
            theme.muted,
            w,
        ),                                                                                // row 9
        Line::default(),                                                                  // row 10: mempool gauge
    ];

    // Block production section (only when blocks_forged > 0).
    // Uses `torsten_leader_checks_total` — the correct metric name for VRF checks.
    if blocks_forged > 0 {
        let leader_checks = app.metrics.get_u64("torsten_leader_checks_total");
        let leader_checks_not_elected =
            app.metrics.get_u64("torsten_leader_checks_not_elected_total");
        // Missed slots: leader checks where we were elected but didn't forge.
        // elected = leader_checks - leader_checks_not_elected
        let elected = leader_checks.saturating_sub(leader_checks_not_elected);
        let missed = elected.saturating_sub(blocks_forged);
        lines.push(Line::default());
        lines.push(metric_line_aligned(
            "Forged:      ",
            App::format_number(blocks_forged),
            theme.success,
            theme.muted,
            w,
        ));
        lines.push(metric_line_aligned(
            "VRF checks:  ",
            App::format_number(leader_checks),
            theme.muted,
            theme.muted,
            w,
        ));
        lines.push(metric_line_aligned(
            "Missed:      ",
            App::format_number(missed),
            if missed > 0 { theme.warning } else { theme.muted },
            theme.muted,
            w,
        ));
    }

    lines.push(Line::default());
    lines.push(metric_line_aligned(
        "Applied:     ",
        blocks_applied,
        theme.muted,
        theme.muted,
        w,
    ));
    lines.push(metric_line_aligned(
        "Received:    ",
        blocks_received,
        theme.muted,
        theme.muted,
        w,
    ));

    frame.render_widget(Paragraph::new(lines), inner);

    // --- Sparkline overlays ---

    // Block rate sparkline (row 2).
    if inner.height > 2 && !app.block_rate_history.is_empty() {
        let spark_area = Rect {
            x: inner.right().saturating_sub(sparkline_width as u16 + 1),
            y: inner.y + 2,
            width: sparkline_width as u16,
            height: 1,
        };
        frame.render_widget(
            SparklineHistory::with_color(&app.block_rate_history, theme.info),
            spark_area,
        );
    }

    // TX rate sparkline (row 4).
    if inner.height > 4 && !app.tx_rate_history.is_empty() {
        let spark_area = Rect {
            x: inner.right().saturating_sub(sparkline_width as u16 + 1),
            y: inner.y + 4,
            width: sparkline_width as u16,
            height: 1,
        };
        frame.render_widget(
            SparklineHistory::with_color(&app.tx_rate_history, theme.success),
            spark_area,
        );
    }

    // Memory sparkline (row 8): shows RSS trend over the last ~2 minutes.
    if inner.height > 8 && !app.memory_history.is_empty() {
        let spark_area = Rect {
            x: inner.right().saturating_sub(sparkline_width as u16 + 1),
            y: inner.y + 8,
            width: sparkline_width as u16,
            height: 1,
        };
        frame.render_widget(
            SparklineHistory::with_color(&app.memory_history, mem_color),
            spark_area,
        );
    }

    // Mempool gauge (row 10).
    if inner.height > 10 && inner.width > 8 {
        let gauge_area = Rect {
            x: inner.x + 2,
            y: inner.y + 10,
            width: inner.width.saturating_sub(4),
            height: 1,
        };
        frame.render_widget(MempoolGauge::new(mempool_txs), gauge_area);

        // Mempool depth sparkline overlaid on the right half of the gauge row.
        if !app.mempool_depth_history.is_empty() {
            let spark_w = (inner.width / 3).min(sparkline_width as u16);
            let spark_area = Rect {
                x: gauge_area.right().saturating_sub(spark_w),
                y: inner.y + 10,
                width: spark_w,
                height: 1,
            };
            frame.render_widget(
                SparklineHistory::with_color(&app.mempool_depth_history, theme.warning),
                spark_area,
            );
        }
    }
}

/// Render the Governance panel (bordered).
///
/// Includes treasury, DRep/proposal counts, pool stats, and disk info.
fn render_governance(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let is_active = app.active_panel == ActivePanel::Governance;
    let block = panel_block("Governance", is_active, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let w = inner.width.saturating_sub(4);

    let treasury = app.metrics.get_u64("torsten_treasury_lovelace");
    let dreps = App::format_number(app.metrics.get_u64("torsten_drep_count"));
    let proposals = App::format_number(app.metrics.get_u64("torsten_proposal_count"));
    let pools = App::format_number(app.metrics.get_u64("torsten_pool_count"));
    let delegations = App::format_number(app.metrics.get_u64("torsten_delegation_count"));
    let disk_avail = app.metrics.get_u64("torsten_disk_available_bytes");

    let mut lines = vec![
        Line::default(),
        metric_line_aligned(
            "Treasury:    ",
            format!("{} ADA", App::format_ada(treasury)),
            theme.success,
            theme.muted,
            w,
        ),
        Line::default(),
        metric_line_aligned("DReps:       ", dreps, theme.fg, theme.muted, w),
        metric_line_aligned("Proposals:   ", proposals, theme.fg, theme.muted, w),
        Line::default(),
        metric_line_aligned("Pools:       ", pools, theme.info, theme.muted, w),
        metric_line_aligned("Delegations: ", delegations, theme.muted, theme.muted, w),
    ];

    // Disk info: show when available (> 0 means the metric is being emitted).
    if disk_avail > 0 {
        let disk_color = if disk_avail > 50_000_000_000 {
            theme.success // > 50 GB
        } else if disk_avail > 10_000_000_000 {
            theme.warning // > 10 GB
        } else {
            theme.error // < 10 GB
        };
        lines.push(Line::default());
        lines.push(metric_line_aligned(
            "Disk avail:  ",
            App::format_bytes(disk_avail),
            disk_color,
            theme.muted,
            w,
        ));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---- Compact mode renderers (no borders, minimal output) ----

/// Render chain status in compact mode (no borders, minimal 2-line output).
fn render_chain_status_compact(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    if area.height < 1 || area.width < 10 {
        return;
    }

    let (status_label, is_synced, is_stalled) = app.sync_status();
    let status_color = if is_synced {
        theme.success
    } else if is_stalled {
        theme.error
    } else {
        theme.warning
    };
    let pct = app.sync_progress_pct();
    let block_num = App::format_number(app.metrics.get_u64("torsten_block_number"));
    let slot_num = App::format_number(app.metrics.get_u64("torsten_slot_number"));
    let epoch = app.metrics.get_u64("torsten_epoch_number");
    let tip_age = app.metrics.get_u64("torsten_tip_age_seconds");

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                "CHAIN ",
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} {:.2}%", status_label, pct),
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Blk: ", Style::default().fg(theme.muted)),
            Span::styled(block_num, Style::default().fg(theme.fg)),
            Span::styled("  Slot: ", Style::default().fg(theme.muted)),
            Span::styled(slot_num, Style::default().fg(theme.fg)),
        ]),
    ];

    if area.height >= 3 {
        let tip_color = if tip_age < 30 {
            theme.success
        } else if tip_age < 120 {
            theme.warning
        } else {
            theme.error
        };
        lines.push(Line::from(vec![
            Span::styled("  Epoch: ", Style::default().fg(theme.muted)),
            Span::styled(App::format_number(epoch), Style::default().fg(theme.fg)),
            Span::styled("  Tip: ", Style::default().fg(theme.muted)),
            Span::styled(format!("{}s", tip_age), Style::default().fg(tip_color)),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Render peers in compact mode (no borders).
fn render_peers_compact(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    if area.height < 1 || area.width < 10 {
        return;
    }

    let total = app.metrics.get_u64("torsten_peers_connected");
    let hot = app.metrics.get_u64("torsten_peers_hot");
    let warm = app.metrics.get_u64("torsten_peers_warm");

    let lines = vec![Line::from(vec![
        Span::styled(
            "PEERS ",
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{}", total),
            Style::default()
                .fg(theme.fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  H:{} W:{}", hot, warm),
            Style::default().fg(theme.muted),
        ),
    ])];

    frame.render_widget(Paragraph::new(lines), area);
}

/// Render performance in compact mode (no borders).
fn render_performance_compact(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    if area.height < 1 || area.width < 10 {
        return;
    }

    let bps = app.blocks_per_second(POLL_INTERVAL_SECS);
    let utxo_count = App::format_number(app.metrics.get_u64("torsten_utxo_count"));
    let mem_bytes = app.metrics.get_u64("torsten_mem_resident_bytes");

    let mem_color = if mem_bytes > 8_000_000_000 {
        theme.error
    } else if mem_bytes > 4_000_000_000 {
        theme.warning
    } else {
        theme.success
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            "PERF  ",
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:.0} blk/s", bps),
            Style::default()
                .fg(theme.info)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  Mem: {}", App::format_bytes(mem_bytes)),
            Style::default().fg(mem_color),
        ),
    ])];

    if area.height >= 2 {
        lines.push(Line::from(vec![
            Span::styled("  UTxOs: ", Style::default().fg(theme.muted)),
            Span::styled(utxo_count, Style::default().fg(theme.fg)),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Render governance in compact mode (no borders).
fn render_governance_compact(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    if area.height < 1 || area.width < 10 {
        return;
    }

    let treasury = app.metrics.get_u64("torsten_treasury_lovelace");
    let pools = App::format_number(app.metrics.get_u64("torsten_pool_count"));

    let lines = vec![Line::from(vec![
        Span::styled(
            "GOV   ",
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} ADA", App::format_ada(treasury)),
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  Pools: {}", pools),
            Style::default().fg(theme.muted),
        ),
    ])];

    frame.render_widget(Paragraph::new(lines), area);
}

/// Render the footer with keyboard shortcuts and current theme name.
fn render_footer(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    // Show the current layout mode indicator
    let mode_label = match app.layout_mode {
        None => "Auto",
        Some(LayoutMode::Full) => "Full",
        Some(LayoutMode::Standard) => "Std",
        Some(LayoutMode::Compact) => "Cpt",
    };

    let theme_name = app.theme().name;

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
            "[Tab/S-Tab]",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cycle  ", Style::default().fg(theme.muted)),
        Span::styled(
            "[1-4]",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" panel  ", Style::default().fg(theme.muted)),
        Span::styled(
            "[m]",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("ode:{}  ", mode_label),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            "[t]",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("heme:{}  ", theme_name),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            "[?]",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("help  ", Style::default().fg(theme.muted)),
        Span::styled("  \u{2502}  ", Style::default().fg(theme.border)),
        Span::styled(
            "torsten-tui",
            Style::default().fg(theme.muted),
        ),
    ]);

    frame.render_widget(Paragraph::new(shortcuts), area);
}

/// Render a centered help overlay on top of everything.
fn render_help_overlay(frame: &mut Frame, area: Rect, theme: Theme) {
    let overlay_width = 52u16;
    let overlay_height = 22u16;

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
            Style::default()
                .fg(theme.fg)
                .add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        help_key_line("q / Esc", "Quit", theme),
        help_key_line("Tab", "Cycle panels forward", theme),
        help_key_line("Shift+Tab", "Cycle panels backward", theme),
        help_key_line("1-4", "Jump to panel (1=Chain, etc.)", theme),
        help_key_line("m", "Toggle layout mode", theme),
        help_key_line("t", "Cycle color theme", theme),
        help_key_line("h / ?", "Toggle this help", theme),
        help_key_line("r", "Force refresh metrics", theme),
        Line::default(),
        Line::from(Span::styled(
            "Layout: Auto/Full/Standard/Compact",
            Style::default().fg(theme.muted),
        )),
        Line::from(Span::styled(
            "Polls metrics every 2 seconds.",
            Style::default().fg(theme.muted),
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
fn help_key_line<'a>(key: &'a str, desc: &'a str, theme: Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{:>10}", key),
            Style::default()
                .fg(theme.info)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(desc, Style::default().fg(theme.muted)),
    ])
}

//! Dashboard rendering — draws all five panels, the header bar, footer, and the
//! optional help overlay.
//!
//! Every colour reference goes through the active [`Theme`] so that theme
//! cycling (key `t`) changes the entire look instantly.
//!
//! Design goals
//! ============
//! - **Labels left, values right** within each panel column. Values are
//!   right-aligned to a fixed column so numbers form clean vertical stacks.
//! - **Thousands separators** on all integer metrics (4,109,330 not 4109330).
//! - **Human-readable bytes** (6.5 GB, 512.0 MB).
//! - **No duplicate fields** — each metric appears in exactly one panel.
//! - **Status pill** in header — colored background for sync status.
//! - **Tip diff indicator** — green check / warning / red X per age bracket.
//! - **RTT metrics** — Low/Avg/High derived from histogram _sum/_count.
//! - **Duplex count** — shown in Connections panel; 0 if metric not populated.
//! - **Visual hierarchy**: bold + color for important values, muted for labels.
//! - **Consistent padding**: one character left/right inside every panel border.
//! - **Footer**: keyboard shortcuts + current theme pill.
//! - **Epoch bar**: smooth 8-shade block-character fill with centered label
//!   (appears ONLY inside the Chain panel, not duplicated in the header).
//! - **RTT bar**: per-band colored segments summing to panel inner width.
//!
//! Panel layout (Standard / Wide, >= 80 x 28):
//!
//! ```text
//! ┌──────────────────────── Header (1 line) ───────────────────────────────────┐
//! ├──────── Node ───────────┬──────────────────── Chain ─────────────────────── ┤
//! ├──── Connections ────────┼─────────────────── Resources ──────────────────── ┤
//! ├────────────────────────── Peers (full width) ──────────────────────────────── ┤
//! │                                   (gap)                                      │
//! ├──────────────────────── Footer (1 line) ───────────────────────────────────┘
//! ```

use crate::app::App;
use crate::layout::{compute_layout, LayoutMode};
use crate::theme::Theme;
use crate::widgets::epoch_progress::EpochProgress;
use crate::widgets::header_bar::HeaderBar;
use crate::widgets::mempool_gauge::MempoolGauge;
use crate::widgets::sparkline_history::SparklineHistory;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
    Frame,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Label column width (characters) inside a panel, including the trailing space.
/// Values are right-aligned within each row so they all start at `LABEL_W + 2`
/// (the 2 accounts for the 1-char left padding in `kv_line`).
const LABEL_W: usize = 14;

/// Width reserved for the value field (used for right-aligning numbers).
const VALUE_W: usize = 12;

// Smooth block-character set for progress bars, index 0 (empty) – 8 (full).
const SMOOTH_BLOCKS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Entry point called from the main event loop each frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let theme = app.theme();
    let layout = compute_layout(area, None);

    if layout.mode == LayoutMode::Compact {
        // Compact mode uses the 2-line HeaderBar widget (status + epoch progress bar).
        render_compact_header(frame, app, theme, layout.header);
    } else {
        // Standard/Wide modes use the single-line inline header.
        render_header(frame, app, theme, layout.header);
    }
    render_node_panel(frame, app, theme, layout.node);
    render_chain_panel(frame, app, theme, layout.chain);
    render_connections_panel(frame, app, theme, layout.connections);
    render_resources_panel(frame, app, theme, layout.resources);
    render_peers_panel(frame, app, theme, layout.peers);
    render_footer(frame, app, theme, layout.footer);

    if app.show_help {
        render_help_overlay(frame, theme, area);
    }
}

// ---------------------------------------------------------------------------
// Header (1 line — status pill only, no epoch bar duplication)
// ---------------------------------------------------------------------------

/// Single-line header:
///   Logo | [STATUS PILL] | Epoch NNN | Era Conway | Net Preview | Tip Xs | Up Xh Xm
///
/// The epoch progress bar lives exclusively inside the Chain panel.
fn render_header(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    if area.height < 1 || area.width < 20 {
        return;
    }

    let (status_label, is_synced, is_stalled) = app.sync_status();
    let pct = app.sync_progress_pct();
    let epoch = app.metrics.get_u64("torsten_epoch_number");
    let tip_age = app.metrics.get_u64("torsten_tip_age_seconds");
    let uptime_secs = app.metrics.get_u64("torsten_uptime_seconds");
    let uptime = App::format_uptime(uptime_secs);
    let era = app.current_era();
    let network = app.network.label();

    // Status pill — colored background so it reads at a glance.
    let status_bg = if !app.metrics.connected {
        theme.error
    } else if is_synced {
        theme.success
    } else if is_stalled {
        theme.error
    } else {
        theme.warning
    };

    let status_text = if !app.metrics.connected {
        " Disconnected ".to_string()
    } else {
        format!(" {} {:.2}% ", status_label, pct)
    };

    // Tip age indicator: check mark / warning / X based on age brackets.
    // Requirements: green check <20s, warning 20-60s, red X >60s.
    let (tip_icon, tip_age_col) = tip_age_indicator(theme, tip_age);
    let tip_str = if tip_age == 0 {
        format!("{} --", tip_icon)
    } else {
        format!("{} {}", tip_icon, format_tip_age(tip_age))
    };

    let line = Line::from(vec![
        // Logo.
        Span::styled(
            " Torsten ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        sep(theme),
        // Sync status pill with colored background.
        Span::styled(
            &status_text,
            Style::default()
                .fg(Color::Black)
                .bg(status_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        sep(theme),
        // Epoch.
        Span::styled("  Epoch ", Style::default().fg(theme.muted)),
        Span::styled(
            App::format_number(epoch),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        sep_spaced(theme),
        // Era.
        Span::styled("Era ", Style::default().fg(theme.muted)),
        Span::styled(
            era,
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        sep_spaced(theme),
        // Network.
        Span::styled("Net ", Style::default().fg(theme.muted)),
        Span::styled(network, Style::default().fg(theme.accent)),
        sep_spaced(theme),
        // Tip age with colored indicator icon.
        Span::styled("Tip ", Style::default().fg(theme.muted)),
        Span::styled(
            &tip_str,
            Style::default()
                .fg(tip_age_col)
                .add_modifier(if tip_age >= 60 {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        sep_spaced(theme),
        // Uptime.
        Span::styled("Up ", Style::default().fg(theme.muted)),
        Span::styled(&uptime, Style::default().fg(theme.muted)),
    ]);

    // Render on the single header line.
    let line_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(line), line_area);
}

// ---------------------------------------------------------------------------
// Compact header (2 lines — used for terminals narrower than 80 cols)
// ---------------------------------------------------------------------------

/// Compact 2-line header for narrow terminals.
///
/// Line 1: logo | sync status | epoch | tip age | uptime
/// Line 2: epoch progress bar
///
/// Uses the [`HeaderBar`] widget so the same rendering logic is shared and
/// all three widget types (HeaderBar, MempoolGauge, SparklineHistory) are
/// exercised in the main rendering path.
fn render_compact_header(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let (_, is_synced, is_stalled) = app.sync_status();
    let pct = app.sync_progress_pct();
    let epoch = app.metrics.get_u64("torsten_epoch_number");
    let tip_age = app.metrics.get_u64("torsten_tip_age_seconds");
    let uptime_secs = app.metrics.get_u64("torsten_uptime_seconds");

    let header = HeaderBar {
        sync_pct: pct,
        is_synced,
        is_stalled,
        epoch,
        tip_age,
        uptime: App::format_uptime(uptime_secs),
        epoch_progress: app.epoch_progress_pct / 100.0,
        connected: app.metrics.connected,
        theme,
    };
    header.render(area, frame.buffer_mut());
}

// ---------------------------------------------------------------------------
// Panel: Node
// ---------------------------------------------------------------------------

fn render_node_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Node", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width < 8 {
        return;
    }

    let role = if app.is_block_producer() {
        "Block Producer"
    } else {
        "Relay"
    };
    let role_color = if app.is_block_producer() {
        theme.warning
    } else {
        theme.info
    };

    let network = app.network.label();
    let version = env!("CARGO_PKG_VERSION");
    let era = app.current_era();
    let uptime_secs = app.metrics.get_u64("torsten_uptime_seconds");
    let uptime = App::format_uptime(uptime_secs);
    // Active peers: hot + warm (hot are active connections, warm are candidates).
    let peers_hot = app.metrics.get_u64("torsten_peers_hot");
    let peers_warm = app.metrics.get_u64("torsten_peers_warm");
    let peers_total = peers_hot + peers_warm;
    let blocks_forged = app.metrics.get_u64("torsten_blocks_forged_total");

    let col_w = inner.width.saturating_sub(2) as usize; // subtract 1-char side padding each side

    // All rows available for text content (sync progress shown in header).
    let text_row_count = inner.height as usize;

    let mut lines = vec![
        kv_aligned("Role", role, role_color, theme, col_w),
        kv_aligned("Network", network, theme.accent, theme, col_w),
        kv_aligned(
            "Version",
            format!("v{}", version),
            theme.muted,
            theme,
            col_w,
        ),
        kv_aligned("Era", era, theme.info, theme, col_w),
        kv_aligned("Uptime", &uptime, theme.fg, theme, col_w),
        kv_aligned(
            "Peers",
            App::format_number(peers_total),
            if peers_total > 0 {
                theme.success
            } else {
                theme.error
            },
            theme,
            col_w,
        ),
        kv_aligned(
            "Blocks Forged",
            App::format_number(blocks_forged),
            if blocks_forged > 0 {
                theme.warning
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
    ];

    lines.truncate(text_row_count);
    let text_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: text_row_count as u16,
    };
    frame.render_widget(Paragraph::new(lines), text_area);
}

// ---------------------------------------------------------------------------
// Panel: Chain
// ---------------------------------------------------------------------------

fn render_chain_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Chain", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let (_, is_synced, is_stalled) = app.sync_status();

    // Row 0: epoch progress bar (only here — NOT duplicated in header).
    let bar_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(
        EpochProgress::new(
            app.slot_in_epoch,
            app.epoch_length(),
            app.epoch_time_remaining_secs,
        )
        .with_epoch(app.metrics.get_u64("torsten_epoch_number"))
        .with_fill_color(if is_synced {
            theme.success
        } else if is_stalled {
            theme.error
        } else {
            theme.gauge_fill
        }),
        bar_area,
    );

    // Remaining lines start at y + 1 (below the bar).
    if inner.height < 2 {
        return;
    }
    let rest = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height - 1,
    };

    let block_num = App::format_number(app.metrics.get_u64("torsten_block_number"));
    let slot_num = App::format_number(app.metrics.get_u64("torsten_slot_number"));
    let tip_age = app.metrics.get_u64("torsten_tip_age_seconds");
    let total_tx = App::format_number(app.metrics.get_u64("torsten_transactions_received_total"));
    let pending_tx = app.metrics.get_u64("torsten_mempool_tx_count");
    let utxo_count = app.metrics.get_u64("torsten_utxo_count");
    let density = app.metrics.get("torsten_chain_density");
    let forks = app.metrics.get_u64("torsten_rollback_count_total");

    let density_str = if density > 0.0 {
        format!("{:.4}", density)
    } else {
        "--".to_string()
    };

    let col_w = inner.width.saturating_sub(2) as usize;

    // Tip age with indicator icon (<20s green check, 20-60s warning, >60s red X).
    let (tip_icon, tip_age_col) = tip_age_indicator(theme, tip_age);
    let tip_str = if tip_age == 0 {
        format!("{} --", tip_icon)
    } else {
        format!("{} {}s", tip_icon, App::format_number(tip_age))
    };

    // Compute how many text rows we can fit before the mempool gauge row.
    // The gauge needs 1 row; the remaining height goes to text rows.
    let text_row_height = rest.height.saturating_sub(1) as usize;

    let mut lines = vec![
        kv_aligned("Block", &block_num, theme.fg, theme, col_w),
        kv_aligned("Slot", &slot_num, theme.fg, theme, col_w),
        kv_aligned(
            "Slot/Epoch",
            App::format_number(app.slot_in_epoch),
            theme.muted,
            theme,
            col_w,
        ),
        kv_line_custom_value(
            "Tip Diff",
            &tip_str,
            tip_age_col,
            theme,
            col_w,
            tip_age >= 60,
        ),
        kv_aligned("Density", &density_str, theme.info, theme, col_w),
        kv_aligned(
            "Forks",
            App::format_number(forks),
            if forks > 0 {
                theme.warning
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        kv_aligned("Total Tx", &total_tx, theme.muted, theme, col_w),
        kv_aligned(
            "UTxO Set",
            App::format_number(utxo_count),
            theme.info,
            theme,
            col_w,
        ),
    ];

    // Truncate text rows to the height available above the gauge row.
    lines.truncate(text_row_height);

    // Render text rows.
    let text_area = Rect {
        x: rest.x,
        y: rest.y,
        width: rest.width,
        height: text_row_height as u16,
    };
    frame.render_widget(Paragraph::new(lines), text_area);

    // Mempool gauge — full-width bar on the last row inside the chain panel.
    // Shows pending tx count relative to the configurable cap (default 4,000 txs).
    // If the node publishes a `torsten_mempool_tx_max` gauge, use it for scaling.
    if rest.height >= 1 {
        let gauge_y = rest.y + text_row_height as u16;
        if gauge_y < rest.y + rest.height {
            let gauge_area = Rect {
                x: rest.x,
                y: gauge_y,
                width: rest.width,
                height: 1,
            };
            let mempool_max = app.metrics.get_u64("torsten_mempool_tx_max");
            let gauge = MempoolGauge::new(pending_tx, theme);
            let gauge = if mempool_max > 0 {
                gauge.with_max(mempool_max)
            } else {
                gauge
            };
            gauge.render(gauge_area, frame.buffer_mut());
        }
    }
}

// ---------------------------------------------------------------------------
// Panel: Connections
// ---------------------------------------------------------------------------

fn render_connections_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Connections", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width < 4 {
        return;
    }

    let inbound = app.metrics.get_u64("torsten_peers_inbound");
    let outbound = app.metrics.get_u64("torsten_peers_outbound");
    let cold = app.metrics.get_u64("torsten_peers_cold");
    let warm = app.metrics.get_u64("torsten_peers_warm");
    let hot = app.metrics.get_u64("torsten_peers_hot");
    let unidir = app.metrics.get_u64("torsten_peers_unidirectional");
    let bidir = app.metrics.get_u64("torsten_peers_bidirectional");
    // Duplex count — metric exists but may be 0 (not yet populated); show 0 explicitly.
    let duplex = app.metrics.get_u64("torsten_peers_duplex");

    let p2p_enabled = outbound > 0 || inbound > 0 || cold > 0 || warm > 0 || hot > 0;
    let p2p_color = if p2p_enabled {
        theme.success
    } else {
        theme.error
    };
    let p2p_label = if p2p_enabled { "Enabled" } else { "Disabled" };

    let col_w = inner.width.saturating_sub(2) as usize;

    // Cold / Warm / Hot compact row.
    let cwh_line = peer_state_row(
        &[
            ("Cold", cold, theme.muted),
            ("Warm", warm, theme.warning),
            ("Hot", hot, theme.success),
        ],
        theme,
    );

    // Uni / Bi / Duplex compact row.
    // Duplex is shown as 0 (not N/A) since the metric is defined but not yet populated.
    let ubd_line = peer_state_row(
        &[
            ("Uni", unidir, theme.muted),
            ("Bi", bidir, theme.info),
            ("Dpx", duplex, theme.accent),
        ],
        theme,
    );

    let mut lines = vec![
        kv_aligned("P2P", p2p_label, p2p_color, theme, col_w),
        kv_aligned(
            "Inbound",
            App::format_number(inbound),
            theme.info,
            theme,
            col_w,
        ),
        kv_aligned(
            "Outbound",
            App::format_number(outbound),
            theme.info,
            theme,
            col_w,
        ),
        cwh_line,
        ubd_line,
    ];

    // Trim to fit available height.
    lines.truncate(inner.height as usize);

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Panel: Resources
// ---------------------------------------------------------------------------

fn render_resources_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Resources", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width < 4 {
        return;
    }

    let cpu_pct = app.metrics.get("torsten_cpu_percent");
    let mem_live = app.metrics.get_u64("torsten_mem_resident_bytes");
    let mem_rss = app.metrics.get_u64("torsten_mem_rss_bytes");
    // Fallback: use live if RSS is unavailable.
    let mem_rss = if mem_rss > 0 { mem_rss } else { mem_live };

    let cpu_color = if cpu_pct > 80.0 {
        theme.error
    } else if cpu_pct > 50.0 {
        theme.warning
    } else {
        theme.success
    };

    let mem_color = if mem_live > 8_000_000_000 {
        theme.error
    } else if mem_live > 4_000_000_000 {
        theme.warning
    } else {
        theme.success
    };

    let col_w = inner.width.saturating_sub(2) as usize;

    // Inline mini CPU bar (max 20 chars, placed on the CPU row as a suffix).
    let bar_w = col_w.saturating_sub(LABEL_W + VALUE_W + 2).min(20);
    let cpu_bar = if bar_w >= 4 {
        build_mini_bar(cpu_pct / 100.0, bar_w, cpu_color, theme)
    } else {
        vec![]
    };

    let cpu_val = format!("{:.1}%", cpu_pct);

    let mut lines: Vec<Line> = Vec::new();

    // CPU row with optional inline bar.
    let mut cpu_spans = vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            format!("{:<width$}", "CPU", width = LABEL_W),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            format!("{:>width$}", cpu_val, width = VALUE_W),
            Style::default().fg(cpu_color).add_modifier(Modifier::BOLD),
        ),
    ];
    if !cpu_bar.is_empty() {
        cpu_spans.push(Span::raw("  "));
        cpu_spans.extend(cpu_bar);
    }
    lines.push(Line::from(cpu_spans));

    // Memory (live/resident) row — human readable bytes with thousands separators.
    lines.push(kv_aligned(
        "Mem (live)",
        App::format_bytes(mem_live),
        mem_color,
        theme,
        col_w,
    ));

    // Memory (RSS) row.
    lines.push(kv_aligned(
        "Mem (RSS)",
        App::format_bytes(mem_rss),
        theme.muted,
        theme,
        col_w,
    ));

    // Inline mem bar showing live/RSS ratio.
    let mem_ratio = if mem_rss > 0 {
        (mem_live as f64 / mem_rss as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mem_bar_w = col_w.min(inner.width.saturating_sub(4) as usize);
    if mem_bar_w >= 8 && inner.height >= 4 {
        let mem_bar = build_smooth_bar(mem_ratio, mem_bar_w, mem_color, theme);
        lines.push(Line::from([vec![Span::raw(" ")], mem_bar].concat()));
    }

    // Block rate sparkline — occupies the last available row in the Resources panel.
    // Rendered directly to the buffer so it overlays the correct row without
    // interfering with Paragraph layout.
    let text_rows = lines.len();
    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);

    // Only draw the sparkline if there is at least one row left after the text rows.
    let spark_y = inner.y + text_rows as u16;
    if !app.block_rate_history.is_empty() && spark_y < inner.y + inner.height && inner.width > 2 {
        let spark_area = Rect {
            x: inner.x + 1, // 1-char left indent
            y: spark_y,
            width: inner.width.saturating_sub(2),
            height: 1,
        };
        // Uniform accent color — all bars the same hue; height carries the meaning.
        SparklineHistory::with_color(&app.block_rate_history, theme.accent)
            .render(spark_area, frame.buffer_mut());
    }
}

// ---------------------------------------------------------------------------
// Panel: Peers (RTT breakdown)
// ---------------------------------------------------------------------------

fn render_peers_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Peers / RTT", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width < 4 {
        return;
    }

    let rtt = &app.rtt_bands;
    let total = rtt.band_0_50 + rtt.band_50_100 + rtt.band_100_200 + rtt.band_200_plus;

    // Format RTT value — derived from histogram _sum/_count for accuracy.
    let fmt_rtt = |ms: Option<f64>| -> String {
        match ms {
            Some(v) => format!("{:.0}ms", v),
            None => "--".to_string(),
        }
    };

    // Bar width = full inner width minus 1-char left margin.
    let bar_width = inner.width.saturating_sub(1) as usize;

    let mut lines: Vec<Line> = Vec::new();

    // RTT distribution bar (colored segments per band).
    if total > 0 && bar_width >= 4 {
        let bar = build_rtt_colored_bar(
            rtt.band_0_50,
            rtt.band_50_100,
            rtt.band_100_200,
            rtt.band_200_plus,
            bar_width,
            theme,
        );
        lines.push(Line::from([vec![Span::raw(" ")], bar].concat()));
    } else {
        lines.push(Line::from(Span::styled(
            " (no handshake data yet)",
            Style::default().fg(theme.muted),
        )));
    }

    // Band counts: two rows of two columns each.
    let half = inner.width.saturating_sub(2) / 2;
    let band_rows: &[(&str, u64, Color, &str, u64, Color)] = &[
        (
            "0-50ms",
            rtt.band_0_50,
            theme.success,
            "50-100ms",
            rtt.band_50_100,
            theme.info,
        ),
        (
            "100-200ms",
            rtt.band_100_200,
            theme.warning,
            "200ms+",
            rtt.band_200_plus,
            theme.error,
        ),
    ];

    for (lbl_a, val_a, col_a, lbl_b, val_b, col_b) in band_rows {
        let val_a_str = App::format_number(*val_a);
        let val_b_str = App::format_number(*val_b);
        let half_w = half as usize;

        // Left cell: label left-aligned, count right-aligned within half width.
        let left_label_w = half_w.saturating_sub(5); // reserve 5 for value
        let left = format!(" {:<lw$}{:>5}", lbl_a, val_a_str, lw = left_label_w.max(1));
        // Right cell: label left-aligned, count right-aligned.
        let right_label_w = (inner.width.saturating_sub(2) as usize)
            .saturating_sub(half_w)
            .saturating_sub(5);
        let right = format!(
            "  {:<rw$}{:>5}",
            lbl_b,
            val_b_str,
            rw = right_label_w.max(1)
        );

        lines.push(Line::from(vec![
            Span::styled(
                left,
                Style::default().fg(*col_a).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                right,
                Style::default().fg(*col_b).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    // Min / Avg / Max RTT line.
    // Avg is computed from histogram _sum / _count for precision.
    let low_str = fmt_rtt(rtt.min_ms);
    let avg_str = fmt_rtt(rtt.avg_ms);
    let high_str = fmt_rtt(rtt.max_ms);

    lines.push(Line::from(vec![
        Span::styled(" Low ", Style::default().fg(theme.muted)),
        Span::styled(
            &low_str,
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("   Avg ", Style::default().fg(theme.muted)),
        Span::styled(
            &avg_str,
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   High ", Style::default().fg(theme.muted)),
        Span::styled(
            &high_str,
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

fn render_footer(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let theme_name = app.theme().name;

    let line = Line::from(vec![
        Span::raw(" "),
        key_span("[q]", theme),
        Span::styled(" Quit  ", Style::default().fg(theme.muted)),
        key_span("[t]", theme),
        Span::styled(" Theme  ", Style::default().fg(theme.muted)),
        key_span("[r]", theme),
        Span::styled(" Refresh  ", Style::default().fg(theme.muted)),
        key_span("[h]", theme),
        Span::styled(" Help  ", Style::default().fg(theme.muted)),
        Span::styled("\u{2502} ", Style::default().fg(theme.border)),
        // Theme pill with colored background.
        Span::styled(
            format!(" {} ", theme_name),
            Style::default().fg(theme.bg).bg(theme.accent),
        ),
        Span::styled("  \u{2502}  ", Style::default().fg(theme.border)),
        Span::styled(
            "torsten-tui",
            Style::default().fg(Color::Rgb(100, 100, 120)),
        ),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

// ---------------------------------------------------------------------------
// Help overlay
// ---------------------------------------------------------------------------

fn render_help_overlay(frame: &mut Frame, theme: &Theme, area: Rect) {
    let overlay_width: u16 = 60;
    let overlay_height: u16 = 20;

    let x = area.x + area.width.saturating_sub(overlay_width) / 2;
    let y = area.y + area.height.saturating_sub(overlay_height) / 2;
    let overlay_area = Rect::new(
        x,
        y,
        overlay_width.min(area.width),
        overlay_height.min(area.height),
    );

    frame.render_widget(Clear, overlay_area);

    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_active))
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
        help_kv("q / Esc", "Quit", theme),
        help_kv("t", "Cycle theme (7 built-in themes)", theme),
        help_kv("r", "Force-refresh metrics now", theme),
        help_kv("h / ?", "Toggle this help overlay", theme),
        Line::default(),
        Line::from(Span::styled(
            "Panels: Node | Chain | Connections | Resources | Peers",
            Style::default().fg(theme.muted),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Tip indicators: check <20s  ! 20-60s  X >60s",
            Style::default().fg(theme.muted),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Metrics polled every 2 seconds (Prometheus :12798).",
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

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

/// Create a bordered panel block with a styled title.
fn panel_block<'a>(title: &'a str, theme: &'a Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                title,
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
        ]))
        // 1-char horizontal padding; vertical padding handled by row content.
        .padding(Padding::new(1, 1, 0, 0))
}

// ---------------------------------------------------------------------------
// Key-value row helpers
// ---------------------------------------------------------------------------

/// Build a label-left / value-right aligned line within a panel.
///
/// The label is left-aligned in a `LABEL_W`-wide column; the value is
/// right-aligned in a `VALUE_W`-wide column.  Total line width = 1 (indent)
/// + LABEL_W + VALUE_W.  If `col_w` is wider, any slack goes to the label.
fn kv_aligned(
    label: &str,
    value: impl Into<String>,
    value_color: Color,
    theme: &Theme,
    col_w: usize,
) -> Line<'static> {
    let label_w = col_w.saturating_sub(VALUE_W).max(LABEL_W);
    let value_w = col_w.saturating_sub(label_w).max(1);
    let label_s = format!("{:<label_w$}", label);
    let value_s = format!("{:>value_w$}", value.into());

    Line::from(vec![
        Span::raw(" "),
        Span::styled(label_s, Style::default().fg(theme.muted)),
        Span::styled(
            value_s,
            Style::default()
                .fg(value_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Build a key-value line with optional BOLD modifier on the value.
///
/// Used for tip-diff where the value already contains the indicator icon and
/// we want full control over the bold flag.
fn kv_line_custom_value(
    label: &str,
    value: &str,
    value_color: Color,
    theme: &Theme,
    col_w: usize,
    bold: bool,
) -> Line<'static> {
    let label_w = col_w.saturating_sub(VALUE_W).max(LABEL_W);
    let value_w = col_w.saturating_sub(label_w).max(1);
    let label_s = format!("{:<label_w$}", label);
    let value_s = format!("{:>value_w$}", value);

    let val_style = if bold {
        Style::default()
            .fg(value_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(value_color)
    };

    Line::from(vec![
        Span::raw(" "),
        Span::styled(label_s, Style::default().fg(theme.muted)),
        Span::styled(value_s, val_style),
    ])
}

/// Compact two-column peer-state row for Cold/Warm/Hot or Uni/Bi/Duplex.
fn peer_state_row(items: &[(&str, u64, Color)], theme: &Theme) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
    for (i, (label, value, color)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::default().fg(theme.muted)));
        }
        // Convert label to owned String so the span is 'static.
        spans.push(Span::styled(
            label.to_string(),
            Style::default().fg(theme.muted),
        ));
        spans.push(Span::raw(" "));
        // Use thousands separators for peer counts.
        spans.push(Span::styled(
            format!("{:>4}", App::format_number(*value)),
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

/// Build a styled key `[x]` span for the footer.
fn key_span<'a>(text: &'a str, theme: &'a Theme) -> Span<'a> {
    Span::styled(
        text,
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )
}

/// Render a help overlay key-binding row.
fn help_kv<'a>(key: &'a str, desc: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{:>12}", key),
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(desc, Style::default().fg(theme.muted)),
    ])
}

// ---------------------------------------------------------------------------
// Separator helpers
// ---------------------------------------------------------------------------

/// Vertical bar separator, no surrounding spaces.
fn sep(theme: &Theme) -> Span<'static> {
    Span::styled("\u{2502}", Style::default().fg(theme.border))
}

/// Vertical bar separator with two spaces on each side.
fn sep_spaced(theme: &Theme) -> Span<'static> {
    Span::styled("  \u{2502}  ", Style::default().fg(theme.border))
}

// ---------------------------------------------------------------------------
// Color and indicator helpers
// ---------------------------------------------------------------------------

/// Return (icon_str, color) for tip-age display.
///
/// Thresholds per requirements:
///   - < 20s  => green check indicator
///   - 20-60s => warning indicator
///   - > 60s  => red X indicator
fn tip_age_indicator(theme: &Theme, tip_age_secs: u64) -> (&'static str, Color) {
    if tip_age_secs == 0 {
        // No data yet — neutral display.
        ("-", theme.muted)
    } else if tip_age_secs < 20 {
        ("OK", theme.success)
    } else if tip_age_secs < 60 {
        ("!!", theme.warning)
    } else {
        ("XX", theme.error)
    }
}

/// Format tip age with a compact unit suffix.
fn format_tip_age(secs: u64) -> String {
    if secs == 0 {
        "--".to_string()
    } else if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

// ---------------------------------------------------------------------------
// Bar / sparkline helpers
// ---------------------------------------------------------------------------

/// Build a smooth progress bar using the 8-shade block-character set.
///
/// Returns a `Vec<Span>` to be composed into a `Line`.
fn build_smooth_bar<'a>(ratio: f64, width: usize, fill: Color, theme: &Theme) -> Vec<Span<'a>> {
    if width == 0 {
        return vec![];
    }
    let ratio = ratio.clamp(0.0, 1.0);
    let total_eighths = (ratio * width as f64 * 8.0).round() as usize;
    let full_blocks = total_eighths / 8;
    let partial_eighths = total_eighths % 8;
    let empty = width.saturating_sub(full_blocks + if partial_eighths > 0 { 1 } else { 0 });

    let mut s = String::with_capacity(width);
    for _ in 0..full_blocks.min(width) {
        s.push(SMOOTH_BLOCKS[8]);
    }
    if partial_eighths > 0 && full_blocks < width {
        s.push(SMOOTH_BLOCKS[partial_eighths]);
    }
    for _ in 0..empty {
        s.push(SMOOTH_BLOCKS[0]);
    }

    // Split by character count (not byte index) since block chars are multi-byte UTF-8.
    let filled_chars = full_blocks.min(width)
        + if partial_eighths > 0 { 1 } else { 0 }.min(width.saturating_sub(full_blocks));
    let filled: String = s.chars().take(filled_chars).collect();
    let empty_str: String = s.chars().skip(filled_chars).collect();
    vec![
        Span::styled(filled, Style::default().fg(fill)),
        Span::styled(empty_str, Style::default().fg(theme.gauge_empty)),
    ]
}

/// Build a minimal inline CPU/mem bar (simpler, for single-row use).
fn build_mini_bar<'a>(ratio: f64, width: usize, fill: Color, theme: &Theme) -> Vec<Span<'a>> {
    if width < 2 {
        return vec![];
    }
    let ratio = ratio.clamp(0.0, 1.0);
    let filled = ((ratio * width as f64) as usize).min(width);
    let empty = width.saturating_sub(filled);

    vec![
        Span::styled("\u{2588}".repeat(filled), Style::default().fg(fill)),
        Span::styled(
            "\u{2591}".repeat(empty),
            Style::default().fg(theme.gauge_empty),
        ),
    ]
}

/// Build a colored RTT distribution bar using distinct colors per band.
///
/// Each band is drawn as a contiguous run of full-block characters,
/// colored by latency severity (success/info/warning/error).
fn build_rtt_colored_bar<'a>(
    b0: u64,
    b1: u64,
    b2: u64,
    b3: u64,
    width: usize,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let total = b0 + b1 + b2 + b3;
    if total == 0 || width == 0 {
        return vec![Span::styled(
            "\u{2591}".repeat(width),
            Style::default().fg(theme.gauge_empty),
        )];
    }

    // Compute pixel widths proportionally, distributing any rounding error to b0.
    let w1 = ((b1 as f64 / total as f64) * width as f64).round() as usize;
    let w2 = ((b2 as f64 / total as f64) * width as f64).round() as usize;
    let w3 = ((b3 as f64 / total as f64) * width as f64).round() as usize;
    let w0 = width.saturating_sub(w1 + w2 + w3);

    // Full block character — uniformly solid, color carries the meaning.
    let ch = '\u{2588}';
    let bands = [
        (w0, theme.success),
        (w1, theme.info),
        (w2, theme.warning),
        (w3, theme.error),
    ];

    bands
        .iter()
        .filter(|(w, _)| *w > 0)
        .map(|(w, color)| Span::styled(ch.to_string().repeat(*w), Style::default().fg(*color)))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tip_age() {
        assert_eq!(format_tip_age(0), "--");
        assert_eq!(format_tip_age(5), "5s");
        assert_eq!(format_tip_age(65), "1m 5s");
        assert_eq!(format_tip_age(3661), "1h 1m");
    }

    #[test]
    fn test_tip_age_indicator_thresholds() {
        let theme = &crate::theme::THEME_MONOKAI;
        // <20s => success (green check)
        let (icon_0, col_0) = tip_age_indicator(theme, 0);
        assert_eq!(icon_0, "-");
        assert_eq!(col_0, theme.muted);

        let (icon_5, col_5) = tip_age_indicator(theme, 5);
        assert_eq!(icon_5, "OK");
        assert_eq!(col_5, theme.success);

        let (icon_19, col_19) = tip_age_indicator(theme, 19);
        assert_eq!(icon_19, "OK");
        assert_eq!(col_19, theme.success);

        // 20-60s => warning
        let (icon_20, col_20) = tip_age_indicator(theme, 20);
        assert_eq!(icon_20, "!!");
        assert_eq!(col_20, theme.warning);

        let (icon_59, col_59) = tip_age_indicator(theme, 59);
        assert_eq!(icon_59, "!!");
        assert_eq!(col_59, theme.warning);

        // >60s => error (red X)
        let (icon_60, col_60) = tip_age_indicator(theme, 60);
        assert_eq!(icon_60, "XX");
        assert_eq!(col_60, theme.error);

        let (icon_300, col_300) = tip_age_indicator(theme, 300);
        assert_eq!(icon_300, "XX");
        assert_eq!(col_300, theme.error);
    }

    #[test]
    fn test_build_rtt_colored_bar_zero_total() {
        let theme = &crate::theme::THEME_DEFAULT;
        let spans = build_rtt_colored_bar(0, 0, 0, 0, 40, theme);
        // Should return a single empty-fill span.
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn test_build_rtt_colored_bar_all_same() {
        let theme = &crate::theme::THEME_DEFAULT;
        let spans = build_rtt_colored_bar(10, 10, 10, 10, 40, theme);
        // Each band gets ~10 chars; all four bands present.
        let total_chars: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(total_chars, 40);
    }

    #[test]
    fn test_build_smooth_bar_empty() {
        let theme = &crate::theme::THEME_DEFAULT;
        let spans = build_smooth_bar(0.0, 0, theme.success, theme);
        assert!(spans.is_empty());
    }

    #[test]
    fn test_build_mini_bar_full() {
        let theme = &crate::theme::THEME_DEFAULT;
        let spans = build_mini_bar(1.0, 10, theme.success, theme);
        let filled: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(filled, 10);
    }

    #[test]
    fn test_kv_aligned_right_aligns_value() {
        let theme = &crate::theme::THEME_DEFAULT;
        let line = kv_aligned("Block", "4,109,330", theme.fg, theme, 30);
        // Verify the line has exactly 3 spans: indent + label + value.
        assert_eq!(line.spans.len(), 3);
        // The value span should contain the formatted number.
        assert!(line.spans[2].content.contains("4,109,330"));
    }

    #[test]
    fn test_monokai_is_default_theme() {
        let app = crate::app::App::new();
        assert_eq!(app.theme().name, "Monokai");
    }
}

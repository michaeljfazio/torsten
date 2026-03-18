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

use crate::app::{App, NodeStatus};
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

/// Width reserved for the value field (used for right-aligning numbers and
/// short strings like "Block Producer", "Waiting (nonce)", "v0.4.4-alpha").
const VALUE_W: usize = 16;

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
    render_governance_panel(frame, app, theme, layout.governance);
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
/// When the node is offline the status pill shows "Node Offline Xm Ys" with
/// the time elapsed since contact was lost.  When data from a previous poll is
/// still available it is shown in the data panels with a stale indicator; the
/// header notes this with a "(stale)" suffix.
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

    let is_offline = app.node_status == NodeStatus::Offline;

    // Status pill — colored background so it reads at a glance.
    let status_bg = if is_offline {
        theme.error
    } else if is_synced {
        theme.success
    } else if is_stalled {
        theme.error
    } else {
        theme.warning
    };

    let status_text = if is_offline {
        // Include time since last contact so the operator knows how long the
        // node has been unreachable.
        match app.offline_duration() {
            Some(dur) => {
                let secs = dur.as_secs();
                if secs < 60 {
                    format!(" Node Offline {}s ", secs)
                } else {
                    format!(" Node Offline {}m {}s ", secs / 60, secs % 60)
                }
            }
            None => " Node Offline ".to_string(),
        }
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

    let mut spans = vec![
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
            status_text,
            Style::default()
                .fg(Color::Black)
                .bg(status_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    // "(stale)" notice — visible only when offline with last-known data.
    if app.is_stale() {
        spans.push(Span::styled(
            " (stale)",
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }

    spans.push(Span::raw(" "));
    spans.push(sep(theme));

    // Epoch, era, network, tip age, uptime — shown with last-known values
    // even when offline so the operator can see the state at last contact.
    spans.extend([
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
            tip_str,
            Style::default()
                .fg(tip_age_col)
                .add_modifier(if tip_age >= 120 {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        sep_spaced(theme),
        // Uptime.
        Span::styled("Up ", Style::default().fg(theme.muted)),
        Span::styled(uptime, Style::default().fg(theme.muted)),
    ]);

    // Render on the single header line.
    let line_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), line_area);
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
        // Use the NodeStatus-derived connectivity flag so that the HeaderBar
        // widget shows "Disconnected" whenever the node cannot be reached,
        // matching the behaviour of the standard (wide) header pill.
        connected: app.node_status != NodeStatus::Offline,
        theme,
    };
    header.render(area, frame.buffer_mut());
}

// ---------------------------------------------------------------------------
// Panel: Node
// ---------------------------------------------------------------------------

fn render_node_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    // Title: "Node" normally; "Node *" with warning color when stale data is shown.
    let block = if app.is_stale() {
        panel_block_stale("Node", theme)
    } else {
        panel_block("Node", theme)
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width < 8 {
        return;
    }

    let col_w = inner.width.saturating_sub(2) as usize;
    let is_offline = app.node_status == NodeStatus::Offline;

    // Top row: node connectivity status.
    // When offline this is the most important information — show it prominently.
    let (status_str, status_color) = match app.node_status {
        NodeStatus::Online => ("Online", theme.success),
        NodeStatus::Offline => ("Offline", theme.error),
        NodeStatus::Unknown => ("Connecting...", theme.muted),
    };

    let mut lines: Vec<Line> = Vec::new();

    // "Status" row — always the first line.
    lines.push(kv_aligned("Status", status_str, status_color, theme, col_w));

    // When offline: show how long the node has been unreachable and why.
    if is_offline {
        if let Some(dur) = app.offline_duration() {
            let secs = dur.as_secs();
            let offline_str = if secs < 60 {
                format!("{}s ago", secs)
            } else {
                format!("{}m {}s ago", secs / 60, secs % 60)
            };
            lines.push(kv_aligned(
                "Last seen",
                offline_str,
                theme.warning,
                theme,
                col_w,
            ));
        }

        // Show the connection error (truncated to fit the column width).
        if let Some(ref err) = app.last_error {
            // Strip verbose prefix text from reqwest errors to keep it short.
            let short_err = err
                .split(':')
                .next()
                .unwrap_or(err.as_str())
                .trim()
                .to_string();
            // Truncate to available column width minus label.
            let max_err_w = col_w.saturating_sub(LABEL_W + 2);
            let truncated = if short_err.len() > max_err_w && max_err_w > 3 {
                format!("{}...", &short_err[..max_err_w.saturating_sub(3)])
            } else {
                short_err
            };
            lines.push(kv_aligned("Error", truncated, theme.error, theme, col_w));
        }

        // When offline with no prior data, show a helpful waiting message.
        if !app.is_stale() {
            lines.push(Line::from(Span::styled(
                " Waiting for node...",
                Style::default().fg(theme.muted),
            )));
            lines.truncate(inner.height as usize);
            frame.render_widget(Paragraph::new(lines), inner);
            return;
        }

        // When stale: add a visual separator before the last-known data.
        lines.push(Line::from(Span::styled(
            " -- last known values --",
            Style::default().fg(theme.muted),
        )));
    }

    // Node metrics — shown with live values when online, stale values when offline.
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
    let peers_hot = app.metrics.get_u64("torsten_peers_hot");
    let peers_warm = app.metrics.get_u64("torsten_peers_warm");
    let peers_total = peers_hot + peers_warm;
    let blocks_forged = app.metrics.get_u64("torsten_blocks_forged_total");

    // When no data has ever been received, use placeholder dashes rather than zeros.
    let no_data = !app.has_data();

    lines.push(kv_aligned("Role", role, role_color, theme, col_w));

    // When running as a block producer, show the abbreviated pool ID and
    // forge activity status directly in the Node panel.
    if app.is_block_producer() {
        if let Some(abbrev) = app.pool_id_abbrev() {
            lines.push(kv_aligned("Pool", abbrev, theme.warning, theme, col_w));
        }
        // "Forge" status: show "Active" when the node has performed at least one
        // leader check (indicating the forge loop is running) and "Waiting" when
        // the leader-check counter is still zero (startup / nonce not established).
        let leader_checks = app.metrics.get_u64("torsten_leader_checks_total");
        let forge_failures = app.metrics.get_u64("torsten_forge_failures_total");
        let forge_label = if no_data {
            "--".to_string()
        } else if forge_failures > 0 {
            format!("Active ({} err)", forge_failures)
        } else if leader_checks > 0 {
            "Active".to_string()
        } else {
            "Waiting (nonce)".to_string()
        };
        let forge_color = if no_data {
            theme.muted
        } else if forge_failures > 0 {
            theme.error
        } else if leader_checks > 0 {
            theme.success
        } else {
            theme.warning
        };
        lines.push(kv_aligned("Forge", forge_label, forge_color, theme, col_w));
    }

    lines.push(kv_aligned("Network", network, theme.accent, theme, col_w));
    lines.push(kv_aligned(
        "Version",
        format!("v{}", version),
        theme.muted,
        theme,
        col_w,
    ));
    lines.push(kv_aligned("Era", era, theme.info, theme, col_w));
    lines.push(kv_aligned(
        "Uptime",
        if no_data { "--".to_string() } else { uptime },
        theme.fg,
        theme,
        col_w,
    ));
    lines.push(kv_aligned(
        "Peers",
        if no_data {
            "--".to_string()
        } else {
            App::format_number(peers_total)
        },
        if no_data || peers_total == 0 {
            theme.error
        } else {
            theme.success
        },
        theme,
        col_w,
    ));
    lines.push(kv_aligned(
        "Blocks Forged",
        if no_data {
            "--".to_string()
        } else {
            App::format_number(blocks_forged)
        },
        if blocks_forged > 0 {
            theme.warning
        } else {
            theme.muted
        },
        theme,
        col_w,
    ));

    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);
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
    // Prefer the dedicated density gauge; fall back to block_number / slot_number
    // for nodes that do not (yet) publish torsten_chain_density.
    let density = {
        let d = app.metrics.get("torsten_chain_density");
        if d > 0.0 {
            d
        } else {
            let slot = app.metrics.get_u64("torsten_slot_number");
            if slot > 0 {
                app.metrics.get_u64("torsten_block_number") as f64 / slot as f64
            } else {
                0.0
            }
        }
    };
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
            tip_age >= 120,
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
    // Unidirectional = outbound-only connections (we dialled, they have not connected back).
    // Bidirectional  = inbound-only connections (they dialled us).
    // Duplex         = outbound + inbound (total peers with bidirectional capability);
    //                  populated by the node when running in InitiatorAndResponder mode.
    let unidir = app.metrics.get_u64("torsten_peers_outbound");
    let bidir = app.metrics.get_u64("torsten_peers_inbound");
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
            ("Cold:", cold, theme.muted),
            ("Warm:", warm, theme.warning),
            ("Hot:", hot, theme.success),
        ],
        theme,
    );

    let ubd_line = peer_state_row(
        &[
            ("Uni:", unidir, theme.muted),
            ("Bi:", bidir, theme.info),
            ("Duplex:", duplex, theme.accent),
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
    // Total system physical memory — used to show RSS as a % of total RAM.
    let mem_total = app.metrics.get_u64("torsten_mem_total_bytes");

    let cpu_color = if cpu_pct > 80.0 {
        theme.error
    } else if cpu_pct > 50.0 {
        theme.warning
    } else {
        theme.success
    };

    // Compute memory color based on fraction of system total when available,
    // or fall back to absolute thresholds if total is not published.
    let mem_pct_of_total = if mem_total > 0 {
        mem_live as f64 / mem_total as f64
    } else {
        0.0
    };
    let mem_color = if mem_pct_of_total > 0.8 || mem_live > 8_000_000_000 {
        theme.error
    } else if mem_pct_of_total > 0.5 || mem_live > 4_000_000_000 {
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

    // Memory bar: show RSS as a percentage of total system RAM when the
    // `torsten_mem_total_bytes` gauge is available.  Fall back to the live/RSS
    // ratio (an intra-process metric) when total RAM is unknown.
    //
    // The bar is rendered in the theme's `mem_color` (green/warning/error
    // depending on how much RAM the node is consuming), which is visually
    // distinct from the muted `gauge_empty` background.  A compact percentage
    // label to the left of the bar makes the value legible at a glance.
    let (mem_ratio, mem_pct_label) = if mem_total > 0 {
        let ratio = (mem_live as f64 / mem_total as f64).clamp(0.0, 1.0);
        (ratio, format!("{:.1}%", ratio * 100.0))
    } else if mem_rss > 0 {
        let ratio = (mem_live as f64 / mem_rss as f64).clamp(0.0, 1.0);
        (ratio, String::new())
    } else {
        (0.0, String::new())
    };
    // Reserve 6 chars for the percentage label (e.g. " 12.3%") when available.
    let pct_label_w = if mem_pct_label.is_empty() { 0 } else { 6 };
    let mem_bar_w = col_w
        .saturating_sub(pct_label_w + 1)
        .min(inner.width.saturating_sub(4) as usize);
    if mem_bar_w >= 4 && inner.height >= 4 {
        let mem_bar = build_smooth_bar(mem_ratio, mem_bar_w, mem_color, theme);
        let prefix = if mem_pct_label.is_empty() {
            vec![Span::raw(" ")]
        } else {
            vec![
                Span::raw(" "),
                Span::styled(
                    format!("{:>5} ", mem_pct_label),
                    Style::default().fg(mem_color).add_modifier(Modifier::BOLD),
                ),
            ]
        };
        lines.push(Line::from([prefix, mem_bar].concat()));
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

    // Per-band histogram rows: each band gets its own row with a proportional
    // bar so the operator can see at a glance which latency buckets are most
    // populated.  Bar width is `count / max_count * bar_max_w`.
    //
    // Layout per row:  " LABEL  COUNT  [████████░░░░░░░░]"
    //   - label: 9 chars left-aligned
    //   - count: 4 chars right-aligned
    //   - bar:   remaining width minus 1-char margin
    // Single compact row with all RTT bands:
    //  0-50:5 | 50-100:0 | 100-200:0 | 200+:0
    let band_items: &[(&str, u64, Color)] = &[
        ("0-50", rtt.band_0_50, theme.success),
        ("50-100", rtt.band_50_100, theme.info),
        ("100-200", rtt.band_100_200, theme.warning),
        ("200+", rtt.band_200_plus, theme.error),
    ];
    let sep = Span::styled(" | ", Style::default().fg(theme.muted));
    let mut band_spans = vec![Span::raw(" ")];
    for (i, (label, count, color)) in band_items.iter().enumerate() {
        if i > 0 {
            band_spans.push(sep.clone());
        }
        band_spans.push(Span::styled(
            format!("{label}:"),
            Style::default().fg(theme.muted),
        ));
        band_spans.push(Span::styled(
            format!("{count}"),
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(band_spans));

    // Single horizontal RTT breakpoint row:
    //   min: 12ms | avg: 45ms | p50: 38ms | p95: 120ms | max: 250ms
    //
    // All five values sit on one line, separated by a muted ` | ` divider,
    // so the panel row is not wasted on vertical stacking.
    let min_str = fmt_rtt(rtt.min_ms);
    let avg_str = fmt_rtt(rtt.avg_ms);
    let p50_str = fmt_rtt(rtt.p50_ms);
    let p95_str = fmt_rtt(rtt.p95_ms);
    let max_str = fmt_rtt(rtt.max_ms);

    let sep = Span::styled(" | ", Style::default().fg(theme.muted));
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("min:", Style::default().fg(theme.muted)),
        Span::styled(
            min_str,
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        Span::styled("avg:", Style::default().fg(theme.muted)),
        Span::styled(
            avg_str,
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        Span::styled("p50:", Style::default().fg(theme.muted)),
        Span::styled(
            p50_str,
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        Span::styled("p95:", Style::default().fg(theme.muted)),
        Span::styled(
            p95_str,
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        Span::styled("max:", Style::default().fg(theme.muted)),
        Span::styled(
            max_str,
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Panel: Governance
// ---------------------------------------------------------------------------

/// Governance panel: DReps, stake pools, active proposals, treasury balance, delegations.
///
/// Displays a concise summary of on-chain governance state sourced from the
/// Prometheus metrics endpoint.  All values are read-only; the panel never
/// modifies node state.
fn render_governance_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Governance", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width < 8 {
        return;
    }

    let drep_count = app.metrics.get_u64("torsten_drep_count");
    let pool_count = app.metrics.get_u64("torsten_pool_count");
    let proposal_count = app.metrics.get_u64("torsten_proposal_count");
    let delegation_count = app.metrics.get_u64("torsten_delegation_count");
    // Treasury is stored in lovelace; convert to ADA for display.
    // The formatted value includes " ADA" suffix and thousands separators, e.g.
    // "14,074,169 ADA" (14 chars) — wider than the default VALUE_W of 12, so
    // we use a wider value column (16) to avoid truncation on a typical panel width.
    let treasury_lovelace = app.metrics.get_u64("torsten_treasury_lovelace");
    let treasury_ada = treasury_lovelace / 1_000_000;
    let treasury_str = format!("{} ADA", App::format_number(treasury_ada));

    let col_w = inner.width.saturating_sub(2) as usize;
    // Treasury value can be long (e.g. "14,074,169 ADA" = 14 chars + spaces).
    // Use a wider value column (16 chars) so the number is never clipped.
    const TREASURY_VALUE_W: usize = 22;
    let treasury_label_w = col_w.saturating_sub(TREASURY_VALUE_W).max(LABEL_W);
    let treasury_value_w = col_w.saturating_sub(treasury_label_w).max(1);

    let mut lines = vec![
        kv_aligned(
            "DReps",
            App::format_number(drep_count),
            if drep_count > 0 {
                theme.info
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        kv_aligned(
            "Pools",
            App::format_number(pool_count),
            if pool_count > 0 {
                theme.info
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        kv_aligned(
            "Proposals",
            App::format_number(proposal_count),
            if proposal_count > 0 {
                theme.warning
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        // Treasury uses a dedicated wider value column to avoid truncation of
        // large ADA values (e.g. "14,074,169 ADA").
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("{:<label_w$}", "Treasury", label_w = treasury_label_w),
                Style::default().fg(theme.muted),
            ),
            Span::styled(
                format!("{:>value_w$}", treasury_str, value_w = treasury_value_w),
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        kv_aligned(
            "Delegations",
            App::format_number(delegation_count),
            theme.fg,
            theme,
            col_w,
        ),
    ];

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
            "Tip indicators: OK <60s  !! 60-120s  XX >120s",
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

/// Create a bordered panel block whose title includes a stale-data indicator.
///
/// Used when the panel is displaying last-known values because the node is
/// currently unreachable.  The `*` suffix on the title is rendered in the
/// warning color so the operator can identify stale panels at a glance.
fn panel_block_stale<'a>(title: &'a str, theme: &'a Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                title,
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " *",
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
        ]))
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

/// Compact peer-state row with fixed-width columns for consistent alignment.
/// Each item is rendered as "Label:N" with the label right-padded and value
/// left-padded so columns line up across rows.
fn peer_state_row(items: &[(&str, u64, Color)], theme: &Theme) -> Line<'static> {
    let sep = Span::styled(" | ", Style::default().fg(theme.muted));
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    for (i, (label, value, color)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(sep.clone());
        }
        spans.push(Span::styled(
            format!("{label}"),
            Style::default().fg(theme.muted),
        ));
        spans.push(Span::styled(
            format!("{}", App::format_number(*value)),
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
///   - < 60s    => green check indicator (within one slot interval)
///   - 60-120s  => warning indicator
///   - > 120s   => red X indicator
fn tip_age_indicator(theme: &Theme, tip_age_secs: u64) -> (&'static str, Color) {
    if tip_age_secs == 0 {
        // No data yet — neutral display.
        ("-", theme.muted)
    } else if tip_age_secs < 60 {
        ("OK", theme.success)
    } else if tip_age_secs < 120 {
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
///
/// Pixel widths are assigned proportionally using the largest-remainder method
/// to ensure the total always equals `width` regardless of rounding.
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

    // Largest-remainder allocation: compute exact fractional widths, take the
    // floor of each, then distribute the remaining pixels to the bands with
    // the highest fractional remainders.  This guarantees sum == width and
    // prevents zero-count bands from receiving pixels due to rounding.
    let bands_raw: [(u64, Color); 4] = [
        (b0, theme.success),
        (b1, theme.info),
        (b2, theme.warning),
        (b3, theme.error),
    ];

    // Compute exact floating-point widths.
    let exact: [f64; 4] = bands_raw.map(|(count, _)| (count as f64 / total as f64) * width as f64);

    // Floor allocations.
    let mut floors: [usize; 4] = exact.map(|v| v as usize);
    let allocated: usize = floors.iter().sum();
    let remainder = width.saturating_sub(allocated);

    // Sort indices by fractional part descending and give one extra pixel each.
    let mut remainders: [(usize, f64); 4] = [
        (0, exact[0] - floors[0] as f64),
        (1, exact[1] - floors[1] as f64),
        (2, exact[2] - floors[2] as f64),
        (3, exact[3] - floors[3] as f64),
    ];
    remainders.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for i in 0..remainder {
        floors[remainders[i].0] += 1;
    }

    // Full block character — uniformly solid, color carries the meaning.
    let ch = '\u{2588}';
    bands_raw
        .iter()
        .zip(floors.iter())
        .filter(|(_, &w)| w > 0)
        .map(|((_, color), &w)| Span::styled(ch.to_string().repeat(w), Style::default().fg(*color)))
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
        // 0 => neutral (no data)
        let (icon_0, col_0) = tip_age_indicator(theme, 0);
        assert_eq!(icon_0, "-");
        assert_eq!(col_0, theme.muted);

        // <60s => success (green OK)
        let (icon_5, col_5) = tip_age_indicator(theme, 5);
        assert_eq!(icon_5, "OK");
        assert_eq!(col_5, theme.success);

        let (icon_59, col_59) = tip_age_indicator(theme, 59);
        assert_eq!(icon_59, "OK");
        assert_eq!(col_59, theme.success);

        // 60-120s => warning
        let (icon_60, col_60) = tip_age_indicator(theme, 60);
        assert_eq!(icon_60, "!!");
        assert_eq!(col_60, theme.warning);

        let (icon_119, col_119) = tip_age_indicator(theme, 119);
        assert_eq!(icon_119, "!!");
        assert_eq!(col_119, theme.warning);

        // >=120s => error (red X)
        let (icon_120, col_120) = tip_age_indicator(theme, 120);
        assert_eq!(icon_120, "XX");
        assert_eq!(col_120, theme.error);

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

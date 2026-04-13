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

    let sync_state = app.sync_status();
    let pct = app.sync_progress_pct();
    let epoch = app.metrics.get_u64("dugite_epoch_number");
    let tip_age = app.metrics.get_u64("dugite_tip_age_seconds");
    let uptime_secs = app.metrics.get_u64("dugite_uptime_seconds");
    let uptime = App::format_uptime(uptime_secs);
    let era = app.current_era();
    let network = app.network.label();

    let is_offline = app.node_status == NodeStatus::Offline;

    // Status pill — colored background so it reads at a glance.
    // Replaying uses the same warning colour as Syncing: progress is being
    // made, just from local ImmutableDB rather than from live peers.
    let status_bg = if is_offline {
        theme.error
    } else if sync_state.is_synced() {
        theme.success
    } else if sync_state.is_stalled() {
        theme.error
    } else {
        // Syncing and Replaying both use the warning colour.
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
        format!(" {} {:.2}% ", sync_state.label(), pct)
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
            " Dugite ",
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
/// all three widget types (HeaderBar, MempoolGauge, and sparkline_history) are
/// exercised in the main rendering path.
fn render_compact_header(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let sync_state = app.sync_status();
    let pct = app.sync_progress_pct();
    let epoch = app.metrics.get_u64("dugite_epoch_number");
    let tip_age = app.metrics.get_u64("dugite_tip_age_seconds");
    let uptime_secs = app.metrics.get_u64("dugite_uptime_seconds");

    let header = HeaderBar {
        sync_pct: pct,
        sync_state,
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
    let uptime_secs = app.metrics.get_u64("dugite_uptime_seconds");
    let uptime = App::format_uptime(uptime_secs);
    let blocks_forged = app.metrics.get_u64("dugite_blocks_forged_total");

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
        let leader_checks = app.metrics.get_u64("dugite_leader_checks_total");
        let forge_failures = app.metrics.get_u64("dugite_forge_failures_total");
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
    // Era is shown in the header bar — not duplicated here.
    lines.push(kv_aligned(
        "Uptime",
        if no_data { "--".to_string() } else { uptime },
        theme.fg,
        theme,
        col_w,
    ));
    // Peers are shown in the Connections panel — not duplicated here.
    lines.push(kv_aligned(
        "Blocks Forged",
        if no_data {
            "--".to_string()
        } else {
            App::format_number(blocks_forged)
        },
        if blocks_forged > 0 {
            theme.success
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

    let sync_state = app.sync_status();

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
        .with_epoch(app.metrics.get_u64("dugite_epoch_number"))
        .with_fill_color(if sync_state.is_synced() {
            theme.success
        } else if sync_state.is_stalled() {
            theme.error
        } else if sync_state.is_replaying() {
            // During ImmutableDB replay use the accent colour so the operator
            // can immediately tell at a glance that replay is in progress.
            theme.accent
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

    let block_num = App::format_number(app.metrics.get_u64("dugite_block_number"));
    let slot_num = App::format_number(app.metrics.get_u64("dugite_slot_number"));
    let tip_age = app.metrics.get_u64("dugite_tip_age_seconds");
    // N2C (dugite-cli LocalTxSubmission) counters — user-submitted transactions
    let n2c_submitted = app.metrics.get_u64("dugite_n2c_txs_submitted_total");
    let n2c_accepted = app.metrics.get_u64("dugite_n2c_txs_accepted_total");
    let n2c_rejected = app.metrics.get_u64("dugite_n2c_txs_rejected_total");
    // P2P (TxSubmission from network peers) counters — counts all txs in synced blocks,
    // not comparable to N2C so kept separate and NOT combined.
    let p2p_rejected = app.metrics.get_u64("dugite_transactions_rejected_total");
    let pending_tx = app.metrics.get_u64("dugite_mempool_tx_count");
    let utxo_count = app.metrics.get_u64("dugite_utxo_count");
    // Prefer the dedicated density gauge; fall back to block_number / slot_number
    // for nodes that do not (yet) publish dugite_chain_density.
    let density = {
        let d = app.metrics.get("dugite_chain_density");
        if d > 0.0 {
            d
        } else {
            let slot = app.metrics.get_u64("dugite_slot_number");
            if slot > 0 {
                app.metrics.get_u64("dugite_block_number") as f64 / slot as f64
            } else {
                0.0
            }
        }
    };
    let blocks_received = app.metrics.get_u64("dugite_blocks_received_total");
    let forks = app.metrics.get_u64("dugite_rollback_count_total");

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
            "Blocks Recv",
            App::format_number(blocks_received),
            if blocks_received > 0 {
                theme.success
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
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
        kv_aligned(
            "Tx Submitted",
            App::format_number(n2c_submitted),
            if n2c_submitted > 0 {
                theme.success
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        kv_aligned(
            "Tx Accepted",
            App::format_number(n2c_accepted),
            if n2c_accepted > 0 {
                theme.success
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        kv_aligned(
            "Tx Rejected",
            App::format_number(n2c_rejected + p2p_rejected),
            if n2c_rejected + p2p_rejected > 0 {
                theme.warning
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        kv_aligned(
            "UTxO Set",
            App::format_number(utxo_count),
            theme.info,
            theme,
            col_w,
        ),
        // Slot/Epoch is secondary info — placed last so it's the first dropped
        // when the panel is too short to show all rows.
        kv_aligned(
            "Slot/Epoch",
            App::format_number(app.slot_in_epoch),
            theme.muted,
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
    // Shows pending tx count relative to the configurable cap (default 16,384 txs).
    // If the node publishes a `dugite_mempool_tx_max` gauge, use it for scaling.
    if rest.height >= 1 {
        let gauge_y = rest.y + text_row_height as u16;
        if gauge_y < rest.y + rest.height {
            let gauge_area = Rect {
                x: rest.x,
                y: gauge_y,
                width: rest.width,
                height: 1,
            };
            let mempool_max = app.metrics.get_u64("dugite_mempool_tx_max");
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

    let inbound = app.metrics.get_u64("dugite_peers_inbound");
    let outbound = app.metrics.get_u64("dugite_peers_outbound");
    let duplex = app.metrics.get_u64("dugite_peers_duplex");
    let cold = app.metrics.get_u64("dugite_peers_cold");
    let warm = app.metrics.get_u64("dugite_peers_warm");
    let hot = app.metrics.get_u64("dugite_peers_hot");

    // Connection manager counters (Haskell ConnectionManagerCounters compat).
    // These are computed per-connection via ConnectionState::to_counters(),
    // matching Haskell's connectionStateToCounters exactly.
    let conn_unidirectional = app.metrics.get_u64("dugite_conn_unidirectional");

    // P2P governor always runs (matching Haskell cardano-node). Show
    // diffusion mode instead: InitiatorAndResponder (relay) vs InitiatorOnly (BP).
    let diffusion_initiator_only = app.metrics.get_u64("dugite_diffusion_mode") == 1;
    let p2p_color = theme.success;
    let p2p_label = if diffusion_initiator_only {
        "InitiatorOnly"
    } else {
        "Enabled"
    };

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

    let mut lines = vec![
        kv_aligned("P2P", p2p_label, p2p_color, theme, col_w),
        cwh_line, // Cold/Warm/Hot — shown prominently after P2P status
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
        kv_aligned(
            "Duplex",
            App::format_number(duplex),
            if duplex > 0 {
                theme.accent
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        kv_aligned(
            "Uni-Direct",
            App::format_number(conn_unidirectional),
            if conn_unidirectional > 0 {
                theme.warning
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
    ];

    // Connection manager counters (Haskell ConnectionManagerCounters compat).
    // Always shown — these are now correctly computed from per-connection state.
    let conn_full_duplex = app.metrics.get_u64("dugite_conn_full_duplex");
    let conn_duplex = app.metrics.get_u64("dugite_conn_duplex");
    let conn_inbound = app.metrics.get_u64("dugite_conn_inbound");
    let conn_outbound = app.metrics.get_u64("dugite_conn_outbound");
    let conn_terminating = app.metrics.get_u64("dugite_conn_terminating");
    let has_conn_metrics = conn_full_duplex > 0
        || conn_duplex > 0
        || conn_unidirectional > 0
        || conn_inbound > 0
        || conn_outbound > 0;
    if has_conn_metrics {
        lines.push(kv_aligned(
            "ConnDuplex",
            App::format_number(conn_duplex),
            theme.info,
            theme,
            col_w,
        ));
        lines.push(kv_aligned(
            "ConnFullDpx",
            App::format_number(conn_full_duplex),
            theme.info,
            theme,
            col_w,
        ));
        lines.push(kv_aligned(
            "ConnUniDir",
            App::format_number(conn_unidirectional),
            theme.info,
            theme,
            col_w,
        ));
        lines.push(kv_aligned(
            "ConnIn",
            App::format_number(conn_inbound),
            theme.info,
            theme,
            col_w,
        ));
        lines.push(kv_aligned(
            "ConnOut",
            App::format_number(conn_outbound),
            theme.info,
            theme,
            col_w,
        ));
        if conn_terminating > 0 {
            lines.push(kv_aligned(
                "ConnTerm",
                App::format_number(conn_terminating),
                theme.warning,
                theme,
                col_w,
            ));
        }
    }

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

    // ---- Gather metrics ----

    let cpu_pct = app.metrics.get("dugite_cpu_percent");

    // RSS memory: prefer dugite_mem_rss_bytes, fall back to resident.
    let mem_rss_raw = app.metrics.get_u64("dugite_mem_rss_bytes");
    let mem_resident = app.metrics.get_u64("dugite_mem_resident_bytes");
    let mem_rss = if mem_rss_raw > 0 {
        mem_rss_raw
    } else {
        mem_resident
    };

    // Peak RSS — shown as a secondary row so the operator can see the
    // high-water mark without polling a separate tool.
    let mem_peak = app.metrics.get_u64("dugite_mem_peak_bytes");

    // Total system physical memory — used to compute the memory gauge ratio.
    let mem_total = app.metrics.get_u64("dugite_mem_total_bytes");

    // UTxO set size and peer counts are shown in Chain / Connections panels
    // respectively — not duplicated here.

    // ---- Derive colors ----

    // CPU: green < 50 %, yellow 50–80 %, red > 80 %.
    let cpu_color = if cpu_pct > 80.0 {
        theme.error
    } else if cpu_pct > 50.0 {
        theme.warning
    } else {
        theme.success
    };

    // Memory: color based on fraction of system total (preferred) or absolute
    // byte thresholds when total RAM is not published by the node.
    let mem_pct_of_total = if mem_total > 0 {
        mem_rss as f64 / mem_total as f64
    } else {
        0.0
    };
    let mem_color = if mem_pct_of_total > 0.8 || mem_rss > 8_000_000_000 {
        theme.error
    } else if mem_pct_of_total > 0.5 || mem_rss > 4_000_000_000 {
        theme.warning
    } else {
        theme.success
    };

    // ---- Layout constants ----

    // col_w is the usable width inside the panel (minus the 1-char left indent
    // already added by panel_block padding and the 1-char right margin).
    let col_w = inner.width.saturating_sub(2) as usize;

    // Width reserved for the sparkline that follows the CPU percentage value.
    // We keep at least LABEL_W + VALUE_W columns for the label+value pair, then
    // use whatever space remains (up to 20 columns) for the sparkline.
    let spark_reserve = col_w.saturating_sub(LABEL_W + VALUE_W + 2).min(20);

    let mut lines: Vec<Line> = Vec::new();

    // ---- Row 1: CPU ----
    //
    // "CPU          47.5%  ▃▅▆▇▅▄▃▅" — percentage right-aligned, then a
    // colored sparkline history filling the remaining space.  The sparkline
    // color tracks the current CPU level: green/yellow/red gradient.
    //
    // The history is stored scaled ×10 (integer), so a threshold of 500 = 50 %
    // and 800 = 80 % maps to the same green/yellow/red breakpoints.
    let cpu_val = format!("{:.1}%", cpu_pct);
    let mut cpu_spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("{:<label_w$}", "CPU", label_w = LABEL_W),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            format!("{:>value_w$}", cpu_val, value_w = VALUE_W),
            Style::default().fg(cpu_color).add_modifier(Modifier::BOLD),
        ),
    ];
    // Append the sparkline inline — rendered as a series of colored Unicode
    // block characters.  We use the 3-tier gradient constructor directly here
    // rather than the uniform-color variant so that history bars reflect their
    // own intensity (low=green, mid=yellow, high=red) rather than the
    // *current* CPU level.
    if spark_reserve >= 4 && !app.cpu_pct_history.is_empty() {
        let spark_w = spark_reserve;
        // Determine how many data points fit within the reserved width.
        let start = if app.cpu_pct_history.len() > spark_w {
            app.cpu_pct_history.len() - spark_w
        } else {
            0
        };
        // Max value in the visible window; the gradient thresholds are applied
        // against the *absolute* scale (max 1000 = 100.0 % × 10) so colors are
        // consistent across frames rather than relative to the local window max.
        let abs_max: u64 = 1000; // represents 100.0 %
        let mut spark_str = String::with_capacity(spark_w);
        for &sample in app.cpu_pct_history.iter().skip(start) {
            let level = if sample == 0 {
                0usize
            } else {
                ((sample as f64 / abs_max as f64) * 7.0).round() as usize
            };
            spark_str.push(crate::widgets::sparkline_history::SPARK_CHARS[level.min(7)]);
        }
        // Pad with spaces on the left when fewer samples than reserved width.
        let pad = spark_w.saturating_sub(spark_str.chars().count());
        cpu_spans.push(Span::raw("  "));
        if pad > 0 {
            cpu_spans.push(Span::styled(
                " ".repeat(pad),
                Style::default().fg(theme.muted),
            ));
        }
        cpu_spans.push(Span::styled(spark_str, Style::default().fg(cpu_color)));
    }
    lines.push(Line::from(cpu_spans));

    // ---- Row 2: Memory ----
    //
    // "Memory       1.8 GB  [████░░░░░░]  12.3%"
    //
    // The RSS value is shown right-aligned.  When `dugite_mem_total_bytes` is
    // available, an inline smooth-bar gauge plus a percentage label follows on
    // the same row so the operator can see RAM pressure at a glance without
    // needing a separate row for the bar.
    let mem_rss_str = App::format_bytes(mem_rss);
    let mem_ratio = if mem_total > 0 {
        (mem_rss as f64 / mem_total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Percentage label, e.g. "12.3%".  Only shown when we have total RAM.
    let mem_pct_label = if mem_total > 0 {
        format!(" {:.1}%", mem_ratio * 100.0)
    } else {
        String::new()
    };
    // Width available for the inline gauge bar: total col_w minus the label,
    // value, percentage label, and a two-char gap.
    let gauge_w = col_w
        .saturating_sub(LABEL_W + VALUE_W + 2 + mem_pct_label.len())
        .min(16);
    if gauge_w >= 4 && mem_total > 0 {
        // Build the row inline: label + value + gap + bar + pct label.
        let bar_spans = build_smooth_bar(mem_ratio, gauge_w, mem_color, theme);
        let mut mem_spans = vec![
            Span::raw(" "),
            Span::styled(
                format!("{:<label_w$}", "Memory", label_w = LABEL_W),
                Style::default().fg(theme.muted),
            ),
            Span::styled(
                format!("{:>value_w$}", mem_rss_str, value_w = VALUE_W),
                Style::default().fg(mem_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
        ];
        mem_spans.extend(bar_spans);
        mem_spans.push(Span::styled(
            mem_pct_label,
            Style::default().fg(mem_color).add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(mem_spans));
    } else {
        // Narrow panel — just the label and value.
        lines.push(kv_aligned("Memory", mem_rss_str, mem_color, theme, col_w));
    }

    // ---- Row 3: Peak RSS ----
    //
    // "Peak      2.1 GB"  — high-water mark for process RSS.  Shown in muted
    // color so it reads as a secondary annotation next to the live Memory row.
    if mem_peak > 0 {
        lines.push(kv_aligned(
            "Peak",
            App::format_bytes(mem_peak),
            theme.muted,
            theme,
            col_w,
        ));
    }

    // ---- Row 4+: Disk space (from Prometheus metrics) ----
    //
    // Read disk stats from the node's metrics endpoint rather than querying
    // the filesystem directly.  The node populates dugite_disk_total_bytes,
    // dugite_disk_used_bytes, and dugite_disk_available_bytes.
    let disk_total = app.metrics.get_u64("dugite_disk_total_bytes");
    let disk_used = app.metrics.get_u64("dugite_disk_used_bytes");
    let disk_free = app.metrics.get_u64("dugite_disk_available_bytes");

    if disk_total > 0 {
        let disk_ratio = (disk_used as f64 / disk_total as f64).clamp(0.0, 1.0);
        let disk_pct = disk_ratio * 100.0;

        // Color thresholds: green < 70 %, yellow 70–90 %, red > 90 %.
        let disk_color = if disk_pct > 90.0 {
            theme.error
        } else if disk_pct > 70.0 {
            theme.warning
        } else {
            theme.success
        };

        let disk_used_str = App::format_bytes(disk_used);
        let disk_pct_label = format!(" {:.1}%", disk_pct);

        // Width available for the inline bar (same calculation as Memory row).
        let disk_gauge_w = col_w
            .saturating_sub(LABEL_W + VALUE_W + 2 + disk_pct_label.len())
            .min(16);

        if disk_gauge_w >= 4 {
            let bar_spans = build_smooth_bar(disk_ratio, disk_gauge_w, disk_color, theme);
            let mut disk_spans = vec![
                Span::raw(" "),
                Span::styled(
                    format!("{:<label_w$}", "Disk", label_w = LABEL_W),
                    Style::default().fg(theme.muted),
                ),
                Span::styled(
                    format!("{:>value_w$}", disk_used_str, value_w = VALUE_W),
                    Style::default().fg(disk_color).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
            ];
            disk_spans.extend(bar_spans);
            disk_spans.push(Span::styled(
                disk_pct_label,
                Style::default().fg(disk_color).add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::from(disk_spans));
        } else {
            lines.push(kv_aligned("Disk", disk_used_str, disk_color, theme, col_w));
        }

        lines.push(kv_aligned(
            "Disk free",
            App::format_bytes(disk_free),
            theme.muted,
            theme,
            col_w,
        ));
    }

    // UTxO Store and Peers are intentionally NOT shown here — they are
    // already displayed in the Chain panel and Connections panel respectively.
    // Duplicating them would waste vertical space and violate the "no duplicate
    // fields" design goal documented in the module header.

    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);
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

    let fmt_rtt = |ms: Option<f64>| -> String {
        match ms {
            Some(v) => format!("{:.0}ms", v),
            None => "--".to_string(),
        }
    };

    let col_w = inner.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    if total == 0 {
        lines.push(Line::from(Span::styled(
            " Waiting for handshake data...",
            Style::default().fg(theme.muted),
        )));
    } else {
        // Row 1: Key stats as right-aligned key-value pairs.
        lines.push(kv_aligned(
            "Avg RTT",
            fmt_rtt(rtt.avg_ms),
            theme.info,
            theme,
            col_w,
        ));
        lines.push(kv_aligned(
            "Min / Max",
            format!("{} / {}", fmt_rtt(rtt.min_ms), fmt_rtt(rtt.max_ms)),
            theme.muted,
            theme,
            col_w,
        ));
        lines.push(kv_aligned(
            "p50 / p95",
            format!("{} / {}", fmt_rtt(rtt.p50_ms), fmt_rtt(rtt.p95_ms)),
            theme.warning,
            theme,
            col_w,
        ));

        // Row 4: Band distribution — compact inline.
        let band_items: &[(&str, u64, Color)] = &[
            ("<50ms:", rtt.band_0_50, theme.success),
            ("<100:", rtt.band_50_100, theme.info),
            ("<200:", rtt.band_100_200, theme.warning),
            ("200+:", rtt.band_200_plus, theme.error),
        ];
        let sep = Span::styled("  ", Style::default().fg(theme.muted));
        let mut band_spans = vec![Span::raw(" ")];
        for (i, (label, count, color)) in band_items.iter().enumerate() {
            if i > 0 {
                band_spans.push(sep.clone());
            }
            band_spans.push(Span::styled(
                label.to_string(),
                Style::default().fg(theme.muted),
            ));
            band_spans.push(Span::styled(
                format!("{count}"),
                Style::default().fg(*color).add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(band_spans));
    }

    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Panel: Governance
// ---------------------------------------------------------------------------

/// Governance panel: on-chain governance state + Conway protocol parameters.
///
/// Shows two subsections:
///   1. Live ledger state — DRep registry (total/active), pools, proposals,
///      committee membership, cumulative dormant-epoch counter, ADA pots,
///      delegation counts.
///   2. Conway protocol parameters — DRep deposit, DRep activity, governance
///      action deposit + lifetime.  These change via ratification so they are
///      real live gauges, not static config.
///
/// All values are read-only; the panel never modifies node state.
fn render_governance_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Governance", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width < 8 {
        return;
    }

    // Live state
    let drep_total = app.metrics.get_u64("dugite_drep_count");
    let drep_active = app.metrics.get_u64("dugite_drep_active");
    let pool_count = app.metrics.get_u64("dugite_pool_count");
    let proposal_count = app.metrics.get_u64("dugite_proposal_count");
    let delegation_count = app.metrics.get_u64("dugite_delegation_count");
    let vote_deleg_count = app.metrics.get_u64("dugite_vote_delegation_count");
    let committee_hot = app.metrics.get_u64("dugite_committee_hot_count");
    let committee_total = app.metrics.get_u64("dugite_committee_total_count");
    let committee_nc = app.metrics.get_u64("dugite_committee_no_confidence");
    let dormant = app.metrics.get_u64("dugite_gov_dormant_epochs");
    let treasury_lovelace = app.metrics.get_u64("dugite_treasury_lovelace");
    let reserves_lovelace = app.metrics.get_u64("dugite_reserves_lovelace");

    // Protocol parameters
    let drep_deposit = app.metrics.get_u64("dugite_pparam_drep_deposit_lovelace");
    let drep_activity = app.metrics.get_u64("dugite_pparam_drep_activity_epochs");
    let gov_action_deposit = app
        .metrics
        .get_u64("dugite_pparam_gov_action_deposit_lovelace");
    let gov_action_lifetime = app
        .metrics
        .get_u64("dugite_pparam_gov_action_lifetime_epochs");

    let ada_str = |lovelace: u64| format!("{} ADA", App::format_number(lovelace / 1_000_000));

    let col_w = inner.width.saturating_sub(2) as usize;
    // Treasury/Reserves values can be long (e.g. "14,074,169 ADA" = 14 chars).
    // Use a wider value column so the number is never clipped.
    const ADA_VALUE_W: usize = 22;
    let ada_label_w = col_w.saturating_sub(ADA_VALUE_W).max(LABEL_W);
    let ada_value_w = col_w.saturating_sub(ada_label_w).max(1);

    let ada_row = |label: &str, lovelace: u64| -> Line<'static> {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("{:<label_w$}", label, label_w = ada_label_w),
                Style::default().fg(theme.muted),
            ),
            Span::styled(
                format!("{:>value_w$}", ada_str(lovelace), value_w = ada_value_w),
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    };

    let drep_value = format!(
        "{} / {}",
        App::format_number(drep_active),
        App::format_number(drep_total)
    );
    let committee_value = if committee_nc > 0 {
        "NO-CONF".to_string()
    } else {
        format!(
            "{} / {}",
            App::format_number(committee_hot),
            App::format_number(committee_total)
        )
    };

    let mut lines = vec![
        kv_aligned(
            "DReps",
            drep_value,
            if drep_active > 0 {
                theme.info
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        kv_aligned(
            "Stake Pools",
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
        kv_aligned(
            "Committee",
            committee_value,
            if committee_nc > 0 {
                theme.error
            } else if committee_hot > 0 {
                theme.info
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        kv_aligned(
            "Dormant Epochs",
            App::format_number(dormant),
            if dormant > 0 {
                theme.warning
            } else {
                theme.muted
            },
            theme,
            col_w,
        ),
        ada_row("Treasury", treasury_lovelace),
        ada_row("Reserves", reserves_lovelace),
        kv_aligned(
            "Stake Delegations",
            App::format_number(delegation_count),
            theme.fg,
            theme,
            col_w,
        ),
        kv_aligned(
            "Vote Delegations",
            App::format_number(vote_deleg_count),
            theme.fg,
            theme,
            col_w,
        ),
        // Visual separator between live state and protocol parameters.
        Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            Style::default().fg(theme.border),
        )),
        kv_aligned(
            "DRep Deposit",
            ada_str(drep_deposit),
            theme.fg,
            theme,
            col_w,
        ),
        kv_aligned(
            "DRep Activity",
            format!("{} epochs", App::format_number(drep_activity)),
            theme.fg,
            theme,
            col_w,
        ),
        kv_aligned(
            "Action Deposit",
            ada_str(gov_action_deposit),
            theme.fg,
            theme,
            col_w,
        ),
        kv_aligned(
            "Action Lifetime",
            format!("{} epochs", App::format_number(gov_action_lifetime)),
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
            "dugite-monitor",
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
            "Dugite Node Dashboard",
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
            label.to_string(),
            Style::default().fg(theme.muted),
        ));
        spans.push(Span::styled(
            App::format_number(*value).to_string(),
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
#[cfg(test)]
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

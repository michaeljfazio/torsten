//! Dashboard rendering — draws all five panels, the header bar, footer, and the
//! optional help overlay.
//!
//! Every colour reference goes through the active [`Theme`] so that theme
//! cycling (key `t`) changes the entire look instantly.
//!
//! Panel layout (two-column standard):
//!
//! ```text
//! ┌──────────────────── Header (1 line) ─────────────────────────────────────┐
//! ├───── Node ──────────┬──────────────────── Chain ───────────────────────── ┤
//! ├── Connections ──────┼─────────────────── Resources ───────────────────── ┤
//! ├───────────────────── Peers (full width) ────────────────────────────────── ┤
//! │                     gap                                                    │
//! ├──────────────────── Footer (1 line) ─────────────────────────────────────┘
//! ```

use crate::app::App;
use crate::layout::compute_layout;
use crate::theme::Theme;
use crate::widgets::epoch_progress::EpochProgress;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
    Frame,
};

// Cardano logo shown in the header.
const LOGO: &str = " Torsten ";

/// Entry point called from the main event loop each frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let theme = app.theme();
    let layout = compute_layout(area, None);

    render_header(frame, app, theme, layout.header);
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
// Header
// ---------------------------------------------------------------------------

/// Single-line header: logo | sync status | epoch | tip age | uptime
fn render_header(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    if area.height < 1 {
        return;
    }
    let (status_label, is_synced, is_stalled) = app.sync_status();
    let pct = app.sync_progress_pct();
    let epoch = app.metrics.get_u64("torsten_epoch_number");
    let tip_age = app.metrics.get_u64("torsten_tip_age_seconds");
    let uptime_secs = app.metrics.get_u64("torsten_uptime_seconds");
    let uptime = App::format_uptime(uptime_secs);

    let status_color = if !app.metrics.connected {
        theme.error
    } else if is_synced {
        theme.success
    } else if is_stalled {
        theme.error
    } else {
        theme.warning
    };

    let status_text = if !app.metrics.connected {
        "Disconnected".to_string()
    } else {
        format!("{} {:.2}%", status_label, pct)
    };

    let era = app.current_era();
    let network = app.network.label();

    let header_line = Line::from(vec![
        Span::styled(
            LOGO,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{2502} ", Style::default().fg(theme.border)),
        Span::styled(
            status_text,
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  \u{2502}  ", Style::default().fg(theme.border)),
        Span::styled("Epoch ", Style::default().fg(theme.muted)),
        Span::styled(
            App::format_number(epoch),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  \u{2502}  ", Style::default().fg(theme.border)),
        Span::styled("Era ", Style::default().fg(theme.muted)),
        Span::styled(
            era,
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  \u{2502}  ", Style::default().fg(theme.border)),
        Span::styled("Net ", Style::default().fg(theme.muted)),
        Span::styled(network, Style::default().fg(theme.accent)),
        Span::styled("  \u{2502}  ", Style::default().fg(theme.border)),
        Span::styled("Tip ", Style::default().fg(theme.muted)),
        Span::styled(
            format!("{}s", tip_age),
            Style::default().fg(tip_age_color(theme, tip_age)),
        ),
        Span::styled("  \u{2502}  ", Style::default().fg(theme.border)),
        Span::styled("Up ", Style::default().fg(theme.muted)),
        Span::styled(uptime, Style::default().fg(theme.muted)),
    ]);

    frame.render_widget(Paragraph::new(header_line), area);
}

// ---------------------------------------------------------------------------
// Panel: Node
// ---------------------------------------------------------------------------

/// Node info panel:
/// - Role: Relay / Block Producer
/// - Network: Mainnet/Preview/Preprod/…
/// - Version: from CARGO_PKG_VERSION
/// - Era: current era
/// - Uptime: human readable
fn render_node_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Node", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width < 4 {
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

    let lines = vec![
        kv_line("Role     ", role.to_string(), role_color, theme),
        kv_line("Network  ", network.to_string(), theme.accent, theme),
        kv_line("Version  ", version.to_string(), theme.muted, theme),
        kv_line("Era      ", era.to_string(), theme.info, theme),
        kv_line("Uptime   ", uptime, theme.muted, theme),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Panel: Chain
// ---------------------------------------------------------------------------

/// Chain panel:
/// - Epoch progress bar with epoch number and % centred
/// - Block, Slot, Slot-in-Epoch, Tip hash (abbrev), Tip diff with health emoji
/// - Density, Forks, Total Tx, Pending Tx
fn render_chain_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Chain", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let block_num = App::format_number(app.metrics.get_u64("torsten_block_number"));
    let slot_num = App::format_number(app.metrics.get_u64("torsten_slot_number"));
    let epoch = app.metrics.get_u64("torsten_epoch_number");
    let tip_age = app.metrics.get_u64("torsten_tip_age_seconds");
    let tip_diff_health = tip_age_emoji(tip_age);
    let total_tx = App::format_number(app.metrics.get_u64("torsten_transactions_received_total"));
    let pending_tx = app.metrics.get_u64("torsten_mempool_tx_count");
    let density = app.metrics.get("torsten_chain_density");
    let forks = app.metrics.get_u64("torsten_rollback_count_total");

    // Row 0: epoch progress bar (1 line tall inside inner).
    if inner.height >= 1 {
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
            .with_epoch(epoch),
            bar_area,
        );
    }

    // Remaining lines start at y + 1.
    if inner.height < 2 {
        return;
    }
    let rest_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height - 1,
    };

    let tip_age_col = tip_age_color(theme, tip_age);
    let density_str = if density > 0.0 {
        format!("{:.4}", density)
    } else {
        "--".to_string()
    };

    let lines = vec![
        kv_line("Block     ", block_num, theme.fg, theme),
        kv_line("Slot      ", slot_num, theme.fg, theme),
        kv_line(
            "Slot/Epoch",
            App::format_number(app.slot_in_epoch),
            theme.muted,
            theme,
        ),
        kv_line(
            "Tip diff  ",
            format!("{}s {}", tip_age, tip_diff_health),
            tip_age_col,
            theme,
        ),
        kv_line("Density   ", density_str, theme.info, theme),
        kv_line(
            "Forks     ",
            App::format_number(forks),
            if forks > 0 {
                theme.warning
            } else {
                theme.muted
            },
            theme,
        ),
        kv_line("Total Tx  ", total_tx, theme.muted, theme),
        kv_line(
            "Pending Tx",
            App::format_number(pending_tx),
            if pending_tx > 0 {
                theme.warning
            } else {
                theme.muted
            },
            theme,
        ),
    ];

    frame.render_widget(Paragraph::new(lines), rest_area);
}

// ---------------------------------------------------------------------------
// Panel: Connections
// ---------------------------------------------------------------------------

/// Connections panel:
/// - P2P enabled flag
/// - Incoming, Outgoing
/// - Cold / Warm / Hot
/// - Uni-Dir, Bi-Dir, Duplex
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
    let duplex = app.metrics.get_u64("torsten_peers_duplex");

    // P2P enabled = outbound > 0 or inbound > 0.
    let p2p_enabled = outbound > 0 || inbound > 0 || cold > 0 || warm > 0 || hot > 0;
    let p2p_color = if p2p_enabled {
        theme.success
    } else {
        theme.error
    };
    let p2p_label = if p2p_enabled { "Enabled" } else { "Disabled" };

    let lines = vec![
        kv_line("P2P      ", p2p_label.to_string(), p2p_color, theme),
        kv_line("Inbound  ", App::format_number(inbound), theme.info, theme),
        kv_line("Outbound ", App::format_number(outbound), theme.info, theme),
        // Cold / Warm / Hot on one line.
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("Cold ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{:<4}", cold),
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Warm ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{:<4}", warm),
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Hot ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{}", hot),
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        // Uni / Bi / Duplex on one line.
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("Uni ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{:<4}", unidir),
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Bi  ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{:<4}", bidir),
                Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Dpx ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{}", duplex),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Panel: Resources
// ---------------------------------------------------------------------------

/// Resources panel:
/// - CPU (sys) percentage
/// - Mem (live) — human readable
/// - Mem (RSS)
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
    // Fallback: if rss is not exposed, use resident.
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

    let lines = vec![
        kv_line("CPU (sys) ", format!("{:.1}%", cpu_pct), cpu_color, theme),
        kv_line("Mem (live)", App::format_bytes(mem_live), mem_color, theme),
        kv_line("Mem (RSS) ", App::format_bytes(mem_rss), theme.muted, theme),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Panel: Peers (RTT breakdown)
// ---------------------------------------------------------------------------

/// Peers panel:
/// - RTT bands: 0-50ms, 50-100ms, 100-200ms, 200ms+
/// - Lowest / Average / Highest RTT
fn render_peers_panel(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let block = panel_block("Peers", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 1 || inner.width < 4 {
        return;
    }

    let rtt = &app.rtt_bands;

    let fmt_rtt = |ms: Option<f64>| -> String {
        match ms {
            Some(v) => format!("{:.0}ms", v),
            None => "--".to_string(),
        }
    };

    // Bar width for the RTT distribution bar.
    let total = rtt.band_0_50 + rtt.band_50_100 + rtt.band_100_200 + rtt.band_200_plus;
    let bar_width = inner.width.saturating_sub(4) as usize;

    let mut lines = vec![
        // RTT bands header row
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("0-50ms  ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{:<5}", rtt.band_0_50),
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  50-100ms  ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{:<5}", rtt.band_50_100),
                Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("100-200ms", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{:<5}", rtt.band_100_200),
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  200ms+    ", Style::default().fg(theme.muted)),
            Span::styled(
                format!("{}", rtt.band_200_plus),
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    // RTT distribution bar (visual breakdown).
    if total > 0 && bar_width > 4 {
        let bar = build_rtt_bar(
            rtt.band_0_50,
            rtt.band_50_100,
            rtt.band_100_200,
            rtt.band_200_plus,
            bar_width,
        );
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(bar, Style::default().fg(theme.muted)),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            "  (no handshake data)",
            Style::default().fg(theme.muted),
        )));
    }

    // Min / Avg / Max
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled("Low ", Style::default().fg(theme.muted)),
        Span::styled(
            fmt_rtt(rtt.min_ms),
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  Avg ", Style::default().fg(theme.muted)),
        Span::styled(
            fmt_rtt(rtt.avg_ms),
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  High ", Style::default().fg(theme.muted)),
        Span::styled(
            fmt_rtt(rtt.max_ms),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

fn render_footer(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let theme_name = app.theme().name;

    let line = Line::from(vec![
        Span::styled("  ", Style::default()),
        key_span("[q]", theme),
        Span::styled("uit  ", Style::default().fg(theme.muted)),
        key_span("[t]", theme),
        Span::styled(
            format!("heme:{}  ", theme_name),
            Style::default().fg(theme.muted),
        ),
        key_span("[r]", theme),
        Span::styled("efresh  ", Style::default().fg(theme.muted)),
        key_span("[h]", theme),
        Span::styled("elp  ", Style::default().fg(theme.muted)),
        Span::styled("\u{2502}  ", Style::default().fg(theme.border)),
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
    let overlay_width: u16 = 52;
    let overlay_height: u16 = 18;

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
        help_kv("t", "Cycle theme", theme),
        help_kv("r", "Force-refresh metrics", theme),
        help_kv("h / ?", "Toggle this help overlay", theme),
        Line::default(),
        Line::from(Span::styled(
            "Panels: Node | Chain | Connections | Resources | Peers",
            Style::default().fg(theme.muted),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Metrics polled every 2 seconds.",
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
// Helper widgets / constructors
// ---------------------------------------------------------------------------

/// Create a bordered panel block with a title.
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
        .padding(Padding::new(1, 1, 0, 0))
}

/// Render a key-value metric line: `"  Label   Value"`.
fn kv_line<'a>(label: &'a str, value: String, value_color: Color, _theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(label, Style::default().fg(_theme.muted)),
        Span::styled(
            value,
            Style::default()
                .fg(value_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Render a help overlay key binding row.
fn help_kv<'a>(key: &'a str, desc: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{:>10}", key),
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(desc, Style::default().fg(theme.muted)),
    ])
}

/// Build a styled key `[x]` span for the footer.
fn key_span<'a>(text: &'a str, theme: &'a Theme) -> Span<'a> {
    Span::styled(
        text.to_string(),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )
}

/// Select color for tip-age display.
fn tip_age_color(theme: &Theme, tip_age_secs: u64) -> Color {
    if tip_age_secs < 30 {
        theme.success
    } else if tip_age_secs < 120 {
        theme.warning
    } else {
        theme.error
    }
}

/// Health emoji based on tip age.
fn tip_age_emoji(tip_age_secs: u64) -> &'static str {
    if tip_age_secs < 30 {
        "\u{2705}" // green check
    } else if tip_age_secs < 120 {
        "\u{26A0}" // warning
    } else {
        "\u{274C}" // red X
    }
}

/// Build a compact ASCII RTT distribution bar using block characters.
///
/// Each band is represented by a different character:
/// - `\u{2588}` (full block) — 0-50ms (good, green)
/// - `\u{2593}` (dark shade) — 50-100ms (info)
/// - `\u{2592}` (medium shade) — 100-200ms (warning)
/// - `\u{2591}` (light shade) — 200ms+ (bad)
fn build_rtt_bar(b0: u64, b1: u64, b2: u64, b3: u64, width: usize) -> String {
    let total = b0 + b1 + b2 + b3;
    if total == 0 || width == 0 {
        return " ".repeat(width);
    }
    let w0 = ((b0 as f64 / total as f64) * width as f64).round() as usize;
    let w1 = ((b1 as f64 / total as f64) * width as f64).round() as usize;
    let w2 = ((b2 as f64 / total as f64) * width as f64).round() as usize;
    // Give any rounding remainder to the last band.
    let w3 = width.saturating_sub(w0 + w1 + w2);

    let mut bar = String::with_capacity(width);
    bar.push_str(&"\u{2588}".repeat(w0)); // 0-50ms — full block
    bar.push_str(&"\u{2593}".repeat(w1)); // 50-100ms — dark shade
    bar.push_str(&"\u{2592}".repeat(w2)); // 100-200ms — medium shade
    bar.push_str(&"\u{2591}".repeat(w3)); // 200ms+ — light shade
    bar
}

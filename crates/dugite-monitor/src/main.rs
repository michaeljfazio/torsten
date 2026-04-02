//! dugite-monitor — Terminal UI dashboard for the Dugite Cardano node.
//!
//! Polls the Dugite Prometheus endpoint (default http://localhost:12798/metrics)
//! every 2 seconds and renders a real-time 5-panel dashboard:
//!
//! - **Node**:         Role, network, version, era, uptime
//! - **Chain**:        Epoch progress bar, block/slot/tip metrics, density, forks, tx counts
//! - **Connections**:  P2P state, inbound/outbound, cold/warm/hot, uni/bi/duplex counts
//! - **Resources**:    CPU %, live memory, RSS memory
//! - **Peers**:        RTT bands (0-50ms, 50-100ms, 100-200ms, 200ms+), min/avg/max RTT
//!
//! # Usage
//!
//! ```bash
//! dugite-monitor                                         # defaults
//! dugite-monitor --metrics-url http://host:12798/metrics # custom endpoint
//! dugite-monitor --network-magic 2                       # preview testnet epoch length
//! ```
//!
//! # Key bindings
//!
//! | Key      | Action                          |
//! |----------|---------------------------------|
//! | q / Esc  | Quit                            |
//! | t        | Cycle theme                     |
//! | r        | Force-refresh metrics           |
//! | h / ?    | Toggle help overlay             |

mod app;
#[allow(dead_code)]
mod disk;
mod layout;
mod metrics;
mod theme;
mod ui;
mod widgets;

use std::io;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;

use app::App;
use metrics::fetch_metrics;

/// Default Prometheus metrics endpoint for the Dugite node.
const DEFAULT_METRICS_URL: &str = "http://localhost:12798/metrics";

/// Poll interval for fetching metrics from the Prometheus endpoint.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// CLI arguments for dugite-monitor.
#[derive(Parser, Debug)]
#[command(
    name = "dugite-monitor",
    about = "Terminal dashboard for the Dugite Cardano node"
)]
struct Args {
    /// URL of the Dugite Prometheus metrics endpoint.
    #[arg(long, default_value = DEFAULT_METRICS_URL)]
    metrics_url: String,

    /// Network magic for epoch length calculation.
    ///
    /// Preview = 2 (epoch length 86,400 slots = 1 day).
    /// Mainnet = 764824073 (epoch length 432,000 slots = 5 days).
    /// Preprod = 1 (epoch length 432,000 slots = 5 days).
    ///
    /// When omitted the epoch length is auto-detected from the
    /// `dugite_network_magic` Prometheus metric.
    #[arg(long)]
    network_magic: Option<u64>,

    /// Path to the node's database directory.
    ///
    /// When supplied the Resources panel shows disk space usage for the
    /// filesystem that contains this directory (total, used, free, and a
    /// usage percentage bar).  When omitted the disk row is hidden.
    ///
    /// Example: `--db-path ./db-preview`
    #[arg(long, default_value = "")]
    db_path: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut app = App::new();

    // Apply network magic epoch length override if provided on the CLI.
    if let Some(magic) = args.network_magic {
        app.epoch_length_override = app::Network::from_magic(magic).epoch_length();
    }

    // Store the database path so the Resources panel can query disk space.
    app.db_path = args.db_path.clone();

    // Setup terminal in raw alternate-screen mode.
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Fetch initial metrics before the first render so the UI is not blank.
    let snapshot = fetch_metrics(&args.metrics_url).await;
    app.update_metrics(snapshot);

    let result = run_loop(&mut terminal, &mut app, &args.metrics_url).await;

    // Restore terminal on exit.
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    result
}

/// Main event loop: renders each frame, handles keyboard input, and periodically
/// refreshes metrics from the Prometheus endpoint.
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    metrics_url: &str,
) -> Result<()> {
    let mut last_fetch = tokio::time::Instant::now();

    loop {
        // Render current state.
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Short poll timeout so the metrics timer fires promptly.
        let timeout = POLL_INTERVAL
            .checked_sub(last_fetch.elapsed())
            .unwrap_or(Duration::ZERO)
            .min(Duration::from_millis(100));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                // Only handle key press events (not release/repeat).
                if key.kind == KeyEventKind::Press {
                    // Any key dismisses the help overlay.
                    if app.show_help {
                        app.show_help = false;
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            app.should_quit = true;
                        }
                        KeyCode::Char('t') => {
                            // Cycle through themes.
                            app.cycle_theme();
                        }
                        KeyCode::Char('r') => {
                            // Force immediate metrics refresh.
                            let snapshot = fetch_metrics(metrics_url).await;
                            app.update_metrics(snapshot);
                            last_fetch = tokio::time::Instant::now();
                        }
                        KeyCode::Char('h') | KeyCode::Char('?') => {
                            app.toggle_help();
                        }
                        _ => {}
                    }
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }

        // Periodic metrics fetch.
        if last_fetch.elapsed() >= POLL_INTERVAL {
            let snapshot = fetch_metrics(metrics_url).await;
            app.update_metrics(snapshot);
            last_fetch = tokio::time::Instant::now();
        }
    }
}

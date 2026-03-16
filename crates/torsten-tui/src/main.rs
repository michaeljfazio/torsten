//! torsten-tui — Terminal UI dashboard for the Torsten Cardano node.
//!
//! Polls the Torsten Prometheus endpoint (default http://localhost:12798/metrics)
//! every 2 seconds and renders a real-time 4-panel dashboard showing chain status,
//! peer information, performance metrics, and governance state.
//!
//! # Usage
//!
//! ```bash
//! torsten-tui                                         # defaults
//! torsten-tui --metrics-url http://host:12798/metrics # custom endpoint
//! torsten-tui --network-magic 2                       # preview testnet epoch length
//! ```

mod app;
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
use theme::find_theme_by_name;

/// Default Prometheus metrics endpoint for the Torsten node.
const DEFAULT_METRICS_URL: &str = "http://localhost:12798/metrics";

/// Poll interval for fetching metrics.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// CLI arguments for torsten-tui.
#[derive(Parser, Debug)]
#[command(
    name = "torsten-tui",
    about = "Terminal dashboard for the Torsten Cardano node"
)]
struct Args {
    /// URL of the Torsten Prometheus metrics endpoint.
    #[arg(long, default_value = DEFAULT_METRICS_URL)]
    metrics_url: String,

    /// Network magic for epoch length calculation.
    ///
    /// Preview = 2 (epoch length 86,400 slots = 1 day).
    /// Mainnet = 764824073 (epoch length 432,000 slots = 5 days).
    /// Preprod = 1 (epoch length 432,000 slots = 5 days).
    ///
    /// When omitted, the epoch length is auto-detected from Prometheus metrics
    /// or defaults to 432,000 slots (mainnet).
    #[arg(long)]
    network_magic: Option<u64>,

    /// Starting color theme name (case-insensitive).
    ///
    /// Available themes: Default, Monokai, Nord, Dracula, Catppuccin Mocha,
    /// Solarized Dark, Solarized Light. Press [t] at runtime to cycle themes.
    #[arg(long, value_name = "THEME")]
    theme: Option<String>,
}

/// Determine epoch length from network magic.
///
/// Returns 0 if the magic is unknown (falls back to auto-detection).
fn epoch_length_from_magic(magic: u64) -> u64 {
    match magic {
        2 => 86_400,            // Preview testnet: 1 day
        1 => 432_000,           // Preprod: 5 days
        764_824_073 => 432_000, // Mainnet: 5 days
        _ => 0,                 // Unknown: auto-detect
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut app = App::new();

    // Apply network magic epoch length override if provided.
    if let Some(magic) = args.network_magic {
        app.epoch_length_override = epoch_length_from_magic(magic);
    }

    // Apply starting theme if provided; silently ignore unknown theme names.
    if let Some(ref name) = args.theme {
        if let Some(idx) = find_theme_by_name(name) {
            app.theme_idx = idx;
        }
    }

    // Setup terminal
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Fetch initial metrics before first render
    let snapshot = fetch_metrics(&args.metrics_url).await;
    app.update_metrics(snapshot);

    // Main event loop
    let result = run_loop(&mut terminal, &mut app, &args.metrics_url).await;

    // Restore terminal
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    result
}

/// Main event loop: poll for keyboard events and periodically refresh metrics.
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    metrics_url: &str,
) -> Result<()> {
    let mut last_fetch = tokio::time::Instant::now();

    loop {
        // Render the current state
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Poll for events with a short timeout so we can check the metrics timer
        let timeout = POLL_INTERVAL
            .checked_sub(last_fetch.elapsed())
            .unwrap_or(Duration::ZERO)
            .min(Duration::from_millis(100));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                // Only handle key press events (not release/repeat)
                if key.kind == KeyEventKind::Press {
                    // If help overlay is showing, any key closes it
                    if app.show_help {
                        app.show_help = false;
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            app.should_quit = true;
                        }
                        KeyCode::Tab => {
                            app.next_panel();
                        }
                        KeyCode::BackTab => {
                            // Shift+Tab: cycle panels backward
                            app.prev_panel();
                        }
                        KeyCode::Char('h') | KeyCode::Char('?') => {
                            app.toggle_help();
                        }
                        KeyCode::Char('m') => {
                            app.toggle_layout_mode();
                        }
                        KeyCode::Char('t') => {
                            // Cycle through built-in color themes
                            app.cycle_theme();
                        }
                        KeyCode::Char('r') => {
                            // Force immediate refresh
                            let snapshot = fetch_metrics(metrics_url).await;
                            app.update_metrics(snapshot);
                            last_fetch = tokio::time::Instant::now();
                        }
                        // Number keys 1-4: jump to specific panel
                        KeyCode::Char(c @ '1'..='4') => {
                            let idx = (c as u8 - b'0') as usize;
                            app.jump_to_panel(idx);
                        }
                        _ => {}
                    }
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }

        // Periodic metrics fetch
        if last_fetch.elapsed() >= POLL_INTERVAL {
            let snapshot = fetch_metrics(metrics_url).await;
            app.update_metrics(snapshot);
            last_fetch = tokio::time::Instant::now();
        }
    }
}

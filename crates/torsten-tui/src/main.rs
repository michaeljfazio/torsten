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
//! ```

mod app;
mod layout;
mod metrics;
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut app = App::new();

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
                        KeyCode::Char('h') => {
                            app.toggle_help();
                        }
                        KeyCode::Char('m') => {
                            app.toggle_layout_mode();
                        }
                        KeyCode::Char('r') => {
                            // Force immediate refresh
                            let snapshot = fetch_metrics(metrics_url).await;
                            app.update_metrics(snapshot);
                            last_fetch = tokio::time::Instant::now();
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

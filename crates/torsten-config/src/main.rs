//! torsten-config — Interactive TUI configuration editor for Torsten node config files.
//!
//! # Usage
//!
//! ```bash
//! torsten-config edit <config-file>
//! ```
//!
//! Opens the named Cardano node configuration JSON file in an interactive
//! terminal editor.  Changes are held in memory until the user presses
//! Ctrl+S; a `.bak` copy of the original file is created on each save.
//!
//! # Key bindings
//!
//! | Key          | Action                                         |
//! |--------------|------------------------------------------------|
//! | j / Down     | Move cursor down                               |
//! | k / Up       | Move cursor up                                 |
//! | Enter        | Edit selected parameter (toggle bool / cycle   |
//! |              | enum / open text buffer for string/number/path)|
//! | Esc          | Cancel current edit                            |
//! | Tab          | Collapse / expand current section              |
//! | Ctrl+S       | Save config to disk (creates .bak backup)      |
//! | q            | Quit (prompts if there are unsaved changes)    |
//!
//! # Two-panel layout (>=80 columns)
//!
//! Left 60%:  parameter tree — sections, keys, right-aligned values.
//! Right 40%: description panel — type, default, docs for selected parameter.
//!
//! Below 80 columns the right panel is hidden and the tree fills the terminal.

mod app;
mod config;
mod schema;
mod ui;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;

use app::App;
use config::load_config;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// Interactive TUI configuration editor for Torsten Cardano node config files.
#[derive(Parser, Debug)]
#[command(
    name = "torsten-config",
    version,
    about = "Interactive TUI editor for Torsten node configuration files"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Available sub-commands.
#[derive(Subcommand, Debug)]
enum Commands {
    /// Open a configuration file in the interactive editor.
    Edit {
        /// Path to the Cardano node configuration JSON file.
        config_file: PathBuf,
    },
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Edit { config_file } => {
            run_editor(&config_file)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Editor lifecycle
// ---------------------------------------------------------------------------

/// Load `path`, set up the terminal, run the event loop, restore terminal.
fn run_editor(path: &Path) -> Result<()> {
    let config =
        load_config(path).with_context(|| format!("loading config file '{}'", path.display()))?;

    let mut app = App::new(config);

    // Set up the terminal in raw alternate-screen mode.
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, &mut app);

    // Restore terminal unconditionally, even on error.
    let _ = disable_raw_mode();
    let _ = io::stdout().execute(LeaveAlternateScreen);

    result
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// Main event / render loop.
///
/// Renders a frame on every iteration, then waits up to 100 ms for a key
/// event.  Returns when `app.should_quit` is set.
fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        // Render current state.
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Consume the feedback message after the first render so it is shown
        // for exactly one frame.
        let _feedback_shown = app.feedback.take();

        if app.should_quit {
            return Ok(());
        }

        // Wait at most 100 ms for a key event (keeps the UI responsive).
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                // Restore the feedback if it was set *during* this handler;
                // we only want to clear it on the *next* frame after it was
                // set, so we re-take at the top of the next iteration.
                handle_key(app, key.code, key.modifiers);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key handler
// ---------------------------------------------------------------------------

/// Dispatch a key press to the appropriate [`App`] action.
fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    // Ctrl+S saves in any mode.
    if code == KeyCode::Char('s') && modifiers.contains(KeyModifiers::CONTROL) {
        app.save();
        return;
    }

    if app.is_typing() {
        handle_typing(app, code);
    } else {
        handle_browse(app, code);
    }
}

/// Handle key events while in typing / edit mode.
fn handle_typing(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => {
            app.confirm_edit();
        }
        KeyCode::Esc => {
            app.cancel_edit();
        }
        KeyCode::Backspace => {
            app.backspace();
        }
        KeyCode::Char(c) => {
            app.type_char(c);
        }
        _ => {}
    }
}

/// Handle key events while in browse / navigation mode.
fn handle_browse(app: &mut App, code: KeyCode) {
    match code {
        // Navigation.
        KeyCode::Down | KeyCode::Char('j') => {
            app.cursor_down();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.cursor_up();
        }

        // Enter edit mode (or toggle/cycle).
        KeyCode::Enter | KeyCode::Char('e') => {
            app.begin_edit();
        }

        // Collapse / expand section.
        KeyCode::Tab => {
            app.toggle_section();
        }

        // Quit.
        KeyCode::Char('q') | KeyCode::Esc => {
            app.request_quit();
        }

        _ => {}
    }
}

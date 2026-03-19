//! torsten-config — Interactive TUI configuration editor for Torsten node config files.
//!
//! # Subcommands
//!
//! | Subcommand | Description                                               |
//! |------------|-----------------------------------------------------------|
//! | edit       | Open a config file in the interactive TUI editor          |
//! | init       | Generate a default config for a named network             |
//! | validate   | Validate a config file against the parameter schema       |
//! | get        | Print the value of a single parameter                     |
//! | set        | Update the value of a single parameter in a config file   |
//!
//! # Interactive editor key bindings
//!
//! | Key          | Action                                         |
//! |--------------|------------------------------------------------|
//! | j / Down     | Move cursor down                               |
//! | k / Up       | Move cursor up                                 |
//! | Enter        | Edit selected parameter (toggle bool / cycle   |
//! |              | enum / open text buffer for string/number/path)|
//! | Esc          | Cancel current edit / close search / close diff|
//! | Tab          | Collapse / expand current section              |
//! | /            | Enter search mode (fuzzy filter)               |
//! | Ctrl+D       | Show diff overlay (original vs. current)       |
//! | Ctrl+S       | Save config to disk (creates .bak backup)      |
//! | q            | Quit (prompts if there are unsaved changes)    |
//!
//! # Two-panel layout (>=80 columns)
//!
//! Left 60%:  parameter tree — sections, keys, right-aligned values.
//! Right 40%: description panel — type, default, tuning hint, docs for selected parameter.
//!
//! Below 80 columns the right panel is hidden and the tree fills the terminal.

mod app;
mod config;
mod diff;
mod schema;
mod search;
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
use schema::{build_lookup, Network};

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
    /// Open a configuration file in the interactive TUI editor.
    Edit {
        /// Path to the Cardano node configuration JSON file.
        config_file: PathBuf,
    },

    /// Generate a default configuration file for the given network.
    ///
    /// Writes a JSON config with sensible defaults to the specified output
    /// path (or stdout if `--out` is omitted).  Genesis file paths use the
    /// conventional `<network>-*-genesis.json` naming relative to the config.
    Init {
        /// Target network: mainnet, preview, or preprod.
        #[arg(long, short)]
        network: String,

        /// Output path for the generated config file.  Prints to stdout if
        /// omitted.
        #[arg(long, short)]
        out: Option<PathBuf>,
    },

    /// Validate a configuration file against the parameter schema.
    ///
    /// Exits with code 0 if the file is valid, 1 if it contains errors.
    /// Suitable for use in CI/CD pipelines.
    Validate {
        /// Path to the Cardano node configuration JSON file to validate.
        config_file: PathBuf,
    },

    /// Print the current value of a single parameter.
    Get {
        /// The JSON key name to read (e.g. "EnableP2P").
        key: String,

        /// Path to the configuration file.
        #[arg(long, short)]
        config: PathBuf,

        /// Also print the parameter's description and type.
        #[arg(long, short)]
        verbose: bool,
    },

    /// Update the value of a single parameter in a config file.
    ///
    /// Creates a `.bak` backup of the original file before writing.
    Set {
        /// The JSON key name to update (e.g. "MinSeverity").
        key: String,

        /// The new value as a string (booleans: "true"/"false", numbers as
        /// decimal, strings and paths as plain text).
        value: String,

        /// Path to the configuration file.
        #[arg(long, short)]
        config: PathBuf,
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
        Commands::Init { network, out } => {
            run_init(&network, out.as_deref())?;
        }
        Commands::Validate { config_file } => {
            run_validate(&config_file)?;
        }
        Commands::Get {
            key,
            config,
            verbose,
        } => {
            run_get(&key, &config, verbose)?;
        }
        Commands::Set { key, value, config } => {
            run_set(&key, &value, &config)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `edit` subcommand — interactive TUI
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

    // Ctrl+D toggles the diff overlay (not available while typing or searching).
    if code == KeyCode::Char('d') && modifiers.contains(KeyModifiers::CONTROL) {
        if !app.is_typing() && !app.search_active {
            app.toggle_diff();
        }
        return;
    }

    // If the diff overlay is showing, only Esc closes it.
    if app.show_diff {
        if code == KeyCode::Esc {
            app.close_diff();
        }
        return;
    }

    // Search mode dispatches to its own handler.
    if app.search_active {
        handle_search(app, code);
        return;
    }

    if app.is_typing() {
        handle_typing(app, code);
    } else {
        handle_browse(app, code);
    }
}

/// Handle key events while in search mode.
fn handle_search(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.clear_search();
        }
        KeyCode::Backspace => {
            app.search_backspace();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.cursor_down();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.cursor_up();
        }
        KeyCode::Enter => {
            // Confirm the search: leave search mode with the cursor on the
            // first match (which is already there), and return to browse.
            app.clear_search();
        }
        KeyCode::Char(c) => {
            app.search_type_char(c);
        }
        _ => {}
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

        // Enter search mode.
        KeyCode::Char('/') => {
            app.enter_search();
        }

        // Quit.
        KeyCode::Char('q') | KeyCode::Esc => {
            app.request_quit();
        }

        _ => {}
    }
}

// ---------------------------------------------------------------------------
// `init` subcommand
// ---------------------------------------------------------------------------

/// Generate a default config file for the named network.
fn run_init(network_str: &str, out: Option<&Path>) -> Result<()> {
    let network = Network::from_str(network_str).with_context(|| {
        format!(
            "unknown network '{}' — valid values are: mainnet, preview, preprod",
            network_str
        )
    })?;

    let map = schema::network_defaults(network);
    let json = serde_json::Value::Object(map);
    let mut pretty =
        serde_json::to_string_pretty(&json).context("serialising default config to JSON")?;
    pretty.push('\n');

    match out {
        Some(path) => {
            std::fs::write(path, &pretty)
                .with_context(|| format!("writing config to '{}'", path.display()))?;
            eprintln!(
                "Wrote default {} config to '{}'",
                network_str,
                path.display()
            );
        }
        None => {
            print!("{pretty}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `validate` subcommand
// ---------------------------------------------------------------------------

/// Validate a config file against the parameter schema.
///
/// Validation rules:
/// - Must be a valid JSON object.
/// - Every known key must have a value that passes [`schema::ParamType::validate`].
/// - Unknown keys are reported as warnings (not errors).
fn run_validate(path: &Path) -> Result<()> {
    let loaded =
        load_config(path).with_context(|| format!("loading config file '{}'", path.display()))?;

    let lookup = build_lookup();
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for entry in &loaded.entries {
        match lookup.get(entry.key.as_str()) {
            Some(def) => {
                // Known parameter — validate the value.
                let raw = entry.display_value();
                if let Err(msg) = def.param_type.validate(&raw) {
                    errors.push(format!("  '{}': {}", entry.key, msg));
                }
            }
            None => {
                warnings.push(format!(
                    "  '{}': unknown parameter (not in schema)",
                    entry.key
                ));
            }
        }
    }

    if !warnings.is_empty() {
        eprintln!("Warnings:");
        for w in &warnings {
            eprintln!("{w}");
        }
    }

    if errors.is_empty() {
        eprintln!(
            "OK — '{}' is valid ({} parameters, {} unknown).",
            path.display(),
            loaded.entries.len(),
            warnings.len()
        );
        Ok(())
    } else {
        eprintln!("Errors:");
        for e in &errors {
            eprintln!("{e}");
        }
        // Non-zero exit via anyhow bail.
        anyhow::bail!(
            "'{}' failed validation: {} error(s)",
            path.display(),
            errors.len()
        );
    }
}

// ---------------------------------------------------------------------------
// `get` subcommand
// ---------------------------------------------------------------------------

/// Print the current value of a single parameter.
fn run_get(key: &str, path: &Path, verbose: bool) -> Result<()> {
    let loaded =
        load_config(path).with_context(|| format!("loading config file '{}'", path.display()))?;

    let entry = loaded
        .entries
        .iter()
        .find(|e| e.key == key)
        .with_context(|| format!("key '{}' not found in '{}'", key, path.display()))?;

    if verbose {
        let lookup = build_lookup();
        if let Some(def) = lookup.get(key) {
            println!("Key:         {}", def.key);
            println!("Type:        {}", def.param_type.label());
            if !def.default.is_empty() {
                println!("Default:     {}", def.default);
            }
            println!("Section:     {}", def.section);
            println!("Description: {}", def.description);
            if !def.tuning_hint.is_empty() {
                println!("Hint:        {}", def.tuning_hint);
            }
            println!();
        }
    }

    println!("{}", entry.display_value());
    Ok(())
}

// ---------------------------------------------------------------------------
// `set` subcommand
// ---------------------------------------------------------------------------

/// Update the value of a single parameter in a config file.
fn run_set(key: &str, value: &str, path: &Path) -> Result<()> {
    let mut loaded =
        load_config(path).with_context(|| format!("loading config file '{}'", path.display()))?;

    // Validate the value against the schema if the key is known.
    let lookup = build_lookup();
    if let Some(def) = lookup.get(key) {
        def.param_type
            .validate(value)
            .map_err(|msg| anyhow::anyhow!("invalid value for '{}': {}", key, msg))?;
    }

    // Find and update the entry.
    let entry = loaded
        .entries
        .iter_mut()
        .find(|e| e.key == key)
        .with_context(|| format!("key '{}' not found in '{}'", key, path.display()))?;

    entry
        .apply_edit(value)
        .with_context(|| format!("applying value '{}' to key '{}'", value, key))?;

    // Save (creates .bak backup automatically).
    config::save_config(&mut loaded)
        .with_context(|| format!("saving config file '{}'", path.display()))?;

    eprintln!("Set '{}' = '{}' in '{}'", key, value, path.display());
    Ok(())
}

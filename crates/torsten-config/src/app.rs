//! Application state for the Torsten config editor TUI.
//!
//! The [`App`] struct is the single source of truth for every piece of
//! mutable state visible to the UI:
//!
//! - The loaded config (sections and parameters).
//! - Cursor position (which section and which parameter within it).
//! - Edit mode state (are we editing a value right now? what has been typed?).
//! - Collapsed/expanded section state.
//! - Unsaved-changes flag (derived from [`LoadedConfig::is_modified`]).
//! - Quit-requested flag.
//! - Feedback message (shown in the footer for one frame after an action).
//!
//! # Section / item model
//!
//! After loading, parameters are grouped into [`Section`]s ordered by the
//! canonical section priority defined in [`crate::schema`].  Each section
//! holds a list of [`Item`]s — one per key found in the config file.
//!
//! The cursor is a `(section_index, item_index)` pair.  When a section is
//! collapsed the cursor skips over all its items.

use std::collections::HashMap;

use crate::config::{ConfigEntry, LoadedConfig};
use crate::schema::{build_lookup, section_priority, ParamDef, ParamType, SECTION_UNKNOWN};

// ---------------------------------------------------------------------------
// Section / item model
// ---------------------------------------------------------------------------

/// A single parameter row in the left-panel tree.
#[derive(Debug)]
pub struct Item {
    /// Index into [`LoadedConfig::entries`] for this item.
    pub entry_idx: usize,
    /// The resolved parameter definition, if the key is known.
    pub def: Option<&'static ParamDef>,
}

/// A logical group of parameters shown as a collapsible section in the tree.
#[derive(Debug)]
pub struct Section {
    /// Display name (e.g. "Network", "Genesis", "Unknown").
    pub name: String,
    /// Parameters belonging to this section, in definition order then file order.
    pub items: Vec<Item>,
    /// Whether the section body is currently visible (true = expanded).
    pub expanded: bool,
}

// ---------------------------------------------------------------------------
// Edit mode
// ---------------------------------------------------------------------------

/// The current editing state for a parameter row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditMode {
    /// Normal browse mode — cursor moves but nothing is being edited.
    None,
    /// User is typing a new value for the selected parameter.
    Typing {
        /// Accumulated key strokes so far.
        buffer: String,
        /// Optional validation error from the last keystroke.
        error: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Complete mutable state for the torsten-config TUI.
pub struct App {
    /// The loaded configuration file.
    pub config: LoadedConfig,
    /// Parameter groups, in canonical display order.
    pub sections: Vec<Section>,
    /// Index of the currently highlighted section.
    pub cursor_section: usize,
    /// Index of the currently highlighted item within the active section.
    pub cursor_item: usize,
    /// Current edit mode.
    pub edit_mode: EditMode,
    /// Message to display in the footer (cleared after one render pass).
    pub feedback: Option<String>,
    /// Set to `true` when the user presses `q` without unsaved changes, or
    /// confirms the quit prompt.
    pub should_quit: bool,
    /// Set to `true` after `q` when there are unsaved changes — triggers the
    /// "unsaved changes — press q again to discard" prompt.
    pub quit_prompt: bool,
}

impl App {
    /// Construct an [`App`] from a loaded config file.
    ///
    /// All sections start expanded.  The cursor starts at section 0, item 0.
    pub fn new(config: LoadedConfig) -> Self {
        let lookup = build_lookup();
        let sections = build_sections(&config, &lookup);

        App {
            config,
            sections,
            cursor_section: 0,
            cursor_item: 0,
            edit_mode: EditMode::None,
            feedback: None,
            should_quit: false,
            quit_prompt: false,
        }
    }

    // -----------------------------------------------------------------------
    // Cursor navigation
    // -----------------------------------------------------------------------

    /// Move the cursor to the previous visible row (vim `k` / arrow-up).
    pub fn cursor_up(&mut self) {
        if self.edit_mode != EditMode::None {
            return;
        }
        let (sec, item) = (self.cursor_section, self.cursor_item);

        if item > 0 {
            // Move up within the current section.
            self.cursor_item -= 1;
        } else if sec > 0 {
            // Move to the last item of the previous section.
            self.cursor_section -= 1;
            // If the previous section is expanded, land on its last item;
            // if collapsed, land on "item 0" (the header row).
            let prev = &self.sections[self.cursor_section];
            if prev.expanded && !prev.items.is_empty() {
                self.cursor_item = prev.items.len() - 1;
            } else {
                self.cursor_item = 0;
            }
        }
        // If the landed section is collapsed, cursor_item is always 0
        // (the section header). This is fine — the section header is always
        // visible even when collapsed.
    }

    /// Move the cursor to the next visible row (vim `j` / arrow-down).
    pub fn cursor_down(&mut self) {
        if self.edit_mode != EditMode::None {
            return;
        }
        let sec = self.cursor_section;
        let expanded = self.sections[sec].expanded;
        let item_count = self.sections[sec].items.len();

        if expanded && self.cursor_item + 1 < item_count {
            // Move down within the current expanded section.
            self.cursor_item += 1;
        } else if sec + 1 < self.sections.len() {
            // Move to the first item of the next section.
            self.cursor_section += 1;
            self.cursor_item = 0;
        }
    }

    // -----------------------------------------------------------------------
    // Section collapse / expand
    // -----------------------------------------------------------------------

    /// Toggle the collapsed / expanded state of the currently focused section.
    pub fn toggle_section(&mut self) {
        if self.edit_mode != EditMode::None {
            return;
        }
        let sec = self.cursor_section;
        self.sections[sec].expanded = !self.sections[sec].expanded;
        // When collapsing, reset item cursor to the section header.
        if !self.sections[sec].expanded {
            self.cursor_item = 0;
        }
    }

    // -----------------------------------------------------------------------
    // Edit mode
    // -----------------------------------------------------------------------

    /// Enter edit mode for the currently selected item.
    ///
    /// - Booleans and enums are toggled/cycled immediately (no typing buffer).
    /// - Strings, numbers, and paths open the typing buffer pre-filled with
    ///   the current value.
    pub fn begin_edit(&mut self) {
        if self.edit_mode != EditMode::None {
            return;
        }
        let Some(item) = self.selected_item() else {
            return;
        };
        let entry = &self.config.entries[item.entry_idx];
        let def = item.def;

        match def.map(|d| &d.param_type) {
            Some(ParamType::Bool) => {
                // Instant toggle — no typing buffer needed.
                let idx = item.entry_idx;
                if let Err(e) = self.config.entries[idx].toggle_bool() {
                    self.feedback = Some(format!("Toggle failed: {e}"));
                } else {
                    let new_val = self.config.entries[idx].display_value();
                    self.feedback = Some(format!("Set to {new_val}"));
                }
            }
            Some(ParamType::Enum { values }) => {
                // Instant cycle through enum choices.
                let choices: Vec<&str> = values.to_vec();
                let idx = item.entry_idx;
                self.config.entries[idx].cycle_enum(&choices);
                let new_val = self.config.entries[idx].display_value();
                self.feedback = Some(format!("Set to {new_val}"));
            }
            _ => {
                // Open typing buffer pre-filled with current value.
                let current = entry.display_value();
                self.edit_mode = EditMode::Typing {
                    buffer: current,
                    error: None,
                };
            }
        }
    }

    /// Append a character to the active typing buffer.
    pub fn type_char(&mut self, c: char) {
        if let EditMode::Typing { buffer, error } = &mut self.edit_mode {
            buffer.push(c);
            *error = None;
        }
    }

    /// Remove the last character from the active typing buffer (backspace).
    pub fn backspace(&mut self) {
        if let EditMode::Typing { buffer, .. } = &mut self.edit_mode {
            buffer.pop();
        }
    }

    /// Confirm the current typing buffer and apply it to the selected entry.
    ///
    /// On validation failure, the error is stored in the typing buffer so the
    /// footer can display it — the edit mode stays open.
    pub fn confirm_edit(&mut self) {
        let EditMode::Typing { buffer, .. } = &self.edit_mode else {
            return;
        };
        let raw = buffer.clone();

        // Validate via schema if a definition is available.
        let Some(item) = self.selected_item() else {
            self.cancel_edit();
            return;
        };
        let def = item.def;
        let entry_idx = item.entry_idx;

        if let Some(def) = def {
            if let Err(msg) = def.param_type.validate(&raw) {
                // Store the error in the buffer — stays in edit mode.
                if let EditMode::Typing { error, .. } = &mut self.edit_mode {
                    *error = Some(msg);
                }
                return;
            }
        }

        // Apply the edit.
        if let Err(e) = self.config.entries[entry_idx].apply_edit(&raw) {
            if let EditMode::Typing { error, .. } = &mut self.edit_mode {
                *error = Some(e.to_string());
            }
            return;
        }

        self.edit_mode = EditMode::None;
        self.feedback = Some(format!("Updated '{}'", self.config.entries[entry_idx].key));
    }

    /// Discard the current edit and return to browse mode.
    pub fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::None;
    }

    // -----------------------------------------------------------------------
    // Save
    // -----------------------------------------------------------------------

    /// Save the config file to disk.
    ///
    /// On success, clears the feedback after one render.  On failure, reports
    /// the error in the feedback line.
    pub fn save(&mut self) {
        match crate::config::save_config(&mut self.config) {
            Ok(()) => {
                self.feedback = Some(format!("Saved to '{}'", self.config.path.display()));
            }
            Err(e) => {
                self.feedback = Some(format!("Save failed: {e}"));
            }
        }
        // Also reset the quit_prompt (the file is clean now).
        self.quit_prompt = false;
    }

    // -----------------------------------------------------------------------
    // Quit handling
    // -----------------------------------------------------------------------

    /// Handle a quit request from the user.
    ///
    /// If there are unsaved changes, set `quit_prompt` so the UI can warn.
    /// If `quit_prompt` is already set (second press), discard changes and quit.
    /// If there are no unsaved changes, quit immediately.
    pub fn request_quit(&mut self) {
        if !self.config.is_modified() {
            self.should_quit = true;
        } else if self.quit_prompt {
            // Second press — discard and quit.
            self.should_quit = true;
        } else {
            self.quit_prompt = true;
            self.feedback =
                Some("Unsaved changes — press Ctrl+S to save or q again to discard".into());
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Return a reference to the currently selected [`Item`], if any.
    pub fn selected_item(&self) -> Option<&Item> {
        let sec = self.sections.get(self.cursor_section)?;
        if sec.expanded {
            sec.items.get(self.cursor_item)
        } else {
            // Collapsed section — no item is selected.
            None
        }
    }

    /// Return a reference to the currently selected [`ConfigEntry`], if any.
    ///
    /// Used by tests and callers that need the raw entry without going through
    /// the section/item indirection.
    #[allow(dead_code)]
    pub fn selected_entry(&self) -> Option<&ConfigEntry> {
        let item = self.selected_item()?;
        self.config.entries.get(item.entry_idx)
    }

    /// Return whether the config has unsaved changes.
    pub fn is_modified(&self) -> bool {
        self.config.is_modified()
    }

    /// Return whether the app is currently in text-input mode.
    pub fn is_typing(&self) -> bool {
        matches!(self.edit_mode, EditMode::Typing { .. })
    }

    /// Return the current typing buffer contents (empty string if not typing).
    pub fn typing_buffer(&self) -> &str {
        match &self.edit_mode {
            EditMode::Typing { buffer, .. } => buffer,
            EditMode::None => "",
        }
    }

    /// Return the current typing validation error, if any.
    pub fn typing_error(&self) -> Option<&str> {
        match &self.edit_mode {
            EditMode::Typing { error, .. } => error.as_deref(),
            EditMode::None => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Section builder
// ---------------------------------------------------------------------------

/// Group the config entries into [`Section`]s and sort them.
///
/// Steps:
/// 1. For each entry, look up its section via the schema lookup table.
/// 2. Group entries by section name.
/// 3. Sort sections by canonical priority.
fn build_sections(
    config: &LoadedConfig,
    lookup: &HashMap<&'static str, &'static ParamDef>,
) -> Vec<Section> {
    // Map section name -> list of items (in file order).
    let mut section_map: HashMap<String, Vec<Item>> = HashMap::new();

    for (entry_idx, entry) in config.entries.iter().enumerate() {
        let def = lookup.get(entry.key.as_str()).copied();
        let section_name = def
            .map(|d| d.section.to_string())
            .unwrap_or_else(|| SECTION_UNKNOWN.to_string());

        section_map
            .entry(section_name)
            .or_default()
            .push(Item { entry_idx, def });
    }

    // Sort sections by priority, then alphabetically within the same priority.
    let mut names: Vec<String> = section_map.keys().cloned().collect();
    names.sort_by(|a, b| {
        let pa = section_priority(a.as_str());
        let pb = section_priority(b.as_str());
        pa.cmp(&pb).then(a.cmp(b))
    });

    names
        .into_iter()
        .map(|name| Section {
            items: section_map.remove(&name).unwrap_or_default(),
            name,
            expanded: true,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_config;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_app(json: &str) -> App {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f.flush().unwrap();
        let config = load_config(f.path()).unwrap();
        // Keep the file alive for the duration of the test.
        std::mem::forget(f);
        App::new(config)
    }

    #[test]
    fn test_cursor_down_up() {
        let mut app =
            make_app(r#"{"EnableP2P": true, "MinSeverity": "Info", "Protocol": "Cardano"}"#);
        // All items land in their known sections; at least one section has items.
        let initial_sec = app.cursor_section;
        let initial_item = app.cursor_item;
        app.cursor_down();
        // Either moved within section or to next section.
        let moved = app.cursor_section != initial_sec || app.cursor_item != initial_item;
        assert!(moved, "cursor_down should move the cursor");

        app.cursor_up();
        assert_eq!(
            (app.cursor_section, app.cursor_item),
            (initial_sec, initial_item),
            "cursor_up should undo the down movement"
        );
    }

    #[test]
    fn test_toggle_section_collapses_and_expands() {
        let mut app = make_app(r#"{"EnableP2P": true}"#);
        assert!(app.sections[0].expanded);
        app.toggle_section();
        assert!(!app.sections[0].expanded);
        app.toggle_section();
        assert!(app.sections[0].expanded);
    }

    #[test]
    fn test_begin_edit_bool_toggles_immediately() {
        let mut app = make_app(r#"{"EnableP2P": true}"#);
        // Find EnableP2P.
        app.begin_edit();
        // The edit should have completed immediately (bool toggle).
        assert_eq!(app.edit_mode, EditMode::None);
        // Value should be flipped.
        let entry = app.selected_entry().unwrap();
        assert_eq!(entry.display_value(), "false");
    }

    #[test]
    fn test_begin_edit_string_opens_buffer() {
        // ShelleyGenesisFile is a Path type, so begin_edit should open the typing buffer.
        let mut app = make_app(r#"{"ShelleyGenesisFile": "shelley-genesis.json"}"#);
        app.begin_edit();
        assert!(app.is_typing());
        assert_eq!(app.typing_buffer(), "shelley-genesis.json");
    }

    #[test]
    fn test_type_and_confirm_string() {
        let mut app = make_app(r#"{"ShelleyGenesisFile": "old.json"}"#);
        app.begin_edit(); // Path type — opens buffer.
        assert!(app.is_typing());
        // Clear buffer and type new value.
        app.backspace(); // Remove 'n'
                         // Just confirm "old.jso" (abbreviated) to test the flow.
        app.confirm_edit();
        assert_eq!(app.edit_mode, EditMode::None);
    }

    #[test]
    fn test_cancel_edit() {
        let mut app = make_app(r#"{"ShelleyGenesisFile": "old.json"}"#);
        app.begin_edit();
        assert!(app.is_typing());
        app.cancel_edit();
        assert_eq!(app.edit_mode, EditMode::None);
        // Value should be unchanged.
        let entry = app.selected_entry().unwrap();
        assert_eq!(entry.display_value(), "old.json");
    }

    #[test]
    fn test_request_quit_with_unsaved_changes() {
        let mut app = make_app(r#"{"EnableP2P": true}"#);
        // Modify something.
        app.begin_edit(); // bool toggle
        assert!(app.is_modified());
        // First quit press: sets quit_prompt.
        app.request_quit();
        assert!(!app.should_quit);
        assert!(app.quit_prompt);
        // Second quit press: quits.
        app.request_quit();
        assert!(app.should_quit);
    }

    #[test]
    fn test_request_quit_clean_quits_immediately() {
        let mut app = make_app(r#"{"EnableP2P": true}"#);
        assert!(!app.is_modified());
        app.request_quit();
        assert!(app.should_quit);
        assert!(!app.quit_prompt);
    }

    #[test]
    fn test_sections_are_ordered() {
        let app =
            make_app(r#"{"EnableP2P": true, "MinSeverity": "Info", "ByronGenesisFile": "b.json"}"#);
        // EnableP2P -> Network, MinSeverity -> Logging, ByronGenesisFile -> Genesis
        // Expected order: Network, Genesis, Logging
        let names: Vec<&str> = app.sections.iter().map(|s| s.name.as_str()).collect();
        let net_pos = names.iter().position(|n| *n == "Network").unwrap();
        let gen_pos = names.iter().position(|n| *n == "Genesis").unwrap();
        let log_pos = names.iter().position(|n| *n == "Logging").unwrap();
        assert!(net_pos < gen_pos, "Network before Genesis");
        assert!(gen_pos < log_pos, "Genesis before Logging");
    }
}

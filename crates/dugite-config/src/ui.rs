//! TUI rendering for the Dugite config editor.
//!
//! # Layout
//!
//! ```text
//! ┌─── Header (1 line) ───────────────────────────────────────────┐
//! │ dugite-config  preview-config.json  [Modified]               │
//! ├─── Search bar (1 line, shown only when search is active) ──────┤
//! │  /EnableP2P_                                                  │
//! ├─── Left panel (60%) ──────────────────────────────────────────┤
//! │  Network                           (section header)           │
//! │    EnableP2P                     true  (modified *)           │
//! │    TargetNumberOfActivePeers       15                         │
//! │  Genesis                                                      │
//! │    ByronGenesisFile    preview-byron-genesis.json             │
//! │  ...                                                          │
//! ├─── Right panel (40%) ─────────────────────────────────────────┤
//! │  Key: EnableP2P                                               │
//! │  Type: bool          Default: true                            │
//! │                                                               │
//! │  Enable the Ouroboros P2P networking                          │
//! │  stack...                                                     │
//! │                                                               │
//! │  Hint: Always enable for production...                        │
//! ├─── Footer (1 line) ───────────────────────────────────────────┤
//! │  j/k Navigate  Enter Edit  Tab Expand/Collapse  Ctrl+S Save  │
//! └───────────────────────────────────────────────────────────────┘
//! ```
//!
//! When the terminal is narrower than 80 columns, the right panel is hidden
//! and the left panel expands to the full width.
//!
//! A diff overlay (`Ctrl+D`) renders on top of the body area, showing only
//! the changed parameters with original vs. current values side by side.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Section};
use crate::config::ConfigEntry;
use crate::diff::DiffEntry;
use crate::schema::{ParamDef, ParamType, SECTION_UNKNOWN};
use crate::search::highlight_ranges;

// ---------------------------------------------------------------------------
// Color palette (hard-coded Slate theme matching dugite-monitor)
// ---------------------------------------------------------------------------

const C_BG: Color = Color::Rgb(24, 24, 32);
const C_FG: Color = Color::Rgb(230, 230, 240);
const C_MUTED: Color = Color::Rgb(160, 160, 170);
const C_ACCENT: Color = Color::Rgb(100, 149, 237); // cornflower blue
const C_SUCCESS: Color = Color::Rgb(80, 220, 100);
const C_WARNING: Color = Color::Rgb(255, 215, 0);
const C_ERROR: Color = Color::Rgb(255, 80, 80);
const C_BORDER: Color = Color::Rgb(70, 70, 85);
const C_BORDER_ACTIVE: Color = Color::Rgb(100, 149, 237);
const C_SECTION_HDR: Color = Color::Rgb(180, 200, 255); // light periwinkle
const C_MODIFIED: Color = Color::Rgb(255, 165, 80); // orange
const C_HINT: Color = Color::Rgb(130, 200, 130); // soft green for tuning hints
const C_SEARCH_BAR: Color = Color::Rgb(60, 60, 80); // dark highlight for search bar
const C_SEARCH_MATCH: Color = Color::Rgb(255, 220, 80); // amber for highlighted match text
const C_DIFF_ORIG: Color = Color::Rgb(255, 100, 100); // red for removed/original value
const C_DIFF_NEW: Color = Color::Rgb(80, 220, 100); // green for new/current value

/// Minimum terminal width to show the right (description) panel.
const MIN_WIDTH_TWO_PANEL: u16 = 80;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Render the complete editor UI onto `frame`.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Fill background.
    frame.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(C_BG)),
        area,
    );

    // Vertical split: header | [search bar] | body | footer.
    let rows = if app.search_active {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(1), // search bar
                Constraint::Min(0),    // body
                Constraint::Length(1), // footer
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(0), // (no search bar)
                Constraint::Min(0),    // body
                Constraint::Length(1), // footer
            ])
            .split(area)
    };

    let header_area = rows[0];
    let search_area = rows[1];
    let body_area = rows[2];
    let footer_area = rows[3];

    draw_header(frame, app, header_area);
    if app.search_active {
        draw_search_bar(frame, app, search_area);
    }
    draw_body(frame, app, body_area);
    draw_footer(frame, app, footer_area);

    // Overlays rendered last (highest z-order).
    if app.show_diff {
        draw_diff_overlay(frame, app, area);
    } else if app.quit_prompt {
        draw_quit_overlay(frame, area);
    }
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let filename = app
        .config
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)");

    let modified_span = if app.is_modified() {
        Span::styled(
            "  [Modified]",
            Style::default().fg(C_MODIFIED).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("", Style::default())
    };

    let line = Line::from(vec![
        Span::styled(
            " dugite-config  ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            filename,
            Style::default().fg(C_FG).add_modifier(Modifier::BOLD),
        ),
        modified_span,
    ]);

    frame.render_widget(Paragraph::new(line).style(Style::default().bg(C_BG)), area);
}

// ---------------------------------------------------------------------------
// Search bar
// ---------------------------------------------------------------------------

/// Render the single-line search bar shown while search is active.
fn draw_search_bar(frame: &mut Frame, app: &App, area: Rect) {
    let match_count = app.filtered_items.len();
    let count_str = if app.search_query.is_empty() {
        String::new()
    } else {
        format!(
            "  ({match_count} match{})",
            if match_count == 1 { "" } else { "es" }
        )
    };

    let line = Line::from(vec![
        Span::styled(
            " / ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.search_query.clone(),
            Style::default().fg(C_FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "_",
            Style::default()
                .fg(C_ACCENT)
                .add_modifier(Modifier::RAPID_BLINK),
        ),
        Span::styled(count_str, Style::default().fg(C_MUTED)),
        Span::styled("   Esc: clear search", Style::default().fg(C_MUTED)),
    ]);

    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(C_SEARCH_BAR)),
        area,
    );
}

// ---------------------------------------------------------------------------
// Body
// ---------------------------------------------------------------------------

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    let use_two_panel = area.width >= MIN_WIDTH_TWO_PANEL;

    if use_two_panel {
        // Horizontal split: left 60% | right 40%.
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        draw_left_panel(frame, app, cols[0]);
        draw_right_panel(frame, app, cols[1]);
    } else {
        draw_left_panel(frame, app, area);
    }
}

// ---------------------------------------------------------------------------
// Left panel — parameter tree
// ---------------------------------------------------------------------------

fn draw_left_panel(frame: &mut Frame, app: &App, area: Rect) {
    let border_color = if !app.is_typing() && !app.search_active {
        C_BORDER_ACTIVE
    } else if app.search_active {
        C_WARNING
    } else {
        C_BORDER
    };

    // Title shows the search query inline when searching.
    let title = if app.search_active && !app.search_query.is_empty() {
        Span::styled(
            format!(" Parameters (/{}) ", app.search_query),
            Style::default().fg(C_WARNING).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " Parameters ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title)
        .style(Style::default().bg(C_BG))
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build the flat list of lines.
    let items = build_list_items(app, inner.width);
    let list = List::new(items).style(Style::default().bg(C_BG).fg(C_FG));
    frame.render_widget(list, inner);
}

/// Build the flat list of [`ListItem`]s for every visible row.
///
/// In normal mode the full tree is rendered.  In search mode only matching
/// items are rendered (the section header is still shown above each match for
/// context, but collapsed sections are treated as expanded for display
/// purposes so that filtered results inside them are still visible).
fn build_list_items(app: &App, panel_width: u16) -> Vec<ListItem<'static>> {
    // Usable width inside the block (border already subtracted by `inner`).
    let w = panel_width as usize;
    let mut items: Vec<ListItem<'static>> = Vec::new();

    // Flat row index for cursor tracking.
    let mut flat_row: usize = 0;
    let cursor_flat = cursor_to_flat(app);

    // If search is active and there are results, render a flat filtered list.
    if app.search_active && !app.search_query.is_empty() {
        return build_search_items(app, w);
    }

    for (sec_idx, section) in app.sections.iter().enumerate() {
        // --- Section header row ---
        let is_cursor_on_section_header = flat_row == cursor_flat && sec_idx == app.cursor_section;

        let section_item = render_section_header(section, is_cursor_on_section_header, w);
        items.push(section_item);
        flat_row += 1;

        if !section.expanded {
            continue;
        }

        // --- Parameter rows ---
        for (item_idx, item) in section.items.iter().enumerate() {
            let is_selected = sec_idx == app.cursor_section && item_idx == app.cursor_item;
            let is_cursor = flat_row == cursor_flat && is_selected;

            let entry = &app.config.entries[item.entry_idx];
            let display_value = if is_selected && app.is_typing() {
                app.typing_buffer().to_string()
            } else {
                entry.display_value()
            };

            let row = render_item_row(
                entry,
                item.def,
                &display_value,
                is_cursor,
                is_selected && app.is_typing(),
                &[], // no highlights in normal mode
                w,
            );
            items.push(row);
            flat_row += 1;
        }
    }

    items
}

/// Build the filtered result list shown during active search.
fn build_search_items(app: &App, w: usize) -> Vec<ListItem<'static>> {
    let mut items: Vec<ListItem<'static>> = Vec::new();

    if app.filtered_items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  No matches",
            Style::default().fg(C_MUTED),
        ))));
        return items;
    }

    for &(sec_idx, item_idx) in &app.filtered_items {
        let section = &app.sections[sec_idx];
        let item = &section.items[item_idx];
        let entry = &app.config.entries[item.entry_idx];
        let is_cursor = sec_idx == app.cursor_section && item_idx == app.cursor_item;

        let display_value = if is_cursor && app.is_typing() {
            app.typing_buffer().to_string()
        } else {
            entry.display_value()
        };

        // Compute highlight ranges for the key.
        let key_ranges = highlight_ranges(&app.search_query, &entry.key);

        // Prepend a section tag in muted text for context.
        items.push(ListItem::new(Line::from(Span::styled(
            format!("  [{}]", section.name),
            Style::default().fg(C_MUTED),
        ))));

        let row = render_item_row(
            entry,
            item.def,
            &display_value,
            is_cursor,
            is_cursor && app.is_typing(),
            &key_ranges,
            w,
        );
        items.push(row);
    }

    items
}

/// Render a section header row.
fn render_section_header(section: &Section, is_cursor: bool, width: usize) -> ListItem<'static> {
    let arrow = if section.expanded { "v" } else { ">" };
    let label = format!("{arrow} {}", section.name);

    // Pad the label to the full panel width so the highlight bar spans it.
    let padded = format!("{label:<width$}", width = width.saturating_sub(0));

    let style = if is_cursor {
        Style::default()
            .fg(C_BG)
            .bg(C_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(C_SECTION_HDR)
            .add_modifier(Modifier::BOLD)
    };

    ListItem::new(Line::from(Span::styled(padded, style)))
}

/// Render a single parameter row, with optional search highlight ranges.
fn render_item_row(
    entry: &ConfigEntry,
    def: Option<&ParamDef>,
    display_value: &str,
    is_cursor: bool,
    is_typing: bool,
    key_ranges: &[(usize, usize)],
    width: usize,
) -> ListItem<'static> {
    // Key label (2-space indent).
    let key_label = format!("  {}", entry.key);

    // Value colour depends on type and state.
    let value_color = if is_typing {
        C_WARNING
    } else if entry.modified {
        C_MODIFIED
    } else {
        value_color_for(def, display_value)
    };

    // Modified indicator.
    let mod_indicator = if entry.modified { "*" } else { " " };

    // Reserve space for " * " suffix.
    let reserved = 3usize;
    let key_len = key_label.len().min(width.saturating_sub(reserved + 1));
    let key_display = if key_label.len() > key_len {
        format!("{:.key_len$}", key_label)
    } else {
        key_label.clone()
    };

    // Truncate value so row fits.
    let max_val_len = width
        .saturating_sub(key_display.len())
        .saturating_sub(reserved);
    let value_display = if display_value.len() > max_val_len && max_val_len > 3 {
        format!("{}...", &display_value[..max_val_len.saturating_sub(3)])
    } else if display_value.len() > max_val_len {
        display_value[..max_val_len].to_string()
    } else {
        display_value.to_string()
    };

    // Spacing between key and value.
    let gap = width
        .saturating_sub(key_display.len())
        .saturating_sub(value_display.len())
        .saturating_sub(2);

    let (bg, fg_key, fg_value) = if is_cursor {
        (C_ACCENT, C_BG, C_BG)
    } else {
        (C_BG, C_MUTED, value_color)
    };

    // Build the line.  When there are search highlight ranges and we are not
    // on the cursor row, we split the key into highlighted / non-highlighted
    // spans.  On the cursor row a single solid span is used.
    let line = if is_cursor || key_ranges.is_empty() {
        // Simple two-span line: key (muted) + value (colored).
        let key_part = format!("{key_display}{:gap$}", "");
        let val_part = format!("{value_display} {mod_indicator}");

        if is_cursor {
            Line::from(Span::styled(
                format!("{key_part}{val_part}"),
                Style::default()
                    .fg(fg_key)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(vec![
                Span::styled(key_part, Style::default().fg(fg_key).bg(bg)),
                Span::styled(
                    val_part,
                    Style::default()
                        .fg(fg_value)
                        .bg(bg)
                        .add_modifier(if entry.modified {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
            ])
        }
    } else {
        // Build a highlighted key with match ranges marked in amber.
        // `key_display` includes the 2-space indent; highlight ranges are
        // relative to `entry.key` (no indent), so offset by 2.
        let indent_offset = 2usize; // "  " prefix length
        let mut spans: Vec<Span<'static>> = Vec::new();
        let key_bytes = key_display.as_bytes();
        let mut cursor_pos: usize = 0;

        // Map ranges from entry.key offsets to key_display offsets.
        let adjusted: Vec<(usize, usize)> = key_ranges
            .iter()
            .filter_map(|&(s, e)| {
                let s2 = s + indent_offset;
                let e2 = e + indent_offset;
                if s2 < key_display.len() {
                    Some((s2, e2.min(key_display.len())))
                } else {
                    None
                }
            })
            .collect();

        for (hs, he) in &adjusted {
            if *hs > cursor_pos {
                spans.push(Span::styled(
                    key_display[cursor_pos..*hs].to_string(),
                    Style::default().fg(fg_key).bg(bg),
                ));
            }
            if *hs < key_bytes.len() {
                spans.push(Span::styled(
                    key_display[*hs..*he].to_string(),
                    Style::default()
                        .fg(C_SEARCH_MATCH)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            cursor_pos = *he;
        }
        if cursor_pos < key_display.len() {
            spans.push(Span::styled(
                key_display[cursor_pos..].to_string(),
                Style::default().fg(fg_key).bg(bg),
            ));
        }

        // Gap + value + modifier.
        spans.push(Span::styled(
            format!("{:gap$}", ""),
            Style::default().fg(fg_key).bg(bg),
        ));
        spans.push(Span::styled(
            format!("{value_display} {mod_indicator}"),
            Style::default()
                .fg(fg_value)
                .bg(bg)
                .add_modifier(if entry.modified {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));

        Line::from(spans)
    };

    ListItem::new(line)
}

/// Pick a color for the displayed value based on type and content.
fn value_color_for(def: Option<&ParamDef>, value: &str) -> Color {
    match def.map(|d| &d.param_type) {
        Some(ParamType::Bool) => {
            if value == "true" {
                C_SUCCESS
            } else {
                C_ERROR
            }
        }
        Some(ParamType::Path) => C_ACCENT,
        Some(ParamType::Enum { .. }) => Color::Rgb(200, 200, 255),
        Some(ParamType::U64 { .. }) => Color::Rgb(255, 200, 100),
        _ => C_FG,
    }
}

// ---------------------------------------------------------------------------
// Right panel — parameter description + tuning hint
// ---------------------------------------------------------------------------

fn draw_right_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_BORDER))
        .title(Span::styled(
            " Description ",
            Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(C_BG))
        .padding(Padding::new(1, 1, 1, 1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let content = build_description_content(app);
    let para = Paragraph::new(content)
        .wrap(Wrap { trim: true })
        .style(Style::default().bg(C_BG).fg(C_FG));
    frame.render_widget(para, inner);
}

/// Build the text content for the description panel, including tuning hints.
fn build_description_content(app: &App) -> Vec<Line<'static>> {
    let Some(item) = app.selected_item() else {
        return vec![Line::from(Span::styled(
            "No parameter selected",
            Style::default().fg(C_MUTED),
        ))];
    };

    let entry = &app.config.entries[item.entry_idx];
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Key name.
    lines.push(Line::from(vec![
        Span::styled("Key:  ", Style::default().fg(C_MUTED)),
        Span::styled(
            entry.key.clone(),
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ),
    ]));

    if let Some(def) = item.def {
        // Type row.
        lines.push(Line::from(vec![
            Span::styled("Type: ", Style::default().fg(C_MUTED)),
            Span::styled(def.param_type.label(), Style::default().fg(C_FG)),
        ]));

        // Enum values (if applicable).
        if let ParamType::Enum { values } = &def.param_type {
            let choices = values.join(" | ");
            lines.push(Line::from(vec![
                Span::styled("      ", Style::default()),
                Span::styled(choices, Style::default().fg(Color::Rgb(200, 200, 255))),
            ]));
        }

        // U64 range (if applicable).
        if let ParamType::U64 { min, max } = &def.param_type {
            lines.push(Line::from(vec![
                Span::styled("Range:", Style::default().fg(C_MUTED)),
                Span::styled(
                    format!(" {min}..{max}"),
                    Style::default().fg(Color::Rgb(255, 200, 100)),
                ),
            ]));
        }

        // Default value.
        if !def.default.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Def:  ", Style::default().fg(C_MUTED)),
                Span::styled(def.default, Style::default().fg(C_MUTED)),
            ]));
        }

        // Separator.
        lines.push(Line::from(""));

        // Description text (word-wrap is handled by Paragraph::wrap).
        lines.push(Line::from(Span::styled(
            def.description,
            Style::default().fg(C_FG),
        )));

        // Tuning hint (shown only when non-empty).
        if !def.tuning_hint.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Hint:",
                Style::default().fg(C_HINT).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                def.tuning_hint,
                Style::default().fg(C_HINT),
            )));
        }
    } else {
        // Unknown key.
        lines.push(Line::from(vec![
            Span::styled("Type: ", Style::default().fg(C_MUTED)),
            Span::styled("unknown (raw JSON)", Style::default().fg(C_MUTED)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "This key is not in the known parameter schema. \
             It will be preserved as-is and is editable as a plain string.",
            Style::default().fg(C_MUTED),
        )));
    }

    // Current value (with edit buffer if typing).
    lines.push(Line::from(""));
    let current = if app.is_typing() {
        format!("{}_", app.typing_buffer()) // cursor blink indicator
    } else {
        entry.display_value()
    };
    lines.push(Line::from(vec![
        Span::styled("Value:", Style::default().fg(C_MUTED)),
        Span::styled(
            format!(" {current}"),
            Style::default().fg(if app.is_typing() {
                C_WARNING
            } else if entry.modified {
                C_MODIFIED
            } else {
                C_SUCCESS
            }),
        ),
    ]));

    // Typing error (if any).
    if let Some(err) = app.typing_error() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(C_ERROR).add_modifier(Modifier::BOLD),
        )));
    }

    // Section tag.
    lines.push(Line::from(""));
    let section_name = item.def.map(|d| d.section).unwrap_or(SECTION_UNKNOWN);
    lines.push(Line::from(vec![
        Span::styled("Section: ", Style::default().fg(C_MUTED)),
        Span::styled(section_name, Style::default().fg(C_MUTED)),
    ]));

    lines
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let line = if app.search_active {
        // Search mode footer.
        Line::from(vec![
            key_hint("Type", "Search"),
            key_hint("Backspace", "Delete"),
            key_hint("j/k", "Navigate matches"),
            key_hint("Esc", "Clear search"),
        ])
    } else if app.is_typing() {
        // Typing mode footer.
        Line::from(vec![
            key_hint("Enter", "Confirm"),
            key_hint("Esc", "Cancel"),
            Span::styled(
                "  Typing mode — press Enter to apply, Esc to cancel",
                Style::default().fg(C_MUTED),
            ),
        ])
    } else {
        // Browse mode footer.
        let modified_hint = if app.is_modified() {
            Span::styled(
                "  [Modified — Ctrl+S to save]",
                Style::default().fg(C_MODIFIED).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("", Style::default())
        };

        Line::from(vec![
            key_hint("j/k", "Navigate"),
            key_hint("Enter", "Edit"),
            key_hint("Tab", "Expand"),
            key_hint("/", "Search"),
            key_hint("^D", "Diff"),
            key_hint("^S", "Save"),
            key_hint("q", "Quit"),
            modified_hint,
        ])
    };

    frame.render_widget(Paragraph::new(line).style(Style::default().bg(C_BG)), area);
}

/// Format a single key-hint pair for the footer.
fn key_hint(key: &'static str, action: &'static str) -> Span<'static> {
    Span::styled(format!("  {key}:{action}"), Style::default().fg(C_MUTED))
}

// ---------------------------------------------------------------------------
// Diff overlay
// ---------------------------------------------------------------------------

/// Render a full-width overlay showing original vs. current values for all
/// modified parameters.
fn draw_diff_overlay(frame: &mut Frame, app: &App, area: Rect) {
    // Centre a box occupying ~80% of the terminal width/height.
    let w = (area.width * 4 / 5).max(60).min(area.width);
    let h = (area.height * 4 / 5).max(10).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let overlay = Rect::new(x, y, w, h);

    frame.render_widget(Clear, overlay);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_ACCENT))
        .title(Span::styled(
            " Diff — modified parameters (Esc to close) ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(C_BG))
        .padding(Padding::new(1, 1, 1, 1));

    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    let diff = app.diff_entries();
    let content = build_diff_content(&diff, inner.width);

    let para = Paragraph::new(content)
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(C_BG));
    frame.render_widget(para, inner);
}

/// Build the diff content lines for rendering inside the overlay.
fn build_diff_content(diff: &[DiffEntry], panel_width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    if diff.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No changes.",
            Style::default().fg(C_MUTED),
        )));
        return lines;
    }

    // Header row.
    let col_key = 30usize.min(panel_width as usize / 3);
    let col_val = (panel_width as usize).saturating_sub(col_key * 2 + 6) / 2;

    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<col_key$}", "Parameter"),
            Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {:<col_val$}", "Original"),
            Style::default()
                .fg(C_DIFF_ORIG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {:<col_val$}", "Current"),
            Style::default().fg(C_DIFF_NEW).add_modifier(Modifier::BOLD),
        ),
    ]));

    // Separator.
    lines.push(Line::from(Span::styled(
        "-".repeat(panel_width as usize),
        Style::default().fg(C_BORDER),
    )));

    for entry in diff {
        let key_display = if entry.key.len() > col_key {
            format!("{:.col_key$}", entry.key)
        } else {
            format!("{:<col_key$}", entry.key)
        };
        let orig_display = if entry.original.len() > col_val {
            format!("{}...", &entry.original[..col_val.saturating_sub(3)])
        } else {
            format!("{:<col_val$}", entry.original)
        };
        let curr_display = if entry.current.len() > col_val {
            format!("{}...", &entry.current[..col_val.saturating_sub(3)])
        } else {
            format!("{:<col_val$}", entry.current)
        };

        lines.push(Line::from(vec![
            Span::styled(key_display, Style::default().fg(C_FG)),
            Span::styled(
                format!("  {orig_display}"),
                Style::default().fg(C_DIFF_ORIG),
            ),
            Span::styled(
                format!("  {curr_display}"),
                Style::default().fg(C_DIFF_NEW).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    lines
}

// ---------------------------------------------------------------------------
// Quit confirmation overlay
// ---------------------------------------------------------------------------

fn draw_quit_overlay(frame: &mut Frame, area: Rect) {
    // Centre a small box.
    let w: u16 = 52.min(area.width);
    let h: u16 = 5.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let overlay = Rect::new(x, y, w, h);

    frame.render_widget(Clear, overlay);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_WARNING))
        .title(Span::styled(
            " Unsaved Changes ",
            Style::default().fg(C_WARNING).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(C_BG));

    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    let para = Paragraph::new(vec![
        Line::from(Span::styled(
            "You have unsaved changes.",
            Style::default().fg(C_FG),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  Ctrl+S",
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Save and quit", Style::default().fg(C_MUTED)),
            Span::styled(
                "    q",
                Style::default().fg(C_ERROR).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Discard and quit", Style::default().fg(C_MUTED)),
        ]),
    ])
    .style(Style::default().bg(C_BG));

    frame.render_widget(para, inner);
}

// ---------------------------------------------------------------------------
// Cursor flat-index helper
// ---------------------------------------------------------------------------

/// Compute the flat list row index corresponding to `app.cursor_section` and
/// `app.cursor_item`.
///
/// The flat list interleaves section headers (one per section) and item rows
/// (one per visible item in expanded sections).
fn cursor_to_flat(app: &App) -> usize {
    let mut flat = 0usize;
    for (sec_idx, section) in app.sections.iter().enumerate() {
        if sec_idx == app.cursor_section {
            // cursor_item 0 = section header (flat_row = flat)
            // cursor_item 1+ = child items (flat_row = flat + 1 + cursor_item - 1)
            //                                         = flat + cursor_item
            // Wait — app uses cursor_item=0 for the first CHILD, not the header.
            // The section header is implicit: when cursor is on a section,
            // the header gets highlighted at flat_row, and items start at flat_row+1.
            //
            // Actually checking render_section_header vs parameter rows:
            // - Section header rendered at flat_row, then flat_row += 1
            // - Item 0 rendered at the NEXT flat_row
            //
            // So cursor_item=0 → first child item → needs flat + 1
            // But cursor on collapsed section also has cursor_item=0 → header
            if section.expanded && !section.items.is_empty() {
                // Cursor is on child item: +1 for header, +cursor_item for offset
                return flat + 1 + app.cursor_item;
            }
            // Cursor is on section header (collapsed or empty)
            return flat;
        }
        flat += 1; // section header
        if section.expanded {
            flat += section.items.len();
        }
    }
    flat
}

//! TUI rendering for the Torsten config editor.
//!
//! # Layout
//!
//! ```text
//! ┌─── Header (1 line) ───────────────────────────────────────────┐
//! │ torsten-config  preview-config.json  [Modified]               │
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
//! ├─── Footer (1 line) ───────────────────────────────────────────┤
//! │  j/k Navigate  Enter Edit  Tab Expand/Collapse  Ctrl+S Save  │
//! └───────────────────────────────────────────────────────────────┘
//! ```
//!
//! When the terminal is narrower than 80 columns, the right panel is hidden
//! and the left panel expands to the full width.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Section};
use crate::config::ConfigEntry;
use crate::schema::{ParamDef, ParamType, SECTION_UNKNOWN};

// ---------------------------------------------------------------------------
// Color palette (hard-coded Slate theme matching torsten-monitor)
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

    // Vertical split: header | body | footer.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    let header_area = rows[0];
    let body_area = rows[1];
    let footer_area = rows[2];

    draw_header(frame, app, header_area);
    draw_body(frame, app, body_area);
    draw_footer(frame, app, footer_area);

    // Quit confirmation overlay (rendered last so it sits on top).
    if app.quit_prompt {
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
            " torsten-config  ",
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
    let border_color = if !app.is_typing() {
        C_BORDER_ACTIVE
    } else {
        C_BORDER
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            " Parameters ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ))
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
/// The list is flat (no native tree widget) — sections are manually indented
/// and the cursor highlight is applied by inspecting `app.cursor_*`.
fn build_list_items(app: &App, panel_width: u16) -> Vec<ListItem<'static>> {
    // Usable width inside the block (border already subtracted by `inner`).
    let w = panel_width as usize;
    let mut items: Vec<ListItem<'static>> = Vec::new();

    // Current flat row index for cursor tracking.
    let mut flat_row: usize = 0;
    // We'll compare against the cursor position to decide whether to highlight.
    // `highlight_rows` is a set of flat indices that should be highlighted.
    // We compute the cursor's flat index here.
    let cursor_flat = cursor_to_flat(app);

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
                // Show the live typing buffer instead of the stored value.
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
                w,
            );
            items.push(row);
            flat_row += 1;
        }
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

/// Render a single parameter row.
fn render_item_row(
    entry: &ConfigEntry,
    def: Option<&ParamDef>,
    display_value: &str,
    is_cursor: bool,
    is_typing: bool,
    width: usize,
) -> ListItem<'static> {
    // Key label (2-space indent).
    let key_label = format!("  {}", entry.key);

    // Value colour depends on type and state.
    let value_color = if is_typing {
        C_WARNING // yellow while editing
    } else if entry.modified {
        C_MODIFIED
    } else {
        value_color_for(def, display_value)
    };

    // Modified indicator.
    let mod_indicator = if entry.modified { "*" } else { " " };

    // Right-align the value within the available space.
    // Row format: "  <key><spaces><value><mod>"
    // Reserve 2 chars for mod_indicator + space.
    let reserved = 3usize; // " * " suffix
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

    // Build the right side: spaces + value + modified marker.
    let gap = width
        .saturating_sub(key_display.len())
        .saturating_sub(value_display.len())
        .saturating_sub(2);

    let row_str = format!("{key_display}{:gap$}{value_display} {mod_indicator}", "");

    let (bg, fg_key, fg_value) = if is_cursor {
        (C_ACCENT, C_BG, C_BG)
    } else {
        (C_BG, C_MUTED, value_color)
    };

    // Build a two-span line: key (muted) + value (colored).
    let key_span_end = key_display.len();
    let value_span_start = key_span_end + gap + 1; // +1 for the leading space of gap
    let _ = value_span_start; // suppress unused-variable warning

    // For cursor highlight, we use a single uniform span.
    let line = if is_cursor {
        Line::from(Span::styled(
            row_str,
            Style::default()
                .fg(fg_key)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        // Split: key part (muted) and value part (colored).
        let key_part = format!("{key_display}{:gap$}", "");
        let val_part = format!("{value_display} {mod_indicator}");

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
// Right panel — parameter description
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

/// Build the text content for the description panel.
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
    let line = if app.is_typing() {
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
// Quit confirmation overlay
// ---------------------------------------------------------------------------

fn draw_quit_overlay(frame: &mut Frame, area: Rect) {
    // Centre a small box.
    let w: u16 = 52.min(area.width);
    let h: u16 = 5.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let overlay = Rect::new(x, y, w, h);

    // Clear the area under the overlay.
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
            if section.expanded {
                // cursor_item 0 means first item row, not the header.
                flat += app.cursor_item;
            }
            return flat;
        }
        flat += 1; // section header
        if section.expanded {
            flat += section.items.len();
        }
    }
    flat
}

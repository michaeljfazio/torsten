//! Custom sync progress bar widget with color-coded status indication.
//!
//! Renders a horizontal gauge showing chain sync progress with:
//! - Configurable fill color when synced (>= 99.9%)
//! - Configurable fill color when actively syncing
//! - Configurable fill color when stalled
//! - Percentage label centered in the bar

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

/// A sync progress bar widget that renders a filled gauge with color-coded status.
pub struct SyncProgressBar {
    /// Progress ratio from 0.0 to 100.0 (percentage).
    progress: f64,
    /// Whether the node is fully synced.
    is_synced: bool,
    /// Whether the node is stalled.
    is_stalled: bool,
    /// Fill color when synced.
    color_synced: Color,
    /// Fill color when syncing.
    color_syncing: Color,
    /// Fill color when stalled.
    color_stalled: Color,
    /// Color for the unfilled portion of the bar.
    color_empty: Color,
}

impl SyncProgressBar {
    /// Create a new progress bar.
    ///
    /// `progress` is the sync percentage (0.0 - 100.0).
    pub fn new(progress: f64, is_synced: bool, is_stalled: bool) -> Self {
        Self {
            progress,
            is_synced,
            is_stalled,
            color_synced: Color::Green,
            color_syncing: Color::Yellow,
            color_stalled: Color::Red,
            color_empty: Color::DarkGray,
        }
    }

    /// Set the fill color used when the node is synced.
    pub fn _fill_color_synced(mut self, color: Color) -> Self {
        self.color_synced = color;
        self
    }

    /// Set the fill color used when the node is actively syncing.
    pub fn _fill_color_syncing(mut self, color: Color) -> Self {
        self.color_syncing = color;
        self
    }

    /// Set the fill color used when the node is stalled.
    pub fn _fill_color_stalled(mut self, color: Color) -> Self {
        self.color_stalled = color;
        self
    }

    /// Set the color for the unfilled (empty) portion of the bar.
    pub fn _empty_color(mut self, color: Color) -> Self {
        self.color_empty = color;
        self
    }

    /// Determine the fill color based on sync state.
    fn bar_color(&self) -> Color {
        if self.is_synced {
            self.color_synced
        } else if self.is_stalled {
            self.color_stalled
        } else {
            self.color_syncing
        }
    }
}

impl Widget for SyncProgressBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 4 || area.height < 1 {
            return;
        }

        let color = self.bar_color();
        let ratio = (self.progress / 100.0).clamp(0.0, 1.0);
        let filled_width = ((area.width as f64) * ratio) as u16;

        // Render the filled portion
        for x in area.left()..area.left().saturating_add(filled_width) {
            if x < area.right() {
                buf[(x, area.top())]
                    .set_char('\u{2501}')
                    .set_style(Style::default().fg(color));
            }
        }

        // Render the unfilled portion
        for x in area.left().saturating_add(filled_width)..area.right() {
            buf[(x, area.top())]
                .set_char('\u{2501}')
                .set_style(Style::default().fg(self.color_empty));
        }

        // Render the percentage label centered
        let label = format!("{:.2}%", self.progress);
        let label_start = area.left() + area.width.saturating_sub(label.len() as u16) / 2;
        for (i, ch) in label.chars().enumerate() {
            let x = label_start + i as u16;
            if x < area.right() {
                let fg = if x < area.left().saturating_add(filled_width) {
                    Color::Black
                } else {
                    Color::White
                };
                buf[(x, area.top())]
                    .set_char(ch)
                    .set_style(Style::default().fg(fg).bg(
                        if x < area.left().saturating_add(filled_width) {
                            color
                        } else {
                            Color::Reset
                        },
                    ));
            }
        }
    }
}

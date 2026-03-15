//! Block rate sparkline widget using Unicode block characters.
//!
//! Renders a horizontal sparkline showing historical block processing rate,
//! using the Unicode block elements (U+2581..U+2588) for 8-level resolution.
//! Bars are colored with a 3-tier gradient based on their value relative to
//! the visible maximum: low (< 33%), mid (33-66%), high (> 66%).

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use std::collections::VecDeque;

/// Unicode block elements from lowest to highest (8 levels).
const SPARK_CHARS: [char; 8] = [
    '\u{2581}', // lower one eighth block
    '\u{2582}', // lower one quarter block
    '\u{2583}', // lower three eighths block
    '\u{2584}', // lower half block
    '\u{2585}', // lower five eighths block
    '\u{2586}', // lower three quarters block
    '\u{2587}', // lower seven eighths block
    '\u{2588}', // full block
];

/// A sparkline widget that displays a series of values as a compact bar chart
/// using Unicode block characters, with a 3-color gradient.
pub struct SparklineHistory<'a> {
    /// Reference to the data points.
    data: &'a VecDeque<u64>,
    /// Color for bars in the low range (< 33% of max).
    color_low: Color,
    /// Color for bars in the mid range (33-66% of max).
    color_mid: Color,
    /// Color for bars in the high range (> 66% of max).
    color_high: Color,
}

impl<'a> SparklineHistory<'a> {
    /// Create a new sparkline from a data source with default colors.
    pub fn new(data: &'a VecDeque<u64>) -> Self {
        Self {
            data,
            color_low: Color::Green,
            color_mid: Color::Yellow,
            color_high: Color::Red,
        }
    }

    /// Set the color for low-value bars (< 33% of max).
    pub fn spark_low(mut self, color: Color) -> Self {
        self.color_low = color;
        self
    }

    /// Set the color for mid-value bars (33-66% of max).
    pub fn spark_mid(mut self, color: Color) -> Self {
        self.color_mid = color;
        self
    }

    /// Set the color for high-value bars (> 66% of max).
    pub fn spark_high(mut self, color: Color) -> Self {
        self.color_high = color;
        self
    }

    /// Determine the bar color based on the value's ratio to the maximum.
    fn bar_color(&self, val: u64, max_val: u64) -> Color {
        if max_val == 0 {
            return self.color_low;
        }
        let ratio = val as f64 / max_val as f64;
        if ratio > 0.66 {
            self.color_high
        } else if ratio > 0.33 {
            self.color_mid
        } else {
            self.color_low
        }
    }
}

impl Widget for SparklineHistory<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.data.is_empty() {
            return;
        }

        let width = area.width as usize;

        // Take the last `width` samples (or fewer if not enough data).
        let start = if self.data.len() > width {
            self.data.len() - width
        } else {
            0
        };

        let max_val = self
            .data
            .iter()
            .skip(start)
            .copied()
            .max()
            .unwrap_or(1)
            .max(1);

        for (i, &val) in self.data.iter().skip(start).enumerate() {
            let x = area.left() + i as u16;
            if x >= area.right() {
                break;
            }

            // Map value to a sparkline character (0..7 index).
            let level = if val == 0 {
                0
            } else {
                ((val as f64 / max_val as f64) * 7.0).round() as usize
            };
            let ch = SPARK_CHARS[level.min(7)];
            let color = self.bar_color(val, max_val);

            buf[(x, area.top())]
                .set_char(ch)
                .set_style(Style::default().fg(color));
        }
    }
}

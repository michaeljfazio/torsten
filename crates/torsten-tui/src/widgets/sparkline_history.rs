//! Block rate sparkline widget using Unicode block characters.
//!
//! Renders a horizontal sparkline showing historical block processing rate,
//! using the Unicode block elements (U+2581..U+2588) for 8-level resolution.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use std::collections::VecDeque;

/// Unicode block elements from lowest to highest (8 levels).
const SPARK_CHARS: [char; 8] = [
    '\u{2581}', // ▁
    '\u{2582}', // ▂
    '\u{2583}', // ▃
    '\u{2584}', // ▄
    '\u{2585}', // ▅
    '\u{2586}', // ▆
    '\u{2587}', // ▇
    '\u{2588}', // █
];

/// A sparkline widget that displays a series of values as a compact bar chart
/// using Unicode block characters.
pub struct SparklineHistory<'a> {
    /// Reference to the data points.
    data: &'a VecDeque<u64>,
    /// Color for the sparkline bars.
    color: Color,
}

impl<'a> SparklineHistory<'a> {
    /// Create a new sparkline from a data source.
    pub fn new(data: &'a VecDeque<u64>, color: Color) -> Self {
        Self { data, color }
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

            buf[(x, area.top())]
                .set_char(ch)
                .set_style(Style::default().fg(self.color));
        }
    }
}

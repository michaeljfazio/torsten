//! Block rate sparkline widget using Unicode block characters.
//!
//! Renders a horizontal sparkline showing historical block processing rate,
//! using the Unicode block elements (U+2581..U+2588) for 8-level resolution.
//! Bars are colored with a 3-tier gradient based on their value relative to
//! the visible maximum: low (< 33%), mid (33–66%), high (> 66%).

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use std::collections::VecDeque;

/// Unicode block elements from lowest to highest (8 levels).
///
/// Exported so that inline sparkline rendering in the UI layer can share the
/// same character set without duplicating it.
pub const SPARK_CHARS: [char; 8] = [
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
    /// Color for bars in the mid range (33–66% of max).
    color_mid: Color,
    /// Color for bars in the high range (> 66% of max).
    color_high: Color,
}

impl<'a> SparklineHistory<'a> {
    /// Create a new sparkline from a data source with default traffic-light coloring.
    ///
    /// The three gradient tiers default to Green / Yellow / Red.
    /// Use [`spark_low`][Self::spark_low], [`spark_mid`][Self::spark_mid],
    /// and [`spark_high`][Self::spark_high] to customise individual tier colors,
    /// or use [`with_color`][Self::with_color] for a uniform single-color variant.
    #[allow(dead_code)]
    pub fn new(data: &'a VecDeque<u64>) -> Self {
        Self {
            data,
            color_low: Color::Green,
            color_mid: Color::Yellow,
            color_high: Color::Red,
        }
    }

    /// Create a sparkline where all bars use the same color.
    ///
    /// Useful when the height of bars alone conveys the meaning (e.g. block rate
    /// displayed with the active accent color) rather than a traffic-light gradient.
    #[cfg(test)]
    pub fn with_color(data: &'a VecDeque<u64>, color: Color) -> Self {
        Self {
            data,
            color_low: color,
            color_mid: color,
            color_high: color,
        }
    }

    /// Set the color for low-value bars (< 33% of max).
    #[allow(dead_code)]
    pub fn spark_low(mut self, color: Color) -> Self {
        self.color_low = color;
        self
    }

    /// Set the color for mid-value bars (33–66% of max).
    #[allow(dead_code)]
    pub fn spark_mid(mut self, color: Color) -> Self {
        self.color_mid = color;
        self
    }

    /// Set the color for high-value bars (> 66% of max).
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_data(vals: &[u64]) -> VecDeque<u64> {
        vals.iter().copied().collect()
    }

    #[test]
    fn test_sparkline_renders_without_panic() {
        let data = make_data(&[10, 20, 30, 40, 50]);
        let w = SparklineHistory::new(&data);
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        w.render(area, &mut buf);
    }

    #[test]
    fn test_sparkline_empty_data_no_panic() {
        let data = VecDeque::new();
        let w = SparklineHistory::new(&data);
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        w.render(area, &mut buf);
    }

    #[test]
    fn test_sparkline_with_color_renders() {
        let data = make_data(&[5, 10, 15, 20]);
        let w = SparklineHistory::with_color(&data, Color::Cyan);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        w.render(area, &mut buf);
        // All rendered cells should be Cyan.
        for x in 0..4u16 {
            let cell = &buf[(x, 0)];
            assert_eq!(cell.fg, Color::Cyan, "cell {x} should be Cyan");
        }
    }

    #[test]
    fn test_sparkline_gradient_colors() {
        let data = make_data(&[1, 50, 100]);
        let w = SparklineHistory::new(&data)
            .spark_low(Color::Blue)
            .spark_mid(Color::Magenta)
            .spark_high(Color::White);
        // The bar_color method should return different colors for different ratios.
        // low: val=1, max=100 → ratio 0.01 → color_low
        assert_eq!(w.bar_color(1, 100), Color::Blue);
        // mid: val=50, max=100 → ratio 0.5 → color_mid
        assert_eq!(w.bar_color(50, 100), Color::Magenta);
        // high: val=80, max=100 → ratio 0.8 → color_high
        assert_eq!(w.bar_color(80, 100), Color::White);
    }

    #[test]
    fn test_sparkline_truncates_to_width() {
        // More data points than display width — should only render the last `width` points.
        let data = make_data(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let w = SparklineHistory::new(&data);
        let area = Rect::new(0, 0, 5, 1); // only 5 columns
        let mut buf = Buffer::empty(area);
        w.render(area, &mut buf);
        // All 5 cells should have been written.
        for x in 0..5u16 {
            let sym = buf[(x, 0)].symbol().chars().next().unwrap_or(' ');
            assert!(sym != ' ', "cell {x} should contain a spark character");
        }
    }
}

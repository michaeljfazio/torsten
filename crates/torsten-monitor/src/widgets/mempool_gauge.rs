//! Horizontal mempool gauge widget for the Torsten TUI dashboard.
//!
//! Renders a color-coded horizontal bar showing mempool tx count relative
//! to a configurable maximum capacity.  The bar is green when the mempool is
//! under 25% full, yellow between 25–75%, and red above 75%.  A centered label
//! shows "current/max txs".
//!
//! The fill, empty, and label colors are drawn from a [`crate::theme::Theme`]
//! so that the widget participates in theme cycling.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

use crate::theme::Theme;

/// Default mempool capacity used for gauge scaling.
const DEFAULT_MEMPOOL_MAX: u64 = 16_384;

/// A horizontal gauge widget showing mempool fill level.
pub struct MempoolGauge<'a> {
    /// Current number of transactions in the mempool.
    pub current: u64,
    /// Maximum capacity for gauge scaling.
    pub max: u64,
    /// Active theme for colors.
    theme: &'a Theme,
}

impl<'a> MempoolGauge<'a> {
    /// Create a new mempool gauge with the default max capacity.
    pub fn new(current: u64, theme: &'a Theme) -> Self {
        Self {
            current,
            max: DEFAULT_MEMPOOL_MAX,
            theme,
        }
    }

    /// Set a custom max capacity for the gauge.
    pub fn with_max(mut self, max: u64) -> Self {
        self.max = max.max(1);
        self
    }

    /// Determine the bar fill color based on fill ratio using theme colors.
    fn bar_color(&self) -> Color {
        let ratio = self.fill_ratio();
        if ratio > 0.75 {
            self.theme.error
        } else if ratio > 0.25 {
            self.theme.warning
        } else {
            self.theme.success
        }
    }

    /// Compute fill ratio clamped to [0.0, 1.0].
    pub fn fill_ratio(&self) -> f64 {
        if self.max == 0 {
            return 0.0;
        }
        (self.current as f64 / self.max as f64).clamp(0.0, 1.0)
    }
}

impl Widget for MempoolGauge<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 8 || area.height < 1 {
            return;
        }

        let color = self.bar_color();
        let ratio = self.fill_ratio();
        let filled_width = ((area.width as f64) * ratio) as u16;

        // Render filled portion.
        for x in area.left()..area.left().saturating_add(filled_width) {
            if x < area.right() {
                buf[(x, area.top())]
                    .set_char('\u{2588}') // full block
                    .set_style(Style::default().fg(color));
            }
        }

        // Render empty portion.
        for x in area.left().saturating_add(filled_width)..area.right() {
            buf[(x, area.top())]
                .set_char('\u{2591}') // light shade
                .set_style(Style::default().fg(self.theme.gauge_empty));
        }

        // Centered label: "current/max txs"
        let label = format!("{}/{} txs", self.current, self.max);
        let label_start = area.left() + area.width.saturating_sub(label.len() as u16) / 2;
        for (i, ch) in label.chars().enumerate() {
            let x = label_start + i as u16;
            if x < area.right() {
                let in_filled = x < area.left().saturating_add(filled_width);
                let (fg, bg) = if in_filled {
                    (Color::Black, color)
                } else {
                    (self.theme.fg, Color::Reset)
                };
                buf[(x, area.top())]
                    .set_char(ch)
                    .set_style(Style::default().fg(fg).bg(bg));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::THEME_DEFAULT;

    #[test]
    fn test_mempool_gauge_empty() {
        let gauge = MempoolGauge::new(0, &THEME_DEFAULT);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        gauge.render(area, &mut buf);
        // Label should be present.
        let line: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line.contains("0/16384 txs"));
    }

    #[test]
    fn test_mempool_gauge_half_full() {
        let gauge = MempoolGauge::new(8192, &THEME_DEFAULT);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        gauge.render(area, &mut buf);
        let line: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line.contains("8192/16384 txs"));
    }

    #[test]
    fn test_mempool_gauge_full() {
        let gauge = MempoolGauge::new(16384, &THEME_DEFAULT);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        gauge.render(area, &mut buf);
        let line: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line.contains("16384/16384 txs"));
    }

    #[test]
    fn test_mempool_gauge_over_capacity() {
        let gauge = MempoolGauge::new(20000, &THEME_DEFAULT);
        assert_eq!(gauge.fill_ratio(), 1.0);
    }

    #[test]
    fn test_mempool_gauge_custom_max() {
        let gauge = MempoolGauge::new(100, &THEME_DEFAULT).with_max(200);
        assert_eq!(gauge.max, 200);
        assert!((gauge.fill_ratio() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_mempool_gauge_color_thresholds() {
        // ~12% → success (green)
        let low = MempoolGauge::new(2000, &THEME_DEFAULT);
        assert_eq!(low.bar_color(), THEME_DEFAULT.success);

        // 50% → warning (yellow)
        let mid = MempoolGauge::new(8192, &THEME_DEFAULT);
        assert_eq!(mid.bar_color(), THEME_DEFAULT.warning);

        // ~88% → error (red)
        let high = MempoolGauge::new(14336, &THEME_DEFAULT);
        assert_eq!(high.bar_color(), THEME_DEFAULT.error);
    }

    #[test]
    fn test_mempool_gauge_narrow_area() {
        let gauge = MempoolGauge::new(100, &THEME_DEFAULT);
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        // Should bail early without panic.
        gauge.render(area, &mut buf);
    }
}

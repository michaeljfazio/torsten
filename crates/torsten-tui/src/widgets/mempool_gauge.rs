//! Horizontal mempool gauge widget for the Torsten TUI dashboard.
//!
//! Displays a color-coded horizontal bar showing mempool tx count relative
//! to a configurable maximum capacity. The bar is colored green when the
//! mempool is under 25% full, yellow between 25-75%, and red above 75%.
//! A centered label shows "current/max txs".

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

/// Default mempool capacity used for gauge scaling.
const DEFAULT_MEMPOOL_MAX: u64 = 4000;

/// Color thresholds.
const GREEN: Color = Color::Rgb(80, 220, 100);
const YELLOW: Color = Color::Rgb(255, 215, 0);
const RED: Color = Color::Rgb(255, 80, 80);

/// A horizontal gauge widget showing mempool fill level.
pub struct MempoolGauge {
    /// Current number of transactions in the mempool.
    pub current: u64,
    /// Maximum capacity for gauge scaling.
    pub max: u64,
}

impl MempoolGauge {
    /// Create a new mempool gauge with the default max capacity.
    pub fn new(current: u64) -> Self {
        Self {
            current,
            max: DEFAULT_MEMPOOL_MAX,
        }
    }

    /// Set a custom max capacity for the gauge.
    #[allow(dead_code)]
    pub fn with_max(mut self, max: u64) -> Self {
        self.max = max.max(1);
        self
    }

    /// Determine the bar color based on fill ratio.
    fn bar_color(&self) -> Color {
        let ratio = self.fill_ratio();
        if ratio > 0.75 {
            RED
        } else if ratio > 0.25 {
            YELLOW
        } else {
            GREEN
        }
    }

    /// Compute fill ratio clamped to [0.0, 1.0].
    fn fill_ratio(&self) -> f64 {
        if self.max == 0 {
            return 0.0;
        }
        (self.current as f64 / self.max as f64).clamp(0.0, 1.0)
    }
}

impl Widget for MempoolGauge {
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
                .set_style(Style::default().fg(Color::DarkGray));
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
                    (Color::White, Color::Reset)
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

    #[test]
    fn test_mempool_gauge_empty() {
        let gauge = MempoolGauge::new(0);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        gauge.render(area, &mut buf);
        // Label should be present.
        let line: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line.contains("0/4000 txs"));
    }

    #[test]
    fn test_mempool_gauge_half_full() {
        let gauge = MempoolGauge::new(2000);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        gauge.render(area, &mut buf);
        let line: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line.contains("2000/4000 txs"));
    }

    #[test]
    fn test_mempool_gauge_full() {
        let gauge = MempoolGauge::new(4000);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        gauge.render(area, &mut buf);
        let line: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line.contains("4000/4000 txs"));
    }

    #[test]
    fn test_mempool_gauge_over_capacity() {
        let gauge = MempoolGauge::new(5000);
        assert_eq!(gauge.fill_ratio(), 1.0);
    }

    #[test]
    fn test_mempool_gauge_custom_max() {
        let gauge = MempoolGauge::new(100).with_max(200);
        assert_eq!(gauge.max, 200);
        assert!((gauge.fill_ratio() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_mempool_gauge_color_thresholds() {
        let low = MempoolGauge::new(500); // 12.5%
        assert_eq!(low.bar_color(), GREEN);

        let mid = MempoolGauge::new(2000); // 50%
        assert_eq!(mid.bar_color(), YELLOW);

        let high = MempoolGauge::new(3500); // 87.5%
        assert_eq!(high.bar_color(), RED);
    }

    #[test]
    fn test_mempool_gauge_narrow_area() {
        let gauge = MempoolGauge::new(100);
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        // Should bail early without panic.
        gauge.render(area, &mut buf);
    }
}

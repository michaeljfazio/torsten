//! Epoch progress bar widget for the Chain panel.
//!
//! Renders a single-line progress bar showing:
//! - Filled portion proportional to `slot_in_epoch / epoch_length`
//! - A centred label: `"Epoch NNN  XX.X%  ~Nd Nh Nm remaining"`
//!
//! The fill colour is cornflower-blue by default and can be overridden.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

/// Default fill colour for the epoch progress bar.
const DEFAULT_FILL: Color = Color::Rgb(100, 149, 237); // cornflower blue
const DIM: Color = Color::Rgb(160, 160, 170);

/// A single-line epoch progress bar widget.
pub struct EpochProgress {
    /// Current slot position within the epoch.
    pub slot_in_epoch: u64,
    /// Total epoch length in slots (must be > 0).
    pub epoch_length: u64,
    /// Seconds remaining until the next epoch boundary.
    pub time_remaining_secs: u64,
    /// Fill colour for the progress bar.
    pub fill_color: Color,
    /// Epoch number (shown in the centred label when > 0).
    pub epoch: u64,
}

impl EpochProgress {
    /// Create a new epoch progress bar.
    pub fn new(slot_in_epoch: u64, epoch_length: u64, time_remaining_secs: u64) -> Self {
        Self {
            slot_in_epoch,
            epoch_length: epoch_length.max(1),
            time_remaining_secs,
            fill_color: DEFAULT_FILL,
            epoch: 0,
        }
    }

    /// Set the epoch number shown inside the bar label.
    pub fn with_epoch(mut self, epoch: u64) -> Self {
        self.epoch = epoch;
        self
    }

    /// Override the fill colour.
    #[allow(dead_code)]
    pub fn with_fill_color(mut self, color: Color) -> Self {
        self.fill_color = color;
        self
    }

    /// Progress ratio in [0.0, 1.0].
    fn ratio(&self) -> f64 {
        (self.slot_in_epoch as f64 / self.epoch_length as f64).clamp(0.0, 1.0)
    }

    /// Format the time remaining as a compact duration string.
    fn format_remaining(&self) -> String {
        let secs = self.time_remaining_secs;
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        let mins = (secs % 3600) / 60;
        if days > 0 {
            format!("~{}d {}h {}m", days, hours, mins)
        } else if hours > 0 {
            format!("~{}h {}m", hours, mins)
        } else {
            format!("~{}m", mins)
        }
    }
}

impl Widget for EpochProgress {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 1 {
            return;
        }

        let ratio = self.ratio();
        let filled = ((area.width as f64) * ratio) as u16;
        let remaining = self.format_remaining();
        let pct = ratio * 100.0;

        // Build centred label.  Try the full form first, fall back to short form.
        let label_full = if self.epoch > 0 {
            format!(" Epoch {}  {:.1}%  {} ", self.epoch, pct, remaining)
        } else {
            format!(" {:.1}%  {} ", pct, remaining)
        };
        let label_short = format!(" {:.1}% ", pct);

        let label = if label_full.len() as u16 <= area.width {
            label_full
        } else if label_short.len() as u16 <= area.width {
            label_short
        } else {
            String::new()
        };

        let label_start = area.left() + area.width.saturating_sub(label.len() as u16) / 2;

        for col in 0..area.width {
            let x = area.left() + col;
            if x >= area.right() {
                break;
            }

            let in_filled = col < filled;

            // Check if this column falls inside the label text.
            let label_offset = col.saturating_sub(label_start.saturating_sub(area.left())) as usize;
            let in_label = !label.is_empty() && x >= label_start && label_offset < label.len();

            if in_label {
                let ch = label.as_bytes()[label_offset] as char;
                let (fg, bg) = if in_filled {
                    (Color::Black, self.fill_color)
                } else {
                    (DIM, Color::Reset)
                };
                buf[(x, area.top())]
                    .set_char(ch)
                    .set_style(Style::default().fg(fg).bg(bg));
            } else if in_filled {
                buf[(x, area.top())]
                    .set_char('\u{2588}') // full block
                    .set_style(Style::default().fg(self.fill_color));
            } else {
                buf[(x, area.top())]
                    .set_char('\u{2591}') // light shade
                    .set_style(Style::default().fg(Color::DarkGray));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_renders_without_panic() {
        let w = EpochProgress::new(200_000, 432_000, 232_000).with_epoch(1234);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        w.render(area, &mut buf);
    }

    #[test]
    fn test_ratio_bounds() {
        assert!((EpochProgress::new(0, 432_000, 0).ratio() - 0.0).abs() < 1e-9);
        assert!((EpochProgress::new(432_000, 432_000, 0).ratio() - 1.0).abs() < 1e-9);
        assert!((EpochProgress::new(216_000, 432_000, 0).ratio() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_format_remaining() {
        let w = |secs| EpochProgress::new(0, 432_000, secs);
        assert_eq!(w(90061).format_remaining(), "~1d 1h 1m");
        assert_eq!(w(3661).format_remaining(), "~1h 1m");
        assert_eq!(w(300).format_remaining(), "~5m");
    }

    #[test]
    fn test_narrow_area_no_panic() {
        let w = EpochProgress::new(200_000, 432_000, 232_000);
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        w.render(area, &mut buf);
    }

    #[test]
    fn test_epoch_label_in_bar() {
        let w = EpochProgress::new(216_000, 432_000, 216_000).with_epoch(500);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        w.render(area, &mut buf);
        let rendered: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            rendered.contains("500"),
            "epoch number should appear in bar"
        );
    }
}

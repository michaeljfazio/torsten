//! Narrow epoch countdown progress bar widget.
//!
//! Displays the slot position within the current epoch as a thin horizontal
//! bar with a label showing "Slot X / Y | ~Nd Nh Mm remaining". Uses the
//! theme accent color for the filled portion.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

/// Accent color for the epoch progress fill.
const ACCENT_BLUE: Color = Color::Rgb(100, 149, 237);
const DIM_WHITE: Color = Color::Rgb(160, 160, 170);

/// A narrow progress bar showing epoch slot position and time remaining.
pub struct EpochProgress {
    /// Current slot within the epoch.
    pub slot_in_epoch: u64,
    /// Total epoch length in slots.
    pub epoch_length: u64,
    /// Seconds remaining until the next epoch boundary.
    pub time_remaining_secs: u64,
    /// Fill color for the progress bar.
    pub fill_color: Color,
}

impl EpochProgress {
    /// Create a new epoch progress bar.
    pub fn new(slot_in_epoch: u64, epoch_length: u64, time_remaining_secs: u64) -> Self {
        Self {
            slot_in_epoch,
            epoch_length: epoch_length.max(1),
            time_remaining_secs,
            fill_color: ACCENT_BLUE,
        }
    }

    /// Set a custom fill color.
    pub fn _fill_color(mut self, color: Color) -> Self {
        self.fill_color = color;
        self
    }

    /// Compute the progress ratio [0.0, 1.0].
    fn ratio(&self) -> f64 {
        (self.slot_in_epoch as f64 / self.epoch_length as f64).clamp(0.0, 1.0)
    }

    /// Format the time remaining as a human-readable duration.
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

        // Build the label.
        let label = format!(
            " Slot {} / {} | {} ",
            format_compact(self.slot_in_epoch),
            format_compact(self.epoch_length),
            remaining
        );

        // If the label is too wide, use a shorter version.
        let label = if label.len() as u16 > area.width {
            format!(" {:.0}% | {} ", ratio * 100.0, remaining)
        } else {
            label
        };

        let label_start = area.left() + area.width.saturating_sub(label.len() as u16) / 2;

        for col in 0..area.width {
            let x = area.left() + col;
            if x >= area.right() {
                break;
            }

            let in_filled = col < filled;

            // Check if this position is within the label.
            let label_idx = col.saturating_sub(label_start.saturating_sub(area.left())) as usize;
            let in_label =
                x >= label_start && label_idx < label.len() && x < label_start + label.len() as u16;

            if in_label {
                let ch = label.as_bytes()[label_idx] as char;
                let (fg, bg) = if in_filled {
                    (Color::Black, self.fill_color)
                } else {
                    (DIM_WHITE, Color::Reset)
                };
                buf[(x, area.top())]
                    .set_char(ch)
                    .set_style(Style::default().fg(fg).bg(bg));
            } else if in_filled {
                buf[(x, area.top())]
                    .set_char('\u{2588}')
                    .set_style(Style::default().fg(self.fill_color));
            } else {
                buf[(x, area.top())]
                    .set_char('\u{2591}')
                    .set_style(Style::default().fg(Color::DarkGray));
            }
        }
    }
}

/// Format a number with comma separators in a compact way.
fn format_compact(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epoch_progress_renders_without_panic() {
        let widget = EpochProgress::new(200_000, 432_000, 232_000);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
    }

    #[test]
    fn test_epoch_progress_at_zero() {
        let widget = EpochProgress::new(0, 432_000, 432_000);
        assert!((widget.ratio() - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_epoch_progress_at_full() {
        let widget = EpochProgress::new(432_000, 432_000, 0);
        assert!((widget.ratio() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_epoch_progress_halfway() {
        let widget = EpochProgress::new(216_000, 432_000, 216_000);
        assert!((widget.ratio() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_format_remaining_days() {
        let widget = EpochProgress::new(0, 432_000, 90061);
        assert_eq!(widget.format_remaining(), "~1d 1h 1m");
    }

    #[test]
    fn test_format_remaining_hours() {
        let widget = EpochProgress::new(0, 432_000, 3661);
        assert_eq!(widget.format_remaining(), "~1h 1m");
    }

    #[test]
    fn test_format_remaining_minutes() {
        let widget = EpochProgress::new(0, 432_000, 300);
        assert_eq!(widget.format_remaining(), "~5m");
    }

    #[test]
    fn test_format_compact() {
        assert_eq!(format_compact(432_000), "432,000");
        assert_eq!(format_compact(0), "0");
        assert_eq!(format_compact(1_000_000), "1,000,000");
    }

    #[test]
    fn test_narrow_area() {
        let widget = EpochProgress::new(200_000, 432_000, 232_000);
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        // Should bail early without panic.
        widget.render(area, &mut buf);
    }
}

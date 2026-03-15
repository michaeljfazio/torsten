//! Compact 2-line header bar widget for the Torsten TUI dashboard.
//!
//! Displays critical node status at a glance:
//! - Line 1: node name, sync status, epoch, tip age, uptime
//! - Line 2: epoch progress bar showing slot position within the current epoch

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};

/// Color constants matching the main UI palette.
const ACCENT_GREEN: Color = Color::Rgb(80, 220, 100);
const ACCENT_YELLOW: Color = Color::Rgb(255, 215, 0);
const ACCENT_RED: Color = Color::Rgb(255, 80, 80);
const DIM_WHITE: Color = Color::Rgb(160, 160, 170);
const BRIGHT_WHITE: Color = Color::Rgb(230, 230, 240);
const BORDER_DIM: Color = Color::Rgb(70, 70, 85);

/// A compact 2-line header bar showing critical node status.
pub struct HeaderBar {
    /// Sync progress percentage (0.0 - 100.0).
    pub sync_pct: f64,
    /// Whether the node is fully synced.
    pub is_synced: bool,
    /// Whether the node is stalled.
    pub is_stalled: bool,
    /// Current epoch number.
    pub epoch: u64,
    /// Tip age in seconds.
    pub tip_age: u64,
    /// Uptime as a formatted string (e.g. "1h 5m").
    pub uptime: String,
    /// Epoch progress as a fraction (0.0 - 1.0).
    pub epoch_progress: f64,
    /// Whether the node is connected.
    pub connected: bool,
}

impl Widget for HeaderBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 1 {
            return;
        }

        // --- Line 1: status summary ---
        let y = area.top();
        let status_color = if !self.connected {
            ACCENT_RED
        } else if self.is_synced {
            ACCENT_GREEN
        } else if self.is_stalled {
            ACCENT_RED
        } else {
            ACCENT_YELLOW
        };

        let status_text = if !self.connected {
            "Disconnected".to_string()
        } else if self.is_synced {
            format!("Synced {:.2}%", self.sync_pct)
        } else if self.is_stalled {
            format!("Stalled {:.2}%", self.sync_pct)
        } else {
            format!("Syncing {:.2}%", self.sync_pct)
        };

        // Build the status line: "torsten-tui | Synced 99.99% | Epoch 1237 | Tip: 12s | Up: 1h 5m"
        let line = format!(
            " torsten-tui \u{2502} {} \u{2502} Epoch {} \u{2502} Tip: {}s \u{2502} Up: {}",
            status_text, self.epoch, self.tip_age, self.uptime
        );

        // Render character by character with appropriate styling
        let mut x = area.left();

        // "torsten-tui" portion
        let prefix = " torsten-tui ";
        for ch in prefix.chars() {
            if x >= area.right() {
                break;
            }
            buf[(x, y)].set_char(ch).set_style(
                Style::default()
                    .fg(BRIGHT_WHITE)
                    .add_modifier(Modifier::BOLD),
            );
            x += 1;
        }

        // Separator
        if x < area.right() {
            buf[(x, y)]
                .set_char('\u{2502}')
                .set_style(Style::default().fg(BORDER_DIM));
            x += 1;
        }

        // Status text
        let status_part = format!(" {} ", status_text);
        for ch in status_part.chars() {
            if x >= area.right() {
                break;
            }
            buf[(x, y)].set_char(ch).set_style(
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            );
            x += 1;
        }

        // Remaining segments
        let segments = [
            format!("\u{2502} Epoch {} ", self.epoch),
            format!("\u{2502} Tip: {}s ", self.tip_age),
            format!("\u{2502} Up: {} ", self.uptime),
        ];

        for segment in &segments {
            for (i, ch) in segment.chars().enumerate() {
                if x >= area.right() {
                    break;
                }
                let style = if i == 0 {
                    // Separator character
                    Style::default().fg(BORDER_DIM)
                } else {
                    Style::default().fg(DIM_WHITE)
                };
                buf[(x, y)].set_char(ch).set_style(style);
                x += 1;
            }
        }

        // Fill remaining width on line 1 with spaces
        while x < area.right() {
            buf[(x, y)].set_char(' ').set_style(Style::default());
            x += 1;
        }

        // --- Line 2: epoch progress bar ---
        if area.height < 2 {
            return;
        }

        let y2 = area.top() + 1;
        let bar_width = area.width;
        let progress = self.epoch_progress.clamp(0.0, 1.0);
        let filled = ((bar_width as f64) * progress) as u16;

        // Epoch progress label
        let label = format!(" Epoch {:.1}% ", progress * 100.0);
        let label_start = (bar_width.saturating_sub(label.len() as u16)) / 2;

        // Avoid computing on very small line lengths; just write what we have.
        // Ensure we don't overflow on the second line allocation.
        let _line_used = line;

        for col in 0..bar_width {
            let abs_x = area.left() + col;
            if abs_x >= area.right() {
                break;
            }

            let in_filled = col < filled;
            let bar_color = if self.is_synced {
                ACCENT_GREEN
            } else if self.is_stalled {
                ACCENT_RED
            } else {
                ACCENT_YELLOW
            };

            // Check if this position falls within the label
            let label_idx = col.saturating_sub(label_start) as usize;
            let in_label = col >= label_start && label_idx < label.len();

            if in_label {
                let ch = label.as_bytes()[label_idx] as char;
                let (fg, bg) = if in_filled {
                    (Color::Black, bar_color)
                } else {
                    (BRIGHT_WHITE, Color::Reset)
                };
                buf[(abs_x, y2)]
                    .set_char(ch)
                    .set_style(Style::default().fg(fg).bg(bg));
            } else if in_filled {
                buf[(abs_x, y2)]
                    .set_char('\u{2588}')
                    .set_style(Style::default().fg(bar_color));
            } else {
                buf[(abs_x, y2)]
                    .set_char('\u{2591}')
                    .set_style(Style::default().fg(Color::DarkGray));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_header(synced: bool, stalled: bool, connected: bool) -> HeaderBar {
        HeaderBar {
            sync_pct: if synced { 99.99 } else { 50.0 },
            is_synced: synced,
            is_stalled: stalled,
            epoch: 1237,
            tip_age: 12,
            uptime: "1h 5m".to_string(),
            epoch_progress: 0.65,
            connected,
        }
    }

    #[test]
    fn test_header_renders_without_panic() {
        let header = make_header(true, false, true);
        let area = Rect::new(0, 0, 120, 2);
        let mut buf = Buffer::empty(area);
        header.render(area, &mut buf);
        // Verify the first line contains "torsten-tui"
        let line1: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line1.contains("torsten-tui"));
    }

    #[test]
    fn test_header_syncing_status() {
        let header = make_header(false, false, true);
        let area = Rect::new(0, 0, 120, 2);
        let mut buf = Buffer::empty(area);
        header.render(area, &mut buf);
        let line1: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line1.contains("Syncing"));
    }

    #[test]
    fn test_header_stalled_status() {
        let header = make_header(false, true, true);
        let area = Rect::new(0, 0, 120, 2);
        let mut buf = Buffer::empty(area);
        header.render(area, &mut buf);
        let line1: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line1.contains("Stalled"));
    }

    #[test]
    fn test_header_disconnected() {
        let header = make_header(false, false, false);
        let area = Rect::new(0, 0, 120, 2);
        let mut buf = Buffer::empty(area);
        header.render(area, &mut buf);
        let line1: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line1.contains("Disconnected"));
    }

    #[test]
    fn test_header_narrow_terminal() {
        let header = make_header(true, false, true);
        let area = Rect::new(0, 0, 30, 2);
        let mut buf = Buffer::empty(area);
        // Should not panic even on narrow terminals.
        header.render(area, &mut buf);
    }

    #[test]
    fn test_header_single_line() {
        let header = make_header(true, false, true);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        // Should render line 1 only, skip progress bar.
        header.render(area, &mut buf);
    }

    #[test]
    fn test_header_too_small() {
        let header = make_header(true, false, true);
        let area = Rect::new(0, 0, 5, 2);
        let mut buf = Buffer::empty(area);
        // Should bail early without panic.
        header.render(area, &mut buf);
    }

    #[test]
    fn test_epoch_progress_bar_fill() {
        let mut header = make_header(true, false, true);
        header.epoch_progress = 0.0;
        let area = Rect::new(0, 0, 100, 2);
        let mut buf = Buffer::empty(area);
        header.render(area, &mut buf);
        // At 0% progress, first cell on line 2 should not be the filled block.
        let first_char = buf[(0, 1)].symbol().chars().next().unwrap_or(' ');
        assert_ne!(first_char, '\u{2588}');

        let mut header2 = make_header(true, false, true);
        header2.epoch_progress = 1.0;
        let mut buf2 = Buffer::empty(area);
        header2.render(area, &mut buf2);
        // At 100% progress, first cell on line 2 should be filled (or part of label).
        // Just verify no panic.
    }
}

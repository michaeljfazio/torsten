//! Theme system for the Torsten TUI dashboard.
//!
//! Provides a [`Theme`] struct with all color fields used across the UI,
//! seven built-in theme definitions, and a helper to cycle between them.

use ratatui::style::Color;

/// Complete color palette for the TUI dashboard.
///
/// Every hardcoded `Color` in the rendering code is replaced by a field
/// from this struct, making it trivial to swap the entire look by
/// switching to a different `Theme` instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Human-readable theme name (shown in the footer).
    pub name: &'static str,
    /// Primary background color.
    pub bg: Color,
    /// Primary foreground / body text color.
    pub fg: Color,
    /// Muted / dimmed text color (labels, secondary info).
    pub muted: Color,
    /// Accent color used for keybindings, active highlights.
    pub accent: Color,
    /// Color indicating success / healthy state.
    pub success: Color,
    /// Color indicating a warning condition.
    pub warning: Color,
    /// Color indicating an error or critical condition.
    pub error: Color,
    /// Color for informational highlights (latency, rates).
    pub info: Color,
    /// Default panel border color.
    pub border: Color,
    /// Border color for the currently active panel.
    pub border_active: Color,
    /// Panel title text color (inactive panels).
    pub title: Color,
    /// Sparkline bar color for low values (< 33%).
    pub spark_low: Color,
    /// Sparkline bar color for mid values (33-66%).
    pub spark_mid: Color,
    /// Sparkline bar color for high values (> 66%).
    pub spark_high: Color,
    /// Progress gauge filled portion color.
    pub gauge_fill: Color,
    /// Progress gauge empty portion color.
    pub gauge_empty: Color,
}

/// Default theme — clean dark with cornflower blue accent and muted grays.
pub const THEME_DEFAULT: Theme = Theme {
    name: "Default",
    bg: Color::Rgb(24, 24, 32),
    fg: Color::Rgb(230, 230, 240),
    muted: Color::Rgb(160, 160, 170),
    accent: Color::Rgb(100, 149, 237), // Cornflower blue
    success: Color::Rgb(80, 220, 100),
    warning: Color::Rgb(255, 215, 0),
    error: Color::Rgb(255, 80, 80),
    info: Color::Rgb(0, 210, 210),
    border: Color::Rgb(70, 70, 85),
    border_active: Color::Rgb(100, 149, 237),
    title: Color::Rgb(180, 180, 200),
    spark_low: Color::Rgb(80, 220, 100),
    spark_mid: Color::Rgb(255, 215, 0),
    spark_high: Color::Rgb(255, 80, 80),
    gauge_fill: Color::Rgb(80, 220, 100),
    gauge_empty: Color::DarkGray,
};

/// Monokai theme — warm, high-contrast palette from the classic editor theme.
pub const THEME_MONOKAI: Theme = Theme {
    name: "Monokai",
    bg: Color::Rgb(39, 40, 34),        // #272822
    fg: Color::Rgb(248, 248, 242),     // #F8F8F2
    muted: Color::Rgb(117, 113, 94),   // #75715E
    accent: Color::Rgb(102, 217, 239), // #66D9EF cyan
    success: Color::Rgb(166, 226, 46), // #A6E22E green
    warning: Color::Rgb(253, 151, 31), // #FD971F orange
    error: Color::Rgb(249, 38, 114),   // #F92672 pink
    info: Color::Rgb(102, 217, 239),   // #66D9EF cyan
    border: Color::Rgb(62, 61, 50),
    border_active: Color::Rgb(102, 217, 239),
    title: Color::Rgb(248, 248, 242),
    spark_low: Color::Rgb(166, 226, 46),
    spark_mid: Color::Rgb(253, 151, 31),
    spark_high: Color::Rgb(249, 38, 114),
    gauge_fill: Color::Rgb(166, 226, 46),
    gauge_empty: Color::Rgb(62, 61, 50),
};

/// Solarized Dark theme — Ethan Schoonover's balanced dark palette.
pub const THEME_SOLARIZED_DARK: Theme = Theme {
    name: "Solarized Dark",
    bg: Color::Rgb(0, 43, 54),        // #002B36 base03
    fg: Color::Rgb(131, 148, 150),    // #839496 base0
    muted: Color::Rgb(88, 110, 117),  // #586E75 base01
    accent: Color::Rgb(38, 139, 210), // #268BD2 blue
    success: Color::Rgb(133, 153, 0), // #859900 green
    warning: Color::Rgb(181, 137, 0), // #B58900 yellow
    error: Color::Rgb(220, 50, 47),   // #DC322F red
    info: Color::Rgb(42, 161, 152),   // #2AA198 cyan
    border: Color::Rgb(7, 54, 66),    // #073642 base02
    border_active: Color::Rgb(38, 139, 210),
    title: Color::Rgb(147, 161, 161), // #93A1A1 base1
    spark_low: Color::Rgb(133, 153, 0),
    spark_mid: Color::Rgb(181, 137, 0),
    spark_high: Color::Rgb(220, 50, 47),
    gauge_fill: Color::Rgb(133, 153, 0),
    gauge_empty: Color::Rgb(7, 54, 66),
};

/// Solarized Light theme — the light variant of Solarized.
pub const THEME_SOLARIZED_LIGHT: Theme = Theme {
    name: "Solarized Light",
    bg: Color::Rgb(253, 246, 227),     // #FDF6E3 base3
    fg: Color::Rgb(101, 123, 131),     // #657B83 base00
    muted: Color::Rgb(147, 161, 161),  // #93A1A1 base1
    accent: Color::Rgb(38, 139, 210),  // #268BD2 blue
    success: Color::Rgb(133, 153, 0),  // #859900 green
    warning: Color::Rgb(181, 137, 0),  // #B58900 yellow
    error: Color::Rgb(220, 50, 47),    // #DC322F red
    info: Color::Rgb(42, 161, 152),    // #2AA198 cyan
    border: Color::Rgb(238, 232, 213), // #EEE8D5 base2
    border_active: Color::Rgb(38, 139, 210),
    title: Color::Rgb(88, 110, 117), // #586E75 base01
    spark_low: Color::Rgb(133, 153, 0),
    spark_mid: Color::Rgb(181, 137, 0),
    spark_high: Color::Rgb(220, 50, 47),
    gauge_fill: Color::Rgb(133, 153, 0),
    gauge_empty: Color::Rgb(238, 232, 213),
};

/// Nord theme — Arctic, north-bluish color palette.
pub const THEME_NORD: Theme = Theme {
    name: "Nord",
    bg: Color::Rgb(46, 52, 64),         // #2E3440 nord0
    fg: Color::Rgb(216, 222, 233),      // #D8DEE9 nord4
    muted: Color::Rgb(76, 86, 106),     // #4C566A nord3
    accent: Color::Rgb(136, 192, 208),  // #88C0D0 frost
    success: Color::Rgb(163, 190, 140), // #A3BE8C green
    warning: Color::Rgb(235, 203, 139), // #EBCB8B yellow
    error: Color::Rgb(191, 97, 106),    // #BF616A red
    info: Color::Rgb(129, 161, 193),    // #81A1C1 frost2
    border: Color::Rgb(59, 66, 82),     // #3B4252 nord1
    border_active: Color::Rgb(136, 192, 208),
    title: Color::Rgb(229, 233, 240), // #E5E9F0 nord5
    spark_low: Color::Rgb(163, 190, 140),
    spark_mid: Color::Rgb(235, 203, 139),
    spark_high: Color::Rgb(191, 97, 106),
    gauge_fill: Color::Rgb(163, 190, 140),
    gauge_empty: Color::Rgb(59, 66, 82),
};

/// Dracula theme — dark purple-tinted palette with vivid accents.
pub const THEME_DRACULA: Theme = Theme {
    name: "Dracula",
    bg: Color::Rgb(40, 42, 54),         // #282A36
    fg: Color::Rgb(248, 248, 242),      // #F8F8F2
    muted: Color::Rgb(98, 114, 164),    // #6272A4 comment
    accent: Color::Rgb(189, 147, 249),  // #BD93F9 purple
    success: Color::Rgb(80, 250, 123),  // #50FA7B green
    warning: Color::Rgb(241, 250, 140), // #F1FA8C yellow
    error: Color::Rgb(255, 85, 85),     // #FF5555 red
    info: Color::Rgb(139, 233, 253),    // #8BE9FD cyan
    border: Color::Rgb(68, 71, 90),     // #44475A current line
    border_active: Color::Rgb(189, 147, 249),
    title: Color::Rgb(248, 248, 242),
    spark_low: Color::Rgb(80, 250, 123),
    spark_mid: Color::Rgb(241, 250, 140),
    spark_high: Color::Rgb(255, 85, 85),
    gauge_fill: Color::Rgb(80, 250, 123),
    gauge_empty: Color::Rgb(68, 71, 90),
};

/// Catppuccin Mocha theme — soothing pastel palette on a warm dark base.
pub const THEME_CATPPUCCIN_MOCHA: Theme = Theme {
    name: "Catppuccin Mocha",
    bg: Color::Rgb(30, 30, 46),         // #1E1E2E base
    fg: Color::Rgb(205, 214, 244),      // #CDD6F4 text
    muted: Color::Rgb(108, 112, 134),   // #6C7086 overlay0
    accent: Color::Rgb(137, 180, 250),  // #89B4FA blue
    success: Color::Rgb(166, 227, 161), // #A6E3A1 green
    warning: Color::Rgb(249, 226, 175), // #F9E2AF yellow
    error: Color::Rgb(243, 139, 168),   // #F38BA8 red
    info: Color::Rgb(148, 226, 213),    // #94E2D5 teal
    border: Color::Rgb(49, 50, 68),     // #313244 surface0
    border_active: Color::Rgb(137, 180, 250),
    title: Color::Rgb(186, 194, 222), // #BAC2DE subtext1
    spark_low: Color::Rgb(166, 227, 161),
    spark_mid: Color::Rgb(249, 226, 175),
    spark_high: Color::Rgb(243, 139, 168),
    gauge_fill: Color::Rgb(166, 227, 161),
    gauge_empty: Color::Rgb(49, 50, 68),
};

/// All built-in themes, indexed for cycling.
pub const THEMES: [Theme; 7] = [
    THEME_MONOKAI,
    THEME_DEFAULT,
    THEME_SOLARIZED_DARK,
    THEME_SOLARIZED_LIGHT,
    THEME_NORD,
    THEME_DRACULA,
    THEME_CATPPUCCIN_MOCHA,
];

/// Return the next theme index, wrapping around to 0 after the last theme.
pub fn cycle_theme(current: usize) -> usize {
    (current + 1) % THEMES.len()
}

/// Look up a theme by name (case-insensitive). Returns the index into [`THEMES`],
/// or `None` if no theme matches.
pub fn find_theme_by_name(name: &str) -> Option<usize> {
    let lower = name.to_lowercase();
    THEMES.iter().position(|t| t.name.to_lowercase() == lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cycle_theme_wraps_around() {
        assert_eq!(cycle_theme(0), 1);
        assert_eq!(cycle_theme(5), 6);
        assert_eq!(cycle_theme(6), 0); // wrap
    }

    #[test]
    fn test_all_themes_have_distinct_names() {
        let mut names: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), original_len, "duplicate theme names found");
    }

    #[test]
    fn test_themes_have_non_default_key_colors() {
        // Verify key color fields are not Color::Reset (the ratatui default).
        for theme in &THEMES {
            assert_ne!(theme.bg, Color::Reset, "{} bg is Reset", theme.name);
            assert_ne!(theme.fg, Color::Reset, "{} fg is Reset", theme.name);
            assert_ne!(theme.accent, Color::Reset, "{} accent is Reset", theme.name);
            assert_ne!(
                theme.success,
                Color::Reset,
                "{} success is Reset",
                theme.name
            );
            assert_ne!(
                theme.warning,
                Color::Reset,
                "{} warning is Reset",
                theme.name
            );
            assert_ne!(theme.error, Color::Reset, "{} error is Reset", theme.name);
            assert_ne!(theme.border, Color::Reset, "{} border is Reset", theme.name);
        }
    }

    #[test]
    fn test_theme_count() {
        assert_eq!(THEMES.len(), 7);
    }

    #[test]
    fn test_find_theme_by_name() {
        // THEMES order: [Monokai=0, Default=1, Solarized Dark=2,
        //                Solarized Light=3, Nord=4, Dracula=5, Catppuccin Mocha=6]
        assert_eq!(find_theme_by_name("Monokai"), Some(0));
        assert_eq!(find_theme_by_name("default"), Some(1));
        assert_eq!(find_theme_by_name("NORD"), Some(4));
        assert_eq!(find_theme_by_name("nonexistent"), None);
    }

    #[test]
    fn test_cycle_all_themes_returns_to_start() {
        let mut idx = 0;
        for _ in 0..THEMES.len() {
            idx = cycle_theme(idx);
        }
        assert_eq!(idx, 0, "cycling through all themes should return to 0");
    }
}

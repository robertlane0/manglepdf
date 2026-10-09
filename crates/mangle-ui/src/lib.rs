//! ManglePDF.
//!
//! The window shell and the design tokens it paints from. Everything visible is drawn
//! here rather than taken from a toolkit widget, so the interface is one program with
//! the look we intend (ADR-0004).
//!
//! The rule from the charter that shapes this crate hardest: the UI thread never
//! parses, decodes or rasterizes. It draws rectangles and text, and it waits for a
//! worker to hand it a finished page.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#![warn(missing_debug_implementations)]

pub mod canvas;
pub mod shell;
pub mod theme;
pub mod worker;

pub use canvas::PagePlacement;
pub use shell::{App, LeftPanel, Region, RightPanel, State};
pub use theme::{Metrics, Spacing, Theme};
pub use worker::{Job, JobResult, Supervisor};

/// Parse the command line the way the acceptance harness expects.
///
/// The flags are the ones `FINISH.md` names: a window size, a scale factor, a theme and
/// a preference reset.
#[must_use]
pub fn options_from(args: &[String]) -> Options {
    let mut o = Options::default();
    let mut i = 0;
    while i < args.len() {
        match args.get(i).map(String::as_str) {
            Some("--window-size") => {
                i += 1;
                if let Some((w, h)) = args.get(i).and_then(|v| parse_size(v)) {
                    o.window_size = Some((w, h));
                }
            }
            Some("--scale") => {
                i += 1;
                if let Some(s) = args.get(i).and_then(|v| v.parse::<f32>().ok()) {
                    o.scale = s.clamp(0.5, 4.0);
                }
            }
            Some("--theme") => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("dark") => o.theme = ThemeChoice::Dark,
                    Some("light") => o.theme = ThemeChoice::Light,
                    _ => {}
                }
            }
            Some("--reset-prefs") => o.reset_prefs = true,
            Some("--open") => {
                i += 1;
                if let Some(path) = args.get(i) {
                    o.open = Some(path.clone());
                }
            }
            _ => {}
        }
        i += 1;
    }
    o
}

/// `1536x1024`, the reference window.
fn parse_size(v: &str) -> Option<(f32, f32)> {
    let (w, h) = v.split_once(['x', 'X'])?;
    let w = w.trim().parse().ok()?;
    let h = h.trim().parse().ok()?;
    (w > 0.0 && h > 0.0).then_some((w, h))
}

/// Which theme to start with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    Light,
    Dark,
}

impl ThemeChoice {
    #[must_use]
    pub fn build(self) -> Theme {
        match self {
            Self::Light => Theme::light(),
            Self::Dark => Theme::dark(),
        }
    }
}

/// Everything the command line can say.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub window_size: Option<(f32, f32)>,
    pub scale: f32,
    pub theme: ThemeChoice,
    pub reset_prefs: bool,
    pub open: Option<String>,
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// Floats parsed from text are compared with a tolerance; an exact comparison
    /// would be testing the decimal representation rather than the value.
    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn the_command_line_is_parsed() {
        let o = options_from(&args(&[
            "--window-size",
            "1536x1024",
            "--scale",
            "2",
            "--theme",
            "dark",
            "--reset-prefs",
            "--open",
            "a.pdf",
        ]));
        assert_eq!(o.window_size, Some((1536.0, 1024.0)));
        assert!(near(o.scale, 2.0), "scale was {}", o.scale);
        assert_eq!(o.theme, ThemeChoice::Dark);
        assert!(o.reset_prefs);
        assert_eq!(o.open.as_deref(), Some("a.pdf"));
    }

    #[test]
    fn a_nonsense_size_is_ignored_rather_than_accepted() {
        assert_eq!(
            options_from(&args(&["--window-size", "wide"])).window_size,
            None
        );
        assert_eq!(
            options_from(&args(&["--window-size", "0x0"])).window_size,
            None
        );
        assert!(near(options_from(&args(&["--scale", "9"])).scale, 4.0));
        assert!(near(options_from(&args(&["--scale", "tiny"])).scale, 0.0));
    }

    #[test]
    fn an_unknown_flag_does_not_stop_the_others_being_read() {
        let o = options_from(&args(&["--nonsense", "--theme", "light", "--reset-prefs"]));
        assert!(o.reset_prefs);
        assert_eq!(o.theme, ThemeChoice::Light);
    }

    #[test]
    fn both_themes_are_legible() {
        for theme in [Theme::light(), Theme::dark()] {
            // Text has to be distinguishable from the surfaces it sits on, or the
            // interface is not usable whatever else is true about it.
            let contrast = |a: egui::Color32, b: egui::Color32| {
                let lum = |c: egui::Color32| {
                    let [r, g, b, _] = c.to_array();
                    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
                };
                (lum(a) - lum(b)).abs()
            };
            assert!(
                contrast(theme.text, theme.panel) > 0.5,
                "body text must stand out from a panel"
            );
            assert!(
                contrast(theme.text_muted, theme.panel) > 0.2,
                "secondary text must still be readable"
            );
            assert!(
                contrast(theme.notice_text, theme.notice_bg) > 0.3,
                "a notice must be readable"
            );
        }
    }
}

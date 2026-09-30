//! Design tokens.
//!
//! Every colour, size and gap the interface uses comes from here. The window paints no
//! literals of its own, which is what makes the look a data change rather than a code
//! change, and what makes the dark theme a second file rather than a second
//! application (ADR-0004).
//!
//! The values are sampled from the reference design: a light, dense, macOS-adjacent
//! document tool. They are recorded here rather than read from the image at run time,
//! because a screenshot is a reference, not a dependency.

use eframe::egui::{Color32, FontId};

/// Sizes that a layout depends on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub top_bar_height: f32,
    pub toolbar_height: f32,
    pub left_panel_width: f32,
    pub right_panel_width: f32,
    pub thumbnails_width: f32,
    pub status_height: f32,
    pub notice_height: f32,
    pub tab_height: f32,
    pub row_height: f32,
    pub header_height: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            top_bar_height: 44.0,
            toolbar_height: 26.0,
            left_panel_width: 232.0,
            right_panel_width: 280.0,
            thumbnails_width: 132.0,
            status_height: 24.0,
            notice_height: 28.0,
            tab_height: 30.0,
            row_height: 26.0,
            header_height: 18.0,
        }
    }
}

/// Gaps and radii.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spacing {
    /// The window's outer inset.
    pub window_inset: f32,
    /// The gap between controls.
    pub item_gap: f32,
    /// The gap between pages on the canvas.
    pub page_gap: f32,
    /// The corner radius of a page.
    pub page_radius: f32,
    /// The corner radius of a field.
    pub field_radius: f32,

    // Sizes, in points, for the type scale.
    pub ui_text: f32,
    pub heading_text: f32,
    pub tab_text: f32,
    pub row_text: f32,
    pub toolbar_text: f32,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            window_inset: 10.0,
            item_gap: 6.0,
            page_gap: 16.0,
            page_radius: 2.0,
            field_radius: 4.0,

            ui_text: 13.0,
            heading_text: 12.0,
            tab_text: 11.0,
            row_text: 12.0,
            toolbar_text: 12.0,
        }
    }
}

/// A type scale: a request in points, a size in points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Ui,
    Heading,
    Tab,
    Row,
    Toolbar,
    Caption,
}

impl Size {
    fn value(self, s: &Spacing) -> f32 {
        match self {
            Self::Ui => s.ui_text,
            Self::Heading => s.heading_text,
            Self::Tab => s.tab_text,
            Self::Row => s.row_text,
            Self::Toolbar => s.toolbar_text,
            Self::Caption => s.row_text - 1.0,
        }
    }
}

/// Text styles, in points, per role.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeScale {
    pub spacing: Spacing,
}

impl TypeScale {
    /// The point size for a role. A request is honoured unless it is absurd, so a
    /// caller asking for a 400-point label gets a large label rather than a window
    /// that no longer fits.
    pub fn font(&self, requested: f32) -> f32 {
        requested.clamp(6.0, 96.0)
    }
}

/// The registered style name for a role.
const fn name_of(size: Size) -> &'static str {
    match size {
        Size::Ui => "ui",
        Size::Heading => "heading",
        Size::Tab => "tab",
        Size::Row => "row",
        Size::Toolbar => "toolbar",
        Size::Caption => "caption",
    }
}

impl TypeScale {
    /// The font for a role at a weight the role implies.
    pub fn font_id(self, role: Size) -> FontId {
        FontId::proportional(self.font(role.value(&self.spacing)))
    }
}

impl Spacing {}

/// A light theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// The window's background, behind everything.
    pub window: Color32,
    /// The chrome: title bar and toolbar.
    pub chrome: Color32,
    /// A panel's background.
    pub panel: Color32,
    /// The canvas behind the pages.
    pub canvas: Color32,
    /// Paper.
    pub page: Color32,
    /// A selected page's paper.
    pub page_selected: Color32,
    pub page_border: Color32,
    pub shadow: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub hover: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub notice_bg: Color32,
    pub notice_text: Color32,

    pub metrics: Metrics,
    pub spacing: Spacing,
    pub type_scale: TypeScale,
}

impl Theme {
    /// The light theme, matching the reference design.
    #[must_use]
    pub fn light() -> Self {
        let spacing = Spacing::default();
        Self {
            window: Color32::from_rgb(0xF2, 0xF2, 0xF2),
            chrome: Color32::from_rgb(0xE6, 0xE6, 0xE6),
            panel: Color32::from_rgb(0xFA, 0xFA, 0xFA),
            canvas: Color32::from_rgb(0xB9, 0xB9, 0xB9),
            page: Color32::WHITE,
            page_selected: Color32::from_rgb(0xE8, 0xF0, 0xFB),
            page_border: Color32::from_rgb(0xD0, 0xD0, 0xD0),
            shadow: Color32::from_black_alpha(28),
            border: Color32::from_rgb(0xD6, 0xD6, 0xD6),
            divider: Color32::from_rgb(0xDE, 0xDE, 0xDE),
            hover: Color32::from_rgb(0xEC, 0xEC, 0xEC),
            accent: Color32::from_rgb(0x1A, 0x6D, 0xD6),
            text: Color32::from_rgb(0x1F, 0x1F, 0x1F),
            text_muted: Color32::from_rgb(0x77, 0x77, 0x77),
            notice_bg: Color32::from_rgb(0xFF, 0xF3, 0xCD),
            notice_text: Color32::from_rgb(0x4A, 0x3B, 0x00),

            metrics: Metrics::default(),
            type_scale: TypeScale { spacing },
            spacing,
        }
    }

    /// The dark theme, which is a data change rather than a second application.
    #[must_use]
    pub fn dark() -> Self {
        let spacing = Spacing::default();
        Self {
            window: Color32::from_rgb(0x20, 0x20, 0x20),
            chrome: Color32::from_rgb(0x2A, 0x2A, 0x2A),
            panel: Color32::from_rgb(0x26, 0x26, 0x26),
            canvas: Color32::from_rgb(0x14, 0x14, 0x14),
            page: Color32::from_rgb(0xE8, 0xE8, 0xE8),
            page_selected: Color32::from_rgb(0x2F, 0x3E, 0x54),
            page_border: Color32::from_rgb(0x3A, 0x3A, 0x3A),
            shadow: Color32::from_black_alpha(120),
            border: Color32::from_rgb(0x40, 0x40, 0x40),
            divider: Color32::from_rgb(0x38, 0x38, 0x38),
            hover: Color32::from_rgb(0x32, 0x32, 0x32),
            accent: Color32::from_rgb(0x5A, 0x9C, 0xF0),
            text: Color32::from_rgb(0xE6, 0xE6, 0xE6),
            text_muted: Color32::from_rgb(0x9A, 0x9A, 0x9A),
            notice_bg: Color32::from_rgb(0x4A, 0x40, 0x18),
            notice_text: Color32::from_rgb(0xF2, 0xE2, 0xA8),

            metrics: Metrics::default(),
            type_scale: TypeScale { spacing },
            spacing,
        }
    }
}

/// The style name a role is registered under, for a caller that has a `TextStyle`.
#[must_use]
pub fn style_name(role: Size) -> &'static str {
    name_of(role)
}

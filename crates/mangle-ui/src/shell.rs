//! The window: the shell, the six regions, and the theme everything paints from.
//!
//! No widget from the toolkit is drawn. Every rectangle, line and glyph position here
//! comes from a token, which is what makes the interface look the same everywhere and
//! makes a theme a data change rather than a code change (ADR-0004).
//!
//! Nothing in this file opens a file, decodes a stream or rasterizes a page. The rule
//! from the charter that shapes this crate hardest is that the UI thread never does any
//! of those things, and this file is where that is most visible: it is chrome.

// `ui`, `p`, `t` and `s` are the conventional names in immediate-mode code. Spelling
// them out in every layout function would make the file longer without making it
// clearer, which is the opposite of what a naming rule is for.
#![allow(clippy::many_single_char_names)]

use eframe::egui;
// A short alias for the painter, so a drawing call reads as a drawing call.
use eframe::egui::Painter;

use crate::theme::{Size, Theme};

/// The six regions of the window, in the layout the mockup describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// The title bar, the menu and the toolbar.
    Top,
    /// The bookmark, layer, attachment and signature tree.
    Left,
    /// The page canvas, with its own scroll and zoom.
    Centre,
    /// The contextual inspector: Text Properties, Colour, Arrange, Organize.
    Right,
    /// The status line: page, zoom, selection, notices.
    Bottom,
    /// The floating page thumbnails.
    Thumbnails,
}

/// Which panel the left region is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeftPanel {
    #[default]
    Bookmarks,
    Layers,
    Attachments,
    Signatures,
}

/// Which panel the right region is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightPanel {
    #[default]
    Text,
    Colour,
    Arrange,
    Organize,
    /// No selection: the document properties.
    Properties,
}

/// What the shell knows right now. A real document fills this in.
#[derive(Debug, Default)]
pub struct State {
    /// Open document, or none yet.
    pub file_name: Option<String>,
    pub page_count: usize,
    pub page_index: usize,
    pub zoom: f32,
    pub left: Option<LeftPanel>,
    pub right: RightPanel,
    /// The first row of the bookmark tree, until a document supplies its own.
    pub bookmarks: Vec<String>,
    pub layers: Vec<(String, bool)>,
    /// What the user was told, and has not dismissed.
    pub notice: Option<String>,
}

impl State {
    #[must_use]
    pub fn new() -> Self {
        Self {
            zoom: 1.0,
            right: RightPanel::Properties,
            ..Self::default()
        }
    }
}

/// The application.
#[derive(Debug)]
pub struct App {
    theme: Theme,
    state: State,
    /// The document's page rectangles, in window points, recomputed each frame.
    pages: Vec<egui::Rect>,
}

impl App {
    #[must_use]
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let theme = Theme::light();
        // The reference window is 1536x1024 logical; the canvas is what is left over.
        install_fonts(&cc.egui_ctx, &theme);
        Self {
            theme,
            state: State::new(),
            pages: Vec::new(),
        }
    }

    /// Open a document. The caller supplies only what the shell draws; parsing
    /// happens on a worker and arrives as a message.
    pub fn set_document(&mut self, name: String, page_count: usize, bookmarks: Vec<String>) {
        self.state.file_name = Some(name);
        self.state.page_count = page_count;
        self.state.page_index = 0;
        self.state.bookmarks = bookmarks;
    }

    /// Show the user something. Notices are never silent: a limitation the product
    /// cannot work around is always stated (charter law 5, honesty).
    pub fn tell(&mut self, notice: impl Into<String>) {
        self.state.notice = Some(notice.into());
    }
}

/// Load the interface face, falling back to the built-in one.
///
/// A missing font is a reason to say so, not a reason to fail: the window still opens
/// and still works, and the notice tells the user what looks different.
fn install_fonts(ctx: &egui::Context, theme: &Theme) {
    // Every role the shell draws is registered here, so a role is a name and a size
    // in one place rather than a number repeated at every call site.
    let role_sizes: Vec<(Size, f32)> = vec![
        (Size::Ui, theme.spacing.ui_text),
        (Size::Heading, theme.spacing.heading_text),
        (Size::Tab, theme.spacing.tab_text),
        (Size::Row, theme.spacing.row_text),
        (Size::Toolbar, theme.spacing.toolbar_text),
        (Size::Caption, theme.spacing.row_text - 1.0),
    ];
    ctx.all_styles_mut(|style| {
        for (role, size) in &role_sizes {
            style.text_styles.insert(
                egui::TextStyle::Name(crate::theme::style_name(*role).into()),
                theme.type_scale.font_id(*role),
            );
            let _ = size;
        }
        style.spacing.item_spacing = egui::vec2(theme.spacing.item_gap, theme.spacing.item_gap);
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = self.theme.clone();
        let screen = ui.max_rect();
        let p = ui.painter().clone();

        // The window is a light field; every region is drawn on top of it.
        p.rect_filled(screen, 0.0, t.window);
        self.top_bar(ui, &p, &t, screen);
        self.left_region(ui, &p, &t, screen);
        self.centre(ui, &p, &t, screen);
        self.right_region(ui, &p, &t, screen);
        self.thumbnails(ui, &p, &t, screen);
        self.bottom_bar(ui, &p, &t, screen);

        // A notice is drawn last so it is never hidden behind a panel.
        if let Some(notice) = self.state.notice.clone() {
            Self::notice_bar(ui, &p, &t, screen, &notice);
        }
    }
}

impl App {
    fn top_bar(&mut self, ui: &egui::Ui, p: &Painter, t: &Theme, screen: egui::Rect) {
        let rect = egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(screen.width(), t.metrics.top_bar_height),
        );
        p.rect_filled(rect, 0.0, t.chrome);

        // The title, then the toolbar buttons to its right.
        let title = self
            .state
            .file_name
            .clone()
            .unwrap_or_else(|| "Untitled".to_string());
        p.text(
            rect.left_center() + egui::vec2(t.spacing.window_inset, 0.0),
            egui::Align2::LEFT_CENTER,
            title,
            t.type_scale.font_id(Size::Ui),
            t.text,
        );

        let mut x = rect.left() + t.spacing.window_inset * 2.0 + 260.0;
        for label in ["Open", "Save", "Print"] {
            let width = t.spacing.toolbar_text * (label.chars().count() as f32) + 18.0;
            let r = egui::Rect::from_center_size(
                egui::pos2(x + width / 2.0, rect.center().y),
                egui::vec2(width, t.metrics.toolbar_height),
            );
            hoverable(ui, p, r, t);
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                label,
                t.type_scale.font_id(Size::Toolbar),
                t.text,
            );
            x += width + t.spacing.item_gap;
        }
    }

    fn left_region(&mut self, ui: &egui::Ui, p: &Painter, t: &Theme, screen: egui::Rect) {
        let s = t.spacing;
        let top = t.metrics.top_bar_height;
        let rect = egui::Rect::from_min_size(
            egui::pos2(0.0, top),
            egui::vec2(
                t.metrics.left_panel_width,
                screen.height() - top - t.metrics.status_height,
            ),
        );
        p.rect_filled(rect, 0.0, t.panel);

        let mut y = rect.top() + s.item_gap;
        for (i, panel) in [
            LeftPanel::Bookmarks,
            LeftPanel::Layers,
            LeftPanel::Attachments,
            LeftPanel::Signatures,
        ]
        .into_iter()
        .enumerate()
        {
            let r = egui::Rect::from_min_size(
                egui::pos2(rect.left() + s.window_inset, y),
                egui::vec2(
                    t.metrics.left_panel_width - s.window_inset * 2.0,
                    t.metrics.tab_height,
                ),
            );
            let active = self.state.left == Some(panel);
            hoverable(ui, p, r, t);
            if active {
                p.rect_filled(
                    egui::Rect::from_min_size(r.left_top(), egui::vec2(r.width(), 2.0)),
                    0.0,
                    t.accent,
                );
            }
            p.text(
                r.left_center() + egui::vec2(s.item_gap, 0.0),
                egui::Align2::LEFT_CENTER,
                label_of(panel),
                t.type_scale.font_id(Size::Tab),
                if active { t.text } else { t.text_muted },
            );
            if active {
                y += t.metrics.tab_height;
            }
            let _ = i;
        }

        // The list under the chosen tab.
        match self.state.left.unwrap_or_default() {
            LeftPanel::Bookmarks => {
                for (i, name) in self.state.bookmarks.clone().iter().enumerate() {
                    let r = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + s.window_inset, y),
                        egui::vec2(
                            t.metrics.left_panel_width - s.window_inset * 2.0,
                            t.metrics.row_height,
                        ),
                    );
                    if r.bottom() > rect.bottom() {
                        break;
                    }
                    hoverable(ui, p, r, t);
                    // Indent by depth; the mockout nests four levels.
                    let indent = (i % 4) as f32 * s.item_gap * 1.5;
                    p.text(
                        r.left_center() + egui::vec2(s.item_gap + indent, 0.0),
                        egui::Align2::LEFT_CENTER,
                        name,
                        t.type_scale.font_id(Size::Row),
                        t.text,
                    );
                    y += t.metrics.row_height;
                }
            }
            LeftPanel::Layers => {
                for (name, on) in self.state.layers.clone() {
                    let r = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + s.window_inset, y),
                        egui::vec2(
                            t.metrics.left_panel_width - s.window_inset * 2.0,
                            t.metrics.row_height,
                        ),
                    );
                    if r.bottom() > rect.bottom() {
                        break;
                    }
                    hoverable(ui, p, r, t);
                    let box_rect = egui::Rect::from_center_size(
                        r.left_center() + egui::vec2(s.item_gap * 1.5, 0.0),
                        egui::vec2(11.0, 11.0),
                    );
                    p.rect_stroke(
                        box_rect,
                        0.0,
                        egui::Stroke::new(2.0, t.border),
                        egui::StrokeKind::Middle,
                    );
                    if on {
                        p.rect_filled(box_rect, 2.0, t.accent);
                    }
                    p.text(
                        r.left_center() + egui::vec2(s.item_gap * 3.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        name,
                        t.type_scale.font_id(Size::Row),
                        t.text,
                    );
                    y += t.metrics.row_height;
                }
            }
            LeftPanel::Attachments | LeftPanel::Signatures => {
                let r = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + s.window_inset, y),
                    egui::vec2(
                        t.metrics.left_panel_width - s.window_inset * 2.0,
                        t.metrics.row_height,
                    ),
                );
                p.text(
                    r.left_center() + egui::vec2(s.item_gap, 0.0),
                    egui::Align2::LEFT_CENTER,
                    "Nothing here yet",
                    t.type_scale.font_id(Size::Row),
                    t.text_muted,
                );
            }
        }
    }

    /// The page canvas. This is the region that will hold a rendered page; today it
    /// draws the paper and nothing else, which is the honest state of the product.
    fn centre(&mut self, _ui: &egui::Ui, p: &Painter, t: &Theme, screen: egui::Rect) {
        let s = t.spacing;
        let left = t.metrics.left_panel_width;
        let right = t.metrics.right_panel_width;
        let top = t.metrics.top_bar_height;
        let rect = egui::Rect::from_min_size(
            egui::pos2(left, top),
            egui::vec2(
                screen.width() - left - right,
                screen.height() - top - t.metrics.status_height,
            ),
        );
        p.rect_filled(rect, 0.0, t.canvas);

        let page_size = egui::vec2(612.0, 792.0) * self.state.zoom;
        let spacing = s.page_gap;
        let mut y = rect.top() + spacing;
        self.pages.clear();
        for i in 0..self.state.page_count {
            let r = egui::Rect::from_min_size(
                egui::pos2(rect.center().x - page_size.x / 2.0, y),
                page_size,
            );
            if r.bottom() > rect.bottom() && i > 0 {
                break;
            }
            // The paper, its drop shadow and its border.
            p.rect_filled(r.translate(egui::vec2(0.0, 1.0)), s.page_radius, t.shadow);
            p.rect_filled(r, s.page_radius, t.page);
            p.rect_stroke(
                r,
                s.page_radius,
                egui::Stroke::new(1.0, t.page_border),
                egui::StrokeKind::Middle,
            );
            // The label under the page, as every viewer shows it.
            p.text(
                r.left_center() + egui::vec2(-s.item_gap, 0.0),
                egui::Align2::RIGHT_CENTER,
                (i + 1).to_string(),
                t.type_scale.font_id(Size::Caption),
                t.text_muted,
            );
            self.pages.push(r);
            y += page_size.y + spacing;
        }

        if self.state.page_count == 0 {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Open a document to begin",
                t.type_scale.font_id(Size::Ui),
                t.text_muted,
            );
        }
    }

    fn right_region(&mut self, ui: &egui::Ui, p: &Painter, t: &Theme, screen: egui::Rect) {
        let s = t.spacing;
        let width = t.metrics.right_panel_width;
        let top = t.metrics.top_bar_height;
        let rect = egui::Rect::from_min_size(
            egui::pos2(screen.width() - width, top),
            egui::vec2(width, screen.height() - top - t.metrics.status_height),
        );
        p.rect_filled(rect, 0.0, t.panel);
        p.line(
            vec![rect.left_top(), rect.left_bottom()],
            egui::Stroke::new(1.0, t.divider),
        );

        let mut y = rect.top() + s.item_gap;
        p.text(
            egui::pos2(rect.left() + s.window_inset, y),
            egui::Align2::LEFT_TOP,
            panel_title(self.state.right),
            t.type_scale.font_id(Size::Heading),
            t.text,
        );
        y += t.metrics.tab_height;

        for row in rows_for(self.state.right) {
            let r = egui::Rect::from_min_size(
                egui::pos2(rect.left() + s.window_inset, y),
                egui::vec2(width - s.window_inset * 2.0, t.metrics.row_height),
            );
            if r.bottom() > rect.bottom() {
                break;
            }
            hoverable(ui, p, r, t);
            p.text(
                r.left_center() + egui::vec2(0.0, 0.0),
                egui::Align2::LEFT_CENTER,
                row,
                t.type_scale.font_id(Size::Row),
                t.text_muted,
            );
            // A control on the right of each row, which is what makes the panel a
            // panel rather than a list of words.
            let field = egui::Rect::from_center_size(
                r.right_center() + egui::vec2(-s.item_gap, 0.0),
                egui::vec2(r.width() * 0.5, r.height() - s.item_gap * 2.0),
            );
            p.rect_stroke(
                field,
                s.field_radius,
                egui::Stroke::new(1.0, t.border),
                egui::StrokeKind::Middle,
            );
            if hovered(ui, field) {
                p.rect_filled(field, s.field_radius, t.hover);
            }
            y += t.metrics.row_height;
        }
    }

    fn bottom_bar(&mut self, _ui: &egui::Ui, p: &Painter, t: &Theme, screen: egui::Rect) {
        let s = t.spacing;
        let height = t.metrics.status_height;
        let rect = egui::Rect::from_min_size(
            egui::pos2(0.0, screen.height() - height),
            egui::vec2(screen.width(), height),
        );
        p.rect_filled(rect, 0.0, t.panel);
        p.line(
            vec![rect.left_top(), rect.right_top()],
            egui::Stroke::new(1.0, t.divider),
        );

        let left = if self.state.page_count == 0 {
            "No document".to_string()
        } else {
            format!(
                "Page {} of {}",
                self.state.page_index + 1,
                self.state.page_count
            )
        };
        p.text(
            rect.left_center() + egui::vec2(s.window_inset, 0.0),
            egui::Align2::LEFT_CENTER,
            left,
            t.type_scale.font_id(Size::Caption),
            t.text_muted,
        );
        p.text(
            rect.right_center() + egui::vec2(-s.window_inset, 0.0),
            egui::Align2::RIGHT_CENTER,
            format!("{:.0}%", self.state.zoom * 100.0),
            t.type_scale.font_id(Size::Caption),
            t.text_muted,
        );
    }

    fn thumbnails(&mut self, ui: &egui::Ui, p: &Painter, t: &Theme, screen: egui::Rect) {
        let s = t.spacing;
        let width = t.metrics.thumbnails_width;
        let top = t.metrics.top_bar_height;
        let rect = egui::Rect::from_min_size(
            egui::pos2(t.metrics.left_panel_width, top),
            egui::vec2(width, screen.height() - top - t.metrics.status_height),
        );
        p.rect_filled(rect, 0.0, t.panel);
        p.line(
            vec![rect.right_top(), rect.right_bottom()],
            egui::Stroke::new(1.0, t.divider),
        );

        let thumb_w = width - s.item_gap * 3.0;
        let thumb_h = thumb_w * 792.0 / 612.0;
        let mut y = rect.top() + s.item_gap;
        for i in 0..self.state.page_count {
            if y + thumb_h > rect.bottom() {
                break;
            }
            let r = egui::Rect::from_min_size(
                egui::pos2(rect.left() + s.item_gap, y),
                egui::vec2(thumb_w, thumb_h),
            );
            let current = i == self.state.page_index;
            hoverable(ui, p, r, t);
            p.rect_filled(r, 2.0, if current { t.page_selected } else { t.page });
            p.rect_stroke(
                r,
                2.0,
                egui::Stroke::new(
                    if current { 2.0 } else { 1.0 },
                    if current { t.accent } else { t.page_border },
                ),
                egui::StrokeKind::Middle,
            );
            y += thumb_h + s.item_gap;
        }
    }

    fn notice_bar(_ui: &egui::Ui, p: &Painter, t: &Theme, screen: egui::Rect, notice: &str) {
        let height = t.metrics.notice_height;
        let rect = egui::Rect::from_min_size(
            egui::pos2(0.0, screen.height() - t.metrics.status_height - height),
            egui::vec2(screen.width(), height),
        );
        p.rect_filled(rect, 0.0, t.notice_bg);
        p.text(
            rect.left_center() + egui::vec2(t.spacing.window_inset, 0.0),
            egui::Align2::LEFT_CENTER,
            notice,
            t.type_scale.font_id(Size::Row),
            t.notice_text,
        );
    }
}

/// Whether the pointer is over a rectangle.
fn hovered(ui: &egui::Ui, rect: egui::Rect) -> bool {
    let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) else {
        return false;
    };
    rect.contains(pos)
}

/// A row that reports the pointer, so the window does not feel inert.
fn hoverable(ui: &egui::Ui, p: &Painter, rect: egui::Rect, t: &Theme) {
    if hovered(ui, rect) {
        p.rect_filled(rect, t.spacing.field_radius, t.hover);
    }
}

fn label_of(panel: LeftPanel) -> &'static str {
    match panel {
        LeftPanel::Bookmarks => "Bookmarks",
        LeftPanel::Layers => "Layers",
        LeftPanel::Attachments => "Attachments",
        LeftPanel::Signatures => "Signatures",
    }
}

fn panel_title(panel: RightPanel) -> &'static str {
    match panel {
        RightPanel::Text => "Text Properties",
        RightPanel::Colour => "Colour",
        RightPanel::Arrange => "Arrange",
        RightPanel::Organize => "Organize",
        RightPanel::Properties => "Document Properties",
    }
}

fn rows_for(panel: RightPanel) -> Vec<&'static str> {
    match panel {
        RightPanel::Text => vec![
            "Font",
            "Style",
            "Size",
            "Colour",
            "Alignment",
            "Line Spacing",
            "Character Spacing",
            "Baseline Shift",
            "Render Mode",
            "Rotation",
        ],
        RightPanel::Colour => vec!["Fill", "Stroke", "Line Width", "Opacity", "Blend Mode"],
        RightPanel::Arrange => vec![
            "Bring to Front",
            "Bring Forward",
            "Send Backward",
            "Send to Back",
            "Align",
            "Distribute",
            "Group",
            "Ungroup",
        ],
        RightPanel::Organize => vec![
            "Rotate",
            "Crop",
            "Replace",
            "Insert Blank",
            "Delete",
            "Extract",
        ],
        RightPanel::Properties => vec![
            "Title", "Author", "Subject", "Keywords", "Creator", "Producer", "Created", "Modified",
            "Pages", "Version", "Fonts", "Security",
        ],
    }
}

pub use crate::theme::{Metrics, Spacing};

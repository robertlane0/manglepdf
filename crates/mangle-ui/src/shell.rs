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

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;
// A short alias for the painter, so a drawing call reads as a drawing call.
use eframe::egui::Painter;

use crate::panel::{self, PanelRow, stepped};
use crate::theme::{Size, Theme};
use crate::worker::{EditRequest, Job, JobResult, Supervisor};

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
    /// Each page's size in points, by index, once the worker has said.
    ///
    /// A `/MediaBox` is not always letter, and a canvas that assumed 612×792 would place a page of
    /// the wrong shape in a rectangle of the right one — which is invisible on every fixture that
    /// *is* letter.
    pub page_points: Vec<(f64, f64)>,
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
/// A page as it has been drawn: the pixels, and where they came from.
///
/// `Debug` by hand rather than derived: a texture handle is a GPU resource that says nothing
/// useful when printed, and the application is `Debug` because a window nobody can print is a
/// window nobody can test.
#[derive(Default)]
pub struct Pages {
    /// Each page drawn so far, by index.
    pub drawn: HashMap<usize, DrawnPage>,
}

impl std::fmt::Debug for Pages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pages")
            .field("drawn", &self.drawn.len())
            .finish()
    }
}

/// One page's pixels, with the placement that made them.
///
/// The placement travels with the texture because the window needs both: the texture is what to
/// draw and the placement is what says where a page point went. Keeping them apart is how a
/// click ends up on a glyph that is not the one under the pointer.
///
/// `Debug` by hand rather than derived: a texture handle is a GPU resource that says nothing
/// useful when printed.
pub struct DrawnPage {
    /// The pixels.
    pub handle: egui::TextureHandle,
    /// The renderer's placement: page space to buffer pixels.
    pub matrix: mangle_content::Matrix,
}

impl std::fmt::Debug for DrawnPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [w, h] = self.handle.size();
        f.debug_struct("DrawnPage")
            .field("pixels", &(w, h))
            .field("matrix", &self.matrix)
            .finish()
    }
}

impl DrawnPage {
    /// The placement of a page drawn in `rect`.
    #[must_use]
    pub fn placement(&self, rect: egui::Rect) -> crate::canvas::PagePlacement {
        let [w, h] = self.handle.size();
        crate::canvas::PagePlacement::new(rect, (w, h), self.matrix)
    }
}

/// The application: the theme, the state, the worker and the pages it has drawn.
///
/// `Debug` by hand rather than derived: a `Supervisor` holds a channel to a thread and a `Pages`
/// holds texture handles, and the useful thing to print is what the window knows, not the handles
/// it draws them with.
pub struct App {
    theme: Theme,
    state: State,
    /// The document's page rectangles, in window points, recomputed each frame.
    pages: Vec<egui::Rect>,
    /// The worker that opens documents and rasterizes pages.
    worker: Supervisor,
    /// The pixels of each page.
    pages_pixels: Pages,
    /// Why a page was not fully drawn, to show the user rather than to swallow.
    notes: Vec<String>,
    /// The toolbar's Open button was pressed and the dialog has not answered yet.
    pending_open: bool,
    /// What is on the page, as the worker summarised it.
    objects: Vec<mangle_edit::Summary>,
    /// Which object is selected, by index into `objects`.
    selected: Option<usize>,
    /// Where a drag started on the page, and what the object under it was.
    drag: Option<Drag>,
}

/// A drag in progress: where the pointer went down, and what it grabbed.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    /// The page point under the pointer when it went down.
    start: (f64, f64),
    /// Which object was under it.
    object: usize,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Every field is named rather than elided with `..`: a `Debug` that prints some fields
        // and hides the rest with a dot is a `Debug` that lies about what the window knows. The
        // theme and the page rectangles are the two that say nothing useful about a window and
        // are both trivially small, so they are printed rather than dropped.
        f.debug_struct("App")
            .field("theme", &self.theme)
            .field("objects", &self.objects.len())
            .field("selected", &self.selected)
            .field("drag", &self.drag)
            .field("state", &self.state)
            .field("pages", &self.pages.len())
            .field("worker", &self.worker)
            .field("drawn", &self.pages_pixels.drawn.len())
            .field("notes", &self.notes)
            .field("pending_open", &self.pending_open)
            .finish()
    }
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
            worker: Supervisor::start(),
            pages_pixels: Pages::default(),
            notes: Vec::new(),
            pending_open: false,
            objects: Vec::new(),
            selected: None,
            drag: None,
        }
    }

    /// Ask the worker to open a file. The shell fills in from the messages it gets
    /// back, so this returns before anything has been read.
    pub fn open(&mut self, path: impl Into<PathBuf>) {
        self.state.file_name = None;
        self.state.page_count = 0;
        self.state.page_points.clear();
        self.state.page_index = 0;
        self.state.bookmarks.clear();
        self.pages_pixels.drawn.clear();
        self.notes.clear();
        self.worker.ask(Job::Open(path.into()));
    }

    /// Ask the worker for a page at a new zoom.
    ///
    /// The pixels are thrown away rather than scaled: a buffer drawn at twice its width is a
    /// buffer that is blurry at any zoom but the one it was made for, and the whole point of a
    /// vector page is that it can be re-drawn sharp.
    pub fn set_zoom(&mut self, zoom: f32) {
        self.state.zoom = zoom.clamp(0.25, 4.0);
        self.pages_pixels.drawn.clear();
        for i in 0..self.state.page_count {
            self.worker.ask(Job::Page(i, f64::from(self.state.zoom)));
        }
    }

    /// Take whatever the worker has sent.
    pub fn pump(&mut self, ctx: &egui::Context) {
        for result in self.worker.drain() {
            match result {
                JobResult::Opened {
                    name,
                    pages,
                    bookmarks,
                } => {
                    self.state.file_name = Some(name);
                    self.state.page_count = pages;
                    self.state.page_index = 0;
                    self.state.bookmarks = bookmarks;
                    self.state.page_points = vec![(612.0, 792.0); pages];
                }
                JobResult::Page {
                    index,
                    rgba,
                    width,
                    height,
                    points,
                    rotate,
                    objects,
                    notes,
                } => {
                    if let Some(slot) = self.state.page_points.get_mut(index) {
                        *slot = points;
                    }
                    // What is on the page, in the window's hands. The summaries are all the
                    // canvas needs: the box, the kind and the spans. The records stay in the
                    // worker, which is where an edit is applied.
                    if index == self.state.page_index {
                        self.objects = objects;
                        // The indices are this page's, so a selection carried over from another
                        // page would now point at whatever happens to be at the same number. It
                        // goes rather than being silently reinterpreted.
                        self.select(self.selected.filter(|at| *at < self.objects.len()));
                    }
                    // Notes are the reason a page is not all there: they are shown, because a
                    // blank region the user cannot account for is worse than a line of text
                    // explaining it.
                    for note in notes {
                        if !self.notes.contains(&note) {
                            self.notes.push(note);
                        }
                    }
                    let handle = ctx.load_texture(
                        format!("page-{index}"),
                        egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba),
                        egui::TextureOptions::LINEAR,
                    );
                    let matrix = mangle_render::page::Placement::fit(
                        &mangle_syntax::Rect::new(0.0, 0.0, points.0, points.1),
                        (width, height),
                        1.0,
                        rotate,
                    )
                    .matrix;
                    self.pages_pixels
                        .drawn
                        .insert(index, DrawnPage { handle, matrix });
                    // The canvas is stale until this frame is redrawn, and a message arriving
                    // from another thread is exactly the case where nothing else would ask.
                    ctx.request_repaint();
                }
                JobResult::Saved { bytes, objects } => {
                    // A save is worth saying out loud: the user asked for a file, and got one.
                    self.state.notice = Some(format!(
                        "saved {bytes} bytes, rewriting {objects} stream(s)"
                    ));
                }
                JobResult::Failed { reason } => {
                    self.tell(reason);
                }
            }
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
        // The worker's answers arrive first, so the regions below draw the page that came in
        // rather than the one from the frame before.
        self.pump(ui.ctx());
        // Then the pointer, so a click on this frame selects on this frame.
        self.pointer(ui);
        // The file dialog is a windowing thing, so it opens outside the regions and the answer
        // comes back as a path. `rfd` is the native dialog, which is what a user expects of a
        // desktop program.
        if std::mem::take(&mut self.pending_open)
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .pick_file()
        {
            self.open(path);
        }
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

        // **Ask for the next frame.** egui calls `ui` only when something wants a repaint, so a
        // window that has drawn its chrome and is waiting on a worker would otherwise never call
        // `pump` again — and the page the worker finishes would sit in the channel with nothing
        // reading it. Polling at a fixed cadence is what makes the message loop run at all; a
        // page that arrives asks for a repaint straight away, so the poll is only a ceiling on
        // how long the window takes to notice, not the latency.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
}

impl App {
    /// What the pointer is doing to the page.
    ///
    /// A click selects; a drag moves by the distance it travelled. The arithmetic is done here
    /// and *once*: a pointer position becomes a page point through the page's placement, and the
    /// edit that goes to the worker is a plain `Move { dx, dy }` in page points. A window that
    /// sent raw pointer deltas would be leaving the arithmetic of "how far is that in the file's
    /// own units" to be done twice.
    fn pointer(&mut self, ui: &egui::Ui) {
        let Some(i) = self
            .state
            .page_index
            .checked_sub(0)
            .filter(|i| *i < self.state.page_count)
        else {
            return;
        };
        let page_rect = match self.pages.get(i) {
            Some(r) => *r,
            None => return,
        };
        // The placement travels with the page's texture, so a click and the pixel agree.
        let Some(drawn) = self.pages_pixels.drawn.get(&i) else {
            return;
        };
        let placement = drawn.placement(page_rect);

        let Some(hover) = ui.ctx().input(|i| i.pointer.hover_pos()) else {
            return;
        };
        // Undo and redo are the two keys a PDF viewer is expected to answer.
        let (undo, redo) = ui.ctx().input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::Z),
                i.modifiers.command && i.key_pressed(egui::Key::Y),
            )
        });
        if undo || redo {
            self.worker
                .ask(if undo { Job::Undo(i) } else { Job::Redo(i) });
            return;
        }

        // **The page answers for the page, and for nothing else.** A click on a stepper in the
        // right-hand panel used to arrive here first, miss the paper, and take the selection
        // away — so the panel redrew itself with nothing selected, its controls were never
        // drawn, and the click that was meant to nudge a size did nothing at all. A press off
        // the paper is the panel's business, not the canvas's.
        //
        // A drag that has already begun is the one thing this still finishes out here: the
        // pointer may leave the paper while the user is still holding the button, and the move
        // has to be measured to where they got to rather than abandoned.
        let on_page = page_rect.contains(hover);
        if !on_page && self.drag.is_none() {
            return;
        }

        let pressed = ui.ctx().input(|i| i.pointer.any_pressed());
        let released = ui.ctx().input(|i| i.pointer.any_released());
        // The decision is pure and tested below; only the acting on it is here.
        match pointer_outcome(
            &self.objects,
            self.drag,
            &placement,
            hover,
            pressed,
            released,
        ) {
            Pointer::Idle => {}
            Pointer::Deselect => {
                self.drag = None;
                self.select(None);
            }
            Pointer::Select { object } => {
                self.drag = Some(Drag {
                    start: placement.to_page(hover).unwrap_or((0.0, 0.0)),
                    object,
                });
                self.select(Some(object));
            }
            Pointer::Move { object, dx, dy } => {
                // A click that did not move is a selection, not an edit; the decision above
                // makes that distinction and this is where the edit is asked for.
                self.drag = None;
                self.select(Some(object));
                self.worker.ask(Job::Edit {
                    page: i,
                    object,
                    request: EditRequest::Move { dx, dy },
                });
            }
        }
    }

    /// Select an object, and open the panel that object is contextual to.
    ///
    /// The two are one method because they are one fact: the panel is *about* the selection, and
    /// a window that updated one without the other would show a text panel for a photograph.
    /// Setting the selection directly is what left the panel on "Document Properties" while a
    /// box was drawn around a run of text.
    fn select(&mut self, object: Option<usize>) {
        self.selected = object;
        self.state.right = panel::panel_for(object.and_then(|i| self.objects.get(i)));
    }

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
            // The Open button is real: it asks the file dialog, and the dialog's answer goes to
            // the worker. Save and Print stay inert, which is the honest state of the product.
            if label == "Open" && ui.ctx().input(|i| i.pointer.any_released()) && hovered(ui, r) {
                self.pending_open = true;
            }
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

        let spacing = s.page_gap;
        let mut y = rect.top() + spacing;
        self.pages.clear();
        for i in 0..self.state.page_count {
            // The page is as large as the file says it is, and only as wide as the zoom makes
            // it. A page of a different shape gets its own rectangle, not a letter-shaped one
            // with its content squeezed into a corner.
            let (pw, ph) = self
                .state
                .page_points
                .get(i)
                .copied()
                .unwrap_or((612.0, 792.0));
            let page_size = egui::vec2(pw as f32, ph as f32) * self.state.zoom;
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
            // The page itself, once the worker has rasterized it. Until then the paper is what
            // there is, which is the honest state of a page that has not arrived.
            if let Some(drawn) = self.pages_pixels.drawn.get(&i) {
                p.image(
                    drawn.handle.id(),
                    r,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            // The selection, drawn onto the page the way a designer sees it: a box at the
            // object's own bounds, and handles at its corners.
            if i == self.state.page_index
                && let Some(object) = self.selected.and_then(|at| self.objects.get(at))
                && let Some(drawn) = self.pages_pixels.drawn.get(&i)
            {
                let box_ = drawn.placement(r);
                let corners = [
                    (object.bounds.x0, object.bounds.y0),
                    (object.bounds.x1, object.bounds.y0),
                    (object.bounds.x1, object.bounds.y1),
                    (object.bounds.x0, object.bounds.y1),
                ];
                let pts: Vec<egui::Pos2> = corners
                    .iter()
                    .filter_map(|(x, y)| box_.to_window(*x, *y))
                    .collect();
                if pts.len() == 4 {
                    p.add(egui::Shape::convex_polygon(
                        pts.clone(),
                        egui::Color32::from_rgba_unmultiplied(90, 150, 255, 40),
                        egui::Stroke::new(1.5, t.accent),
                    ));
                    // Handles, so a corner reads as a corner.
                    for corner in &pts {
                        p.rect_filled(
                            egui::Rect::from_center_size(*corner, egui::vec2(7.0, 7.0)),
                            1.0,
                            t.page,
                        );
                        p.rect_stroke(
                            egui::Rect::from_center_size(*corner, egui::vec2(7.0, 7.0)),
                            1.0,
                            egui::Stroke::new(1.0, t.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            }
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

        // The selected object's own text state, which is what every value in this panel is read
        // out of. Read from the model rather than held by the window: a panel that remembered a
        // value would be showing the wrong number the moment an edit landed.
        let text = self
            .selected
            .and_then(|i| self.objects.get(i))
            .and_then(|o| o.text.as_ref());

        // What a control was clicked by, applied after the loop rather than inside it. The loop
        // holds a borrow of the selected object to read its values, and the edit ends what that
        // borrow began — so the clicks are remembered here and acted on after the loop.
        let mut clicked: Option<(mangle_edit::TextProperty, f64)> = None;
        let mut clicked_do: Option<panel::Do> = None;

        for (row, name) in rows_for(self.state.right).into_iter().enumerate() {
            let r = egui::Rect::from_min_size(
                egui::pos2(rect.left() + s.window_inset, y),
                egui::vec2(width - s.window_inset * 2.0, t.metrics.row_height),
            );
            if r.bottom() > rect.bottom() {
                break;
            }
            hoverable(ui, p, r, t);
            p.text(
                r.left_center(),
                egui::Align2::LEFT_CENTER,
                name,
                t.type_scale.font_id(Size::Row),
                t.text_muted,
            );
            // A control on the right of each row, which is what makes the panel a
            // panel rather than a list of words.
            let field = egui::Rect::from_center_size(
                r.right_center() + egui::vec2(-s.item_gap, 0.0),
                egui::vec2(r.width() * 0.5, r.height() - s.item_gap * 2.0),
            );
            let enabled = text.is_some();
            p.rect_stroke(
                field,
                s.field_radius,
                egui::Stroke::new(1.0, t.border),
                egui::StrokeKind::Middle,
            );
            if enabled && hovered(ui, field) {
                p.rect_filled(field, s.field_radius, t.hover);
            }
            let ink = if enabled { t.text } else { t.text_muted };
            let which = panel::panel_row(self.state.right, row, text);
            match which {
                PanelRow::Empty => {}
                PanelRow::Value(value) => {
                    p.text(
                        field.center(),
                        egui::Align2::CENTER_CENTER,
                        value,
                        t.type_scale.font_id(Size::Row),
                        ink,
                    );
                }
                PanelRow::Stepper {
                    shown,
                    step,
                    property,
                } => {
                    // The value in the middle of the field, with the two halves of the stepper
                    // inside it — so the whole control is one rectangle a user can aim at, and
                    // the number is not pushed somewhere else by the buttons beside it.
                    let quarter = field.width() / 4.0;
                    p.text(
                        field.center(),
                        egui::Align2::CENTER_CENTER,
                        shown,
                        t.type_scale.font_id(Size::Row),
                        ink,
                    );
                    for (by, label, left) in [
                        (-step, "−", field.left()),
                        (step, "+", field.right() - quarter),
                    ] {
                        let part = egui::Rect::from_min_size(
                            egui::pos2(left, field.top()),
                            egui::vec2(quarter, field.height()),
                        );
                        p.text(
                            part.center(),
                            egui::Align2::CENTER_CENTER,
                            label,
                            t.type_scale.font_id(Size::Row),
                            t.text_muted,
                        );
                        if hovered(ui, part) && ui.ctx().input(|i| i.pointer.any_released()) {
                            clicked = Some((property, by));
                        }
                    }
                }
            }
            y += t.metrics.row_height;
        }

        if let Some((property, by)) = clicked {
            self.step_text(property, by);
        }
        if let Some(do_) = clicked_do {
            self.act(do_);
        }

        // **Arrange is a section of this panel, not a panel of its own** — the mockup puts its
        // four buttons under the properties, and a right-hand side that swapped its whole
        // composition to move an object in the z-order would be a side that forgot what it was
        // showing. It is drawn for whatever is selected, because anything on a page has an order
        // in it; with nothing selected there is nothing to move, so the section is not drawn.
        if self.selected.is_some() {
            arrange_section(ui, p, t, rect, &mut y, &mut clicked_do);
            if let Some(do_) = clicked_do {
                self.act(do_);
            }
        }
    }

    /// Do what a panel control asks, on the selected object.
    fn act(&mut self, do_: panel::Do) {
        let Some(object) = self.selected else {
            return;
        };
        match do_ {
            panel::Do::Arrange(arrange) => self.worker.ask(Job::Arrange {
                page: self.state.page_index,
                object,
                arrange,
            }),
        }
    }

    /// One click of a stepper: set the selected object's property to what it has, plus `by`.
    ///
    /// The value it has is read out of the object's own text state rather than out of the panel,
    /// so a click after an undo moves from where the object really is and not from where the
    /// window last remembered it.
    fn step_text(&mut self, property: mangle_edit::TextProperty, by: f64) {
        // The decision is pure and tested in `panel`; only the asking for the edit is here.
        let Some((object, next)) = self
            .selected
            .and_then(|at| self.objects.get(at))
            .and_then(|o| o.text.as_ref())
            .and_then(|state| stepped(property, state, by))
            .map(|next| (self.selected.unwrap_or(0), next))
        else {
            return;
        };
        self.worker.ask(Job::Edit {
            page: self.state.page_index,
            object,
            request: EditRequest::Text(next),
        });
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

/// The Arrange section: four buttons, two to a row, under the panel's properties.
///
/// A free function because it needs nothing from the window but a painter and a place to draw: the
/// labels and the verbs both come from `panel`, which is what keeps them together, and what the
/// section is *for* is the selected object, which the caller already knows about. Taking `&mut
/// self` would have been a signature that promised more than the body reads.
fn arrange_section(
    ui: &egui::Ui,
    p: &Painter,
    t: &Theme,
    rect: egui::Rect,
    y: &mut f32,
    clicked_do: &mut Option<panel::Do>,
) {
    let s = t.spacing;
    *y += t.metrics.row_height * 0.5;
    p.text(
        egui::pos2(rect.left() + s.window_inset, *y),
        egui::Align2::LEFT_TOP,
        "Arrange",
        t.type_scale.font_id(Size::Heading),
        t.text,
    );
    *y += t.metrics.row_height;

    // Two to a row, so the four fit under the properties without the section running off the
    // bottom of a short window.
    let cell = egui::vec2(
        (rect.width() - s.window_inset * 2.0 - s.item_gap) / 2.0,
        t.metrics.row_height * 2.0,
    );
    for (i, name) in panel::ARRANGE_ROWS.iter().enumerate() {
        let at = egui::pos2(
            rect.left() + s.window_inset + (i % 2) as f32 * (cell.x + s.item_gap),
            *y + (i / 2) as f32 * (cell.y + s.item_gap),
        );
        let button = egui::Rect::from_min_size(at, cell);
        if button.bottom() > rect.bottom() {
            return;
        }
        // Filled rather than outlined: a button is a thing to press, and that has to be visible
        // before anyone touches it.
        p.rect_filled(button, s.field_radius, t.hover);
        p.rect_stroke(
            button,
            s.field_radius,
            egui::Stroke::new(1.0, t.border),
            egui::StrokeKind::Middle,
        );
        p.text(
            button.center(),
            egui::Align2::CENTER_CENTER,
            *name,
            t.type_scale.font_id(Size::Row),
            t.text,
        );
        if hovered(ui, button) && ui.ctx().input(|i| i.pointer.any_released()) {
            *clicked_do = panel::arrange_of(i);
        }
    }
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
        // The names live in `panel`, beside the indices of the rows that are steppers: a row
        // inserted here without one there is a row that silently stops being a control.
        RightPanel::Text => panel::TEXT_ROWS.to_vec(),
        RightPanel::Colour => vec!["Fill", "Stroke", "Line Width", "Opacity", "Blend Mode"],
        // The names live in `panel`, beside which of them is a button: a row inserted here
        // without one there is a row that silently stops being one.
        RightPanel::Arrange => panel::ARRANGE_ROWS.to_vec(),
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

/// What a pointer event did to the selection, decided without any windowing.
///
/// Taking the window's input as plain values is what makes the interaction testable: the
/// arithmetic of "is this a click or a drag, and on what" is the part that is easy to get wrong,
/// and a test that needed a real mouse could not answer it at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pointer {
    /// Nothing happened.
    Idle,
    /// A click on the paper, away from anything: the selection goes.
    Deselect,
    /// A click on an object, which becomes the selection.
    Select {
        /// Which object.
        object: usize,
    },
    /// A drag that travelled, which asks for a move by that distance.
    Move {
        /// Which object.
        object: usize,
        /// How far right, in page points.
        dx: f64,
        /// How far down, in page points.
        dy: f64,
    },
}

/// Decide what a pointer event means.
///
/// `hover` is where the pointer is, `pressed` and `released` are this frame's buttons, and `drag`
/// is a drag that began on an earlier frame. The placement turns the pointer into a page point,
/// which is the only windowing thing left and is itself a pure function tested in `canvas`.
fn pointer_outcome(
    objects: &[mangle_edit::Summary],
    drag: Option<Drag>,
    placement: &crate::canvas::PagePlacement,
    hover: egui::Pos2,
    pressed: bool,
    released: bool,
) -> Pointer {
    if pressed {
        // A click on the paper, away from anything: the selection goes.
        let Some((x, y)) = placement.to_page(hover) else {
            return Pointer::Deselect;
        };
        // The topmost object under the point, which is the one the user can see.
        return match objects
            .iter()
            .enumerate()
            .rev()
            .find(|(_, o)| o.contains(x, y))
            .map(|(i, _)| i)
        {
            Some(object) => Pointer::Select { object },
            None => Pointer::Deselect,
        };
    }
    if released
        && let Some(drag) = drag
        && let Some((x, y)) = placement.to_page_clamped(hover)
    {
        let dx = x - drag.start.0;
        let dy = y - drag.start.1;
        // A click that did not move is a selection, not an edit.
        if dx.abs() > 1e-6 || dy.abs() > 1e-6 {
            return Pointer::Move {
                object: drag.object,
                dx,
                dy,
            };
        }
    }
    Pointer::Idle
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for;
    // the panic-free rule is about what the product does with a file, not about tests.
    // `single_range_in_vec_init` fires on a one-element range list, and here that list *is* the
    // subject: an object's spans are a list of byte ranges, which is one element when the object
    // is one operation.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::single_range_in_vec_init
    )]

    use super::*;
    use mangle_content::Matrix;
    use mangle_edit::Summary;

    /// A letter page drawn at one pixel per point, so a window point is a page point.
    fn placement() -> crate::canvas::PagePlacement {
        crate::canvas::PagePlacement::new(
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(612.0, 792.0)),
            (612, 792),
            Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 792.0),
        )
    }

    fn object(index: usize, x0: f64, y0: f64, x1: f64, y1: f64) -> Summary {
        let _ = index;
        Summary {
            kind: mangle_edit::Kind::Image,
            bounds: mangle_content::state::ClipBounds { x0, y0, x1, y1 },
            spans: vec![0..4],
            form: None,
            line_break: None,
            text: None,
        }
    }

    /// A click on an object selects it — and the topmost one, because that is what a user can
    /// see and click.
    #[test]
    fn a_click_on_an_object_selects_the_topmost() {
        let low = object(0, 0.0, 0.0, 100.0, 100.0);
        let high = object(1, 0.0, 0.0, 100.0, 100.0);
        let p = placement();
        let got = pointer_outcome(&[low, high], None, &p, egui::pos2(50.0, 742.0), true, false);
        assert_eq!(got, Pointer::Select { object: 1 });
    }

    /// A click on the paper, away from everything, drops the selection.
    #[test]
    fn a_click_on_the_paper_drops_the_selection() {
        let only = object(0, 0.0, 0.0, 10.0, 10.0);
        let p = placement();
        let got = pointer_outcome(&[only], None, &p, egui::pos2(600.0, 10.0), true, false);
        assert_eq!(got, Pointer::Deselect);
    }

    /// A drag moves by the distance it travelled, in the page's own coordinates — which is why
    /// the arithmetic goes through the placement rather than being the raw pointer delta.
    #[test]
    fn a_drag_moves_by_its_distance_in_page_points() {
        let target = object(0, 100.0, 300.0, 200.0, 400.0);
        let p = placement();
        let drag = Drag {
            start: (110.0, 310.0),
            object: 0,
        };
        // Down and right by 40 in the window is down and right by 40 on the page.
        let got = pointer_outcome(
            &[target],
            Some(drag),
            &p,
            egui::pos2(150.0, 462.0),
            false,
            true,
        );
        match got {
            Pointer::Move { object, dx, dy } => {
                assert_eq!(object, 0);
                // The tolerance is a hundredth of a point, not a float epsilon: the drag went
                // through an inverse and a forward transform, and what is being asserted is the
                // distance, not the last bit of it. A tenth of a point is what GOAL.md §4.1's
                // fidelity law asks a position to be right to, so a hundredth is comfortably
                // inside it and still catches a real error.
                assert!((dx - 40.0).abs() < 1e-2, "dx was {dx}");
                assert!((dy - 20.0).abs() < 1e-2, "dy was {dy}");
            }
            other => panic!("a drag should be a move, was {other:?}"),
        }
    }

    /// A click that did not move is a selection, not an edit — otherwise every click would ask
    /// the worker to move the object by nothing.
    #[test]
    fn a_click_that_did_not_move_is_not_an_edit() {
        let target = object(0, 100.0, 300.0, 200.0, 400.0);
        let p = placement();
        let drag = Drag {
            start: (110.0, 310.0),
            object: 0,
        };
        let got = pointer_outcome(
            &[target],
            Some(drag),
            &p,
            egui::pos2(110.0, 482.0),
            false,
            true,
        );
        assert_eq!(
            got,
            Pointer::Idle,
            "a click that did not travel is not an edit"
        );
    }
}

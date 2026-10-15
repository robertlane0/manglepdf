//! The worker that opens a document, rasterizes its pages, and applies edits.
//!
//! # Why the UI thread does none of this
//!
//! The charter's hardest rule for this crate is that the UI thread never parses, decodes or
//! rasterizes. It is not a performance rule: `render_page` on a large page is hundreds of
//! milliseconds, and a window that stops answering for that long is a window the user has to
//! kill. So the work happens on a thread and arrives here as a message, and the UI thread's only
//! job is to turn pixels into a texture.
//!
//! # One source of truth
//!
//! After an edit the worker **re-opens what it has just written** and renders that. A canvas that
//! drew the model in memory would be free to disagree with the file on disk, and the user's
//! opinion of the edit would be formed by whichever of the two they were looking at. GOAL.md
//! §4.1's sixth law is that the canvas draws the content that will be saved, and re-opening is the
//! cheapest way to make that literally true.
//!
//! # The edit session
//!
//! One [`mangle_edit::Editor`] at a time, for the page being edited, holding the document in an
//! `Arc` the worker also lent it. It is dropped whenever the document is replaced, because a
//! session belongs to the bytes it was opened over — after a save those bytes have moved, so the
//! session's history describes a document that is no longer in hand.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use mangle_content::Resources;
use mangle_doc::PageTree;
use mangle_render::page::{PageRender, RenderOptions, render_page};
use mangle_syntax::document::Document;
use mangle_syntax::{OpenOptions, object::Ref};

/// What to do, as a message.
#[derive(Debug, Clone)]
pub enum Job {
    /// Open a file. The first page is rendered as soon as it is open.
    Open(PathBuf),
    /// Render the page at this index, at this many pixels per point.
    Page(usize, f64),
    /// Apply an edit to an object of a page.
    Edit {
        /// Which page, from zero.
        page: usize,
        /// Which of the page's objects, by index.
        object: usize,
        /// What to do to it.
        request: EditRequest,
    },
    /// Move an object of a page in the z-order.
    Arrange {
        /// Which page, from zero.
        page: usize,
        /// Which of the page's objects, by index.
        object: usize,
        /// Which way.
        arrange: mangle_edit::Arrange,
    },
    /// Undo the last edit on this page.
    Undo(usize),
    /// Redo the last undone edit on this page.
    Redo(usize),
    /// Close whatever is open.
    Close,
}

/// An edit as the window was asked to make it.
///
/// Deliberately *not* a gesture. A drag arrives here as a `Move` in page points, computed by the
/// window from the pointer and the page's placement, so the arithmetic of "twelve points to the
/// right" is done once and tested once rather than in both places.
#[derive(Debug, Clone, PartialEq)]
pub enum EditRequest {
    /// Move the object by this many points.
    Move {
        /// How far right.
        dx: f64,
        /// How far down.
        dy: f64,
    },
    /// Remove it.
    Delete,
    /// Turn it about the given point, in degrees counter-clockwise.
    ///
    /// The point is decided by the window, which is the only side that knows the object's box in
    /// page space — the arithmetic of "about its own centre" needs that box, and a worker that
    /// was handed a rotation with no centre would have to look one up.
    Rotate {
        /// How far, in degrees counter-clockwise.
        degrees: f64,
        /// The centre to turn about, in page space.
        about: (f64, f64),
    },
    /// Set one text property of a run of glyphs, to this value.
    ///
    /// The absolute value is what makes this one edit: a stepper nudges a number it read out of
    /// the object's own text state, so the value that arrives here is already the answer and the
    /// worker is not asked to know what a click meant.
    Text(mangle_edit::TextProperty),
    /// Replace the run's shown string with this text, in its own font.
    ///
    /// The whole string is what travels, not a diff: the worker owns the session and the
    /// session owns the font table, so the window hands over what the user typed and the
    /// refusal names what cannot be written — a missing glyph, a `TJ`'s kerns — in the
    /// worker's reply rather than in the window.
    Retype {
        /// What the run should say instead.
        text: String,
    },
    /// Recolour the object with this `#RRGGBB`, in its own colour space.
    ///
    /// The hex travels unparsed: the worker parses it, and what does not parse is
    /// answered as a failure rather than written, so garbage in the field can never
    /// become an edit.
    Recolour {
        /// What the colour field held when it was committed.
        hex: String,
    },
}

/// What came back.
#[derive(Debug, Clone)]
pub enum JobResult {
    /// The file opened: its name, how many pages, and the first row of bookmarks.
    Opened {
        /// The file's own name, without its directories.
        name: String,
        /// How many pages it has.
        pages: usize,
        /// The first row of the bookmark tree, until a document supplies its own.
        bookmarks: Vec<String>,
    },
    /// A page came back as pixels.
    Page {
        /// Which page, from zero.
        index: usize,
        /// The pixels, top-down, eight bits per channel.
        rgba: Vec<u8>,
        /// How wide the buffer is, in pixels.
        width: usize,
        /// How tall it is, in pixels.
        height: usize,
        /// The page's size in points, which is what the canvas lays it out from.
        points: (f64, f64),
        /// The page's rotation, which the canvas has to lay it out with.
        rotate: i32,
        /// What is on the page, in the order the file drew it.
        objects: Vec<mangle_edit::Summary>,
        /// The page's whole text, in the order the file drew it.
        ///
        /// The page's own text rather than the objects' contents joined: an object's content is
        /// what the *panel* shows for one selection, and a search runs over the page because a
        /// phrase a user types is a phrase of prose, and prose does not respect where one object's
        /// box ends.
        text: String,
        /// Why anything on it was not drawn.
        notes: Vec<String>,
    },
    /// An edit was saved, and the saved file re-opened.
    Saved {
        /// How many bytes the saved file is.
        bytes: usize,
        /// How many objects the revision rewrote.
        objects: usize,
        /// What the status line shows about a substitution, if the edit made one.
        notice: Option<String>,
    },
    /// The file could not be opened, or the job could not be done.
    Failed {
        /// What went wrong, in a sentence a user can read.
        reason: String,
    },
}

/// Everything the worker holds between jobs.
struct Worker {
    /// The open document, shared with the edit session.
    doc: Arc<Document>,
    /// The page tree of the open document.
    pages: PageTree,
    /// Each page's resources, in page order.
    resources: Vec<Resources>,
    /// The edit session, one page at a time.
    session: Option<mangle_edit::Editor>,
    /// Replacement system font directories (`--font-dirs`) for renders and fallbacks.
    system_dirs: Vec<PathBuf>,
}

impl Worker {
    /// Open `path` and read everything a job needs from it.
    fn open(path: &PathBuf, system_dirs: Vec<PathBuf>) -> Result<Self, String> {
        let bytes = std::fs::read(path).unwrap_or_default();
        let doc = Document::open(bytes, OpenOptions::default()).map_err(|e| e.to_string())?;
        let doc = Arc::new(doc);
        let pages =
            PageTree::build(doc.as_ref(), root_of(doc.as_ref())).map_err(|e| e.to_string())?;
        let resources = pages
            .pages()
            .iter()
            .map(|page| resources_of(&doc, page))
            .collect();
        Ok(Self {
            doc,
            pages,
            resources,
            session: None,
            system_dirs,
        })
    }

    /// Render one page, sending the pixels and what is on it.
    fn page(&self, outbox: &Sender<JobResult>, index: usize, scale: f64) {
        let Some(page) = self.pages.get(index) else {
            let _ = outbox.send(JobResult::Failed {
                reason: format!("this document has no page {}", index + 1),
            });
            return;
        };
        let Some(resources) = self.resources.get(index) else {
            let _ = outbox.send(JobResult::Failed {
                reason: "the page's resources could not be read".to_string(),
            });
            return;
        };
        let render: PageRender = render_page(
            self.doc.as_ref(),
            page,
            resources,
            RenderOptions {
                scale,
                font_dirs: self.system_dirs.clone(),
                ..RenderOptions::default()
            },
        );
        // The model is built here rather than in the window, because it is the whole document's
        // worth of records and the window needs the bounds only.
        let run = mangle_content::interp::run_with(
            &mangle_content::ContentStream::parse(&page.decoded_contents(self.doc.as_ref())),
            resources,
        );
        let model = mangle_edit::PageModel::build(&run.records);
        let _ = outbox.send(JobResult::Page {
            index,
            rgba: render.image.pixels.clone(),
            width: render.image.width,
            height: render.image.height,
            points: page.inherited.displayed_size(),
            rotate: page.inherited.rotation(),
            objects: self
                .resources
                .get(index)
                .map(|resources| {
                    model
                        .objects()
                        .iter()
                        .map(|o| mangle_edit::summarise(&self.doc, resources, o))
                        .collect()
                })
                .unwrap_or_default(),
            text: self
                .resources
                .get(index)
                .map(|resources| mangle_edit::page_text(resources, &self.doc, &model))
                .unwrap_or_default(),
            notes: render.notes,
        });
    }

    /// The edit session for `page`, opened if this worker does not have one yet.
    ///
    /// The session is rebuilt when the page changes, so a second edit on the same page keeps the
    /// first one's history. **It is opened on demand rather than on the first edit**, because an
    /// arrange is often the very first thing asked of a page — moving an object nobody has edited
    /// — and a worker that refused until something had been edited would refuse the most ordinary
    /// request it gets. Returns `false` after sending the reason if it could not be opened.
    fn session_for(&mut self, outbox: &Sender<JobResult>, page: usize) -> bool {
        if self.session.as_ref().is_none_or(|e| e.page().index != page) {
            let (Some(page_dict), Some(page_resources)) =
                (self.pages.get(page), self.resources.get(page))
            else {
                let _ = outbox.send(JobResult::Failed {
                    reason: format!("this document has no page {}", page + 1),
                });
                return false;
            };
            match mangle_edit::Editor::open(Arc::clone(&self.doc), page_dict, page_resources) {
                Ok(mut editor) => {
                    editor.set_system_dirs(self.system_dirs.clone());
                    self.session = Some(editor);
                }
                Err(e) => {
                    let _ = outbox.send(JobResult::Failed {
                        reason: e.to_string(),
                    });
                    return false;
                }
            }
        }
        self.session.is_some()
    }

    /// Apply an edit, save it, and draw the file as saved.
    fn edit(
        &mut self,
        outbox: &Sender<JobResult>,
        page: usize,
        object: usize,
        request: &EditRequest,
    ) {
        if !self.session_for(outbox, page) {
            return;
        }
        let Some(editor) = self.session.as_mut() else {
            return;
        };
        let change = match request {
            EditRequest::Move { dx, dy } => mangle_edit::Change::move_by(*dx, *dy),
            EditRequest::Delete => mangle_edit::Change::Delete,
            EditRequest::Rotate { degrees, about } => {
                mangle_edit::Change::rotate_about(about.0, about.1, *degrees)
            }
            EditRequest::Text(property) => mangle_edit::Change::Text(*property),
            EditRequest::Retype { text } => mangle_edit::Change::Retype { text: text.clone() },
            EditRequest::Recolour { hex } => match parse_hex_colour(hex) {
                Some(colour) => mangle_edit::Change::recolour(colour),
                None => {
                    let _ = outbox.send(JobResult::Failed {
                        reason: format!(
                            "{hex:?} is not a `#RRGGBB` colour, so nothing was recoloured"
                        ),
                    });
                    return;
                }
            },
        };
        let notice = match editor.apply(object, &change) {
            Ok(outcome) => outcome.notice,
            Err(e) => {
                let _ = outbox.send(JobResult::Failed {
                    reason: e.to_string(),
                });
                return;
            }
        };
        self.save_and_redraw(outbox, page, notice);
    }

    /// Move an object in the z-order, save it, and draw the file as saved.
    ///
    /// This is the one edit that is not a `q … Q` wrapper: arranging *moves bytes* from one place
    /// in the stream to another and re-materialises the state at the destination, so the object
    /// is drawn by the same operators in a different position rather than by the same operators
    /// under a different matrix.
    fn arrange(
        &mut self,
        outbox: &Sender<JobResult>,
        page: usize,
        object: usize,
        arrange: mangle_edit::Arrange,
    ) {
        if !self.session_for(outbox, page) {
            return;
        }
        let Some(editor) = self.session.as_mut() else {
            return;
        };
        if let Err(e) = editor.arrange(object, arrange) {
            let _ = outbox.send(JobResult::Failed {
                reason: e.to_string(),
            });
            return;
        }
        self.save_and_redraw(outbox, page, None);
    }

    /// Save the session, then re-open what it wrote and draw *that*.
    ///
    /// Every mutating job ends here, and deliberately so: the canvas draws the file, not the
    /// model in memory, so the two cannot disagree. A save that failed is answered with its own
    /// reason rather than silently falling back to the unsaved page.
    fn save_and_redraw(&mut self, outbox: &Sender<JobResult>, page: usize, notice: Option<String>) {
        let saved = match self.session.as_mut().map(mangle_edit::Editor::save) {
            Some(Ok(save)) => save,
            Some(Err(e)) => {
                let _ = outbox.send(JobResult::Failed {
                    reason: e.to_string(),
                });
                return;
            }
            None => return,
        };
        let _ = outbox.send(JobResult::Saved {
            bytes: saved.bytes.len(),
            objects: saved.rewritten.len(),
            notice,
        });
        self.reopen(saved.bytes, outbox);
        self.page(outbox, page, 1.0);
    }

    /// Step the history of the page being edited, then draw what the history holds.
    fn step(&mut self, outbox: &Sender<JobResult>, page: usize, undo: bool) {
        let Some(editor) = self.session.as_mut() else {
            let _ = outbox.send(JobResult::Failed {
                reason: "nothing has been edited on this page yet".to_string(),
            });
            return;
        };
        let stepped = if undo { editor.undo() } else { editor.redo() };
        if let Err(e) = stepped {
            let _ = outbox.send(JobResult::Failed {
                reason: e.to_string(),
            });
            return;
        }
        let saved = match editor.save() {
            Ok(save) => save,
            Err(e) => {
                let _ = outbox.send(JobResult::Failed {
                    reason: e.to_string(),
                });
                return;
            }
        };
        let _ = outbox.send(JobResult::Saved {
            bytes: saved.bytes.len(),
            objects: saved.rewritten.len(),
            notice: None,
        });
        self.reopen(saved.bytes, outbox);
        self.page(outbox, page, 1.0);
    }

    /// Replace the open document with `bytes`, and throw the edit session away.
    ///
    /// A session belongs to the document it was opened over: after a save the bytes have moved,
    /// so its states no longer correspond to anything. The session is rebuilt when the next edit
    /// arrives.
    fn reopen(&mut self, bytes: Vec<u8>, outbox: &Sender<JobResult>) {
        match Document::open(bytes, OpenOptions::default()) {
            Ok(fresh) => {
                self.doc = Arc::new(fresh);
                match PageTree::build(self.doc.as_ref(), root_of(self.doc.as_ref())) {
                    Ok(pages) => {
                        self.resources = pages
                            .pages()
                            .iter()
                            .map(|p| resources_of(&self.doc, p))
                            .collect();
                        self.pages = pages;
                    }
                    Err(e) => {
                        let _ = outbox.send(JobResult::Failed {
                            reason: format!("the saved file would not reopen: {e}"),
                        });
                    }
                }
                self.session = None;
            }
            Err(e) => {
                let _ = outbox.send(JobResult::Failed {
                    reason: format!("the saved file would not reopen: {e}"),
                });
            }
        }
    }
}

/// Owns the worker thread and the channel it answers on.
#[derive(Debug)]
pub struct Supervisor {
    jobs: Sender<Job>,
    results: Receiver<JobResult>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Supervisor {
    /// Start a worker.
    #[must_use]
    pub fn start() -> Self {
        Self::start_with(Vec::new())
    }

    /// Start a worker with replacement system font directories (`--font-dirs`).
    ///
    /// The directories reach page renders and edit-session fallbacks, so the
    /// window draws and embeds what the flag names. Empty is bundled-only.
    #[must_use]
    pub fn start_with(font_dirs: Vec<PathBuf>) -> Self {
        let (jobs, inbox) = channel();
        let (outbox, results) = channel();
        let worker = thread::Builder::new()
            .name("mangle-document".to_string())
            .spawn(move || run(inbox, outbox, font_dirs))
            .ok();
        Self {
            jobs,
            results,
            worker,
        }
    }

    /// Ask for something.
    pub fn ask(&self, job: Job) {
        // A worker that has gone away is not a reason to take the window down with it: the job is
        // dropped, and the notice that was already showing is still showing.
        let _ = self.jobs.send(job);
    }

    /// Whatever has come back since the last call, oldest first.
    pub fn drain(&self) -> Vec<JobResult> {
        self.results.try_iter().collect()
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.ask(Job::Close);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The worker's own loop: one document at a time, answered in order.
fn run(inbox: Receiver<Job>, outbox: Sender<JobResult>, font_dirs: Vec<PathBuf>) {
    let mut open: Option<Worker> = None;
    while let Ok(job) = inbox.recv() {
        match job {
            // A close is answered by leaving: the thread ends and the channel goes with it.
            Job::Close => return,
            Job::Open(path) => {
                open = None;
                match Worker::open(&path, font_dirs.clone()) {
                    Ok(worker) => {
                        let _ = outbox.send(JobResult::Opened {
                            name: path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
                            pages: worker.pages.len(),
                            bookmarks: Vec::new(),
                        });
                        // The first page straight away, so the window is never blank on a file it
                        // did open.
                        worker.page(&outbox, 0, 1.0);
                        open = Some(worker);
                    }
                    Err(e) => {
                        let _ = outbox.send(JobResult::Failed { reason: e });
                    }
                }
            }
            Job::Page(index, scale) => {
                let Some(worker) = &open else {
                    let _ = outbox.send(JobResult::Failed {
                        reason: "no document is open".to_string(),
                    });
                    continue;
                };
                worker.page(&outbox, index, scale);
            }
            Job::Edit {
                page,
                object,
                request,
            } => {
                let Some(worker) = &mut open else {
                    let _ = outbox.send(JobResult::Failed {
                        reason: "no document is open".to_string(),
                    });
                    continue;
                };
                worker.edit(&outbox, page, object, &request);
            }
            Job::Arrange {
                page,
                object,
                arrange,
            } => {
                let Some(worker) = &mut open else {
                    let _ = outbox.send(JobResult::Failed {
                        reason: "no document is open".to_string(),
                    });
                    continue;
                };
                worker.arrange(&outbox, page, object, arrange);
            }
            Job::Undo(page) => {
                let Some(worker) = &mut open else {
                    let _ = outbox.send(JobResult::Failed {
                        reason: "no document is open".to_string(),
                    });
                    continue;
                };
                worker.step(&outbox, page, true);
            }
            Job::Redo(page) => {
                let Some(worker) = &mut open else {
                    let _ = outbox.send(JobResult::Failed {
                        reason: "no document is open".to_string(),
                    });
                    continue;
                };
                worker.step(&outbox, page, false);
            }
        }
    }
}

/// The page tree root, or the first page in the file when the catalogue is missing.
fn root_of(doc: &Document) -> Ref {
    doc.catalog()
        .ok()
        .and_then(|catalog| catalog.get("Pages").and_then(|o| o.as_ref_id()))
        .or_else(|| doc.pages().ok().and_then(|p| p.first().map(|r| r.obj)))
        .unwrap_or_else(|| Ref::new(1, 0))
}

/// Parse `#RRGGBB` (with or without the `#`) into sRGB, `None` for anything else.
///
/// Seven characters would be an alpha the file's own spaces cannot hold uniformly,
/// and three would be a guess about doubling — so exactly six hex digits or refusal.
fn parse_hex_colour(hex: &str) -> Option<mangle_content::state::Rgba> {
    let digits = hex.strip_prefix('#').unwrap_or(hex);
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| {
        digits
            .get(at..at + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .map(|v| f64::from(v) / 255.0)
    };
    Some(mangle_content::state::Rgba {
        r: channel(0)?,
        g: channel(2)?,
        b: channel(4)?,
        a: 1.0,
    })
}

/// A page's resources, resolved the way the renderer needs them.
fn resources_of(doc: &Document, page: &mangle_doc::Page) -> Resources {
    page.inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for;
    // the panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Wait for the answers to arrive, rather than sleeping a fixed time.
    ///
    /// The worker is a **queue**, not a server: the page it renders on opening is answered before
    /// the job asked for afterwards, so a fixed sleep would be a race and a flaky test.
    fn collect(supervisor: &Supervisor, wanted: usize) -> Vec<JobResult> {
        let mut out = Vec::new();
        for _ in 0..400 {
            out.extend(supervisor.drain());
            if out.len() >= wanted {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
        out
    }

    #[test]
    fn only_six_hex_digits_are_a_colour() {
        let red = parse_hex_colour("#FF0000").expect("six digits with a hash");
        assert!((red.r - 1.0).abs() < 1e-9 && red.g == 0.0 && red.b == 0.0);
        assert!(parse_hex_colour("00ff00").is_some(), "no hash is fine too");
        for bad in ["", "#", "#FFF", "#FF0000FF", "red", "#GGGGGG", "# FF0000"] {
            assert!(
                parse_hex_colour(bad).is_none(),
                "{bad:?} must not parse as a colour"
            );
        }
    }

    #[test]
    fn a_worker_opens_a_corpus_file_and_answers_with_a_page() {
        let path = PathBuf::from("../../corpus/wild/gov__nist-sp800-88.pdf");
        if !path.exists() {
            eprintln!("skipped: the corpus file is not fetched");
            return;
        }
        let supervisor = Supervisor::start();
        supervisor.ask(Job::Open(path));
        let mut opened = None;
        let mut page = None;
        for result in collect(&supervisor, 2) {
            match result {
                JobResult::Opened { name, pages, .. } => opened = Some((name, pages)),
                JobResult::Page {
                    index,
                    width,
                    height,
                    objects,
                    ..
                } => page = Some((index, width, height, objects.len())),
                JobResult::Failed { reason } => panic!("the worker failed: {reason}"),
                JobResult::Saved { .. } => {}
            }
        }
        let (name, pages) = opened.expect("the file opened");
        assert!(
            PathBuf::from(&name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdf")),
            "{name}"
        );
        assert!(pages > 0, "and it has pages");
        let (index, width, height, objects) = page.expect("the first page came back");
        assert_eq!(index, 0);
        assert!(width > 0 && height > 0, "{width}x{height}");
        assert!(
            objects > 0,
            "and what is on it, so the window can hit-test without holding the document"
        );
    }

    /// A configured `--font-dirs` reaches the worker's renders.
    ///
    /// A one-page file naming an unembedded Helvetica is opened through a
    /// supervisor started with the repository's own bundled directory standing
    /// in as a system directory: the page's notes must name `--font-dirs` as
    /// the face's source rather than the bundled default.
    #[test]
    fn a_configured_font_dir_reaches_worker_renders() {
        let content = b"BT /F1 24 Tf 1 0 0 1 20 50 Tm (Hi) Tj ET";
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = vec![0usize];
        offsets.push(pdf.len());
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>\nendobj\n",
        );
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> \
              /Contents 4 0 R >>\nendobj\n",
        );
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
        );
        let startxref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{startxref}\n%%EOF\n")
                .as_bytes(),
        );

        let path =
            std::env::temp_dir().join(format!("mangle-font-dirs-{}.pdf", std::process::id()));
        std::fs::write(&path, &pdf).expect("writes the test file");
        let supervisor = Supervisor::start_with(vec![PathBuf::from("../../assets/fonts")]);
        supervisor.ask(Job::Open(path.clone()));
        let mut notes = None;
        for result in collect(&supervisor, 2) {
            match result {
                JobResult::Page { notes: n, .. } => notes = Some(n),
                JobResult::Failed { reason } => panic!("the worker failed: {reason}"),
                _ => {}
            }
        }
        std::fs::remove_file(&path).ok();
        let notes = notes.expect("a page came back");
        assert!(
            notes.iter().any(|note| note.contains("--font-dirs")),
            "the render used the configured directory: {notes:?}"
        );
    }

    /// An edit on a real page is saved and the saved file is re-opened and drawn.
    ///
    /// The re-open is the point: the window must not be able to show an edit the file does not
    /// have.
    #[test]
    fn an_edit_is_saved_and_the_saved_file_redrawn() {
        let path = PathBuf::from("../../corpus/wild/gov__nist-sp800-88.pdf");
        if !path.exists() {
            eprintln!("skipped: the corpus file is not fetched");
            return;
        }
        let supervisor = Supervisor::start();
        supervisor.ask(Job::Open(path.clone()));
        // Wait for the first page, so the object index below means something.
        let objects = collect(&supervisor, 2)
            .into_iter()
            .find_map(|r| match r {
                JobResult::Page { objects, .. } => Some(objects),
                _ => None,
            })
            .expect("a page");
        assert!(!objects.is_empty(), "the page has something to move");

        supervisor.ask(Job::Edit {
            page: 0,
            object: 0,
            request: EditRequest::Move { dx: 9.0, dy: 0.0 },
        });
        let mut saved = None;
        let mut redrawn = None;
        let mut after = None;
        for result in collect(&supervisor, 3) {
            match result {
                JobResult::Saved { bytes, objects, .. } => saved = Some((bytes, objects)),
                JobResult::Page { index, objects, .. } => {
                    redrawn = Some(index);
                    after = Some(objects);
                }
                JobResult::Failed { reason } => panic!("the edit failed: {reason}"),
                JobResult::Opened { .. } => {}
            }
        }
        assert!(saved.is_some(), "the edit was saved");
        assert_eq!(redrawn, Some(0), "and the page was drawn again");
        // **And the object actually moved.** A test that only checked a page came back would
        // pass on an edit that silently did nothing, which is precisely the failure mode this
        // whole loop exists to prevent.
        //
        // The expected position is read through the object's own CTM, because a move is in the
        // stream's *user space* while the box is in *device* space. On this page the object is
        // drawn inside a `cm` scaled by about 106, so nine points there is nine hundred here —
        // and asserting "+9" would have "proved" the edit wrong when it was right.
        let after = after.expect("the page came back with what is on it");
        let before_bounds = objects.first().map(|o| o.bounds).expect("a page");
        let after_bounds = after.first().map(|o| o.bounds).expect("still there");
        let travelled = after_bounds.x0 - before_bounds.x0;
        assert!(
            travelled > 1.0,
            "the move is in user space and this page scales it up: 9 points became {travelled}"
        );
        assert!(
            (after_bounds.y0 - before_bounds.y0).abs() < 1e-6
                && ((after_bounds.x1 - after_bounds.x0) - (before_bounds.x1 - before_bounds.x0))
                    .abs()
                    < 1e-6,
            "and it moved straight across, without changing size: {before_bounds:?} -> \
             {after_bounds:?}"
        );
    }

    /// A retype with a missing glyph reports its substitution through the save notice.
    ///
    /// The window path without a window: the same `Job::Edit` the panel sends, driven
    /// here, asserting the `Saved` answer carries the fallback notice rather than only
    /// the byte counts. `α` is not in the WinAnsi fonts this page sets, so a run that
    /// takes it falls back to an embedded Liberation subset.
    #[test]
    fn a_retype_with_a_missing_glyph_reports_its_substitution() {
        let path = PathBuf::from("../../corpus/wild/gov__nist-sp800-88.pdf");
        if !path.exists() {
            eprintln!("skipped: the corpus file is not fetched");
            return;
        }
        let supervisor = Supervisor::start();
        supervisor.ask(Job::Open(path));
        let mut objects = collect(&supervisor, 2)
            .into_iter()
            .find_map(|r| match r {
                JobResult::Page { objects, .. } => Some(objects),
                _ => None,
            })
            .expect("a page");
        // An own-font success rewrites the text the later attempts read, so the scan
        // restarts from the redrawn page — bounded, because such a success is rare and
        // each restart re-tries only fast refusals before it.
        let mut restarts = 0usize;
        loop {
            let mut progressed = false;
            for (index, summary) in objects.iter().enumerate() {
                if !matches!(
                    summary.kind,
                    mangle_edit::Kind::Line
                        | mangle_edit::Kind::Block
                        | mangle_edit::Kind::LooseRun
                ) {
                    continue;
                }
                let Some(content) = summary.content.as_deref() else {
                    continue;
                };
                let Some(position) = content
                    .char_indices()
                    .rev()
                    .find(|(_, c)| c.is_ascii_alphanumeric())
                else {
                    continue;
                };
                let mut text = content.to_string();
                text.replace_range(position.0..position.0 + position.1.len_utf8(), "α");
                supervisor.ask(Job::Edit {
                    page: 0,
                    object: index,
                    request: EditRequest::Retype { text },
                });
                let mut answers = collect(&supervisor, 1);
                assert!(
                    !answers.is_empty(),
                    "the worker went quiet on object {index}"
                );
                match answers.remove(0) {
                    JobResult::Saved { notice, .. } => match notice {
                        Some(notice) => {
                            assert!(
                                notice.contains('α') && notice.contains("Liberation"),
                                "the notice names the character and the face: {notice}"
                            );
                            return;
                        }
                        None => {
                            // An own-font success: the page changed under the scan, so
                            // re-read it rather than reasoning about stale indices.
                            restarts += 1;
                            assert!(restarts <= 3, "too many own-font successes to scan past");
                            objects = collect(&supervisor, 1)
                                .into_iter()
                                .find_map(|r| match r {
                                    JobResult::Page { objects, .. } => Some(objects),
                                    _ => None,
                                })
                                .expect("the redrawn page");
                            progressed = true;
                            break;
                        }
                    },
                    JobResult::Failed { .. } => {}
                    other => panic!("an edit answers Saved or Failed, not {other:?}"),
                }
            }
            assert!(progressed, "no run fell back on this page");
        }
    }

    /// A stepper's edit reaches the file, which is the only place it counts.
    ///
    /// The worker re-opens what it just saved and redraws *that*, so the read-out a window shows
    /// after a click is the document's rather than a model's — and this walks the whole way
    /// round: edit, save, re-open, re-summarise. An edit that stopped in memory would leave the
    /// window drawing something the file does not have, and a test that stopped in memory would
    /// call that working.
    #[test]
    fn a_text_edit_reaches_the_file_and_comes_back_in_the_readout() {
        let path = PathBuf::from("../../corpus/wild/gov__irs-f1040.pdf");
        if !path.exists() {
            eprintln!("skipped: the corpus file is not fetched");
            return;
        }
        let supervisor = Supervisor::start();
        supervisor.ask(Job::Open(path));
        let objects = collect(&supervisor, 2)
            .into_iter()
            .find_map(|r| match r {
                JobResult::Page { objects, .. } => Some(objects),
                _ => None,
            })
            .expect("a page");
        // A run whose size the page actually states. Some runs inherit theirs from an enclosing
        // form and the record carries `0`, which is the honest answer for "not set here" — and
        // exactly the kind of object a stepper must not be offered, because there is no number
        // to nudge. `panel_row` already shows those rows empty.
        let (index, before) = objects
            .iter()
            .enumerate()
            .find_map(|(i, o)| {
                o.text
                    .as_ref()
                    .filter(|t| t.size > 0.0)
                    .map(|t| (i, t.size))
            })
            .expect("a page with text drawn at a stated size");

        // The size to set has to be one the page does not already draw at, or "some object is now
        // drawn at this size" would be true before the edit as well and would prove nothing. The
        // search is bounded because a page cannot draw at two hundred different sizes and still
        // have a run left to look at.
        let target = (1..200u32)
            .map(|k| before + f64::from(k))
            .find(|t| {
                !objects
                    .iter()
                    .any(|o| o.text.as_ref().is_some_and(|s| (s.size - t).abs() < 1e-6))
            })
            .expect("a size this page does not already draw at");

        supervisor.ask(Job::Edit {
            page: 0,
            object: index,
            request: EditRequest::Text(mangle_edit::TextProperty::Size(target)),
        });
        let mut after = None;
        for result in collect(&supervisor, 4) {
            match result {
                JobResult::Page { objects, .. } => after = Some(objects),
                JobResult::Failed { reason } => panic!("the text edit failed: {reason}"),
                _ => {}
            }
        }
        let after = after.expect("the page came back");
        // Matched by size rather than by index: a bigger size makes a taller box, the box
        // changes how the run groups, and which index the run ends up at is the grouping's
        // business. What must not change is that the page still draws it, at the new size.
        let found = after
            .iter()
            .filter(|o| {
                o.text
                    .as_ref()
                    .is_some_and(|t| (t.size - target).abs() < 1e-6)
            })
            .count();
        assert!(
            found > 0,
            "the page now draws a run at {target}, which it did not before the edit"
        );
    }

    /// A stepper's click reaches the file, by the same arithmetic the panel does.
    ///
    /// This is the whole gesture, assembled the way `panel::stepped` assembles it: read the
    /// object's own text state, add the step, ask the worker to write the absolute value. The
    /// value that goes in the request is computed by the same function the panel calls, so this
    /// test is the join between the two halves — a row that drew the wrong number and a worker
    /// that wrote the right one would pass the panel tests and the corpus tests alone.
    #[test]
    fn a_stepper_click_asks_for_the_value_the_panel_read_out() {
        let path = PathBuf::from("../../corpus/wild/gov__irs-f1040.pdf");
        if !path.exists() {
            eprintln!("skipped: the corpus file is not fetched");
            return;
        }
        let supervisor = Supervisor::start();
        supervisor.ask(Job::Open(path));
        let objects = collect(&supervisor, 2)
            .into_iter()
            .find_map(|r| match r {
                JobResult::Page { objects, .. } => Some(objects),
                _ => None,
            })
            .expect("a page");
        let (index, state) = objects
            .iter()
            .enumerate()
            .find_map(|(i, o)| o.text.as_ref().filter(|t| t.size > 0.0).map(|t| (i, t)))
            .expect("a run drawn at a stated size");

        // The size row: one click of "+", which is one point.
        let step = mangle_edit::TextProperty::Size(state.size);
        let asked = crate::panel::stepped(step, state, 1.0).expect("a size is a number");
        assert_eq!(
            asked,
            mangle_edit::TextProperty::Size(state.size + 1.0),
            "one click of the size stepper is one point"
        );
        // The line-spacing row: one click of "+", which is a tenth of a multiple of the size.
        let spacing = mangle_edit::TextProperty::Leading(state.leading);
        let widened = crate::panel::stepped(spacing, state, 0.1).expect("a leading is a number");
        // A tenth of a multiple of the size, which is a tenth of the size in points.
        assert!(
            match widened {
                mangle_edit::TextProperty::Leading(points) => {
                    (points - (state.leading + state.size * 0.1)).abs() < 1e-6
                }
                other => panic!("a line spacing writes a leading, was {other:?}"),
            },
            "a tenth of a multiple of {} is {} points, was {widened:?}",
            state.size,
            state.size * 0.1
        );

        // And the whole thing through the worker, so the join is real and not just arithmetic.
        let target = state.size + 1.0;
        supervisor.ask(Job::Edit {
            page: 0,
            object: index,
            request: EditRequest::Text(asked),
        });
        let mut after = None;
        for result in collect(&supervisor, 4) {
            match result {
                JobResult::Page { objects, .. } => after = Some(objects),
                JobResult::Failed { reason } => panic!("the stepper edit failed: {reason}"),
                _ => {}
            }
        }
        let after = after.expect("the page came back");
        assert!(
            after.iter().any(|o| o
                .text
                .as_ref()
                .is_some_and(|t| (t.size - target).abs() < 1e-6)),
            "the page now draws a run at {target}, which it did not before the click"
        );
    }

    /// An arrange reaches the file, on the first thing asked of a page.
    ///
    /// The session is opened on demand, so this is also the check that an arrange does not need
    /// something to have been edited first: it is often the *first* thing a user asks of a page,
    /// and a worker that refused until then would refuse the most ordinary request it gets.
    ///
    /// What is asserted is that the object still exists and the page still has the same number of
    /// objects — an arrange moves bytes, it does not remove anything, so a lost object would mean
    /// the move had dropped it. The order itself is verified by `mangle-edit`'s own arrange tests,
    /// which assert the operator positions directly.
    #[test]
    fn an_arrange_reaches_the_file_without_anything_edited_first() {
        let path = PathBuf::from("../../corpus/wild/gov__nist-sp800-88.pdf");
        if !path.exists() {
            eprintln!("skipped: the corpus file is not fetched");
            return;
        }
        let supervisor = Supervisor::start();
        supervisor.ask(Job::Open(path));
        let before = collect(&supervisor, 2)
            .into_iter()
            .find_map(|r| match r {
                JobResult::Page { objects, .. } => Some(objects),
                _ => None,
            })
            .expect("a page");
        assert!(!before.is_empty(), "the page has something on it");
        let last = before.len() - 1;

        supervisor.ask(Job::Arrange {
            page: 0,
            object: last,
            arrange: mangle_edit::Arrange::ToFront,
        });
        let mut saved = None;
        let mut after = None;
        for result in collect(&supervisor, 4) {
            match result {
                JobResult::Saved { bytes, objects, .. } => saved = Some((bytes, objects)),
                JobResult::Page { objects, .. } => after = Some(objects),
                JobResult::Failed { reason } => panic!("the arrange failed: {reason}"),
                JobResult::Opened { .. } => {}
            }
        }
        assert!(saved.is_some(), "the arrange was saved");
        let after = after.expect("the page was drawn again");
        assert_eq!(
            after.len(),
            before.len(),
            "an arrange moves bytes, it does not remove anything"
        );
    }

    /// A rotation and a delete both reach the file, on a page nobody has edited.
    ///
    /// Same shape as the stepper and arrange checks: the values are the ones the panel computes,
    /// and what is asserted is that the page comes back without the object, or with it turned.
    /// The order of the operators is `mangle-edit`'s own business and is asserted there.
    #[test]
    fn a_rotation_and_a_delete_reach_the_file() {
        let path = PathBuf::from("../../corpus/wild/gov__nist-sp800-88.pdf");
        if !path.exists() {
            eprintln!("skipped: the corpus file is not fetched");
            return;
        }
        let supervisor = Supervisor::start();
        supervisor.ask(Job::Open(path));
        let before = collect(&supervisor, 2)
            .into_iter()
            .find_map(|r| match r {
                JobResult::Page { objects, .. } => Some(objects),
                _ => None,
            })
            .expect("a page");
        let Some(last) = before.len().checked_sub(1) else {
            panic!("the page has something on it");
        };
        let Some(bounds) = before.get(last).map(|o| o.bounds) else {
            panic!("the page has an object at {last}");
        };
        // About the object's own centre, in page space — the same arithmetic the panel does,
        // which is the point: this checks the join, not a second opinion about where the middle
        // of a box is.
        let about = (
            f64::midpoint(bounds.x0, bounds.x1),
            f64::midpoint(bounds.y0, bounds.y1),
        );

        supervisor.ask(Job::Edit {
            page: 0,
            object: last,
            request: EditRequest::Rotate {
                degrees: 90.0,
                about,
            },
        });
        let mut turned = None;
        for result in collect(&supervisor, 4) {
            match result {
                JobResult::Page { objects, .. } => turned = Some(objects),
                JobResult::Failed { reason } => panic!("the rotation failed: {reason}"),
                _ => {}
            }
        }
        let turned = turned.expect("the page came back");
        assert_eq!(
            turned.len(),
            before.len(),
            "a rotation turns an object, it does not remove one"
        );

        // And the delete takes it away, which is the one edit with an obvious result.
        supervisor.ask(Job::Edit {
            page: 0,
            object: last,
            request: EditRequest::Delete,
        });
        let mut after = None;
        for result in collect(&supervisor, 4) {
            match result {
                JobResult::Page { objects, .. } => after = Some(objects),
                JobResult::Failed { reason } => panic!("the delete failed: {reason}"),
                _ => {}
            }
        }
        assert_eq!(
            after.expect("the page came back").len(),
            before.len() - 1,
            "the deleted object is gone"
        );
    }

    /// An object the page does not have is answered with a reason rather than a panic.
    #[test]
    fn an_object_that_is_not_on_the_page_is_answered_with_a_reason() {
        let path = PathBuf::from("../../corpus/wild/gov__nist-sp800-88.pdf");
        if !path.exists() {
            eprintln!("skipped: the corpus file is not fetched");
            return;
        }
        let supervisor = Supervisor::start();
        supervisor.ask(Job::Open(path));
        collect(&supervisor, 2);
        supervisor.ask(Job::Edit {
            page: 0,
            object: 9999,
            request: EditRequest::Delete,
        });
        let reasons: Vec<String> = collect(&supervisor, 2)
            .into_iter()
            .filter_map(|r| match r {
                JobResult::Failed { reason } => Some(reason),
                _ => None,
            })
            .collect();
        assert!(
            reasons.iter().any(|r| !r.is_empty()),
            "the index the window holds can be from before another file was opened, and the \\
             reason is what the notice shows: {reasons:?}"
        );
    }
}

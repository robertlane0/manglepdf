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
        /// Why anything on it was not drawn.
        notes: Vec<String>,
    },
    /// An edit was saved, and the saved file re-opened.
    Saved {
        /// How many bytes the saved file is.
        bytes: usize,
        /// How many objects the revision rewrote.
        objects: usize,
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
}

impl Worker {
    /// Open `path` and read everything a job needs from it.
    fn open(path: &PathBuf) -> Result<Self, String> {
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
            objects: model.objects().iter().map(mangle_edit::summarise).collect(),
            notes: render.notes,
        });
    }

    /// Apply an edit, save it, and draw the file as saved.
    fn edit(
        &mut self,
        outbox: &Sender<JobResult>,
        page: usize,
        object: usize,
        request: &EditRequest,
    ) {
        // The session is rebuilt when the page changes, so a second edit on the same page keeps
        // the first one's history.
        if self.session.as_ref().is_none_or(|e| e.page().index != page) {
            let (Some(page_dict), Some(page_resources)) =
                (self.pages.get(page), self.resources.get(page))
            else {
                let _ = outbox.send(JobResult::Failed {
                    reason: format!("this document has no page {}", page + 1),
                });
                return;
            };
            match mangle_edit::Editor::open(Arc::clone(&self.doc), page_dict, page_resources) {
                Ok(editor) => self.session = Some(editor),
                Err(e) => {
                    let _ = outbox.send(JobResult::Failed {
                        reason: e.to_string(),
                    });
                    return;
                }
            }
        }
        let Some(editor) = self.session.as_mut() else {
            return;
        };
        let change = match request {
            EditRequest::Move { dx, dy } => mangle_edit::Change::move_by(*dx, *dy),
            EditRequest::Delete => mangle_edit::Change::Delete,
        };
        if let Err(e) = editor.apply(object, &change) {
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
        let (jobs, inbox) = channel();
        let (outbox, results) = channel();
        let worker = thread::Builder::new()
            .name("mangle-document".to_string())
            .spawn(move || run(inbox, outbox))
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
fn run(inbox: Receiver<Job>, outbox: Sender<JobResult>) {
    let mut open: Option<Worker> = None;
    while let Ok(job) = inbox.recv() {
        match job {
            // A close is answered by leaving: the thread ends and the channel goes with it.
            Job::Close => return,
            Job::Open(path) => {
                open = None;
                match Worker::open(&path) {
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
        for result in collect(&supervisor, 3) {
            match result {
                JobResult::Saved { bytes, objects } => saved = Some((bytes, objects)),
                JobResult::Page { index, .. } => redrawn = Some(index),
                JobResult::Failed { reason } => panic!("the edit failed: {reason}"),
                JobResult::Opened { .. } => {}
            }
        }
        assert!(saved.is_some(), "the edit was saved");
        assert_eq!(redrawn, Some(0), "and the page was drawn again");
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

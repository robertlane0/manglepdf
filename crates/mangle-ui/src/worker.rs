//! The worker that opens a document and rasterizes its pages.
//!
//! # Why the UI thread does none of this
//!
//! The charter's hardest rule for this crate is that the UI thread never parses, decodes or
//! rasterizes. It is not a performance rule: `render_page` on a large page is hundreds of
//! milliseconds, and a window that stops answering for that long is a window the user has to kill.
//! So the work happens on a thread and arrives here as a message, and the UI thread's only job is
//! to turn pixels into a texture.
//!
//! # The shape of the message
//!
//! A worker sends [`JobResult`]s; a [`Supervisor`] owns the channel and the worker's handle. The
//! result carries the **pixels** and the page's point size, because the canvas places a page from
//! its `/MediaBox` and draws from its buffer, and nothing else about the page is the UI's to know.
//!
//! A document is opened once and kept in the worker, so the pages that follow the first are a
//! message round trip rather than a re-parse. That matters: the corpus's biggest file takes seconds
//! to open, and a thumbnail strip that re-parsed it per thumbnail would be a thumbnail strip nobody
//! waits for.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use mangle_content::Resources;
use mangle_doc::PageTree;
use mangle_render::page::{PageRender, RenderOptions, render_page};
use mangle_syntax::OpenOptions;
use mangle_syntax::document::Document;

/// What to do, as a message.
#[derive(Debug, Clone)]
pub enum Job {
    /// Open a file. The first page is rendered as soon as it is open.
    Open(PathBuf),
    /// Render the page at this index, at this many pixels per point.
    Page(usize, f64),
    /// Close whatever is open.
    Close,
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
        /// Why anything on it was not drawn.
        notes: Vec<String>,
    },
    /// The file could not be opened.
    Failed {
        /// What went wrong, in a sentence a user can read.
        reason: String,
    },
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
        // A worker that has gone away is not a reason to take the window down with it: the job
        // is dropped and the next one is tried, and the notice that was already showing is still
        // showing.
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
///
/// A document is held across jobs, which is what makes the second and later pages cheap. A job that
/// names a page the open document does not have is answered with the reason rather than a panic:
/// the index the UI holds can be from before the user opened something else.
fn run(inbox: Receiver<Job>, outbox: Sender<JobResult>) {
    let mut open: Option<(PathBuf, Document, PageTree, Vec<Resources>)> = None;
    while let Ok(job) = inbox.recv() {
        match job {
            Job::Close => {
                // A close is answered by leaving: the thread ends and the channel goes with it.
                return;
            }
            Job::Open(path) => {
                match Document::open(
                    std::fs::read(&path).unwrap_or_default(),
                    OpenOptions::default(),
                ) {
                    Ok(doc) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.to_string_lossy().into_owned());
                        let mut resources = Vec::new();
                        let pages = match PageTree::build(&doc, root_of(&doc)) {
                            Ok(tree) => tree,
                            Err(e) => {
                                let _ = outbox.send(JobResult::Failed {
                                    reason: e.to_string(),
                                });
                                continue;
                            }
                        };
                        for page in pages.pages() {
                            resources.push(resources_of(&doc, page));
                        }
                        let _ = outbox.send(JobResult::Opened {
                            name,
                            pages: pages.len(),
                            bookmarks: Vec::new(),
                        });
                        open = Some((path, doc, pages, resources));
                        // The first page straight away, so the window is never blank on a file it
                        // did open.
                        if let Some((_, doc, pages, resources)) = &open {
                            send_page(&outbox, doc, pages, resources, 0, 1.0);
                        }
                    }
                    Err(e) => {
                        let _ = outbox.send(JobResult::Failed {
                            reason: e.to_string(),
                        });
                    }
                }
            }
            Job::Page(index, scale) => {
                let Some((_, doc, pages, resources)) = &open else {
                    let _ = outbox.send(JobResult::Failed {
                        reason: "no document is open".to_string(),
                    });
                    continue;
                };
                send_page(&outbox, doc, pages, resources, index, scale);
            }
        }
    }
}

/// Send one page, or the reason it could not be drawn.
fn send_page(
    outbox: &Sender<JobResult>,
    doc: &Document,
    pages: &PageTree,
    resources: &[Resources],
    index: usize,
    scale: f64,
) {
    let Some(page) = pages.get(index) else {
        let _ = outbox.send(JobResult::Failed {
            reason: format!("this document has no page {}", index + 1),
        });
        return;
    };
    let Some(resources) = resources.get(index) else {
        let _ = outbox.send(JobResult::Failed {
            reason: "the page's resources could not be read".to_string(),
        });
        return;
    };
    let render: PageRender = render_page(
        doc,
        page,
        resources,
        RenderOptions {
            scale,
            ..RenderOptions::default()
        },
    );
    let _ = outbox.send(JobResult::Page {
        index,
        rgba: render.image.pixels.clone(),
        width: render.image.width,
        height: render.image.height,
        points: page.inherited.displayed_size(),
        notes: render.notes,
    });
}

/// The page tree root, or the first object in the file when the catalogue is missing.
fn root_of(doc: &Document) -> mangle_syntax::object::Ref {
    doc.catalog()
        .ok()
        .and_then(|catalog| catalog.get("Pages").and_then(|o| o.as_ref_id()))
        .or_else(|| doc.pages().ok().and_then(|p| p.first().map(|r| r.obj)))
        .unwrap_or_else(|| mangle_syntax::object::Ref::new(1, 0))
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
        for _ in 0..200 {
            for result in supervisor.drain() {
                match result {
                    JobResult::Opened { name, pages, .. } => opened = Some((name, pages)),
                    JobResult::Page {
                        index,
                        width,
                        height,
                        ..
                    } => {
                        page = Some((index, width, height));
                    }
                    JobResult::Failed { reason } => panic!("the worker failed: {reason}"),
                }
            }
            if opened.is_some() && page.is_some() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
        let (name, pages) = opened.expect("the file opened");
        assert!(
            PathBuf::from(&name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdf")),
            "{name}"
        );
        assert!(pages > 0, "and it has pages");
        let (index, width, height) = page.expect("the first page came back");
        assert_eq!(index, 0);
        assert!(width > 0 && height > 0, "{width}x{height}");
    }

    #[test]
    fn a_page_the_document_does_not_have_is_answered_with_a_reason() {
        let path = PathBuf::from("../../corpus/wild/gov__nist-sp800-88.pdf");
        if !path.exists() {
            eprintln!("skipped: the corpus file is not fetched");
            return;
        }
        let supervisor = Supervisor::start();
        supervisor.ask(Job::Open(path));
        // The worker is a **queue**, not a server: the page it renders on opening is answered
        // before the job asked for afterwards, so a fixed sleep would be a race. Poll until the
        // reason arrives, and give up rather than hang: a test that hangs the suite is worse
        // than one that fails.
        supervisor.ask(Job::Page(9999, 1.0));
        let mut reason = None;
        for _ in 0..400 {
            for result in supervisor.drain() {
                if let JobResult::Failed { reason: why } = result
                    && why.contains("no page 10000")
                {
                    reason = Some(why);
                }
            }
            if reason.is_some() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            reason.is_some(),
            "the index the window holds can be from before another file was opened, and the \
             reason is what the notice shows"
        );
    }
}

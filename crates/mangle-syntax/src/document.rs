//! The document: lazy object access over a file's bytes, with a copy-on-write overlay.
//!
//! This is the type the whole product is built on. It never re-serialises anything it did
//! not change, and every edit is recorded in the overlay so an incremental save can find
//! exactly the objects that differ.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use crate::decrypt::decrypt_object;
use crate::error::{Error, Result};
use crate::object::{Dict, Name, Object, Ref, Stream};
use crate::parser::Parser;
use crate::recovery::Recovery;
use crate::stream::decode_stream;
use crate::xref::{Xref, XrefEntry, find_root};

/// What happened while opening, for the UI and for save decisions.
#[derive(Debug, Clone)]
pub struct DocumentInfo {
    /// How the structure was established.
    pub recovery: Recovery,
    /// The document's `/ID` first element.
    pub file_id: Vec<u8>,
    /// `/Size` from the trailer.
    pub size: u32,
    /// The version in the header, e.g. `1.7`.
    pub version: String,
    /// Encryption state.
    pub encryption: EncryptionState,
}

/// Whether and how the document is encrypted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EncryptionState {
    pub encrypted: bool,
    /// `/Filter`, e.g. `Standard`.
    pub filter: Option<String>,
    /// `/R`.
    pub revision: i32,
    /// What the user may do, once decrypted.
    pub restricted: bool,
}

/// How to open a document.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// Password for the user or owner role.
    pub password: Option<Vec<u8>>,
    /// Skip decryption entirely (for the Inspector and for measuring).
    pub ignore_encryption: bool,
    /// Guard against pathological files; 0 means the default.
    pub object_budget: usize,
}

/// A reference to a page, resolved through the page tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRef {
    /// The page dictionary's object number.
    pub obj: Ref,
    /// Position in the flattened page order, from zero.
    pub index: usize,
}

/// The default object budget for a hostile file.
const DEFAULT_BUDGET: usize = 8_000_000;
/// The deepest page tree we will walk.
const MAX_PAGE_TREE_DEPTH: usize = 64;
/// The most pages we will enumerate.
const MAX_PAGES: usize = 1_000_000;

/// A document: the file bytes, the cross-reference, and a copy-on-write overlay.
#[derive(Debug)]
pub struct Document {
    bytes: Arc<Vec<u8>>,
    xref: RwLock<Arc<Xref>>,
    /// Objects that have been changed or created. This is what gets written.
    overlay: RwLock<BTreeMap<Ref, Object>>,
    /// Decoded objects, so repeated reads are cheap.
    cache: RwLock<BTreeMap<Ref, Arc<Object>>>,
    decryptor: RwLock<Option<mangle_crypto::Decryptor>>,
    /// Objects removed from the graph.
    removed: RwLock<std::collections::BTreeSet<Ref>>,
    info: DocumentInfo,
    budget: std::sync::atomic::AtomicUsize,
    budget_limit: usize,
}

impl Document {
    /// Open a document from memory.
    pub fn open(data: Vec<u8>, options: OpenOptions) -> Result<Self> {
        let limit = if options.object_budget == 0 {
            DEFAULT_BUDGET
        } else {
            options.object_budget
        };
        let (xref, recovery) = crate::recovery::read_or_repair(&data)?;
        let version = header_version(&data);
        let file_id = xref.id0();
        let size = xref.size();

        let mut encryption = EncryptionState::default();
        let mut decryptor = None;
        if let Some(eobj) = xref.trailer().get("Encrypt").and_then(Object::as_ref_id) {
            encryption.encrypted = true;
            let (state, dec) = open_encryption(&data, &xref, eobj, &options, &file_id)?;
            encryption = state;
            decryptor = dec;
        }

        let doc = Self {
            bytes: Arc::new(data),
            xref: RwLock::new(Arc::new(xref)),
            overlay: RwLock::new(BTreeMap::new()),
            cache: RwLock::new(BTreeMap::new()),
            decryptor: RwLock::new(decryptor),
            removed: RwLock::new(std::collections::BTreeSet::new()),
            info: DocumentInfo {
                recovery,
                file_id,
                size,
                version,
                encryption,
            },
            budget: std::sync::atomic::AtomicUsize::new(0),
            budget_limit: limit,
        };
        Ok(doc)
    }

    /// The file bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// What happened when the file was opened.
    #[must_use]
    pub fn info(&self) -> &DocumentInfo {
        &self.info
    }

    /// The cross-reference state, shared without copying.
    #[must_use]
    pub fn xref(&self) -> Arc<Xref> {
        match self.xref.read() {
            Ok(g) => Arc::clone(&g),
            Err(p) => Arc::clone(p.get_ref()),
        }
    }

    /// A snapshot of the cross-reference, for callers that need to read it.
    pub fn with_xref<T>(&self, f: impl FnOnce(&Xref) -> T) -> T {
        match self.xref.read() {
            Ok(g) => f(&g),
            Err(p) => f(p.get_ref()),
        }
    }

    /// The document catalogue, if it can be found.
    #[must_use]
    pub fn catalog_ref(&self) -> Option<Ref> {
        let from_trailer = self.with_xref(|x| x.trailer().get("Root").and_then(Object::as_ref_id));
        if let Some(r) = from_trailer
            && self.object(r).is_some()
        {
            return Some(r);
        }
        let bytes = self.bytes().to_vec();
        self.with_xref(|x| find_root(&bytes, x))
    }

    /// The document catalogue.
    pub fn catalog(&self) -> Result<Object> {
        let r = self
            .catalog_ref()
            .ok_or(Error::Structure("no catalogue".into()))?;
        self.object(r)
            .ok_or(Error::Structure("the catalogue is missing".into()))
    }

    /// Fetch an object, following the overlay, then the cache, then the file.
    pub fn object(&self, r: Ref) -> Option<Object> {
        if self.removed.read().ok()?.contains(&r) {
            return None;
        }
        if let Some(o) = self.overlay.read().ok()?.get(&r) {
            return Some(o.clone());
        }
        if let Some(c) = self.cache.read().ok()?.get(&r) {
            return Some((**c).clone());
        }
        let obj = self.load_from_file(r)?;
        if let Ok(mut c) = self.cache.write() {
            c.insert(r, Arc::new(obj.clone()));
        }
        Some(obj)
    }

    /// Fetch a shared object, avoiding a copy on repeated reads.
    #[must_use]
    pub fn shared(&self, r: Ref) -> Option<Arc<Object>> {
        if self.removed.read().ok()?.contains(&r) {
            return None;
        }
        if let Some(o) = self.overlay.read().ok()?.get(&r) {
            return Some(Arc::new(o.clone()));
        }
        if let Ok(c) = self.cache.read()
            && let Some(a) = c.get(&r)
        {
            return Some(a.clone());
        }
        let obj = self.load_from_file(r)?;
        let arc = Arc::new(obj);
        if let Ok(mut c) = self.cache.write() {
            c.insert(r, arc.clone());
        }
        Some(arc)
    }

    fn spend(&self) -> bool {
        let used = self
            .budget
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        used < self.budget_limit
    }

    fn load_from_file(&self, r: Ref) -> Option<Object> {
        if !self.spend() {
            return None;
        }
        let entry = self.with_xref(|x| x.get(r.num))?;
        match entry {
            XrefEntry::InFile { offset, .. } => self.parse_at(offset, r),
            XrefEntry::InStream { stream, index } => {
                let container = self.object(Ref::new(stream, 0))?;
                let Object::Stream(s) = container else {
                    return None;
                };
                crate::objstm::ObjectStream::from_stream(&s).object_at(index)
            }
            XrefEntry::Free { .. } | XrefEntry::Missing => None,
        }
    }

    fn parse_at(&self, offset: usize, r: Ref) -> Option<Object> {
        let tail = self.bytes.get(offset..)?;
        // `offset` should point at `N G obj`; be tolerant and search a little. Offsets
        // are converted back to absolute ones so the parser always sees the whole file.
        let start = (0..tail.len().min(4096))
            .find(|i| crate::recovery::looks_like_object(tail, *i))
            .unwrap_or(0);
        let mut p = Parser::at(self.bytes(), offset + start);
        let _num = p.next_object().ok().flatten();
        let _gen = p.next_object().ok().flatten();
        if !p.next_keyword(b"obj") {
            return None;
        }
        let dict_start = p.position();
        let first = p.next_object().ok().flatten()?;
        let mut obj = match first {
            Object::Dict(d) => {
                // A stream may follow the dictionary.
                let has_stream = p.next_keyword(b"stream");
                if has_stream {
                    let len = d.get("Length").and_then(|o| self.resolve_length(o));
                    match Parser::parse_stream(self.bytes(), dict_start, len) {
                        Ok((sd, raw, _)) => Object::Stream(Stream {
                            dict: sd,
                            raw,
                            file_offset: Some(offset),
                            synthetic: false,
                        }),
                        // The keyword was there but the body is unreadable; keep the
                        // dictionary so the rest of the document still resolves.
                        Err(_) => Object::Dict(d),
                    }
                } else {
                    Object::Dict(d)
                }
            }
            other => other,
        };
        if let Some(d) = self.decryptor.read().ok()?.as_ref() {
            decrypt_object(d, r.num, r.generation, &mut obj, 0);
        }
        Some(obj)
    }

    /// `/Length` may be an indirect reference; resolve one level, no deeper.
    fn resolve_length(&self, obj: &Object) -> Option<i64> {
        match obj {
            Object::Ref(r) if r.generation == 0 => {
                let entry = self.with_xref(|x| x.get(r.num))?;
                if !matches!(entry, XrefEntry::InFile { .. }) {
                    return None;
                }
                self.object(*r).and_then(|o| o.as_i64())
            }
            other => other.as_i64(),
        }
    }

    /// Replace an object. The change is recorded for the incremental writer.
    pub fn set(&self, r: Ref, obj: Object) {
        if let Ok(mut o) = self.overlay.write() {
            o.insert(r, obj);
        }
        if let Ok(mut c) = self.cache.write() {
            c.remove(&r);
        }
    }

    /// Remove an object from the graph.
    pub fn remove(&self, r: Ref) {
        if let Ok(mut o) = self.overlay.write() {
            o.remove(&r);
        }
        if let Ok(mut c) = self.cache.write() {
            c.remove(&r);
        }
        if let Ok(mut s) = self.removed.write() {
            s.insert(r);
        }
    }

    /// Allocate a fresh object number after the current `/Size`.
    pub fn alloc(&self) -> Ref {
        let num = self.with_xref(|x| x.size());
        if let Ok(mut x) = self.xref.write() {
            Arc::make_mut(&mut *x).set_trailer_key_size(num + 1);
        }
        Ref::new(num, 0)
    }

    /// Every object that has been changed or added, in order.
    #[must_use]
    pub fn overlay(&self) -> Vec<(Ref, Object)> {
        self.overlay
            .read()
            .map(|o| o.iter().map(|(k, v)| (*k, v.clone())).collect())
            .unwrap_or_default()
    }

    /// Whether anything has been changed.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.overlay.read().map(|o| !o.is_empty()).unwrap_or(false)
            || self.removed.read().map(|s| !s.is_empty()).unwrap_or(false)
    }

    /// The `/Info` dictionary, if any.
    pub fn info_dict(&self) -> Option<Object> {
        self.with_xref(|x| x.trailer.get("Info").and_then(Object::as_ref_id))
            .and_then(|r| self.object(r))
    }

    /// The document's `/ID`, needed by the incremental trailer.
    #[must_use]
    pub fn file_id(&self) -> Vec<u8> {
        self.info.file_id.clone()
    }

    /// The pages, in document order.
    pub fn pages(&self) -> Result<Vec<PageRef>> {
        let mut out = Vec::new();
        let Some(catalog) = self.catalog_ref() else {
            return Err(Error::Structure("no catalogue".into()));
        };
        let cat = self
            .object(catalog)
            .ok_or(Error::Structure("no catalogue".into()))?;
        let Some(pages) = cat.get("Pages").and_then(Object::as_ref_id) else {
            return Err(Error::Structure("the catalogue has no /Pages".into()));
        };
        let mut seen = std::collections::BTreeSet::new();
        self.walk_pages(pages, &mut out, &mut seen, 0, None)?;
        Ok(out)
    }

    /// How many pages the document claims, and how many we actually found.
    pub fn page_count(&self) -> Result<usize> {
        Ok(self.pages()?.len())
    }

    fn walk_pages(
        &self,
        node: Ref,
        out: &mut Vec<PageRef>,
        seen: &mut std::collections::BTreeSet<Ref>,
        depth: usize,
        inherited: Option<Dict>,
    ) -> Result<()> {
        if depth > MAX_PAGE_TREE_DEPTH {
            return Err(Error::Limit {
                kind: "page tree depth",
                limit: MAX_PAGE_TREE_DEPTH,
            });
        }
        if !seen.insert(node) {
            return Err(Error::Cycle("the page tree"));
        }
        let Some(obj) = self.object(node) else {
            return Ok(());
        };
        let Some(d) = obj.as_dict() else {
            return Ok(());
        };
        let merged = merge_inherited(d, inherited.as_ref());
        let kids = d.get("Kids").and_then(Object::as_array);
        let kind = d.get("Type").and_then(Object::as_name);
        match kids {
            Some(kids) if kind != Some(&b"Page"[..]) => {
                for kid in kids {
                    let Some(r) = kid.as_ref_id() else { continue };
                    self.walk_pages(r, out, seen, depth + 1, Some(merged.clone()))?;
                    if out.len() >= MAX_PAGES {
                        return Err(Error::Limit {
                            kind: "page count",
                            limit: MAX_PAGES,
                        });
                    }
                }
            }
            _ => {
                // A leaf, whether or not it bothers to say `/Type /Page`.
                out.push(PageRef {
                    obj: node,
                    index: out.len(),
                });
            }
        }
        Ok(())
    }

    /// Decode a stream object's data.
    #[must_use]
    pub fn decoded(&self, s: &Stream) -> Vec<u8> {
        decode_stream(s).data
    }

    /// Every object number the file knows about.
    #[must_use]
    pub fn object_numbers(&self) -> Vec<u32> {
        self.with_xref(|x| x.object_numbers().collect())
    }

    /// The object numbers that have been removed from the graph.
    #[must_use]
    pub fn removed_refs(&self) -> std::collections::BTreeSet<Ref> {
        self.removed.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// The trailer's `/Info` reference, if the file has one.
    #[must_use]
    pub fn info_ref(&self) -> Option<Ref> {
        self.with_xref(|x| x.trailer().get("Info").and_then(Object::as_ref_id))
    }
}

fn merge_inherited(d: &Dict, parent: Option<&Dict>) -> Dict {
    let mut out = parent.cloned().unwrap_or_default();
    // `/Resources`, `/MediaBox`, `/CropBox`, `/Rotate` and `/Resources` are inheritable.
    for key in ["Resources", "MediaBox", "CropBox", "Rotate"] {
        if let Some(v) = d.get(key) {
            out.set(key, v.clone());
        }
    }
    out
}

fn header_version(data: &[u8]) -> String {
    let head = data.get(..32).unwrap_or(&[]);
    let text = String::from_utf8_lossy(head);
    for part in text.split(|c: char| !c.is_ascii_digit() && c != '.') {
        if part.len() >= 3 && part.contains('.') {
            return part.to_string();
        }
    }
    "1.7".to_string()
}

fn open_encryption(
    data: &[u8],
    xref: &Xref,
    encrypt_ref: Ref,
    options: &OpenOptions,
    file_id: &[u8],
) -> Result<(EncryptionState, Option<mangle_crypto::Decryptor>)> {
    let mut state = EncryptionState {
        encrypted: true,
        ..Default::default()
    };
    if options.ignore_encryption {
        return Ok((state, None));
    }
    // Read the `/Encrypt` dictionary without going through `Document`, which does not
    // exist yet. Its own strings are not encrypted.
    let dict = read_encrypt_dict(data, xref, encrypt_ref)?;
    state.filter = dict
        .get("Filter")
        .and_then(Object::as_name)
        .map(|n| String::from_utf8_lossy(n).into_owned());
    state.revision = dict.get("R").and_then(Object::as_i64).unwrap_or(0) as i32;
    if state.filter.as_deref() != Some("Standard") {
        return Err(Error::Encrypted(format!(
            "unsupported security handler `{}`",
            state.filter.unwrap_or_else(|| "none".into())
        )));
    }
    let mut ed = mangle_crypto::EncryptionDict {
        filter: state.filter.clone(),
        version: dict.get("V").and_then(Object::as_i64).unwrap_or(0) as i32,
        revision: state.revision,
        key_length_bits: dict
            .get("Length")
            .and_then(Object::as_i64)
            .map_or(0, |v| v as i32),
        owner: dict
            .get("O")
            .and_then(Object::as_bytes)
            .unwrap_or(&[])
            .to_vec(),
        user: dict
            .get("U")
            .and_then(Object::as_bytes)
            .unwrap_or(&[])
            .to_vec(),
        permissions: dict.get("P").and_then(Object::as_i64).unwrap_or(-1) as i32,
        encrypt_metadata: dict
            .get("EncryptMetadata")
            .and_then(Object::as_bool)
            .unwrap_or(true),
        perms: dict
            .get("Perms")
            .and_then(Object::as_bytes)
            .unwrap_or(&[])
            .to_vec(),
        o: dict
            .get("O")
            .and_then(Object::as_bytes)
            .unwrap_or(&[])
            .to_vec(),
        u: dict
            .get("U")
            .and_then(Object::as_bytes)
            .unwrap_or(&[])
            .to_vec(),
        oe: dict
            .get("OE")
            .and_then(Object::as_bytes)
            .unwrap_or(&[])
            .to_vec(),
        ue: dict
            .get("UE")
            .and_then(Object::as_bytes)
            .unwrap_or(&[])
            .to_vec(),
        id0: file_id.to_vec(),
        ..Default::default()
    };
    // The AES-256 handlers keep the file key in a private `/Encrypt` entry in revisions 5
    // and 6; recover it by running the user-password check and reading the result.
    let password = options.password.clone().unwrap_or_default();
    let attempt = mangle_crypto::validate_user_password(&ed, &password);
    match attempt {
        Ok((d, perms)) => {
            ed.key = d.key().to_vec();
            state.restricted = !perms.all();
            Ok((state, Some(d)))
        }
        Err(_) => match mangle_crypto::owner_password_key(&ed, &password) {
            Ok(d) => {
                ed.key = d.key().to_vec();
                state.restricted = false;
                Ok((state, Some(d)))
            }
            Err(e) => Err(Error::Encrypted(e.to_string())),
        },
    }
}

fn read_encrypt_dict(data: &[u8], xref: &Xref, r: Ref) -> Result<Dict> {
    let Some(XrefEntry::InFile { offset, .. }) = xref.get(r.num) else {
        return Err(Error::Encrypted(
            "the /Encrypt dictionary is not in the file".into(),
        ));
    };
    let Some(chunk) = data.get(offset..) else {
        return Err(Error::Encrypted(
            "the /Encrypt dictionary is out of range".into(),
        ));
    };
    let mut p = Parser::at(chunk, 0);
    let _ = p.next_object().ok().flatten();
    let _ = p.next_object().ok().flatten();
    if !p.next_keyword(b"obj") {
        return Err(Error::Encrypted(
            "the /Encrypt dictionary is malformed".into(),
        ));
    }
    match p.next_object() {
        Ok(Some(Object::Dict(d))) => Ok(d),
        _ => Err(Error::Encrypted(
            "the /Encrypt dictionary is malformed".into(),
        )),
    }
}

/// Read a document, for tests and the CLI.
pub fn open_file(path: &std::path::Path, options: OpenOptions) -> Result<Document> {
    let data = std::fs::read(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    Document::open(data, options)
}

/// Convenience: the name of a dictionary key as a [`Name`].
#[must_use]
pub fn key(s: &str) -> Name {
    Name::new(s)
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    /// A small but complete file, built the way a real producer would.
    fn sample(pages: usize) -> Vec<u8> {
        let mut objs: Vec<String> = Vec::new();
        let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + i)).collect();
        objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
        objs.push(format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            kids.join(" ")
        ));
        objs.push("<< /Length 44 >>\nstream\nBT /F1 12 Tf 10 10 Td (page) Tj ET\nendstream".into());
        for _ in 0..pages {
            objs.push(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 3 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
                    .to_string(),
            );
        }
        objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());

        let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes(),
        );
        for o in &offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn opens_a_clean_file() {
        let doc = Document::open(sample(2), OpenOptions::default()).expect("open");
        assert!(doc.info().recovery.banner().is_none());
        assert_eq!(doc.page_count().expect("pages"), 2);
        let cat = doc.catalog().expect("catalog");
        assert_eq!(
            cat.get("Type").and_then(Object::as_name),
            Some(&b"Catalog"[..])
        );
    }

    #[test]
    fn objects_load_lazily_and_repeatably() {
        let doc = Document::open(sample(1), OpenOptions::default()).expect("open");
        let a = doc.object(Ref::new(1, 0)).expect("object 1");
        let b = doc.object(Ref::new(1, 0)).expect("object 1 again");
        assert_eq!(a, b);
    }

    #[test]
    fn streams_keep_their_raw_bytes() {
        let doc = Document::open(sample(1), OpenOptions::default()).expect("open");
        let s = doc.object(Ref::new(3, 0)).expect("stream");
        let Object::Stream(st) = s else {
            panic!("expected a stream")
        };
        assert!(st.raw.starts_with(b"BT /F1"), "{:?}", st.raw);
        assert_eq!(doc.decoded(&st), st.raw);
    }

    #[test]
    fn overlay_wins_over_the_file() {
        let doc = Document::open(sample(1), OpenOptions::default()).expect("open");
        assert!(!doc.is_modified());
        doc.set(
            Ref::new(1, 0),
            Object::Dict(
                [
                    (Name::new("Type"), Object::name("Catalog")),
                    (Name::new("Marked"), Object::Bool(true)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        assert!(doc.is_modified());
        let cat = doc.catalog().expect("catalog");
        assert_eq!(cat.get("Marked").and_then(Object::as_bool), Some(true));
        assert_eq!(doc.overlay().len(), 1);
    }

    #[test]
    fn removal_hides_an_object() {
        let doc = Document::open(sample(1), OpenOptions::default()).expect("open");
        assert!(doc.object(Ref::new(5, 0)).is_some());
        doc.remove(Ref::new(5, 0));
        assert!(doc.object(Ref::new(5, 0)).is_none());
        assert!(doc.is_modified());
    }

    #[test]
    fn allocation_goes_past_size() {
        let doc = Document::open(sample(1), OpenOptions::default()).expect("open");
        let a = doc.alloc();
        let b = doc.alloc();
        assert_eq!(a.num + 1, b.num);
    }

    #[test]
    fn a_page_tree_cycle_terminates() {
        // Two pages pointing at each other as parents.
        let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /Parent 3 0 R >>\nendobj\n\
                    3 0 obj\n<< /Type /Page /Parent 2 0 R /Kids [2 0 R] >>\nendobj\n\
                    trailer\n<< /Root 1 0 R /Size 4 >>\nstartxref\n0\n%%EOF\n";
        let doc = Document::open(data.to_vec(), OpenOptions::default()).expect("open");
        // Must terminate; it may report a cycle or return the pages it found.
        let _ = doc.pages();
    }

    #[test]
    fn budget_stops_runaway_reads() {
        let doc = Document::open(
            sample(1),
            OpenOptions {
                object_budget: 4,
                ..Default::default()
            },
        )
        .expect("open");
        for i in 0..20 {
            let _ = doc.object(Ref::new(i, 0));
        }
        // The document must stay usable rather than hang or panic.
        assert_eq!(doc.page_count().unwrap_or(0), 0);
    }
}

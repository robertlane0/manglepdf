//! Document metadata: the `/Info` dictionary and the XMP packet.
//!
//! These two disagree in real files, often because a tool updated one and not the
//! other. Neither is authoritative over the other, so both are kept and the UI shows
//! both.

use mangle_syntax::{Dict, Object, Ref};

use crate::Resolver;
use crate::outlines::decode_pdf_text;

/// The `/Info` dictionary, decoded.
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    /// `/CreationDate` and `/ModDate`, verbatim. Parsing them needs a date format the
    /// specification only half-defines, and a wrong date is worse than an unparsed one.
    pub created: Option<String>,
    pub modified: Option<String>,
    pub trapped: Option<String>,
}

impl Info {
    #[must_use]
    pub fn from_dict(d: &Dict) -> Self {
        let text = |k: &str| d.get(k).and_then(Object::as_bytes).map(decode_pdf_text);
        let mut info = Self {
            title: text("Title"),
            author: text("Author"),
            subject: text("Subject"),
            keywords: text("Keywords"),
            creator: text("Creator"),
            producer: text("Producer"),
            created: text("CreationDate"),
            modified: text("ModDate"),
            trapped: d
                .get("Trapped")
                .and_then(Object::as_name)
                .map(|n| String::from_utf8_lossy(n).into_owned()),
        };
        // An empty string is the same as no value, and a UI showing an empty title
        // field is clearer than one showing "".
        for slot in [
            &mut info.title,
            &mut info.author,
            &mut info.subject,
            &mut info.keywords,
            &mut info.creator,
            &mut info.producer,
        ] {
            if slot.as_deref() == Some("") {
                *slot = None;
            }
        }
        info
    }
}

/// The document's XMP packet, plus the fields most consumers care about.
#[derive(Debug, Clone, Default)]
pub struct DocumentMetadata {
    pub info: Info,
    /// The raw XMP packet, decoded as UTF-8 with invalid bytes replaced.
    pub xmp: Option<String>,
    /// `/Lang`, the document's natural language.
    pub language: Option<String>,
}

impl DocumentMetadata {
    /// Read `/Info` from the trailer and `/Metadata` from the catalogue.
    #[must_use]
    pub fn build(resolver: &dyn Resolver, info_ref: Option<Ref>, catalog: &Dict) -> Self {
        let info = info_ref
            .and_then(|r| resolver.resolve(r))
            .and_then(|o| o.as_dict().cloned())
            .map_or_else(Info::default, |d| Info::from_dict(&d));
        let xmp = catalog
            .get("Metadata")
            .and_then(|o| crate::follow(resolver, o.clone()))
            .and_then(|o| match o {
                Object::Stream(s) => {
                    let data = mangle_syntax::stream::decode_stream(&s).data;
                    Some(String::from_utf8_lossy(&data).into_owned())
                }
                _ => None,
            });
        Self {
            info,
            xmp,
            language: catalog
                .get("Lang")
                .and_then(Object::as_bytes)
                .map(decode_pdf_text),
        }
    }
}

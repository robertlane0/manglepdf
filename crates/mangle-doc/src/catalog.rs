//! The catalogue, and the one call that assembles everything above it.

use mangle_syntax::{Dict, Object, Ref};

use crate::attachments::EmbeddedFiles;
use crate::error::Result;
use crate::labels::PageLabel;
use crate::layers::LayerTree;
use crate::metadata::DocumentMetadata;
use crate::names::NameTree;
use crate::outlines::Outline;
use crate::pages::PageTree;
use crate::{Resolver, follow, follow_from};

/// The document catalogue and everything hanging off it.
#[derive(Debug)]
pub struct Catalog {
    /// The catalogue's object number.
    pub id: Ref,
    /// The catalogue dictionary.
    pub dict: Dict,
    /// Flattened pages, with inheritance resolved.
    pub pages: PageTree,
    /// The page labelling scheme.
    pub labels: PageLabel,
    /// `/Outlines`.
    pub outline: Outline,
    /// `/Dests` (name tree) merged with the legacy `/Dests` dictionary.
    pub destinations: NameTree,
    /// `/OCGs`.
    pub layers: LayerTree,
    /// `/Names` `/EmbeddedFiles`.
    pub attachments: EmbeddedFiles,
    /// `/Info` and `/Metadata`.
    pub metadata: DocumentMetadata,
    /// `/AcroForm` fields dictionary, when there is one.
    pub acro_form: Option<Dict>,
    /// `/ViewerPreferences`, `/PageLayout`, `/PageMode` and friends.
    pub viewer_preferences: Dict,
}

impl Catalog {
    /// Assemble the document from a resolver and the catalogue's object number.
    pub fn build(resolver: &dyn Resolver, id: Ref) -> Result<Self> {
        let dict = resolver
            .resolve(id)
            .and_then(|o| o.as_dict().cloned())
            .ok_or_else(|| crate::Error::Dangling("the catalogue is missing".into()))?;

        let pages_ref = dict
            .get("Pages")
            .and_then(Object::as_ref_id)
            .ok_or_else(|| crate::Error::Structure("the catalogue has no /Pages".into()))?;
        let pages = PageTree::build(resolver, pages_ref)?;
        let labels = PageLabel::build(resolver, &dict);
        let outline = dict
            .get("Outlines")
            .and_then(Object::as_ref_id)
            .map_or_else(|| Ok(Outline::default()), |r| Outline::build(resolver, r))?;
        let destinations = build_destinations(resolver, &dict);
        let layers = LayerTree::build(resolver, &dict);
        let attachments = EmbeddedFiles::build(resolver, &dict)?;
        let metadata = DocumentMetadata::build(resolver, None, &dict);
        let acro_form = follow_from(resolver, &dict, "AcroForm").and_then(|o| o.as_dict().cloned());
        let viewer_preferences = follow_from(resolver, &dict, "ViewerPreferences")
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();

        Ok(Self {
            id,
            dict,
            pages,
            labels,
            outline,
            destinations,
            layers,
            attachments,
            metadata,
            acro_form,
            viewer_preferences,
        })
    }

    /// Where to open: the catalogue's `/OpenAction` or `/Page`.
    #[must_use]
    pub fn initial_destination(&self, resolver: &dyn Resolver) -> Option<Ref> {
        self.dict
            .get("OpenAction")
            .and_then(|o| follow(resolver, o.clone()))
            .and_then(|o| destination_page(&o, resolver, &self.pages))
    }

    /// The structure tree root, when the document is tagged.
    #[must_use]
    pub fn struct_tree(&self, resolver: &dyn Resolver) -> Option<Ref> {
        follow_from(resolver, &self.dict, "StructTreeRoot").and_then(|o| o.as_ref_id())
    }
}

/// `/OpenAction` may be a destination or an action; a GoTo action names the page in
/// its `/D`.
fn destination_page(obj: &Object, resolver: &dyn Resolver, pages: &PageTree) -> Option<Ref> {
    let target = match obj {
        Object::Dict(d) => d
            .get("D")
            .and_then(|d| follow(resolver, d.clone()))
            .unwrap_or_else(|| obj.clone()),
        other => other.clone(),
    };
    if let Some(r) = target.as_ref_id()
        && pages.get(r.num as usize).is_some()
    {
        return Some(r);
    }
    let arr = target.as_array()?;
    arr.first()?.as_ref_id()
}

/// Merge the two destination mechanisms. A file may carry both, and the name tree
/// wins on a collision, which is what the specification says.
fn build_destinations(resolver: &dyn Resolver, dict: &Dict) -> NameTree {
    let mut tree = NameTree::default();
    if let Some(names) = dict
        .get("Names")
        .and_then(|o| follow(resolver, o.clone()))
        .and_then(|o| o.as_dict().cloned())
        && let Some(dests) = names.get("Dests").and_then(|o| follow(resolver, o.clone()))
    {
        if let Ok(built) = NameTree::build_from_node(resolver, dests) {
            tree = built;
        }
    }
    if let Some(legacy) = dict.get("Dests").and_then(|o| follow(resolver, o.clone()))
        && let Some(d) = legacy.as_dict()
    {
        tree = tree.merged(NameTree::from_legacy(resolver, d));
    }
    tree
}

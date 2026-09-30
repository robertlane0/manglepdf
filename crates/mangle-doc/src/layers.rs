//! Optional content: the groups a user can switch on and off, and the commands that
//! decide what is visible.

use mangle_syntax::{Dict, Object, Ref, Stream};

use crate::{Resolver, follow, kids_of};

/// One optional-content group.
#[derive(Debug, Clone)]
pub struct Layer {
    /// The group's object number.
    pub id: Ref,
    /// `/Name`, when it has one. A group without a name is legal and still togglable.
    pub name: Option<Vec<u8>>,
    /// `/Intent`: `View`, `Design` or both.
    pub intent: Vec<Vec<u8>>,
    /// `/Usage`: `/View`, `/Print`, `/Export`, or none at all.
    pub usage: Vec<Vec<u8>>,
    /// The default state, for `/AS`.
    pub on: bool,
    /// The group's configuration dictionary, if it has one.
    pub config: Option<Dict>,
    /// The group is nested inside another.
    pub parent: Option<Ref>,
    /// The thumbnail or panel icon.
    pub icon: Option<Stream>,
}

impl Layer {
    /// Does this group show on screen?
    #[must_use]
    pub fn visible(&self) -> bool {
        self.on
            && self
                .intent
                .iter()
                .any(|i| i == b"View" || i == b"View/Design")
    }

    /// Does this group print?
    #[must_use]
    pub fn printable(&self) -> bool {
        self.intent
            .iter()
            .any(|i| i == b"Print" || i == b"View/Design")
            || (self.intent.is_empty() && self.on)
    }

    /// The name as text, for the layers panel.
    #[must_use]
    pub fn name_text(&self) -> String {
        self.name
            .as_ref()
            .map(|n| crate::outlines::decode_pdf_text(n))
            .unwrap_or_else(|| format!("layer {}", self.id.num))
    }
}

/// A visibility rule from an `/OCMD` dictionary.
#[derive(Debug, Clone, Default)]
pub struct LayerCommand {
    pub name: String,
    /// `/OCGs`: the groups this command talks about.
    pub groups: Vec<Ref>,
    /// `/P`: `AnyOn` (0) or `AllOn` (1). The default is `AnyOn`.
    pub policy: Policy,
    /// `/VE`: the expression, which we evaluate only in its simple forms.
    pub expression: Option<Object>,
}

/// How a command combines its groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    #[default]
    AnyOn,
    AllOn,
}

/// Every layer in the document, keyed by object number.
#[derive(Debug, Clone, Default)]
pub struct LayerTree {
    layers: Vec<Layer>,
}

impl LayerTree {
    /// Read `/OCGs` from the catalogue and walk each group's own `/OCGs` children.
    #[must_use]
    pub fn build(resolver: &dyn Resolver, catalog: &Dict) -> Self {
        let mut layers = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let Some(root) = catalog
            .get("OCGs")
            .and_then(|o| follow(resolver, o.clone()))
            .and_then(|o| o.as_dict().cloned())
        else {
            return Self::default();
        };
        let mut queue: Vec<(Object, Option<Ref>)> = vec![(Object::Dict(root), None)];
        while let Some((obj, parent)) = queue.pop() {
            if layers.len() >= 10_000 {
                break;
            }
            for item in entries(resolver, &obj) {
                let Some(d) = item.as_dict() else { continue };
                let id = match d.get("Self").and_then(Object::as_ref_id) {
                    Some(r) if seen.insert(r) => r,
                    _ => continue,
                };
                layers.push(Layer {
                    id,
                    name: d.get("Name").and_then(Object::as_bytes).map(<[u8]>::to_vec),
                    intent: name_list(d, "Intent"),
                    usage: name_list(d, "Usage"),
                    on: d.get("ON").and_then(Object::as_bool).unwrap_or(true),
                    config: d.get("Config").and_then(Object::as_dict).cloned(),
                    parent,
                    icon: d
                        .get("Thumb")
                        .and_then(|t| follow(resolver, t.clone()))
                        .and_then(|o| match o {
                            Object::Stream(s) => Some(s),
                            _ => None,
                        }),
                });
                // A group may nest further groups.
                for kid in kids_of(resolver, d) {
                    if kid.as_dict().is_some_and(|k| k.get("Type").is_some()) {
                        queue.push((kid, Some(id)));
                    }
                }
            }
        }
        Self { layers }
    }

    #[must_use]
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    #[must_use]
    pub fn get(&self, id: Ref) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// The `/OCMD` dictionaries in the document, in catalogue order.
    #[must_use]
    pub fn commands(&self, resolver: &dyn Resolver, catalog: &Dict) -> Vec<LayerCommand> {
        let Some(root) = catalog
            .get("OCMDs")
            .and_then(|o| follow(resolver, o.clone()))
            .and_then(|o| o.as_dict().cloned())
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for item in entries(resolver, &Object::Dict(root)) {
            let Some(d) = item.as_dict() else { continue };
            if let Some(r) = d.get("Self").and_then(Object::as_ref_id)
                && !seen.insert(r)
            {
                continue;
            }
            out.push(read_command(d));
        }
        out
    }
}

fn entries(resolver: &dyn Resolver, obj: &Object) -> Vec<Object> {
    let Some(d) = obj.as_dict() else {
        return Vec::new();
    };
    d.get("OCGs")
        .and_then(|o| follow(resolver, o.clone()))
        .and_then(|o| o.as_array().map(<[Object]>::to_vec))
        .map(|a| a.into_iter().filter_map(|x| follow(resolver, x)).collect())
        .unwrap_or_default()
}

fn read_command(d: &Dict) -> LayerCommand {
    let policy = match d.get("P").and_then(Object::as_name) {
        Some(b"ALLON") => Policy::AllOn,
        _ => Policy::AnyOn,
    };
    LayerCommand {
        name: String::new(),
        groups: d
            .get("OCGs")
            .and_then(|o| o.as_array())
            .map(|a| a.iter().filter_map(Object::as_ref_id).collect())
            .unwrap_or_default(),
        policy,
        expression: d.get("VE").cloned(),
    }
}

fn name_list(d: &Dict, key: &str) -> Vec<Vec<u8>> {
    match d.get(key) {
        Some(Object::Name(n)) => vec![n.as_bytes().to_vec()],
        Some(Object::Array(a)) => a
            .iter()
            .filter_map(Object::as_name)
            .map(<[u8]>::to_vec)
            .collect(),
        Some(Object::Dict(inner)) => match inner.get(key) {
            Some(Object::Name(n)) => vec![n.as_bytes().to_vec()],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

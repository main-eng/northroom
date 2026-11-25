//! A page's links (`Link` annotations): where each sits and where it leads, a web address or
//! another page of the document. Destinations named in the document's `Dests` dictionary or
//! name tree are looked up.
//!
//! Left out: links that run other actions (launching files, JavaScript, named page moves) and
//! links into other documents.

use super::object::{Dict, Document, Object, Ref};
use std::collections::HashMap;

/// Where a link leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A web or mail address.
    Address(String),
    /// A page of this document, counted from 0.
    Page(usize),
}

/// How deep a name tree is followed.
const TREE_DEPTH_MAX: usize = 32;
/// How many of a name tree's nodes one lookup visits at most.
const TREE_NODES_MAX: usize = 10_000;

/// The schemes a link may open: the web and mail. Anything else (`file:`, `javascript:`)
/// stays shut.
const SCHEMES: [&str; 3] = ["http://", "https://", "mailto:"];

/// Where a link annotation leads: its action's address or destination, or its own `Dest`.
pub fn target_of(
    document: &Document,
    annotation: &Dict,
    pages: &HashMap<Ref, usize>,
) -> Option<Target> {
    if let Some(action) = document.entry(annotation, "A").as_dict() {
        return match document.entry(action, "S").as_name()? {
            "URI" => address(document.entry(action, "URI").as_bytes()?),
            "GoTo" => destination(document, &document.entry(action, "D"), pages, 0),
            _ => None,
        };
    }
    destination(document, &document.entry(annotation, "Dest"), pages, 0)
}

/// A link's address, when it is one a person can safely open.
fn address(bytes: &[u8]) -> Option<Target> {
    let text = String::from_utf8_lossy(bytes).trim().to_owned();
    let lower = text.to_ascii_lowercase();
    SCHEMES
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        .then_some(Target::Address(text))
}

/// The page a destination names: an array starting with the page (or its number), a dictionary
/// holding one (`D`), or a name looked up in the document.
fn destination(
    document: &Document,
    object: &Object,
    pages: &HashMap<Ref, usize>,
    depth: usize,
) -> Option<Target> {
    if depth > 4 {
        return None;
    }
    match object {
        Object::Array(items) => match items.first()? {
            Object::Ref(page) => pages.get(page).copied().map(Target::Page),
            Object::Integer(number) => usize::try_from(*number).ok().map(Target::Page),
            _ => None,
        },
        Object::Dict(dict) => destination(document, &document.entry(dict, "D"), pages, depth + 1),
        Object::Name(name) => destination(
            document,
            &named(document, name.as_bytes())?,
            pages,
            depth + 1,
        ),
        Object::String(name) => destination(document, &named(document, name)?, pages, depth + 1),
        _ => None,
    }
}

/// The destination the catalog names `name`: in its `Dests` dictionary, else its `Names` tree.
fn named(document: &Document, name: &[u8]) -> Option<Object> {
    let catalog = document.entry(&document.trailer, "Root");
    let catalog = catalog.as_dict()?;
    if let Some(dests) = document.entry(catalog, "Dests").as_dict() {
        if let Some(found) = dests.get(String::from_utf8_lossy(name).as_ref()) {
            return Some(document.resolve(found));
        }
    }
    let names = document.entry(catalog, "Names");
    let tree = document.entry(names.as_dict()?, "Dests");
    let mut budget = TREE_NODES_MAX;
    in_tree(document, tree.as_dict()?, name, 0, &mut budget)
}

/// `name`'s value in a name tree: in a node's `Names` pairs, else under its `Kids`.
/// `budget` counts down the nodes still to visit, so a tree whose kids loop back on themselves
/// ends rather than running for ever.
fn in_tree(
    document: &Document,
    node: &Dict,
    name: &[u8],
    depth: usize,
    budget: &mut usize,
) -> Option<Object> {
    if depth > TREE_DEPTH_MAX || *budget == 0 {
        return None;
    }
    *budget -= 1;
    if let Some(pairs) = document.entry(node, "Names").as_array() {
        for pair in pairs.chunks(2) {
            if document.resolve(&pair[0]).as_bytes() == Some(name) {
                return pair.get(1).map(|value| document.resolve(value));
            }
        }
    }
    let kids = document.entry(node, "Kids");
    kids.as_array()?.iter().find_map(|kid| {
        let kid = document.resolve(kid);
        in_tree(document, kid.as_dict()?, name, depth + 1, budget)
    })
}

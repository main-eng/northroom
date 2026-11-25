//! Optional content: a PDF's layers, and which of them are hidden when the file opens. Content
//! marked with a hidden layer (`BDC /OC`), or a picture or form that names one (`/OC`), draws
//! nothing.
//!
//! Left out: visibility expressions (`VE`), which fall back to the groups they name, and a
//! layer's print-only or zoom-dependent usage.

use super::object::{Dict, Document, Object, Ref};
use std::collections::HashSet;

/// The layers hidden in the document's default configuration.
#[derive(Default)]
pub struct Layers {
    hidden: HashSet<Ref>,
}

impl Layers {
    /// Reads the default configuration (`OCProperties` `D`): every layer starts as its
    /// `BaseState` says, then those in `ON` show and those in `OFF` hide.
    pub fn of(document: &Document) -> Self {
        let catalog = document.entry(&document.trailer, "Root");
        let properties = catalog
            .as_dict()
            .map(|catalog| document.entry(catalog, "OCProperties"));
        let Some(properties) = properties.as_ref().and_then(Object::as_dict) else {
            return Self::default();
        };
        let defaults = document.entry(properties, "D");
        let Some(defaults) = defaults.as_dict() else {
            return Self::default();
        };
        let references = |object: Object| -> Vec<Ref> {
            object
                .as_array()
                .map(|items| items.iter().filter_map(reference_of).collect())
                .unwrap_or_default()
        };
        let mut hidden: HashSet<Ref> = HashSet::new();
        if document.entry(defaults, "BaseState").as_name() == Some("OFF") {
            hidden.extend(references(document.entry(properties, "OCGs")));
        }
        for shown in references(document.entry(defaults, "ON")) {
            hidden.remove(&shown);
        }
        hidden.extend(references(document.entry(defaults, "OFF")));
        Self { hidden }
    }

    /// Whether `object`, a layer or a group of layers (`OCMD`), hides what it marks.
    pub fn hides(&self, document: &Document, object: &Object) -> bool {
        let resolved = document.resolve(object);
        let Some(dict) = resolved.as_dict() else {
            return false;
        };
        if document.entry(dict, "Type").as_name() == Some("OCMD") {
            return self.membership_hides(document, dict);
        }
        reference_of(object).is_some_and(|layer| self.hidden.contains(&layer))
    }

    /// A group of layers hides by its policy over its layers: shown when any is on, by default.
    fn membership_hides(&self, document: &Document, dict: &Dict) -> bool {
        let layers: Vec<Ref> = match dict.get("OCGs") {
            Some(Object::Ref(layer)) => vec![*layer],
            Some(other) => document
                .resolve(other)
                .as_array()
                .map(|items| items.iter().filter_map(reference_of).collect())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        if layers.is_empty() {
            return false;
        }
        let on = layers
            .iter()
            .filter(|layer| !self.hidden.contains(layer))
            .count();
        let shown = match document.entry(dict, "P").as_name() {
            Some("AllOn") => on == layers.len(),
            Some("AnyOff") => on < layers.len(),
            Some("AllOff") => on == 0,
            _ => on > 0,
        };
        !shown
    }
}

fn reference_of(object: &Object) -> Option<Ref> {
    match object {
        Object::Ref(reference) => Some(*reference),
        _ => None,
    }
}

//! Model extraction for the SMOL formal verifier.
//!
//! Builds an index of all classes, identifies `classifies` state classes and
//! their "classification roots" (the entity type referenced by a `context`
//! field), and constructs an explicit-state transition system used for
//! reachability / CTL model checking.

use std::collections::{HashMap, HashSet};

use smol_syntax::ast::{ClassDecl, Expr, FieldDecl, FieldModifier, File, Type};
use smol_syntax::span::Span;

/// A class declaration together with the file it was declared in.
#[derive(Clone, Debug)]
pub struct ClassEntry {
    pub decl: ClassDecl,
    pub file: String,
}

/// Maps class name to their declaration
#[derive(Clone, Debug, Default)]
pub struct ClassMap {
    entries: HashMap<String, ClassEntry>,
}

impl ClassMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: String, entry: ClassEntry) {
        self.entries.entry(name).or_insert(entry);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ClassEntry> {
        self.entries.get(name)
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    #[must_use]
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ClassEntry)> {
        self.entries.iter()
    }

    /// Names of the full inheritance chain for `name`, most-derived first.
    #[must_use]
    pub fn ancestors(&self, name: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = Some(name.to_owned());
        while let Some(c) = current {
            result.push(c.clone());
            current = self.get(&c).and_then(|e| e.decl.extends.clone());
        }
        result
    }

    /// Resolve a field by walking the inheritance chain.
    #[must_use]
    pub fn field(&self, class: &str, name: &str) -> Option<&FieldDecl> {
        for anc in self.ancestors(class) {
            if let Some(entry) = self.get(&anc) {
                if let Some(f) = entry.decl.fields.iter().find(|f| f.name == name) {
                    return Some(f);
                }
            }
        }
        None
    }

    /// Return `true` if `sub` is `sup` or a subclass of it.
    #[must_use]
    pub fn is_subtype(&self, sub: &str, sup: &str) -> bool {
        self.ancestors(sub).iter().any(|a| a == sup)
    }

    /// The `classifies` IRI declared on `class` or any ancestor.
    #[must_use]
    pub fn classifies_iri(&self, class: &str) -> Option<String> {
        for anc in self.ancestors(class) {
            if let Some(entry) = self.get(&anc) {
                if let Some(ann) = &entry.decl.classifies {
                    return Some(ann.iri.clone());
                }
            }
        }
        None
    }

    /// If `class` (or an ancestor) declares a `context` field, return the name
    /// of the entity type that field points to.
    #[must_use]
    pub fn context_entity(&self, class: &str) -> Option<String> {
        for anc in self.ancestors(class) {
            if let Some(entry) = self.get(&anc) {
                for f in &entry.decl.fields {
                    if f.modifier == FieldModifier::Context {
                        if let Type::Named(n) = &f.ty {
                            return Some(n.clone());
                        }
                    }
                }
            }
        }
        None
    }
}

/// Build the merged class index from all parsed files (first declaration wins).
#[must_use]
pub fn build_class_map(files: &[File]) -> ClassMap {
    let mut map = ClassMap::new();
    for file in files {
        for cls in &file.classes {
            if !map.contains(&cls.name) {
                map.insert(
                    cls.name.clone(),
                    ClassEntry {
                        decl: cls.clone(),
                        file: file.path.clone(),
                    },
                );
            }
        }
    }
    map
}

/// A reclassifiable state: a class that carries a `classifies` annotation.
#[derive(Clone, Debug)]
pub struct StateClass {
    pub name: String,
    pub iri: String,
    /// The entity type being classified (from a `context` field).
    pub root: String,
    pub file: String,
    pub span: Span,
}

/// A classification root: one entity type and states it can be classified into.
#[derive(Clone, Debug)]
pub struct Root {
    pub entity: String,
    pub states: Vec<String>,
    /// True when some `adapt`/`classify` site targets this entity.
    pub adapted: bool,
}

/// THe extracted classification model
#[derive(Clone, Debug)]
pub struct Extracted {
    /// Every `classifies` state class (including orphans with no root).
    pub states: Vec<StateClass>,
    /// Classification roots (one per entity type that has state classes).
    pub roots: Vec<Root>,
    /// Maps a state-class name to its root entity.
    pub state_roots: HashMap<String, String>, // state class -> root entity
}

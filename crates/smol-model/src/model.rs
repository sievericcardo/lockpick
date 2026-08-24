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

/// The extracted classification model
#[derive(Clone, Debug)]
pub struct Extracted {
    /// Every `classifies` state class (including orphans with no root).
    pub states: Vec<StateClass>,
    /// Classification roots (one per entity type that has state classes).
    pub roots: Vec<Root>,
    /// Maps a state-class name to its root entity.
    pub state_roots: HashMap<String, String>, // state class -> root entity
}

/// Determine the classification root (entity type) of a state class `name`.
///
/// Two patterns are supported:
/// 1. *State-holder* pattern: `Healthy extends HealthState` where `HealthState`
///    declares `context Plant` — the root is the context entity (`Plant`).
/// 2. *Direct reclassification* pattern: `OperatingPump extends Pump` — the root
///    is the nearest ancestor without its own `classifies` annotation (`Pump`).
#[must_use]
pub fn state_root(map: &ClassMap, name: &str) -> Option<String> {
    // 1. A `context` field on an ancestor defines the root entity type.
    if let Some(root) = map.context_entity(name) {
        return Some(root);
    }
    // 2. Nearest ancestor without its own `classifies` annotation is the root.
    for ancestor in map.ancestors(name) {
        if ancestor == name {
            continue;
        }
        let has_classifies: bool = map
            .get(&ancestor)
            .map_or(false, |e| e.decl.classifies.is_some());
        if !has_classifies {
            return Some(ancestor);
        }
    }
    None
}

/// Collect all `classifies` state classes and group them by root entity.
/// A state class whose root cannot be determined is kept in `states` with an
/// empty `root` and flagged separately.
#[must_use]
pub fn extract(map: &ClassMap) -> Extracted {
    let mut states = Vec::new();
    let mut state_roots = HashMap::new();
    let mut roots_map: HashMap<String, Root> = HashMap::new();

    for (name, entry) in map.iter() {
        let Some(annotation) = &entry.decl.classifies else {
            continue;
        };
        let root = state_root(map, name);
        let state = StateClass {
            name: name.clone(),
            iri: annotation.iri.clone(),
            root: root.clone().unwrap_or_default(),
            file: entry.file.clone(),
            span: annotation.span,
        };
        if let Some(root) = root {
            state_roots.insert(name.clone(), root.clone());
            roots_map.entry(root.clone()).or_insert_with(|| Root {
                entity: root.clone(),
                states: Vec::new(),
                adapted: false,
            });
            states.push(state);
        }
    }

    for state in &states {
        if !state.root.is_empty() {
            if let Some(root) = roots_map.get_mut(&state.root) {
                root.states.push(state.name.clone());
            }
        }
    }

    Extracted {
        states,
        roots: roots_map.into_values().collect(),
        state_roots,
    }
}

/// Return the names of all classes that *directly* declare a `context` field of
/// type `entity`.  These are the state-holder classes for that entity.
#[must_use]
pub fn context_holders(map: &ClassMap, entity: &str) -> Vec<String> {
    let mut result = Vec::new();
    for (name, entry) in map.iter() {
        let has = entry.decl.fields.iter().any(|f| {
            f.modifier == FieldModifier::Context
                && matches!(&f.ty, Type::Named(n) if n == entity)
        });
        if has {
            result.push(name.clone());
        }
    }
    result
}

/// A simplified resolved type used only for model-level reasoning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedType {
    /// A named (user-defined) class type.
    Class(String),
    /// A `List<T>` / `ExplList<T>` type.
    List(Box<ResolvedType>),
    /// An `FMO[...]` type with its declared port names.
    Fmo(Vec<String>),
    /// A primitive type: `Int`, `Double`, `String`, `Boolean`, or `Unit`.
    Primitive(String),
    /// An unknown type (e.g., a variable that cannot be resolved).
    Unknown,
}

/// Convert an AST `Type` into a simplified resolved type.
#[allow(unreachable_patterns)]
#[must_use]
pub fn resolve_type(ty: &Type) -> ResolvedType {
    match ty {
        Type::Named(n) => ResolvedType::Class(n.clone()),
        Type::List(inner) | Type::ExplList(inner) => {
            ResolvedType::List(Box::new(resolve_type(inner)))
        }
        Type::Generic(n, _) => ResolvedType::Class(n.clone()),
        Type::Fmo(fields) => {
            ResolvedType::Fmo(fields.iter().map(|f| f.name.clone()).collect())
        },
        Type::Int | Type::Double | Type::String | Type::Boolean | Type::Unit => {
            ResolvedType::Primitive(format!("{:?}", ty))
        }
        _ => ResolvedType::Unknown,
    }
}

/// A lexical scope for resolving variable / field types inside a method body.
pub struct Resolver<'a> {
    pub map: &'a ClassMap,
    pub local_vars: HashMap<String, ResolvedType>,
    pub this: String,
}

impl<'a> Resolver<'a> {
    #[must_use]
    pub fn new(map: &'a ClassMap, this: &str) -> Self {
        Self {
            map,
            local_vars: HashMap::new(),
            this: this.to_owned(),
        }
    }

    /// Resolve the (simplified) type of an expression, if possible.
    #[must_use]
    pub fn resolve_expression(&self, expression: &Expr) -> ResolvedType {
        match expression {
            Expr::Var(name, _) => {
                if name == "this" {
                    ResolvedType::Class(self.this.clone())
                } else if let Some(resolved_type) = self.local_vars.get(name) {
                    resolved_type.clone()
                } else if let Some(f) = self.map.field(&self.this, name) {
                    resolve_type(&f.ty)
                } else {
                    ResolvedType::Unknown
                }
            }
            Expr::Field { receiver, field, .. } => {
                let receiver_type = self.resolve_expression(receiver);
                match receiver_type {
                    ResolvedType::List(inner) => match field.as_str() {
                        "content" => *inner,
                        "next" => ResolvedType::List(inner),
                        _ => ResolvedType::Unknown,
                    }
                    ResolvedType::Class(class_name) => self
                        .map
                        .field(&class_name, field)
                        .map_or(ResolvedType::Unknown, |f| {
                            resolve_type(&f.ty)
                        }),
                    _ => ResolvedType::Unknown,
                }
            }
            Expr::New { class, .. } => ResolvedType::Class(class.clone()),
            Expr::Classify { expr: inner, .. } => self.resolve_expression(inner),
            Expr::BuiltinCall { .. } | Expr::Construct { .. } | Expr::Access { .. } => ResolvedType::Unknown,
            Expr::IntLit(_, _)
            | Expr::FloatLit(_, _)
            | Expr::StrLit(_, _)
            | Expr::BoolLit(_, _) => ResolvedType::Primitive(format!("{:?}", expression)),
            Expr::Null(_)
            | Expr::BinOp { .. }
            | Expr::UnaryOp { .. }
            | Expr::Member { .. }
            | Expr::Validate { .. }
            | Expr::Simulate { .. }
            | Expr::Port { .. }
            | Expr::Super { .. } => ResolvedType::Unknown,
            _ => ResolvedType::Unknown
        }
    }

    /// Return the classification root for a resolved expression, if known.
    #[must_use]
    pub fn root_of(&self, resolved_type: &ResolvedType, state_roots: &HashMap<String, String>) -> Option<String> {
        let ResolvedType::Class(name) = resolved_type else {
            return None;
        };
        if let Some(root) = state_roots.get(name) {
            return Some(root.clone());
        }
        for ancestor in self.map.ancestors(name) {
            if let Some(root) = state_roots.get(&ancestor) {
                return Some(root.clone());
            }
            if let Some(entity) = self.map.context_entity(&ancestor) {
                return Some(entity);
            }
        }

        if state_roots.values().any(|r| r == name) {
            return Some(name.clone());
        }

        None
    }
}

/// An explicit-state transition system (Kripke-style) for CTL model checking.
#[derive(Debug, Clone)]
pub struct TransitionSystem {
    /// Number of states
    pub n: usize,
    /// The initial states (one per classification root, "unclassified")
    pub initials: Vec<usize>,
    /// Successor lists,
    pub successors: Vec<Vec<usize>>,
    /// Name of each state (`""` for unclassified roots)
    pub names: Vec<String>,
}

/// Build the global transition system from the roots.
///
/// Each root gets an "unclassified" initial state.  If the root is adapted,
/// the initial state can move to any state of the root, and each state can
/// reclassify into any other state (an over-approximation of the data-driven
/// reclassification).  Every state self-loops (an entity stays in its state).
#[must_use]
pub fn build_transition_system(roots: &[Root]) -> TransitionSystem {
    let mut n = 0usize;
    let mut initials = Vec::new();
    let mut names = Vec::new();
    let mut successors = Vec::new();

    // A map from root entity to the index of its unclassified state
    struct RootInfo {
        init: usize,
        states: Vec<usize>,
    }
    let mut infos: Vec<RootInfo> = Vec::new();

    for root in roots {
        let init = n;
        n += 1;
        initials.push(init);
        names.push(String::new());
        successors.push(Vec::new());

        let mut state_indices = Vec::new();
        for state in &root.states {
            let index = n;
            n += 1;
            names.push(state.clone());
            successors.push(Vec::new());
            state_indices.push(index);
        }
        infos.push(RootInfo{
            init,
            states: state_indices,
        });
    }

    // Wire edges (need all node counts known)
    // let mut successors = vec!(Vec::<usize>::new(); n);
    for (root, info) in roots.iter().zip(infos.iter()) {
        if root.adapted {
            // Unclassified -> any states
            successors[info.init] = info.states.clone();
            // Any state -> any state (reclassification) + self-loop
            for &state in &info.states {
                let mut output = info.states.clone();
                if !output.contains(&state) {
                    output.push(state);
                }
                successors[state] = output;
            }
        } else {
            successors[info.init] = Vec::new();
            for &state in &info.states {
                successors[state] = vec!(state);
            }
        }
    }

    TransitionSystem {
        n,
        initials,
        successors,
        names,
    }
}

/// Compute the set of states reachable from the initial states via BFS.
#[must_use]
pub fn reachable(ts: &TransitionSystem) -> HashSet<usize> {
    let mut seen = HashSet::new();
    let mut stack: Vec<usize> = ts.initials.clone();
    while let Some(s) = stack.pop() {
        if seen.insert(s) {
            for &next in &ts.successors[s] {
                stack.push(next);
            }
        }
    }
    seen
}

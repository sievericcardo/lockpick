//! `ClassEnv` — the resolved environment of all class declarations.
//!
//! Built by the declaration pass from the merged AST of all source files.
//! Stored in a plain `HashMap` so it can be shared cheaply by the body pass.

use smol_syntax::ast::{FieldDecl, MethodDecl, Type};
use smol_syntax::span::Span;
use std::collections::HashMap;

/// Resolved information about a single SMOL class, indexed by name.
#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub name: String,
    /// Direct superclass name (already validated to exist).
    pub extends: Option<String>,
    pub is_abstract: bool,
    /// Flattened field list (inherited fields first, own fields at the end).
    pub fields: Vec<FieldDecl>,
    /// Own method declarations.
    pub methods: Vec<MethodDecl>,
    /// `classifies` IRI, if any.
    pub classifies: Option<String>,
    /// Definition span.
    pub span: Span,
    /// Source file.
    pub file: String,
}

impl ClassInfo {
    /// Look up a field by name (searches own fields, not inherited — use
    /// `ClassEnv::resolve_field` for that).
    #[must_use]
    pub fn own_field(&self, name: &str) -> Option<&FieldDecl> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Look up a method by name (own methods only).
    #[must_use]
    pub fn own_method(&self, name: &str) -> Option<&MethodDecl> {
        self.methods.iter().find(|m| m.name == name)
    }
}

/// The complete class environment for one SMOL project.
#[derive(Debug, Default)]
pub struct ClassEnv {
    classes: HashMap<String, ClassInfo>,
}

impl ClassEnv {
    pub fn insert(&mut self, info: ClassInfo) {
        self.classes.insert(info.name.clone(), info);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ClassInfo> {
        self.classes.get(name)
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    /// Resolve a field by walking the inheritance chain.
    #[must_use]
    pub fn resolve_field<'a>(&'a self, class: &str, field: &str) -> Option<&'a FieldDecl> {
        let mut current = class;
        loop {
            let info = self.get(current)?;
            if let Some(f) = info.own_field(field) {
                return Some(f);
            }
            current = info.extends.as_deref()?;
        }
    }

    /// Resolve a method by walking the inheritance chain.
    #[must_use]
    pub fn resolve_method<'a>(&'a self, class: &str, method: &str) -> Option<&'a MethodDecl> {
        let mut current = class;
        loop {
            let info = self.get(current)?;
            if let Some(m) = info.own_method(method) {
                return Some(m);
            }
            current = info.extends.as_deref()?;
        }
    }

    /// Return the full inheritance chain for `class` (most-derived first).
    #[must_use]
    pub fn ancestors<'a>(&'a self, class: &'a str) -> Vec<&'a str> {
        let mut result = Vec::new();
        let mut current = class;
        loop {
            result.push(current);
            match self.get(current).and_then(|i| i.extends.as_deref()) {
                Some(parent) => current = parent,
                None => break,
            }
        }
        result
    }

    /// Return the total number of constructor arguments for `class`, including
    /// fields inherited from all ancestor classes.
    #[must_use]
    #[allow(clippy::while_let_loop)]
    pub fn constructor_field_count(&self, class: &str) -> usize {
        let mut count = 0;
        let mut current = class;
        loop {
            match self.get(current) {
                Some(info) => {
                    count += info.fields.len();
                    current = match &info.extends {
                        Some(p) => p,
                        None => break,
                    };
                }
                None => break,
            }
        }
        count
    }

    /// Return `true` if `sub` is a subtype of `sup` (including equality).
    #[must_use]
    pub fn is_subtype(&self, sub: &str, sup: &str) -> bool {
        if sub == sup {
            return true;
        }
        let mut current = sub;
        loop {
            match self.get(current).and_then(|i| i.extends.as_deref()) {
                Some(parent) if parent == sup => return true,
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }

    /// Check if `ty_a` is assignable to `ty_b`.
    ///
    /// Nullable rules (strict): `Nullable(T)` accepts `T` and `null`;
    /// plain `T` does NOT accept `Nullable(T)`.
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub fn is_assignable(&self, from: &Type, to: &Type) -> bool {
        match (from, to) {
            // null is represented as Type::Named("_null") internally — allow any ref type.
            (Type::Named(a), Type::Named(_)) if a == "_null" => true,
            // `null` flows into any nullable type.
            (Type::Named(a), Type::Nullable(_)) if a == "_null" => true,
            // `Nullable(T)` flows into `Nullable(U)` iff `T` flows into `U`.
            (Type::Nullable(from_ty), Type::Nullable(to_ty)) => self.is_assignable(from_ty, to_ty),
            // `T` flows into `Nullable(U)` iff `T` flows into `U`.
            (from_ty, Type::Nullable(to_ty)) => self.is_assignable(from_ty, to_ty),
            // Strict: `Nullable(T)` never flows into a non-nullable slot.
            (Type::Nullable(_), _) => false,
            (Type::Int, Type::Int)
            | (Type::Double, Type::Double)
            | (Type::String, Type::String)
            | (Type::Boolean, Type::Boolean)
            | (Type::Unit, Type::Unit) => true,
            (Type::List(a), Type::List(b)) => self.is_assignable(a, b),
            (Type::ExplList(a), Type::ExplList(b)) => self.is_assignable(a, b),
            (Type::Named(a), Type::Named(b)) => self.is_subtype(a, b),
            _ => false,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ClassInfo> {
        self.classes.values()
    }

    /// Return the names of all classes whose direct superclass is `class`.
    #[must_use]
    pub fn direct_subclasses<'a>(&'a self, class: &str) -> Vec<&'a str> {
        self.classes
            .values()
            .filter(|i| i.extends.as_deref() == Some(class))
            .map(|i| i.name.as_str())
            .collect()
    }
}

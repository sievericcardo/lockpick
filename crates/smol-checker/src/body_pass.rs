//! Body pass: type-check method bodies and the `main` block.
//!
//! Uses a simple scope-stack `VarEnv` (no `salsa` persistence here — the
//! incremental layer is at the declaration pass).

use std::collections::HashMap;
use smol_syntax::ast::{FieldDecl, MethodDecl, Type};
use smol_syntax::span::Span;

/// Resolved information about a single SMOL class, indexed by name.
#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub name:        String,
    /// Direct superclass name (already validated to exist).
    pub extends:     Option<String>,
    pub is_abstract: bool,
    /// Flattened field list (inherited fields first, own fields at the end).
    pub fields:      Vec<FieldDecl>,
    /// Own method declarations.
    pub methods:     Vec<MethodDecl>,
    /// `classifies` IRI, if any.
    pub classifies:  Option<String>,
    /// Definition span.
    pub span:        Span,
    /// Source file.
    pub file:        String,
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

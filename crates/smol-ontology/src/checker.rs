use std::collections::HashMap;

use smol_syntax::ast::File;
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

pub fn check_ontology(
    smol_files: &[File],
    ontology_paths: &[std::path::PathBuf],
    shacl_paths:   &[std::path::PathBuf],
    extra_prefixes: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    Vec::new()
}
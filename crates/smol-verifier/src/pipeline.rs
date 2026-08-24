//! Pipeline: tie together parsing, type-checking, and ontology checks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use smol_syntax::ast::File;
use smol_syntax::diagnostic::Diagnostic;

use crate::manifest;

/// Verify from a `smol.toml` manifest path.
///
/// `extra_prefixes` — CLI-supplied prefix map (`--prefix name=IRI`).
/// These take precedence over any `[prefixes]` section in `smol.toml`.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn run_manifest(
    manifest_path:  &Path,
    extra_prefixes: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    match manifest::load(manifest_path) {
        Err(diags) => diags,
        Ok(resolved) => {
            // Merge: extra_prefixes > manifest prefixes
            let mut merged = resolved.prefixes.clone();
            for (k, v) in extra_prefixes {
                merged.insert(k.clone(), v.clone());
            }
            run_files(
                &resolved.smol_files,
                &resolved.ontology_files,
                &resolved.shacl_files,
                &merged,
                &resolved.properties,
            )
        }
    }
}

/// Verify from explicit file lists.
///
/// `extra_prefixes` — caller-supplied prefix map (from CLI / manifest).
/// `properties` — CTL formulas to model-check (from the manifest, if any).
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn run_files(
    smol_paths:     &[PathBuf],
    ontology_paths: &[PathBuf],
    shacl_paths:    &[PathBuf],
    extra_prefixes: &HashMap<String, String>,
    properties:     &[String],
) -> Vec<Diagnostic> {
    let mut all_diags = Vec::new();

    let parse_results: Vec<(File, Vec<Diagnostic>)> = smol_paths
        .par_iter()
        .map(|path| {
            let src = match std::fs::read_to_string(path) {
                Ok(s)  => s,
                Err(e) => {
                    return (
                        File {
                            path:    path.to_string_lossy().into_owned(),
                            classes: Vec::new(),
                            main:    None,
                        },
                        vec![smol_syntax::diagnostic::Diagnostic::error(
                            smol_syntax::diagnostic::DiagnosticCode::FileReadFailed,
                            format!("cannot read `{}`: {e}", path.display()),
                        )],
                    );
                }
            };
            let path_str = path.to_string_lossy().into_owned();
            let (mut file, diags) = smol_parser::parse(&path_str, &src);
            file.path = path_str;
            (file, diags)
        })
        .collect();

    let (files, parse_diags): (Vec<File>, Vec<Vec<Diagnostic>>) =
        parse_results.into_iter().unzip();

    for pd in parse_diags {
        all_diags.extend(pd);
    }

    // Abort further checks if we have parse errors — AST may be incomplete.
    let had_parse_error = all_diags.iter().any(smol_syntax::diagnostic::Diagnostic::is_error);

    // Build model-check properties from the manifest formulas.
    let model_properties: Vec<smol_model::Property> = properties
        .iter()
        .map(|f| smol_model::Property {
            name: f.clone(),
            formula: f.clone(),
        })
        .collect();

    let (checker_diags, (onto_diags, model_diags)) = rayon::join(
        || smol_checker::check(&files),
        || {
            let onto = if had_parse_error {
                Vec::new()
            } else {
                smol_ontology::check_ontology(&files, ontology_paths, shacl_paths, extra_prefixes)
            };
            let model = if had_parse_error {
                Vec::new()
            } else {
                smol_model::check_model(&files, &model_properties)
            };
            (onto, model)
        },
    );

    all_diags.extend(checker_diags);
    all_diags.extend(onto_diags);
    all_diags.extend(model_diags);

    all_diags
}

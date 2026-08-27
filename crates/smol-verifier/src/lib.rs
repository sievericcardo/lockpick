//! `smol-verifier` — public API for the SMOL language verifier.
//!
//! # Usage
//!
//! ```rust,no_run
//! use std::path::Path;
//! use smol_verifier::verify_manifest;
//!
//! let diags = verify_manifest(Path::new("examples/greenhouse-adapt/smol.toml"));
//! let errors = diags.iter().filter(|d| d.is_error()).count();
//! println!("{errors} errors found");
//! ```

pub mod manifest;
pub mod pipeline;

pub use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode, Label, Severity};

/// Verify a SMOL project described by a `smol.toml` manifest file.
///
/// # Errors
///
/// Returns a non-empty [`Diagnostic`] list when errors or warnings are found.
/// Manifest / IO problems are also reported as `Diagnostic::error(...)`.
#[must_use]
pub fn verify_manifest(manifest: &std::path::Path) -> Vec<Diagnostic> {
    pipeline::run_manifest(manifest, &std::collections::HashMap::default())
}

/// Like [`verify_manifest`] but with additional SPARQL prefixes that take
/// precedence over the `[prefixes]` section in `smol.toml`.
///
/// Keys are prefix names (without colon), values are IRI strings.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn verify_manifest_with_prefixes(
    manifest: &std::path::Path,
    extra_prefixes: &std::collections::HashMap<String, String>,
) -> Vec<Diagnostic> {
    pipeline::run_manifest(manifest, extra_prefixes)
}

/// Verify a SMOL project from explicit file lists (no `smol.toml`).
///
/// `smol_files` — paths to `.smol` source files.
/// `ontology_files` — paths to `.ttl` / `.owx` ontology files.
#[must_use]
pub fn verify_files(
    smol_files: &[std::path::PathBuf],
    ontology_files: &[std::path::PathBuf],
) -> Vec<Diagnostic> {
    pipeline::run_files(
        smol_files,
        ontology_files,
        &[],
        &std::collections::HashMap::default(),
        &[],
        3,
    )
}

/// Like [`verify_files`] but with additional SPARQL prefixes.
///
/// Keys are prefix names (without colon), values are IRI strings.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn verify_files_with_prefixes(
    smol_files: &[std::path::PathBuf],
    ontology_files: &[std::path::PathBuf],
    extra_prefixes: &std::collections::HashMap<String, String>,
) -> Vec<Diagnostic> {
    pipeline::run_files(smol_files, ontology_files, &[], extra_prefixes, &[], 3)
}

/// Like [`verify_files`] but with CTL properties to model-check.
///
/// Each property is a CTL formula string (e.g. `"AG (Healthy || Unhealthy)"`).
#[must_use]
pub fn verify_files_with_properties(
    smol_files: &[std::path::PathBuf],
    ontology_files: &[std::path::PathBuf],
    properties: &[String],
) -> Vec<Diagnostic> {
    pipeline::run_files(
        smol_files,
        ontology_files,
        &[],
        &std::collections::HashMap::default(),
        properties,
        3,
    )
}

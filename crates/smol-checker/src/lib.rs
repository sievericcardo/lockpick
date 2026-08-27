//! `smol-checker` — declaration + body type-checking for SMOL programs.
//!
//! Exposes a single entry point:
//! ```rust,ignore
//! pub fn check(files: &[smol_syntax::ast::File]) -> Vec<Diagnostic>
//! ```

pub mod adapts;
pub mod body_pass;
pub mod class_env;
pub mod decl_pass;

use smol_syntax::ast::File;
use smol_syntax::diagnostic::Diagnostic;

/// Type-check a complete set of parsed SMOL files (from one project).
///
/// Runs the declaration pass first (building the `ClassEnv`), then the body
/// pass on each method body and the `main` block, and finally the `adapts`
/// modifier checks.
#[must_use]
pub fn check(files: &[File]) -> Vec<Diagnostic> {
    let (env, mut diags) = decl_pass::build_class_env(files);
    let body_diags = body_pass::check_bodies(files, &env);
    diags.extend(body_diags);
    let adapts_diags = adapts::check_adapts(files, &env);
    diags.extend(adapts_diags);
    diags
}

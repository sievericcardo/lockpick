//! `smol-checker` — multi-pass type checking and static analysis for SMOL programs.
//!
//! Exposes a single entry point:
//! ```rust,ignore
//! pub fn check(files: &[smol_syntax::ast::File]) -> Vec<Diagnostic>
//! ```
//!
//! ## Passes
//!
//! 1. **Declaration pass** — builds `ClassEnv`, validates class hierarchy, checks field/method signatures
//! 2. **Body pass** — type-checks method bodies and the `main` block
//! 3. **Adapts checking** — validates `adapts` modifiers on methods that invoke `adapt`/`classify`
//! 4. **Dead code** — warns on unused variables, parameters, and unreachable methods (E2015–E2017)
//! 5. **Null-safety** — flow-sensitive detection of definite null dereferences (E2018)
//! 6. **SPARQL taint** — tracks RDF-query taint in arguments to `access`/`construct` (W3016,
//!    inter-procedural summaries)
//! 7. **FMO lifecycle** — path-sensitive typestate: `FMO[…]` variables must be `destroy()`ed
//!    on *every* path (W2020)
//! 8. **Domain effects** — `domain`-field writes require the `domain` modifier; `hidden`
//!    fields must not leak into `links` fragments (E2019)
//! 9. **Symbolic execution** — interval abstract interpretation proving definite
//!    integer overflow (E2021), definite division-by-zero (E2022), and
//!    loop-termination side conditions (E5012, via `smol-model`)
//! 10. **Hoare contracts** — weakest-precondition VCs over JML `requires`/
//!    `ensures` comments discharged by a syntactic prover (E6001–E6004)

pub mod adapts;
pub mod body_pass;
pub mod class_env;
pub mod dead_code;
pub mod decl_pass;
pub mod domain_effects;
pub mod hoare;
pub mod null_safety;
pub mod resource_lifecycle;
pub mod sparql_taint;
pub mod symex;

use smol_syntax::ast::File;
use smol_syntax::diagnostic::Diagnostic;

/// Type-check a complete set of parsed SMOL files (from one project).
///
/// Runs the declaration pass first (building the `ClassEnv`), then the body
/// pass, adapts modifier checks, and finally all static-analysis passes
/// (dead code, null-safety, SPARQL taint, FMO lifecycle).
#[must_use]
pub fn check(files: &[File]) -> Vec<Diagnostic> {
    let (env, mut diags) = decl_pass::build_class_env(files);
    diags.extend(body_pass::check_bodies(files, &env));
    diags.extend(adapts::check_adapts(files, &env));
    diags.extend(dead_code::check_dead_code(files, &env));
    diags.extend(null_safety::check_null_safety(files, &env));
    diags.extend(sparql_taint::check_sparql_taint(files, &env));
    diags.extend(resource_lifecycle::check_fmo_lifecycle(files, &env));
    diags.extend(domain_effects::check_domain_effects(files, &env));
    diags.extend(symex::check_symex(files, &env));
    diags.extend(hoare::check_hoare(files, &env));
    diags
}

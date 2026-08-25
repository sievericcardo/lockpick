//! `smol-ontology` — OWL reasoning checks for SMOL programs.
//!
//! All oxidowl async calls are wrapped in a one-shot `tokio::runtime::Runtime`
//! so callers of this crate stay fully synchronous.
//!
//! Checks performed:
//! 1. Load ontology files; verify consistency.
//! 2. Per `classifies "<IRI>"`: verify the IRI is an `owl:Class`.
//! 3. Per `links "..."`: verify each predicate IRI exists in the ontology.
//! 4. Per `construct`/`access`/`retrieves` SPARQL literal: parse-validate with
//!    an empty store (catches syntax errors + unknown prefixes).
//! 5. Per SPARQL query (`construct`/`retrieves`/`classifies…SPARQL`): every
//!    element referenced must be present in the loaded ontology (when one was
//!    submitted). `prog:`-prefixed elements are runtime-only and skipped;
//!    unresolvable prefixes produce a warning (they may be injected at runtime).
//! 5b. Ontology-data sanity: predicates that are near-duplicates of another
//!     predicate (e.g. `odrl:assignero` next to `odrl:assigner`) trigger a
//!     typo warning.
//! 6. SHACL shapes validation for shape graphs provided in `smol.toml`.

pub mod checker;

pub use checker::check_ontology;

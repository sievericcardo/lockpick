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
//! Lockpick: A Rust library for verification of SMOL programs
pub use smol_verifier::{
    // Re-export key functions
    verify_manifest,
    verify_manifest_with_prefixes,
    verify_files,
    verify_files_with_prefixes,
    verify_files_with_properties,
    // Re-export types
    Diagnostic,
    DiagnosticCode,
    Label,
    Severity,
};

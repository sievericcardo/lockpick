//! Lockpick: A Rust library for verification of SMOL programs
pub use smol_verifier::{
    // Re-export types
    Diagnostic,
    DiagnosticCode,
    Label,
    Severity,
    verify_files,
    verify_files_with_prefixes,
    verify_files_with_properties,
    // In-memory verification (no disk reads for source files)
    verify_sources,
    verify_sources_with_prefixes,
    verify_sources_with_properties,
    // Re-export key functions
    verify_manifest,
    verify_manifest_with_prefixes,
};

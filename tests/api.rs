//! End-to-end tests for the `lockpick` library API.
//!
//! Run with: `cargo test -p lockpick`

use std::time::{SystemTime, UNIX_EPOCH};

use lockpick::{verify_files, verify_sources, DiagnosticCode};

/// Write `content` to a uniquely-named temp file (pid + timestamp suffix) so
/// parallel test runs never collide on the same path.
fn write_tmp(base: &str, content: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    let p = std::env::temp_dir().join(format!(
        "lockpick_{}_{}_{base}",
        std::process::id(),
        nanos
    ));
    std::fs::write(&p, content).unwrap();
    p
}

/// The in-memory `verify_sources` entry point must agree with `verify_files`
/// for the same source text (parity check).
#[test]
fn verify_sources_matches_verify_files() {
    let src = r#"
class Foo extends NonExistentParent ()
end
"#;
    let p = write_tmp("lockpick_sources_parity.smol", src);

    let from_files = verify_files(&[p.clone()], &[]);
    let from_sources = verify_sources(&[(p.clone(), src.to_string())], &[]);
    let _ = std::fs::remove_file(&p);

    assert_eq!(from_files.len(), from_sources.len());
    let codes_f: Vec<DiagnosticCode> = from_files.iter().map(|d| d.code).collect();
    let codes_s: Vec<DiagnosticCode> = from_sources.iter().map(|d| d.code).collect();
    assert_eq!(codes_f, codes_s);
    assert!(
        from_sources
            .iter()
            .any(|d| d.code == DiagnosticCode::UnknownClass),
        "expected UnknownClass from verify_sources, got: {from_sources:?}"
    );
}

/// `Diagnostic` values are serde-serializable, so `--format json` and the LSP
/// bridge can round-trip them.
#[test]
fn diagnostics_are_serde_serializable() {
    let src = "class A () end\nclass A () end\n"; // duplicate class
    let p = write_tmp("lockpick_sources_serde.smol", src);
    let diags = verify_sources(&[(p.clone(), src.to_string())], &[]);
    let _ = std::fs::remove_file(&p);

    assert!(
        diags.iter().any(|d| d.code == DiagnosticCode::DuplicateClass),
        "expected DuplicateClass, got: {diags:?}"
    );

    let json = serde_json::to_string(&diags).expect("serialize");
    let reparsed: Vec<lockpick::Diagnostic> = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(reparsed.len(), diags.len());
    assert_eq!(reparsed[0].code, diags[0].code);
}
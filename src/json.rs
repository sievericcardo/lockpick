//! Machine-readable JSON serialization of verification results.
//!
//! Used by the `check --format json` CLI mode.  The contract mirrors the
//! [`smol_verifier::Diagnostic`] structure while normalizing `Severity` to a
//! lowercase string and exposing the numeric `DiagnosticCode` value, so that
//! consumers can both display (`code`) and match on (`code_number`) diagnostics.

use serde::Serialize;
use smol_verifier::{Diagnostic, Severity};

/// Top-level JSON document written to stdout in `--format json` mode.
#[derive(Serialize)]
pub struct JsonOutput {
    pub diagnostics: Vec<JsonDiagnostic>,
    pub error_count: usize,
    pub warning_count: usize,
    pub note_count: usize,
}

impl JsonOutput {
    /// Build the JSON document from a verifier diagnostic list.
    #[must_use]
    pub fn from_diagnostics(diags: &[Diagnostic]) -> Self {
        let error_count = diags.iter().filter(|d| d.severity == Severity::Error).count();
        let warning_count = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        let note_count = diags.iter().filter(|d| d.severity == Severity::Note).count();
        let diagnostics = diags.iter().map(JsonDiagnostic::from).collect();
        Self {
            diagnostics,
            error_count,
            warning_count,
            note_count,
        }
    }
}

/// A single diagnostic in the JSON contract.
#[derive(Serialize)]
pub struct JsonDiagnostic {
    /// Lowercase severity: `"error" | "warning" | "note"`.
    pub severity: &'static str,
    /// Display form, e.g. `"E2004"`.
    pub code: String,
    /// Numeric value of the `DiagnosticCode` variant, e.g. `2004`.
    pub code_number: u16,
    pub message: String,
    /// Source file path; `null` when absent.
    pub file: Option<String>,
    /// Primary byte-offset span; `null` when absent.
    pub span: Option<JsonSpan>,
    /// Secondary labels (may be empty).
    pub labels: Vec<JsonLabel>,
}

impl From<&Diagnostic> for JsonDiagnostic {
    fn from(d: &Diagnostic) -> Self {
        let severity = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        Self {
            severity,
            code: d.code.to_string(),
            code_number: d.code as u16,
            message: d.message.clone(),
            file: d.file.clone(),
            span: d.span.map(|s| JsonSpan {
                start: s.start,
                end: s.end,
            }),
            labels: d
                .labels
                .iter()
                .map(|l| JsonLabel {
                    message: l.message.clone(),
                    file: l.file.clone(),
                    span: l.span.map(|s| JsonSpan {
                        start: s.start,
                        end: s.end,
                    }),
                })
                .collect(),
        }
    }
}

/// A byte-offset span `{ "start": usize, "end": usize }`.
#[derive(Serialize)]
pub struct JsonSpan {
    pub start: usize,
    pub end: usize,
}

/// A secondary label attached to a diagnostic.
#[derive(Serialize)]
pub struct JsonLabel {
    pub message: String,
    pub file: Option<String>,
    pub span: Option<JsonSpan>,
}
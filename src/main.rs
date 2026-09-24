//! `lockpick` — CLI for the SMOL language verifier.
//!
//! Usage:
//!   lockpick check [--manifest smol.toml]
//!   lockpick check file1.smol file2.smol [--ontology ont.ttl]
//!   lockpick check --files file1.smol --format json

mod json;

use std::path::PathBuf;
use std::process;

use ariadne::{Color, Label, Report, ReportKind, Source};
use clap::{Parser, Subcommand};
use smol_verifier::{Diagnostic, Severity};

// ─── CLI definition ───────────────────────────────────────────────────────── //

#[derive(Parser)]
#[command(
    name    = "lockpick",
    version,
    about   = "SMOL language verifier — type checker + OWL reasoning",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// Type-check and verify a SMOL project. Use `--format json` for machine-readable output.
    Check {
        /// Path to the `smol.toml` manifest file.
        #[arg(short, long, default_value = "smol.toml")]
        manifest: PathBuf,

        /// Additional `.smol` source files (instead of / in addition to manifest).
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,

        /// Additional `.smol` source files; alias for the positional `FILE` arguments.
        #[arg(long = "files", value_name = "FILE")]
        explicit_files: Vec<PathBuf>,

        /// Additional ontology files (`.ttl`, `.owx`).
        #[arg(short, long = "ontology", value_name = "ONT_FILE")]
        ontologies: Vec<PathBuf>,

        /// Do not use a `smol.toml` manifest; only use `--files` and `--ontologies`.
        #[arg(long)]
        no_manifest: bool,

        /// Extra SPARQL prefix declaration (repeatable).
        ///
        /// Format: `NAME=IRI`, e.g. `--prefix domain=http://example.org/domain/`
        /// These take precedence over `[prefixes]` in `smol.toml`.
        #[arg(long = "prefix", value_name = "NAME=IRI")]
        prefixes: Vec<String>,

        /// Output format: human-readable ariadne reports (`text`, default) or a
        /// single JSON document on stdout (`json`). The summary line always goes
        /// to stderr and the exit code is the same in both modes.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
}

// ─── Entry point ──────────────────────────────────────────────────────────── //

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Check {
            manifest,
            files,
            explicit_files,
            ontologies,
            no_manifest,
            prefixes,
            format,
        } => {
            // Parse `--prefix NAME=IRI` arguments into a HashMap.
            let mut extra_prefixes = std::collections::HashMap::new();
            for raw in &prefixes {
                match raw.split_once('=') {
                    Some((name, iri)) => {
                        extra_prefixes.insert(name.to_owned(), iri.to_owned());
                    }
                    None => {
                        eprintln!(
                            "warning: ignoring malformed --prefix value `{raw}` \
                             (expected NAME=IRI)"
                        );
                    }
                }
            }

            // Merge positional `FILE` arguments and the `--files` alias.
            let mut files = files;
            files.extend(explicit_files);

            let diags = if no_manifest || !files.is_empty() {
                smol_verifier::verify_files_with_prefixes(&files, &ontologies, &extra_prefixes)
            } else {
                smol_verifier::verify_manifest_with_prefixes(&manifest, &extra_prefixes)
            };

            let error_count = diags.iter().filter(|d| d.is_error()).count();
            let warning_count = diags
                .iter()
                .filter(|d| d.severity == Severity::Warning)
                .count();

            match format {
                OutputFormat::Text => print_diagnostics(&diags),
                OutputFormat::Json => {
                    let output = json::JsonOutput::from_diagnostics(&diags);
                    println!(
                        "{}",
                        serde_json::to_string(&output)
                            .expect("serializing JSON output must not fail")
                    );
                }
            }

            if error_count > 0 {
                eprintln!("\n{error_count} error(s), {warning_count} warning(s)");
                process::exit(1);
            } else if warning_count > 0 {
                eprintln!("\n0 errors, {warning_count} warning(s)");
                process::exit(0);
            } else {
                eprintln!("\nNo issues found.");
                process::exit(0);
            }
        }
    }
}

// ─── ariadne-powered diagnostic output ───────────────────────────────────── //

fn print_diagnostics(diags: &[Diagnostic]) {
    // Group diagnostics by file; diagnostics without a file use "<unknown>".
    for diag in diags {
        let file_id = diag.file.as_deref().unwrap_or("<unknown>");
        let kind = match diag.severity {
            Severity::Error => ReportKind::Error,
            Severity::Warning => ReportKind::Warning,
            Severity::Note => ReportKind::Advice,
        };

        // Read source for span-accurate reporting, falling back to empty source.
        let src = if file_id == "<unknown>" {
            String::new()
        } else {
            std::fs::read_to_string(file_id).unwrap_or_default()
        };

        let span_range = diag
            .span
            .map(|s| s.start..s.end)
            .unwrap_or(0..src.len().min(1));

        let mut report = Report::build(kind, (file_id, span_range.clone()))
            .with_code(diag.code.to_string())
            .with_message(&diag.message);

        report = report.with_label(
            Label::new((file_id, span_range))
                .with_message(diag.message.clone())
                .with_color(if diag.is_error() {
                    Color::Red
                } else {
                    Color::Yellow
                }),
        );

        for label in &diag.labels {
            if let (Some(lfile), Some(lspan)) = (&label.file, &label.span) {
                let _lsrc = std::fs::read_to_string(lfile).unwrap_or_default();
                let lrange = lspan.start..lspan.end;
                report = report.with_label(
                    Label::new((lfile.as_str(), lrange))
                        .with_message(&label.message)
                        .with_color(Color::Cyan),
                );
            } else {
                report = report.with_note(&label.message);
            }
        }

        let cache = (file_id, Source::from(src));
        let _ = report.finish().eprint(cache);
    }
}
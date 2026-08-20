use std::collections::HashMap;

use oxidowl::{OntologyFormat, Reasoner, ReasonerConfig, ShaclValidator};
use oxidowl::query::SparqlStore;

use smol_syntax::ast::File;
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;
use std::fmt::Write;


// oxidowl prints progress messages to stdout/stderr during ontology loading
// and reasoning.  We silence them when running as a library by redirecting
// both file descriptors to /dev/null for the duration of each call and then
// restoring them immediately afterwards.

/// Run `f` with stdout and stderr redirected to `/dev/null`.
/// On non-Unix platforms the closure is called as-is.
fn with_suppressed_output<T, F: FnOnce() -> T>(f: F) -> T {
    #[cfg(unix)]
    {
        use libc::{close, dup, dup2, open, O_WRONLY, STDERR_FILENO, STDOUT_FILENO};
        use std::ffi::CString;
        unsafe {
            let devnull = CString::new("/dev/null").unwrap();
            let null_fd = open(devnull.as_ptr(), O_WRONLY);
            let saved_out = dup(STDOUT_FILENO);
            let saved_err = dup(STDERR_FILENO);
            if null_fd >= 0 {
                dup2(null_fd, STDOUT_FILENO);
                dup2(null_fd, STDERR_FILENO);
                close(null_fd);
            }
            let result = f();
            if saved_out >= 0 { dup2(saved_out, STDOUT_FILENO); close(saved_out); }
            if saved_err >= 0 { dup2(saved_err, STDERR_FILENO); close(saved_err); }
            result
        }
    }
    #[cfg(not(unix))]
    { f() }
}

/// Run all ontology-level checks for a set of SMOL source files and ontology
/// paths (already resolved to absolute paths from the manifest).
///
/// `extra_prefixes` — caller-supplied prefix map (from CLI / manifest).
/// Keys are prefix names (without colon), values are IRI strings.
/// Precedence: `extra_prefixes` > TTL-extracted prefixes > built-ins.
///
/// Returns a list of [`Diagnostic`]s.  Never panics.
pub fn check_ontology(
    smol_files: &[File],
    ontology_paths: &[std::path::PathBuf],
    shacl_paths:   &[std::path::PathBuf],
    extra_prefixes: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Load ontology into a Reasoner
    let config  = ReasonerConfig::default();
    let mut reasoner = match with_suppressed_output(|| Reasoner::new(config)) {
        Ok(r)  => r,
        Err(e) => {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::OntologyLoadFailed,
                    format!("failed to create reasoner: {e}"),
                ),
            );
            return diags;
        }
    };

    for path in ontology_paths {
        let path_str = path.to_string_lossy();
        if let Err(e) = with_suppressed_output(|| reasoner.load_ontology_from_file(path_str.as_ref(), OntologyFormat::Auto)) {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::OntologyLoadFailed,
                    format!("failed to load ontology `{path_str}`: {e}"),
                ),
            );
        }
    }

    // Consistency check
    match with_suppressed_output(|| reasoner.is_consistent()) {
        Ok(true)  => {}
        Ok(false) => {
            diags.push(Diagnostic::error(
                DiagnosticCode::OntologyInconsistent,
                "loaded ontology is inconsistent (unsatisfiable)".to_owned(),
            ));
        }
        Err(e) => {
            diags.push(Diagnostic::error(
                DiagnosticCode::OntologyLoadFailed,
                format!("consistency check failed: {e}"),
            ));
        }
    }

    // Build SPARQL store for IRI/SPARQL checks
    let _sparql_store = match with_suppressed_output(SparqlStore::new) {
        Ok(s)  => s,
        Err(e) => {
            diags.push(Diagnostic::error(
                DiagnosticCode::OntologyLoadFailed,
                format!("failed to create SPARQL store: {e}"),
            ));
            return diags;
        }
    };

    // Load ontologies into the SPARQL store
    let mut ont_store = match with_suppressed_output(SparqlStore::new) {
        Ok(s)  => s,
        Err(e) => {
            diags.push(Diagnostic::error(
                DiagnosticCode::OntologyLoadFailed,
                format!("failed to create ontology SPARQL store: {e}"),
            ));
            return diags;
        }
    };
    // Collect TTL content and extract prefixes while loading.
    let mut ttl_prefixes: HashMap<String, String> = HashMap::new();
    for path in ontology_paths {
        if let Ok(content) = std::fs::read_to_string(path) {
            let extracted = extract_ttl_prefixes(&content);
            for (k, v) in extracted {
                ttl_prefixes.entry(k).or_insert(v);
            }
            let _ = with_suppressed_output(|| ont_store.load_turtle(&content)); // best-effort
        }
    }

    // Build merged prefix map: extra_prefixes > ttl_prefixes > built-ins
    let prefix_map = build_prefix_map(extra_prefixes, &ttl_prefixes);

    // Per-file annotation checks
    for file in smol_files {
        for cls in &file.classes {
            // classifies "<IRI>" — IRI / Manchester / SPARQL
            if let Some(ann) = &cls.classifies {
                check_classifies_annotation(&ann.iri, &ont_store, &prefix_map, file, ann.span, &mut diags);
            }

            // retrieves "SPARQL" — must be a valid SELECT query
            if let Some(ann) = &cls.retrieves {
                check_retrieves_sparql(&ann.sparql, file, ann.span, &prefix_map, &mut diags);
            }

            // links "turtle" — validate Turtle triples syntax
            check_links(&cls.links, &prefix_map, file, &mut diags);

            // Method-level SPARQL in construct/access bodies
            for method in &cls.methods {
                check_method_sparql(
                    &method.body,
                    file,
                    &prefix_map,
                    &mut diags,
                );
            }
        }

        if let Some(stmts) = &file.main {
            check_stmts_sparql(stmts, file, &prefix_map, &mut diags);
        }
    }

    // SHACL validation
    for shapes_path in shacl_paths {
        let shapes_ttl = match std::fs::read_to_string(shapes_path) {
            Ok(s)  => s,
            Err(e) => {
                diags.push(Diagnostic::error(
                    DiagnosticCode::FileReadFailed,
                    format!("cannot read SHACL shapes file `{}`: {e}", shapes_path.display()),
                ));
                continue;
            }
        };

        // Validate against an empty data graph (structural check of shapes).
        match with_suppressed_output(|| ShaclValidator::new(&shapes_ttl, "")) {
            Ok(mut v) => {
                if let Ok(report) = v.validate() {
                    if !report.conforms {
                        for violation in &report.results {
                            let msg = violation.messages.first()
                                .map_or("(no message)", |m| m.value.as_str());
                            diags.push(Diagnostic::error(
                                DiagnosticCode::ShaclViolation,
                                format!(
                                    "SHACL violation in `{}`: {}",
                                    shapes_path.display(),
                                    msg
                                ),
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                diags.push(Diagnostic::error(
                    DiagnosticCode::OntologyLoadFailed,
                    format!("failed to load SHACL shapes `{}`: {e}", shapes_path.display()),
                ));
            }
        }
    }

    diags
}

/// Build the merged prefix map: extra > ttl-extracted > built-ins.
fn build_prefix_map(
    extra: &HashMap<String, String>,
    ttl: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    // Built-ins (lowest priority)
    map.insert("prog".into(),  "https://www.smolang.org/prog/".into());
    map.insert("rdf".into(),   "http://www.w3.org/1999/02/22-rdf-syntax-ns#".into());
    map.insert("rdfs".into(),  "http://www.w3.org/2000/01/rdf-schema#".into());
    map.insert("owl".into(),   "http://www.w3.org/2002/07/owl#".into());
    map.insert("xsd".into(),   "http://www.w3.org/2001/XMLSchema#".into());
    map.insert("smol".into(),  "https://www.smolang.org/smol/".into());
    map.insert("run".into(),   "https://www.smolang.org/run/".into());
    map.insert("domain".into(), "http://example.org/domain/".into());
    // TTL-extracted (medium priority)
    for (k, v) in ttl {
        map.insert(k.clone(), v.clone());
    }
    // Caller-supplied (highest priority)
    for (k, v) in extra {
        map.insert(k.clone(), v.clone());
    }
    map
}

/// Extract `@prefix name: <IRI> .` declarations from a Turtle string.
fn extract_ttl_prefixes(ttl: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in ttl.lines() {
        let trimmed = line.trim();
        // Handle both `@prefix` and SPARQL-style `PREFIX` declarations.
        let rest = if let Some(r) = trimmed.strip_prefix("@prefix") {
            r
        } else if let Some(_r) = trimmed.to_lowercase().strip_prefix("prefix") {
            // SPARQL BASE / PREFIX blocks in TTL are not standard, but be lenient.
            &trimmed[6..]
        } else {
            continue;
        };
        let rest = rest.trim();
        // rest: `name: <IRI> .`  or  `name: <IRI>`
        if let Some(colon_pos) = rest.find(':') {
            let prefix_name = rest[..colon_pos].trim();
            let after_colon = rest[colon_pos + 1..].trim();
            if let Some(iri_start) = after_colon.find('<') {
                if let Some(iri_end) = after_colon[iri_start..].find('>') {
                    let iri = &after_colon[iri_start + 1..iri_start + iri_end];
                    out.insert(prefix_name.to_owned(), iri.to_owned());
                }
            }
        }
    }
    out
}

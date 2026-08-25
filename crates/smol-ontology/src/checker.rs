use std::collections::{HashMap, HashSet};

use oxidowl::query::SparqlStore;
use oxidowl::semantics::RdfTerm;
use oxidowl::{OntologyFormat, Reasoner, ReasonerConfig, ShaclValidator};

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
        use libc::{O_WRONLY, STDERR_FILENO, STDOUT_FILENO, close, dup, dup2, open};
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
            if saved_out >= 0 {
                dup2(saved_out, STDOUT_FILENO);
                close(saved_out);
            }
            if saved_err >= 0 {
                dup2(saved_err, STDERR_FILENO);
                close(saved_err);
            }
            result
        }
    }
    #[cfg(not(unix))]
    {
        f()
    }
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
    shacl_paths: &[std::path::PathBuf],
    extra_prefixes: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Load ontology into a Reasoner
    let config = ReasonerConfig::default();
    let mut reasoner = match with_suppressed_output(|| Reasoner::new(config)) {
        Ok(r) => r,
        Err(e) => {
            diags.push(Diagnostic::error(
                DiagnosticCode::OntologyLoadFailed,
                format!("failed to create reasoner: {e}"),
            ));
            return diags;
        }
    };

    for path in ontology_paths {
        let path_str = path.to_string_lossy();
        if let Err(e) = with_suppressed_output(|| {
            reasoner.load_ontology_from_file(path_str.as_ref(), OntologyFormat::Auto)
        }) {
            diags.push(Diagnostic::error(
                DiagnosticCode::OntologyLoadFailed,
                format!("failed to load ontology `{path_str}`: {e}"),
            ));
        }
    }

    // Consistency check
    match with_suppressed_output(|| reasoner.is_consistent()) {
        Ok(true) => {}
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
        Ok(s) => s,
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
        Ok(s) => s,
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

    // Presence checks only make sense when at least one ontology was actually
    // submitted AND it yielded triples in the store.
    let ontology_loaded = !ontology_paths.is_empty() && ont_store_has_data(&ont_store);

    // Ontology-internal data sanity: predicates that are near-duplicates of
    // one another are usually typos (e.g. `odrl:assignero` vs `odrl:assigner`).
    if ontology_loaded {
        check_predicate_typos(&ont_store, &mut diags);
    }

    // Per-file annotation checks
    for file in smol_files {
        for cls in &file.classes {
            // classifies "<IRI>" — IRI / Manchester / SPARQL
            if let Some(ann) = &cls.classifies {
                check_classifies_annotation(
                    &ann.iri,
                    &ont_store,
                    &prefix_map,
                    ontology_loaded,
                    file,
                    ann.span,
                    &mut diags,
                );
            }

            // retrieves "SPARQL" — must be a valid SELECT query
            if let Some(ann) = &cls.retrieves {
                check_retrieves_sparql(
                    &ann.sparql,
                    &ont_store,
                    &prefix_map,
                    ontology_loaded,
                    file,
                    ann.span,
                    &mut diags,
                );
            }

            // links "turtle" — validate Turtle triples syntax
            check_links(&cls.links, &prefix_map, file, &mut diags);

            // Method-level SPARQL in construct/access bodies
            for method in &cls.methods {
                check_method_sparql(
                    &method.body,
                    &ont_store,
                    &prefix_map,
                    ontology_loaded,
                    file,
                    &mut diags,
                );
            }
        }

        if let Some(stmts) = &file.main {
            check_stmts_sparql(
                stmts,
                &ont_store,
                &prefix_map,
                ontology_loaded,
                file,
                &mut diags,
            );
        }
    }

    // SHACL validation
    for shapes_path in shacl_paths {
        let shapes_ttl = match std::fs::read_to_string(shapes_path) {
            Ok(s) => s,
            Err(e) => {
                diags.push(Diagnostic::error(
                    DiagnosticCode::FileReadFailed,
                    format!(
                        "cannot read SHACL shapes file `{}`: {e}",
                        shapes_path.display()
                    ),
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
                            let msg = violation
                                .messages
                                .first()
                                .map_or("(no message)", |m| m.value.as_str());
                            diags.push(Diagnostic::error(
                                DiagnosticCode::ShaclViolation,
                                format!("SHACL violation in `{}`: {}", shapes_path.display(), msg),
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                diags.push(Diagnostic::error(
                    DiagnosticCode::OntologyLoadFailed,
                    format!(
                        "failed to load SHACL shapes `{}`: {e}",
                        shapes_path.display()
                    ),
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
    map.insert("prog".into(), "https://www.smolang.org/prog/".into());
    map.insert(
        "rdf".into(),
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#".into(),
    );
    map.insert(
        "rdfs".into(),
        "http://www.w3.org/2000/01/rdf-schema#".into(),
    );
    map.insert("owl".into(), "http://www.w3.org/2002/07/owl#".into());
    map.insert("xsd".into(), "http://www.w3.org/2001/XMLSchema#".into());
    map.insert("smol".into(), "https://www.smolang.org/smol/".into());
    map.insert("run".into(), "https://www.smolang.org/run/".into());
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

/// Verify the `classifies` annotation — supports three forms:
/// 1. SPARQL SELECT or ASK query
/// 2. Manchester OWL class expression (e.g. `<domain:models> some <ast:Class>`)
/// 3. Plain IRI or prefixed name (e.g. `domain:Sensor`)
fn check_classifies_annotation(
    iri: &str,
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    ontology_loaded: bool,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    // Detect SPARQL queries: after stripping PREFIX/BASE prologue, starts with SELECT or ASK.
    if is_sparql_query(iri) {
        check_sparql_syntax(
            iri,
            "classifies",
            store,
            prefix_map,
            ontology_loaded,
            file,
            span,
            diags,
        );
        return;
    }

    // Manchester / IRI form: extract all angle-bracketed IRIs and validate each.
    let bracketed_iris = extract_angle_iris(iri);
    if !bracketed_iris.is_empty() {
        for bare_iri in bracketed_iris {
            let Some(full_iri) = resolve_iri(&bare_iri, prefix_map, file, span, diags) else {
                continue;
            };
            validate_iri_as_owl_class(&full_iri, store, file, span, diags);
        }
        return;
    }

    // No angle-bracketed IRIs — fall back to existing prefix / plain-name resolution.
    check_classifies_plain_iri(iri, store, prefix_map, file, span, diags);
}

/// Returns true if `s` looks like a SPARQL query (SELECT or ASK after prologue).
fn is_sparql_query(s: &str) -> bool {
    let after_prologue = strip_sparql_prologue(s);
    let trimmed = after_prologue.trim().to_uppercase();
    trimmed.starts_with("SELECT ") || trimmed.starts_with("ASK ")
}

/// Strip SPARQL PREFIX/BASE declarations and blank/comment lines from the
/// beginning of a query string, returning the remainder starting at the
/// first substantive line.
fn strip_sparql_prologue(s: &str) -> &str {
    let mut byte_pos = 0;
    for line in s.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_uppercase();
        if upper.starts_with("PREFIX ")
            || upper.starts_with("BASE ")
            || trimmed.starts_with('#')
            || trimmed.is_empty()
        {
            byte_pos += line.len() + 1; // +1 for the newline
        } else {
            break;
        }
    }
    if byte_pos >= s.len() {
        s
    } else {
        &s[byte_pos..]
    }
}

/// Extract all `<IRI>` segments from a Manchester-style expression, returning
/// just the bare IRI strings (without angle brackets).
fn extract_angle_iris(s: &str) -> Vec<String> {
    let mut iris = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
            if i < bytes.len() {
                iris.push(s[start..i].to_owned());
            }
        }
        i += 1;
    }
    iris
}

/// Resolve a plain or angle-bracketed IRI to its full form.
///
/// Handles three shapes:
/// - `<http://…>` — already a full IRI (brackets stripped).
/// - `prefix:Local` — expanded via `prefix_map`; emits an `UnknownPrefix`
///   warning when the prefix is not known.
/// - a bare local name — returned unchanged.
///
/// Returns `None` (after emitting a warning) when the prefix is unresolved.
fn resolve_iri<'a>(
    iri: &'a str,
    prefix_map: &HashMap<String, String>,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) -> Option<String> {
    // Already a full IRI in angle brackets: `<http://...>`
    if iri.starts_with('<') {
        return Some(iri.trim_matches(|c| c == '<' || c == '>').to_owned());
    }

    // Prefixed name: `domain:Sensor`
    if let Some(colon_pos) = iri.find(':') {
        let prefix = &iri[..colon_pos];
        let local = &iri[colon_pos + 1..];
        if let Some(base) = prefix_map.get(prefix) {
            return Some(format!("{base}{local}"));
        }
        diags.push(
            Diagnostic::warning(
                DiagnosticCode::UnknownPrefix,
                format!(
                    "unknown prefix `{prefix}:` in `classifies` annotation \
                     — add `--prefix {prefix}=<IRI>` or `[prefixes]` in smol.toml"
                ),
            )
            .with_file(&file.path)
            .with_span(span),
        );
        return None;
    }

    // Plain local name.
    Some(iri.to_owned())
}

/// Resolve a plain (non-angle-bracketed) IRI using prefix-map or local name.
fn check_classifies_plain_iri(
    iri: &str,
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    let Some(full) = resolve_iri(iri, prefix_map, file, span, diags) else {
        return;
    };
    validate_iri_as_owl_class(&full, store, file, span, diags);
}

/// Issue an error if `iri` is not declared as `owl:Class` in the store.
fn validate_iri_as_owl_class(
    iri: &str,
    store: &SparqlStore,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    let query = format!("ASK {{ <{iri}> a <http://www.w3.org/2002/07/owl#Class> }}");
    if let Ok(false) = store.execute_ask(&query) {
        diags.push(
            Diagnostic::error(
                DiagnosticCode::UnknownOntologyClass,
                format!("IRI `{iri}` is not declared as owl:Class in the ontology"),
            )
            .with_file(&file.path)
            .with_span(span),
        );
    }
}

/// Validate SPARQL syntax by running the query against an empty store.
/// Prepends `prefix_map` entries so abbreviated names in queries resolve.
/// Also verifies that every non-`prog:` element referenced by the query is
/// present in the loaded ontology (when one was submitted) and that every
/// prefix used by the query is resolvable.
#[allow(clippy::too_many_arguments)]
fn check_sparql_syntax(
    sparql: &str,
    context: &str,
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    ontology_loaded: bool,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    let normalised = replace_smol_placeholders(sparql);
    let preamble = build_sparql_preamble(prefix_map);
    let with_prefixes = format!("{preamble}{normalised}");
    let Ok(test_store) = with_suppressed_output(SparqlStore::new) else {
        return;
    };
    if let Err(e) = test_store.execute_select(&with_prefixes) {
        diags.push(
            Diagnostic::error(
                DiagnosticCode::SparqlSyntaxError,
                format!("{context} SPARQL syntax error: {e}"),
            )
            .with_file(&file.path)
            .with_span(span),
        );
    }

    check_sparql_elements(
        &normalised,
        context,
        store,
        prefix_map,
        ontology_loaded,
        file,
        span,
        diags,
    );
}

/// Validate `retrieves` SPARQL: must be a syntactically-valid SELECT query.
#[allow(clippy::too_many_arguments)]
fn check_retrieves_sparql(
    sparql: &str,
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    ontology_loaded: bool,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    let normalised = replace_smol_placeholders(sparql);

    // Enforce SELECT.
    if !is_select_query(&normalised) {
        diags.push(
            Diagnostic::error(
                DiagnosticCode::RetrievesNotSelect,
                "retrieves must be a SELECT SPARQL query".to_owned(),
            )
            .with_file(&file.path)
            .with_span(span),
        );
        return;
    }

    let preamble = build_sparql_preamble(prefix_map);
    let with_prefixes = format!("{preamble}{normalised}");
    let Ok(test_store) = with_suppressed_output(SparqlStore::new) else {
        return;
    };
    if let Err(e) = test_store.execute_select(&with_prefixes) {
        diags.push(
            Diagnostic::error(
                DiagnosticCode::SparqlSyntaxError,
                format!("retrieves SPARQL syntax error: {e}"),
            )
            .with_file(&file.path)
            .with_span(span),
        );
    }

    check_sparql_elements(
        &normalised,
        "retrieves",
        store,
        prefix_map,
        ontology_loaded,
        file,
        span,
        diags,
    );
}

/// Returns true if `s` (with placeholders already replaced) is a SELECT query,
/// after stripping any PREFIX/BASE prologue.
fn is_select_query(s: &str) -> bool {
    let after_prologue = strip_sparql_prologue(s);
    after_prologue.trim().to_uppercase().starts_with("SELECT ")
}

/// Validate `links` Turtle triples by wrapping with a blank-node subject,
/// prepending prefix declarations, and parsing as Turtle.
fn check_links(
    links: &[smol_syntax::ast::LinksAnnotation],
    prefix_map: &HashMap<String, String>,
    file: &File,
    diags: &mut Vec<Diagnostic>,
) {
    for link in links {
        let normalised = replace_smol_placeholders(&link.turtle);
        // Ensure the fragment ends with a dot so Turtle property lists terminate.
        let terminated = if normalised.trim_end().ends_with('.') {
            normalised
        } else {
            format!("{normalised} .")
        };
        let preamble = build_sparql_preamble(prefix_map);
        let wrapped = format!("{preamble}\n[] {terminated}");

        let Ok(mut test_store) = with_suppressed_output(SparqlStore::new) else {
            continue;
        };
        if let Err(e) = with_suppressed_output(|| test_store.load_turtle(&wrapped)) {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::LinkTurtleInvalid,
                    format!("links Turtle syntax error: {e}"),
                )
                .with_file(&file.path)
                .with_span(link.span),
            );
        }
    }
}

/// Build a SPARQL preamble string with standard prefixes + caller-supplied map.
fn build_sparql_preamble(prefix_map: &HashMap<String, String>) -> String {
    let mut preamble = String::from(
        "PREFIX prog:   <https://www.smolang.org/prog/>\n\
         PREFIX rdf:    <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
         PREFIX rdfs:   <http://www.w3.org/2000/01/rdf-schema#>\n\
         PREFIX owl:    <http://www.w3.org/2002/07/owl#>\n\
         PREFIX xsd:    <http://www.w3.org/2001/XMLSchema#>\n\
         PREFIX smol:   <https://www.smolang.org/smol/>\n\
         PREFIX run:    <https://www.smolang.org/run/>\n\
          PREFIX domain: <http://example.org/domain/>\n",
    );
    for (name, iri) in prefix_map {
        let skip = matches!(
            name.as_str(),
            "prog" | "rdf" | "rdfs" | "owl" | "xsd" | "smol" | "run" | "domain"
        );
        if !skip {
            let _ = writeln!(preamble, "PREFIX {name}: <{iri}>");
        }
    }
    preamble
}

/// Replace SMOL argument placeholders (`%1`, `%2`, …, `%N`) with SPARQL dummy
/// literals so the query can be parsed.
fn replace_smol_placeholders(sparql: &str) -> String {
    let mut result = sparql.to_owned();
    // Replace named placeholders (%word) and positional (%N).
    let re_pos = regex_lite::Regex::new(r"%(\d+)").unwrap();
    let re_named = regex_lite::Regex::new(r"%(\w+)").unwrap();
    result = re_pos
        .replace_all(&result, r#""__smol_arg__""#)
        .into_owned();
    result = re_named
        .replace_all(&result, r#""__smol_arg__""#)
        .into_owned();
    result
}

// Walk AST looking for Construct/Access/Retrieves expressions
fn check_method_sparql(
    stmts: &[smol_syntax::ast::Stmt],
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    ontology_loaded: bool,
    file: &File,
    diags: &mut Vec<Diagnostic>,
) {
    check_stmts_sparql(stmts, store, prefix_map, ontology_loaded, file, diags);
}

#[allow(clippy::match_same_arms)]
fn check_stmts_sparql(
    stmts: &[smol_syntax::ast::Stmt],
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    ontology_loaded: bool,
    file: &File,
    diags: &mut Vec<Diagnostic>,
) {
    use smol_syntax::ast::Stmt;
    for stmt in stmts {
        #[allow(clippy::match_same_arms)]
        match stmt {
            Stmt::VarDecl { init, .. } => {
                check_expr_sparql(init, store, prefix_map, ontology_loaded, file, diags)
            }
            Stmt::Assign { rhs, lhs, .. } => {
                check_expr_sparql(lhs, store, prefix_map, ontology_loaded, file, diags);
                check_expr_sparql(rhs, store, prefix_map, ontology_loaded, file, diags);
            }
            Stmt::Expr { expr, .. } => {
                check_expr_sparql(expr, store, prefix_map, ontology_loaded, file, diags)
            }
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    check_expr_sparql(e, store, prefix_map, ontology_loaded, file, diags);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                check_expr_sparql(cond, store, prefix_map, ontology_loaded, file, diags);
                check_stmts_sparql(then_body, store, prefix_map, ontology_loaded, file, diags);
                check_stmts_sparql(else_body, store, prefix_map, ontology_loaded, file, diags);
            }
            Stmt::While { cond, body, .. } => {
                check_expr_sparql(cond, store, prefix_map, ontology_loaded, file, diags);
                check_stmts_sparql(body, store, prefix_map, ontology_loaded, file, diags);
            }
            Stmt::Print { value, .. }
            | Stmt::Destroy { value, .. }
            | Stmt::Adapt { expr: value, .. } => {
                check_expr_sparql(value, store, prefix_map, ontology_loaded, file, diags);
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
}

#[allow(clippy::match_same_arms)]
fn check_expr_sparql(
    expr: &smol_syntax::ast::Expr,
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    ontology_loaded: bool,
    file: &File,
    diags: &mut Vec<Diagnostic>,
) {
    use smol_syntax::ast::Expr;
    match expr {
        Expr::Construct { sparql, args, span } => {
            check_sparql_syntax(
                sparql,
                "construct",
                store,
                prefix_map,
                ontology_loaded,
                file,
                *span,
                diags,
            );
            for arg in args {
                check_expr_sparql(arg, store, prefix_map, ontology_loaded, file, diags);
            }
        }
        Expr::Access {
            connector, args, ..
        } => {
            // access() accepts non-SPARQL query languages (e.g. InfluxDB Flux),
            // so the query string is intentionally not validated as SPARQL.
            if let Some(c) = connector {
                check_expr_sparql(c, store, prefix_map, ontology_loaded, file, diags);
            }
            for arg in args {
                check_expr_sparql(arg, store, prefix_map, ontology_loaded, file, diags);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            check_expr_sparql(receiver, store, prefix_map, ontology_loaded, file, diags);
            for a in args {
                check_expr_sparql(a, store, prefix_map, ontology_loaded, file, diags);
            }
        }
        Expr::BinOp { left, right, .. } => {
            check_expr_sparql(left, store, prefix_map, ontology_loaded, file, diags);
            check_expr_sparql(right, store, prefix_map, ontology_loaded, file, diags);
        }
        Expr::UnaryOp { expr: inner, .. } => {
            check_expr_sparql(inner, store, prefix_map, ontology_loaded, file, diags)
        }
        Expr::New { args, .. } => {
            for a in args {
                check_expr_sparql(a, store, prefix_map, ontology_loaded, file, diags);
            }
        }
        Expr::Field { receiver, .. } => {
            check_expr_sparql(receiver, store, prefix_map, ontology_loaded, file, diags)
        }
        Expr::Classify { expr: inner, .. } => {
            check_expr_sparql(inner, store, prefix_map, ontology_loaded, file, diags)
        }
        Expr::Simulate { args, .. } => {
            for a in args {
                check_expr_sparql(a, store, prefix_map, ontology_loaded, file, diags);
            }
        }
        Expr::Port { receiver, .. } => {
            check_expr_sparql(receiver, store, prefix_map, ontology_loaded, file, diags)
        }
        Expr::Super { args, .. } => {
            for a in args {
                check_expr_sparql(a, store, prefix_map, ontology_loaded, file, diags);
            }
        }
        // Member / Validate / literal / variable expressions: no SPARQL to check.
        _ => {}
    }
}

/// Ensure every RDF resource referenced by a SPARQL query is resolvable and
/// (when an ontology was submitted) actually present in it.
///
/// - `prog:`-prefixed elements are runtime-only and are skipped entirely.
/// - A prefixed name whose prefix is not in the merged prefix table triggers a
///   warning: such prefixes may legitimately be injected at runtime.
/// - Any remaining element must exist in the loaded ontology, otherwise an
///   error is emitted.
#[allow(clippy::too_many_arguments)]
fn check_sparql_elements(
    sparql: &str,
    context: &str,
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    ontology_loaded: bool,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    let (prefixed, full_iris) = extract_sparql_refs(sparql);

    let mut warned_prefixes: HashSet<String> = HashSet::new();
    let mut checked: HashSet<String> = HashSet::new();

    for (prefix, local) in prefixed {
        // prog:, smol:, domain: and run: are reserved for runtime-only elements.
        if prefix == "prog" || prefix == "smol" || prefix == "domain" || prefix == "run" {
            continue;
        }
        match prefix_map.get(prefix.as_str()) {
            None => {
                if warned_prefixes.insert(prefix.to_owned()) {
                    diags.push(
                        Diagnostic::warning(
                            DiagnosticCode::UnknownPrefix,
                            format!(
                                "{context}: prefix `{prefix}:` is not declared in the loaded \
                                 prefixes/ontology — it may be injected at runtime"
                            ),
                        )
                        .with_file(&file.path)
                        .with_span(span),
                    );
                }
            }
            Some(base) => {
                if !ontology_loaded {
                    continue;
                }
                let full = format!("{base}{local}");
                if checked.insert(full.clone()) && !element_in_store(store, &full) {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::SparqlElementMissing,
                            format!(
                                "{context}: `{prefix}:{local}` ({full}) is referenced by the \
                                 SPARQL query but is not present in the ontology"
                            ),
                        )
                        .with_file(&file.path)
                        .with_span(span),
                    );
                }
            }
        }
    }

    if !ontology_loaded {
        return;
    }

    const PROG_NS: &str = "https://www.smolang.org/prog/";
    for iri in full_iris {
        if iri.starts_with(PROG_NS) {
            continue;
        }
        if checked.insert(iri.clone()) && !element_in_store(store, &iri) {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::SparqlElementMissing,
                    format!(
                        "<{iri}> is referenced by the {context} SPARQL query but is \
                         not present in the ontology"
                    ),
                )
                .with_file(&file.path)
                .with_span(span),
            );
        }
    }
}

/// Whether the registry store holds at least one triple (i.e. an ontology was
/// actually loaded). Returns `false` on any store error.
fn ont_store_has_data(store: &SparqlStore) -> bool {
    store.execute_ask("ASK { ?s ?p ?o }").unwrap_or(false)
}

/// Flag ontology-data predicates that are near-duplicates of another predicate
/// (same namespace, local name identical apart from a single leading or
/// trailing character), e.g. `odrl:assignero` next to `odrl:assigner`.
///
/// These are almost always typos: at runtime a SPARQL filter on the correct
/// term silently matches nothing. Emits one warning per typo candidate.
fn check_predicate_typos(store: &SparqlStore, diags: &mut Vec<Diagnostic>) {
    let Ok(rows) =
        store.execute_select("SELECT DISTINCT ?p WHERE { ?s ?p ?o . FILTER(isIRI(?p)) }")
    else {
        return;
    };

    // Group predicate IRIs by namespace for a precise, cheap comparison.
    let mut by_namespace: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        if let Some(RdfTerm::Iri(url)) = row.get("p") {
            let iri = url.as_str();
            let ns_end = iri
                .rfind('/')
                .map_or(0, |i| i + 1)
                .max(iri.rfind('#').map_or(0, |i| i + 1));
            by_namespace
                .entry(iri[..ns_end].to_owned())
                .or_default()
                .push(iri.to_owned());
        }
    }

    let mut warned: HashSet<String> = HashSet::new();
    for (_, predicates) in by_namespace {
        for i in 0..predicates.len() {
            for j in i + 1..predicates.len() {
                let (longer, shorter) = match near_duplicate(&predicates[i], &predicates[j]) {
                    Some(pair) => pair,
                    None => continue,
                };
                if warned.insert(longer.to_owned()) {
                    diags.push(Diagnostic::warning(
                        DiagnosticCode::SuspiciousOntologyPredicate,
                        format!(
                            "ontology predicate `{longer}` is a near-duplicate of \
                             `{shorter}` (identical apart from one character) — \
                             this is usually a typo and would silently break \
                             SPARQL matching at runtime"
                        ),
                    ));
                }
            }
        }
    }
}

/// If two predicate IRIs are near-duplicates (same namespace, one local name
/// being the other plus a single leading/trailing character), return the pair
/// ordered as `(longer, shorter)`.
fn near_duplicate<'a>(a: &'a str, b: &'a str) -> Option<(&'a str, &'a str)> {
    let a_local = split_local(a);
    let b_local = split_local(b);
    if !is_one_char_extension(a_local, b_local) && !is_one_char_extension(b_local, a_local) {
        return None;
    }
    if a_local.len() > b_local.len() {
        Some((a, b))
    } else {
        Some((b, a))
    }
}

/// The local (final) segment of an IRI: everything after the last `/` or `#`.
fn split_local(iri: &str) -> &str {
    let cut = iri
        .rfind('/')
        .map_or(0, |i| i + 1)
        .max(iri.rfind('#').map_or(0, |i| i + 1));
    &iri[cut..]
}

/// `longer` is `shorter` plus exactly one leading or trailing character.
fn is_one_char_extension(longer: &str, shorter: &str) -> bool {
    longer.len() == shorter.len() + 1
        && !longer.is_empty()
        && (longer.starts_with(shorter) || longer.ends_with(shorter))
}

/// Whether `iri` occurs in any position of any triple in the loaded ontology.
///
/// Returns `true` on store errors so that a broken query/store does not
/// cascade into a pile of spurious "missing element" errors.
fn element_in_store(store: &SparqlStore, iri: &str) -> bool {
    let query = format!(
        "ASK {{ {{ <{iri}> ?p ?o }} UNION {{ ?s <{iri}> ?o }} UNION {{ ?s ?p <{iri}> }} }}"
    );
    store.execute_ask(&query).unwrap_or(true)
}

/// Extract the RDF resources referenced by a SPARQL query:
///
/// - `prefixed`: `(prefix, local)` pairs, e.g. `ast:Pump` or the default
///   namespace `:hasId` (reported with the empty prefix).
/// - `full_iris`: IRIs written in angle brackets, e.g. `<http://…>`.
///
/// PREFIX/BASE declaration IRIs, string literals, comments, blank nodes and
/// variables are skipped.
fn extract_sparql_refs(sparql: &str) -> (Vec<(String, String)>, Vec<String>) {
    let bytes = sparql.as_bytes();
    let mut i = 0;
    let mut prefixed = Vec::new();
    let mut full_iris = Vec::new();
    // Set while scanning a `PREFIX x:` / `BASE` declaration so the following
    // `<iri>` (which is a namespace declaration, not an element reference) is
    // not recorded.
    let mut skip_next_iri = false;

    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' | b'\'' => {
                i = skip_string(sparql, i);
            }
            b'<' => match iriref_end(sparql, i) {
                Some((content, next)) => {
                    if skip_next_iri {
                        skip_next_iri = false;
                    } else {
                        full_iris.push(content.to_owned());
                    }
                    i = next;
                }
                None => i += 1, // comparison operator `<` / `<=`
            },
            b'?' | b'_' => {
                // Variables (?x) and blank-node labels (_:b0).
                i += 1;
                while i < bytes.len() && is_pn_char(bytes[i]) {
                    i += 1;
                }
            }
            _ => {
                if is_pn_start(bytes[i]) || bytes[i] == b':' {
                    let start = i;
                    while i < bytes.len() && is_pn_char(bytes[i]) {
                        i += 1;
                    }
                    let token = &sparql[start..i];
                    if token.eq_ignore_ascii_case("PREFIX") || token.eq_ignore_ascii_case("BASE") {
                        skip_next_iri = true;
                    } else if let Some((prefix, local)) = split_prefixed_name(token) {
                        if local.is_empty() {
                            // `name:` — the prefix being declared in `PREFIX name: <iri>`
                            skip_next_iri = true;
                        } else {
                            prefixed.push((prefix.to_owned(), local.to_owned()));
                        }
                    }
                } else {
                    i += 1;
                }
            }
        }
    }

    (prefixed, full_iris)
}

/// Skip past a `'…'` / `"…"` (or triple-quoted) string literal starting at
/// `i`. Escapes are honoured. Returns the index just past the closing quote —
/// or end-of-input if the literal is unclosed.
fn skip_string(sparql: &str, i: usize) -> usize {
    let bytes = sparql.as_bytes();
    let quote = bytes[i];
    let triple = i + 2 < bytes.len() && bytes[i + 1] == quote && bytes[i + 2] == quote;
    let mut j = i + if triple { 3 } else { 1 };
    while j < bytes.len() {
        if bytes[j] == b'\\' {
            j += 2;
            continue;
        }
        if triple
            && j + 2 < bytes.len()
            && bytes[j] == quote
            && bytes[j + 1] == quote
            && bytes[j + 2] == quote
        {
            return j + 3;
        }
        if !triple && bytes[j] == quote {
            return j + 1;
        }
        if bytes[j] == b'\n' && !triple {
            return j + 1; // unterminated single-line literal
        }
        j += 1;
    }
    j
}

/// If `i` points at the start of a `<…>` IRI reference, return its contents
/// and the index just past the closing `>`. Returns `None` when the `<` is a
/// comparison operator (no `>` before whitespace / illegal IRI chars).
fn iriref_end(sparql: &str, i: usize) -> Option<(&str, usize)> {
    let bytes = sparql.as_bytes();
    let mut j = i + 1;
    while j < bytes.len() {
        let c = bytes[j];
        match c {
            b'>' if j > i + 1 => return Some((&sparql[i + 1..j], j + 1)),
            b'<' | b'"' | b'\'' | b' ' | b'\t' | b'\n' | b'\r' | b'{' | b'}' | b'|' | b'^'
            | b'`' | b'(' | b')' | b'[' | b']' | b',' | b';' | b'=' | b'\\' => return None,
            _ => j += 1,
        }
    }
    None
}

/// Split a prefixed-name token into `(prefix, local)`. The default namespace
/// (`:hasId`) yields the empty prefix. Returns `None` for anything that is not
/// a well-formed single prefixed name (e.g. `a:b:c` or a bare word).
fn split_prefixed_name(token: &str) -> Option<(&str, &str)> {
    let colon = token.find(':')?;
    let prefix = &token[..colon];
    let local = &token[colon + 1..];
    if local.contains(':') {
        return None;
    }
    if !(prefix.is_empty()
        || prefix.chars().next()?.is_ascii_alphabetic()
        || prefix.starts_with('_'))
    {
        return None;
    }
    Some((prefix, local))
}

fn is_pn_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b':'
}

fn is_pn_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b':' || c == b'-' || c == b'.'
}

// regex_lite helper (small inline regex, avoids pulling in full `regex`)
mod regex_lite {
    /// Trivial hand-rolled placeholder substitution without a regex crate.
    #[allow(dead_code)]
    pub struct Regex(String);

    impl Regex {
        #[allow(clippy::unnecessary_wraps)]
        pub fn new(pattern: &str) -> Result<Self, ()> {
            Ok(Self(pattern.to_owned()))
        }

        /// Replace `%N` (digits) or `%word` occurrences with `replacement`.
        #[allow(clippy::unused_self)]
        pub fn replace_all<'a>(
            &self,
            text: &'a str,
            replacement: &str,
        ) -> std::borrow::Cow<'a, str> {
            // Simple character-scan replacement for %N and %word patterns.
            let mut result = String::with_capacity(text.len());
            let bytes = text.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' && i + 1 < bytes.len() {
                    let rest = &text[i + 1..];
                    let end = rest
                        .find(|c: char| !c.is_alphanumeric() && c != '_')
                        .unwrap_or(rest.len());
                    if end > 0 {
                        result.push_str(replacement);
                        i += 1 + end;
                        continue;
                    }
                }
                result.push(bytes[i] as char);
                i += 1;
            }
            std::borrow::Cow::Owned(result)
        }
    }
}

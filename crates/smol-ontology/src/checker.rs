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

/// Verify the `classifies` annotation — supports three forms:
/// 1. SPARQL SELECT or ASK query
/// 2. Manchester OWL class expression (e.g. `<domain:models> some <ast:Class>`)
/// 3. Plain IRI or prefixed name (e.g. `domain:Sensor`)
fn check_classifies_annotation(
    iri: &str,
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    // Detect SPARQL queries: after stripping PREFIX/BASE prologue, starts with SELECT or ASK.
    if is_sparql_query(iri) {
        check_sparql_syntax(iri, "classifies", file, span, prefix_map, diags);
        return;
    }

    // Manchester / IRI form: extract all angle-bracketed IRIs and validate each.
    let bracketed_iris = extract_angle_iris(iri);
    if !bracketed_iris.is_empty() {
        for full_iri in bracketed_iris {
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
        if upper.starts_with("PREFIX ") || upper.starts_with("BASE ") || trimmed.starts_with('#') || trimmed.is_empty() {
            byte_pos += line.len() + 1; // +1 for the newline
        } else {
            break;
        }
    }
    if byte_pos >= s.len() { s } else { &s[byte_pos..] }
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

/// Resolve a plain (non-angle-bracketed) IRI using prefix-map or local name.
fn check_classifies_plain_iri(
    iri: &str,
    store: &SparqlStore,
    prefix_map: &HashMap<String, String>,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    // Already a full IRI in angle brackets: `<http://...>`
    if iri.starts_with('<') {
        let full = iri.trim_matches(|c| c == '<' || c == '>');
        validate_iri_as_owl_class(full, store, file, span, diags);
        return;
    }

    // Prefixed name: `domain:Sensor`
    if let Some(colon_pos) = iri.find(':') {
        let prefix = &iri[..colon_pos];
        let local  = &iri[colon_pos + 1..];
        if let Some(base) = prefix_map.get(prefix) {
            let full = format!("{base}{local}");
            validate_iri_as_owl_class(&full, store, file, span, diags);
        } else {
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
        }
        return;
    }

    // Plain local name — validate directly.
    validate_iri_as_owl_class(iri, store, file, span, diags);
}

/// Issue an error if `iri` is not declared as `owl:Class` in the store.
fn validate_iri_as_owl_class(
    iri: &str,
    store: &SparqlStore,
    file: &File,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    let query = format!(
        "ASK {{ <{iri}> a <http://www.w3.org/2002/07/owl#Class> }}"
    );
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
fn check_sparql_syntax(
    sparql: &str,
    context: &str,
    file: &File,
    span: Span,
    prefix_map: &HashMap<String, String>,
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
}

/// Validate `retrieves` SPARQL: must be a syntactically-valid SELECT query.
fn check_retrieves_sparql(
    sparql: &str,
    file: &File,
    span: Span,
    prefix_map: &HashMap<String, String>,
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
    let re_pos   = regex_lite::Regex::new(r"%(\d+)").unwrap();
    let re_named = regex_lite::Regex::new(r"%(\w+)").unwrap();
    result = re_pos  .replace_all(&result, r#""__smol_arg__""#).into_owned();
    result = re_named.replace_all(&result, r#""__smol_arg__""#).into_owned();
    result
}

// Walk AST looking for Construct/Access/Retrieves expressions
fn check_method_sparql(
    stmts: &[smol_syntax::ast::Stmt],
    file: &File,
    prefix_map: &HashMap<String, String>,
    diags: &mut Vec<Diagnostic>,
) {
    check_stmts_sparql(stmts, file, prefix_map, diags);
}

#[allow(clippy::match_same_arms)]
fn check_stmts_sparql(
    stmts: &[smol_syntax::ast::Stmt],
    file: &File,
    prefix_map: &HashMap<String, String>,
    diags: &mut Vec<Diagnostic>,
) {
    use smol_syntax::ast::Stmt;
    for stmt in stmts {
        #[allow(clippy::match_same_arms)]
        match stmt {
            Stmt::VarDecl { init, .. }     => check_expr_sparql(init, file, prefix_map, diags),
            Stmt::Assign  { rhs, lhs, .. } => {
                check_expr_sparql(lhs, file, prefix_map, diags);
                check_expr_sparql(rhs, file, prefix_map, diags);
            }
            Stmt::Expr    { expr, .. }     => check_expr_sparql(expr, file, prefix_map, diags),
            Stmt::Return  { value, .. }    => {
                if let Some(e) = value { check_expr_sparql(e, file, prefix_map, diags); }
            }
            Stmt::If      { cond, then_body, else_body, .. } => {
                check_expr_sparql(cond, file, prefix_map, diags);
                check_stmts_sparql(then_body, file, prefix_map, diags);
                check_stmts_sparql(else_body, file, prefix_map, diags);
            }
            Stmt::While   { cond, body, .. } => {
                check_expr_sparql(cond, file, prefix_map, diags);
                check_stmts_sparql(body, file, prefix_map, diags);
            }
            Stmt::Print { value, .. }
            | Stmt::Destroy { value, .. }
            | Stmt::Adapt { expr: value, .. } => {
                check_expr_sparql(value, file, prefix_map, diags);
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
}

#[allow(clippy::match_same_arms)]
fn check_expr_sparql(
    expr: &smol_syntax::ast::Expr,
    file: &File,
    prefix_map: &HashMap<String, String>,
    diags: &mut Vec<Diagnostic>,
) {
    use smol_syntax::ast::Expr;
    match expr {
        Expr::Construct { sparql, args, span } => {
            check_sparql_syntax(sparql, "construct", file, *span, prefix_map, diags);
            for arg in args { check_expr_sparql(arg, file, prefix_map, diags); }
        }
        Expr::Access { connector, args, .. } => {
            // access() accepts non-SPARQL query languages (e.g. InfluxDB Flux),
            // so the query string is intentionally not validated as SPARQL.
            if let Some(c) = connector { check_expr_sparql(c, file, prefix_map, diags); }
            for arg in args { check_expr_sparql(arg, file, prefix_map, diags); }
        }
        Expr::MethodCall { receiver, args, .. } => {
            check_expr_sparql(receiver, file, prefix_map, diags);
            for a in args { check_expr_sparql(a, file, prefix_map, diags); }
        }
        Expr::BinOp { left, right, .. } => {
            check_expr_sparql(left,  file, prefix_map, diags);
            check_expr_sparql(right, file, prefix_map, diags);
        }
        Expr::UnaryOp { expr: inner, .. }   => check_expr_sparql(inner, file, prefix_map, diags),
        Expr::New      { args, .. }          => { for a in args { check_expr_sparql(a, file, prefix_map, diags); } }
        Expr::Field    { receiver, .. }      => check_expr_sparql(receiver, file, prefix_map, diags),
        Expr::Classify { expr: inner, .. }   => check_expr_sparql(inner, file, prefix_map, diags),
        Expr::Simulate { args, .. }          => { for a in args { check_expr_sparql(a, file, prefix_map, diags); } }
        Expr::Port     { receiver, .. }      => check_expr_sparql(receiver, file, prefix_map, diags),
        Expr::Super    { args, .. }          => { for a in args { check_expr_sparql(a, file, prefix_map, diags); } }
        // Member / Validate / literal / variable expressions: no SPARQL to check.
        _ => {}
    }
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
        pub fn replace_all<'a>(&self, text: &'a str, replacement: &str) -> std::borrow::Cow<'a, str> {
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

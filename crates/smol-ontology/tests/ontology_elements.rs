//! Tests for SPARQL element-presence checks (E3013) and prefix warnings (E3010).

use std::collections::HashMap;
use std::path::PathBuf;

use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

fn parse(src: &str) -> Vec<smol_syntax::ast::File> {
    let (file, _) = smol_parser::parse("<test>.smol", src);
    vec![file]
}

fn write_tmp(name: &str, content: &str) -> PathBuf {
    let p = std::env::temp_dir().join(name);
    std::fs::write(&p, content).unwrap();
    p
}

fn has_code(diags: &[Diagnostic], code: DiagnosticCode) -> bool {
    diags.iter().any(|d| d.code == code)
}

const ONT: &str = r#"
@prefix : <http://example.org/domain/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
:device rdf:type owl:Class .
:hasId rdf:type owl:DatatypeProperty .
"#;

const SRC_OK: &str = r#"
class Sensor ()
    Unit read()
        List<Object> xs = construct("
            SELECT ?id {
                ?x :hasId ?id .
            }");
    end
end
"#;

const SRC_PROG: &str = r#"
class Sensor ()
    Unit read()
        List<Object> xs = construct("
            SELECT ?id {
                ?x prog:Pump_id ?id .
            }");
    end
end
"#;

const SRC_UNKNOWN_PREFIX: &str = r#"
class Sensor ()
    Unit read()
        List<Object> xs = construct("
            SELECT ?id {
                ?x dyn:live ?id .
            }");
    end
end
"#;

#[test]
fn present_element_is_ok() {
    let ont = write_tmp("smol_elements_present.ttl", ONT);
    let diags = smol_ontology::check_ontology(&parse(SRC_OK), &[ont], &[], &HashMap::new());
    assert!(
        !has_code(&diags, DiagnosticCode::SparqlElementMissing),
        "expected no E3013, got: {diags:#?}"
    );
}

#[test]
fn missing_element_is_error() {
    // Remove the :hasId property from the ontology.
    let ont_no_hasid = ONT.replace(":hasId rdf:type owl:DatatypeProperty .\n", "");
    let ont = write_tmp("smol_elements_missing.ttl", &ont_no_hasid);
    let diags = smol_ontology::check_ontology(&parse(SRC_OK), &[ont], &[], &HashMap::new());
    assert!(
        has_code(&diags, DiagnosticCode::SparqlElementMissing),
        "expected E3013 for missing :hasId, got: {diags:#?}"
    );
}

#[test]
fn prog_elements_are_skipped() {
    let ont = write_tmp(
        "smol_elements_prog.ttl",
        &ONT.replace(":hasId rdf:type owl:DatatypeProperty .\n", ""),
    );
    let diags = smol_ontology::check_ontology(&parse(SRC_PROG), &[ont], &[], &HashMap::new());
    assert!(
        !has_code(&diags, DiagnosticCode::SparqlElementMissing),
        "prog: elements must be skipped, got: {diags:#?}"
    );
}

#[test]
fn unknown_prefix_is_warning() {
    let ont = write_tmp("smol_elements_unknown_prefix.ttl", ONT);
    let diags =
        smol_ontology::check_ontology(&parse(SRC_UNKNOWN_PREFIX), &[ont], &[], &HashMap::new());
    assert!(
        has_code(&diags, DiagnosticCode::UnknownPrefix),
        "expected unknown-prefix warning, got: {diags:#?}"
    );
}

#[test]
fn no_ontology_skips_presence_but_keeps_syntax() {
    // Without an ontology the E3013 checks must be skipped entirely
    // (prog-free query referencing a non-existent element).
    let diags = smol_ontology::check_ontology(&parse(SRC_OK), &[], &[], &HashMap::new());
    assert!(
        !has_code(&diags, DiagnosticCode::SparqlElementMissing),
        "no ontology => no E3013, got: {diags:#?}"
    );
}

const SRC_FULL_IRI: &str = r#"
class Sensor ()
    Unit read()
        List<Object> xs = construct("
            SELECT ?id {
                ?x <http://example.org/domain/hasId> ?id .
            }");
    end
end
"#;

const SRC_FULL_PROG_IRI: &str = r#"
class Sensor ()
    Unit read()
        List<Object> xs = construct("
            SELECT ?id {
                ?x <https://www.smolang.org/prog/Pump_id> ?id .
            }");
    end
end
"#;

#[test]
fn full_iri_is_checked() {
    let ont_no_hasid = ONT.replace(":hasId rdf:type owl:DatatypeProperty .\n", "");
    let ont = write_tmp("smol_elements_fulliri.ttl", &ont_no_hasid);
    let diags = smol_ontology::check_ontology(&parse(SRC_FULL_IRI), &[ont], &[], &HashMap::new());
    assert!(
        has_code(&diags, DiagnosticCode::SparqlElementMissing),
        "expected E3013 for missing full IRI, got: {diags:#?}"
    );
}

#[test]
fn prog_full_iri_is_excluded() {
    let ont = write_tmp(
        "smol_elements_fullprog.ttl",
        &ONT.replace(":hasId rdf:type owl:DatatypeProperty .\n", ""),
    );
    let diags =
        smol_ontology::check_ontology(&parse(SRC_FULL_PROG_IRI), &[ont], &[], &HashMap::new());
    assert!(
        !has_code(&diags, DiagnosticCode::SparqlElementMissing),
        "prog: full IRI must be skipped, got: {diags:#?}"
    );
}

const SRC_RETRIEVES: &str = r#"
class Actuator ()
classifies ":device";
retrieves "SELECT DISTINCT ?name WHERE {
    :device rdf:type owl:Class ;
        :hasName ?name .
}";
end
"#;

#[test]
fn retrieves_elements_are_checked() {
    // `hasId` exists but `hasName` does not -> exactly one E3013.
    let ont = write_tmp("smol_elements_retrieves.ttl", ONT);
    let diags = smol_ontology::check_ontology(&parse(SRC_RETRIEVES), &[ont], &[], &HashMap::new());
    let missing: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::SparqlElementMissing)
        .collect();
    assert_eq!(
        missing.len(),
        1,
        "expected exactly one E3013 (:hasName), got: {diags:#?}"
    );
}

// --- Near-duplicate predicate (typo) detection ---

const ONT_ASSIGNERO: &str = r#"
@prefix odrl: <http://www.w3.org/ns/odrl/2/> .
@prefix : <http://example.org/domain/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
:perm1 a odrl:Permission ;
    odrl:assigner :a1 .
:perm2 a odrl:Permission ;
    odrl:assignero :a1 .
"#;

#[test]
fn near_duplicate_predicate_is_warned() {
    let ont = write_tmp("smol_elements_assignero.ttl", ONT_ASSIGNERO);
    let diags = smol_ontology::check_ontology(&parse(SRC_OK), &[ont], &[], &HashMap::new());
    assert!(
        has_code(&diags, DiagnosticCode::SuspiciousOntologyPredicate),
        "expected E3014 for odrl:assignero, got: {diags:#?}"
    );
    let warned: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::SuspiciousOntologyPredicate)
        .collect();
    assert!(
        warned.iter().any(|d| d.message.contains("assignero")),
        "warning must mention the typo predicate, got: {diags:#?}"
    );
}

const ONT_ASSIGNEE: &str = r#"
@prefix odrl: <http://www.w3.org/ns/odrl/2/> .
@prefix : <http://example.org/domain/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
:perm1 a odrl:Permission ;
    odrl:assigner :a1 ;
    odrl:assignee :a1 .
"#;

#[test]
fn distinct_similar_predicates_not_warned() {
    // odrl:assigner vs odrl:assignee differ by substitution, not by one
    // extra character — they must not be flagged.
    let ont = write_tmp("smol_elements_assignee.ttl", ONT_ASSIGNEE);
    let diags = smol_ontology::check_ontology(&parse(SRC_OK), &[ont], &[], &HashMap::new());
    assert!(
        !has_code(&diags, DiagnosticCode::SuspiciousOntologyPredicate),
        "assigner/assignee must not be flagged, got: {diags:#?}"
    );
}

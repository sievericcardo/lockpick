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

const SRC_UNDECLARED_PREFIX: &str = r#"
class PolicyModel ()
    Unit adaptAddPermissions()
        List<Object> permissions = construct("
            PREFIX odrl: <http://www.w3.org/ns/odrl/2/>
            SELECT ?id {
                ?perm a odrl:Permission ;
                    pol:identifier ?id .
            }");
    end
end
"#;

#[test]
fn no_ontology_undeclared_prefixes_are_not_syntax_errors() {
    // With no ontology loaded, prefixes that are used but not declared (neither
    // in the query nor in the merged prefix map) may be injected at runtime, so
    // they must not fail SPARQL syntax validation — they are warnings instead.
    let diags =
        smol_ontology::check_ontology(&parse(SRC_UNDECLARED_PREFIX), &[], &[], &HashMap::new());
    assert!(
        !has_code(&diags, DiagnosticCode::SparqlSyntaxError),
        "undeclared prefixes must not be a syntax error, got: {diags:#?}"
    );
    assert!(
        has_code(&diags, DiagnosticCode::UnknownPrefix),
        "undeclared prefixes should still warn (E3010), got: {diags:#?}"
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

const ONT_QUERY_LOCAL_DEFAULT: &str = r#"
@prefix : <http://www.semanticweb.org/untitled/> .
@prefix bedreflyt: <http://www.smolang.org/bedreflyt/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
bedreflyt:City rdf:type owl:Class .
bedreflyt:cityName rdf:type owl:DatatypeProperty .
"#;

const SRC_QUERY_LOCAL_DEFAULT: &str = r#"
class CityModel ()
    Unit adaptAddition()
        List<Object> newCities = construct("
            PREFIX : <http://www.smolang.org/bedreflyt/>
            SELECT DISTINCT ?cityName {
                ?city a :City ;
                    :cityName ?cityName .
            }");
    end
end
"#;

#[test]
fn query_local_default_prefix_is_respected() {
    // The query re-binds `:` to <http://www.smolang.org/bedreflyt/>, so
    // `:City` / `:cityName` must resolve there (matching `bedreflyt:` in the
    // ontology) and NOT to the ontology's own default namespace.
    let ont = write_tmp("smol_elements_query_local.ttl", ONT_QUERY_LOCAL_DEFAULT);
    let diags = smol_ontology::check_ontology(
        &parse(SRC_QUERY_LOCAL_DEFAULT),
        &[ont],
        &[],
        &HashMap::new(),
    );
    assert!(
        !has_code(&diags, DiagnosticCode::SparqlElementMissing),
        "query-local `:` must resolve to the ontology's bedreflyt: elements, got: {diags:#?}"
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

// --- classifies disjoint-union (E3015) ---

const DU_ONT: &str = r#"
@prefix domain: <http://example.org/domain/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
domain:HealthState rdf:type owl:Class ;
    owl:disjointUnionOf ( domain:Healthy domain:Unhealthy domain:Dead ) .
domain:Healthy rdf:type owl:Class ; rdfs:subClassOf domain:HealthState .
domain:Unhealthy rdf:type owl:Class ; rdfs:subClassOf domain:HealthState .
domain:Dead rdf:type owl:Class ; rdfs:subClassOf domain:HealthState .
"#;

const DU_SRC_OK: &str = r#"
abstract class Twin() end
class HealthState extends Twin() classifies "<domain:HealthState>"; end
class Healthy extends HealthState() classifies "<domain:Healthy>"; end
class Unhealthy extends HealthState() classifies "<domain:Unhealthy>"; end
class Dead extends HealthState() classifies "<domain:Dead>"; end
"#;

#[test]
fn disjoint_union_matching_ontology_is_ok() {
    let ont = write_tmp("smol_du_ok.ttl", DU_ONT);
    let diags = smol_ontology::check_ontology(&parse(DU_SRC_OK), &[ont], &[], &HashMap::new());
    assert!(
        !has_code(&diags, DiagnosticCode::ClassifiesDisjointUnion),
        "matching disjoint union must not error, got: {diags:#?}"
    );
}

#[test]
fn disjoint_union_missing_ontology_declaration_is_error() {
    // Same SMOL model but the ontology never declares HealthState as a
    // disjoint union (only subClassOf) -> coverage check must fire.
    let no_du = DU_ONT.replace(
        "    owl:disjointUnionOf ( domain:Healthy domain:Unhealthy domain:Dead ) .\n",
        "",
    );
    let ont = write_tmp("smol_du_missing.ttl", &no_du);
    let diags = smol_ontology::check_ontology(&parse(DU_SRC_OK), &[ont], &[], &HashMap::new());
    assert!(
        has_code(&diags, DiagnosticCode::ClassifiesDisjointUnion),
        "missing ontology disjoint union must error, got: {diags:#?}"
    );
}

#[test]
fn disjoint_union_mismatched_ontology_members_is_error() {
    // Ontology declares a 4th member (Sick) that the SMOL model does not have.
    let with_sick = DU_ONT.replace(
        "    owl:disjointUnionOf ( domain:Healthy domain:Unhealthy domain:Dead ) .\n",
        "    owl:disjointUnionOf ( domain:Healthy domain:Unhealthy domain:Dead domain:Sick ) .\n",
    );
    let ont = write_tmp("smol_du_mismatch.ttl", &with_sick);
    let diags = smol_ontology::check_ontology(&parse(DU_SRC_OK), &[ont], &[], &HashMap::new());
    assert!(
        has_code(&diags, DiagnosticCode::ClassifiesDisjointUnion),
        "mismatched disjoint union members must error, got: {diags:#?}"
    );
}

const DU_SRC_BAD_SUBSUMES: &str = r#"
abstract class Twin() end
class HealthState extends Twin() classifies "<domain:HealthState>"; end
class Healthy extends HealthState() classifies "<domain:Healthy>"; end
class Unhealthy extends HealthState() classifies "<domain:Unhealthy>"; end
class Dead extends HealthState() classifies "<domain:Greenhouse>"; end
"#;

#[test]
fn disjoint_union_subclass_not_under_superclass_is_error() {
    // `Dead` classifies domain:Greenhouse which is not a subclass of
    // HealthState -> subsumption check must fire.
    let ont = write_tmp("smol_du_subsumes.ttl", DU_ONT);
    let diags =
        smol_ontology::check_ontology(&parse(DU_SRC_BAD_SUBSUMES), &[ont], &[], &HashMap::new());
    assert!(
        has_code(&diags, DiagnosticCode::ClassifiesDisjointUnion),
        "subclass classifying a non-subsumed class must error, got: {diags:#?}"
    );
}

// The superclass itself may omit `classifies`; the superclass OWL class is then
// inferred from the ontology's `owl:disjointUnionOf` (as in the greenhouse
// example where `HealthState` declares no `classifies`).

const DU_SRC_INFERRED_OK: &str = r#"
abstract class Twin() end
class HealthState extends Twin() end
class Healthy extends HealthState() classifies "<domain:Healthy>"; end
class Unhealthy extends HealthState() classifies "<domain:Unhealthy>"; end
class Dead extends HealthState() classifies "<domain:Dead>"; end
"#;

#[test]
fn disjoint_union_inferred_superclass_matching_is_ok() {
    // Subclasses Healthy/Unhealthy/Dead match the ontology's disjoint union of
    // HealthState -> no error even though HealthState has no `classifies`.
    let ont = write_tmp("smol_du_inferred_ok.ttl", DU_ONT);
    let diags =
        smol_ontology::check_ontology(&parse(DU_SRC_INFERRED_OK), &[ont], &[], &HashMap::new());
    assert!(
        !has_code(&diags, DiagnosticCode::ClassifiesDisjointUnion),
        "inferred superclass with matching disjoint union must not error, got: {diags:#?}"
    );
}

const DU_SRC_INFERRED_MISSING_STATE: &str = r#"
abstract class Twin() end
class HealthState extends Twin() end
class Healthy extends HealthState() classifies "<domain:Healthy>"; end
class Unhealthy extends HealthState() classifies "<domain:Unhealthy>"; end
"#;

#[test]
fn disjoint_union_inferred_superclass_missing_state_is_error() {
    // One state (`Dead`) is removed: the subclasses' disjoint union is
    // {Healthy, Unhealthy} but the ontology's disjoint union of HealthState is
    // {Healthy, Unhealthy, Dead} -> must error.
    let ont = write_tmp("smol_du_inferred_missing.ttl", DU_ONT);
    let diags = smol_ontology::check_ontology(
        &parse(DU_SRC_INFERRED_MISSING_STATE),
        &[ont],
        &[],
        &HashMap::new(),
    );
    assert!(
        has_code(&diags, DiagnosticCode::ClassifiesDisjointUnion),
        "missing a disjoint-union state must error, got: {diags:#?}"
    );
}

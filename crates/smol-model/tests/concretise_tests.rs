//! Tests for initial-state concretisation (`smol_model::concretise`, §2.10).
//!
//! Run with: `cargo test -p smol-model --test concretise_tests`

use smol_model::model;
use smol_syntax::diagnostic::DiagnosticCode;

fn parse(src: &str) -> smol_syntax::ast::File {
    let (file, diags) = smol_parser::parse("test.smol", src);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    file
}

fn concretise_diags(src: &str) -> Vec<smol_syntax::diagnostic::Diagnostic> {
    let file = parse(src);
    let files = [file];
    let map = model::build_class_map(&files);
    let extracted = model::extract(&map);
    let roots: Vec<model::Root> = extracted
        .roots
        .iter()
        .map(|r| model::Root {
            entity: r.entity.clone(),
            states: r.states.clone(),
            adapted: false,
        })
        .collect();
    let ts = model::build_transition_system(&roots);
    smol_model::concretise::check_initial_concretisation(&files, &map, &extracted, &ts)
}

fn has_concretisation_failure(diags: &[smol_syntax::diagnostic::Diagnostic]) -> bool {
    diags
        .iter()
        .any(|d| d.message.contains("InitialConcretisationFailure"))
}

const MODEL: &str = r#"
class Twin () end
class Plant extends Twin () end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end
class Model () end
"#;

#[test]
fn simple_main_maps_ok() {
    let src = format!("{MODEL}\nmain\n Plant p = new Plant();\nend\n");
    let diags = concretise_diags(&src);
    assert!(
        !has_concretisation_failure(&diags),
        "simple main creating Plant must map: {diags:?}"
    );
}

#[test]
fn state_class_main_maps_ok() {
    let src = format!("{MODEL}\nmain\n Healthy h = new Healthy();\nend\n");
    let diags = concretise_diags(&src);
    assert!(
        !has_concretisation_failure(&diags),
        "main creating a state class must map: {diags:?}"
    );
}

#[test]
fn unmapped_main_flagged() {
    let src = format!("{MODEL}\nmain\n Model m = new Model();\nend\n");
    let diags = concretise_diags(&src);
    assert!(
        has_concretisation_failure(&diags),
        "main creating only unrelated Model must be flagged: {diags:?}"
    );
    // Dedicated code E5013.
    assert!(
        diags
            .iter()
            .any(|d| d.code == DiagnosticCode::InitialConcretisationFailure),
        "expected E5013 InitialConcretisationFailure: {diags:?}"
    );
}

#[test]
fn branching_main_partial_mapping() {
    let src = format!(
        "{MODEL}\nmain\n Model m = new Model();\n if m != null then\n Plant p = new Plant();\n else\n print(\"none\");\n end\nend\n"
    );
    let diags = concretise_diags(&src);
    assert!(
        !has_concretisation_failure(&diags),
        "branching main with one mapping arm must stay silent: {diags:?}"
    );
}

#[test]
fn branching_main_no_mapping_flagged() {
    let src = format!(
        "{MODEL}\nmain\n Model m = new Model();\n if m != null then\n Model a = new Model();\n else\n Model b = new Model();\n end\nend\n"
    );
    let diags = concretise_diags(&src);
    assert!(
        has_concretisation_failure(&diags),
        "branching main with no mapping arm must be flagged: {diags:?}"
    );
}

#[test]
fn empty_main_silent() {
    let src = format!("{MODEL}\nmain\n print(\"hello\");\nend\n");
    let diags = concretise_diags(&src);
    assert!(
        !has_concretisation_failure(&diags),
        "empty (entity-free) main must stay silent: {diags:?}"
    );
}

#[test]
fn missing_main_silent() {
    let diags = concretise_diags(MODEL);
    assert!(
        !has_concretisation_failure(&diags),
        "missing main must stay silent: {diags:?}"
    );
}

#[test]
fn loop_single_unroll_maps() {
    let src = format!(
        "{MODEL}\nmain\n Model m = new Model();\n while m != null do\n Plant p = new Plant();\n end\nend\n"
    );
    let diags = concretise_diags(&src);
    assert!(
        !has_concretisation_failure(&diags),
        "loop body creating Plant (one-unroll) must map: {diags:?}"
    );
}

#[test]
fn pipeline_wire_is_additive() {
    // End-to-end via check_model: unmapped main surfaces the failure, and a
    // mapped main does not break the existing CTL checks.
    let mapped = format!("{MODEL}\nmain\n Plant p = new Plant();\nend\n");
    let file = parse(&mapped);
    let diags = smol_model::check_model(&[file], &[], 3);
    assert!(
        !has_concretisation_failure(&diags),
        "mapped main must not flag via pipeline: {diags:?}"
    );

    let unmapped = format!("{MODEL}\nmain\n Model m = new Model();\nend\n");
    let file = parse(&unmapped);
    let diags = smol_model::check_model(&[file], &[], 3);
    assert!(
        has_concretisation_failure(&diags),
        "unmapped main must flag via pipeline: {diags:?}"
    );
}

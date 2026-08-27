//! Unit tests for the whole-program CFG extraction (`smol_model::cfg`) and
//! the whole-program transition system (`smol_model::model::build_program_ts`).
//!
//! Run with: `cargo test -p smol-model --test cfg_tests`

use smol_model::cfg::{build_cfg, init_entity_states, state_labels};
use smol_model::model;

fn parse_src(src: &str) -> (smol_syntax::ast::File, Vec<smol_syntax::diagnostic::Diagnostic>) {
    smol_parser::parse("test.smol", src)
}

fn cfg_of(src: &str) -> (smol_model::cfg::Cfg, smol_syntax::ast::File) {
    let (file, diags) = parse_src(src);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let map = model::build_class_map(std::slice::from_ref(&file));
    let extracted = model::extract(&map);
    let main = file.main.as_ref().expect("main block");
    let cfg = build_cfg(main, &map, &extracted, "test.smol", "", true, 3);
    (cfg, file)
}

#[test]
fn sequential_statements_are_linear() {
    let (cfg, _) = cfg_of(
        "main\n\
             print(\"a\");\n\
             print(\"b\");\n\
             print(\"c\");\n\
         end\n",
    );

    assert_eq!(cfg.nodes.len(), 3, "one node per statement");
    // Every node is labelled `in_main`.
    assert!(
        cfg.nodes.iter().all(|n| n.labels.contains("in_main")),
        "all main nodes must carry `in_main`: {:?}",
        cfg.nodes
    );
    // Linear chain: entry -> node1 -> exit, no branches.
    assert_eq!(cfg.nodes[cfg.entry].succs.len(), 1);
    let mid = cfg.nodes[cfg.entry].succs[0];
    assert_eq!(cfg.nodes[mid].succs, vec![cfg.exits[0]]);
}

#[test]
fn if_builds_branch_and_merge() {
    let (cfg, _) = cfg_of(
        "main\n\
             if x != null then\n\
                 print(\"a\");\n\
             else\n\
                 print(\"b\");\n\
             end\n\
             print(\"c\");\n\
         end\n",
    );

    // branch, then-node, else-node, merge, c-node
    assert_eq!(cfg.nodes.len(), 5);

    let guards: Vec<&str> = cfg
        .nodes
        .iter()
        .filter(|n| n.labels.iter().any(|l| l.starts_with("guard_")))
        .map(|n| {
            n.labels
                .iter()
                .find(|l| l.starts_with("guard_"))
                .unwrap()
                .as_str()
        })
        .collect();
    assert_eq!(guards.len(), 1, "exactly one guard node for the if");
    assert!(guards[0].starts_with("guard_test_smol_"));

    // The merge node has two predecessors and one successor (the `c` node).
    let merge = cfg
        .nodes
        .iter()
        .find(|n| cfg.nodes.iter().filter(|p| p.succs.contains(&n.id)).count() == 2)
        .expect("merge node with two predecessors");
    assert_eq!(merge.succs.len(), 1);
}

#[test]
fn while_builds_loop_with_back_edge() {
    let (cfg, _) = cfg_of(
        "main\n\
             while x != null do\n\
                 print(\"a\");\n\
             end\n\
             print(\"b\");\n\
         end\n",
    );

    // header, body-node, loop-exit, b-node
    assert_eq!(cfg.nodes.len(), 4);

    let header = cfg.nodes[cfg.entry].clone();
    // Loop-entry edge (body) + loop-exit edge (after the loop).
    assert_eq!(header.succs.len(), 2);
    // The body node has a back-edge to the header.
    let body = &cfg.nodes[header.succs[0]];
    assert!(body.succs.contains(&header.id), "body must loop back to header");
    // The loop-exit node leads to the continuation node.
    let exit = &cfg.nodes[header.succs[1]];
    assert_eq!(exit.succs.len(), 1);
    let cont = &cfg.nodes[exit.succs[0]];
    assert!(cont.labels.contains("in_main"));
}

#[test]
fn adapt_widens_root_entity_states() {
    let src = r#"
class Twin () end
class Plant extends Twin (hidden String id) end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end

main
    Plant p = null;
    adapt(p);
end
"#;
    let (cfg, _) = cfg_of(src);

    assert_eq!(cfg.nodes.len(), 2);
    // The node following `adapt(p)` labels `after_adapt_Plant` and both states.
    let adapt_node = &cfg.nodes[1];
    assert!(adapt_node.labels.contains("after_adapt_Plant"));
    assert!(adapt_node.labels.contains("Healthy"));
    assert!(adapt_node.labels.contains("Unhealthy"));
    // The first node (before the adapt) is still unclassified.
    assert!(!cfg.nodes[0].labels.contains("Healthy"));
}

#[test]
fn classify_widens_root_entity_states() {
    let src = r#"
class Twin () end
class Plant extends Twin (hidden String id) end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end

main
    Plant p = null;
    HealthState h = classify(p);
end
"#;
    let (cfg, _) = cfg_of(src);

    let classify_node = &cfg.nodes[1];
    assert!(classify_node.labels.contains("after_classify_Plant"));
    assert!(classify_node.labels.contains("Healthy"));
    assert!(classify_node.labels.contains("Unhealthy"));
}

#[test]
fn method_inlining_labels_entry_and_exit() {
    let src = r#"
class Twin () end
class Plant extends Twin (hidden String id) end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end

class Model ()
    Unit run(Plant p)
        adapt(p);
    end
end

main
    Model m = new Model();
    m.run(null);
end
"#;
    let (file, diags) = parse_src(src);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let map = model::build_class_map(std::slice::from_ref(&file));
    let extracted = model::extract(&map);

    let program_ts = model::build_program_ts(std::slice::from_ref(&file), &map, &extracted, 3);

    let entry_labels: Vec<&str> = program_ts
        .labels
        .iter()
        .flat_map(|l| l.iter())
        .map(String::as_str)
        .filter(|l| l.starts_with("entry_") || l.starts_with("exit_"))
        .collect();
    assert!(
        entry_labels.contains(&"entry_Model_run")
            && entry_labels.contains(&"exit_Model_run"),
        "expected inlined entry/exit labels, got {entry_labels:?}"
    );

    // `in_main` on every main statement node, `at_program_end` on the terminal.
    assert!(
        program_ts
            .labels
            .iter()
            .any(|l| l.contains("at_program_end")),
        "missing at_program_end"
    );
}

#[test]
fn structural_property_holds_on_program_ts() {
    let src = r#"
class Twin () end
class Plant extends Twin (hidden String id) end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end

class Model ()
    Unit run(Plant p)
        adapt(p);
    end
end

main
    Model m = new Model();
    m.run(null);
end
"#;
    let (file, diags) = parse_src(src);
    assert!(diags.is_empty());
    let map = model::build_class_map(std::slice::from_ref(&file));
    let extracted = model::extract(&map);
    let program_ts = model::build_program_ts(std::slice::from_ref(&file), &map, &extracted, 3);

    // When the inlined method exits, the plant must be in one of its states.
    let prop = smol_model::ctl::parse_prop(
        "AG (exit_Model_run -> (Healthy || Unhealthy))",
    )
    .expect("property parses");
    assert!(
        smol_model::ctl::verify(&program_ts, &prop).is_ok(),
        "structural AG property should hold"
    );

    // The program eventually reaches its terminal node on all paths.
    let prop = smol_model::ctl::parse_prop("AF at_program_end").expect("property parses");
    assert!(smol_model::ctl::verify(&program_ts, &prop).is_ok());

    // `AG at_program_end` must fail: only the terminal node carries it.
    let prop = smol_model::ctl::parse_prop("AG at_program_end").expect("property parses");
    assert!(smol_model::ctl::verify(&program_ts, &prop).is_err());
}

#[test]
fn state_labels_skip_unclassified() {
    let src = r#"
class Twin () end
class Plant extends Twin (hidden String id) end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end
main
    print("x");
end
"#;
    let (file, diags) = parse_src(src);
    assert!(diags.is_empty());
    let map = model::build_class_map(std::slice::from_ref(&file));
    let extracted = model::extract(&map);

    let init = init_entity_states(&extracted);
    let labels = state_labels(&init);
    assert!(
        labels.is_empty(),
        "unclassified initial states yield no state-class APs: {labels:?}"
    );
}

// Regression: `target.field = classify(x)` must keep the assignment target so
// that the checker can disambiguate among multiple `context` holders
// (previously the receiver was dropped, producing a spurious E5002).
#[test]
fn classify_field_target_disambiguates() {
    let src = r#"
class Twin () end
class Plant extends Twin (hidden HealthState healthState) end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end
class WaterStrategy extends Twin (context Plant plant) end

class Model ()
    Unit go()
        List<Plant> newPlants = construct("SELECT ?x {}");
        Plant newPlant = newPlants.content;
        newPlant.healthState = classify(newPlant);
    end
end
"#;
    let (file, diags) = parse_src(src);
    assert!(diags.is_empty(), "parse errors: {diags:?}");
    let model_diags = smol_model::check_model(&[file], &[], 3);
    assert!(
        !model_diags
            .iter()
            .any(|d| d.code == smol_syntax::diagnostic::DiagnosticCode::ClassifyAmbiguous),
        "field-targeted classify must disambiguate to HealthState: {model_diags:?}"
    );
}
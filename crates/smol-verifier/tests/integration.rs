//! Integration tests for the SMOL verifier.
//!
//! Run with: `cargo test -p smol-verifier --test integration`

use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is the crates/smol-verifier directory.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Go up two levels: crates/smol-verifier -> crates -> workspace root
    manifest_dir
        .parent()
        .expect("crates/")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

// greenhouse-adapt / bedreflyt: legacy examples use `List` fields
// Per the model-correctness rule (E2013), `List<T>` fields are hidden by default
// and must be `ExplList<T>`.  These examples were written before the rule and are
// intentionally left unchanged, so they are now expected to produce E2013.

#[test]
fn greenhouse_adapt_flags_list_fields() {
    use smol_syntax::diagnostic::DiagnosticCode;

    let manifest = workspace_root().join("examples/greenhouse-adapt/smol.toml");

    if !manifest.exists() {
        eprintln!("SKIP: manifest not found at {}", manifest.display());
        return;
    }

    let diags = smol_verifier::verify_manifest(&manifest);
    assert!(
        diags
            .iter()
            .any(|d| d.code == DiagnosticCode::ListFieldNotExplicit),
        "expected ListFieldNotExplicit for greenhouse-adapt, got: {diags:?}"
    );
}

#[test]
fn bedreflyt_flags_list_fields() {
    use smol_syntax::diagnostic::DiagnosticCode;

    let manifest = workspace_root().join("examples/bedreflyt/smol.toml");

    if !manifest.exists() {
        eprintln!("SKIP: manifest not found at {}", manifest.display());
        return;
    }

    let diags = smol_verifier::verify_manifest(&manifest);
    assert!(
        diags
            .iter()
            .any(|d| d.code == DiagnosticCode::ListFieldNotExplicit),
        "expected ListFieldNotExplicit for bedreflyt, got: {diags:?}"
    );
}

// odrl: uses ExplList<T> fields, must be clean
#[test]
fn odrl_no_errors() {
    let manifest = workspace_root().join("examples/odrl/smol.toml");

    if !manifest.exists() {
        eprintln!("SKIP: manifest not found at {}", manifest.display());
        return;
    }

    let diags = smol_verifier::verify_manifest(&manifest);
    let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();

    for e in &errors {
        eprintln!(
            "[{}] {}: {}",
            e.code,
            e.file.as_deref().unwrap_or("?"),
            e.message
        );
    }

    assert!(
        errors.is_empty(),
        "expected zero errors for odrl, got {}",
        errors.len()
    );
}

// error-file.smol: List fields should be rejected
#[test]
fn error_file_flags_list_fields() {
    use smol_syntax::diagnostic::DiagnosticCode;

    let error_file = workspace_root().join("examples/error-file.smol");
    if !error_file.exists() {
        eprintln!("SKIP: error-file.smol not found");
        return;
    }

    let diags = smol_verifier::verify_files(&[error_file], &[]);
    let list_errors: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::ListFieldNotExplicit)
        .collect();

    assert!(
        list_errors.len() >= 7,
        "expected at least 7 ListFieldNotExplicit errors, got {} (all: {diags:?})",
        list_errors.len()
    );
}

// Model checking via the public API
#[test]
fn model_check_property_violation_via_public_api() {
    use smol_syntax::diagnostic::DiagnosticCode;

    let src = r#"
class Twin () end
class Plant extends Twin (hidden String id) end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end

class Model ()
    Unit go(Plant p)
        adapt(p);
    end
end
"#;

    let tmp = std::env::temp_dir().join("smol_model_prop.smol");
    std::fs::write(&tmp, src).unwrap();

    let properties = vec!["AG Healthy".to_owned()];
    let diags = smol_verifier::verify_files_with_properties(&[tmp.clone()], &[], &properties);
    let _ = std::fs::remove_file(&tmp);

    assert!(
        diags
            .iter()
            .any(|d| d.code == DiagnosticCode::InvariantViolation),
        "expected InvariantViolation, got: {diags:?}"
    );
}

#[test]
fn model_check_valid_property_via_public_api() {
    use smol_syntax::diagnostic::DiagnosticCode;

    let src = r#"
class Twin () end
class Plant extends Twin (hidden String id) end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end

class Model ()
    Unit go(Plant p)
        adapt(p);
    end
end
"#;

    let tmp = std::env::temp_dir().join("smol_model_prop_ok.smol");
    std::fs::write(&tmp, src).unwrap();

    let properties = vec!["EF Healthy".to_owned()];
    let diags = smol_verifier::verify_files_with_properties(&[tmp.clone()], &[], &properties);
    let _ = std::fs::remove_file(&tmp);

    assert!(
        !diags
            .iter()
            .any(|d| d.code == DiagnosticCode::InvariantViolation),
        "expected no InvariantViolation, got: {diags:?}"
    );
}

// Negative test: unknown class reference
#[test]
fn error_unknown_class() {
    use smol_syntax::diagnostic::DiagnosticCode;

    let smol_src = r#"
class Foo extends NonExistentParent ()
end
"#;

    let tmp = std::env::temp_dir().join("smol_test_unknown_class.smol");
    std::fs::write(&tmp, smol_src).unwrap();

    let diags = smol_verifier::verify_files(&[tmp.clone()], &[]);
    let _ = std::fs::remove_file(&tmp);

    let has_unknown = diags.iter().any(|d| d.code == DiagnosticCode::UnknownClass);
    assert!(
        has_unknown,
        "expected UnknownClass diagnostic; got: {diags:?}"
    );
}

// Negative test: abstract class instantiation
#[test]
fn error_abstract_instantiation() {
    use smol_syntax::diagnostic::DiagnosticCode;

    let smol_src = r#"
abstract class Animal ()
    abstract Unit speak()
end

main
    Animal a = new Animal();
end
"#;

    let tmp = std::env::temp_dir().join("smol_test_abstract_inst.smol");
    std::fs::write(&tmp, smol_src).unwrap();

    let diags = smol_verifier::verify_files(&[tmp.clone()], &[]);
    let _ = std::fs::remove_file(&tmp);

    let has_err = diags
        .iter()
        .any(|d| d.code == DiagnosticCode::AbstractInstantiation);
    assert!(
        has_err,
        "expected AbstractInstantiation diagnostic; got: {diags:?}"
    );
}

// Negative test: override without parent
#[test]
fn error_override_without_parent() {
    use smol_syntax::diagnostic::DiagnosticCode;

    let smol_src = r#"
class Dog ()
    override Unit bark()
        print("woof");
    end
end
"#;

    let tmp = std::env::temp_dir().join("smol_test_override.smol");
    std::fs::write(&tmp, smol_src).unwrap();

    let diags = smol_verifier::verify_files(&[tmp.clone()], &[]);
    let _ = std::fs::remove_file(&tmp);

    let has_err = diags
        .iter()
        .any(|d| d.code == DiagnosticCode::OverrideWithoutParent);
    assert!(
        has_err,
        "expected OverrideWithoutParent diagnostic; got: {diags:?}"
    );
}

// Negative test: duplicate class
#[test]
fn error_duplicate_class() {
    use smol_syntax::diagnostic::DiagnosticCode;

    let src_a = r#"class Foo () end"#;
    let src_b = r#"class Foo () end"#;

    let tmp_a = std::env::temp_dir().join("smol_dup_a.smol");
    let tmp_b = std::env::temp_dir().join("smol_dup_b.smol");
    std::fs::write(&tmp_a, src_a).unwrap();
    std::fs::write(&tmp_b, src_b).unwrap();

    let diags = smol_verifier::verify_files(&[tmp_a.clone(), tmp_b.clone()], &[]);
    let _ = std::fs::remove_file(&tmp_a);
    let _ = std::fs::remove_file(&tmp_b);

    let has_err = diags
        .iter()
        .any(|d| d.code == DiagnosticCode::DuplicateClass);
    assert!(has_err, "expected DuplicateClass; got: {diags:?}");
}

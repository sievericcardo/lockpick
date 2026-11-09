//! Escape-analysis tests for the FMO lifecycle pass (plan §2.5 remaining):
//! `W2024 FmoEscaped` for FMOs leaving local ownership.

use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

fn check_src(src: &str) -> Vec<Diagnostic> {
    let (file, parse_diags) = smol_parser::parse("<test>", src);
    let mut diags = parse_diags;
    diags.extend(smol_checker::check(&[file]));
    diags
}

fn has_code(diags: &[Diagnostic], code: DiagnosticCode) -> bool {
    diags.iter().any(|d| d.code == code)
}

#[test]
fn fmo_stored_in_field_escapes() {
    let src = r#"
class Sim ()
    Unit run()
        FMO[in Double speed, out Double pos] fmu = null;
        this.shared = fmu;
        destroy(fmu);
    end
end
"#;
    // `this.shared` is undeclared on Sim — the escape check runs on the AST
    // regardless of the (separate) UnknownField diagnostic.
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::FmoEscaped),
        "expected FmoEscaped for field-stored FMO, got: {diags:#?}"
    );
    assert!(
        !has_code(&diags, DiagnosticCode::FmoNotDestroyed),
        "escaped FMO must not also warn FmoNotDestroyed, got: {diags:#?}"
    );
}

#[test]
fn fmo_passed_to_helper_escapes() {
    let src = r#"
class Sim ()
    Unit register(Unit x)
        skip;
    end
    Unit run()
        FMO[in Double speed, out Double pos] fmu = null;
        this.register(fmu);
        destroy(fmu);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::FmoEscaped),
        "expected FmoEscaped for FMO passed to a helper, got: {diags:#?}"
    );
}

#[test]
fn fmo_used_as_receiver_does_not_escape() {
    let src = r#"
class Sim ()
    Unit run()
        FMO[in Double speed, out Double pos] fmu = null;
        Double v = fmu.port("pos");
        print(v);
        destroy(fmu);
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::FmoEscaped),
        "receiver use must not escape, got: {diags:#?}"
    );
    assert!(
        !has_code(&diags, DiagnosticCode::FmoNotDestroyed),
        "destroyed FMO must stay clean, got: {diags:#?}"
    );
}

#[test]
fn fmo_returned_escapes() {
    let src = r#"
class Sim ()
    Unit run()
        FMO[in Double speed, out Double pos] fmu = null;
        return fmu;
    end
end
"#;
    // A `return fmu` from a Unit method is a type error elsewhere; the escape
    // finding must still fire and suppress the not-destroyed warning.
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::FmoEscaped),
        "expected FmoEscaped for returned FMO, got: {diags:#?}"
    );
}

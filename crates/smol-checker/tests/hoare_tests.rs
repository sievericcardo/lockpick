//! Hoare-contract tests (plan §2.9, codes 6000–6999).
//!
//! Contracts are written as JML comments (`/*@ requires … @*/`,
//! `/*@ ensures … @*/`) — the `smol.pest` grammar is fixed.

use smol_checker::hoare::check_hoare;
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

fn hoare_diags(src: &str) -> Vec<Diagnostic> {
    let (file, _) = smol_parser::parse("<test>", src);
    let files = vec![file];
    let (env, _) = smol_checker::decl_pass::build_class_env(&files);
    check_hoare(&files, &env)
}

fn has_code(diags: &[Diagnostic], code: DiagnosticCode) -> bool {
    diags.iter().any(|d| d.code == code)
}

fn has_no_hoare_codes(diags: &[Diagnostic]) -> bool {
    diags.iter().all(|d| {
        !matches!(
            d.code,
            DiagnosticCode::PreconditionUnsatisfied
                | DiagnosticCode::PostconditionViolated
                | DiagnosticCode::SmtDischargeFailed
                | DiagnosticCode::SmtTimeout
        )
    })
}

// ── Postconditions (E6002) ────────────────────────────────────────────────

#[test]
fn valid_constant_contract_passes() {
    let src = r#"
class Calc ()
    Int one()
    /*@ requires 1 > 0 @*/
    /*@ ensures result == 1 @*/
        return 1;
    end
end
"#;
    let diags = hoare_diags(src);
    assert!(
        diags.is_empty(),
        "valid contract must produce no diagnostics, got: {diags:#?}"
    );
}

#[test]
fn valid_symbolic_contract_is_silent() {
    // `requires x > 0` justifies `ensures result > 0` semantically, but the
    // syntactic prover cannot track it — it must stay silent, not guess.
    let src = r#"
class Calc ()
    Int ident(Int x)
    /*@ requires x > 0 @*/
    /*@ ensures result > 0 @*/
        return x;
    end
end
"#;
    let diags = hoare_diags(src);
    assert!(
        has_no_hoare_codes(&diags),
        "unprovable-but-valid contract must stay silent, got: {diags:#?}"
    );
}

#[test]
fn postcondition_violated_detected() {
    // `ensures result == 0` but the body returns `1`.
    let src = r#"
class Calc ()
    Int one()
    /*@ ensures result == 0 @*/
        return 1;
    end
end
"#;
    let diags = hoare_diags(src);
    assert!(
        has_code(&diags, DiagnosticCode::PostconditionViolated),
        "expected PostconditionViolated, got: {diags:#?}"
    );
    assert_eq!(
        diags.len(),
        1,
        "exactly one diagnostic expected, got: {diags:#?}"
    );
}

#[test]
fn postcondition_through_locals_detected() {
    // Constant propagation through a local must still catch the violation.
    let src = r#"
class Calc ()
    Int one()
    /*@ ensures result == 1 @*/
        Int r = 0;
        return r;
    end
end
"#;
    let diags = hoare_diags(src);
    assert!(
        has_code(&diags, DiagnosticCode::PostconditionViolated),
        "expected PostconditionViolated via local, got: {diags:#?}"
    );
}

// ── Preconditions at call sites (E6001) ───────────────────────────────────

#[test]
fn precondition_unsatisfied_at_call_site() {
    // `requires x > 0` called with literal `0`.
    let src = r#"
class Calc ()
    Int checked(Int x)
    /*@ requires x > 0 @*/
        return x;
    end
    Int driver()
        return this.checked(0);
    end
end
"#;
    let diags = hoare_diags(src);
    assert!(
        has_code(&diags, DiagnosticCode::PreconditionUnsatisfied),
        "expected PreconditionUnsatisfied, got: {diags:#?}"
    );
    assert_eq!(
        diags.len(),
        1,
        "exactly one diagnostic expected, got: {diags:#?}"
    );
}

#[test]
fn satisfied_call_is_silent() {
    let src = r#"
class Calc ()
    Int checked(Int x)
    /*@ requires x > 0 @*/
        return x;
    end
    Int driver()
        return this.checked(5);
    end
end
"#;
    let diags = hoare_diags(src);
    assert!(
        has_no_hoare_codes(&diags),
        "satisfied precondition must stay silent, got: {diags:#?}"
    );
}

#[test]
fn unknown_arg_is_silent() {
    // The argument is an unknown parameter — no proof of violation possible.
    let src = r#"
class Calc ()
    Int checked(Int x)
    /*@ requires x > 0 @*/
        return x;
    end
    Int driver(Int y)
        return this.checked(y);
    end
end
"#;
    let diags = hoare_diags(src);
    assert!(
        has_no_hoare_codes(&diags),
        "unknown argument must stay silent, got: {diags:#?}"
    );
}

// ── Silence / termination ─────────────────────────────────────────────────

#[test]
fn no_contracts_silent() {
    // Contract-free code (the shape of all existing examples/) yields nothing.
    let src = r#"
class Box (Int value)
    Int compute()
        Int result = this.value;
        result = result + 1;
        return result;
    end
end

main
    Box b = new Box(0);
    Int v = b.compute();
    print(v);
end
"#;
    let diags = hoare_diags(src);
    assert!(
        diags.is_empty(),
        "contract-free code must produce no diagnostics, got: {diags:#?}"
    );
}

#[test]
fn while_with_contract_is_skipped_silently() {
    // Loops have no invariant language yet: skip, terminate, stay silent.
    let src = r#"
class Loop ()
    Int count(Int n)
    /*@ ensures result == 0 @*/
        while n != 0 do
            n = n - 1;
        end
        return n;
    end
end
"#;
    let diags = hoare_diags(src);
    assert!(
        has_no_hoare_codes(&diags),
        "loop body must be skipped silently, got: {diags:#?}"
    );
}

#[test]
fn constant_branch_is_resolved() {
    let src = r#"
class Calc ()
    Int one()
    /*@ ensures result == 1 @*/
        if true then
            return 1;
        else
            return 2;
        end
    end
end
"#;
    let diags = hoare_diags(src);
    assert!(
        has_no_hoare_codes(&diags),
        "constant branch must resolve silently, got: {diags:#?}"
    );
}

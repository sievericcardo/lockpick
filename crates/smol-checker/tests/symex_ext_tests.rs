//! Extension tests for `smol-checker::symex` (plan §§2.3–2.4 remaining):
//! `decreases` clauses, widening + non-constant intervals.

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

fn errors(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags.iter().filter(|d| d.is_error()).collect()
}

// ── `decreases` clauses (§2.3) ─────────────────────────────────────────────

#[test]
fn valid_decreases_suppresses_e5012() {
    let src = r#"
class Loop ()
    Unit run()
        Int i = 0;
        while i < 10 do
            /*@ decreases 10 - i @*/
            print(i);
            i = i + 1;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        errors(&diags).is_empty(),
        "expected no errors, got: {diags:#?}"
    );
    assert!(
        !has_code(&diags, DiagnosticCode::LoopTerminationUnproven),
        "valid decreases clause must suppress E5012, got: {diags:#?}"
    );
}

#[test]
fn invalid_decreases_keeps_e5012() {
    let src = r#"
class Loop ()
    Unit run()
        Int i = 0;
        while i < 10 do
            /*@ decreases 10 - i @*/
            print(i);
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::LoopTerminationUnproven),
        "decreases clause with no monotonic update must keep E5012, got: {diags:#?}"
    );
}

// ── Variable integer bounds (E5012) ─────────────────────────────────────────

#[test]
fn variable_bound_loop_without_progress_is_warned() {
    let src = r#"
class Loop ()
    Unit run()
        Int i = 0;
        Int n = 10;
        while i < n do
            print(i);
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::LoopTerminationUnproven),
        "variable-bound loop with no progress must keep E5012, got: {diags:#?}"
    );
}

#[test]
fn variable_bound_loop_advancing_var_is_silent() {
    let src = r#"
class Loop ()
    Unit run()
        Int i = 0;
        Int n = 10;
        while i < n do
            i = i + 1;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::LoopTerminationUnproven),
        "variable-bound loop advancing the compared variable must be silent, got: {diags:#?}"
    );
}

#[test]
fn variable_bound_loop_advancing_bound_is_silent() {
    let src = r#"
class Loop ()
    Unit run()
        Int i = 0;
        Int n = 10;
        while i < n do
            n = n - 1;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::LoopTerminationUnproven),
        "variable-bound loop advancing the bound variable must be silent, got: {diags:#?}"
    );
}

#[test]
fn constant_bound_loop_still_checked() {
    let src = r#"
class Loop ()
    Unit run()
        Int i = 0;
        while i < 10 do
            print(i);
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::LoopTerminationUnproven),
        "constant-bound loop with no progress must keep E5012, got: {diags:#?}"
    );
}

// ── Widening + non-constant intervals (§2.4) ───────────────────────────────

#[test]
fn widening_detects_loop_doubled_overflow() {
    let src = r#"
class Calc ()
    Unit run()
        Int x = 1;
        while x > 0 do
            x = x * 2;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::IntegerOverflow),
        "expected IntegerOverflow for the unbounded doubling loop, got: {diags:#?}"
    );
}

#[test]
fn non_constant_divisor_is_silent() {
    let src = r#"
class Calc ()
    Int div(Int x, Int d)
        if d >= 0 then
            if d <= 5 then
                Int q = x / d;
                return q;
            else
                return 0;
            end
        else
            return 0;
        end
    end
end
"#;
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::DivisionByZero),
        "divisor in [0, 5] is not provably zero and must stay silent, got: {diags:#?}"
    );
    assert!(
        errors(&diags).is_empty(),
        "expected no errors, got: {diags:#?}"
    );
}

#[test]
fn constant_divisor_zero_still_flagged() {
    let src = r#"
class Calc ()
    Int div(Int x, Int y)
        Int q = x / 0;
        return q;
    end
end
"#;
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::DivisionByZero),
        "expected DivisionByZero for a constant zero divisor, got: {diags:#?}"
    );
}

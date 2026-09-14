//! Field-granularity null-safety tests (plan §2.1 remaining):
//! E2023 `NullableFieldDereference` for provably-nullable receivers.

use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode, Severity};

fn check_src(src: &str) -> Vec<Diagnostic> {
    let (file, parse_diags) = smol_parser::parse("<test>", src);
    let mut diags = parse_diags;
    diags.extend(smol_checker::check(&[file]));
    diags
}

fn has_code(diags: &[Diagnostic], code: DiagnosticCode) -> bool {
    diags.iter().any(|d| d.code == code)
}

fn codes(diags: &[Diagnostic], code: DiagnosticCode) -> Vec<&Diagnostic> {
    diags.iter().filter(|d| d.code == code).collect()
}

const PRELUDE: &str = r#"
class Inner ()
    Unit ping()
        skip;
    end
end
"#;

fn with_prelude(body: &str) -> String {
    format!("{PRELUDE}\n{body}")
}

// ── Flagged: nullable field assigned null, then dereferenced ────────────────

#[test]
fn nullable_field_assigned_null_then_dereferenced_is_flagged() {
    let src = with_prelude(
        r#"
class Outer (Nullable<Inner> f)
    Unit run()
        this.f = null;
        this.f.ping();
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "expected E2023 for deref of field assigned null, got: {diags:#?}"
    );
}

#[test]
fn nullable_field_never_guarded_is_flagged() {
    let src = with_prelude(
        r#"
class Outer (Nullable<Inner> f)
    Unit run()
        this.f.ping();
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "expected E2023 for unguarded deref of Nullable field, got: {diags:#?}"
    );
}

#[test]
fn nullable_field_dereference_is_error_level() {
    let src = with_prelude(
        r#"
class Outer (Nullable<Inner> f)
    Unit run()
        this.f.ping();
    end
end
"#,
    );
    let diags = check_src(&src);
    let flagged = codes(&diags, DiagnosticCode::NullableFieldDereference);
    assert!(!flagged.is_empty(), "expected E2023, got: {diags:#?}");
    assert!(
        flagged.iter().all(|d| d.severity == Severity::Error),
        "E2023 must be error-level, got: {flagged:#?}"
    );
}

#[test]
fn nullable_field_read_is_flagged() {
    let src = with_prelude(
        r#"
class Outer (Nullable<Inner> f)
    Int run()
        return this.f.size;
    end
end
"#,
    );
    // `this.f.size` on a Nullable field must flag even for plain field reads.
    // (Note: `size` does not exist on `Inner`, so the body pass also reports
    // `UnknownField` here — the E2023 assertion below is unaffected.)
    let diags = check_src(&src);
    assert!(
        has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "expected E2023 for field read on Nullable receiver, got: {diags:#?}"
    );
}

// ── Silent: guarded nullable-field access ───────────────────────────────────

#[test]
fn guarded_nullable_field_access_is_silent() {
    let src = with_prelude(
        r#"
class Outer (Nullable<Inner> f)
    Unit run()
        if this.f != null then
            this.f.ping();
        else
            skip;
        end
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "guarded access must not be flagged, got: {diags:#?}"
    );
    assert!(
        !has_code(&diags, DiagnosticCode::NullDereference),
        "guarded access must not raise E2018 either, got: {diags:#?}"
    );
}

#[test]
fn eq_guard_else_branch_is_silent() {
    let src = with_prelude(
        r#"
class Outer (Nullable<Inner> f)
    Unit run()
        if this.f == null then
            skip;
        else
            this.f.ping();
        end
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "`== null` else-branch must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn var_field_path_guard_is_silent() {
    let src = with_prelude(
        r#"
class Box (Nullable<Inner> opt)
    Unit run(Box b)
        if b.opt != null then
            b.opt.ping();
        else
            skip;
        end
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "guarded `var.field` access must not be flagged, got: {diags:#?}"
    );
}

// ── Nullable params ─────────────────────────────────────────────────────────

#[test]
fn nullable_param_unguarded_is_flagged() {
    let src = with_prelude(
        r#"
class Svc ()
    Unit run(Nullable<Inner> x)
        x.ping();
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "expected E2023 for unguarded deref of Nullable param, got: {diags:#?}"
    );
}

#[test]
fn nullable_param_guarded_is_silent() {
    let src = with_prelude(
        r#"
class Svc ()
    Unit run(Nullable<Inner> x)
        if x != null then
            x.ping();
        else
            skip;
        end
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "guarded Nullable param must not be flagged, got: {diags:#?}"
    );
}

// ── Non-nullable receivers are never flagged ────────────────────────────────

#[test]
fn non_nullable_param_unguarded_is_silent() {
    let src = with_prelude(
        r#"
class Svc ()
    Unit run(Inner x)
        x.ping();
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "non-nullable param must not be flagged, got: {diags:#?}"
    );
    assert!(
        !has_code(&diags, DiagnosticCode::NullDereference),
        "non-nullable param must not raise E2018 either, got: {diags:#?}"
    );
}

#[test]
fn non_nullable_field_is_silent() {
    let src = with_prelude(
        r#"
class Outer (Inner f)
    Unit run()
        this.f.ping();
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "non-nullable field must not be flagged, got: {diags:#?}"
    );
    assert!(
        !has_code(&diags, DiagnosticCode::NullDereference),
        "non-nullable field must not raise E2018 either, got: {diags:#?}"
    );
}

// ── Assignment flow ─────────────────────────────────────────────────────────

#[test]
fn field_reassigned_non_null_is_silent() {
    let src = with_prelude(
        r#"
class Outer (Nullable<Inner> f)
    Unit run()
        this.f = null;
        this.f = new Inner();
        this.f.ping();
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "field reassigned to non-null must not be flagged, got: {diags:#?}"
    );
}

#[test]
fn local_propagated_from_nullable_field_is_flagged() {
    // `var s = this.f` propagates both Maybe-nullness and provable
    // nullability, so `s.ping()` flags E2023.  (The body pass independently
    // reports `TypeMismatch` for `Nullable → Nullable` flows under its
    // current strict discipline — out of scope here; only E2023 is asserted.)
    let src = with_prelude(
        r#"
class Outer (Nullable<Inner> f)
    Unit run()
        Nullable<Inner> s = this.f;
        s.ping();
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "expected E2023 for local propagated from Nullable field, got: {diags:#?}"
    );
}

#[test]
fn local_propagated_then_guarded_is_silent() {
    let src = with_prelude(
        r#"
class Outer (Nullable<Inner> f)
    Unit run()
        Nullable<Inner> s = this.f;
        if s != null then
            s.ping();
        else
            skip;
        end
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        !has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "guarded propagated local must not be flagged, got: {diags:#?}"
    );
}

// ── Legacy E2018 behaviour is unchanged ─────────────────────────────────────

#[test]
fn legacy_e2018_still_fires_for_null_local_without_e2023_double_report() {
    let src = with_prelude(
        r#"
class Svc ()
    Unit run()
        Inner x = null;
        x.ping();
    end
end
"#,
    );
    let diags = check_src(&src);
    assert!(
        has_code(&diags, DiagnosticCode::NullDereference),
        "expected legacy E2018 for null local, got: {diags:#?}"
    );
    assert!(
        !has_code(&diags, DiagnosticCode::NullableFieldDereference),
        "definite-null local must report E2018 only (no E2023 double-report), got: {diags:#?}"
    );
}

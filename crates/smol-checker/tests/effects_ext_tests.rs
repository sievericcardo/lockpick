//! Extended effect-contract tests (plan §2.6 remaining):
//! `pure` / `domain-reads` / `domain-writes` enforcement + strict-mode
//! promotion of E2019.

use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode, Severity};

// `SMOL_STRICT_EFFECTS` is process-global, and tests in this binary run on
// multiple threads — serialize every test that touches the env var so the
// strict window can never leak into a default-behavior assert.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn check_src(src: &str) -> Vec<Diagnostic> {
    // Callers hold `ENV_LOCK` and set/clear `SMOL_STRICT_EFFECTS` as needed.
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

#[test]
fn pure_method_writing_plain_field_is_flagged() {
    let src = r#"
class C (Int x)
    /*@ pure @*/
    Unit set()
        this.x = 1;
    end
end
"#;
    // Guard against strict-mode leakage from a parallel test.
    let _guard = lock_env();
    unsafe { std::env::remove_var("SMOL_STRICT_EFFECTS") };
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::PureViolation),
        "expected PureViolation for pure method writing a field, got: {diags:#?}"
    );
    assert!(
        codes(&diags, DiagnosticCode::PureViolation)
            .iter()
            .all(|d| d.severity == Severity::Error),
        "PureViolation must be error-level, got: {diags:#?}"
    );
}

#[test]
fn pure_method_calling_non_pure_method_is_flagged() {
    let src = r#"
class C (Int x)
    Unit helper()
        skip;
    end
    /*@ pure @*/
    Unit driver()
        this.helper();
    end
end
"#;
    let _guard = lock_env();
    unsafe { std::env::remove_var("SMOL_STRICT_EFFECTS") };
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::PureViolation),
        "expected PureViolation for pure method calling non-pure method, got: {diags:#?}"
    );
}

#[test]
fn pure_method_calling_pure_method_is_clean() {
    let src = r#"
class C (Int x)
    /*@ pure @*/
    Int get()
        return this.x;
    end
    /*@ pure @*/
    Int fwd()
        return this.get();
    end
end
"#;
    let _guard = lock_env();
    unsafe { std::env::remove_var("SMOL_STRICT_EFFECTS") };
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::PureViolation)
            && !has_code(&diags, DiagnosticCode::EffectContractViolation),
        "pure-into-pure with no writes must be silent, got: {diags:#?}"
    );
}

#[test]
fn domain_reads_writing_domain_field_is_flagged() {
    let src = r#"
class Plant (domain Double moisture)
    /*@ domain-reads @*/
    Unit sync()
        this.moisture = 1.0;
    end
end
"#;
    let _guard = lock_env();
    unsafe { std::env::remove_var("SMOL_STRICT_EFFECTS") };
    let diags = check_src(src);
    assert!(
        has_code(&diags, DiagnosticCode::EffectContractViolation),
        "expected EffectContractViolation for domain-reads writing domain field, got: {diags:#?}"
    );
    assert!(
        codes(&diags, DiagnosticCode::EffectContractViolation)
            .iter()
            .all(|d| d.severity == Severity::Error),
        "EffectContractViolation must be error-level, got: {diags:#?}"
    );
}

#[test]
fn domain_writes_method_is_silent() {
    let src = r#"
class Plant (domain Double moisture)
    /*@ domain-writes @*/
    Unit sync()
        this.moisture = 1.0;
    end
end
"#;
    let _guard = lock_env();
    unsafe { std::env::remove_var("SMOL_STRICT_EFFECTS") };
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::EffectContractViolation)
            && !has_code(&diags, DiagnosticCode::PureViolation)
            && !has_code(&diags, DiagnosticCode::DomainMutation),
        "domain-writes method writing domain field must be silent, got: {diags:#?}"
    );
}

#[test]
fn unannotated_method_is_silent() {
    let src = r#"
class Plant (domain Double moisture)
    Unit sync()
        this.moisture = 1.0;
    end
end
"#;
    let _guard = lock_env();
    unsafe { std::env::remove_var("SMOL_STRICT_EFFECTS") };
    let diags = check_src(src);
    assert!(
        !has_code(&diags, DiagnosticCode::DomainMutation),
        "unannotated domain write must not produce DomainMutation, got: {diags:#?}"
    );
    assert!(
        !has_code(&diags, DiagnosticCode::EffectContractViolation)
            && !has_code(&diags, DiagnosticCode::PureViolation),
        "unannotated methods must never get the new codes, got: {diags:#?}"
    );
}

#[test]
fn strict_env_does_not_produce_e2019() {
    let src = r#"
class Plant (domain Double moisture)
    Unit sync()
        this.moisture = 1.0;
    end
end
"#;
    let _guard = lock_env();
    unsafe { std::env::set_var("SMOL_STRICT_EFFECTS", "1") };
    let diags = check_src(src);
    // NOTE: still holding the first `_guard` — do NOT lock again (Mutex is
    // not reentrant); clean up the env var before asserting.
    unsafe { std::env::remove_var("SMOL_STRICT_EFFECTS") };
    assert!(
        !has_code(&diags, DiagnosticCode::DomainMutation),
        "strict mode must not produce E2019 for a plain domain write, got: {diags:#?}"
    );
    assert!(
        !has_code(&diags, DiagnosticCode::EffectContractViolation)
            && !has_code(&diags, DiagnosticCode::PureViolation),
        "strict mode must not invent contract codes for unannotated code, got: {diags:#?}"
    );
}

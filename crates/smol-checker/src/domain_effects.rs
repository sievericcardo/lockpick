//! Effect system for `domain` / `hidden` fields (plan §2.6).
//!
//! Inspired by KeY's `assignable` clauses and Crowbar's behavioral effects:
//! methods declare the observable state they may mutate, and the checker
//! enforces the declaration.
//!
//! # Rules
//!
//! 1. **Domain-write effect** (`DomainMutation`, E2019): *removed / not
//!    enforced.* Earlier versions warned when a method with *no* effect
//!    annotation that was *not* marked `domain` wrote a `domain`-modified
//!    field, directly or transitively through the call graph. The verifier
//!    no longer reports this: a method may change a `domain` attribute
//!    without the `domain` keyword. The fixed-point computation of
//!    domain-write effects is retained only to support the `domain-reads`
//!    contract in rule 3.
//! 2. **Hidden-leak effect** (`DomainMutation`, E2019, warning level): a
//!    `links` Turtle fragment must not interpolate a `hidden` field
//!    (`%field` where `field` is declared `hidden`), because `links`
//!    triples are RDF-observable and would expose hidden state.
//! 3. **Effect contracts** (annotated methods only):
//!    - `pure` methods must not write *any* field (directly or
//!      transitively) and must not call a non-`pure`/`domain` method
//!      (directly or transitively) → `PureViolation` (E2027, error).
//!    - `domain-reads` methods must not write a `domain` field (directly
//!      or transitively) → `EffectContractViolation` (E2026, error).
//!    - `domain-writes` methods may write anything (silent).
//!    - Unannotated methods are never flagged with the new contract codes
//!      (E2026/E2027).
//!
//! # Strict mode
//!
//! When the `SMOL_STRICT_EFFECTS=1` environment variable is set, the
//! remaining E2019 `DomainMutation` diagnostics (rule 2, hidden-field
//! leakage into `links`) are upgraded from warning to error severity. Unset
//! (default) preserves warning behavior. The flag is read here (inside this
//! pass) so the `check()` signature stays unchanged.
//!
//! # Soundness contract
//!
//! - Receiver types that cannot be resolved (`Unknown`) never trigger a
//!   diagnostic (no false positives on unresolvable code).
//! - Rule 2 reuses the same `%placeholder` extraction discipline as the
//!   `LinksTypeMismatch` (E5004) check, so the two passes agree on what a
//!   placeholder is.
//! - Rule 3 only fires on explicitly annotated violators; unresolvable
//!   receivers and callees are ignored.

use std::collections::{HashMap, HashSet};

use smol_syntax::ast::{Effect, Expr, FieldModifier, File, Stmt, Type};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

use crate::class_env::ClassEnv;

/// Strict-mode flag: `SMOL_STRICT_EFFECTS=1` upgrades E2019 warnings to errors.
///
/// Read inside this pass so `check()` needs no signature change. Any value
/// other than exactly `"1"` (including unset) preserves warning behavior.
fn strict_effects() -> bool {
    std::env::var("SMOL_STRICT_EFFECTS").is_ok_and(|v| v == "1")
}

/// Build a `DomainMutation` diagnostic — error instead of warning when strict.
fn domain_mutation_diag(message: String) -> Diagnostic {
    if strict_effects() {
        Diagnostic::error(DiagnosticCode::DomainMutation, message)
    } else {
        Diagnostic::warning(DiagnosticCode::DomainMutation, message)
    }
}

/// Map a local variable name to its declared class (for `Named` types only).
fn local_classes(stmts: &[Stmt]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    collect_locals(stmts, &mut map);
    map
}

fn collect_locals(stmts: &[Stmt], map: &mut HashMap<String, String>) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { ty, name, .. } => {
                if let Type::Named(c) = ty {
                    map.insert(name.clone(), c.clone());
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_locals(then_body, map);
                collect_locals(else_body, map);
            }
            Stmt::While { body, .. } => collect_locals(body, map),
            _ => {}
        }
    }
}

/// Resolve the class of a receiver expression, if statically known.
fn receiver_class(
    receiver: &Expr,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> Option<String> {
    match receiver {
        Expr::Var(name, _) => {
            if name == "this" {
                Some(this.to_owned())
            } else if let Some(c) = locals.get(name) {
                Some(c.clone())
            } else {
                // Possibly a field of `this` holding an object; resolve via env.
                env.resolve_field(this, name).and_then(|f| match &f.ty {
                    Type::Named(c) => Some(c.clone()),
                    _ => None,
                })
            }
        }
        Expr::Field {
            receiver, field, ..
        } => {
            let recv = receiver_class(receiver, this, locals, env)?;
            env.resolve_field(&recv, field).and_then(|f| match &f.ty {
                Type::Named(c) => Some(c.clone()),
                _ => None,
            })
        }
        _ => None,
    }
}

/// If `lhs` is a field write `recv.field`, return `(holder_class, field)`.
fn field_write_target(
    lhs: &Expr,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> Option<(String, String)> {
    if let Expr::Field {
        receiver, field, ..
    } = lhs
    {
        let holder = receiver_class(receiver, this, locals, env)?;
        return Some((holder, field.clone()));
    }
    None
}

/// Collect `(holder, method)` call edges inside `stmts`, resolving receivers.
fn collect_calls(
    stmts: &[Stmt],
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
    out: &mut Vec<(String, String)>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { init, .. } => collect_calls_expr(init, this, locals, env, out),
            Stmt::Assign { lhs, rhs, .. } => {
                collect_calls_expr(lhs, this, locals, env, out);
                collect_calls_expr(rhs, this, locals, env, out);
            }
            Stmt::Expr { expr, .. }
            | Stmt::Print { value: expr, .. }
            | Stmt::Destroy { value: expr, .. }
            | Stmt::Adapt { expr, .. } => collect_calls_expr(expr, this, locals, env, out),
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    collect_calls_expr(v, this, locals, env, out);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                collect_calls_expr(cond, this, locals, env, out);
                collect_calls(then_body, this, locals, env, out);
                collect_calls(else_body, this, locals, env, out);
            }
            Stmt::While { cond, body, .. } => {
                collect_calls_expr(cond, this, locals, env, out);
                collect_calls(body, this, locals, env, out);
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
}

fn collect_calls_expr(
    expr: &Expr,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
    out: &mut Vec<(String, String)>,
) {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            if let Some(holder) = receiver_class(receiver, this, locals, env) {
                out.push((holder, method.clone()));
            }
            collect_calls_expr(receiver, this, locals, env, out);
            for a in args {
                collect_calls_expr(a, this, locals, env, out);
            }
        }
        Expr::Field { receiver, .. } => collect_calls_expr(receiver, this, locals, env, out),
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Construct { args, .. }
        | Expr::Access { args, .. }
        | Expr::Simulate { args, .. }
        | Expr::Super { args, .. } => {
            for a in args {
                collect_calls_expr(a, this, locals, env, out);
            }
        }
        Expr::BinOp { left, right, .. } => {
            collect_calls_expr(left, this, locals, env, out);
            collect_calls_expr(right, this, locals, env, out);
        }
        Expr::UnaryOp { expr, .. } | Expr::Classify { expr, .. } => {
            collect_calls_expr(expr, this, locals, env, out);
        }
        Expr::Port { receiver, .. } => collect_calls_expr(receiver, this, locals, env, out),
        _ => {}
    }
}

/// True if `stmts` directly assign to a `domain` field of any object.
fn directly_writes_domain(
    stmts: &[Stmt],
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> bool {
    let mut found = false;
    check_direct(stmts, this, locals, env, &mut found);
    found
}

fn check_direct(
    stmts: &[Stmt],
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
    found: &mut bool,
) {
    for stmt in stmts {
        if *found {
            return;
        }
        match stmt {
            Stmt::Assign { lhs, .. } => {
                if let Some((holder, field)) = field_write_target(lhs, this, locals, env)
                    && let Some(decl) = env.resolve_field(&holder, &field)
                    && decl.modifier == FieldModifier::Domain
                {
                    *found = true;
                    return;
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                check_direct(then_body, this, locals, env, found);
                check_direct(else_body, this, locals, env, found);
            }
            Stmt::While { body, .. } => check_direct(body, this, locals, env, found),
            _ => {}
        }
    }
}

/// True if `stmts` directly assign to *any* resolvable field.
fn directly_writes_any(
    stmts: &[Stmt],
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> bool {
    let mut found = false;
    check_any_direct(stmts, this, locals, env, &mut found);
    found
}

fn check_any_direct(
    stmts: &[Stmt],
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
    found: &mut bool,
) {
    for stmt in stmts {
        if *found {
            return;
        }
        match stmt {
            Stmt::Assign { lhs, .. } => {
                if let Some((holder, field)) = field_write_target(lhs, this, locals, env)
                    && env.resolve_field(&holder, &field).is_some()
                {
                    *found = true;
                    return;
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                check_any_direct(then_body, this, locals, env, found);
                check_any_direct(else_body, this, locals, env, found);
            }
            Stmt::While { body, .. } => check_any_direct(body, this, locals, env, found),
            _ => {}
        }
    }
}

/// Extract `%word` placeholders from a Turtle fragment (same discipline as
/// the E5004 `links` check in `smol-model`).
fn extract_placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '%' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next.is_ascii_alphabetic() || next == '_' {
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '_') {
                    j += 1;
                }
                out.push(bytes[i + 1..j].iter().collect());
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Run the domain-effect pass over all classes.
#[must_use]
pub fn check_domain_effects(files: &[File], env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Index every concrete method body for the fixed-point computation.
    // Key: (holder class, method name) resolved through inheritance at the
    // call site, so we store per-declaring-class entries and resolve callees
    // with `resolve_method`.
    struct MethodInfo {
        this: String,
        name: String,
        is_domain: bool,
        effect: Option<Effect>,
        calls: Vec<(String, String)>,
        direct_write: bool,
        direct_any_write: bool,
        span: smol_syntax::span::Span,
    }
    let mut methods: Vec<MethodInfo> = Vec::new();
    // (class, method) -> index into `methods` (first declaration wins).
    let mut index: HashMap<(String, String), usize> = HashMap::new();

    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                let locals = local_classes(&method.body);
                let mut calls = Vec::new();
                collect_calls(&method.body, &cls.name, &locals, env, &mut calls);
                let direct_write = directly_writes_domain(&method.body, &cls.name, &locals, env);
                let direct_any_write = directly_writes_any(&method.body, &cls.name, &locals, env);
                index
                    .entry((cls.name.clone(), method.name.clone()))
                    .or_insert_with(|| {
                        methods.push(MethodInfo {
                            this: cls.name.clone(),
                            name: method.name.clone(),
                            is_domain: method.flags.is_domain,
                            effect: method.flags.effect,
                            calls,
                            direct_write,
                            direct_any_write,
                            span: method.span,
                        });
                        methods.len() - 1
                    });
            }
        }
    }

    // Fixed-point: a method has the domain-write effect if it writes directly
    // or calls a method that has the effect. Resolution follows inheritance.
    let mut has_effect = vec![false; methods.len()];
    for (i, m) in methods.iter().enumerate() {
        has_effect[i] = m.direct_write;
    }
    // Same propagation for writes to *any* field (for `pure` contracts).
    let mut has_any_write = vec![false; methods.len()];
    for (i, m) in methods.iter().enumerate() {
        has_any_write[i] = m.direct_any_write;
    }
    // Map each method to its resolved callee indices (computed once).
    let mut callees: Vec<Vec<usize>> = vec![Vec::new(); methods.len()];
    for (i, m) in methods.iter().enumerate() {
        let mut seen = HashSet::new();
        for (holder, name) in &m.calls {
            // Walk the holder's ancestry to find the declaring class.
            for anc in env.ancestors(holder) {
                if let Some(&j) = index.get(&(anc.to_owned(), name.clone()))
                    && seen.insert(j)
                {
                    callees[i].push(j);
                    break;
                }
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..methods.len() {
            if has_effect[i] {
                continue;
            }
            if callees[i].iter().any(|&j| has_effect[j]) {
                has_effect[i] = true;
                changed = true;
            }
        }
    }
    // Propagate any-field writes through the same call graph.
    let mut changed_any = true;
    while changed_any {
        changed_any = false;
        for i in 0..methods.len() {
            if has_any_write[i] {
                continue;
            }
            if callees[i].iter().any(|&j| has_any_write[j]) {
                has_any_write[i] = true;
                changed_any = true;
            }
        }
    }

    // A `pure` method must not call a non-`pure`/`domain` method. A callee
    // counts as bad when it is `domain`-modified or its effect is not
    // exactly `Pure` (unannotated callees are treated as impure). Callees
    // that resolve in `env` but have no indexed body (e.g. abstract) are
    // judged by their declaration flags; unresolvable callees are ignored.
    let callee_is_bad = |holder: &str, name: &str| -> bool {
        for anc in env.ancestors(holder) {
            if let Some(&j) = index.get(&(anc.to_owned(), name.to_owned())) {
                return methods[j].is_domain || methods[j].effect != Some(Effect::Pure);
            }
        }
        if let Some(decl) = env.resolve_method(holder, name) {
            return decl.flags.is_domain || decl.flags.effect != Some(Effect::Pure);
        }
        false
    };
    let mut has_bad_call = vec![false; methods.len()];
    for (i, m) in methods.iter().enumerate() {
        has_bad_call[i] = m.calls.iter().any(|(h, n)| callee_is_bad(h, n));
    }
    // Transitive closure: calling a method that itself writes (any field)
    // or calls a bad callee is also a bad call.
    let mut changed_bad = true;
    while changed_bad {
        changed_bad = false;
        for i in 0..methods.len() {
            if has_bad_call[i] {
                continue;
            }
            if callees[i]
                .iter()
                .any(|&j| has_bad_call[j] || has_any_write[j])
            {
                has_bad_call[i] = true;
                changed_bad = true;
            }
        }
    }

    // Rule 3: effect contracts (annotated methods only — error level).
    for (i, m) in methods.iter().enumerate() {
        let info = env.get(&m.this);
        let file = info.map_or("<unknown>", |c| c.file.as_str());
        match m.effect {
            Some(Effect::Pure) => {
                if has_any_write[i] {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::PureViolation,
                            format!(
                                "pure method `{}` writes a field (directly or transitively) \
                                 but `pure` methods must not mutate any state",
                                m.name
                            ),
                        )
                        .with_file(file)
                        .with_span(m.span),
                    );
                }
                if has_bad_call[i] {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::PureViolation,
                            format!(
                                "pure method `{}` calls a non-`pure` or `domain` method \
                                 (directly or transitively) but `pure` methods must only \
                                 call `pure` methods",
                                m.name
                            ),
                        )
                        .with_file(file)
                        .with_span(m.span),
                    );
                }
            }
            Some(Effect::DomainReads) => {
                if has_effect[i] {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::EffectContractViolation,
                            format!(
                                "method `{}` is declared `domain-reads` but writes a \
                                 `domain` field (directly or transitively)",
                                m.name
                            ),
                        )
                        .with_file(file)
                        .with_span(m.span),
                    );
                }
            }
            // `domain-writes` may write anything (silent); unannotated
            // methods keep the E2019 behavior below.
            Some(Effect::DomainWrites) | None => {}
        }
    }

    // Rule 1 (domain-write effect warning for non-`domain` methods) has been
    // removed: the verifier no longer reports E2019 for direct or transitive
    // writes to `domain` fields. The fixed-point results above are kept only
    // because rule 3 (`domain-reads`, `pure`) still consumes them.

    // Rule 2: hidden fields must not be interpolated into `links` fragments.
    for file in files {
        for cls in &file.classes {
            for link in &cls.links {
                for name in extract_placeholders(&link.turtle) {
                    if name == "this" {
                        continue;
                    }
                    if let Some(decl) = env.resolve_field(&cls.name, &name)
                        && decl.modifier == FieldModifier::Hidden
                    {
                        diags.push(
                            domain_mutation_diag(format!(
                                "`links` annotation interpolates hidden field `%{name}`; \
                                 hidden state would be exposed in the RDF knowledge graph"
                            ))
                            .with_file(&file.path)
                            .with_span(link.span),
                        );
                    }
                }
            }
        }
    }

    diags
}

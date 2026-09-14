//! Hoare-contract checking: weakest-precondition VCs over `requires`/`ensures`
//! discharged by a syntactic prover (plan §2.9, codes 6000–6999).
//!
//! For every method carrying `requires`/`ensures` clauses this pass performs
//! two checks:
//!
//! 1. **Postconditions (`E6002 PostconditionViolated`):** the weakest
//!    precondition of each `return` site is computed by forward constant
//!    propagation — a terminating, degenerate form of WP over straight-line
//!    code. `VarDecl`/`Assign` extend a constant map, `If` is followed only
//!    when its condition folds to a constant, and statements without local
//!    effects (`Expr`, `Print`, `Destroy`, `Adapt`, `Skip`, `Breakpoint`) are
//!    ignored. Each `ensures` clause is then substituted with the returned
//!    value for `result` and constant-folded; a clause provably `false` is
//!    reported at the `return` span.
//! 2. **Preconditions (`E6001 PreconditionUnsatisfied`):** at every
//!    `receiver.method(args)` call site whose callee declares `requires`,
//!    actual arguments are substituted for formal parameters and folded; a
//!    clause provably `false` is reported at the call span.
//!
//! `while` loops are skipped conservatively (there is no invariant language
//! yet — termination side conditions live with `E5012`); anything that does
//! not fold to a constant stays silent.
//!
//! # Soundness contract (no false positives)
//!
//! A diagnostic is emitted only when a contract clause folds to the boolean
//! literal `false` after substituting tracked constants. Unknown values
//! propagate as *unknown* (Kleene three-valued logic), never as `false`, so
//! the pass stays silent on anything it cannot prove. In particular,
//! contract-free programs yield zero diagnostics.
//!
//! Termination: every walk is structural over the finite AST (`while` bodies
//! are visited once, never to a fixed point), so the pass always terminates.

use std::collections::HashMap;

use smol_syntax::ast::{BinOp, Expr, File, MethodDecl, Stmt, Type, UnaryOp};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::class_env::ClassEnv;

/// Check Hoare contracts over all method bodies, field initialisers, and the
/// `main` block.
///
/// Contract-free code produces no diagnostics.
#[must_use]
pub fn check_hoare(files: &[File], env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for file in files {
        for cls in &file.classes {
            // Field initialisers may contain calls with contracts.
            let mut types = HashMap::new();
            types.insert("this".to_owned(), cls.name.clone());
            for init in &cls.init_fields {
                visit_expr_for_calls(&init.init, Some(cls.name.as_str()), &types, &file.path, env, &mut diags);
            }
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                check_postconditions(method, &file.path, &mut diags);
                let mut call_types = method_call_types(cls.name.as_str(), method);
                check_calls_in_stmts(
                    &method.body,
                    Some(cls.name.as_str()),
                    &mut call_types,
                    &file.path,
                    env,
                    &mut diags,
                );
            }
        }
        if let Some(main) = &file.main {
            let mut call_types = HashMap::new();
            check_calls_in_stmts(main, None, &mut call_types, &file.path, env, &mut diags);
        }
    }
    diags
}

// ── Postconditions (E6002) ────────────────────────────────────────────────

/// One reachable `return` site with the constant map in force there.
struct ReturnSite {
    value: Option<Expr>,
    span: Span,
    consts: HashMap<String, Expr>,
}

fn check_postconditions(method: &MethodDecl, file: &str, diags: &mut Vec<Diagnostic>) {
    if method.ensures.is_empty() {
        return;
    }
    // Only pure (variable/constant) contracts are in the syntactic fragment.
    if !method.ensures.iter().all(contract_is_pure) {
        return;
    }
    let mut consts = HashMap::new();
    let mut returns = Vec::new();
    if !collect_returns(&method.body, &mut consts, &mut returns) {
        return; // unknown control flow (branch/loop) → silent
    }
    for site in &returns {
        // Vacuity: contradictory `requires` make every postcondition hold.
        let vacuous = method
            .requires
            .iter()
            .any(|r| eval_bool(&subst_expr(r, &site.consts)) == Some(false));
        if vacuous {
            continue;
        }
        for post in &method.ensures {
            let mut vc = subst_expr(post, &site.consts);
            if let Some(ret) = &site.value {
                let mut ret_map = HashMap::new();
                ret_map.insert("result".to_owned(), ret.clone());
                vc = subst_expr(&vc, &ret_map);
            } else if mentions_var(&vc, "result") {
                continue; // `result` with no value is unprovable → silent
            }
            if eval_bool(&vc) == Some(false) {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::PostconditionViolated,
                        format!(
                            "postcondition of `{}` violated: the returned value \
                             provably falsifies an `ensures` clause",
                            method.name
                        ),
                    )
                    .with_file(file)
                    .with_span(site.span),
                );
                break; // one diagnostic per return site
            }
        }
    }
}

/// Walk straight-line code, tracking constants. Returns `false` when control
/// flow is not statically resolvable (non-constant `if`, any `while`).
/// Statements after a top-level `return` are unreachable and ignored.
fn collect_returns(
    stmts: &[Stmt],
    consts: &mut HashMap<String, Expr>,
    out: &mut Vec<ReturnSite>,
) -> bool {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { name, init, .. } => {
                track_binding(name, init, consts);
            }
            Stmt::Assign { lhs, rhs, .. } => {
                if let Expr::Var(name, _) = lhs {
                    track_binding(name, rhs, consts);
                }
            }
            Stmt::Return { value, span } => {
                out.push(ReturnSite {
                    value: value.as_ref().map(|v| subst_expr(v, consts)),
                    span: *span,
                    consts: consts.clone(),
                });
                break; // unreachable tail
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => match eval_bool(&subst_expr(cond, consts)) {
                Some(true) => {
                    if !collect_returns(then_body, consts, out) {
                        return false;
                    }
                    // A `return` inside the taken branch ends this path.
                    if !out.is_empty() && is_terminated(then_body, consts) {
                        break;
                    }
                }
                Some(false) => {
                    if !collect_returns(else_body, consts, out) {
                        return false;
                    }
                    if !out.is_empty() && is_terminated(else_body, consts) {
                        break;
                    }
                }
                None => return false,
            },
            Stmt::While { .. } => return false, // conservative skip: no invariants yet
            Stmt::Expr { .. }
            | Stmt::Adapt { .. }
            | Stmt::Print { .. }
            | Stmt::Destroy { .. }
            | Stmt::Skip { .. }
            | Stmt::Breakpoint { .. } => {}
        }
    }
    true
}

/// Best-effort check whether the last executed statement of a taken branch
/// was a `return` (used to stop after a terminating constant branch).
fn is_terminated(stmts: &[Stmt], consts: &HashMap<String, Expr>) -> bool {
    for stmt in stmts {
        match stmt {
            Stmt::Return { .. } => return true,
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => match eval_bool(&subst_expr(cond, consts)) {
                Some(true) => return is_terminated(then_body, consts),
                Some(false) => return is_terminated(else_body, consts),
                None => return false,
            },
            Stmt::While { .. } => return false,
            _ => {}
        }
    }
    false
}

/// Record `name = rhs` when `rhs` folds to a literal, else forget `name`.
fn track_binding(name: &str, rhs: &Expr, consts: &mut HashMap<String, Expr>) {
    let folded = subst_expr(rhs, consts);
    if is_literal(&folded) {
        consts.insert(name.to_owned(), folded);
    } else {
        consts.remove(name);
    }
}

// ── Preconditions at call sites (E6001) ────────────────────────────────────

/// Seed the receiver-type map: `this` plus parameters of named class type.
fn method_call_types(class: &str, method: &MethodDecl) -> HashMap<String, String> {
    let mut types = HashMap::new();
    types.insert("this".to_owned(), class.to_owned());
    for param in &method.params {
        if let Type::Named(name) = &param.ty {
            types.insert(param.name.clone(), name.clone());
        }
    }
    types
}

fn check_calls_in_stmts(
    stmts: &[Stmt],
    this_class: Option<&str>,
    types: &mut HashMap<String, String>,
    file: &str,
    env: &ClassEnv,
    diags: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { ty, name, init, .. } => {
                visit_expr_for_calls(init, this_class, types, file, env, diags);
                if let Type::Named(class) = ty {
                    types.insert(name.clone(), class.clone());
                } else {
                    types.remove(name);
                }
            }
            Stmt::Assign { lhs, rhs, .. } => {
                visit_expr_for_calls(rhs, this_class, types, file, env, diags);
                if !matches!(lhs, Expr::Var(..)) {
                    visit_expr_for_calls(lhs, this_class, types, file, env, diags);
                }
                if let Expr::Var(name, _) = lhs {
                    match rhs {
                        Expr::New { class, .. } => {
                            types.insert(name.clone(), class.clone());
                        }
                        Expr::Var(other, _) => {
                            if let Some(ty) = types.get(other).cloned() {
                                types.insert(name.clone(), ty);
                            } else {
                                types.remove(name);
                            }
                        }
                        _ => {
                            types.remove(name);
                        }
                    }
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    visit_expr_for_calls(v, this_class, types, file, env, diags);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                visit_expr_for_calls(cond, this_class, types, file, env, diags);
                // Branches share the entry map; merges are over-approximated by
                // keeping the entry (unknown on conflict) — call-site checks
                // only fire on definite violations, so this stays sound.
                let mut then_types = types.clone();
                check_calls_in_stmts(then_body, this_class, &mut then_types, file, env, diags);
                check_calls_in_stmts(else_body, this_class, types, file, env, diags);
            }
            Stmt::While { cond, body, .. } => {
                visit_expr_for_calls(cond, this_class, types, file, env, diags);
                let mut body_types = types.clone();
                check_calls_in_stmts(body, this_class, &mut body_types, file, env, diags);
            }
            Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
                visit_expr_for_calls(value, this_class, types, file, env, diags);
            }
            Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
                visit_expr_for_calls(expr, this_class, types, file, env, diags);
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
}

/// Visit an expression tree; check every `MethodCall` against the callee's
/// `requires` clauses.
fn visit_expr_for_calls(
    expr: &Expr,
    this_class: Option<&str>,
    types: &HashMap<String, String>,
    file: &str,
    env: &ClassEnv,
    diags: &mut Vec<Diagnostic>,
) {
    if let Expr::MethodCall {
        receiver,
        method,
        args,
        span,
    } = expr
    {
        check_call(receiver, method, args, *span, this_class, types, file, env, diags);
    }
    match expr {
        Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => {
            visit_expr_for_calls(receiver, this_class, types, file, env, diags);
        }
        Expr::MethodCall {
            receiver, args, ..
        } => {
            visit_expr_for_calls(receiver, this_class, types, file, env, diags);
            for arg in args {
                visit_expr_for_calls(arg, this_class, types, file, env, diags);
            }
        }
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. }
        | Expr::Construct { args, .. }
        | Expr::Simulate { args, .. } => {
            for arg in args {
                visit_expr_for_calls(arg, this_class, types, file, env, diags);
            }
        }
        Expr::Access {
            connector, args, ..
        } => {
            if let Some(c) = connector {
                visit_expr_for_calls(c, this_class, types, file, env, diags);
            }
            for arg in args {
                visit_expr_for_calls(arg, this_class, types, file, env, diags);
            }
        }
        Expr::Classify { expr, .. } | Expr::UnaryOp { expr, .. } => {
            visit_expr_for_calls(expr, this_class, types, file, env, diags);
        }
        Expr::BinOp { left, right, .. } => {
            visit_expr_for_calls(left, this_class, types, file, env, diags);
            visit_expr_for_calls(right, this_class, types, file, env, diags);
        }
        _ => {}
    }
}

fn check_call(
    receiver: &Expr,
    method: &str,
    args: &[Expr],
    span: Span,
    this_class: Option<&str>,
    types: &HashMap<String, String>,
    file: &str,
    env: &ClassEnv,
    diags: &mut Vec<Diagnostic>,
) {
    let class = match receiver {
        Expr::Var(name, _) => {
            if name == "this" {
                this_class.map(str::to_owned)
            } else {
                types.get(name).cloned()
            }
        }
        _ => None,
    };
    let Some(class) = class else { return };
    let Some(callee) = env.resolve_method(&class, method) else {
        return;
    };
    if callee.requires.is_empty() || callee.params.len() != args.len() {
        return;
    }
    let subst_map: HashMap<String, Expr> = callee
        .params
        .iter()
        .zip(args.iter())
        .map(|(p, a)| (p.name.clone(), a.clone()))
        .collect();
    for req in &callee.requires {
        if eval_bool(&subst_expr(req, &subst_map)) == Some(false) {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::PreconditionUnsatisfied,
                    format!(
                        "precondition of `{method}` unsatisfied: the call arguments \
                         provably violate a `requires` clause"
                    ),
                )
                .with_file(file)
                .with_span(span),
            );
            break; // one diagnostic per call site
        }
    }
}

// ── Syntactic prover: substitution + constant folding ─────────────────────

fn is_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::IntLit(..)
            | Expr::FloatLit(..)
            | Expr::StrLit(..)
            | Expr::BoolLit(..)
            | Expr::Null(_)
    )
}

/// Deep substitution of `Var(name)` occurrences via `map`.
fn subst_expr(expr: &Expr, map: &HashMap<String, Expr>) -> Expr {
    match expr {
        Expr::Var(name, _) => map.get(name).cloned().unwrap_or_else(|| expr.clone()),
        Expr::Field {
            receiver,
            field,
            span,
        } => Expr::Field {
            receiver: Box::new(subst_expr(receiver, map)),
            field: field.clone(),
            span: *span,
        },
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } => Expr::MethodCall {
            receiver: Box::new(subst_expr(receiver, map)),
            method: method.clone(),
            args: args.iter().map(|a| subst_expr(a, map)).collect(),
            span: *span,
        },
        Expr::New {
            class,
            ty_param,
            args,
            span,
        } => Expr::New {
            class: class.clone(),
            ty_param: ty_param.clone(),
            args: args.iter().map(|a| subst_expr(a, map)).collect(),
            span: *span,
        },
        Expr::Construct { sparql, args, span } => Expr::Construct {
            sparql: sparql.clone(),
            args: args.iter().map(|a| subst_expr(a, map)).collect(),
            span: *span,
        },
        Expr::Access {
            sparql,
            connector,
            args,
            span,
        } => Expr::Access {
            sparql: sparql.clone(),
            connector: connector
                .as_ref()
                .map(|c| Box::new(subst_expr(c, map))),
            args: args.iter().map(|a| subst_expr(a, map)).collect(),
            span: *span,
        },
        Expr::Classify { expr, span } => Expr::Classify {
            expr: Box::new(subst_expr(expr, map)),
            span: *span,
        },
        Expr::BuiltinCall { name, args, span } => Expr::BuiltinCall {
            name: name.clone(),
            args: args.iter().map(|a| subst_expr(a, map)).collect(),
            span: *span,
        },
        Expr::Simulate { sparql, args, span } => Expr::Simulate {
            sparql: sparql.clone(),
            args: args.iter().map(|a| subst_expr(a, map)).collect(),
            span: *span,
        },
        Expr::Port {
            receiver,
            port,
            span,
        } => Expr::Port {
            receiver: Box::new(subst_expr(receiver, map)),
            port: port.clone(),
            span: *span,
        },
        Expr::Super { args, span } => Expr::Super {
            args: args.iter().map(|a| subst_expr(a, map)).collect(),
            span: *span,
        },
        Expr::BinOp {
            op,
            left,
            right,
            span,
        } => Expr::BinOp {
            op: *op,
            left: Box::new(subst_expr(left, map)),
            right: Box::new(subst_expr(right, map)),
            span: *span,
        },
        Expr::UnaryOp { op, expr, span } => Expr::UnaryOp {
            op: *op,
            expr: Box::new(subst_expr(expr, map)),
            span: *span,
        },
        _ => expr.clone(),
    }
}

/// `true` if `expr` mentions `Var(name)` anywhere.
fn mentions_var(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Var(n, _) => n == name,
        Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => mentions_var(receiver, name),
        Expr::MethodCall {
            receiver, args, ..
        } => {
            mentions_var(receiver, name) || args.iter().any(|a| mentions_var(a, name))
        }
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. }
        | Expr::Construct { args, .. }
        | Expr::Simulate { args, .. } => args.iter().any(|a| mentions_var(a, name)),
        Expr::Access {
            connector, args, ..
        } => {
            args.iter().any(|a| mentions_var(a, name))
                || connector.as_ref().is_some_and(|c| mentions_var(c, name))
        }
        Expr::Classify { expr, .. } | Expr::UnaryOp { expr, .. } => mentions_var(expr, name),
        Expr::BinOp { left, right, .. } => {
            mentions_var(left, name) || mentions_var(right, name)
        }
        _ => false,
    }
}

/// `true` if `expr` is in the syntactic fragment (variables, literals,
/// boolean/arithmetic connectives only).
fn contract_is_pure(expr: &Expr) -> bool {
    match expr {
        Expr::Var(..)
        | Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StrLit(..)
        | Expr::BoolLit(..)
        | Expr::Null(_) => true,
        Expr::BinOp { left, right, .. } => contract_is_pure(left) && contract_is_pure(right),
        Expr::UnaryOp { expr, .. } => contract_is_pure(expr),
        _ => false,
    }
}

/// Fold an integer expression to a constant. `None` = unknown.
fn eval_int(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::IntLit(n, _) => Some(*n),
        Expr::BinOp {
            op, left, right, ..
        } => {
            let a = eval_int(left)?;
            let b = eval_int(right)?;
            match op {
                BinOp::Add => a.checked_add(b),
                BinOp::Sub => a.checked_sub(b),
                BinOp::Mul => a.checked_mul(b),
                BinOp::Div | BinOp::Mod => {
                    if b == 0 {
                        None
                    } else if *op == BinOp::Div {
                        a.checked_div(b)
                    } else {
                        a.checked_rem(b)
                    }
                }
                _ => None,
            }
        }
        Expr::UnaryOp { op, expr, .. } => match op {
            UnaryOp::Neg => eval_int(expr)?.checked_neg(),
            UnaryOp::Not => None,
        },
        _ => None,
    }
}

/// Numeric constant for comparisons (int or float).
fn eval_num(expr: &Expr) -> Option<f64> {
    if let Some(n) = eval_int(expr) {
        // Only exactly-representable integers coerce, so folding can
        // never round a verdict.
        if (-(1_i64 << 53)..=(1_i64 << 53)).contains(&n) {
            return Some(n as f64);
        }
        return None;
    }
    match expr {
        Expr::FloatLit(x, _) => Some(*x),
        _ => None,
    }
}

/// Fold a boolean expression to a constant (Kleene three-valued logic:
/// `None` = unknown). Only definite `Some(false)` verdicts may become
/// diagnostics.
fn eval_bool(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::BoolLit(b, _) => Some(*b),
        Expr::UnaryOp { op, expr, .. } => match op {
            UnaryOp::Not => eval_bool(expr).map(|b| !b),
            UnaryOp::Neg => None,
        },
        Expr::BinOp {
            op, left, right, ..
        } => match op {
            BinOp::And => match (eval_bool(left), eval_bool(right)) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            },
            BinOp::Or => match (eval_bool(left), eval_bool(right)) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            },
            BinOp::Eq | BinOp::NotEq => eval_equality(left, right).map(|eq| {
                if *op == BinOp::Eq {
                    eq
                } else {
                    !eq
                }
            }),
            BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                let (a, b) = (eval_num(left)?, eval_num(right)?);
                Some(match op {
                    BinOp::Lt => a < b,
                    BinOp::LtEq => a <= b,
                    BinOp::Gt => a > b,
                    BinOp::GtEq => a >= b,
                    _ => unreachable!(),
                })
            }
            _ => None,
        },
        _ => None,
    }
}

/// Fold an equality test. `None` = unknown (non-constant or incomparable).
fn eval_equality(left: &Expr, right: &Expr) -> Option<bool> {
    // Null lattice: `null` equals only `null`.
    let left_null = matches!(left, Expr::Null(_));
    let right_null = matches!(right, Expr::Null(_));
    if left_null || right_null {
        return Some(left_null && right_null);
    }
    if let (Some(a), Some(b)) = (eval_num(left), eval_num(right)) {
        return Some(a == b);
    }
    match (left, right) {
        (Expr::StrLit(a, _), Expr::StrLit(b, _)) => Some(a == b),
        (Expr::BoolLit(a, _), Expr::BoolLit(b, _)) => Some(a == b),
        _ => None,
    }
}

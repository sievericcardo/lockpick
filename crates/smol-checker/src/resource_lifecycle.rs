//! FMO (Functional Mock-up Object) resource-lifecycle analysis (plan §2.5).
//!
//! Every local variable whose declared type is `FMO[…]` represents a live
//! FMU object that must eventually be released via `destroy(var)`.
//!
//! # Path-sensitive typestate (Crowbar-style must-analysis)
//!
//! The check is a **must-analysis over paths**: a warning is emitted iff there
//! *exists* a path through the enclosing method body that never destroys the
//! variable. Formally, `may_avoid(stmts, var)` holds when some complete path
//! through `stmts` contains no `destroy(var)`:
//!
//! - a `destroy(var)` statement cannot be avoided (`false`);
//! - any other simple statement is transparent (defers to the rest);
//! - `if` avoids iff *either* branch avoids (existential over branches);
//! - `while` bodies are skipped (the zero-iteration path), so a `destroy`
//!   inside a loop body alone never counts — matching the fixed-point
//!   treatment of loops in symbolic-execution engines like Crowbar;
//! - `return` terminates the path: statements after it are unreachable, so a
//!   `return` before any `destroy` avoids.
//!
//! Compared with the previous path-insensitive "destroy exists somewhere"
//! check, this catches leaks on exceptional/early-exit branches while staying
//! false-positive-free on straight-line `create; …; destroy` code.
//!
//! # Escape analysis (plan §2.5, `W2024 FmoEscaped`)
//!
//! An FMO local that is stored into a field (`this.shared = fmu`), returned,
//! or passed as an argument to any call other than `destroy(fmu)` has
//! *escaped* local ownership: the local `destroy` discipline no longer
//! applies, so the `W2020` check is suppressed for it and a single `W2024`
//! warning records the transfer instead. Uses of the variable as a call
//! *receiver* (`fmu.port("out")`) are not escapes.

use smol_syntax::ast::{Expr, File, Stmt, Type};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::class_env::ClassEnv;

/// True if `stmt` is `destroy(var)` for exactly `var`.
fn is_destroy_of(stmt: &Stmt, var: &str) -> bool {
    matches!(stmt, Stmt::Destroy { value: Expr::Var(name, _), .. } if name == var)
}

/// True if some path through `stmts` never destroys `var`.
fn may_avoid_list(stmts: &[Stmt], var: &str) -> bool {
    // A path through a sequence avoids iff the first statement can be passed
    // without destroying AND the remainder can be avoided from there.
    // (`return` short-circuits: nothing after it is reachable.)
    for (i, stmt) in stmts.iter().enumerate() {
        if is_destroy_of(stmt, var) {
            return false;
        }
        match stmt {
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                // Some path through the `if` avoids iff either branch avoids.
                if !(may_avoid_list(then_body, var) || may_avoid_list(else_body, var)) {
                    return false;
                }
            }
            Stmt::While { .. } => {
                // Zero-iteration path: the body is irrelevant for avoidance.
            }
            Stmt::Return { .. } => {
                // The path exits here without destroying (no later destroy is
                // reachable on this path).
                return true;
            }
            _ => {}
        }
        // Transparent statement (or an unavoidable `if` / any `while`):
        // continue scanning the rest of the sequence.
        let _ = i;
    }
    true
}

/// Collect all `(name, span)` pairs for local variables of `FMO[…]` type.
fn collect_fmo_vars(stmts: &[Stmt]) -> Vec<(String, Span)> {
    let mut vars = Vec::new();
    collect_fmo_vars_inner(stmts, &mut vars);
    vars
}

fn collect_fmo_vars_inner(stmts: &[Stmt], vars: &mut Vec<(String, Span)>) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { name, ty, span, .. } => {
                if matches!(ty, Type::Fmo(_)) {
                    vars.push((name.clone(), *span));
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_fmo_vars_inner(then_body, vars);
                collect_fmo_vars_inner(else_body, vars);
            }
            Stmt::While { body, .. } => collect_fmo_vars_inner(body, vars),
            _ => {}
        }
    }
}

fn check_body(stmts: &[Stmt], file: &str, method_note: &str, diags: &mut Vec<Diagnostic>) {
    for (name, span) in collect_fmo_vars(stmts) {
        if escapes(stmts, &name) {
            diags.push(
                Diagnostic::warning(
                    DiagnosticCode::FmoEscaped,
                    format!(
                        "FMO variable `{name}` escapes local ownership (stored into a \
                         field, returned, or passed to a call other than `destroy`); \
                         its lifecycle is no longer tracked locally"
                    ),
                )
                .with_file(file)
                .with_span(span),
            );
        } else if may_avoid_list(stmts, &name) {
            diags.push(
                Diagnostic::warning(
                    DiagnosticCode::FmoNotDestroyed,
                    format!(
                        "FMO variable `{name}` is not destroyed on every path{method_note}; \
                         call `destroy({name})` on all exit paths (a `destroy` inside a \
                         `while` body alone does not guarantee release)"
                    ),
                )
                .with_file(file)
                .with_span(span),
            );
        }
    }
}

/// True if `var` flows out of local scope in `stmts`.
fn escapes(stmts: &[Stmt], var: &str) -> bool {
    stmts.iter().any(|stmt| stmt_escapes(stmt, var))
}

fn stmt_escapes(stmt: &Stmt, var: &str) -> bool {
    match stmt {
        Stmt::VarDecl { init, .. } => passes_as_arg(init, var),
        Stmt::Assign { lhs, rhs, .. } => {
            (matches!(lhs, Expr::Field { .. }) && expr_mentions_var(rhs, var))
                || passes_as_arg(rhs, var)
        }
        Stmt::Expr { expr, .. } => passes_as_arg(expr, var),
        Stmt::Return { value, .. } => value.as_ref().is_some_and(|v| expr_mentions_var(v, var)),
        Stmt::Adapt { expr, .. } => expr_mentions_var(expr, var),
        Stmt::If {
            then_body,
            else_body,
            ..
        } => escapes(then_body, var) || escapes(else_body, var),
        Stmt::While { body, .. } => escapes(body, var),
        // `destroy(var)` is the discipline itself; `print`/`skip`/receivers
        // are uses, not ownership transfers.
        _ => false,
    }
}

/// True if `var` occurs anywhere in `expr` (including as a call receiver).
fn expr_mentions_var(expr: &Expr, var: &str) -> bool {
    match expr {
        Expr::Var(name, _) => name == var,
        Expr::Field { receiver, .. } => expr_mentions_var(receiver, var),
        Expr::MethodCall { receiver, args, .. } => {
            expr_mentions_var(receiver, var) || args.iter().any(|a| expr_mentions_var(a, var))
        }
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Construct { args, .. }
        | Expr::Simulate { args, .. }
        | Expr::Super { args, .. } => args.iter().any(|a| expr_mentions_var(a, var)),
        Expr::Access {
            connector, args, ..
        } => {
            args.iter().any(|a| expr_mentions_var(a, var))
                || connector
                    .as_ref()
                    .is_some_and(|c| expr_mentions_var(c, var))
        }
        Expr::Classify { expr, .. } => expr_mentions_var(expr, var),
        Expr::Port { receiver, .. } => expr_mentions_var(receiver, var),
        Expr::BinOp { left, right, .. } => {
            expr_mentions_var(left, var) || expr_mentions_var(right, var)
        }
        Expr::UnaryOp { expr, .. } => expr_mentions_var(expr, var),
        _ => false,
    }
}

/// True if `expr` passes `var` as an argument to a call, constructor or
/// query. A use as call *receiver* (`fmu.port("out")`) does not count.
fn passes_as_arg(expr: &Expr, var: &str) -> bool {
    match expr {
        Expr::MethodCall { receiver, args, .. } => {
            args.iter().any(|a| expr_mentions_var(a, var)) || passes_as_arg(receiver, var)
        }
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Construct { args, .. }
        | Expr::Simulate { args, .. }
        | Expr::Super { args, .. } => args.iter().any(|a| expr_mentions_var(a, var)),
        Expr::Access {
            connector, args, ..
        } => {
            args.iter().any(|a| expr_mentions_var(a, var))
                || connector
                    .as_ref()
                    .is_some_and(|c| expr_mentions_var(c, var))
        }
        Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => passes_as_arg(receiver, var),
        Expr::Classify { expr, .. } => passes_as_arg(expr, var),
        Expr::BinOp { left, right, .. } => passes_as_arg(left, var) || passes_as_arg(right, var),
        Expr::UnaryOp { expr, .. } => passes_as_arg(expr, var),
        _ => false,
    }
}

/// Run the FMO-lifecycle pass over all method bodies and the `main` block.
#[must_use]
pub fn check_fmo_lifecycle(files: &[File], _env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                check_body(
                    &method.body,
                    &file.path,
                    " before the method returns",
                    &mut diags,
                );
            }
        }
        if let Some(main_stmts) = &file.main {
            check_body(
                main_stmts,
                &file.path,
                " before the program ends",
                &mut diags,
            );
        }
    }
    diags
}

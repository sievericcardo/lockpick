//! Flow-sensitive null-safety analysis.
//!
//! Tracks the *definite* nullness of local variables and flags dereferences of
//! a variable that is provably `null` at that point.  See module-level docs in
//! FORMAL_VERIFICATION_PLAN.md §2.1 for design rationale.
//!
//! # Soundness contract (no false positives)
//!
//! - Variables assigned `null` → `Null`; `new …` / literals → `NonNull`;
//!   everything else (params, field reads, call returns) → `Maybe` (not flagged)
//! - `if x != null` / `while x != null` narrows `x` to `NonNull` inside the
//!   branch/body.  `if x == null` narrows to `Null` (then) / `NonNull` (else).
//! - Only a **direct-variable receiver** that is `Null` triggers a diagnostic.

use std::collections::{HashMap, HashSet};

use smol_syntax::ast::{BinOp, Expr, File, Stmt};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::class_env::ClassEnv;

#[derive(Clone, PartialEq, Eq, Debug)]
enum Nullness {
    Null,
    NonNull,
    Maybe,
}

type NullEnv = HashMap<String, Nullness>;

fn join(a: Nullness, b: Nullness) -> Nullness {
    if a == b { a } else { Nullness::Maybe }
}

fn nullness_of(expr: &Expr, env: &NullEnv) -> Nullness {
    match expr {
        Expr::Null(_) => Nullness::Null,
        Expr::New { .. }
        | Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StrLit(..)
        | Expr::BoolLit(..) => Nullness::NonNull,
        Expr::Var(name, _) => env.get(name.as_str()).cloned().unwrap_or(Nullness::Maybe),
        _ => Nullness::Maybe,
    }
}

fn narrow(env: &mut NullEnv, cond: &Expr, branch_is_true: bool) {
    let Expr::BinOp {
        op, left, right, ..
    } = cond
    else {
        return;
    };
    let var_name = match (left.as_ref(), right.as_ref()) {
        (Expr::Var(n, _), Expr::Null(_)) | (Expr::Null(_), Expr::Var(n, _)) => n.clone(),
        _ => return,
    };
    match op {
        BinOp::NotEq => {
            if branch_is_true {
                env.insert(var_name, Nullness::NonNull);
            }
        }
        BinOp::Eq => {
            env.insert(
                var_name,
                if branch_is_true {
                    Nullness::Null
                } else {
                    Nullness::NonNull
                },
            );
        }
        _ => {}
    }
}

fn assigned_in(stmts: &[Stmt]) -> HashSet<String> {
    let mut vars = HashSet::new();
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { name, .. } => {
                vars.insert(name.clone());
            }
            Stmt::Assign { lhs, .. } => {
                if let Expr::Var(name, _) = lhs {
                    vars.insert(name.clone());
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                vars.extend(assigned_in(then_body));
                vars.extend(assigned_in(else_body));
            }
            Stmt::While { body, .. } => {
                vars.extend(assigned_in(body));
            }
            _ => {}
        }
    }
    vars
}

fn check_var_receiver(
    receiver: &Expr,
    env: &NullEnv,
    file: &str,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    if let Expr::Var(name, _) = receiver {
        if env.get(name.as_str()) == Some(&Nullness::Null) {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::NullDereference,
                    format!("`{name}` is null at this point"),
                )
                .with_file(file)
                .with_span(span),
            );
        }
    }
}

fn check_expr(expr: &Expr, env: &NullEnv, file: &str, diags: &mut Vec<Diagnostic>) {
    match expr {
        Expr::Field { receiver, span, .. } => {
            check_var_receiver(receiver, env, file, *span, diags);
            check_expr(receiver, env, file, diags);
        }
        Expr::MethodCall {
            receiver,
            args,
            span,
            ..
        } => {
            check_var_receiver(receiver, env, file, *span, diags);
            check_expr(receiver, env, file, diags);
            for arg in args {
                check_expr(arg, env, file, diags);
            }
        }
        Expr::New { args, .. }
        | Expr::Construct { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. } => {
            for arg in args {
                check_expr(arg, env, file, diags);
            }
        }
        Expr::Access {
            connector, args, ..
        } => {
            if let Some(c) = connector {
                check_expr(c, env, file, diags);
            }
            for arg in args {
                check_expr(arg, env, file, diags);
            }
        }
        Expr::Simulate { args, .. } => {
            for arg in args {
                check_expr(arg, env, file, diags);
            }
        }
        Expr::Classify { expr, .. } | Expr::UnaryOp { expr, .. } => {
            check_expr(expr, env, file, diags);
        }
        Expr::BinOp { left, right, .. } => {
            check_expr(left, env, file, diags);
            check_expr(right, env, file, diags);
        }
        Expr::Port { receiver, .. } => {
            check_expr(receiver, env, file, diags);
        }
        _ => {}
    }
}

fn check_stmts(stmts: &[Stmt], env: &mut NullEnv, file: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { name, init, .. } => {
                check_expr(init, env, file, &mut diags);
                env.insert(name.clone(), nullness_of(init, env));
            }
            Stmt::Assign { lhs, rhs, .. } => {
                check_expr(rhs, env, file, &mut diags);
                if !matches!(lhs, Expr::Var(..)) {
                    check_expr(lhs, env, file, &mut diags);
                }
                if let Expr::Var(name, _) = lhs {
                    env.insert(name.clone(), nullness_of(rhs, env));
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    check_expr(v, env, file, &mut diags);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                check_expr(cond, env, file, &mut diags);

                let mut then_env = env.clone();
                narrow(&mut then_env, cond, true);
                diags.extend(check_stmts(then_body, &mut then_env, file));

                let mut else_env = env.clone();
                narrow(&mut else_env, cond, false);
                diags.extend(check_stmts(else_body, &mut else_env, file));

                let all: HashSet<String> =
                    then_env.keys().chain(else_env.keys()).cloned().collect();
                for name in all {
                    let tn = then_env.get(&name).cloned().unwrap_or(Nullness::Maybe);
                    let en = else_env.get(&name).cloned().unwrap_or(Nullness::Maybe);
                    env.insert(name, join(tn, en));
                }
            }
            Stmt::While { cond, body, .. } => {
                check_expr(cond, env, file, &mut diags);
                let mut body_env = env.clone();
                narrow(&mut body_env, cond, true);
                diags.extend(check_stmts(body, &mut body_env, file));
                for name in assigned_in(body) {
                    env.insert(name, Nullness::Maybe);
                }
            }
            Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
                check_expr(value, env, file, &mut diags);
            }
            Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
                check_expr(expr, env, file, &mut diags);
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
    diags
}

/// Run the null-safety pass over all method bodies and the `main` block.
#[must_use]
pub fn check_null_safety(files: &[File], _env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                let mut null_env = NullEnv::new();
                null_env.insert("this".to_owned(), Nullness::NonNull);
                diags.extend(check_stmts(&method.body, &mut null_env, &file.path));
            }
        }
        if let Some(main_stmts) = &file.main {
            let mut null_env = NullEnv::new();
            diags.extend(check_stmts(main_stmts, &mut null_env, &file.path));
        }
    }
    diags
}

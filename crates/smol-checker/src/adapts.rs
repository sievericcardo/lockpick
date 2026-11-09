//! `adapts` modifier checks.
//!
//! The `adapts Foo, Bar` modifier on a method declares the list of classes that
//! are reclassified at runtime by that method (e.g.
//! `override Unit adaptAddition() adapts HealthState, Plant`).
//!
//! Two rules are enforced here:
//!
//! 1. **Presence**: a method that invokes `adapt`/`classify` — directly, or
//!    transitively through a method it calls (resolved statically) — must
//!    declare a non-empty `adapts`.
//! 2. **Override containment**: an overriding method's `adapts` classes must be
//!    a subset of the supermethod's `adapts` classes.

use std::collections::HashMap;

use smol_syntax::ast::{Expr, File, MethodDecl, Stmt, Type};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

use crate::class_env::ClassEnv;

/// Check all `adapts` rules across a project.
#[must_use]
pub fn check_adapts(files: &[File], env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Method identity: (class name, method name).
    let mut direct: HashMap<(String, String), bool> = HashMap::new();
    let mut calls: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();

    for file in files {
        for cls in &file.classes {
            for m in &cls.methods {
                let key = (cls.name.clone(), m.name.clone());
                if m.flags.is_abstract {
                    direct.insert(key.clone(), false);
                    calls.insert(key, Vec::new());
                    continue;
                }
                let mut locals = HashMap::new();
                for p in &m.params {
                    locals.insert(p.name.clone(), p.ty.clone());
                }
                collect_locals(&m.body, &mut locals);
                direct.insert(key.clone(), has_direct_adapt_classify(&m.body));
                calls.insert(key, collect_calls(&m.body, &cls.name, env, &locals));
            }
        }
    }

    // Fixed-point: a method uses `adapt`/`classify` if it does so directly or if
    // any statically-resolved method it calls uses them.
    let mut usage: HashMap<(String, String), bool> =
        direct.keys().map(|k| (k.clone(), false)).collect();
    loop {
        let mut changed = false;
        for k in direct.keys() {
            if usage[k] {
                continue;
            }
            let uses = direct[k]
                || calls
                    .get(k)
                    .is_some_and(|cs| cs.iter().any(|t| usage.get(t).copied().unwrap_or(false)));
            if uses {
                usage.insert(k.clone(), true);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // 1. Presence.
    for file in files {
        for cls in &file.classes {
            for m in &cls.methods {
                if m.flags.is_abstract {
                    continue;
                }
                let key = (cls.name.clone(), m.name.clone());
                if usage.get(&key).copied().unwrap_or(false) && m.adapts.is_empty() {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::MissingAdapts,
                            format!(
                                "method `{}::{}` invokes `adapt`/`classify` (directly or via a \
                                 called method) but does not declare `adapts` — it must list the \
                                 classes reclassified at runtime",
                                cls.name, m.name
                            ),
                        )
                        .with_file(&file.path)
                        .with_span(m.span),
                    );
                }
            }
        }
    }

    // 2. Override containment.
    for file in files {
        for cls in &file.classes {
            for m in &cls.methods {
                if !m.flags.is_override {
                    continue;
                }
                let Some(parent) = cls.extends.as_deref() else {
                    continue;
                };
                let Some((super_cls, super_m)) = resolve_method_target(env, parent, &m.name) else {
                    continue;
                };
                for a in &m.adapts {
                    if !super_m.adapts.contains(a) {
                        diags.push(
                            Diagnostic::error(
                                DiagnosticCode::MissingAdapts,
                                format!(
                                    "overriding method `{}::{}` declares `adapts {a}` but the \
                                     supermethod `{}::{}` does not — the supermethod must list all \
                                     classes reclassified by its overrides",
                                    cls.name, m.name, super_cls, m.name
                                ),
                            )
                            .with_file(&file.path)
                            .with_span(m.span),
                        );
                    }
                }
            }
        }
    }

    diags
}

/// Resolve `name` to the method it refers to in `class` (walking the
/// inheritance chain), returning the class that declares it together with the
/// method.
fn resolve_method_target<'a>(
    env: &'a ClassEnv,
    class: &str,
    name: &str,
) -> Option<(String, &'a MethodDecl)> {
    let mut cur = class;
    loop {
        let info = env.get(cur)?;
        if let Some(m) = info.own_method(name) {
            return Some((cur.to_owned(), m));
        }
        cur = info.extends.as_deref()?;
    }
}

/// Collect the declared types of local variables (`Type name = …;`) so method
/// calls on them can be resolved.
fn collect_locals(body: &[Stmt], locals: &mut HashMap<String, Type>) {
    for stmt in body {
        collect_locals_stmt(stmt, locals);
    }
}

fn collect_locals_stmt(stmt: &Stmt, locals: &mut HashMap<String, Type>) {
    match stmt {
        Stmt::VarDecl { ty, name, .. } => {
            locals.insert(name.clone(), ty.clone());
        }
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            collect_locals(then_body, locals);
            collect_locals(else_body, locals);
        }
        Stmt::While { body, .. } => collect_locals(body, locals),
        _ => {}
    }
}

/// Does the body use `adapt` or `classify` directly?
fn has_direct_adapt_classify(body: &[Stmt]) -> bool {
    body.iter().any(has_adapt_classify_stmt)
}

fn has_adapt_classify_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Adapt { .. } => true,
        Stmt::VarDecl { init, .. } => has_adapt_classify_expr(init),
        Stmt::Assign { lhs, rhs, .. } => {
            has_adapt_classify_expr(lhs) || has_adapt_classify_expr(rhs)
        }
        Stmt::Expr { expr, .. } => has_adapt_classify_expr(expr),
        Stmt::Return { value: Some(e), .. } => has_adapt_classify_expr(e),
        Stmt::If {
            cond,
            then_body,
            else_body,
            ..
        } => {
            has_adapt_classify_expr(cond)
                || then_body.iter().any(has_adapt_classify_stmt)
                || else_body.iter().any(has_adapt_classify_stmt)
        }
        Stmt::While { cond, body, .. } => {
            has_adapt_classify_expr(cond) || body.iter().any(has_adapt_classify_stmt)
        }
        Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => has_adapt_classify_expr(value),
        Stmt::Skip { .. } | Stmt::Breakpoint { .. } | Stmt::Return { value: None, .. } => false,
    }
}

fn has_adapt_classify_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Classify { .. } => true,
        Expr::MethodCall { receiver, args, .. } => {
            has_adapt_classify_expr(receiver) || args.iter().any(has_adapt_classify_expr)
        }
        Expr::Field { receiver, .. } => has_adapt_classify_expr(receiver),
        Expr::BinOp { left, right, .. } => {
            has_adapt_classify_expr(left) || has_adapt_classify_expr(right)
        }
        Expr::UnaryOp { expr: inner, .. } => has_adapt_classify_expr(inner),
        Expr::New { args, .. } => args.iter().any(has_adapt_classify_expr),
        Expr::Construct { args, .. }
        | Expr::Access { args, .. }
        | Expr::Simulate { args, .. }
        | Expr::Super { args, .. }
        | Expr::BuiltinCall { args, .. } => args.iter().any(has_adapt_classify_expr),
        Expr::Port { receiver, .. } => has_adapt_classify_expr(receiver),
        _ => false,
    }
}

/// Collect the set of `(class, method)` targets the body calls, resolving each
/// receiver's type statically.
fn collect_calls(
    body: &[Stmt],
    this: &str,
    env: &ClassEnv,
    locals: &HashMap<String, Type>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for stmt in body {
        collect_calls_stmt(stmt, this, env, locals, &mut out);
    }
    out
}

fn collect_calls_stmt(
    stmt: &Stmt,
    this: &str,
    env: &ClassEnv,
    locals: &HashMap<String, Type>,
    out: &mut Vec<(String, String)>,
) {
    match stmt {
        Stmt::VarDecl { init, .. } => collect_calls_expr(init, this, env, locals, out),
        Stmt::Assign { lhs, rhs, .. } => {
            collect_calls_expr(lhs, this, env, locals, out);
            collect_calls_expr(rhs, this, env, locals, out);
        }
        Stmt::Expr { expr, .. } => collect_calls_expr(expr, this, env, locals, out),
        Stmt::Return { value: Some(e), .. } => collect_calls_expr(e, this, env, locals, out),
        Stmt::If {
            cond,
            then_body,
            else_body,
            ..
        } => {
            collect_calls_expr(cond, this, env, locals, out);
            for s in then_body {
                collect_calls_stmt(s, this, env, locals, out);
            }
            for s in else_body {
                collect_calls_stmt(s, this, env, locals, out);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_calls_expr(cond, this, env, locals, out);
            for s in body {
                collect_calls_stmt(s, this, env, locals, out);
            }
        }
        Stmt::Print { value, .. }
        | Stmt::Destroy { value, .. }
        | Stmt::Adapt { expr: value, .. } => collect_calls_expr(value, this, env, locals, out),
        Stmt::Skip { .. } | Stmt::Breakpoint { .. } | Stmt::Return { value: None, .. } => {}
    }
}

fn collect_calls_expr(
    expr: &Expr,
    this: &str,
    env: &ClassEnv,
    locals: &HashMap<String, Type>,
    out: &mut Vec<(String, String)>,
) {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let rt = resolve_receiver_type(env, locals, this, receiver);
            if let Type::Named(c) = &rt
                && let Some((tc, _)) = resolve_method_target(env, c, method)
            {
                out.push((tc, method.clone()));
            }
            collect_calls_expr(receiver, this, env, locals, out);
            for a in args {
                collect_calls_expr(a, this, env, locals, out);
            }
        }
        Expr::Field { receiver, .. } => collect_calls_expr(receiver, this, env, locals, out),
        Expr::BinOp { left, right, .. } => {
            collect_calls_expr(left, this, env, locals, out);
            collect_calls_expr(right, this, env, locals, out);
        }
        Expr::UnaryOp { expr: inner, .. } => collect_calls_expr(inner, this, env, locals, out),
        Expr::New { args, .. } => {
            for a in args {
                collect_calls_expr(a, this, env, locals, out);
            }
        }
        Expr::Construct { args, .. }
        | Expr::Access { args, .. }
        | Expr::Simulate { args, .. }
        | Expr::Super { args, .. }
        | Expr::BuiltinCall { args, .. } => {
            for a in args {
                collect_calls_expr(a, this, env, locals, out);
            }
        }
        Expr::Classify { expr: inner, .. } => collect_calls_expr(inner, this, env, locals, out),
        Expr::Port { receiver, .. } => collect_calls_expr(receiver, this, env, locals, out),
        _ => {}
    }
}

/// Lightweight receiver-type resolution for method-call targets.
fn resolve_receiver_type(
    env: &ClassEnv,
    locals: &HashMap<String, Type>,
    this: &str,
    expr: &Expr,
) -> Type {
    match expr {
        Expr::Var(name, _) => {
            if name == "this" {
                Type::Named(this.to_owned())
            } else if let Some(t) = locals.get(name) {
                t.clone()
            } else if let Some(f) = env.resolve_field(this, name) {
                f.ty.clone()
            } else {
                Type::Named("_unknown".to_owned())
            }
        }
        Expr::Field {
            receiver, field, ..
        } => {
            let rt = resolve_receiver_type(env, locals, this, receiver);
            match rt {
                Type::List(inner) => match field.as_str() {
                    "content" => (*inner).clone(),
                    "next" => Type::List(inner),
                    _ => Type::Named("_unknown".to_owned()),
                },
                Type::ExplList(inner) => match field.as_str() {
                    "content" => (*inner).clone(),
                    "next" => Type::ExplList(inner),
                    _ => Type::Named("_unknown".to_owned()),
                },
                Type::Named(c) => env
                    .resolve_field(&c, field)
                    .map(|f| f.ty.clone())
                    .unwrap_or_else(|| Type::Named("_unknown".to_owned())),
                _ => Type::Named("_unknown".to_owned()),
            }
        }
        Expr::New { class, .. } => Type::Named(class.clone()),
        _ => Type::Named("_unknown".to_owned()),
    }
}

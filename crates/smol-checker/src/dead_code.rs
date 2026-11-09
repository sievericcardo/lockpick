//! Dead-code analysis pass.
//!
//! Detects:
//! - **Unused variables** (`UnusedVariable` warning): a local variable is declared
//!   but its name never appears as a read within the enclosing method/main block.
//! - **Unused parameters** (`UnusedParameter` warning): a formal parameter is
//!   never read inside the method body.
//! - **Dead methods** (`DeadMethod` warning): a concrete, non-override method
//!   whose name never appears in any method-call expression in the entire program.
//!   Override methods are excluded because they may be invoked through virtual
//!   dispatch on a base-class reference.

use std::collections::HashSet;

use smol_syntax::ast::{Expr, File, Param, Stmt};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::class_env::ClassEnv;

/// Entry point for the dead-code pass.
#[must_use]
pub fn check_dead_code(files: &[File], _env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Collect every method name that appears on the call side anywhere.
    let called_names = collect_all_called_method_names(files);

    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }

                // Unused variables / parameters inside method body.
                diags.extend(check_unused_in_method(
                    &method.body,
                    &method.params,
                    &file.path,
                ));

                // Dead method: not an override and name never appears as a call.
                if !method.flags.is_override && !called_names.contains(method.name.as_str()) {
                    diags.push(
                        Diagnostic::warning(
                            DiagnosticCode::DeadMethod,
                            format!(
                                "`{}::{}` is never called anywhere in the program",
                                cls.name, method.name
                            ),
                        )
                        .with_file(&file.path)
                        .with_span(method.span),
                    );
                }
            }
        }

        // Unused variables in the top-level main block.
        if let Some(main_stmts) = &file.main {
            diags.extend(check_unused_vars_in_stmts(main_stmts, &file.path));
        }
    }

    diags
}

// ── Unused variables ─────────────────────────────────────────────────────────

/// Check one method body for unused parameters and unused local variables.
fn check_unused_in_method(body: &[Stmt], params: &[Param], file: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mut reads: HashSet<String> = HashSet::new();
    collect_reads_in_stmts(body, &mut reads);

    for param in params {
        if !reads.contains(&param.name) {
            diags.push(
                Diagnostic::warning(
                    DiagnosticCode::UnusedParameter,
                    format!("parameter `{}` is never read", param.name),
                )
                .with_file(file)
                .with_span(param.span),
            );
        }
    }

    diags.extend(check_unused_vars_in_stmts(body, file));
    diags
}

/// Emit `UnusedVariable` warnings for every `VarDecl` whose name is never read.
fn check_unused_vars_in_stmts(stmts: &[Stmt], file: &str) -> Vec<Diagnostic> {
    let mut reads: HashSet<String> = HashSet::new();
    collect_reads_in_stmts(stmts, &mut reads);

    let mut diags = Vec::new();
    let decls = collect_var_decls(stmts);
    for (name, span) in decls {
        if !reads.contains(&name) {
            diags.push(
                Diagnostic::warning(
                    DiagnosticCode::UnusedVariable,
                    format!("variable `{name}` is declared but never used"),
                )
                .with_file(file)
                .with_span(span),
            );
        }
    }
    diags
}

/// Recursively collect all `VarDecl` names (and their spans) inside `stmts`.
fn collect_var_decls(stmts: &[Stmt]) -> Vec<(String, Span)> {
    let mut out = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { name, span, .. } => out.push((name.clone(), *span)),
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                out.extend(collect_var_decls(then_body));
                out.extend(collect_var_decls(else_body));
            }
            Stmt::While { body, .. } => out.extend(collect_var_decls(body)),
            _ => {}
        }
    }
    out
}

/// Collect every variable name that appears as a *read* (not as the target of a
/// plain variable assignment).
fn collect_reads_in_stmts(stmts: &[Stmt], reads: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { init, .. } => collect_reads_in_expr(init, reads),

            Stmt::Assign { lhs, rhs, .. } => {
                // `x = expr` — the LHS identifier is a *write*, not a read.
                // For `this.field = expr`, `this` IS read.
                if !matches!(lhs, Expr::Var(..)) {
                    collect_reads_in_expr(lhs, reads);
                }
                collect_reads_in_expr(rhs, reads);
            }

            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    collect_reads_in_expr(v, reads);
                }
            }

            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                collect_reads_in_expr(cond, reads);
                collect_reads_in_stmts(then_body, reads);
                collect_reads_in_stmts(else_body, reads);
            }

            Stmt::While { cond, body, .. } => {
                collect_reads_in_expr(cond, reads);
                collect_reads_in_stmts(body, reads);
            }

            Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
                collect_reads_in_expr(value, reads);
            }

            Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
                collect_reads_in_expr(expr, reads);
            }

            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
}

fn collect_reads_in_expr(expr: &Expr, reads: &mut HashSet<String>) {
    match expr {
        Expr::Var(name, _) => {
            reads.insert(name.clone());
        }

        Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => {
            collect_reads_in_expr(receiver, reads);
        }

        Expr::MethodCall { receiver, args, .. } => {
            collect_reads_in_expr(receiver, reads);
            for a in args {
                collect_reads_in_expr(a, reads);
            }
        }

        Expr::New { args, .. }
        | Expr::Construct { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. } => {
            for a in args {
                collect_reads_in_expr(a, reads);
            }
        }

        Expr::Access {
            connector, args, ..
        } => {
            if let Some(c) = connector {
                collect_reads_in_expr(c, reads);
            }
            for a in args {
                collect_reads_in_expr(a, reads);
            }
        }

        Expr::Simulate { args, .. } => {
            for a in args {
                collect_reads_in_expr(a, reads);
            }
        }

        Expr::Classify { expr, .. } => collect_reads_in_expr(expr, reads),

        Expr::BinOp { left, right, .. } => {
            collect_reads_in_expr(left, reads);
            collect_reads_in_expr(right, reads);
        }

        Expr::UnaryOp { expr, .. } => collect_reads_in_expr(expr, reads),

        // Literals and string-expression builtins have no embedded variables.
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StrLit(..)
        | Expr::BoolLit(..)
        | Expr::Null(..)
        | Expr::Member { .. }
        | Expr::Validate { .. } => {}
    }
}

// ── Dead methods ─────────────────────────────────────────────────────────────

/// Return the set of all method names that appear as call sites anywhere in
/// the program (main block + every method body, across all files).
fn collect_all_called_method_names(files: &[File]) -> HashSet<String> {
    let mut names: HashSet<String> = HashSet::new();
    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                collect_called_names_in_stmts(&method.body, &mut names);
            }
        }
        if let Some(main_stmts) = &file.main {
            collect_called_names_in_stmts(main_stmts, &mut names);
        }
    }
    names
}

fn collect_called_names_in_stmts(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { init, .. } => collect_called_names_in_expr(init, names),
            Stmt::Assign { lhs, rhs, .. } => {
                collect_called_names_in_expr(lhs, names);
                collect_called_names_in_expr(rhs, names);
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    collect_called_names_in_expr(v, names);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                collect_called_names_in_expr(cond, names);
                collect_called_names_in_stmts(then_body, names);
                collect_called_names_in_stmts(else_body, names);
            }
            Stmt::While { cond, body, .. } => {
                collect_called_names_in_expr(cond, names);
                collect_called_names_in_stmts(body, names);
            }
            Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
                collect_called_names_in_expr(value, names);
            }
            Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
                collect_called_names_in_expr(expr, names);
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
}

fn collect_called_names_in_expr(expr: &Expr, names: &mut HashSet<String>) {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            names.insert(method.clone());
            collect_called_names_in_expr(receiver, names);
            for a in args {
                collect_called_names_in_expr(a, names);
            }
        }

        Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => {
            collect_called_names_in_expr(receiver, names);
        }

        Expr::New { args, .. }
        | Expr::Construct { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. } => {
            for a in args {
                collect_called_names_in_expr(a, names);
            }
        }

        Expr::Access {
            connector, args, ..
        } => {
            if let Some(c) = connector {
                collect_called_names_in_expr(c, names);
            }
            for a in args {
                collect_called_names_in_expr(a, names);
            }
        }

        Expr::Simulate { args, .. } => {
            for a in args {
                collect_called_names_in_expr(a, names);
            }
        }

        Expr::Classify { expr, .. } => collect_called_names_in_expr(expr, names),

        Expr::BinOp { left, right, .. } => {
            collect_called_names_in_expr(left, names);
            collect_called_names_in_expr(right, names);
        }

        Expr::UnaryOp { expr, .. } => collect_called_names_in_expr(expr, names),

        Expr::Var(..)
        | Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StrLit(..)
        | Expr::BoolLit(..)
        | Expr::Null(..)
        | Expr::Member { .. }
        | Expr::Validate { .. } => {}
    }
}

//! SPARQL injection taint analysis (plan §2.8).
//!
//! Values that originate from `access(…)` or `construct(…)` calls contain
//! data from the RDF knowledge graph.  If such a tainted value is later passed
//! as a positional argument (`%1`, `%2`, …) to another `access`/`construct`
//! call, a potential SPARQL-injection is flagged as a warning.
//!
//! Taint propagates through variable assignment and field reads on tainted
//! variables.  The analysis is intra-procedural and forward (no global
//! interprocedural propagation).

use std::collections::HashSet;

use smol_syntax::ast::{Expr, File, Stmt};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::class_env::ClassEnv;

/// Returns `true` if `expr` is or transitively contains a tainted source.
fn is_tainted(expr: &Expr, tainted: &HashSet<String>) -> bool {
    match expr {
        Expr::Var(name, _) => tainted.contains(name.as_str()),
        // A direct access/construct result is always tainted.
        Expr::Access { .. } | Expr::Construct { .. } => true,
        // Field read on a tainted receiver propagates taint.
        Expr::Field { receiver, .. } => is_tainted(receiver, tainted),
        _ => false,
    }
}

fn warn_if_tainted(
    arg: &Expr,
    tainted: &HashSet<String>,
    file: &str,
    call_span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    if is_tainted(arg, tainted) {
        diags.push(
            Diagnostic::warning(
                DiagnosticCode::SparqlInjection,
                "tainted value (originating from an `access`/`construct` result) \
                 passed as a SPARQL query argument; if the backend uses string \
                 interpolation rather than parameterised binding this may allow \
                 SPARQL injection"
                    .to_owned(),
            )
            .with_file(file)
            .with_span(call_span),
        );
    }
}

fn check_expr_taint(
    expr: &Expr,
    tainted: &HashSet<String>,
    file: &str,
    diags: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Access { args, span, .. } | Expr::Construct { args, span, .. } => {
            for arg in args {
                warn_if_tainted(arg, tainted, file, *span, diags);
                check_expr_taint(arg, tainted, file, diags);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            check_expr_taint(receiver, tainted, file, diags);
            for arg in args {
                check_expr_taint(arg, tainted, file, diags);
            }
        }
        Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => {
            check_expr_taint(receiver, tainted, file, diags);
        }
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. }
        | Expr::Simulate { args, .. } => {
            for arg in args {
                check_expr_taint(arg, tainted, file, diags);
            }
        }
        Expr::Classify { expr, .. } | Expr::UnaryOp { expr, .. } => {
            check_expr_taint(expr, tainted, file, diags);
        }
        Expr::BinOp { left, right, .. } => {
            check_expr_taint(left, tainted, file, diags);
            check_expr_taint(right, tainted, file, diags);
        }
        _ => {}
    }
}

fn check_stmts_taint(stmts: &[Stmt], tainted: &mut HashSet<String>, file: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { name, init, .. } => {
                check_expr_taint(init, tainted, file, &mut diags);
                if is_tainted(init, tainted) {
                    tainted.insert(name.clone());
                }
            }
            Stmt::Assign { lhs, rhs, .. } => {
                check_expr_taint(rhs, tainted, file, &mut diags);
                if let Expr::Var(name, _) = lhs {
                    if is_tainted(rhs, tainted) {
                        tainted.insert(name.clone());
                    } else {
                        tainted.remove(name.as_str());
                    }
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    check_expr_taint(v, tainted, file, &mut diags);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                check_expr_taint(cond, tainted, file, &mut diags);
                let mut then_t = tainted.clone();
                diags.extend(check_stmts_taint(then_body, &mut then_t, file));
                let mut else_t = tainted.clone();
                diags.extend(check_stmts_taint(else_body, &mut else_t, file));
                // Conservative union of both branches.
                for name in then_t {
                    tainted.insert(name);
                }
                for name in else_t {
                    tainted.insert(name);
                }
            }
            Stmt::While { cond, body, .. } => {
                check_expr_taint(cond, tainted, file, &mut diags);
                let mut body_t = tainted.clone();
                diags.extend(check_stmts_taint(body, &mut body_t, file));
                for name in body_t {
                    tainted.insert(name);
                }
            }
            Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
                check_expr_taint(value, tainted, file, &mut diags);
            }
            Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
                check_expr_taint(expr, tainted, file, &mut diags);
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
    diags
}

/// Run the SPARQL-taint pass over all method bodies and the `main` block.
#[must_use]
pub fn check_sparql_taint(files: &[File], _env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                let mut tainted: HashSet<String> = HashSet::new();
                diags.extend(check_stmts_taint(&method.body, &mut tainted, &file.path));
            }
        }
        if let Some(main_stmts) = &file.main {
            let mut tainted: HashSet<String> = HashSet::new();
            diags.extend(check_stmts_taint(main_stmts, &mut tainted, &file.path));
        }
    }
    diags
}

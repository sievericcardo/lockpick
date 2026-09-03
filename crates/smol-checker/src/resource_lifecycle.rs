//! FMO (Functional Mock-up Object) resource-lifecycle analysis (plan §2.5).
//!
//! Every local variable whose declared type is `FMO[…]` represents a live
//! FMU object that must eventually be released via `destroy(var)`.  If no
//! `destroy` call is found anywhere in the enclosing method body, a warning
//! is emitted.
//!
//! # Limitations (future work)
//!
//! The current check is path-insensitive: it only verifies that *some*
//! `destroy` call exists, not that it is reached on *every* exit path.
//! A full typestate or linear-type analysis (§2.5 in the plan) would be
//! needed for path-sensitive guarantees.

use std::collections::HashSet;

use smol_syntax::ast::{Expr, File, Stmt, Type};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::class_env::ClassEnv;

/// Collect all variable names passed directly to `destroy(var)` in `stmts`.
fn collect_destroyed(stmts: &[Stmt]) -> HashSet<String> {
    let mut destroyed = HashSet::new();
    for stmt in stmts {
        match stmt {
            Stmt::Destroy {
                value: Expr::Var(name, _),
                ..
            } => {
                destroyed.insert(name.clone());
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                destroyed.extend(collect_destroyed(then_body));
                destroyed.extend(collect_destroyed(else_body));
            }
            Stmt::While { body, .. } => {
                destroyed.extend(collect_destroyed(body));
            }
            _ => {}
        }
    }
    destroyed
}

/// Collect all `(name, span)` pairs for local variables of `FMO[…]` type.
fn collect_fmo_vars(stmts: &[Stmt]) -> Vec<(String, Span)> {
    let mut vars = Vec::new();
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
                vars.extend(collect_fmo_vars(then_body));
                vars.extend(collect_fmo_vars(else_body));
            }
            Stmt::While { body, .. } => {
                vars.extend(collect_fmo_vars(body));
            }
            _ => {}
        }
    }
    vars
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
                let fmo_vars = collect_fmo_vars(&method.body);
                if fmo_vars.is_empty() {
                    continue;
                }
                let destroyed = collect_destroyed(&method.body);
                for (name, span) in fmo_vars {
                    if !destroyed.contains(&name) {
                        diags.push(
                            Diagnostic::warning(
                                DiagnosticCode::FmoNotDestroyed,
                                format!(
                                    "FMO variable `{name}` may not be destroyed before the \
                                     method returns; call `destroy({name})` on all exit paths"
                                ),
                            )
                            .with_file(&file.path)
                            .with_span(span),
                        );
                    }
                }
            }
        }
        if let Some(main_stmts) = &file.main {
            let fmo_vars = collect_fmo_vars(main_stmts);
            let destroyed = collect_destroyed(main_stmts);
            for (name, span) in fmo_vars {
                if !destroyed.contains(&name) {
                    diags.push(
                        Diagnostic::warning(
                            DiagnosticCode::FmoNotDestroyed,
                            format!(
                                "FMO variable `{name}` may not be destroyed before the \
                                 program ends; call `destroy({name})`"
                            ),
                        )
                        .with_file(&file.path)
                        .with_span(span),
                    );
                }
            }
        }
    }
    diags
}

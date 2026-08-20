//! Body pass: type-check method bodies and the `main` block.
//!
//! Uses a simple scope-stack `VarEnv` (no `salsa` persistence here — the
//! incremental layer is at the declaration pass).

use std::collections::HashMap;

use smol_syntax::ast::{Type, File, Stmt, Expr, UnaryOp, BinOp};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

use crate::class_env::ClassEnv;

/// A lexically-scoped map from variable name to type.
#[derive(Debug, Default, Clone)]
struct VarEnv {
    scopes: Vec<HashMap<String, Type>>,
}

impl VarEnv {
    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn insert(&mut self, name: String, ty: Type) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<&Type> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
    }
}

// Public entry
#[must_use]
pub fn check_bodies(files: &[File], env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for file in files {
        for cls in &file.classes {
            let self_ty = Type::Named(cls.name.clone());
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                let mut var_env = VarEnv::default();
                var_env.push();
                // `this` is always in scope inside a method.
                var_env.insert("this".to_owned(), self_ty.clone());
                // Parameters
                for param in &method.params {
                    var_env.insert(param.name.clone(), param.ty.clone());
                }
                let body_diags = check_stmts(
                    &method.body,
                    &mut var_env,
                    env,
                    &file.path,
                    &method.ret_type,
                );
                diags.extend(body_diags);
                var_env.pop();
            }
        }
        // main block — no `this`
        if let Some(stmts) = &file.main {
            let mut var_env = VarEnv::default();
            var_env.push();
            let body_diags =
                check_stmts(stmts, &mut var_env, env, &file.path, &Type::Unit);
            diags.extend(body_diags);
            var_env.pop();
        }
    }
    diags
}

// Statement checking
fn check_stmts(
    stmts: &[Stmt],
    var_env: &mut VarEnv,
    env: &ClassEnv,
    file: &str,
    expected_ret: &Type,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for stmt in stmts {
        let d = check_stmt(stmt, var_env, env, file, expected_ret);
        diags.extend(d);
    }
    diags
}

#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn check_stmt(
    stmt: &Stmt,
    var_env: &mut VarEnv,
    env: &ClassEnv,
    file: &str,
    expected_ret: &Type,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    match stmt {
        Stmt::VarDecl { ty, name, init, span } => {
            let init_ty = infer_expr(init, var_env, env, file, &mut diags);
            if !tys_compatible(env, &init_ty, ty) {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::TypeMismatch,
                        format!(
                            "cannot assign `{init_ty}` to variable `{name}: {ty}`"
                        ),
                    )
                        .with_file(file)
                        .with_span(*span),
                );
            }
            // access() and construct() always return a linked list.
            // SMOL allows declaring the variable with the element type as shorthand:
            //   Room r = access("...")   →  r is actually List<Room>
            // If the declared type is already List<T> we use it as-is.
            let var_ty = if matches!(init, Expr::Access { .. } | Expr::Construct { .. })
                && !matches!(ty, Type::List(_) | Type::ExplList(_))
            {
                Type::List(Box::new(ty.clone()))
            } else {
                ty.clone()
            };
            var_env.insert(name.clone(), var_ty);
        }

        Stmt::Assign { lhs, rhs, span } => {
            let lhs_ty = infer_expr(lhs, var_env, env, file, &mut diags);
            let rhs_ty = infer_expr(rhs, var_env, env, file, &mut diags);
            if !tys_compatible(env, &rhs_ty, &lhs_ty) {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::TypeMismatch,
                        format!("type mismatch in assignment: `{rhs_ty}` vs `{lhs_ty}`"),
                    )
                        .with_file(file)
                        .with_span(*span),
                );
            }
        }

        Stmt::Return { value, span } => {
            let got = value
                .as_ref()
                .map_or(Type::Unit, |e| infer_expr(e, var_env, env, file, &mut diags));
            if !tys_compatible(env, &got, expected_ret) {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::TypeMismatch,
                        format!(
                            "return type mismatch: expected `{expected_ret}`, got `{got}`"
                        ),
                    )
                        .with_file(file)
                        .with_span(*span),
                );
            }
        }

        Stmt::If { cond, then_body, else_body, span } => {
            let cond_ty = infer_expr(cond, var_env, env, file, &mut diags);
            if !matches!(cond_ty, Type::Boolean) {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::TypeMismatch,
                        format!("condition must be Boolean, got `{cond_ty}`"),
                    )
                        .with_file(file)
                        .with_span(*span),
                );
            }
            var_env.push();
            diags.extend(check_stmts(then_body, var_env, env, file, expected_ret));
            var_env.pop();
            var_env.push();
            diags.extend(check_stmts(else_body, var_env, env, file, expected_ret));
            var_env.pop();
        }

        Stmt::While { cond, body, span } => {
            let cond_ty = infer_expr(cond, var_env, env, file, &mut diags);
            if !matches!(cond_ty, Type::Boolean) {
                // `null` comparisons produce Boolean logically; relax for now
                let _ = cond_ty;
                let _ = span;
            }
            var_env.push();
            diags.extend(check_stmts(body, var_env, env, file, expected_ret));
            var_env.pop();
        }

        Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
            infer_expr(value, var_env, env, file, &mut diags);
        }
        Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        Stmt::Adapt { expr, .. } | Stmt::Expr { expr, .. } => {
            infer_expr(expr, var_env, env, file, &mut diags);
        }
    }

    diags
}

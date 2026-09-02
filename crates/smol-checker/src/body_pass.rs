//! Body pass: type-check method bodies and the `main` block.
//!
//! Uses a simple scope-stack `VarEnv` (no `salsa` persistence here — the
//! incremental layer is at the declaration pass).

use std::collections::HashMap;

use smol_syntax::ast::{BinOp, Expr, File, Stmt, Type, UnaryOp};
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
            let body_diags = check_stmts(stmts, &mut var_env, env, &file.path, &Type::Unit);
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
        Stmt::VarDecl {
            ty,
            name,
            init,
            span,
        } => {
            let init_ty = infer_expr(init, var_env, env, file, &mut diags);
            if !tys_compatible(env, &init_ty, ty) {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::TypeMismatch,
                        format!("cannot assign `{init_ty}` to variable `{name}: {ty}`"),
                    )
                    .with_file(file)
                    .with_span(*span),
                );
            }
            // access() and construct() always return a linked list.
            // SMOL allows declaring the variable with the element type as shorthand:
            //   Room r = access("...")   →  r is actually List<Room>
            // If the declared type is already List<T> we use it as-is.
            let var_ty = if matches!(
                init,
                Expr::Access { .. } | Expr::Construct { .. } | Expr::Member { .. }
            ) && !matches!(ty, Type::List(_) | Type::ExplList(_))
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
            let got = value.as_ref().map_or(Type::Unit, |e| {
                infer_expr(e, var_env, env, file, &mut diags)
            });
            if !tys_compatible(env, &got, expected_ret) {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::TypeMismatch,
                        format!("return type mismatch: expected `{expected_ret}`, got `{got}`"),
                    )
                    .with_file(file)
                    .with_span(*span),
                );
            }
        }

        Stmt::If {
            cond,
            then_body,
            else_body,
            span,
        } => {
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

//  Expression inference
/// Infer the type of `expr`, recording any errors in `diags`.
/// Returns `Ty::Named("_unknown")` on failure so checking can continue.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn infer_expr(
    expr: &Expr,
    var_env: &VarEnv,
    env: &ClassEnv,
    file: &str,
    diags: &mut Vec<Diagnostic>,
) -> Type {
    match expr {
        Expr::IntLit(_, _) => Type::Int,
        Expr::FloatLit(_, _) => Type::Double,
        Expr::StrLit(_, _) => Type::String,
        Expr::BoolLit(_, _) => Type::Boolean,
        Expr::Null(_) => Type::Named("_null".to_owned()),

        Expr::Var(name, span) => {
            // `breakpoint` is a SMOL built-in debug statement (no args, Unit type).
            if name == "breakpoint" {
                return Type::Unit;
            }
            if let Some(ty) = var_env.lookup(name) {
                ty.clone()
            } else {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::UnknownVariable,
                        format!("unknown variable `{name}`"),
                    )
                    .with_file(file)
                    .with_span(*span),
                );
                Type::Named("_unknown".to_owned())
            }
        }

        Expr::Field {
            receiver,
            field,
            span,
        } => {
            let rec_ty = infer_expr(receiver, var_env, env, file, diags);
            resolve_field_type(&rec_ty, field, env, file, *span, diags)
        }

        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } => {
            let rec_ty = infer_expr(receiver, var_env, env, file, diags);
            for arg in args {
                infer_expr(arg, var_env, env, file, diags);
            }
            resolve_method_return(&rec_ty, method, env, file, *span, diags)
        }

        Expr::New {
            class,
            ty_param,
            args,
            span,
        } => {
            // `new List<T>(head, tail)` / `new ExplList<T>(head, tail)` are the
            // linked-list constructors.
            if class == "List" || class == "ExplList" {
                for arg in args {
                    infer_expr(arg, var_env, env, file, diags);
                }
                let inner = ty_param
                    .as_deref()
                    .cloned()
                    .unwrap_or(Type::Named("_unknown".to_owned()));
                return if class == "ExplList" {
                    Type::ExplList(Box::new(inner))
                } else {
                    Type::List(Box::new(inner))
                };
            }
            if !env.contains(class) {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::UnknownClass,
                        format!("unknown class `{class}`"),
                    )
                    .with_file(file)
                    .with_span(*span),
                );
                return Type::Named("_unknown".to_owned());
            }
            if let Some(info) = env.get(class) {
                if info.is_abstract {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::AbstractInstantiation,
                            format!("cannot instantiate abstract class `{class}`"),
                        )
                        .with_file(file)
                        .with_span(*span),
                    );
                }
                let expected = env.constructor_field_count(class);
                if args.len() != expected {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::WrongArgCount,
                            format!(
                                "`new {class}` expects {expected} arguments, got {}",
                                args.len()
                            ),
                        )
                        .with_file(file)
                        .with_span(*span),
                    );
                }
            }
            for arg in args {
                infer_expr(arg, var_env, env, file, diags);
            }
            Type::Named(class.clone())
        }

        // construct / access return List<_unknown> — caller annotates via assignment
        Expr::Construct { args, .. } => {
            for arg in args {
                infer_expr(arg, var_env, env, file, diags);
            }
            Type::Named("_list_inferred".to_owned())
        }
        Expr::Access { .. } => Type::Named("_list_inferred".to_owned()),

        Expr::Classify { expr: inner, .. } => {
            let inner_ty = infer_expr(inner, var_env, env, file, diags);
            // classify(x) where x: T returns the class S that holds T as a
            // `context` field — i.e., the digital-twin state type for T.
            // Fall back to walking T's ancestor chain for `classifies` (legacy).
            if let Type::Named(ref class) = inner_ty {
                find_context_holder(class, env)
                    .or_else(|| findclassify_ty(class, env))
                    .unwrap_or(Type::Named("_unknown".to_owned()))
            } else {
                Type::Named("_unknown".to_owned())
            }
        }

        Expr::BuiltinCall { name, args, .. } => {
            for arg in args {
                infer_expr(arg, var_env, env, file, diags);
            }
            // Known built-in return types
            match name.as_str() {
                "doubleToString" | "intToString" | "booleanToString" => Type::String,
                "stringToDouble" => Type::Double,
                "stringToInt" => Type::Int,
                _ => Type::Named("_unknown".to_owned()),
            }
        }

        Expr::BinOp {
            op,
            left,
            right,
            span,
        } => {
            let lt = infer_expr(left, var_env, env, file, diags);
            let rt = infer_expr(right, var_env, env, file, diags);
            binop_result_ty(*op, &lt, &rt, file, *span, diags)
        }

        Expr::UnaryOp {
            op, expr: inner, ..
        } => {
            let t = infer_expr(inner, var_env, env, file, diags);
            match op {
                UnaryOp::Neg => t,
                UnaryOp::Not => Type::Boolean,
            }
        }

        // FMU / semantic builtins — parse without false errors, defer deep checking
        Expr::Member { .. } => Type::Named("_list_inferred".to_owned()),
        Expr::Validate { .. } => Type::Boolean,
        Expr::Simulate { args, .. } => {
            for arg in args {
                infer_expr(arg, var_env, env, file, diags);
            }
            Type::Named("_fmo".to_owned())
        }
        Expr::Port { receiver, .. } => {
            infer_expr(receiver, var_env, env, file, diags);
            Type::Double
        }
        Expr::Super { args, .. } => {
            for arg in args {
                infer_expr(arg, var_env, env, file, diags);
            }
            Type::Unit
        }
    }
}

fn resolve_field_type(
    receiver_ty: &Type,
    field: &str,
    env: &ClassEnv,
    file: &str,
    span: smol_syntax::span::Span,
    diags: &mut Vec<Diagnostic>,
) -> Type {
    match receiver_ty {
        // Properly-typed List<T> — preserves element type through .content
        Type::List(inner_ty) => match field {
            "content" => *inner_ty.clone(),
            "next" => receiver_ty.clone(),
            _ => {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::UnknownField,
                        format!("List has no field `{field}`"),
                    )
                    .with_file(file)
                    .with_span(span),
                );
                Type::Named("_unknown".to_owned())
            }
        },
        // Explicitly-typed ExplList<T> — same structure, but exposed via RDF.
        Type::ExplList(inner_ty) => match field {
            "content" => *inner_ty.clone(),
            "next" => receiver_ty.clone(),
            _ => {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::UnknownField,
                        format!("ExplList has no field `{field}`"),
                    )
                    .with_file(file)
                    .with_span(span),
                );
                Type::Named("_unknown".to_owned())
            }
        },
        Type::Named(class) => {
            // Unparameterised List (backwards-compat)
            if class == "List" || class.starts_with("_list") {
                return match field {
                    "content" => Type::Named("_unknown".to_owned()),
                    "next" => receiver_ty.clone(),
                    _ => {
                        diags.push(
                            Diagnostic::error(
                                DiagnosticCode::UnknownField,
                                format!("List has no field `{field}`"),
                            )
                            .with_file(file)
                            .with_span(span),
                        );
                        Type::Named("_unknown".to_owned())
                    }
                };
            }
            if let Some(f) = env.resolve_field(class, field) {
                f.ty.clone()
            } else {
                // Fall back: if ALL direct subclasses define the field with the
                // same type, the access is valid at runtime (e.g. a parent method
                // accessing a field that every concrete subclass provides).
                let subs = env.direct_subclasses(class);
                if !subs.is_empty() {
                    let types: Vec<Type> = subs
                        .iter()
                        .filter_map(|s| env.resolve_field(s, field).map(|f| f.ty.clone()))
                        .collect();
                    if types.len() == subs.len() {
                        let first = &types[0];
                        if types.iter().all(|t| t == first) {
                            return first.clone();
                        }
                    }
                }
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::UnknownField,
                        format!("class `{class}` has no field `{field}`"),
                    )
                    .with_file(file)
                    .with_span(span),
                );
                Type::Named("_unknown".to_owned())
            }
        }
        _ => Type::Named("_unknown".to_owned()),
    }
}

fn resolve_method_return(
    receiver_ty: &Type,
    method: &str,
    env: &ClassEnv,
    file: &str,
    span: smol_syntax::span::Span,
    diags: &mut Vec<Diagnostic>,
) -> Type {
    match receiver_ty {
        // Properly-typed List<T>
        Type::List(_) => match method {
            "length" => Type::Int,
            _ => Type::Named("_unknown".to_owned()),
        },
        // Explicitly-typed ExplList<T>
        Type::ExplList(_) => match method {
            "length" => Type::Int,
            _ => Type::Named("_unknown".to_owned()),
        },
        Type::Named(class) => {
            // Unparameterised List (backwards-compat)
            if class == "List" || class.starts_with("_list") {
                return match method {
                    "length" => Type::Int,
                    _ => Type::Named("_unknown".to_owned()),
                };
            }
            if let Some(m) = env.resolve_method(class, method) {
                m.ret_type.clone()
            } else {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::UnknownField,
                        format!("class `{class}` has no method `{method}`"),
                    )
                    .with_file(file)
                    .with_span(span),
                );
                Type::Named("_unknown".to_owned())
            }
        }
        _ => Type::Named("_unknown".to_owned()),
    }
}

/// Walk the class hierarchy to find the nearest ancestor that has a
/// `classifies` annotation — that's the type returned by `classify()`.
fn findclassify_ty(class: &str, env: &ClassEnv) -> Option<Type> {
    for ancestor in env.ancestors(class) {
        if let Some(info) = env.get(ancestor) {
            if info.classifies.is_some() {
                return Some(Type::Named(info.name.clone()));
            }
        }
    }
    None
}

/// Search all classes for one that holds `ty` as a `context` field.
/// This is the primary resolution strategy for `classify(x: T)`: find the
/// state class `S` that declares `context T field`, which is the type that
/// `classify()` instantiates at runtime.
/// Returns `None` when there is more than one candidate (ambiguous), since the
/// ontology alone determines the choice at runtime.
fn find_context_holder(ty: &str, env: &ClassEnv) -> Option<Type> {
    use smol_syntax::ast::FieldModifier;
    let mut found: Vec<Type> = Vec::new();
    for info in env.iter() {
        for field in &info.fields {
            if matches!(field.modifier, FieldModifier::Context)
                && matches!(&field.ty, Type::Named(n) if n == ty)
            {
                found.push(Type::Named(info.name.clone()));
                break;
            }
        }
    }
    if found.len() == 1 {
        found.into_iter().next()
    } else {
        None
    }
}

fn binop_result_ty(
    op: BinOp,
    lt: &Type,
    rt: &Type,
    file: &str,
    span: smol_syntax::span::Span,
    diags: &mut Vec<Diagnostic>,
) -> Type {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => match (lt, rt) {
            (Type::Int, Type::Int) => Type::Int,
            (Type::Int | Type::Double, Type::Double) | (Type::Double, Type::Int) => Type::Double,
            _ => {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::TypeMismatch,
                        format!("arithmetic on `{lt}` and `{rt}`"),
                    )
                    .with_file(file)
                    .with_span(span),
                );
                Type::Named("_unknown".to_owned())
            }
        },
        BinOp::Concat => Type::String,
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            Type::Boolean
        }
        BinOp::And | BinOp::Or => Type::Boolean,
    }
}

/// Returns `true` if `from` can be assigned to a slot of type `to`.
/// Accepts `_null` for any named type (reference type).
fn tys_compatible(env: &ClassEnv, from: &Type, to: &Type) -> bool {
    match (from, to) {
        (Type::Named(n), _) if n == "_null" => {
            matches!(to, Type::Named(_) | Type::List(_) | Type::ExplList(_))
        }
        (Type::Named(n), _) if n.starts_with('_') => true, // unknown/inferred — don't error twice
        // Unparameterised List is compatible with any List<T>
        (Type::Named(n), Type::List(_)) if n == "List" => true,
        (Type::List(_), Type::Named(n)) if n == "List" => true,
        // Numeric widening: Int literals can be assigned to Double fields
        (Type::Int, Type::Double) => true,
        _ => env.is_assignable(from, to),
    }
}

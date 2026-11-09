//! Flow-sensitive null-safety analysis.
//!
//! Tracks the *definite* nullness of local variables **and field paths**
//! (e.g. `this.f`, `x.f`) and flags dereferences of receivers that are
//! provably nullable at that point.
//!
//! # Soundness contract (no false positives)
//!
//! - Variables assigned `null` → `Null`; `new …` / literals → `NonNull`;
//!   everything else (params, field reads, call returns) → `Maybe` (not flagged
//!   by E2018).
//! - Field paths (`this.f`, `x.f`, …) are tracked in a side table keyed by
//!   their canonical path string.  `this.f = null` → `Null`;
//!   `this.f = <non-null expr>` → that expr's nullness; otherwise `Maybe`.
//! - `if x != null` / `while x != null` narrows `x` to `NonNull` inside the
//!   branch/body.  `if x == null` narrows to `Null` (then) / `NonNull` (else).
//!   The same narrowing applies to field-path guards such as
//!   `if this.f != null`.
//! - E2018 (`NullDereference`, unchanged): only a **direct-variable receiver**
//!   that is `Null` is flagged.
//! - E2023 (`NullableFieldDereference`): a receiver (variable *or* field path)
//!   used for field access / method call is flagged iff it is **provably
//!   nullable** — its declared type is `Nullable` (`T?`), it was assigned
//!   `null`, or it was assigned from a provably-nullable expression (e.g.
//!   `var s = this.f`) — **and** its current nullness is not `NonNull`
//!   (i.e. `Null` or unguarded `Maybe`).  Receivers whose nullness is unknown
//!   but not provably nullable (plain params, non-nullable fields, call
//!   results) are never flagged.
//! - Branch joins use the lattice join (`Null` ⊔ `NonNull` = `Maybe`); loop
//!   bodies are joined back into the pre-loop environment (no blunt havoc),
//!   so a `NonNull` value established before a loop that the loop never
//!   nulls is not spuriously flagged.

use std::collections::{HashMap, HashSet};

use smol_syntax::ast::{BinOp, Expr, File, Stmt, Type};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::class_env::ClassEnv;

#[derive(Clone, PartialEq, Eq, Debug)]
enum Nullness {
    Null,
    NonNull,
    Maybe,
}

/// Flow-sensitive nullness state for one method body (or the `main` block).
struct NullEnv {
    /// Definite nullness of locals/params by name (`this` → `NonNull`).
    vars: HashMap<String, Nullness>,
    /// Definite nullness of field paths by canonical key (`this.f`, `x.f.y`).
    fields: HashMap<String, Nullness>,
    /// Locals/params that are *provably* nullable (declared `T?`, assigned
    /// `null`, or assigned from a provably-nullable expression).  Sticky:
    /// once provably nullable, always provably nullable — the *current*
    /// nullness still decides whether a dereference is flagged.
    prov_vars: HashSet<String>,
    /// Same as `prov_vars`, for field-path keys.
    prov_fields: HashSet<String>,
    /// Declared static types of locals/params/`this` (for `T?` resolution).
    var_types: HashMap<String, Type>,
}

impl NullEnv {
    fn new() -> Self {
        Self {
            vars: HashMap::new(),
            fields: HashMap::new(),
            prov_vars: HashSet::new(),
            prov_fields: HashSet::new(),
            var_types: HashMap::new(),
        }
    }
}

/// Read-only context needed to resolve declared (`T?`) nullability.
struct Ctx<'a> {
    env: &'a ClassEnv,
}

fn join(a: Nullness, b: Nullness) -> Nullness {
    if a == b { a } else { Nullness::Maybe }
}

/// Canonical key for a field path: `this.f`, `x.f`, `this.f.g`.
/// Returns `None` for expressions that are not trackable paths
/// (call results, literals, …).  A bare `Var` yields its own name so that
/// longer paths can be built recursively; callers decide whether the target
/// is a variable or a genuine (dotted) field path.
fn path_key(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Var(name, _) => Some(name.clone()),
        Expr::Field {
            receiver, field, ..
        } => Some(format!("{}.{field}", path_key(receiver)?)),
        _ => None,
    }
}

/// A narrowable dereference target: either a plain variable or a dotted
/// field path.
enum Target {
    Var(String),
    FieldPath(String),
}

fn as_target(expr: &Expr) -> Option<Target> {
    match expr {
        Expr::Var(name, _) => Some(Target::Var(name.clone())),
        Expr::Field { .. } => {
            let key = path_key(expr)?;
            if key.contains('.') {
                Some(Target::FieldPath(key))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Best-effort static type of an expression for `T?` resolution.
/// Returns `None` when the type cannot be determined (unknown variable,
/// call result, …) — callers treat that as "not provably nullable".
fn static_type(expr: &Expr, flow: &NullEnv, ctx: &Ctx) -> Option<Type> {
    match expr {
        Expr::Var(name, _) => flow.var_types.get(name).cloned(),
        Expr::Field {
            receiver, field, ..
        } => {
            let mut rec_ty = static_type(receiver, flow, ctx)?;
            // A nullable owner still exposes the inner class's fields.
            if let Type::Nullable(inner) = rec_ty {
                rec_ty = (*inner).clone();
            }
            match rec_ty {
                Type::Named(class) => ctx.env.resolve_field(&class, field).map(|f| f.ty.clone()),
                Type::List(inner) => match field.as_str() {
                    "content" => Some((*inner).clone()),
                    "next" => Some(Type::List(inner)),
                    _ => None,
                },
                Type::ExplList(inner) => match field.as_str() {
                    "content" => Some((*inner).clone()),
                    "next" => Some(Type::ExplList(inner)),
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_static_nullable(expr: &Expr, flow: &NullEnv, ctx: &Ctx) -> bool {
    matches!(static_type(expr, flow, ctx), Some(Type::Nullable(_)))
}

fn nullness_of(expr: &Expr, flow: &NullEnv) -> Nullness {
    match expr {
        Expr::Null(_) => Nullness::Null,
        Expr::New { .. }
        | Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StrLit(..)
        | Expr::BoolLit(..) => Nullness::NonNull,
        Expr::Var(name, _) => flow
            .vars
            .get(name.as_str())
            .cloned()
            .unwrap_or(Nullness::Maybe),
        Expr::Field { .. } => path_key(expr)
            .and_then(|k| flow.fields.get(&k).cloned())
            .unwrap_or(Nullness::Maybe),
        _ => Nullness::Maybe,
    }
}

/// `true` iff the expression is *provably* nullable: the `null` literal, a
/// statically-`Nullable` (`T?`) typed variable/field path, or a variable /
/// field path previously assigned `null` (or a provably-nullable value).
/// Anything unresolvable (call results, unknown vars) is `false`.
fn is_provably_nullable(expr: &Expr, flow: &NullEnv, ctx: &Ctx) -> bool {
    match expr {
        Expr::Null(_) => true,
        Expr::Var(name, _) => {
            flow.prov_vars.contains(name.as_str()) || is_static_nullable(expr, flow, ctx)
        }
        Expr::Field { .. } => {
            if is_static_nullable(expr, flow, ctx) {
                return true;
            }
            path_key(expr)
                .map(|k| flow.prov_fields.contains(&k))
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// Narrow `flow` using a `!= null` / `== null` guard on a variable or field
/// path.  `branch_is_true` selects the then-side (`true`) or else-side
/// (`false`) environment.
fn narrow(flow: &mut NullEnv, cond: &Expr, branch_is_true: bool) {
    let Expr::BinOp {
        op, left, right, ..
    } = cond
    else {
        return;
    };
    if !matches!(op, BinOp::NotEq | BinOp::Eq) {
        return;
    }
    // Find the guarded target: the non-`Null` side must be a variable or a
    // field path, the other side the `null` literal.
    let target = match (left.as_ref(), right.as_ref()) {
        (t, Expr::Null(_)) | (Expr::Null(_), t) => match as_target(t) {
            Some(t) => t,
            None => return,
        },
        _ => return,
    };
    let narrowed = match op {
        BinOp::NotEq => {
            if branch_is_true {
                Some(Nullness::NonNull)
            } else {
                None
            }
        }
        BinOp::Eq => Some(if branch_is_true {
            Nullness::Null
        } else {
            Nullness::NonNull
        }),
        _ => None,
    };
    let Some(n) = narrowed else { return };
    match target {
        Target::Var(name) => {
            flow.vars.insert(name, n);
        }
        Target::FieldPath(key) => {
            flow.fields.insert(key, n);
        }
    }
}

/// Receiver display name for diagnostics (`x`, `this.f`, …).
fn display_target(expr: &Expr) -> String {
    match expr {
        Expr::Var(name, _) => name.clone(),
        _ => path_key(expr).unwrap_or_else(|| "<receiver>".to_owned()),
    }
}

/// Check the receiver of a field access / method call.
///
/// - Bare-variable receivers keep the legacy E2018 behaviour (`Null` → error)
///   and additionally report E2023 when the variable is provably nullable
///   (`T?` param, `null` assignment, `var s = this.f`, …) but merely `Maybe`.
/// - Field-path receivers (`this.f`, `x.f`) report E2023 when provably
///   nullable and not currently `NonNull` (i.e. a missing or failing guard).
/// - Anything else (call results, `this`, unknown shapes) is never flagged.
fn check_receiver(
    receiver: &Expr,
    flow: &NullEnv,
    ctx: &Ctx,
    file: &str,
    span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    let nullness = nullness_of(receiver, flow);
    let provable = is_provably_nullable(receiver, flow, ctx);
    match as_target(receiver) {
        Some(Target::Var(name)) => {
            if nullness == Nullness::Null {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::NullDereference,
                        format!("`{name}` is null at this point"),
                    )
                    .with_file(file)
                    .with_span(span),
                );
            } else if nullness == Nullness::Maybe && provable {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::NullableFieldDereference,
                        format!("`{name}` may be null at this point"),
                    )
                    .with_file(file)
                    .with_span(span),
                );
            }
        }
        Some(Target::FieldPath(_)) if nullness != Nullness::NonNull && provable => {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::NullableFieldDereference,
                    format!("`{}` may be null at this point", display_target(receiver)),
                )
                .with_file(file)
                .with_span(span),
            );
        }
        _ => {}
    }
}

fn check_expr(expr: &Expr, flow: &NullEnv, ctx: &Ctx, file: &str, diags: &mut Vec<Diagnostic>) {
    match expr {
        Expr::Field { receiver, span, .. } => {
            check_receiver(receiver, flow, ctx, file, *span, diags);
            check_expr(receiver, flow, ctx, file, diags);
        }
        Expr::MethodCall {
            receiver,
            args,
            span,
            ..
        } => {
            check_receiver(receiver, flow, ctx, file, *span, diags);
            check_expr(receiver, flow, ctx, file, diags);
            for arg in args {
                check_expr(arg, flow, ctx, file, diags);
            }
        }
        Expr::New { args, .. }
        | Expr::Construct { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. } => {
            for arg in args {
                check_expr(arg, flow, ctx, file, diags);
            }
        }
        Expr::Access {
            connector, args, ..
        } => {
            if let Some(c) = connector {
                check_expr(c, flow, ctx, file, diags);
            }
            for arg in args {
                check_expr(arg, flow, ctx, file, diags);
            }
        }
        Expr::Simulate { args, .. } => {
            for arg in args {
                check_expr(arg, flow, ctx, file, diags);
            }
        }
        Expr::Classify { expr, .. } | Expr::UnaryOp { expr, .. } => {
            check_expr(expr, flow, ctx, file, diags);
        }
        Expr::BinOp { left, right, .. } => {
            check_expr(left, flow, ctx, file, diags);
            check_expr(right, flow, ctx, file, diags);
        }
        Expr::Port { receiver, .. } => {
            check_expr(receiver, flow, ctx, file, diags);
        }
        _ => {}
    }
}

/// Merge `other` into `base` after a join point (end of `if` / `while`):
/// nullness joins on the lattice, provability unions (sticky), declared
/// types union.
fn merge_into(base: &mut NullEnv, other: &NullEnv) {
    let keys: HashSet<String> = base.vars.keys().chain(other.vars.keys()).cloned().collect();
    for name in &keys {
        let a = base.vars.get(name).cloned().unwrap_or(Nullness::Maybe);
        let b = other.vars.get(name).cloned().unwrap_or(Nullness::Maybe);
        base.vars.insert(name.clone(), join(a, b));
    }
    let keys: HashSet<String> = base
        .fields
        .keys()
        .chain(other.fields.keys())
        .cloned()
        .collect();
    for key in &keys {
        let a = base.fields.get(key).cloned().unwrap_or(Nullness::Maybe);
        let b = other.fields.get(key).cloned().unwrap_or(Nullness::Maybe);
        base.fields.insert(key.clone(), join(a, b));
    }
    base.prov_vars.extend(other.prov_vars.iter().cloned());
    base.prov_fields.extend(other.prov_fields.iter().cloned());
    for (k, v) in &other.var_types {
        base.var_types.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

fn check_stmts(stmts: &[Stmt], flow: &mut NullEnv, ctx: &Ctx, file: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { ty, name, init, .. } => {
                check_expr(init, flow, ctx, file, &mut diags);
                let rhs_null = nullness_of(init, flow);
                let rhs_prov = is_provably_nullable(init, flow, ctx);
                flow.var_types.insert(name.clone(), ty.clone());
                flow.vars.insert(name.clone(), rhs_null);
                if matches!(ty, Type::Nullable(_)) || rhs_prov {
                    flow.prov_vars.insert(name.clone());
                }
            }
            Stmt::Assign { lhs, rhs, .. } => {
                check_expr(rhs, flow, ctx, file, &mut diags);
                if !matches!(lhs, Expr::Var(..)) {
                    check_expr(lhs, flow, ctx, file, &mut diags);
                }
                let rhs_null = nullness_of(rhs, flow);
                let rhs_prov = is_provably_nullable(rhs, flow, ctx);
                match as_target(lhs) {
                    Some(Target::Var(name)) => {
                        flow.vars.insert(name.clone(), rhs_null);
                        if rhs_prov {
                            flow.prov_vars.insert(name);
                        }
                    }
                    Some(Target::FieldPath(key)) => {
                        flow.fields.insert(key.clone(), rhs_null);
                        if rhs_prov {
                            flow.prov_fields.insert(key);
                        }
                    }
                    None => {}
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    check_expr(v, flow, ctx, file, &mut diags);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                check_expr(cond, flow, ctx, file, &mut diags);

                let mut then_flow = NullEnv {
                    vars: flow.vars.clone(),
                    fields: flow.fields.clone(),
                    prov_vars: flow.prov_vars.clone(),
                    prov_fields: flow.prov_fields.clone(),
                    var_types: flow.var_types.clone(),
                };
                narrow(&mut then_flow, cond, true);
                diags.extend(check_stmts(then_body, &mut then_flow, ctx, file));

                let mut else_flow = NullEnv {
                    vars: flow.vars.clone(),
                    fields: flow.fields.clone(),
                    prov_vars: flow.prov_vars.clone(),
                    prov_fields: flow.prov_fields.clone(),
                    var_types: flow.var_types.clone(),
                };
                narrow(&mut else_flow, cond, false);
                diags.extend(check_stmts(else_body, &mut else_flow, ctx, file));

                // Join both successors back into the current environment.
                merge_into(flow, &then_flow);
                merge_into(flow, &else_flow);
            }
            Stmt::While { cond, body, .. } => {
                check_expr(cond, flow, ctx, file, &mut diags);
                let mut body_flow = NullEnv {
                    vars: flow.vars.clone(),
                    fields: flow.fields.clone(),
                    prov_vars: flow.prov_vars.clone(),
                    prov_fields: flow.prov_fields.clone(),
                    var_types: flow.var_types.clone(),
                };
                narrow(&mut body_flow, cond, true);
                diags.extend(check_stmts(body, &mut body_flow, ctx, file));
                // The loop may run zero times: join the body exit with the
                // pre-loop state instead of havocing to `Maybe`, so values
                // that stay `NonNull` on every path are not spuriously
                // flagged while genuinely-nullable ones still are.
                merge_into(flow, &body_flow);
            }
            Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
                check_expr(value, flow, ctx, file, &mut diags);
            }
            Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
                check_expr(expr, flow, ctx, file, &mut diags);
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
    diags
}

/// Run the null-safety pass over all method bodies and the `main` block.
#[must_use]
pub fn check_null_safety(files: &[File], env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for file in files {
        for cls in &file.classes {
            let ctx = Ctx { env };
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                let mut flow = NullEnv::new();
                flow.vars.insert("this".to_owned(), Nullness::NonNull);
                flow.var_types
                    .insert("this".to_owned(), Type::Named(cls.name.clone()));
                for param in &method.params {
                    flow.var_types.insert(param.name.clone(), param.ty.clone());
                    flow.vars.insert(param.name.clone(), Nullness::Maybe);
                }
                diags.extend(check_stmts(&method.body, &mut flow, &ctx, &file.path));
            }
        }
        if let Some(main_stmts) = &file.main {
            let ctx = Ctx { env };
            let mut flow = NullEnv::new();
            diags.extend(check_stmts(main_stmts, &mut flow, &ctx, &file.path));
        }
    }
    diags
}

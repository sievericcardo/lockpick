//! SPARQL injection taint analysis (plan §2.8).
//!
//! Values that originate from `access(…)` or `construct(…)` calls contain
//! data from the RDF knowledge graph. If such a tainted value is later passed
//! as a positional argument (`%1`, `%2`, …) to another `access`/`construct`
//! call, a potential SPARQL-injection is flagged as a warning.
//!
//! # Inter-procedural summaries (KeY/Crowbar-style method contracts)
//!
//! Beyond the intra-procedural flow (taint through assignment and field reads
//! on tainted variables), each method gets a **summary** computed to a fixed
//! point over the call graph:
//!
//! - `sink_params`: formal parameters that flow into an `access`/`construct`
//!   query argument (taint sinks);
//! - `returns_tainted`: the method unconditionally returns query-derived data;
//! - `return_taint_params`: formal parameters whose taint flows to the return
//!   value.
//!
//! Call sites then apply the callee summary: passing a tainted actual for a
//! `sink_params` formal warns (taint crossing a method boundary into a query),
//! and assigning `y = obj.m(…)` taints `y` when the summary says the result
//! is query-derived. The fixed-point iteration is monotone (sets only grow)
//! over a finite domain, so it always terminates — the same architecture as
//! Crowbar's behavioral-contract summaries discharged per method.
//!
//! # Heap taint (plan §2.8, heap/ontology flows)
//!
//! Taint can also flow through the heap:
//!
//! - `x.f = tainted` / `this.f = tainted` marks the heap slot `(x, f)` tainted;
//! - `l = new List<T>(tainted)` / `new ExplList<T>(tainted)` marks the list
//!   slot `l` tainted; `l.content = tainted` / `l.next = tainted` likewise
//!   unions into the list slot;
//! - `y = x.f` propagates the `(x, f)` slot taint to `y`;
//! - `y = l.content` / `y = l.next` (and chained `.content`/`.next`
//!   traversals) propagates the list slot taint to `y`. A `.next` result is
//!   itself a list, so it additionally carries the list-slot taint forward,
//!   enabling chains like `l.next.content`.
//!
//! The existing `W3016` (`SparqlInjection`) diagnostic then fires when a
//! heap-read tainted value flows into an `access`/`construct` positional arg.
//! The model is intentionally narrow (per-variable slots, strong updates on
//! direct overwrites) so clean lists/fields are never flagged: taint appears
//! in a slot only after a provably-tainted store on the same slot/variable.

use std::collections::{HashMap, HashSet};

use smol_syntax::ast::{Expr, File, Stmt, Type};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::class_env::ClassEnv;

/// Taint lattice element: `Query` (data from `access`/`construct`) plus the
/// set of formal-parameter sources taint may originate from.
#[derive(Clone, Default, Debug)]
struct TSet {
    query: bool,
    params: HashSet<String>,
}

impl TSet {
    fn param(name: &str) -> Self {
        let mut s = Self::default();
        s.params.insert(name.to_owned());
        return s;
    }

    fn query() -> Self {
        Self {
            query: true,
            params: HashSet::new(),
        }
    }

    fn union(&mut self, other: &Self) {
        self.query |= other.query;
        self.params.extend(other.params.iter().cloned());
    }

    fn is_empty(&self) -> bool {
        !self.query && self.params.is_empty()
    }
}

/// Method summary used across call boundaries.
#[derive(Clone, Default, Debug)]
struct Summary {
    /// Formals that flow into a query argument.
    sink_params: HashSet<String>,
    /// The return value is unconditionally query-derived.
    returns_tainted: bool,
    /// Formals whose taint flows to the return value.
    return_taint_params: HashSet<String>,
}

struct MethodEntry<'a> {
    this: &'a str,
    name: &'a str,
    params: Vec<String>,
    body: &'a [Stmt],
}

/// Map local variable names to their declared class (for `Named` types).
fn local_classes(stmts: &[Stmt]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    collect_locals(stmts, &mut map);
    map
}

fn collect_locals(stmts: &[Stmt], map: &mut HashMap<String, String>) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { ty, name, .. } => {
                if let Type::Named(c) = ty {
                    map.insert(name.clone(), c.clone());
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_locals(then_body, map);
                collect_locals(else_body, map);
            }
            Stmt::While { body, .. } => collect_locals(body, map),
            _ => {}
        }
    }
}

fn receiver_class(
    receiver: &Expr,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> Option<String> {
    match receiver {
        Expr::Var(name, _) => {
            if name == "this" {
                Some(this.to_owned())
            } else if let Some(c) = locals.get(name) {
                Some(c.clone())
            } else {
                env.resolve_field(this, name).and_then(|f| match &f.ty {
                    Type::Named(c) => Some(c.clone()),
                    _ => None,
                })
            }
        }
        Expr::Field {
            receiver, field, ..
        } => {
            let recv = receiver_class(receiver, this, locals, env)?;
            env.resolve_field(&recv, field).and_then(|f| match &f.ty {
                Type::Named(c) => Some(c.clone()),
                _ => None,
            })
        }
        _ => None,
    }
}

/// Resolve `(holder, method)` to the declaring class + formal parameter names.
fn resolve_callee<'e>(
    holder: &str,
    method: &str,
    env: &'e ClassEnv,
) -> Option<(String, Vec<String>)> {
    let mut current = holder;
    loop {
        let info = env.get(current)?;
        if let Some(m) = info.own_method(method) {
            return Some((
                current.to_owned(),
                m.params.iter().map(|p| p.name.clone()).collect(),
            ));
        }
        current = info.extends.as_deref()?;
    }
}

// ---------------------------------------------------------------------------
// Heap helpers (shared vocabulary)
// ---------------------------------------------------------------------------

fn is_list_new(expr: &Expr) -> bool {
    matches!(expr, Expr::New { class, .. } if class == "List" || class == "ExplList")
}

fn is_list_field(field: &str) -> bool {
    field == "content" || field == "next"
}

fn base_var(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var(name, _) => Some(name.as_str()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Symbolic (summary) pass with heap slots
// ---------------------------------------------------------------------------

/// Heap-derived symbolic taint for a `receiver.field` read.
///
/// - `content`/`next` on `Var(v)` yields the list slot `v`;
/// - any other field on `Var(v)` yields the field slot `(v, field)`;
/// - chained receivers (e.g. `a.next.content`) resolve through the recursive
///   `sym_taint` of the receiver, so no extra lookup is needed here.
fn sym_heap_lookup(
    receiver: &Expr,
    field: &str,
    field_heap: &HashMap<(String, String), TSet>,
    list_heap: &HashMap<String, TSet>,
) -> TSet {
    if let Some(base) = base_var(receiver) {
        if is_list_field(field) {
            return list_heap.get(base).cloned().unwrap_or_default();
        }
        let key = (base.to_owned(), field.to_owned());
        return field_heap.get(&key).cloned().unwrap_or_default();
    }
    TSet::default()
}

/// Symbolic taint of an expression under abstract env `tainted`.
#[allow(clippy::too_many_arguments)]
fn sym_taint(
    expr: &Expr,
    tainted: &HashMap<String, TSet>,
    field_heap: &HashMap<(String, String), TSet>,
    list_heap: &HashMap<String, TSet>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> TSet {
    match expr {
        Expr::Var(name, _) => tainted.get(name).cloned().unwrap_or_default(),
        Expr::Access { .. } | Expr::Construct { .. } => TSet::query(),
        Expr::Field {
            receiver, field, ..
        } => {
            let mut out = sym_taint(
                receiver, tainted, field_heap, list_heap, summaries, this, locals, env,
            );
            out.union(&sym_heap_lookup(receiver, field, field_heap, list_heap));
            out
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let mut out = TSet::default();
            if let Some(holder) = receiver_class(receiver, this, locals, env)
                && let Some((decl, formals)) = resolve_callee(&holder, method, env)
                && let Some(sum) = summaries.get(&(decl, method.clone()))
            {
                if sum.returns_tainted {
                    out.query = true;
                }
                for (arg, formal) in args.iter().zip(formals.iter()) {
                    if sum.return_taint_params.contains(formal) {
                        let t = sym_taint(
                            arg, tainted, field_heap, list_heap, summaries, this, locals, env,
                        );
                        out.union(&t);
                    }
                }
            }
            out
        }
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. }
        | Expr::Simulate { args, .. } => {
            let out = TSet::default();
            for a in args {
                // Constructor / builtin results are not query-tainted themselves,
                // but their arguments may carry param taint into fields — which
                // the field-read rule picks up only for tainted locals. Here we
                // propagate nothing (conservative against false positives).
                let _ = sym_taint(
                    a, tainted, field_heap, list_heap, summaries, this, locals, env,
                );
            }
            out
        }
        Expr::Classify { expr, .. } | Expr::UnaryOp { expr, .. } => sym_taint(
            expr, tainted, field_heap, list_heap, summaries, this, locals, env,
        ),
        Expr::BinOp { left, right, .. } => {
            let mut out = sym_taint(
                left, tainted, field_heap, list_heap, summaries, this, locals, env,
            );
            out.union(&sym_taint(
                right, tainted, field_heap, list_heap, summaries, this, locals, env,
            ));
            out
        }
        Expr::Port { receiver, .. } => sym_taint(
            receiver, tainted, field_heap, list_heap, summaries, this, locals, env,
        ),
        _ => TSet::default(),
    }
}

/// Union of argument taints for `new List/ExplList(args)` — the value stored
/// into the fresh list slot.
#[allow(clippy::too_many_arguments)]
fn sym_new_list_taint(
    args: &[Expr],
    tainted: &HashMap<String, TSet>,
    field_heap: &HashMap<(String, String), TSet>,
    list_heap: &HashMap<String, TSet>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> TSet {
    let mut out = TSet::default();
    for a in args {
        out.union(&sym_taint(
            a, tainted, field_heap, list_heap, summaries, this, locals, env,
        ));
    }
    out
}

/// Full value carried by `rhs` when stored into the heap: direct taint plus,
/// for list-valued RHS, the element taint it already carries.
#[allow(clippy::too_many_arguments)]
fn sym_store_value(
    rhs: &Expr,
    tainted: &HashMap<String, TSet>,
    field_heap: &HashMap<(String, String), TSet>,
    list_heap: &HashMap<String, TSet>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> TSet {
    let mut out = sym_taint(
        rhs, tainted, field_heap, list_heap, summaries, this, locals, env,
    );
    if let Expr::New { args, .. } = rhs
        && is_list_new(rhs)
    {
        out.union(&sym_new_list_taint(
            args, tainted, field_heap, list_heap, summaries, this, locals, env,
        ));
    } else if let Expr::Var(name, _) = rhs
        && let Some(lt) = list_heap.get(name)
    {
        out.union(lt);
    }
    out
}

/// Record `lhs_var = rhs` in the symbolic state (direct + list-slot flow).
#[allow(clippy::too_many_arguments)]
fn sym_assign_to_var(
    lhs_var: &str,
    rhs: &Expr,
    tainted: &mut HashMap<String, TSet>,
    field_heap: &mut HashMap<(String, String), TSet>,
    list_heap: &mut HashMap<String, TSet>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) {
    let t = sym_taint(
        rhs, tainted, field_heap, list_heap, summaries, this, locals, env,
    );
    if t.is_empty() {
        tainted.remove(lhs_var);
    } else {
        tainted.insert(lhs_var.to_owned(), t.clone());
    }
    // List-slot propagation for the fresh/overwritten variable.
    if let Expr::New { args, .. } = rhs
        && is_list_new(rhs)
    {
        let lt = sym_new_list_taint(
            args, tainted, field_heap, list_heap, summaries, this, locals, env,
        );
        // NB: `tainted` was just updated; recompute against pre-state is
        // unnecessary here because list args cannot name `lhs_var`'s new
        // value yet. `lt` was computed from the old state for the args.
        if lt.is_empty() {
            list_heap.remove(lhs_var);
        } else {
            list_heap.insert(lhs_var.to_owned(), lt);
        }
    } else if let Expr::Var(src, _) = rhs {
        match list_heap.get(src).cloned() {
            Some(lt) if !lt.is_empty() => {
                list_heap.insert(lhs_var.to_owned(), lt);
            }
            _ => {
                list_heap.remove(lhs_var);
            }
        }
        // Object alias: carry known field slots of `src` over to `lhs_var`
        // (best-effort for `y = x; y.f` reads after an earlier `x.f` store).
        let aliased: Vec<((String, String), TSet)> = field_heap
            .iter()
            .filter(|((b, _), _)| b == src)
            .map(|((_, f), v)| ((lhs_var.to_owned(), f.clone()), v.clone()))
            .collect();
        // Drop stale slots of a reassigned variable first.
        let stale: Vec<(String, String)> = field_heap
            .keys()
            .filter(|(b, _)| b == lhs_var)
            .cloned()
            .collect();
        for k in stale {
            field_heap.remove(&k);
        }
        for (k, v) in aliased {
            field_heap.insert(k, v);
        }
    } else if let Expr::Field { field, .. } = rhs
        && is_list_field(field)
    {
        // `x = y.next` — the result is itself a list carrying the traversal.
        if t.is_empty() {
            list_heap.remove(lhs_var);
        } else {
            list_heap.insert(lhs_var.to_owned(), t);
        }
    } else {
        list_heap.remove(lhs_var);
        // A plain overwrite kills stale object-field knowledge for this var.
        let stale: Vec<(String, String)> = field_heap
            .keys()
            .filter(|(b, _)| b == lhs_var)
            .cloned()
            .collect();
        for k in stale {
            field_heap.remove(&k);
        }
    }
}

/// Record `receiver.field = rhs` in the symbolic heap state.
#[allow(clippy::too_many_arguments)]
fn sym_store_to_field(
    receiver: &Expr,
    field: &str,
    rhs: &Expr,
    tainted: &HashMap<String, TSet>,
    field_heap: &mut HashMap<(String, String), TSet>,
    list_heap: &mut HashMap<String, TSet>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) {
    let Some(base) = base_var(receiver) else {
        return;
    };
    let v = sym_store_value(
        rhs, tainted, field_heap, list_heap, summaries, this, locals, env,
    );
    if is_list_field(field) {
        // A list slot accumulates every stored element (`new` args plus
        // `l.content = …` stores); union keeps earlier elements' taint.
        if v.is_empty() {
            // Clean overwrite of one element cannot kill taint possibly held
            // by other elements — keep the slot (may-analysis, FP-free).
        } else {
            list_heap.entry(base.to_owned()).or_default().union(&v);
        }
    } else if v.is_empty() {
        field_heap.remove(&(base.to_owned(), field.to_owned()));
    } else {
        field_heap.insert((base.to_owned(), field.to_owned()), v);
    }
}

fn sym_join(
    dst_tainted: &mut HashMap<String, TSet>,
    dst_fields: &mut HashMap<(String, String), TSet>,
    dst_lists: &mut HashMap<String, TSet>,
    src_tainted: HashMap<String, TSet>,
    src_fields: HashMap<(String, String), TSet>,
    src_lists: HashMap<String, TSet>,
) {
    for (k, v) in src_tainted {
        dst_tainted.entry(k).or_default().union(&v);
    }
    for (k, v) in src_fields {
        dst_fields.entry(k).or_default().union(&v);
    }
    for (k, v) in src_lists {
        dst_lists.entry(k).or_default().union(&v);
    }
}

/// Walk statements computing symbolic taint, sink hits, and return taint.
#[allow(clippy::too_many_arguments)]
fn walk_stmts_sym(
    stmts: &[Stmt],
    tainted: &mut HashMap<String, TSet>,
    field_heap: &mut HashMap<(String, String), TSet>,
    list_heap: &mut HashMap<String, TSet>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
    sink_hits: &mut HashSet<String>,
    returns: &mut TSet,
) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { name, init, .. } => {
                walk_expr_sym(
                    init, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                );
                sym_assign_to_var(
                    name, init, tainted, field_heap, list_heap, summaries, this, locals, env,
                );
            }
            Stmt::Assign { lhs, rhs, .. } => {
                walk_expr_sym(
                    rhs, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                );
                match lhs {
                    Expr::Var(name, _) => {
                        sym_assign_to_var(
                            name, rhs, tainted, field_heap, list_heap, summaries, this, locals, env,
                        );
                    }
                    Expr::Field {
                        receiver, field, ..
                    } => {
                        sym_store_to_field(
                            receiver, field, rhs, tainted, field_heap, list_heap, summaries, this,
                            locals, env,
                        );
                    }
                    _ => {}
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    walk_expr_sym(
                        v, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                    );
                    let t = sym_taint(
                        v, tainted, field_heap, list_heap, summaries, this, locals, env,
                    );
                    returns.union(&t);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                walk_expr_sym(
                    cond, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                );
                let mut then_t = tainted.clone();
                let mut then_f = field_heap.clone();
                let mut then_l = list_heap.clone();
                walk_stmts_sym(
                    then_body,
                    &mut then_t,
                    &mut then_f,
                    &mut then_l,
                    summaries,
                    this,
                    locals,
                    env,
                    sink_hits,
                    returns,
                );
                let mut else_t = tainted.clone();
                let mut else_f = field_heap.clone();
                let mut else_l = list_heap.clone();
                walk_stmts_sym(
                    else_body,
                    &mut else_t,
                    &mut else_f,
                    &mut else_l,
                    summaries,
                    this,
                    locals,
                    env,
                    sink_hits,
                    returns,
                );
                sym_join(tainted, field_heap, list_heap, then_t, then_f, then_l);
                sym_join(tainted, field_heap, list_heap, else_t, else_f, else_l);
            }
            Stmt::While { cond, body, .. } => {
                walk_expr_sym(
                    cond, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                );
                let mut body_t = tainted.clone();
                let mut body_f = field_heap.clone();
                let mut body_l = list_heap.clone();
                walk_stmts_sym(
                    body,
                    &mut body_t,
                    &mut body_f,
                    &mut body_l,
                    summaries,
                    this,
                    locals,
                    env,
                    sink_hits,
                    returns,
                );
                sym_join(tainted, field_heap, list_heap, body_t, body_f, body_l);
            }
            Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
                walk_expr_sym(
                    value, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                );
            }
            Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
                walk_expr_sym(
                    expr, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                );
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
}

/// Walk an expression recording sink hits (query args receiving param/query taint).
#[allow(clippy::too_many_arguments)]
fn walk_expr_sym(
    expr: &Expr,
    tainted: &HashMap<String, TSet>,
    field_heap: &HashMap<(String, String), TSet>,
    list_heap: &HashMap<String, TSet>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
    sink_hits: &mut HashSet<String>,
) {
    match expr {
        Expr::Access { args, .. } | Expr::Construct { args, .. } => {
            for arg in args {
                let t = sym_taint(
                    arg, tainted, field_heap, list_heap, summaries, this, locals, env,
                );
                sink_hits.extend(t.params.iter().cloned());
                walk_expr_sym(
                    arg, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                );
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            // Inter-procedural sink: a param-tainted actual passed for a
            // callee sink formal propagates the sink hit to the caller param.
            if let Some(holder) = receiver_class(receiver, this, locals, env)
                && let Some((decl, formals)) = resolve_callee(&holder, method, env)
                && let Some(sum) = summaries.get(&(decl, method.clone()))
            {
                for (arg, formal) in args.iter().zip(formals.iter()) {
                    if sum.sink_params.contains(formal) {
                        let t = sym_taint(
                            arg, tainted, field_heap, list_heap, summaries, this, locals, env,
                        );
                        sink_hits.extend(t.params.iter().cloned());
                    }
                }
            }
            walk_expr_sym(
                receiver, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
            );
            for a in args {
                walk_expr_sym(
                    a, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                );
            }
        }
        Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => {
            walk_expr_sym(
                receiver, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
            );
        }
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. }
        | Expr::Simulate { args, .. } => {
            for a in args {
                walk_expr_sym(
                    a, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
                );
            }
        }
        Expr::Classify { expr, .. } | Expr::UnaryOp { expr, .. } => {
            walk_expr_sym(
                expr, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
            );
        }
        Expr::BinOp { left, right, .. } => {
            walk_expr_sym(
                left, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
            );
            walk_expr_sym(
                right, tainted, field_heap, list_heap, summaries, this, locals, env, sink_hits,
            );
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Concrete (diagnostic) pass: booleans instead of param sets, summaries applied.
// ---------------------------------------------------------------------------

fn concrete_heap_lookup(
    receiver: &Expr,
    field: &str,
    field_heap: &HashSet<(String, String)>,
    list_heap: &HashSet<String>,
) -> bool {
    if let Some(base) = base_var(receiver) {
        if is_list_field(field) {
            return list_heap.contains(base);
        }
        return field_heap.contains(&(base.to_owned(), field.to_owned()));
    }
    false
}

/// Concrete taint of an expression: true = query-derived at runtime.
#[allow(clippy::too_many_arguments)]
fn is_tainted(
    expr: &Expr,
    tainted: &HashSet<String>,
    field_heap: &HashSet<(String, String)>,
    list_heap: &HashSet<String>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> bool {
    match expr {
        Expr::Var(name, _) => tainted.contains(name.as_str()),
        Expr::Access { .. } | Expr::Construct { .. } => true,
        Expr::Field {
            receiver, field, ..
        } => {
            if is_tainted(
                receiver, tainted, field_heap, list_heap, summaries, this, locals, env,
            ) {
                return true;
            }
            concrete_heap_lookup(receiver, field, field_heap, list_heap)
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            if let Some(holder) = receiver_class(receiver, this, locals, env)
                && let Some((decl, formals)) = resolve_callee(&holder, method, env)
                && let Some(sum) = summaries.get(&(decl, method.clone()))
            {
                if sum.returns_tainted {
                    return true;
                }
                for (arg, formal) in args.iter().zip(formals.iter()) {
                    if sum.return_taint_params.contains(formal)
                        && is_tainted(
                            arg, tainted, field_heap, list_heap, summaries, this, locals, env,
                        )
                    {
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn concrete_new_list_tainted(
    args: &[Expr],
    tainted: &HashSet<String>,
    field_heap: &HashSet<(String, String)>,
    list_heap: &HashSet<String>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> bool {
    args.iter().any(|a| {
        is_tainted(
            a, tainted, field_heap, list_heap, summaries, this, locals, env,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn concrete_store_value(
    rhs: &Expr,
    tainted: &HashSet<String>,
    field_heap: &HashSet<(String, String)>,
    list_heap: &HashSet<String>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) -> bool {
    if is_tainted(
        rhs, tainted, field_heap, list_heap, summaries, this, locals, env,
    ) {
        return true;
    }
    if is_list_new(rhs)
        && let Expr::New { args, .. } = rhs
    {
        return concrete_new_list_tainted(
            args, tainted, field_heap, list_heap, summaries, this, locals, env,
        );
    }
    if let Expr::Var(name, _) = rhs
        && list_heap.contains(name.as_str())
    {
        return true;
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn concrete_assign_to_var(
    lhs_var: &str,
    rhs: &Expr,
    tainted: &mut HashSet<String>,
    field_heap: &mut HashSet<(String, String)>,
    list_heap: &mut HashSet<String>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) {
    if is_tainted(
        rhs, tainted, field_heap, list_heap, summaries, this, locals, env,
    ) {
        tainted.insert(lhs_var.to_owned());
    } else {
        tainted.remove(lhs_var);
    }
    if let Expr::New { args, .. } = rhs
        && is_list_new(rhs)
    {
        if concrete_new_list_tainted(
            args, tainted, field_heap, list_heap, summaries, this, locals, env,
        ) {
            list_heap.insert(lhs_var.to_owned());
        } else {
            list_heap.remove(lhs_var);
        }
    } else if let Expr::Var(src, _) = rhs {
        if list_heap.contains(src.as_str()) {
            list_heap.insert(lhs_var.to_owned());
        } else {
            list_heap.remove(lhs_var);
        }
        // Best-effort object alias: carry `src.*` slots to `lhs_var.*`.
        let aliased: Vec<(String, String)> = field_heap
            .iter()
            .filter(|(b, _)| b == src)
            .map(|(_, f)| (lhs_var.to_owned(), f.clone()))
            .collect();
        field_heap.retain(|(b, _)| b != lhs_var);
        for k in aliased {
            field_heap.insert(k);
        }
    } else if let Expr::Field { field, .. } = rhs
        && is_list_field(field)
    {
        if is_tainted(
            rhs, tainted, field_heap, list_heap, summaries, this, locals, env,
        ) {
            list_heap.insert(lhs_var.to_owned());
        } else {
            list_heap.remove(lhs_var);
        }
    } else {
        list_heap.remove(lhs_var);
        field_heap.retain(|(b, _)| b != lhs_var);
    }
}

#[allow(clippy::too_many_arguments)]
fn concrete_store_to_field(
    receiver: &Expr,
    field: &str,
    rhs: &Expr,
    tainted: &HashSet<String>,
    field_heap: &mut HashSet<(String, String)>,
    list_heap: &mut HashSet<String>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
) {
    let Some(base) = base_var(receiver) else {
        return;
    };
    let v = concrete_store_value(
        rhs, tainted, field_heap, list_heap, summaries, this, locals, env,
    );
    if is_list_field(field) {
        // Accumulate: a clean store cannot prove the whole list clean.
        if v {
            list_heap.insert(base.to_owned());
        }
    } else if v {
        field_heap.insert((base.to_owned(), field.to_owned()));
    } else {
        field_heap.remove(&(base.to_owned(), field.to_owned()));
    }
}

#[allow(clippy::too_many_arguments)]
fn warn_if_tainted(
    arg: &Expr,
    tainted: &HashSet<String>,
    field_heap: &HashSet<(String, String)>,
    list_heap: &HashSet<String>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
    file: &str,
    call_span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    if is_tainted(
        arg, tainted, field_heap, list_heap, summaries, this, locals, env,
    ) {
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

#[allow(clippy::too_many_arguments)]
fn check_expr_taint(
    expr: &Expr,
    tainted: &HashSet<String>,
    field_heap: &HashSet<(String, String)>,
    list_heap: &HashSet<String>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
    file: &str,
    diags: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Access { args, span, .. } | Expr::Construct { args, span, .. } => {
            for arg in args {
                warn_if_tainted(
                    arg, tainted, field_heap, list_heap, summaries, this, locals, env, file, *span,
                    diags,
                );
                check_expr_taint(
                    arg, tainted, field_heap, list_heap, summaries, this, locals, env, file, diags,
                );
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
            ..
        } => {
            // Cross-boundary sink: tainted actual for a callee sink formal.
            if let Some(holder) = receiver_class(receiver, this, locals, env)
                && let Some((decl, formals)) = resolve_callee(&holder, method, env)
                && let Some(sum) = summaries.get(&(decl, method.clone()))
            {
                for (arg, formal) in args.iter().zip(formals.iter()) {
                    if sum.sink_params.contains(formal)
                        && is_tainted(
                            arg, tainted, field_heap, list_heap, summaries, this, locals, env,
                        )
                    {
                        diags.push(
                            Diagnostic::warning(
                                DiagnosticCode::SparqlInjection,
                                format!(
                                    "tainted value flows into SPARQL query via method `{method}` \
                                     (parameter `{formal}` is passed to an \
                                     `access`/`construct` query inside the callee)"
                                ),
                            )
                            .with_file(file)
                            .with_span(*span),
                        );
                    }
                }
            }
            check_expr_taint(
                receiver, tainted, field_heap, list_heap, summaries, this, locals, env, file, diags,
            );
            for arg in args {
                check_expr_taint(
                    arg, tainted, field_heap, list_heap, summaries, this, locals, env, file, diags,
                );
            }
        }
        Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => {
            check_expr_taint(
                receiver, tainted, field_heap, list_heap, summaries, this, locals, env, file, diags,
            );
        }
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. }
        | Expr::Simulate { args, .. } => {
            for arg in args {
                check_expr_taint(
                    arg, tainted, field_heap, list_heap, summaries, this, locals, env, file, diags,
                );
            }
        }
        Expr::Classify { expr, .. } | Expr::UnaryOp { expr, .. } => {
            check_expr_taint(
                expr, tainted, field_heap, list_heap, summaries, this, locals, env, file, diags,
            );
        }
        Expr::BinOp { left, right, .. } => {
            check_expr_taint(
                left, tainted, field_heap, list_heap, summaries, this, locals, env, file, diags,
            );
            check_expr_taint(
                right, tainted, field_heap, list_heap, summaries, this, locals, env, file, diags,
            );
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn check_stmts_taint(
    stmts: &[Stmt],
    tainted: &mut HashSet<String>,
    field_heap: &mut HashSet<(String, String)>,
    list_heap: &mut HashSet<String>,
    summaries: &HashMap<(String, String), Summary>,
    this: &str,
    locals: &HashMap<String, String>,
    env: &ClassEnv,
    file: &str,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl { name, init, .. } => {
                check_expr_taint(
                    init, tainted, field_heap, list_heap, summaries, this, locals, env, file,
                    &mut diags,
                );
                concrete_assign_to_var(
                    name, init, tainted, field_heap, list_heap, summaries, this, locals, env,
                );
            }
            Stmt::Assign { lhs, rhs, .. } => {
                check_expr_taint(
                    rhs, tainted, field_heap, list_heap, summaries, this, locals, env, file,
                    &mut diags,
                );
                match lhs {
                    Expr::Var(name, _) => {
                        concrete_assign_to_var(
                            name, rhs, tainted, field_heap, list_heap, summaries, this, locals, env,
                        );
                    }
                    Expr::Field {
                        receiver, field, ..
                    } => {
                        concrete_store_to_field(
                            receiver, field, rhs, tainted, field_heap, list_heap, summaries, this,
                            locals, env,
                        );
                    }
                    _ => {}
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    check_expr_taint(
                        v, tainted, field_heap, list_heap, summaries, this, locals, env, file,
                        &mut diags,
                    );
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                check_expr_taint(
                    cond, tainted, field_heap, list_heap, summaries, this, locals, env, file,
                    &mut diags,
                );
                let mut then_t = tainted.clone();
                let mut then_f = field_heap.clone();
                let mut then_l = list_heap.clone();
                diags.extend(check_stmts_taint(
                    then_body,
                    &mut then_t,
                    &mut then_f,
                    &mut then_l,
                    summaries,
                    this,
                    locals,
                    env,
                    file,
                ));
                let mut else_t = tainted.clone();
                let mut else_f = field_heap.clone();
                let mut else_l = list_heap.clone();
                diags.extend(check_stmts_taint(
                    else_body,
                    &mut else_t,
                    &mut else_f,
                    &mut else_l,
                    summaries,
                    this,
                    locals,
                    env,
                    file,
                ));
                for name in then_t {
                    tainted.insert(name);
                }
                for name in else_t {
                    tainted.insert(name);
                }
                for k in then_f {
                    field_heap.insert(k);
                }
                for k in else_f {
                    field_heap.insert(k);
                }
                for v in then_l {
                    list_heap.insert(v);
                }
                for v in else_l {
                    list_heap.insert(v);
                }
            }
            Stmt::While { cond, body, .. } => {
                check_expr_taint(
                    cond, tainted, field_heap, list_heap, summaries, this, locals, env, file,
                    &mut diags,
                );
                let mut body_t = tainted.clone();
                let mut body_f = field_heap.clone();
                let mut body_l = list_heap.clone();
                diags.extend(check_stmts_taint(
                    body,
                    &mut body_t,
                    &mut body_f,
                    &mut body_l,
                    summaries,
                    this,
                    locals,
                    env,
                    file,
                ));
                for name in body_t {
                    tainted.insert(name);
                }
                for k in body_f {
                    field_heap.insert(k);
                }
                for v in body_l {
                    list_heap.insert(v);
                }
            }
            Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
                check_expr_taint(
                    value, tainted, field_heap, list_heap, summaries, this, locals, env, file,
                    &mut diags,
                );
            }
            Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
                check_expr_taint(
                    expr, tainted, field_heap, list_heap, summaries, this, locals, env, file,
                    &mut diags,
                );
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }
    diags
}

/// Compute method summaries to a fixed point over the call graph.
fn compute_summaries<'a>(
    methods: &[MethodEntry<'a>],
    env: &ClassEnv,
) -> HashMap<(String, String), Summary> {
    let mut summaries: HashMap<(String, String), Summary> = HashMap::new();
    for m in methods {
        summaries.insert((m.this.to_owned(), m.name.to_owned()), Summary::default());
    }

    loop {
        let mut changed = false;
        for m in methods {
            let locals = local_classes(m.body);
            let mut tainted: HashMap<String, TSet> = HashMap::new();
            for p in &m.params {
                tainted.insert(p.clone(), TSet::param(p));
            }
            let mut field_heap: HashMap<(String, String), TSet> = HashMap::new();
            let mut list_heap: HashMap<String, TSet> = HashMap::new();
            let mut sink_hits = HashSet::new();
            let mut returns = TSet::default();
            walk_stmts_sym(
                m.body,
                &mut tainted,
                &mut field_heap,
                &mut list_heap,
                &summaries,
                m.this,
                &locals,
                env,
                &mut sink_hits,
                &mut returns,
            );

            let key = (m.this.to_owned(), m.name.to_owned());
            if let Some(sum) = summaries.get_mut(&key) {
                let before = (
                    sum.sink_params.len(),
                    sum.returns_tainted,
                    sum.return_taint_params.len(),
                );
                sum.sink_params.extend(sink_hits);
                if returns.query {
                    sum.returns_tainted = true;
                }
                sum.return_taint_params.extend(returns.params);
                let after = (
                    sum.sink_params.len(),
                    sum.returns_tainted,
                    sum.return_taint_params.len(),
                );
                if before != after {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    summaries
}

/// Run the SPARQL-taint pass over all method bodies and the `main` block.
#[must_use]
pub fn check_sparql_taint(files: &[File], env: &ClassEnv) -> Vec<Diagnostic> {
    // Collect method entries for summary computation.
    let mut methods: Vec<MethodEntry<'_>> = Vec::new();
    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                methods.push(MethodEntry {
                    this: &cls.name,
                    name: &method.name,
                    params: method.params.iter().map(|p| p.name.clone()).collect(),
                    body: &method.body,
                });
            }
        }
    }
    let summaries = compute_summaries(&methods, env);

    let mut diags = Vec::new();
    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                let locals = local_classes(&method.body);
                let mut tainted: HashSet<String> = HashSet::new();
                let mut field_heap: HashSet<(String, String)> = HashSet::new();
                let mut list_heap: HashSet<String> = HashSet::new();
                diags.extend(check_stmts_taint(
                    &method.body,
                    &mut tainted,
                    &mut field_heap,
                    &mut list_heap,
                    &summaries,
                    &cls.name,
                    &locals,
                    env,
                    &file.path,
                ));
            }
        }
        if let Some(main_stmts) = &file.main {
            let locals = local_classes(main_stmts);
            let mut tainted: HashSet<String> = HashSet::new();
            let mut field_heap: HashSet<(String, String)> = HashSet::new();
            let mut list_heap: HashSet<String> = HashSet::new();
            diags.extend(check_stmts_taint(
                main_stmts,
                &mut tainted,
                &mut field_heap,
                &mut list_heap,
                &summaries,
                "",
                &locals,
                env,
                &file.path,
            ));
        }
    }
    diags
}

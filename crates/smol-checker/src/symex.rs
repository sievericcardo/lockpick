//! Symbolic execution with interval abstract interpretation (plan §§2.3–2.4).
//!
//! A KeY/Crowbar-inspired verification layer, scaled to what is decidable
//! without an external SMT solver:
//!
//! - **Interval domain** over `Int`: every integer expression evaluates to
//!   `[lo, hi]` (`None` = unbounded). Assignments transfer the abstract state
//!   forward; `if` branches narrow it from comparisons against constant *or*
//!   interval bounds; loop heads join entry and back-edge states, iterate the
//!   body a second time (N=2), then **widen** unstable bounds to `-/+inf`.
//!   `Add`/`Sub`/`Mul` (and unary negation) propagate full intervals through
//!   `i128` intermediates (no overflow panics; out-of-range finite bounds fall
//!   back to top to stay sound).
//! - **E2021 `IntegerOverflow`** (error): integer arithmetic whose exact
//!   interval is *provably* outside the `i64` range (`lo > i64::MAX` or
//!   `hi < i64::MIN` as `i128`), plus unbounded loop-level doublings
//!   (`x = x * k`, `|k| >= 2`, growing away from zero with no bound from the
//!   guard) detected after widening. Only definite cases are flagged, so the
//!   check stays false-positive-free.
//! - **E2022 `DivisionByZero`** (error): `/` or `%` whose divisor interval is
//!   provably the singleton `{0}` at that program point (path-sensitive:
//!   `if y == 0 then x / y` is caught, `if y != 0 then x / y` is not).
//!   Non-singleton divisors such as `[0, 5]` stay silent — no new false
//!   positives.
//! - **E5012 `LoopTerminationUnproven`** (warning): a `while` loop guarded by
//!   an integer comparison over a tracked `Int` variable, whose body shows no
//!   monotonic update of the compared variable toward the bound (the
//!   ranking-function / variant argument of plan §2.3, in syntactic form).
//!   The bound may be a constant *or* an integer expression over tracked
//!   `Int` variables; for variable bounds the warning is also suppressed when
//!   the body drives a bound variable in the opposite direction
//!   (`while i < n` terminates by raising `i` *or* lowering `n`).
//!   Integer bounds may be constants or expressions over tracked `Int`
//!   variables, including non-linear ones such as `n * n`; since this is a
//!   warning-level may-analysis, a loop whose bound is a non-linear
//!   expression may still be flagged when no syntactic `var = var ± k`
//!   progress is visible.
//!   A `decreases expr` clause acts as the explicit variant: it suppresses
//!   the warning when the body monotonically updates a variable of the
//!   clause toward the bound (e.g. `decreases 10 - i` with `i = i + 1`);
//!   a `decreases` clause with no visible monotonic update keeps the warning.
//!   Loops over lists (`E5008` territory) and boolean-variable guards are
//!   skipped to avoid duplicate or speculative diagnostics.
//!
//! # Soundness contract
//!
//! Errors (E2021/E2022) are emitted only for *definite* violations: interval
//! arithmetic provably outside `i64`, singleton-`{0}` divisors, or loop
//! doublings proven (post-widening) to grow without bound away from zero.
//! Joins and widening over-approximate, which can only *remove* definiteness,
//! never invent it. Termination is a may-analysis at warning level by design.

use std::collections::{HashMap, HashSet};

use smol_syntax::ast::{BinOp, Expr, File, Stmt, Type};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::class_env::ClassEnv;

/// Abstract integer value: `[lo, hi]`, `None` = unbounded on that side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Interval {
    lo: Option<i64>,
    hi: Option<i64>,
}

impl Interval {
    fn top() -> Self {
        Self { lo: None, hi: None }
    }

    fn singleton(v: i64) -> Self {
        Self {
            lo: Some(v),
            hi: Some(v),
        }
    }

    fn is_singleton(&self) -> Option<i64> {
        match (self.lo, self.hi) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        }
    }

    /// Least upper bound (used at control-flow merges).
    fn join(&self, other: &Self) -> Self {
        Self {
            lo: match (self.lo, other.lo) {
                (Some(a), Some(b)) => Some(a.min(b)),
                _ => None,
            },
            hi: match (self.hi, other.hi) {
                (Some(a), Some(b)) => Some(a.max(b)),
                _ => None,
            },
        }
    }

    /// Widening operator (used at loop heads after N=2 iterations):
    /// bounds that moved between the two head states are extrapolated to
    /// `-/+inf`; stable bounds are kept.
    fn widen(&self, other: &Self) -> Self {
        Self {
            lo: match (self.lo, other.lo) {
                (Some(a), Some(b)) => {
                    if b < a {
                        None
                    } else {
                        Some(a)
                    }
                }
                _ => None,
            },
            hi: match (self.hi, other.hi) {
                (Some(a), Some(b)) => {
                    if b > a {
                        None
                    } else {
                        Some(a)
                    }
                }
                _ => None,
            },
        }
    }
}

/// Finish an exact `i128` interval `[lo, hi]` (`None` = unbounded side).
///
/// Returns `(interval, overflow_proven)`; `overflow_proven` is true only when
/// the interval is *provably* outside the `i64` range (every value exceeds
/// `i64::MAX` or every value is below `i64::MIN`). Partially out-of-range
/// finite bounds represent *possible* (not definite) overflow, so they fall
/// back to top with `false` — sound and false-positive-free.
fn finish_interval(lo: Option<i128>, hi: Option<i128>) -> (Interval, bool) {
    let provable = matches!(lo, Some(l) if l > i64::MAX as i128)
        || matches!(hi, Some(h) if h < i64::MIN as i128);
    if provable {
        return (Interval::top(), true);
    }
    let in_range = |v: i128| v >= i64::MIN as i128 && v <= i64::MAX as i128;
    if lo.map_or(true, in_range) && hi.map_or(true, in_range) {
        (
            Interval {
                lo: lo.map(|v| v as i64),
                hi: hi.map(|v| v as i64),
            },
            false,
        )
    } else {
        // Possible but not definite overflow: give up precision soundly.
        (Interval::top(), false)
    }
}

fn add_iv(l: Interval, r: Interval) -> (Interval, bool) {
    let lo = match (l.lo, r.lo) {
        (Some(a), Some(b)) => Some(a as i128 + b as i128),
        _ => None,
    };
    let hi = match (l.hi, r.hi) {
        (Some(a), Some(b)) => Some(a as i128 + b as i128),
        _ => None,
    };
    finish_interval(lo, hi)
}

fn sub_iv(l: Interval, r: Interval) -> (Interval, bool) {
    let lo = match (l.lo, r.hi) {
        (Some(a), Some(b)) => Some(a as i128 - b as i128),
        _ => None,
    };
    let hi = match (l.hi, r.lo) {
        (Some(a), Some(b)) => Some(a as i128 - b as i128),
        _ => None,
    };
    finish_interval(lo, hi)
}

/// Scale an interval by a constant, preserving unbounded sides precisely.
fn scale_iv(k: i64, iv: Interval) -> (Interval, bool) {
    if k == 0 {
        return (Interval::singleton(0), false);
    }
    let lo = iv.lo.map(|v| v as i128 * k as i128);
    let hi = iv.hi.map(|v| v as i128 * k as i128);
    let (elo, ehi) = if k > 0 { (lo, hi) } else { (hi, lo) };
    finish_interval(elo, ehi)
}

fn mul_iv(l: Interval, r: Interval) -> (Interval, bool) {
    // `{0}` annihilates, even against unbounded intervals.
    if l.is_singleton() == Some(0) || r.is_singleton() == Some(0) {
        return (Interval::singleton(0), false);
    }
    // Constant scaling keeps unbounded sides precise (covers `x * 2`).
    if let Some(k) = l.is_singleton() {
        return scale_iv(k, r);
    }
    if let Some(k) = r.is_singleton() {
        return scale_iv(k, l);
    }
    match (l.lo, l.hi, r.lo, r.hi) {
        (Some(a), Some(b), Some(c), Some(d)) => {
            let ps = [
                a as i128 * c as i128,
                a as i128 * d as i128,
                b as i128 * c as i128,
                b as i128 * d as i128,
            ];
            let lo = ps.iter().min().copied();
            let hi = ps.iter().max().copied();
            finish_interval(lo, hi)
        }
        // Unbounded non-singleton products: over-approximate soundly.
        _ => (Interval::top(), false),
    }
}

/// Evaluate an integer expression in the abstract state.
/// Returns `(interval, overflow_proven)`; `overflow_proven` is true only when
/// the exact interval is provably outside the `i64` range.
fn eval_int(expr: &Expr, env: &HashMap<String, Interval>) -> (Interval, bool) {
    match expr {
        Expr::IntLit(n, _) => (Interval::singleton(*n), false),
        Expr::Var(name, _) => (
            env.get(name.as_str()).copied().unwrap_or(Interval::top()),
            false,
        ),
        Expr::BinOp {
            op, left, right, ..
        } => {
            let (l, lo) = eval_int(left, env);
            let (r, ro) = eval_int(right, env);
            let overflow = lo || ro;
            match op {
                BinOp::Add => {
                    let (iv, ov) = add_iv(l, r);
                    (iv, overflow || ov)
                }
                BinOp::Sub => {
                    let (iv, ov) = sub_iv(l, r);
                    (iv, overflow || ov)
                }
                BinOp::Mul => {
                    let (iv, ov) = mul_iv(l, r);
                    (iv, overflow || ov)
                }
                BinOp::Div => {
                    if let (Some(a), Some(b)) = (l.is_singleton(), r.is_singleton()) {
                        if b == 0 {
                            return (Interval::top(), overflow);
                        }
                        let (iv, ov) = finish_interval(
                            Some(a as i128 / b as i128),
                            Some(a as i128 / b as i128),
                        );
                        return (iv, overflow || ov);
                    }
                    (Interval::top(), overflow)
                }
                BinOp::Mod => {
                    if let (Some(a), Some(b)) = (l.is_singleton(), r.is_singleton()) {
                        if b == 0 {
                            return (Interval::top(), overflow);
                        }
                        let (iv, ov) = finish_interval(
                            Some(a as i128 % b as i128),
                            Some(a as i128 % b as i128),
                        );
                        return (iv, overflow || ov);
                    }
                    (Interval::top(), overflow)
                }
                _ => (Interval::top(), overflow),
            }
        }
        Expr::UnaryOp { op, expr, .. } => {
            let (inner, overflow) = eval_int(expr, env);
            match op {
                smol_syntax::ast::UnaryOp::Neg => {
                    // Negation flips the interval; `i128` intermediates
                    // cannot overflow on `i64` inputs.
                    let (iv, ov) = finish_interval(
                        inner.hi.map(|h| -(h as i128)),
                        inner.lo.map(|l| -(l as i128)),
                    );
                    (iv, overflow || ov)
                }
                _ => (Interval::top(), overflow),
            }
        }
        _ => (Interval::top(), false),
    }
}

/// Narrow `env` using `cond` assumed `branch_true`.
///
/// The bound may be a constant *or* a non-singleton interval: for
/// `x < y` with `y ∈ [blo, bhi]`, `x < y ≤ bhi` gives `x ≤ bhi - 1`, and
/// dually for the other operators/branches. Sides with an unbounded bound
/// give no information and are skipped. `==` intersects with the bound
/// interval; `!=` (true-branch) stays unrepresentable.
fn narrow(env: &mut HashMap<String, Interval>, cond: &Expr, branch_true: bool) {
    let Expr::BinOp {
        op, left, right, ..
    } = cond
    else {
        return;
    };
    // Find (var, bound) with the var on either side.
    let (var, bound_expr, flipped) = match (left.as_ref(), right.as_ref()) {
        (Expr::Var(v, _), b) => (v.clone(), b.clone(), false),
        (b, Expr::Var(v, _)) => (v.clone(), b.clone(), true),
        _ => return,
    };
    let (bound, _) = eval_int(&bound_expr, env);
    let (blo, bhi) = (bound.lo, bound.hi);
    if blo.is_none() && bhi.is_none() {
        // Fully unknown bound gives no information.
        return;
    }
    let entry = env.entry(var).or_insert(Interval::top());
    // Effective operator from the variable's perspective.
    let eff = match (op, flipped) {
        (BinOp::Lt, false) | (BinOp::Gt, true) => BinOp::Lt,
        (BinOp::LtEq, false) | (BinOp::GtEq, true) => BinOp::LtEq,
        (BinOp::Gt, false) | (BinOp::Lt, true) => BinOp::Gt,
        (BinOp::GtEq, false) | (BinOp::LtEq, true) => BinOp::GtEq,
        (BinOp::Eq, _) => BinOp::Eq,
        (BinOp::NotEq, _) => BinOp::NotEq,
        _ => return,
    };
    match (eff, branch_true) {
        (BinOp::Lt, true) => {
            if let Some(h) = bhi {
                let t = h.saturating_sub(1);
                entry.hi = Some(entry.hi.map_or(t, |x| x.min(t)));
            }
        }
        (BinOp::Lt, false) => {
            if let Some(l) = blo {
                entry.lo = Some(entry.lo.map_or(l, |x| x.max(l)));
            }
        }
        (BinOp::LtEq, true) => {
            if let Some(h) = bhi {
                entry.hi = Some(entry.hi.map_or(h, |x| x.min(h)));
            }
        }
        (BinOp::LtEq, false) => {
            if let Some(l) = blo {
                let t = l.saturating_add(1);
                entry.lo = Some(entry.lo.map_or(t, |x| x.max(t)));
            }
        }
        (BinOp::Gt, true) => {
            if let Some(l) = blo {
                let t = l.saturating_add(1);
                entry.lo = Some(entry.lo.map_or(t, |x| x.max(t)));
            }
        }
        (BinOp::Gt, false) => {
            if let Some(h) = bhi {
                entry.hi = Some(entry.hi.map_or(h, |x| x.min(h)));
            }
        }
        (BinOp::GtEq, true) => {
            if let Some(l) = blo {
                entry.lo = Some(entry.lo.map_or(l, |x| x.max(l)));
            }
        }
        (BinOp::GtEq, false) => {
            if let Some(h) = bhi {
                let t = h.saturating_sub(1);
                entry.hi = Some(entry.hi.map_or(t, |x| x.min(t)));
            }
        }
        (BinOp::Eq, true) | (BinOp::NotEq, false) => {
            if let (Some(l), Some(h)) = (blo, bhi) {
                if l == h {
                    *entry = Interval::singleton(l);
                } else {
                    // `var == bound` with `bound ∈ [l, h]`: intersect.
                    entry.lo = Some(entry.lo.map_or(l, |x| x.max(l)));
                    entry.hi = Some(entry.hi.map_or(h, |x| x.min(h)));
                }
            }
        }
        // `!=` true-branch excludes one value — unrepresentable.
        _ => {}
    }
}

/// Pointwise join of two abstract states.
fn join_env(
    a: &HashMap<String, Interval>,
    b: &HashMap<String, Interval>,
) -> HashMap<String, Interval> {
    let mut out = a.clone();
    for (k, bv) in b {
        out.entry(k.clone())
            .and_modify(|av| *av = av.join(bv))
            .or_insert(*bv);
    }
    out
}

/// Pointwise widening of two successive loop-head states.
fn widen_env(
    prev: &HashMap<String, Interval>,
    next: &HashMap<String, Interval>,
) -> HashMap<String, Interval> {
    let mut out = prev.clone();
    for (k, nv) in next {
        out.entry(k.clone())
            .and_modify(|pv| *pv = pv.widen(nv))
            .or_insert(*nv);
    }
    out
}

/// Drop duplicate diagnostics emitted by the two loop-body iterations.
/// Keeps the first occurrence of each `(code, span, message)` triple at or
/// after `start`, preserving genuinely new widened-state diagnostics.
fn dedup_diags(diags: &mut Vec<Diagnostic>, start: usize) {
    let mut seen: Vec<(u16, Option<Span>, String)> = Vec::new();
    let mut i = start;
    while i < diags.len() {
        let key = (
            diags[i].code as u16,
            diags[i].span,
            diags[i].message.clone(),
        );
        if seen.contains(&key) {
            diags.remove(i);
        } else {
            seen.push(key);
            i += 1;
        }
    }
}

struct Symex<'a> {
    file: &'a str,
    /// Variables declared with type `Int` (the only ones tracked).
    int_vars: HashSet<String>,
    diags: Vec<Diagnostic>,
}

impl Symex<'_> {
    /// Check an expression for definite arithmetic faults.
    fn check_expr(&mut self, expr: &Expr, env: &HashMap<String, Interval>) {
        match expr {
            Expr::BinOp {
                op,
                left,
                right,
                span,
            } => {
                self.check_expr(left, env);
                self.check_expr(right, env);
                match op {
                    BinOp::Div | BinOp::Mod => {
                        // E2022 fires only on the provable singleton `{0}`;
                        // wider intervals (e.g. `[0, 5]`) stay silent.
                        let (divisor, _) = eval_int(right, env);
                        if divisor.is_singleton() == Some(0) {
                            let what = if *op == BinOp::Div {
                                "division"
                            } else {
                                "remainder"
                            };
                            self.diags.push(
                                Diagnostic::error(
                                    DiagnosticCode::DivisionByZero,
                                    format!(
                                        "{what} by zero: the divisor is provably `0` at this point"
                                    ),
                                )
                                .with_file(self.file)
                                .with_span(*span),
                            );
                        }
                    }
                    BinOp::Add | BinOp::Sub | BinOp::Mul => {
                        let (_, overflow) = eval_int(expr, env);
                        if overflow {
                            self.diags.push(
                                Diagnostic::error(
                                    DiagnosticCode::IntegerOverflow,
                                    "integer arithmetic overflows the 64-bit range \
                                     (operands provably exceed `i64`)"
                                        .to_owned(),
                                )
                                .with_file(self.file)
                                .with_span(*span),
                            );
                        }
                    }
                    _ => {}
                }
            }
            Expr::UnaryOp { expr, span, .. } => {
                self.check_expr(expr, env);
                let (_, overflow) = eval_int(
                    &Expr::UnaryOp {
                        op: smol_syntax::ast::UnaryOp::Neg,
                        expr: Box::new((**expr).clone()),
                        span: *span,
                    },
                    env,
                );
                if overflow {
                    self.diags.push(
                        Diagnostic::error(
                            DiagnosticCode::IntegerOverflow,
                            "unary negation overflows the 64-bit range".to_owned(),
                        )
                        .with_file(self.file)
                        .with_span(*span),
                    );
                }
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.check_expr(receiver, env);
                for a in args {
                    self.check_expr(a, env);
                }
            }
            Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => {
                self.check_expr(receiver, env);
            }
            Expr::New { args, .. }
            | Expr::BuiltinCall { args, .. }
            | Expr::Super { args, .. }
            | Expr::Construct { args, .. }
            | Expr::Access { args, .. }
            | Expr::Simulate { args, .. } => {
                for a in args {
                    self.check_expr(a, env);
                }
            }
            Expr::Classify { expr, .. } => self.check_expr(expr, env),
            _ => {}
        }
    }

    fn check_stmts(&mut self, stmts: &[Stmt], env: &mut HashMap<String, Interval>) {
        for stmt in stmts {
            match stmt {
                Stmt::VarDecl { ty, name, init, .. } => {
                    self.check_expr(init, env);
                    if matches!(ty, Type::Int) {
                        self.int_vars.insert(name.clone());
                        let (iv, _) = eval_int(init, env);
                        env.insert(name.clone(), iv);
                    }
                }
                Stmt::Assign { lhs, rhs, .. } => {
                    self.check_expr(rhs, env);
                    if let Expr::Var(name, _) = lhs {
                        if self.int_vars.contains(name) {
                            let (iv, _) = eval_int(rhs, env);
                            env.insert(name.clone(), iv);
                        }
                    } else {
                        self.check_expr(lhs, env);
                    }
                }
                Stmt::Return { value, .. } => {
                    if let Some(v) = value {
                        self.check_expr(v, env);
                    }
                }
                Stmt::If {
                    cond,
                    then_body,
                    else_body,
                    ..
                } => {
                    self.check_expr(cond, env);
                    let mut then_env = env.clone();
                    narrow(&mut then_env, cond, true);
                    self.check_stmts(then_body, &mut then_env);
                    let mut else_env = env.clone();
                    narrow(&mut else_env, cond, false);
                    self.check_stmts(else_body, &mut else_env);
                    // Join.
                    let joined = join_env(&then_env, &else_env);
                    for (k, v) in joined {
                        env.insert(k, v);
                    }
                }
                Stmt::While {
                    cond,
                    decreases,
                    body,
                    span,
                    ..
                } => {
                    self.check_expr(cond, env);
                    if let Some(dec) = decreases {
                        self.check_expr(dec, env);
                    }
                    self.check_termination(cond, decreases.as_deref(), body, *span);
                    let pre_len = self.diags.len();
                    // First iteration: entry -> back edge.
                    let mut body_env = env.clone();
                    narrow(&mut body_env, cond, true);
                    self.check_stmts(body, &mut body_env);
                    let head1 = join_env(env, &body_env);
                    // Second iteration: stabilisation pass for widening.
                    let mut body_env2 = head1.clone();
                    narrow(&mut body_env2, cond, true);
                    self.check_stmts(body, &mut body_env2);
                    let head2 = join_env(&head1, &body_env2);
                    // Widen unstable bounds to +/-inf (N=2).
                    let widened = widen_env(&head1, &head2);
                    self.check_loop_overflow(cond, body, &widened, *span);
                    *env = widened;
                    dedup_diags(&mut self.diags, pre_len);
                }
                Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
                    self.check_expr(value, env);
                }
                Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
                    self.check_expr(expr, env);
                }
                Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
            }
        }
    }

    /// Loop-termination side condition (E5012, warning level).
    ///
    /// With a `decreases expr` clause, the clause is the explicit variant:
    /// the warning is suppressed when the body shows a monotonic update of
    /// a clause variable toward the loop bound (e.g. `decreases 10 - i`
    /// with `i = i + 1`); a clause with no visible monotonic update keeps
    /// the warning. Loops without a clause keep the existing syntactic
    /// check unchanged.
    fn check_termination(
        &mut self,
        cond: &Expr,
        decreases: Option<&Expr>,
        body: &[Stmt],
        span: Span,
    ) {
        match (decreases, loop_need(cond, &self.int_vars)) {
            (Some(dec), LoopNeed::Always) => {
                // Unbounded guard: any monotonic update of a clause
                // variable counts as a variant argument.
                let vars = collect_vars(dec);
                if vars
                    .iter()
                    .any(|v| find_progress(body, v, &ProgressNeed::Either))
                {
                    return;
                }
                self.diags.push(
                    Diagnostic::warning(
                        DiagnosticCode::LoopTerminationUnproven,
                        "`while true` has no termination argument (SMOL has no `break`); \
                         the `decreases` clause shows no monotonic update"
                            .to_owned(),
                    )
                    .with_file(self.file)
                    .with_span(span),
                );
            }
            (
                Some(dec),
                LoopNeed::Bounded {
                    var,
                    need,
                    bound_vars,
                },
            ) => {
                let vars = collect_vars(dec);
                if vars.contains(&var) && find_progress(body, &var, &need) {
                    return;
                }
                // A clause variable that is a bound variable progressing in
                // the opposite direction also establishes the variant
                // (e.g. `decreases n` with `while i < n do n = n - 1`).
                for bv in &bound_vars {
                    if vars.contains(bv) && find_progress(body, bv, &opposite_need(&need)) {
                        return;
                    }
                }
                self.diags.push(
                    Diagnostic::warning(
                        DiagnosticCode::LoopTerminationUnproven,
                        format!(
                            "`while` loop over `{var}` shows no monotonic update toward its bound; \
                             the `decreases` clause does not establish a variant"
                        ),
                    )
                    .with_file(self.file)
                    .with_span(span),
                );
            }
            (Some(_), LoopNeed::Skipped) => {
                // Guards the syntactic check skips (list loops, non-Int or
                // non-constant bounds) stay silent, as without a clause.
            }
            (None, LoopNeed::Always) => {
                // `while true` never terminates (SMOL has no `break`).
                self.diags.push(
                    Diagnostic::warning(
                        DiagnosticCode::LoopTerminationUnproven,
                        "`while true` has no termination argument (SMOL has no `break`); \
                         restructure with a bounded condition and a decreasing variant"
                            .to_owned(),
                    )
                    .with_file(self.file)
                    .with_span(span),
                );
            }
            (
                None,
                LoopNeed::Bounded {
                    var,
                    need,
                    bound_vars,
                },
            ) => {
                if find_progress(body, &var, &need) {
                    return;
                }
                // A variable bound advances toward the compared variable in
                // the opposite direction (`while i < n` terminates if `n`
                // decreases even when `i` stays put).
                for bv in &bound_vars {
                    if find_progress(body, bv, &opposite_need(&need)) {
                        return;
                    }
                }
                self.diags.push(
                    Diagnostic::warning(
                        DiagnosticCode::LoopTerminationUnproven,
                        format!(
                            "`while` loop over `{var}` shows no monotonic update toward its bound; \
                             termination is unproven (provide a decreasing variant, e.g. \
                             `{var} = {var} + 1` for an upper bound)"
                        ),
                    )
                    .with_file(self.file)
                    .with_span(span),
                );
            }
            (None, LoopNeed::Skipped) => {}
        }
    }

    /// Loop-level overflow after widening (E2021, error level).
    ///
    /// Flags loops that repeatedly multiply an integer variable away from
    /// zero with no bound from the guard (e.g. `while x > 0 do x = x * 2`),
    /// once widening proves the value grows without bound: doubling a
    /// positive (or negative) value forever must eventually leave `i64`.
    /// Additive updates, bounded guards, and values that may be zero stay
    /// silent.
    fn check_loop_overflow(
        &mut self,
        cond: &Expr,
        body: &[Stmt],
        widened: &HashMap<String, Interval>,
        span: Span,
    ) {
        for (var, iv) in widened {
            if !self.int_vars.contains(var) {
                continue;
            }
            if !find_mul_growth(body, var) {
                continue;
            }
            let grows_up = iv.hi.is_none()
                && matches!(iv.lo, Some(lo) if lo > 0)
                && !upper_bounded_by_cond(cond, var);
            let grows_down = iv.lo.is_none()
                && matches!(iv.hi, Some(hi) if hi < 0)
                && !lower_bounded_by_cond(cond, var);
            if grows_up || grows_down {
                self.diags.push(
                    Diagnostic::error(
                        DiagnosticCode::IntegerOverflow,
                        format!(
                            "`while` loop multiplies `{var}` without a bound; \
                             interval widening proves the value grows without bound \
                             and must eventually exceed `i64`"
                        ),
                    )
                    .with_file(self.file)
                    .with_span(span),
                );
            }
        }
    }
}

/// What a loop guard demands of its variable for termination.
enum LoopNeed {
    /// `while true`: no bound at all.
    Always,
    /// Integer comparison of a tracked Int variable against an integer bound
    /// (constant or an expression over tracked Int vars); `bound_vars` are
    /// the variable names appearing in the bound expression.
    Bounded {
        var: String,
        need: ProgressNeed,
        bound_vars: HashSet<String>,
    },
    /// Guards the syntactic check skips (lists, non-Int, non-integer bound).
    Skipped,
}

/// Classify a loop guard exactly as the syntactic termination check does:
/// `while true` is `Always`; `x <rel> bound` over an `Int` variable with an
/// integer bound (constant or an expression over tracked Int vars) is
/// `Bounded`; anything else is `Skipped`.
fn loop_need(cond: &Expr, int_vars: &HashSet<String>) -> LoopNeed {
    if matches!(cond, Expr::BoolLit(true, _)) {
        return LoopNeed::Always;
    }
    let Expr::BinOp {
        op, left, right, ..
    } = cond
    else {
        return LoopNeed::Skipped;
    };
    // Expect `x <rel> bound` with an Int variable and an integer expression
    // (constant or over tracked Int vars) on the other side.
    let (var, bound_expr, flipped) = match (left.as_ref(), right.as_ref()) {
        (Expr::Var(v, _), b) => (v.clone(), b.clone(), false),
        (b, Expr::Var(v, _)) => (v.clone(), b.clone(), true),
        _ => return LoopNeed::Skipped,
    };
    if !int_vars.contains(&var) {
        return LoopNeed::Skipped;
    }
    if !is_int_expr(&bound_expr, int_vars) {
        return LoopNeed::Skipped;
    }
    let mut bound_vars = collect_vars(&bound_expr);
    bound_vars.remove(&var);
    // Direction the variable must move for the bound to be approached:
    // upper-bounded (`x < b`) needs increases; lower-bounded needs decreases.
    let need = match (op, flipped) {
        (BinOp::Lt, false) | (BinOp::LtEq, false) | (BinOp::Gt, true) | (BinOp::GtEq, true) => {
            ProgressNeed::Increase
        }
        (BinOp::Gt, false) | (BinOp::GtEq, false) | (BinOp::Lt, true) | (BinOp::LtEq, true) => {
            ProgressNeed::Decrease
        }
        (BinOp::NotEq, _) => ProgressNeed::Either,
        _ => return LoopNeed::Skipped,
    };
    LoopNeed::Bounded {
        var,
        need,
        bound_vars,
    }
}

/// True if `expr` is an integer expression: an integer literal, a variable
/// tracked as `Int`, an arithmetic `BinOp` (Add/Sub/Mul/Div/Mod) over integer
/// expressions, or a unary negation over an integer expression.
fn is_int_expr(expr: &Expr, int_vars: &HashSet<String>) -> bool {
    match expr {
        Expr::IntLit(..) => true,
        Expr::Var(name, _) => int_vars.contains(name.as_str()),
        Expr::BinOp {
            op, left, right, ..
        } => match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                is_int_expr(left, int_vars) && is_int_expr(right, int_vars)
            }
            _ => false,
        },
        Expr::UnaryOp {
            op: smol_syntax::ast::UnaryOp::Neg,
            expr,
            ..
        } => is_int_expr(expr, int_vars),
        _ => false,
    }
}

/// Collect all variable names occurring in an expression (used for
/// `decreases` clauses).
fn collect_vars(expr: &Expr) -> HashSet<String> {
    let mut out = HashSet::new();
    collect_vars_into(expr, &mut out);
    out
}

fn collect_vars_into(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Var(n, _) => {
            out.insert(n.clone());
        }
        Expr::BinOp { left, right, .. } => {
            collect_vars_into(left, out);
            collect_vars_into(right, out);
        }
        Expr::UnaryOp { expr, .. } => collect_vars_into(expr, out),
        Expr::MethodCall { receiver, args, .. } => {
            collect_vars_into(receiver, out);
            for a in args {
                collect_vars_into(a, out);
            }
        }
        Expr::Field { receiver, .. } | Expr::Port { receiver, .. } => {
            collect_vars_into(receiver, out);
        }
        Expr::New { args, .. }
        | Expr::BuiltinCall { args, .. }
        | Expr::Super { args, .. }
        | Expr::Construct { args, .. }
        | Expr::Access { args, .. }
        | Expr::Simulate { args, .. } => {
            for a in args {
                collect_vars_into(a, out);
            }
        }
        Expr::Classify { expr, .. } => collect_vars_into(expr, out),
        _ => {}
    }
}

/// True if `stmts` contain an assignment `var = var ± k` (constant step)
/// moving `var` in the required direction. Nested `if`s are searched;
/// nested `while`s are not (their effects don't run once per outer iteration).
fn find_progress(stmts: &[Stmt], var: &str, need: &ProgressNeed) -> bool {
    stmts.iter().any(|s| stmt_progress(s, var, need))
}

enum ProgressNeed {
    Increase,
    Decrease,
    Either,
}

/// The direction a *bound* variable must move for the compared variable to
/// reach it: the mirror of the compared variable's required direction.
fn opposite_need(need: &ProgressNeed) -> ProgressNeed {
    match need {
        ProgressNeed::Increase => ProgressNeed::Decrease,
        ProgressNeed::Decrease => ProgressNeed::Increase,
        ProgressNeed::Either => ProgressNeed::Either,
    }
}

fn stmt_progress(stmt: &Stmt, var: &str, need: &ProgressNeed) -> bool {
    match stmt {
        Stmt::Assign { lhs, rhs, .. } => {
            if !matches!(lhs, Expr::Var(n, _) if n == var) {
                return false;
            }
            let Expr::BinOp {
                op, left, right, ..
            } = rhs
            else {
                return false;
            };
            let step: Option<i64> = match (left.as_ref(), right.as_ref()) {
                (Expr::Var(n, _), Expr::IntLit(k, _)) if n == var => match op {
                    BinOp::Add => Some(*k),
                    BinOp::Sub => Some(-k),
                    _ => None,
                },
                (Expr::IntLit(k, _), Expr::Var(n, _)) if n == var => match op {
                    BinOp::Add => Some(*k),
                    _ => None,
                },
                _ => None,
            };
            match (step, need) {
                (Some(k), ProgressNeed::Increase) => k > 0,
                (Some(k), ProgressNeed::Decrease) => k < 0,
                (Some(k), ProgressNeed::Either) => k != 0,
                _ => false,
            }
        }
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            // Progress on either path is evidence (may-analysis; documented).
            find_progress(then_body, var, need) || find_progress(else_body, var, need)
        }
        _ => false,
    }
}

/// True if `stmts` multiply `var` away from zero (`var = var * k` with
/// `|k| >= 2`, or `var = var * var`). Nested `if`s are searched; nested
/// `while`s are not.
fn find_mul_growth(stmts: &[Stmt], var: &str) -> bool {
    stmts.iter().any(|s| stmt_mul_growth(s, var))
}

fn stmt_mul_growth(stmt: &Stmt, var: &str) -> bool {
    match stmt {
        Stmt::Assign { lhs, rhs, .. } => {
            if !matches!(lhs, Expr::Var(n, _) if n == var) {
                return false;
            }
            let Expr::BinOp {
                op, left, right, ..
            } = rhs
            else {
                return false;
            };
            if *op != BinOp::Mul {
                return false;
            }
            match (left.as_ref(), right.as_ref()) {
                (Expr::Var(n, _), Expr::IntLit(k, _)) if n == var => *k >= 2 || *k <= -2,
                (Expr::IntLit(k, _), Expr::Var(n, _)) if n == var => *k >= 2 || *k <= -2,
                (Expr::Var(a, _), Expr::Var(b, _)) => a == var && b == var,
                _ => false,
            }
        }
        Stmt::If {
            then_body,
            else_body,
            ..
        } => find_mul_growth(then_body, var) || find_mul_growth(else_body, var),
        _ => false,
    }
}

/// Effective comparison of `var` against the other side of `cond`, from the
/// variable's perspective (`None` if `var` is not compared).
fn cond_comparison_var(cond: &Expr, var: &str) -> Option<BinOp> {
    let Expr::BinOp {
        op, left, right, ..
    } = cond
    else {
        return None;
    };
    let flipped = match (left.as_ref(), right.as_ref()) {
        (Expr::Var(v, _), _) if v == var => false,
        (_, Expr::Var(v, _)) if v == var => true,
        _ => return None,
    };
    match (op, flipped) {
        (BinOp::Lt, false) | (BinOp::Gt, true) => Some(BinOp::Lt),
        (BinOp::LtEq, false) | (BinOp::GtEq, true) => Some(BinOp::LtEq),
        (BinOp::Gt, false) | (BinOp::Lt, true) => Some(BinOp::Gt),
        (BinOp::GtEq, false) | (BinOp::LtEq, true) => Some(BinOp::GtEq),
        _ => None,
    }
}

/// True if the guard keeps `var` below a bound while the loop runs
/// (e.g. `x < b`); conservative: any `<`/`<=` comparison on `var` counts.
fn upper_bounded_by_cond(cond: &Expr, var: &str) -> bool {
    matches!(
        cond_comparison_var(cond, var),
        Some(BinOp::Lt | BinOp::LtEq)
    )
}

/// True if the guard keeps `var` above a bound while the loop runs
/// (e.g. `x > b`); conservative: any `>`/`>=` comparison on `var` counts.
fn lower_bounded_by_cond(cond: &Expr, var: &str) -> bool {
    matches!(
        cond_comparison_var(cond, var),
        Some(BinOp::Gt | BinOp::GtEq)
    )
}

/// Run the symbolic-execution pass over all method bodies and `main`.
#[must_use]
pub fn check_symex(files: &[File], _env: &ClassEnv) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                let mut sx = Symex {
                    file: &file.path,
                    int_vars: HashSet::new(),
                    diags: Vec::new(),
                };
                let mut env = HashMap::new();
                sx.check_stmts(&method.body, &mut env);
                diags.extend(sx.diags);
            }
        }
        if let Some(main_stmts) = &file.main {
            let mut sx = Symex {
                file: &file.path,
                int_vars: HashSet::new(),
                diags: Vec::new(),
            };
            let mut env = HashMap::new();
            sx.check_stmts(main_stmts, &mut env);
            diags.extend(sx.diags);
        }
    }
    diags
}

//! Initial-state concretisation.
//!
//! Symbolically simulates the `main` block(s) to produce a set of concrete
//! initial states (entity multisets), then checks that at least one concrete
//! state maps to the abstract model's initial states (by class / state-class
//! name match against the classification roots and `classifies` states).
//!
//! Abstraction rules:
//! - `if`/`else`: union of the branch states (state-set, capped at [`MAX_STATES`]).
//! - `while`: single unroll (zero-or-one iteration abstraction).
//! - Empty / missing `main`: empty state-set (silent — nothing to concretise).

use std::collections::{HashMap, HashSet};

use smol_syntax::ast::{Expr, File, Stmt};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};
use smol_syntax::span::Span;

use crate::model::{ClassMap, Extracted, TransitionSystem};

/// Cap on the concrete state-set size (branch-union widening).
pub const MAX_STATES: usize = 8;

/// A single concrete initial state: entity multiset plus linked `classifies` IRIs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConcreteState {
    /// Class name -> instance count (from `new ClassName(...)` in `main`).
    pub entities: HashMap<String, usize>,
    /// `classifies` IRIs attached to the created entities (when present).
    pub iris: HashSet<String>,
}

impl ConcreteState {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    fn add_entity(&mut self, map: &ClassMap, class: &str) {
        *self.entities.entry(class.to_owned()).or_insert(0) += 1;
        if let Some(iri) = map.classifies_iri(class) {
            self.iris.insert(iri);
        }
    }
}

/// Symbolically simulate the `main` blocks of `files`, returning the set of
/// concrete initial states. Returns an empty vec when there is no `main` or
/// it contains no statements.
#[must_use]
pub fn concrete_initial_states(files: &[File], map: &ClassMap) -> Vec<ConcreteState> {
    let mut main_stmts: Vec<Stmt> = Vec::new();
    for f in files {
        if let Some(m) = &f.main {
            main_stmts.extend(m.iter().cloned());
        }
    }
    if main_stmts.is_empty() {
        return Vec::new();
    }
    let mut states = vec![ConcreteState::default()];
    simulate_block(&main_stmts, map, &mut states);
    states
}

fn simulate_block(stmts: &[Stmt], map: &ClassMap, states: &mut Vec<ConcreteState>) {
    for stmt in stmts {
        simulate_stmt(stmt, map, states);
    }
}

fn simulate_stmt(stmt: &Stmt, map: &ClassMap, states: &mut Vec<ConcreteState>) {
    match stmt {
        Stmt::If {
            cond,
            then_body,
            else_body,
            ..
        } => {
            // Union of branch states: simulate each arm from a clone of every
            // incoming state, then merge (capped).
            let mut out = Vec::new();
            for base in states.iter() {
                // Condition may itself construct objects (rare); collect those.
                let mut cond_state = base.clone();
                collect_news_in_expr(cond, map, &mut cond_state);

                let mut then_states = vec![cond_state.clone()];
                simulate_block(then_body, map, &mut then_states);

                let mut else_states = if else_body.is_empty() {
                    vec![cond_state]
                } else {
                    let mut es = vec![cond_state];
                    simulate_block(else_body, map, &mut es);
                    es
                };

                out.append(&mut then_states);
                out.append(&mut else_states);
            }
            *states = dedup_capped(out);
        }
        Stmt::While { cond, body, .. } => {
            // Zero-or-one iteration abstraction: {zero} ∪ {one unroll}.
            let mut out = Vec::new();
            for base in states.iter() {
                let mut zero = base.clone();
                collect_news_in_expr(cond, map, &mut zero);
                out.push(zero.clone());

                let mut one = vec![zero];
                simulate_block(body, map, &mut one);
                out.append(&mut one);
            }
            *states = dedup_capped(out);
        }
        Stmt::VarDecl { init, .. } => {
            for s in states.iter_mut() {
                collect_news_in_expr(init, map, s);
            }
        }
        Stmt::Assign { lhs, rhs, .. } => {
            for s in states.iter_mut() {
                collect_news_in_expr(lhs, map, s);
                collect_news_in_expr(rhs, map, s);
            }
        }
        Stmt::Expr { expr, .. } | Stmt::Adapt { expr, .. } => {
            for s in states.iter_mut() {
                collect_news_in_expr(expr, map, s);
            }
        }
        Stmt::Print { value, .. } | Stmt::Destroy { value, .. } => {
            for s in states.iter_mut() {
                collect_news_in_expr(value, map, s);
            }
        }
        Stmt::Return { value, .. } => {
            if let Some(v) = value {
                for s in states.iter_mut() {
                    collect_news_in_expr(v, map, s);
                }
            }
        }
        Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
    }
}

/// Collect every `new ClassName(...)` in `expr` (recursively) into `state`.
fn collect_news_in_expr(expr: &Expr, map: &ClassMap, state: &mut ConcreteState) {
    match expr {
        Expr::New { class, args, .. } => {
            // `new List<T>(...)` / `new ExplList<T>(...)` are transient query
            // containers, not domain entities — skip them so a list-heavy
            // `main` is not mistaken for an entity construction.
            if class != "List" && class != "ExplList" {
                state.add_entity(map, class);
            }
            for a in args {
                collect_news_in_expr(a, map, state);
            }
        }
        Expr::Field { receiver, .. } => collect_news_in_expr(receiver, map, state),
        Expr::MethodCall { receiver, args, .. } => {
            collect_news_in_expr(receiver, map, state);
            for a in args {
                collect_news_in_expr(a, map, state);
            }
        }
        Expr::Classify { expr: inner, .. } => collect_news_in_expr(inner, map, state),
        Expr::BinOp { left, right, .. } => {
            collect_news_in_expr(left, map, state);
            collect_news_in_expr(right, map, state);
        }
        Expr::UnaryOp { expr: inner, .. } => collect_news_in_expr(inner, map, state),
        Expr::BuiltinCall { args, .. }
        | Expr::Construct { args, .. }
        | Expr::Access { args, .. }
        | Expr::Simulate { args, .. }
        | Expr::Super { args, .. } => {
            for a in args {
                collect_news_in_expr(a, map, state);
            }
        }
        Expr::Port { receiver, .. } => collect_news_in_expr(receiver, map, state),
        Expr::IntLit(_, _)
        | Expr::FloatLit(_, _)
        | Expr::StrLit(_, _)
        | Expr::BoolLit(_, _)
        | Expr::Null(_)
        | Expr::Var(_, _)
        | Expr::Member { .. }
        | Expr::Validate { .. } => {}
    }
}

fn dedup_capped(mut states: Vec<ConcreteState>) -> Vec<ConcreteState> {
    let mut out: Vec<ConcreteState> = Vec::new();
    for s in states.drain(..) {
        if !out.contains(&s) {
            out.push(s);
        }
        if out.len() >= MAX_STATES {
            break;
        }
    }
    out
}

/// True when `state` maps to some abstract initial state of the Kripke model.
///
/// The abstract initial states (see [`crate::kripke::to_kripke`] over the
/// [`TransitionSystem`] built by [`crate::model::build_transition_system`])
/// are the per-root *unclassified* states; a concrete state maps to them when
/// it instantiates the classified entity: some created class equals — or is a
/// subtype of — a classification root entity or a `classifies` state class,
/// or one of its linked IRIs matches a known state IRI.
#[must_use]
pub fn maps_to_initial(
    state: &ConcreteState,
    map: &ClassMap,
    extracted: &Extracted,
    _ts: &TransitionSystem,
) -> bool {
    if state.is_empty() {
        return false;
    }
    let state_iris: HashSet<&str> = extracted.states.iter().map(|s| s.iri.as_str()).collect();
    for class in state.entities.keys() {
        // Direct name match against roots and state classes.
        if extracted.roots.iter().any(|r| r.entity == *class) {
            return true;
        }
        if extracted.state_roots.contains_key(class) {
            return true;
        }
        // Subtype match: e.g. concrete `SpecialPlant extends Plant`.
        for root in &extracted.roots {
            if map.is_subtype(class, &root.entity) {
                return true;
            }
        }
        for state_name in extracted.state_roots.keys() {
            if map.is_subtype(class, state_name) {
                return true;
            }
        }
        // IRI match via the entity's own `classifies` annotation.
        if let Some(iri) = map.classifies_iri(class)
            && state_iris.contains(iri.as_str())
        {
            return true;
        }
    }
    // Linked IRIs collected during simulation match a known state IRI.
    if state.iris.iter().any(|i| state_iris.contains(i.as_str())) {
        return true;
    }
    false
}

fn first_main_span(files: &[File]) -> Option<(String, Span)> {
    for f in files {
        if let Some(stmts) = &f.main {
            if let Some(first) = stmts.first() {
                return Some((f.path.clone(), stmt_span(first)));
            }
            // `main` present but empty: still attribute to the file without span.
            return Some((f.path.clone(), Span::default()));
        }
    }
    None
}

fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::VarDecl { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::Expr { span, .. }
        | Stmt::Return { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. }
        | Stmt::Print { span, .. }
        | Stmt::Destroy { span, .. }
        | Stmt::Skip { span }
        | Stmt::Adapt { span, .. }
        | Stmt::Breakpoint { span } => *span,
    }
}

/// Concretisation check over the `main` block(s).
///
/// Maps each simulated concrete state to the abstract Kripke initial states;
/// when no abstract model exists (no classification roots) or no `main`
/// exists (or it is empty), the check is silent. Otherwise, if *no* concrete
/// state maps to any abstract initial state, a single diagnostic is emitted.
///
/// NOTE: reports `DiagnosticCode::InitialConcretisationFailure` (E5013).
#[must_use]
pub fn check_initial_concretisation(
    files: &[File],
    map: &ClassMap,
    extracted: &Extracted,
    ts: &TransitionSystem,
) -> Vec<Diagnostic> {
    if extracted.roots.is_empty() {
        return Vec::new();
    }
    let states = concrete_initial_states(files, map);
    if states.is_empty() {
        return Vec::new();
    }
    // A purely empty concrete state (no `new` at all) carries no entity to
    // map: stay silent rather than failing closed on trivial mains.
    let non_empty: Vec<&ConcreteState> = states.iter().filter(|s| !s.is_empty()).collect();
    if non_empty.is_empty() {
        return Vec::new();
    }
    if non_empty
        .iter()
        .any(|s| maps_to_initial(s, map, extracted, ts))
    {
        return Vec::new();
    }

    // No concrete state maps to any abstract initial state.
    let mut diag = Diagnostic::error(
        DiagnosticCode::InitialConcretisationFailure,
        format!(
            "InitialConcretisationFailure: `main` constructs no entity that maps to any abstract \
             initial state (concrete entities: {})",
            describe_concrete(&non_empty),
        ),
    );
    if let Some((file, span)) = first_main_span(files) {
        diag = diag.with_file(file).with_span(span);
    }
    vec![diag]
}

fn describe_concrete(states: &[&ConcreteState]) -> String {
    let mut parts = Vec::new();
    for s in states.iter().take(4) {
        let mut ents: Vec<String> = s.entities.iter().map(|(k, v)| format!("{k}×{v}")).collect();
        ents.sort();
        parts.push(format!("{{{}}}", ents.join(", ")));
    }
    if states.len() > 4 {
        parts.push(format!("… +{} more", states.len() - 4));
    }
    parts.join(" | ")
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    fn parse(src: &str) -> File {
        let (file, diags) = smol_parser::parse("test.smol", src);
        assert!(diags.is_empty(), "parse errors: {diags:?}");
        file
    }

    const MODEL: &str = r#"
class Twin () end
class Plant extends Twin () end
class HealthState extends Twin (context Plant plant) end
class Healthy extends HealthState () classifies "<ast:Healthy>"; end
class Unhealthy extends HealthState () classifies "<ast:Unhealthy>"; end
class Model () end
"#;

    #[test]
    fn empty_main_is_silent() {
        let file = parse(&format!("{MODEL}\nmain\n print(\"x\");\nend\n"));
        let files = [file];
        let map = crate::model::build_class_map(&files);
        let extracted = crate::model::extract(&map);
        let roots: Vec<crate::model::Root> = extracted
            .roots
            .iter()
            .map(|r| crate::model::Root {
                entity: r.entity.clone(),
                states: r.states.clone(),
                adapted: false,
            })
            .collect();
        let ts = crate::model::build_transition_system(&roots);
        assert!(check_initial_concretisation(&files, &map, &extracted, &ts).is_empty());
    }
}

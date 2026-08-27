//! Whole-program control-flow-graph (CFG) extraction.
//!
//! This module lifts the classification-hierarchy Kripke structure into a
//! *whole-program* CFG-based Kripke structure: states become program points
//! (CFG nodes), each labelled with the *set* of atomic propositions that hold
//! there — classification state-class names, structural markers
//! (`in_main`, `at_program_end`, `entry_C_M`, `exit_C_M`,
//! `after_adapt_ROOT`, `after_classify_ROOT`) and guard-derived conditions
//! (`guard_FILE_OFFSET`).
//!
//! Entity classification states are threaded forward through the CFG by an
//! abstract interpretation: each node carries a `HashMap<Root, HashSet<StateName>>`
//! describing, for every classification root, the set of states the root may
//! be in at that program point.  `adapt`/`classify` widen the root to all of
//! its possible states; `if`/`while` merge points join the incoming sets.
//!
//! Method calls are inlined up to a configurable depth and summarized
//! (conservatively widened) beyond it.

use std::collections::{HashMap, HashSet};

use smol_syntax::ast::{Expr, MethodDecl, Stmt};

use crate::model::{ClassMap, Extracted, ResolvedType, Resolver};

/// The set of possible states for each classification root at a program point.
///
/// Each root maps to a set of state-class names; the empty string `""` is the
/// reserved *unclassified* marker with which every root starts.
pub type EntityStates = HashMap<String, HashSet<String>>;

/// A single CFG node: a program point carrying a set of AP labels.
#[derive(Clone, Debug)]
pub struct CfgNode {
    /// Stable, dense index (also the position in [`Cfg::nodes`]).
    pub id: usize,
    /// Atomic-proposition labels satisfied at this node.
    pub labels: HashSet<String>,
    /// Successor node ids.
    pub succs: Vec<usize>,
    /// Entity-state abstract value at this program point.
    pub entity_states: EntityStates,
}

/// A whole-program control-flow graph.
#[derive(Clone, Debug)]
pub struct Cfg {
    pub nodes: Vec<CfgNode>,
    /// The initial (entry) node id.
    pub entry: usize,
    /// Terminal node ids (no successors yet; the program ends here).
    pub exits: Vec<usize>,
}

/// The structural / guard AP label prefixes.  A CTL property that references
/// any of these must be checked against the whole-program `Ts`.
const STRUCTURAL_PREFIXES: &[&str] = &[
    "in_main",
    "at_program_end",
    "entry_",
    "exit_",
    "after_adapt_",
    "after_classify_",
    "guard_",
];

/// True if `label` is a structural or guard AP (as opposed to a plain
/// classification state-class name).
#[must_use]
pub fn is_structural_label(label: &str) -> bool {
    STRUCTURAL_PREFIXES.iter().any(|p| label.starts_with(p))
}

/// Replace every character that is not `[A-Za-z0-9_]` with `_`, so that a
/// source file name / offset can be embedded in an AP label that the CTL
/// lexer accepts (identifiers are `[A-Za-z0-9_]+`).
#[must_use]
pub fn sanitize_ident(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The state-class APs implied by an entity-state abstract value.
#[must_use]
pub fn state_labels(states: &EntityStates) -> HashSet<String> {
    let mut out = HashSet::new();
    for set in states.values() {
        for s in set {
            if !s.is_empty() {
                out.insert(s.clone());
            }
        }
    }
    out
}

/// Initial entity-state value: every root is *unclassified* (`{""}`).
#[must_use]
pub fn init_entity_states(extracted: &Extracted) -> EntityStates {
    let mut states = HashMap::new();
    for root in &extracted.roots {
        states.insert(root.entity.clone(), HashSet::from(["".to_owned()]));
    }
    states
}

/// The pointwise union of two entity-state values.
#[must_use]
pub fn union_states(a: &EntityStates, b: &EntityStates) -> EntityStates {
    let mut out = a.clone();
    for (k, v) in b {
        out.entry(k.clone()).or_default().extend(v.iter().cloned());
    }
    out
}

/// A partially-built statement block: the set of node ids whose predecessors
/// (respectively successors) are not yet connected, plus the entity-state
/// value flowing out of the block.
#[derive(Clone)]
struct Block {
    entries: Vec<usize>,
    exits: Vec<usize>,
    out_states: EntityStates,
}

struct CfgBuilder<'a> {
    nodes: Vec<CfgNode>,
    map: &'a ClassMap,
    extracted: &'a Extracted,
}

impl<'a> CfgBuilder<'a> {
    fn add_node(&mut self, labels: HashSet<String>, entity_states: EntityStates) -> usize {
        let id = self.nodes.len();
        self.nodes.push(CfgNode {
            id,
            labels,
            succs: Vec::new(),
            entity_states,
        });
        id
    }

    fn connect(&mut self, from: &[usize], to: &[usize]) {
        for &f in from {
            for &t in to {
                if !self.nodes[f].succs.contains(&t) {
                    self.nodes[f].succs.push(t);
                }
            }
        }
    }

    /// Build a block for a straight-line statement sequence.
    #[allow(clippy::too_many_arguments)]
    fn build_stmts(
        &mut self,
        stmts: &[Stmt],
        resolver: &mut Resolver<'a>,
        file: &str,
        this: &str,
        in_main: bool,
        depth: usize,
        depth_limit: usize,
        entry_states: EntityStates,
    ) -> Block {
        let mut first_entries: Option<Vec<usize>> = None;
        let mut prev_exits: Option<Vec<usize>> = None;
        let mut last_exits: Vec<usize> = Vec::new();
        let mut states = entry_states;

        for stmt in stmts {
            let b = self.build_stmt(
                stmt,
                resolver,
                file,
                this,
                in_main,
                depth,
                depth_limit,
                states.clone(),
            );
            if first_entries.is_none() {
                first_entries = Some(b.entries.clone());
            }
            if let Some(prev) = &prev_exits {
                self.connect(prev, &b.entries);
            }
            prev_exits = Some(b.exits.clone());
            last_exits = b.exits.clone();
            states = b.out_states;
        }

        Block {
            entries: first_entries.unwrap_or_default(),
            exits: last_exits,
            out_states: states,
        }
    }

    /// Build the block for a single statement.
    #[allow(clippy::too_many_arguments)]
    fn build_stmt(
        &mut self,
        stmt: &Stmt,
        resolver: &mut Resolver<'a>,
        file: &str,
        this: &str,
        in_main: bool,
        depth: usize,
        depth_limit: usize,
        entry_states: EntityStates,
    ) -> Block {
        match stmt {
            Stmt::VarDecl { ty, name, init, .. } => {
                resolver
                    .local_vars
                    .insert(name.clone(), crate::model::resolve_type(ty));
                if let Expr::Classify { expr: inner, .. } = init {
                    self.classify_node(resolver, inner, in_main, entry_states)
                } else {
                    self.simple_node(entry_states, HashSet::new(), in_main)
                }
            }
            Stmt::Assign { rhs, .. } => {
                if let Expr::Classify { expr: inner, .. } = rhs {
                    self.classify_node(resolver, inner, in_main, entry_states)
                } else {
                    self.simple_node(entry_states, HashSet::new(), in_main)
                }
            }
            Stmt::Expr { expr, .. } => {
                if let Expr::MethodCall {
                    receiver, method, ..
                } = expr
                    && let Some(blk) = self.try_inline_call(
                        receiver,
                        method,
                        resolver,
                        file,
                        depth,
                        depth_limit,
                        &entry_states,
                    )
                {
                    return blk;
                }
                if let Expr::Classify { expr: inner, .. } = expr {
                    self.classify_node(resolver, inner, in_main, entry_states)
                } else {
                    self.simple_node(entry_states, HashSet::new(), in_main)
                }
            }
            Stmt::Adapt { expr, .. } => {
                let root = self.root_of_expr(resolver, expr);
                let mut states = entry_states.clone();
                let mut extra = HashSet::new();
                if let Some(r) = root {
                    self.widen_root(&mut states, &r);
                    extra.insert(format!("after_adapt_{}", sanitize_ident(&r)));
                }
                self.simple_node(states, extra, in_main)
            }
            Stmt::Print { .. }
            | Stmt::Destroy { .. }
            | Stmt::Skip { .. }
            | Stmt::Breakpoint { .. }
            | Stmt::Return { .. } => self.simple_node(entry_states, HashSet::new(), in_main),
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => self.build_if(
                cond,
                then_body,
                else_body,
                resolver,
                file,
                this,
                in_main,
                depth,
                depth_limit,
                entry_states,
            ),
            Stmt::While { cond, body, .. } => self.build_while(
                cond,
                body,
                resolver,
                file,
                this,
                in_main,
                depth,
                depth_limit,
                entry_states,
            ),
        }
    }

    /// A single-node block (used by most simple statements).
    fn simple_node(
        &mut self,
        states: EntityStates,
        extra: HashSet<String>,
        in_main: bool,
    ) -> Block {
        let mut labels = state_labels(&states);
        labels.extend(extra);
        if in_main {
            labels.insert("in_main".to_owned());
        }
        let id = self.add_node(labels, states.clone());
        Block {
            entries: vec![id],
            exits: vec![id],
            out_states: states,
        }
    }

    /// A node that widens a root after `classify(x)`, labelled `after_classify_ROOT`.
    fn classify_node(
        &mut self,
        resolver: &Resolver<'a>,
        inner: &Expr,
        in_main: bool,
        entry_states: EntityStates,
    ) -> Block {
        let root = self.root_of_expr(resolver, inner);
        let mut states = entry_states;
        let mut extra = HashSet::new();
        if let Some(r) = root {
            self.widen_root(&mut states, &r);
            extra.insert(format!("after_classify_{}", sanitize_ident(&r)));
        }
        self.simple_node(states, extra, in_main)
    }

    /// Build the CFG for an `if` statement: branch node → then/else sub-CFGs
    /// → merge node (entity states joined at the merge).
    #[allow(clippy::too_many_arguments)]
    fn build_if(
        &mut self,
        cond: &Expr,
        then_body: &[Stmt],
        else_body: &[Stmt],
        resolver: &mut Resolver<'a>,
        file: &str,
        this: &str,
        in_main: bool,
        depth: usize,
        depth_limit: usize,
        entry_states: EntityStates,
    ) -> Block {
        let guard = format!("guard_{}_{}", sanitize_ident(file), cond.span().start);

        let mut branch_labels = state_labels(&entry_states);
        branch_labels.insert(guard);
        if in_main {
            branch_labels.insert("in_main".to_owned());
        }
        let branch = self.add_node(branch_labels, entry_states.clone());

        let then_b = self.build_stmts(
            then_body,
            resolver,
            file,
            this,
            in_main,
            depth,
            depth_limit,
            entry_states.clone(),
        );
        let else_b = self.build_stmts(
            else_body,
            resolver,
            file,
            this,
            in_main,
            depth,
            depth_limit,
            entry_states.clone(),
        );

        let merged = union_states(&then_b.out_states, &else_b.out_states);
        let mut merge_labels = state_labels(&merged);
        if in_main {
            merge_labels.insert("in_main".to_owned());
        }
        let merge = self.add_node(merge_labels, merged.clone());

        // An empty branch is a pass-through: connect it straight to the merge
        // so that the merge remains reachable (no dead CFG node).
        if then_b.entries.is_empty() {
            self.connect(&[branch], &[merge]);
        } else {
            self.connect(&[branch], &then_b.entries);
            self.connect(&then_b.exits, &[merge]);
        }
        if else_b.entries.is_empty() {
            self.connect(&[branch], &[merge]);
        } else {
            self.connect(&[branch], &else_b.entries);
            self.connect(&else_b.exits, &[merge]);
        }

        Block {
            entries: vec![branch],
            exits: vec![merge],
            out_states: merged,
        }
    }

    /// Build the CFG for a `while` loop: loop-header node (guard) → body
    /// sub-CFG with a back-edge to the header, plus an explicit loop-exit node.
    /// Entity states are joined (a single-pass abstract join of the entry and
    /// body-out states).
    #[allow(clippy::too_many_arguments)]
    fn build_while(
        &mut self,
        cond: &Expr,
        body: &[Stmt],
        resolver: &mut Resolver<'a>,
        file: &str,
        this: &str,
        in_main: bool,
        depth: usize,
        depth_limit: usize,
        entry_states: EntityStates,
    ) -> Block {
        let guard = format!("guard_{}_{}", sanitize_ident(file), cond.span().start);

        let mut header_labels = state_labels(&entry_states);
        header_labels.insert(guard.clone());
        if in_main {
            header_labels.insert("in_main".to_owned());
        }
        let header = self.add_node(header_labels, entry_states.clone());

        let body_b = self.build_stmts(
            body,
            resolver,
            file,
            this,
            in_main,
            depth,
            depth_limit,
            entry_states.clone(),
        );

        // Join the states that can flow around the loop back into the header.
        let joined = union_states(&entry_states, &body_b.out_states);
        self.nodes[header].entity_states = joined.clone();
        let mut hlabels = state_labels(&joined);
        hlabels.insert(guard);
        if in_main {
            hlabels.insert("in_main".to_owned());
        }
        self.nodes[header].labels = hlabels;

        // Loop-entry edge: header → body.
        self.connect(&[header], &body_b.entries);
        // Back-edge: body exits → header.
        self.connect(&body_b.exits, &[header]);

        // Loop-exit node: where execution resumes after the loop terminates.
        let mut exit_labels = state_labels(&joined);
        if in_main {
            exit_labels.insert("in_main".to_owned());
        }
        let exit = self.add_node(exit_labels, joined.clone());
        // Loop-exit edge: header → after-loop node.
        self.connect(&[header], &[exit]);

        Block {
            entries: vec![header],
            exits: vec![exit],
            out_states: joined,
        }
    }

    /// Inline (or summarize) a method call, returning the resulting block, or
    /// `None` when the call cannot be resolved to a known method.
    #[allow(clippy::too_many_arguments)]
    fn try_inline_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        resolver: &Resolver<'a>,
        file: &str,
        depth: usize,
        depth_limit: usize,
        entry_states: &EntityStates,
    ) -> Option<Block> {
        let recv_type = resolver.resolve_expression(receiver);
        let ResolvedType::Class(class_name) = recv_type else {
            return None;
        };
        let method_decl = self.find_method(&class_name, method)?;

        let entry_label = format!(
            "entry_{}_{}",
            sanitize_ident(&class_name),
            sanitize_ident(method)
        );
        let exit_label = format!(
            "exit_{}_{}",
            sanitize_ident(&class_name),
            sanitize_ident(method)
        );

        if depth < depth_limit && !method_decl.flags.is_abstract && !method_decl.body.is_empty() {
            // Inline the callee body as a sub-CFG.
            let mut entry_labels = state_labels(entry_states);
            entry_labels.insert(entry_label);
            let entry = self.add_node(entry_labels, entry_states.clone());

            let mut callee_resolver = Resolver::new(self.map, &class_name);
            for p in &method_decl.params {
                callee_resolver
                    .local_vars
                    .insert(p.name.clone(), crate::model::resolve_type(&p.ty));
            }
            let body_b = self.build_stmts(
                &method_decl.body,
                &mut callee_resolver,
                file,
                &class_name,
                false,
                depth + 1,
                depth_limit,
                entry_states.clone(),
            );

            let mut exit_labels = state_labels(&body_b.out_states);
            exit_labels.insert(exit_label);
            let exit = self.add_node(exit_labels, body_b.out_states.clone());

            self.connect(&[entry], &body_b.entries);
            self.connect(&body_b.exits, &[exit]);

            Some(Block {
                entries: vec![entry],
                exits: vec![exit],
                out_states: body_b.out_states,
            })
        } else {
            // Summarize: entry → exit with all entity states conservatively widened.
            let widened = self.widen_all(entry_states);
            let mut el = state_labels(&widened);
            el.insert(entry_label);
            let entry = self.add_node(el, widened.clone());
            let mut xl = state_labels(&widened);
            xl.insert(exit_label);
            let exit = self.add_node(xl, widened.clone());
            self.connect(&[entry], &[exit]);
            Some(Block {
                entries: vec![entry],
                exits: vec![exit],
                out_states: widened,
            })
        }
    }

    /// Look up a method by name on `class` (walking the inheritance chain).
    fn find_method(&self, class: &str, name: &str) -> Option<MethodDecl> {
        for anc in self.map.ancestors(class) {
            if let Some(entry) = self.map.get(&anc)
                && let Some(m) = entry.decl.methods.iter().find(|m| m.name == name)
            {
                return Some(m.clone());
            }
        }
        None
    }

    /// Resolve the classification root of an expression, if determinable.
    fn root_of_expr(&self, resolver: &Resolver<'a>, expr: &Expr) -> Option<String> {
        let t = resolver.resolve_expression(expr);
        resolver.root_of(&t, &self.extracted.state_roots)
    }

    /// Widen a single root to all of its possible states.
    fn widen_root(&self, states: &mut EntityStates, root: &str) {
        if let Some(r) = self.extracted.roots.iter().find(|r| r.entity == root) {
            let all: HashSet<String> = r.states.iter().cloned().collect();
            states.insert(root.to_owned(), all);
        }
    }

    /// Conservatively widen every root to all of its possible states.
    fn widen_all(&self, states: &EntityStates) -> EntityStates {
        let mut out = states.clone();
        for root in &self.extracted.roots {
            let all: HashSet<String> = root.states.iter().cloned().collect();
            out.insert(root.entity.clone(), all);
        }
        out
    }
}

/// Build a control-flow graph for a statement list.
///
/// `file` and `this` identify the enclosing source file and class (empty for
/// `main`); `in_main` marks whether these statements belong to the `main`
/// block (adding the `in_main` AP to every node).
///
/// # Panics
///
/// Panics if `stmts` is non-empty and the builder fails to create any node
/// (this cannot happen: every non-empty statement list produces at least one
/// node).
#[must_use]
pub fn build_cfg(
    stmts: &[Stmt],
    map: &ClassMap,
    extracted: &Extracted,
    file: &str,
    this: &str,
    in_main: bool,
    depth_limit: usize,
) -> Cfg {
    let mut builder = CfgBuilder {
        nodes: Vec::new(),
        map,
        extracted,
    };
    let entry_states = init_entity_states(extracted);
    let mut resolver = Resolver::new(map, this);
    let block = builder.build_stmts(
        stmts,
        &mut resolver,
        file,
        this,
        in_main,
        0,
        depth_limit,
        entry_states,
    );

    let entry = block.entries.first().copied().unwrap_or(0);
    Cfg {
        nodes: builder.nodes,
        entry,
        exits: block.exits,
    }
}

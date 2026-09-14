//! Model-level (formal) checks over the extracted classification model.

use std::collections::{HashMap, HashSet};

use smol_syntax::ast::{Expr, FieldModifier, File, Stmt, Type};
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

use crate::model::{
    self, ClassMap, Extracted, ResolvedType, Resolver, StateClass, TransitionSystem,
};

/// A user-supplied CTL property to verify.
#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    pub formula: String,
}

/// Run all model checks and return diagnostics.
///
/// `inline_depth` controls how many levels of method calls are inlined when
/// building the whole-program CFG (deeper calls are summarized).
#[must_use]
pub fn check_model(
    files: &[File],
    properties: &[Property],
    inline_depth: usize,
) -> Vec<Diagnostic> {
    let map = model::build_class_map(files);
    let extracted = model::extract(&map);

    let mut diags = Vec::new();

    check_context_fields(&map, files, &mut diags);
    check_classifies(&extracted, &mut diags);
    check_links(&map, files, &mut diags);

    let adapted = check_bodies(&map, &extracted, files, &mut diags);

    let roots: Vec<model::Root> = extracted
        .roots
        .iter()
        .map(|r| model::Root {
            entity: r.entity.clone(),
            states: r.states.clone(),
            adapted: adapted.contains(&r.entity),
        })
        .collect();

    let ts = model::build_transition_system(&roots);
    check_reachability(&ts, &extracted.states, &mut diags);

    // §2.10 initial-state concretisation (additive: never alters CTL results).
    diags.extend(crate::concretise::check_initial_concretisation(
        files, &map, &extracted, &ts,
    ));

    // Whole-program CFG-based model (structural / guard APs, method sequencing).
    let program_ts = model::build_program_ts(files, &map, &extracted, inline_depth);
    check_unreachable_statements(&program_ts, &mut diags);

    for prop in properties {
        check_property(&ts, &program_ts, prop, &mut diags);
    }

    diags
}

// E5001: `context` fields
fn check_context_fields(map: &ClassMap, files: &[File], diags: &mut Vec<Diagnostic>) {
    for file in files {
        for cls in &file.classes {
            let context_fields: Vec<_> = cls
                .fields
                .iter()
                .filter(|f| f.modifier == FieldModifier::Context)
                .collect();

            if context_fields.len() > 1 {
                diags.push(
                    Diagnostic::error(
                        DiagnosticCode::ContextFieldInvalid,
                        format!(
                            "class `{}` declares {} `context` fields; a twin may have at most one",
                            cls.name,
                            context_fields.len()
                        ),
                    )
                    .with_file(&file.path)
                    .with_span(cls.span),
                );
            }

            for f in context_fields {
                if let Type::Named(t) = &f.ty {
                    if !map.contains(t) {
                        diags.push(
                            Diagnostic::error(
                                DiagnosticCode::ContextFieldInvalid,
                                format!(
                                    "`context` field `{}` references unknown type `{t}`",
                                    f.name
                                ),
                            )
                            .with_file(&file.path)
                            .with_span(f.span),
                        );
                    }
                } else {
                    diags.push(
                        Diagnostic::error(
                            DiagnosticCode::ContextFieldInvalid,
                            format!("`context` field `{}` must be a named class type", f.name),
                        )
                        .with_file(&file.path)
                        .with_span(f.span),
                    );
                }
            }
        }
    }
}

// E5003: `classifies` validity
fn check_classifies(extracted: &Extracted, diags: &mut Vec<Diagnostic>) {
    // A `classifies` class with no root at all (no superclass and no `context`
    // field) cannot determine the entity it classifies.
    for s in &extracted.states {
        if s.root.is_empty() {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::ClassifiesInvalid,
                    format!(
                        "state class `{}` has a `classifies` annotation but no superclass and no \
                         `context` field — cannot determine the entity it classifies",
                        s.name
                    ),
                )
                .with_file(&s.file)
                .with_span(s.span),
            );
        }
    }

    // Two classes binding the same OWL class is a model conflict.
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for s in &extracted.states {
        if let Some(prev) = seen.insert(s.iri.as_str(), s.name.as_str()) {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::ClassifiesInvalid,
                    format!(
                        "classes `{prev}` and `{}` both `classifies` the same IRI `{}`",
                        s.name, s.iri
                    ),
                )
                .with_file(&s.file)
                .with_span(s.span),
            );
        }
    }
}

// E5004: `links` placeholders must be declared fields
fn check_links(map: &ClassMap, files: &[File], diags: &mut Vec<Diagnostic>) {
    for file in files {
        for cls in &file.classes {
            for link in &cls.links {
                for name in extract_placeholders(&link.turtle) {
                    if name == "this" {
                        continue;
                    }
                    if map.field(&cls.name, &name).is_none() {
                        diags.push(
                            Diagnostic::error(
                                DiagnosticCode::LinksTypeMismatch,
                                format!(
                                    "`links` annotation references `%{name}`, which is not a \
                                     field of class `{}`",
                                    cls.name
                                ),
                            )
                            .with_file(&file.path)
                            .with_span(link.span),
                        );
                    }
                }
            }
        }
    }
}

/// Extract `%word` placeholders from a Turtle/SPARQL fragment.
fn extract_placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '%' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next.is_ascii_alphabetic() || next == '_' {
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '_') {
                    j += 1;
                }
                out.push(bytes[i + 1..j].iter().collect());
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

// Body walk: E5002 / E5005 / E5006 / E5008 + adapted roots
fn check_bodies(
    map: &ClassMap,
    extracted: &Extracted,
    files: &[File],
    diags: &mut Vec<Diagnostic>,
) -> HashSet<String> {
    let mut adapted: HashSet<String> = HashSet::new();

    for file in files {
        for cls in &file.classes {
            for method in &cls.methods {
                if method.flags.is_abstract {
                    continue;
                }
                let mut resolver = Resolver::new(map, &cls.name);
                for p in &method.params {
                    resolver
                        .local_vars
                        .insert(p.name.clone(), model::resolve_type(&p.ty));
                }
                let mut bc = BodyChecker {
                    map,
                    extracted,
                    adapted: &mut adapted,
                    diags,
                    file: &file.path,
                };
                bc.walk_stmts(&mut resolver, &method.body);
            }
        }
        if let Some(stmts) = &file.main {
            let mut resolver = Resolver::new(map, "");
            let mut bc = BodyChecker {
                map,
                extracted,
                adapted: &mut adapted,
                diags,
                file: &file.path,
            };
            bc.walk_stmts(&mut resolver, stmts);
        }
    }

    adapted
}

struct BodyChecker<'a> {
    map: &'a ClassMap,
    extracted: &'a Extracted,
    adapted: &'a mut HashSet<String>,
    diags: &'a mut Vec<Diagnostic>,
    file: &'a str,
}

impl BodyChecker<'_> {
    fn walk_stmts(&mut self, resolver: &mut Resolver, stmts: &[Stmt]) {
        for stmt in stmts {
            self.walk_stmt(resolver, stmt);
        }
    }

    fn walk_stmt(&mut self, resolver: &mut Resolver, stmt: &Stmt) {
        match stmt {
            Stmt::VarDecl { ty, name, init, .. } => {
                resolver
                    .local_vars
                    .insert(name.clone(), model::resolve_type(ty));
                if let Expr::Classify { expr: inner, span } = init {
                    self.mark_adapted(resolver, inner);
                    let target = model::resolve_type(ty);
                    self.check_classify_ambiguity(resolver, inner, *span, Some(&target));
                    self.walk_expr(resolver, inner);
                } else {
                    self.walk_expr(resolver, init);
                }
            }
            Stmt::Assign { lhs, rhs, .. } => {
                self.walk_expr(resolver, lhs);
                if let Expr::Classify { expr: inner, span } = rhs {
                    self.mark_adapted(resolver, inner);
                    let target = resolver.resolve_expression(lhs);
                    self.check_classify_ambiguity(resolver, inner, *span, Some(&target));
                    self.walk_expr(resolver, inner);
                } else {
                    self.walk_expr(resolver, rhs);
                }
            }
            Stmt::Expr { expr, .. }
            | Stmt::Print { value: expr, .. }
            | Stmt::Destroy { value: expr, .. }
            | Stmt::Return {
                value: Some(expr), ..
            } => {
                self.walk_expr(resolver, expr);
            }
            Stmt::Return { value: None, .. } => {}
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                self.walk_expr(resolver, cond);
                self.walk_stmts(resolver, then_body);
                self.walk_stmts(resolver, else_body);
            }
            Stmt::While {
                cond, body, span, ..
            } => {
                self.walk_expr(resolver, cond);
                self.check_while_advance(resolver, cond, body, *span);
                self.walk_stmts(resolver, body);
            }
            Stmt::Adapt { expr, .. } => {
                self.mark_adapted(resolver, expr);
                self.walk_expr(resolver, expr);
            }
            Stmt::Skip { .. } | Stmt::Breakpoint { .. } => {}
        }
    }

    fn walk_expr(&mut self, resolver: &Resolver, expr: &Expr) {
        match expr {
            Expr::Classify { expr: inner, span } => {
                self.mark_adapted(resolver, inner);
                self.check_classify_ambiguity(resolver, inner, *span, None);
                self.walk_expr(resolver, inner);
            }
            Expr::Port {
                receiver,
                port,
                span,
            } => {
                if let ResolvedType::Fmo(ports) = resolver.resolve_expression(receiver) {
                    if !ports.contains(port) {
                        self.diags.push(
                            Diagnostic::error(
                                DiagnosticCode::FmoPortUnknown,
                                format!(
                                    "FMO port `{port}` is not declared; available ports: {}",
                                    if ports.is_empty() {
                                        "(none)".to_owned()
                                    } else {
                                        ports.join(", ")
                                    }
                                ),
                            )
                            .with_file(self.file)
                            .with_span(*span),
                        );
                    }
                }
                self.walk_expr(resolver, receiver);
            }
            Expr::Member { expr, span } => {
                if expr.trim().is_empty() {
                    self.diags.push(
                        Diagnostic::error(
                            DiagnosticCode::MemberExpressionInvalid,
                            "`member` expression must not be empty".to_owned(),
                        )
                        .with_file(self.file)
                        .with_span(*span),
                    );
                }
            }
            Expr::Validate { expr, span } => {
                if expr.trim().is_empty() {
                    self.diags.push(
                        Diagnostic::error(
                            DiagnosticCode::MemberExpressionInvalid,
                            "`validate` expression must not be empty".to_owned(),
                        )
                        .with_file(self.file)
                        .with_span(*span),
                    );
                }
            }
            Expr::Field { receiver, .. } => self.walk_expr(resolver, receiver),
            Expr::MethodCall {
                receiver,
                method,
                args,
                span,
            } => {
                if method == "port" {
                    if let ResolvedType::Fmo(ports) = resolver.resolve_expression(receiver) {
                        if let Some(Expr::StrLit(port_name, _)) = args.first() {
                            if !ports.contains(port_name) {
                                self.diags.push(
                                    Diagnostic::error(
                                        DiagnosticCode::FmoPortUnknown,
                                        format!(
                                            "FMO port `{port_name}` is not declared; available ports: {}",
                                            if ports.is_empty() {
                                                "(none)".to_owned()
                                            } else {
                                                ports.join(", ")
                                            }
                                        ),
                                    )
                                        .with_file(self.file)
                                        .with_span(*span),
                                );
                            }
                        }
                    }
                }
                self.walk_expr(resolver, receiver);
                for a in args {
                    self.walk_expr(resolver, a);
                }
            }
            Expr::New { args, .. } => {
                for a in args {
                    self.walk_expr(resolver, a);
                }
            }
            Expr::BinOp { left, right, .. } => {
                self.walk_expr(resolver, left);
                self.walk_expr(resolver, right);
            }
            Expr::UnaryOp { expr: inner, .. } => self.walk_expr(resolver, inner),
            Expr::Construct { args, .. }
            | Expr::Access { args, .. }
            | Expr::Simulate { args, .. }
            | Expr::Super { args, .. } => {
                for a in args {
                    self.walk_expr(resolver, a);
                }
            }
            Expr::BuiltinCall { args, .. } => {
                for a in args {
                    self.walk_expr(resolver, a);
                }
            }
            Expr::IntLit(_, _)
            | Expr::FloatLit(_, _)
            | Expr::StrLit(_, _)
            | Expr::BoolLit(_, _)
            | Expr::Null(_)
            | Expr::Var(_, _) => {}
        }
    }

    fn mark_adapted(&mut self, resolver: &Resolver, expr: &Expr) {
        let r = resolver.resolve_expression(expr);
        if let Some(root) = resolver.root_of(&r, &self.extracted.state_roots) {
            self.adapted.insert(root);
        }
    }

    fn check_classify_ambiguity(
        &mut self,
        resolver: &Resolver,
        expr: &Expr,
        span: smol_syntax::span::Span,
        target: Option<&ResolvedType>,
    ) {
        let r = resolver.resolve_expression(expr);
        let ResolvedType::Class(entity) = r else {
            return;
        };
        let holders = model::context_holders(self.map, &entity);

        // An assignment target of a state-holder type disambiguates the result:
        // `h = classify(x)` where `h: HealthState` and `HealthState` declares
        // `context Plant` selects that holder, even when several holders exist.
        if let Some(ResolvedType::Class(s)) = target {
            let matching = holders
                .iter()
                .filter(|h| h.as_str() == s || self.map.is_subtype(s, h))
                .count();
            if matching == 1 {
                return;
            }
        }

        if holders.len() == 1 {
            return;
        }
        let msg = if holders.is_empty() {
            format!(
                "`classify(...)` on `{entity}` has no state-holder class: no class declares \
                 `context {entity}`"
            )
        } else {
            format!(
                "`classify(...)` on `{entity}` is ambiguous: multiple classes declare \
                 `context {entity}` ({})",
                holders.join(", ")
            )
        };
        self.diags.push(
            Diagnostic::error(DiagnosticCode::ClassifyAmbiguous, msg)
                .with_file(self.file)
                .with_span(span),
        );
    }

    fn check_while_advance(
        &mut self,
        resolver: &Resolver,
        cond: &Expr,
        body: &[Stmt],
        span: smol_syntax::span::Span,
    ) {
        let list_vars = collect_list_vars(cond, resolver);
        for var in list_vars {
            if !is_advanced(body, &var) {
                self.diags.push(
                    Diagnostic::error(
                        DiagnosticCode::AdaptationDeadlock,
                        format!(
                            "`while` loop over list `{var}` never advances it with `{var} = {var}.next` \
                             — the adaptation loop may never terminate"
                        ),
                    )
                        .with_file(self.file)
                        .with_span(span),
                );
            }
        }
    }
}

/// Collect the names of list-typed variables referenced in `cond`.
fn collect_list_vars(expr: &Expr, resolver: &Resolver) -> Vec<String> {
    let mut out = Vec::new();
    collect_vars_inner(expr, resolver, &mut out);
    out
}

fn collect_vars_inner(expr: &Expr, resolver: &Resolver, out: &mut Vec<String>) {
    match expr {
        Expr::Var(name, _) => {
            if matches!(resolver.resolve_expression(expr), ResolvedType::List(_))
                && !out.contains(name)
            {
                out.push(name.clone());
            }
        }
        Expr::Field { receiver, .. } => collect_vars_inner(receiver, resolver, out),
        Expr::BinOp { left, right, .. } => {
            collect_vars_inner(left, resolver, out);
            collect_vars_inner(right, resolver, out);
        }
        Expr::UnaryOp { expr: inner, .. } => collect_vars_inner(inner, resolver, out),
        Expr::MethodCall { receiver, args, .. } => {
            collect_vars_inner(receiver, resolver, out);
            for a in args {
                collect_vars_inner(a, resolver, out);
            }
        }
        Expr::Classify { expr: inner, .. } => collect_vars_inner(inner, resolver, out),
        Expr::BuiltinCall { args, .. }
        | Expr::Construct { args, .. }
        | Expr::Access { args, .. } => {
            for a in args {
                collect_vars_inner(a, resolver, out);
            }
        }
        _ => {}
    }
}

/// True if `body` contains an assignment `var = var.next` somewhere.
fn is_advanced(body: &[Stmt], var: &str) -> bool {
    for stmt in body {
        if stmt_advances(stmt, var) {
            return true;
        }
    }
    false
}

fn stmt_advances(stmt: &Stmt, var: &str) -> bool {
    match stmt {
        Stmt::Assign { lhs, rhs, .. } => {
            if let Expr::Var(lhs_name, _) = lhs {
                if lhs_name == var {
                    if let Expr::Field {
                        receiver, field, ..
                    } = rhs
                    {
                        if field == "next" {
                            if let Expr::Var(rec, _) = receiver.as_ref() {
                                return rec == var;
                            }
                        }
                    }
                }
            }
            false
        }
        Stmt::If {
            then_body,
            else_body,
            ..
        } => {
            then_body.iter().any(|s| stmt_advances(s, var))
                || else_body.iter().any(|s| stmt_advances(s, var))
        }
        Stmt::While { body, .. } => body.iter().any(|s| stmt_advances(s, var)),
        _ => false,
    }
}

// E5007: reachability
fn check_reachability(ts: &TransitionSystem, states: &[StateClass], diags: &mut Vec<Diagnostic>) {
    let reach = model::reachable(ts);
    let name_to_idx: HashMap<&str, usize> = {
        let mut m = HashMap::new();
        for (i, labels) in ts.labels.iter().enumerate() {
            for l in labels {
                m.entry(l.as_str()).or_insert(i);
            }
        }
        m
    };

    for s in states {
        if s.root.is_empty() {
            continue; // already reported via E5003
        }
        let Some(&idx) = name_to_idx.get(s.name.as_str()) else {
            continue;
        };
        if !reach.contains(&idx) {
            diags.push(
                Diagnostic::error(
                    DiagnosticCode::StateNotReachable,
                    format!(
                        "state class `{}` (`classifies <{}>`) is unreachable: no `adapt`/`classify` \
                         call ever targets entity `{}`",
                        s.name, s.iri, s.root
                    ),
                )
                    .with_file(&s.file)
                    .with_span(s.span),
            );
        }
    }
}

// E5010: whole-program unreachable statement (CFG node with no path from init)
fn check_unreachable_statements(ts: &TransitionSystem, diags: &mut Vec<Diagnostic>) {
    let reach = model::reachable(ts);
    for i in 0..ts.n {
        if !reach.contains(&i) && !ts.labels[i].is_empty() {
            let mut v: Vec<String> = ts.labels[i].iter().cloned().collect();
            v.sort();
            diags.push(Diagnostic::error(
                DiagnosticCode::UnreachableStatement,
                format!(
                    "program point `{}` is unreachable: no path from the initial state",
                    v.join(" ")
                ),
            ));
        }
    }
}

// User CTL properties (E5009 / E5011): dispatch per formula.
fn check_property(
    ts: &TransitionSystem,
    program_ts: &TransitionSystem,
    prop: &Property,
    diags: &mut Vec<Diagnostic>,
) {
    match crate::ctl::parse_prop(&prop.formula) {
        Err(e) => {
            diags.push(Diagnostic::error(
                DiagnosticCode::InvariantViolation,
                format!("invalid property `{}`: {e}", prop.formula),
            ));
        }
        Ok(parsed) => {
            // Dispatch: formulas referencing structural / guard APs are checked
            // against the whole-program CFG model; classification-only formulas
            // use the fast classification-hierarchy model.
            let uses_structural = contains_structural_ap(&parsed);
            let target = if uses_structural { program_ts } else { ts };
            let code = if uses_structural {
                DiagnosticCode::ProgramPropertyViolation
            } else {
                DiagnosticCode::InvariantViolation
            };
            if let Err(msg) = crate::ctl::verify(target, &parsed) {
                diags.push(Diagnostic::error(
                    code,
                    format!("property `{}` failed: {msg}", prop.name),
                ));
            }
        }
    }
}

/// True if `p` references a structural / guard atomic proposition anywhere.
fn contains_structural_ap(p: &crate::ctl::Property) -> bool {
    use crate::ctl::Property;
    match p {
        Property::Atom(a) => crate::cfg::is_structural_label(a),
        Property::True | Property::False => false,
        Property::Not(a) => contains_structural_ap(a),
        Property::Ax(a)
        | Property::Ex(a)
        | Property::Ag(a)
        | Property::Ef(a)
        | Property::Af(a)
        | Property::Eg(a) => contains_structural_ap(a),
        Property::And(a, b)
        | Property::Or(a, b)
        | Property::Implies(a, b)
        | Property::Au(a, b)
        | Property::Eu(a, b) => contains_structural_ap(a) || contains_structural_ap(b),
    }
}

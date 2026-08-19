//! Pest-based parser for SMOL.
//!
//! This module implements parsing using the pest PEG grammar (smol.pest).
//! It converts the pest parse tree into the `smol_syntax` AST.

use pest::Parser;
use pest::iterators::Pair;
use smol_syntax::ast::*;
use smol_syntax::span::Span;
use smol_syntax::diagnostic::{Diagnostic, DiagnosticCode};

#[derive(pest_derive::Parser)]
#[grammar = "smol.pest"]
pub struct SmolParser;

/// Parse a SMOL source file using the pest grammar.
/// Returns `(File, Vec<Diagnostic>)` where diagnostics include any parsing errors.
pub fn parse(path: &str, src: &str) -> (File, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();

    match SmolParser::parse(Rule::program, src) {
        Ok(mut pairs) => {
            let program_pair = pairs.next().expect("Program rule must produce a pair");
            let (classes, main_stmts) = parse_program(program_pair, &mut diagnostics);
            let file = File {
                path: path.to_owned(),
                classes,
                main: main_stmts,
            };
            (file, diagnostics)
        }
        Err(err) => {
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::UnexpectedToken,
                    format!("Pest parser error: {}", err),
                )
                    .with_file(path)
                    .with_span(Span::new(0, 1))
            );
            (
                File {
                    path: path.to_owned(),
                    classes: Vec::new(),
                    main: None,
                },
                diagnostics
            )
        }
    }
}

fn parse_program(pair: Pair<Rule>, _diagnostics: &mut Vec<Diagnostic>) -> (Vec<ClassDecl>, Option<Vec<Stmt>>) {
    let mut classes = Vec::new();
    let mut main_stmts = None;

    for  inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::class_def => {
                classes.push(parse_class_def(inner));
            }
            Rule::main_block => {
                main_stmts = Some(parse_main_block(inner));
            }
            _ => {}
        }
    }

    (classes, main_stmts)
}

fn parse_main_block(pair: Pair<Rule>) -> Vec<Stmt> {
    Vec::new()
}

/// Parse a class definition
fn parse_class_def(pair: Pair<Rule>) -> ClassDecl {
    let span = span_from_pair(&pair);
    let mut is_abstract = false;
    let mut is_hidden = false;
    let mut name = String::new();
    let mut type_params = Vec::new();
    let mut extends = None;
    let mut fields = Vec::new();
    let mut init_fields = Vec::new();
    let mut classifies = None;
    let mut retrieves = None;
    let mut links = Vec::new();
    let mut methods = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::abstract_kw => is_abstract = true,
            Rule::hidden_kw => is_hidden = true,
            Rule::name => name = parse_name(inner),
            Rule::name_list => type_params = parse_name_list(inner),
            Rule::type_expr => extends = Some(extract_type_name(parse_type_expr(inner))),
            Rule::field_decl_list => fields = parse_field_decl_list(inner),
            Rule::field_decl_init_list => init_fields = parse_field_decl_init_list(inner),
            Rule::classifies_block => classifies = Some(parse_classifies_block(inner)),
            Rule::retrieves_block => retrieves = Some(parse_retrieves_block(inner)),
            Rule::models_block => links = parse_models_block(inner),
            Rule::method_def => methods.push(parse_method_def(inner)),
            _ => {}
        }
    }

    ClassDecl {
        is_abstract,
        is_hidden,
        name,
        type_params,
        extends,
        fields,
        init_fields,
        classifies,
        retrieves,
        links,
        methods,
        span,
    }
}

fn span_from_pair(pair: &Pair<Rule>) -> Span {
    let start = pair.as_span().start();
    let end = pair.as_span().end();
    Span::new(start, end)
}

/// Extract the simple type name from a Type for the extends field
fn extract_type_name(ty: Type) -> String {
    match ty {
        Type::Named(n) => n,
        Type::Generic(n, _) => n,
        _ => "Object".to_owned(),
    }
}

fn parse_name(pair: Pair<Rule>) -> String {
    pair.as_str().to_owned()
}

fn parse_name_list(pair: Pair<Rule>) -> Vec<String> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::name)
        .map(parse_name)
        .collect()
}

/// Parse a type expression into a Type AST node
fn parse_type_expr(pair: Pair<Rule>) -> Type {
    let inner = pair.into_inner().next().expect("Type expression must have inner");
    match inner.as_rule() {
        Rule::simple_type => {
            let name_str = inner
                .into_inner()
                .next()
                .expect("Simple type must have a name");
            Type::Named(name_str.as_str().to_owned())
        }
        Rule::generic_type => {
            let mut parts = inner.into_inner();
            let name_str = parts.next().expect("generic_type must have name");
            let name = name_str.as_str().to_owned();

            let type_list = parts.next().expect("generic_type must have type_list");
            let type_params = parse_type_list(type_list);
            Type::Generic(name, type_params)
        }
        Rule::fmu_type => parse_fmu_type(inner),
        _ => Type::Named("Unknown".to_owned()),
    }
}

fn parse_type_list(pair: Pair<Rule>) -> Vec<Type> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::type_expr)
        .map(parse_type_expr)
        .collect()
}

fn parse_fmu_type(pair: Pair<Rule>) -> Type {
    let mut ports = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::fmu_param_list {
            ports = parse_fmu_param_list(inner);
        }
    }
    Type::Fmo(ports)
}

fn parse_fmu_param_list(pair: Pair<Rule>) -> Vec<FmoPort> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::fmu_param)
        .map(parse_fmu_param)
        .collect()
}

fn parse_fmu_param(pair: Pair<Rule>) -> FmoPort {
    let span = span_from_pair(&pair);
    let mut direction = FmoDirection::In;
    let mut ty = Type::Int;
    let mut name = String::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::fmu_in => direction = FmoDirection::In,
            Rule::fmu_out => direction = FmoDirection::Out,
            Rule::type_expr => ty = parse_type_expr(inner),
            Rule::name => name = parse_name(inner),
            _ => {}
        }
    }

    FmoPort { direction, ty, name, span }
}

fn parse_field_decl_list(pair: Pair<Rule>) -> Vec<FieldDecl> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::field_decl)
        .map(parse_field_decl)
        .collect()
}

fn parse_field_decl(pair: Pair<Rule>) -> FieldDecl {
    let span = span_from_pair(&pair);
    let mut modifier = FieldModifier::None;
    let mut ty = Type::Int;
    let mut name = String::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::hidden_kw => modifier = FieldModifier::Hidden,
            Rule::domain_kw => modifier = FieldModifier::Domain,
            Rule::context_kw => modifier = FieldModifier::Context,
            Rule::type_expr => ty = parse_type_expr(inner),
            Rule::name => name = parse_name(inner),
            _ => {}
        }
    }

    FieldDecl {
        modifier,
        ty,
        name,
        span,
    }
}

fn parse_field_decl_init_list(pair: Pair<Rule>) -> Vec<InitFieldDecl> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::field_decl_init)
        .map(parse_field_decl_init)
        .collect()
}

fn parse_field_decl_init(pair: Pair<Rule>) -> InitFieldDecl {
    let span = span_from_pair(&pair);
    let mut modifier = FieldModifier::None;
    let mut ty = Type::Int;
    let mut name = String::new();
    let mut init = Expr::Null(span);

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::hidden_kw => modifier = FieldModifier::Hidden,
            Rule::domain_kw => modifier = FieldModifier::Domain,
            Rule::context_kw => modifier = FieldModifier::Context,
            Rule::type_expr => ty = parse_type_expr(inner),
            Rule::name => name = parse_name(inner),
            Rule::expression => init = parse_expression(inner),
            _ => {}
        }
    }

    InitFieldDecl {
        modifier,
        ty,
        name,
        init,
        span,
    }
}

fn parse_classifies_block(pair: Pair<Rule>) -> ClassifiesAnnotation {
    let span = span_from_pair(&pair);
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::string_lit {
            let iri = parse_string_lit(inner);
            return ClassifiesAnnotation { iri, span };
        }
    }

    ClassifiesAnnotation {
        iri: String::new(),
        span,
    }
}

fn parse_retrieves_block(pair: Pair<Rule>) -> RetrievesAnnotation {
    let span = span_from_pair(&pair);
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::string_lit {
            let sparql = parse_string_lit(inner);
            return RetrievesAnnotation { sparql, span };
        }
    }

    RetrievesAnnotation {
        sparql: String::new(),
        span,
    }
}

fn parse_models_block(pair: Pair<Rule>) -> Vec<LinksAnnotation> {
    let mut links = Vec::new();
    let inner_list: Vec<_> = pair.into_inner().collect();

    for inner in inner_list {
        match inner.as_rule() {
            Rule::simple_models_block => {
                for p in inner.into_inner() {
                    if p.as_rule() == Rule::string_lit {
                        let span = span_from_pair(&p);
                        let turtle = parse_string_list(p);
                        links.push(LinksAnnotation {
                            turtle,
                            guard: None,
                            span,
                        });
                    }
                }
            }
            Rule::complex_models_block => {
                let block_span = span_from_pair(&inner);
                let mut guard_expr = None;
                let mut turtle_str = String::new();

                for p in inner.into_inner() {
                    match p.as_rule() {
                        Rule::expression => {
                            guard_expr = Some(Box::new(parse_expression(p)));
                        },
                        Rule::string_lit => {
                            turtle_str = parse_string_lit(p);
                        },
                        _ => {}
                    }
                }

                if !turtle_str.is_empty() {
                    links.push(LinksAnnotation {
                        turtle: turtle_str,
                        guard: guard_expr,
                        span: block_span,
                    });
                }
            }
            _ => {}
        }
    }

    links
}

fn parse_method_def(pair: Pair<Rule>) -> MethodDecl {
    let span = span_from_pair(&pair);
    let mut flags = MethodFlags::default();
    let mut ret_type = Type::Unit;
    let mut name = String::new();
    let mut params = Vec::new();
    let mut adapts = Vec::new();
    let mut body = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::abstract_kw => flags.is_abstract = true,
            Rule::rule_kw => flags.is_rule = true,
            Rule::domain_kw => flags.is_domain = true,
            Rule::override_kw => flags.is_override = true,
            Rule::type_expr => ret_type = parse_type_expr(inner),
            Rule::name => name = parse_name(inner),
            Rule::param_list => params = parse_param_list(inner),
            Rule::name_list => params = parse_name_list(inner),
            Rule::statement_list => body = parse_statement_list(inner),
            _ => {}
        }
    }

    MethodDecl {
        flags,
        ret_type,
        name,
        params,
        adapts,
        body,
        span,
    }
}

fn parse_param_list(pair: Pair<Rule>) -> Vec<Param> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::param)
        .map(parse_param)
        .collect()
}

fn parse_param(pair: Pair<Rule>) -> Param {
    let span = span_from_pair(&pair);
    let mut ty = Type::Int;
    let mut name = String::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::type_expr => ty = parse_type_expr(inner),
            Rule::name => name = parse_name(inner),
            _ => {}
        }
    }

    Param { ty, name, span }
}

fn parse_statement_list(pair: Pair<Rule>) -> Vec<Stmt> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::single_stmt)
        .map(parse_statement)
        .collect()
}

fn parse_statement(pair: Pair<Rule>) -> Stmt {
    let inner = pair.into_inner().next().expect("statement must have inner");
    parse_single_stmt(inner)
}

fn parse_single_stmt(pair: Pair<Rule>) -> Stmt {
    match pair.as_rule() {
        Rule::skip_stmt => {
            let span = span_from_pair(&pair);
            Stmt::Skip { span }
        }
        Rule::return_stmt => {
            let span = span_from_pair(&pair);
            let mut value = None;
            for inner in pair.into_inner() {
                if inner.as_rule() == Rule::expression {
                    value = Some(parse_expression(inner));
                }
            }
            Stmt::Return { value, span }
        }
        Rule::breakpoint_stmt => {
            let span = span_from_pair(&pair);
            Stmt::Breakpoint { span }
        }
        Rule::output_stmt => {
            let span = span_from_pair(&pair);
            let mut value = Expr::Null(span);
            for inner in pair.into_inner() {
                if inner.as_rule() == Rule::expression {
                    value = parse_expression(inner);
                }
            }
            Stmt::Print { value, span }
        }
        Rule::destroy_stmt => {
            let span = span_from_pair(&pair);
            let mut value = Expr::Null(span);
            for inner in pair.into_inner() {
                if inner.as_rule() == Rule::expression {
                    value = parse_expression(inner);
                }
            }
            Stmt::Destroy { value, span }
        }
        Rule::adapt_stmt => {
            let span = span_from_pair(&pair);
            let mut expr = Expr::Null(span);
            for inner in pair.into_inner() {
                if inner.as_rule() == Rule::expression {
                    expr = parse_expression(inner);
                }
            }
            Stmt::Adapt { expr, span }
        }
        Rule::tick_stmt => {
            let span = span_from_pair(&pair);
            Stmt::Skip { span }
        }
        Rule::if_stmt => parse_if_stmt(pair),
        Rule::while_stmt => parse_while_stmt(pair),
        Rule::assign_stmt => parse_assign_stmt(pair),
        Rule::call_stmt => parse_call_stmt(pair),
        Rule::create_stmt => parse_create_stmt(pair),
        Rule::sparql_stmt => parse_sparql_stmt(pair),
        Rule::construct_stmt => parse_construct_stmt(pair),
        Rule::owl_stmt => parse_owl_stmt(pair),
        Rule::validate_stmt => parse_validate_stmt(pair),
        Rule::simulate_stmt => parse_simulate_stmt(pair),
        Rule::classify_stmt => parse_classify_stmt(pair),
        Rule::super_stmt => parse_super_stmt(pair),
        _ => {
            let span = span_from_pair(&pair);
            Stmt::Skip { span }
        }
    }
}

fn parse_if_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    let mut cond = Expr::Null(span);
    let mut then_body = Vec::new();
    let mut else_body = Vec::new();
    let mut found_else = false;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::expression => cond = parse_expression(inner),
            Rule::statement_list => {
                if !found_else {
                    then_body = parse_statement_list(inner);
                    found_else = true;
                } else {
                    else_body = parse_statement_list(inner);
                }
            }
            _ => {}
        }
    }

    Stmt::If {
        cond,
        then_body,
        else_body,
        span,
    }
}

fn parse_while_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    let mut cond = Expr::Null(span);
    let mut body = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::expression => cond = parse_expression(inner),
            Rule::statement_list => body = parse_statement_list(inner),
            _ => {}
        }
    }

    Stmt::While { cond, body, span }
}

fn parse_assign_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    let mut exprs = Vec::new();

    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::expression {
            exprs.push(parse_expression(inner));
        }
    }

    if exprs.len() == 2 {
        Stmt::Assign {
            lhs: exprs[0].clone(),
            rhs: exprs[1].clone(),
            span,
        }
    } else {
        Stmt::Skip { span }
    }
}

fn parse_call_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    Stmt::Skip { span }
}

fn parse_create_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    Stmt::Skip { span }
}

fn parse_sparql_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    Stmt::Skip { span }
}

fn parse_construct_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    Stmt::Skip { span }
}

fn parse_owl_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    Stmt::Skip { span }
}

fn parse_validate_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    Stmt::Skip { span }
}

fn parse_simulate_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    Stmt::Skip { span }
}

fn parse_classify_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    Stmt::Skip { span }
}

fn parse_super_stmt(pair: Pair<Rule>) -> Stmt {
    let span = span_from_pair(&pair);
    Stmt::Skip { span }
}

/// Parse expressions with precedence handling
fn parse_expression(pair: Pair<Rule>) -> Expr {
    match pair.as_rule() {
        Rule::expression => {
            let inner = pair.into_inner().next().expect("expression must have inner");
            parse_expression(inner)
        }
        Rule::expr_or => parse_expr_or(pair),
        Rule::expr_and => parse_expr_and(pair),
        Rule::expr_eq => parse_expr_eq(pair),
        Rule::expr_cmp => parse_expr_cmp(pair),
        Rule::expr_add => parse_expr_add(pair),
        Rule::expr_mul => parse_expr_mul(pair),
        Rule::expr_unary => parse_expr_unary(pair),
        Rule::expr_postfix => parse_expr_postfix(pair),
        Rule::expr_primary => parse_expr_primary(pair),
        _ => Expr::Null(span_from_pair(&pair)),
    }
}

fn parse_expr_or(pair: Pair<Rule>) -> Expr {
    let span = span_from_pair(&pair);
    let terms: Vec<Expr> = pair
        .into_inner()
        .filter(|p| p.as_rule() == Rule::expr_and)
        .map(|p| parse_expression(p))
        .collect();

    if terms.is_empty() {
        Expr::Null(span)
    } else if terms.len() == 1 {
        terms.into_iter().next().unwrap()
    } else {
        let mut result = terms[0].clone();
        for term in terms.iter().skip(1) {
            result = Expr::BinOp {
                op: BinOp::Or,
                left: Box::new(result),
                right: Box::new(term.clone()),
                span,
            };
        }
        result
    }
}

fn parse_expr_and(pair: Pair<Rule>) -> Expr {
    let span = span_from_pair(&pair);
    let terms: Vec<Expr> = pair
        .into_inner()
        .filter(|p| p.as_rule() == Rule::expr_eq)
        .map(|p| parse_expression(p))
        .collect();

    if terms.is_empty() {
        Expr::Null(span)
    } else if terms.len() == 1 {
        terms.into_iter().next().unwrap()
    } else {
        let mut result = terms[0].clone();
        for term in terms.iter().skip(1) {
            result = Expr::BinOp {
                op: BinOp::And,
                left: Box::new(result),
                right: Box::new(term.clone()),
                span,
            };
        }
        result
    }
}

fn parse_expr_eq(pair: Pair<Rule>) -> Expr {
    let span = span_from_pair(&pair);
    let pairs: Vec<_> = pair.into_inner().collect();

    if pairs.is_empty() {
        return Expr::Null(span);
    }

    let mut result = parse_expression(pairs[0].clone());
    let mut i = 1;

    while i < pairs.len() {
        let op_str = pairs[i].as_str();
        let op = if op_str.contains("==") {
            BinOp::Eq
        } else {
            BinOp::NotEq
        };

        if i + 1 < pairs.len() {
            let right = parse_expression(pairs[i + 1].clone());
            result = Expr::BinOp {
                op,
                left: Box::new(result),
                right: Box::new(right),
                span,
            };
            i += 2;
        } else {
            break;
        }
    }

    result
}

fn parse_expr_cmp(pair: Pair<Rule>) -> Expr {
    let span = span_from_pair(&pair);
    let mut result = Expr::Null(span);

    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::expr_add {
            result = parse_expression(inner);
        }
    }

    result
}

fn parse_expr_add(pair: Pair<Rule>) -> Expr {
    let span = span_from_pair(&pair);
    let mut result = Expr::Null(span);

    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::expr_mul {
            result = parse_expression(inner);
        }
    }

    result
}

fn parse_expr_mul(pair: Pair<Rule>) -> Expr {
    let span = span_from_pair(&pair);
    let mut result = Expr::Null(span);

    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::expr_unary {
            result = parse_expression(inner);
        }
    }

    result
}

fn parse_expr_unary(pair: Pair<Rule>) -> Expr {
    match pair.as_rule() {
        Rule::not_expr => {
            let span = span_from_pair(&pair);
            for inner in pair.into_inner() {
                if inner.as_rule() == Rule::expr_postfix {
                    let expr = parse_expression(inner);
                    return Expr::UnaryOp {
                        op: UnaryOp::Not,
                        expr: Box::new(expr),
                        span,
                    };
                }
            }
            Expr::Null(span)
        }
        Rule::expr_postfix => parse_expression(pair),
        _ => Expr::Null(span_from_pair(&pair)),
    }
}

fn parse_expr_postfix(pair: Pair<Rule>) -> Expr {
    let mut result = Expr::Null(span_from_pair(&pair));

    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::expr_primary {
            result = parse_expression(inner);
        } else if inner.as_rule() == Rule::postfix {
            result = apply_postfix(result, inner);
        }
    }

    result
}

fn apply_postfix(expr: Expr, postfix: Pair<Rule>) -> Expr {
    for inner in postfix.into_inner() {
        return match inner.as_rule() {
            Rule::method_call_postfix => {
                let span = span_from_pair(&inner);
                let mut method = String::new();
                let mut args = Vec::new();

                for p in inner.into_inner() {
                    match p.as_rule() {
                        Rule::name => method = parse_name(p),
                        Rule::arg_list => args = parse_arg_list(p),
                        _ => {}
                    }
                }

                Expr::MethodCall {
                    receiver: Box::new(expr),
                    method,
                    args,
                    span,
                }
            }
            Rule::field_access_postfix => {
                let span = span_from_pair(&inner);
                let field = inner
                    .into_inner()
                    .next()
                    .map(|n| parse_name(n))
                    .unwrap_or_default();

                Expr::Field {
                    receiver: Box::new(expr),
                    field,
                    span,
                }
            }
            Rule::port_access => {
                let span = span_from_pair(&inner);
                let port = inner
                    .into_inner()
                    .find(|p| p.as_rule() == Rule::string_lit)
                    .map(parse_string_lit)
                    .unwrap_or_default();

                Expr::Port {
                    receiver: Box::new(expr),
                    port,
                    span,
                }
            }
            _ => expr,
        };
    }
    expr
}

fn parse_expr_primary(pair: Pair<Rule>) -> Expr {
    let span = span_from_pair(&pair);
    let inner = pair.into_inner().next().expect("primary must have inner");

    match inner.as_rule() {
        Rule::integer_lit => {
            let n: i64 = inner.as_str().parse().unwrap_or(0);
            Expr::IntLit(n, span)
        }
        Rule::float_lit => {
            let f: f64 = inner.as_str().parse().unwrap_or(0.0);
            Expr::FloatLit(f, span)
        }
        Rule::string_lit => {
            let s = parse_string_lit(inner);
            Expr::StrLit(s, span)
        }
        Rule::true_lit => Expr::BoolLit(true, span),
        Rule::false_lit => Expr::BoolLit(false, span),
        Rule::null_lit => Expr::Null(span),
        Rule::unit_lit => Expr::Null(span),
        Rule::this_expr => Expr::Var("this".to_owned(), span),
        Rule::name_expr => {
            let name = parse_name(inner);
            Expr::Var(name, span)
        }
        Rule::conversion_expr => parse_conversion_expr(inner),
        Rule::expression => parse_expression(inner),
        _ => Expr::Null(span),
    }
}

fn parse_conversion_expr(pair: Pair<Rule>) -> Expr {
    let span = span_from_pair(&pair);
    let mut name = String::new();
    let mut args = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::conversion => name = inner.as_str().to_owned(),
            Rule::expression => args.push(parse_expression(inner)),
            _ => {}
        }
    }

    Expr::BuiltinCall { name, args, span }
}

fn parse_arg_list(pair: Pair<Rule>) -> Vec<Expr> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::expression)
        .map(parse_expression)
        .collect()
}

fn parse_string_lit(pair: Pair<Rule>) -> String {
    let s = pair.as_str();
    if s.starts_with('"') && s.ends_with('"') {
        let unquoted = &s[1..s.len() - 1];
        unquoted.replace("\\\"", "\"")
    } else {
        s.to_owned()
    }
}

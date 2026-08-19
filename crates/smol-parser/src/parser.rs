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

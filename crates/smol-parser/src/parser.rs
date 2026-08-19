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

fn parse_class_def(pair: Pair<Rule>) -> ClassDecl {
    // return nothing for now, just a placeholder
    ClassDecl {
        is_abstract: false,
        is_hidden: false,
        name: String::new(),
        type_params: vec![],
        extends: None,
        fields: vec![],
        init_fields: vec![],
        classifies: None,
        retrieves: None,
        links: vec![],
        methods: Vec::new(),
        span: Default::default(),
    }
}

fn parse_main_block(pair: Pair<Rule>) -> Vec<Stmt> {
    Vec::new()
}

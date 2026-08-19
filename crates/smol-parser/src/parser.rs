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

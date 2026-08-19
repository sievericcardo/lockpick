//! `smol-parser` — pest PEG parser for SMOL source files.
//!
//! Entry points:
//! - [`lex`]           — tokenise a source string using logos; returns `Vec<SpannedToken>`.
//! - [`parse`]         — parse a source string using pest PEG grammar; returns an AST [`File`].
//!
//! Diagnostics are collected as [`Diagnostic`] values and returned alongside
//! partial ASTs so that multiple errors per file are always reported.
pub mod parser;
pub mod lexer;

pub use lexer::lex;
pub use parser::parse;
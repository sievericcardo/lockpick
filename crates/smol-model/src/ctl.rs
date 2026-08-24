//! A small CTL (Computation Tree Logic) property language and explicit-state
//! model checker.
//!
//! Supported syntax:
//!
//! ```text
//! expr   := impl
//! impl   := or ( "->" or )*
//! or     := and ( "||" and )*
//! and    := unary ( "&&" unary )*
//! unary  := "!" unary
//!         | "AX" unary | "EX" unary | "AG" unary | "EF" unary | "AF" unary | "EG" unary
//!         | "A[" expr "U" expr "]" | "E[" expr "U" expr "]"
//!         | primary
//! primary := "true" | "false" | ident | "(" expr ")"
//! ```
//!
//! An `ident` is a state-class name (an atomic proposition that holds exactly
//! in the state of that name).
use citreelo::ctl::{BinaryCTLOperator, CTLFormula, CTLFormulaLeaf, UnaryCTLOperator};
use citreelo::solve::CtlModelChecker;

use crate::kripke::{to_kripke, Ap};
use crate::model::TransitionSystem;

/// A CTL property, which can be evaluated on a [`TransitionSystem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Property {
    True,
    False,
    /// An atomic proposition (a state-class name).
    Atom(String),
    Not(Box<Property>),
    And(Box<Property>, Box<Property>),
    Or(Box<Property>, Box<Property>),
    Implies(Box<Property>, Box<Property>),
    Ax(Box<Property>),
    Ex(Box<Property>),
    Ag(Box<Property>),
    Ef(Box<Property>),
    Af(Box<Property>),
    Eg(Box<Property>),
    Au(Box<Property>, Box<Property>),
    Eu(Box<Property>, Box<Property>),
}

impl std::fmt::Display for Property {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Property::True => write!(f, "true"),
            Property::False => write!(f, "false"),
            Property::Atom(a) => write!(f, "{a}"),
            Property::Not(p) => write!(f, "!{p}"),
            Property::And(a, b) => write!(f, "({a} && {b})"),
            Property::Or(a, b) => write!(f, "({a} || {b})"),
            Property::Implies(a, b) => write!(f, "({a} -> {b})"),
            Property::Ax(p) => write!(f, "AX {p}"),
            Property::Ex(p) => write!(f, "EX {p}"),
            Property::Ag(p) => write!(f, "AG {p}"),
            Property::Ef(p) => write!(f, "EF {p}"),
            Property::Af(p) => write!(f, "AF {p}"),
            Property::Eg(p) => write!(f, "EG {p}"),
            Property::Au(a, b) => write!(f, "A[{a} U {b}]"),
            Property::Eu(a, b) => write!(f, "E[{a} U {b}]"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    And,
    Or,
    Implies,
    Not,
    LParen,
    RParen,
    LBracket,
    RBracket,
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut toks = Vec::new();
    let bytes: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            c if c.is_whitespace() => {
                i += 1;
            }
            '(' => {
                toks.push(Token::LParen);
                i += 1;
            }
            ')' => {
                toks.push(Token::RParen);
                i += 1;
            }
            '[' => {
                toks.push(Token::LBracket);
                i += 1;
            }
            ']' => {
                toks.push(Token::RBracket);
                i += 1;
            }
            '!' => {
                toks.push(Token::Not);
                i += 1;
            }
            '&' if bytes.get(i + 1) == Some(&'&') => {
                toks.push(Token::And);
                i += 2;
            }
            '|' if bytes.get(i + 1) == Some(&'|') => {
                toks.push(Token::Or);
                i += 2;
            }
            '-' if bytes.get(i + 1) == Some(&'>') => {
                toks.push(Token::Implies);
                i += 2;
            }
            c if c.is_ascii_alphanumeric() || c == '_' => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_')
                {
                    i += 1;
                }
                let word: String = bytes[start..i].iter().collect();
                toks.push(Token::Ident(word));
            }
            _ => {
                return Err(format!("unexpected character `{c}` in property"));
            }
        }
    }
    Ok(toks)
}

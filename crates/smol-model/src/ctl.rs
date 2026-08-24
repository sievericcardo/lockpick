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

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, tok: Token) -> Result<(), String> {
        if self.peek() == Some(&tok) {
            self.bump();
            Ok(())
        } else {
            Err(format!("expected {tok:?}, got {:?}", self.peek()))
        }
    }

    fn parse_expr(&mut self) -> Result<Property, String> {
        self.parse_impl()
    }

    fn parse_impl(&mut self) -> Result<Property, String> {
        let mut left = self.parse_or()?;
        while self.peek() == Some(&Token::Implies) {
            self.bump();
            let right = self.parse_or()?;
            left = Property::Implies(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<Property, String> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(&Token::Or) {
            self.bump();
            let right = self.parse_and()?;
            left = Property::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Property, String> {
        let mut left = self.parse_unary()?;
        while self.peek() == Some(&Token::And) {
            self.bump();
            let right = self.parse_unary()?;
            left = Property::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Property, String> {
        match self.peek().cloned() {
            Some(Token::Not) => {
                self.bump();
                Ok(Property::Not(Box::new(self.parse_unary()?)))
            }
            Some(Token::Ident(ref kw)) => {
                let kw = kw.clone();
                // `A`/`E` are path quantifiers only when followed by `[`.
                let next_is_bracket =
                    matches!(self.toks.get(self.pos + 1), Some(Token::LBracket));
                match kw.as_str() {
                    "AX" => {
                        self.bump();
                        Ok(Property::Ax(Box::new(self.parse_unary()?)))
                    }
                    "EX" => {
                        self.bump();
                        Ok(Property::Ex(Box::new(self.parse_unary()?)))
                    }
                    "AG" => {
                        self.bump();
                        Ok(Property::Ag(Box::new(self.parse_unary()?)))
                    }
                    "EF" => {
                        self.bump();
                        Ok(Property::Ef(Box::new(self.parse_unary()?)))
                    }
                    "AF" => {
                        self.bump();
                        Ok(Property::Af(Box::new(self.parse_unary()?)))
                    }
                    "EG" => {
                        self.bump();
                        Ok(Property::Eg(Box::new(self.parse_unary()?)))
                    }
                    "A" if next_is_bracket => {
                        self.bump();
                        self.expect(Token::LBracket)?;
                        let l = self.parse_expr()?;
                        self.eat_until_ident()?;
                        let r = self.parse_expr()?;
                        self.expect(Token::RBracket)?;
                        Ok(Property::Au(Box::new(l), Box::new(r)))
                    }
                    "E" if next_is_bracket => {
                        self.bump();
                        self.expect(Token::LBracket)?;
                        let l = self.parse_expr()?;
                        self.eat_until_ident()?;
                        let r = self.parse_expr()?;
                        self.expect(Token::RBracket)?;
                        Ok(Property::Eu(Box::new(l), Box::new(r)))
                    }
                    _ => self.parse_primary(),
                }
            }
            _ => self.parse_primary(),
        }
    }

    fn eat_until_ident(&mut self) -> Result<(), String> {
        // After parsing the left operand of an until, consume the `U` keyword.
        match self.bump() {
            Some(Token::Ident(u)) if u == "U" => Ok(()),
            other => Err(format!("expected `U`, got {other:?}")),
        }
    }

    fn parse_primary(&mut self) -> Result<Property, String> {
        match self.bump() {
            Some(Token::Ident(s)) if s == "true" => Ok(Property::True),
            Some(Token::Ident(s)) if s == "false" => Ok(Property::False),
            Some(Token::Ident(s)) => Ok(Property::Atom(s)),
            Some(Token::LParen) => {
                let e = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(e)
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

/// Parse a CTL property string.
///
/// # Errors
///
/// Returns a message describing the parse failure.
pub fn parse_prop(input: &str) -> Result<Property, String> {
    let toks = tokenize(input)?;
    let mut p = Parser { toks, pos: 0 };
    let prop = p.parse_expr()?;
    if p.pos != p.toks.len() {
        return Err(format!(
            "unexpected trailing tokens starting at `{:?}`",
            p.peek()
        ));
    }
    Ok(prop)
}

/// Lower a [`Property`] into a [`citreelo`] [`CTLFormula`].
fn lower(prop: &Property) -> CTLFormula<Ap> {
    use CTLFormula as Formula;
    match prop {
        Property::True => Formula::Leaf(CTLFormulaLeaf::True),
        Property::False => Formula::Leaf(CTLFormulaLeaf::False),
        Property::Atom(a) => Formula::Leaf(CTLFormulaLeaf::AtomicProp(Ap(a.clone()))),
        Property::Not(p) => Formula::Unary(UnaryCTLOperator::Not, Box::new(lower(p))),
        Property::And(a, b) => {
            Formula::Binary(BinaryCTLOperator::And, Box::new(lower(a)), Box::new(lower(b)))
        }
        Property::Or(a, b) => {
            Formula::Binary(BinaryCTLOperator::Or, Box::new(lower(a)), Box::new(lower(b)))
        }
        Property::Implies(a, b) => {
            Formula::Binary(BinaryCTLOperator::Imply, Box::new(lower(a)), Box::new(lower(b)))
        }
        Property::Ax(p) => Formula::Unary(UnaryCTLOperator::AX, Box::new(lower(p))),
        Property::Ex(p) => Formula::Unary(UnaryCTLOperator::EX, Box::new(lower(p))),
        Property::Ag(p) => Formula::Unary(UnaryCTLOperator::AG, Box::new(lower(p))),
        Property::Ef(p) => Formula::Unary(UnaryCTLOperator::EF, Box::new(lower(p))),
        Property::Af(p) => Formula::Unary(UnaryCTLOperator::AF, Box::new(lower(p))),
        Property::Eg(p) => Formula::Unary(UnaryCTLOperator::EG, Box::new(lower(p))),
        Property::Au(a, b) => {
            Formula::Binary(BinaryCTLOperator::AU, Box::new(lower(a)), Box::new(lower(b)))
        }
        Property::Eu(a, b) => {
            Formula::Binary(BinaryCTLOperator::EU, Box::new(lower(a)), Box::new(lower(b)))
        }
    }
}

/// Evaluate `prop` on every state, returning a satisfaction bitvector.
#[must_use]
pub fn check(ts: &TransitionSystem, prop: &Property) -> Vec<bool> {
    let kripke = to_kripke(ts);
    let checker = CtlModelChecker::new(&kripke);
    let sat = checker.get_sat_set(&lower(prop));
    (0..ts.n).map(|i| sat.contains(&i)).collect()
}

//! Token definitions for the SMOL lexer.

use crate::span::Span;

/// All tokens produced by the logos lexer (including trivia for error recovery).
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Abstract,
    Access,
    Adapts,
    Bool(bool), // true / false
    Class,
    Classify,
    Classifies,
    Construct,
    Context,
    Destroy,
    Do,
    Domain,
    Else,
    End,
    Extends,
    Hidden,
    If,
    Links,
    Main,
    New,
    Null,
    Override,
    Print,
    Retrieves,
    Return,
    Skip,
    Then,
    This,
    While,

    // Built-in types
    KwBoolean,
    KwDouble,
    KwInt,
    KwList,
    KwExplList,
    KwString,
    KwUnit,

    Rule,
    Member,
    Validate,
    Adapt,
    Simulate,
    Tick,
    Port,
    Breakpoint,
    Super,
    KwIn,
    KwOut,

    // Data connectors
    Influxdb,
    /// `SPARQL` connector keyword.
    KwSparql,
    /// `FMO` type keyword.
    KwFmo,

    // Identifiers & literals
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    /// A quoted string literal (contents already unescaped).
    StrLit(String),

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,    // ==
    NotEq, // !=
    Lt,
    LtEq,
    Gt,
    GtEq,
    Assign,   // =
    PlusPlus, // ++ (string concat)
    Bang,     // !
    And,      // &&
    Or,       // ||

    // Delimiters
    LParen,
    RParen,
    LAngle,   // < used in List<T>
    RAngle,   // >
    LBracket, // [ used in FMO[...]
    RBracket, // ]
    Comma,
    Dot,
    Semi,
    Colon,
    SemiColon, // ;

    // Special
    /// Error token produced by logos for unrecognised bytes.
    Error(char),
}

/// A spanned token: the token value plus its byte-offset range in the source.
#[derive(Debug, Clone)]
pub struct SpannedToken {
    pub kind: Token,
    pub span: Span,
}

impl SpannedToken {
    #[must_use]
    pub fn new(kind: Token, span: Span) -> Self {
        Self { kind, span }
    }
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::Abstract => write!(f, "abstract"),
            Token::Access => write!(f, "access"),
            Token::Adapts => write!(f, "adapts"),
            Token::Bool(b) => write!(f, "{b}"),
            Token::Class => write!(f, "class"),
            Token::Classify => write!(f, "classify"),
            Token::Classifies => write!(f, "classifies"),
            Token::Construct => write!(f, "construct"),
            Token::Context => write!(f, "context"),
            Token::Destroy => write!(f, "destroy"),
            Token::Do => write!(f, "do"),
            Token::Domain => write!(f, "domain"),
            Token::Else => write!(f, "else"),
            Token::End => write!(f, "end"),
            Token::Extends => write!(f, "extends"),
            Token::Hidden => write!(f, "hidden"),
            Token::If => write!(f, "if"),
            Token::Links => write!(f, "links"),
            Token::Main => write!(f, "main"),
            Token::New => write!(f, "new"),
            Token::Null => write!(f, "null"),
            Token::Override => write!(f, "override"),
            Token::Print => write!(f, "print"),
            Token::Retrieves => write!(f, "retrieves"),
            Token::Return => write!(f, "return"),
            Token::Skip => write!(f, "skip"),
            Token::Then => write!(f, "then"),
            Token::This => write!(f, "this"),
            Token::While => write!(f, "while"),
            Token::KwBoolean => write!(f, "Boolean"),
            Token::KwDouble => write!(f, "Double"),
            Token::KwInt => write!(f, "Int"),
            Token::KwList => write!(f, "List"),
            Token::KwExplList => write!(f, "ExplList"),
            Token::KwString => write!(f, "String"),
            Token::KwUnit => write!(f, "Unit"),
            Token::Influxdb => write!(f, "INFLUXDB"),
            Token::KwSparql => write!(f, "SPARQL"),
            Token::KwFmo => write!(f, "FMO"),
            Token::Rule => write!(f, "rule"),
            Token::Member => write!(f, "member"),
            Token::Validate => write!(f, "validate"),
            Token::Adapt => write!(f, "adapt"),
            Token::Simulate => write!(f, "simulate"),
            Token::Tick => write!(f, "tick"),
            Token::Port => write!(f, "port"),
            Token::Breakpoint => write!(f, "breakpoint"),
            Token::Super => write!(f, "super"),
            Token::KwIn => write!(f, "in"),
            Token::KwOut => write!(f, "out"),
            Token::Ident(s) => write!(f, "{s}"),
            Token::IntLit(v) => write!(f, "{v}"),
            Token::FloatLit(v) => write!(f, "{v}"),
            Token::StrLit(s) => write!(f, "\"{s}\""),
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Percent => write!(f, "%"),
            Token::Eq => write!(f, "=="),
            Token::NotEq => write!(f, "!="),
            Token::Lt | Token::LAngle => write!(f, "<"),
            Token::LtEq => write!(f, "<="),
            Token::Gt | Token::RAngle => write!(f, ">"),
            Token::GtEq => write!(f, ">="),
            Token::Assign => write!(f, "="),
            Token::PlusPlus => write!(f, "++"),
            Token::Bang => write!(f, "!"),
            Token::And => write!(f, "&&"),
            Token::Or => write!(f, "||"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::Comma => write!(f, ","),
            Token::Dot => write!(f, "."),
            Token::Semi | Token::SemiColon => write!(f, ";"),
            Token::Colon => write!(f, ":"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::Error(c) => write!(f, "unexpected `{c}`"),
        }
    }
}

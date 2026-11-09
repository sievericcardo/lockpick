//! logos-based lexer for SMOL source files.

use logos::Logos;
use smol_syntax::span::Span;
use smol_syntax::token::{SpannedToken, Token};

/// Run the logos lexer over `src` and return the full token stream.
///
/// Whitespace and comments are silently discarded.  Unrecognised characters
/// produce a [`Token::Error`] entry so the parser can report them.
#[must_use]
pub fn lex(src: &str) -> Vec<SpannedToken> {
    LexToken::lexer(src)
        .spanned()
        .filter_map(|(tok_result, range)| {
            let span = Span::new(range.start, range.end);
            let tok = match tok_result {
                Ok(LexToken::LineComment | LexToken::BlockComment) => return None,
                Ok(lt) => lt.into_token(src, span),
                Err(()) => {
                    let ch = src[range].chars().next().unwrap_or('\0');
                    Token::Error(ch)
                }
            };
            Some(SpannedToken::new(tok, span))
        })
        .collect()
}

// ─── Internal logos token enum ───────────────────────────────────────────── //

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
enum LexToken {
    // ── Trivia ────────────────────────────────────────────────────────────── //
    #[regex(r"//[^\n]*", allow_greedy = true)]
    LineComment,

    #[regex(r"/\*([^*]|\*[^/])*\*/")]
    BlockComment,

    // ── Keywords ──────────────────────────────────────────────────────────── //
    #[token("abstract")]
    Abstract,
    #[token("access")]
    Access,
    #[token("adapts")]
    Adapts,
    #[token("class")]
    Class,
    #[token("classifies")]
    Classifies,
    #[token("classify")]
    Classify,
    #[token("construct")]
    Construct,
    #[token("context")]
    Context,
    #[token("destroy")]
    Destroy,
    #[token("do")]
    Do,
    #[token("domain")]
    Domain,
    #[token("else")]
    Else,
    #[token("end")]
    End,
    #[token("extends")]
    Extends,
    #[token("false")]
    False,
    #[token("hidden")]
    Hidden,
    #[token("if")]
    If,
    #[token("links")]
    Links,
    #[token("main")]
    Main,
    #[token("new")]
    New,
    #[token("null")]
    Null,
    #[token("override")]
    Override,
    #[token("print")]
    Print,
    #[token("retrieves")]
    Retrieves,
    #[token("return")]
    Return,
    #[token("rule")]
    Rule,
    #[token("simulate")]
    Simulate,
    #[token("skip")]
    Skip,
    #[token("super")]
    Super,
    #[token("then")]
    Then,
    #[token("tick")]
    Tick,
    #[token("this")]
    This,
    #[token("true")]
    True,
    #[token("validate")]
    Validate,
    #[token("while")]
    While,

    #[token("adapt")]
    Adapt,
    #[token("breakpoint")]
    Breakpoint,
    #[token("member")]
    Member,
    #[token("out")]
    KwOut,
    #[token("in")]
    KwIn,
    #[token("port")]
    Port,

    // ── Built-in type keywords ───────────────────────────────────────────────── //
    #[token("Boolean")]
    KwBoolean,
    #[token("Double")]
    KwDouble,
    #[token("Int")]
    KwInt,
    #[token("List")]
    KwList,
    #[token("ExplList")]
    KwExplList,
    #[token("String")]
    KwString,
    #[token("Unit")]
    KwUnit,

    // ── Data connector keywords ────────────────────────────────────────────── //
    #[token("INFLUXDB")]
    Influxdb,
    #[token("SPARQL")]
    KwSparql,
    #[token("FMO")]
    KwFmo,

    // ── Literals ──────────────────────────────────────────────────────────── //
    #[regex(r"-?[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?")]
    FloatLit,

    #[regex(r"-?[0-9]+")]
    IntLit,

    /// Quoted string — supports multiline (needed for embedded SPARQL).
    #[regex(r#""([^"\\]|\\.)*""#)]
    StrLit,

    // ── Identifier ────────────────────────────────────────────────────────── //
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]
    Ident,

    // ── Operators ────────────────────────────────────────────────────────── //
    #[token("++")]
    PlusPlus,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("==")]
    Eq,
    #[token("!=")]
    NotEq,
    #[token("<=")]
    LtEq,
    #[token(">=")]
    GtEq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("=")]
    Assign,
    #[token("!")]
    Bang,
    #[token("&&")]
    And,
    #[token("||")]
    Or,

    // ── Delimiters ────────────────────────────────────────────────────────── //
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token(";")]
    SemiColon,
    #[token(":")]
    Colon,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
}

impl LexToken {
    fn into_token(self, src: &str, span: Span) -> Token {
        match self {
            LexToken::LineComment | LexToken::BlockComment => {
                unreachable!("trivia filtered before this call")
            }
            LexToken::Abstract => Token::Abstract,
            LexToken::Access => Token::Access,
            LexToken::Adapts => Token::Adapts,
            LexToken::Class => Token::Class,
            LexToken::Classifies => Token::Classifies,
            LexToken::Classify => Token::Classify,
            LexToken::Construct => Token::Construct,
            LexToken::Context => Token::Context,
            LexToken::Destroy => Token::Destroy,
            LexToken::Do => Token::Do,
            LexToken::Domain => Token::Domain,
            LexToken::Else => Token::Else,
            LexToken::End => Token::End,
            LexToken::Extends => Token::Extends,
            LexToken::False => Token::Bool(false),
            LexToken::Hidden => Token::Hidden,
            LexToken::If => Token::If,
            LexToken::Links => Token::Links,
            LexToken::Main => Token::Main,
            LexToken::New => Token::New,
            LexToken::Null => Token::Null,
            LexToken::Override => Token::Override,
            LexToken::Print => Token::Print,
            LexToken::Retrieves => Token::Retrieves,
            LexToken::Return => Token::Return,
            LexToken::Rule => Token::Rule,
            LexToken::Simulate => Token::Simulate,
            LexToken::Skip => Token::Skip,
            LexToken::Super => Token::Super,
            LexToken::Then => Token::Then,
            LexToken::Tick => Token::Tick,
            LexToken::This => Token::This,
            LexToken::True => Token::Bool(true),
            LexToken::Validate => Token::Validate,
            LexToken::While => Token::While,

            LexToken::Adapt => Token::Adapt,
            LexToken::Breakpoint => Token::Breakpoint,
            LexToken::Member => Token::Member,
            LexToken::KwOut => Token::KwOut,
            LexToken::KwIn => Token::KwIn,
            LexToken::Port => Token::Port,

            LexToken::KwBoolean => Token::KwBoolean,
            LexToken::KwDouble => Token::KwDouble,
            LexToken::KwInt => Token::KwInt,
            LexToken::KwList => Token::KwList,
            LexToken::KwExplList => Token::KwExplList,
            LexToken::KwString => Token::KwString,
            LexToken::KwUnit => Token::KwUnit,
            LexToken::Influxdb => Token::Influxdb,
            LexToken::KwSparql => Token::KwSparql,
            LexToken::KwFmo => Token::KwFmo,

            LexToken::IntLit => {
                let text = &src[span.start..span.end];
                Token::IntLit(text.parse().unwrap_or(0))
            }
            LexToken::FloatLit => {
                let text = &src[span.start..span.end];
                Token::FloatLit(text.parse().unwrap_or(0.0))
            }
            LexToken::StrLit => {
                // Strip surrounding quotes and unescape basic sequences.
                let inner = &src[span.start + 1..span.end - 1];
                Token::StrLit(unescape(inner))
            }
            LexToken::Ident => Token::Ident(src[span.start..span.end].to_owned()),

            LexToken::PlusPlus => Token::PlusPlus,
            LexToken::Plus => Token::Plus,
            LexToken::Minus => Token::Minus,
            LexToken::Star => Token::Star,
            LexToken::Slash => Token::Slash,
            LexToken::Percent => Token::Percent,
            LexToken::Eq => Token::Eq,
            LexToken::NotEq => Token::NotEq,
            LexToken::LtEq => Token::LtEq,
            LexToken::GtEq => Token::GtEq,
            LexToken::Lt => Token::Lt,
            LexToken::Gt => Token::Gt,
            LexToken::Assign => Token::Assign,
            LexToken::Bang => Token::Bang,
            LexToken::And => Token::And,
            LexToken::Or => Token::Or,

            LexToken::LParen => Token::LParen,
            LexToken::RParen => Token::RParen,
            LexToken::Comma => Token::Comma,
            LexToken::Dot => Token::Dot,
            LexToken::SemiColon => Token::SemiColon,
            LexToken::Colon => Token::Colon,
            LexToken::LBracket => Token::LBracket,
            LexToken::RBracket => Token::RBracket,
        }
    }
}

/// Minimal unescaping for SMOL string literals.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') | None => out.push('\\'),
                Some(c) => {
                    out.push('\\');
                    out.push(c);
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

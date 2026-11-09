//! Abstract syntax tree node types for SMOL programs.
//!
//! A SMOL program is a collection of *class declarations* followed by an
//! optional *main block*.  Multiple source files contribute to the same
//! program; the parser returns a [`File`] per source file and the checker
//! merges them.

use crate::span::Span;

// Type expressions

/// Direction of an FMO port (`in` or `out`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FmoDirection {
    In,
    Out,
}

/// A single port declaration inside an `FMO[...]` type.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FmoPort {
    pub direction: FmoDirection,
    pub ty: Type,
    pub name: String,
    pub span: Span,
}

/// A type expression as it appears in field declarations, method signatures,
/// or local variable declarations.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Type {
    /// Primitive `Int`
    Int,
    /// Primitive `Double`
    Double,
    /// Primitive `String`
    String,
    /// Primitive `Boolean`
    Boolean,
    /// Return type of procedures that produce no value.
    Unit,
    /// Generic list: `List<T>` (hidden by default — transient query result).
    List(Box<Type>),
    /// Explicit list: `ExplList<T>` (exposed model relation via RDF).
    ExplList(Box<Type>),
    /// Reference to a named user-defined class.
    Named(String),
    /// Generic named type: `ClassName<T1, T2>`
    Generic(String, Vec<Type>),
    /// FMU object type: `FMO[in Double speed, out Double position]`
    Fmo(Vec<FmoPort>),
    /// Nullable type: `Nullable<T>` — accepts a value of `T` or `null`.
    /// (Spelled as a generic so it parses with the fixed `smol.pest` grammar.)
    Nullable(Box<Type>),
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::Double => write!(f, "Double"),
            Type::String => write!(f, "String"),
            Type::Boolean => write!(f, "Boolean"),
            Type::Unit => write!(f, "Unit"),
            Type::List(t) => write!(f, "List<{t}>"),
            Type::ExplList(t) => write!(f, "ExplList<{t}>"),
            Type::Named(n) => write!(f, "{n}"),
            Type::Generic(n, params) => {
                write!(f, "{n}<")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ">")
            }
            Type::Fmo(ports) => {
                write!(f, "FMO[")?;
                for (i, p) in ports.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    let dir = match p.direction {
                        FmoDirection::In => "in",
                        FmoDirection::Out => "out",
                    };
                    write!(f, "{dir} {} {}", p.ty, p.name)?;
                }
                write!(f, "]")
            }
            Type::Nullable(inner) => write!(f, "Nullable<{inner}>"),
        }
    }
}

// Field modifiers

/// Optional modifier on a class field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FieldModifier {
    /// Default: no modifier (plain data field).
    None,
    /// `domain` — mirrored into the RDF knowledge graph.
    Domain,
    /// `hidden` — not exposed in the RDF layer.
    Hidden,
    /// `context` — twinning context pointer.
    Context,
}

/// A single field in a class parameter list.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FieldDecl {
    pub modifier: FieldModifier,
    pub ty: Type,
    pub name: String,
    pub span: Span,
}

// Method / parameter

/// A formal parameter in a method signature.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Param {
    pub ty: Type,
    pub name: String,
    pub span: Span,
}

/// Effect contract of a method (`pure` / `domain-reads` / `domain-writes` prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    /// No observable side effects.
    Pure,
    /// May read (but not write) domain state.
    DomainReads,
    /// May read and write domain state.
    DomainWrites,
}

/// Method flags.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct MethodFlags {
    pub is_abstract: bool,
    pub is_override: bool,
    /// `rule` modifier — method may only call other rule methods.
    pub is_rule: bool,
    /// `domain` modifier — method is exposed in the RDF knowledge graph.
    pub is_domain: bool,
    /// Effect contract from the `pure` / `domain-reads` / `domain-writes`
    /// prefix. `None` means no declared contract (legacy default).
    pub effect: Option<Effect>,
}

impl MethodFlags {
    /// Convenience: `true` iff the method is declared `pure`.
    #[must_use]
    pub fn is_pure(&self) -> bool {
        matches!(self.effect, Some(Effect::Pure))
    }
}

/// A method (or abstract method declaration).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MethodDecl {
    pub flags: MethodFlags,
    pub ret_type: Type,
    pub name: String,
    pub params: Vec<Param>,
    /// Classes mentioned in `adapts Foo, Bar`.
    pub adapts: Vec<String>,
    /// `requires <expr>` preconditions (Hoare triples / ODRL guards).
    /// Empty when no clause is present (legacy default).
    pub requires: Vec<Expr>,
    /// `ensures <expr>` postconditions. Empty when absent.
    pub ensures: Vec<Expr>,
    /// Body statements; empty for abstract methods.
    pub body: Vec<Stmt>,
    pub span: Span,
}

// Class annotations

/// `classifies "<IRI>"` annotation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClassifiesAnnotation {
    /// The IRI string between the angle-brackets in the source.
    /// E.g. `"ast:Healthy"`.
    pub iri: String,
    pub span: Span,
}

/// `retrieves "..."` annotation — SPARQL SELECT string.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RetrievesAnnotation {
    pub sparql: String,
    pub span: Span,
}

/// `links "..."` annotation — Turtle property list fragment.
/// May carry an optional guard expression: `links (expr) "turtle"`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LinksAnnotation {
    pub turtle: String,
    /// Optional guard expression for conditional links.
    pub guard: Option<Box<Expr>>,
    pub span: Span,
}

/// An initialized internal field in a class body: `[modifier] Type name = expr;`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InitFieldDecl {
    pub modifier: FieldModifier,
    pub ty: Type,
    pub name: String,
    pub init: Expr,
    pub span: Span,
}

// Class declaration

/// A full class or `abstract class` declaration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClassDecl {
    pub is_abstract: bool,
    /// `hidden class` — not exposed in the RDF layer.
    pub is_hidden: bool,
    pub name: String,
    /// Generic type parameters: `class Foo<A, B>`
    pub type_params: Vec<String>,
    pub extends: Option<String>,
    pub fields: Vec<FieldDecl>,
    /// Initialized internal fields: `Int count = 0;`
    pub init_fields: Vec<InitFieldDecl>,
    pub classifies: Option<ClassifiesAnnotation>,
    pub retrieves: Option<RetrievesAnnotation>,
    /// Possibly multiple `links` annotations (including conditional ones).
    pub links: Vec<LinksAnnotation>,
    pub methods: Vec<MethodDecl>,
    pub span: Span,
}

// Statements

/// A statement inside a method body or the `main` block.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Stmt {
    /// `Type var = expr;`
    VarDecl {
        ty: Type,
        name: String,
        init: Expr,
        span: Span,
    },
    /// `lhs = rhs;`
    Assign { lhs: Expr, rhs: Expr, span: Span },
    /// `expr;` — call or standalone expression.
    Expr { expr: Expr, span: Span },
    /// `return expr;` or bare `return;`
    Return { value: Option<Expr>, span: Span },
    /// `if cond then stmts [else stmts] end`
    If {
        cond: Expr,
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
        span: Span,
    },
    /// `while cond do stmts end`
    While {
        cond: Expr,
        /// Optional `decreases <expr>` termination measure.
        decreases: Option<Box<Expr>>,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `print(expr);`
    Print { value: Expr, span: Span },
    /// `destroy(expr);`
    Destroy { value: Expr, span: Span },
    /// `skip;` — no-operation statement.
    Skip { span: Span },
    /// `adapt(expr);` — trigger an adaptation.
    Adapt { expr: Expr, span: Span },
    /// `breakpoint;` — debug breakpoint (no runtime semantics).
    Breakpoint { span: Span },
}

// Expressions

/// An expression.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Expr {
    // Literals
    IntLit(i64, Span),
    FloatLit(f64, Span),
    StrLit(String, Span),
    BoolLit(bool, Span),
    Null(Span),

    // Variables / paths
    /// Plain identifier or `this`.
    Var(String, Span),
    /// `expr.field` or `expr.method(args)`
    Field {
        receiver: Box<Expr>,
        field: String,
        span: Span,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        args: Vec<Expr>,
        span: Span,
    },

    // Object creation
    /// `new ClassName(arg, ...)` or `new List<T>(args)`
    New {
        class: String,
        /// Generic type parameter, e.g. `BaseModel` in `new List<BaseModel>(...)`.
        ty_param: Option<Box<Type>>,
        args: Vec<Expr>,
        span: Span,
    },

    // SMOL builtins
    /// `construct("SPARQL" [, arg, ...])`
    Construct {
        sparql: String,
        /// Optional positional binding arguments (`%1`, `%2`, …).
        args: Vec<Expr>,
        span: Span,
    },
    /// `access("SPARQL" [, connector, args...])`
    Access {
        sparql: String,
        connector: Option<Box<Expr>>,
        args: Vec<Expr>,
        span: Span,
    },
    /// `classify(expr)`
    Classify {
        expr: Box<Expr>,
        span: Span,
    },
    /// `doubleToString(expr)` and other built-in functions.
    BuiltinCall {
        name: String,
        args: Vec<Expr>,
        span: Span,
    },

    // SMOL FMU / semantic built-ins
    /// `member("owl-class-expression")`
    Member {
        expr: String,
        span: Span,
    },
    /// `validate("turtle-fragment")`
    Validate {
        expr: String,
        span: Span,
    },
    /// `simulate("sparql" [, arg, ...])`
    Simulate {
        sparql: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// `fmu.port("name")` — access a named FMO port.
    Port {
        receiver: Box<Expr>,
        port: String,
        span: Span,
    },
    /// `super(args)` — call parent class method.
    Super {
        args: Vec<Expr>,
        span: Span,
    },

    // Binary operators
    BinOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },

    // Unary operators
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub fn span(&self) -> Span {
        match self {
            Expr::IntLit(_, s)
            | Expr::FloatLit(_, s)
            | Expr::StrLit(_, s)
            | Expr::BoolLit(_, s)
            | Expr::Null(s)
            | Expr::Var(_, s) => *s,
            Expr::Field { span, .. } => *span,
            Expr::MethodCall { span, .. } => *span,
            Expr::New { span, .. } => *span,
            Expr::Construct { span, .. } => *span,
            Expr::Access { span, .. } => *span,
            Expr::Classify { span, .. } => *span,
            Expr::BuiltinCall { span, .. } => *span,
            Expr::BinOp { span, .. } => *span,
            Expr::UnaryOp { span, .. } => *span,
            Expr::Member { span, .. } => *span,
            Expr::Validate { span, .. } => *span,
            Expr::Simulate { span, .. } => *span,
            Expr::Port { span, .. } => *span,
            Expr::Super { span, .. } => *span,
        }
    }
}

/// Binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Concat, // ++
}

/// Unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UnaryOp {
    Neg,
    Not,
}

// Top-level program

/// One parsed source file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct File {
    /// Absolute path to the source file.
    pub path: String,
    pub classes: Vec<ClassDecl>,
    /// Statements in the `main` block, if present.
    pub main: Option<Vec<Stmt>>,
}

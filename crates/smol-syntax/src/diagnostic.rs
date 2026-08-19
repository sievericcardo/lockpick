//! Diagnostic types emitted by all pipeline stages.

use crate::span::Span;

/// Severity of a diagnostic message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// A structured diagnostic produced by any verifier stage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code:     DiagnosticCode,
    pub message:  String,
    /// Source file path (relative to the manifest directory).
    pub file:     Option<String>,
    pub span:     Option<Span>,
    /// Optional secondary labels / notes.
    pub labels:   Vec<Label>,
}

impl Diagnostic {
    pub fn error(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            file:    None,
            span:    None,
            labels:  Vec::new(),
        }
    }

    pub fn warning(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            file:    None,
            span:    None,
            labels:  Vec::new(),
        }
    }

    #[must_use]
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    #[must_use]
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    #[must_use]
    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// A secondary label attached to a diagnostic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Label {
    pub message: String,
    pub file:    Option<String>,
    pub span:    Option<Span>,
}

impl Label {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            file:    None,
            span:    None,
        }
    }

    #[must_use]
    pub fn with_file_span(mut self, file: impl Into<String>, span: Span) -> Self {
        self.file = Some(file.into());
        self.span = Some(span);
        self
    }
}

/// Stable numeric codes for every diagnostic the verifier can emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u16)]
pub enum DiagnosticCode {
    // Parse errors
    UnexpectedToken           = 1001,
    UnclosedDelimiter         = 1002,
    UnexpectedEof             = 1003,

    // Type errors
    DuplicateClass            = 2001,
    CyclicInheritance         = 2002,
    UnknownClass              = 2003,
    TypeMismatch              = 2004,
    AbstractInstantiation     = 2005,
    MissingAbstractBody       = 2006,
    OverrideWithoutParent     = 2007,
    MissingReturnValue        = 2008,
    UnknownField              = 2009,
    UnknownVariable           = 2010,
    WrongArgCount             = 2011,
    DuplicateField            = 2012,
    ListFieldNotExplicit      = 2013,

    // Ontology errors
    OntologyLoadFailed        = 3001,
    OntologyInconsistent      = 3002,
    UnknownOntologyClass      = 3003,
    ClassifiesNotReachable    = 3004,
    SparqlSyntaxError         = 3005,
    SparqlArgCountMismatch    = 3006,
    SparqlFieldMismatch       = 3007,
    LinksPredicateInvalid     = 3008,
    ShaclViolation            = 3009,
    UnknownPrefix             = 3010,
    RetrievesNotSelect        = 3011,
    LinkTurtleInvalid         = 3012,

    // Manifest / IO errors
    ManifestNotFound          = 4001,
    ManifestParseFailed       = 4002,
    GlobExpansionFailed       = 4003,
    FileReadFailed            = 4004,

    // Model errors (formal verifier)
    ContextFieldInvalid       = 5001,
    ClassifyAmbiguous         = 5002,
    ClassifiesInvalid         = 5003,
    LinksTypeMismatch         = 5004,
    MemberExpressionInvalid   = 5005,
    FmoPortUnknown            = 5006,
    StateNotReachable         = 5007,
    AdaptationDeadlock        = 5008,
    InvariantViolation        = 5009,
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "E{:04}", *self as u16)
    }
}

//! Source-span types used throughout the SMOL pipeline.

/// A byte-offset range within a single source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct Span {
    pub start: usize,
    pub end:   usize,
}

impl Span {
    #[inline]
    #[must_use]
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[inline]
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end:   self.end.max(other.end),
        }
    }
}

impl From<std::ops::Range<usize>> for Span {
    fn from(r: std::ops::Range<usize>) -> Self {
        Self::new(r.start, r.end)
    }
}

impl From<Span> for std::ops::Range<usize> {
    fn from(s: Span) -> Self {
        s.start..s.end
    }
}

impl chumsky::span::Span for Span {
    type Context = ();
    type Offset  = usize;

    fn new(_ctx: (), range: std::ops::Range<usize>) -> Self {
        Self::new(range.start, range.end)
    }
    fn context(&self) {  }
    fn start(&self)   -> usize { self.start }
    fn end(&self)     -> usize { self.end }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

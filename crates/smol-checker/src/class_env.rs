//! `ClassEnv` — the resolved environment of all class declarations.
//!
//! Built by the declaration pass from the merged AST of all source files.
//! Stored in a plain `HashMap` so it can be shared cheaply by the body pass.
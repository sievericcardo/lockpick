//! Body pass: type-check method bodies and the `main` block.
//!
//! Uses a simple scope-stack `VarEnv` (no `salsa` persistence here — the
//! incremental layer is at the declaration pass).

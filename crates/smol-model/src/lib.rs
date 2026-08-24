//! `smol-model` — the formal (model) verification layer for SMOL programs.
//!
//! Extends the type checker with checks over the *digital-twin model*:
//! twinning (`context`) validity, `classifies` state structure, `classify()`
//! ambiguity, `links`/`member`/`validate`/FMO well-formedness, adaptation-loop
//! termination, state reachability, and user-supplied CTL properties verified
//! with an explicit-state model checker.
//!
//! The main entry point is [`check_model`].

pub mod checks;
pub mod ctl;
pub mod kripke;
pub mod model;

pub use checks::{Property, check_model};

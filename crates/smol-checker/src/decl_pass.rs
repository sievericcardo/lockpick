//! Declaration pass: build the `ClassEnv` from all parsed files.
//!
//! Errors detected:
//! - Duplicate class name
//! - Unknown superclass
//! - Cyclic inheritance
//! - Abstract method with body / non-abstract without body
//! - `override` without a parent method
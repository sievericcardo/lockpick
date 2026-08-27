//! Bridge between the extracted SMOL transition system (`Ts`) and the
//! [`citreelo`] BDD-based CTL model checker.
//!
//! [`citreelo`] operates on a [`KripkeStructure`] whose states are labelled
//! with values in a user-chosen domain, over which atomic propositions are
//! evaluated.  In the SMOL model, each state carries a *set* of atomic
//! propositions (state-class names, structural markers, guard conditions), so
//! the domain is a `HashSet<String>`.
use std::collections::HashSet;

use crate::model::TransitionSystem;
use citreelo::kripke::{AtomicProposition, KripkeState, KripkeStructure};

/// An atomic proposition: the name of a state-class or a structural marker.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ap(pub String);

impl AtomicProposition<HashSet<String>> for Ap {
    fn is_satisfied_on_state_domain(&self, state_domain: &HashSet<String>) -> bool {
        state_domain.contains(&self.0)
    }
}

/// Build the [`citreelo`] Kripke structure for `ts`.
///
/// State indices are preserved (`TransitionSystem` state `i` becomes Kripke state `i`), so
/// satisfaction sets returned by [`CtlModelChecker`](citreelo::solve::CtlModelChecker)
/// line up with `TransitionSystem` indices.
///
/// `citreelo` requires the transition relation to be total (no deadlock state).
/// `build_transition_system` only ever produces an empty successor list for the unclassified
/// initial state of an *unadapted* root, which is given an explicit self-loop
/// here — the standard CTL encoding of a terminating state.
#[must_use]
pub fn to_kripke(ts: &TransitionSystem) -> KripkeStructure<HashSet<String>> {
    let states: Vec<KripkeState<HashSet<String>>> = ts
        .successors
        .iter()
        .enumerate()
        .map(|(i, succ)| {
            let mut targets = succ.clone();
            if targets.is_empty() {
                targets.push(i);
            }
            KripkeState::new(ts.labels[i].clone(), targets)
        })
        .collect();

    KripkeStructure::new(states).expect("transition system must be well-formed for citreelo")
}

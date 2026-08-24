//! Bridge between the extracted SMOL transition system (`Ts`) and the
//! [`citreelo`] BDD-based CTL model checker.
//!
//! [`citreelo`] operates on a [`KripkeStructure`] whose states are labelled
//! with values in a user-chosen domain, over which atomic propositions are
//! evaluated.  In the SMOL model, an atomic proposition is simply a state-class
//! name that holds exactly in the state of that name, so the domain is the
//! state name (`String`, empty for unclassified initial states).
use crate::model::TransitionSystem;
use citreelo::kripke::{AtomicProposition, KripkeState, KripkeStructure};

/// An atomic proposition: the name of a state class.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ap(pub String);

impl AtomicProposition<String> for Ap {
    fn is_satisfied_on_state_domain(&self, state_domain: &String) -> bool {
        state_domain == &self.0
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
pub fn to_kripke(ts: &TransitionSystem) -> KripkeStructure<String> {
    let states: Vec<KripkeState<String>> = ts
        .successors
        .iter()
        .enumerate()
        .map(|(i, succ)| {
            let mut targets = succ.clone();
            if targets.is_empty() {
                targets.push(i);
            }
            KripkeState::new(ts.names[i].clone(), targets)
        })
        .collect();

    KripkeStructure::new(states).expect("transition system must be well-formed for citreelo")
}

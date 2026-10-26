//! Physical outcomes versus application knowledge (Phase 9).
//!
//! An operation that was *submitted* but whose answer never reached the
//! application has an outcome the application does not know. The model keeps
//! the two apart:
//!
//! * the **physical** outcome is what the database did (`World`); it is
//!   reflected in the store like any other commit or abort;
//! * the **knowledge** of the application is what it can conclude from what
//!   it observed (`OutcomeKnowledge`): after a lost acknowledgment, both
//!   physical outcomes that the semantics allow remain possible.
//!
//! The explorer treats each legal physical outcome as its own transition, so
//! a bounded verification quantifies over every modeled alternative. The
//! record kept in the state (`Ambiguity`) is part of the canonical encoding:
//! two states with the same rows but a different history of unresolved
//! outcomes are never merged (see `docs/phase9/EPISTEMIC-SEMANTICS.md`).

use serde::{Deserialize, Serialize};

/// What PostgreSQL did with an operation whose answer was lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum World {
    /// The server executed (and, for COMMIT or an autocommit statement,
    /// committed) the operation.
    Committed,
    /// The server never executed it, or rolled it back.
    NotCommitted,
}

impl World {
    pub fn label(self) -> &'static str {
        match self {
            Self::Committed => "committed",
            Self::NotCommitted => "not committed",
        }
    }
}

/// A set of worlds (at most two).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct OutcomeSet(u8);

impl OutcomeSet {
    pub const BOTH: Self = Self(0b11);

    pub fn single(w: World) -> Self {
        Self(match w {
            World::Committed => 1,
            World::NotCommitted => 2,
        })
    }

    pub fn contains(self, w: World) -> bool {
        self.0 & Self::single(w).0 != 0
    }

    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn worlds(self) -> Vec<World> {
        [World::Committed, World::NotCommitted]
            .into_iter()
            .filter(|w| self.contains(*w))
            .collect()
    }
}

impl Serialize for OutcomeSet {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.worlds().serialize(s)
    }
}

/// The operation whose outcome was lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InFlightOp {
    /// An explicit COMMIT of an open transaction.
    Commit,
    /// A write statement outside a transaction (it commits by itself), on
    /// the table with this schema index.
    Autocommit(u16),
}

/// How the acknowledgment was lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Via {
    /// The connection broke while the process lived: the application got a
    /// connection error and went on.
    ConnectionLost,
    /// The process died with the operation in flight.
    ProcessCrash,
}

/// One unresolved outcome, as kept in the state of the actor that submitted
/// the operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ambiguity {
    /// Identity within the actor (the orphan's sequence number).
    pub seq: u32,
    pub op: InFlightOp,
    /// The physical outcome in this execution. `Committed` means the
    /// operation reached the server and takes effect when the server
    /// completes it (it may not have completed yet: see
    /// `State::orphans`); it becomes `NotCommitted` if the server then
    /// rejects it (a serialization failure, a constraint violation).
    pub world: World,
    /// Every outcome the semantics allowed at that point (what the
    /// application cannot exclude): the explorer visits one transition per
    /// member.
    pub possible: OutcomeSet,
    /// Execution (attempt) of the actor that submitted the operation.
    pub attempt: u32,
    pub via: Via,
}

/// What an application can conclude about one of its operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "knowledge")]
pub enum OutcomeKnowledge {
    KnownCommitted,
    KnownAborted,
    KnownFailed,
    Ambiguous { possible_outcomes: OutcomeSet },
}

impl OutcomeKnowledge {
    pub fn of(a: &Ambiguity) -> Self {
        Self::Ambiguous {
            possible_outcomes: a.possible,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::KnownCommitted => "known committed".to_owned(),
            Self::KnownAborted => "known aborted".to_owned(),
            Self::KnownFailed => "known failed".to_owned(),
            Self::Ambiguous { possible_outcomes } => format!(
                "ambiguous: {}",
                possible_outcomes
                    .worlds()
                    .iter()
                    .map(|w| w.label())
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_sets_are_sets_of_worlds() {
        let both = OutcomeSet::BOTH;
        assert_eq!(both.len(), 2);
        assert!(both.contains(World::Committed) && both.contains(World::NotCommitted));
        let one = OutcomeSet::single(World::Committed);
        assert!(one.contains(World::Committed) && !one.contains(World::NotCommitted));
        assert_eq!(one.worlds(), vec![World::Committed]);
        assert!(OutcomeSet::default().is_empty());
    }

    #[test]
    fn ambiguous_knowledge_names_every_possible_outcome() {
        let a = Ambiguity {
            seq: 0,
            op: InFlightOp::Commit,
            world: World::Committed,
            possible: OutcomeSet::BOTH,
            attempt: 0,
            via: Via::ProcessCrash,
        };
        assert_eq!(
            OutcomeKnowledge::of(&a).label(),
            "ambiguous: committed or not committed"
        );
    }
}

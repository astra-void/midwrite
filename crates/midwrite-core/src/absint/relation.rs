//! Relationships between two groups of effects, from value provenance.
//!
//! See `docs/architecture/value-analysis.md`. A relationship is evidence for
//! review, never a reason to suppress or demote a finding: two effects that
//! share a value may still be unrelated in the business sense, and two
//! effects without a shared value may still belong to one invariant.

use serde::{Deserialize, Serialize};

use super::value::{Atom, Provenance};

/// How two groups of effects are related by the values their arguments
/// carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RelationKind {
    /// The second group's arguments contain (exactly) the result of an
    /// effect of the first group, or both use the same value exactly.
    ProvenShared,
    /// A value is shared, but only inside a computation (a template, an
    /// arithmetic expression) or through a joined value.
    PossiblyRelated,
    /// Every argument value of both groups is known and nothing is shared.
    /// Not a proof of independence.
    NoRelationEstablished,
    /// The operations provably concern different resources. Reserved: never
    /// produced, because disjointness of database rows cannot be
    /// established without modeling predicates.
    ProvenDisjoint,
    /// Some argument value is unknown, so nothing can be concluded.
    Unknown,
}

/// One shared value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedValue {
    /// `derivedFromResult` or `sameValue`.
    pub kind: String,
    /// The value in the first operation's arguments (or the first
    /// operation's result).
    pub first: String,
    /// The value in the second operation's arguments.
    pub second: String,
    /// Both sides use it as an argument value or a property of one (not
    /// only inside a computation).
    pub exact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueRelation {
    pub status: RelationKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared: Vec<SharedValue>,
}

/// One effect of a group: its id (for `EffectResult` atoms) and the
/// provenance of its arguments, if the frontend recorded any.
#[derive(Clone, Copy, Debug)]
pub struct Member<'a> {
    pub effect: u32,
    pub provenance: Option<&'a Provenance>,
}

/// Relate `first` (performed first) and `second`.
pub fn relate(first: &[Member<'_>], second: &[Member<'_>]) -> ValueRelation {
    let mut shared: Vec<SharedValue> = Vec::new();
    let mut unknown = false;
    for member in first.iter().chain(second) {
        match member.provenance {
            None => unknown = true,
            Some(provenance) => unknown |= provenance.unknown,
        }
    }
    let results: Vec<u32> = first.iter().map(|m| m.effect).collect();
    for b in second.iter().filter_map(|m| m.provenance) {
        for input in &b.inputs {
            if let Atom::EffectResult { effect } = input.atom.root()
                && results.contains(effect)
            {
                push(
                    &mut shared,
                    SharedValue {
                        kind: "derivedFromResult".to_owned(),
                        first: input.atom.root().to_string(),
                        second: input.atom.to_string(),
                        exact: input.exact,
                    },
                );
            }
        }
        for a in first.iter().filter_map(|m| m.provenance) {
            for x in &a.inputs {
                for y in &b.inputs {
                    if x.atom.is_within(&y.atom) || y.atom.is_within(&x.atom) {
                        push(
                            &mut shared,
                            SharedValue {
                                kind: "sameValue".to_owned(),
                                first: x.atom.to_string(),
                                second: y.atom.to_string(),
                                exact: x.exact && y.exact,
                            },
                        );
                    }
                }
            }
        }
    }
    shared.sort();
    let status = if shared.iter().any(|s| s.exact) {
        RelationKind::ProvenShared
    } else if !shared.is_empty() {
        RelationKind::PossiblyRelated
    } else if unknown {
        RelationKind::Unknown
    } else {
        RelationKind::NoRelationEstablished
    };
    ValueRelation { status, shared }
}

fn push(shared: &mut Vec<SharedValue>, value: SharedValue) {
    if !shared.contains(&value) {
        shared.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::absint::value::{AbsValue, Input};

    fn prov(atoms: &[(Atom, bool)], unknown: bool) -> Provenance {
        Provenance {
            inputs: atoms
                .iter()
                .map(|(atom, exact)| Input {
                    atom: atom.clone(),
                    exact: *exact,
                })
                .collect(),
            unknown,
        }
    }

    fn field(atom: Atom, name: &str) -> Atom {
        AbsValue::atom(atom).field(name).single().cloned().unwrap()
    }

    #[test]
    fn result_flowing_into_the_second_effect_is_proven_shared() {
        let a = prov(&[(Atom::Param { index: 0 }, true)], false);
        let b = prov(
            &[(field(Atom::EffectResult { effect: 1 }, "id"), true)],
            false,
        );
        let relation = relate(
            &[Member {
                effect: 1,
                provenance: Some(&a),
            }],
            &[Member {
                effect: 2,
                provenance: Some(&b),
            }],
        );
        assert_eq!(relation.status, RelationKind::ProvenShared);
        assert_eq!(relation.shared[0].kind, "derivedFromResult");
    }

    #[test]
    fn derived_values_are_only_possibly_related() {
        let a = prov(&[(field(Atom::Param { index: 0 }, "id"), true)], false);
        let b = prov(&[(field(Atom::Param { index: 0 }, "id"), false)], false);
        let members = |p| {
            [Member {
                effect: 0,
                provenance: Some(p),
            }]
        };
        assert_eq!(
            relate(&members(&a), &members(&b)).status,
            RelationKind::PossiblyRelated
        );
    }

    #[test]
    fn different_names_never_mean_disjoint() {
        let a = prov(&[(Atom::Param { index: 0 }, true)], false);
        let b = prov(&[(Atom::Param { index: 1 }, true)], false);
        let first = [Member {
            effect: 0,
            provenance: Some(&a),
        }];
        let second = [Member {
            effect: 1,
            provenance: Some(&b),
        }];
        assert_eq!(
            relate(&first, &second).status,
            RelationKind::NoRelationEstablished
        );
        let c = prov(&[(Atom::Param { index: 1 }, true)], true);
        let unknown = [Member {
            effect: 1,
            provenance: Some(&c),
        }];
        assert_eq!(relate(&first, &unknown).status, RelationKind::Unknown);
        let missing = [Member {
            effect: 1,
            provenance: None,
        }];
        assert_eq!(relate(&first, &missing).status, RelationKind::Unknown);
    }
}

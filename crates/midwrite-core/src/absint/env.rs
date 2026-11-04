//! Abstract environments: binding → abstract value.
//!
//! Keys are semantic binding identities chosen by the frontend (Oxc symbol
//! ids), never source names, so shadowed or same-named bindings stay
//! distinct. A binding absent from an environment has not been assigned on
//! that path (it is declared later or out of scope); joins keep the value
//! of the side that has it.

use std::collections::BTreeMap;

use super::value::{AbsValue, Joined};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Env<K: Ord> {
    bindings: BTreeMap<K, AbsValue>,
}

impl<K: Ord> Default for Env<K> {
    fn default() -> Self {
        Self {
            bindings: BTreeMap::new(),
        }
    }
}

/// What a join of environments did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EnvJoin {
    pub changed: bool,
    /// Bindings widened to `Top` by the join.
    pub widened: usize,
}

impl<K: Ord + Clone> Env<K> {
    pub fn get(&self, key: &K) -> Option<&AbsValue> {
        self.bindings.get(key)
    }

    /// Strong update (the binding is a single variable).
    pub fn set(&mut self, key: K, value: AbsValue) {
        self.bindings.insert(key, value);
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &AbsValue)> {
        self.bindings.iter()
    }

    /// `self := self ⊔ other`, pointwise.
    pub fn join(&mut self, other: &Env<K>) -> EnvJoin {
        let mut out = EnvJoin::default();
        for (key, theirs) in &other.bindings {
            match self.bindings.get_mut(key) {
                Some(mine) => match mine.join(theirs) {
                    Joined::Unchanged => {}
                    Joined::Grew => out.changed = true,
                    Joined::Widened => {
                        out.changed = true;
                        out.widened += 1;
                    }
                },
                None => {
                    self.bindings.insert(key.clone(), theirs.clone());
                    out.changed = true;
                }
            }
        }
        out
    }

    /// `self ⊑ other`, pointwise (a binding absent from `other` must be
    /// absent from `self`).
    pub fn leq(&self, other: &Env<K>) -> bool {
        self.bindings.iter().all(|(key, value)| {
            other
                .bindings
                .get(key)
                .is_some_and(|theirs| value.leq(theirs))
        })
    }

    /// `self ⊑ other` on the bindings `other` has: bindings only `self`
    /// has (declared after `other`'s point, such as inside a loop body that
    /// re-declares them on every iteration) are ignored.
    pub fn leq_on_existing(&self, other: &Env<K>) -> bool {
        self.bindings.iter().all(|(key, value)| {
            other
                .bindings
                .get(key)
                .is_none_or(|theirs| value.leq(theirs))
        })
    }

    /// Widening at a loop head: every binding whose value differs between
    /// `previous` and `self` becomes `Top`. Returns how many were widened.
    pub fn widen_from(&mut self, previous: &Env<K>) -> usize {
        let mut widened = 0;
        for (key, value) in self.bindings.iter_mut() {
            let unchanged = previous.bindings.get(key).is_some_and(|old| old == value);
            if !unchanged && !value.is_top() {
                *value = AbsValue::top();
                widened += 1;
            }
        }
        widened
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::absint::value::Atom;

    fn v(index: u32) -> AbsValue {
        AbsValue::atom(Atom::Param { index })
    }

    #[test]
    fn join_is_pointwise_and_keeps_one_sided_bindings() {
        let mut a: Env<u32> = Env::default();
        a.set(1, v(0));
        let mut b = a.clone();
        b.set(1, v(1));
        b.set(2, v(2));
        let joined = a.join(&b);
        assert!(joined.changed);
        assert_eq!(a.get(&1).unwrap().atoms().len(), 2);
        assert_eq!(a.get(&2), Some(&v(2)));
        assert!(b.leq(&a));
        assert!(!a.join(&b).changed);
    }

    #[test]
    fn widening_sends_changed_bindings_to_top() {
        let mut before: Env<u32> = Env::default();
        before.set(1, v(0));
        before.set(2, v(5));
        let mut after = before.clone();
        after.set(1, v(1));
        assert_eq!(after.widen_from(&before), 1);
        assert!(after.get(&1).unwrap().is_top());
        assert_eq!(after.get(&2), Some(&v(5)));
    }
}

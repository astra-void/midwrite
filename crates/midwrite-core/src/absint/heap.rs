//! A bounded heap of abstract objects, one per allocation site.
//!
//! See `docs/phase5/HEAP-AND-ALIASES.md`. Objects are identified by the
//! allocation site that created them (an object or array literal, a `new`
//! expression) within one function. Property writes are *strong* (replace
//! the property) only when the target is exactly one non-summary object;
//! otherwise they are *weak* (join). Objects that escape into code the
//! analysis does not see answer every read with `Top`.

use std::collections::BTreeMap;

use super::value::{AbsValue, Atom, Joined, MAX_FIELD_DEPTH, MAX_RECORD_PROPS};

/// What an abstract object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectKind {
    /// An object literal.
    Plain,
    /// An array literal (elements are properties `"0"`, `"1"`, …).
    Array,
    /// `new C(...)` of class `class` (frontend numbering).
    Instance { class: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    pub kind: ObjectKind,
    pub props: BTreeMap<Box<str>, AbsValue>,
    /// May have properties other than `props` (spread, computed write).
    pub open: bool,
    /// Reachable from code the analysis does not see: may have been
    /// changed arbitrarily.
    pub escaped: bool,
    /// Stands for several concrete objects (allocated repeatedly on one
    /// path, i.e. in a loop): only weak updates.
    pub summary: bool,
}

/// Counters of heap operations (performance and precision evidence).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapStats {
    pub strong_updates: usize,
    pub weak_updates: usize,
    pub escapes: usize,
    /// Writes through a receiver that is not a known object (a parameter,
    /// an unknown value): nothing can be updated.
    pub unknown_target_writes: usize,
}

/// The objects of one function, keyed by allocation site.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heap {
    objects: BTreeMap<u32, Object>,
    pub stats: HeapStats,
}

impl Heap {
    /// The object allocated at `site`. Evaluating the same site again on
    /// the same path turns it into a summary object (a loop).
    pub fn allocate(&mut self, site: u32, kind: ObjectKind) -> Atom {
        match self.objects.get_mut(&site) {
            Some(object) => {
                object.summary = true;
                object.kind = kind;
            }
            None => {
                self.objects.insert(
                    site,
                    Object {
                        kind,
                        props: BTreeMap::new(),
                        open: false,
                        escaped: false,
                        summary: false,
                    },
                );
            }
        }
        Atom::Object { id: site }
    }

    pub fn get(&self, site: u32) -> Option<&Object> {
        self.objects.get(&site)
    }

    /// Initialize a property of a freshly allocated object (a literal's own
    /// property): always a strong update unless the object is a summary.
    pub fn init(&mut self, site: u32, name: &str, value: AbsValue) {
        if let Some(object) = self.objects.get_mut(&site) {
            if object.summary {
                object
                    .props
                    .entry(name.into())
                    .and_modify(|v| {
                        v.join(&value);
                    })
                    .or_insert(value);
            } else {
                object.props.insert(name.into(), value);
            }
        }
    }

    /// Mark an allocated object open (it may hold unlisted properties).
    pub fn open(&mut self, site: u32) {
        if let Some(object) = self.objects.get_mut(&site) {
            object.open = true;
        }
    }

    /// Property `name` of `value`.
    pub fn read(&self, value: &AbsValue, name: &str) -> AbsValue {
        if value.is_top() {
            return AbsValue::top();
        }
        let mut out = AbsValue::bottom();
        for atom in value.atoms() {
            let part = match atom {
                Atom::Object { id } => match self.objects.get(id) {
                    Some(object) if object.escaped => AbsValue::top(),
                    Some(object) => match object.props.get(name) {
                        Some(value) => value.clone(),
                        // Absent: prototype properties and open objects
                        // are not modeled; never `undefined`.
                        _ => AbsValue::top(),
                    },
                    None => AbsValue::top(),
                },
                other => other.field(name),
            };
            if out.join(&part) == Joined::Widened {
                break;
            }
        }
        out
    }

    /// `value.name = new`: strong when `value` is exactly one non-summary
    /// object, weak (join) for each object otherwise.
    pub fn write(&mut self, target: &AbsValue, name: &str, new: AbsValue) {
        if target.is_top() {
            self.stats.unknown_target_writes += 1;
            // The written value may now be reachable from anywhere.
            self.escape(&new);
            return;
        }
        let strong = match target.single() {
            Some(Atom::Object { id }) => self.objects.get(id).is_some_and(|o| !o.summary),
            _ => false,
        };
        let mut unknown = false;
        let mut escaped_target = false;
        for atom in target.atoms() {
            match atom {
                Atom::Object { id } => {
                    if let Some(object) = self.objects.get_mut(id) {
                        escaped_target |= object.escaped;
                        if strong {
                            self.stats.strong_updates += 1;
                            object.props.insert(name.into(), new.clone());
                        } else {
                            self.stats.weak_updates += 1;
                            object
                                .props
                                .entry(name.into())
                                .and_modify(|v| {
                                    v.join(&new);
                                })
                                .or_insert_with(|| new.clone());
                        }
                    }
                }
                _ => unknown = true,
            }
        }
        if unknown {
            self.stats.unknown_target_writes += 1;
        }
        if unknown || escaped_target {
            self.escape(&new);
        }
    }

    /// `target[unknown] = …`: every object `target` may be loses what is
    /// known about its properties; anything else is an unknown target.
    pub fn write_unknown(&mut self, target: &AbsValue) {
        let mut any_object = false;
        for atom in target.atoms() {
            if let Atom::Object { id } = atom
                && let Some(object) = self.objects.get_mut(id)
            {
                any_object = true;
                object.open = true;
                for value in object.props.values_mut() {
                    *value = AbsValue::top();
                }
                self.stats.weak_updates += 1;
            }
        }
        if !any_object {
            self.stats.unknown_target_writes += 1;
        }
    }

    /// Mark every object escaped (used when a fixpoint budget runs out).
    pub fn havoc(&mut self) {
        for object in self.objects.values_mut() {
            if !object.escaped {
                object.escaped = true;
                self.stats.escapes += 1;
            }
        }
    }

    /// `self ⊑ other`: every object of `self` exists in `other` with
    /// larger properties and flags.
    pub fn leq(&self, other: &Heap) -> bool {
        self.objects.iter().all(|(site, mine)| {
            other.objects.get(site).is_some_and(|theirs| {
                (theirs.escaped || !mine.escaped)
                    && (theirs.open || !mine.open)
                    && (theirs.summary || !mine.summary)
                    && (theirs.escaped
                        || mine.props.iter().all(|(name, value)| {
                            theirs.props.get(name).is_some_and(|t| value.leq(t))
                        }))
            })
        })
    }

    /// `self ⊑ other` on the objects `other` has (objects allocated later
    /// are ignored; they can only matter through values of existing
    /// bindings or objects, which are compared).
    pub fn leq_on_existing(&self, other: &Heap) -> bool {
        self.objects
            .iter()
            .all(|(site, mine)| match other.objects.get(site) {
                None => true,
                Some(theirs) => {
                    (theirs.escaped || !mine.escaped)
                        && (theirs.open || !mine.open)
                        && (theirs.summary || !mine.summary)
                        && (theirs.escaped
                            || mine.props.iter().all(|(name, value)| {
                                theirs.props.get(name).is_some_and(|t| value.leq(t))
                            }))
                }
            })
    }

    /// Widening at a loop head: objects that differ from `previous` lose
    /// their properties (open, every property `Top`). Returns how many.
    pub fn widen_from(&mut self, previous: &Heap) -> usize {
        let mut widened = 0;
        for (site, object) in self.objects.iter_mut() {
            let unchanged = previous.objects.get(site).is_some_and(|old| old == object);
            if !unchanged && !object.escaped {
                object.open = true;
                for value in object.props.values_mut() {
                    *value = AbsValue::top();
                }
                widened += 1;
            }
        }
        widened
    }

    /// Mark every object reachable from `value` escaped.
    pub fn escape(&mut self, value: &AbsValue) {
        let mut work: Vec<u32> = value
            .atoms()
            .iter()
            .filter_map(|atom| match atom {
                Atom::Object { id } => Some(*id),
                _ => None,
            })
            .collect();
        while let Some(id) = work.pop() {
            let Some(object) = self.objects.get_mut(&id) else {
                continue;
            };
            if object.escaped {
                continue;
            }
            object.escaped = true;
            self.stats.escapes += 1;
            for value in object.props.values() {
                for atom in value.atoms() {
                    if let Atom::Object { id } = atom {
                        work.push(*id);
                    }
                }
            }
        }
    }

    /// `self := self ⊔ other` (pointwise on objects; flags are or-ed).
    pub fn join(&mut self, other: &Heap) -> bool {
        let mut changed = false;
        for (site, theirs) in &other.objects {
            match self.objects.get_mut(site) {
                None => {
                    self.objects.insert(*site, theirs.clone());
                    changed = true;
                }
                Some(mine) => {
                    for (name, value) in &theirs.props {
                        match mine.props.get_mut(name) {
                            Some(existing) => changed |= existing.join(value).changed(),
                            None => {
                                // Absent on one side: the property may be
                                // missing there (an `undefined` read).
                                let mut joined = value.clone();
                                joined.join(&AbsValue::lit(crate::ir::Literal::Undefined));
                                mine.props.insert(name.clone(), joined);
                                changed = true;
                            }
                        }
                    }
                    for (name, value) in mine.props.iter_mut() {
                        if !theirs.props.contains_key(name) {
                            changed |= value
                                .join(&AbsValue::lit(crate::ir::Literal::Undefined))
                                .changed();
                        }
                    }
                    let flags = (mine.open, mine.escaped, mine.summary);
                    mine.open |= theirs.open;
                    mine.escaped |= theirs.escaped;
                    mine.summary |= theirs.summary;
                    changed |= flags != (mine.open, mine.escaped, mine.summary);
                }
            }
        }
        changed
    }

    /// The value with every heap object replaced by a [`Atom::Record`] of
    /// its properties, so that it means something outside this function
    /// (return values, effect provenance). Escaped and summary objects
    /// become `Top`.
    pub fn export(&self, value: &AbsValue) -> AbsValue {
        self.export_depth(value, 0)
    }

    fn export_depth(&self, value: &AbsValue, depth: usize) -> AbsValue {
        if !value
            .atoms()
            .iter()
            .any(|atom| matches!(atom, Atom::Object { .. }))
        {
            return value.clone();
        }
        let mut out = AbsValue::bottom();
        for atom in value.atoms() {
            let part = match atom {
                Atom::Object { id } => match self.objects.get(id) {
                    Some(object)
                        if !object.escaped
                            && !object.summary
                            && depth < MAX_FIELD_DEPTH
                            && object.props.len() <= MAX_RECORD_PROPS =>
                    {
                        AbsValue::atom(Atom::Record {
                            props: object
                                .props
                                .iter()
                                .map(|(name, value)| {
                                    (name.clone(), self.export_depth(value, depth + 1))
                                })
                                .collect(),
                            open: object.open,
                        })
                    }
                    _ => AbsValue::top(),
                },
                other => AbsValue::atom(other.clone()),
            };
            if out.join(&part) == Joined::Widened {
                break;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Literal;

    fn s(text: &str) -> AbsValue {
        AbsValue::lit(Literal::String(text.into()))
    }

    #[test]
    fn aliases_share_the_object_and_strong_updates_replace() {
        let mut heap = Heap::default();
        let payload = AbsValue::atom(heap.allocate(10, ObjectKind::Plain));
        heap.init(10, "orderId", s("a"));
        let request = payload.clone(); // `const request = payload`
        heap.write(&request, "orderId", s("b"));
        assert_eq!(heap.read(&payload, "orderId"), s("b"));
        assert_eq!(heap.stats.strong_updates, 1);
    }

    #[test]
    fn writes_through_uncertain_targets_are_weak() {
        let mut heap = Heap::default();
        let a = heap.allocate(1, ObjectKind::Plain);
        let b = heap.allocate(2, ObjectKind::Plain);
        heap.init(1, "x", s("a"));
        heap.init(2, "x", s("b"));
        let either = AbsValue::from_atoms([a.clone(), b]);
        heap.write(&either, "x", s("c"));
        let x = heap.read(&AbsValue::atom(a), "x");
        assert!(x.contains(&Atom::lit(Literal::String("a".into()))));
        assert!(x.contains(&Atom::lit(Literal::String("c".into()))));
        assert_eq!(heap.stats.weak_updates, 2);
    }

    #[test]
    fn summary_objects_never_take_strong_updates() {
        let mut heap = Heap::default();
        let site = heap.allocate(5, ObjectKind::Plain);
        heap.init(5, "x", s("a"));
        heap.allocate(5, ObjectKind::Plain); // the same site again: a loop
        heap.write(&AbsValue::atom(site.clone()), "x", s("b"));
        let x = heap.read(&AbsValue::atom(site), "x");
        assert_eq!(x.atoms().len(), 2);
    }

    #[test]
    fn escape_is_transitive_and_selective() {
        let mut heap = Heap::default();
        let inner = heap.allocate(1, ObjectKind::Plain);
        heap.init(1, "id", s("i"));
        let outer = heap.allocate(2, ObjectKind::Plain);
        heap.init(2, "inner", AbsValue::atom(inner.clone()));
        let other = heap.allocate(3, ObjectKind::Plain);
        heap.init(3, "id", s("o"));
        heap.escape(&AbsValue::atom(outer));
        assert!(heap.read(&AbsValue::atom(inner), "id").is_top());
        assert_eq!(heap.read(&AbsValue::atom(other), "id"), s("o"));
    }

    #[test]
    fn writes_into_unknown_receivers_make_the_value_escape() {
        let mut heap = Heap::default();
        let object = heap.allocate(1, ObjectKind::Plain);
        heap.init(1, "id", s("i"));
        heap.write(
            &AbsValue::atom(Atom::Param { index: 0 }),
            "child",
            AbsValue::atom(object.clone()),
        );
        assert!(heap.read(&AbsValue::atom(object), "id").is_top());
        assert_eq!(heap.stats.unknown_target_writes, 1);
    }

    #[test]
    fn join_keeps_properties_of_both_paths() {
        let mut left = Heap::default();
        left.allocate(1, ObjectKind::Plain);
        left.init(1, "x", s("a"));
        let mut right = left.clone();
        right.init(1, "x", s("b"));
        assert!(left.join(&right));
        let x = left.read(&AbsValue::atom(Atom::Object { id: 1 }), "x");
        assert_eq!(x.atoms().len(), 2);
        assert!(!left.join(&right));
    }

    #[test]
    fn export_turns_objects_into_records() {
        let mut heap = Heap::default();
        let object = heap.allocate(1, ObjectKind::Plain);
        heap.init(1, "ok", AbsValue::lit(Literal::Boolean(true)));
        let exported = heap.export(&AbsValue::atom(object));
        assert_eq!(exported.to_string(), "{ok: true}");
        assert!(heap.export(&AbsValue::top()).is_top());
    }
}

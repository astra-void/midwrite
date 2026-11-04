//! Abstract values: bounded sets of identities.
//!
//! An [`AbsValue`] over-approximates the set of JavaScript values an
//! expression may evaluate to, restricted to the identities Midwrite
//! needs: functions and classes (call targets), allocation sites (objects and
//! receivers), summary inputs (`Param`, `This`), results of calls and effects
//! (provenance), and primitive literals (branch facts). Everything else is
//! [`AbsValue::top`].
//!
//! The lattice is the powerset of [`Atom`]s with a greatest element `Top`,
//! ordered by inclusion. A join whose result would hold more than
//! [`MAX_ATOMS`] atoms is widened to `Top`; [`Field`](Atom::Field) paths are
//! at most [`MAX_FIELD_DEPTH`] deep. Both bounds make every ascending chain
//! finite. Atoms are kept sorted and deduplicated, so equal sets have equal
//! representations (canonical form) and comparison is structural.

use std::fmt;

use serde::Serialize;

use crate::ir::Literal;

/// Largest number of atoms a value holds before it is widened to `Top`.
pub const MAX_ATOMS: usize = 6;
/// Deepest property path a [`Atom::Field`] represents.
pub const MAX_FIELD_DEPTH: usize = 3;
/// Largest number of properties kept in an [`Atom::Record`].
pub const MAX_RECORD_PROPS: usize = 16;

/// One identity a value may have.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Atom {
    /// A primitive constant.
    Lit { value: Literal },
    /// A function of the function table (frontends choose the numbering:
    /// a start offset while lowering, a table index after linking).
    Function { id: u32 },
    /// A class constructor (same numbering conventions as functions).
    Class { id: u32 },
    /// An object allocated at a site of the current function's heap.
    Object { id: u32 },
    /// An object whose properties are known, outside any heap (a returned
    /// object literal, as seen by callers). `open`: it may have other
    /// properties than those listed.
    Record {
        props: Box<[(Box<str>, AbsValue)]>,
        open: bool,
    },
    /// The value passed for parameter `index` (a summary input).
    Param { index: u32 },
    /// The receiver of the current method call (a summary input).
    This,
    /// The value returned by call site `site` of the current function
    /// (symbolic until the callee's summary is instantiated).
    CallResult { site: u64 },
    /// The value produced by effect `effect` of the current function (a row
    /// or a response). Never a concrete row.
    EffectResult { effect: u32 },
    /// A fresh unknown value with an identity (the result of an unknown
    /// call): unknown content, but every use refers to the same value.
    Opaque { id: u64 },
    /// Property `name` of a symbolic value.
    Field { base: Box<Atom>, name: Box<str> },
    /// Static program data: a module-level binding or an import (a table,
    /// a configuration constant, a client). Not an input of interest for
    /// provenance, and not unknown either. Its properties are static too.
    Static { symbol: u32 },
}

impl Atom {
    pub fn lit(value: Literal) -> Self {
        Self::Lit { value }
    }

    /// Whether the atom denotes an input or a result whose identity is
    /// meaningful across operations (used for provenance).
    pub fn is_symbolic(&self) -> bool {
        matches!(
            self,
            Self::Param { .. }
                | Self::This
                | Self::CallResult { .. }
                | Self::EffectResult { .. }
                | Self::Opaque { .. }
                | Self::Field { .. }
        )
    }

    /// The atom a property path starts from (`order` for `order.id.x`).
    pub fn root(&self) -> &Atom {
        match self {
            Self::Field { base, .. } => base.root(),
            other => other,
        }
    }

    /// Number of `Field` steps.
    pub fn depth(&self) -> usize {
        match self {
            Self::Field { base, .. } => 1 + base.depth(),
            _ => 0,
        }
    }

    /// Whether `self` is `other` or a property path below it.
    pub fn is_within(&self, other: &Atom) -> bool {
        self == other
            || match self {
                Self::Field { base, .. } => base.is_within(other),
                _ => false,
            }
    }

    /// Property `name` of this atom, without a heap: symbolic atoms give a
    /// `Field` path (bounded), records their property, anything else Top.
    pub fn field(&self, name: &str) -> AbsValue {
        match self {
            Self::Record { props, open } => match props.iter().find(|(key, _)| &**key == name) {
                Some((_, value)) => value.clone(),
                None if *open => AbsValue::top(),
                // Prototype properties are not modeled: never `undefined`.
                None => AbsValue::top(),
            },
            Self::Static { .. } => AbsValue::atom(self.clone()),
            atom if atom.is_symbolic() && atom.depth() < MAX_FIELD_DEPTH => {
                AbsValue::atom(Self::Field {
                    base: Box::new(atom.clone()),
                    name: name.into(),
                })
            }
            _ => AbsValue::top(),
        }
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lit { value } => match value {
                Literal::Boolean(b) => write!(f, "{b}"),
                Literal::String(s) => write!(f, "{s:?}"),
                Literal::Number(n) => write!(f, "{n}"),
                Literal::Null => write!(f, "null"),
                Literal::Undefined => write!(f, "undefined"),
            },
            Self::Function { id } => write!(f, "fn#{id}"),
            Self::Class { id } => write!(f, "class#{id}"),
            Self::Object { id } => write!(f, "obj#{id}"),
            Self::Record { props, open } => {
                write!(f, "{{")?;
                for (index, (key, value)) in props.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{key}: {value}")?;
                }
                if *open {
                    write!(f, ", …")?;
                }
                write!(f, "}}")
            }
            Self::Param { index } => write!(f, "param#{index}"),
            Self::This => write!(f, "this"),
            Self::CallResult { site } => write!(f, "call#{site}.result"),
            Self::EffectResult { effect } => write!(f, "effect#{effect}.result"),
            Self::Opaque { id } => write!(f, "opaque#{id}"),
            Self::Field { base, name } => write!(f, "{base}.{name}"),
            Self::Static { symbol } => write!(f, "static#{symbol}"),
        }
    }
}

/// How a join changed its left operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Joined {
    Unchanged,
    /// New atoms were added within the bound.
    Grew,
    /// The bound was exceeded (or `Top` joined in): the value is now `Top`.
    Widened,
}

impl Joined {
    pub fn changed(self) -> bool {
        self != Self::Unchanged
    }
}

/// A bounded set of [`Atom`]s, or `Top`. The default value is bottom (the
/// empty set: no value, unreachable).
///
/// Most values have zero or one atom: those are stored inline, without a
/// heap allocation.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AbsValue {
    repr: Repr,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Repr {
    #[default]
    Bottom,
    One(Atom),
    /// Two or more atoms, sorted and deduplicated.
    Many(Vec<Atom>),
    Top,
}

impl Serialize for AbsValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let atoms = self.atoms();
        let fields = usize::from(!atoms.is_empty()) + usize::from(self.is_top());
        let mut state = serializer.serialize_struct("AbsValue", fields)?;
        if !atoms.is_empty() {
            state.serialize_field("atoms", atoms)?;
        }
        if self.is_top() {
            state.serialize_field("top", &true)?;
        }
        state.end()
    }
}

impl AbsValue {
    pub fn bottom() -> Self {
        Self::default()
    }

    pub fn top() -> Self {
        Self { repr: Repr::Top }
    }

    pub fn atom(atom: Atom) -> Self {
        Self {
            repr: Repr::One(atom),
        }
    }

    pub fn lit(value: Literal) -> Self {
        Self::atom(Atom::lit(value))
    }

    /// The value holding every atom of `atoms` (widened past the bound).
    pub fn from_atoms(atoms: impl IntoIterator<Item = Atom>) -> Self {
        let mut value = Self::bottom();
        for atom in atoms {
            value.join_atom(atom);
        }
        value
    }

    pub fn is_top(&self) -> bool {
        matches!(self.repr, Repr::Top)
    }

    pub fn is_bottom(&self) -> bool {
        matches!(self.repr, Repr::Bottom)
    }

    /// The atoms (empty for `Top` and bottom).
    pub fn atoms(&self) -> &[Atom] {
        match &self.repr {
            Repr::Bottom | Repr::Top => &[],
            Repr::One(atom) => std::slice::from_ref(atom),
            Repr::Many(atoms) => atoms,
        }
    }

    /// The only atom, if the value is exactly one identity.
    pub fn single(&self) -> Option<&Atom> {
        match &self.repr {
            Repr::One(atom) => Some(atom),
            _ => None,
        }
    }

    /// Whether `atom` is one of the value's identities.
    pub fn contains(&self, atom: &Atom) -> bool {
        match &self.repr {
            Repr::One(own) => own == atom,
            Repr::Many(atoms) => atoms.binary_search(atom).is_ok(),
            Repr::Bottom | Repr::Top => false,
        }
    }

    fn join_atom(&mut self, atom: Atom) -> Joined {
        match &mut self.repr {
            Repr::Top => Joined::Unchanged,
            Repr::Bottom => {
                self.repr = Repr::One(atom);
                Joined::Grew
            }
            Repr::One(own) => {
                if *own == atom {
                    return Joined::Unchanged;
                }
                let own = std::mem::replace(own, Atom::This);
                let atoms = if own < atom {
                    vec![own, atom]
                } else {
                    vec![atom, own]
                };
                // MAX_ATOMS ≥ 2.
                self.repr = Repr::Many(atoms);
                Joined::Grew
            }
            Repr::Many(atoms) => match atoms.binary_search(&atom) {
                Ok(_) => Joined::Unchanged,
                Err(position) => {
                    if atoms.len() >= MAX_ATOMS {
                        self.repr = Repr::Top;
                        Joined::Widened
                    } else {
                        atoms.insert(position, atom);
                        Joined::Grew
                    }
                }
            },
        }
    }

    /// `self := self ⊔ other`.
    pub fn join(&mut self, other: &AbsValue) -> Joined {
        if self.is_top() {
            return Joined::Unchanged;
        }
        match &other.repr {
            Repr::Bottom => Joined::Unchanged,
            Repr::Top => {
                self.repr = Repr::Top;
                Joined::Widened
            }
            Repr::One(atom) => self.join_atom(atom.clone()),
            Repr::Many(atoms) => {
                if self.is_bottom() {
                    self.repr = other.repr.clone();
                    return Joined::Grew;
                }
                let mut outcome = Joined::Unchanged;
                for atom in atoms {
                    match self.join_atom(atom.clone()) {
                        Joined::Widened => return Joined::Widened,
                        Joined::Grew => outcome = Joined::Grew,
                        Joined::Unchanged => {}
                    }
                }
                outcome
            }
        }
    }

    /// `self ⊔ other`, without changing either.
    #[must_use]
    pub fn joined(&self, other: &AbsValue) -> AbsValue {
        let mut out = self.clone();
        out.join(other);
        out
    }

    /// `self ⊑ other`.
    pub fn leq(&self, other: &AbsValue) -> bool {
        other.is_top() || (!self.is_top() && self.atoms().iter().all(|atom| other.contains(atom)))
    }

    /// Property `name` of every atom, without a heap (see [`Atom::field`]).
    pub fn field(&self, name: &str) -> AbsValue {
        match &self.repr {
            Repr::Top => Self::top(),
            Repr::Bottom => Self::bottom(),
            Repr::One(atom) => atom.field(name),
            Repr::Many(atoms) => {
                let mut out = Self::bottom();
                for atom in atoms {
                    if out.join(&atom.field(name)) == Joined::Widened {
                        break;
                    }
                }
                out
            }
        }
    }

    /// Replace atoms: `f` returns the value an atom stands for, or `None`
    /// to keep it. `Field` paths and record properties are substituted
    /// recursively, so `Param(0).id` becomes the `id` of the argument.
    #[must_use]
    pub fn substitute(&self, f: &dyn Fn(&Atom) -> Option<AbsValue>) -> AbsValue {
        match &self.repr {
            Repr::Top => Self::top(),
            Repr::Bottom => Self::bottom(),
            Repr::One(atom) => substitute_atom(atom, f),
            Repr::Many(atoms) => {
                let mut out = Self::bottom();
                for atom in atoms {
                    if out.join(&substitute_atom(atom, f)) == Joined::Widened {
                        break;
                    }
                }
                out
            }
        }
    }
}

fn substitute_atom(atom: &Atom, f: &dyn Fn(&Atom) -> Option<AbsValue>) -> AbsValue {
    if let Some(value) = f(atom) {
        return value;
    }
    match atom {
        Atom::Field { base, name } => {
            let base = substitute_atom(base, f);
            base.field(name)
        }
        Atom::Record { props, open } => AbsValue::atom(Atom::Record {
            props: props
                .iter()
                .map(|(key, value)| (key.clone(), value.substitute(f)))
                .collect(),
            open: *open,
        }),
        other => AbsValue::atom(other.clone()),
    }
}

impl fmt::Display for AbsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_top() {
            return write!(f, "⊤");
        }
        match self.atoms() {
            [] => write!(f, "⊥"),
            [atom] => write!(f, "{atom}"),
            atoms => {
                write!(f, "{{")?;
                for (index, atom) in atoms.iter().enumerate() {
                    if index > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{atom}")?;
                }
                write!(f, "}}")
            }
        }
    }
}

/// One identity an effect's arguments are derived from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub atom: Atom,
    /// The argument (or one of its properties, transitively) *is* this value.
    /// `false`: the value only occurs inside a computation (a template, an
    /// arithmetic expression, a call of a pure builtin).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exact: bool,
}

/// What the abstract values of an effect's arguments say about where they
/// come from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    /// Symbolic identities the arguments contain, sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    /// Some argument (or part of one) is an unknown value: the inputs are
    /// incomplete.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unknown: bool,
}

/// Largest number of inputs kept per effect.
pub const MAX_INPUTS: usize = 16;

impl Provenance {
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty() && !self.unknown
    }

    /// Add every symbolic identity of `value`. Records are traversed;
    /// `Top` marks the provenance unknown.
    pub fn add(&mut self, value: &AbsValue, exact: bool) {
        self.add_depth(value, exact, 0);
    }

    fn add_depth(&mut self, value: &AbsValue, exact: bool, depth: usize) {
        if value.is_top() {
            self.unknown = true;
            return;
        }
        for atom in value.atoms() {
            match atom {
                Atom::Record { props, open } => {
                    if *open {
                        self.unknown = true;
                    }
                    if depth < MAX_FIELD_DEPTH {
                        for (_, value) in props.iter() {
                            self.add_depth(value, exact, depth + 1);
                        }
                    }
                }
                atom if atom.is_symbolic() => self.push(Input {
                    atom: atom.clone(),
                    exact,
                }),
                _ => {}
            }
        }
    }

    fn push(&mut self, input: Input) {
        if let Some(existing) = self.inputs.iter_mut().find(|i| i.atom == input.atom) {
            existing.exact |= input.exact;
            return;
        }
        if self.inputs.len() >= MAX_INPUTS {
            self.unknown = true;
            return;
        }
        self.inputs.push(input);
        self.inputs.sort();
    }

    /// The same provenance with atoms substituted (see
    /// [`AbsValue::substitute`]). A substitution that yields a record
    /// contributes its properties; one that yields `Top` makes it unknown.
    #[must_use]
    pub fn substitute(&self, f: &dyn Fn(&Atom) -> Option<AbsValue>) -> Provenance {
        let mut out = Provenance {
            inputs: Vec::new(),
            unknown: self.unknown,
        };
        for input in &self.inputs {
            let value = AbsValue::atom(input.atom.clone()).substitute(f);
            out.add(&value, input.exact);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(index: u32) -> Atom {
        Atom::Param { index }
    }

    #[test]
    fn join_is_union_and_widens_past_the_bound() {
        let mut value = AbsValue::atom(p(0));
        assert_eq!(value.join(&AbsValue::atom(p(1))), Joined::Grew);
        assert_eq!(value.join(&AbsValue::atom(p(1))), Joined::Unchanged);
        for index in 2..MAX_ATOMS as u32 {
            assert_eq!(value.join(&AbsValue::atom(p(index))), Joined::Grew);
        }
        assert_eq!(value.atoms().len(), MAX_ATOMS);
        assert_eq!(value.join(&AbsValue::atom(p(99))), Joined::Widened);
        assert!(value.is_top());
        assert_eq!(value.join(&AbsValue::atom(p(100))), Joined::Unchanged);
    }

    #[test]
    fn canonical_form_does_not_depend_on_order() {
        let a = AbsValue::from_atoms([p(2), p(0), p(1)]);
        let b = AbsValue::from_atoms([p(1), p(2), p(0), p(1)]);
        assert_eq!(a, b);
    }

    #[test]
    fn fields_are_bounded_paths() {
        let order = AbsValue::atom(Atom::EffectResult { effect: 3 });
        let id = order.field("id");
        assert_eq!(id.to_string(), "effect#3.result.id");
        assert!(id.field("a").field("b").field("c").is_top());
        assert!(
            AbsValue::lit(Literal::Number("1".into()))
                .field("x")
                .is_top()
        );
    }

    #[test]
    fn substitution_resolves_parameters_through_fields_and_records() {
        let value = AbsValue::atom(p(0)).field("order").field("id");
        let argument = AbsValue::atom(Atom::Record {
            props: vec![(
                "order".into(),
                AbsValue::atom(Atom::EffectResult { effect: 7 }),
            )]
            .into(),
            open: false,
        });
        let result = value.substitute(&|atom| (atom == &p(0)).then(|| argument.clone()));
        assert_eq!(result.to_string(), "effect#7.result.id");
    }

    #[test]
    fn provenance_collects_symbolic_atoms_and_unknowns() {
        let mut provenance = Provenance::default();
        provenance.add(
            &AbsValue::atom(Atom::Record {
                props: vec![
                    ("id".into(), AbsValue::atom(p(0)).field("id")),
                    ("kind".into(), AbsValue::lit(Literal::String("x".into()))),
                ]
                .into(),
                open: false,
            }),
            true,
        );
        assert_eq!(provenance.inputs.len(), 1);
        assert!(!provenance.unknown);
        provenance.add(&AbsValue::top(), true);
        assert!(provenance.unknown);
    }
}

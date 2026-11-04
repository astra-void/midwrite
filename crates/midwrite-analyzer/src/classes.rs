//! Classes declared in a module, and bounded method dispatch.
//!
//! See `docs/phase5/CLASS-RESOLUTION.md`. A class is identified by the start
//! offset of its declaration in its module. Method lookup follows JavaScript
//! semantics for the supported subset:
//!
//! * an instance *field* holding a function (`save = async () => {}`) is an
//!   own property of every instance and shadows prototype methods, whichever
//!   class in the chain declares it (the most derived declaration wins,
//!   because subclass field initializers run after the base constructor);
//! * otherwise the method is looked up on the prototype chain, starting at
//!   the receiver's class.
//!
//! Dynamic changes (`this.save = ...`, `C.prototype.save = ...`,
//! `Object.defineProperty`/`Object.assign`/`Object.setPrototypeOf` on `this`
//! or a class) disable resolution of the affected names.

use std::collections::{BTreeMap, BTreeSet};

/// What a class extends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SuperRef {
    /// A class declared in the same module (start offset).
    Local(u32),
    /// A class imported from `module` under `name` (`default` for a default
    /// import).
    Imported { module: String, name: String },
    /// A global constructor (`Error`, `Map`, ...): no methods Midwrite
    /// cares about, no instance fields.
    Builtin(String),
    /// Anything else (a mixin call, a member expression, a namespace member).
    Unknown(String),
}

/// One class declaration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClassInfo {
    pub name: String,
    pub superclass: Option<SuperRef>,
    /// Instance methods (prototype), by name → function start offset.
    pub methods: BTreeMap<String, u32>,
    /// Static methods.
    pub statics: BTreeMap<String, u32>,
    /// Instance fields initialized with a function (own properties).
    pub fields: BTreeMap<String, u32>,
    /// Getters, setters and accessor properties: never called as methods.
    pub accessors: BTreeSet<String>,
    /// The explicit constructor (start offset).
    pub constructor: Option<u32>,
    /// Some instance field initializer evaluates a call or `new`: the
    /// constructor runs code that is not in a function of the table.
    pub field_initializer_calls: bool,
    /// Names assigned dynamically on instances or the prototype; `"*"` when
    /// `Object.defineProperty`/`assign`/`setPrototypeOf` touches them.
    pub dynamic: BTreeSet<String>,
}

impl ClassInfo {
    pub fn is_dynamic(&self, name: &str) -> bool {
        self.dynamic.contains(name) || self.dynamic.contains("*")
    }

    /// Whether this class itself declares `name` as a method or field.
    pub fn declares(&self, name: &str) -> bool {
        self.methods.contains_key(name)
            || self.fields.contains_key(name)
            || self.accessors.contains(name)
    }
}

/// How a method call's receiver relates to the class it is looked up in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ReceiverKind {
    /// The receiver's class is exactly the class (`new C()`). `escaped`:
    /// the object reached unanalyzed code before the call.
    Exact { escaped: bool },
    /// `this` inside an instance method of the class: any subclass instance.
    This,
    /// The class itself (`C.m()`).
    Static,
    /// `this` inside a static method: the class or any subclass.
    StaticThis,
}

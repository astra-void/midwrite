//! Linking call targets to the function table, and method dispatch over
//! the classes of all analyzed modules.
//!
//! See `docs/phase5/CLASS-RESOLUTION.md`. Calls are linked only through
//! syntax the analysis can follow (a declaration, an import that resolves to
//! an export, a receiver whose class is known); names alone never link two
//! functions.

use std::collections::{BTreeMap, HashMap};

use midwrite_core::absint::summary::{ArgFact, CallFact, FunctionFacts};
use midwrite_core::ir::CallResolution;

use crate::classes::{ClassInfo, ReceiverKind, SuperRef};
use crate::lower::{CallTarget, ClassRef, CompactFacts, FactArg, FactTable};
use crate::resolve::Resolution;

/// What an imported name resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Export {
    /// A function of the project table.
    Function(usize),
    /// An object service: function properties by name, as table indices.
    Object(BTreeMap<String, usize>),
    /// A class: (module, class start offset).
    Class(usize, u32),
    /// A reason the import cannot be linked to a function.
    Unlinked(String),
}

/// Module and export resolution, provided by project analysis (or a stub
/// for a single file).
pub(crate) trait ImportResolver {
    fn module(&mut self, from: usize, specifier: &str) -> Resolution;
    fn export(&mut self, module: usize, name: &str) -> Export;
    fn module_name(&self, module: usize) -> &str;
}

/// Resolution for a single file analyzed on its own: imports are not
/// followed.
pub(crate) struct NoImports;

impl ImportResolver for NoImports {
    fn module(&mut self, _: usize, _: &str) -> Resolution {
        Resolution::NotFound(
            "imports are not followed when a file is analyzed on its own".to_owned(),
        )
    }

    fn export(&mut self, _: usize, name: &str) -> Export {
        Export::Unlinked(format!("`{name}` is not followed"))
    }

    fn module_name(&self, _: usize) -> &str {
        "this file"
    }
}

/// What a class extends, after linking.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Parent {
    None,
    Class(usize),
    Builtin,
    Unknown(String),
}

struct ClassNode {
    module: usize,
    info: ClassInfo,
    parent: Parent,
}

/// The classes of every analyzed module, with their (linked) superclasses
/// and subclasses.
pub(crate) struct ClassTable {
    nodes: Vec<ClassNode>,
    by_key: HashMap<(usize, u32), usize>,
    children: Vec<Vec<usize>>,
}

/// One module's view for linking.
pub(crate) struct ModuleView<'a> {
    pub classes: &'a HashMap<u32, ClassInfo>,
    /// Function start offset → index in the module.
    pub index: &'a HashMap<u32, usize>,
    /// Index of the module's first function in the table.
    pub offset: usize,
}

impl ClassTable {
    pub fn build(modules: &[ModuleView<'_>], imports: &mut dyn ImportResolver) -> Self {
        let mut nodes = Vec::new();
        let mut by_key = HashMap::new();
        for (module, view) in modules.iter().enumerate() {
            let mut starts: Vec<&u32> = view.classes.keys().collect();
            starts.sort();
            for start in starts {
                by_key.insert((module, *start), nodes.len());
                nodes.push(ClassNode {
                    module,
                    info: view.classes[start].clone(),
                    parent: Parent::None,
                });
            }
        }
        for node in &mut nodes {
            let module = node.module;
            let parent = match &node.info.superclass {
                None => Parent::None,
                Some(SuperRef::Local(start)) => match by_key.get(&(module, *start)) {
                    Some(node) => Parent::Class(*node),
                    None => {
                        Parent::Unknown("a class of this file that was not collected".to_owned())
                    }
                },
                Some(SuperRef::Imported {
                    module: specifier,
                    name,
                }) => match imports.module(module, specifier) {
                    Resolution::File(target) => match imports.export(target, name) {
                        Export::Class(m, start) => match by_key.get(&(m, start)) {
                            Some(node) => Parent::Class(*node),
                            None => Parent::Unknown(format!("`{name}` from \"{specifier}\"")),
                        },
                        _ => Parent::Unknown(format!(
                            "`{name}` from \"{specifier}\", which is not a class Midwrite analyzes"
                        )),
                    },
                    Resolution::Package => {
                        Parent::Unknown(format!("`{name}` from the package \"{specifier}\""))
                    }
                    _ => Parent::Unknown(format!("`{name}` from \"{specifier}\"")),
                },
                Some(SuperRef::Builtin(_)) => Parent::Builtin,
                Some(SuperRef::Unknown(text)) => Parent::Unknown(format!("`{text}`")),
            };
            node.parent = parent;
        }
        let mut children = vec![Vec::new(); nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            if let Parent::Class(parent) = node.parent {
                children[parent].push(index);
            }
        }
        Self {
            nodes,
            by_key,
            children,
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    fn node(
        &self,
        module: usize,
        class: &ClassRef,
        imports: &mut dyn ImportResolver,
    ) -> Result<usize, String> {
        match class {
            ClassRef::Local(start) => self
                .by_key
                .get(&(module, *start))
                .copied()
                .ok_or_else(|| "the class was not collected".to_owned()),
            ClassRef::Imported {
                module: specifier,
                name,
            } => match imports.module(module, specifier) {
                Resolution::File(target) => match imports.export(target, name) {
                    Export::Class(m, start) => {
                        self.by_key.get(&(m, start)).copied().ok_or_else(|| {
                            format!("`{name}` from \"{specifier}\" was not collected")
                        })
                    }
                    _ => Err(format!(
                        "`{name}` imported from \"{specifier}\" is not a class Midwrite analyzes"
                    )),
                },
                Resolution::Package => Err(format!(
                    "`{name}` is a class from the package \"{specifier}\"; packages are not followed"
                )),
                Resolution::NotFound(reason) => Err(format!(
                    "`{name}` is imported from \"{specifier}\", which is not resolved: {reason}"
                )),
                Resolution::Ambiguous(_) => Err(format!(
                    "`{name}` is imported from \"{specifier}\", which matches several files"
                )),
            },
        }
    }

    /// The classes from `node` up its known superclass chain, and how the
    /// chain ends.
    fn chain(&self, node: usize) -> (Vec<usize>, Parent) {
        let mut out = vec![node];
        let mut current = node;
        loop {
            match &self.nodes[current].parent {
                Parent::Class(parent) if !out.contains(parent) && out.len() < 32 => {
                    out.push(*parent);
                    current = *parent;
                }
                Parent::Class(_) => {
                    return (out, Parent::Unknown("a cyclic superclass chain".to_owned()));
                }
                end => return (out, end.clone()),
            }
        }
    }

    fn descendants(&self, node: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut work = self.children[node].clone();
        while let Some(child) = work.pop() {
            if out.contains(&child) {
                continue;
            }
            out.push(child);
            work.extend(self.children[child].iter().copied());
        }
        out.sort_unstable();
        out
    }

    fn function(&self, node: usize, start: u32, modules: &[ModuleView<'_>]) -> Option<usize> {
        let module = &modules[self.nodes[node].module];
        module.index.get(&start).map(|index| module.offset + index)
    }

    fn name(&self, node: usize) -> &str {
        &self.nodes[node].info.name
    }

    /// Method `name` called on a receiver of `class` (see the module docs
    /// of `classes.rs` for the lookup rules).
    pub fn method(
        &self,
        node: usize,
        name: &str,
        receiver: ReceiverKind,
        modules: &[ModuleView<'_>],
    ) -> Result<(usize, CallResolution), String> {
        let (chain, end) = self.chain(node);
        for &class in &chain {
            if self.nodes[class].info.is_dynamic(name) {
                return Err(format!(
                    "`{name}` is assigned dynamically on instances or the prototype of `{}`",
                    self.name(class)
                ));
            }
        }
        let is_static = matches!(receiver, ReceiverKind::Static | ReceiverKind::StaticThis);
        // Instance fields are own properties and win over prototype
        // methods; the most derived declaration wins.
        let field = if is_static {
            None
        } else {
            chain
                .iter()
                .find_map(|&class| self.nodes[class].info.fields.get(name).map(|s| (class, *s)))
        };
        let found = field.or_else(|| {
            chain.iter().find_map(|&class| {
                let info = &self.nodes[class].info;
                let methods = if is_static {
                    &info.statics
                } else {
                    &info.methods
                };
                methods.get(name).map(|start| (class, *start))
            })
        });
        let Some((declaring, start)) = found else {
            return Err(match &end {
                Parent::Unknown(what) => format!(
                    "`{name}` is not declared by `{}` or its analyzed superclasses; it may be inherited from {what}",
                    self.name(node)
                ),
                _ => format!(
                    "`{name}` is not a method of `{}` or its superclasses",
                    self.name(node)
                ),
            });
        };
        if chain
            .iter()
            .take_while(|&&class| class != declaring)
            .any(|&class| self.nodes[class].info.accessors.contains(name))
        {
            return Err(format!(
                "`{name}` is an accessor or data property of a class in the chain"
            ));
        }
        let exact = matches!(receiver, ReceiverKind::Exact { .. });
        if matches!(receiver, ReceiverKind::This | ReceiverKind::StaticThis) {
            let overrides: Vec<String> = self
                .descendants(node)
                .into_iter()
                .filter(|&sub| {
                    let info = &self.nodes[sub].info;
                    if receiver == ReceiverKind::StaticThis {
                        info.statics.contains_key(name)
                    } else {
                        info.declares(name) || info.is_dynamic(name)
                    }
                })
                .map(|sub| format!("`{}`", self.name(sub)))
                .collect();
            if !overrides.is_empty() {
                return Err(format!(
                    "`this.{name}()` may dispatch to an override in {}; several targets are possible",
                    overrides.join(", ")
                ));
            }
        }
        let mut assumptions = Vec::new();
        if let Parent::Unknown(what) = &end
            && !is_static
        {
            assumptions.push(format!(
                "that {what}, an unanalyzed superclass of `{}`, does not install an instance property `{name}`",
                self.name(node)
            ));
        }
        if receiver == (ReceiverKind::Exact { escaped: true }) {
            assumptions.push(format!(
                "that the receiver, which reached unanalyzed code before the call, still has `{}.{name}`",
                self.name(node)
            ));
        }
        let function = self
            .function(declaring, start, modules)
            .ok_or_else(|| format!("`{}.{name}` has no analyzable body", self.name(declaring)))?;
        Ok((
            function,
            CallResolution::Method {
                class: self.name(declaring).to_owned(),
                exact,
                assumption: (!assumptions.is_empty()).then(|| assumptions.join("; and ")),
            },
        ))
    }

    /// The constructor `new C(...)` runs: `Ok(None)` when every constructor
    /// up a known chain is implicit and no field initializer calls anything.
    pub fn constructor(
        &self,
        node: usize,
        modules: &[ModuleView<'_>],
    ) -> Result<Option<usize>, String> {
        let (chain, end) = self.chain(node);
        for &class in &chain {
            let info = &self.nodes[class].info;
            if info.field_initializer_calls {
                return Err(format!(
                    "a field initializer of `{}` evaluates calls when an instance is created",
                    info.name
                ));
            }
            if let Some(start) = info.constructor {
                return self
                    .function(class, start, modules)
                    .map(Some)
                    .ok_or_else(|| {
                        format!("the constructor of `{}` has no analyzable body", info.name)
                    });
            }
        }
        match end {
            Parent::Unknown(what) => Err(format!(
                "`{}` extends {what}, whose constructor is not analyzed",
                self.name(node)
            )),
            _ => Ok(None),
        }
    }

    /// `super(...)` (`name: None`) or `super.name(...)` in class `node`.
    pub fn super_call(
        &self,
        node: usize,
        name: Option<&str>,
        modules: &[ModuleView<'_>],
    ) -> Result<Option<(usize, CallResolution)>, String> {
        match (&self.nodes[node].parent, name) {
            (Parent::None | Parent::Builtin, None) => Ok(None),
            (Parent::Unknown(what), _) => Err(format!(
                "`{}` extends {what}, which is not analyzed",
                self.name(node)
            )),
            (Parent::None | Parent::Builtin, Some(name)) => Err(format!(
                "`super.{name}` refers to a superclass Midwrite does not model"
            )),
            (Parent::Class(parent), None) => {
                Ok(self.constructor(*parent, modules)?.map(|function| {
                    (
                        function,
                        CallResolution::Method {
                            class: self.name(*parent).to_owned(),
                            exact: true,
                            assumption: None,
                        },
                    )
                }))
            }
            (Parent::Class(parent), Some(name)) => {
                // `super.m` reads the parent prototype: no instance fields.
                let (chain, end) = self.chain(*parent);
                let found = chain.iter().find_map(|&class| {
                    self.nodes[class]
                        .info
                        .methods
                        .get(name)
                        .map(|start| (class, *start))
                });
                match found {
                    Some((class, start)) => self
                        .function(class, start, modules)
                        .map(|function| {
                            Some((
                                function,
                                CallResolution::Method {
                                    class: self.name(class).to_owned(),
                                    exact: true,
                                    assumption: None,
                                },
                            ))
                        })
                        .ok_or_else(|| format!("`{name}` has no analyzable body")),
                    None => Err(match end {
                        Parent::Unknown(what) => format!("`super.{name}` may come from {what}"),
                        _ => format!("`super.{name}` is not a method of the superclasses"),
                    }),
                }
            }
        }
    }
}

/// The outcome of linking one call target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Linked {
    /// A function of the table.
    Function(usize, Option<CallResolution>),
    /// The callback the caller passes for this parameter.
    Callback(u32),
    /// Nothing runs (an implicit constructor chain).
    Nothing,
    /// Not linked, with the reason; `package` for calls into packages.
    Unlinked { reason: String, package: bool },
    /// Not a call of a function at all (a method call on an imported
    /// binding that is not an object service): left as it was.
    NotAFunction,
}

/// Link `target`, written in module `module`.
pub(crate) fn link_target(
    module: usize,
    target: &CallTarget,
    modules: &[ModuleView<'_>],
    classes: &ClassTable,
    imports: &mut dyn ImportResolver,
) -> Linked {
    let view = &modules[module];
    match target {
        CallTarget::Local(start) => match view.index.get(start) {
            Some(index) => Linked::Function(view.offset + index, None),
            None => Linked::Unlinked {
                reason: "the function has no analyzable body".to_owned(),
                package: false,
            },
        },
        CallTarget::Param(index) => Linked::Callback(*index),
        CallTarget::Imported {
            module: specifier,
            name,
            member,
        } => {
            let label = if name == "default" {
                "the default export".to_owned()
            } else {
                format!("`{name}`")
            };
            match imports.module(module, specifier) {
                Resolution::File(target_module) => {
                    let file = imports.module_name(target_module).to_owned();
                    match (imports.export(target_module, name), member) {
                        (Export::Function(index), None) => Linked::Function(index, None),
                        (Export::Object(methods), Some(member)) => match methods.get(member) {
                            Some(index) => Linked::Function(*index, None),
                            None => Linked::Unlinked {
                                reason: format!(
                                    "`{name}` imported from \"{specifier}\" ({file}) is an object without a function property `{member}` Midwrite can summarize"
                                ),
                                package: false,
                            },
                        },
                        (Export::Object(_), None) => Linked::Unlinked {
                            reason: format!(
                                "{label} imported from \"{specifier}\" ({file}) is an object, not a function"
                            ),
                            package: false,
                        },
                        (Export::Class(m, start), Some(member)) => match classes
                            .by_key
                            .get(&(m, start))
                        {
                            Some(node) => {
                                match classes.method(*node, member, ReceiverKind::Static, modules) {
                                    Ok((function, resolution)) => {
                                        Linked::Function(function, Some(resolution))
                                    }
                                    Err(reason) => Linked::Unlinked {
                                        reason,
                                        package: false,
                                    },
                                }
                            }
                            None => Linked::NotAFunction,
                        },
                        (Export::Unlinked(reason), None) => Linked::Unlinked {
                            reason: format!(
                                "{label} is imported from \"{specifier}\" ({file}), but {reason}"
                            ),
                            package: false,
                        },
                        // A method call on an imported binding that is not
                        // an object service or class: not a function call.
                        (_, Some(_)) | (Export::Class(..), None) => Linked::NotAFunction,
                    }
                }
                Resolution::Package if member.is_none() => Linked::Unlinked {
                    reason: format!(
                        "{label} is imported from the package \"{specifier}\"; packages are not followed"
                    ),
                    package: true,
                },
                Resolution::NotFound(reason) if member.is_none() => Linked::Unlinked {
                    reason: format!(
                        "{label} is imported from \"{specifier}\", which is not resolved: {reason}"
                    ),
                    package: false,
                },
                Resolution::Ambiguous(candidates) if member.is_none() => Linked::Unlinked {
                    reason: format!(
                        "{label} is imported from \"{specifier}\", which matches several files ({}); Midwrite does not choose one",
                        candidates.join(", ")
                    ),
                    package: false,
                },
                _ => Linked::NotAFunction,
            }
        }
        CallTarget::Method {
            class,
            name,
            receiver,
        } => match classes.node(module, class, imports) {
            Ok(node) => match classes.method(node, name, *receiver, modules) {
                Ok((function, resolution)) => Linked::Function(function, Some(resolution)),
                Err(reason) => Linked::Unlinked {
                    reason,
                    package: false,
                },
            },
            Err(reason) => Linked::Unlinked {
                package: reason.contains("package"),
                reason,
            },
        },
        CallTarget::Constructor(class) => match classes.node(module, class, imports) {
            Ok(node) => match classes.constructor(node, modules) {
                Ok(Some(function)) => Linked::Function(function, None),
                Ok(None) => Linked::Nothing,
                Err(reason) => Linked::Unlinked {
                    reason,
                    package: false,
                },
            },
            Err(reason) => Linked::Unlinked {
                package: reason.contains("package"),
                reason,
            },
        },
        CallTarget::Super { class, name } => match classes.by_key.get(&(module, *class)) {
            Some(node) => match classes.super_call(*node, name.as_deref(), modules) {
                Ok(Some((function, resolution))) => Linked::Function(function, Some(resolution)),
                Ok(None) => Linked::Nothing,
                Err(reason) => Linked::Unlinked {
                    reason,
                    package: false,
                },
            },
            None => Linked::Unlinked {
                reason: "the enclosing class was not collected".to_owned(),
                package: false,
            },
        },
    }
}

/// The summary facts of a function, with targets linked.
pub(crate) fn link_facts(
    module: usize,
    facts: &CompactFacts,
    table: &FactTable,
    modules: &[ModuleView<'_>],
    classes: &ClassTable,
    imports: &mut dyn ImportResolver,
) -> FunctionFacts {
    let view = &modules[module];
    let mut out = FunctionFacts {
        refuted: facts.refuted.clone(),
        calls: Vec::with_capacity(facts.calls.len()),
        params: facts.params.clone(),
    };
    for call in &facts.calls {
        let args: Vec<ArgFact> = call
            .args
            .iter()
            .copied()
            .map(|arg| match arg {
                FactArg::Other => ArgFact::Other,
                FactArg::Param(index) => ArgFact::Param(index),
                FactArg::Function(start) => match view.index.get(&start) {
                    Some(index) => ArgFact::Function(view.offset + index),
                    None => ArgFact::Other,
                },
            })
            .collect();
        let label = &table.labels[call.label as usize];
        match link_target(
            module,
            &table.targets[call.target as usize],
            modules,
            classes,
            imports,
        ) {
            Linked::Function(callee, _) => out.calls.push(CallFact {
                callee,
                args,
                label: label.clone(),
            }),
            Linked::Nothing => {}
            Linked::Callback(_) => {}
            Linked::Unlinked { reason, .. } => {
                if out.refuted.is_none() {
                    out.refuted = Some(format!("calls `{label}`, which is not linked: {reason}"));
                }
            }
            Linked::NotAFunction => {
                if out.refuted.is_none() {
                    out.refuted = Some(format!("calls `{label}`, whose target is not known"));
                }
            }
        }
    }
    out
}

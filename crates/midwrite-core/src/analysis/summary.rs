//! Call summaries.
//!
//! The analysis of a function is intraprocedural, except for calls of other
//! functions the frontend could identify: functions declared in the same
//! file, functions imported from other modules (project analysis), methods
//! found by dispatch on a known receiver, immediately invoked function
//! expressions, and callbacks a caller passes for a parameter. The frontend
//! reports every awaited, returned or concurrent call of such a function as
//! a [`LocalCall`]: an unknown effect that stands for the call, the callee,
//! the database handles and abstract values passed as arguments.
//! [`inline_calls`] replaces that effect with a [`Node::Call`] holding the
//! callee's *summary*, which is the callee's own IR with its calls expanded
//! in turn (at most [`AnalysisOptions::max_call_depth`] levels deep, never
//! recursively). The caller's path enumeration then sees the callee's
//! commits and external effects in order, at their own locations.
//!
//! Every inlined operation records the chain of calls it is reached through
//! and whether the callee performs it on every path through the callee
//! ([`SummaryOrigin`]); operations on only some paths lower confidence.
//!
//! **Inert callees**. With solved effect summaries
//! ([`crate::absint::summary`]), a call of a callee that is *inert* under the
//! arguments of this call (no write, transaction, external effect or
//! unknown call on any path, transitively, including the callbacks it runs)
//! is resolved to an empty [`CallScope`] marked `inert`, whatever its depth
//! or recursion. A callee that is not inert and performs no recognized
//! operation still stays an unknown effect; its detail now says why it is
//! not inert.
//!
//! **Concurrent calls.** A call in concurrent position (a `Promise.all`
//! element) is summarized like an awaited one, and every ordered effect and
//! transaction copied from the callee becomes `concurrent`.
//!
//! **Parameters.** An operation on a parameter of the callee
//! ([`DbHandle::Parameter`]) takes the handle the caller passes for it
//! ([`CallArgument`]). When the caller passes something that is not a
//! recognized handle, the operation becomes an unknown effect
//! ([`UnknownReason::UnresolvedHandle`]). Abstract argument values are
//! substituted into the callee's effect provenance and return value, so
//! provenance survives calls and returns; facts about the callee's returned
//! value (`$ret`) are renamed to the caller's binding of the call result.
//!
//! **Callbacks and context sensitivity.** A call in the callee of its own
//! parameter (`await action()`) is linked, at each instantiation, to the
//! function the caller passes for that parameter. The memo key of an
//! expansion is therefore `(callee, remaining depth, callbacks)`, where
//! `callbacks` lists only function-valued arguments for parameters the
//! callee may run; at most [`MAX_SPECIALIZATIONS`] distinct contexts are
//! cached per callee. Additional contexts are expanded without caching;
//! cache capacity must not decide whether a callback's effects exist.
//!
//! **Argument effects.** Operations in an argument expression that only run
//! because the callee awaits that parameter (a lazy query builder, an
//! unrecognized call wrapped in `withTimeout(...)`) are lowered as
//! placeholders; they are kept, as `concurrent`, only when the callee is
//! resolved and awaits the parameter.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use crate::absint::summary::Solved;
use crate::absint::{AbsValue, Atom, CapturedValue};
use crate::ir::{
    Arm, Branch, CallId, CallResolution, CallScope, CallSite, DbHandle, DeferredHttpMethod, Effect,
    EffectId, EffectKind, Execution, Exit, ExitKind, Fact, FactInput, FunctionIr, Handler, Literal,
    Loop, Node, Recognition, RecognitionBasis, SummaryOrigin, Switch, TransactionId,
    TransactionInfo, TransactionScope, Try, UnknownReason,
};
use crate::location::{FileId, Location};

use super::AnalysisOptions;
use super::detect::{Point, replay};
use super::paths::{self, Event};

/// Most distinct callback contexts cached per callee.
pub const MAX_SPECIALIZATIONS: usize = 8;

/// The fact variable that stands for a function's returned value in its
/// return facts; renamed to the caller's binding when inlined.
pub const RETURN_VARIABLE: &str = "$ret";

/// A call, in one function's IR, of another function of the function table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCall {
    /// The unknown effect that stands for the call in the caller's IR.
    pub effect: EffectId,
    /// Index of the callee in the function table; `None` when the callee was
    /// not lowered as a function of its own, could not be linked, or is a
    /// callback ([`Self::parameter`]).
    pub callee: Option<usize>,
    /// The effect only marks where the callee's summary goes (for example
    /// `return notify(id)` in an `async` function, which is not otherwise an
    /// unknown effect). It is removed when the call is not summarized.
    pub placeholder: bool,
    /// Resolve only the value of an unawaited synchronous inert factory;
    /// do not introduce its body as an awaited effect scope.
    pub value_only: bool,
    /// The target is the function returned by this earlier call-result site.
    pub returned_callee: Option<u64>,
    /// A closure or module-initialized callable, including lexical values.
    pub callable: Option<AbsValue>,
    /// Invoke the value of a module export as an entry definition.
    pub entrypoint: bool,
    /// The arguments by position: the database handle each passes, or
    /// `None` when it is not a recognized handle (or is a spread).
    pub arguments: Vec<Option<CallArgument>>,
    /// The callee is the function this function's caller passes for this
    /// parameter (`await action()`).
    pub parameter: Option<u32>,
    /// Abstract values of the arguments, by position, in the caller's frame
    /// (functions numbered by table index). Empty when not computed.
    pub values: Vec<AbsValue>,
    /// Abstract value of the receiver (`this` inside the callee).
    pub receiver: Option<AbsValue>,
    /// The arguments as the callee receives them, by position: primitive
    /// literals, parameters of the caller and object literals as closed
    /// records of those (anything else unknown). Unlike [`Self::values`],
    /// taken before the call can change the objects it is passed. Used
    /// only to decide the callee's facts on its
    /// [`FunctionIr::fact_inputs`]. Empty when not computed.
    pub entry_values: Vec<AbsValue>,
    /// How many arguments the call passes, when it is known exactly (no
    /// spread argument): a parameter at or past it receives `undefined`.
    /// `None` when not computed.
    pub argument_count: Option<u32>,
    /// The `CallResult` site that stands for this call's result in the
    /// caller's abstract values.
    pub result: Option<u64>,
    /// The fact variable of the caller's `const` binding of the call's
    /// result (`result#12`), when member tests on it may be correlated with
    /// the callee's returns.
    pub binding: Option<String>,
    /// Facts about properties of the result (`$ret.ok`) may be renamed to
    /// the binding: every reference of it is a property read, so no visible
    /// code changes its properties.
    pub binding_members: bool,
    /// How the target was determined, if not by name.
    pub resolution: Option<CallResolution>,
    /// Effects lowered from argument expressions that run only if the callee
    /// awaits the corresponding parameter: `(parameter, effect)`.
    pub argument_effects: Vec<(u32, EffectId)>,
}

impl LocalCall {
    pub fn new(effect: EffectId, callee: Option<usize>, placeholder: bool) -> Self {
        Self {
            effect,
            callee,
            placeholder,
            value_only: false,
            returned_callee: None,
            callable: None,
            entrypoint: false,
            arguments: Vec::new(),
            parameter: None,
            values: Vec::new(),
            receiver: None,
            entry_values: Vec::new(),
            argument_count: None,
            result: None,
            binding: None,
            binding_members: false,
            resolution: None,
            argument_effects: Vec::new(),
        }
    }
}

/// A recognized database or transaction handle passed as an argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallArgument {
    /// The handle in the caller's IR (a transaction of the caller, a root
    /// handle, or a parameter of the caller).
    pub handle: DbHandle,
    /// Why the argument is that handle.
    pub recognition: Recognition,
    /// The argument as written.
    pub label: String,
}

/// The files of a project-wide function table: `ids[i]` is the file
/// function `i` is declared in, `names[id]` its path.
#[derive(Clone, Copy, Debug)]
pub struct ProjectFiles<'a> {
    pub ids: &'a [FileId],
    pub names: &'a [String],
}

/// Counters describing one [`inline_calls`] run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineStats {
    /// Calls replaced by the callee's summary (inert calls included).
    pub summarized: usize,
    /// Calls kept as unknown effects (or removed placeholders).
    pub not_summarized: usize,
    /// Callee expansions computed.
    pub expansions: usize,
    /// Callee expansions reused from the memo table.
    pub reused: usize,
    /// Reuses found through the depth-canonical key (an expansion
    /// that never reached the depth bound is the same at every depth from
    /// the one it needs), which the exact `(callee, depth, callbacks)` key
    /// misses. Included in `reused`.
    pub canonical_reuses: usize,
    /// Calls resolved as inert.
    pub inert: usize,
    /// Calls in concurrent position that were summarized.
    pub concurrent: usize,
    /// Calls of a parameter linked to the function the caller passed.
    pub callbacks: usize,
    /// Expansions computed for a non-empty callback context.
    pub specializations: usize,
    /// Callback contexts expanded without caching because the cache is full.
    pub specialization_limit: usize,
    /// Argument placeholder effects kept (the callee awaits the parameter)
    /// and dropped.
    pub argument_effects_kept: usize,
    pub argument_effects_dropped: usize,
}

/// Replace the calls of every function with the callees' summaries, for a
/// function table that comes from a single file. `calls[i]` lists the calls
/// in `functions[i]`.
pub fn inline_local_calls(
    functions: &mut [FunctionIr],
    calls: &[Vec<LocalCall>],
    options: &AnalysisOptions,
) -> InlineStats {
    inline_calls(functions, calls, None, options)
}

/// Replace the calls of every function with the callees' summaries.
/// `calls[i]` lists the calls in `functions[i]`; `files`, when given, says
/// which file each function is declared in. Locations inlined from a
/// function of another file are marked with that file ([`Location::file`]).
pub fn inline_calls(
    functions: &mut [FunctionIr],
    calls: &[Vec<LocalCall>],
    files: Option<ProjectFiles<'_>>,
    options: &AnalysisOptions,
) -> InlineStats {
    inline_calls_with(functions, calls, files, None, options)
}

/// Like [`inline_calls`], with solved effect summaries (`solved[i]` belongs
/// to `functions[i]`): calls of inert callees are resolved, and callback
/// contexts only include parameters callees may run.
pub fn inline_calls_with(
    functions: &mut [FunctionIr],
    calls: &[Vec<LocalCall>],
    files: Option<ProjectFiles<'_>>,
    solved: Option<&[Solved]>,
    options: &AnalysisOptions,
) -> InlineStats {
    if calls.iter().all(Vec::is_empty) {
        return InlineStats::default();
    }
    let originals = functions.to_vec();
    let mut inliner = Inliner {
        originals: &originals,
        calls,
        files,
        solved,
        options,
        memo: HashMap::new(),
        canonical: HashMap::new(),
        contexts: HashMap::new(),
        stats: InlineStats::default(),
        direct: Vec::new(),
        requests: Vec::new(),
    };
    for (index, function) in functions.iter_mut().enumerate() {
        if calls.get(index).is_some_and(|sites| !sites.is_empty()) {
            *function = inliner
                .expand(
                    index,
                    &mut vec![(index, Vec::new())],
                    options.max_call_depth,
                    &Context::default(),
                )
                .function;
        }
    }
    inliner.stats
}

/// Prepare a function for analysis on its own: operations on parameters
/// become what the function can know about them without a caller (see
/// [`DbHandle::Parameter`]).
pub fn resolve_parameters(function: &FunctionIr) -> Cow<'_, FunctionIr> {
    let mut any = function
        .transactions
        .iter()
        .any(|t| t.parent_parameter.is_some());
    visit(&function.body, &mut |node| {
        any |= matches!(node, Node::Effect(effect) if parameter_of(&effect.kind).is_some());
    });
    if !any {
        return Cow::Borrowed(function);
    }
    let mut resolved = function.clone();
    for info in &mut resolved.transactions {
        info.parent_parameter = None;
    }
    resolved.body = standalone_nodes(&function.body);
    Cow::Owned(resolved)
}

fn standalone_nodes(nodes: &[Node]) -> Vec<Node> {
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node {
            Node::Effect(effect) => match parameter_of(&effect.kind) {
                None => out.push(node.clone()),
                Some((_, name, true)) => {
                    let mut effect = effect.clone();
                    set_handle(
                        &mut effect.kind,
                        DbHandle::Root {
                            name: name.to_owned(),
                        },
                    );
                    out.push(Node::Effect(effect));
                }
                // An awaited call on an unconfigured parameter is an unknown
                // call, exactly as if it had not been recognized at all.
                Some((_, _, false)) => {
                    if matches!(effect.execution, Execution::Awaited | Execution::Concurrent) {
                        out.push(Node::Effect(Effect {
                            kind: EffectKind::Unknown {
                                reason: UnknownReason::UnrecognizedCall,
                                detail: "no configuration entry or supported API matches it"
                                    .to_owned(),
                                deferred: None,
                            },
                            recognition: None,
                            provenance: None,
                            ..effect.clone()
                        }));
                    }
                }
            },
            _ => out.push(with_bodies(node, &mut |body| standalone_nodes(body))),
        }
    }
    out
}

/// The parameter an operation is issued on: `(index, name, configured)`.
fn parameter_of(kind: &EffectKind) -> Option<(u32, &str, bool)> {
    match kind {
        EffectKind::DatabaseRead {
            handle:
                DbHandle::Parameter {
                    index,
                    name,
                    configured,
                },
            ..
        }
        | EffectKind::DatabaseWrite {
            handle:
                DbHandle::Parameter {
                    index,
                    name,
                    configured,
                },
            ..
        } => Some((*index, name, *configured)),
        _ => None,
    }
}

fn set_handle(kind: &mut EffectKind, new: DbHandle) {
    if let EffectKind::DatabaseRead { handle, .. } | EffectKind::DatabaseWrite { handle, .. } = kind
    {
        *handle = new;
    }
}

/// Which function each callback parameter of the function being expanded
/// stands for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Context {
    callbacks: BTreeMap<u32, AbsValue>,
}

impl Context {
    fn key(&self) -> Vec<(u32, AbsValue)> {
        self.callbacks
            .iter()
            .map(|(p, f)| (*p, f.clone()))
            .collect()
    }
}

/// (callee, remaining depth, callback context).
type MemoKey = (usize, usize, Vec<(u32, AbsValue)>);

/// (callee, callback context): the depth-canonical key.
type CanonicalKey = (usize, Vec<(u32, AbsValue)>);

/// A memo request of a call site: (callee, callback context used).
type Request = (usize, Vec<(u32, AbsValue)>);

/// The expansions in progress, outermost first: (function, callback
/// context). A call is recursive when it would expand one of them again.
/// The same function with other callbacks (`retry(() => retry(save))`) is
/// a different expansion; the depth bound still ends any chain of them.
type Stack = Vec<(usize, Vec<(u32, AbsValue)>)>;

struct Inliner<'a> {
    /// The functions as lowered, before any inlining.
    originals: &'a [FunctionIr],
    calls: &'a [Vec<LocalCall>],
    files: Option<ProjectFiles<'a>>,
    solved: Option<&'a [Solved]>,
    options: &'a AnalysisOptions,
    /// Expansions that do not depend on the call chain, by (callee, depth,
    /// callback context).
    memo: HashMap<MemoKey, Rc<Summary>>,
    /// Expansions that made no depth-bound decision, by (callee,
    /// callback context). Each is the expansion at every remaining depth
    /// from its `needed` depth on.
    canonical: HashMap<CanonicalKey, Rc<Summary>>,
    /// Distinct non-empty callback contexts retained in the cache per callee.
    contexts: HashMap<usize, HashSet<Vec<(u32, AbsValue)>>>,
    stats: InlineStats,
    /// Per expansion in progress (innermost last): call sites it
    /// summarized itself, and the memo requests of its call sites.
    direct: Vec<usize>,
    requests: Vec<Vec<Request>>,
}

/// A callee expanded for inlining, with the operations it performs on every
/// path.
struct Summary {
    expanded: Expanded,
    certain: Certainty,
    /// Phase 8 bookkeeping for [`InlineStats::summarized`]: the call sites
    /// this expansion itself summarized, and the memo requests its call
    /// sites made (callee, callback context used), so that a depth-canonical
    /// reuse accounts exactly as the exact-key memo did.
    direct: usize,
    requests: Vec<Request>,
    /// Per expansion, not per instantiation: the position of each
    /// effect id in visit order (instantiated ids are `base + position`),
    /// and the number of call scopes in the body.
    positions: HashMap<u32, u32>,
    calls: u32,
}

/// Identifier allocation for the function being expanded.
struct Ids {
    initialization_blocked: bool,
    next_effect: u32,
    next_call: u32,
    transactions: Vec<TransactionInfo>,
    /// Fact inputs of instantiated callees that are still inputs of the
    /// function being expanded (a parameter forwarded to the callee).
    fact_inputs: Vec<(String, FactInput)>,
    /// A call was left unsummarized because of the depth bound.
    cut: bool,
    /// A call was left unsummarized because it is recursive: the expansion
    /// depends on the chain of calls being expanded.
    recursive: bool,
    /// The smallest remaining depth at which the same expansion
    /// results (every call site's decision is the same).
    needed: usize,
    /// A decision was made because the remaining depth was 0 (a
    /// cut call, or an inert callee resolved without expanding it): the
    /// expansion is only known for this exact depth.
    exact: bool,
    /// Instantiated return values of summarized calls, by the caller's
    /// `CallResult` site.
    call_results: HashMap<u64, AbsValue>,
}

/// A function with its calls expanded.
struct Expanded {
    initialization_blocked: bool,
    function: FunctionIr,
    /// Some call it makes, directly or through its callees, lies beyond the
    /// depth bound, so the summary may miss operations.
    cut: bool,
    /// Some call it makes was cut as recursive.
    recursive: bool,
    /// See [`Ids::needed`] and [`Ids::exact`].
    needed: usize,
    exact: bool,
}

/// What became of one call site.
struct Outcome {
    node: Option<Node>,
    /// Parameters the resolved callee awaits (argument effects for them are
    /// kept); empty when not summarized.
    awaited: Vec<u32>,
}

impl<'a> Inliner<'a> {
    /// Evaluate only proven-inert module initializers. This interprets the
    /// lowered value graph; it never runs application initialization code.
    fn resolve_value(
        &mut self,
        value: &AbsValue,
        stack: &mut Stack,
        depth: usize,
        remaining: usize,
        blocked: &mut bool,
    ) -> AbsValue {
        if value.is_top() || remaining == 0 {
            return AbsValue::top();
        }
        let mut result = AbsValue::bottom();
        for atom in value.atoms() {
            let part = match atom {
                Atom::ModuleValue { initializer } => {
                    let index = *initializer as usize;
                    if index >= self.originals.len()
                        || self.options.max_call_depth == 0
                        || stack.iter().any(|(f, _)| *f == index)
                        || !self.inert_here(index, &Context::default())
                    {
                        *blocked = true;
                        AbsValue::top()
                    } else {
                        let summary = self.summary(index, stack, depth, &Context::default());
                        if summary.expanded.initialization_blocked {
                            *blocked = true;
                            return AbsValue::top();
                        }
                        stack.push((index, Vec::new()));
                        let value = summary
                            .expanded
                            .function
                            .returns
                            .as_deref()
                            .map_or_else(AbsValue::top, |v| {
                                self.resolve_value(v, stack, depth, remaining - 1, blocked)
                            });
                        stack.pop();
                        value
                    }
                }
                Atom::Closure {
                    id,
                    parameters,
                    captures,
                } => {
                    let captures = captures
                        .iter()
                        .map(|capture| CapturedValue {
                            value: self.resolve_value(
                                &capture.value,
                                stack,
                                depth,
                                remaining - 1,
                                blocked,
                            ),
                            handle: capture.handle.clone(),
                            default: capture.default.clone(),
                        })
                        .collect();
                    crate::absint::value::closure_value(*id, *parameters, captures)
                }
                _ => AbsValue::atom(atom.clone()),
            };
            result.join(&part);
        }
        result
    }
    fn file_of(&self, index: usize) -> Option<FileId> {
        self.files.and_then(|files| files.ids.get(index).copied())
    }

    fn file_name(&self, file: FileId) -> &str {
        self.files
            .and_then(|files| files.names.get(file.0 as usize))
            .map_or("another module", String::as_str)
    }

    fn solved(&self, index: usize) -> Option<&'a Solved> {
        self.solved.and_then(|solved| solved.get(index))
    }

    /// Function `index` with its calls expanded up to `depth` levels.
    /// `stack` holds the expansions in progress, outermost first.
    fn expand(
        &mut self,
        index: usize,
        stack: &mut Stack,
        depth: usize,
        context: &Context,
    ) -> Expanded {
        let originals: &'a [FunctionIr] = self.originals;
        let calls: &'a [Vec<LocalCall>] = self.calls;
        let original = &originals[index];
        let sites: HashMap<EffectId, &'a LocalCall> = calls
            .get(index)
            .map(|calls| calls.iter().map(|call| (call.effect, call)).collect())
            .unwrap_or_default();
        if sites.is_empty() {
            return Expanded {
                initialization_blocked: false,
                function: original.clone(),
                cut: false,
                recursive: false,
                needed: 0,
                exact: false,
            };
        }
        // Argument placeholder effect → (owning call effect, parameter).
        let owners: HashMap<EffectId, (EffectId, u32)> = calls
            .get(index)
            .into_iter()
            .flatten()
            .flat_map(|call| {
                call.argument_effects
                    .iter()
                    .map(move |(parameter, effect)| (*effect, (call.effect, *parameter)))
            })
            .collect();
        let mut next_call = 0;
        visit(&original.body, &mut |node| {
            if let Node::Call(call) = node {
                next_call = next_call.max(call.call.0 + 1);
            }
        });
        let mut ids = Ids {
            initialization_blocked: false,
            next_effect: original
                .effects()
                .iter()
                .map(|effect| effect.id.0 + 1)
                .max()
                .unwrap_or(0),
            // Proven SDK callbacks can already contain lexical call scopes.
            next_call,
            transactions: original.transactions.clone(),
            fact_inputs: Vec::new(),
            cut: false,
            recursive: false,
            needed: 0,
            exact: false,
            call_results: HashMap::new(),
        };
        let body = self.rewrite(
            index,
            &original.body,
            &sites,
            &owners,
            stack,
            depth,
            context,
            &mut ids,
        );
        let mut function = FunctionIr {
            name: original.name.clone(),
            location: original.location,
            is_async: original.is_async,
            transactions: ids.transactions,
            body,
            returns: original.returns.clone(),
            fact_inputs: original
                .fact_inputs
                .iter()
                .map(|(variable, value)| (variable.clone(), value.clone()))
                .chain(std::mem::take(&mut ids.fact_inputs))
                .collect(),
        };
        if !ids.call_results.is_empty() {
            substitute_call_results(&mut function, &ids.call_results);
        }
        Expanded {
            initialization_blocked: ids.initialization_blocked,
            function,
            cut: ids.cut,
            recursive: ids.recursive,
            needed: ids.needed,
            exact: ids.exact,
        }
    }

    /// The summary of `index` for a call `depth` levels from the bound.
    /// Depth and recursion bound expansion; cache capacity only bounds reuse.
    fn summary(
        &mut self,
        index: usize,
        stack: &mut Stack,
        depth: usize,
        context: &Context,
    ) -> Rc<Summary> {
        let key = (index, depth, context.key());
        if let Some(summary) = self.memo.get(&key) {
            self.stats.reused += 1;
            return Rc::clone(summary);
        }
        if let Some(summary) = self.canonical.get(&(index, key.2.clone()))
            && summary.expanded.needed <= depth
        {
            self.stats.reused += 1;
            self.stats.canonical_reuses += 1;
            let summary = Rc::clone(summary);
            self.replay(key, &summary);
            return summary;
        }
        let mut cache_context = true;
        if !context.callbacks.is_empty() {
            let seen = self.contexts.entry(index).or_default();
            if !seen.contains(&key.2) {
                if seen.len() >= MAX_SPECIALIZATIONS {
                    self.stats.specialization_limit += 1;
                    cache_context = false;
                } else {
                    seen.insert(key.2.clone());
                    self.stats.specializations += 1;
                }
            }
        }
        self.stats.expansions += 1;
        stack.push((index, key.2.clone()));
        self.direct.push(0);
        self.requests.push(Vec::new());
        let expanded = self.expand(index, stack, depth, context);
        let direct = self.direct.pop().unwrap_or(0);
        let requests = self.requests.pop().unwrap_or_default();
        stack.pop();
        let certain = certainty(&expanded.function, self.options.max_paths);
        let positions = expanded
            .function
            .effects()
            .iter()
            .enumerate()
            .map(|(offset, effect)| (effect.id.0, u32::try_from(offset).unwrap_or(u32::MAX)))
            .collect();
        let calls = count_calls(&expanded.function.body);
        let summary = Rc::new(Summary {
            expanded,
            certain,
            direct,
            requests,
            positions,
            calls,
        });
        if cache_context && !summary.expanded.recursive {
            if !summary.expanded.exact {
                self.canonical
                    .entry((index, key.2.clone()))
                    .or_insert_with(|| Rc::clone(&summary));
            }
            self.memo.insert(key, Rc::clone(&summary));
        }
        summary
    }

    /// A depth-canonical reuse for the exact `key`: the exact-key memo
    /// would have expanded `key` afresh (and, in turn, every nested key not
    /// yet in it). Record those keys and count their summarized call sites,
    /// so that the memo and [`InlineStats::summarized`] are exactly what the
    /// exact-key memo produces. The expansion is the same (`Summary` is
    /// depth-independent from `needed` on).
    fn replay(&mut self, key: MemoKey, summary: &Rc<Summary>) {
        if self.memo.contains_key(&key) {
            return;
        }
        self.stats.summarized += summary.direct;
        let depth = key.1;
        self.memo.insert(key, Rc::clone(summary));
        for (callee, context) in &summary.requests {
            let nested = (*callee, depth.saturating_sub(1), context.clone());
            if self.memo.contains_key(&nested) {
                continue;
            }
            // Non-exact expansions only request non-exact expansions.
            if let Some(found) = self.canonical.get(&(*callee, context.clone())).cloned() {
                self.replay(nested, &found);
            }
        }
    }

    fn count_summarized(&mut self) {
        self.stats.summarized += 1;
        if let Some(top) = self.direct.last_mut() {
            *top += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rewrite(
        &mut self,
        caller: usize,
        nodes: &[Node],
        sites: &HashMap<EffectId, &'a LocalCall>,
        owners: &HashMap<EffectId, (EffectId, u32)>,
        stack: &mut Stack,
        depth: usize,
        context: &Context,
        ids: &mut Ids,
    ) -> Vec<Node> {
        let mut out = Vec::with_capacity(nodes.len());
        // Argument placeholders waiting for their call (which follows them
        // in the same block).
        let mut pending: Vec<(EffectId, u32, Effect)> = Vec::new();
        for node in nodes {
            if let Node::Effect(effect) = node
                && let Some(&(owner, parameter)) = owners.get(&effect.id)
            {
                pending.push((owner, parameter, effect.clone()));
                continue;
            }
            let site = match node {
                Node::Effect(effect) => sites.get(&effect.id).map(|site| (effect, *site)),
                _ => None,
            };
            match site {
                Some((effect, site)) => {
                    let outcome = self.call_site(caller, effect, site, stack, depth, context, ids);
                    let mut index = 0;
                    while index < pending.len() {
                        if pending[index].0 == effect.id {
                            let (_, parameter, mut argument) = pending.remove(index);
                            if outcome.awaited.contains(&parameter) {
                                self.stats.argument_effects_kept += 1;
                                if argument.execution.is_ordered()
                                    || argument.execution == Execution::FireAndForget
                                {
                                    argument.execution = Execution::Concurrent;
                                }
                                out.push(Node::Effect(argument));
                            } else {
                                self.stats.argument_effects_dropped += 1;
                            }
                        } else {
                            index += 1;
                        }
                    }
                    out.extend(outcome.node);
                }
                None => out.push(with_bodies(node, &mut |body| {
                    self.rewrite(caller, body, sites, owners, stack, depth, context, ids)
                })),
            }
        }
        self.stats.argument_effects_dropped += pending.len();
        out
    }

    /// The callback context for expanding `index` at `site`: function
    /// values passed for parameters the callee may run.
    fn callee_context(&self, index: usize, site: &LocalCall, context: &Context) -> Context {
        let mut callbacks = BTreeMap::new();
        for (slot, value) in site.values.iter().enumerate() {
            let slot = u32::try_from(slot).unwrap_or(u32::MAX);
            let runs = self
                .solved(index)
                .is_none_or(|solved| solved.param(slot as usize).may_run());
            if !runs {
                continue;
            }
            match value.single() {
                Some(Atom::Function { .. } | Atom::Closure { .. }) => {
                    callbacks.insert(slot, value.clone());
                }
                Some(Atom::Param { index: parameter }) => {
                    if let Some(function) = context.callbacks.get(parameter) {
                        callbacks.insert(slot, function.clone());
                    }
                }
                _ => {}
            }
        }
        Context { callbacks }
    }

    /// Whether `index` is inert when called with the callbacks of
    /// `callee_context`.
    fn inert_here(&self, index: usize, callee_context: &Context) -> bool {
        let Some(solved) = self.solved(index) else {
            return false;
        };
        if !solved.is_inert() {
            return false;
        }
        solved.params.iter().enumerate().all(|(slot, used)| {
            if !used.may_run() {
                return true;
            }
            let slot = u32::try_from(slot).unwrap_or(u32::MAX);
            match callee_context.callbacks.get(&slot) {
                Some(function) => self.callable_inert(function, 0),
                // Not a known function: what runs is unknown. (No argument
                // at all: calling `undefined` throws; still not inert.)
                None => false,
            }
        })
    }

    fn callable_inert(&self, value: &AbsValue, depth: usize) -> bool {
        if depth >= crate::absint::value::MAX_FIELD_DEPTH {
            return false;
        }
        let (id, parameters, captures) = match value.single() {
            Some(Atom::Function { id }) => (*id, 0, &[][..]),
            Some(Atom::Closure {
                id,
                parameters,
                captures,
            }) => (*id, *parameters, captures.as_ref()),
            _ => return false,
        };
        let Some(solved) = self.solved(id as usize).filter(|s| s.is_inert()) else {
            return false;
        };
        solved.params.iter().enumerate().all(|(slot, used)| {
            !used.may_run()
                || (slot as u32)
                    .checked_sub(parameters)
                    .and_then(|slot| captures.get(slot as usize))
                    .is_some_and(|capture| self.callable_inert(&capture.value, depth + 1))
        })
    }

    /// The node replacing the effect that stands for a call: the callee's
    /// summary, or the unknown effect with the reason it is kept.
    #[allow(clippy::too_many_arguments)]
    fn call_site(
        &mut self,
        caller: usize,
        effect: &Effect,
        site: &LocalCall,
        stack: &mut Stack,
        depth: usize,
        context: &Context,
        ids: &mut Ids,
    ) -> Outcome {
        let mut effective = site.clone();
        // Earlier factory results may themselves be callbacks passed on to
        // another helper. Resolve those before choosing a callback context.
        for value in &mut effective.values {
            *value = value.substitute(&|atom| match atom {
                Atom::CallResult { site } => ids.call_results.get(site).cloned(),
                Atom::Param { index } => context.callbacks.get(index).cloned(),
                _ => None,
            });
            *value = self.resolve_value(value, stack, depth, 16, &mut ids.initialization_blocked);
        }
        let callable = effective
            .callable
            .clone()
            .or_else(|| {
                effective
                    .returned_callee
                    .and_then(|site| ids.call_results.get(&site).cloned())
            })
            .or_else(|| {
                effective
                    .parameter
                    .and_then(|slot| context.callbacks.get(&slot).cloned())
            })
            .map(|value| {
                value.substitute(&|atom| match atom {
                    Atom::Param { index } => context.callbacks.get(index).cloned(),
                    _ => None,
                })
            })
            .map(|value| {
                self.resolve_value(&value, stack, depth, 16, &mut ids.initialization_blocked)
            });
        let dynamic_index = match callable.as_ref().and_then(AbsValue::single) {
            Some(Atom::Function { id }) => Some(*id as usize),
            Some(Atom::Closure {
                id,
                parameters,
                captures,
            }) => {
                let arity = *parameters as usize;
                effective.values.truncate(arity);
                effective.arguments.truncate(arity);
                effective.entry_values.truncate(arity);
                let missing = if effective.argument_count.is_some() {
                    AbsValue::lit(Literal::Undefined)
                } else {
                    AbsValue::top()
                };
                effective.values.resize(arity, missing.clone());
                effective.arguments.resize(arity, None);
                effective.entry_values.resize(arity, missing);
                for capture in captures {
                    effective.values.push(capture.value.clone());
                    // Only immutable primitives decide a delayed closure's
                    // branches. Objects remain shared, mutable references.
                    effective.entry_values.push(match capture.value.single() {
                        Some(Atom::Lit { .. }) => capture.value.clone(),
                        _ => AbsValue::top(),
                    });
                    effective
                        .arguments
                        .push(capture.handle.clone().map(captured_argument));
                }
                effective.argument_count = Some(effective.values.len() as u32);
                Some(*id as usize)
            }
            _ => None,
        };
        let site = &effective;
        let not_summarized = |this: &mut Self, reason: Option<String>| {
            this.stats.not_summarized += 1;
            if site.placeholder {
                return Outcome {
                    node: None,
                    awaited: Vec::new(),
                };
            }
            let mut effect = effect.clone();
            if let (Some(reason), EffectKind::Unknown { detail, .. }) = (reason, &mut effect.kind) {
                *detail = reason;
            }
            Outcome {
                node: Some(Node::Effect(effect)),
                awaited: Vec::new(),
            }
        };
        // Running a callback parameter does not count toward the depth
        // bound: its body is code of a caller already being expanded, and
        // the wrapper that runs it was already counted. The stack still
        // ends any cycle of callbacks.
        let callback = site.parameter.is_some() || site.entrypoint;
        let index = match (dynamic_index.or(site.callee), site.parameter) {
            (Some(index), _) => index,
            (None, Some(parameter)) => {
                return not_summarized(
                    self,
                    Some(format!(
                        "`{}` calls parameter {} of this function; which function it runs is decided by each caller",
                        effect.label,
                        parameter + 1
                    )),
                );
            }
            (None, None) => {
                return not_summarized(
                    self,
                    site.returned_callee.map(|_| {
                        "the earlier call's result is not a single known function".to_owned()
                    }),
                );
            }
        };
        if site.parameter.is_some() {
            self.stats.callbacks += 1;
        }
        let originals: &'a [FunctionIr] = self.originals;
        let callee = &originals[index];
        let declared = match (self.file_of(caller), self.file_of(index)) {
            (Some(a), Some(b)) if a != b => format!(
                "`{}` is declared in {} at {}",
                callee.name,
                self.file_name(b),
                callee.location
            ),
            _ => format!(
                "`{}` is declared in this file at {}",
                callee.name, callee.location
            ),
        };
        let callee_context = self.callee_context(index, site, context);
        let awaited: Vec<u32> = self
            .solved(index)
            .map(|solved| {
                solved
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(_, used)| used.awaited)
                    .map(|(slot, _)| u32::try_from(slot).unwrap_or(u32::MAX))
                    .collect()
            })
            .unwrap_or_default();
        let inert = self.options.max_call_depth > 0 && self.inert_here(index, &callee_context);
        if site.value_only {
            // Plain calls remain outside effect summaries. A synchronous,
            // inert factory can nevertheless tell us which function a later
            // awaited call runs. An async factory returns a promise here.
            if inert && !callee.is_async {
                let summary = (depth > 0
                    && !stack
                        .iter()
                        .any(|(f, used)| *f == index && *used == callee_context.key()))
                .then(|| self.summary(index, stack, depth - 1, &callee_context));
                if let Some(summary) = &summary {
                    ids.initialization_blocked |= summary.expanded.initialization_blocked;
                    ids.needed = ids.needed.max(summary.expanded.needed + 1);
                    ids.exact |= summary.expanded.exact;
                }
                let returned = summary
                    .as_ref()
                    .and_then(|s| s.expanded.function.returns.as_deref());
                let mut outcome =
                    self.inert_call(caller, index, effect, site, awaited, ids, returned);
                outcome.node = None;
                return outcome;
            }
            return not_summarized(self, None);
        }
        let solved_here = self.solved(index);
        let not_inert = || -> String {
            match solved_here.map(|solved| &solved.inert) {
                Some(Err(reason)) => format!("; it is not proven inert: {reason}"),
                Some(Ok(())) => "; it is inert only for some arguments (a callback it runs is not a known inert function)".to_owned(),
                None => String::new(),
            }
        };
        let reason = if self.options.max_call_depth == 0 {
            format!("{declared}, but call summaries are disabled")
        } else if stack
            .iter()
            .any(|(f, used)| *f == index && *used == callee_context.key())
        {
            if inert {
                return self.inert_call(caller, index, effect, site, awaited, ids, None);
            }
            ids.recursive = true;
            format!("{declared}, but the call is recursive; recursive calls are not summarized")
        } else if depth == 0 && !callback {
            // A decision of the depth bound: this expansion holds for this
            // exact depth only.
            ids.exact = true;
            if inert {
                return self.inert_call(caller, index, effect, site, awaited, ids, None);
            }
            ids.cut = true;
            format!(
                "{declared}, but it is more than {} call(s) deep; deeper calls are not summarized",
                self.options.max_call_depth
            )
        } else {
            let depth = if callback { depth } else { depth - 1 };
            let summary = self.summary(index, stack, depth, &callee_context);
            let used = callee_context.key();
            if let Some(top) = self.requests.last_mut() {
                top.push((index, used));
            }
            let expanded = &summary.expanded;
            ids.initialization_blocked |= expanded.initialization_blocked;
            ids.needed = ids.needed.max(expanded.needed + 1);
            ids.exact |= expanded.exact;
            if inert && !expanded.function.has_recognized_effects() {
                return self.inert_call(
                    caller,
                    index,
                    effect,
                    site,
                    awaited,
                    ids,
                    expanded.function.returns.as_deref(),
                );
            }
            ids.cut |= expanded.cut;
            ids.recursive |= expanded.recursive;
            if !expanded.function.has_recognized_effects() && expanded.cut {
                format!(
                    "{declared}, but the calls it makes reach more than {} call(s) deep, and deeper calls are not summarized{}",
                    self.options.max_call_depth,
                    not_inert()
                )
            } else if !expanded.function.has_recognized_effects() {
                format!(
                    "{declared}, but it performs no recognized database operation or configured external effect, so its summary is not used{}",
                    not_inert()
                )
            } else if has_unsupported_exit(&expanded.function.body) {
                format!(
                    "{declared}, but it contains control flow Midwrite does not model, so its summary is not used"
                )
            } else {
                let saved = (ids.next_effect, ids.next_call, ids.transactions.len());
                let call = self.instantiate(
                    caller,
                    index,
                    &summary,
                    effect,
                    site,
                    ids,
                    site.entrypoint || !callee_context.callbacks.is_empty(),
                );
                if has_recognized_nodes(&call.body) {
                    self.count_summarized();
                    if effect.execution == Execution::Concurrent {
                        self.stats.concurrent += 1;
                    }
                    return Outcome {
                        node: Some(Node::Call(call)),
                        awaited,
                    };
                }
                // Every operation depended on a handle or a request method
                // this call does not pass.
                ids.next_effect = saved.0;
                ids.next_call = saved.1;
                ids.transactions.truncate(saved.2);
                match decided_http(&expanded.function.body, &call.body) {
                    Some(detail) => format!(
                        "{declared}, but at this call it performs no recognized database operation or configured external effect: {detail}"
                    ),
                    None => format!(
                        "{declared}, but it performs database operations only on its parameters, and this call passes no recognized database or transaction handle for them"
                    ),
                }
            }
        };
        // Awaiting a parameter is source evidence independent of whether
        // the wrapper's own effects can be summarized (for example a query
        // helper with logging in its catch handler).
        let mut outcome = not_summarized(self, Some(reason));
        outcome.awaited = awaited;
        outcome
    }

    /// An empty call scope for a call of an inert callee.
    #[allow(clippy::too_many_arguments)]
    fn inert_call(
        &mut self,
        caller: usize,
        index: usize,
        effect: &Effect,
        site: &LocalCall,
        awaited: Vec<u32>,
        ids: &mut Ids,
        returned: Option<&AbsValue>,
    ) -> Outcome {
        self.count_summarized();
        self.stats.inert += 1;
        let callee = &self.originals[index];
        let call = CallId(ids.next_call);
        ids.next_call += 1;
        if let Some(result) = site.result {
            // An inert callee's value is whatever it returns, with this
            // call's arguments.
            let frame = u64::from(call.0) + 1;
            let values = &site.values;
            let receiver = site.receiver.clone();
            let mut returned =
                returned
                    .or(callee.returns.as_deref())
                    .map_or_else(AbsValue::top, |value| {
                        value.substitute(&|atom| {
                            frame_atom(
                                atom,
                                frame,
                                values,
                                receiver.as_ref(),
                                None,
                                &site.arguments,
                                0,
                                site.argument_count,
                            )
                        })
                    });
            if !returns_on_all_paths(&callee.body) {
                returned.join(&AbsValue::lit(Literal::Undefined));
            }
            ids.call_results.insert(result, returned);
        }
        let callee_location = match (self.file_of(caller), self.file_of(index)) {
            (Some(a), Some(b)) if a != b && callee.location.file.is_none() => Location {
                file: Some(b),
                ..callee.location
            },
            _ => callee.location,
        };
        Outcome {
            node: Some(Node::Call(CallScope {
                call,
                callee: callee.name.as_str().into(),
                label: effect.label.clone(),
                location: effect.location,
                callee_location,
                execution: effect.execution,
                body: Vec::new(),
                resolution: site.resolution.clone().map(Box::new),
                inert: true,
                callback_context: false,
            })),
            awaited,
        }
    }

    /// Copy the summary of `index` into the function being expanded, at the
    /// call `call_effect`, with fresh identifiers and the call's arguments.
    #[allow(clippy::too_many_arguments)]
    fn instantiate(
        &self,
        caller: usize,
        index: usize,
        summary: &Summary,
        call_effect: &Effect,
        site_info: &LocalCall,
        ids: &mut Ids,
        callback_context: bool,
    ) -> CallScope {
        let callee = &summary.expanded.function;
        let certain = &summary.certain;
        let call = CallId(ids.next_call);
        let file = match (self.file_of(caller), self.file_of(index)) {
            (Some(a), Some(b)) if a != b => Some(b),
            _ => None,
        };
        let resolution = site_info.resolution.clone().map(Box::new);
        let site = CallSite {
            call,
            callee: callee.name.as_str().into(),
            label: call_effect.label.clone(),
            location: call_effect.location,
            callee_location: match file {
                Some(file) if callee.location.file.is_none() => Location {
                    file: Some(file),
                    ..callee.location
                },
                _ => callee.location,
            },
            resolution: site_info.resolution.clone().map(std::sync::Arc::new),
        };
        // Effect ids are assigned in the order `Remap::node` visits them,
        // which is the order of `FunctionIr::effects`.
        let effect_ids = EffectIds {
            base: ids.next_effect,
            positions: &summary.positions,
        };
        let remap = Remap {
            site: &site,
            certain,
            call_base: ids.next_call + 1,
            transaction_base: u32::try_from(ids.transactions.len()).unwrap_or(u32::MAX),
            file,
            arguments: &site_info.arguments,
            variables: format!("{call}:"),
            binding: site_info.binding.as_deref(),
            binding_members: site_info.binding_members,
            outer: call_effect.execution,
            // A lazy builder a non-async callee returns runs only if the
            // call's result is awaited; a fire-and-forget call (an unawaited
            // `xs.map(cb)`) never runs it.
            drop_returned: std::cell::Cell::new(call_effect.execution == Execution::FireAndForget),
            frame: u64::from(call.0) + 1,
            values: &site_info.values,
            receiver: site_info.receiver.as_ref(),
            entry_values: &site_info.entry_values,
            argument_count: site_info.argument_count,
            fact_inputs: &callee.fact_inputs,
            effect_ids,
        };
        // A callee input that is an input of the caller too (a forwarded
        // parameter) is decided where the caller is called.
        for (variable, input) in &callee.fact_inputs {
            let value = remap.entry_value(&input.value);
            if is_parameter_path(&value) {
                ids.fact_inputs.push((
                    format!("{}{variable}", remap.variables),
                    FactInput {
                        value,
                        ..input.clone()
                    },
                ));
            }
        }
        ids.next_call += 1 + summary.calls;
        for info in &callee.transactions {
            let (parent, parent_parameter) = match (info.parent, info.parent_parameter) {
                (Some(parent), _) => (Some(remap.transaction(parent)), None),
                (None, Some(parameter)) => {
                    match remap.argument(parameter).map(|argument| &argument.handle) {
                        Some(DbHandle::Transaction { id }) => (Some(*id), None),
                        Some(DbHandle::Parameter { index, .. }) => (None, Some(*index)),
                        _ => (None, None),
                    }
                }
                (None, None) => (None, None),
            };
            ids.transactions.push(TransactionInfo {
                id: remap.transaction(info.id),
                parent,
                label: info.label.clone(),
                location: remap.loc(info.location),
                handle: info.handle.clone(),
                recognition: info.recognition.clone(),
                summary: Some(remap.origin(
                    info.summary.as_ref(),
                    certain.transactions.contains(&info.id),
                )),
                parent_parameter,
            });
        }
        if let Some(result) = site_info.result {
            let mut returned = callee
                .returns
                .as_deref()
                .map_or_else(AbsValue::top, |value| remap.value(value));
            if !returns_on_all_paths(&callee.body) {
                returned.join(&AbsValue::lit(Literal::Undefined));
            }
            ids.call_results.insert(result, returned);
        }
        CallScope {
            call,
            callee: callee.name.as_str().into(),
            label: call_effect.label.clone(),
            location: call_effect.location,
            callee_location: remap.loc(callee.location),
            execution: call_effect.execution,
            body: remap.nodes(&callee.body, &mut ids.next_effect),
            resolution,
            inert: false,
            callback_context,
        }
    }
}

/// Conservative normal-return coverage. Loops and try/finally are left
/// unknown; a possible implicit `undefined` must not become a known callable.
fn returns_on_all_paths(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Exit(exit) => matches!(exit.kind, ExitKind::Return | ExitKind::Throw),
        Node::Branch(branch) => {
            !branch.arms.is_empty()
                && branch
                    .arms
                    .iter()
                    .all(|arm| returns_on_all_paths(&arm.body))
        }
        _ => false,
    })
}

fn captured_argument(handle: DbHandle) -> CallArgument {
    let label = match &handle {
        DbHandle::Root { name } | DbHandle::Parameter { name, .. } => name.clone(),
        DbHandle::Transaction { id } => format!("transaction {}", id.0),
    };
    CallArgument {
        handle,
        recognition: Recognition {
            basis: RecognitionBasis::Parameter,
            receiver: label.clone(),
            detail: "recognized database handle preserved by a lexical capture".to_owned(),
        },
        label,
    }
}

/// An atom of a callee's frame, seen from the caller: parameters and the
/// receiver become the caller's argument values, the callee's effect results
/// the copied effects, and the callee's call results and opaque values get
/// identities unique to this instantiation (`frame`).
#[allow(clippy::too_many_arguments)]
fn frame_atom(
    atom: &Atom,
    frame: u64,
    values: &[AbsValue],
    receiver: Option<&AbsValue>,
    effect_ids: Option<&EffectIds<'_>>,
    arguments: &[Option<CallArgument>],
    transaction_base: u32,
    argument_count: Option<u32>,
) -> Option<AbsValue> {
    match atom {
        Atom::Param { index } => Some(values.get(*index as usize).cloned().unwrap_or_else(|| {
            if argument_count.is_some_and(|count| *index >= count) {
                AbsValue::lit(Literal::Undefined)
            } else {
                AbsValue::top()
            }
        })),
        Atom::This => Some(receiver.cloned().unwrap_or_else(AbsValue::top)),
        Atom::EffectResult { effect } => Some(match effect_ids.and_then(|m| m.get(*effect)) {
            Some(new) => AbsValue::atom(Atom::EffectResult { effect: new }),
            None => AbsValue::top(),
        }),
        Atom::CallResult { site } => Some(AbsValue::atom(Atom::CallResult {
            site: mix(frame, *site),
        })),
        Atom::Opaque { id } => Some(AbsValue::atom(Atom::Opaque {
            id: mix(frame, *id),
        })),
        Atom::Closure {
            id,
            parameters,
            captures,
        } => Some(crate::absint::value::closure_value(
            *id,
            *parameters,
            captures
                .iter()
                .map(|capture| CapturedValue {
                    value: {
                        let value = capture.value.substitute(&|a| {
                            frame_atom(
                                a,
                                frame,
                                values,
                                receiver,
                                effect_ids,
                                arguments,
                                transaction_base,
                                argument_count,
                            )
                        });
                        let value = if value.single() == Some(&Atom::lit(Literal::Undefined)) {
                            capture
                                .default
                                .as_ref()
                                .map_or(value.clone(), |default| AbsValue::lit(default.clone()))
                        } else {
                            value
                        };
                        if value
                            .atoms()
                            .iter()
                            .any(|a| matches!(a, Atom::Object { .. } | Atom::Record { .. }))
                        {
                            AbsValue::top()
                        } else {
                            value
                        }
                    },
                    handle: match &capture.handle {
                        Some(DbHandle::Parameter { index, .. }) => arguments
                            .get(*index as usize)
                            .and_then(|a| a.as_ref())
                            .map(|a| a.handle.clone()),
                        Some(DbHandle::Transaction { id }) => Some(DbHandle::Transaction {
                            id: TransactionId(transaction_base + id.0),
                        }),
                        handle => handle.clone(),
                    },
                    default: capture.default.clone(),
                })
                .collect(),
        )),
        _ => None,
    }
}

/// Whether `value` is one parameter or a property path of one
/// (`param#1.status`): an input a caller can decide.
fn is_parameter_path(value: &AbsValue) -> bool {
    value
        .single()
        .is_some_and(|atom| matches!(atom.root(), Atom::Param { .. }))
}

/// The request methods an options argument with entry value `value` can
/// give `fetch` (uppercase): `when_absent` when it sets no `method` (it is
/// `undefined`, or a closed record without the key; prototype properties
/// are not modeled), `GET` for an `undefined` method. `None` unless every
/// possibility is known.
fn passed_methods(value: &AbsValue, when_absent: Option<&[String]>) -> Option<BTreeSet<String>> {
    let absent = || when_absent.map(|methods| methods.iter().cloned().collect());
    let method = match value.single()? {
        Atom::Lit {
            value: Literal::Undefined,
        } => return absent(),
        Atom::Record { props, open: false } => {
            match props.iter().find(|(key, _)| &**key == "method") {
                Some((_, method)) => method,
                None => return absent(),
            }
        }
        _ => return None,
    };
    if method.atoms().is_empty() {
        return None;
    }
    method
        .atoms()
        .iter()
        .map(|atom| match atom {
            Atom::Lit {
                value: Literal::String(method),
            } => Some(method.to_ascii_uppercase()),
            Atom::Lit {
                value: Literal::Undefined,
            } => Some("GET".to_owned()),
            _ => None,
        })
        .collect()
}

/// Callee effect id → id in the caller: `base` plus the effect's position
/// in visit order.
struct EffectIds<'a> {
    base: u32,
    positions: &'a HashMap<u32, u32>,
}

impl EffectIds<'_> {
    fn get(&self, id: u32) -> Option<u32> {
        self.positions.get(&id).map(|p| self.base + p)
    }
}

/// A 64-bit identity for value `id` of instantiation `frame`, distinct from
/// small caller-frame identities with overwhelming probability.
fn mix(frame: u64, id: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in frame.to_le_bytes().into_iter().chain(id.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash | (1 << 63)
}

/// Replace the caller's `CallResult` atoms of summarized calls by the
/// callees' instantiated return values, in the function's own effects and
/// its returned value.
fn substitute_call_results(function: &mut FunctionIr, results: &HashMap<u64, AbsValue>) {
    let f = |atom: &Atom| match atom {
        Atom::CallResult { site } => results.get(site).cloned(),
        _ => None,
    };
    if let Some(returns) = &mut function.returns {
        **returns = returns.substitute(&f);
    }
    fn walk(nodes: &mut [Node], f: &dyn Fn(&Atom) -> Option<AbsValue>) {
        for node in nodes {
            match node {
                Node::Effect(effect) => {
                    if let Some(provenance) = &mut effect.provenance {
                        **provenance = provenance.substitute(f);
                    }
                }
                Node::Transaction(scope) => walk(&mut scope.body, f),
                Node::Branch(branch) => branch.arms.iter_mut().for_each(|a| walk(&mut a.body, f)),
                Node::Switch(switch) => switch.cases.iter_mut().for_each(|c| walk(&mut c.body, f)),
                Node::Loop(lp) => walk(&mut lp.body, f),
                Node::Call(call) => walk(&mut call.body, f),
                Node::Try(try_node) => {
                    walk(&mut try_node.block, f);
                    if let Some(handler) = &mut try_node.handler {
                        walk(&mut handler.body, f);
                    }
                    if let Some(finalizer) = &mut try_node.finalizer {
                        walk(finalizer, f);
                    }
                }
                Node::Exit(_) => {}
            }
        }
    }
    walk(&mut function.body, &f);
}

/// Identifier translation from a callee's IR into its caller's.
struct Remap<'a> {
    site: &'a CallSite,
    certain: &'a Certainty,
    call_base: u32,
    transaction_base: u32,
    /// The callee's file, when it differs from the caller's.
    file: Option<FileId>,
    arguments: &'a [Option<CallArgument>],
    /// Prefix that makes the callee's variables distinct from the caller's
    /// and from other calls of the same callee.
    variables: String,
    /// The caller's binding of the call's result: the callee's `$ret` facts
    /// are renamed to it.
    binding: Option<&'a str>,
    binding_members: bool,
    /// How the call itself is executed: a concurrent call makes the callee's
    /// ordered operations concurrent, a fire-and-forget call makes them
    /// fire-and-forget (the caller does not wait for them).
    outer: Execution,
    /// Leave out the callee's [`Execution::ReturnedLazy`] builders (outside
    /// transaction scopes): see where it is set.
    drop_returned: std::cell::Cell<bool>,
    frame: u64,
    values: &'a [AbsValue],
    receiver: Option<&'a AbsValue>,
    /// [`LocalCall::entry_values`] of the call.
    entry_values: &'a [AbsValue],
    /// [`LocalCall::argument_count`] of the call.
    argument_count: Option<u32>,
    /// [`FunctionIr::fact_inputs`] of the callee.
    fact_inputs: &'a std::collections::BTreeMap<String, FactInput>,
    /// Callee effect id → id in the caller.
    effect_ids: EffectIds<'a>,
}

impl Remap<'_> {
    fn transaction(&self, id: TransactionId) -> TransactionId {
        TransactionId(self.transaction_base + id.0)
    }

    fn call(&self, id: CallId) -> CallId {
        CallId(self.call_base + id.0)
    }

    fn loc(&self, location: Location) -> Location {
        match (location.file, self.file) {
            (None, Some(file)) => Location {
                file: Some(file),
                ..location
            },
            _ => location,
        }
    }

    fn argument(&self, parameter: u32) -> Option<&CallArgument> {
        self.arguments.get(parameter as usize)?.as_ref()
    }

    /// A callee-frame value in the caller's frame.
    fn value(&self, value: &AbsValue) -> AbsValue {
        value.substitute(&|atom| {
            frame_atom(
                atom,
                self.frame,
                self.values,
                self.receiver,
                Some(&self.effect_ids),
                self.arguments,
                self.transaction_base,
                self.argument_count,
            )
        })
    }

    /// A callee input value (over its parameters) as this call passes it.
    fn entry_value(&self, value: &AbsValue) -> AbsValue {
        value.substitute(&|atom| match atom {
            Atom::Param { index } => Some(
                self.entry_values
                    .get(*index as usize)
                    .cloned()
                    .unwrap_or_else(AbsValue::top),
            ),
            Atom::Lit { .. } | Atom::Field { .. } | Atom::Record { .. } => None,
            _ => Some(AbsValue::top()),
        })
    }

    /// Whether this call passes a literal for the input `fact` is about
    /// that contradicts it: the arm (or exit) can never be taken here.
    fn decided_never(&self, fact: &Fact) -> bool {
        let Some(input) = fact.variable().and_then(|v| self.fact_inputs.get(v)) else {
            return false;
        };
        let Some(value) = self.passed_input(input) else {
            return false;
        };
        let passed = Fact::Equals {
            variable: fact.variable().unwrap_or_default().to_owned(),
            literal: value,
        };
        passed.contradicts(fact)
    }

    /// The literal this call passes for a callee input, if it is one.
    fn passed_input(&self, input: &FactInput) -> Option<Literal> {
        let mut settled = input.value.single()?;
        for _ in 0..input.late_reads {
            let Atom::Field { base, .. } = settled else {
                return None;
            };
            settled = base;
        }
        let value = match self.passed_path(settled).single().cloned() {
            Some(Atom::Lit {
                value: Literal::Null | Literal::Undefined,
            }) if input.late_reads > 0 => Literal::Undefined,
            Some(Atom::Lit { value }) if input.late_reads == 0 => value,
            _ => return None,
        };
        match (value, &input.default) {
            (Literal::Null | Literal::Undefined, Some(default)) => Some(default.clone()),
            (value, _) => Some(value),
        }
    }

    /// The value this call passes for a callee parameter path: an omitted
    /// argument is `undefined`, and so is a property of `null` or
    /// `undefined` (see [`FactInput::value`]). A property a record does not
    /// list stays unknown ([`Atom::field`]).
    fn passed_path(&self, atom: &Atom) -> AbsValue {
        match atom {
            Atom::Param { index } => match self.entry_values.get(*index as usize) {
                Some(value) => value.clone(),
                None if self.argument_count.is_some_and(|count| *index >= count) => {
                    AbsValue::lit(Literal::Undefined)
                }
                None => AbsValue::top(),
            },
            Atom::Field { base, name } => {
                let base = self.passed_path(base);
                match base.single() {
                    Some(Atom::Lit {
                        value: Literal::Null | Literal::Undefined,
                    }) => AbsValue::lit(Literal::Undefined),
                    Some(Atom::Record { .. }) => base.field(name),
                    _ => AbsValue::top(),
                }
            }
            Atom::Lit { .. } => AbsValue::atom(atom.clone()),
            _ => AbsValue::top(),
        }
    }

    /// A callee arm fact in the caller.
    fn arm_fact(&self, fact: &Fact) -> Fact {
        if self.decided_never(fact) {
            return Fact::Never;
        }
        fact.renamed(|variable| format!("{}{variable}", self.variables))
    }

    /// A callee fact in the caller: `$ret…` facts concern the caller's
    /// binding of the result; everything else is the callee's own.
    fn fact(&self, fact: &Fact) -> Fact {
        if self.decided_never(fact) {
            return Fact::Never;
        }
        fact.renamed(
            |variable| match (self.binding, variable.strip_prefix(RETURN_VARIABLE)) {
                (Some(binding), Some(rest))
                    if rest.is_empty() || (rest.starts_with('.') && self.binding_members) =>
                {
                    format!("{binding}{rest}")
                }
                _ => format!("{}{variable}", self.variables),
            },
        )
    }

    /// The origin of an operation inlined through this call: this call
    /// first, then the calls it was already reached through in the callee.
    fn origin(&self, inner: Option<&SummaryOrigin>, on_every_path: bool) -> SummaryOrigin {
        let mut calls = vec![self.site.clone()];
        if let Some(inner) = inner {
            calls.extend(inner.calls.iter().map(|site| CallSite {
                call: self.call(site.call),
                location: self.loc(site.location),
                callee_location: self.loc(site.callee_location),
                ..site.clone()
            }));
        }
        SummaryOrigin {
            calls,
            on_every_path,
        }
    }

    /// The kind and recognition of a callee effect in the caller.
    fn effect_kind(&self, effect: &Effect) -> (EffectKind, Option<Box<Recognition>>) {
        let handle = match &effect.kind {
            EffectKind::DatabaseRead { handle, .. } | EffectKind::DatabaseWrite { handle, .. } => {
                handle
            }
            EffectKind::Unknown {
                deferred: Some(deferred),
                ..
            } => return self.deferred_http(effect, deferred),
            other => return (other.clone(), effect.recognition.clone()),
        };
        let (handle, recognition) = match handle {
            DbHandle::Transaction { id } => (
                DbHandle::Transaction {
                    id: self.transaction(*id),
                },
                effect.recognition.clone(),
            ),
            root @ DbHandle::Root { .. } => (root.clone(), effect.recognition.clone()),
            DbHandle::Parameter { index, name, .. } => match self.argument(*index) {
                Some(argument) => (
                    argument.handle.clone(),
                    Some(Box::new(Recognition {
                        basis: argument.recognition.basis,
                        receiver: argument.label.clone(),
                        detail: format!(
                            "`{name}` is parameter {} of `{}`, bound to the argument `{}` passed at {}: {}",
                            index + 1,
                            self.site.callee,
                            argument.label,
                            self.site.location,
                            argument.recognition.detail
                        ),
                    })),
                ),
                None => {
                    let passed = if (*index as usize) < self.arguments.len() {
                        "the argument passed for it is not a recognized database or transaction handle"
                    } else {
                        "no argument is passed for it"
                    };
                    return (
                        EffectKind::Unknown {
                            reason: UnknownReason::UnresolvedHandle,
                            detail: format!(
                                "`{}` is issued on `{name}`, parameter {} of `{}`, but at the call `{}` {passed}",
                                effect.label,
                                index + 1,
                                self.site.callee,
                                self.site.label
                            ),
                            deferred: None,
                        },
                        None,
                    );
                }
            },
        };
        let mut kind = effect.kind.clone();
        set_handle(&mut kind, handle);
        (kind, recognition)
    }

    /// A callee HTTP effect whose method comes from its parameter, decided
    /// by what this call passes for it ([`DeferredHttpMethod`]).
    fn deferred_http(
        &self,
        effect: &Effect,
        deferred: &DeferredHttpMethod,
    ) -> (EffectKind, Option<Box<Recognition>>) {
        let index = deferred.parameter;
        let passed = match self.entry_values.get(index as usize) {
            Some(value) => value.clone(),
            None if self.argument_count.is_some_and(|count| index >= count) => {
                AbsValue::lit(Literal::Undefined)
            }
            None => AbsValue::top(),
        };
        let unknown = |detail: String, deferred: Option<Box<DeferredHttpMethod>>| {
            (
                EffectKind::Unknown {
                    reason: UnknownReason::UnrecognizedCall,
                    detail,
                    deferred,
                },
                None,
            )
        };
        let name = &deferred.name;
        let site = &self.site.label;
        // Passed on from the caller's own parameter: decided one call further
        // out.
        if let Some(Atom::Param { index }) = passed.single() {
            let mut forwarded = deferred.clone();
            forwarded.parameter = *index;
            forwarded.provenance = deferred.provenance.as_ref().map(|provenance| {
                provenance.substitute(&|atom| {
                    frame_atom(
                        atom,
                        self.frame,
                        self.values,
                        self.receiver,
                        Some(&self.effect_ids),
                        self.arguments,
                        self.transaction_base,
                        self.argument_count,
                    )
                })
            });
            return unknown(
                format!(
                    "the request method of `{}` comes from `{name}`, which the call `{site}` passes on from a parameter; it is decided where that function is called",
                    effect.label
                ),
                Some(Box::new(forwarded)),
            );
        }
        let Some(methods) = passed_methods(&passed, deferred.when_absent.as_deref()) else {
            return unknown(
                format!(
                    "the request method of `{}` comes from `{name}`, and the call `{site}` passes no literal method for it",
                    effect.label
                ),
                None,
            );
        };
        let listed = |method: &String| deferred.methods.contains(method);
        let shown = methods.iter().cloned().collect::<Vec<_>>().join(" or ");
        if methods.iter().all(listed) {
            let mut recognition = deferred.recognition.clone();
            recognition.detail.push_str(&format!(
                "; the request method {shown} comes from `{name}` as passed by the call `{site}`"
            ));
            return (
                EffectKind::External {
                    category: deferred.category,
                    best_effort: deferred.best_effort,
                    recovery: deferred.recovery,
                },
                Some(Box::new(recognition)),
            );
        }
        let detail = if methods.iter().any(listed) {
            format!(
                "the call `{site}` makes `{}` use {shown}, and not every one is among the rule's httpMethods",
                effect.label
            )
        } else {
            format!(
                "the call `{site}` makes `{}` use {shown}, which no `httpMethods` entry lists, so no configuration entry matches it",
                effect.label
            )
        };
        unknown(detail, None)
    }

    fn execution(&self, execution: Execution) -> Execution {
        // The call is consumed, so a builder the callee returns runs.
        let execution = match execution {
            Execution::ReturnedLazy if self.outer != Execution::ReturnedLazy => Execution::Returned,
            other => other,
        };
        match self.outer {
            Execution::Concurrent if execution.is_ordered() => Execution::Concurrent,
            Execution::FireAndForget if execution.is_ordered() => Execution::FireAndForget,
            _ => execution,
        }
    }

    fn nodes(&self, nodes: &[Node], next_effect: &mut u32) -> Vec<Node> {
        let mut out = Vec::with_capacity(nodes.len());
        for node in nodes {
            if self.drop_returned.get()
                && matches!(node, Node::Effect(effect) if effect.execution == Execution::ReturnedLazy)
            {
                // Keep the ids of the remaining effects where `positions`
                // puts them.
                *next_effect += 1;
                continue;
            }
            out.push(self.node(node, next_effect));
        }
        out
    }

    fn arms(&self, arms: &[Arm], next_effect: &mut u32) -> Vec<Arm> {
        arms.iter()
            .map(|arm| Arm {
                label: arm.label.clone(),
                body: self.nodes(&arm.body, next_effect),
                facts: arm.facts.iter().map(|fact| self.arm_fact(fact)).collect(),
            })
            .collect()
    }

    fn node(&self, node: &Node, next_effect: &mut u32) -> Node {
        match node {
            Node::Effect(effect) => {
                let id = EffectId(*next_effect);
                *next_effect += 1;
                let (kind, recognition) = self.effect_kind(effect);
                let source = match &effect.kind {
                    EffectKind::Unknown {
                        deferred: Some(deferred),
                        ..
                    } => deferred.provenance.as_ref(),
                    _ => effect.provenance.as_deref(),
                };
                let provenance = match (&kind, source) {
                    (EffectKind::Unknown { .. }, _) | (_, None) => None,
                    (_, Some(provenance)) => Some(Box::new(provenance.substitute(&|atom| {
                        frame_atom(
                            atom,
                            self.frame,
                            self.values,
                            self.receiver,
                            Some(&self.effect_ids),
                            self.arguments,
                            self.transaction_base,
                            self.argument_count,
                        )
                    }))),
                };
                Node::Effect(Effect {
                    id,
                    kind,
                    label: effect.label.clone(),
                    location: self.loc(effect.location),
                    execution: self.execution(effect.execution),
                    recognition,
                    summary: Some(self.origin(
                        effect.summary.as_ref(),
                        self.certain.effects.contains(&effect.id),
                    )),
                    // Names in the callee's scope are not comparable with the
                    // caller's; not copied.
                    references: Box::default(),
                    provenance,
                })
            }
            Node::Transaction(scope) => {
                // Operations a transaction callback returns run with it.
                let dropping = self.drop_returned.replace(false);
                let body = self.nodes(&scope.body, next_effect);
                self.drop_returned.set(dropping);
                Node::Transaction(TransactionScope {
                    transaction: self.transaction(scope.transaction),
                    execution: self.execution(scope.execution),
                    body,
                })
            }
            Node::Call(call) => Node::Call(CallScope {
                call: self.call(call.call),
                callee: call.callee.clone(),
                label: call.label.clone(),
                location: self.loc(call.location),
                callee_location: self.loc(call.callee_location),
                execution: self.execution(call.execution),
                body: self.nodes(&call.body, next_effect),
                resolution: call.resolution.clone(),
                inert: call.inert,
                callback_context: call.callback_context,
            }),
            Node::Exit(exit) => Node::Exit(Exit {
                transaction: exit.transaction.map(|id| self.transaction(id)),
                location: self.loc(exit.location),
                facts: exit.facts.iter().map(|fact| self.fact(fact)).collect(),
                ..exit.clone()
            }),
            Node::Branch(branch) => Node::Branch(Branch {
                kind: branch.kind,
                condition: branch.condition.clone(),
                location: self.loc(branch.location),
                arms: self.arms(&branch.arms, next_effect),
            }),
            Node::Switch(switch) => Node::Switch(Switch {
                discriminant: switch.discriminant.clone(),
                location: self.loc(switch.location),
                cases: self.arms(&switch.cases, next_effect),
                has_default: switch.has_default,
            }),
            Node::Loop(lp) => Node::Loop(Loop {
                kind: lp.kind,
                header: lp.header.clone(),
                location: self.loc(lp.location),
                runs_at_least_once: lp.runs_at_least_once,
                body: self.nodes(&lp.body, next_effect),
            }),
            Node::Try(try_node) => Node::Try(Try {
                location: self.loc(try_node.location),
                block: self.nodes(&try_node.block, next_effect),
                handler: try_node.handler.as_ref().map(|handler| Handler {
                    location: self.loc(handler.location),
                    body: self.nodes(&handler.body, next_effect),
                }),
                finalizer: try_node
                    .finalizer
                    .as_ref()
                    .map(|finalizer| self.nodes(finalizer, next_effect)),
            }),
        }
    }
}

/// Operations a function performs on every path through it.
#[derive(Debug, Default)]
struct Certainty {
    /// Effects executed on every path.
    effects: BTreeSet<EffectId>,
    /// Transactions that commit at least one write on every path.
    transactions: BTreeSet<TransactionId>,
}

fn certainty(function: &FunctionIr, max_paths: usize) -> Certainty {
    let paths = paths::enumerate(function, max_paths);
    if paths.truncated {
        // Unexplored paths may lack any operation.
        return Certainty::default();
    }
    let mut result: Option<Certainty> = None;
    for trace in &paths.traces {
        let effects: BTreeSet<EffectId> = trace
            .iter()
            .filter_map(|event| match event {
                Event::Effect { effect, .. } => Some(effect.id),
                _ => None,
            })
            .collect();
        let transactions: BTreeSet<TransactionId> = replay(function, trace)
            .points
            .iter()
            .filter_map(|point| match point {
                Point::Commit {
                    transaction: Some(id),
                    ..
                } => Some(*id),
                _ => None,
            })
            .collect();
        result = Some(match result {
            None => Certainty {
                effects,
                transactions,
            },
            Some(previous) => Certainty {
                effects: previous.effects.intersection(&effects).copied().collect(),
                transactions: previous
                    .transactions
                    .intersection(&transactions)
                    .copied()
                    .collect(),
            },
        });
    }
    result.unwrap_or_default()
}

/// Clone `node`, replacing each body it directly contains with `f(body)`.
fn with_bodies(node: &Node, f: &mut dyn FnMut(&[Node]) -> Vec<Node>) -> Node {
    let arms = |arms: &[Arm], f: &mut dyn FnMut(&[Node]) -> Vec<Node>| -> Vec<Arm> {
        arms.iter()
            .map(|arm| Arm {
                label: arm.label.clone(),
                body: f(&arm.body),
                facts: arm.facts.clone(),
            })
            .collect()
    };
    match node {
        Node::Effect(_) | Node::Exit(_) => node.clone(),
        Node::Transaction(scope) => Node::Transaction(TransactionScope {
            transaction: scope.transaction,
            execution: scope.execution,
            body: f(&scope.body),
        }),
        Node::Branch(branch) => Node::Branch(Branch {
            kind: branch.kind,
            condition: branch.condition.clone(),
            location: branch.location,
            arms: arms(&branch.arms, f),
        }),
        Node::Switch(switch) => Node::Switch(Switch {
            discriminant: switch.discriminant.clone(),
            location: switch.location,
            cases: arms(&switch.cases, f),
            has_default: switch.has_default,
        }),
        Node::Loop(lp) => Node::Loop(Loop {
            kind: lp.kind,
            header: lp.header.clone(),
            location: lp.location,
            runs_at_least_once: lp.runs_at_least_once,
            body: f(&lp.body),
        }),
        Node::Try(try_node) => Node::Try(Try {
            location: try_node.location,
            block: f(&try_node.block),
            handler: try_node.handler.as_ref().map(|handler| Handler {
                location: handler.location,
                body: f(&handler.body),
            }),
            finalizer: try_node.finalizer.as_deref().map(&mut *f),
        }),
        Node::Call(call) => Node::Call(CallScope {
            call: call.call,
            callee: call.callee.clone(),
            label: call.label.clone(),
            location: call.location,
            callee_location: call.callee_location,
            execution: call.execution,
            body: f(&call.body),
            resolution: call.resolution.clone(),
            inert: call.inert,
            callback_context: call.callback_context,
        }),
    }
}

/// The bodies `node` directly contains.
fn bodies(node: &Node) -> Vec<&[Node]> {
    match node {
        Node::Effect(_) | Node::Exit(_) => Vec::new(),
        Node::Transaction(scope) => vec![&scope.body],
        Node::Branch(branch) => branch.arms.iter().map(|arm| &arm.body[..]).collect(),
        Node::Switch(switch) => switch.cases.iter().map(|case| &case.body[..]).collect(),
        Node::Loop(lp) => vec![&lp.body],
        Node::Try(try_node) => {
            let mut out = vec![&try_node.block[..]];
            out.extend(try_node.handler.iter().map(|handler| &handler.body[..]));
            out.extend(try_node.finalizer.as_deref());
            out
        }
        Node::Call(call) => vec![&call.body],
    }
}

/// Visit every node of `nodes`, recursively.
pub(super) fn visit(nodes: &[Node], f: &mut dyn FnMut(&Node)) {
    for node in nodes {
        f(node);
        for body in bodies(node) {
            visit(body, f);
        }
    }
}

fn has_unsupported_exit(nodes: &[Node]) -> bool {
    let mut found = false;
    visit(nodes, &mut |node| {
        found |= matches!(node, Node::Exit(exit) if exit.kind == ExitKind::Unsupported);
    });
    found
}

/// Whether `nodes` contain a transaction or a recognized effect. Operations
/// on parameters the caller could not bind are unknown effects by now.
fn has_recognized_nodes(nodes: &[Node]) -> bool {
    let mut found = false;
    visit(nodes, &mut |node| {
        found |= match node {
            Node::Transaction(_) => true,
            Node::Effect(effect) => effect.is_recognized(),
            _ => false,
        };
    });
    found
}

/// When `callee` defers a request method to its callers: why the copy of
/// it at a call (`instantiated`) is unknown, taken from its first unknown
/// effect.
fn decided_http(callee: &[Node], instantiated: &[Node]) -> Option<String> {
    let mut deferred = false;
    visit(callee, &mut |node| {
        deferred |= matches!(
            node,
            Node::Effect(Effect {
                kind: EffectKind::Unknown {
                    deferred: Some(_),
                    ..
                },
                ..
            })
        );
    });
    let mut detail = None;
    visit(instantiated, &mut |node| {
        if let Node::Effect(Effect {
            kind: EffectKind::Unknown { detail: text, .. },
            ..
        }) = node
            && detail.is_none()
        {
            detail = Some(text.clone());
        }
    });
    detail.filter(|_| deferred)
}

fn count_calls(nodes: &[Node]) -> u32 {
    let mut count = 0;
    visit(nodes, &mut |node| {
        if matches!(node, Node::Call(_)) {
            count += 1;
        }
    });
    count
}

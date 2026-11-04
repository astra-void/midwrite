//! Effect recognition for call chains.
//!
//! A call such as `db.update(orders).set({...}).where(...)` is *unwound* into
//! a root (an identifier or `this`) and a list of property/call segments. An
//! identifier root is resolved to its provenance ([`crate::bindings`]); then
//! the root and the leading properties are matched against the configured
//! receiver paths (`db`, `ctx.db`, `this.db`), and the remaining segments
//! against the supported Drizzle API or the configured external effects.
//! Anything else is [`Classified::NotRecognized`].

use oxc_ast::ast::{Argument, CallExpression, Expression, IdentifierReference};
use oxc_semantic::Scoping;
use oxc_span::GetSpan;

use midwrite_core::config::{Config, ExternalTarget, ReceiverRule, is_receiver_path};
use midwrite_core::ir::{
    DbHandle, ExternalCategory, Recognition, RecognitionBasis, TransactionId, UnknownReason,
    WriteOperation,
};

use midwrite_core::analysis::CallArgument;

use crate::bindings::{LocalBindings, Origin, Resolved, SymbolTable, resolve, resolve_reference};

pub(crate) use crate::semantic::{Chain, Root, Segment, unwind};

/// Whether reading a Drizzle query or writing with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DbAccess {
    Read,
    Write(WriteOperation),
}

/// A recognized Drizzle query builder.
#[derive(Clone, Debug)]
pub(crate) struct Builder {
    pub access: DbAccess,
    pub table: Option<String>,
    pub handle: DbHandle,
    pub recognition: Recognition,
    pub label: String,
}

/// Whether a builder chain executes by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Marker {
    /// A lazy builder: it executes only when awaited (or adopted by an
    /// async `return`, or `Promise.all`).
    Lazy,
    /// Ends in `.execute()`, `.then()`, `.catch()` or `.finally()`.
    Explicit,
    /// Turned into a prepared statement or SQL text; not executed here.
    Never,
}

#[derive(Debug)]
pub(crate) enum Classified<'e, 'a> {
    Db {
        builder: Builder,
        marker: Marker,
        calls: Vec<&'e CallExpression<'a>>,
    },
    Transaction {
        call: &'e CallExpression<'a>,
        handle: DbHandle,
        recognition: Recognition,
        label: String,
    },
    Rollback {
        transaction: TransactionId,
    },
    DbUnknown {
        reason: UnknownReason,
        detail: String,
        label: String,
        marker: Marker,
        calls: Vec<&'e CallExpression<'a>>,
    },
    External {
        category: ExternalCategory,
        best_effort: bool,
        recovery: Option<midwrite_core::ir::Recovery>,
        recognition: Recognition,
        label: String,
        calls: Vec<&'e CallExpression<'a>>,
    },
    /// `Promise.all([...])` and friends.
    Concurrent {
        call: &'e CallExpression<'a>,
    },
    NotRecognized,
}

const WRITE_METHODS: [(&str, WriteOperation); 3] = [
    ("insert", WriteOperation::Insert),
    ("update", WriteOperation::Update),
    ("delete", WriteOperation::Delete),
];
const SELECT_METHODS: [&str; 4] = ["select", "selectDistinct", "selectDistinctOn", "$count"];
const EXECUTING_METHODS: [&str; 4] = ["execute", "then", "catch", "finally"];
const NON_EXECUTING_METHODS: [&str; 3] = ["prepare", "toSQL", "getSQL"];
const PROMISE_CONTINUATIONS: [&str; 3] = ["then", "catch", "finally"];
const CONCURRENT_COMBINATORS: [&str; 4] = ["all", "allSettled", "race", "any"];

/// A configured receiver path for lookalike detection: `(kind,
/// configuration field, path, properties)`.
pub(crate) type LookalikePath = (&'static str, &'static str, String, Vec<String>);

pub(crate) struct Recognizer<'c> {
    pub config: &'c Config,
    pub scoping: &'c Scoping,
    pub table: &'c SymbolTable,
    pub source: &'c str,
    /// [`Self::lookalike_paths`], computed once (it was rebuilt for every
    /// classified call).
    pub lookalikes: std::cell::OnceCell<Vec<LookalikePath>>,
}

impl Recognizer<'_> {
    pub fn resolve(&self, ident: &IdentifierReference<'_>, locals: &LocalBindings) -> Resolved {
        resolve(ident, self.scoping, self.table, locals)
    }

    pub fn classify<'e, 'a>(
        &self,
        chain: &Chain<'e, 'a>,
        locals: &LocalBindings,
    ) -> Classified<'e, 'a> {
        if !matches!(chain.segments.last(), Some(Segment::Call(_))) {
            return Classified::NotRecognized;
        }
        let Some(root) = chain.root else {
            return self.lookalike(chain);
        };
        let resolved = match root {
            Root::Identifier(ident) => Some(self.resolve(ident, locals)),
            Root::This(_) => None,
        };

        if let Some(resolved) = &resolved {
            if let Origin::PendingBuilder(symbol) = resolved.origin
                && let Some(builder) = locals.pending_builders.get(&symbol)
            {
                return Classified::Db {
                    builder: builder.clone(),
                    marker: marker(&chain.segments),
                    calls: calls(&chain.segments),
                };
            }
            if let Some((handle, recognition)) = provenance_handle(resolved) {
                return self.classify_db(chain, root, 0, handle, recognition);
            }
        }
        let parameter = match root {
            Root::Identifier(ident) => self.parameter(ident, locals),
            Root::This(_) => None,
        };
        if let Some((length, handle, recognition)) =
            self.configured_database(chain, root, resolved.as_ref())
        {
            // A configured name that is a parameter of this function: which
            // handle it holds is up to each caller.
            let handle = match (&parameter, length) {
                (Some((index, name)), 0) => DbHandle::Parameter {
                    index: *index,
                    name: name.clone(),
                    configured: true,
                },
                _ => handle,
            };
            return self.classify_db(chain, root, length, handle, recognition);
        }
        // A Drizzle-shaped read or write on any other parameter: a database
        // operation only if a caller passes a database handle for it.
        // Everything else on such a parameter is classified as usual.
        if let Some((index, name)) = parameter {
            let recognition = Recognition {
                basis: RecognitionBasis::Parameter,
                receiver: name.clone(),
                detail: format!("`{name}` is parameter {} of this function", index + 1),
            };
            let handle = DbHandle::Parameter {
                index,
                name,
                configured: false,
            };
            if let classified @ Classified::Db { .. } =
                self.classify_db(chain, root, 0, handle, recognition)
            {
                return classified;
            }
        }
        if let Some(classified) = self.classify_external(chain, root, resolved.as_ref()) {
            return classified;
        }
        if let Some(resolved) = &resolved
            && resolved.origin == Origin::Unresolved
            && resolved.name == "Promise"
            && let [Segment::Property(method), Segment::Call(call)] = chain.segments[..]
            && CONCURRENT_COMBINATORS.contains(&method)
        {
            return Classified::Concurrent { call };
        }
        self.lookalike(chain)
    }

    /// The position and name of `ident` if it names a parameter of the
    /// function being lowered.
    pub fn parameter(
        &self,
        ident: &IdentifierReference<'_>,
        locals: &LocalBindings,
    ) -> Option<(u32, String)> {
        let symbol = resolve_reference(self.scoping, ident)?;
        locals.params.get(&symbol).cloned()
    }

    /// The database handle an argument expression denotes: a transaction
    /// handle, a `drizzle(...)` instance, a configured receiver (`db`,
    /// `this.db`), or a parameter of the function being lowered. Anything
    /// else (including calls such as `getDb()`) is `None`.
    pub fn argument_handle(
        &self,
        expression: &Expression<'_>,
        locals: &LocalBindings,
    ) -> Option<CallArgument> {
        let chain = unwind(expression);
        let root = chain.root?;
        let label = compact(self.slice(root.start(), receiver_chain_end(expression)));
        let resolved = match root {
            Root::Identifier(ident) => Some(self.resolve(ident, locals)),
            Root::This(_) => None,
        };
        if chain.segments.is_empty()
            && let Some(resolved) = &resolved
            && let Some((handle, recognition)) = provenance_handle(resolved)
        {
            return Some(CallArgument {
                handle,
                recognition,
                label,
            });
        }
        let parameter = match root {
            Root::Identifier(ident) if chain.segments.is_empty() => self.parameter(ident, locals),
            _ => None,
        };
        let configured = self
            .config
            .database_receivers
            .iter()
            .filter_map(|rule| {
                let length = match_path(
                    &chain,
                    root,
                    resolved.as_ref(),
                    rule.name(),
                    rule.imported_from(),
                )?;
                (length == chain.segments.len()).then_some((length, rule))
            })
            .max_by_key(|(length, _)| *length);
        match (parameter, configured) {
            (Some((index, name)), configured) => Some(CallArgument {
                recognition: Recognition {
                    basis: if configured.is_some() {
                        RecognitionBasis::ConfiguredName
                    } else {
                        RecognitionBasis::Parameter
                    },
                    receiver: name.clone(),
                    detail: format!(
                        "`{name}` is parameter {} of the calling function",
                        index + 1
                    ),
                },
                handle: DbHandle::Parameter {
                    index,
                    name,
                    configured: configured.is_some(),
                },
                label,
            }),
            (None, Some((length, rule))) => {
                let receiver = receiver_text(root, &chain.segments[..length]);
                Some(CallArgument {
                    recognition: Recognition {
                        basis: configured_basis(rule.imported_from().is_some(), rule.name()),
                        receiver: receiver.clone(),
                        detail: format!(
                            "matches databaseReceivers entry `{}`; {}",
                            rule.name(),
                            root_detail(resolved.as_ref(), &receiver, rule.name())
                        ),
                    },
                    handle: DbHandle::Root { name: receiver },
                    label,
                })
            }
            (None, None) => None,
        }
    }

    /// `this.repo.db.update(...)`, `deps.queue.publish(...)`: a configured
    /// receiver path (without a leading `this`) reached through some other
    /// expression. Its path is not configured, so it is reported as unknown
    /// instead of being silently ignored or guessed.
    fn lookalike<'e, 'a>(&self, chain: &Chain<'e, 'a>) -> Classified<'e, 'a> {
        let configured = self.lookalikes.get_or_init(|| self.lookalike_paths());
        let segments = &chain.segments;
        for index in 0..segments.len() {
            for (kind, field, path, properties) in configured {
                let end = index + properties.len();
                let path_matches = properties.iter().enumerate().all(|(offset, expected)| {
                    matches!(segments.get(index + offset), Some(Segment::Property(name)) if name == expected)
                });
                let (true, Some(Segment::Property(_)), Some(Segment::Call(call))) =
                    (path_matches, segments.get(end), segments.get(end + 1))
                else {
                    continue;
                };
                let start = chain
                    .root
                    .map_or_else(|| segments_start(segments), |root| root.start());
                let receiver = compact(self.slice(start, receiver_end(segments, end - 1)));
                let label = truncate(&compact(self.slice(start, call.span.end)), 60);
                let hint = if is_receiver_path(&receiver) {
                    format!("; add `{receiver}` to `{field}` to analyze it")
                } else {
                    String::new()
                };
                return Classified::DbUnknown {
                    reason: UnknownReason::UnsupportedReceiver,
                    detail: format!(
                        "`{receiver}` resembles the configured {kind} receiver `{path}`, but its path is not configured{hint}"
                    ),
                    label,
                    marker: marker(&segments[end..]),
                    calls: calls(segments),
                };
            }
        }
        Classified::NotRecognized
    }

    /// Configured receiver paths as property sequences without a leading
    /// `this`: `(kind, configuration field, path, properties)`.
    fn lookalike_paths(&self) -> Vec<LookalikePath> {
        let database = self
            .config
            .database_receivers
            .iter()
            .map(|rule| ("database", "databaseReceivers", rule.name()));
        let external = self
            .config
            .external_effects
            .iter()
            .filter_map(|rule| match rule.target() {
                Some(ExternalTarget::Method { receiver, .. }) => {
                    Some(("external-effect", "externalEffects", receiver))
                }
                _ => None,
            });
        database
            .chain(external)
            .map(|(kind, field, path)| {
                let properties: Vec<&str> = path
                    .split('.')
                    .skip_while(|segment| *segment == THIS)
                    .collect();
                (kind, field, path, properties)
            })
            // A bare `this` is rejected by validation; never match it here.
            // Call paths (`getDb()`) have no lookalike form.
            .filter(|(_, _, path, properties)| !properties.is_empty() && !path.contains('('))
            .map(|(kind, field, path, properties)| {
                (
                    kind,
                    field,
                    path.to_owned(),
                    properties.into_iter().map(str::to_owned).collect(),
                )
            })
            .collect()
    }

    /// The `databaseReceivers` entry with the longest path that the chain
    /// starts with. Returns the number of property segments the path covers
    /// after the root, the handle and the evidence.
    fn configured_database(
        &self,
        chain: &Chain<'_, '_>,
        root: Root<'_, '_>,
        resolved: Option<&Resolved>,
    ) -> Option<(usize, DbHandle, Recognition)> {
        let mut best: Option<(usize, &ReceiverRule)> = None;
        for rule in &self.config.database_receivers {
            let Some(length) = match_path(chain, root, resolved, rule.name(), rule.imported_from())
            else {
                continue;
            };
            if best.is_none_or(|(longest, _)| length > longest) {
                best = Some((length, rule));
            }
        }
        let (length, rule) = best?;
        let receiver = receiver_text(root, &chain.segments[..length]);
        let detail = format!(
            "matches databaseReceivers entry `{}`; {}",
            rule.name(),
            root_detail(resolved, &receiver, rule.name())
        );
        Some((
            length,
            DbHandle::Root {
                name: receiver.clone(),
            },
            Recognition {
                basis: configured_basis(rule.imported_from().is_some(), rule.name()),
                receiver,
                detail,
            },
        ))
    }

    /// Classify the Drizzle call that follows the receiver, which covers the
    /// root and the first `receiver_length` segments of the chain.
    fn classify_db<'e, 'a>(
        &self,
        chain: &Chain<'e, 'a>,
        root: Root<'e, 'a>,
        receiver_length: usize,
        handle: DbHandle,
        recognition: Recognition,
    ) -> Classified<'e, 'a> {
        let segments = &chain.segments[receiver_length..];
        let receiver = recognition.receiver.clone();
        // Skip a CTE prefix: `db.with(cte).update(...)`.
        let mut start = 0;
        while let (Some(Segment::Property("with" | "$with")), Some(Segment::Call(_))) =
            (segments.get(start), segments.get(start + 1))
        {
            start += 2;
        }
        let Some(Segment::Property(method)) = segments.get(start).copied() else {
            return Classified::NotRecognized;
        };
        let operation_call = segments[start + 1..]
            .iter()
            .find_map(|segment| match segment {
                Segment::Call(call) => Some(*call),
                Segment::Property(_) => None,
            });
        let Some(operation_call) = operation_call else {
            return Classified::NotRecognized;
        };
        let label = self.label(root.start(), operation_call);
        let all_calls = calls(segments);

        if self.config.transaction_methods.iter().any(|m| m == method)
            && matches!(segments.get(start + 1), Some(Segment::Call(_)))
        {
            return Classified::Transaction {
                call: operation_call,
                handle,
                recognition,
                label,
            };
        }
        if method == "rollback"
            && let DbHandle::Transaction { id } = handle
        {
            return Classified::Rollback { transaction: id };
        }

        let (access, table) = if let Some((_, operation)) =
            WRITE_METHODS.iter().find(|(name, _)| *name == method)
        {
            (DbAccess::Write(*operation), first_arg_name(operation_call))
        } else if SELECT_METHODS.contains(&method) {
            let table = if method == "$count" {
                first_arg_name(operation_call)
            } else {
                method_call(segments, "from").and_then(first_arg_name)
            };
            (DbAccess::Read, table)
        } else if method == "query" {
            let table = match segments.get(start + 1) {
                Some(Segment::Property(table)) => Some((*table).to_owned()),
                _ => None,
            };
            (DbAccess::Read, table)
        } else if method == "execute" {
            return Classified::DbUnknown {
                reason: UnknownReason::RawSql,
                detail: format!("`{label}` executes raw SQL"),
                label,
                // The operation itself (`execute`) is lazy; only later calls count.
                marker: marker(segments.get(start + 2..).unwrap_or_default()),
                calls: all_calls,
            };
        } else {
            return Classified::DbUnknown {
                reason: UnknownReason::UnrecognizedDatabaseMethod,
                detail: format!(
                    "`{receiver}.{method}(...)` is called on a recognized database handle, but `{method}` is not a modeled Drizzle method."
                ),
                label,
                // The operation itself (`execute`) is lazy; only later calls count.
                marker: marker(segments.get(start + 2..).unwrap_or_default()),
                calls: all_calls,
            };
        };

        Classified::Db {
            builder: Builder {
                access,
                table,
                handle,
                recognition,
                label,
            },
            marker: marker(&segments[start..]),
            calls: all_calls,
        }
    }

    fn classify_external<'e, 'a>(
        &self,
        chain: &Chain<'e, 'a>,
        root: Root<'e, 'a>,
        resolved: Option<&Resolved>,
    ) -> Option<Classified<'e, 'a>> {
        let segments = &chain.segments;
        for rule in &self.config.external_effects {
            let (path, method) = match rule.target() {
                Some(ExternalTarget::Function { name }) => (name, None),
                Some(ExternalTarget::Method { receiver, method }) => (receiver, Some(method)),
                None => continue,
            };
            let Some(length) =
                match_path(chain, root, resolved, path, rule.imported_from.as_deref())
            else {
                continue;
            };
            // Expect the method (if any), the effect call, then only promise
            // continuations (`.then`, `.catch`, `.finally`).
            let mut index = length;
            if let Some(method) = method {
                if !matches!(segments.get(index), Some(Segment::Property(name)) if *name == method)
                {
                    continue;
                }
                index += 1;
            }
            let Some(Segment::Call(call)) = segments.get(index) else {
                continue;
            };
            // `firstArgument` narrows the rule to calls whose first argument
            // is that string literal (`triggerJob("notification", ...)`).
            if let Some(expected) = &rule.first_argument {
                let first = call
                    .arguments
                    .first()
                    .and_then(Argument::as_expression)
                    .map(Expression::get_inner_expression);
                let matches = match first {
                    Some(Expression::StringLiteral(literal)) => {
                        literal.value.as_str() == Some(expected.as_str())
                    }
                    Some(Expression::TemplateLiteral(template)) => {
                        template.expressions.is_empty()
                            && template.quasis.first().is_some_and(|quasi| {
                                quasi.value.cooked.as_ref().and_then(|c| c.as_str())
                                    == Some(expected.as_str())
                            })
                    }
                    _ => false,
                };
                if !matches {
                    continue;
                }
            }
            let continuations_only = segments[index + 1..].chunks(2).all(|pair| {
                matches!(pair, [Segment::Property(name), Segment::Call(_)] if PROMISE_CONTINUATIONS.contains(name))
            });
            if !continuations_only {
                continue;
            }
            let receiver = receiver_text(root, &segments[..length]);
            let detail = format!(
                "matches externalEffects entry `{rule}`; {}",
                root_detail(resolved, &receiver, path)
            );
            return Some(Classified::External {
                category: rule.category,
                best_effort: rule.is_best_effort(),
                recovery: rule.effective_recovery(),
                recognition: Recognition {
                    basis: configured_basis(rule.imported_from.is_some(), path),
                    receiver,
                    detail,
                },
                label: self.label(root.start(), call),
                calls: calls(segments),
            });
        }
        None
    }

    /// A short label for the call from offset `start` (the chain's root) to
    /// the end of `call`, taken from the source.
    pub fn label(&self, start: u32, call: &CallExpression<'_>) -> String {
        let full = compact(self.slice(start, call.span.end));
        if full.chars().count() <= 60 {
            return full;
        }
        let callee = compact(self.slice(start, call.callee.span().end));
        let first = call
            .arguments
            .first()
            .map(|arg| compact(self.slice(arg.span().start, arg.span().end)));
        match first {
            Some(first) if first.chars().count() <= 30 => {
                let more = if call.arguments.len() > 1 {
                    ", …"
                } else {
                    ""
                };
                format!("{callee}({first}{more})")
            }
            Some(_) => format!("{callee}(…)"),
            None => format!("{callee}()"),
        }
    }

    /// Label for an arbitrary (unrecognized) call.
    pub fn call_label(&self, call: &CallExpression<'_>) -> String {
        let full = compact(self.slice(call.span.start, call.span.end));
        if full.chars().count() <= 60 {
            return full;
        }
        let callee = truncate(
            &compact(self.slice(call.callee.span().start, call.callee.span().end)),
            50,
        );
        format!("{callee}(…)")
    }

    /// A cheap label naming only the callee (`this.save(…)`): used in
    /// summary explanations, computed for many calls.
    pub fn callee_label(&self, callee: &Expression<'_>) -> String {
        let span = callee.span();
        let text = self.slice(span.start, span.end);
        let text = if text.chars().any(char::is_whitespace) {
            compact(text)
        } else {
            text.to_owned()
        };
        format!("{}(…)", truncate(&text, 50))
    }

    pub fn slice(&self, start: u32, end: u32) -> &str {
        self.source
            .get(start as usize..end as usize)
            .unwrap_or_default()
    }
}

const THIS: &str = "this";

/// A handle recognized by provenance alone, whatever the configuration says:
/// a transaction callback parameter or a `drizzle(...)` instance.
fn provenance_handle(resolved: &Resolved) -> Option<(DbHandle, Recognition)> {
    let (handle, basis) = match &resolved.origin {
        Origin::TransactionHandle(id) => (
            DbHandle::Transaction { id: *id },
            RecognitionBasis::TransactionHandle,
        ),
        Origin::DrizzleInstance { .. } => (
            DbHandle::Root {
                name: resolved.written.clone(),
            },
            RecognitionBasis::DrizzleConstructor,
        ),
        _ => return None,
    };
    Some((
        handle,
        Recognition {
            basis,
            receiver: resolved.written.clone(),
            detail: resolved.describe(),
        },
    ))
}

/// Whether the chain starts with the configured receiver `path` (`db`,
/// `ctx.db`, `this.db`). Returns the number of property segments the path
/// covers after the root.
///
/// A path may end with `()` (`getDb()`, `services.db()`): the receiver is the
/// value a call of that function, without arguments, returns. Such a path
/// also matches a `const` initialized by that call (`const db = getDb()`),
/// covering no segment of the chain.
fn match_path(
    chain: &Chain<'_, '_>,
    root: Root<'_, '_>,
    resolved: Option<&Resolved>,
    path: &str,
    imported_from: Option<&str>,
) -> Option<usize> {
    let (body, call) = match path.strip_suffix("()") {
        Some(body) => (body, true),
        None => (path, false),
    };
    let mut parts = body.split('.');
    let first = parts.next()?;
    let properties: Vec<&str> = parts.collect();
    // `const db = getDb()` for the path `getDb()`.
    if call
        && properties.is_empty()
        && let (Root::Identifier(_), Some(resolved)) = (root, resolved)
        && let Origin::CallResult { callee, import } = &resolved.origin
    {
        let module_ok = imported_from.is_none_or(|module| {
            import
                .as_ref()
                .is_some_and(|import| import.module == module)
        });
        return (callee == first && module_ok).then_some(0);
    }
    let root_matches = match (root, resolved) {
        (Root::This(_), _) => first == THIS,
        (Root::Identifier(_), Some(resolved)) => {
            let origin_ok = match &resolved.origin {
                Origin::Unresolved | Origin::Import(_) | Origin::Local => true,
                Origin::CallResult { .. } => !call,
                // The function a configured call names may be declared here.
                Origin::Contrary(what) => call && what.contains("function"),
                _ => false,
            };
            let module_ok = imported_from.is_none_or(|module| {
                matches!(&resolved.origin, Origin::Import(import) if import.module == module)
            });
            first != THIS && resolved.name == first && origin_ok && module_ok
        }
        (Root::Identifier(_), None) => false,
    };
    let properties_match = properties.iter().enumerate().all(|(index, expected)| {
        matches!(chain.segments.get(index), Some(Segment::Property(name)) if name == expected)
    });
    let call_matches = !call
        || matches!(chain.segments.get(properties.len()), Some(Segment::Call(c)) if c.arguments.is_empty());
    (root_matches && properties_match && call_matches)
        .then_some(properties.len() + usize::from(call))
}

/// The receiver as written: the root and the properties (and call) of its path.
fn receiver_text(root: Root<'_, '_>, segments: &[Segment<'_, '_>]) -> String {
    let mut text = root.written().to_owned();
    for segment in segments {
        match segment {
            Segment::Property(name) => {
                text.push('.');
                text.push_str(name);
            }
            Segment::Call(_) => text.push_str("()"),
        }
    }
    text
}

/// How strong a configured match is: an `importedFrom` check beats a member
/// path, which is weaker than a plain name.
fn configured_basis(imported: bool, path: &str) -> RecognitionBasis {
    if imported {
        RecognitionBasis::ConfiguredImport
    } else if path.trim_end_matches("()").contains('.') {
        RecognitionBasis::ConfiguredMemberPath
    } else {
        RecognitionBasis::ConfiguredName
    }
}

/// How the root of a configured receiver was resolved.
fn root_detail(resolved: Option<&Resolved>, receiver: &str, path: &str) -> String {
    let member = path.trim_end_matches("()").contains('.');
    let call = path.ends_with("()");
    match resolved {
        Some(resolved) if call => format!(
            "{}; the value `{}` returns is trusted to be the database handle",
            resolved.describe(),
            path.trim_end_matches("()")
        ),
        Some(resolved) if !member => resolved.describe(),
        Some(resolved) => format!(
            "{}; what `{receiver}` holds is not checked",
            resolved.describe()
        ),
        None => format!("`this` is not resolved; what `{receiver}` holds is not checked"),
    }
}

/// End offset of an argument expression without wrappers (`tx as Tx`).
fn receiver_chain_end(expression: &Expression<'_>) -> u32 {
    expression.get_inner_expression().span().end
}

/// Start offset of the expression a chain of segments is built on.
fn segments_start(segments: &[Segment<'_, '_>]) -> u32 {
    segments
        .iter()
        .find_map(|segment| match segment {
            Segment::Call(call) => Some(call.span.start),
            Segment::Property(_) => None,
        })
        .unwrap_or_default()
}

/// End offset of the receiver expression ending at `segments[index]`.
fn receiver_end(segments: &[Segment<'_, '_>], index: usize) -> u32 {
    // The callee of the first call after `index` is `receiver.method`;
    // the receiver ends where `.method` begins.
    segments[index..]
        .iter()
        .find_map(|segment| match segment {
            Segment::Call(call) => match &call.callee {
                Expression::StaticMemberExpression(member) => Some(member.object.span().end),
                callee => Some(callee.span().end),
            },
            Segment::Property(_) => None,
        })
        .unwrap_or_default()
}

fn calls<'e, 'a>(segments: &[Segment<'e, 'a>]) -> Vec<&'e CallExpression<'a>> {
    segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::Call(call) => Some(*call),
            Segment::Property(_) => None,
        })
        .collect()
}

/// Method names in call position, in order.
fn methods<'a>(segments: &[Segment<'_, 'a>]) -> Vec<&'a str> {
    segments
        .windows(2)
        .filter_map(|pair| match pair {
            [Segment::Property(name), Segment::Call(_)] => Some(*name),
            _ => None,
        })
        .collect()
}

fn marker(segments: &[Segment<'_, '_>]) -> Marker {
    let methods = methods(segments);
    if methods.iter().any(|m| NON_EXECUTING_METHODS.contains(m)) {
        Marker::Never
    } else if methods
        .last()
        .is_some_and(|m| EXECUTING_METHODS.contains(m))
    {
        Marker::Explicit
    } else {
        Marker::Lazy
    }
}

fn method_call<'e, 'a>(segments: &[Segment<'e, 'a>], name: &str) -> Option<&'e CallExpression<'a>> {
    segments.windows(2).find_map(|pair| match pair {
        [Segment::Property(method), Segment::Call(call)] if *method == name => Some(*call),
        _ => None,
    })
}

/// Name of the table argument: `orders` or `schema.orders`.
fn first_arg_name(call: &CallExpression<'_>) -> Option<String> {
    match call.arguments.first()? {
        Argument::SpreadElement(_) => None,
        argument => match argument.to_expression().get_inner_expression() {
            Expression::Identifier(ident) => Some(ident.name.as_str().to_owned()),
            Expression::StaticMemberExpression(member) => {
                Some(member.property.name.as_str().to_owned())
            }
            _ => None,
        },
    }
}

/// Collapse whitespace runs to single spaces, and drop the whitespace a
/// line break leaves inside member chains and brackets
/// (`db\n  .insert(t)` becomes `db.insert(t)`): no space before `.`, `?.`
/// or `)`, nor after `(`. One pass, one allocation (labels are computed for
/// many calls).
pub(crate) fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if !c.is_whitespace() {
            out.push(c);
            continue;
        }
        while chars.peek().is_some_and(|(_, next)| next.is_whitespace()) {
            chars.next();
        }
        let Some(&(next_index, next)) = chars.peek() else {
            break; // trailing whitespace
        };
        let _ = index;
        let starts_optional_chain = next == '?' && text[next_index + 1..].starts_with('.');
        let skip = out.is_empty()
            || out.ends_with('(')
            || next == '.'
            || next == ')'
            || starts_optional_chain;
        if !skip {
            out.push(' ');
        }
    }
    out
}

/// Shorten `text` to at most `max` characters, marking the cut with `…`.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::compact;

    #[test]
    fn compact_rejoins_member_chains() {
        assert_eq!(compact("db\n    .insert(invoices)"), "db.insert(invoices)");
        assert_eq!(
            compact("queue.publish(\n  \"x\",\n  { id }\n)"),
            "queue.publish(\"x\", { id })"
        );
        assert_eq!(compact("a  &&\n b"), "a && b");
        assert_eq!(compact("  a ?\n b : c  "), "a ? b : c");
        assert_eq!(compact("x\n  ?.y( z )"), "x?.y(z)");
        assert_eq!(compact("f(\n)"), "f()");
    }
}

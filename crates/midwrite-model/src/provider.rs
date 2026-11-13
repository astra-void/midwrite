//! The boundary between the model checker and the TypeScript frontend.
//!
//! The model crate does not parse TypeScript. A *provider* (implemented in
//! `midwrite-analyzer`) lowers one function of an analyzed project to the
//! concurrency IR, or says precisely why it cannot. Whatever it does not
//! understand must become [`crate::ir::Op::Unknown`] (never a no-op).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ir::Step;
use crate::value::Value;

/// A Drizzle table identifier as written in the source, bound to a table of
/// the scenario's database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableBinding {
    /// The identifier in source, e.g. `products` in `eq(products.id, x)`.
    pub identifier: String,
    /// The table name in the scenario's schema.
    pub table: String,
    /// The columns of the table; property names in source must equal these.
    pub columns: Vec<String>,
    /// The declaration the table was read from (schema module
    /// relative to the project root, binding). When set, an identifier in
    /// source denotes this table only if it resolves (through imports and
    /// re-exports) to this declaration, whatever its local name; a
    /// different declaration with the same name is not this table.
    pub source: Option<(String, String)>,
}

/// A call in source that is an externally visible effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalBinding {
    /// Callee as written: `publishEvent`, `queue.publish`, `stripe.charges.create`.
    pub callee: String,
    pub channel: String,
    /// Index of the argument that is the idempotency key, if any.
    pub key_argument: Option<usize>,
}

/// An application helper that runs a callback in a database
/// transaction and re-runs it in a NEW transaction after a serialization
/// failure or deadlock (SQLSTATE 40001 / 40P01). Declared by the scenario:
/// the helper's own loop is not analyzed; source verification runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionRetryBinding {
    /// Callee as written: `withTransactionRetry`.
    pub callee: String,
    /// Index of the callback argument (receives the transaction handle).
    pub callback_argument: usize,
    /// Index of an options object literal with `attempts` (total number of
    /// executions) and `isolationLevel`.
    pub options_argument: Option<usize>,
    /// Total attempts when the options do not say.
    pub attempts: u32,
}

/// What a scenario *declares* about how a wrapper invokes the selected
/// callback. It is an assumption supplied by the scenario author,
/// reported as such, and never shown as proven source semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationContract {
    /// `awaited` (the wrapper awaits or returns the callback's promise) or
    /// `fire-and-forget`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<String>,
    /// Conditions under which the wrapper invokes the callback
    /// (`authenticated-user`, `validation-succeeded`, ...): free text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<String>,
    /// The selected callback is a tRPC procedure resolver called with
    /// `{ ctx, input }`: what the context holds, as declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trpc: Option<TrpcContext>,
}

/// The declared context of a tRPC procedure (`midwrite-contracts.json`
/// `trpc`): the `ctx` fields that hold a project binding, as
/// `module#export` relative to the project root (`db` →
/// `packages/db/src/client.ts#db`). Other `ctx` fields are values of the
/// request. The resolver's `input` is the procedure's `.input(schema)`
/// parse of the request input, which the extractor reads from source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrpcContext {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub context: std::collections::BTreeMap<String, String>,
}

/// How a wrapper uses a callback, as far as its analyzed source shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WrapperFacts {
    /// The wrapper as called (`validatedActionWithUser`).
    pub wrapper: String,
    /// Where the wrapper is defined (`src/lib/auth/middleware.ts:28:1`), if
    /// its source was analyzed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defined_at: Option<String>,
    /// `returned-function-awaits`, `returned-function-returns-promise`,
    /// `returned-function-fire-and-forget`, `invoked-during-wrap`, `stored`,
    /// `not-invoked` or `unknown`.
    pub invocation: String,
    /// Whether some path of the invoking function exits before the call.
    pub conditional: bool,
    /// The exits that can prevent the call (`if (!user) throw ...`), with
    /// locations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guards: Vec<String>,
    /// What the wrapper passes for each callback parameter, as written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    /// Why the invocation is unknown, or other remarks.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// The selected entry function of an actor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryInfo {
    /// The selector as written in the scenario.
    pub selector: String,
    /// `file@line:column` of the function node: its identity.
    pub anchor: String,
    /// The function's source file and last line (with the anchor line:
    /// the region the model represents).
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub end_line: u32,
    /// The name `midwrite check` reports for this function, for linking
    /// (by position): its own name or `<anonymous@line:column>`.
    pub static_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper: Option<WrapperFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<InvocationContract>,
    /// How the declared contract relates to the source facts:
    /// `not-needed` (direct function), `consistent`, `required` (source
    /// facts are unknown; the contract is the only basis) or `absent`.
    pub contract_status: String,
}

#[derive(Clone, Debug)]
pub struct ExtractRequest {
    /// Project root; `file` is relative to it.
    pub root: PathBuf,
    pub file: String,
    pub function: String,
    /// Concrete values of the function's parameters, in order.
    pub arguments: Vec<Value>,
    pub tables: Vec<TableBinding>,
    pub externals: Vec<ExternalBinding>,
    /// Declared transaction-retry helpers.
    pub transaction_retries: Vec<TransactionRetryBinding>,
    /// How many levels of calls to local/imported helper functions are
    /// inlined.
    pub max_inline_depth: usize,
    /// A source selector (`file#binding/arg[1]`, `file@line:col`) choosing
    /// the entry function instead of `function`.
    pub selector: Option<String>,
    /// Declared invocation assumptions for a wrapped callback.
    pub contract: Option<InvocationContract>,
    /// Model reads through a replicated database (`withReplicas`)
    /// as replica reads ([`crate::ir::Conn::Replica`]). When false such
    /// reads stay unknown operations.
    pub replicas: bool,
    /// Project functions declared to build a replicated database.
    pub replica_factories: Vec<ReplicaFactoryBinding>,
    /// Emit [`crate::ir::Op::ReturnValue`] before the main task's
    /// returns (requested when an invariant reads return values; otherwise
    /// returns do not emit a value operation).
    pub return_values: bool,
    /// Lower with JavaScript value semantics (scenario feature
    /// `js-values`): `undefined` distinct from `null` (missing properties,
    /// out-of-range indices, `?.`, `??`, `== null`), strings (template
    /// literals, `+` with a string operand, string methods), arrays (literals,
    /// indexing, `.length`, `includes`/`indexOf`/`join`, pure-callback
    /// `map`/`filter`/`find`/..., destructuring, `for...of`), `Map` and `Set`
    /// (`docs/concepts/value-semantics.md`). When false, `undefined` is
    /// `null` and these forms stay unknown.
    pub js_values: bool,
}

/// A project function DECLARED (by the scenario or the project's
/// contract file) to return a replicated database with drizzle-orm
/// `withReplicas` routing: `select`/`selectDistinct`/`$count`/`query`/`with`
/// read from a replica; `insert`/`update`/`delete`/`execute`/`transaction`
/// use the primary. The declaration is an assumption, reported as such; the
/// factory's body is not interpreted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplicaFactoryBinding {
    /// `module#export` relative to the project root
    /// (`packages/db/src/replicas.ts#withReplicas`).
    pub factory: String,
    /// Index of the primary database argument.
    #[serde(default)]
    pub primary_argument: usize,
    /// Methods of the result that return a database routing every
    /// operation, reads included, to the primary (`usePrimaryOnly`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub primary_only_methods: Vec<String>,
    /// Properties of the result holding the primary database (`$primary`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub primary_properties: Vec<String>,
    /// Why the author believes the routing holds.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct Extracted {
    /// Parameter names in order (arguments are bound to them as locals).
    pub params: Vec<String>,
    /// Code vectors; index 0 is the main task, others are `Promise.all`
    /// children and fire-and-forget tasks.
    pub codes: Vec<Vec<Step>>,
    /// Assumptions made during extraction (for the report).
    pub assumptions: Vec<String>,
    /// The selected entry (only for selector-based actors).
    pub entry: Option<EntryInfo>,
}

pub trait ActorProvider {
    fn extract(&self, request: &ExtractRequest) -> Result<Extracted, String>;

    /// The declaration (module, binding) the name `local` of
    /// module `file` resolves to through imports and re-exports, if the
    /// frontend can tell.
    fn resolve_reference(
        &self,
        root: &std::path::Path,
        file: &str,
        local: &str,
    ) -> Option<(String, String)> {
        let _ = (root, file, local);
        None
    }

    /// The Drizzle schema facts of `file` (relative to `root`),
    /// read statically.
    fn extract_schema(
        &self,
        root: &std::path::Path,
        file: &str,
    ) -> Result<crate::schema_facts::ExtractedSchema, String> {
        let _ = root;
        Err(format!(
            "no TypeScript frontend is available to read the schema in `{file}`"
        ))
    }
}

/// A provider that refuses every request: used when no project is available.
pub struct NoProvider;

impl ActorProvider for NoProvider {
    fn extract(&self, request: &ExtractRequest) -> Result<Extracted, String> {
        Err(format!(
            "no TypeScript frontend is available to extract `{}` from `{}`",
            request.function, request.file
        ))
    }
}

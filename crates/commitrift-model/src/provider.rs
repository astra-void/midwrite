//! The boundary between the model checker and the TypeScript frontend.
//!
//! The model crate does not parse TypeScript. A *provider* (implemented in
//! `commitrift-analyzer`) lowers one function of an analyzed project to the
//! concurrency IR, or says precisely why it cannot. Whatever it does not
//! understand must become [`crate::ir::Op::Unknown`] (never a no-op).

use std::path::PathBuf;

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
    /// How many levels of calls to local/imported helper functions are
    /// inlined.
    pub max_inline_depth: usize,
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
}

pub trait ActorProvider {
    fn extract(&self, request: &ExtractRequest) -> Result<Extracted, String>;
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

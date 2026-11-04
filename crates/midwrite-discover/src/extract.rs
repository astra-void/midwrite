//! Extraction through the model checker's own loader.
//!
//! Discovery never lowers TypeScript itself. Every entry is extracted with
//! the request the scenario loader would make for a generated scenario
//! (same table bindings, externals, transaction helpers, inline depth and
//! invocation contract), so the operations a candidate is built from are the
//! operations the model checker will explore. Requests are memoized: the
//! extractor does not depend on argument values, so one extraction serves
//! every scenario built later for the same entry.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use midwrite_analyzer::TsActorProvider;
use midwrite_model::provider::{
    ActorProvider, ExternalBinding, ExtractRequest, Extracted, InvocationContract,
    ReplicaFactoryBinding, TableBinding, TransactionRetryBinding,
};
use midwrite_model::schema_facts::ExtractedSchema;

/// The default inline depth of the scenario loader (`extraction.maxInlineDepth`).
pub const INLINE_DEPTH: usize = 3;

/// A provider that memoizes extractions by request (arguments excluded) and
/// schema reads by file.
pub struct MemoProvider {
    inner: TsActorProvider,
    extracted: RefCell<BTreeMap<String, Result<Extracted, String>>>,
    schemas: RefCell<BTreeMap<String, Result<ExtractedSchema, String>>>,
    pub hits: Cell<u64>,
    pub misses: Cell<u64>,
}

impl MemoProvider {
    /// Phase 12: the shared analysis context of the project.
    pub fn context(
        &self,
        root: &Path,
    ) -> std::sync::Arc<midwrite_analyzer::context::ProjectContext> {
        self.inner.context(root)
    }

    pub fn new(root: &Path) -> Self {
        Self {
            inner: TsActorProvider::new(root.to_path_buf()),
            extracted: RefCell::new(BTreeMap::new()),
            schemas: RefCell::new(BTreeMap::new()),
            hits: Cell::new(0),
            misses: Cell::new(0),
        }
    }
}

/// Everything in a request that can change the extracted code. Argument
/// values are bound as locals after extraction and are not part of it.
pub fn request_key(r: &ExtractRequest) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{}\u{1f}{}\u{1f}{:?}",
        r.root.display(),
        r.file,
        r.function,
        r.selector,
        r.contract,
        r.tables,
        r.externals,
        r.transaction_retries,
        r.max_inline_depth,
        r.replicas,
        r.replica_factories
    ) + &format!("\u{1f}{}\u{1f}{}", r.return_values, r.js_values)
}

impl ActorProvider for MemoProvider {
    fn extract(&self, request: &ExtractRequest) -> Result<Extracted, String> {
        let key = request_key(request);
        if let Some(hit) = self.extracted.borrow().get(&key) {
            self.hits.set(self.hits.get() + 1);
            return hit.clone();
        }
        self.misses.set(self.misses.get() + 1);
        let result = self.inner.extract(request);
        self.extracted.borrow_mut().insert(key, result.clone());
        result
    }

    fn resolve_reference(&self, root: &Path, file: &str, local: &str) -> Option<(String, String)> {
        self.inner.resolve_reference(root, file, local)
    }

    fn extract_schema(&self, root: &Path, file: &str) -> Result<ExtractedSchema, String> {
        let key = format!("{}\u{1f}{file}", root.display());
        if let Some(hit) = self.schemas.borrow().get(&key) {
            return hit.clone();
        }
        // Phase 12: through the provider, which resolves default callbacks
        // imported from other project modules.
        let result = self.inner.extract_schema(root, file);
        self.schemas.borrow_mut().insert(key, result.clone());
        result
    }
}

/// What an extraction of one entry needs besides the selector.
#[derive(Clone, Debug, Default)]
pub struct Bindings {
    pub root: PathBuf,
    pub tables: Vec<TableBinding>,
    pub externals: Vec<ExternalBinding>,
    pub transaction_retries: Vec<TransactionRetryBinding>,
    /// Phase 13: extract reads through a replicated database as replica
    /// reads (`Conn::Replica`); set when the project builds a replicated
    /// database (`ProjectIndex::replicated`).
    pub replicas: bool,
    /// Phase 13: declared replica factories (`midwrite-contracts.json`
    /// `replicaFactories`).
    pub replica_factories: Vec<ReplicaFactoryBinding>,
    /// Phase 14: extract with JavaScript value semantics (`undefined`,
    /// arrays, strings, `Map`/`Set`); discovery always does.
    pub js_values: bool,
}

/// Extract the entry `selector` as the scenario loader would, with the
/// declared contract or, without one, an empty contract that lets the
/// extractor report the wrapper's facts instead of refusing (discovery then
/// decides from those facts what a generated scenario must declare).
pub fn extract_entry(
    provider: &dyn ActorProvider,
    bindings: &Bindings,
    selector: &str,
    contract: Option<InvocationContract>,
) -> Result<Extracted, String> {
    let file = selector
        .split(['#', '@'])
        .next()
        .unwrap_or_default()
        .to_owned();
    let request = ExtractRequest {
        root: bindings.root.clone(),
        file,
        function: String::new(),
        arguments: Vec::new(),
        tables: bindings.tables.clone(),
        externals: bindings.externals.clone(),
        transaction_retries: bindings.transaction_retries.clone(),
        max_inline_depth: INLINE_DEPTH,
        selector: Some(selector.to_owned()),
        contract: Some(contract.unwrap_or_default()),
        replicas: bindings.replicas,
        replica_factories: bindings.replica_factories.clone(),
        return_values: false,
        js_values: bindings.js_values,
    };
    let mut extracted = provider.extract(&request)?;
    midwrite_model::scenario::seal_unknowns(&mut extracted.codes);
    Ok(extracted)
}

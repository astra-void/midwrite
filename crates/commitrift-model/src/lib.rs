//! CommitRift Phase 6: a bounded explicit-state model checker for
//! concurrency, PostgreSQL transaction isolation, crashes and retries.
//!
//! See `docs/phase6/` for the semantics, what is modeled and what is not.

pub mod db;
pub mod dsl;
pub mod engine;
pub mod exec;
pub mod explore;
pub mod footprint;
pub mod html;
pub mod invariant;
pub mod ir;
pub mod model;
#[cfg(feature = "postgres")]
pub mod oracle;
pub mod oracle_types;
pub mod provider;
pub mod report;
pub mod scenario;
pub mod state;
pub mod value;
pub mod witness;

//! CommitRift Phase 11: automatic scenario synthesis.
//!
//! From the original TypeScript of a project, discovery builds an entry and
//! effect index, finds pairs of effects that may interact under concurrent
//! execution or failures, attaches property candidates whose origin is
//! always stated, synthesizes small initial database states with the model
//! checker's own semantics, and writes scenarios in the existing scenario
//! format. Nothing here executes application code; model checking is a
//! separate, budgeted step (`docs/phase11/`).

pub mod candidate;
pub mod contracts;
pub mod detect;
pub mod domains;
pub mod effects;
pub mod eval;
pub mod extract;
pub mod pipeline;
pub mod project;
pub mod report;
pub mod synth;
pub mod worlds;

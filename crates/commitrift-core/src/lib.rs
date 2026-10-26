//! CommitRift core: the language-independent half of the analyzer.
//!
//! * [`absint`]: the bounded abstract domains of the value layer (values,
//!   heap, environments, effect summaries, effect relationships).
//! * [`ir`]: the Effect IR that frontends lower source code into.
//! * [`analysis`]: failure-path analysis over the IR.
//! * [`diagnostic`]: diagnostics and the stable code catalog.
//! * [`config`]: user configuration describing which calls are effects.
//! * [`recovery`]: recovery contracts the configuration asserts for whole
//!   functions.
//! * [`report`]: the scan result, also the JSON output format.
//! * [`trace`]: the Failure Trace IR, one candidate path behind a finding,
//!   with its failure-injection boundary.
//! * [`verification`]: verification levels, from static candidate to
//!   runtime-reproduced.
//!
//! This crate has no dependency on any parser; see `commitrift-analyzer` for
//! the TypeScript frontend.

pub mod absint;
pub mod analysis;
pub mod config;
pub mod diagnostic;
pub mod fasthash;
pub mod ir;
pub mod location;
pub mod recovery;
pub mod report;
pub mod trace;
pub mod verification;
